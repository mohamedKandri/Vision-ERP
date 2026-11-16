//! Floor plan: floors (rooms), table layout on a grid, live table status, moving an order to another table.

use crate::auth::SessionUser;
use crate::db::atomic;

/// Run `f` inside a savepoint; commit on Ok, roll back on Err.
fn in_tx<T>(conn: &Connection, f: impl FnOnce(&Connection) -> ApiResult<T>) -> ApiResult<T> {
    let tx = atomic(conn)?;
    let r = f(&tx)?;
    tx.commit()?;
    Ok(r)
}
use crate::error::{ApiError, ApiResult};
use crate::audit;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

pub const GRID_W: i64 = 24;
pub const GRID_H: i64 = 16;
/// A table is busy until its orders are paid (encaissé), even if the kitchen is still serving them.
const OPEN: &str = "o.status NOT IN ('closed','voided') AND o.payment_status NOT IN ('paid','refunded')";

/// Floors, tables with position, and the open orders on each table (for the floor view).
pub fn plan(conn: &Connection) -> ApiResult<Value> {
    let floors = conn
        .prepare("SELECT id, name, sort, active FROM floors ORDER BY sort, id")?
        .query_map([], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "sort": r.get::<_, i64>(2)?, "active": r.get::<_, bool>(3)?})))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut tables = conn
        .prepare("SELECT id, name, seats, sort, active, floor_id, x, y, w, h, shape FROM dining_tables ORDER BY sort, id")?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "seats": r.get::<_, i64>(2)?, "sort": r.get::<_, i64>(3)?,
                      "active": r.get::<_, bool>(4)?, "floor_id": r.get::<_, Option<i64>>(5)?, "x": r.get::<_, i64>(6)?, "y": r.get::<_, i64>(7)?,
                      "w": r.get::<_, i64>(8)?, "h": r.get::<_, i64>(9)?, "shape": r.get::<_, String>(10)?, "orders": []}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT o.table_id, o.id, o.number, o.split_index, o.status, o.payment_status, o.total_cents, o.paid_cents, o.created_at
         FROM orders o WHERE o.table_id IS NOT NULL AND o.type='dine_in' AND {OPEN} ORDER BY o.id"
    ))?;
    let open = stmt.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, json!({"id": r.get::<_, i64>(1)?, "number": r.get::<_, i64>(2)?, "split_index": r.get::<_, i64>(3)?,
            "status": r.get::<_, String>(4)?, "payment_status": r.get::<_, String>(5)?, "total_cents": r.get::<_, i64>(6)?,
            "paid_cents": r.get::<_, i64>(7)?, "created_at": r.get::<_, String>(8)?})))
    })?;
    for row in open {
        let (tid, o) = row?;
        if let Some(t) = tables.iter_mut().find(|t| t["id"] == tid) {
            t["orders"].as_array_mut().unwrap().push(o);
        }
    }
    Ok(json!({"grid": {"w": GRID_W, "h": GRID_H}, "floors": floors, "tables": tables}))
}

#[derive(Deserialize)]
pub struct FloorIn {
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub sort: i64,
    #[serde(default = "yes")]
    pub active: bool,
}
fn yes() -> bool {
    true
}

pub fn save_floor(conn: &Connection, me: &SessionUser, f: FloorIn) -> ApiResult<i64> {
    let name = f.name.trim();
    if name.is_empty() || name.chars().count() > 30 {
        return Err(ApiError::invalid("floor name required (max 30)"));
    }
    let id = match f.id {
        Some(id) => {
            if conn.execute("UPDATE floors SET name=?2, sort=?3, active=?4 WHERE id=?1", params![id, name, f.sort, f.active])? == 0 {
                return Err(ApiError::not_found("floor"));
            }
            id
        }
        None => {
            conn.execute("INSERT INTO floors (name, sort, active) VALUES (?1,?2,?3)", params![name, f.sort, f.active])?;
            conn.last_insert_rowid()
        }
    };
    audit::log(conn, Some(me.id), "floor.saved", Some("floor"), Some(&id.to_string()), Some(json!({"name": name, "active": f.active})))?;
    Ok(id)
}

/// A floor can be deleted only when no table is on it.
pub fn delete_floor(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<()> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM dining_tables WHERE floor_id=?1", [id], |r| r.get(0))?;
    if n > 0 {
        return Err(ApiError::new("conflict", "move or delete the tables on this floor first"));
    }
    if conn.execute("DELETE FROM floors WHERE id=?1", [id])? == 0 {
        return Err(ApiError::not_found("floor"));
    }
    audit::log(conn, Some(me.id), "floor.deleted", Some("floor"), Some(&id.to_string()), None)?;
    Ok(())
}

