use embassy_time::Duration;

mod client;
mod server;

pub use client::*;
pub use server::*;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub enum HttpMethod {
    Post,
}
