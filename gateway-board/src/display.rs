use core::{
    fmt::Write,
    ops::{Deref, DerefMut},
};

use display_interface::DisplayError;
use embassy_time::{Duration, Ticker, Timer};
use esp_hal::{
    gpio::{GpioPin, Level, Output, OutputConfig},
    i2c::master::{ConfigError, I2c},
    peripherals::I2C0,
    time::Rate,
    Async,
};
use log::info;
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
        // No need to use mutexes or other synchonization
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

pub async fn display_demo(hardware: GatewayDisplayHardware) -> ! {
    let mut display = GatewayDisplay::new(hardware)
        .await
        .expect("failed to initialize display");

    async fn do_display(display: &mut GatewayDisplay) -> Result<(), GatewayDisplayError> {
        display.set_display_on(true)?;

        display.clear()?;
        display.set_brightness(Brightness::BRIGHTEST)?;
        display.set_mirror(false)?;

        writeln!(display, "Hello, World!")?;

        let mut ticker = Ticker::every(Duration::from_millis(100));
        let mut counter = 0u32;

        loop {
            display.set_position(0, 2)?;
            write!(display, "{}.{}", counter / 10, counter % 10)?;
            counter += 1;
            ticker.next().await;
        }
    }

    do_display(&mut display).await.expect("do_display failure");

    unreachable!()
}
