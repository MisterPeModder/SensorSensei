use core::fmt::{Display, Formatter};

use defmt::{error, info, warn, Display2Format};
use embassy_futures::select::Either;
use embassy_time::{Duration, Instant, Timer};
use heapless::spsc::Consumer;
use protocol::{
    app::v1::{HandshakeEnd, HandshakeStart, Packet, SensorData, SensorValue, SensorValuePoint},
    codec::{AsyncDecoder, AsyncEncoder},
    link::v1::LinkLayer,
};
use thiserror::Error;

use crate::{
    comm::link::SensorBoardLinkLayer, lora::LoraController, PROTOCOL_VERSION_MAJOR,
    PROTOCOL_VERSION_MINOR,
};

pub const VALUES_QUEUE_SIZE: usize = 4;
pub const VALUES_MEASURE_INTERVAL: u64 = 10;
pub const VALUES_SEND_INTERVAL: u64 = 5;

pub struct SensorBoardAppLayer<LINK> {
    link: LINK,
    offset: usize,
}

#[derive(Debug, Error)]
pub enum SensorBoardAppLayerError<LINK: core::error::Error> {
    Decoding,
    UnexpectedPacket(u8),
    IncompatibleProtocol(u8, u8),
    Timeout,
    Link(LINK),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppLayerPhase {
    Handshake,
    Uplink { sensor_epoch: Instant, diff: i64 },
}

pub async fn run(
    lora: LoraController,
    mut consumer: Consumer<'static, SensorValue, VALUES_QUEUE_SIZE>,
) -> ! {
    let link = SensorBoardLinkLayer::new(lora);
    let mut phase = AppLayerPhase::Handshake;
    let mut app = SensorBoardAppLayer::new(link);

    loop {
        match comm_cycle(&mut app, &mut phase, &mut consumer).await {
            Err(SensorBoardAppLayerError::Timeout) => {
                warn!("app: Timeout exceeded, re-initiating handshake...");
                app.reset();
                phase = AppLayerPhase::Handshake;
            }
            Err(err) => {
                error!("app: comm error: {}", Display2Format(&err));
            }
            _ => (),
        }
    }
}

async fn comm_cycle<LINK: LinkLayer>(
    app: &mut SensorBoardAppLayer<LINK>,
    phase: &mut AppLayerPhase,
    consumer: &mut Consumer<'static, SensorValue, VALUES_QUEUE_SIZE>,
) -> Result<(), SensorBoardAppLayerError<LINK::Error>> {
    match phase {
        AppLayerPhase::Handshake => {
            let (sensor_epoch, diff) = app_initiate_handshake(app).await?;
            *phase = AppLayerPhase::Uplink { sensor_epoch, diff };
            Ok(())
        }
        AppLayerPhase::Uplink { sensor_epoch, diff } => {
            app_send_values(app, consumer, *sensor_epoch, *diff).await
        }
    }
}

async fn app_initiate_handshake<LINK: LinkLayer>(
    app: &mut SensorBoardAppLayer<LINK>,
) -> Result<(Instant, i64), SensorBoardAppLayerError<LINK::Error>> {
    info!("Initiating handshake...");

    app.emit(&Packet::HandshakeStart(HandshakeStart {
        major: PROTOCOL_VERSION_MAJOR,
        minor: PROTOCOL_VERSION_MINOR,
    }))
    .await?;
    app.flush().await?;
    info!("Handshake initiated, waiting for handshake end...");

    let res = embassy_futures::select::select(
        app.read::<Packet>(),
        embassy_time::Timer::after(embassy_time::Duration::from_secs(5)),
    )
    .await;

    let gw_epoch = match res {
        Either::First(Ok(Packet::HandshakeEnd(HandshakeEnd { major, minor, .. })))
            if major != PROTOCOL_VERSION_MAJOR || minor != PROTOCOL_VERSION_MINOR =>
        {
            return Err(SensorBoardAppLayerError::IncompatibleProtocol(major, minor))
        }
        Either::First(Ok(Packet::HandshakeEnd(HandshakeEnd { epoch, .. }))) => {
            Instant::from_millis(epoch)
        }
        Either::First(Ok(pkt)) => return Err(SensorBoardAppLayerError::UnexpectedPacket(pkt.id())),
        Either::First(Err(e)) => return Err(e),
        Either::Second(()) => return Err(SensorBoardAppLayerError::Timeout),
    };

    info!("Gateway epoch millis: {}", gw_epoch.as_millis());
    let s_epoch = Instant::now();
    let diff = (s_epoch.as_micros() as i64).wrapping_sub(gw_epoch.as_micros() as i64);
    info!(
        "Sensor epoch millis: {} (diff = {}us)",
        s_epoch.as_millis(),
        diff
    );

    Ok((s_epoch, diff))
}

async fn app_send_values<LINK: LinkLayer>(
    app: &mut SensorBoardAppLayer<LINK>,
    consumer: &mut Consumer<'static, SensorValue, VALUES_QUEUE_SIZE>,
    sensor_epoch: Instant,
    diff: i64,
) -> Result<(), SensorBoardAppLayerError<LINK::Error>> {
    let mut values: heapless::Vec<SensorValue, VALUES_QUEUE_SIZE> = heapless::Vec::new();
    while let Some(value) = consumer.dequeue() {
        // SAFETY: the queue and the vec have the same max size (VALUES_QUEUE_SIZE)
        unsafe { values.push_unchecked(value) }
    }

    if !values.is_empty() {
        info!("Sending {} values...", values.len());
        let time_offset: i64 = (sensor_epoch.elapsed().as_micros() as i64 - diff) / 1_000_000;

        // FIXME: artificial delay, remove if LBT is implemented
        Timer::after(Duration::from_millis(1000)).await;
        app.emit(&Packet::SensorData(SensorData {
            count: values.len() as u8,
        }))
        .await?;

        for value in values {
            app.emit(SensorValuePoint { value, time_offset }).await?;
        }
        app.flush().await?;

        info!("Waiting for ack...");

        let res = embassy_futures::select::select(
            app.read::<Packet>(),
            embassy_time::Timer::after(embassy_time::Duration::from_secs(5)),
        )
        .await;

        match res {
            Either::First(Ok(Packet::Ack)) => {}
            Either::First(Ok(pkt)) => {
                return Err(SensorBoardAppLayerError::UnexpectedPacket(pkt.id()))
            }
            Either::First(Err(e)) => return Err(e),
            Either::Second(()) => return Err(SensorBoardAppLayerError::Timeout),
        }
    }

    embassy_time::Timer::after(embassy_time::Duration::from_secs(VALUES_SEND_INTERVAL)).await;

    Ok(())
}

impl<LINK: LinkLayer> SensorBoardAppLayer<LINK> {
    pub fn new(link: LINK) -> Self {
        Self { link, offset: 0 }
    }

