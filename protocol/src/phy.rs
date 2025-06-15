use core::future::Future;

/// Physical Layer abstraction: provides raw read/write access to radio hardware
pub trait PhysicalLayer {
    type Error: core::error::Error;

    /// Read the next full physical packet.
    async fn read(&mut self) -> Result<(), Self::Error>;

    /// Returns the buffer containing the received data.
    fn rx_buffer(&self) -> &[u8];

    /// Appends `data` to the buffer for sending.
    async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error>;

    /// Sends any buffered data to the physical layer.
    async fn flush(&mut self) -> Result<(), Self::Error>;
}

impl<PHY: PhysicalLayer> PhysicalLayer for &mut PHY {
    type Error = PHY::Error;

    fn read(&mut self) -> impl Future<Output = Result<(), Self::Error>> {
        (*self).read()
    }

    fn rx_buffer(&self) -> &[u8] {
        (*self as &PHY).rx_buffer()
    }

    fn write(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Self::Error>> {
        (*self).write(buf)
    }

    fn flush(&mut self) -> impl Future<Output = Result<(), Self::Error>> {
        (*self).flush()
    }
}
