use super::{HttpMethod, SOCKET_TIMEOUT};
use crate::{
    net::{tcp::BoxedTcpSocket, GATEWAY_IP},
    FutureTimeoutExt,
};
use core::net::Ipv4Addr;
use defmt::{debug, error, info, warn, Format};
use embassy_futures::select::Either;
use embassy_net::{tcp::TcpSocket, IpListenEndpoint, Stack};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;

#[cfg(feature = "display-ssd1306")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};

/// Dummy dual-stack HTTP server.
///
/// Endpoints:
/// - AP mode: server on the gateway IP
/// - STA mode: server exposed on an IP got from DHCP
pub struct HttpServer<'a> {
    endpoint: IpListenEndpoint,
    ap_socket: BoxedTcpSocket<'a>,
    sta_socket: Option<(BoxedTcpSocket<'a>, Ipv4Addr)>,
}

pub struct HttpServerRequest<'a, 'r> {
    method: HttpMethod,
    body: &'r mut [u8],
    sock: &'r mut TcpSocket<'a>,
}

pub struct HttpServerResponse<'a, 'r> {
    sock: &'r mut TcpSocket<'a>,
    pub status: u16,
}

#[derive(Format)]
pub enum HttpServerError {
    SocketError,
    SocketEof,
    FullBuffer,
}

#[cfg(feature = "display-ssd1306")]
#[derive(Clone, Copy)]
pub enum DisplayStatus {
    Initializing,
    ApOnly(Ipv4Addr, u16),
    DualStack(Ipv4Addr, u16),
}

#[cfg(feature = "display-ssd1306")]
pub static CURRENT_STATUS: Mutex<CriticalSectionRawMutex, DisplayStatus> =
    Mutex::new(DisplayStatus::Initializing);

impl<'a> HttpServer<'a> {
    pub async fn new(ap_stack: Stack<'a>, sta_stack: Stack<'a>, port: u16) -> Self {
        info!("http: waiting for AP and STA stacks...");

        let sta_address: Option<Ipv4Addr> = loop {
            if let Some(config) = sta_stack.config_v4() {
                break Some(config.address.address());
            }
            if let Err(crate::TimeoutError) = sta_stack
                .wait_config_up()
                .with_timeout(Duration::from_secs(20))
                .await
            {
                warn!("http: STA stack failed to configure after 20 seconds, disabling STA mode");
                break None;
            }
        };

        ap_stack.wait_link_up().await;

        if sta_address.is_some() {
            sta_stack.wait_link_up().await;
        }

        let endpoint = IpListenEndpoint { addr: None, port };
        let mut ap_socket = BoxedTcpSocket::new(ap_stack).expect("ap_socket: alloc failure");
        let sta_socket = sta_address.map(|a| {
            let mut socket = BoxedTcpSocket::new(sta_stack).expect("sta_socket: alloc failure");
            socket.set_timeout(Some(SOCKET_TIMEOUT));
            (socket, a)
        });

        ap_socket.set_timeout(Some(SOCKET_TIMEOUT));

        HttpServer {
            endpoint,
            ap_socket,
            sta_socket,
        }
    }

