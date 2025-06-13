use embassy_time::Duration;

pub mod api;
mod client;
mod server;

pub use client::*;
pub use server::*;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
}

impl AsRef<str> for HttpMethod {
    fn as_ref(&self) -> &str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        }
    }
}

impl TryFrom<&[u8]> for HttpMethod {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            b"GET" => Ok(HttpMethod::Get),
            b"POST" => Ok(HttpMethod::Post),
            _ => Err(()),
        }
    }
}
