//! Client side: forward calls to the Host, relay its live events, and keep new orders in a
//! local outbox while the Wi-Fi is down.

use crate::api::Core;
use crate::error::{ApiError, ApiResult};
use crate::net;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Error codes from the Host are interned so `ApiError` can keep `&'static str` codes.
fn intern(code: &str) -> &'static str {
    static MAP: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut m = MAP.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    if let Some(s) = m.get(code) {
        return s;
    }
    let s: &'static str = Box::leak(code.to_string().into_boxed_str());
    m.insert(code.to_string(), s);
    s
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(3)).timeout_read(Duration::from_secs(20)).build()
}

fn offline(e: impl std::fmt::Display) -> ApiError {
    net::set_online(false);
    ApiError::new("offline", format!("Host unreachable: {e}"))
}

fn decode(v: Value) -> ApiResult<Value> {
    if let Some(ok) = v.get("ok") {
        return Ok(ok.clone());
    }
    let e = &v["err"];
    Err(ApiError {
        code: intern(e["code"].as_str().unwrap_or("internal")),
        message: e["message"].as_str().unwrap_or("").to_string(),
        retry_after_secs: e["retry_after_secs"].as_i64(),
    })
}

/// Ask a Host to pair this device. Returns (token, shop name).
pub fn pair(host: &str, pin: &str, code: Option<&str>, name: &str, kind: &str) -> ApiResult<(String, String)> {
    let resp = agent()
        .post(&format!("http://{host}/pair"))
        .send_json(json!({ "pin": pin, "code": code, "name": name, "kind": kind }))
        .map_err(offline)?;
    let v = decode(resp.into_json::<Value>().map_err(offline)?)?;
    net::set_online(true);
    Ok((v["token"].as_str().unwrap_or("").to_string(), v["shop_name"].as_str().unwrap_or("").to_string()))
}

pub fn forward(method: &str, token: Option<&str>, params: &Value) -> ApiResult<Value> {
    let cfg = net::config();
    if cfg.host.is_empty() || cfg.token.is_empty() {
        return Err(ApiError::new("not_paired", "this device is not paired with a Host"));
    }
    let resp = agent()
        .post(&format!("http://{}/rpc", cfg.host))
        .set("X-Device-Token", &cfg.token)
        .send_json(json!({ "token": token, "method": method, "params": params }));
    let resp = match resp {
        Ok(r) => r,
        // A 4xx/5xx still carries our JSON body.
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(offline(e)),
    };
    let v: Value = resp.into_json().map_err(offline)?;
    net::set_online(true);
    decode(v)
}

/// A call from this device's screen while in Client mode.
pub fn call(core: &Core, token: Option<&str>, method: &str, params: Value) -> ApiResult<Value> {
    if let Some(t) = token {
        *net::LAST_SESSION.lock().unwrap() = Some(t.to_string());
    }
    match forward(method, token, &params) {
        Err(e) if e.code == "offline" && method == "orders.submit" && params.get("order_id").map(|v| v.is_null()).unwrap_or(true) => {
            // New order while the Host is unreachable: keep it and send it later.
            let conn = core.conn.lock().unwrap();
            conn.execute("INSERT INTO outbox (method, params) VALUES (?1, ?2)", params![method, params.to_string()])?;
            Ok(json!({ "offline": true, "outbox_id": conn.last_insert_rowid() }))
        }
        r => r,
    }
}

pub fn outbox_count(core: &Core) -> i64 {
    core.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0)).unwrap_or(0)
}

/// Replay the outbox in order. Stops at the first network failure.
pub fn flush_outbox(core: &Core, emit: &Emit) -> usize {
    let rows: Vec<(i64, String, String)> = {
        let conn = core.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT id, method, params FROM outbox ORDER BY id") {
            Ok(s) => s,
            Err(_) => return 0,
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default()
    };
    let token = net::LAST_SESSION.lock().unwrap().clone();
    let mut sent = 0;
    for (id, method, p) in rows {
        let params: Value = serde_json::from_str(&p).unwrap_or(Value::Null);
        match forward(&method, token.as_deref(), &params) {
            Ok(_) => {
                let _ = core.conn.lock().unwrap().execute("DELETE FROM outbox WHERE id=?1", [id]);
                sent += 1;
            }
            Err(e) if e.code == "offline" => break,
            Err(e) => {
                // Kept for the user to see (e.g. product now out of stock, or login needed).
                let _ = core.conn.lock().unwrap().execute(
                    "UPDATE outbox SET attempts=attempts+1, last_error=?2 WHERE id=?1",
                    params![id, format!("{}: {}", e.code, e.message)],
                );
                if e.code == "unauthorized" {
                    break;
                }
            }
        }
    }
    if sent > 0 {
        emit("burger://changed", json!("orders"));
    }
    sent
}

/// Background threads for Client mode: WebSocket events + outbox replay. Idle in Host mode.
pub fn start(core: Arc<Core>, emit: Emit) {
    let (c2, e2) = (core.clone(), emit.clone());
    std::thread::spawn(move || loop {
        if net::is_client() && net::online() {
            flush_outbox(&c2, &e2);
        }
        std::thread::sleep(Duration::from_secs(4));
    });
    std::thread::spawn(move || {
        let mut was_online = true;
        loop {
            let cfg = net::config();
            if cfg.mode != net::Mode::Client || cfg.token.is_empty() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            match tungstenite::connect(format!("ws://{}/ws?device={}", cfg.host, cfg.token)) {
                Ok((mut ws, _)) => {
                    net::set_online(true);
                    if !was_online {
                        emit("burger://connection", json!({ "online": true }));
                        emit("burger://changed", json!("all"));
                    }
                    was_online = true;
                    flush_outbox(&core, &emit);
                    loop {
                        match ws.read() {
                            Ok(tungstenite::Message::Text(t)) => emit("burger://changed", json!(t.as_str())),
                            Ok(tungstenite::Message::Close(_)) | Err(_) => break,
                            Ok(_) => {}
                        }
                        if net::config().mode != net::Mode::Client {
                            break;
                        }
                    }
                }
                Err(_) => {}
            }
            net::set_online(false);
            if was_online {
                emit("burger://connection", json!({ "online": false }));
            }
            was_online = false;
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}
