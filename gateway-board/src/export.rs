//! Sensor data exporting

use crate::net::http::{HttpBody, HttpClient, HttpClientError, HttpMethod};
use defmt::{error, info};
use protocol::app::v1::SensorValue;

pub trait ValuesExporter {
    async fn export(
        &self,
        client: &mut HttpClient<'_>,
        values: &[SensorValue],
    ) -> Result<(), HttpClientError>;
}

pub struct SensorCommunityExporter;

#[repr(u8)]
#[derive(Clone, Copy)]
enum SensorCommunitySensor {
    ParticulateMatter = 1,
    TemperaturePressure = 3,
}

impl SensorCommunitySensor {
    fn supports_value(self, value: SensorValue) -> bool {
        match self {
            SensorCommunitySensor::ParticulateMatter => matches!(value, SensorValue::AirQuality(_)),
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
        values: &[SensorValue],
    ) -> Result<(), HttpClientError> {
        self.export_by_sensor(client, SensorCommunitySensor::ParticulateMatter, values)
            .await?;
        self.export_by_sensor(client, SensorCommunitySensor::TemperaturePressure, values)
            .await
    }
}

impl SensorCommunityExporter {
    async fn export_by_sensor(
        &self,
        client: &mut HttpClient<'_>,
        sensor: SensorCommunitySensor,
        values: &[SensorValue],
    ) -> Result<(), HttpClientError> {
        use core::fmt::Write;

        if values.iter().filter(|&&v| sensor.supports_value(v)).count() == 0 {
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

        for (i, value) in values.iter().copied().enumerate() {
            if !sensor.supports_value(value) {
                continue;
            }
            self.write_value_to_body(req.body(), value, i == 0);
            exported_count += 1;
        }
        req.body().extend_from_slice(br#"]}"#);

        let res = req.finish().await?;

        if res.status() < 200 || res.status() >= 300 {
            error!(
                "export: sensor.community: request failed: {=u16}",
                res.status()
            );
        } else {
            info!(
                "export: sensor.community: successfully exported {=u32} value(s)",
                exported_count
            );
        }
        Ok(())
    }

    fn write_value_to_body(&self, body_buf: &mut HttpBody, value: SensorValue, first_value: bool) {
        use core::fmt::Write;

        if !first_value {
            body_buf.push(b',');
        }

        let _ = match value {
            SensorValue::Temperature(v) => {
                write!(body_buf, r#"{{"value":{},"value_type":"temperature"}}"#, v)
            }
            SensorValue::Pressure(v) => {
                write!(body_buf, r#"{{"value":{},"value_type":"pressure"}}"#, v)
            }
            SensorValue::Altitude(v) => {
                write!(body_buf, r#"{{"value":{},"value_type":"altitude"}}"#, v)
            }
            SensorValue::AirQuality(v) => {
                write!(body_buf, r#"{{"value":{},"value_type":"dust_density"}}"#, v)
            }
            SensorValue::Unknown { .. } => Ok(()),
        };
    }
}
