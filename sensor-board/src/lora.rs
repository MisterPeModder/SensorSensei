use defmt::{error, trace, Format};
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_futures::select::Either;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use embassy_time::Delay;
use esp_hal::{
    gpio::{GpioPin, Input, InputConfig, Level, Output, OutputConfig},
    peripherals::SPI2,
    spi::Mode,
    time::Rate,
    Async,
};
use lora_phy::{
    iv::GenericSx127xInterfaceVariant,
    mod_params::{
        Bandwidth, CodingRate, ModulationParams, PacketParams, RadioError, SpreadingFactor,
    },
    sx127x::{self, Sx1276, Sx127x},
    LoRa,
};
use protocol::phy::PhysicalLayer;
use static_cell::StaticCell;
use thiserror::Error;

/// Channel to use, should be "unique". Use same frequencies as other devices causes spurious packets.
const LORA_FREQUENCY_IN_HZ: u32 = 868_200_000;
/// Channel width. Lower values increase time on air, but may be able to find clear frequencies.
const LORA_BANDWITH: Bandwidth = Bandwidth::_250KHz;
/// Controls the forward error correction. Higher values are more robust, but reduces the ratio
/// of actual data in transmissions.
const LORA_CODING_RATE: CodingRate = CodingRate::_4_8;
/// Controls the chirp rate. Lower values are slower bandwidth (longer time on air), but more robust.
const LORA_SPREADING_FACTOR: SpreadingFactor = SpreadingFactor::_10;
const LORA_RX_BUF_SIZE: usize = 128;

pub struct LoraHardware {
    pub spi: SPI2,
    pub spi_nss: GpioPin<18>,
    pub spi_scl: GpioPin<5>,
    pub spi_mosi: GpioPin<27>,
    pub spi_miso: GpioPin<19>,
    pub reset: GpioPin<23>,
    pub dio1: GpioPin<26>,
}

type AsyncSpi = esp_hal::spi::master::Spi<'static, Async>;
type TBeamLora32Iv = GenericSx127xInterfaceVariant<Output<'static>, Input<'static>>;
type TBeamLora32Lora = LoRa<
    Sx127x<SpiDevice<'static, NoopRawMutex, AsyncSpi, Output<'static>>, TBeamLora32Iv, Sx1276>,
    Delay,
>;

pub struct LoraController {
    lora: TBeamLora32Lora,
    modulation_params: ModulationParams,
    tx_packet_params: PacketParams,
    rx_packet_params: PacketParams,
    rx_buffer: heapless::Vec<u8, LORA_RX_BUF_SIZE>,
    tx_buffer: heapless::Vec<u8, LORA_RX_BUF_SIZE>,
}

