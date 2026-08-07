use crate::models::{InterfaceInfo, NetworkInfo, NetworkSnapshot};
#[cfg(not(windows))]
use std::collections::HashMap;

/// 枚举本机所有 IPv4 网卡，返回 (网卡名, IP)。
/// 回环地址排到末尾；存在真实网卡时直接排除回环，列表第一个即默认网卡。
#[cfg(not(windows))]
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

#[cfg(not(windows))]
fn mac_of(name: &str) -> String {
    mac_address::mac_address_by_name(name)
        .ok()
        .flatten()
        .map(|value| value.to_string().to_uppercase())
        .unwrap_or_else(|| "--".into())
}

/// 网卡名 → 链路速率（Mbps），查不到速率的网卡不包含在结果中。
#[cfg(not(windows))]
fn link_speeds() -> HashMap<String, u64> {
    #[cfg(target_os = "linux")]
    {
        link_speeds_linux()
    }
    // macOS 没有 Linux 的 /sys 文件系统；未取得速率时由界面显示“未知”。
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        HashMap::new()
    }
}

#[cfg(windows)]
fn collect_interface_info_windows() -> Vec<InterfaceInfo> {
    use std::net::Ipv4Addr;
    use std::{mem, ptr};
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    // Microsoft 建议预分配 15 KB，通常一次调用即可，网卡变化导致溢出时再按返回值扩容。
    let mut byte_len = 15_000u32;
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    for _ in 0..3 {
        // Vec<usize> 保证缓冲区满足 IP_ADAPTER_ADDRESSES 的指针对齐要求。
        let words = (byte_len as usize).div_ceil(mem::size_of::<usize>());
        let mut buffer = vec![0usize; words];
        let addresses = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result = unsafe {
            GetAdaptersAddresses(AF_INET as u32, flags, ptr::null(), addresses, &mut byte_len)
        };
        if result == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if result != NO_ERROR {
            return Vec::new();
        }

        let mut interfaces = Vec::new();
        let mut adapter = addresses;
        while !adapter.is_null() {
            let current = unsafe { &*adapter };
            if current.OperStatus == IfOperStatusUp {
                let name = unsafe { wide_string(current.FriendlyName) };
                let mac_len =
                    (current.PhysicalAddressLength as usize).min(current.PhysicalAddress.len());
                let mac = format_physical_address(&current.PhysicalAddress[..mac_len]);
                let speed_mbps =
                    current.TransmitLinkSpeed.max(current.ReceiveLinkSpeed) / 1_000_000;
                let mut unicast = current.FirstUnicastAddress;
                while !unicast.is_null() {
                    let address = unsafe { &(*unicast).Address };
                    if !address.lpSockaddr.is_null()
                        && address.iSockaddrLength as usize >= mem::size_of::<SOCKADDR_IN>()
                        && unsafe { (*address.lpSockaddr).sa_family } == AF_INET
                    {
                        let sockaddr = unsafe { &*address.lpSockaddr.cast::<SOCKADDR_IN>() };
                        let bytes = unsafe { sockaddr.sin_addr.S_un.S_addr }.to_ne_bytes();
                        let ip = Ipv4Addr::from(bytes);
                        interfaces.push(InterfaceInfo {
                            ip: ip.to_string(),
                            mac: mac.clone(),
                            interface_name: name.clone(),
                            speed_mbps,
                        });
                    }
                    unicast = unsafe { (*unicast).Next };
                }
            }
            adapter = current.Next;
        }
        interfaces.sort_by_key(|item| item.ip.starts_with("127."));
        if interfaces.iter().any(|item| !item.ip.starts_with("127.")) {
            interfaces.retain(|item| !item.ip.starts_with("127."));
        }
        return interfaces;
    }
    Vec::new()
}

#[cfg(windows)]
unsafe fn wide_string(value: *const u16) -> String {
    use std::slice;

    if value.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *value.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { slice::from_raw_parts(value, len) })
}

