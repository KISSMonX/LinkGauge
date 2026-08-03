use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    custom_packet_length: Option<u32>,
}

fn settings_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("iperf3-gui"))
        .join("settings.json")
}

fn load_settings(app: &AppHandle) -> AppSettings {
    fs::read_to_string(settings_path(app))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, text).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_custom_packet_length(app: AppHandle) -> u32 {
    load_settings(&app).custom_packet_length.unwrap_or(0)
}

#[tauri::command]
pub fn save_custom_packet_length(app: AppHandle, length: u32) -> Result<(), String> {
    if length == 0 || length > 262_144 {
        return Err("自定义报文长度应在 1 ~ 262144 bytes 之间".into());
    }
    let mut settings = load_settings(&app);
    settings.custom_packet_length = Some(length);
    save_settings(&app, &settings)
}
