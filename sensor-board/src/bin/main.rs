#![no_std]
#![no_main]

use bmp280_ehal::BMP280;
use defmt::info;
use dust_sensor_gp2y1014au::{Gp2y1014au, Gp2y1014auHardware};
use embassy_executor::Spawner;
use esp_hal::gpio::GpioPin;
use esp_hal::peripherals::{ADC2, I2C0};
use esp_hal::{clock::CpuClock, i2c::master::I2c, time::Rate, timer::timg::TimerGroup};
use esp_println as _;
use heapless::spsc::{Consumer, Producer, Queue};
use protocol::app::v1::SensorValue;
use sensor_board::comm::app::{VALUES_MEASURE_INTERVAL, VALUES_QUEUE_SIZE};
use sensor_board::lora::{LoraController, LoraHardware};

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // Set up ESP32
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timer_group.timer1);

    let lora = LoraController::new(LoraHardware {
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

    let values_queue: &'static mut Queue<SensorValue, VALUES_QUEUE_SIZE> = {
        static mut Q: Queue<SensorValue, VALUES_QUEUE_SIZE> = Queue::new();
        // SAFETY:
        // This looks very janky, but it is the recommended way to create a queue in the `heapless` docs
        // It is not undefined behavior because this is the *only* possible reference to this global variable.
        #[allow(static_mut_refs)]
        unsafe {
            &mut Q
        }
    };

    let (producer, consumer) = values_queue.split();

    spawner.must_spawn(take_measurements(
        producer,
        peripherals.I2C0,
        peripherals.GPIO22,
        peripherals.GPIO21,
        peripherals.ADC2,
        peripherals.GPIO13,
        peripherals.GPIO4,
    ));
    spawner.must_spawn(communicate(lora, consumer));
}

#[embassy_executor::task]
async fn take_measurements(
    mut producer: Producer<'static, SensorValue, VALUES_QUEUE_SIZE>,
    i2c: I2C0,
    scl: GpioPin<22>,
    sda: GpioPin<21>,
    adci: ADC2,
    dust_led: GpioPin<13>,
    dust_data: GpioPin<4>,
) -> ! {
    let i2c = I2c::new(
        i2c,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_hz(500000)),
    )
    .unwrap()
    .with_scl(scl)
    .with_sda(sda)
    .into_async();

    let mut bmp = BMP280::new(i2c).unwrap();

    info!("ID of BMP chip, {}", bmp.id());

    let mut dust_sensor = Gp2y1014au::new(
        Gp2y1014auHardware {
            adci,
            pin_led: dust_led,
            pin_data: dust_data,
        },
        1024,
    );

    loop {
        info!("Taking measurements...");
        match dust_sensor.read().await {
            Ok(value) => {
                let density = dust_sensor.convert_analog_to_density(value);
                info!("Measured dust density: {}mg/m3", density);
                _ = producer.enqueue(SensorValue::AirQuality(density));
            }
            Err(e) => {
                info!("Error reading sensor: {:?}", e);
            }
        }
        // Read BMP280 sensor
        let pressure = bmp.pressure_one_shot() as f32;
        let temperature = bmp.temp_one_shot() as f32;
        info!("Measured pressure: {}Pa", pressure);
        info!("Measured temperature: {}°C", temperature);
        _ = producer.enqueue(SensorValue::Pressure(pressure));
        _ = producer.enqueue(SensorValue::Temperature(temperature));

        // sleep
        embassy_time::Timer::after(embassy_time::Duration::from_secs(VALUES_MEASURE_INTERVAL))
            .await;
    }
}

#[embassy_executor::task]
async fn communicate(
    lora: LoraController,
    consumer: Consumer<'static, SensorValue, VALUES_QUEUE_SIZE>,
) -> ! {
    sensor_board::comm::app::run(lora, consumer).await;
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    defmt::info!("Panic: {}", info);
    loop {}
}
