#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, timer::timg::TimerGroup};
use log::info;

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
#[cfg(feature = "display-ssd1306")]
async fn display_things(hardware: gateway_board::display::GatewayDisplayHardware) -> ! {
    gateway_board::display::display_demo(hardware).await
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
    #[cfg(feature = "display-ssd1306")]
    spawner.must_spawn(display_things(
        gateway_board::display::GatewayDisplayHardware {
            i2c: peripherals.I2C0,
            vext: peripherals.GPIO36,
            sda: peripherals.GPIO17,
            scl: peripherals.GPIO18,
            rst: peripherals.GPIO21,
        },
    ));
}
