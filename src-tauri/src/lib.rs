mod models;
mod report;
mod runner;
mod runtime;
mod system;

pub fn run() {
    tauri::Builder::default()
        .manage(runner::AppState::default())
        .invoke_handler(tauri::generate_handler![
            runner::start_test,
            runner::stop_test,
            system::get_network_info,
            runtime::get_iperf_runtime_info,
            report::generate_report
        ])
        .run(tauri::generate_context!())
        .expect("failed to run iperf3 GUI");
}
