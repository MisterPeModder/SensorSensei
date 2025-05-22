//! Networking-related functionality

use core::net::Ipv4Addr;
use defmt::info;

mod dhcp;
pub mod http;
mod tcp;
mod wifi;

pub use dhcp::GatewayDhcpServer;
pub use wifi::{init_wifi, WifiController, WifiStackRunners};

use crate::net::http::{HttpClientError, HttpMethod};
use embassy_net::{Ipv4Cidr, Stack};
use embedded_io_async::Write;

/// IP Address of the DHCP Gateway when in Wi-Fi AP mode.
pub const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 2, 1);
pub const GATEWAY_SUBNET_MASK: u8 = 24;
pub const GATEWAY_RANGE: Ipv4Cidr = Ipv4Cidr::new(GATEWAY_IP, GATEWAY_SUBNET_MASK);

pub async fn http_client_demo(stack: Stack<'_>) -> Result<(), HttpClientError> {
    use core::fmt::Write;

    let client = http::HttpClient::new(stack);

    info!("http-demo: starting request");
    let mut req = client
        .request(
            HttpMethod::Post,
            "api.sensor.community",
            80u16,
            "/v1/push-sensor-data/",
        )
        .await?;

    info!("http-demo: sending headers");
    req.add("Content-Type", "application/json").await?;
    req.add("User-Agent", "NRZ-2021-134-B4-ESP32/4123/4123")
        .await?;
    req.add("X-Sensor", "esp32-32344").await?;
    req.add("X-Pin", "3").await?;

    const BODY: &[u8] = br#"{"sensordatavalues":[{"value":24,"value_type":"temperature"}]}"#;
    let mut body_len: heapless::String<10> = heapless::String::new();
    write!(&mut body_len, "{}", BODY.len()).unwrap();

    req.add("Content-Length", body_len).await?;

    info!("http-demo: writing body");
    let mut body = req.body().await?;
    body.write_all(BODY).await?;
    let res = body.finish().await?;

    info!("http-demo: done! status={}", res.status());
    Ok(())
}
