//! Host LAN server: `/ping`, `/pair`, `/rpc`, `/ws`. Every call except `/ping` and `/pair`
//! needs a paired device token (`X-Device-Token`, or `?device=` for the WebSocket).
//! User permissions are then checked by the same `dispatch` as on the Host itself.

use crate::api::{self, Core};
use crate::error::ApiError;
use crate::net;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

pub fn router(core: Arc<Core>) -> Router {
    Router::new()
        .route("/ping", get(ping))
        .route("/pair", post(pair))
        .route("/rpc", post(rpc))
        .route("/ws", get(ws))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(core)
}

pub async fn serve(core: Arc<Core>, port: u16) {
    match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => {
            *net::SERVER_ERROR.lock().unwrap() = None;
            let _ = axum::serve(l, router(core).into_make_service_with_connect_info::<SocketAddr>()).await;
        }
        Err(e) => *net::SERVER_ERROR.lock().unwrap() = Some(format!("port {port}: {e}")),
    }
}

fn err(e: ApiError) -> Json<Value> {
    Json(json!({ "err": e }))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("worker panicked")
}

async fn ping(State(core): State<Arc<Core>>) -> Json<Value> {
    let shop = blocking(move || {
        let conn = core.conn.lock().unwrap();
        crate::settings::get(&conn, "shop_name").ok()
    })
    .await;
    Json(json!({ "app": "burger-os", "version": env!("CARGO_PKG_VERSION"), "host": !net::is_client(), "shop_name": shop }))
}

#[derive(Deserialize)]
struct PairIn {
    pin: String,
    code: Option<String>,
    name: String,
    kind: String,
}

async fn pair(State(core): State<Arc<Core>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Json(p): Json<PairIn>) -> Json<Value> {
    if net::is_client() {
        return err(ApiError::new("not_host", "this device is not the Host"));
    }
    let r = blocking(move || {
        net::check_pairing(&p.pin, p.code.as_deref())?;
        let conn = core.conn.lock().unwrap();
        let token = net::register_device(&conn, &p.name, &p.kind)?;
        let shop = crate::settings::get(&conn, "shop_name")?;
        net::publish("devices");
        Ok::<_, ApiError>(json!({ "token": token, "shop_name": shop, "from": addr.ip().to_string() }))
    })
    .await;
    match r {
        Ok(v) => Json(json!({ "ok": v })),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct RpcIn {
    token: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn rpc(State(core): State<Arc<Core>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<RpcIn>) -> Json<Value> {
    if net::is_client() {
        return err(ApiError::new("not_host", "this device is not the Host"));
    }
    let device = headers.get("x-device-token").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let ip = addr.ip().to_string();
    let r = blocking(move || {
        {
            let conn = core.conn.lock().unwrap();
            net::check_device(&conn, &device, &ip)?;
        }
        api::handle(&core, body.token.as_deref(), &body.method, body.params)
    })
    .await;
    match r {
        Ok(v) => Json(json!({ "ok": v })),
        Err(e) => err(e),
    }
}

async fn ws(State(core): State<Arc<Core>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(q): Query<HashMap<String, String>>, up: WebSocketUpgrade) -> impl IntoResponse {
    let device = q.get("device").cloned().unwrap_or_default();
    let ip = addr.ip().to_string();
    let ok = blocking(move || {
        let conn = core.conn.lock().unwrap();
        net::check_device(&conn, &device, &ip).is_ok()
    })
    .await;
    if !ok || net::is_client() {
        return (axum::http::StatusCode::UNAUTHORIZED, "device not paired").into_response();
    }
    up.on_upgrade(stream).into_response()
}

async fn stream(mut socket: WebSocket) {
    let mut rx = net::events().subscribe();
    let every = std::time::Duration::from_secs(20);
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(topic) => if socket.send(Message::Text(topic.into())).await.is_err() { break },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Missed events: tell the client to refresh everything.
                    if socket.send(Message::Text("all".into())).await.is_err() { break }
                }
                Err(_) => break,
            },
            _ = ping.tick() => if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
