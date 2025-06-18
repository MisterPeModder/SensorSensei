use core::fmt::{Display, Formatter};

use defmt::{error, info, warn, Debug2Format};
use embassy_time::{Duration, Instant, Timer};
use protocol::{
    app::v1::{HandshakeEnd, HandshakeStart, Packet, SensorData, SensorValuePoint},
    codec::{AsyncDecoder, AsyncEncoder},
    link::v1::LinkLayer,
    phy::PhysicalLayer,
};
use thiserror::Error;

#[cfg(feature = "display-ssd1306")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};

use crate::{
    comm::link::GatewayLinkLayer, ValueSender, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR,
};

pub struct GatewayAppLayer<LINK> {
    link: LINK,
    offset: usize,
}

#[derive(Debug, Error)]
pub enum GatewayAppLayerError<LINK: core::error::Error> {
    Decoding,
    UnexpectedPacket(u8),
    IncompatibleProtocol(u8, u8),
    Timeout,
    Link(LINK),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppLayerPhase {
    Initial,
    Handshake,
    Uplink,
}

#[cfg(feature = "display-ssd1306")]
pub struct DisplayStatus {
    pub phase: AppLayerPhase,
}

#[cfg(feature = "display-ssd1306")]
pub static CURRENT_STATUS: Mutex<CriticalSectionRawMutex, DisplayStatus> =
    Mutex::new(DisplayStatus {
        phase: AppLayerPhase::Initial,
    });

/// Listens for LoRa packets in an infinite loop.
pub async fn run<PHY: PhysicalLayer>(phy: PHY, mut value_sender: ValueSender) -> ! {
    // let mut value_sender = self.value_sender.take().expect("broken: no sender");
    let link = GatewayLinkLayer::new(phy);
    let mut phase = AppLayerPhase::Initial;
    let mut app = GatewayAppLayer::new(link);

    loop {
        #[cfg(feature = "display-ssd1306")]
        {
            // update the display status
            CURRENT_STATUS.lock().await.phase = phase;
        }
        if let Err(err) = comm_cycle(&mut app, &mut phase, &mut value_sender).await {
            error!("comm error: {:?}", Debug2Format(&err));
        }
    }
}

async fn comm_cycle<LINK: LinkLayer>(
    app: &mut GatewayAppLayer<LINK>,
    phase: &mut AppLayerPhase,
    value_sender: &mut ValueSender,
) -> Result<(), GatewayAppLayerError<LINK::Error>> {
    info!("app: Waiting for sensor board request...");

    match app.read::<Packet>().await? {
        Packet::HandshakeStart(pkt) => match app_on_handshake_start(app, pkt).await {
            Ok(()) => {
                *phase = AppLayerPhase::Uplink;
                Ok(())
            }
            Err(e) => {
                *phase = AppLayerPhase::Handshake;
                Err(e)
            }
        },
        Packet::SensorData(pkt) if *phase == AppLayerPhase::Uplink => {
            app_on_sensor_data(app, value_sender, pkt).await
        }
        pkt => Err(GatewayAppLayerError::UnexpectedPacket(pkt.id())),
    }
}

async fn app_on_handshake_start<LINK: LinkLayer>(
    app: &mut GatewayAppLayer<LINK>,
    pkt: HandshakeStart,
) -> Result<(), GatewayAppLayerError<LINK::Error>> {
    if pkt.major != PROTOCOL_VERSION_MAJOR {
        return Err(GatewayAppLayerError::IncompatibleProtocol(
            pkt.major, pkt.minor,
        ));
    }
    info!("app: got handshake start");

    // FIXME: artificial delay, remove if LBT is implemented
    Timer::after(Duration::from_millis(100)).await;

    let epoch = Instant::now();
    app.emit(&Packet::HandshakeEnd(HandshakeEnd {
        major: PROTOCOL_VERSION_MAJOR,
        minor: PROTOCOL_VERSION_MINOR,
        epoch: epoch.as_millis(),
    }))
    .await?;
    app.flush().await?;

    info!("Client handshake complete, waiting for sensor data...");

    Ok(())
}

async fn app_on_sensor_data<LINK: LinkLayer>(
    app: &mut GatewayAppLayer<LINK>,
    value_sender: &mut ValueSender,
    pkt: SensorData,
) -> Result<(), GatewayAppLayerError<LINK::Error>> {
    info!("app: got sensor data");

    for _ in 0..pkt.count {
        // Send values to other thread for exporting
        if let Some(value_point) = value_sender.try_send() {
            *value_point = app.read::<SensorValuePoint>().await?;
            value_sender.send_done();
        } else {
            let value_point = app.read::<SensorValuePoint>().await?;
            warn!(
                "lora: dropping value #{=u32} (at T+{=i64}): queue is full",
                value_point.value.id(),
                value_point.time_offset
            );
        }
    }
    embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
    info!("Done receiving sensor data, sending ack");

    app.emit(&Packet::Ack).await?;
    app.flush().await?;
    Ok(())
}

impl<LINK: LinkLayer> GatewayAppLayer<LINK> {
    pub fn new(link: LINK) -> Self {
        Self { link, offset: 0 }
    }

    pub async fn flush(&mut self) -> Result<(), GatewayAppLayerError<LINK::Error>> {
        self.link
            .flush(None)
            .await
            .map_err(GatewayAppLayerError::Link)
    }
}

impl<LINK: LinkLayer> AsyncEncoder for GatewayAppLayer<LINK> {
    type Error = GatewayAppLayerError<LINK::Error>;

    async fn emit_bytes(&mut self, mut buf: &[u8]) -> Result<(), Self::Error> {
        while !buf.is_empty() {
            let written = self
                .link
                .write(None, buf)
                .await
                .map_err(GatewayAppLayerError::Link)?;
            buf = &buf[written..];
        }
        Ok(())
    }
}

impl<LINK: LinkLayer> AsyncDecoder for GatewayAppLayer<LINK> {
    type Error = GatewayAppLayerError<LINK::Error>;

    async fn read_bytes(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut bytes_read = 0usize;

        while bytes_read < buf.len() {
            let (read, _from) = self
                .link
                .read(&mut buf[bytes_read..])
                .await
                .map_err(GatewayAppLayerError::Link)?;
            self.offset += read;
            bytes_read += read;
        }
        Ok(())
    }

    fn current_offset(&self) -> usize {
        self.offset
    }

    fn decoding_error(&self) -> Self::Error {
        GatewayAppLayerError::Decoding
    }
}

impl<LINK: core::error::Error> Display for GatewayAppLayerError<LINK> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match &self {
            GatewayAppLayerError::Decoding => f.write_str("decoding error"),
            GatewayAppLayerError::Link(err) => write!(f, "{}", err),
            GatewayAppLayerError::UnexpectedPacket(id) => write!(f, "unexpected packet: {}", id),
            GatewayAppLayerError::IncompatibleProtocol(major, minor) => {
                write!(f, "incompatible protocol: {}.{}", major, minor)
            }
            GatewayAppLayerError::Timeout => f.write_str("timeout exceeded"),
        }
    }
}
