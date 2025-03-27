#![no_std]
#![no_main]

use core::fmt::Write;

use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, gpio::AnyPin, i2c::master::AnyI2c, timer::timg::TimerGroup};
use gateway_board::display::{GatewayDisplay, GatewayDisplayError};
use log::info;
use ssd1306::prelude::*;

#[embassy_executor::task]
async fn print_hello() {
    let mut ticker = Ticker::every(Duration::from_millis(1_000));
    let mut cycle: u32 = 0;

    loop {
        cycle += 1;
        info!("Hello from Gateway Board! x{cycle}");
        ticker.next().await;
    }
}

#[embassy_executor::task]
async fn display_things(i2c: AnyI2c, sda: AnyPin, scl: AnyPin) {
    info!("initializing display...");

    let mut display = GatewayDisplay::new(i2c, sda, scl)
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
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_println::logger::init_logger_from_env();

    // wokwi: needed so that the console output is formatted correctly
    esp_println::print!("\x1b[20h");

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    spawner.must_spawn(print_hello());
    spawner.must_spawn(display_things(
        peripherals.I2C0.into(),
        peripherals.GPIO17.into(),
        peripherals.GPIO18.into(),
    ));
}
