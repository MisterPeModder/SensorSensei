pub mod v1 {
    /// Exposes an IO interface for the LoRa physical layer + the MAC layer for the application layer to build upon.
    pub trait IdentifiedReadWrite {
        type Error: core::fmt::Debug;
        /// Identifies a peer that this impl can receive data from.  
        /// May be zero-size if relevant. (e.g. the gateway ID)
        type SourceId: Copy + Eq + core::hash::Hash;
        /// Identifies a peer that this impl can send data to.  
        /// May be zero-size if relevant. (e.g. the gateway ID)
        type DestId: Copy + Eq + core::hash::Hash;

        /// Read data from a peer.
        ///
        /// Returns the source peer ID and the number of bytes read.
        /// The number of bytes is smaller or equal to the buffer length.
        ///
        /// Multiple calls to read() may be needed in order to read an entire app-level packet:
        /// It is adviced to call read() in a loop until it reports 0 bytes read.
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
        /// Returns the amount of bytes written, this number is smaller of equal to the buffer length.
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
}
