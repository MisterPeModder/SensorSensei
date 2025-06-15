use crate::phy::PhysicalLayer;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Exposes an IO interface for the LoRa physical layer + the MAC layer for the application layer to build upon.
pub trait LinkLayer {
    type Error: core::error::Error;
    /// Identifies a peer that this impl can send data to and receive data from.
    /// May be zero-size if relevant. (e.g., the gateway ID)
    type PeerId: Copy + Eq + core::hash::Hash;

    /// Read data from a peer.
    ///
    /// Returns the source peer ID and the number of bytes read.
    /// The number of bytes is smaller or equal to the buffer length.
    ///
    /// Multiple calls to read() may be needed to read an entire app-level packet:
    /// It is advised to call read() in a loop until it reports 0 bytes read.
    async fn read(&mut self, buf: &mut [u8]) -> Result<(usize, Self::PeerId), Self::Error>;

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
    async fn write(&mut self, dest: Option<Self::PeerId>, buf: &[u8])
        -> Result<usize, Self::Error>;

    /// Forcefully send any remaining data to the peer (or everyone when `dest` is None).
    ///
    /// This is needed because `write()` may buffer its data instead of sending it.
    async fn flush(&mut self, dest: Option<Self::PeerId>) -> Result<(), Self::Error>;

    fn reset(&mut self);
}

/// *The* Gateway ID, version 1 of the protocol only supports one gateway.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct GatewayId;

/// 4-bit ID of a sensor board.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct SensorBoardId(pub u8);

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum LinkPhase {
    Handshake,
    Data,
}

impl LinkPhase {
    fn to_bits(self) -> u8 {
        match self {
            Self::Handshake => 0b10,
            Self::Data => 0b00,
        }
    }
    fn from_bits(bits: u8) -> Self {
        if bits == 0b10 {
            Self::Handshake
        } else {
            Self::Data
        }
    }
}

pub struct LinkPacket<'a> {
    pub phase: LinkPhase,
    pub id: u8,
    pub payload: &'a [u8],
}

impl<'a> LinkPacket<'a> {
    pub async fn write<PHY: PhysicalLayer>(
        self,
        mut phy: PHY,
        sig_key: &[u8],
    ) -> Result<(), PHY::Error> {
        let action_bits: u8 = self.phase.to_bits();
        let header_meta: u8 = (action_bits << 6) | ((self.id & 0b1111) << 2); // id (4 bits)
        let sig_bits: u64 = Self::sign_payload(self.payload, sig_key);

        let header: u64 = (header_meta as u64) << 56 | (sig_bits >> 6);

        phy.write(&header.to_be_bytes()[..5]).await?;
        phy.write(self.payload).await?;
        phy.flush().await
    }

