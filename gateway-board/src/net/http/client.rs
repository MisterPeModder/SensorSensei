use super::{HttpMethod, SOCKET_TIMEOUT};
use crate::net::tcp::BoxedTcpSocket;
use alloc::fmt;
use core::ops::{Deref, DerefMut};
use defmt::{error, info, trace};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::ConnectError;
use embassy_net::{IpEndpoint, Stack};
use embedded_io_async::Write;
use thiserror::Error;

/// Basic HTTP 1.0 Client.
///
/// # Usage Example
///
/// ```no_run
/// # async fn http_client_demo(network_stack: ::embassy_net::Stack<'_>) {
/// use embedded_io_async::Write;
/// use gateway_board::net::http::{HttpClient, HttpMethod};
///
/// let mut client = HttpClient::new(network_stack);
///
/// // Start POST request
/// let mut request = client.request(HttpMethod::Post, "example.com", 80, "/").await.unwrap();
///
/// // Add headers
/// request.header("Content-Type", "application/json").await.unwrap();
/// request.header("User-Agent", "http-demo").await.unwrap();
///
/// // Write request body
/// request.body().extend_from_slice(br#"{"key":"value}"#);
///
/// let response = request.finish().await.unwrap();
/// assert_eq!(response.status(), 200);
/// # }
pub struct HttpClient<'a> {
    stack: Stack<'a>,
    body_buf: alloc::vec::Vec<u8>,
}

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("allocation failure")]
    AllocationFailure,
    #[error("connection error {0:?}")]
    Connect(ConnectError),
    #[error("io error {0:?}")]
    Io(embassy_net::tcp::Error),
    #[error("io buffer overflow")]
    BufferOverflow,
    #[error("invalid HTTP response")]
    InvalidHttpResponse,
    #[error("DNS error")]
    DnsError,
}

pub struct HttpRequest<'a> {
    socket: BoxedTcpSocket<'a>,
    body: &'a mut HttpBody,
}

pub struct HttpResponse {
    status: u16,
}

impl<'a> HttpClient<'a> {
    #[must_use]
    pub fn new(stack: Stack<'a>) -> Self {
        HttpClient {
            stack,
            body_buf: alloc::vec::Vec::new(),
        }
    }

    pub async fn request<'b>(
        &'b mut self,
        method: HttpMethod,
        host: &str,
        port: u16,
        path: impl AsRef<[u8]>,
    ) -> Result<HttpRequest<'b>, HttpClientError> {
        info!("http-client: DNS lookup for {}...", host);
        let address = match self.stack.dns_query(host, DnsQueryType::A).await {
            Ok(res) => res[0],
            Err(_) => return Err(HttpClientError::DnsError),
        };
        info!("http-client: {} resolved to {}", host, address);

        let endpoint = IpEndpoint::new(address, port);

        info!("http-client: connecting to {}", endpoint);
        let mut socket =
            BoxedTcpSocket::new(self.stack).map_err(|()| HttpClientError::AllocationFailure)?;
        socket.set_timeout(Some(SOCKET_TIMEOUT));
        socket.connect(endpoint).await?;

        socket.write_all(method.as_ref().as_bytes()).await?;
        socket.write_all(b" ").await?;
        socket.write_all(path.as_ref()).await?;
        socket.write_all(b" HTTP/1.0\r\n").await?;

        self.body_buf.clear();

        let mut headers = HttpRequest {
            socket,
            body: HttpBody::from_mut_vec(&mut self.body_buf),
        };
        headers.header(b"Host", host.as_bytes()).await?;
        Ok(headers)
    }
}

impl HttpRequest<'_> {
    pub async fn header(
        &mut self,
        name: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
    ) -> Result<(), HttpClientError> {
        self.socket.write_all(name.as_ref()).await?;
        self.socket.write_all(b": ").await?;
        self.socket.write_all(value.as_ref()).await?;
        self.socket.write_all(b"\r\n").await?;
        Ok(())
    }

    pub fn body(&mut self) -> &mut HttpBody {
        self.body
    }

    pub async fn finish(mut self) -> Result<HttpResponse, HttpClientError> {
        use core::fmt::Write;

        let mut content_len_str: heapless::String<10> = heapless::String::new();
        _ = write!(&mut content_len_str, "{}", self.body.len());
        self.header("Content-Length", content_len_str).await?;
        self.socket.write_all(b"\r\n").await?;
        self.socket.write_all(self.body).await?;
        self.socket.flush().await?;
        info!("http: request finished, waiting for response");
        HttpResponse::read(self.socket).await
    }
}

