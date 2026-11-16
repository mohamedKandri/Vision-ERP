pub mod api;
pub mod audit;
pub mod auth;
pub mod client;
pub mod cash;
pub mod db;
pub mod error;
pub mod escpos;
pub mod floors;
pub mod item;
pub mod kds;
pub mod net;
pub mod menu;
pub mod orders;
pub mod pricing;
pub mod printing;
pub mod seed;
pub mod server;
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
    tauri::async_runtime::spawn_blocking(move || {
        let params = params.unwrap_or(Value::Null);
        if method.starts_with("device.") {
            device_local(&core, token.as_deref(), &method, params)
        } else if net::is_client() {
            client::call(&core, token.as_deref(), &method, params)
        } else {
            api::handle(&core, token.as_deref(), &method, params)
        }
    })
    .await
    .map_err(|e| ApiError::new("internal", e.to_string()))?
}

/// Methods about this device itself (never forwarded to the Host).
fn device_local(core: &Core, token: Option<&str>, method: &str, params: Value) -> Result<Value, ApiError> {
    let cfg = net::config();
    let str_param = |k: &str| params.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    // Changing the role of a device that already has a shop needs its owner (or a Host manager).
    let allowed = |core: &Core| -> Result<(), ApiError> {
        if net::is_client() {
            let me = client::forward("auth.me", token, &Value::Null);
            return match me {
                Ok(u) if u["permissions"].as_array().map(|a| a.iter().any(|p| p == "settings.manage")).unwrap_or(false) => Ok(()),
                Err(e) if e.code == "offline" => Ok(()), // escape hatch: Host gone for good
                _ => Err(ApiError::forbidden("settings.manage")),
            };
        }
        let users: i64 = core.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if users == 0 {
            return Ok(());
        }
        match api::dispatch(core, token, "auth.me", Value::Null) {
            Ok(u) if u["role"] == "owner" => Ok(()),
            _ => Err(ApiError::forbidden("owner")),
        }
    };
    match method {
        "device.get" => Ok(serde_json::json!({
            "mode": cfg.mode, "host": cfg.host, "name": cfg.name, "kind": cfg.kind, "register": cfg.register,
            "port": cfg.port, "shop_name": cfg.shop_name, "paired": !cfg.token.is_empty(),
            "addresses": net::local_ipv4().iter().map(|ip| format!("{ip}:{}", cfg.port)).collect::<Vec<_>>(),
            "online": net::online(), "outbox": client::outbox_count(core),
            "server_error": net::SERVER_ERROR.lock().unwrap().clone(),
        })),
        "device.join" => {
            allowed(core)?;
            let mut host = str_param("host");
            if host.is_empty() {
                return Err(ApiError::invalid("Host address required"));
            }
            if !host.contains(':') {
                host = format!("{host}:{}", net::DEFAULT_PORT);
            }
            let name = str_param("name");
            let kind = { let k = str_param("kind"); if k.is_empty() { "pos".to_string() } else { k } };
            let code = str_param("code");
            let (token, shop) = client::pair(&host, &str_param("pin"), if code.is_empty() { None } else { Some(&code) }, &name, &kind)?;
            let register = { let r = str_param("register"); if r.is_empty() { cfg.register.clone() } else { r } };
            net::save(net::DeviceConfig { mode: net::Mode::Client, host, token, name, kind, register, port: cfg.port, shop_name: shop })?;
            device_local(core, None, "device.get", Value::Null)
        }
        "device.leave" => {
            allowed(core)?;
            net::save(net::DeviceConfig { mode: net::Mode::Host, port: cfg.port, register: cfg.register.clone(), kind: cfg.kind.clone(), name: cfg.name.clone(), ..Default::default() })?;
            device_local(core, None, "device.get", Value::Null)
        }
        "device.update" => {
            // This device's own label, screen type and register; any logged-in user.
            let ok = if net::is_client() { client::forward("auth.me", token, &Value::Null).is_ok() } else { api::dispatch(core, token, "auth.me", Value::Null).is_ok() };
            if !ok {
                return Err(ApiError::unauthorized());
            }
            let mut c = cfg.clone();
            let (n, k, r) = (str_param("name"), str_param("kind"), str_param("register"));
            if !n.is_empty() { c.name = n; }
            if ["pos", "kds", "display", "office"].contains(&k.as_str()) { c.kind = k; }
            if !r.is_empty() && r.len() <= 40 { c.register = r; }
            net::save(c)?;
            device_local(core, None, "device.get", Value::Null)
        }
        _ => Err(ApiError::new("unknown_method", method.to_string())),
    }
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
            let cfg = net::load(dir.join("device.json"));
            printing::start_worker(core.conn.clone());

            // Host: LAN server for paired devices (it refuses work while in Client mode).
            tauri::async_runtime::spawn(server::serve(core.clone(), cfg.port));
            // Push change events to this device's own screen.
            let handle = app.handle().clone();
            let mut rx = net::events().subscribe();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                while let Ok(topic) = rx.recv().await {
                    let _ = handle.emit("burger://changed", topic);
                }
            });
            // Client: relay the Host's events and replay the offline outbox.
            let handle = app.handle().clone();
            client::start(core.clone(), Arc::new(move |ev: &str, payload: Value| {
                use tauri::Emitter;
                let _ = handle.emit(ev, payload);
            }));
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc])
        .run(tauri::generate_context!())
        .expect("error while running BURGER OS");
}

#[cfg(test)]
mod print_pipeline_test;

#[cfg(test)]
mod lan_test;
