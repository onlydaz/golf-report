// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = golf_report_lib::plc_route_helper_entry() {
        std::process::exit(code);
    }
    golf_report_lib::run()
}
