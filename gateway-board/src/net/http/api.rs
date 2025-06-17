use core::fmt::Write;
use core::str::FromStr;
use defmt::{info, warn};

use crate::{
    config::CONFIG,
    net::http::{HttpMethod, HttpServerError, HttpServerRequest, HttpServerResponse},
};

#[derive(PartialEq)]
pub enum ConfigurationVariable {
    CsrfToken,
    WifiStaSsid,
    WifiStaPassword,
    WifiApSsid,
    DnsServer1,
    DnsServer2,
    HtmlFormAction,
}

impl TryFrom<&[u8]> for ConfigurationVariable {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            b"csrf_token" => Ok(ConfigurationVariable::CsrfToken),
            b"wifi_sta_ssid" => Ok(ConfigurationVariable::WifiStaSsid),
            b"wifi_sta_password" => Ok(ConfigurationVariable::WifiStaPassword),
            b"wifi_ap_ssid" => Ok(ConfigurationVariable::WifiApSsid),
            b"dns_server_1" => Ok(ConfigurationVariable::DnsServer1),
            b"dns_server_2" => Ok(ConfigurationVariable::DnsServer2),
            b"action" => Ok(ConfigurationVariable::HtmlFormAction),
            _ => Err(()),
        }
    }
}

enum HtmlFormAction {
    Apply,
    SaveAndReboot,
}

impl TryFrom<&[u8]> for HtmlFormAction {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            b"apply" => Ok(Self::Apply),
            b"save-reboot" => Ok(Self::SaveAndReboot),
            _ => Err(()),
        }
    }
}

pub async fn dispatch_http_request<'a, 'r>(
    request: HttpServerRequest<'a, 'r>,
) -> Result<HttpServerResponse<'a, 'r>, HttpServerError> {
    Ok(match request.method() {
        HttpMethod::Get => return_dashboard_form(request).await?,
        HttpMethod::Post => handle_dashboard_post(request).await?,
    })
}

async fn return_dashboard_form<'a, 'r>(
    request: HttpServerRequest<'a, 'r>,
) -> Result<HttpServerResponse<'a, 'r>, HttpServerError> {
    info!("HTTP GET request, returning form page");
    let mut res = request.new_response();
    res.status = 200;
    let config = CONFIG.lock().await;

    let mut ip_str: heapless::String<15> = heapless::String::new();
    write!(&mut ip_str, "{}", config.dns_server_1).ok();
    #[rustfmt::skip]
    res.write_all_vectored(&[concat!("HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n",
r#"<!DOCTYPE html>
<html lang="en">
<head>
<title>Gateway Board Configuration</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
body {
font-family: Arial, Helvetica, sans-serif;
}
#gw-config {
display: flex; flex-direction: column; gap: 0.5em; max-width: 400px;
}
#gw-config label {
font-weight: bold;
}
</style>
</head>
<body>
<h1>Gateway Board Configuration</h1>
<form method="post" id="gw-config">
<input type="hidden" name="csrf_token" value=""#).as_bytes(), config.csrf_token.as_bytes(), br#"">
<label for="wifi_sta_ssid">WiFi external access point SSID</label>
<input type="text" name="wifi_sta_ssid" placeholder="WiFi SSID" value=""#, config.wifi_sta_ssid.as_deref().unwrap_or("").as_bytes(), br#"" required>
<label for="wifi_sta_password">WiFi external access point password</label>
<input type="password" name="wifi_sta_password" placeholder="WiFi Password" value="(_unchanged_)" required>
<label for="wifi_sta_ssid">WiFi internal access point SSID</label>
<input type="text" name="wifi_ap_ssid" placeholder="WiFi AP SSID" value=""#, config.wifi_ap_ssid.as_bytes(), br#"" required>
<label for="dns_server_1">Primary DNS server</label>
<input type="text" name="dns_server_1" placeholder="1.1.1.1" value=""#, ip_str.as_bytes(), br#"" required>"#,
    ]).await?;

    ip_str.clear();
    write!(&mut ip_str, "{}", config.dns_server_2).ok();

    #[rustfmt::skip]
    res.write_all_vectored(&[
br#"<label for="dns_server_2">Secondary DNS server</label>
<input type="text" name="dns_server_2" placeholder="1.0.0.1" value=""#, ip_str.as_bytes(), br#"" required>
<button type="submit" name="action" value="apply">Apply</button>
<button type="submit" name="action" value="save-reboot">Save & Reboot</button>
</form>
</body>"#,
    ]).await?;
    Ok(res)
}

async fn return_processing_form_page<'a, 'r>(
    request: HttpServerRequest<'a, 'r>,
) -> Result<HttpServerResponse<'a, 'r>, HttpServerError> {
    info!("HTTP POST request, returning processing page");
    let mut res = request.new_response();
    res.status = 202;

    #[rustfmt::skip]
    res.write_all(concat!("HTTP/1.0 202 Accepted\r\nRefresh: 2, /\r\nConnection: close\r\n\r\n",
r#"<!DOCTYPE html>
<html lang="en">
<head>
<title>Processing request...</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
</head>
<body>
<h1>Processing request...</h1>
<p>Please wait a few seconds.</p>
</body>"#
    ).as_bytes()) .await?;
    Ok(res)
}

