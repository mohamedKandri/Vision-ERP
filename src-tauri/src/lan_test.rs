//! Two devices in one test: a real Host server on a local socket, and a "client" talking
//! to it over HTTP + WebSocket exactly like a second tablet would.

use crate::api::{dispatch, Core};
use crate::db::open_in_memory;
use crate::net;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn post(url: &str, device: Option<&str>, body: Value) -> Value {
    let mut req = ureq::post(url);
    if let Some(d) = device {
        req = req.set("X-Device-Token", d);
    }
    match req.send_json(body) {
        Ok(r) => r.into_json().unwrap(),
        Err(ureq::Error::Status(_, r)) => r.into_json().unwrap(),
        Err(e) => panic!("{e}"),
    }
}

fn ok(v: Value) -> Value {
    assert!(v.get("ok").is_some(), "expected ok, got {v}");
    v["ok"].clone()
}

#[test]
fn host_and_client_over_lan() {
    // ---- Host
    let core = Arc::new(Core::new(open_in_memory().unwrap()));
    dispatch(&core, None, "setup.init", json!({"shop_name": "Burger Lab", "language": "fr", "owner_name": "Owner", "owner_pin": "1234", "demo_users": true, "demo_menu": true})).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let c2 = core.clone();
    rt.spawn(async move {
        axum::serve(listener, crate::server::router(c2).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    let base = format!("http://{addr}");

    let ping: Value = ureq::get(&format!("{base}/ping")).call().unwrap().into_json().unwrap();
    assert_eq!(ping["shop_name"], "Burger Lab");

    // ---- Pairing: owner opens a window on the Host; the tablet uses the PIN.
    let users = dispatch(&core, None, "auth.users", Value::Null).unwrap();
    let uid = |n: &str| users.as_array().unwrap().iter().find(|u| u["name"] == n).unwrap()["id"].clone();
    let owner = dispatch(&core, None, "auth.login", json!({"user_id": uid("Owner"), "pin": "1234"})).unwrap()["token"].as_str().unwrap().to_string();
    let cashier_local = dispatch(&core, None, "auth.login", json!({"user_id": uid("Salma"), "pin": "3333"})).unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!(dispatch(&core, Some(&cashier_local), "devices.pairing_start", Value::Null).unwrap_err().code, "forbidden");
    let window = dispatch(&core, Some(&owner), "devices.pairing_start", Value::Null).unwrap();
    let pin = window["pin"].as_str().unwrap().to_string();
    let wrong = if pin == "000000" { "111111" } else { "000000" };
    assert_eq!(post(&format!("{base}/pair"), None, json!({"pin": wrong, "name": "Caisse 2", "kind": "pos"}))["err"]["code"], "wrong_pin");
    let paired = ok(post(&format!("{base}/pair"), None, json!({"pin": pin, "name": "Caisse 2", "kind": "pos"})));
    let device = paired["token"].as_str().unwrap().to_string();
    assert_eq!(paired["shop_name"], "Burger Lab");

    // ---- No device token, no service.
    assert_eq!(post(&format!("{base}/rpc"), None, json!({"method": "auth.users"}))["err"]["code"], "device_unknown");
    assert_eq!(post(&format!("{base}/rpc"), Some("forged"), json!({"method": "auth.users"}))["err"]["code"], "device_unknown");

    // ---- Live events over WebSocket.
    let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/ws?device={device}")).unwrap();
    assert!(tungstenite::connect(format!("ws://{addr}/ws?device=forged")).is_err(), "unpaired device can't listen");
    let rpc = |token: Option<&str>, method: &str, params: Value| post(&format!("{base}/rpc"), Some(&device), json!({"token": token, "method": method, "params": params}));

    // ---- Remote cashier logs in on the tablet and sends an order.
    let cashier = ok(rpc(None, "auth.login", json!({"user_id": uid("Salma"), "pin": "3333"})))["token"].as_str().unwrap().to_string();
    assert_eq!(rpc(Some(&cashier), "settings.set", json!({"shop_name": "x"}))["err"]["code"], "forbidden", "same permissions as on the Host");
    let ctx = ok(rpc(Some(&cashier), "pos.context", json!({"register": "caisse2"})));
    let find = |fr: &str| ctx["menu"]["products"].as_array().unwrap().iter().find(|p| p["names"]["fr"] == fr).unwrap()["id"].clone();
    let order = ok(rpc(Some(&cashier), "orders.submit", json!({"type": "takeaway", "send": true, "register": "caisse2",
        "lines": [{"config": {"product_id": find("Tiramisu"), "variant_id": null, "modifiers": []}, "qty": 1},
                  {"config": {"product_id": find("Coca-Cola 33cl"), "variant_id": null, "modifiers": []}, "qty": 1}]})));
    let oid = order["id"].as_i64().unwrap();
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_mut() { s.set_read_timeout(Some(Duration::from_secs(3))).ok(); }
    let msg = loop {
        let m = ws.read().unwrap();
        if m.is_text() {
            break m;
        }
    };
    assert_eq!(msg.to_text().unwrap(), "orders", "tablet is told that orders changed");

    // ---- Kitchen screen on another device: bump both stations -> order ready -> customer board.
    let kitchen = ok(rpc(None, "auth.login", json!({"user_id": uid("Youssef"), "pin": "4444"})))["token"].as_str().unwrap().to_string();
    assert_eq!(rpc(Some(&kitchen), "orders.submit", json!({}))["err"]["code"], "forbidden", "kitchen can't sell");
    let stations = ok(rpc(Some(&kitchen), "kds.stations", Value::Null));
    let sid = |code: &str| stations.as_array().unwrap().iter().find(|s| s["code"] == code).unwrap()["id"].clone();
    let board = ok(rpc(Some(&kitchen), "kds.board", json!({"station_id": sid("drinks")})));
    assert_eq!(board[0]["order_id"], oid);
    ok(rpc(Some(&kitchen), "kds.bump", json!({"order_id": oid, "station_id": sid("drinks")})));
    ok(rpc(Some(&kitchen), "kds.bump", json!({"order_id": oid, "station_id": sid("assembly")})));
    let o = ok(rpc(Some(&cashier), "orders.get", json!({"id": oid})));
    assert_eq!(o["status"], "ready");
    let display = ok(rpc(None, "display.board", Value::Null)); // public: no user needed
    assert!(display["ready"].as_array().unwrap().iter().any(|n| n == &json!(o["number"].as_i64().unwrap().to_string())));

    // ---- Revoked tablet is cut off.
    let devices = dispatch(&core, Some(&owner), "devices.list", Value::Null).unwrap();
    let did = devices[0]["id"].clone();
    assert_eq!(devices[0]["name"], "Caisse 2");
    dispatch(&core, Some(&owner), "devices.revoke", json!({"id": did})).unwrap();
    assert_eq!(rpc(Some(&cashier), "orders.list", Value::Null)["err"]["code"], "device_unknown");

    // ---- Offline client: a new order is kept in the outbox, then replayed when the Host is back.
    // One process plays both roles here, so the device role stays "Host" (the server keeps
    // serving) and only the forwarder's address/token are pointed around.
    let client_core = Core::new(open_in_memory().unwrap());
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap(); // closed port
    net::save(net::DeviceConfig { host: dead.to_string(), token: "t".into(), ..Default::default() }).unwrap();
    let r = crate::client::call(&client_core, Some(&cashier), "orders.submit", json!({"type": "takeaway", "send": true,
        "lines": [{"config": {"product_id": find("Cookie"), "variant_id": null, "modifiers": []}, "qty": 1}]})).unwrap();
    assert_eq!(r["offline"], true);
    assert!(!net::online());
    assert_eq!(crate::client::outbox_count(&client_core), 1);
    // Existing orders and payments are not queued: they need the Host.
    assert_eq!(crate::client::call(&client_core, Some(&cashier), "orders.pay", json!({"order_id": oid})).unwrap_err().code, "offline");

    // Host reachable again with a fresh pairing (the old token was revoked).
    let w = dispatch(&core, Some(&owner), "devices.pairing_start", Value::Null).unwrap();
    let (token2, _) = crate::client::pair(&addr.to_string(), w["pin"].as_str().unwrap(), None, "Caisse 2", "pos").unwrap();
    net::save(net::DeviceConfig { host: addr.to_string(), token: token2, ..Default::default() }).unwrap();
    let t2 = crate::client::call(&client_core, None, "auth.login", json!({"user_id": uid("Salma"), "pin": "3333"})).unwrap()["token"].as_str().unwrap().to_string();
    *net::LAST_SESSION.lock().unwrap() = Some(t2);
    let before = dispatch(&core, Some(&owner), "orders.list", json!({"scope": "today"})).unwrap().as_array().unwrap().len();
    let emitted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let e2 = emitted.clone();
    let emit: crate::client::Emit = Arc::new(move |ev, _| e2.lock().unwrap().push(ev.to_string()));
    let start = Instant::now();
    assert_eq!(crate::client::flush_outbox(&client_core, &emit), 1, "queued order replayed");
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(net::online());
    assert_eq!(crate::client::outbox_count(&client_core), 0);
    let after = dispatch(&core, Some(&owner), "orders.list", json!({"scope": "today"})).unwrap().as_array().unwrap().len();
    assert_eq!(after, before + 1, "the offline order now exists on the Host");
    assert_eq!(emitted.lock().unwrap().as_slice(), ["burger://changed"]);
    net::save(net::DeviceConfig::default()).unwrap();
}
