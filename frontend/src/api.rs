use gloo_net::http::Request;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Status {
    pub usb_enabled: bool,
    pub usb_ready: bool,
    pub host_os: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Config {
    pub usb_manufacturer: String,
    pub usb_product: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ScriptMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub dsl: Option<String>,
}

pub async fn get_status() -> Result<Status, String> {
    let res = Request::get("/status").send().await.map_err(err)?;
    let text = res.text().await.map_err(err)?;
    serde_json::from_str::<Status>(&text).map_err(|e| format!("parse status: {e}: {text}"))
}

pub async fn get_config() -> Result<Config, String> {
    let res = Request::get("/config").send().await.map_err(err)?;
    let text = res.text().await.map_err(err)?;
    serde_json::from_str::<Config>(&text).map_err(|e| format!("parse config: {e}: {text}"))
}

pub async fn save_config(manufacturer: &str, product: &str) -> Result<(), String> {
    let m = utf8_percent_encode(manufacturer, NON_ALPHANUMERIC).to_string();
    let p = utf8_percent_encode(product, NON_ALPHANUMERIC).to_string();
    let url = format!("/config?manufacturer={}&product={}", m, p);
    // Send an explicit empty body to ensure Content-Length is set,
    // avoiding any edge-cases in HTTP parsers for bodyless POSTs.
    let res = Request::post(&url)
        .body(String::new())
        .map_err(err)?
        .send()
        .await
        .map_err(err)?;
    if res.ok() {
        Ok(())
    } else {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        Err(format!("save config: HTTP {}: {}", status, body))
    }
}

pub async fn usb_register(assistant: bool, os: Option<&str>) -> Result<(), String> {
    let mut url = format!("/usb/register?assistant={}", if assistant { "1" } else { "0" });
    if let Some(os_val) = os {
        if !os_val.is_empty() {
            url.push_str(&format!("&os={}", os_val));
        }
    }
    // Use GET to avoid body-finalization deadlocks on some clients.
    let res = Request::get(&url).send().await.map_err(err)?;
    if res.ok() {
        Ok(())
    } else {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        Err(format!("usb register: HTTP {}: {}", status, body))
    }
}

pub async fn list_scripts() -> Result<Vec<ScriptMeta>, String> {
    let res = Request::get("/kb/scripts").send().await.map_err(err)?;
    let text = res.text().await.map_err(err)?;
    serde_json::from_str::<Vec<ScriptMeta>>(&text)
        .map_err(|e| format!("parse scripts: {e}: {} chars", text.len()))
}

pub async fn run_script(dsl: &str) -> Result<(), String> {
    let res = Request::post("/kb/script")
        .body(dsl.to_string())
        .map_err(err)?
        .send()
        .await
        .map_err(err)?;
    // 202 Accepted is success
    if res.status() == 202 || res.ok() {
        Ok(())
    } else {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        Err(format!("run script: HTTP {}: {}", status, body))
    }
}

fn err<E: std::fmt::Display>(e: E) -> String { format!("{e}") }
