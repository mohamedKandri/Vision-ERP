//! Customers (Phase 9): found by phone at the till, linked to their orders, with an
//! optional loyalty scheme chosen in Settings:
//! - `stamps`: each product marked "earns a stamp" gives one stamp when the order is fully
//!   paid; with `loyalty_stamps_needed` stamps the most expensive eligible item of an order
//!   is free;
//! - `points`: `loyalty_points_per_mad` points per MAD paid; each point is worth
//!   `loyalty_point_value_cents` off a later order.
//!
//! The `loyalty_moves` ledger is the truth. `sync_order` brings it in line with an order
//! (idempotent, called from `orders::recompute` and `orders::void`): points/stamps are earned
//! once paid and taken back on refund; a reward used is given back if the order is voided or
//! refunded, or if the reward discount is removed.

use crate::auth::SessionUser;
use crate::db::atomic;
use crate::error::{ApiError, ApiResult};
use crate::{audit, settings};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

// ------------------------------------------------------------------ pure rules

/// Digits only; "+212 6.." / "00212 6.." become "06..". None if it can't be a phone number.
pub fn normalize_phone(raw: &str) -> Option<String> {
    let t = raw.trim();
    let mut d: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if t.starts_with('+') || t.starts_with("00") {
        if t.starts_with("00") {
            d = d[2..].to_string();
        }
        if let Some(rest) = d.strip_prefix("212") {
            if rest.len() == 9 {
                d = format!("0{rest}");
            }
        }
    } else if d.len() == 12 && d.starts_with("212") {
        d = format!("0{}", &d[3..]);
    }
    (6..=16).contains(&d.len()).then_some(d)
}

/// Points earned on an amount paid: floor(MAD × points per MAD); never negative.
pub fn points_for(total_cents: i64, per_mad: i64) -> i64 {
    if total_cents <= 0 || per_mad <= 0 { 0 } else { total_cents * per_mad / 100 }
}

/// The most points usable on an order: limited by the balance and by the amount due.
pub fn max_points(balance: i64, due_cents: i64, point_value_cents: i64) -> i64 {
    if point_value_cents <= 0 || balance <= 0 || due_cents <= 0 { 0 } else { balance.min(due_cents / point_value_cents) }
}

// ------------------------------------------------------------------ settings

#[derive(Clone, Copy)]
pub struct Scheme {
    pub mode: Mode,
    pub stamps_needed: i64,
    pub points_per_mad: i64,
    pub point_value_cents: i64,
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    None,
    Stamps,
    Points,
}

pub fn scheme(conn: &Connection) -> ApiResult<Scheme> {
    let s = settings::get_all(conn)?;
    let mode = match s["loyalty_mode"].as_str() {
        Some("stamps") => Mode::Stamps,
        Some("points") => Mode::Points,
        _ => Mode::None,
    };
    Ok(Scheme {
        mode,
        stamps_needed: s["loyalty_stamps_needed"].as_i64().unwrap_or(10).max(1),
        points_per_mad: s["loyalty_points_per_mad"].as_i64().unwrap_or(0),
        point_value_cents: s["loyalty_point_value_cents"].as_i64().unwrap_or(0),
    })
}

// ------------------------------------------------------------------ customers

fn check_text(v: &str, max: usize, what: &str) -> ApiResult<()> {
    if v.chars().count() > max {
        return Err(ApiError::invalid(format!("{what} too long")));
    }
    Ok(())
}

/// Find the customer for a phone number typed at the till, creating them if new. A name or
/// address typed for this order becomes the customer's latest one. None when there's no
/// usable phone number.
pub fn link(conn: &Connection, phone: &str, name: &str, address: &str) -> ApiResult<Option<i64>> {
    let Some(phone) = normalize_phone(phone) else { return Ok(None) };
    let (name, address) = (name.trim(), address.trim());
    let found: Option<i64> = conn.query_row("SELECT id FROM customers WHERE phone=?1", [&phone], |r| r.get(0)).optional()?;
    Ok(Some(match found {
        Some(id) => {
            conn.execute(
                "UPDATE customers SET name = CASE WHEN ?2 <> '' THEN ?2 ELSE name END,
                   address = CASE WHEN ?3 <> '' THEN ?3 ELSE address END, active = 1,
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![id, name, address],
            )?;
            id
        }
        None => {
            conn.execute("INSERT INTO customers (name, phone, address) VALUES (?1, ?2, ?3)", params![name, phone, address])?;
            conn.last_insert_rowid()
        }
    }))
}

