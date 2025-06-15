use core::future::Future;

/// Physical Layer abstraction: provides raw read/write access to radio hardware
pub trait PhysicalLayer {
    type Error: core::error::Error;

    /// Read the next full physical packet.
    /// NOTE: The buffer may be reused for reading and writing.
    async fn read(&mut self) -> Result<(), Self::Error>;

    fn buffer(&self) -> &[u8];

    /// Appends `data` to the buffer for sending.
    /// NOTE: The buffer may be reused for reading and writing.
    async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error>;

    /// Sends any buffered data to the physical layer.
    /// NOTE: The buffer may be reused for reading and writing.
    async fn flush(&mut self) -> Result<(), Self::Error>;
}

impl<PHY: PhysicalLayer> PhysicalLayer for &mut PHY {
    type Error = PHY::Error;

    fn read(&mut self) -> impl Future<Output = Result<(), Self::Error>> {
        (*self).read()
    }

    fn buffer(&self) -> &[u8] {
        (*self as &PHY).buffer()
    }

    fn write(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Self::Error>> {
        (*self).write(buf)
    }

    fn flush(&mut self) -> impl Future<Output = Result<(), Self::Error>> {
        (*self).flush()
    }
}
