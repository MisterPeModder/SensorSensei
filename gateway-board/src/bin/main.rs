#![no_std]
#![no_main]

use defmt::{info, warn};
use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    peripherals::{RADIO_CLK, RNG, TIMG0, WIFI},
    rng::Rng,
    timer::timg::TimerGroup,
};
use gateway_board::{config::CONFIG, ValueChannel, ValueReceiver, ValueSender};
use protocol::app::v1::{SensorValue, SensorValuePoint};
use static_cell::StaticCell;

pub enum ConfigurationVariable {
    WifiStaSsid,
    WifiStaPassword,
    WifiApSsid,
    DnsServer1,
    DnsServer2,
}

impl TryFrom<&[u8]> for ConfigurationVariable {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            b"wifi_sta_ssid" => Ok(ConfigurationVariable::WifiStaSsid),
            b"wifi_sta_password" => Ok(ConfigurationVariable::WifiStaPassword),
            b"wifi_ap_ssid" => Ok(ConfigurationVariable::WifiApSsid),
            b"dns_server_1" => Ok(ConfigurationVariable::DnsServer1),
            b"dns_server_2" => Ok(ConfigurationVariable::DnsServer2),
            _ => Err(()),
        }
    }
}

#[embassy_executor::task]
#[cfg(feature = "display-ssd1306")]
async fn display_things(hardware: gateway_board::display::GatewayDisplayHardware) -> ! {
    gateway_board::display::display_demo(hardware).await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_wifi_controller(mut controller: gateway_board::net::WifiController<'static>) {
    info!("start wifi task");
    controller.run().await.expect("error while running wifi");
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_dhcp(stack: embassy_net::Stack<'static>) -> ! {
    let mut dhcp_server = gateway_board::net::GatewayDhcpServer::new(stack);
    dhcp_server.run().await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task(pool_size = 2)]
async fn run_net_stack(
    mut runner: embassy_net::Runner<'static, esp_wifi::wifi::WifiDevice<'static>>,
) -> ! {
    runner.run().await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_http(
    ap_stack: embassy_net::Stack<'static>,
    sta_stack: embassy_net::Stack<'static>,
) -> ! {
    use core::str::FromStr;
    use gateway_board::net::http::HttpServerRequest;

    let mut server = gateway_board::net::http::HttpServer::new(ap_stack, sta_stack, 80).await;
    server
        .run(async |mut req: HttpServerRequest| {
            Ok(match req.method() {
                // Handle the root path with a dummy page
                gateway_board::net::http::HttpMethod::Get => {
                    info!("HTTP GET request, returning form page");
                    let mut res = req.new_response();
                    res.return_dummy_page().await?;
                    res
                }
                // Handle the POST method for form submission
                gateway_board::net::http::HttpMethod::Post => {
                    info!("HTTP POST request, processing form submission");

                    // FIXME: Lock the config with a mutex while updating it
                    let config = CONFIG.lock().await;

                    for (key, value) in util::encoding::decode_form_url_encoded(&mut req.body()) {
                        let config_var = match ConfigurationVariable::try_from(key) {
                            Ok(k) => k,
                            Err(_) => {
                                warn!("Invalid configuration variable name: {:?}", key);
                                continue;
                            }
                        };
                        let Ok(value_str) = core::str::from_utf8(value) else {
                            warn!("Invalid UTF-8 in value for {:?}", key);
                            continue;
                        };

                        match config_var {
                            ConfigurationVariable::WifiStaSsid => {
                                if let Some(valid_ssid) =
                                    heapless::String::<32>::from_str(value_str).ok()
                                {
                                    if valid_ssid.is_empty() {
                                        info!(
                                            "Empty WiFi STA SSID received, clearing config value"
                                        );
                                        config.wifi_sta_ssid = None;
                                    } else {
                                        info!("Setting WiFi STA SSID to: {}", valid_ssid);
                                        config.wifi_sta_ssid = Some(valid_ssid);
                                    }
                                } else {
                                    warn!("Invalid WiFi STA SSID, keeping current value");
                                }
                            }
                            ConfigurationVariable::WifiStaPassword => {
                                if let Some(valid_ssid) =
                                    heapless::String::<64>::from_str(value_str).ok()
                                {
                                    if valid_ssid.is_empty() {
                                        info!(
                                            "Empty WiFi STA PASS received, clearing config value"
                                        );
                                        config.wifi_sta_pass = None;
                                    } else {
                                        info!("Updating WiFi STA PASS");
                                        config.wifi_sta_pass = Some(valid_ssid);
                                    }
                                } else {
                                    warn!("Invalid WiFi STA PASS, keeping current value");
                                }
                            }
                            ConfigurationVariable::WifiApSsid => {
                                if let Some(ssid) = heapless::String::<32>::from_str(value_str).ok()
                                {
                                    info!("Setting WiFi AP SSID to: {}", ssid);
                                    config.wifi_ap_ssid = ssid;
                                } else {
                                    warn!("Invalid WiFi AP SSID, keeping current value");
                                };
                            }
                            ConfigurationVariable::DnsServer1 => {
                                if let Some(dns_server) = value_str.parse().ok() {
                                    info!("Setting DNS server 1 to: {}", dns_server);
                                    config.dns_server_1 = dns_server;
                                } else {
                                    warn!("Invalid DNS server 1 address, keeping current value");
                                }
                            }
                            ConfigurationVariable::DnsServer2 => {
                                if let Some(dns_server) = value_str.parse().ok() {
                                    info!("Setting DNS server 2 to: {}", dns_server);
                                    config.dns_server_2 = dns_server;
                                } else {
                                    warn!("Invalid DNS server 2 address, keeping current value");
                                }
                            }
                        }
                    }
                    let mut res = req.new_response();
                    info!("Form submission processed successfully");
                    // Here you would typically process the form data
                    // For now, we just return a success response
                    res.return_dummy_page().await?;
                    res
                }
            })
        })
        .await
    // server.run_demo().await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn export_values(
    sta_stack: embassy_net::Stack<'static>,
    mut value_receiver: ValueReceiver,
) -> ! {
    use gateway_board::export;

    let mut value_buf: heapless::Vec<SensorValuePoint, { VALUE_CHANNEL_SIZE * 2 }> =
        heapless::Vec::new();

    loop {
        let values = export::collect_values(&mut value_buf, &mut value_receiver).await;
        export::export_to_all(sta_stack, values).await;
    }
}

#[cfg(feature = "wifi")]
static ESP_WIFI_CTRL: StaticCell<esp_wifi::EspWifiController<'static>> = StaticCell::new();

#[cfg(feature = "lora")]
#[embassy_executor::task]
async fn run_lora(hardware: gateway_board::lora::LoraHardware, sender: ValueSender) {
    use gateway_board::lora::LoraController;

    let mut lora = LoraController::new(hardware, sender)
        .await
        .expect("failed to initialize LoRa");
    lora.run().await;
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 72 * 1024);

    // wokwi: needed so that the console output is formatted correctly
    esp_println::print!("\x1b[20h");

    cfg_if::cfg_if! {
        if #[cfg(feature = "board-esp32dev")] {
            let timg1 = TimerGroup::new(peripherals.TIMG1);
            esp_hal_embassy::init(timg1.timer0);
        } else {
            use esp_hal::timer::systimer::SystemTimer;
            let systimer = SystemTimer::new(peripherals.SYSTIMER);
            esp_hal_embassy::init(systimer.alarm0);
        }
    }

    info!("HAL intialized!");

    let (value_sender, value_receiver) = make_value_channel();

    #[cfg(feature = "wifi")]
    setup_wifi(
        spawner,
        peripherals.TIMG0,
        peripherals.RNG,
        peripherals.RADIO_CLK,
        peripherals.WIFI,
        value_receiver,
    )
    .await;

    #[cfg(feature = "display-ssd1306")]
    spawner.must_spawn(display_things(
        gateway_board::display::GatewayDisplayHardware {
            i2c: peripherals.I2C0,
            vext: peripherals.GPIO36,
            sda: peripherals.GPIO17,
            scl: peripherals.GPIO18,
            rst: peripherals.GPIO21,
        },
    ));

    #[cfg(feature = "lora")]
    spawner.must_spawn(run_lora(
        gateway_board::lora::LoraHardware {
            spi: peripherals.SPI2,
            spi_nss: peripherals.GPIO8,
            spi_scl: peripherals.GPIO9,
            spi_mosi: peripherals.GPIO10,
            spi_miso: peripherals.GPIO11,
            reset: peripherals.GPIO12,
            busy: peripherals.GPIO13,
            dio1: peripherals.GPIO14,
        },
        value_sender,
    ));
}

#[cfg(feature = "wifi")]
async fn setup_wifi(
    spawner: Spawner,
    timg0: TIMG0,
    rng: RNG,
    radio_clk: RADIO_CLK,
    wifi: WIFI,
    value_receiver: ValueReceiver,
) {
    let timg0 = TimerGroup::new(timg0);
    let rng = Rng::new(rng);

    let esp_wifi_ctrl = ESP_WIFI_CTRL.init_with(|| {
        esp_wifi::init(timg0.timer0, rng, radio_clk).expect("failed to init ESP wifi controller")
    });
    let (mut wifi_ctrl, wifi_runners) = gateway_board::net::init_wifi(esp_wifi_ctrl, rng, wifi)
        .await
        .expect("failed to initialize wifi stack");

    wifi_ctrl
        .enable_ap(CONFIG.lock().await.wifi_ap_ssid.clone())
        .expect("AP configuration failed");

    let wifi_sta_ssid = CONFIG.lock().await.wifi_sta_ssid.clone();
    let wifi_sta_pass = CONFIG.lock().await.wifi_sta_pass.clone();
    match (wifi_sta_ssid, wifi_sta_pass) {
        (None, Some(_)) => warn!("not connecting to wifi: missing SSID"),
        (Some(_), None) => warn!("not connecting to wifi: missing password"),
        (None, None) => warn!("not connecting to wifi: missing SSID and password"),
        (Some(sta_ssid), Some(sta_pass)) => {
            wifi_ctrl
                .enable_sta(sta_ssid, sta_pass)
                .expect("STA configuration failed");
        }
    }

    let ap_stack = wifi_ctrl.ap_stack;
    let sta_stack = wifi_ctrl.sta_stack;

    spawner.must_spawn(run_net_stack(wifi_runners.ap_runner));
    spawner.must_spawn(run_net_stack(wifi_runners.sta_runner));
    spawner.must_spawn(run_dhcp(ap_stack));
    spawner.must_spawn(run_wifi_controller(wifi_ctrl));
    spawner.must_spawn(run_http(ap_stack, sta_stack));
    spawner.must_spawn(export_values(sta_stack, value_receiver));
}

const VALUE_CHANNEL_SIZE: usize = 4;

/// Create a pair and sender/receiver for sensor values.
/// The channel itself is a singleton allocated in static memory, calling this function twice will result in a panic.
fn make_value_channel() -> (ValueSender, ValueReceiver) {
    static VALUE_CHANNEL_BUF: StaticCell<[SensorValuePoint; VALUE_CHANNEL_SIZE]> =
        StaticCell::new();
    static VALUE_CHANNEL: StaticCell<ValueChannel> = StaticCell::new();

    const DUMMY_VALUE: SensorValuePoint = SensorValuePoint {
        value: SensorValue::Unknown {
            id: 255,
            value_len: 0,
        },
        time_offset: -99,
    };

    let value_channel: &'static mut ValueChannel = VALUE_CHANNEL.init_with(|| {
        ValueChannel::new(VALUE_CHANNEL_BUF.init_with(|| [DUMMY_VALUE; VALUE_CHANNEL_SIZE]))
    });
    value_channel.split()
}
