use defmt::{error, info, warn, Debug2Format};
use embassy_net::{DhcpConfig, Runner, Stack, StackResources, StaticConfigV4};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use enumset::{enum_set, EnumSet};
use esp_hal::{peripheral::Peripheral, peripherals::WIFI, rng::Rng};
use esp_wifi::{
    wifi::{
        AccessPointConfiguration, ClientConfiguration, Configuration as WifiConfiguration,
        WifiDevice, WifiError, WifiEvent, WifiMode, WifiState,
    },
    EspWifiController,
};
use static_cell::StaticCell;

use crate::config::CONFIG;

use super::{GATEWAY_IP, GATEWAY_RANGE};

const MAX_SOCKETS_AP: usize = 3;
const MAX_SOCKETS_STA: usize = 4;
const DELAY: Duration = Duration::from_millis(2500);

static STACK_RESOURCES_AP: StaticCell<StackResources<MAX_SOCKETS_AP>> = StaticCell::new();
static STACK_RESOURCES_STA: StaticCell<StackResources<MAX_SOCKETS_STA>> = StaticCell::new();

#[derive(Debug)]
pub struct WifiConfigurationError;

pub fn init_wifi<'d>(
    esp_wifi_ctrl: &'d mut EspWifiController,
    mut rng: Rng,
    wifi: impl Peripheral<P = WIFI> + 'd,
) -> Result<(WifiController<'d>, WifiStackRunners<'d>), WifiError> {
    let (ctrl, interfaces) = esp_wifi::wifi::new(esp_wifi_ctrl, wifi)?;
    let ap_device = interfaces.ap;
    let sta_device = interfaces.sta;

    let mut dns_servers = heapless::Vec::new();

    dns_servers.push(CONFIG.dns_server_1).unwrap();
    dns_servers.push(CONFIG.dns_server_2).unwrap();

    let ap_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: GATEWAY_RANGE,
        gateway: Some(GATEWAY_IP),
        dns_servers,
    });
    let sta_config = embassy_net::Config::dhcpv4(DhcpConfig::default());

    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());

    let ap_stack_res = STACK_RESOURCES_AP.init_with(StackResources::<MAX_SOCKETS_AP>::new);
    let sta_stack_res = STACK_RESOURCES_STA.init_with(StackResources::<MAX_SOCKETS_STA>::new);

    let (ap_stack, ap_runner) = embassy_net::new(ap_device, ap_config, ap_stack_res, seed);
    let (sta_stack, sta_runner) = embassy_net::new(sta_device, sta_config, sta_stack_res, seed);

    let ctrl = WifiController {
        ap_stack,
        sta_stack,
        ctrl,
        ap_config: None,
        sta_config: None,
    };
    let runner = WifiStackRunners {
        ap_runner,
        sta_runner,
    };

    Ok((ctrl, runner))
}

#[non_exhaustive]
pub struct WifiStackRunners<'d> {
    pub ap_runner: Runner<'d, WifiDevice<'d>>,
    pub sta_runner: Runner<'d, WifiDevice<'d>>,
}

/// Stateful Wi-Fi controller that allows for enabling/disabling AP and/or STA modes at runtime.
pub struct WifiController<'d> {
    pub ap_stack: Stack<'d>,
    pub sta_stack: Stack<'d>,
    ctrl: esp_wifi::wifi::WifiController<'d>,
    ap_config: Option<AccessPointConfiguration>,
    sta_config: Option<ClientConfiguration>,
}

type ControllerMutex<'a, 'd> = Mutex<NoopRawMutex, &'a mut esp_wifi::wifi::WifiController<'d>>;

impl<'d> WifiController<'d> {
    pub fn enable_ap(
        &mut self,
        ssid: impl TryInto<heapless::String<32>>,
    ) -> Result<(), WifiConfigurationError> {
        self.ap_config = Some(AccessPointConfiguration {
            ssid: ssid.try_into().map_err(|_| WifiConfigurationError)?,
            ..Default::default()
        });
        Ok(())
    }

    pub fn enable_sta(
        &mut self,
        ssid: impl TryInto<heapless::String<32>>,
        password: impl TryInto<heapless::String<64>>,
    ) -> Result<(), WifiConfigurationError> {
        self.sta_config = Some(ClientConfiguration {
            ssid: ssid.try_into().map_err(|_| WifiConfigurationError)?,
            password: password.try_into().map_err(|_| WifiConfigurationError)?,
            ..Default::default()
        });
        Ok(())
    }

