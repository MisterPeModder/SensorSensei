//! Networking-related functionality

use core::net::Ipv4Addr;

mod dhcp;
pub mod http;
mod tcp;
pub mod wifi;

pub use dhcp::GatewayDhcpServer;
pub use wifi::{init_wifi, WifiController, WifiStackRunners};

use embassy_net::Ipv4Cidr;

/// IP Address of the DHCP Gateway when in Wi-Fi AP mode.
pub const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 2, 1);
pub const GATEWAY_SUBNET_MASK: u8 = 24;
pub const GATEWAY_RANGE: Ipv4Cidr = Ipv4Cidr::new(GATEWAY_IP, GATEWAY_SUBNET_MASK);
