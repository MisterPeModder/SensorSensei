use defmt::{info, trace, warn};
use protocol::{
    link::v1::{LinkLayer, LinkPacket, LinkPhase, SensorBoardId},
    phy::PhysicalLayer,
};

use crate::lora::LORA_RX_BUF_SIZE;

pub struct GatewayLinkLayer<PHY> {
    phase: LinkPhase,
    curr_sensor_id: SensorBoardId,
    phy: PHY,
    tx_buf: heapless::Vec<u8, 64>,
    payload_start: usize,
    payload_end: usize,
}

impl<PHY: PhysicalLayer> GatewayLinkLayer<PHY> {
    pub fn new(phy: PHY) -> Self {
        Self {
            phase: LinkPhase::Handshake,
            curr_sensor_id: SensorBoardId(15),
            phy,
            tx_buf: heapless::Vec::new(),
            payload_start: 0,
            payload_end: 0,
        }
    }

    async fn handle_inbound_handshake(&mut self) -> Result<(), PHY::Error> {
        info!("link: sensor board handshake received");
        let payload =
            heapless::Vec::<u8, LORA_RX_BUF_SIZE>::from_slice(LinkPacket::get_payload(&self.phy))
                .unwrap();
        self.curr_sensor_id = SensorBoardId((self.curr_sensor_id.0.wrapping_add(1)) & 0x0F);

        // FIXME: artificial delay, remove if LBT is implemented
        embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
        info!(
            "link: replying to sensor board handshake with id {=u8} and MAC {=[u8]:02x}",
            self.curr_sensor_id.0, &payload
        );
        LinkPacket {
            phase: LinkPhase::Handshake,
            id: self.curr_sensor_id.0,
            payload: &payload,
        }
        .write(&mut self.phy, b"SECRET")
        .await
    }

    /// Requests the next payload from the PHY, clearing the rx buffer.
    async fn read_payload(&mut self) -> Result<(), PHY::Error> {
        loop {
            let (res_phase, res_id) = LinkPacket::read(&mut self.phy, b"SECRET").await?;

            if res_phase == LinkPhase::Handshake {
                info!("link: GatewayLinkLayer::read_payload(), inbound handshake");
                self.handle_inbound_handshake().await?;
                continue;
            }

            if res_id != self.curr_sensor_id.0 {
                trace!(
                    "link: received packet for different sensor board: {=u8}, expected: {=u8}",
                    res_id,
                    self.curr_sensor_id.0
                );
                continue;
            }

            if res_phase != LinkPhase::Data {
                warn!("link: unexpected phase from gateway, reconnecting");
                self.phase = LinkPhase::Handshake;
                continue;
            }

            break Ok(());
        }
    }
}

impl<PHY: PhysicalLayer> LinkLayer for GatewayLinkLayer<PHY> {
    type Error = PHY::Error;
    type PeerId = SensorBoardId;

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
        Ok((bytes_to_copy, self.curr_sensor_id))
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
        // FIXME: "hardcoded" destination, should be set by the app layer
        let dest = self.curr_sensor_id.0;
        info!("link: flushing");
        LinkPacket {
            phase: LinkPhase::Data,
            id: dest,
            payload: &self.tx_buf,
        }
        .write(&mut self.phy, b"SECRET")
        .await?;
        self.tx_buf.clear();
        self.phy.flush().await
    }

    fn reset(&mut self) {
        info!("link: resetting");
        self.phase = LinkPhase::Handshake;
        self.curr_sensor_id = SensorBoardId(15);
        self.payload_start = 0;
        self.payload_end = 0;
        self.tx_buf.clear();
    }
}
