mod converter;
mod error;
mod figcraft_exporter;
mod parser;

use std::path::PathBuf;

#[tauri::command]
fn convert_xd_to_figcraft(input_path: String, output_path: String) -> Result<(), String> {
    converter::convert(PathBuf::from(input_path), PathBuf::from(output_path))
        .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![convert_xd_to_figcraft])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri application");
}