#[derive(Deserialize)]
pub struct TableIn {
    pub id: Option<i64>,
    pub name: String,
    pub seats: i64,
    #[serde(default)]
    pub sort: i64,
    #[serde(default = "yes")]
    pub active: bool,
    pub floor_id: Option<i64>,
    #[serde(default)]
    pub x: i64,
    #[serde(default)]
    pub y: i64,
    #[serde(default = "three")]
    pub w: i64,
    #[serde(default = "three")]
    pub h: i64,
    #[serde(default = "square")]
    pub shape: String,
}
fn three() -> i64 {
    3
}
fn square() -> String {
    "square".into()
}

fn check_box(x: i64, y: i64, w: i64, h: i64) -> ApiResult<()> {
    if !(1..=12).contains(&w) || !(1..=12).contains(&h) || x < 0 || y < 0 || x + w > GRID_W || y + h > GRID_H {
        return Err(ApiError::invalid("the table must fit on the floor"));
    }
    Ok(())
}

fn check_floor(conn: &Connection, floor: Option<i64>) -> ApiResult<()> {
    if let Some(f) = floor {
        if conn.query_row("SELECT 1 FROM floors WHERE id=?1", [f], |_| Ok(())).optional()?.is_none() {
            return Err(ApiError::invalid("unknown floor"));
        }
    }
    Ok(())
}

pub fn save_table(conn: &Connection, me: &SessionUser, t: TableIn) -> ApiResult<i64> {
    let name = t.name.trim();
    if name.is_empty() || name.chars().count() > 30 || !(1..=50).contains(&t.seats) {
        return Err(ApiError::invalid("table name and 1-50 seats required"));
    }
    if !["square", "round"].contains(&t.shape.as_str()) {
        return Err(ApiError::invalid("unknown table shape"));
    }
    check_box(t.x, t.y, t.w, t.h)?;
    check_floor(conn, t.floor_id)?;
    let dup: Option<i64> = conn
        .query_row("SELECT id FROM dining_tables WHERE lower(name)=lower(?1) AND id IS NOT ?2", params![name, t.id], |r| r.get(0))
        .optional()?;
    if dup.is_some() {
        return Err(ApiError::new("conflict", "a table with this name already exists"));
    }
    let id = in_tx(conn, |tx| {
        let id = match t.id {
            Some(id) => {
                if !t.active {
                    let open: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM orders o WHERE o.table_id=?1 AND {OPEN}"), [id], |r| r.get(0))?;
                    if open > 0 {
                        return Err(ApiError::new("conflict", "this table has an open order"));
                    }
                }
                if tx.execute(
                    "UPDATE dining_tables SET name=?2, seats=?3, sort=?4, active=?5, floor_id=?6, x=?7, y=?8, w=?9, h=?10, shape=?11 WHERE id=?1",
                    params![id, name, t.seats, t.sort, t.active, t.floor_id, t.x, t.y, t.w, t.h, t.shape],
                )? == 0
                {
                    return Err(ApiError::not_found("table"));
                }
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO dining_tables (name, seats, sort, active, floor_id, x, y, w, h, shape) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![name, t.seats, t.sort, t.active, t.floor_id, t.x, t.y, t.w, t.h, t.shape],
                )?;
                tx.last_insert_rowid()
            }
        };
        audit::log(tx, Some(me.id), "table.saved", Some("table"), Some(&id.to_string()), Some(json!({"name": name, "active": t.active, "floor_id": t.floor_id})))?;
        Ok(id)
    })?;
    Ok(id)
}