pub fn balance(conn: &Connection, customer_id: i64) -> ApiResult<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(stamps),0), COALESCE(SUM(points),0) FROM loyalty_moves WHERE customer_id=?1",
        [customer_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn customer_json(conn: &Connection, id: i64) -> ApiResult<Value> {
    let c = conn
        .query_row(
            "SELECT c.id, c.name, c.phone, c.address, c.note, c.active, c.created_at,
               (SELECT COUNT(*) FROM orders o WHERE o.customer_id = c.id AND o.paid_at IS NOT NULL AND o.payment_status <> 'refunded'),
               (SELECT COALESCE(SUM(o.total_cents),0) FROM orders o WHERE o.customer_id = c.id AND o.paid_at IS NOT NULL AND o.payment_status <> 'refunded'),
               (SELECT MAX(o.created_at) FROM orders o WHERE o.customer_id = c.id)
             FROM customers c WHERE c.id=?1",
            [id],
            |r| {
                Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "phone": r.get::<_, String>(2)?,
                          "address": r.get::<_, String>(3)?, "note": r.get::<_, String>(4)?, "active": r.get::<_, bool>(5)?,
                          "created_at": r.get::<_, String>(6)?, "orders": r.get::<_, i64>(7)?, "spent_cents": r.get::<_, i64>(8)?,
                          "last_order_at": r.get::<_, Option<String>>(9)?}))
            },
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("customer"))?;
    let (stamps, points) = balance(conn, id)?;
    let mut c = c;
    c["stamps"] = json!(stamps);
    c["points"] = json!(points);
    Ok(c)
}

/// The till: who is this phone number? (null when unknown)
pub fn lookup(conn: &Connection, phone: &str) -> ApiResult<Value> {
    let Some(phone) = normalize_phone(phone) else { return Ok(Value::Null) };
    match conn.query_row("SELECT id FROM customers WHERE phone=?1 AND active=1", [&phone], |r| r.get::<_, i64>(0)).optional()? {
        Some(id) => customer_json(conn, id),
        None => Ok(Value::Null),
    }
}