/// One-stop shop for LoRa-related errors
#[derive(Debug, Error, Format)]
pub enum LoraError {
    #[error(transparent)]
    Config(#[from] esp_hal::spi::master::ConfigError),
    #[error("radio error: {0:?}")]
    Radio(RadioError),
    #[error("buffer overflow")]
    BufferOverflow,
}

impl From<RadioError> for LoraError {
    fn from(value: RadioError) -> Self {
        LoraError::Radio(value)
    }
}

static SPI_BUS: StaticCell<Mutex<NoopRawMutex, AsyncSpi>> = StaticCell::new();

impl LoraController {
    pub async fn new(hardware: LoraHardware) -> Result<Self, LoraError> {
        // The SPI bus used by the lora dio is exclusive to it.
        // No need to use mutexes or other synchronization
        let spi: AsyncSpi = esp_hal::spi::master::Spi::new(
            hardware.spi,
            esp_hal::spi::master::Config::default()
                .with_frequency(Rate::from_khz(100))
                .with_mode(Mode::_0),
        )?
        .with_sck(hardware.spi_scl)
        .with_mosi(hardware.spi_mosi)
        .with_miso(hardware.spi_miso)
        .into_async();

        // Create the SX126x configuration
        let sx127x_config = sx127x::Config {
            chip: Sx1276,
            tcxo_used: false,
            tx_boost: true,
            rx_boost: true,
        };

        // Initialize GPIO pins
        let nss = Output::new(hardware.spi_nss, Level::High, OutputConfig::default());
        let reset = Output::new(hardware.reset, Level::Low, OutputConfig::default());
        let dio1 = Input::new(hardware.dio1, InputConfig::default());

        // Initialize the SPI bus
        let spi_bus: &mut Mutex<NoopRawMutex, AsyncSpi> = SPI_BUS.init_with(|| Mutex::new(spi));
        let spi_device: SpiDevice<
            '_,
            NoopRawMutex,
            esp_hal::spi::master::Spi<'_, Async>,
            Output<'static>,
        > = SpiDevice::new(spi_bus, nss);

        // Create the radio instance
        let iv: GenericSx127xInterfaceVariant<Output<'static>, Input<'static>> =
            GenericSx127xInterfaceVariant::new(reset, dio1, None, None)?;
        let mut lora: TBeamLora32Lora =
            LoRa::new(Sx127x::new(spi_device, iv, sx127x_config), false, Delay).await?;

        let modulation_params = lora.create_modulation_params(
            LORA_SPREADING_FACTOR,
            LORA_BANDWITH,
            LORA_CODING_RATE,
            LORA_FREQUENCY_IN_HZ,
        )?;

        // Don't ask: I don't know what that is either
        let tx_packet_params =
            lora.create_tx_packet_params(4, false, true, false, &modulation_params)?;

        let rx_packet_params = lora.create_rx_packet_params(
            4,
            false,
            LORA_RX_BUF_SIZE as u8,
            true,
            false,
            &modulation_params,
        )?;

        Ok(LoraController {
            lora,
            modulation_params,
            tx_packet_params,
            rx_packet_params,
            rx_buffer: heapless::Vec::new(),
            tx_buffer: heapless::Vec::new(),
        })
    }

    async fn send(&mut self) -> Result<(), LoraError> {
        self.lora
            .prepare_for_tx(
                &self.modulation_params,
                &mut self.tx_packet_params,
                20,
                &self.tx_buffer,
            )
            .await?;

        trace!("phy: sending {=usize} bytes", self.tx_buffer.len());
        self.lora.tx().await?;
        self.tx_buffer.clear();
        trace!("phy: done sending");
        Ok(())
    }

    async fn recv(&mut self) -> Result<(), LoraError> {
        self.lora
            .prepare_for_rx(
                lora_phy::RxMode::Continuous,
                &self.modulation_params,
                &self.rx_packet_params,
            )
            .await?;

        unsafe {
            self.rx_buffer.set_len(LORA_RX_BUF_SIZE);
        }
        trace!("phy: waiting for data (timeout in 5 seconds)");

        let res = embassy_futures::select::select(
            self.lora.rx(&self.rx_packet_params, &mut self.rx_buffer),
            embassy_time::Timer::after(embassy_time::Duration::from_secs(5)),
        )
        .await;

        match res {
            Either::First(x) => {
                let (received_len, rx_pkt_status) = x?;
                unsafe {
                    self.rx_buffer.set_len(received_len as usize);
                }
                trace!(
                    "phy: received packet of length {=usize} (rssi: {=i16}, snr: {=i16})",
                    self.rx_buffer.len(),
                    rx_pkt_status.rssi,
                    rx_pkt_status.snr
                );
            }
            Either::Second(()) => {
                trace!("phy: timeout while waiting for data");
                self.rx_buffer.clear();
            }
        }

        Ok(())
    }

    pub async fn sleep(&mut self, wakeup: bool) -> Result<(), LoraError> {
        Ok(self.lora.sleep(wakeup).await?)
    }
}

impl PhysicalLayer for LoraController {
    type Error = LoraError;

    async fn read(&mut self) -> Result<(), Self::Error> {
        self.rx_buffer.clear();
        self.recv().await?;
        Ok(())
    }

    fn rx_buffer(&self) -> &[u8] {
        &self.rx_buffer
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        self.tx_buffer
            .extend_from_slice(data)
            .map_err(|_| LoraError::BufferOverflow)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        if self.tx_buffer.is_empty() {
            return Ok(());
        }
        self.send().await
    }
}
