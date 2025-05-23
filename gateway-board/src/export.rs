//! Sensor data exporting

use crate::{
    net::http::{HttpBody, HttpClient, HttpClientError, HttpMethod},
    ValueReceiver,
};
use defmt::{error, info, Debug2Format};
use embassy_net::Stack;
use protocol::app::v1::{SensorValue, SensorValuePoint};

pub trait ValuesExporter {
    async fn export(
        &self,
        client: &mut HttpClient<'_>,
        values: &[SensorValuePoint],
    ) -> Result<(), HttpClientError>;
}

pub struct SensorCommunityExporter;

/// Attempts to fetch as many values as possible from `receiver` until either the buffer is full or the channel is empty.
pub async fn collect_values<'a, const N: usize>(
    buf: &'a mut heapless::Vec<SensorValuePoint, N>,
    receiver: &mut ValueReceiver,
) -> &'a [SensorValuePoint] {
    info!("export: waiting for values");
    buf.clear();

    // wait until at least one value is received
    buf.push(*receiver.receive().await).ok();
    receiver.receive_done();

    while !buf.is_full() {
        match receiver.try_receive() {
            Some(value) => {
                buf.push(*value).ok();
                receiver.receive_done();
            }
            None => {
                break;
            }
        }
    }
    buf.as_slice()
}

/// Exports the given values using all exporters
pub async fn export_to_all(stack: Stack<'_>, values: &[SensorValuePoint]) {
    info!("export: waiting for network");
    stack.wait_link_up().await;

    let mut client = HttpClient::new(stack);
    let ex = SensorCommunityExporter;

    if let Err(e) = ex.export(&mut client, values).await {
        error!("export: sensor.community: error: {}", Debug2Format(&e));
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum SensorCommunitySensor {
    // sensor.community expects certain "pin" values for each sensor type
    ParticulateMatter = 1,
    TemperaturePressure = 3,
}

impl SensorCommunitySensor {
    fn supports_value(self, value: SensorValue) -> bool {
        match self {
            SensorCommunitySensor::ParticulateMatter => {
                matches!(value, SensorValue::AirQuality(_))
            }
            SensorCommunitySensor::TemperaturePressure => matches!(
                value,
                SensorValue::Temperature(_) | SensorValue::Pressure(_)
            ),
        }
    }
}

impl ValuesExporter for SensorCommunityExporter {
    async fn export(
        &self,
        client: &mut HttpClient<'_>,
        values: &[SensorValuePoint],
    ) -> Result<(), HttpClientError> {
        Self::export_by_sensor(client, SensorCommunitySensor::ParticulateMatter, values).await?;
        Self::export_by_sensor(client, SensorCommunitySensor::TemperaturePressure, values).await
    }
}

impl SensorCommunityExporter {
    async fn export_by_sensor(
        client: &mut HttpClient<'_>,
        sensor: SensorCommunitySensor,
        values: &[SensorValuePoint],
    ) -> Result<(), HttpClientError> {
        use core::fmt::Write;

        if values
            .iter()
            .filter(|&&v| sensor.supports_value(v.value))
            .count()
            == 0
        {
            // don't send empty requests
            return Ok(());
        }

        let mut header_buf: heapless::String<10> = heapless::String::new();

        let mut req = client
            .request(
                HttpMethod::Post,
                "api.sensor.community",
                80u16,
                "/v1/push-sensor-data/",
            )
            .await?;

        req.header("Content-Type", "application/json").await?;
        req.header("User-Agent", "NRZ-2021-134-B4-ESP32/4123/4123")
            .await?;
        req.header("X-Sensor", "esp32-32344").await?;

        _ = write!(&mut header_buf, "{}", sensor as u8);
        req.header("X-Pin", &header_buf).await?;

        req.body().extend_from_slice(br#"{"sensordatavalues":["#);

        let mut exported_count: u32 = 0;

        for value in values.iter().copied() {
            if !sensor.supports_value(value.value) {
                continue;
            }
            Self::write_value_to_body(req.body(), value.value, exported_count == 0);
            exported_count += 1;
        }
        req.body().extend_from_slice(b"]}");

        let response = req.finish().await?;

        if response.status() < 200 || response.status() >= 300 {
            error!(
                "export: sensor.community: request failed: {=u16}",
                response.status()
            );
        } else {
            info!(
                "export: sensor.community: successfully exported {=u32} value(s)",
                exported_count
            );
        }
        Ok(())
    }

    fn write_value_to_body(body_buf: &mut HttpBody, value: SensorValue, first_value: bool) {
        use core::fmt::Write;

        if !first_value {
            body_buf.push(b',');
        }

        let _ = match value {
            SensorValue::Temperature(v) => {
                write!(body_buf, r#"{{"value":{v},"value_type":"temperature"}}"#)
            }
            SensorValue::Pressure(v) => {
                write!(body_buf, r#"{{"value":{v},"value_type":"pressure"}}"#)
            }
            SensorValue::Altitude(v) => {
                write!(body_buf, r#"{{"value":{v},"value_type":"altitude"}}"#)
            }
            SensorValue::AirQuality(v) => {
                write!(body_buf, r#"{{"value":{v},"value_type":"dust_density"}}"#)
            }
            SensorValue::Unknown { .. } => Ok(()),
        };
    }
}
