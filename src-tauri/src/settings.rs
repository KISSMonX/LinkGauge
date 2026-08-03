use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    /// TCP 自定义报文长度（默认 128KB，上限 1MB）
    custom_packet_length: Option<u32>,
    /// UDP 自定义报文长度（默认 8KB，上限 64KB）
    custom_udp_packet_length: Option<u32>,
}

fn settings_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("linkgauge"))
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
pub fn get_custom_packet_length(app: AppHandle, protocol: String) -> u32 {
    let settings = load_settings(&app);
    if protocol == "udp" {
        settings.custom_udp_packet_length.unwrap_or(0)
    } else {
        settings.custom_packet_length.unwrap_or(0)
    }
}

#[tauri::command]
pub fn save_custom_packet_length(app: AppHandle, protocol: String, length: u32) -> Result<(), String> {
    // TCP 报文长度上限 1MB，UDP 上限 64KB
    let limit = if protocol == "udp" { 65_536 } else { 1_048_576 };
    if length == 0 || length > limit {
        return Err(format!("自定义报文长度应在 1 ~ {limit} bytes 之间"));
    }
    let mut settings = load_settings(&app);
    if protocol == "udp" {
        settings.custom_udp_packet_length = Some(length);
    } else {
        settings.custom_packet_length = Some(length);
    }
    save_settings(&app, &settings)
}

/// 返回程序安装目录下的 config 文件夹路径（不存在则创建），作为导出配置对话框的默认位置。
/// 注意：tauri 的 executable_dir() 在 Windows 上不支持（dirs crate 限制），
/// 因此基于 current_exe() 定位程序所在目录。
#[tauri::command]
pub async fn get_export_dir() -> Result<String, String> {
    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("无法定位程序目录：{e}"))?
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let dir = exe_dir.join("config");
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建配置目录：{e}"))?;
    Ok(dir.to_string_lossy().to_string())
}

/// 将配置 JSON 写入用户通过保存对话框指定的路径
#[tauri::command]
pub async fn export_config(path: String, config: String) -> Result<String, String> {
    let path = PathBuf::from(&path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目录：{e}"))?;
    }
    fs::write(&path, config).map_err(|e| format!("写入配置文件失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}
