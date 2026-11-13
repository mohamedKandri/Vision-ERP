//! Full printing path through the public API: setup -> printers -> order -> background
//! worker -> real TCP sockets standing in for network printers.

use crate::api::{dispatch, Core};
use crate::db::open_in_memory;
use serde_json::{json, Value};
use std::io::Read;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A fake network printer: records the bytes of every connection.
fn fake_printer(listener: TcpListener) -> Arc<Mutex<Vec<Vec<u8>>>> {
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { break };
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            g.lock().unwrap().push(buf);
        }
    });
    got
}

fn wait_for(cond: impl Fn() -> bool, secs: u64) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn login(core: &Core, name: &str, pin: &str) -> String {
    let users = dispatch(core, None, "auth.users", Value::Null).unwrap();
    let id = users.as_array().unwrap().iter().find(|u| u["name"] == name).unwrap()["id"].clone();
    dispatch(core, None, "auth.login", json!({"user_id": id, "pin": pin})).unwrap()["token"].as_str().unwrap().to_string()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn order_to_kitchen_printer_receipt_drawer_and_retry() {
    let core = Core::new(open_in_memory().unwrap());
    dispatch(&core, None, "setup.init", json!({"shop_name": "Burger Lab", "language": "fr", "owner_name": "Owner", "owner_pin": "1234", "demo_users": true, "demo_menu": true})).unwrap();
    let owner = login(&core, "Owner", "1234");
    let cashier = login(&core, "Salma", "3333");

    let grill_l = TcpListener::bind("127.0.0.1:0").unwrap();
    let grill_addr = grill_l.local_addr().unwrap();
    let grill = fake_printer(grill_l);
    let front_l = TcpListener::bind("127.0.0.1:0").unwrap();
    let front_addr = front_l.local_addr().unwrap();
    let front = fake_printer(front_l);

    let gid = dispatch(&core, Some(&owner), "printers.save", json!({"name": "Grill", "kind": "network", "address": grill_addr.to_string(), "paper_mm": 80, "codepage": "cp850"})).unwrap()["id"].clone();
    dispatch(&core, Some(&owner), "printers.save", json!({"name": "Comptoir", "kind": "network", "address": front_addr.to_string(), "paper_mm": 58, "codepage": "cp850", "drawer": true, "receipt_register": "main"})).unwrap();
    let stations = dispatch(&core, Some(&owner), "printers.list", Value::Null).unwrap()["stations"].clone();
    let grill_station = stations.as_array().unwrap().iter().find(|s| s["code"] == "grill").unwrap()["id"].clone();
    dispatch(&core, Some(&owner), "stations.routing", json!({"station_id": grill_station, "printer_id": gid, "show_on_screen": true})).unwrap();
    assert_eq!(dispatch(&core, Some(&cashier), "printers.list", Value::Null).unwrap_err().code, "forbidden", "cashier can't manage printers");

    crate::printing::start_worker(core.conn.clone());

    // Order: 2 Kefta Burger (grill) + 1 Coca (drinks station has no printer).
    let ctx = dispatch(&core, Some(&cashier), "pos.context", json!({"register": "main"})).unwrap();
    assert_eq!(ctx["receipt_printer"]["name"], "Comptoir");
    let find = |fr: &str| ctx["menu"]["products"].as_array().unwrap().iter().find(|p| p["names"]["fr"] == fr).unwrap().clone();
    let kefta = find("Kefta Burger");
    let coca = find("Coca-Cola 33cl");
    let a_point = ctx["menu"]["modifier_groups"].as_array().unwrap().iter().find(|g| g["names"]["fr"] == "Cuisson").unwrap()["options"][1]["id"].clone();
    dispatch(&core, Some(&cashier), "cash.open", json!({"register": "main", "float_cents": 0})).unwrap();
    let order = dispatch(&core, Some(&cashier), "orders.submit", json!({
        "type": "takeaway", "customer_name": "Ali", "send": true, "register": "main",
        "lines": [
            {"config": {"product_id": kefta["id"], "variant_id": null, "modifiers": [{"option_id": a_point, "qty": 1}]}, "qty": 2, "note": "bien cuit"},
            {"config": {"product_id": coca["id"], "variant_id": null, "modifiers": []}, "qty": 1}
        ]})).unwrap();
    assert!(wait_for(|| !grill.lock().unwrap().is_empty(), 10), "kitchen ticket reached the grill printer");
    let ticket = text(&grill.lock().unwrap()[0]);
    assert!(ticket.contains("2 x Kefta Burger") && ticket.contains("NOUVELLE") && ticket.contains("Ali") && ticket.contains("!! bien cuit"), "{ticket}");
    assert!(!ticket.contains("Coca"), "drinks station has no printer");
    assert!(grill.lock().unwrap()[0].ends_with(&[0x1B, b'd', 3, 0x1D, b'V', 66, 0]), "ticket is cut");

    // Cash payment, then receipt with drawer kick.
    let oid = order["id"].clone();
    dispatch(&core, Some(&cashier), "orders.pay", json!({"order_id": oid, "register": "main", "payments": [{"method": "cash", "amount_cents": order["total_cents"], "tendered_cents": 20000}]})).unwrap();
    dispatch(&core, Some(&cashier), "print.receipt", json!({"register": "main", "order_id": oid, "kick": true,
        "doc": [{"kind": "text", "text": "Burger Lab", "align": "center", "bold": true, "big": true}, {"kind": "row", "left": "TOTAL TTC", "right": "112,00 DH", "bold": true}]})).unwrap();
    assert!(wait_for(|| front.lock().unwrap().len() >= 2, 10), "drawer + receipt reached the front printer");
    let f = front.lock().unwrap().clone();
    assert!(f[0].windows(5).any(|w| w == [0x1B, b'p', 0, 25, 250]), "drawer kick first");
    assert!(text(&f[1]).contains("TOTAL TTC") && text(&f[1]).contains("112,00 DH"));

    // The grill printer goes offline (unreachable address): the add-on ticket fails, stays queued, and is reported.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    dispatch(&core, Some(&owner), "printers.save", json!({"id": gid, "name": "Grill", "kind": "network", "address": dead_addr.to_string(), "paper_mm": 80, "codepage": "cp850"})).unwrap();
    dispatch(&core, Some(&cashier), "orders.reopen", json!({"order_id": oid, "approver_pin": "2222"})).unwrap();
    dispatch(&core, Some(&cashier), "orders.submit", json!({"order_id": oid, "send": true, "register": "main",
        "lines": [{"config": {"product_id": kefta["id"], "variant_id": null, "modifiers": [{"option_id": a_point, "qty": 1}]}, "qty": 1}]})).unwrap();
    assert!(wait_for(|| {
        let p = dispatch(&core, Some(&cashier), "print.problems", Value::Null).unwrap();
        p.as_array().map(|a| a.iter().any(|x| x["name"] == "Grill" && x["failed"] == 1)).unwrap_or(false)
    }, 10), "failed kitchen ticket is reported to the till");
    assert_eq!(grill.lock().unwrap().len(), 1);

    // Printer back + retry: the queued "AJOUT" ticket prints; nothing was lost.
    dispatch(&core, Some(&owner), "printers.save", json!({"id": gid, "name": "Grill", "kind": "network", "address": grill_addr.to_string(), "paper_mm": 80, "codepage": "cp850"})).unwrap();
    dispatch(&core, Some(&cashier), "print.retry", json!({"printer_id": gid})).unwrap();
    assert!(wait_for(|| grill.lock().unwrap().len() == 2, 10), "queued ticket printed after retry");
    let add = text(&grill.lock().unwrap()[1]);
    assert!(add.contains("AJOUT") && add.contains("1 x Kefta Burger") && !add.contains("2 x"), "{add}");
    assert!(wait_for(|| dispatch(&core, Some(&cashier), "print.problems", Value::Null).unwrap().as_array().unwrap().is_empty(), 5));

    // Drawer button and kitchen reprint go through the same queue.
    dispatch(&core, Some(&cashier), "drawer.open", json!({"register": "main", "reason": "manual"})).unwrap();
    dispatch(&core, Some(&cashier), "print.kitchen_reprint", json!({"id": oid})).unwrap();
    assert!(wait_for(|| grill.lock().unwrap().len() == 3 && front.lock().unwrap().len() == 3, 10));
    // "RÉIMPRESSION" in code page 850 (É = 0x90).
    assert!(grill.lock().unwrap()[2].windows(12).any(|w| w == [b"R".as_slice(), &[0x90], b"IMPRESSION"].concat().as_slice()), "reprint ticket");
}
