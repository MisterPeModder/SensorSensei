use crate::codec::{AsyncDecode, AsyncDecoder, AsyncEncode, AsyncEncoder, ToLeb128Ext};
use core::future::Future;

/// A version 1.0 packet. ([reference])
///
/// [reference]: https://github.com/MisterPeModder/T-IOT-902/blob/master/doc/protocol.md#42-packet-types
#[repr(u8)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum Packet {
    HandshakeStart(HandshakeStart) = 0,
    HandshakeEnd(HandshakeEnd) = 1,
    Ack = 2,
    SensorData(SensorData) = 3,
    ResetConnection = 4,
}

/// Payload of `HandshakeStart` packet. ([reference])
///
/// [reference]: https://github.com/MisterPeModder/T-IOT-902/blob/master/doc/protocol.md#433-handshakestart
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct HandshakeStart {
    pub major: u8,
    pub minor: u8,
}

/// Payload of `HandshakeEnd` packet. ([reference])
///
/// [reference]: https://github.com/MisterPeModder/T-IOT-902/blob/master/doc/protocol.md#434-handshakeend
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct HandshakeEnd {
    pub major: u8,
    pub minor: u8,
    pub epoch: u64,
}

/// Payload header of the `SensorData` packet. ([reference])  
/// The actual values are separate from this struct because of buffering and allocation limitations.
///
/// [reference]: https://github.com/MisterPeModder/T-IOT-902/blob/master/doc/protocol.md#436-sensordata
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct SensorData {
    /// The number of [`SensorValuePoint`] values that constitutes this packet.
    pub count: u8,
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct SensorValuePoint {
    pub value: SensorValue,
    pub time_offset: i64,
}

#[repr(u32)]
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum SensorValue {
    Temperature(f32) = 0,
    Pressure(f32) = 1,
    Altitude(f32) = 2,
    AirQuality(f32) = 3,
    Unknown { id: u32, value_len: u32 } = u32::MAX,
}

impl Packet {
    pub const fn id(&self) -> u8 {
        unsafe {
            // SAFETY: Because `Self` is marked `repr(u8)`, its layout is a `repr(C)` `union`
            // between `repr(C)` structs, each of which has the `u8` discriminant as its first
            // field, so we can read the discriminant without offsetting the pointer.
            *core::mem::transmute::<*const Packet, *const u8>(self as *const _)
        }
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &Packet {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        encoder.emit(self.id()).await?;
        match self {
            Packet::HandshakeStart(handshake_start) => encoder.emit(handshake_start).await,
            Packet::HandshakeEnd(handshake_end) => encoder.emit(handshake_end).await,
            Packet::Ack => Ok(()),
            Packet::SensorData(sensor_data) => encoder.emit(sensor_data).await,
            Packet::ResetConnection => Ok(()),
        }
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for Packet {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let id: u8 = decoder.read().await?;
        match id {
            0 => Ok(Packet::HandshakeStart(decoder.read().await?)),
            1 => Ok(Packet::HandshakeEnd(decoder.read().await?)),
            2 => Ok(Packet::Ack),
            3 => Ok(Packet::SensorData(decoder.read().await?)),
            4 => Ok(Packet::ResetConnection),
            _ => Err(decoder.decoding_error()),
        }
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &HandshakeStart {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        encoder.emit((self.major, self.minor, 0u32)).await
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for HandshakeStart {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let (major, minor, tail_len): (u8, u8, u32) = decoder.read().await?;

        // forward compat: discard `tail_len` bytes
        decoder.read_discard(tail_len as usize).await?;
        Ok(Self { major, minor })
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &HandshakeEnd {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        let mut tail: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let epoch_len = self
            .epoch
            .to_leb128((&mut tail[0..10]).try_into().unwrap())
            .len();
        let tail_len = epoch_len;
        encoder
            .emit((self.major, self.minor, tail_len as u32))
            .await?;
        encoder.emit_bytes(&tail[..tail_len]).await
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for HandshakeEnd {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let major: u8 = decoder.read().await?;
        let minor: u8 = decoder.read().await?;
        let mut tail_len: usize = decoder.read::<u32>().await? as usize;

        let epoch: u64 = if major == 1 {
            let pos: usize = decoder.current_offset();
            let epoch: u64 = decoder.read().await?;
            let epoch_len = decoder.current_offset().wrapping_sub(pos);

            // remove already read bytes from total
            tail_len = tail_len
                .checked_sub(epoch_len)
                // if epoch_len is somehow greater than the reported payload length:
                // the sender is fake news, and this is an error
                .ok_or_else(|| decoder.decoding_error())?;
            epoch
        } else {
            0
        };

        // forward compat: discard `tail_len` bytes
        decoder
            .read_discard(tail_len.min(u32::MAX as usize))
            .await?;
        Ok(Self {
            major,
            minor,
            epoch,
        })
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for SensorData {
    fn encode(self, encoder: &mut E) -> impl Future<Output = Result<(), E::Error>> {
        encoder.emit(self.count)
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for SensorData {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        Ok(Self {
            count: decoder.read().await?,
        })
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for SensorValuePoint {
    fn encode(self, encoder: &mut E) -> impl Future<Output = Result<(), E::Error>> {
        encoder.emit((self.time_offset, self.value))
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for SensorValuePoint {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let time_offset: i64 = decoder.read().await?;
        let value: SensorValue = decoder.read().await?;
        Ok(Self { time_offset, value })
    }
}

impl SensorValue {
    pub const fn id(&self) -> u32 {
        unsafe {
            // SAFETY: Because `Self` is marked `repr(u32)`, its layout is a `repr(C)` `union`
            // between `repr(C)` structs, each of which has the `u32` discriminant as its first
            // field, so we can read the discriminant without offsetting the pointer.
            *core::mem::transmute::<*const SensorValue, *const u32>(self as *const _)
        }
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for SensorValue {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        if let SensorValue::Unknown { id, .. } = self {
            encoder.emit(id).await?;
        } else {
            encoder.emit(self.id()).await?;
        }
        match self {
            SensorValue::Temperature(value) => encoder.emit((4u32, value)).await,
            SensorValue::Pressure(value) => encoder.emit((4u32, value)).await,
            SensorValue::Altitude(value) => encoder.emit((4u32, value)).await,
            SensorValue::AirQuality(value) => encoder.emit((4u32, value)).await,
            SensorValue::Unknown { value_len, .. } => encoder.emit(value_len).await,
        }
    }
}

impl<D: AsyncDecoder + ?Sized> AsyncDecode<D> for SensorValue {
    async fn decode(decoder: &mut D) -> Result<Self, D::Error> {
        let kind: u32 = decoder.read().await?;
        let mut value_len: usize = decoder.read::<u32>().await? as usize;
        let pos: usize = decoder.current_offset();

        let value = match kind {
            0 => SensorValue::Temperature(decoder.read().await?),
            1 => SensorValue::Pressure(decoder.read().await?),
            2 => SensorValue::Altitude(decoder.read().await?),
            3 => SensorValue::AirQuality(decoder.read().await?),
            id => SensorValue::Unknown {
                id,
                value_len: value_len as u32,
            },
        };

        // remove already read bytes from total
        let actual_value_len = decoder.current_offset().wrapping_sub(pos);
        value_len = value_len
            .checked_sub(actual_value_len)
            // if the actual length is somehow greater than the reported payload length:
            // the sender is lying, and this is an error
            .ok_or_else(|| decoder.decoding_error())?;

        decoder.read_discard(value_len).await?;
        Ok(value)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{
        error::Error,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    const NOOP_RAW_WAKER_VTABLE: RawWakerVTable =
        RawWakerVTable::new(|_| NOOP_RAW_WAKER, |_| {}, |_| {}, |_| {});
    const NOOP_RAW_WAKER: RawWaker = RawWaker::new(core::ptr::null(), &NOOP_RAW_WAKER_VTABLE);
    const NOOP_WAKER: Waker = unsafe { Waker::from_raw(NOOP_RAW_WAKER) };

    trait RunBlockingExt: Future {
        /// Evaluates this future by spin blocking, not quite energy-efficient.
        fn run_blocking(mut self) -> Self::Output
        where
            Self: Sized,
        {
            let waker = &NOOP_WAKER;
            let mut this: Pin<&mut Self> = unsafe { Pin::new_unchecked(&mut self) };
            let mut cx = Context::from_waker(waker);

            loop {
                if let Poll::Ready(res) = this.as_mut().poll(&mut cx) {
                    break res;
                }
                core::hint::spin_loop();
            }
        }
    }

    impl<F: Future> RunBlockingExt for F {}

    #[derive(Default)]
    struct AllocatingTestCodec {
        buf: Vec<u8>,
        offset: usize,
    }

    impl AsyncEncoder for AllocatingTestCodec {
        type Error = std::collections::TryReserveError;

        async fn emit_bytes(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
            self.buf.try_reserve(buf.len())?;
            self.buf.extend_from_slice(buf);
            Ok(())
        }
    }

    impl AsyncDecoder for AllocatingTestCodec {
        type Error = Arc<dyn std::error::Error>;

        async fn read_bytes(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
            if buf.len() > self.buf.len() {
                Err(Box::<dyn Error>::from(format!(
                    "tried to read {} bytes from buffer of {} bytes",
                    buf.len(),
                    self.buf.len()
                ))
                .into())
            } else {
                let mut new_buf = self.buf.split_off(buf.len());
                std::mem::swap(&mut new_buf, &mut self.buf);
                buf.copy_from_slice(&new_buf);
                self.offset += buf.len();
                Ok(())
            }
        }

        fn current_offset(&self) -> usize {
            self.offset
        }

        fn decoding_error(&self) -> Self::Error {
            Box::<dyn Error>::from("decoding error").into()
        }
    }

    impl AllocatingTestCodec {
        pub fn emit_alloc<T: AsyncEncode<Self>>(
            &mut self,
            value: T,
        ) -> Result<Box<[u8]>, <Self as AsyncEncoder>::Error> {
            self.buf.clear();
            self.emit(value).run_blocking()?;
            Ok(self.buf.clone().into_boxed_slice())
        }
    }

    #[test]
    fn test_codec_uleb128() {
        let mut codec = AllocatingTestCodec::default();
        let values: &[(u128, &[u8])] = &[
            (0, &[0]),
            (12, &[12]),
            (275, &[0x93, 0x02]),
            (71921, &[0xf1, 0xb1, 0x04]),
            (5626730, &[0xea, 0xb6, 0xd7, 0x02]),
            (3721843041, &[0xe1, 0xa2, 0xdb, 0xee, 0x0d]),
            (u32::MAX as u128, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (41705795455, &[0xff, 0xde, 0xef, 0xae, 0x9b, 0x01]),
            (
                u64::MAX as u128,
                &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
            ),
        ];

        for &(to_emit, expected) in values {
            if let Ok(u32v) = u32::try_from(to_emit) {
                assert_eq!(&codec.emit_alloc(u32v).unwrap()[..], expected);
                let pos = codec.current_offset();
                assert_eq!(u32v, codec.read::<u32>().run_blocking().unwrap());
                assert_eq!(codec.current_offset() - pos, expected.len());
            }
            if let Ok(u64v) = u64::try_from(to_emit) {
                assert_eq!(&codec.emit_alloc(u64v).unwrap()[..], expected);
                let pos = codec.current_offset();
                assert_eq!(u64v, codec.read::<u64>().run_blocking().unwrap());
                assert_eq!(codec.current_offset() - pos, expected.len());
            }
        }
    }

    #[test]
    fn test_codec_sleb128() {
        let mut codec = AllocatingTestCodec::default();
        let values: &[(i128, &[u8])] = &[
            (0, &[0]),
            (-1, &[0x7f]),
            (-12, &[0x74]),
            (-275, &[0xed, 0x7d]),
            (-71921, &[0x8f, 0xce, 0x7b]),
            (-5626730, &[0x96, 0xc9, 0xa8, 0x7d]),
            (-3721843041, &[0x9f, 0xdd, 0xa4, 0x91, 0x72]),
            (i32::MIN as i128, &[0x80, 0x80, 0x80, 0x80, 0x78]),
            (-41705795455, &[0x81, 0xa1, 0x90, 0xd1, 0xe4, 0x7e]),
            (
                i64::MIN as i128,
                &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7f],
            ),
        ];

        for &(to_emit, expected) in values {
            if let Ok(i64v) = i64::try_from(to_emit) {
                assert_eq!(&codec.emit_alloc(i64v).unwrap()[..], expected);
                let pos = codec.current_offset();
                assert_eq!(i64v, codec.read::<i64>().run_blocking().unwrap());
                assert_eq!(codec.current_offset() - pos, expected.len());
            }
        }
    }

    #[test]
    fn test_codec_f32() {
        let mut codec = AllocatingTestCodec::default();

        assert_eq!(
            &codec.emit_alloc(123.456f32).unwrap()[..],
            [0x79, 0xe9, 0xf6, 0x42]
        );
        assert_eq!(codec.read::<f32>().run_blocking().unwrap(), 123.456f32);
        assert_eq!(codec.current_offset(), 4);
        assert_eq!(
            &codec.emit_alloc(22.3f32).unwrap()[..],
            [0x66, 0x66, 0xb2, 0x41]
        );
        assert_eq!(codec.read::<f32>().run_blocking().unwrap(), 22.3f32);
        assert_eq!(codec.current_offset(), 8);
    }

    #[test]
    fn test_decode_unknown_packet() {
        let mut codec = AllocatingTestCodec::default();
        let encoded = [0x99, 0x01, 0x15, 0x00];

        codec.buf.extend(&encoded);
        assert!(codec.read::<Packet>().run_blocking().is_err());
        assert_eq!(codec.current_offset(), 1);
    }

    #[test]
    fn test_codec_handshake_start_packet() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::HandshakeStart(HandshakeStart {
            major: 1,
            minor: 21,
        });
        let encoded = [0x00, 0x01, 0x15, 0x00];

        assert_eq!(&codec.emit_alloc(&packet).unwrap()[..], encoded,);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_decode_handshake_start_packet_trailing_bytes() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::HandshakeStart(HandshakeStart {
            major: 1,
            minor: 21,
        });
        let encoded = [0x00, 0x01, 0x15, 0x03, 0xca, 0xfe, 0x99];

        codec.buf.extend(&encoded);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_codec_handshake_end_packet() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::HandshakeEnd(HandshakeEnd {
            major: 1,
            minor: 0,
            epoch: 1744854025,
        });
        let encoded = [0x01, 0x01, 0x00, 0x05, 0x89, 0xb8, 0x81, 0xc0, 0x6];

        assert_eq!(&codec.emit_alloc(&packet).unwrap()[..], encoded,);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_decode_handshake_end_packet_trailing_bytes() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::HandshakeEnd(HandshakeEnd {
            major: 1,
            minor: 21,
            epoch: 1744854025,
        });
        let encoded = [
            0x01, 0x01, 0x15, 0x08, 0x89, 0xb8, 0x81, 0xc0, 0x6, 0x01, 0x02, 0x03,
        ];

        codec.buf.extend(&encoded);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_decode_handshake_end_from_the_future() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::HandshakeEnd(HandshakeEnd {
            major: 2,
            minor: 3,
            epoch: 0,
        });
        let encoded = [0x01, 0x02, 0x03, 0x02, 0xba, 0xbe];

        codec.buf.extend(&encoded);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_codec_ack_packet() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::Ack;
        let encoded = [0x02];

        assert_eq!(&codec.emit_alloc(&packet).unwrap()[..], encoded);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_codec_sensor_data_packet_empty() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::SensorData(SensorData { count: 0 });
        let encoded = [0x03, 0x00];

        assert_eq!(&codec.emit_alloc(&packet).unwrap()[..], encoded);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }

    #[test]
    fn test_codec_sensor_data_packet_normal() {
        let mut codec = AllocatingTestCodec::default();

        let values: &[(SensorValuePoint, &[u8])] = &[
            (
                SensorValuePoint {
                    value: SensorValue::Temperature(22.3),
                    time_offset: -35,
                },
                &[0x5d, 0x00, 0x04, 0x66, 0x66, 0xb2, 0x41],
            ),
            (
                SensorValuePoint {
                    value: SensorValue::Pressure(1.01),
                    time_offset: 2,
                },
                &[0x02, 0x01, 0x04, 0xae, 0x47, 0x81, 0x3f],
            ),
            (
                SensorValuePoint {
                    value: SensorValue::Altitude(0.9),
                    time_offset: 3,
                },
                &[0x03, 0x02, 0x04, 0x66, 0x66, 0x66, 0x3f],
            ),
            (
                SensorValuePoint {
                    value: SensorValue::AirQuality(0.52),
                    time_offset: 6,
                },
                &[0x06, 0x03, 0x04, 0xb8, 0x1e, 0x05, 0x3f],
            ),
            (
                SensorValuePoint {
                    value: SensorValue::Unknown {
                        id: 999,
                        value_len: 0,
                    },
                    time_offset: 9,
                },
                &[0x09, 0xe7, 0x07, 0x00],
            ),
        ];

        let packet_header = Packet::SensorData(SensorData {
            count: values.len() as u8,
        });
        let encoded_packet_header = [0x03, 0x05];

        // Header
        assert_eq!(
            &codec.emit_alloc(&packet_header).unwrap()[..],
            &encoded_packet_header
        );
        assert_eq!(
            codec.read::<Packet>().run_blocking().unwrap(),
            packet_header
        );
        assert_eq!(codec.current_offset(), encoded_packet_header.len());

        // Values
        for (value, encoded) in values {
            assert_eq!(&codec.emit_alloc(value).unwrap()[..], *encoded);
            let pos = codec.current_offset();
            assert_eq!(
                &codec.read::<SensorValuePoint>().run_blocking().unwrap(),
                value
            );
            assert_eq!(codec.current_offset() - pos, encoded.len());
        }
    }

    #[test]
    fn test_codec_sensor_data_packet_unknown_read_tail() {
        let mut codec = AllocatingTestCodec::default();

        let value = SensorValuePoint {
            value: SensorValue::Unknown {
                id: 999,
                value_len: 3,
            },
            time_offset: 9,
        };
        let encoded = [0x09, 0xe7, 0x07, 0x03];

        assert_eq!(&codec.emit_alloc(value).unwrap()[..], encoded);
        let pos = codec.current_offset();
        // add tail data back
        codec.buf.extend(&[0x00, 0x00, 0x00]);
        assert_eq!(
            codec.read::<SensorValuePoint>().run_blocking().unwrap(),
            value
        );
        assert_eq!(codec.current_offset() - pos, encoded.len() + 3);
    }

    #[test]
    fn test_codec_reset_connection_packet() {
        let mut codec = AllocatingTestCodec::default();
        let packet = Packet::ResetConnection;
        let encoded = [0x04];

        assert_eq!(&codec.emit_alloc(&packet).unwrap()[..], &encoded);
        assert_eq!(codec.read::<Packet>().run_blocking().unwrap(), packet);
        assert_eq!(codec.current_offset(), encoded.len());
    }
}
