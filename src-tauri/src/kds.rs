//! Kitchen Display System: per-station items, tap-to-done, bump, recall, and the order
//! becoming "ready" when every station has finished. Also the public customer board.

use crate::db::atomic;
use crate::error::{ApiError, ApiResult};
use crate::item::MenuIndex;
use crate::printing;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

/// Called in the same transaction that sends lines to the kitchen.
pub fn create_parts(conn: &Connection, order_id: i64, line_ids: &[i64]) -> ApiResult<()> {
    if line_ids.is_empty() {
        return Ok(());
    }
    let screens: Vec<i64> = conn
        .prepare("SELECT id FROM stations WHERE active=1 AND show_on_screen=1")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if screens.is_empty() {
        return Ok(());
    }
    let ix = MenuIndex::load(conn)?;
    let fr = printing::parts(conn, &ix, line_ids, "fr")?;
    let ar = printing::parts(conn, &ix, line_ids, "ar")?;
    let en = printing::parts(conn, &ix, line_ids, "en")?;
    for ((f, a), e) in fr.iter().zip(&ar).zip(&en) {
        let Some(st) = f.station_id.filter(|s| screens.contains(s)) else { continue };
        conn.execute(
            "INSERT INTO kitchen_parts (order_id, line_id, part_index, station_id, qty, name, details, note) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                order_id, f.line_id, f.part_index, st, f.qty,
                json!({"fr": f.name, "ar": a.name, "en": e.name}).to_string(),
                json!({"fr": f.details, "ar": a.details, "en": e.details}).to_string(),
                f.note
            ],
        )?;
    }
    Ok(())
}

pub fn void_line(conn: &Connection, line_id: i64) -> ApiResult<()> {
    conn.execute("UPDATE kitchen_parts SET voided=1 WHERE line_id=?1", [line_id])?;
    Ok(())
}

pub fn void_order(conn: &Connection, order_id: i64) -> ApiResult<()> {
    conn.execute("UPDATE kitchen_parts SET voided=1 WHERE order_id=?1", [order_id])?;
    Ok(())
}

