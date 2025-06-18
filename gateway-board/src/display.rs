use core::{
    fmt::Write,
    ops::{Deref, DerefMut},
};

use defmt::info;
use display_interface::DisplayError;
use embassy_time::{Duration, Ticker, Timer};
use esp_hal::{
    gpio::{GpioPin, Level, Output, OutputConfig},
    i2c::master::{ConfigError, I2c},
    peripherals::I2C0,
    time::Rate,
    Async,
};
use ssd1306::{
    mode::{DisplayConfig, TerminalMode, TerminalModeError},
    prelude::{Brightness, DisplayRotation, I2CInterface},
    size::DisplaySize128x64,
    I2CDisplayInterface, Ssd1306,
};
use thiserror::Error;

type HeltecLora32Display =
    Ssd1306<I2CInterface<I2c<'static, Async>>, DisplaySize128x64, TerminalMode>;

/// Wraps the SSD1306 API for ease of use.
pub struct GatewayDisplay(HeltecLora32Display);

pub struct GatewayDisplayHardware {
    pub i2c: I2C0,
    pub vext: GpioPin<36>,
    pub sda: GpioPin<17>,
    pub scl: GpioPin<18>,
    pub rst: GpioPin<21>,
}

impl GatewayDisplay {
    pub async fn new(
        hardware: GatewayDisplayHardware,
    ) -> Result<GatewayDisplay, GatewayDisplayError> {
        info!("initializing display...");

        // init power
        let mut vext = Output::new(hardware.vext, Level::Low, OutputConfig::default());
        vext.set_low();

        // init screen
        let mut rst = Output::new(hardware.rst, Level::High, OutputConfig::default());
        Timer::after_millis(1).await;
        rst.set_low();
        Timer::after_millis(1).await;
        rst.set_high();

        // The I2C bus used by the screen is exclusive to it.
        // No need to use mutexes or other synchronization
        let i2c: I2c<'static, Async> = I2c::new(
            hardware.i2c,
            esp_hal::i2c::master::Config::default().with_frequency(Rate::from_hz(500000)),
        )?
        .with_scl(hardware.scl)
        .with_sda(hardware.sda)
        .into_async();

        let interface = I2CDisplayInterface::new(i2c);
        let mut inner_display =
            Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
                .into_terminal_mode();
        inner_display.init()?;

        Ok(GatewayDisplay(inner_display))
    }
}

impl Deref for GatewayDisplay {
    type Target = HeltecLora32Display;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for GatewayDisplay {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl core::fmt::Write for GatewayDisplay {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Regular ::core::fmt::Write impl for Ssd1306 type is bugged:
        // It only prints the last character, hence this custom wrapper and impl.
        for c in s.chars() {
            let _ = self.0.print_char(c);
        }
        Ok(())
    }
}

/// Catch-all error for anything wrong that might happen when using the display.
#[derive(Debug, Error)]
pub enum GatewayDisplayError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("display error")]
    Display(DisplayError),
    #[error("terminal mode error {0:?}")]
    TerminalMode(TerminalModeError),
    #[error(transparent)]
    FormatError(#[from] core::fmt::Error),
}

impl From<DisplayError> for GatewayDisplayError {
    fn from(value: DisplayError) -> Self {
        GatewayDisplayError::Display(value)
    }
}

impl From<TerminalModeError> for GatewayDisplayError {
    fn from(value: TerminalModeError) -> Self {
        GatewayDisplayError::TerminalMode(value)
    }
}

pub async fn run_display(hardware: GatewayDisplayHardware) -> ! {
    let mut display = GatewayDisplay::new(hardware)
        .await
        .expect("failed to initialize display");

    do_display(&mut display).await.expect("do_display failure");

    unreachable!()
}

async fn do_display(display: &mut GatewayDisplay) -> Result<(), GatewayDisplayError> {
    display.set_display_on(true)?;

    display.clear()?;
    display.set_brightness(Brightness::NORMAL)?;
    display.set_mirror(false)?;

    #[cfg(any(feature = "wifi", feature = "lora"))]
    {
        let mut ticker = Ticker::every(Duration::from_millis(2000));

        display.set_position(0, 0)?;
        write!(display, "-== Gateway  ==-")?;

        loop {
            #[cfg(feature = "wifi")]
            {
                draw_wifi_page(display).await?;
                ticker.next().await;
                draw_http_page(display).await?;
                ticker.next().await;
            }

            #[cfg(feature = "lora")]
            {
                draw_lora_page(display).await?;
                ticker.next().await;
            }
        }
    }

    #[cfg(not(any(feature = "wifi", feature = "lora")))]
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[cfg(feature = "wifi")]
async fn draw_http_page(display: &mut GatewayDisplay) -> Result<(), GatewayDisplayError> {
    use core::net::Ipv4Addr;

    display.set_position(0, 2)?;
    write!(display, "* HTTP ")?;

    // try to read HTTP server status without blocking
    let address: Option<(Ipv4Addr, u16)> = {
        crate::net::http::CURRENT_STATUS
            .try_lock()
            .ok()
            .and_then(|status| status.address)
        // force lock guard to drop after this
    };

    display.set_position(0, 3)?;
    match address {
        Some((address, port)) => write!(display, "{address:<16}\nport: {port:<10}")?,
        None => write!(display, "(no address yet)\n                ")?,
    }

    Ok(())
}

#[cfg(feature = "wifi")]
async fn draw_wifi_page(display: &mut GatewayDisplay) -> Result<(), GatewayDisplayError> {
    use crate::net::wifi::StackStatus;

    display.set_position(0, 2)?;
    write!(display, "* Wi-Fi")?;

    let (ap_status, sta_status) = {
        crate::net::wifi::CURRENT_STATUS
            .try_lock()
            .map(|status| (status.ap_status, status.sta_status))
            .unwrap_or((StackStatus::Initializing, StackStatus::Initializing))
        // force lock guard to drop after this
    };

    display.set_position(0, 3)?;
    write!(
        display,
        "AP: {:<12}\nSTA: {:<11}",
        ap_status.as_ref(),
        sta_status.as_ref()
    )?;

    Ok(())
}

#[cfg(feature = "lora")]
async fn draw_lora_page(display: &mut GatewayDisplay) -> Result<(), GatewayDisplayError> {
    use crate::comm::app::AppLayerPhase;

    display.set_position(0, 2)?;
    write!(display, "* LoRa")?;

    let phase = {
        crate::comm::app::CURRENT_STATUS
            .try_lock()
            .map(|status| status.phase)
            .unwrap_or(AppLayerPhase::Initial)
        // force lock guard to drop after this
    };

    display.set_position(0, 3)?;
    match phase {
        AppLayerPhase::Initial => write!(display, "waiting...      \n                ")?,
        AppLayerPhase::Handshake => write!(display, "handshaking...  \n                ")?,
        AppLayerPhase::Uplink => write!(display, "connected       \n                ")?,
    }

    Ok(())
}