    pub fn reset(&mut self) {
        self.link.reset();
        self.offset = 0;
    }

    pub async fn flush(&mut self) -> Result<(), SensorBoardAppLayerError<LINK::Error>> {
        self.link
            .flush(None)
            .await
            .map_err(SensorBoardAppLayerError::Link)
    }
}

impl<LINK: LinkLayer> AsyncEncoder for SensorBoardAppLayer<LINK> {
    type Error = SensorBoardAppLayerError<LINK::Error>;

    async fn emit_bytes(&mut self, mut buf: &[u8]) -> Result<(), Self::Error> {
        while !buf.is_empty() {
            let written = self
                .link
                .write(None, buf)
                .await
                .map_err(SensorBoardAppLayerError::Link)?;
            buf = &buf[written..];
        }
        Ok(())
    }
}

impl<LINK: LinkLayer> AsyncDecoder for SensorBoardAppLayer<LINK> {
    type Error = SensorBoardAppLayerError<LINK::Error>;

    async fn read_bytes(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut bytes_read = 0usize;

        while bytes_read < buf.len() {
            let (read, _from) = self
                .link
                .read(&mut buf[bytes_read..])
                .await
                .map_err(SensorBoardAppLayerError::Link)?;
            self.offset += read;
            bytes_read += read;
        }
        Ok(())
    }

    fn current_offset(&self) -> usize {
        self.offset
    }

    fn decoding_error(&self) -> Self::Error {
        SensorBoardAppLayerError::Decoding
    }
}

impl<LINK: core::error::Error> Display for SensorBoardAppLayerError<LINK> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match &self {
            SensorBoardAppLayerError::Decoding => f.write_str("decoding error"),
            SensorBoardAppLayerError::Link(err) => write!(f, "{}", err),
            SensorBoardAppLayerError::UnexpectedPacket(id) => {
                write!(f, "unexpected packet: {}", id)
            }
            SensorBoardAppLayerError::IncompatibleProtocol(major, minor) => {
                write!(f, "incompatible protocol: {}.{}", major, minor)
            }
            SensorBoardAppLayerError::Timeout => f.write_str("timeout exceeded"),
        }
    }
}