/// The POS marked the order ready/served: clear it from every screen.
pub fn clear_order(conn: &Connection, order_id: i64) -> ApiResult<()> {
    conn.execute(
        "UPDATE kitchen_parts SET done_at=COALESCE(done_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
           bumped_at=COALESCE(bumped_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')) WHERE order_id=?1",
        [order_id],
    )?;
    Ok(())
}

/// Move the order along: some items done -> preparing; all done -> ready.
fn progress(conn: &Connection, order_id: i64) -> ApiResult<()> {
    let (total, done): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COUNT(done_at) FROM kitchen_parts WHERE order_id=?1 AND voided=0",
        [order_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let status: String = conn.query_row("SELECT status FROM orders WHERE id=?1", [order_id], |r| r.get(0))?;
    if total > 0 && done == total {
        if ["sent", "preparing", "open"].contains(&status.as_str()) {
            crate::orders::set_status(conn, order_id, "ready")?;
        }
    } else if done > 0 && status == "sent" {
        conn.execute("UPDATE orders SET status='preparing' WHERE id=?1", [order_id])?;
    } else if status == "ready" {
        // A recall reopened an item.
        conn.execute("UPDATE orders SET status='preparing', ready_at=NULL WHERE id=?1", [order_id])?;
    }
    Ok(())
}

pub fn toggle(conn: &Connection, part_id: i64) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let order_id: i64 = tx
        .query_row("SELECT order_id FROM kitchen_parts WHERE id=?1 AND voided=0", [part_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| ApiError::not_found("item"))?;
    tx.execute(
        "UPDATE kitchen_parts SET done_at = CASE WHEN done_at IS NULL THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') END WHERE id=?1",
        [part_id],
    )?;
    progress(&tx, order_id)?;
    tx.commit()?;
    Ok(order_id)
}

/// Bump: this station finished the whole ticket.
pub fn bump(conn: &Connection, order_id: i64, station_id: i64) -> ApiResult<()> {
    let tx = atomic(conn)?;
    let n = tx.execute(
        "UPDATE kitchen_parts SET done_at=COALESCE(done_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
           bumped_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE order_id=?1 AND station_id=?2 AND bumped_at IS NULL",
        params![order_id, station_id],
    )?;
    if n == 0 {
        return Err(ApiError::not_found("ticket"));
    }
    progress(&tx, order_id)?;
    tx.commit()?;
    Ok(())
}

/// Bring back the last ticket this station bumped.
pub fn recall(conn: &Connection, station_id: i64) -> ApiResult<Option<i64>> {
    let tx = atomic(conn)?;
    let last: Option<i64> = tx
        .query_row(
            "SELECT k.order_id FROM kitchen_parts k JOIN orders o ON o.id=k.order_id
             WHERE k.station_id=?1 AND k.bumped_at IS NOT NULL AND o.status NOT IN ('closed','voided')
             ORDER BY k.bumped_at DESC LIMIT 1",
            [station_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(order_id) = last {
        tx.execute("UPDATE kitchen_parts SET bumped_at=NULL, done_at=NULL WHERE order_id=?1 AND station_id=?2", params![order_id, station_id])?;
        progress(&tx, order_id)?;
    }
    tx.commit()?;
    Ok(last)
}

/// Open tickets for one station (or all stations), oldest first.
pub fn board(conn: &Connection, station_id: Option<i64>) -> ApiResult<Value> {
    let parts: Vec<Value> = conn
        .prepare(
            "SELECT k.id, k.order_id, k.station_id, k.qty, k.name, k.details, k.note, k.sent_at, k.done_at, k.voided
             FROM kitchen_parts k JOIN orders o ON o.id=k.order_id
             WHERE k.bumped_at IS NULL AND (?1 IS NULL OR k.station_id=?1) AND o.status NOT IN ('closed','voided','served')
             ORDER BY k.order_id, k.line_id, k.part_index",
        )?
        .query_map([station_id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "order_id": r.get::<_, i64>(1)?, "station_id": r.get::<_, i64>(2)?,
                "qty": r.get::<_, i64>(3)?, "names": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                "details": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
                "note": r.get::<_, String>(6)?, "sent_at": r.get::<_, String>(7)?, "done": r.get::<_, Option<String>>(8)?.is_some(),
                "voided": r.get::<_, bool>(9)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    let mut tickets: Vec<Value> = Vec::new();
    for p in parts {
        let oid = p["order_id"].as_i64().unwrap();
        if let Some(t) = tickets.iter_mut().find(|t| t["order_id"] == oid) {
            t["items"].as_array_mut().unwrap().push(p);
            continue;
        }
        let head = conn.query_row(
            "SELECT o.number, o.split_index, o.type, tb.name, o.customer_name, o.note, o.status, o.payment_status
             FROM orders o LEFT JOIN dining_tables tb ON tb.id=o.table_id WHERE o.id=?1",
            [oid],
            |r| {
                Ok(json!({
                    "order_id": oid, "number": r.get::<_, i64>(0)?, "split_index": r.get::<_, i64>(1)?, "type": r.get::<_, String>(2)?,
                    "table_name": r.get::<_, Option<String>>(3)?, "customer_name": r.get::<_, String>(4)?, "note": r.get::<_, String>(5)?,
                    "status": r.get::<_, String>(6)?, "payment_status": r.get::<_, String>(7)?,
                }))
            },
        )?;
        let mut t = head;
        t["sent_at"] = p["sent_at"].clone();
        t["items"] = json!([p]);
        tickets.push(t);
    }
    // Hide tickets whose items were all voided (nothing left to cook).
    tickets.retain(|t| t["items"].as_array().unwrap().iter().any(|i| i["voided"] == false));
    Ok(json!(tickets))
}

/// Public customer screen: numbers being prepared and ready to collect.
pub fn display_board(conn: &Connection) -> ApiResult<Value> {
    let q = |status_sql: &str| -> ApiResult<Vec<Value>> {
        Ok(conn
            .prepare(&format!(
                "SELECT number, split_index FROM orders WHERE {status_sql}
                   AND date(created_at,'localtime') = date('now','localtime') ORDER BY COALESCE(ready_at, sent_at, created_at) LIMIT 30"
            ))?
            .query_map([], |r| {
                let (n, s): (i64, i64) = (r.get(0)?, r.get(1)?);
                Ok(json!(if s > 0 { format!("{n}-{}", s + 1) } else { n.to_string() }))
            })?
            .collect::<Result<_, _>>()?)
    };
    Ok(json!({
        "preparing": q("status IN ('sent','preparing')")?,
        "ready": q("status = 'ready' AND ready_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 minutes')")?,
    }))
}

pub fn screen_stations(conn: &Connection) -> ApiResult<Value> {
    let rows: Vec<Value> = conn
        .prepare("SELECT id, code, names FROM stations WHERE active=1 AND show_on_screen=1 ORDER BY sort, id")?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "code": r.get::<_, String>(1)?,
                      "names": serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap_or(Value::Null)}))
        })?
        .collect::<Result<_, _>>()?;
    Ok(json!(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::{demo, new_order, simple_line};

    fn station(c: &Connection, code: &str) -> i64 {
        c.query_row("SELECT id FROM stations WHERE code=?1", [code], |r| r.get(0)).unwrap()
    }
    fn status(c: &Connection, id: i64) -> String {
        c.query_row("SELECT status FROM orders WHERE id=?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn items_go_to_their_station_screens() {
        let (c, _, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1), simple_line(&c, "Coca-Cola 33cl", 2)], true);
        let drinks = board(&c, Some(station(&c, "drinks"))).unwrap();
        assert_eq!(drinks.as_array().unwrap().len(), 1);
        let items = drinks[0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["names"]["ar"], "كوكا كولا 33 سل");
        assert_eq!(items[0]["qty"], 2);
        assert_eq!(board(&c, None).unwrap()[0]["items"].as_array().unwrap().len(), 2);
        assert_eq!(board(&c, Some(station(&c, "grill"))).unwrap().as_array().unwrap().len(), 0);
        let _ = id;
    }

    #[test]
    fn order_becomes_ready_when_all_stations_are_done() {
        let (c, _, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1), simple_line(&c, "Coca-Cola 33cl", 1)], true);
        assert_eq!(status(&c, id), "sent");
        let drink_part = board(&c, Some(station(&c, "drinks"))).unwrap()[0]["items"][0]["id"].as_i64().unwrap();
        toggle(&c, drink_part).unwrap();
        assert_eq!(status(&c, id), "preparing");
        toggle(&c, drink_part).unwrap(); // undo
        toggle(&c, drink_part).unwrap();
        bump(&c, id, station(&c, "drinks")).unwrap();
        assert_eq!(board(&c, Some(station(&c, "drinks"))).unwrap().as_array().unwrap().len(), 0);
        assert_eq!(status(&c, id), "preparing", "dessert not done yet");
        bump(&c, id, station(&c, "assembly")).unwrap();
        assert_eq!(status(&c, id), "ready");
        assert_eq!(display_board(&c).unwrap()["ready"], json!([c.query_row("SELECT CAST(number AS TEXT) FROM orders WHERE id=?1", [id], |r| r.get::<_, String>(0)).unwrap()]));
        // Recall the last bump: back on screen, order no longer ready.
        assert_eq!(recall(&c, station(&c, "assembly")).unwrap(), Some(id));
        assert_eq!(status(&c, id), "preparing");
        assert_eq!(board(&c, Some(station(&c, "assembly"))).unwrap().as_array().unwrap().len(), 1);
        assert!(bump(&c, id, station(&c, "grill")).is_err());
    }

    #[test]
    fn combo_parts_and_voids() {
        let (c, owner, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 2)], true);
        let line: i64 = c.query_row("SELECT id FROM order_lines WHERE order_id=?1", [id], |r| r.get(0)).unwrap();
        crate::orders::remove_line(&c, &cashier, line, Some("erreur".into()), Some("2222")).unwrap();
        assert!(board(&c, None).unwrap().as_array().unwrap().is_empty(), "voided items leave the screen");
        let b = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1)], true);
        crate::orders::void(&c, &owner, b, "x", None).ok(); // unpaid -> voided
        assert!(board(&c, None).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn served_from_pos_clears_screen_and_station_without_screen_is_skipped() {
        let (c, owner, cashier) = demo();
        crate::printing::set_station_routing(&c, &owner, station(&c, "drinks"), Some(
            crate::printing::save_printer(&c, &owner, serde_json::from_value(json!({"name":"P","kind":"network","address":"127.0.0.1:1","paper_mm":80,"codepage":"cp850"})).unwrap()).unwrap()
        ), false).unwrap();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Coca-Cola 33cl", 1), simple_line(&c, "Cookie", 1)], true);
        let all = board(&c, None).unwrap();
        assert_eq!(all[0]["items"].as_array().unwrap().len(), 1, "drinks prints only");
        crate::orders::set_status(&c, id, "served").unwrap();
        assert!(board(&c, None).unwrap().as_array().unwrap().is_empty());
    }
}
