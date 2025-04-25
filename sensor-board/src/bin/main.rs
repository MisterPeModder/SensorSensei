//! This example runs on the Heltec WiFi LoRa ESP32 board, which has a builtin Semtech Sx1276 radio.
//! It demonstrates LORA P2P send functionality.
#![no_std]
#![no_main]

use bmp280_ehal::BMP280;
use defmt::info;
use dust_sensor_gp2y1014au::{Gp2y1014au, Gp2y1014auHardware};
use embassy_executor::Spawner;
use esp_hal::{clock::CpuClock, i2c::master::I2c, time::Rate, timer::timg::TimerGroup};
use esp_println as _;
use sensor_board::lora::{LoraController, LoraHardware};

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
    info!("Starting LoRa P2P TX example");
    // Set up ESP32
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timer_group.timer1);

    let i2c = I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_hz(500000)),
    )
    .unwrap()
    .with_scl(peripherals.GPIO22)
    .with_sda(peripherals.GPIO21)
    .into_async();

    let mut bmp = BMP280::new(i2c).unwrap();
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
        // Read BMP280 sensor
        info!("Pressure: {} Pa", bmp.pressure());
        info!("Temperature: {} C", bmp.temp());

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
