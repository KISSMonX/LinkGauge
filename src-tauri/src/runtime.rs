use crate::models::IperfRuntimeInfo;
use std::path::PathBuf;
use tauri::{path::BaseDirectory, AppHandle, Manager};
use tokio::process::Command;

pub fn resolve_iperf_path(app: &AppHandle, requested: &str) -> (PathBuf, bool) {
    let requested = requested.trim();
    if !requested.is_empty() && requested != "iperf3" && requested != "bundled" {
        return (PathBuf::from(requested), false);
    }

    let relative = bundled_relative_path();
    let mut candidates = Vec::new();
    if let Ok(path) = app.path().resolve(&relative, BaseDirectory::Resource) {
        candidates.push(path);
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(&relative),
    );
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(&relative));
    }

    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return (path, true);
    }
    (
        PathBuf::from(if cfg!(windows) {
            "iperf3.exe"
        } else {
            "iperf3"
        }),
        false,
    )
}

#[tauri::command]
pub async fn get_iperf_runtime_info(app: AppHandle) -> IperfRuntimeInfo {
    let (path, bundled) = resolve_iperf_path(&app, "bundled");
    let output = Command::new(&path).arg("--version").output().await;
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let version = text.lines().next().unwrap_or("iperf3").trim().to_string();
            IperfRuntimeInfo {
                available: true,
                bundled,
                path: path.to_string_lossy().to_string(),
                version,
            }
        }
        _ => IperfRuntimeInfo {
            available: false,
            bundled,
            path: path.to_string_lossy().to_string(),
            version: "未找到".into(),
        },
    }
}

fn bundled_relative_path() -> PathBuf {
    #[cfg(all(windows, target_arch = "x86_64"))]
    return PathBuf::from("iperf3/windows-x86_64/iperf3.exe");
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return PathBuf::from("iperf3/linux-x86_64/iperf3");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return PathBuf::from("iperf3/linux-aarch64/iperf3");
    #[allow(unreachable_code)]
    PathBuf::from("iperf3")
}
