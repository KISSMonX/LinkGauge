mod models;
mod report;
mod runner;
mod settings;
mod system;

use tauri::{Manager, RunEvent};

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
        .manage(runner::AppState::default())
        .invoke_handler(tauri::generate_handler![
            runner::start_test,
            runner::stop_test,
            runner::open_log_dir,
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
