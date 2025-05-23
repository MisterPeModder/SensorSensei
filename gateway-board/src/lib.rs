#![no_std]
#![allow(async_fn_in_trait)]
#![allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use protocol::app::v1::SensorValuePoint;

extern crate alloc;

pub mod config;
#[cfg(feature = "display-ssd1306")]
pub mod display;
pub mod export;
#[cfg(feature = "lora")]
pub mod lora;
#[cfg(feature = "wifi")]
pub mod net;

pub const PROTOCOL_VERSION_MAJOR: u8 = 1;
pub const PROTOCOL_VERSION_MINOR: u8 = 0;

pub type ValueChannel =
    embassy_sync::zerocopy_channel::Channel<'static, NoopRawMutex, SensorValuePoint>;
pub type ValueSender =
    embassy_sync::zerocopy_channel::Sender<'static, NoopRawMutex, SensorValuePoint>;
pub type ValueReceiver =
    embassy_sync::zerocopy_channel::Receiver<'static, NoopRawMutex, SensorValuePoint>;
