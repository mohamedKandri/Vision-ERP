//! Device role (Host / Client), pairing, and change events.
//!
//! - Host: owns the database, serves `/rpc`, `/pair`, `/ws` on the LAN (server.rs).
//! - Client: forwards every call to the Host and relays its events (client.rs).

use crate::error::{ApiError, ApiResult};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 7420;
pub const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
pub const PAIRING_MAX_TRIES: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Host,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub mode: Mode,
    /// Client: "192.168.1.20:7420".
    #[serde(default)]
    pub host: String,
    /// Client: secret given by the Host at pairing.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub name: String,
    /// pos | kds | display | office
    #[serde(default = "pos")]
    pub kind: String,
    #[serde(default = "main_register")]
    pub register: String,
    /// Host: port to listen on.
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub shop_name: String,
}
fn pos() -> String {
    "pos".into()
}
fn main_register() -> String {
    "main".into()
}
fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self { mode: Mode::Host, host: String::new(), token: String::new(), name: String::new(), kind: pos(), register: main_register(), port: DEFAULT_PORT, shop_name: String::new() }
    }
}

static CONFIG: OnceLock<RwLock<DeviceConfig>> = OnceLock::new();
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
pub static ONLINE: AtomicBool = AtomicBool::new(true);
pub static SERVER_ERROR: Mutex<Option<String>> = Mutex::new(None);
/// Client: latest user session token, used to replay the outbox.
pub static LAST_SESSION: Mutex<Option<String>> = Mutex::new(None);

pub fn load(path: PathBuf) -> DeviceConfig {
    let cfg: DeviceConfig = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let _ = CONFIG_PATH.set(path);
    let _ = CONFIG.set(RwLock::new(cfg.clone()));
    cfg
}

pub fn config() -> DeviceConfig {
    CONFIG.get().map(|c| c.read().unwrap().clone()).unwrap_or_default()
}

pub fn save(cfg: DeviceConfig) -> ApiResult<()> {
    if let Some(p) = CONFIG_PATH.get() {
        std::fs::write(p, serde_json::to_string_pretty(&cfg).unwrap()).map_err(|e| ApiError::new("internal", format!("save device config: {e}")))?;
    }
    match CONFIG.get() {
        Some(c) => *c.write().unwrap() = cfg,
        None => {
            let _ = CONFIG.set(RwLock::new(cfg));
        }
    }
    Ok(())
}

pub fn is_client() -> bool {
    config().mode == Mode::Client
}

