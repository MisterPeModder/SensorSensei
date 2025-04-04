use defmt::{error, info, Format};
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
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
    iv::GenericSx126xInterfaceVariant,
    mod_params::{
        Bandwidth, CodingRate, ModulationParams, PacketParams, RadioError, SpreadingFactor,
    },
    sx126x::{self, Sx1262, Sx126x, TcxoCtrlVoltage},
    LoRa,
};
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
    pub spi_nss: GpioPin<8>,
    pub spi_scl: GpioPin<9>,
    pub spi_mosi: GpioPin<10>,
    pub spi_miso: GpioPin<11>,
    pub reset: GpioPin<12>,
    pub busy: GpioPin<13>,
    pub dio1: GpioPin<14>,
}

type AsyncSpi = esp_hal::spi::master::Spi<'static, Async>;
type HeltecLora32Iv = GenericSx126xInterfaceVariant<Output<'static>, Input<'static>>;
type HeltecLora32Lora = LoRa<
    Sx126x<SpiDevice<'static, NoopRawMutex, AsyncSpi, Output<'static>>, HeltecLora32Iv, Sx1262>,
    Delay,
>;

pub struct LoraController {
    lora: HeltecLora32Lora,
    modulation_params: ModulationParams,
    rx_packet_params: PacketParams,
}

/// One-stop shop for LoRa-related errors
#[derive(Debug, Error, Format)]
pub enum LoraError {
    #[error(transparent)]
    Config(#[from] esp_hal::spi::master::ConfigError),
    #[error("radio error: {0:?}")]
    Radio(RadioError),
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
        // No need to use mutexes or other synchonization
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
        let sx126x_config = sx126x::Config {
            chip: Sx1262,
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
            use_dcdc: false,
            rx_boost: true,
        };

        // Initialize GPIO pins
        let nss = Output::new(hardware.spi_nss, Level::High, OutputConfig::default());
        let reset = Output::new(hardware.reset, Level::Low, OutputConfig::default());
        let busy = Input::new(hardware.busy, InputConfig::default());
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
        let iv: GenericSx126xInterfaceVariant<Output<'static>, Input<'static>> =
            GenericSx126xInterfaceVariant::new(reset, dio1, busy, None, None).unwrap();
        let mut lora: HeltecLora32Lora =
            LoRa::new(Sx126x::new(spi_device, iv, sx126x_config), false, Delay)
                .await
                .unwrap();

        let modulation_params = lora.create_modulation_params(
            LORA_SPREADING_FACTOR,
            LORA_BANDWITH,
            LORA_CODING_RATE,
            LORA_FREQUENCY_IN_HZ,
        )?;

        // Don't ask: I don't know what that is either
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
            rx_packet_params,
        })
    }

    /// Listens for LoRa packets in an infinte loop.
    pub async fn run(&mut self) -> Result<(), LoraError> {
        let mut rx_buffer = [0u8; LORA_RX_BUF_SIZE];

        info!("lora: preparing for receiving");
        self.lora
            .prepare_for_rx(
                lora_phy::RxMode::Continuous,
                &self.modulation_params,
                &self.rx_packet_params,
            )
            .await?;

        info!("lora: indefinitely listeting for packets");
        loop {
            if let Err(e) = self.recv(&mut rx_buffer).await {
                error!("lora: error: {:?}", e);
            }
        }
    }

    async fn recv(&mut self, rx_buffer: &mut [u8; LORA_RX_BUF_SIZE]) -> Result<(), LoraError> {
        *rx_buffer = [0u8; LORA_RX_BUF_SIZE];
        match self.lora.rx(&self.rx_packet_params, rx_buffer).await {
            Ok((received_len, _rx_pkt_status)) => {
                match core::str::from_utf8(&rx_buffer[..received_len as usize]) {
                    Ok(received_str) => info!("lora: recieved \"{}\"", received_str),
                    Err(_) => info!("lora: recieved binary payload (len={})", received_len),
                }
            }
            Err(err) => info!("lora: rx unsuccessful = {}", err),
        }

        Ok(())
    }
}
