use embassy_net::{Runner, Stack, StackResources, StaticConfigV4};
use embassy_time::{Duration, Timer};
use esp_hal::{peripheral::Peripheral, peripherals::WIFI, rng::Rng};
use esp_wifi::{
    wifi::{AccessPointConfiguration, WifiController, WifiDevice, WifiEvent, WifiState},
    EspWifiController,
};
use log::info;
use static_cell::StaticCell;

use super::{GATEWAY_IP, GATEWAY_RANGE};

/// Max number of IP sockets allowed to connect
const MAX_SOCKETS: usize = 3;
static NW_STACK_RESOUCES: StaticCell<StackResources<MAX_SOCKETS>> = StaticCell::new();

#[non_exhaustive]
pub struct WifiApStack<'d> {
    pub stack: Stack<'d>,
    pub runner: Runner<'d, WifiDevice<'d>>,
    pub controller: WifiController<'d>,
}

impl<'d> WifiApStack<'d> {
    pub fn new(
        esp_wifi_ctrl: &'d mut EspWifiController,
        mut rng: Rng,
        wifi: impl Peripheral<P = WIFI> + 'd,
    ) -> Self {
        let (controller, interfaces) =
            esp_wifi::wifi::new(esp_wifi_ctrl, wifi).expect("failed to create wifi interfaces");
        let ap_stack = interfaces.ap;

        let ap_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
            address: GATEWAY_RANGE,
            gateway: Some(GATEWAY_IP),
            dns_servers: Default::default(),
        });
        let seed = (rng.random() as u64) << 32 | rng.random() as u64;

        let nw_stack_res = NW_STACK_RESOUCES.init_with(StackResources::<MAX_SOCKETS>::new);

        let (stack, runner) = embassy_net::new(ap_stack, ap_config, nw_stack_res, seed);

        Self {
            stack,
            runner,
            controller,
        }
    }
}

pub async fn run_access_point(mut controller: WifiController<'_>) -> ! {
    info!("device capabilities: {:?}", controller.capabilities());
    loop {
        if let WifiState::ApStarted = esp_wifi::wifi::wifi_state() {
            // wait until we're no longer connected
            controller.wait_for_event(WifiEvent::ApStop).await;
            Timer::after(Duration::from_millis(5000)).await
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config =
                esp_wifi::wifi::Configuration::AccessPoint(AccessPointConfiguration {
                    ssid: "esp-wifi".try_into().unwrap(),
                    ..Default::default()
                });
            controller.set_configuration(&client_config).unwrap();
            info!("Starting wifi");
            controller.start_async().await.unwrap();
            info!("Wifi started!");
        }
    }
}