#[repr(transparent)]
pub struct HttpBody(alloc::vec::Vec<u8>);

impl fmt::Write for HttpBody {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.as_mut().extend_from_slice(s.as_bytes());
        Ok(())
    }
}

impl HttpBody {
    #[inline]
    pub const fn from_mut_vec(vec: &mut alloc::vec::Vec<u8>) -> &mut HttpBody {
        // SAFETY: HttpBody has the same memory layout as Vec<u8> due to repr(transparent)
        unsafe { core::mem::transmute(vec) }
    }

    #[inline]
    #[must_use]
    pub const fn as_vec(&self) -> &alloc::vec::Vec<u8> {
        // SAFETY: HttpBody has the same memory layout as Vec<u8> due to repr(transparent)
        unsafe { core::mem::transmute(self) }
    }

    #[inline]
    #[must_use]
    pub const fn as_mut_vec(&mut self) -> &mut alloc::vec::Vec<u8> {
        // SAFETY: HttpBody has the same memory layout as Vec<u8> due to repr(transparent)
        unsafe { core::mem::transmute(self) }
    }
}

impl Deref for HttpBody {
    type Target = alloc::vec::Vec<u8>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_vec()
    }
}

impl DerefMut for HttpBody {
    #[inline]
    fn deref_mut(&mut self) -> &mut <Self as Deref>::Target {
        self.as_mut_vec()
    }
}

impl AsRef<alloc::vec::Vec<u8>> for HttpBody {
    #[inline]
    fn as_ref(&self) -> &alloc::vec::Vec<u8> {
        self.as_vec()
    }
}

impl AsMut<alloc::vec::Vec<u8>> for HttpBody {
    #[inline]
    fn as_mut(&mut self) -> &mut alloc::vec::Vec<u8> {
        self.as_mut_vec()
    }
}

impl From<ConnectError> for HttpClientError {
    #[inline]
    fn from(e: ConnectError) -> Self {
        Self::Connect(e)
    }
}

impl From<embassy_net::tcp::Error> for HttpClientError {
    #[inline]
    fn from(e: embassy_net::tcp::Error) -> Self {
        Self::Io(e)
    }
}

impl AsRef<str> for HttpMethod {
    fn as_ref(&self) -> &str {
        match self {
            HttpMethod::Post => "POST",
        }
    }
}

impl HttpResponse {
    async fn read(mut socket: BoxedTcpSocket<'_>) -> Result<Self, HttpClientError> {
        let mut buf = [0u8; 128];

        let res_line_len = Self::read_line(&mut socket, &mut buf).await?;
        let mut res_line: &str = core::str::from_utf8(&buf[..res_line_len])
            .map_err(|_| HttpClientError::InvalidHttpResponse)?;

        if !res_line.starts_with("HTTP/1.0 ") && !res_line.starts_with("HTTP/1.1 ") {
            trace!("http-client: unsupported response method");
            return Err(HttpClientError::InvalidHttpResponse);
        }
        res_line = &res_line[9..];

        let status_len = res_line
            .find(' ')
            .ok_or(HttpClientError::InvalidHttpResponse)?;
        let status: u16 = res_line[0..status_len]
            .parse()
            .map_err(|_| HttpClientError::InvalidHttpResponse)?;

        while socket.read(&mut buf).await? > 0 {
            // we have the status, consume the rest of the response
        }

        Ok(HttpResponse { status })
    }

    async fn read_line(
        socket: &mut BoxedTcpSocket<'_>,
        buf: &mut [u8],
    ) -> Result<usize, HttpClientError> {
        let mut filled = socket.read(buf).await?;

        loop {
            match memchr::memchr2(b'\r', b'\n', &buf[..filled]) {
                None => {
                    if filled == buf.len() {
                        return Err(HttpClientError::BufferOverflow);
                    }
                    filled += socket.read(&mut buf[filled..]).await?;
                }
                Some(line_len) => break Ok(line_len),
            }
        }
    }
}

impl HttpResponse {
    #[inline]
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }
}