#[cfg(windows)]
fn format_physical_address(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "--".into();
    }
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(target_os = "linux")]
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

#[cfg(not(windows))]
fn collect_interface_info() -> Vec<InterfaceInfo> {
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

#[cfg(windows)]
fn collect_interface_info() -> Vec<InterfaceInfo> {
    collect_interface_info_windows()
}

fn build_network_snapshot() -> NetworkSnapshot {
    // 启动只构建一次快照，避免多个 IPC 分别枚举同一批系统网卡。
    let interfaces = collect_interface_info();
    let hostname = hostname::get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "localhost".into());
    snapshot_from_interfaces(interfaces, hostname)
}

fn snapshot_from_interfaces(interfaces: Vec<InterfaceInfo>, hostname: String) -> NetworkSnapshot {
    let first = interfaces.first();
    let info = NetworkInfo {
        ip: first
            .map(|value| value.ip.clone())
            .unwrap_or_else(|| "127.0.0.1".into()),
        mac: first
            .map(|value| value.mac.clone())
            .unwrap_or_else(|| "--".into()),
        interface_name: first
            .map(|value| value.interface_name.clone())
            .unwrap_or_else(|| "默认网卡".into()),
        speed_mbps: first.map_or(0, |value| value.speed_mbps),
        hostname,
    };
    NetworkSnapshot { info, interfaces }
}

#[tauri::command]
pub async fn get_network_snapshot() -> NetworkSnapshot {
    // 系统网卡枚举属于同步 API，放到阻塞线程避免影响 Tauri 的异步任务调度。
    tokio::task::spawn_blocking(build_network_snapshot)
        .await
        .unwrap_or_else(|_| NetworkSnapshot {
            info: NetworkInfo {
                ip: "127.0.0.1".into(),
                mac: "--".into(),
                hostname: "localhost".into(),
                interface_name: "默认网卡".into(),
                speed_mbps: 0,
            },
            interfaces: Vec::new(),
        })
}

#[cfg(test)]
mod tests {
    use super::snapshot_from_interfaces;
    use crate::models::InterfaceInfo;

    #[test]
    fn network_snapshot_info_matches_first_interface() {
        let snapshot = snapshot_from_interfaces(
            vec![InterfaceInfo {
                ip: "192.0.2.10".into(),
                mac: "00:11:22:33:44:55".into(),
                interface_name: "Ethernet".into(),
                speed_mbps: 1000,
            }],
            "host-a".into(),
        );
        assert_eq!(snapshot.info.ip, "192.0.2.10");
        assert_eq!(snapshot.info.mac, "00:11:22:33:44:55");
        assert_eq!(snapshot.info.interface_name, "Ethernet");
        assert_eq!(snapshot.info.speed_mbps, 1000);
        assert_eq!(snapshot.info.hostname, "host-a");
        assert_eq!(snapshot.interfaces.len(), 1);
    }

    #[test]
    fn network_snapshot_has_loopback_fallback() {
        let snapshot = snapshot_from_interfaces(Vec::new(), "host-b".into());
        assert_eq!(snapshot.info.ip, "127.0.0.1");
        assert_eq!(snapshot.info.mac, "--");
        assert_eq!(snapshot.info.speed_mbps, 0);
        assert!(snapshot.interfaces.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn physical_address_uses_canonical_uppercase_format() {
        assert_eq!(
            super::format_physical_address(&[0, 1, 0xAB, 0xFF]),
            "00:01:AB:FF"
        );
        assert_eq!(super::format_physical_address(&[]), "--");
    }

    #[cfg(windows)]
    #[test]
    fn windows_ip_helper_returns_valid_ipv4_interfaces() {
        let interfaces = super::collect_interface_info_windows();
        assert!(!interfaces.is_empty());
        for interface in interfaces {
            assert!(!interface.interface_name.is_empty());
            assert!(interface.ip.parse::<std::net::Ipv4Addr>().is_ok());
        }
    }
}
