use core::ops::{Deref, DerefMut};

use display_interface::DisplayError;
use esp_hal::{
    gpio::AnyPin,
    i2c::master::{AnyI2c, ConfigError, I2c},
    time::Rate,
    Async,
};
use log::info;
use ssd1306::{
    mode::{TerminalMode, TerminalModeError},
    prelude::{DisplayRotation, I2CInterface},
    size::DisplaySize128x64,
    I2CDisplayInterface, Ssd1306,
};
use thiserror::Error;

type HeltecLora32Display =
    Ssd1306<I2CInterface<I2c<'static, Async>>, DisplaySize128x64, TerminalMode>;

/// Wraps the SSD1306 API for ease of use.
pub struct GatewayDisplay(HeltecLora32Display);

impl GatewayDisplay {
    pub async fn new(
        i2c: AnyI2c,
        sda: AnyPin,
        scl: AnyPin,
    ) -> Result<GatewayDisplay, GatewayDisplayError> {
        info!("initializing display...");

        // The I2C bus used by the screen is exclusive to it.
        // No need to use mutexes or other synchonization
        let i2c: I2c<'static, Async> = I2c::new(
            i2c,
            esp_hal::i2c::master::Config::default().with_frequency(Rate::from_hz(500000)),
        )?
        .with_scl(scl)
        .with_sda(sda)
        .into_async();

        let interface = I2CDisplayInterface::new(i2c);
        let inner_display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
            .into_terminal_mode();

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
