use core::fmt::Write;
use core::{net::Ipv4Addr, str::FromStr};
use defmt::{info, warn};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use esp_hal::rng::Rng;

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