pub fn local_ipv4() -> Vec<String> {
    if_addrs::get_if_addrs()
        .map(|v| {
            v.into_iter()
                .filter(|i| !i.is_loopback())
                .filter_map(|i| match i.ip() {
                    std::net::IpAddr::V4(ip) if !ip.is_link_local() => Some(ip.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ events

static EVENTS: OnceLock<tokio::sync::broadcast::Sender<String>> = OnceLock::new();

pub fn events() -> &'static tokio::sync::broadcast::Sender<String> {
    EVENTS.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// Topic of a successful call, or None for read-only methods.
pub fn topic_of(method: &str) -> Option<&'static str> {
    let read_suffixes = [
        ".get", ".list", ".board", ".status", ".context", ".me", ".users", ".problems", ".jobs", ".job", ".current", ".report",
        ".sessions", ".discover", ".info", ".stations", ".history", ".alerts", ".food_cost", ".suggestions", ".summary", ".lookup",
    ];
    if read_suffixes.iter().any(|s| method.ends_with(s)) || method.starts_with("auth.") || method == "app.status" {
        return None;
    }
    if method == "backup.restore" {
        return Some("all"); // every screen reloads (and goes back to the login)
    }
    Some(match method.split('.').next().unwrap_or("") {
        "orders" | "kds" | "loyalty" => "orders",
        "customers" => "customers",
        "cash" | "drawer" => "cash",
        "menu" | "products" | "categories" | "modifier_groups" => "menu",
        "print" | "printers" | "stations" => "print",
        "settings" | "taxes" | "tables" => "settings",
        "users" | "permissions" => "users",
        "devices" => "devices",
        "ingredients" | "recipes" | "stock" | "counts" | "suppliers" | "purchases" => "stock",
        _ => "other",
    })
}

pub fn publish(topic: &str) {
    let _ = events().send(topic.to_string());
}

// ------------------------------------------------------------------ pairing (Host)

struct Pairing {
    pin: String,
    code: String,
    expires: Instant,
    tries: u32,
}

static PAIRING: Mutex<Option<Pairing>> = Mutex::new(None);

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Start a pairing window: a 6-digit PIN (typed on the new device) and a code carried by the QR.
pub fn start_pairing() -> Value {
    let mut rng = rand::thread_rng();
    let pin = format!("{:06}", rng.gen_range(0..1_000_000));
    let code: String = (0..8).map(|_| char::from(b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"[rng.gen_range(0..32)])).collect();
    let cfg = config();
    let ips = local_ipv4();
    let host = ips.first().map(|ip| format!("{ip}:{}", cfg.port)).unwrap_or_default();
    *PAIRING.lock().unwrap() = Some(Pairing { pin: pin.clone(), code: code.clone(), expires: Instant::now() + PAIRING_TTL, tries: 0 });
    json!({
        "pin": pin, "code": code, "expires_in": PAIRING_TTL.as_secs(), "port": cfg.port,
        "addresses": ips.iter().map(|ip| format!("{ip}:{}", cfg.port)).collect::<Vec<_>>(),
        "qr": format!("burgeros://pair?host={host}&code={code}&pin={pin}"),
    })
}

/// Check a pairing attempt. The PIN alone is enough (typed by hand); the QR also carries the code.
/// After 5 wrong tries the window closes and the owner must start a new one.
pub fn check_pairing(pin: &str, code: Option<&str>) -> ApiResult<()> {
    let mut guard = PAIRING.lock().unwrap();
    let p = guard.as_mut().ok_or_else(|| ApiError::new("pairing_closed", "start pairing on the Host first"))?;
    if Instant::now() > p.expires {
        *guard = None;
        return Err(ApiError::new("pairing_closed", "pairing expired"));
    }
    let ok = p.pin == pin && code.map(|c| c.eq_ignore_ascii_case(&p.code)).unwrap_or(true);
    if !ok {
        p.tries += 1;
        if p.tries >= PAIRING_MAX_TRIES {
            *guard = None;
            return Err(ApiError::new("pairing_closed", "too many wrong PINs; start pairing again"));
        }
        return Err(ApiError::new("wrong_pin", "wrong pairing PIN"));
    }
    *guard = None; // one device per pairing window
    Ok(())
}

pub fn register_device(conn: &Connection, name: &str, kind: &str) -> ApiResult<String> {
    if !["pos", "kds", "display", "office"].contains(&kind) {
        return Err(ApiError::invalid("unknown device kind"));
    }
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError::invalid("device name required"));
    }
    let mut b = [0u8; 32];
    rand::thread_rng().fill(&mut b);
    let token = hex::encode(b);
    conn.execute("INSERT INTO devices (name, kind, token_hash) VALUES (?1, ?2, ?3)", params![name, kind, sha256_hex(&token)])?;
    crate::audit::log(conn, None, "device.paired", Some("device"), Some(&conn.last_insert_rowid().to_string()), Some(json!({"name": name, "kind": kind})))?;
    Ok(token)
}

/// Host: is this device token valid? Records last seen.
pub fn check_device(conn: &Connection, token: &str, ip: &str) -> ApiResult<i64> {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM devices WHERE token_hash=?1 AND revoked=0", [sha256_hex(token)], |r| r.get(0))
        .optional()?;
    let id = id.ok_or_else(|| ApiError::new("device_unknown", "this device is not paired (or was removed)"))?;
    conn.execute("UPDATE devices SET last_seen=strftime('%Y-%m-%dT%H:%M:%fZ','now'), last_ip=?2 WHERE id=?1", params![id, ip])?;
    Ok(id)
}

pub fn list_devices(conn: &Connection) -> ApiResult<Value> {
    let rows: Vec<Value> = conn
        .prepare("SELECT id, name, kind, created_at, last_seen, last_ip, revoked FROM devices ORDER BY revoked, name")?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "kind": r.get::<_, String>(2)?,
                      "created_at": r.get::<_, String>(3)?, "last_seen": r.get::<_, Option<String>>(4)?,
                      "last_ip": r.get::<_, Option<String>>(5)?, "revoked": r.get::<_, bool>(6)?}))
        })?
        .collect::<Result<_, _>>()?;
    Ok(json!(rows))
}

