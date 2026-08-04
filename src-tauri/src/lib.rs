mod models;
mod report;
mod runner;
mod settings;
mod ssh;
mod system;

use tauri::{Manager, RunEvent};

/// 前端「退出确认框」确认后调用：结束整个应用。
/// 主窗口 ✕ 由前端 onCloseRequested 拦截弹确认框，确认后走这里退出，
/// 分离的客户端/服务端子窗口随主进程一起退出，而非等到最后一个窗口关闭。
#[tauri::command]
fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// 将客户端 / 服务端标签页分离为独立窗口（窗口 label 即 side，前端按 label 渲染对应界面）。
/// 已存在同名窗口时直接聚焦，避免重复创建。
/// 注意：必须用 async 命令创建窗口——Windows 上在同步命令内调用 build() 会死锁
/// （WebView2 问题，tauri 文档明确警告），表现为窗口空白、页面无法加载。
#[tauri::command]
async fn create_side_window(app: tauri::AppHandle, side: String) -> Result<(), String> {
    if side != "client" && side != "server" {
        return Err("无效的窗口类型".into());
    }
    if let Some(window) = app.get_webview_window(&side) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    let title = format!(
        "LinkGauge · {}",
        if side == "client" {
            "客户端"
        } else {
            "服务端"
        }
    );
    // 客户端/服务端窗口同尺寸（含测试概览与实时曲线的三栏布局）
    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        side.clone(),
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(title)
    .inner_size(1280.0, 820.0)
    .min_inner_size(1000.0, 700.0)
    .resizable(true);
    // 放在主窗口右下偏移处，避免与新窗口完全重叠；主窗口不存在时居中。
    // outer_position 返回物理像素坐标，按 DPI 缩放换算成逻辑坐标后再传给 position
    if let Some(main) = app.get_webview_window("main") {
        if let Ok(pos) = main.outer_position() {
            let scale = main.scale_factor().unwrap_or(1.0);
            builder =
                builder.position((pos.x as f64 + 64.0) / scale, (pos.y as f64 + 48.0) / scale);
        } else {
            builder = builder.center();
        }
    } else {
        builder = builder.center();
    }
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}

/// 「关于」页信息：应用版本号（tauri.conf.json）+ 构建时的 Git commit id（build.rs 注入）
#[tauri::command]
fn get_app_info(app: tauri::AppHandle) -> serde_json::Value {
    serde_json::json!({
        "version": app.package_info().version.to_string(),
        "commit": env!("LINKGAUGE_COMMIT"),
    })
}

/// 用系统默认浏览器打开外部链接（「关于」页的项目地址 / 提交反馈）。
/// 与 runner::open_log_dir 相同的方式：调系统命令，不引入额外依赖。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("仅允许打开 http(s) 链接".into());
    }
    let result = std::process::Command::new(if cfg!(windows) {
        "cmd"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .args(if cfg!(windows) {
        vec!["/C", "start", "", url.as_str()]
    } else {
        vec![url.as_str()]
    })
    .spawn()
    .map_err(|e| format!("无法打开链接：{e}"))?;
    drop(result);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 已有一个实例运行时，聚焦已有窗口而不是启动第二个实例
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .setup(|_app| Ok(()))
        .manage(runner::AppState::default())
        .manage(ssh::SshState::default())
        .invoke_handler(tauri::generate_handler![
            exit_app,
            create_side_window,
            get_app_info,
            open_url,
            runner::start_test,
            runner::stop_test,
            runner::set_locale,
            runner::open_log_dir,
            ssh::ssh_connect,
            ssh::ssh_send,
            ssh::ssh_resize,
            ssh::ssh_disconnect,
            ssh::ssh_scrollback,
            system::get_network_info,
            system::get_network_interfaces,
            settings::get_custom_packet_length,
            settings::save_custom_packet_length,
            settings::get_export_dir,
            settings::export_config,
            report::generate_report,
            report::get_report_dir,
            report::open_report_dir
        ])
        .build(tauri::generate_context!())
        .expect("failed to build LinkGauge")
        .run(|app, event| {
            // 应用退出时终止遗留的 ping 测试子进程（riperf3 为进程内引擎，随进程退出）
            if let RunEvent::Exit = event {
                let state = app.state::<runner::AppState>();
                runner::kill_all_children_sync(&state);
            }
        });
}
