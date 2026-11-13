//! Cash sessions per register: open with a float, cash in/out, X report (any time),
//! close with counted cash -> Z report (numbered, stored, never changes afterwards).

use crate::auth::SessionUser;
use crate::db::atomic;
use crate::error::{ApiError, ApiResult};
use crate::orders::{next_seq, open_session_id};
use crate::audit;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn open(conn: &Connection, me: &SessionUser, register: &str, float_cents: i64) -> ApiResult<i64> {
    if !(0..=10_000_000).contains(&float_cents) {
        return Err(ApiError::invalid("float out of range"));
    }
    if register.trim().is_empty() || register.len() > 40 {
        return Err(ApiError::invalid("bad register name"));
    }
    if open_session_id(conn, register)?.is_some() {
        return Err(ApiError::new("conflict", "the till is already open"));
    }
    conn.execute(
        "INSERT INTO cash_sessions (register, opened_by, float_cents) VALUES (?1, ?2, ?3)",
        params![register, me.id, float_cents],
    )?;
    let id = conn.last_insert_rowid();
    audit::log(conn, Some(me.id), "cash.opened", Some("cash_session"), Some(&id.to_string()), Some(json!({"register": register, "float_cents": float_cents})))?;
    Ok(id)
}

pub fn movement(conn: &Connection, me: &SessionUser, register: &str, kind: &str, amount_cents: i64, reason: &str) -> ApiResult<()> {
    let sid = open_session_id(conn, register)?.ok_or_else(|| ApiError::new("no_cash_session", "open the till first"))?;
    if !["in", "out"].contains(&kind) {
        return Err(ApiError::invalid("kind must be in or out"));
    }
    if !(1..=10_000_000).contains(&amount_cents) {
        return Err(ApiError::invalid("amount out of range"));
    }
    if reason.trim().is_empty() || reason.chars().count() > 200 {
        return Err(ApiError::new("reason_required", "a reason is required"));
    }
    conn.execute(
        "INSERT INTO cash_movements (session_id, kind, amount_cents, reason, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sid, kind, amount_cents, reason.trim(), me.id],
    )?;
    audit::log(conn, Some(me.id), if kind == "in" { "cash.in" } else { "cash.out" }, Some("cash_session"), Some(&sid.to_string()),
        Some(json!({"amount_cents": amount_cents, "reason": reason.trim()})))?;
    Ok(())
}