pub fn revoke_device(conn: &Connection, user: i64, id: i64) -> ApiResult<()> {
    if conn.execute("UPDATE devices SET revoked=1 WHERE id=?1", [id])? == 0 {
        return Err(ApiError::not_found("device"));
    }
    crate::audit::log(conn, Some(user), "device.revoked", Some("device"), Some(&id.to_string()), None)?;
    publish("devices");
    Ok(())
}

pub fn set_online(v: bool) {
    ONLINE.store(v, Ordering::SeqCst);
}

pub fn online() -> bool {
    ONLINE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn pairing_window_rules() {
        assert_eq!(check_pairing("123456", None).unwrap_err().code, "pairing_closed");
        let p = start_pairing();
        let pin = p["pin"].as_str().unwrap().to_string();
        let code = p["code"].as_str().unwrap().to_string();
        assert_eq!(pin.len(), 6);
        assert!(p["qr"].as_str().unwrap().contains(&code));
        let wrong = if pin == "000000" { "111111" } else { "000000" };
        assert_eq!(check_pairing(wrong, None).unwrap_err().code, "wrong_pin");
        assert_eq!(check_pairing(&pin, Some("WRONGCOD")).unwrap_err().code, "wrong_pin");
        check_pairing(&pin, Some(&code.to_lowercase())).unwrap();
        assert_eq!(check_pairing(&pin, None).unwrap_err().code, "pairing_closed", "one device per window");
        // 5 wrong tries close the window.
        let p = start_pairing();
        let pin = p["pin"].as_str().unwrap().to_string();
        let wrong = if pin == "000000" { "111111" } else { "000000" };
        for _ in 0..4 {
            assert_eq!(check_pairing(wrong, None).unwrap_err().code, "wrong_pin");
        }
        assert_eq!(check_pairing(wrong, None).unwrap_err().code, "pairing_closed");
        assert_eq!(check_pairing(&pin, None).unwrap_err().code, "pairing_closed");
    }

    #[test]
    fn device_tokens_are_hashed_and_revocable() {
        let c = open_in_memory().unwrap();
        let token = register_device(&c, "Caisse 2", "pos").unwrap();
        let stored: String = c.query_row("SELECT token_hash FROM devices", [], |r| r.get(0)).unwrap();
        assert_ne!(stored, token);
        assert_eq!(stored, sha256_hex(&token));
        let id = check_device(&c, &token, "192.168.1.30").unwrap();
        assert_eq!(check_device(&c, "nope", "x").unwrap_err().code, "device_unknown");
        c.execute("INSERT INTO users (id, name, role, pin_hash) VALUES (1,'O','owner','x')", []).unwrap();
        revoke_device(&c, 1, id).unwrap();
        assert_eq!(check_device(&c, &token, "x").unwrap_err().code, "device_unknown");
        assert!(register_device(&c, "x", "toaster").is_err());
    }

    #[test]
    fn topics() {
        assert_eq!(topic_of("orders.submit"), Some("orders"));
        assert_eq!(topic_of("kds.bump"), Some("orders"));
        assert_eq!(topic_of("orders.list"), None);
        assert_eq!(topic_of("pos.context"), None);
        assert_eq!(topic_of("auth.login"), None);
        assert_eq!(topic_of("products.save"), Some("menu"));
        assert_eq!(topic_of("products.set_available"), Some("menu"));
    }
}
