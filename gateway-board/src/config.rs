use core::fmt::Write;
use core::mem::MaybeUninit;
use core::{net::Ipv4Addr, str::FromStr};
use defmt::{error, info, warn, Debug2Format};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embedded_storage::{ReadStorage, Storage};
use esp_hal::rng::Rng;
use esp_storage::FlashStorage;
use sha2::{Digest, Sha256};

const CURRENT_CONFIG_VERSION: u8 = 1;
/// Start of the non-volatile storage (NVS) partition
const NVS_PARTITION_OFFSET: u32 = 0x9000;

pub struct EnvVariables {
    pub wifi_sta_ssid: Option<&'static str>,
    pub wifi_sta_pass: Option<&'static str>,
    pub wifi_ap_ssid: Option<&'static str>,
    pub dns_server_1: Option<&'static str>,
    pub dns_server_2: Option<&'static str>,
    pub influx_db_host: Option<&'static str>,
    pub influx_db_port: Option<&'static str>,
    pub influx_db_api_token: Option<&'static str>,
    pub influx_db_org: Option<&'static str>,
    pub influx_db_bucket: Option<&'static str>,
}

#[derive(Clone)]
pub struct InfluxDBConfig {
    /// Host of the InfluxDB instance
    pub host: &'static str,
    /// Port of the InfluxDB instance. Defaults to 8086 if not specified.
    pub port: u16,
    /// Organization name in InfluxDB
    pub org: &'static str,
    /// Bucket name in InfluxDB where data will be written
    pub bucket: &'static str,
    /// API token for authentication with InfluxDB
    pub api_token: &'static str,
}

pub struct Config {
    /// Name of the Wi-Fi network to connect to (optional)
    pub wifi_sta_ssid: Option<heapless::String<32>>,
    /// Password for the Wi-Fi network to connect to (optional)
    pub wifi_sta_pass: Option<heapless::String<64>>,
    /// Name of the Wi-Fi access point (AP) to create for the configuration dashboard
    pub wifi_ap_ssid: heapless::String<32>,
    /// Primary DNS server
    pub dns_server_1: Ipv4Addr,
    /// Secondary DNS server
    pub dns_server_2: Ipv4Addr,
    /// InfluxDB configuration (optional)
    pub influx_db: Option<InfluxDBConfig>,
    /// CSRF token for the configuration dashboard
    pub csrf_token: heapless::String<32>,
}

impl Config {
    pub const fn new() -> Self {
        Self {
            wifi_sta_ssid: None,
            wifi_sta_pass: None,
            wifi_ap_ssid: heapless::String::new(),
            dns_server_1: Ipv4Addr::new(0, 0, 0, 0),
            dns_server_2: Ipv4Addr::new(0, 0, 0, 0),
            influx_db: None,
            csrf_token: heapless::String::new(),
        }
    }

    pub async fn global_init(rng: Rng) {
        let mut guard = CONFIG.lock().await;
        let config: &mut Config = &mut guard;
        config.load_from_env(rng);
        config.load_from_flash();
        config.save_to_flash();
    }

    pub fn load_from_env(&mut self, mut rng: Rng) -> &mut Self {
        info!("Loading configuration from environment variables...");

        self.wifi_sta_ssid = ENVIRONMENT_VARIABLES.wifi_sta_ssid.and_then(|ssid| {
            heapless::String::<32>::from_str(ssid)
                .map(Some)
                .unwrap_or_else(|_| {
                    warn!("WIFI_STA_SSID is too long, using default None");
                    None
                })
        });
        self.wifi_sta_pass = ENVIRONMENT_VARIABLES.wifi_sta_pass.and_then(|ssid| {
            heapless::String::<64>::from_str(ssid)
                .map(Some)
                .unwrap_or_else(|_| {
                    warn!("WIFI_STA_PASS is too long, using default None");
                    None
                })
        });

        let wifi_ap_ssid = ENVIRONMENT_VARIABLES
            .wifi_ap_ssid
            .unwrap_or("lora-gateway-wifi");
        self.wifi_ap_ssid = heapless::String::<32>::from_str(wifi_ap_ssid).unwrap_or_else(|_| {
            warn!("WIFI_AP_SSID is too long, using default 'lora-gateway-wifi'");
            heapless::String::<32>::from_str("lora-gateway-wifi").unwrap()
        });

        self.dns_server_1 = ENVIRONMENT_VARIABLES
            .dns_server_1
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| Ipv4Addr::new(1, 1, 1, 1)); // Cloudflare DNS (main)

