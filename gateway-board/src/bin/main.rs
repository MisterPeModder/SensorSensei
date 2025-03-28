#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, rng::Rng, timer::timg::TimerGroup};
use log::info;

#[embassy_executor::task]
#[cfg(feature = "display-ssd1306")]
async fn display_things(hardware: gateway_board::display::GatewayDisplayHardware) -> ! {
    gateway_board::display::display_demo(hardware).await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_wifi_ap(controller: esp_wifi::wifi::WifiController<'static>) -> ! {
    info!("start wifi task");
    gateway_board::net::run_access_point(controller).await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn run_dhcp(stack: embassy_net::Stack<'static>) -> ! {
    let mut dhcp_server = gateway_board::net::GatewayDhcpServer::new(stack);
    dhcp_server.run().await
}

#[cfg(feature = "wifi")]
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, esp_wifi::wifi::WifiDevice<'static>>) {
    runner.run().await
}

#[cfg(feature = "wifi")]
static ESP_WIFI_CTRL: static_cell::StaticCell<esp_wifi::EspWifiController<'static>> =
    static_cell::StaticCell::new();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

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
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let rng = Rng::new(peripherals.RNG);

    info!("HAL intialized!");

    #[cfg(feature = "wifi")]
    {
        let esp_wifi_ctrl = ESP_WIFI_CTRL.init_with(|| {
            esp_wifi::init(timg0.timer0, rng, peripherals.RADIO_CLK)
                .expect("failed to init ESP wifi controller")
        });
        let wifi_ap_stack =
            gateway_board::net::WifiApStack::new(esp_wifi_ctrl, rng, peripherals.WIFI);

        spawner.must_spawn(run_wifi_ap(wifi_ap_stack.controller));
        spawner.must_spawn(run_dhcp(wifi_ap_stack.stack));
        spawner.must_spawn(net_task(wifi_ap_stack.runner));
    }

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
}
