#![no_std]

extern crate alloc;

#[cfg(feature = "display-ssd1306")]
pub mod display;
mod export;
#[cfg(feature = "lora")]
pub mod lora;
#[cfg(feature = "wifi")]
pub mod net;

pub const PROTOCOL_VERSION_MAJOR: u8 = 1;
pub const PROTOCOL_VERSION_MINOR: u8 = 0;