async fn return_reboot_page<'a, 'r>(
    request: HttpServerRequest<'a, 'r>,
) -> Result<HttpServerResponse<'a, 'r>, HttpServerError> {
    let mut res = request.new_response();
    res.status = 200;

    #[rustfmt::skip]
    res.write_all(concat!("HTTP/1.0 202 Accepted\r\nRefresh: 10, /\r\nConnection: close\r\n\r\n",
r#"<!DOCTYPE html>
<html lang="en">
<head>
<title>Gateway rebooting...</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
</head>
<body>
<h1>Gateway Board is rebooting...</h1>
<p>Please wait a few seconds for the system to come back online.</p>
</body>"#
    ).as_bytes()).await?;
    Ok(res)
}

async fn handle_dashboard_post<'a, 'r>(
    mut request: HttpServerRequest<'a, 'r>,
) -> Result<HttpServerResponse<'a, 'r>, HttpServerError> {
    info!("HTTP POST request, processing form submission");
    let mut valid_csrf_token: bool = false;
    let mut action = HtmlFormAction::Apply; // Default action

    // Scope the config lock to this block to ensure it is released before returning
    {
        let mut config = CONFIG.lock().await;

        for (key, value) in util::encoding::decode_form_url_encoded(request.body()) {
            let Ok(config_var) = ConfigurationVariable::try_from(key) else {
                warn!("Invalid configuration variable name: {=[u8]:a}", key);
                continue;
            };

            let Ok(value_str) = core::str::from_utf8(value) else {
                warn!("Invalid UTF-8 in value for {=[u8]:a}", key);
                continue;
            };

            // Expect CSRF token to be the first field in the form
            if !valid_csrf_token && config_var != ConfigurationVariable::CsrfToken {
                warn!("Missing or invalid CSRF token (or not as first variable). Aborting form processing.");
                break;
            }

            match config_var {
                ConfigurationVariable::CsrfToken => {
                    if value_str.is_empty() {
                        warn!("Empty CSRF token received, ignoring");
                        continue;
                    }
                    info!("Validating CSRF token: {}", value_str);
                    valid_csrf_token = config.csrf_token == value_str;
                }
                ConfigurationVariable::WifiStaSsid => {
                    match heapless::String::<32>::from_str(value_str) {
                        Ok(s) if s.is_empty() => {
                            info!("Empty WiFi STA SSID received, clearing config.");
                            config.wifi_sta_ssid = None;
                        }
                        Ok(s) => {
                            info!("Setting WiFi STA SSID: {}", s);
                            config.wifi_sta_ssid = Some(s);
                        }
                        Err(_) => warn!("Invalid WiFi STA SSID, keeping current value."),
                    }
                }
                ConfigurationVariable::WifiStaPassword => {
                    match heapless::String::<64>::from_str(value_str) {
                        Ok(s) if s.is_empty() => {
                            info!("Empty WiFi STA PASS received, clearing config.");
                            config.wifi_sta_pass = None;
                        }
                        Ok(s) if s != "(_unchanged_)" => {
                            info!("Updating WiFi STA PASS.");
                            config.wifi_sta_pass = Some(s);
                        }
                        Ok(_) => { /* unchanged, skip */ }
                        Err(_) => warn!("Invalid WiFi STA PASS, keeping current value."),
                    }
                }
                ConfigurationVariable::WifiApSsid => {
                    match heapless::String::<32>::from_str(value_str) {
                        Ok(s) => {
                            info!("Setting WiFi AP SSID: {}", s);
                            config.wifi_ap_ssid = s;
                        }
                        Err(_) => warn!("Invalid WiFi AP SSID, keeping current value."),
                    }
                }
                ConfigurationVariable::DnsServer1 => match value_str.parse() {
                    Ok(ip) => {
                        info!("Setting DNS server 1: {}", ip);
                        config.dns_server_1 = ip;
                    }
                    Err(_) => warn!("Invalid DNS server 1 address."),
                },
                ConfigurationVariable::DnsServer2 => match value_str.parse() {
                    Ok(ip) => {
                        info!("Setting DNS server 2: {}", ip);
                        config.dns_server_2 = ip;
                    }
                    Err(_) => warn!("Invalid DNS server 2 address."),
                },
                ConfigurationVariable::HtmlFormAction => match HtmlFormAction::try_from(value) {
                    // browser typically sends this as the last field
                    Ok(a) => action = a,
                    Err(_) => warn!("Invalid HTML form action: {=[u8]:a}", value),
                },
            }
        }
    }

    if !valid_csrf_token {
        let mut res = request.new_response();
        warn!("CSRF token is missing or invalid in form submission");
        res.return_bad_request().await?;
        return Ok(res);
    }

    match action {
        HtmlFormAction::Apply => {
            info!("Form submission processed successfully");
            // Return a response indicating the form was processed
            return_processing_form_page(request).await
        }
        HtmlFormAction::SaveAndReboot => {
            info!("Form submitted with 'Save and Reboot' action, rebooting now");
            // Return a response indicating the system is rebooting
            let mut res = return_reboot_page(request).await?;

            // force closing the connection
            res.finish_connection().await;

            // then reboot the system
            esp_hal::system::software_reset()
        }
    }
}
