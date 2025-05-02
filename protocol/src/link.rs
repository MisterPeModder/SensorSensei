pub mod v1 {
    use crate::phy::PhysicalLayer;

    /// Exposes an IO interface for the LoRa physical layer + the MAC layer for the application layer to build upon.
    pub trait LinkLayer {
        type Error: core::error::Error;
        /// Identifies a peer that this impl can receive data from.  
        /// May be zero-size if relevant. (e.g., the gateway ID)
        type SourceId: Copy + Eq + core::hash::Hash;
        /// Identifies a peer that this impl can send data to.  
        /// May be zero-size if relevant. (e.g., the gateway ID)
        type DestId: Copy + Eq + core::hash::Hash;

        /// Read data from a peer.
        ///
        /// Returns the source peer ID and the number of bytes read.
        /// The number of bytes is smaller or equal to the buffer length.
        ///
        /// Multiple calls to read() may be needed to read an entire app-level packet:
        /// It is advised to call read() in a loop until it reports 0 bytes read.
        async fn read(&mut self, buf: &mut [u8]) -> Result<(usize, Self::SourceId), Self::Error>;

        /// Write data to a peer or broadcast to everyone.
        ///
        /// This function writes part of (or all of) the passed buffer to the desired peer.
        /// When `dest` is `None`, the data is sent to everyone.
        ///
        /// Note:  
        /// This function is not guaranteed to immediately send the data to the peer and instead buffer it for efficiency reasons.
        /// Please call `flush()` upon finishing writing app-level packets.
        ///
        /// Returns the number of bytes written, this number is smaller or equal to the buffer length.
        async fn write(
            &mut self,
            dest: Option<Self::DestId>,
            buf: &[u8],
        ) -> Result<usize, Self::Error>;

        /// Forcefully send any remaining data to the peer (or everyone when `dest` is None).
        ///
        /// This is needed because `write()` may buffer its data instead of sending it.
        async fn flush(&mut self, dest: Option<Self::DestId>) -> Result<(), Self::Error>;
    }

    /// Really broken link layer implementation
    pub struct DummyLinkLayer<PHY> {
        phy: PHY,
        tx_buf: heapless::Vec<u8, 64>,
    }

    impl<PHY: PhysicalLayer> DummyLinkLayer<PHY> {
        pub fn new(phy: PHY) -> Self {
            DummyLinkLayer {
                phy,
                tx_buf: heapless::Vec::new(),
            }
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq, Hash)]
    pub struct DummyId;

    impl<PHY: PhysicalLayer> LinkLayer for DummyLinkLayer<PHY> {
        type Error = PHY::Error;
        type SourceId = DummyId;
        type DestId = DummyId;

        async fn read(&mut self, buf: &mut [u8]) -> Result<(usize, Self::SourceId), Self::Error> {
            let bytes_read = self.phy.recv(buf).await?;
            Ok((bytes_read, DummyId))
        }

        async fn write(
            &mut self,
            dest: Option<Self::DestId>,
            buf: &[u8],
        ) -> Result<usize, Self::Error> {
            let mut bytes_sent = 0usize;

            for chunk in buf.chunks(self.tx_buf.capacity()) {
                if let Err(()) = self.tx_buf.extend_from_slice(chunk) {
                    self.flush(dest).await?;
                    // SAFETY:
                    // - size of `chunk` is lower or equal to tx_buf's capacity
                    // - `buf` and `chunk` cannot overlap, tx_buf is private to this struct
                    unsafe {
                        self.tx_buf
                            .as_mut_ptr()
                            .copy_from_nonoverlapping(chunk.as_ptr(), chunk.len());
                        self.tx_buf.set_len(chunk.len());
                    }
                }
                bytes_sent += chunk.len();
            }
            Ok(bytes_sent)
        }

        async fn flush(&mut self, dest: Option<Self::DestId>) -> Result<(), Self::Error> {
            _ = dest; // no caching
            let mut buf: &[u8] = &self.tx_buf;
            while !buf.is_empty() {
                let bytes_sent = self.phy.send(&self.tx_buf).await?;
                buf = &buf[bytes_sent..];
            }
            self.tx_buf.clear();
            Ok(())
        }
    }
}
