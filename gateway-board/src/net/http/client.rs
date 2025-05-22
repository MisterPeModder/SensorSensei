use super::{HttpMethod, SOCKET_TIMEOUT};
use crate::net::tcp::BoxedTcpSocket;
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
/// let client = HttpClient::new(network_stack);
///
/// // Start POST request
/// let mut request = client.request(HttpMethod::Post, "example.com", 80, "/").await.unwrap();
///
/// // Add headers
/// request.add("Content-Type", "application/json").await.unwrap();
/// request.add("Content-Length", "14").await.unwrap();
///
/// // Write request body
/// let mut body = request.body().await.unwrap();
/// body.write_all(br#"{"key":"value}"#).await.unwrap();
///
/// let response = body.finish().await.unwrap();
/// assert_eq!(response.status(), 200);
/// # }
pub struct HttpClient<'a> {
    stack: Stack<'a>,
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

pub struct HttpRequestHeaders<'a> {
    socket: BoxedTcpSocket<'a>,
}

pub struct HttpRequestBody<'a> {
    socket: BoxedTcpSocket<'a>,
}

pub struct HttpResponse {
    status: u16,
}

impl<'a> HttpClient<'a> {
    pub fn new(stack: Stack<'a>) -> Self {
        HttpClient { stack }
    }

    pub async fn request(
        &self,
        method: HttpMethod,
        host: &str,
        port: u16,
        path: impl AsRef<[u8]>,
    ) -> Result<HttpRequestHeaders<'a>, HttpClientError> {
        info!("http-client: DNS lookup for {}...", host);
        let address = match self.stack.dns_query(host, DnsQueryType::A).await {
            Ok(res) => res[0],
            Err(_) => return Err(HttpClientError::DnsError),
        };
        info!("http-client: {} resolved to {}", host, address);

        let endpoint = IpEndpoint::new(address, port);

        info!("http-client: connecting to {}", endpoint);
        let mut socket =
            BoxedTcpSocket::new(self.stack).map_err(|_| HttpClientError::AllocationFailure)?;
        socket.set_timeout(Some(SOCKET_TIMEOUT));
        socket.connect(endpoint).await?;

        socket.write_all(method.as_ref().as_bytes()).await?;
        socket.write_all(b" ").await?;
        socket.write_all(path.as_ref()).await?;
        socket.write_all(b" HTTP/1.0\r\n").await?;

        let mut headers = HttpRequestHeaders { socket };
        headers.add(b"Host", host.as_bytes()).await?;
        Ok(headers)
    }
}

impl<'a> HttpRequestHeaders<'a> {
    pub async fn add(
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

    pub async fn body(mut self) -> Result<HttpRequestBody<'a>, HttpClientError> {
        self.socket.write_all(b"\r\n").await?;
        Ok(HttpRequestBody {
            socket: self.socket,
        })
    }
}

impl HttpRequestBody<'_> {
    pub async fn finish(mut self) -> Result<HttpResponse, HttpClientError> {
        self.socket.flush().await?;
        info!("http: request finished, waiting for response");
        HttpResponse::read(self.socket).await
    }
}

impl embedded_io_async::ErrorType for HttpRequestBody<'_> {
    type Error = embassy_net::tcp::Error;
}

impl embedded_io_async::Write for HttpRequestBody<'_> {
    #[inline]
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.socket.write(buf).await
    }

    #[inline]
    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.socket.flush().await
    }

    #[inline]
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.socket.write_all(buf).await
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
    pub fn status(&self) -> u16 {
        self.status
    }
}