    /// Runs the HTTP server indefinitely.
    /// Accepts a `handler` function for client requests and responses.
    pub async fn run<H>(&mut self, mut handler: H) -> !
    where
        H: for<'r> AsyncFnMut(
            HttpServerRequest<'a, 'r>,
        ) -> Result<HttpServerResponse<'a, 'r>, HttpServerError>,
    {
        match &self.sta_socket {
            Some((_, sta_address)) => {
                info!(
                    "http-server: running dual-stack on port {}, STA address is {}, gateway IP is {}",
                    self.endpoint.port, sta_address, GATEWAY_IP,
                );
            }
            None => {
                info!(
                    "http-server: running single-stack on port {}, gateway IP is {}",
                    self.endpoint.port, GATEWAY_IP,
                );
            }
        }

        #[cfg(feature = "display-ssd1306")]
        {
            match &self.sta_socket {
                Some((_, sta_address)) => {
                    *CURRENT_STATUS.lock().await =
                        DisplayStatus::DualStack(*sta_address, self.endpoint.port);
                }
                None => {
                    *CURRENT_STATUS.lock().await =
                        DisplayStatus::ApOnly(GATEWAY_IP, self.endpoint.port);
                }
            }
        }

        let mut buffer = heapless::Vec::<u8, 1024>::new();
        loop {
            info!("http-server: waiting for connection");

            let Some(sock) = (match self.sta_socket {
                Some((ref mut sta_socket, _)) => {
                    Self::accept_socket_dual_stack(self.endpoint, &mut self.ap_socket, sta_socket)
                        .await
                }
                None => Self::accept_socket_single_stack(self.endpoint, &mut self.ap_socket).await,
            }) else {
                continue;
            };

            match Self::handle_client_request(sock, &mut handler, &mut buffer).await {
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

    async fn accept_socket_dual_stack<'b>(
        endpoint: IpListenEndpoint,
        ap_socket: &'b mut TcpSocket<'a>,
        sta_socket: &'b mut TcpSocket<'a>,
    ) -> Option<&'b mut TcpSocket<'a>> {
        let r = embassy_futures::select::select(
            ap_socket.accept(endpoint),
            sta_socket.accept(endpoint),
        )
        .await;

        match r {
            Either::First(Ok(())) => Some(ap_socket),
            Either::Second(Ok(())) => Some(sta_socket),
            Either::First(Err(e)) => {
                error!("http-server: AP socket error: {:?}", e);
                None
            }
            Either::Second(Err(e)) => {
                error!("http-server: STA socket error: {:?}", e);
                None
            }
        }
    }

    async fn accept_socket_single_stack<'b>(
        endpoint: IpListenEndpoint,
        socket: &'b mut TcpSocket<'a>,
    ) -> Option<&'b mut TcpSocket<'a>> {
        socket.accept(endpoint).await.map(|_| socket).ok()
    }

    /// Called upon HTTP request to the given socket.
    /// This "parses" the incoming request and forwards them to the handler function.
    async fn handle_client_request<'r, H>(
        sock: &'r mut TcpSocket<'a>,
        handler: &mut H,
        buffer: &'r mut heapless::Vec<u8, 1024>,
    ) -> Result<HttpServerResponse<'a, 'r>, HttpServerError>
    where
        H: AsyncFnMut(
            HttpServerRequest<'a, 'r>,
        ) -> Result<HttpServerResponse<'a, 'r>, HttpServerError>,
    {
        debug!("http-server: handling client request");
        buffer.clear();
        let method_end = Self::read_until_byte(sock, buffer, b' ').await?;
        let Ok(method) = HttpMethod::try_from(&buffer[..method_end]) else {
            let mut res = HttpServerResponse::new(sock);
            res.return_bad_request().await?;
            return Ok(res);
        };
        Self::shift_buffer(buffer, method_end + 1);
        debug!("http-server: method: {}", AsRef::<str>::as_ref(&method));

        // Read the Content-Length header, expecting 'Content-Length: ' (other formats are not supported).
        // OR read until the end of the headers.
        let (content_length, headers_end) = match Self::read_until_bytes(
            sock,
            buffer,
            &[b"Content-Length: ", b"\r\n\r\n"],
        )
        .await?
        {
            // "Content-Length: " encountered first, read the content length
            ReadUntilBytesMatch {
                pattern: 0,
                start: content_length_start,
            } => {
                Self::shift_buffer(buffer, content_length_start + 16); // 16 is the length of "Content-Length: "
                let content_length_end = Self::read_until_byte(sock, buffer, b'\r').await?;

                let Some(content_length) = core::str::from_utf8(&buffer[..content_length_end])
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                else {
                    info!("http-server: invalid Content-Length header");
                    let mut res = HttpServerResponse::new(sock);
                    res.return_bad_request().await?;
                    return Ok(res);
                };
                debug!("http-server: content length: {}", content_length);

                // then discard other headers
                let headers_end = Self::read_until_bytes(sock, buffer, &[b"\r\n\r\n"])
                    .await?
                    .start;

                (content_length, headers_end)
            }
            // "\r\n\r\n" encountered first, no Content-Length header, thus no body
            ReadUntilBytesMatch {
                pattern: _,
                start: headers_end,
            } => (0, headers_end),
        };

        if content_length > 0 {
            Self::shift_buffer(buffer, headers_end + 4); // 4 is the length of "\r\n\r\n"

            // the number of bytes of the body that are not yet in the buffer
            let mut remaining = content_length.saturating_sub(buffer.len());

            while remaining > 0 {
                info!(
                    "http-server: reading remaining body ({=usize}/{=usize})",
                    buffer.len(),
                    content_length
                );
                Self::read_append(sock, buffer).await?;
                remaining = content_length.saturating_sub(buffer.len());
            }
            info!(
                "http-server: body fully read ({=usize}/{=usize})",
                buffer.len(),
                content_length
            );
        }

        let req = HttpServerRequest {
            method,
            body: &mut buffer[..content_length],
            sock,
        };
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

    /// Reads the socket until one of the byte sequences are encountered, or there is a networking error, or the buffer is completely full.
    /// Returns a match with the position of the first pattern found and its index in the `patterns` slice.
    async fn read_until_bytes<const N: usize>(
        sock: &mut TcpSocket<'_>,
        buf: &mut heapless::Vec<u8, N>,
        patterns: &[&[u8]],
    ) -> Result<ReadUntilBytesMatch, HttpServerError> {
        let mut offset = 0usize;
        loop {
            for (i, pattern) in patterns.iter().enumerate() {
                if let Some(pos) = memchr::memmem::find(&buf[offset..], pattern) {
                    return Ok(ReadUntilBytesMatch {
                        start: offset + pos,
                        pattern: i,
                    });
                }
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

struct ReadUntilBytesMatch {
    start: usize,
    pattern: usize,
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

    pub fn body(&mut self) -> &mut [u8] {
        self.body
    }

    pub fn new_response(self) -> HttpServerResponse<'a, 'r> {
        HttpServerResponse::new(self.sock)
    }
}

impl<'a, 'r> HttpServerResponse<'a, 'r> {
    pub fn new(sock: &'r mut TcpSocket<'a>) -> Self {
        HttpServerResponse { status: 200, sock }
    }

    pub async fn write_all(&mut self, data: &[u8]) -> Result<(), HttpServerError> {
        self.sock
            .write_all(data)
            .await
            .map_err(|_| HttpServerError::SocketError)
    }

    pub async fn write_all_vectored(&mut self, bufs: &[&[u8]]) -> Result<(), HttpServerError> {
        for buf in bufs {
            self.sock
                .write_all(buf)
                .await
                .map_err(|_| HttpServerError::SocketError)?;
        }
        Ok(())
    }

    pub async fn return_bad_request(&mut self) -> Result<(), HttpServerError> {
        self.status = 400;
        self.sock
            .write_all(b"HTTP/1.0 400 Bad Request\r\nConnection: close\r\n\r\n")
            .await
            .map_err(|_| HttpServerError::SocketError)
    }

    pub async fn return_not_found(&mut self) -> Result<(), HttpServerError> {
        self.status = 404;
        self.sock
            .write_all(b"HTTP/1.0 400 Not Found\r\nConnection: close\r\n\r\n")
            .await
            .map_err(|_| HttpServerError::SocketError)
    }

    pub async fn return_see_other(&mut self, location: &str) -> Result<(), HttpServerError> {
        self.status = 303;
        self.write_all_vectored(&[
            b"HTTP/1.0 303 See Other\r\nLocation: ",
            location.as_bytes(),
            b"\r\nConnection: close\r\n\r\n",
        ])
        .await
    }

    pub async fn finish_connection(&mut self) {
        HttpServer::finish_connection(self.sock).await
    }
}
