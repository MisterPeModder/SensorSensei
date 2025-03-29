use core::net::Ipv4Addr;

use embassy_futures::select::Either;
use embassy_net::{tcp::TcpSocket, IpListenEndpoint, Stack};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use log::{error, info};

use super::tcp::BoxedTcpSocket;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

/// Dummy dual-stack HTTP server.
///
/// Endpoints:
/// - AP mode:  server on the gateway IP
/// - STA mode: server exposed on an IP got from DHCP
pub struct HttpServer<'a> {
    endpoint: IpListenEndpoint,
    ap_socket: BoxedTcpSocket<'a>,
    sta_socket: BoxedTcpSocket<'a>,
    sta_address: Ipv4Addr,
}

impl<'a> HttpServer<'a> {
    pub async fn new(ap_stack: Stack<'a>, sta_stack: Stack<'a>, port: u16) -> Self {
        info!("http: waiting for AP and STA stacks...");

        let sta_address = loop {
            if let Some(config) = sta_stack.config_v4() {
                let address = config.address.address();
                break address;
            }
            Timer::after(Duration::from_millis(500)).await;
        };
        while !(ap_stack.is_link_up() && sta_stack.is_link_up()) {
            Timer::after(Duration::from_millis(500)).await;
        }

        let endpoint = IpListenEndpoint { addr: None, port };
        let mut ap_socket = BoxedTcpSocket::new(ap_stack).expect("ap_socket: alloc failure");
        let mut sta_socket = BoxedTcpSocket::new(sta_stack).expect("sta_socket: alloc failure");

        ap_socket.set_timeout(Some(SOCKET_TIMEOUT));
        sta_socket.set_timeout(Some(SOCKET_TIMEOUT));

        HttpServer {
            endpoint,
            ap_socket,
            sta_socket,
            sta_address,
        }
    }

    pub async fn run(&mut self) -> ! {
        info!(
            "http: server running on port {}, STA address is {}",
            self.endpoint.port, self.sta_address
        );
        loop {
            info!("http: waiting for connection");

            let r = embassy_futures::select::select(
                self.ap_socket.accept(self.endpoint),
                self.sta_socket.accept(self.endpoint),
            )
            .await;

            match r {
                Either::First(Ok(())) => Self::handle_client(&mut self.ap_socket).await,
                Either::Second(Ok(())) => Self::handle_client(&mut self.sta_socket).await,
                Either::First(Err(e)) => error!("http: AP socket error: {e:?}"),
                Either::Second(Err(e)) => error!("http: STA socket error: {e:?}"),
            }
        }
    }

    async fn handle_client(sock: &mut TcpSocket<'a>) {
        let mut buffer = [0u8; 1024];
        let mut pos = 0;
        loop {
            match sock.read(&mut buffer).await {
                Ok(0) => {
                    esp_println::println!("http: read EOF");
                    break;
                }
                Ok(len) => {
                    let to_print =
                        unsafe { core::str::from_utf8_unchecked(&buffer[..(pos + len)]) };

                    if to_print.contains("\r\n\r\n") {
                        esp_println::println!("{}", to_print);
                        break;
                    }

                    pos += len;
                }
                Err(e) => {
                    error!("AP read error: {:?}", e);
                    break;
                }
            };
        }

        let r = sock
            .write_all(concat!("HTTP/1.0 200 OK\r\n\r\n", include_str!("index.html")).as_bytes())
            .await;

        if let Err(e) = r {
            error!("http: write error: {e:?}");
        }

        let r = sock.flush().await;
        if let Err(e) = r {
            error!("http: flush error: {:?}", e);
        }
        Timer::after(Duration::from_millis(1000)).await;
        sock.close();
        Timer::after(Duration::from_millis(1000)).await;
        sock.abort();
    }
}
