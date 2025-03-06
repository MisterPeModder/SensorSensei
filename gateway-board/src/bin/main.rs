#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::{clock::CpuClock, delay::Delay, main};
use log::info;

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);

    esp_println::logger::init_logger_from_env();

    let delay = Delay::new();
    let mut cycle: u32 = 0;
    loop {
        cycle += 1;
        info!("Hello from Node Board! x{cycle}");
        delay.delay_millis(500);
    }
}
