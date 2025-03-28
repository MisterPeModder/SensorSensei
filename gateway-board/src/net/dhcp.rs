use core::net::{Ipv4Addr, SocketAddrV4};

use edge_dhcp::{
    io::DEFAULT_SERVER_PORT,
    server::{Server, ServerOptions},
};
use edge_nal::UdpBind;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_net::Stack;
use embassy_time::{Duration, Timer};

use super::GATEWAY_IP;

const UDP_POOL_SIZE: usize = 3;
const UDP_RXTX_BUF_SIZE: usize = 1024;
const UDP_PACKET_META_SIZE: usize = 10;
const DHCP_MAX_LEASES: usize = 64;
const DHCP_BUF_SIZE: usize = 1500;

pub struct GatewayDhcpServer {
    stack: Stack<'static>,
    buf: [u8; DHCP_BUF_SIZE],
    udp_bufs: UdpBuffers<UDP_POOL_SIZE, UDP_RXTX_BUF_SIZE, UDP_RXTX_BUF_SIZE, UDP_PACKET_META_SIZE>,
}

impl GatewayDhcpServer {
    pub fn new(stack: Stack<'static>) -> Self {
        Self {
            stack,
            buf: [0u8; DHCP_BUF_SIZE],
            udp_bufs: UdpBuffers::new(),
        }
    }

    pub async fn run(&mut self) -> ! {
        let unbound_socket = Udp::new(self.stack, &self.udp_bufs);
        let mut bound_socket = unbound_socket
            .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                DEFAULT_SERVER_PORT,
            )))
            .await
            .unwrap();
        let mut gw_buf = [Ipv4Addr::UNSPECIFIED];

        loop {
            _ = edge_dhcp::io::server::run(
                &mut Server::<_, DHCP_MAX_LEASES>::new_with_et(GATEWAY_IP),
                &ServerOptions::new(GATEWAY_IP, Some(&mut gw_buf)),
                &mut bound_socket,
                &mut self.buf,
            )
            .await
            .inspect_err(|e| log::warn!("DHCP server error: {e:?}"));
            Timer::after(Duration::from_millis(500)).await;
        }
    }
}