    /// Read the next link packet, ignoring malformed packets.
    /// Returns the link phase and the ID of the packet, for the actual payload use `get_payload()`.
    ///
    /// Implentation note: I had to split read() and get_payload() because of the weirdest lifetime errors I've ever seen.
    /// if you have an afternoon and some sanity to spare, I'd be happy to hear how to fix this.
    pub async fn read<PHY: PhysicalLayer>(
        mut phy: PHY,
        sig_key: &[u8],
    ) -> Result<(LinkPhase, u8), PHY::Error> {
        loop {
            phy.read().await?;
            let bytes: &[u8] = phy.rx_buffer();
            if bytes.len() < 6 {
                #[cfg(feature = "defmt")]
                defmt::trace!("link: packet too small: {}", bytes.len());
                continue;
            }

            let header_meta: u8 = bytes[0];
            let sig_bits: u64 =
                u64::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], 0, 0, 0])
                    << 6;
            let payload = &bytes[5..];

            // first 34 bits of the signature of the actual payload
            let actual_sig = Self::sign_payload(payload, sig_key) & 0xffffffffc0000000;

            if actual_sig == sig_bits {
                break Ok((
                    LinkPhase::from_bits(header_meta >> 6),
                    (header_meta >> 2) & 0xf,
                ));
            }
            // wrong signature

            #[cfg(feature = "defmt")]
            defmt::trace!(
                "link: signature mismatch: expected {=u64:x}, got {=u64:x}",
                actual_sig,
                sig_bits
            );
        }
    }

    /// Ugly hack to get around lifetime issues. See the comment in `read()`.
    pub fn get_payload<PHY: PhysicalLayer>(phy: &'a PHY) -> &'a [u8] {
        &phy.rx_buffer()[5..]
    }

    fn sign_payload(payload: &[u8], sig_key: &[u8]) -> u64 {
        let mut sig = Hmac::<Sha256>::new_from_slice(sig_key).expect("HMAC should not fail");
        sig.update(payload);
        let sig_bytes: [u8; 32] = sig.finalize().into_bytes().into();

        // only use the first 5 bytes, zero-extend to 8 bytes
        u64::from_be_bytes([
            sig_bytes[0],
            sig_bytes[1],
            sig_bytes[2],
            sig_bytes[3],
            sig_bytes[4],
            0,
            0,
            0,
        ])
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{phy::PhysicalLayer, test::RunBlockingExt};
    use core::{
        error::Error,
        fmt::{Debug, Display, Formatter},
        sync::atomic::{AtomicUsize, Ordering},
    };
    use hex_literal::hex;

    #[derive(Default)]
    struct TestingPhy {
        read_bufs: &'static [&'static [u8]],
        current_read_buf: AtomicUsize,
        buf: Vec<u8>,
        sent: Vec<u8>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TestingError;

    impl Display for TestingError {
        fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
            Debug::fmt(self, f)
        }
    }

    impl Error for TestingError {}

    impl PhysicalLayer for TestingPhy {
        type Error = TestingError;

        async fn read(&mut self) -> Result<(), Self::Error> {
            match self
                .read_bufs
                .get(self.current_read_buf.load(Ordering::Relaxed))
            {
                Some(_buf) => {
                    self.current_read_buf.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
                None => Err(TestingError),
            }
        }

        fn rx_buffer(&self) -> &[u8] {
            &self.read_bufs[self.current_read_buf.load(Ordering::Relaxed) - 1]
        }

        async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error> {
            self.buf.extend_from_slice(data);
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            self.sent.extend_from_slice(&self.buf);
            self.buf.clear();
            Ok(())
        }
    }

    #[test]
    fn test_link_packet_encoding() {
        let mut phy = TestingPhy::default();

        let payload = b"this is the payload";
        let secret_key = b"secret key";
        let signature: u64 = 0x86c6662bba4d02ed & !((1u64 << 30) - 1);

        let packet = LinkPacket {
            phase: LinkPhase::Handshake,
            id: 5,
            payload,
        };

        assert_eq!(packet.write(&mut phy, secret_key).run_blocking(), Ok(()));

        let encoded: &[u8] = &phy.sent;
        assert_eq!(encoded.len(), 5 + payload.len());

        // action + id
        assert_eq!(encoded[0] & 0b11111100, 0b10_0101_00);

        let actual_sig: u64 = u64::from_be_bytes([
            encoded[0], encoded[1], encoded[2], encoded[3], encoded[4], 0, 0, 0,
        ]) << 6;

        // signature
        assert_eq!(actual_sig, signature);

        // payload
        assert_eq!(&encoded[5..], payload.as_ref());

        println!("{:x?}", actual_sig);
    }

    const LINK_PACKET_VALID: [u8; 24] = hex!("961b1998ae7468697320697320746865207061796c6f6164");
    const LINK_PACKET_BAD_SIG: [u8; 24] = hex!("932b1998ae7468697320697320746865207061796c6f6164");

    #[test]
    fn test_link_packet_decoding_bad_packets() {
        let mut phy = TestingPhy::default();
        let secret_key = b"secret key";

        phy.read_bufs = &[b"", b"short", &LINK_PACKET_BAD_SIG];
        assert!(LinkPacket::read(&mut phy, secret_key.as_ref())
            .run_blocking()
            .is_err());
    }

    #[test]
    fn test_link_packet_decoding_invalid_key() {
        let mut phy = TestingPhy::default();
        let secret_key = b"not the secret key";

        phy.read_bufs = &[&LINK_PACKET_VALID];
        assert!(LinkPacket::read(&mut phy, secret_key.as_ref())
            .run_blocking()
            .is_err());
    }

    #[test]
    fn test_link_packet_decoding_valid() {
        let mut phy = TestingPhy::default();
        let secret_key = b"secret key";

        phy.read_bufs = &[b"", b"short", &LINK_PACKET_BAD_SIG, &LINK_PACKET_VALID];

        let Ok(packet) = LinkPacket::read(&mut phy, secret_key.as_ref()).run_blocking() else {
            panic!("Failed to read valid packet");
        };

        assert!(packet.0 == LinkPhase::Handshake);
        assert_eq!(packet.1, 5);
        assert_eq!(LinkPacket::get_payload(&phy), b"this is the payload");
    }
}
