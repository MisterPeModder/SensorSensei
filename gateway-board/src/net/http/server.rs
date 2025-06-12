use super::{HttpMethod, SOCKET_TIMEOUT};
use crate::net::tcp::BoxedTcpSocket;
use core::net::Ipv4Addr;
use defmt::{error, info, Format};
use embassy_futures::select::Either;
use embassy_net::{tcp::TcpSocket, IpListenEndpoint, Stack};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;

/// Dummy dual-stack HTTP server.
///
/// Endpoints:
/// - AP mode: server on the gateway IP
/// - STA mode: server exposed on an IP got from DHCP
pub struct HttpServer<'a> {
    endpoint: IpListenEndpoint,
    ap_socket: BoxedTcpSocket<'a>,
    sta_socket: BoxedTcpSocket<'a>,
    sta_address: Ipv4Addr,
}

pub struct HttpServerRequest<'a, 'r> {
    method: HttpMethod,
    sock: &'r mut TcpSocket<'a>,
}

pub struct HttpServerResponse<'a, 'r> {
    sock: &'r mut TcpSocket<'a>,
    status: u16,
}

#[derive(Format)]
pub enum HttpServerError {
    SocketError,
    SocketEof,
    FullBuffer,
}

impl<'a> HttpServer<'a> {
    pub async fn new(ap_stack: Stack<'a>, sta_stack: Stack<'a>, port: u16) -> Self {
        info!("http: waiting for AP and STA stacks...");

        let sta_address = loop {
            if let Some(config) = sta_stack.config_v4() {
                let address = config.address.address();
                break address;
            }
            sta_stack.wait_config_up().await;
        };
        ap_stack.wait_link_up().await;
        sta_stack.wait_link_up().await;

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

    /// FIXME: temporary
    pub async fn run_demo(&mut self) -> ! {
        self.run(async |req: HttpServerRequest| {
            let mut res = req.new_response();
            res.return_dummy_page().await?;
            Ok(res)
        })
        .await
    }

    /// Runs the HTTP server indefinitely.
    /// Accepts a `handler` function for client requests and responses.
    pub async fn run<H>(&mut self, mut handler: H) -> !
    where
        H: for<'r> AsyncFnMut(
            HttpServerRequest<'a, 'r>,
        ) -> Result<HttpServerResponse<'a, 'r>, HttpServerError>,
    {
        info!(
            "http-server: running on port {}, STA address is {}",
            self.endpoint.port, self.sta_address
        );
        loop {
            info!("http-server: waiting for connection");

            let r = embassy_futures::select::select(
                self.ap_socket.accept(self.endpoint),
                self.sta_socket.accept(self.endpoint),
            )
            .await;

            let sock = match r {
                Either::First(Ok(())) => &mut self.ap_socket,
                Either::Second(Ok(())) => &mut self.sta_socket,
                Either::First(Err(e)) => {
                    error!("http-server: AP socket error: {:?}", e);
                    continue;
                }
                Either::Second(Err(e)) => {
                    error!("http-server: STA socket error: {:?}", e);
                    continue;
                }
            };

            match Self::handle_client_request(sock, &mut handler).await {
                Ok(res) => {
                    info!("http-server: client response: {:?}", res.status);
                }
                Err(e) => {
                    error!("http-server: client handling error: {:?}", e);
                }
            }

            // always terminate connection, regardless of errors
            Self::finish_connection(sock).await;
        }
    }

