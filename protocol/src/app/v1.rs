use core::future::Future;

use crate::{
    app::HandshakeGeneric,
    codec::{AsyncEncode, AsyncEncoder, ToLeb128Ext},
};

pub enum Packet<'v> {
    HandshakeStart(HandshakeStart),
    HandshakeEnd(HandshakeEnd),
    Ack,
    SensorData(&'v [SensorData<'v>]),
}

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum PacketKind {
    HandshakeStart = 0,
    HandshakeEnd = 1,
    Ack = 2,
    SensorData = 3,
}

pub struct HandshakeStart {
    pub major: u8,
    pub minor: u8,
}

pub struct HandshakeEnd {
    pub major: u8,
    pub minor: u8,
    pub client_id: u8,
    pub epoch: u64,
}

pub struct SensorData<'v> {
    pub value: SensorValue<'v>,
    pub time_offset: i64,
}

#[derive(Clone, Copy)]
pub enum SensorValue<'v> {
    Temperature(f32),
    Pressure(f32),
    Altitude(f32),
    Unknown { id: u32, value: &'v [u8] },
}

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum SensorValueKind {
    Temperature = 0,
    Pressure = 1,
    Altitude = 2,
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &Packet<'_> {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        match self {
            Packet::HandshakeStart(handshake_start) => {
                encoder
                    .emit((PacketKind::HandshakeStart, handshake_start))
                    .await
            }
            Packet::HandshakeEnd(handshake_end) => {
                encoder
                    .emit((PacketKind::HandshakeEnd, handshake_end))
                    .await
            }
            Packet::Ack => encoder.emit(PacketKind::Ack).await,
            &Packet::SensorData(sensor_data) => {
                encoder
                    .emit((PacketKind::SensorData, sensor_data.len() as u8))
                    .await?;
                encoder
                    .emit_from_iter(&sensor_data[..sensor_data.len().min(u8::MAX as usize)])
                    .await
            }
        }
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &HandshakeStart {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        encoder
            .emit(&HandshakeGeneric {
                major: self.major,
                minor: self.minor,
                tail: &[],
            })
            .await
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &HandshakeEnd {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        let mut tail: [u8; 11] = [self.client_id, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let epoch_len = self
            .epoch
            .to_leb128((&mut tail[1..11]).try_into().unwrap())
            .len();
        encoder
            .emit(&HandshakeGeneric {
                major: self.major,
                minor: self.minor,
                tail: &tail[..epoch_len + 1],
            })
            .await
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for PacketKind {
    fn encode(self, encoder: &mut E) -> impl Future<Output = Result<(), E::Error>> {
        encoder.emit(self as u8)
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for &SensorData<'_> {
    fn encode(self, encoder: &mut E) -> impl Future<Output = Result<(), E::Error>> {
        encoder.emit((self.time_offset, self.value))
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for SensorValue<'_> {
    async fn encode(self, encoder: &mut E) -> Result<(), E::Error> {
        match self {
            SensorValue::Temperature(value) => {
                encoder
                    .emit((SensorValueKind::Temperature, 4u32, value))
                    .await
            }
            SensorValue::Pressure(value) => {
                encoder.emit((SensorValueKind::Pressure, 4u32, value)).await
            }
            SensorValue::Altitude(value) => {
                encoder.emit((SensorValueKind::Altitude, 4u32, value)).await
            }
            SensorValue::Unknown { id, value } => {
                encoder.emit((id, value.len() as u32)).await?;
                encoder
                    .emit_bytes(&value[..value.len().min(u32::MAX as usize)])
                    .await
            }
        }
    }
}

impl<E: AsyncEncoder + ?Sized> AsyncEncode<E> for SensorValueKind {
    fn encode(self, encoder: &mut E) -> impl Future<Output = Result<(), E::Error>> {
        AsyncEncode::encode(self as u32, encoder)
    }
}

#[cfg(test)]
mod test {
    use std::{
        pin::Pin,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    use super::*;

    const NOOP_RAW_WAKER_VTABLE: RawWakerVTable =
        RawWakerVTable::new(|_| NOOP_RAW_WAKER, |_| {}, |_| {}, |_| {});
    const NOOP_RAW_WAKER: RawWaker = RawWaker::new(core::ptr::null(), &NOOP_RAW_WAKER_VTABLE);
    const NOOP_WAKER: Waker = unsafe { Waker::from_raw(NOOP_RAW_WAKER) };

    trait RunBlockingExt: Future {
        /// Evalutes this future by spin blocking, not quite energy efficient.
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
    struct AllocatingTestEncoder(pub Vec<u8>);

    impl AsyncEncoder for AllocatingTestEncoder {
        type Error = std::collections::TryReserveError;

        async fn emit_bytes(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
            self.0.try_reserve(buf.len())?;
            self.0.extend_from_slice(buf);
            Ok(())
        }
    }

    impl AllocatingTestEncoder {
        pub fn emit_alloc<T: AsyncEncode<Self>>(
            &mut self,
            value: T,
        ) -> Result<Box<[u8]>, <Self as AsyncEncoder>::Error> {
            self.0.clear();
            self.emit(value).run_blocking()?;
            Ok(self.0.clone().into_boxed_slice())
        }
    }

    #[test]
    fn test_encoder_emit_uleb128() {
        let mut encoder = AllocatingTestEncoder::default();
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
                let actual = encoder.emit_alloc(u32v).unwrap();
                assert_eq!(
                    &actual[..],
                    expected,
                    "emit {}u32: expected {:?}, got {:?}",
                    u32v,
                    expected,
                    actual
                );
            }

            if let Ok(i64v) = i64::try_from(to_emit) {
                let actual = encoder.emit_alloc(i64v).unwrap();
                assert_eq!(
                    &actual[..],
                    expected,
                    "emit {}i64: expected {:?}, got {:?}",
                    i64v,
                    expected,
                    actual
                );
            }

            if let Ok(u64v) = u64::try_from(to_emit) {
                let actual = encoder.emit_alloc(u64v).unwrap();
                assert_eq!(
                    &actual[..],
                    expected,
                    "emit {}u64: expected {:?}, got {:?}",
                    u64v,
                    expected,
                    actual
                );
            }
        }
    }

    #[test]
    fn test_encoder_emit_sleb128() {
        let mut encoder = AllocatingTestEncoder::default();
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
                let actual = encoder.emit_alloc(i64v).unwrap();
                assert_eq!(
                    &actual[..],
                    expected,
                    "emit {}i64: expected {:?}, got {:?}",
                    i64v,
                    expected,
                    actual
                );
            }
        }
    }

    #[test]
    fn test_encoder_emit_f32() {
        let mut encoder = AllocatingTestEncoder::default();

        assert_eq!(
            &encoder.emit_alloc(123.456f32).unwrap()[..],
            [0x79, 0xe9, 0xf6, 0x42]
        );
        assert_eq!(
            &encoder.emit_alloc(22.3f32).unwrap()[..],
            [0x66, 0x66, 0xb2, 0x41]
        );
    }

    #[test]
    fn test_encoder_emit_packets() {
        let mut encoder = AllocatingTestEncoder::default();

        assert_eq!(
            &encoder
                .emit_alloc(&Packet::HandshakeStart(HandshakeStart {
                    major: 1,
                    minor: 21,
                }))
                .unwrap()[..],
            [0x00, 0x01, 0x15, 0x00]
        );
        assert_eq!(
            &encoder
                .emit_alloc(&Packet::HandshakeEnd(HandshakeEnd {
                    major: 1,
                    minor: 21,
                    client_id: 5,
                    epoch: 1744854025,
                }))
                .unwrap()[..],
            [0x01, 0x01, 0x15, 0x06, 0x05, 0x89, 0xb8, 0x81, 0xc0, 0x6]
        );
        assert_eq!(&encoder.emit_alloc(&Packet::Ack).unwrap()[..], [0x02]);
    }

    #[test]
    fn test_encoder_emit_sensor_data_packets() {
        let mut encoder = AllocatingTestEncoder::default();

        assert_eq!(
            &encoder.emit_alloc(&Packet::SensorData(&[])).unwrap()[..],
            [0x03, 0x00]
        );

        assert_eq!(
            &encoder
                .emit_alloc(&Packet::SensorData(&[
                    SensorData {
                        value: SensorValue::Temperature(22.3),
                        time_offset: -35
                    },
                    SensorData {
                        value: SensorValue::Pressure(1.01),
                        time_offset: 2,
                    },
                    SensorData {
                        value: SensorValue::Altitude(0.9),
                        time_offset: 3,
                    },
                    SensorData {
                        value: SensorValue::Unknown {
                            id: 999,
                            value: &[0xca, 0xfe]
                        },
                        time_offset: 9,
                    }
                ]))
                .unwrap()[..],
            [
                // Header
                0x03, // packet id
                0x04, // values count
                // Value 1
                0x5d, 0x00, 0x04, 0x66, 0x66, 0xb2, 0x41, //
                // Value 2
                0x02, 0x01, 0x04, 0xae, 0x47, 0x81, 0x3f, //
                // Value 3
                0x03, 0x02, 0x04, 0x66, 0x66, 0x66, 0x3f, //
                // Value 4
                0x09, 0xe7, 0x07, 0x02, 0xca, 0xfe
            ]
        );
    }
}
