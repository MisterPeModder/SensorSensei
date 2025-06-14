# Sensor.Community

## Registering a sensor

Go to https://devices.sensor.community/sensors, and click "Register new sensor"

## Displaying data

Go the https://devices.sensor.community/sensors/SENSOR_ID/data, where SENSOR_ID is the '#' number in the [devices page](https://devices.sensor.community/sensors).

## Sending data (manually)

```bash
curl -v 'https://api.sensor.community/v1/push-sensor-data/' \
    -H 'User-Agent: NRZ-2021-134-B4-ESP32/4123/4123' \
    -H 'Content-Type: application/json' \
    -H "X-Sensor: $SENSOR_UID" \
    -H "X-Pin: $SENSOR_VALUE_PIN" \
    -X POST -d "{\"sensordatavalues\": [{\"value\": $VALUE, \"value_type\": $VALUE_TYPE}]}"
```
|Variable|Example|Description|
|---|---|---|
|`SENSOR_UID`|`esp32-123456`|The UID of the board as shown in the [devices page](https://devices.sensor.community/sensors)|
|`SENSOR_VALUE_PIN`|`3`|The id of the sensor, go the hardware settings > enable expert fields to show the pin|
|`VALUE`|`25`|The actual value, expects a number|
|`VALUE_TYPE`|`"temperature"`|The kind of value|