#[derive(Deserialize, Default)]
pub struct ListIn {
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub fn list(conn: &Connection, f: &ListIn) -> ApiResult<Value> {
    let q = f.q.as_deref().map(str::trim).unwrap_or("");
    let digits: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
    let ids: Vec<i64> = conn
        .prepare(
            "SELECT id FROM customers
             WHERE ?1 = '' OR instr(lower(name), lower(?1)) > 0 OR (?2 <> '' AND instr(phone, ?2) > 0) OR instr(lower(address), lower(?1)) > 0
             ORDER BY COALESCE((SELECT MAX(created_at) FROM orders o WHERE o.customer_id = customers.id), created_at) DESC, id DESC
             LIMIT ?3 OFFSET ?4",
        )?
        .query_map(params![q, digits, f.limit.unwrap_or(100).clamp(1, 500), f.offset.unwrap_or(0).max(0)], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(json!(ids.into_iter().map(|id| customer_json(conn, id)).collect::<ApiResult<Vec<_>>>()?))
}

/// A customer with their last orders and loyalty moves.
pub fn get(conn: &Connection, id: i64) -> ApiResult<Value> {
    let mut c = customer_json(conn, id)?;
    c["order_list"] = json!(conn
        .prepare(
            "SELECT id, number, split_index, type, status, payment_status, total_cents, created_at, paid_at
             FROM orders WHERE customer_id=?1 ORDER BY id DESC LIMIT 100",
        )?
        .query_map([id], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "number": r.get::<_, i64>(1)?, "split_index": r.get::<_, i64>(2)?,
                      "type": r.get::<_, String>(3)?, "status": r.get::<_, String>(4)?, "payment_status": r.get::<_, String>(5)?,
                      "total_cents": r.get::<_, i64>(6)?, "created_at": r.get::<_, String>(7)?, "paid_at": r.get::<_, Option<String>>(8)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?);
    c["moves"] = json!(conn
        .prepare(
            "SELECT m.at, m.kind, m.stamps, m.points, m.reason, o.number, o.split_index, u.name
             FROM loyalty_moves m LEFT JOIN orders o ON o.id = m.order_id LEFT JOIN users u ON u.id = m.user_id
             WHERE m.customer_id=?1 ORDER BY m.id DESC LIMIT 100",
        )?
        .query_map([id], |r| {
            Ok(json!({"at": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?, "stamps": r.get::<_, i64>(2)?,
                      "points": r.get::<_, i64>(3)?, "reason": r.get::<_, Option<String>>(4)?, "number": r.get::<_, Option<i64>>(5)?,
                      "split_index": r.get::<_, Option<i64>>(6)?, "user": r.get::<_, Option<String>>(7)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?);
    Ok(c)
}

#[derive(Deserialize)]
pub struct CustomerIn {
    pub id: Option<i64>,
    #[serde(default)]
    pub name: String,
    pub phone: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub note: String,
    #[serde(default = "yes")]
    pub active: bool,
}
fn yes() -> bool {
    true
}

pub fn save(conn: &Connection, me: &SessionUser, c: CustomerIn) -> ApiResult<i64> {
    let phone = normalize_phone(&c.phone).ok_or_else(|| ApiError::new("bad_phone", "not a phone number"))?;
    check_text(&c.name, 80, "name")?;
    check_text(&c.address, 300, "address")?;
    check_text(&c.note, 500, "note")?;
    let tx = atomic(conn)?;
    let other: Option<i64> = tx.query_row("SELECT id FROM customers WHERE phone=?1", [&phone], |r| r.get(0)).optional()?;
    if other.is_some() && other != c.id {
        return Err(ApiError::new("phone_taken", "another customer has this phone number"));
    }
    let id = match c.id {
        Some(id) => {
            let n = tx.execute(
                "UPDATE customers SET name=?2, phone=?3, address=?4, note=?5, active=?6, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![id, c.name.trim(), phone, c.address.trim(), c.note.trim(), c.active],
            )?;
            if n == 0 {
                return Err(ApiError::not_found("customer"));
            }
            id
        }
        None => {
            tx.execute(
                "INSERT INTO customers (name, phone, address, note, active) VALUES (?1,?2,?3,?4,?5)",
                params![c.name.trim(), phone, c.address.trim(), c.note.trim(), c.active],
            )?;
            tx.last_insert_rowid()
        }
    };
    audit::log(&tx, Some(me.id), if c.id.is_some() { "customer.updated" } else { "customer.created" }, Some("customer"), Some(&id.to_string()), None)?;
    tx.commit()?;
    Ok(id)
}

/// Manager correction of a balance (a lost card, a goodwill gesture...).
pub fn adjust(conn: &Connection, me: &SessionUser, customer_id: i64, stamps: i64, points: i64, reason: &str) -> ApiResult<()> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(ApiError::new("reason_required", "a reason is required"));
    }
    check_text(reason, 200, "reason")?;
    if (stamps == 0 && points == 0) || stamps.abs() > 1000 || points.abs() > 1_000_000 {
        return Err(ApiError::invalid("bad adjustment"));
    }
    let tx = atomic(conn)?;
    customer_json(&tx, customer_id)?;
    let (s, p) = balance(&tx, customer_id)?;
    if s + stamps < 0 || p + points < 0 {
        return Err(ApiError::new("not_enough", "the balance can't go below zero"));
    }
    tx.execute(
        "INSERT INTO loyalty_moves (customer_id, kind, stamps, points, reason, user_id) VALUES (?1,'adjust',?2,?3,?4,?5)",
        params![customer_id, stamps, points, reason, me.id],
    )?;
    audit::log(&tx, Some(me.id), "loyalty.adjusted", Some("customer"), Some(&customer_id.to_string()),
        Some(json!({"stamps": stamps, "points": points, "reason": reason})))?;
    tx.commit()?;
    Ok(())
}

// ------------------------------------------------------------------ loyalty on orders

struct OrderState {
    customer_id: Option<i64>,
    status: String,
    payment_status: String,
    paid: bool,
    total: i64,
    redeem_stamps: i64,
    redeem_points: i64,
    paid_cents: i64,
}

fn order_state(conn: &Connection, order_id: i64) -> ApiResult<Option<OrderState>> {
    Ok(conn
        .query_row(
            "SELECT customer_id, status, payment_status, paid_at IS NOT NULL, total_cents, loyalty_redeem_stamps, loyalty_redeem_points,
               paid_cents FROM orders WHERE id=?1",
            [order_id],
            |r| {
                Ok(OrderState {
                    customer_id: r.get(0)?, status: r.get(1)?, payment_status: r.get(2)?, paid: r.get(3)?, total: r.get(4)?,
                    redeem_stamps: r.get(5)?, redeem_points: r.get(6)?, paid_cents: r.get(7)?,
                })
            },
        )
        .optional()?)
}

/// Eligible items on an order (products that earn a stamp, not voided, not offered).
fn eligible_qty(conn: &Connection, order_id: i64) -> ApiResult<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(l.qty),0) FROM order_lines l JOIN products p ON p.id = l.product_id
         WHERE l.order_id=?1 AND l.voided=0 AND p.loyalty_stamp=1 AND COALESCE(l.discount_kind,'') <> 'comp'",
        [order_id],
        |r| r.get(0),
    )?)
}

fn recorded(conn: &Connection, order_id: i64, redeem: bool) -> ApiResult<(i64, i64)> {
    // Earn side = earn moves and their reversals; redeem side = redeem moves and theirs.
    // Reversals carry the reason "earn"/"redeem" so the two sides stay apart.
    let sql = if redeem {
        "SELECT COALESCE(SUM(stamps),0), COALESCE(SUM(points),0) FROM loyalty_moves
         WHERE order_id=?1 AND (kind='redeem' OR (kind='reverse' AND reason='redeem'))"
    } else {
        "SELECT COALESCE(SUM(stamps),0), COALESCE(SUM(points),0) FROM loyalty_moves
         WHERE order_id=?1 AND (kind='earn' OR (kind='reverse' AND reason='earn'))"
    };
    Ok(conn.query_row(sql, [order_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// Bring the ledger in line with the order. Safe to call any number of times.
pub fn sync_order(conn: &Connection, order_id: i64, user_id: Option<i64>) -> ApiResult<()> {
    let Some(o) = order_state(conn, order_id)? else { return Ok(()) };
    let Some(cid) = o.customer_id else { return Ok(()) };
    let sch = scheme(conn)?;
    let cancelled = o.status == "voided" || o.payment_status == "refunded";

    // Earned: once fully paid, unless refunded. Stamps don't count the item given as a reward.
    let earn = if o.paid && !cancelled {
        match sch.mode {
            Mode::Stamps => ((eligible_qty(conn, order_id)? - i64::from(o.redeem_stamps > 0)).max(0), 0),
            Mode::Points => (0, points_for(o.total, sch.points_per_mad)),
            Mode::None => (0, 0),
        }
    } else {
        (0, 0)
    };
    // Used: what the reward took, unless the order was cancelled.
    let used = if cancelled { (0, 0) } else { (-o.redeem_stamps, -o.redeem_points) };

    for (target, redeem) in [(earn, false), (used, true)] {
        let have = recorded(conn, order_id, redeem)?;
        let (ds, dp) = (target.0 - have.0, target.1 - have.1);
        if ds == 0 && dp == 0 {
            continue;
        }
        let kind = match (redeem, ds + dp > 0) {
            (false, true) => "earn",
            (true, false) => "redeem",
            _ => "reverse",
        };
        conn.execute(
            "INSERT INTO loyalty_moves (customer_id, order_id, kind, stamps, points, reason, user_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![cid, order_id, kind, ds, dp, if redeem { "redeem" } else { "earn" }, user_id],
        )?;
    }
    Ok(())
}

/// What the till shows for an order's customer: balance and whether a reward can be used.
pub fn order_info(conn: &Connection, order_id: i64) -> ApiResult<Value> {
    let Some(o) = order_state(conn, order_id)? else { return Ok(Value::Null) };
    let Some(cid) = o.customer_id else { return Ok(Value::Null) };
    let sch = scheme(conn)?;
    let c = customer_json(conn, cid)?;
    let (stamps, points) = (c["stamps"].as_i64().unwrap_or(0), c["points"].as_i64().unwrap_or(0));
    let open = o.paid_cents == 0 && !matches!(o.status.as_str(), "closed" | "voided") && o.payment_status == "unpaid";
    let has_discount: bool = conn.query_row("SELECT discount_kind IS NOT NULL FROM orders WHERE id=?1", [order_id], |r| r.get(0))?;
    let redeemed = o.redeem_stamps > 0 || o.redeem_points > 0;
    let (can, max) = match sch.mode {
        Mode::Stamps => (open && !has_discount && stamps >= sch.stamps_needed && eligible_qty(conn, order_id)? > 0, 0),
        Mode::Points => {
            let m = max_points(points, o.total, sch.point_value_cents);
            (open && !has_discount && m > 0, m)
        }
        Mode::None => (false, 0),
    };
    Ok(json!({
        "customer": {"id": cid, "name": c["name"], "phone": c["phone"]},
        "mode": match sch.mode { Mode::Stamps => "stamps", Mode::Points => "points", Mode::None => "none" },
        "stamps": stamps, "points": points, "stamps_needed": sch.stamps_needed, "point_value_cents": sch.point_value_cents,
        "redeemed_stamps": o.redeem_stamps, "redeemed_points": o.redeem_points, "redeemed": redeemed,
        "can_redeem": can, "max_points": max,
    }))
}

/// Use the reward on an unpaid order: a fixed order discount (no manager PIN: the customer
/// earned it), audited. Stamps: the most expensive eligible item. Points: `points` of them
/// (default: as many as possible).
pub fn redeem(conn: &Connection, me: &SessionUser, order_id: i64, points: Option<i64>) -> ApiResult<Value> {
    let tx = atomic(conn)?;
    let o = order_state(&tx, order_id)?.ok_or_else(|| ApiError::not_found("order"))?;
    let cid = o.customer_id.ok_or_else(|| ApiError::new("no_customer", "add the customer's phone number first"))?;
    let (status, paid, kind): (String, i64, Option<String>) =
        tx.query_row("SELECT status, paid_cents, discount_kind FROM orders WHERE id=?1", [order_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    if matches!(status.as_str(), "closed" | "voided") || paid != 0 || o.payment_status != "unpaid" {
        return Err(ApiError::new("order_closed", "rewards are used before payment"));
    }
    if kind.is_some() {
        return Err(ApiError::new("discount_exists", "remove the other discount first"));
    }
    let sch = scheme(&tx)?;
    let (stamps, bal_points) = balance(&tx, cid)?;
    let (value, used_stamps, used_points) = match sch.mode {
        Mode::None => return Err(ApiError::new("loyalty_off", "loyalty is turned off in Settings")),
        Mode::Stamps => {
            if stamps < sch.stamps_needed {
                return Err(ApiError::new("not_enough", "not enough stamps"));
            }
            let best: Option<i64> = tx.query_row(
                "SELECT MAX((l.total_cents + l.qty - 1) / l.qty) FROM order_lines l JOIN products p ON p.id = l.product_id
                 WHERE l.order_id=?1 AND l.voided=0 AND p.loyalty_stamp=1 AND COALESCE(l.discount_kind,'') <> 'comp'",
                [order_id],
                |r| r.get(0),
            )?;
            match best {
                Some(v) if v > 0 => (v.min(o.total), sch.stamps_needed, 0),
                _ => return Err(ApiError::new("nothing_eligible", "no item on this order earns the reward")),
            }
        }
        Mode::Points => {
            let max = max_points(bal_points, o.total, sch.point_value_cents);
            let n = points.unwrap_or(max);
            if n <= 0 || n > max {
                return Err(ApiError::new("not_enough", "not enough points"));
            }
            (n * sch.point_value_cents, 0, n)
        }
    };
    tx.execute(
        "UPDATE orders SET discount_kind='fixed', discount_value=?2, discount_reason='Fidélité', discount_by=?3,
           loyalty_redeem_stamps=?4, loyalty_redeem_points=?5 WHERE id=?1",
        params![order_id, value, me.id, used_stamps, used_points],
    )?;
    audit::log(&tx, Some(me.id), "loyalty.redeemed", Some("order"), Some(&order_id.to_string()),
        Some(json!({"customer_id": cid, "stamps": used_stamps, "points": used_points, "value_cents": value})))?;
    crate::orders::recompute(&tx, order_id)?;
    tx.commit()?;
    Ok(json!({"value_cents": value, "stamps": used_stamps, "points": used_points}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::{demo, simple_line};
    use crate::orders::{self, DiscountIn, NewLine, PayIn, SubmitIn};
    use serde_json::Map;

    #[test]
    fn phones_and_points() {
        assert_eq!(normalize_phone("06 12 34 56 78").as_deref(), Some("0612345678"));
        assert_eq!(normalize_phone("+212 6-12-34-56-78").as_deref(), Some("0612345678"));
        assert_eq!(normalize_phone("00212612345678").as_deref(), Some("0612345678"));
        assert_eq!(normalize_phone("212612345678").as_deref(), Some("0612345678"));
        assert_eq!(normalize_phone("+33 6 12 34 56 78").as_deref(), Some("33612345678"));
        assert_eq!(normalize_phone("0522.11.22.33").as_deref(), Some("0522112233"));
        assert_eq!(normalize_phone("12345"), None);
        assert_eq!(normalize_phone("abc"), None);
        assert_eq!(normalize_phone("1234567890123456789"), None);
        assert_eq!(points_for(4950, 1), 49);
        assert_eq!(points_for(4950, 2), 99);
        assert_eq!(points_for(-500, 1), 0);
        assert_eq!(points_for(5000, 0), 0);
        assert_eq!(max_points(300, 2000, 10), 200, "can't go over the amount due");
        assert_eq!(max_points(50, 2000, 10), 50);
        assert_eq!(max_points(50, 2000, 0), 0);
    }

    fn set(c: &Connection, pairs: &[(&str, Value)]) {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert((*k).into(), v.clone());
        }
        settings::set_many(c, &m).unwrap();
    }
    fn order(c: &Connection, me: &SessionUser, phone: &str, lines: Vec<NewLine>) -> i64 {
        orders::submit(c, me, SubmitIn {
            order_id: None, kind: Some("takeaway".into()), table_id: None, customer_name: Some("Sara".into()),
            customer_phone: Some(phone.into()), customer_address: None, delivery_fee_cents: None, note: None, lines, send: true,
            hold: false, hold_label: None, register: "main".into(),
        })
        .unwrap()
    }
    fn total(c: &Connection, id: i64) -> i64 {
        c.query_row("SELECT total_cents FROM orders WHERE id=?1", [id], |r| r.get(0)).unwrap()
    }
    fn pay(c: &Connection, me: &SessionUser, id: i64) {
        orders::pay(c, me, id, vec![PayIn { method: "cash".into(), amount_cents: total(c, id), tendered_cents: None }], "main").unwrap();
    }
    fn burger(c: &Connection, qty: i64) -> NewLine {
        let mut l = simple_line(c, "Kefta Burger", qty);
        l.config.variant_id = c
            .query_row("SELECT id FROM product_variants WHERE product_id=?1 AND json_extract(names,'$.fr')='Simple'", [l.config.product_id], |r| r.get(0))
            .optional()
            .unwrap();
        let opt: i64 = c.query_row("SELECT id FROM modifier_options WHERE json_extract(names,'$.fr')='À point' ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
        l.config.modifiers = vec![crate::item::ModSel { option_id: opt, qty: 1 }];
        l
    }
    fn ledger_ok(c: &Connection) {
        // Balances never go below zero.
        let bad: i64 = c
            .query_row("SELECT COUNT(*) FROM (SELECT SUM(stamps) s, SUM(points) p FROM loyalty_moves GROUP BY customer_id) WHERE s < 0 OR p < 0", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bad, 0);
    }

    #[test]
    fn customers_are_found_by_phone_and_keep_the_latest_address() {
        let (c, _, cashier) = demo();
        let a = order(&c, &cashier, "06 12 34 56 78", vec![simple_line(&c, "Cookie", 1)]);
        let b = orders::submit(&c, &cashier, SubmitIn {
            order_id: None, kind: Some("delivery".into()), table_id: None, customer_name: Some("".into()),
            customer_phone: Some("+212612345678".into()), customer_address: Some("5 rue Y".into()), delivery_fee_cents: None, note: None,
            lines: vec![simple_line(&c, "Cookie", 1)], send: false, hold: false, hold_label: None, register: "main".into(),
        }).unwrap();
        let ids: Vec<Option<i64>> = [a, b].iter().map(|id| c.query_row("SELECT customer_id FROM orders WHERE id=?1", [id], |r| r.get(0)).unwrap()).collect();
        assert!(ids[0].is_some());
        assert_eq!(ids[0], ids[1], "same customer for both spellings of the number");
        let cust = lookup(&c, "0612345678").unwrap();
        assert_eq!((cust["name"].as_str(), cust["address"].as_str()), (Some("Sara"), Some("5 rue Y")), "an empty name doesn't erase it");
        // No phone, no customer.
        let n = orders::submit(&c, &cashier, SubmitIn { order_id: None, kind: Some("takeaway".into()), table_id: None,
            customer_name: Some("Ali".into()), customer_phone: None, customer_address: None, delivery_fee_cents: None, note: None,
            lines: vec![simple_line(&c, "Cookie", 1)], send: false, hold: false, hold_label: None, register: "main".into() }).unwrap();
        assert_eq!(c.query_row("SELECT customer_id FROM orders WHERE id=?1", [n], |r| r.get::<_, Option<i64>>(0)).unwrap(), None);
        assert_eq!(lookup(&c, "0699999999").unwrap(), Value::Null);
        // Back office: edit, duplicate phone refused, search.
        let id = ids[0].unwrap();
        save(&c, &cashier, CustomerIn { id: Some(id), name: "Sara B.".into(), phone: "0612345678".into(), address: "5 rue Y".into(), note: "VIP".into(), active: true }).unwrap();
        let other = save(&c, &cashier, CustomerIn { id: None, name: "Omar".into(), phone: "0700000000".into(), address: "".into(), note: "".into(), active: true }).unwrap();
        let e = save(&c, &cashier, CustomerIn { id: Some(other), name: "Omar".into(), phone: "06-12-34-56-78".into(), address: "".into(), note: "".into(), active: true }).unwrap_err();
        assert_eq!(e.code, "phone_taken");
        assert_eq!(save(&c, &cashier, CustomerIn { id: None, name: "X".into(), phone: "12".into(), address: "".into(), note: "".into(), active: true }).unwrap_err().code, "bad_phone");
        assert_eq!(list(&c, &ListIn { q: Some("sara".into()), ..Default::default() }).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(list(&c, &ListIn { q: Some("34 56".into()), ..Default::default() }).unwrap()[0]["name"], "Sara B.");
        let g = get(&c, id).unwrap();
        assert_eq!(g["order_list"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn stamps_earned_on_payment_reward_and_reversals() {
        let (c, owner, cashier) = demo();
        set(&c, &[("loyalty_mode", json!("stamps")), ("loyalty_stamps_needed", json!(3))]);
        // 2 burgers + a cookie: 2 stamps, only once paid.
        let a = order(&c, &cashier, "0612345678", vec![burger(&c, 2), simple_line(&c, "Cookie", 1)]);
        let cid: i64 = c.query_row("SELECT customer_id FROM orders WHERE id=?1", [a], |r| r.get(0)).unwrap();
        assert_eq!(balance(&c, cid).unwrap(), (0, 0), "nothing before payment");
        pay(&c, &cashier, a);
        assert_eq!(balance(&c, cid).unwrap().0, 2);
        orders::recompute(&c, a).unwrap();
        assert_eq!(balance(&c, cid).unwrap().0, 2, "idempotent");
        // Not enough yet.
        let b = order(&c, &cashier, "0612345678", vec![burger(&c, 1)]);
        assert_eq!(redeem(&c, &cashier, b, None).unwrap_err().code, "not_enough");
        assert_eq!(order_info(&c, b).unwrap()["can_redeem"], json!(false));
        pay(&c, &cashier, b);
        assert_eq!(balance(&c, cid).unwrap().0, 3);
        // Reward: the most expensive burger of the order is free; that burger earns nothing.
        let r = order(&c, &cashier, "0612345678", vec![burger(&c, 2), simple_line(&c, "Cookie", 1)]);
        assert_eq!(order_info(&c, r).unwrap()["can_redeem"], json!(true));
        let before = total(&c, r);
        let v = redeem(&c, &cashier, r, None).unwrap();
        let burger_price = c.query_row("SELECT unit_cents FROM order_lines WHERE order_id=?1 ORDER BY unit_cents DESC LIMIT 1", [r], |x| x.get::<_, i64>(0)).unwrap();
        assert_eq!(v["value_cents"], json!(burger_price));
        assert_eq!(total(&c, r), before - burger_price);
        assert_eq!(balance(&c, cid).unwrap().0, 0, "3 stamps used");
        assert_eq!(redeem(&c, &cashier, r, None).unwrap_err().code, "discount_exists");
        // Removing the reward discount gives the stamps back; using it again takes them.
        orders::set_order_discount(&c, &owner, r, None, None).unwrap();
        assert_eq!(balance(&c, cid).unwrap().0, 3);
        redeem(&c, &cashier, r, None).unwrap();
        pay(&c, &cashier, r);
        assert_eq!(balance(&c, cid).unwrap().0, 1, "2 burgers, one free: 1 stamp");
        // Refund: the earned stamp is taken back and the used stamps come back.
        orders::refund(&c, &cashier, r, "erreur", Some("2222"), "main").unwrap();
        assert_eq!(balance(&c, cid).unwrap().0, 3);
        // Void an unpaid order with a reward: stamps come back.
        let v2 = order(&c, &cashier, "0612345678", vec![burger(&c, 1)]);
        redeem(&c, &cashier, v2, None).unwrap();
        assert_eq!(balance(&c, cid).unwrap().0, 0);
        orders::void(&c, &cashier, v2, "client parti", Some("2222")).unwrap();
        assert_eq!(balance(&c, cid).unwrap().0, 3);
        ledger_ok(&c);
        let audit: i64 = c.query_row("SELECT COUNT(*) FROM audit_log WHERE action='loyalty.redeemed'", [], |r| r.get(0)).unwrap();
        assert_eq!(audit, 3);
    }

    #[test]
    fn points_earned_used_and_capped() {
        let (c, owner, cashier) = demo();
        set(&c, &[("loyalty_mode", json!("points")), ("loyalty_points_per_mad", json!(1)), ("loyalty_point_value_cents", json!(10))]);
        let a = order(&c, &cashier, "0612345678", vec![simple_line(&c, "Tiramisu", 4)]); // 100,00
        let cid: i64 = c.query_row("SELECT customer_id FROM orders WHERE id=?1", [a], |r| r.get(0)).unwrap();
        pay(&c, &cashier, a);
        assert_eq!(balance(&c, cid).unwrap(), (0, 100));
        // Cookie 12,00: at most 120 points could be used, but the balance is 100 -> 10,00 off.
        let b = order(&c, &cashier, "0612345678", vec![simple_line(&c, "Cookie", 1)]);
        assert_eq!(order_info(&c, b).unwrap()["max_points"], json!(100));
        assert_eq!(redeem(&c, &cashier, b, Some(101)).unwrap_err().code, "not_enough");
        let v = redeem(&c, &cashier, b, Some(40)).unwrap();
        assert_eq!(v["value_cents"], json!(400));
        assert_eq!(total(&c, b), 800);
        assert_eq!(balance(&c, cid).unwrap().1, 60);
        pay(&c, &cashier, b);
        assert_eq!(balance(&c, cid).unwrap().1, 68, "8,00 paid earns 8 points");
        // Manual adjustment (manager), never below zero.
        adjust(&c, &owner, cid, 0, -8, "erreur").unwrap();
        assert_eq!(adjust(&c, &owner, cid, 0, -100, "x").unwrap_err().code, "not_enough");
        assert_eq!(adjust(&c, &owner, cid, 0, 5, " ").unwrap_err().code, "reason_required");
        assert_eq!(balance(&c, cid).unwrap().1, 60);
        // Loyalty off: nothing more is earned, nothing can be used.
        set(&c, &[("loyalty_mode", json!("none"))]);
        let d = order(&c, &cashier, "0612345678", vec![simple_line(&c, "Cookie", 1)]);
        assert_eq!(redeem(&c, &cashier, d, None).unwrap_err().code, "loyalty_off");
        pay(&c, &cashier, d);
        assert_eq!(balance(&c, cid).unwrap().1, 60);
        // A manager discount blocks the reward (one order discount at a time).
        set(&c, &[("loyalty_mode", json!("points"))]);
        let e = order(&c, &cashier, "0612345678", vec![simple_line(&c, "Cookie", 1)]);
        orders::set_order_discount(&c, &owner, e, Some(DiscountIn { kind: "percent".into(), value: 1000, reason: "x".into() }), None).unwrap();
        assert_eq!(redeem(&c, &cashier, e, None).unwrap_err().code, "discount_exists");
        ledger_ok(&c);
    }
}
