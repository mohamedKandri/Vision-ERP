pub mod api;
pub mod audit;
pub mod backup;
pub mod auth;
pub mod client;
pub mod customers;
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
pub mod purchasing;
pub mod reports;
pub mod seed;
pub mod server;
pub mod settings;
pub mod stock;

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
        "device.save_export" => {
            // A report file (CSV or PDF) saved on THIS device, for any logged-in user.
            let ok = if net::is_client() { client::forward("auth.me", token, &Value::Null).is_ok() } else { api::dispatch(core, token, "auth.me", Value::Null).is_ok() };
            if !ok {
                return Err(ApiError::unauthorized());
            }
            let name = export::safe_name(&str_param("name")).ok_or_else(|| ApiError::invalid("bad file name"))?;
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD
                .decode(params.get("data").and_then(|v| v.as_str()).unwrap_or(""))
                .map_err(|_| ApiError::invalid("bad file data"))?;
            if data.is_empty() || data.len() > 20_000_000 {
                return Err(ApiError::invalid("file too large"));
            }
            let path = export::save(&name, &data).map_err(|e| ApiError::new("export_failed", e))?;
            Ok(serde_json::json!({ "path": path }))
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

/// Report files written on this device (Phase 8).
pub mod export {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// (preferred folder, fallback folder) set at start-up.
    pub static DIRS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();

    /// Letters, digits, space, dot, dash, underscore; .csv or .pdf; at most 100 characters.
    pub fn safe_name(name: &str) -> Option<String> {
        let name = name.trim();
        let ok_chars = name.chars().all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c));
        let lower = name.to_ascii_lowercase();
        let ext_ok = lower.ends_with(".csv") || lower.ends_with(".pdf");
        (ok_chars && ext_ok && name.len() > 4 && name.len() <= 100 && !name.starts_with('.') && !name.contains("..")).then(|| name.to_string())
    }

    /// `name`, or `name (2)`, `name (3)`... when the file already exists.
    fn free_path(dir: &std::path::Path, name: &str) -> PathBuf {
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let mut p = dir.join(name);
        let mut n = 2;
        while p.exists() && n < 1000 {
            p = dir.join(format!("{stem} ({n}).{ext}"));
            n += 1;
        }
        p
    }

    fn write_in(dir: &std::path::Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let p = free_path(dir, name);
        std::fs::write(&p, data)?;
        Ok(p)
    }

    /// The Downloads/BURGER-OS folder (where a backup to restore can be put).
    pub fn import_dir() -> Option<PathBuf> {
        DIRS.get().map(|(main, _)| main.clone())
    }

    /// Writes to the Downloads/BURGER-OS folder, else to the app's own exports folder.
    pub fn save(name: &str, data: &[u8]) -> Result<String, String> {
        let (main, fallback) = DIRS.get().ok_or("export folder not ready")?;
        write_in(main, name, data)
            .or_else(|_| write_in(fallback, name, data))
            .map(|p| p.display().to_string())
            .map_err(|e| e.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn names_and_collisions() {
            assert_eq!(safe_name("ventes 2026-11-17.csv").as_deref(), Some("ventes 2026-11-17.csv"));
            assert_eq!(safe_name("Rapport.PDF").as_deref(), Some("Rapport.PDF"));
            for bad in ["../x.csv", "a/b.csv", "a\\b.csv", "x.exe", ".csv", "rapport", "C:x.csv", "ventes\u{0645}.csv"] {
                assert_eq!(safe_name(bad), None, "{bad}");
            }
            let d = tempfile::tempdir().unwrap();
            let a = write_in(d.path(), "r.csv", b"1").unwrap();
            let b = write_in(d.path(), "r.csv", b"2").unwrap();
            assert_eq!(a.file_name().unwrap(), "r.csv");
            assert_eq!(b.file_name().unwrap(), "r (2).csv");
            assert_eq!(std::fs::read(&a).unwrap(), b"1", "never overwrites");
        }
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
            // Debug builds only: BURGER_OS_DATA_DIR points the app at a throwaway data folder,
            // so UI tests never touch the shop's real database.
            let dir = match std::env::var_os("BURGER_OS_DATA_DIR").filter(|_| cfg!(debug_assertions)) {
                Some(d) => std::path::PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            // Exports: Downloads/BURGER-OS (the app's own folder in test builds).
            let fallback = dir.join("exports");
            let main = match app.path().download_dir() {
                Ok(d) if std::env::var_os("BURGER_OS_DATA_DIR").is_none() => d.join("BURGER-OS"),
                _ => fallback.clone(),
            };
            let _ = export::DIRS.set((main, fallback));
            let _ = backup::DIR.set(dir.join("backups"));
            let conn = db::open(&dir.join("burger-os.db"))?;
            let core = Arc::new(Core::new(conn));
            backup::start_daily(core.conn.clone());
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
