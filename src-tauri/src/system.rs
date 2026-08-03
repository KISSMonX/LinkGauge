use crate::models::{InterfaceInfo, NetworkInfo};
use std::collections::HashMap;

/// 枚举本机所有 IPv4 网卡，返回 (网卡名, IP)。
/// 回环地址排到末尾；存在真实网卡时直接排除回环，列表第一个即默认网卡。
fn collect_interfaces() -> Vec<(String, String)> {
    let mut list = Vec::new();
    if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
        for (name, ip) in interfaces {
            if ip.is_ipv4() {
                list.push((name, ip.to_string()));
            }
        }
    }
    list.sort_by_key(|(_, ip)| ip.starts_with("127."));
    if list.iter().any(|(_, ip)| !ip.starts_with("127.")) {
        list.retain(|(_, ip)| !ip.starts_with("127."));
    }
    list
}

fn mac_of(name: &str) -> String {
    mac_address::mac_address_by_name(name)
        .ok()
        .flatten()
        .map(|value| value.to_string().to_uppercase())
        .unwrap_or_else(|| "--".into())
}

/// 网卡名 → 链路速率（Mbps），查不到速率的网卡不包含在结果中。
fn link_speeds() -> HashMap<String, u64> {
    #[cfg(windows)]
    {
        link_speeds_windows()
    }
    #[cfg(not(windows))]
    {
        link_speeds_linux()
    }
}

#[cfg(windows)]
fn link_speeds_windows() -> HashMap<String, u64> {
    use serde_json::Value;
    use std::process::Command as StdCommand;

    let script = "Get-CimInstance Win32_NetworkAdapter -Filter 'NetEnabled=True' | Select-Object NetConnectionID,Speed | ConvertTo-Json -Compress";
    let output = StdCommand::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output();
    let mut map = HashMap::new();
    let text = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        _ => return map,
    };
    // ConvertTo-Json 单条记录输出对象，多条输出数组，两种形态都兼容
    let items: Vec<Value> = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => items,
        Ok(Value::Object(item)) => vec![Value::Object(item)],
        _ => return map,
    };
    for item in items {
        let name = item
            .get("NetConnectionID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let speed = item.get("Speed").and_then(Value::as_u64).unwrap_or(0);
        if name.is_empty() {
            continue;
        }
        let mbps = speed / 1_000_000;
        if mbps > 0 {
            map.insert(name.to_string(), mbps);
        }
    }
    map
}

#[cfg(not(windows))]
fn link_speeds_linux() -> HashMap<String, u64> {
    let mut map = HashMap::new();
    let entries = match std::fs::read_dir("/sys/class/net") {
        Ok(entries) => entries,
        Err(_) => return map,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let speed = std::fs::read_to_string(entry.path().join("speed"))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if speed > 0 {
            map.insert(name, speed);
        }
    }
    map
}

#[tauri::command]
pub fn get_network_interfaces() -> Vec<InterfaceInfo> {
    let speeds = link_speeds();
    collect_interfaces()
        .into_iter()
        .map(|(name, ip)| InterfaceInfo {
            ip,
            interface_name: name.clone(),
            mac: mac_of(&name),
            speed_mbps: speeds.get(&name).copied().unwrap_or(0),
        })
        .collect()
}

#[tauri::command]
pub fn get_network_info() -> NetworkInfo {
    let first = get_network_interfaces().into_iter().next();
    let hostname = hostname::get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "localhost".into());
    NetworkInfo {
        ip: first
            .as_ref()
            .map(|value| value.ip.clone())
            .unwrap_or_else(|| "127.0.0.1".into()),
        mac: first
            .as_ref()
            .map(|value| value.mac.clone())
            .unwrap_or_else(|| "--".into()),
        interface_name: first
            .as_ref()
            .map(|value| value.interface_name.clone())
            .unwrap_or_else(|| "默认网卡".into()),
        speed_mbps: first.map(|value| value.speed_mbps).unwrap_or(0),
        hostname,
    }
}
