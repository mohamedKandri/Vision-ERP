pub mod api;
pub mod audit;
pub mod auth;
pub mod cash;
pub mod db;
pub mod error;
pub mod escpos;
pub mod item;
pub mod menu;
pub mod orders;
pub mod pricing;
pub mod printing;
pub mod seed;
pub mod settings;

use api::Core;
use error::ApiError;
use serde_json::Value;
use std::sync::Arc;
use tauri::Manager;

/// The one IPC command. Runs off the UI thread so database work, PIN hashing or a slow
/// Bluetooth printer never freeze the screen. The webview never touches SQLite directly.
#[tauri::command]
async fn rpc(
    core: tauri::State<'_, Arc<Core>>,
    token: Option<String>,
    method: String,
    params: Option<Value>,
) -> Result<Value, ApiError> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || api::dispatch(&core, token.as_deref(), &method, params.unwrap_or(Value::Null)))
        .await
        .map_err(|e| ApiError::new("internal", e.to_string()))?
}

#[cfg(target_os = "android")]
mod android_bluetooth {
    //! Bridges the print worker to the Kotlin `PrinterPlugin` (Bluetooth SPP, SUNMI InnerPrinter).
    use base64::Engine;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
    use tauri::Wry;

    struct Bridge(PluginHandle<Wry>);

    impl crate::printing::Bluetooth for Bridge {
        fn write(&self, target: &str, data: &[u8]) -> Result<(), String> {
            let payload = json!({ "target": target, "data": base64::engine::general_purpose::STANDARD.encode(data) });
            self.0.run_mobile_plugin::<Value>("write", payload).map(|_| ()).map_err(|e| e.to_string())
        }
        fn paired(&self) -> Result<Vec<Value>, String> {
            let v: Value = self.0.run_mobile_plugin("listPaired", json!({})).map_err(|e| e.to_string())?;
            Ok(v["devices"].as_array().cloned().unwrap_or_default())
        }
    }

    pub fn init() -> TauriPlugin<Wry> {
        Builder::new("btprinter")
            .setup(|_app, api| {
                let handle = api.register_android_plugin("ma.burgeros.app", "PrinterPlugin")?;
                // Ask for BLUETOOTH_CONNECT (Android 12+) at start-up, off the setup thread.
                let h2 = handle.clone();
                std::thread::spawn(move || {
                    let _ = h2.run_mobile_plugin::<Value>("requestPermissions", json!({ "permissions": ["bluetooth"] }));
                });
                crate::printing::set_bluetooth(Arc::new(Bridge(handle)));
                Ok(())
            })
            .build()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android_bluetooth::init());
    builder
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::open(&dir.join("burger-os.db"))?;
            let core = Arc::new(Core::new(conn));
            printing::start_worker(core.conn.clone());
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc])
        .run(tauri::generate_context!())
        .expect("error while running BURGER OS");
}

#[cfg(test)]
mod print_pipeline_test;
