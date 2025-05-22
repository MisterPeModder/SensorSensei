use core::net::Ipv4Addr;

pub struct Config {
    /// Name of the Wi-Fi network to connect to (optional)
    pub wifi_sta_ssid: Option<&'static str>,
    /// Password for the Wi-Fi network to connect to (optional)
    pub wifi_sta_pass: Option<&'static str>,
    /// Name of the Wi-Fi access point (AP) to create for the configuration dashboard
    pub wifi_ap_ssid: &'static str,
    /// Primary DNS server
    pub dns_server_1: Ipv4Addr,
    /// Secondary DNS server
    pub dns_server_2: Ipv4Addr,
}

pub const CONFIG: Config = Config {
    wifi_sta_ssid: option_env!("WIFI_STA_SSID"),
    wifi_sta_pass: option_env!("WIFI_STA_PASS"),
    wifi_ap_ssid: match option_env!("WIFI_AP_SSID") {
        Some(ssid) => ssid,
        None => "lora-gateway-wifi",
    },
    dns_server_1: Ipv4Addr::new(1, 1, 1, 1), // Cloudflare DNS (main)
    dns_server_2: Ipv4Addr::new(1, 0, 0, 1), // Cloudflare DNS (backup)
};
