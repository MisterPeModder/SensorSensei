#![cfg_attr(not(test), no_std)]

use defmt::Format;
use embassy_futures::yield_now;
use embassy_time::Timer;
use esp_hal::analog::adc::{Adc, AdcChannel, AdcConfig, AdcPin, Attenuation, RegisterAccess};
use esp_hal::gpio::{AnalogPin, Level, Output, OutputConfig};
use esp_hal::peripheral::Peripheral;
use esp_hal::Blocking;

const DUST_SENSOR_VCC: f32 = 5.0; // Vcc of the sensor

pub struct Gp2y1014auHardware<ADCI, PinLed, PinData> {
    pub adci: ADCI,
    pub pin_led: PinLed,
    pub pin_data: PinData,
}

pub struct Gp2y1014au<'d, ADCI, PinData> {
    pin_led: Output<'d>,
    pin_data: AdcPin<PinData, ADCI>,
    adc_reader: Adc<'d, ADCI, Blocking>,
    adc_resolution: f32,
}

#[derive(Format)]
pub enum Error<AdcError> {
    ReadError(AdcError),
}

impl<'d, ADCI, PinData> Gp2y1014au<'d, ADCI, PinData>
where
    ADCI: RegisterAccess,
    PinData: AdcChannel + AnalogPin,
{
    /// Creates a new instance of the Gp2y1014au dust sensor
    pub fn new<PeripheralADCI, PinLed>(
        hardware: Gp2y1014auHardware<PeripheralADCI, PinLed, PinData>,
        adc_resolution: u32,
    ) -> Self
    where
        PeripheralADCI: Peripheral<P = ADCI> + 'd,
        PinLed: Peripheral<P = PinLed> + esp_hal::gpio::OutputPin,
    {
        let pin_led = Output::new(hardware.pin_led, Level::Low, OutputConfig::default());
        let mut adc1_config = AdcConfig::new();
        let pin_data = adc1_config.enable_pin(hardware.pin_data, Attenuation::_0dB);
        let adc_reader = Adc::new(hardware.adci, adc1_config);

        Self {
            pin_led,
            adc_reader,
            pin_data,
            adc_resolution: adc_resolution as f32,
        }
    }

    /// Reads the pin state.
    ///
    /// The error types returned back from this will either be `Error::LedError` or `Error::ReadError`.
    ///
    /// * `Error::ReadError` - Implies the OneShot::read function failed for some reason. `nb::Error::WouldBlock`
    /// is already handled in the code as a loop.
    /// * `Error::LedError` - Implies the pin for the LED was either failed to be set low or high respectively.
    /// This error indicates you should probably discard the result and call the method again.
    pub async fn read(&mut self) -> Result<u16, Error<()>> {
        self.pin_led.set_low();
        Timer::after_millis(280).await;
        let result = loop {
            let read_result = self.adc_reader.read_oneshot(&mut self.pin_data);

            match read_result {
                Ok(word) => break Ok(word),
                Err(nb::Error::Other(failed)) => break Err(Error::ReadError(failed)),
                Err(nb::Error::WouldBlock) => yield_now().await,
            };
        };
        Timer::after_millis(40).await;
        self.pin_led.set_high();

        result
    }

    /// Measures the density of dust in the air.
    ///
    /// This function will call the read function and convert the result to a density value.
    ///
    pub async fn measure(&mut self) -> Result<f32, Error<()>> {
        let analog_value = self.read().await?;
        Ok(self.convert_analog_to_density(analog_value))
    }

    /// Converts the analog value to a density value in mg/m3.
    pub fn convert_analog_to_density(&self, analog_value: u16) -> f32 {
        let voltage: f32 = (analog_value as f32) * (DUST_SENSOR_VCC / self.adc_resolution);
        // linear eqaution taken from http://www.howmuchsnow.com/arduino/airquality/
        if voltage < 0.6 {
            return 0.0;
        }
        let dust_density = 0.17 * voltage - 0.1;
        dust_density
    }
}
