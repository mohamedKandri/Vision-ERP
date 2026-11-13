pub mod api;
pub mod audit;
pub mod auth;
pub mod cash;
pub mod db;
pub mod error;
pub mod item;
pub mod menu;
pub mod orders;
pub mod pricing;
pub mod seed;
pub mod settings;

use api::Core;
use error::ApiError;
use serde_json::Value;
use tauri::Manager;

/// The one IPC command. The webview never touches SQLite directly.
#[tauri::command]
fn rpc(
    core: tauri::State<'_, Core>,
    token: Option<String>,
    method: String,
    params: Option<Value>,
) -> Result<Value, ApiError> {
    api::dispatch(&core, token.as_deref(), &method, params.unwrap_or(Value::Null))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::open(&dir.join("burger-os.db"))?;
            app.manage(Core::new(conn));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc])
        .run(tauri::generate_context!())
        .expect("error while running BURGER OS");
}