/// Everything an X or Z report shows, computed from payments linked to the session.
pub fn summary(conn: &Connection, session_id: i64) -> ApiResult<Value> {
    let (register, opened_by, opened_at, float, closed_at, counted, z_number, note): (String, String, String, i64, Option<String>, Option<i64>, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT s.register, u.name, s.opened_at, s.float_cents, s.closed_at, s.counted_cents, s.z_number, s.note
             FROM cash_sessions s JOIN users u ON u.id = s.opened_by WHERE s.id = ?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("cash session"))?;

    let pay = |method: &str, kind: &str| -> ApiResult<i64> {
        Ok(conn.query_row(
            "SELECT COALESCE(SUM(amount_cents),0) FROM payments WHERE cash_session_id=?1 AND method=?2 AND kind=?3",
            params![session_id, method, kind],
            |r| r.get(0),
        )?)
    };
    let cash_sales = pay("cash", "payment")?;
    let card_sales = pay("card", "payment")?;
    let cash_refunds = pay("cash", "refund")?; // negative
    let card_refunds = pay("card", "refund")?;
    let (cash_in, cash_out): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(CASE WHEN kind='in' THEN amount_cents END),0), COALESCE(SUM(CASE WHEN kind='out' THEN amount_cents END),0)
         FROM cash_movements WHERE session_id=?1",
        [session_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let movements = conn
        .prepare("SELECT m.kind, m.amount_cents, m.reason, u.name, m.at FROM cash_movements m JOIN users u ON u.id=m.user_id WHERE session_id=?1 ORDER BY m.id")?
        .query_map([session_id], |r| {
            Ok(json!({"kind": r.get::<_, String>(0)?, "amount_cents": r.get::<_, i64>(1)?, "reason": r.get::<_, String>(2)?,
                      "user": r.get::<_, String>(3)?, "at": r.get::<_, String>(4)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_cash = float + cash_sales + cash_refunds + cash_in - cash_out;

    // Orders whose payments landed in this session (each counted once).
    let order_ids = "SELECT DISTINCT order_id FROM payments WHERE cash_session_id = ?1 AND kind='payment'";
    let (orders, gross, discounts): (i64, i64, i64) = conn.query_row(
        &format!("SELECT COUNT(*), COALESCE(SUM(total_cents),0), COALESCE(SUM(discount_cents),0) FROM orders WHERE id IN ({order_ids}) AND payment_status <> 'refunded'"),
        [session_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let by_type = conn
        .prepare(&format!("SELECT type, COUNT(*), SUM(total_cents) FROM orders WHERE id IN ({order_ids}) AND payment_status <> 'refunded' GROUP BY type ORDER BY type"))?
        .query_map([session_id], |r| Ok(json!({"type": r.get::<_, String>(0)?, "count": r.get::<_, i64>(1)?, "total_cents": r.get::<_, i64>(2)?})))?
        .collect::<Result<Vec<_>, _>>()?;
    // VAT: sum the stored buckets of those orders.
    let mut vat: Vec<Value> = Vec::new();
    let vats: Vec<String> = conn
        .prepare(&format!("SELECT vat FROM orders WHERE id IN ({order_ids}) AND payment_status <> 'refunded'"))?
        .query_map([session_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for v in vats {
        for b in serde_json::from_str::<Vec<Value>>(&v).unwrap_or_default() {
            match vat.iter_mut().find(|x| x["tax_id"] == b["tax_id"]) {
                Some(x) => {
                    for k in ["base", "tax", "total"] {
                        x[k] = json!(x[k].as_i64().unwrap_or(0) + b[k].as_i64().unwrap_or(0));
                    }
                }
                None => vat.push(b),
            }
        }
    }
    vat.sort_by_key(|b| (b["rate_bp"].as_i64().unwrap_or(0), b["tax_id"].as_i64().unwrap_or(0)));

    // Voids and comps during the session window on this register.
    let until = closed_at.clone().unwrap_or_else(|| "9999".into());
    let (voids, void_total): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(total_cents),0) FROM orders WHERE register=?1 AND voided_at >= ?2 AND voided_at <= ?3",
        params![register, opened_at, until],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (refund_count,): (i64,) = conn.query_row(
        "SELECT COUNT(DISTINCT order_id) FROM payments WHERE cash_session_id=?1 AND kind='refund'",
        [session_id],
        |r| Ok((r.get(0)?,)),
    )?;
    let comps: i64 = conn.query_row(
        &format!("SELECT COALESCE(SUM(gross_cents),0) FROM order_lines WHERE discount_kind='comp' AND voided=0 AND order_id IN ({order_ids})"),
        [session_id],
        |r| r.get(0),
    )?;
    let open_orders: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orders WHERE register=?1 AND status NOT IN ('closed','voided') AND payment_status IN ('unpaid','partial')",
        [&register],
        |r| r.get(0),
    )?;

    Ok(json!({
        "session_id": session_id, "register": register, "opened_by": opened_by, "opened_at": opened_at,
        "closed_at": closed_at, "z_number": z_number, "note": note,
        "float_cents": float, "cash_sales_cents": cash_sales, "card_sales_cents": card_sales,
        "cash_refunds_cents": -cash_refunds, "card_refunds_cents": -card_refunds,
        "cash_in_cents": cash_in, "cash_out_cents": cash_out, "movements": movements,
        "expected_cash_cents": expected_cash, "counted_cash_cents": counted,
        "difference_cents": counted.map(|c| c - expected_cash),
        "orders": orders, "sales_total_cents": gross, "average_ticket_cents": if orders > 0 { gross / orders } else { 0 },
        "discounts_cents": discounts, "comps_cents": comps, "voids": voids, "voids_total_cents": void_total,
        "refunds": refund_count, "by_type": by_type, "vat": vat, "open_orders": open_orders,
        "net_sales_cents": cash_sales + card_sales + cash_refunds + card_refunds,
    }))
}

pub fn current(conn: &Connection, register: &str) -> ApiResult<Value> {
    match open_session_id(conn, register)? {
        Some(id) => summary(conn, id),
        None => Ok(Value::Null),
    }
}

/// Close the till: store counted cash, expected cash and a frozen Z report.
pub fn close(conn: &Connection, me: &SessionUser, register: &str, counted_cents: i64, note: &str) -> ApiResult<Value> {
    if !(0..=100_000_000).contains(&counted_cents) {
        return Err(ApiError::invalid("counted amount out of range"));
    }
    if note.chars().count() > 300 {
        return Err(ApiError::invalid("note too long"));
    }
    let tx = atomic(conn)?;
    let sid = open_session_id(&tx, register)?.ok_or_else(|| ApiError::new("no_cash_session", "the till is not open"))?;
    let before = summary(&tx, sid)?;
    let expected = before["expected_cash_cents"].as_i64().unwrap_or(0);
    let z = next_seq(&tx, "z_number")?;
    tx.execute(
        "UPDATE cash_sessions SET closed_by=?2, closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'), counted_cents=?3,
           expected_cents=?4, z_number=?5, note=?6 WHERE id=?1",
        params![sid, me.id, counted_cents, expected, z, note.trim()],
    )?;
    let report = summary(&tx, sid)?;
    tx.execute("UPDATE cash_sessions SET z_report=?2 WHERE id=?1", params![sid, report.to_string()])?;
    audit::log(&tx, Some(me.id), "cash.closed", Some("cash_session"), Some(&sid.to_string()),
        Some(json!({"z_number": z, "expected_cents": expected, "counted_cents": counted_cents, "difference_cents": counted_cents - expected})))?;
    tx.commit()?;
    Ok(report)
}

pub fn sessions(conn: &Connection, limit: i64) -> ApiResult<Value> {
    let rows = conn
        .prepare(
            "SELECT s.id, s.register, s.z_number, s.opened_at, s.closed_at, s.float_cents, s.expected_cents, s.counted_cents, u.name
             FROM cash_sessions s JOIN users u ON u.id = s.opened_by ORDER BY s.id DESC LIMIT ?1",
        )?
        .query_map([limit.clamp(1, 500)], |r| {
            let expected: Option<i64> = r.get(6)?;
            let counted: Option<i64> = r.get(7)?;
            Ok(json!({"id": r.get::<_, i64>(0)?, "register": r.get::<_, String>(1)?, "z_number": r.get::<_, Option<i64>>(2)?,
                      "opened_at": r.get::<_, String>(3)?, "closed_at": r.get::<_, Option<String>>(4)?,
                      "float_cents": r.get::<_, i64>(5)?, "expected_cents": expected, "counted_cents": counted,
                      "difference_cents": match (expected, counted) { (Some(e), Some(c)) => Some(c - e), _ => None },
                      "opened_by": r.get::<_, String>(8)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

/// Stored Z report for a closed session (frozen at close), or a live summary for an open one.
pub fn report(conn: &Connection, session_id: i64) -> ApiResult<Value> {
    let z: Option<String> = conn
        .query_row("SELECT z_report FROM cash_sessions WHERE id=?1", [session_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| ApiError::not_found("cash session"))?;
    match z {
        Some(s) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
        None => summary(conn, session_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::{demo, new_order, simple_line};
    use crate::orders::{pay, refund, void, PayIn};

    #[test]
    fn full_shift_x_and_z() {
        let (c, owner, cashier) = demo(); // demo() opens 'main' with a 500.00 float
        assert_eq!(open(&c, &owner, "main", 0).unwrap_err().code, "conflict");
        let a = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 2)], true); // 50.00
        pay(&c, &cashier, a, vec![PayIn { method: "cash".into(), amount_cents: 5000, tendered_cents: Some(10000) }], "main").unwrap();
        let b = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], true); // 12.00
        pay(&c, &cashier, b, vec![PayIn { method: "card".into(), amount_cents: 1200, tendered_cents: None }], "main").unwrap();
        let v = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], false);
        void(&c, &owner, v, "erreur", None).unwrap();
        movement(&c, &cashier, "main", "out", 2000, "achat pain").unwrap();
        movement(&c, &cashier, "main", "in", 500, "monnaie").unwrap();
        assert_eq!(movement(&c, &cashier, "main", "out", 100, " ").unwrap_err().code, "reason_required");

        let x = current(&c, "main").unwrap();
        assert_eq!(x["cash_sales_cents"], json!(5000));
        assert_eq!(x["card_sales_cents"], json!(1200));
        assert_eq!(x["expected_cash_cents"], json!(50000 + 5000 - 2000 + 500));
        assert_eq!(x["orders"], json!(2));
        assert_eq!(x["average_ticket_cents"], json!(3100));
        assert_eq!(x["voids"], json!(1));
        let vat_total: i64 = x["vat"].as_array().unwrap().iter().map(|b| b["total"].as_i64().unwrap()).sum();
        assert_eq!(vat_total, 6200);

        // Refund the cash order: drawer goes down, sales go down.
        refund(&c, &owner, a, "erreur prix", None, "main").unwrap();
        let x = current(&c, "main").unwrap();
        assert_eq!(x["cash_refunds_cents"], json!(5000));
        assert_eq!(x["expected_cash_cents"], json!(50000 - 2000 + 500));
        assert_eq!(x["orders"], json!(1));
        assert_eq!(x["net_sales_cents"], json!(1200));

        let z = close(&c, &cashier, "main", 48000, "").unwrap();
        assert_eq!(z["z_number"], json!(1));
        assert_eq!(z["difference_cents"], json!(-500));
        assert!(current(&c, "main").unwrap().is_null());
        assert_eq!(close(&c, &cashier, "main", 0, "").unwrap_err().code, "no_cash_session");
        // Z is frozen.
        let sid = z["session_id"].as_i64().unwrap();
        assert_eq!(report(&c, sid).unwrap()["expected_cash_cents"], json!(48500));
        let id2 = open(&c, &cashier, "main", 30000).unwrap();
        assert_ne!(id2, sid);
        let z2 = close(&c, &cashier, "main", 30000, "").unwrap();
        assert_eq!(z2["z_number"], json!(2));
        assert_eq!(z2["difference_cents"], json!(0));
    }
}