        self.dns_server_2 = ENVIRONMENT_VARIABLES
            .dns_server_2
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| Ipv4Addr::new(1, 0, 0, 1)); // Cloudflare DNS (backup)

        // Randomize the CSRF token for security purposes
        self.csrf_token = heapless::String::<32>::new();
        // Generate a random CSRF token from batch of 32bits integers
        info!("Generating CSRF token...");
        for _ in 0..4 {
            // Generate 4 random bytes (32 bits) at a time
            let random_bytes: u32 = rng.random();
            write!(self.csrf_token, "{:08x}", random_bytes).unwrap();
        }

        self.influx_db = match (
            ENVIRONMENT_VARIABLES.influx_db_host,
            ENVIRONMENT_VARIABLES.influx_db_api_token,
            ENVIRONMENT_VARIABLES.influx_db_org,
            ENVIRONMENT_VARIABLES.influx_db_bucket,
        ) {
            (Some(host), Some(api_token), Some(org), Some(bucket)) => {
                info!(
                    "InfluxDB configured to host '{}' with org '{}' and bucket '{}'",
                    host, org, bucket
                );

                Some(InfluxDBConfig {
                    host,
                    port: ENVIRONMENT_VARIABLES
                        .influx_db_port
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(8086),
                    org,
                    bucket,
                    api_token,
                })
            }
            _ => {
                warn!("InfluxDB is not configured (missing some environment variables).");
                None
            }
        };

        info!("Configuration loaded successfully.");
        self
    }

    pub fn save_to_flash(&self) {
        let mut storage = FlashStorage::new();

        let mut config = SerializedConfig {
            header: SerializedConfigHeader {
                version: CURRENT_CONFIG_VERSION,
                checksum: [0; 32], // Placeholder for checksum
            },
            payload: SerializedConfigPayload {
                wifi_sta_ssid: self.wifi_sta_ssid.clone().map(|s| s.into()).into(),
                wifi_sta_pass: self.wifi_sta_pass.clone().map(|s| s.into()).into(),
                wifi_ap_ssid: self.wifi_ap_ssid.clone().into(),
                dns_server_1: self.dns_server_1.into(),
                dns_server_2: self.dns_server_2.into(),
            },
        };

        config.header.checksum = config.payload.checksum();

        let z = unsafe {
            core::slice::from_raw_parts(
                &config as *const SerializedConfig as *const u8,
                size_of::<SerializedConfig>(),
            )
        };

        let res = storage.write(NVS_PARTITION_OFFSET, z);

        if let Err(err) = res {
            error!("config: saving to flash failed: {}", Debug2Format(&err));
        } else {
            info!("config: saved to flash successfully");
        }
    }

    pub fn load_from_flash(&mut self) {
        let mut storage = FlashStorage::new();

        let config = unsafe {
            let mut config: MaybeUninit<SerializedConfig> = MaybeUninit::uninit();

            if let Err(e) = storage.read(
                NVS_PARTITION_OFFSET,
                &mut *config
                    .as_mut_ptr()
                    .cast::<[u8; size_of::<SerializedConfig>()]>(),
            ) {
                error!(
                    "Failed to read configuration from flash: {}",
                    Debug2Format(&e)
                );
                return;
            }

            config.assume_init()
        };

        if config.header.version != CURRENT_CONFIG_VERSION {
            warn!(
                "config: unexpected version in flash {=u8} (expected: {=u8}), skipping load",
                config.header.version, CURRENT_CONFIG_VERSION
            );
            return;
        }

        if config.header.checksum != config.payload.checksum() {
            warn!("config: checksum mismatch in flash, skipping load");
            return;
        }

        let payload = &config.payload;

        if let Ok(wifi_sta_ssid) = payload.wifi_sta_ssid.try_decode() {
            self.wifi_sta_ssid = wifi_sta_ssid;
        }
        if let Ok(wifi_sta_pass) = payload.wifi_sta_pass.try_decode() {
            self.wifi_sta_pass = wifi_sta_pass;
        }
        if let Ok(wifi_ap_ssid) = payload.wifi_ap_ssid.try_into() {
            self.wifi_ap_ssid = wifi_ap_ssid;
        }
        self.dns_server_1 = Ipv4Addr::from_bits(payload.dns_server_1);
        self.dns_server_2 = Ipv4Addr::from_bits(payload.dns_server_2);
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::new()
    }
}

