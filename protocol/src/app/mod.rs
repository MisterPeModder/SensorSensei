use crate::codec::{AsyncEncode, AsyncEncoder};

pub mod v1;

/// Common structure of handshake packets, regardless of procotol version.
pub struct HandshakeGeneric<'v> {
    major: u8,
    minor: u8,
    /// Trailing data after major, minor.
    /// Encoded as `len: u32, data: [u8; len]`.
    tail: &'v [u8],
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &HandshakeGeneric<'_> {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        encoder
            .emit((self.major, self.minor, self.tail.len() as u32))
            .await?;
        encoder
            .emit_from_iter(&self.tail[..self.tail.len().min(u32::MAX as usize)])
            .await
    }
}
