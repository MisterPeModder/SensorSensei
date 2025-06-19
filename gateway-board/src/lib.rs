#![no_std]
#![allow(async_fn_in_trait)]
#![allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use protocol::app::v1::SensorValuePoint;

extern crate alloc;

#[cfg(feature = "lora")]
pub mod comm;
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

struct TimeoutError;

trait FutureTimeoutExt: core::future::Future {
    fn with_timeout(
        self,
        timeout: embassy_time::Duration,
    ) -> impl core::future::Future<Output = Result<Self::Output, TimeoutError>>
    where
        Self: Sized;
}

impl<F: core::future::Future> FutureTimeoutExt for F {
    async fn with_timeout(
        self,
        timeout: embassy_time::Duration,
    ) -> Result<Self::Output, TimeoutError> {
        match embassy_futures::select::select(self, embassy_time::Timer::after(timeout)).await {
            embassy_futures::select::Either::First(result) => Ok(result),
            embassy_futures::select::Either::Second(_) => Err(TimeoutError),
        }
    }
}