    /// Runs the Wi-Fi access point (AP mode) and/or a connection to an external access point (STA mode).
    /// Returns only when both AP and STA modes are externally stopped.
    pub async fn run(&mut self) -> Result<(), WifiError> {
        let config = self.create_config();
        self.ctrl.set_configuration(&config)?;

        let mode: WifiMode = (&config).try_into()?;
        let mut ap_enabled = mode.is_ap();
        let mut sta_enabled = mode.is_sta();

        info!(
            "wifi device capabilities: {:?}",
            Debug2Format(&self.ctrl.capabilities())
        );

        while ap_enabled | sta_enabled {
            let ctrl: ControllerMutex = ControllerMutex::new(&mut self.ctrl);
            embassy_futures::join::join(
                Self::ensure_ap_connected(&ctrl, self.ap_config.as_ref(), ap_enabled),
                Self::ensure_sta_connected(&ctrl, self.sta_config.as_ref(), sta_enabled),
            )
            .await;

            self.poll_events(&mut ap_enabled, &mut sta_enabled).await;
        }

        Ok(())
    }

    async fn ensure_ap_connected<'a>(
        ctrl: &ControllerMutex<'a, 'd>,
        config: Option<&AccessPointConfiguration>,
        enabled: bool,
    ) {
        if !enabled {
            return;
        }
        let config = config.expect("broken: no AP config in AP mode!");

        while !matches!(esp_wifi::wifi::ap_state(), WifiState::ApStarted) {
            info!("wifi AP: starting access point...");
            if let Err(e) = ctrl.lock().await.start_async().await {
                error!("wifi AP: start failed, attempting after {}: {:?}", DELAY, e);
                Timer::after(DELAY).await;
            } else {
                info!(
                    "wifi AP: access point started, ssid=`{}`, auth_method=`{:?}`",
                    config.ssid, config.auth_method
                );
            }
        }
    }

    async fn ensure_sta_connected<'a>(
        ctrl: &ControllerMutex<'a, 'd>,
        config: Option<&ClientConfiguration>,
        enabled: bool,
    ) {
        if !enabled {
            return;
        }
        let config = config.expect("broken: no STA config in STA mode!");

        loop {
            match esp_wifi::wifi::sta_state() {
                WifiState::StaStarted | WifiState::StaConnected | WifiState::StaDisconnected => {
                    // station mode stated, attempt to connect
                    if !matches!(esp_wifi::wifi::ap_state(), WifiState::StaConnected) {
                        info!(
                            "wifi STA: connecting to `{}` using auth `{:?}`",
                            config.ssid, config.auth_method
                        );
                        if let Err(e) = ctrl.lock().await.connect_async().await {
                            error!(
                                "wifi STA: connect failed, attempting after {}: {:?}",
                                DELAY, e
                            );
                            Timer::after(DELAY).await;
                        } else {
                            info!("wifi STA: connected to access point");
                            return;
                        }
                    }
                }
                _ => {
                    // station mode isn't started yet
                    info!("wifi STA: starting controller...");
                    if let Err(e) = ctrl.lock().await.start_async().await {
                        error!(
                            "wifi STA: start failed, attempting after {}: {:?}",
                            DELAY, e
                        );
                        Timer::after(DELAY).await;
                    } else {
                        info!("wifi STA: started");
                    }
                }
            }
        }
    }

    async fn poll_events(&mut self, ap_enabled: &mut bool, sta_enabled: &mut bool) {
        const EVENTS_TO_WAIT: EnumSet<WifiEvent> = enum_set! { WifiEvent::ApStop | WifiEvent::StaStop | WifiEvent::StaDisconnected | WifiEvent::ApStaconnected | WifiEvent::ApStadisconnected };

        let events = self.ctrl.wait_for_events(EVENTS_TO_WAIT, true).await;

        if events.contains(WifiEvent::ApStop) {
            warn!("wifi AP: stopped access point");
            *ap_enabled = false;
        }
        if events.contains(WifiEvent::StaStop) {
            warn!("wifi STA mode stopped");
            *sta_enabled = false;
        }
        if events.contains(WifiEvent::StaDisconnected) {
            warn!("disconnected from AP");
        }
        if events.contains(WifiEvent::ApStaconnected) {
            info!("wifi AP: new client connected");
        }
        if events.contains(WifiEvent::ApStaconnected) {
            info!("wifi AP: client disconnected");
        }
    }

    fn create_config(&self) -> WifiConfiguration {
        match (self.ap_config.clone(), self.sta_config.clone()) {
            (None, None) => WifiConfiguration::None,
            (None, Some(sta)) => WifiConfiguration::Client(sta),
            (Some(ap), None) => WifiConfiguration::AccessPoint(ap),
            (Some(ap), Some(sta)) => WifiConfiguration::Mixed(sta, ap),
        }
    }
}
