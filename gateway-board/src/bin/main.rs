#![no_std]
#![no_main]

use defmt::{error, info, warn, Debug2Format};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    peripherals::{RADIO_CLK, RNG, TIMG0, WIFI},
    rng::Rng,
    timer::timg::TimerGroup,
};
use gateway_board::config::CONFIG;

#[embassy_executor::task]
#[cfg(feature = "display-ssd1306")]
async fn display_things(hardware: gateway_board::display::GatewayDisplayHardware) -> ! {
    gateway_board::display::display_demo(hardware).await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_wifi_controller(mut controller: gateway_board::net::WifiController<'static>) {
    info!("start wifi task");
    controller.run().await.expect("error while running wifi")
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
    let mut server = gateway_board::net::http::HttpServer::new(ap_stack, sta_stack, 80).await;
    server.run().await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_http_client(ap_stack: embassy_net::Stack<'static>) -> ! {
    Timer::after(Duration::from_millis(10_000)).await;
    loop {
        info!("http-client: attempting request");
        if let Err(e) = gateway_board::net::http_client_demo(ap_stack).await {
            error!("http-client: error: {}", Debug2Format(&e));
        }
        Timer::after(Duration::from_millis(5000)).await;
    }
}

#[cfg(feature = "wifi")]
static ESP_WIFI_CTRL: static_cell::StaticCell<esp_wifi::EspWifiController<'static>> =
    static_cell::StaticCell::new();

#[cfg(feature = "lora")]
#[embassy_executor::task]
async fn run_lora(hardware: gateway_board::lora::LoraHardware) {
    use gateway_board::lora::LoraController;

    let mut lora = LoraController::new(hardware)
        .await
        .expect("failed to initialize LoRa");
    lora.run().await.expect("error while setting LoRa recieve");
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

    #[cfg(feature = "wifi")]
    setup_wifi(
        spawner,
        peripherals.TIMG0,
        peripherals.RNG,
        peripherals.RADIO_CLK,
        peripherals.WIFI,
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
    spawner.must_spawn(run_lora(gateway_board::lora::LoraHardware {
        spi: peripherals.SPI2,
        spi_nss: peripherals.GPIO8,
        spi_scl: peripherals.GPIO9,
        spi_mosi: peripherals.GPIO10,
        spi_miso: peripherals.GPIO11,
        reset: peripherals.GPIO12,
        busy: peripherals.GPIO13,
        dio1: peripherals.GPIO14,
    }));
}

#[cfg(feature = "wifi")]
async fn setup_wifi(spawner: Spawner, timg0: TIMG0, rng: RNG, radio_clk: RADIO_CLK, wifi: WIFI) {
    let timg0 = TimerGroup::new(timg0);
    let rng = Rng::new(rng);

    let esp_wifi_ctrl = ESP_WIFI_CTRL.init_with(|| {
        esp_wifi::init(timg0.timer0, rng, radio_clk).expect("failed to init ESP wifi controller")
    });
    let (mut wifi_ctrl, wifi_runners) = gateway_board::net::init_wifi(esp_wifi_ctrl, rng, wifi)
        .expect("failed to initialize wifi stack");

    wifi_ctrl
        .enable_ap(CONFIG.wifi_ap_ssid)
        .expect("AP configuration failed");

    match (CONFIG.wifi_sta_ssid, CONFIG.wifi_sta_pass) {
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
    spawner.must_spawn(run_http_client(sta_stack));
}
