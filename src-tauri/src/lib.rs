mod models;
mod report;
mod runner;
mod runtime;
mod settings;
mod system;

use tauri::{Manager, RunEvent};

pub fn run() {
    tauri::Builder::default()
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
            system::get_network_info,
            system::get_network_interfaces,
            settings::get_custom_packet_length,
            settings::save_custom_packet_length,
            runtime::get_iperf_runtime_info,
            report::generate_report
        ])
        .build(tauri::generate_context!())
        .expect("failed to build iperf3 GUI")
        .run(|app, event| {
            // 应用退出时终止遗留的测试子进程，防止 iperf3 残留占用端口
            if let RunEvent::Exit = event {
                let state = app.state::<runner::AppState>();
                runner::kill_all_children_sync(&state);
            }
        });
}