/// Remove a table from the plan. Never-used tables are deleted; tables with past orders are
/// kept for history but taken off the floor (inactive, no floor). Not while an order is open on it.
pub fn delete_table(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<()> {
    let open: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM orders o WHERE o.table_id=?1 AND {OPEN}"), [id], |r| r.get(0))?;
    if open > 0 {
        return Err(ApiError::new("conflict", "this table has an open order"));
    }
    let used: i64 = conn.query_row("SELECT COUNT(*) FROM orders WHERE table_id=?1", [id], |r| r.get(0))?;
    let n = if used == 0 {
        conn.execute("DELETE FROM dining_tables WHERE id=?1", [id])?
    } else {
        conn.execute("UPDATE dining_tables SET active=0, floor_id=NULL, name=name || ' #' || id WHERE id=?1", [id])?
    };
    if n == 0 {
        return Err(ApiError::not_found("table"));
    }
    audit::log(conn, Some(me.id), "table.deleted", Some("table"), Some(&id.to_string()), Some(json!({"kept_for_history": used > 0})))?;
    Ok(())
}

#[derive(Deserialize)]
pub struct Pos {
    pub id: i64,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// Save the positions after dragging tables around (one transaction, all or nothing).
pub fn save_layout(conn: &Connection, me: &SessionUser, floor_id: i64, items: Vec<Pos>) -> ApiResult<()> {
    check_floor(conn, Some(floor_id))?;
    in_tx(conn, |tx| {
        for p in &items {
            check_box(p.x, p.y, p.w, p.h)?;
            if tx.execute("UPDATE dining_tables SET x=?2, y=?3, w=?4, h=?5 WHERE id=?1 AND floor_id=?6", params![p.id, p.x, p.y, p.w, p.h, floor_id])? == 0 {
                return Err(ApiError::invalid("table is not on this floor"));
            }
        }
        audit::log(tx, Some(me.id), "floor.layout", Some("floor"), Some(&floor_id.to_string()), Some(json!({"tables": items.len()})))?;
        Ok(())
    })
}

/// Move an open dine-in order (and its split parts) to another table.
pub fn move_order(conn: &Connection, me: &SessionUser, order_id: i64, table_id: i64) -> ApiResult<()> {
    in_tx(conn, |tx| {
        let row: Option<(String, Option<i64>, String, String)> = tx
            .query_row("SELECT type, table_id, status, payment_status FROM orders WHERE id=?1", [order_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?;
        let (kind, from, status, pay) = row.ok_or_else(|| ApiError::not_found("order"))?;
        if kind != "dine_in" || ["closed", "voided"].contains(&status.as_str()) || pay == "refunded" {
            return Err(ApiError::invalid("only an open dine-in order can change table"));
        }
        if tx.query_row("SELECT 1 FROM dining_tables WHERE id=?1 AND active=1", [table_id], |_| Ok(())).optional()?.is_none() {
            return Err(ApiError::invalid("unknown table"));
        }
        tx.execute(
            &format!("UPDATE orders SET table_id=?2 WHERE (id=?1 OR parent_id=?1) AND type='dine_in' AND status NOT IN ('closed','voided')"),
            params![order_id, table_id],
        )?;
        audit::log(tx, Some(me.id), "order.table_moved", Some("order"), Some(&order_id.to_string()), Some(json!({"from": from, "to": table_id})))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::{demo, simple_line};
    use crate::orders::{submit, SubmitIn};

    fn dine_in(c: &Connection, me: &SessionUser, table: i64) -> i64 {
        submit(c, me, SubmitIn { order_id: None, kind: Some("dine_in".into()), table_id: Some(table), customer_name: None, customer_phone: None,
            customer_address: None, delivery_fee_cents: None, note: None, lines: vec![simple_line(c, "Cookie", 2)], send: false, hold: false,
            hold_label: None, register: "main".into() }).unwrap()
    }

    #[test]
    fn migration_lays_out_existing_tables_without_overlap() {
        let (c, _, _) = demo();
        let p = plan(&c).unwrap();
        assert_eq!(p["floors"].as_array().unwrap().len(), 2);
        let tables = p["tables"].as_array().unwrap();
        assert_eq!(tables.len(), 10);
        for a in tables {
            assert!(a["floor_id"].is_i64());
            let terrace = a["name"].as_str().unwrap().starts_with("Terrasse");
            assert_eq!(a["floor_id"] == 2, terrace);
            for b in tables {
                if a["id"] != b["id"] && a["floor_id"] == b["floor_id"] {
                    let (ax, ay, bx, by) = (a["x"].as_i64().unwrap(), a["y"].as_i64().unwrap(), b["x"].as_i64().unwrap(), b["y"].as_i64().unwrap());
                    assert!(ax + 3 <= bx || bx + 3 <= ax || ay + 3 <= by || by + 3 <= ay, "{a} overlaps {b}");
                }
            }
        }
    }

    #[test]
    fn plan_shows_open_orders_per_table_and_move_works() {
        let (c, _, cashier) = demo();
        let o = dine_in(&c, &cashier, 3);
        let p = plan(&c).unwrap();
        let t3 = p["tables"].as_array().unwrap().iter().find(|t| t["id"] == 3).unwrap();
        assert_eq!(t3["orders"][0]["id"], json!(o));
        assert_eq!(t3["orders"][0]["total_cents"], json!(2400)); // 2 cookies x 12 DH
        move_order(&c, &cashier, o, 5).unwrap();
        let p = plan(&c).unwrap();
        let find = |id: i64| p["tables"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap().clone();
        assert!(find(3)["orders"].as_array().unwrap().is_empty());
        assert_eq!(find(5)["orders"][0]["id"], json!(o));
        // Paid (not yet served) frees the table; closed orders do not show either.
        c.execute("UPDATE orders SET payment_status='paid' WHERE id=?1", [o]).unwrap();
        assert!(plan(&c).unwrap()["tables"].as_array().unwrap().iter().all(|t| t["orders"].as_array().unwrap().is_empty()), "paid frees the table");
        c.execute("UPDATE orders SET status='closed' WHERE id=?1", [o]).unwrap();
        assert!(plan(&c).unwrap()["tables"].as_array().unwrap().iter().all(|t| t["orders"].as_array().unwrap().is_empty()));
        assert!(move_order(&c, &cashier, o, 4).is_err(), "closed order cannot move");
    }

    #[test]
    fn table_rules() {
        let (c, owner, cashier) = demo();
        let mk = |id: Option<i64>, name: &str, x: i64, w: i64| TableIn { id, name: name.into(), seats: 4, sort: 0, active: true, floor_id: Some(1), x, y: 0, w, h: 3, shape: "round".into() };
        let id = save_table(&c, &owner, mk(None, "Bar 1", 20, 3)).unwrap();
        assert!(save_table(&c, &owner, mk(None, "Bar 2", 22, 3)).is_err(), "off the board");
        assert!(save_table(&c, &owner, mk(None, "bar 1", 0, 3)).is_err(), "duplicate name");
        let mut bad = mk(None, "X", 0, 3);
        bad.floor_id = Some(99);
        assert!(save_table(&c, &owner, bad).is_err());
        // Can't deactivate a table with an open order.
        let o = dine_in(&c, &cashier, id);
        let mut off = mk(Some(id), "Bar 1", 20, 3);
        off.active = false;
        assert!(save_table(&c, &owner, off).is_err());
        c.execute("UPDATE orders SET status='voided' WHERE id=?1", [o]).unwrap();
        let mut off = mk(Some(id), "Bar 1", 20, 3);
        off.active = false;
        save_table(&c, &owner, off).unwrap();
        // Layout: all-or-nothing.
        assert!(save_layout(&c, &owner, 1, vec![Pos { id: 1, x: 5, y: 5, w: 3, h: 3 }, Pos { id: 2, x: 23, y: 0, w: 3, h: 3 }]).is_err());
        assert_ne!(c.query_row("SELECT x FROM dining_tables WHERE id=1", [], |r| r.get::<_, i64>(0)).unwrap(), 5, "rolled back");
        save_layout(&c, &owner, 1, vec![Pos { id: 1, x: 5, y: 5, w: 4, h: 2 }]).unwrap();
        assert!(save_layout(&c, &owner, 2, vec![Pos { id: 1, x: 0, y: 0, w: 3, h: 3 }]).is_err(), "table 1 is not on the terrace");
        // Delete: used table kept for history (off the floor), unused one removed.
        delete_table(&c, &owner, id).unwrap();
        assert_eq!(c.query_row("SELECT floor_id IS NULL AND active=0 FROM dining_tables WHERE id=?1", [id], |r| r.get::<_, bool>(0)).unwrap(), true);
        let fresh = save_table(&c, &owner, mk(None, "Bar 9", 0, 3)).unwrap();
        delete_table(&c, &owner, fresh).unwrap();
        assert!(c.query_row("SELECT 1 FROM dining_tables WHERE id=?1", [fresh], |_| Ok(())).is_err());
        let busy = dine_in(&c, &cashier, 4);
        assert!(delete_table(&c, &owner, 4).is_err(), "open order on it");
        let _ = busy;
        // Floors.
        assert!(delete_floor(&c, &owner, 1).is_err(), "has tables");
        let f = save_floor(&c, &owner, FloorIn { id: None, name: "Étage".into(), sort: 3, active: true }).unwrap();
        delete_floor(&c, &owner, f).unwrap();
    }
}
