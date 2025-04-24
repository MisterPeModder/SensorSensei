//! This example runs on the Heltec WiFi LoRa ESP32 board, which has a builtin Semtech Sx1276 radio.
//! It demonstrates LORA P2P send functionality.
#![no_std]
#![no_main]

use defmt::info;
use dust_sensor_gp2y1014au::{Gp2y1014au, Gp2y1014auHardware};
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

    let mut dust_sensor = Gp2y1014au::new(
        Gp2y1014auHardware {
            adci: peripherals.ADC2,
            pin_led: peripherals.GPIO13,
            pin_data: peripherals.GPIO4,
        },
        1024,
    );

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
        info!("Sending packet {}", buffer);
        lora.send(buffer).await.unwrap();

        match dust_sensor.read().await {
            Ok(value) => {
                let density = dust_sensor.convert_analog_to_density(value);
                info!("Sensor value: {} mg/m3 (raw: {})", density, value);
            }
            Err(e) => {
                info!("Error reading sensor: {:?}", e);
            }
        }

        // sleep
        info!("Sleeping for 2 seconds");
        embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    defmt::info!("Panic: {}", info);
    loop {}
}
