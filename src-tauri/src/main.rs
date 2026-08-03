#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    iperf3_gui_lib::run();
}