pub const ENVIRONMENT_VARIABLES: EnvVariables = EnvVariables {
    // These are the environment variables that can be set to configure the gateway
    wifi_sta_ssid: option_env!("WIFI_STA_SSID"),
    wifi_sta_pass: option_env!("WIFI_STA_PASS"),
    wifi_ap_ssid: option_env!("WIFI_AP_SSID"),
    dns_server_1: option_env!("DNS_SERVER_1"),
    dns_server_2: option_env!("DNS_SERVER_2"),
    influx_db_host: option_env!("INFLUXDB_HOST"),
    influx_db_port: option_env!("INFLUXDB_PORT"),
    influx_db_api_token: option_env!("INFLUXDB_API_TOKEN"),
    influx_db_org: option_env!("INFLUXDB_ORG"),
    influx_db_bucket: option_env!("INFLUXDB_BUCKET"),
};

pub static CONFIG: Mutex<CriticalSectionRawMutex, Config> = Mutex::new(Config::new());

#[repr(C, align(4))]
struct SerializedConfig {
    header: SerializedConfigHeader,
    payload: SerializedConfigPayload,
}

#[repr(C, align(1))]
struct SerializedConfigHeader {
    version: u8,
    checksum: [u8; 32],
}

#[repr(C, align(1))]
struct SerializedConfigPayload {
    wifi_sta_ssid: SerializedOption<SerializedString<32>>,
    wifi_sta_pass: SerializedOption<SerializedString<64>>,
    wifi_ap_ssid: SerializedString<32>,
    dns_server_1: u32,
    dns_server_2: u32,
}

#[repr(C, align(1))]
#[derive(Clone, Copy)]
struct SerializedString<const N: usize> {
    length: u8,
    data: [u8; N],
}

#[repr(C, align(1))]
#[derive(Clone, Copy)]
struct SerializedOption<T: Copy> {
    is_some: u8, // 0 for None, otherwise for Some
    value: MaybeUninit<T>,
}

impl SerializedConfigPayload {
    /// Computes the SHA-256 checksum of a payload.
    fn checksum(&self) -> [u8; 32] {
        // SAFETY: the payload *must* be fully initialized and contain *no* padding bytes at all.
        unsafe {
            let bytes: &[u8] = &*(self as *const Self).cast::<[u8; size_of::<Self>()]>();
            Sha256::digest(bytes).into()
        }
    }
}

impl<const N: usize> From<heapless::String<N>> for SerializedString<N> {
    fn from(val: heapless::String<N>) -> Self {
        let length = val.len().min(u8::MAX as usize) as u8;
        let mut data = [0u8; N];
        data[..length as usize].copy_from_slice(val.as_bytes());
        SerializedString { length, data }
    }
}

impl<const N: usize> TryFrom<SerializedString<N>> for heapless::String<N> {
    type Error = ();

    fn try_from(value: SerializedString<N>) -> Result<Self, Self::Error> {
        if value.length as usize > N {
            return Err(());
        }
        let mut string = heapless::String::<N>::new();
        string
            .push_str(core::str::from_utf8(&value.data[..value.length as usize]).map_err(|_| ())?)
            .map_err(|_| ())?;
        Ok(string)
    }
}

impl<const N: usize> SerializedOption<SerializedString<N>> {
    pub fn try_decode(self) -> Result<Option<heapless::String<N>>, ()> {
        Option::<SerializedString<N>>::from(self)
            .map(|s| s.try_into())
            .transpose()
    }
}

impl<T: Copy> From<Option<T>> for SerializedOption<T> {
    fn from(val: Option<T>) -> Self {
        match val {
            Some(value) => SerializedOption {
                is_some: 1,
                value: MaybeUninit::new(value),
            },
            None => SerializedOption {
                is_some: 0,
                value: MaybeUninit::uninit(),
            },
        }
    }
}

impl<T: Copy> From<SerializedOption<T>> for Option<T> {
    fn from(value: SerializedOption<T>) -> Option<T> {
        if value.is_some == 0 {
            None
        } else {
            Some(unsafe { value.value.assume_init() })
        }
    }
}
