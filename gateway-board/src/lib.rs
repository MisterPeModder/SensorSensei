#![no_std]

#[cfg(feature = "display-ssd1306")]
pub mod display;
#[cfg(feature = "lora")]
pub mod lora;
#[cfg(feature = "wifi")]
pub mod net;

pub const PROTOCOL_VERSION_MAJOR: u8 = 1;
pub const PROTOCOL_VERSION_MINOR: u8 = 0;
