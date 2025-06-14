use core::future::Future;

/// Physical Layer abstraction: provides raw read/write access to radio hardware
pub trait PhysicalLayer {
    type Error: core::error::Error;

    async fn send(&mut self, data: &[u8]) -> Result<usize, Self::Error>;
    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;

    /// Sends the entire `data` buffer, ensuring that all bytes are sent.
    async fn send_exact(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        let mut sent = 0usize;
        while sent < data.len() {
            let bytes_sent = self.send(&data[sent..]).await?;
            sent += bytes_sent;
        }
        Ok(())
    }
}

impl<PHY: PhysicalLayer> PhysicalLayer for &mut PHY {
    type Error = PHY::Error;

    fn send(&mut self, data: &[u8]) -> impl Future<Output = Result<usize, Self::Error>> {
        (*self).send(data)
    }

    fn recv(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Self::Error>> {
        (*self).recv(buf)
    }
}
