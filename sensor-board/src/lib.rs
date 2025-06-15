#![no_std]

#[cfg(feature = "lora")]
pub mod comm;
#[cfg(feature = "lora")]
pub mod lora;

pub const PROTOCOL_VERSION_MAJOR: u8 = 1;
pub const PROTOCOL_VERSION_MINOR: u8 = 0;
