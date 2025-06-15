use defmt::{info, trace, warn};
use embassy_futures::select::Either;
use esp_hal::efuse::Efuse;
use protocol::link::v1::LinkPacket;
use protocol::{
    link::v1::{GatewayId, LinkLayer, LinkPhase, SensorBoardId},
    phy::PhysicalLayer,
};

#[derive(Copy, Clone)]
enum SensorBoardLinkPhase {
    Handshake,
    Data(SensorBoardId),
}

pub struct SensorBoardLinkLayer<PHY> {
    phase: SensorBoardLinkPhase,
    phy: PHY,
    tx_buf: heapless::Vec<u8, 64>,
    payload_start: usize,
    payload_end: usize,
}

impl<PHY: PhysicalLayer> SensorBoardLinkLayer<PHY> {
    pub fn new(phy: PHY) -> Self {
        Self {
            phase: SensorBoardLinkPhase::Handshake,
            phy,
            tx_buf: heapless::Vec::new(),
            payload_start: 0,
            payload_end: 0,
        }
    }

    async fn connect(&mut self) -> Result<SensorBoardId, PHY::Error> {
        if let SensorBoardLinkPhase::Data(id) = self.phase {
            // Already connected, no need to do anything
            return Ok(id);
        }

        loop {
            info!("link: connecting to gateway...");
            self.phase = Self::try_connect(&mut self.phy).await?;

            match self.phase {
                SensorBoardLinkPhase::Handshake => {
                    info!("link: handshake failed, trying again in 2 seconds...");
                    embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
                }
                SensorBoardLinkPhase::Data(id) => {
                    info!("link: connected to gateway with ID: {}", id.0);
                    break Ok(id);
                }
            }
        }
    }

    async fn try_connect(phy: &mut PHY) -> Result<SensorBoardLinkPhase, PHY::Error> {
        // TODO(protocol): specify that MAC address is 6 bytes
        let mac = Efuse::read_base_mac_address();

        info!("link: initiating handshake with MAC: {=[u8]:02x}", mac);
        LinkPacket {
            phase: LinkPhase::Handshake,
            id: 0,
            payload: &mac,
        }
        .write(&mut *phy, b"SECRET")
        .await?;

        info!("link: reading handshake response...");

        let res = embassy_futures::select::select(
            LinkPacket::read(&mut *phy, b"SECRET"),
            embassy_time::Timer::after(embassy_time::Duration::from_secs(5)),
        )
        .await;

        let (res_phase, res_id) = match res {
            Either::First(res) => res?,
            Either::Second(()) => {
                warn!("link: timeout while waiting for handshake response");
                return Ok(SensorBoardLinkPhase::Handshake);
            }
        };
        let payload = LinkPacket::get_payload(phy);

        Ok(if res_phase == LinkPhase::Handshake && payload == mac {
            SensorBoardLinkPhase::Data(SensorBoardId(res_id))
        } else {
            SensorBoardLinkPhase::Handshake
        })
    }

    /// Requests the next payload from the PHY, clearing the rx buffer.
    async fn read_payload(&mut self) -> Result<(), PHY::Error> {
        loop {
            let id = self.connect().await?;
            let (res_phase, res_id) = LinkPacket::read(&mut self.phy, b"SECRET").await?;

            if res_id != id.0 {
                trace!(
                    "link: received packet for different sensor board: {=u8}, expected: {=u8}",
                    res_id,
                    id.0
                );
                continue;
            }

            if res_phase != LinkPhase::Data {
                warn!("link: unexpected phase from gateway, reconnecting");
                self.phase = SensorBoardLinkPhase::Handshake;
                continue;
            }

            break Ok(());
        }
    }
}

impl<PHY: PhysicalLayer> LinkLayer for SensorBoardLinkLayer<PHY> {
    type Error = PHY::Error;
    type PeerId = GatewayId;

    async fn read(&mut self, buf: &mut [u8]) -> Result<(usize, Self::PeerId), Self::Error> {
        if self.payload_start >= self.payload_end {
            // payload is empty/consumed, read a new one
            self.read_payload().await?;
            self.payload_start = 0;
            self.payload_end = LinkPacket::get_payload(&self.phy).len();
        }

        let bytes_available = self.payload_end - self.payload_start;
        let bytes_to_copy = buf.len().min(bytes_available);

        buf[..bytes_to_copy].copy_from_slice(
            &LinkPacket::get_payload(&self.phy)
                [self.payload_start..self.payload_start + bytes_to_copy],
        );

        self.payload_start += bytes_to_copy;

        Ok((bytes_to_copy, GatewayId))
    }

    async fn write(
        &mut self,
        dest: Option<Self::PeerId>,
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

    async fn flush(&mut self, _dest: Option<Self::PeerId>) -> Result<(), Self::Error> {
        let id = self.connect().await?;
        LinkPacket {
            phase: LinkPhase::Data,
            id: id.0,
            payload: &self.tx_buf,
        }
        .write(&mut self.phy, b"SECRET")
        .await?;
        self.tx_buf.clear();
        self.phy.flush().await
    }

    fn reset(&mut self) {
        self.phase = SensorBoardLinkPhase::Handshake;
        self.payload_start = 0;
        self.payload_end = 0;
        self.tx_buf.clear();
    }
}
