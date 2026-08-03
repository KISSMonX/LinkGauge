use crate::models::NetworkInfo;

#[tauri::command]
pub fn get_network_info() -> NetworkInfo {
    let ip = local_ip_address::local_ip()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| "127.0.0.1".into());
    let hostname = hostname::get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "localhost".into());
    let mac = mac_address::get_mac_address()
        .ok()
        .flatten()
        .map(|value| value.to_string().to_uppercase())
        .unwrap_or_else(|| "--".into());

    NetworkInfo {
        ip,
        mac,
        hostname,
        interface_name: "默认网卡".into(),
    }
}
