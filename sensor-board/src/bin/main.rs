//! This example runs on the Heltec WiFi LoRa ESP32 board, which has a builtin Semtech Sx1276 radio.
//! It demonstrates LORA P2P send functionality.
#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use esp_hal::{clock::CpuClock, timer::timg::TimerGroup};
use esp_println as _;
use sensor_board::lora::{LoraController, LoraHardware};

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
    info!("Starting LoRa P2P TX example");
    // Set up ESP32
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timer_group.timer1);

    let mut lora = LoraController::new(LoraHardware {
        spi: peripherals.SPI2,
        spi_nss: peripherals.GPIO18,
        spi_scl: peripherals.GPIO5,
        spi_mosi: peripherals.GPIO27,
        spi_miso: peripherals.GPIO19,
        reset: peripherals.GPIO23,
        dio1: peripherals.GPIO26,
    })
    .await
    .unwrap();
    let buffer = b"world";

    loop {
        // for i in 0..10 {
        info!("Sending packet {}", buffer);
        lora.send(buffer).await.unwrap();
        // sleep
        info!("Sleeping for 1 second");
        embassy_time::Timer::after(embassy_time::Duration::from_secs(1)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    defmt::info!("Panic: {}", info);
    loop {}
}