    /// Called upon HTTP request to the given socket.
    /// This "parses" the incoming request and forwards them to the handler function.
    async fn handle_client_request<'r, H>(
        sock: &'r mut TcpSocket<'a>,
        handler: &mut H,
    ) -> Result<HttpServerResponse<'a, 'r>, HttpServerError>
    where
        H: AsyncFnMut(
            HttpServerRequest<'a, 'r>,
        ) -> Result<HttpServerResponse<'a, 'r>, HttpServerError>,
    {
        let mut buffer = heapless::Vec::<u8, 1024>::new();

        let method_end = Self::read_until_byte(sock, &mut buffer, b' ').await?;
        let Ok(method) = HttpMethod::try_from(&buffer[..method_end]) else {
            let mut res = HttpServerResponse::new(sock);
            res.return_bad_request().await?;
            return Ok(res);
        };
        Self::shift_buffer(&mut buffer, method_end + 1);

        // discard all the headers
        Self::read_until_bytes(sock, &mut buffer, b"\r\n\r\n").await?;

        // FIXME: we don't care about the bodies for now

        let req = HttpServerRequest { method, sock };
        handler(req).await
    }

    async fn finish_connection(sock: &mut TcpSocket<'_>) {
        sock.flush()
            .await
            .unwrap_or_else(|e| error!("http-server: failed to flush response{:?}", e));
        Timer::after(Duration::from_millis(1000)).await;
        sock.close();
        Timer::after(Duration::from_millis(1000)).await;
        sock.abort();
    }

    /// Reads bytes from the socket and appends them to the buffer in a *very* safe way.
    async fn read_append<const N: usize>(
        sock: &mut TcpSocket<'_>,
        buf: &mut heapless::Vec<u8, N>,
    ) -> Result<(), HttpServerError> {
        if buf.is_full() {
            return Err(HttpServerError::FullBuffer);
        }
        let old_len = buf.len();

        let free: &mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().add(old_len), N - old_len) };
        let count = sock.read(free).await?;

        unsafe {
            buf.set_len(old_len + count);
        }

        if count == 0 {
            Err(HttpServerError::SocketEof)
        } else {
            Ok(())
        }
    }

    /// Reads the socket until a specific byte is encountered, or there is a networking error, or the buffer is completely full.
    /// Returns the position of the byte from the start of the buffer.
    async fn read_until_byte<const N: usize>(
        sock: &mut TcpSocket<'_>,
        buf: &mut heapless::Vec<u8, N>,
        byte: u8,
    ) -> Result<usize, HttpServerError> {
        let mut offset = 0usize;
        loop {
            if let Some(pos) = memchr::memchr(byte, &buf[offset..]) {
                break Ok(offset + pos);
            }
            offset = buf.len();
            Self::read_append(sock, buf).await?;
        }
    }

    /// Reads the socket until a specific byte sequence is encountered, or there is a networking error, or the buffer is completely full.
    /// Returns the position of the beginning of the sequence from the start of the buffer.
    async fn read_until_bytes<const N: usize>(
        sock: &mut TcpSocket<'_>,
        buf: &mut heapless::Vec<u8, N>,
        bytes: &[u8],
    ) -> Result<usize, HttpServerError> {
        let mut offset = 0usize;
        loop {
            if let Some(pos) = memchr::memmem::find(&buf[offset..], bytes) {
                break Ok(offset + pos);
            }
            offset = buf.len();
            Self::read_append(sock, buf).await?;
        }
    }

    /// Totally 100% efficient way to consume `count` bytes from the buffer.
    fn shift_buffer<const N: usize>(buf: &mut heapless::Vec<u8, N>, count: usize) {
        buf.copy_within(count.., 0);
        buf.truncate(buf.len() - count);
    }
}

impl From<embassy_net::tcp::Error> for HttpServerError {
    fn from(_: embassy_net::tcp::Error) -> Self {
        HttpServerError::SocketError
    }
}

impl<'a, 'r> HttpServerRequest<'a, 'r> {
    pub fn method(&self) -> HttpMethod {
        self.method
    }

    pub fn new_response(self) -> HttpServerResponse<'a, 'r> {
        HttpServerResponse::new(self.sock)
    }
}

impl<'a, 'r> HttpServerResponse<'a, 'r> {
    pub fn new(sock: &'r mut TcpSocket<'a>) -> Self {
        HttpServerResponse { status: 200, sock }
    }

    pub async fn return_bad_request(&mut self) -> Result<(), HttpServerError> {
        self.status = 400;
        self.sock
            .write_all(b"HTTP/1.0 400 Bad Request\r\n\r\n")
            .await
            .map_err(|_| HttpServerError::SocketError)
    }

    /// FIXME: remove this (and the index.html file) once unused
    async fn return_dummy_page(&mut self) -> Result<(), HttpServerError> {
        self.status = 200;
        self.sock
            .write_all(concat!("HTTP/1.0 200 OK\r\n\r\n", include_str!("index.html")).as_bytes())
            .await
            .map_err(|_| HttpServerError::SocketError)
    }
}
