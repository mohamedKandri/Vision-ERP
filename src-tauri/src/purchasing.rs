//! Suppliers, purchase orders and goods receipts.
//!
//! A supplier sells ingredients by pack ("Carton 10 kg"). A purchase order goes
//! draft -> ordered -> partial -> closed (or cancelled). Receiving goods adds stock
//! (`purchase` moves valued at the real price) and updates the ingredient's cost as a
//! weighted average. Prices are excluding VAT.

use crate::auth::SessionUser;
use crate::db::atomic;
use crate::error::{ApiError, ApiResult};
use crate::orders::next_seq;
use crate::stock::{self, value_mc, Move};
use crate::audit;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

const MAX_MILLI: i64 = 1_000_000_000_000;
const MAX_CENTS: i64 = 100_000_000; // 1 000 000 DH

// ------------------------------------------------------------------ pure helpers

fn div_round_128(n: i128, d: i128) -> i128 {
    let q = n / d;
    let r = n % d;
    if 2 * r.abs() >= d.abs() {
        q + if (n < 0) != (d < 0) { -1 } else { 1 }
    } else {
        q
    }
}

/// Cents per kg (g), per litre (ml) or per piece, for `value_cents` paid for `qty_milli`.
pub fn unit_cost(unit: &str, qty_milli: i64, value_cents: i64) -> i64 {
    if qty_milli <= 0 {
        return 0;
    }
    let scale: i128 = if unit == "piece" { 1000 } else { 1_000_000 };
    div_round_128(value_cents as i128 * scale, qty_milli as i128) as i64
}

/// New cost after receiving `qty_milli` for `value_cents`, averaged with the stock on hand.
/// With no stock on hand (or negative stock) the received price becomes the cost.
pub fn weighted_cost(unit: &str, old_cost: i64, old_stock_milli: i64, qty_milli: i64, value_cents: i64) -> i64 {
    if qty_milli <= 0 {
        return old_cost;
    }
    if old_stock_milli <= 0 {
        return unit_cost(unit, qty_milli, value_cents);
    }
    let scale: i128 = if unit == "piece" { 1000 } else { 1_000_000 };
    let total_mc = value_mc(unit, old_cost, old_stock_milli) + value_cents as i128 * 1000;
    div_round_128(total_mc * scale, (old_stock_milli + qty_milli) as i128 * 1000) as i64
}

/// Value of `qty_milli` at `pack_cost_cents` per pack of `pack_milli`.
pub fn line_value(qty_milli: i64, pack_milli: i64, pack_cost_cents: i64) -> i64 {
    if pack_milli <= 0 {
        return 0;
    }
    div_round_128(qty_milli as i128 * pack_cost_cents as i128, pack_milli as i128) as i64
}

/// Status of an ordered purchase from (ordered, received) per line.
pub fn po_status(lines: &[(i64, i64)]) -> &'static str {
    if lines.iter().all(|(_, r)| *r == 0) {
        "ordered"
    } else if lines.iter().all(|(o, r)| r >= o) {
        "closed"
    } else {
        "partial"
    }
}

/// Packs to order so stock goes back up to twice the minimum, counting what is already on
/// order. 0 when the ingredient isn't at or under its minimum.
pub fn suggest_packs(stock_milli: i64, min_milli: i64, on_order_milli: i64, pack_milli: i64) -> i64 {
    if min_milli <= 0 || pack_milli <= 0 || stock_milli + on_order_milli > min_milli {
        return 0;
    }
    let need = 2 * min_milli - stock_milli - on_order_milli;
    ((need + pack_milli - 1) / pack_milli).max(1)
}

// ------------------------------------------------------------------ suppliers

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct SupplierItemIn {
    pub ingredient_id: i64,
    #[serde(default)]
    pub pack_label: String,
    pub pack_milli: i64,
    #[serde(default)]
    pub pack_cost_cents: i64,
    #[serde(default)]
    pub preferred: bool,
}

#[derive(Deserialize)]
pub struct SupplierIn {
    pub id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub note: String,
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub items: Vec<SupplierItemIn>,
}

fn check_len(s: &str, max: usize, what: &str) -> ApiResult<()> {
    if s.chars().count() > max {
        return Err(ApiError::invalid(format!("{what} too long")));
    }
    Ok(())
}

fn check_pack(pack_milli: i64, cost: i64, label: &str) -> ApiResult<()> {
    if !(1..=MAX_MILLI).contains(&pack_milli) || !(0..=MAX_CENTS).contains(&cost) {
        return Err(ApiError::invalid("bad pack size or price"));
    }
    check_len(label, 60, "pack label")
}

pub fn save_supplier(conn: &Connection, me: &SessionUser, s: SupplierIn) -> ApiResult<i64> {
    let name = s.name.trim();
    if name.is_empty() {
        return Err(ApiError::invalid("name required"));
    }
    check_len(name, 80, "name")?;
    check_len(&s.phone, 30, "phone")?;
    check_len(&s.email, 120, "email")?;
    check_len(&s.address, 300, "address")?;
    check_len(&s.note, 500, "note")?;
    let mut seen = HashSet::new();
    for i in &s.items {
        if !seen.insert(i.ingredient_id) {
            return Err(ApiError::invalid("an ingredient is listed twice"));
        }
        check_pack(i.pack_milli, i.pack_cost_cents, &i.pack_label)?;
    }
    let tx = atomic(conn)?;
    for i in &s.items {
        if tx.query_row("SELECT 1 FROM ingredients WHERE id=?1", [i.ingredient_id], |_| Ok(())).optional()?.is_none() {
            return Err(ApiError::not_found("ingredient"));
        }
    }
    let id = match s.id {
        None => {
            tx.execute(
                "INSERT INTO suppliers (name, phone, email, address, note, active) VALUES (?1,?2,?3,?4,?5,?6)",
                params![name, s.phone.trim(), s.email.trim(), s.address.trim(), s.note.trim(), s.active],
            )?;
            tx.last_insert_rowid()
        }
        Some(id) => {
            let n = tx.execute(
                "UPDATE suppliers SET name=?2, phone=?3, email=?4, address=?5, note=?6, active=?7 WHERE id=?1",
                params![id, name, s.phone.trim(), s.email.trim(), s.address.trim(), s.note.trim(), s.active],
            )?;
            if n == 0 {
                return Err(ApiError::not_found("supplier"));
            }
            id
        }
    };
    tx.execute("DELETE FROM supplier_items WHERE supplier_id=?1", [id])?;
    for i in &s.items {
        if i.preferred {
            // One preferred supplier per ingredient: this one takes over.
            tx.execute("UPDATE supplier_items SET preferred=0 WHERE ingredient_id=?1 AND supplier_id<>?2", params![i.ingredient_id, id])?;
        }
        tx.execute(
            "INSERT INTO supplier_items (supplier_id, ingredient_id, pack_label, pack_milli, pack_cost_cents, preferred) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, i.ingredient_id, i.pack_label.trim(), i.pack_milli, i.pack_cost_cents, i.preferred],
        )?;
    }
    audit::log(&tx, Some(me.id), if s.id.is_none() { "supplier.created" } else { "supplier.updated" }, Some("supplier"), Some(&id.to_string()),
        Some(json!({"name": name, "items": s.items.len(), "active": s.active})))?;
    tx.commit()?;
    Ok(id)
}

fn names_of(s: String) -> Value {
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

pub fn list_suppliers(conn: &Connection) -> ApiResult<Value> {
    let mut items_stmt = conn.prepare(
        "SELECT si.ingredient_id, i.names, i.unit, si.pack_label, si.pack_milli, si.pack_cost_cents, si.preferred
         FROM supplier_items si JOIN ingredients i ON i.id = si.ingredient_id WHERE si.supplier_id=?1 ORDER BY i.sort, i.id",
    )?;
    let sups: Vec<(i64, String, String, String, String, String, bool, i64)> = conn
        .prepare(
            "SELECT s.id, s.name, s.phone, s.email, s.address, s.note, s.active,
               (SELECT COUNT(*) FROM purchase_orders WHERE supplier_id=s.id AND status IN ('draft','ordered','partial'))
             FROM suppliers s ORDER BY s.active DESC, s.name COLLATE NOCASE",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for (id, name, phone, email, address, note, active, open) in sups {
        let items = items_stmt
            .query_map([id], |r| {
                Ok(json!({"ingredient_id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "unit": r.get::<_, String>(2)?,
                          "pack_label": r.get::<_, String>(3)?, "pack_milli": r.get::<_, i64>(4)?,
                          "pack_cost_cents": r.get::<_, i64>(5)?, "preferred": r.get::<_, bool>(6)?}))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        out.push(json!({"id": id, "name": name, "phone": phone, "email": email, "address": address, "note": note,
                        "active": active, "open_orders": open, "items": items}));
    }
    Ok(json!(out))
}

// ------------------------------------------------------------------ purchase orders

#[derive(Deserialize, Clone)]
pub struct PoLineIn {
    pub ingredient_id: i64,
    #[serde(default)]
    pub pack_label: String,
    pub pack_milli: i64,
    pub qty_packs: i64,
    pub pack_cost_cents: i64,
}

#[derive(Deserialize)]
pub struct PoIn {
    pub id: Option<i64>,
    pub supplier_id: i64,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub expected_date: String,
    #[serde(default)]
    pub lines: Vec<PoLineIn>,
}

struct PoRow {
    number: i64,
    status: String,
}

fn load_po(conn: &Connection, id: i64) -> ApiResult<PoRow> {
    conn.query_row("SELECT number, status FROM purchase_orders WHERE id=?1", [id], |r| Ok(PoRow { number: r.get(0)?, status: r.get(1)? }))
        .optional()?
        .ok_or_else(|| ApiError::not_found("purchase order"))
}

fn valid_date(d: &str) -> bool {
    d.is_empty()
        || (d.len() == 10
            && d.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() }))
}

fn write_lines(conn: &Connection, po_id: i64, lines: &[PoLineIn]) -> ApiResult<()> {
    if lines.len() > 200 {
        return Err(ApiError::invalid("too many lines"));
    }
    for l in lines {
        check_pack(l.pack_milli, l.pack_cost_cents, &l.pack_label)?;
        if !(1..=100_000).contains(&l.qty_packs) {
            return Err(ApiError::invalid("quantity must be 1-100000 packs"));
        }
        if conn.query_row("SELECT 1 FROM ingredients WHERE id=?1", [l.ingredient_id], |_| Ok(())).optional()?.is_none() {
            return Err(ApiError::not_found("ingredient"));
        }
    }
    conn.execute("DELETE FROM purchase_order_lines WHERE po_id=?1", [po_id])?;
    for l in lines {
        conn.execute(
            "INSERT INTO purchase_order_lines (po_id, ingredient_id, pack_label, pack_milli, qty_packs, pack_cost_cents) VALUES (?1,?2,?3,?4,?5,?6)",
            params![po_id, l.ingredient_id, l.pack_label.trim(), l.pack_milli, l.qty_packs, l.pack_cost_cents],
        )?;
    }
    Ok(())
}

/// Create or edit a draft purchase order.
pub fn save_po(conn: &Connection, me: &SessionUser, p: PoIn) -> ApiResult<i64> {
    check_len(&p.note, 500, "note")?;
    if !valid_date(&p.expected_date) {
        return Err(ApiError::invalid("date must be YYYY-MM-DD"));
    }
    let tx = atomic(conn)?;
    let active: Option<bool> = tx.query_row("SELECT active FROM suppliers WHERE id=?1", [p.supplier_id], |r| r.get(0)).optional()?;
    match active {
        None => return Err(ApiError::not_found("supplier")),
        Some(false) => return Err(ApiError::invalid("supplier is inactive")),
        _ => {}
    }
    let id = match p.id {
        None => {
            let number = next_seq(&tx, "po_number")?;
            tx.execute(
                "INSERT INTO purchase_orders (number, supplier_id, note, expected_date, created_by) VALUES (?1,?2,?3,?4,?5)",
                params![number, p.supplier_id, p.note.trim(), p.expected_date, me.id],
            )?;
            tx.last_insert_rowid()
        }
        Some(id) => {
            if load_po(&tx, id)?.status != "draft" {
                return Err(ApiError::new("po_locked", "only a draft can be edited"));
            }
            tx.execute(
                "UPDATE purchase_orders SET supplier_id=?2, note=?3, expected_date=?4 WHERE id=?1",
                params![id, p.supplier_id, p.note.trim(), p.expected_date],
            )?;
            id
        }
    };
    write_lines(&tx, id, &p.lines)?;
    tx.commit()?;
    Ok(id)
}

pub fn list_pos(conn: &Connection, status: Option<String>) -> ApiResult<Value> {
    let rows = conn
        .prepare(
            "SELECT po.id, po.number, po.status, s.name, po.expected_date, po.created_at, po.ordered_at, po.closed_at,
               COALESCE(SUM(l.qty_packs * l.pack_cost_cents), 0), COALESCE(SUM(l.received_cents), 0), COUNT(l.id), po.supplier_id
             FROM purchase_orders po JOIN suppliers s ON s.id = po.supplier_id
             LEFT JOIN purchase_order_lines l ON l.po_id = po.id
             WHERE (?1 IS NULL OR po.status = ?1 OR (?1 = 'open' AND po.status IN ('draft','ordered','partial')))
             GROUP BY po.id ORDER BY po.id DESC LIMIT 200",
        )?
        .query_map([status], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "number": r.get::<_, i64>(1)?, "status": r.get::<_, String>(2)?,
                      "supplier_name": r.get::<_, String>(3)?, "expected_date": r.get::<_, String>(4)?,
                      "created_at": r.get::<_, String>(5)?, "ordered_at": r.get::<_, Option<String>>(6)?,
                      "closed_at": r.get::<_, Option<String>>(7)?, "total_cents": r.get::<_, i64>(8)?,
                      "received_cents": r.get::<_, i64>(9)?, "lines": r.get::<_, i64>(10)?, "supplier_id": r.get::<_, i64>(11)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

pub fn get_po(conn: &Connection, id: i64) -> ApiResult<Value> {
    let head = conn
        .query_row(
            "SELECT po.number, po.status, po.supplier_id, s.name, s.phone, po.note, po.expected_date, po.created_at,
               po.ordered_at, po.closed_at, u.name
             FROM purchase_orders po JOIN suppliers s ON s.id = po.supplier_id JOIN users u ON u.id = po.created_by WHERE po.id=?1",
            [id],
            |r| {
                Ok(json!({"id": id, "number": r.get::<_, i64>(0)?, "status": r.get::<_, String>(1)?, "supplier_id": r.get::<_, i64>(2)?,
                          "supplier_name": r.get::<_, String>(3)?, "supplier_phone": r.get::<_, String>(4)?, "note": r.get::<_, String>(5)?,
                          "expected_date": r.get::<_, String>(6)?, "created_at": r.get::<_, String>(7)?,
                          "ordered_at": r.get::<_, Option<String>>(8)?, "closed_at": r.get::<_, Option<String>>(9)?,
                          "created_by": r.get::<_, String>(10)?}))
            },
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("purchase order"))?;
    let lines = conn
        .prepare(
            "SELECT l.id, l.ingredient_id, i.names, i.unit, l.pack_label, l.pack_milli, l.qty_packs, l.pack_cost_cents,
               l.received_milli, l.received_cents, i.stock_milli
             FROM purchase_order_lines l JOIN ingredients i ON i.id = l.ingredient_id WHERE l.po_id=?1 ORDER BY l.id",
        )?
        .query_map([id], |r| {
            let pack: i64 = r.get(5)?;
            let qty: i64 = r.get(6)?;
            let cost: i64 = r.get(7)?;
            Ok(json!({"id": r.get::<_, i64>(0)?, "ingredient_id": r.get::<_, i64>(1)?, "names": names_of(r.get(2)?),
                      "unit": r.get::<_, String>(3)?, "pack_label": r.get::<_, String>(4)?, "pack_milli": pack, "qty_packs": qty,
                      "pack_cost_cents": cost, "ordered_milli": pack * qty, "total_cents": qty * cost,
                      "received_milli": r.get::<_, i64>(8)?, "received_cents": r.get::<_, i64>(9)?, "stock_milli": r.get::<_, i64>(10)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut rl = conn.prepare("SELECT po_line_id, qty_milli, value_cents FROM purchase_receipt_lines WHERE receipt_id=?1")?;
    let receipts: Vec<(i64, String, String, String)> = conn
        .prepare("SELECT r.id, r.at, r.note, u.name FROM purchase_receipts r JOIN users u ON u.id=r.user_id WHERE r.po_id=?1 ORDER BY r.id")?
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut rec = Vec::new();
    for (rid, at, note, user) in receipts {
        let ls = rl
            .query_map([rid], |r| Ok(json!({"po_line_id": r.get::<_, i64>(0)?, "qty_milli": r.get::<_, i64>(1)?, "value_cents": r.get::<_, i64>(2)?})))?
            .collect::<Result<Vec<_>, _>>()?;
        rec.push(json!({"id": rid, "at": at, "note": note, "user_name": user, "lines": ls}));
    }
    let mut out = head;
    let total: i64 = lines.iter().map(|l| l["total_cents"].as_i64().unwrap()).sum();
    let received: i64 = lines.iter().map(|l| l["received_cents"].as_i64().unwrap()).sum();
    out["lines"] = json!(lines);
    out["receipts"] = json!(rec);
    out["total_cents"] = json!(total);
    out["received_cents"] = json!(received);
    Ok(out)
}

/// Draft -> ordered (sent to the supplier). Lines are locked from now on.
pub fn order_po(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<()> {
    let tx = atomic(conn)?;
    let po = load_po(&tx, id)?;
    if po.status != "draft" {
        return Err(ApiError::new("po_locked", "only a draft can be ordered"));
    }
    let n: i64 = tx.query_row("SELECT COUNT(*) FROM purchase_order_lines WHERE po_id=?1", [id], |r| r.get(0))?;
    if n == 0 {
        return Err(ApiError::new("po_empty", "add at least one line"));
    }
    tx.execute("UPDATE purchase_orders SET status='ordered', ordered_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", [id])?;
    let total: i64 = tx.query_row("SELECT SUM(qty_packs*pack_cost_cents) FROM purchase_order_lines WHERE po_id=?1", [id], |r| r.get(0))?;
    audit::log(&tx, Some(me.id), "purchase.ordered", Some("purchase_order"), Some(&id.to_string()), Some(json!({"number": po.number, "total_cents": total})))?;
    tx.commit()?;
    Ok(())
}

#[derive(Deserialize)]
pub struct ReceiveLineIn {
    pub line_id: i64,
    pub qty_milli: i64,
    /// Price per pack actually charged; defaults to the ordered price.
    pub pack_cost_cents: Option<i64>,
}

#[derive(Deserialize)]
pub struct ReceiveIn {
    pub id: i64,
    pub lines: Vec<ReceiveLineIn>,
    #[serde(default)]
    pub note: String,
}

/// Record goods received (all or part of an order). Stock goes up, the cost is averaged,
/// and the supplier's last price is remembered. Returns {status, value_cents}.
pub fn receive(conn: &Connection, me: &SessionUser, r: ReceiveIn) -> ApiResult<Value> {
    check_len(&r.note, 300, "note")?;
    let lines: Vec<&ReceiveLineIn> = r.lines.iter().filter(|l| l.qty_milli != 0).collect();
    if lines.is_empty() {
        return Err(ApiError::invalid("nothing received"));
    }
    let tx = atomic(conn)?;
    let po = load_po(&tx, r.id)?;
    if po.status != "ordered" && po.status != "partial" {
        return Err(ApiError::new("po_locked", "this order can't be received"));
    }
    let supplier: i64 = tx.query_row("SELECT supplier_id FROM purchase_orders WHERE id=?1", [r.id], |x| x.get(0))?;
    tx.execute("INSERT INTO purchase_receipts (po_id, note, user_id) VALUES (?1,?2,?3)", params![r.id, r.note.trim(), me.id])?;
    let receipt = tx.last_insert_rowid();
    let reason = format!("BC-{}", po.number);
    let mut seen = HashSet::new();
    let mut total = 0;
    for l in lines {
        if !seen.insert(l.line_id) {
            return Err(ApiError::invalid("a line is listed twice"));
        }
        if l.qty_milli < 0 || l.qty_milli > MAX_MILLI {
            return Err(ApiError::invalid("bad quantity"));
        }
        let (po_id, ing, label, pack, cost): (i64, i64, String, i64, i64) = tx
            .query_row(
                "SELECT po_id, ingredient_id, pack_label, pack_milli, pack_cost_cents FROM purchase_order_lines WHERE id=?1",
                [l.line_id],
                |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?)),
            )
            .optional()?
            .ok_or_else(|| ApiError::not_found("line"))?;
        if po_id != r.id {
            return Err(ApiError::invalid("line is not on this order"));
        }
        let price = l.pack_cost_cents.unwrap_or(cost);
        if !(0..=MAX_CENTS).contains(&price) {
            return Err(ApiError::invalid("bad price"));
        }
        let value = line_value(l.qty_milli, pack, price);
        let (unit, old_cost, stock_milli): (String, i64, i64) =
            tx.query_row("SELECT unit, cost_cents, stock_milli FROM ingredients WHERE id=?1", [ing], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?)))?;
        let new_cost = weighted_cost(&unit, old_cost, stock_milli, l.qty_milli, value);
        tx.execute("UPDATE ingredients SET cost_cents=?2 WHERE id=?1", params![ing, new_cost])?;
        let mut m = Move::new(ing, l.qty_milli, "purchase");
        m.reason = &reason;
        m.po_id = Some(r.id);
        m.user_id = Some(me.id);
        m.value_cents = Some(value);
        stock::record(&tx, &m)?;
        tx.execute(
            "UPDATE purchase_order_lines SET received_milli = received_milli + ?2, received_cents = received_cents + ?3 WHERE id=?1",
            params![l.line_id, l.qty_milli, value],
        )?;
        tx.execute(
            "INSERT INTO purchase_receipt_lines (receipt_id, po_line_id, qty_milli, value_cents) VALUES (?1,?2,?3,?4)",
            params![receipt, l.line_id, l.qty_milli, value],
        )?;
        // Remember the supplier's latest price for this pack.
        tx.execute(
            "INSERT INTO supplier_items (supplier_id, ingredient_id, pack_label, pack_milli, pack_cost_cents) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT (supplier_id, ingredient_id) DO UPDATE SET pack_cost_cents = excluded.pack_cost_cents
             WHERE supplier_items.pack_milli = excluded.pack_milli",
            params![supplier, ing, label, pack, price],
        )?;
        total += value;
    }
    let state: Vec<(i64, i64)> = tx
        .prepare("SELECT qty_packs * pack_milli, received_milli FROM purchase_order_lines WHERE po_id=?1")?
        .query_map([r.id], |x| Ok((x.get(0)?, x.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let status = po_status(&state);
    tx.execute(
        "UPDATE purchase_orders SET status=?2, closed_at = CASE WHEN ?2='closed' THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') END WHERE id=?1",
        params![r.id, status],
    )?;
    audit::log(&tx, Some(me.id), "purchase.received", Some("purchase_order"), Some(&r.id.to_string()),
        Some(json!({"number": po.number, "value_cents": total, "status": status, "receipt_id": receipt})))?;
    stock::refresh_86(&tx, Some(me.id))?;
    tx.commit()?;
    Ok(json!({"status": status, "value_cents": total}))
}

/// Close an order that won't be delivered in full (what's missing is no longer expected).
pub fn close_po(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<()> {
    let po = load_po(conn, id)?;
    if po.status != "ordered" && po.status != "partial" {
        return Err(ApiError::new("po_locked", "only an ordered purchase can be closed"));
    }
    conn.execute("UPDATE purchase_orders SET status='closed', closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", [id])?;
    audit::log(conn, Some(me.id), "purchase.closed", Some("purchase_order"), Some(&id.to_string()), Some(json!({"number": po.number, "was": po.status})))?;
    Ok(())
}

/// Cancel a draft or an order nothing was received for.
pub fn cancel_po(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<()> {
    let po = load_po(conn, id)?;
    if po.status != "draft" && po.status != "ordered" {
        return Err(ApiError::new("po_locked", "goods were already received: close it instead"));
    }
    conn.execute("UPDATE purchase_orders SET status='cancelled', closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", [id])?;
    audit::log(conn, Some(me.id), "purchase.cancelled", Some("purchase_order"), Some(&id.to_string()), Some(json!({"number": po.number})))?;
    Ok(())
}

// ------------------------------------------------------------------ suggestions

/// Ingredients at or under their minimum, with the quantity still on order, the supplier to
/// buy from (preferred, else the cheapest per unit) and how many packs to order.
pub fn suggestions(conn: &Connection) -> ApiResult<Value> {
    let rows: Vec<(i64, String, String, i64, i64, i64)> = conn
        .prepare(
            "SELECT i.id, i.names, i.unit, i.stock_milli, i.min_milli,
               COALESCE((SELECT SUM(MAX(l.qty_packs * l.pack_milli - l.received_milli, 0)) FROM purchase_order_lines l
                 JOIN purchase_orders po ON po.id = l.po_id
                 WHERE l.ingredient_id = i.id AND po.status IN ('draft','ordered','partial')), 0)
             FROM ingredients i WHERE i.active = 1 AND i.min_milli > 0 ORDER BY i.sort, i.id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<Result<_, _>>()?;
    let mut sup = conn.prepare(
        "SELECT si.supplier_id, s.name, si.pack_label, si.pack_milli, si.pack_cost_cents FROM supplier_items si
         JOIN suppliers s ON s.id = si.supplier_id WHERE si.ingredient_id=?1 AND s.active=1
         ORDER BY si.preferred DESC, CAST(si.pack_cost_cents AS REAL) / si.pack_milli, s.name LIMIT 1",
    )?;
    let mut out = Vec::new();
    for (id, names, unit, stock_milli, min, on_order) in rows {
        if stock_milli > min {
            continue;
        }
        let s: Option<(i64, String, String, i64, i64)> =
            sup.query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
        let (packs, sid, sname, label, pack, cost) = match &s {
            Some((sid, sname, label, pack, cost)) => (suggest_packs(stock_milli, min, on_order, *pack), Some(*sid), Some(sname.clone()), label.clone(), *pack, *cost),
            None if stock_milli + on_order > min => continue,
            None => (0, None, None, String::new(), 0, 0),
        };
        if s.is_some() && packs == 0 {
            continue; // enough already on order
        }
        out.push(json!({
            "ingredient_id": id, "names": names_of(names), "unit": unit, "stock_milli": stock_milli, "min_milli": min,
            "on_order_milli": on_order, "supplier_id": sid, "supplier_name": sname, "pack_label": label, "pack_milli": pack,
            "pack_cost_cents": cost, "packs": packs, "total_cents": packs * cost,
        }));
    }
    Ok(json!(out))
}

#[derive(Deserialize)]
pub struct SuggestedIn {
    pub ingredient_id: i64,
    pub supplier_id: i64,
    pub packs: i64,
}

/// One draft purchase order per supplier from the chosen suggestions. Returns the new ids.
pub fn create_from_suggestions(conn: &Connection, me: &SessionUser, picks: Vec<SuggestedIn>) -> ApiResult<Vec<i64>> {
    if picks.is_empty() {
        return Err(ApiError::invalid("choose at least one ingredient"));
    }
    let tx = atomic(conn)?;
    let mut by_supplier: BTreeMap<i64, Vec<PoLineIn>> = BTreeMap::new();
    for p in &picks {
        let (label, pack, cost): (String, i64, i64) = tx
            .query_row(
                "SELECT pack_label, pack_milli, pack_cost_cents FROM supplier_items WHERE supplier_id=?1 AND ingredient_id=?2",
                params![p.supplier_id, p.ingredient_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| ApiError::invalid("this supplier doesn't sell that ingredient"))?;
        by_supplier.entry(p.supplier_id).or_default().push(PoLineIn {
            ingredient_id: p.ingredient_id, pack_label: label, pack_milli: pack, qty_packs: p.packs, pack_cost_cents: cost,
        });
    }
    let mut ids = Vec::new();
    for (supplier_id, lines) in by_supplier {
        ids.push(save_po(&tx, me, PoIn { id: None, supplier_id, note: String::new(), expected_date: String::new(), lines })?);
    }
    audit::log(&tx, Some(me.id), "purchase.suggested_created", None, None, Some(json!({"orders": ids})))?;
    tx.commit()?;
    Ok(ids)
}

// ------------------------------------------------------------------ demo data

/// Four demo suppliers for the demo ingredients (matched by French name). Does nothing when
/// suppliers already exist.
pub fn seed_demo(conn: &Connection) -> ApiResult<bool> {
    let any: i64 = conn.query_row("SELECT COUNT(*) FROM suppliers", [], |r| r.get(0))?;
    if any > 0 {
        return Ok(false);
    }
    // (supplier, phone, [(ingredient fr name, pack label, pack size in units, pack price in cents)])
    #[rustfmt::skip]
    let data: &[(&str, &str, &[(&str, &str, f64, i64)])] = &[
        ("Boucherie Al Baraka", "0522 00 00 01", &[
            ("Viande hachée de bœuf", "Sachet 5 kg", 5000.0, 45000), ("Kefta", "Sachet 2 kg", 2000.0, 19000),
            ("Filet de poulet", "Carton 5 kg", 5000.0, 32500), ("Bacon de bœuf", "Barquette 1 kg", 1000.0, 18000),
        ]),
        ("Boulangerie du Centre", "0522 00 00 02", &[
            ("Pain burger", "Sachet de 24", 24.0, 6000), ("Cookie", "Boîte de 12", 12.0, 4800),
            ("Brownie (portion)", "Plaque de 12", 12.0, 7200), ("Tiramisu (portion)", "Plateau de 10", 10.0, 9000),
        ]),
        ("Primeur Souk", "0661 00 00 03", &[
            ("Oignon", "Sac 5 kg", 5000.0, 4000), ("Tomate", "Cagette 5 kg", 5000.0, 4500), ("Salade", "Caisse 2 kg", 2000.0, 3000),
            ("Champignons", "Barquette 1 kg", 1000.0, 4000), ("Avocat", "Plateau 2 kg", 2000.0, 7000), ("Oranges", "Sac 10 kg", 10000.0, 8000),
        ]),
        ("Grossiste Métro", "0522 00 00 04", &[
            ("Cheddar (tranche)", "Paquet de 84", 84.0, 10080), ("Emmental (tranche)", "Paquet de 40", 40.0, 6000),
            ("Frites surgelées", "Carton 4 x 2,5 kg", 10000.0, 18000), ("Potatoes surgelées", "Sac 2,5 kg", 2500.0, 6500),
            ("Rings d'oignon surgelés", "Sac 1 kg", 1000.0, 4000), ("Nuggets", "Carton de 100", 100.0, 2500),
            ("Poulet pané", "Carton de 40", 40.0, 24000), ("Galette végétale", "Carton de 20", 20.0, 18000),
            ("Sauce burger", "Bidon 5 L", 5000.0, 20000), ("Ketchup", "Bidon 5 L", 5000.0, 12500), ("Mayonnaise", "Bidon 5 L", 5000.0, 15000),
            ("Canette Coca-Cola 33cl", "Pack de 24", 24.0, 10800), ("Canette Fanta 33cl", "Pack de 24", 24.0, 10800),
            ("Eau minérale 50cl", "Pack de 12", 12.0, 3000), ("Boîte burger", "Carton de 250", 250.0, 20000),
            ("Cornet frites", "Carton de 500", 500.0, 20000), ("Sachet", "Paquet de 100", 100.0, 5000),
        ]),
    ];
    let tx = atomic(conn)?;
    for (name, phone, items) in data {
        tx.execute("INSERT INTO suppliers (name, phone) VALUES (?1, ?2)", params![name, phone])?;
        let sid = tx.last_insert_rowid();
        for (fr, label, units, price) in items.iter() {
            let ing: Option<i64> = tx
                .query_row("SELECT id FROM ingredients WHERE json_extract(names,'$.fr')=?1", [fr], |r| r.get(0))
                .optional()?;
            if let Some(ing) = ing {
                tx.execute(
                    "INSERT INTO supplier_items (supplier_id, ingredient_id, pack_label, pack_milli, pack_cost_cents, preferred)
                     VALUES (?1,?2,?3,?4,?5,1)",
                    params![sid, ing, label, (units * 1000.0).round() as i64, price],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(true)
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::demo;

    #[test]
    fn pure_rules() {
        // 10 kg bought for 900.00 -> 90.00 per kg; 24 buns for 60.00 -> 2.50 each.
        assert_eq!(unit_cost("g", 10_000_000, 90_000), 9000);
        assert_eq!(unit_cost("piece", 24_000, 6000), 250);
        assert_eq!(unit_cost("g", 0, 100), 0);
        // Average: 5 kg at 90/kg on hand + 5 kg bought at 100/kg -> 95/kg.
        assert_eq!(weighted_cost("g", 9000, 5_000_000, 5_000_000, 50_000), 9500);
        // No stock (or negative stock) on hand: the new price is the cost.
        assert_eq!(weighted_cost("g", 9000, 0, 5_000_000, 50_000), 10000);
        assert_eq!(weighted_cost("g", 9000, -200_000, 5_000_000, 50_000), 10000);
        // 100 buns at 2.50 + 24 bought at 2.75 each (66.00) -> 316.00 / 124 = 2.5484 -> 2.55.
        assert_eq!(weighted_cost("piece", 250, 100_000, 24_000, 6600), 255);
        assert_eq!(weighted_cost("piece", 250, 100_000, 0, 6600), 250);
        // Value of a partial pack: 2.5 kg of a 5 kg pack at 450.00 = 225.00; rounding.
        assert_eq!(line_value(2_500_000, 5_000_000, 45_000), 22_500);
        assert_eq!(line_value(1000, 3000, 100), 33);
        assert_eq!(line_value(2000, 3000, 100), 67);
        // Status.
        assert_eq!(po_status(&[(10, 0), (5, 0)]), "ordered");
        assert_eq!(po_status(&[(10, 10), (5, 0)]), "partial");
        assert_eq!(po_status(&[(10, 12), (5, 5)]), "closed", "more than ordered still closes");
        // Suggestions: back up to 2 x minimum, whole packs, counting what is on order.
        assert_eq!(suggest_packs(30_000, 40_000, 0, 24_000), 3, "need 50 buns -> 3 packs of 24");
        assert_eq!(suggest_packs(40_000, 40_000, 0, 24_000), 2, "at the minimum counts");
        assert_eq!(suggest_packs(50_000, 40_000, 0, 24_000), 0);
        assert_eq!(suggest_packs(30_000, 40_000, 24_000, 24_000), 0, "already on order");
        assert_eq!(suggest_packs(10_000, 40_000, 24_000, 24_000), 2);
        assert_eq!(suggest_packs(39_000, 40_000, 0, 100_000), 1, "one big pack is enough");
        assert_eq!(suggest_packs(0, 0, 0, 24_000), 0, "no minimum, no suggestion");
    }

    fn ing(c: &Connection, fr: &str) -> (i64, i64, i64) {
        c.query_row("SELECT id, stock_milli, cost_cents FROM ingredients WHERE json_extract(names,'$.fr')=?1", [fr], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
    }
    fn sid(c: &Connection, name: &str) -> i64 {
        c.query_row("SELECT id FROM suppliers WHERE name=?1", [name], |r| r.get(0)).unwrap()
    }

    #[test]
    fn order_receive_partial_then_full_updates_stock_and_cost() {
        let (c, owner, _) = demo();
        stock::seed_demo(&c, owner.id).unwrap();
        assert!(!seed_demo(&c).unwrap(), "stock demo seeds suppliers too");
        let butcher = sid(&c, "Boucherie Al Baraka");
        let (beef, beef0, cost0) = ing(&c, "Viande hachée de bœuf");
        assert_eq!((beef0, cost0), (20_000_000, 9000));

        // Draft: 4 bags of 5 kg at 450.00.
        let id = save_po(&c, &owner, PoIn { id: None, supplier_id: butcher, note: "lundi".into(), expected_date: "2026-11-20".into(),
            lines: vec![PoLineIn { ingredient_id: beef, pack_label: "Sachet 5 kg".into(), pack_milli: 5_000_000, qty_packs: 4, pack_cost_cents: 45_000 }] }).unwrap();
        let po = get_po(&c, id).unwrap();
        assert_eq!(po["status"], "draft");
        assert_eq!(po["total_cents"], 180_000);
        assert_eq!(po["number"], 1);
        // Can't receive a draft; ordering locks it.
        let line = po["lines"][0]["id"].as_i64().unwrap();
        assert_eq!(receive(&c, &owner, ReceiveIn { id, lines: vec![ReceiveLineIn { line_id: line, qty_milli: 1, pack_cost_cents: None }], note: String::new() }).unwrap_err().code, "po_locked");
        order_po(&c, &owner, id).unwrap();
        assert_eq!(save_po(&c, &owner, PoIn { id: Some(id), supplier_id: butcher, note: String::new(), expected_date: String::new(), lines: vec![] }).unwrap_err().code, "po_locked");

        // First delivery: 10 kg, at a higher price (500.00 per 5 kg = 100/kg).
        let r = receive(&c, &owner, ReceiveIn { id, lines: vec![ReceiveLineIn { line_id: line, qty_milli: 10_000_000, pack_cost_cents: Some(50_000) }], note: "BL 123".into() }).unwrap();
        assert_eq!(r["status"], "partial");
        assert_eq!(r["value_cents"], 100_000);
        let (_, beef1, cost1) = ing(&c, "Viande hachée de bœuf");
        assert_eq!(beef1, 30_000_000);
        // (20 kg x 90 + 10 kg x 100) / 30 kg = 93.33
        assert_eq!(cost1, 9333);
        // The supplier's price list now shows 500.00.
        let p: i64 = c.query_row("SELECT pack_cost_cents FROM supplier_items WHERE supplier_id=?1 AND ingredient_id=?2", params![butcher, beef], |r| r.get(0)).unwrap();
        assert_eq!(p, 50_000);
        // Rest of the order: 10 kg at the ordered price -> closed.
        let r = receive(&c, &owner, ReceiveIn { id, lines: vec![ReceiveLineIn { line_id: line, qty_milli: 10_000_000, pack_cost_cents: None }], note: String::new() }).unwrap();
        assert_eq!(r["status"], "closed");
        let po = get_po(&c, id).unwrap();
        assert_eq!(po["received_cents"], 190_000);
        assert_eq!(po["receipts"].as_array().unwrap().len(), 2);
        assert_eq!(po["receipts"][0]["note"], "BL 123");
        assert_eq!(receive(&c, &owner, ReceiveIn { id, lines: vec![ReceiveLineIn { line_id: line, qty_milli: 1, pack_cost_cents: None }], note: String::new() }).unwrap_err().code, "po_locked");
        // Ledger: purchase moves valued at the real price, linked to the order.
        let h = stock::history(&c, stock::HistoryIn { ingredient_id: Some(beef), kind: Some("purchase".into()), ..Default::default() }).unwrap();
        let vals: Vec<i64> = h.as_array().unwrap().iter().map(|m| m["value_cents"].as_i64().unwrap()).collect();
        assert_eq!(vals, [90_000, 100_000]);
        assert_eq!(h[0]["po_number"], 1);
        assert_eq!(h[0]["reason"], "BC-1");
        let bad: i64 = c.query_row("SELECT COUNT(*) FROM ingredients i WHERE stock_milli <> (SELECT COALESCE(SUM(delta_milli),0) FROM stock_moves WHERE ingredient_id=i.id)", [], |r| r.get(0)).unwrap();
        assert_eq!(bad, 0);
    }

    #[test]
    fn close_cancel_and_validation() {
        let (c, owner, _) = demo();
        stock::seed_demo(&c, owner.id).unwrap();
        let s = sid(&c, "Primeur Souk");
        let (tomato, _, _) = ing(&c, "Tomate");
        let mk = |n: i64| save_po(&c, &owner, PoIn { id: None, supplier_id: s, note: String::new(), expected_date: String::new(),
            lines: vec![PoLineIn { ingredient_id: tomato, pack_label: "Cagette 5 kg".into(), pack_milli: 5_000_000, qty_packs: n, pack_cost_cents: 4500 }] });
        // Validation.
        assert_eq!(mk(0).unwrap_err().code, "invalid");
        assert_eq!(save_po(&c, &owner, PoIn { id: None, supplier_id: s, note: String::new(), expected_date: "20/11/2026".into(), lines: vec![] }).unwrap_err().code, "invalid");
        assert_eq!(save_po(&c, &owner, PoIn { id: None, supplier_id: 999, note: String::new(), expected_date: String::new(), lines: vec![] }).unwrap_err().code, "not_found");
        let empty = save_po(&c, &owner, PoIn { id: None, supplier_id: s, note: String::new(), expected_date: String::new(), lines: vec![] }).unwrap();
        assert_eq!(order_po(&c, &owner, empty).unwrap_err().code, "po_empty");
        // Cancel a draft; cancel an ordered one; can't cancel after a receipt, but can close.
        cancel_po(&c, &owner, empty).unwrap();
        let a = mk(2).unwrap();
        order_po(&c, &owner, a).unwrap();
        cancel_po(&c, &owner, a).unwrap();
        assert_eq!(order_po(&c, &owner, a).unwrap_err().code, "po_locked");
        let b = mk(2).unwrap();
        order_po(&c, &owner, b).unwrap();
        let line = get_po(&c, b).unwrap()["lines"][0]["id"].as_i64().unwrap();
        receive(&c, &owner, ReceiveIn { id: b, lines: vec![ReceiveLineIn { line_id: line, qty_milli: 5_000_000, pack_cost_cents: None }], note: String::new() }).unwrap();
        assert_eq!(cancel_po(&c, &owner, b).unwrap_err().code, "po_locked");
        close_po(&c, &owner, b).unwrap();
        assert_eq!(get_po(&c, b).unwrap()["status"], "closed");
        // A line from another order is refused.
        let d = mk(1).unwrap();
        order_po(&c, &owner, d).unwrap();
        assert_eq!(receive(&c, &owner, ReceiveIn { id: d, lines: vec![ReceiveLineIn { line_id: line, qty_milli: 1, pack_cost_cents: None }], note: String::new() }).unwrap_err().code, "invalid");
        let open = list_pos(&c, Some("open".into())).unwrap();
        assert_eq!(open.as_array().unwrap().iter().map(|p| p["id"].as_i64().unwrap()).collect::<Vec<_>>(), [d]);
    }

    #[test]
    fn suppliers_preferred_and_suggestions() {
        let (c, owner, _) = demo();
        stock::seed_demo(&c, owner.id).unwrap();
        let (bun, _, _) = ing(&c, "Pain burger");
        // Nothing low yet.
        assert!(suggestions(&c).unwrap().as_array().unwrap().is_empty());
        // Bring buns to 30 (minimum 40): 50 needed -> 3 packs of 24 from the bakery.
        stock::adjust(&c, &owner, bun, -170_000, "test").unwrap();
        let sg = suggestions(&c).unwrap();
        let row = sg.as_array().unwrap().iter().find(|r| r["ingredient_id"] == bun).unwrap().clone();
        assert_eq!(row["supplier_name"], "Boulangerie du Centre");
        assert_eq!(row["packs"], 3);
        assert_eq!(row["total_cents"], 18_000);
        // A second, cheaper supplier marked preferred takes over.
        let other = save_supplier(&c, &owner, SupplierIn { id: None, name: "Pain Express".into(), phone: String::new(), email: String::new(),
            address: String::new(), note: String::new(), active: true,
            items: vec![SupplierItemIn { ingredient_id: bun, pack_label: "Sachet de 50".into(), pack_milli: 50_000, pack_cost_cents: 11_000, preferred: true }] }).unwrap();
        let prefs: i64 = c.query_row("SELECT COUNT(*) FROM supplier_items WHERE ingredient_id=?1 AND preferred=1", [bun], |r| r.get(0)).unwrap();
        assert_eq!(prefs, 1, "only one preferred supplier per ingredient");
        let row = suggestions(&c).unwrap().as_array().unwrap().iter().find(|r| r["ingredient_id"] == bun).unwrap().clone();
        assert_eq!(row["supplier_id"], other);
        assert_eq!(row["packs"], 1);
        // Create the drafts; what's on order is no longer suggested.
        let ids = create_from_suggestions(&c, &owner, vec![SuggestedIn { ingredient_id: bun, supplier_id: other, packs: 1 }]).unwrap();
        assert_eq!(ids.len(), 1);
        let po = get_po(&c, ids[0]).unwrap();
        assert_eq!((po["status"].as_str(), po["total_cents"].as_i64()), (Some("draft"), Some(11_000)));
        assert!(suggestions(&c).unwrap().as_array().unwrap().iter().all(|r| r["ingredient_id"] != bun));
        assert_eq!(create_from_suggestions(&c, &owner, vec![SuggestedIn { ingredient_id: bun, supplier_id: sid(&c, "Primeur Souk"), packs: 1 }]).unwrap_err().code, "invalid");
        // Supplier validation.
        let dup = save_supplier(&c, &owner, SupplierIn { id: None, name: "X".into(), phone: String::new(), email: String::new(), address: String::new(), note: String::new(), active: true,
            items: vec![
                SupplierItemIn { ingredient_id: bun, pack_label: String::new(), pack_milli: 1000, pack_cost_cents: 1, preferred: false },
                SupplierItemIn { ingredient_id: bun, pack_label: String::new(), pack_milli: 1000, pack_cost_cents: 1, preferred: false },
            ] });
        assert_eq!(dup.unwrap_err().code, "invalid");
        assert_eq!(save_supplier(&c, &owner, SupplierIn { id: None, name: " ".into(), phone: String::new(), email: String::new(), address: String::new(), note: String::new(), active: true, items: vec![] }).unwrap_err().code, "invalid");
        let list = list_suppliers(&c).unwrap();
        assert_eq!(list.as_array().unwrap().iter().find(|s| s["id"] == other).unwrap()["open_orders"], 1);
    }

    #[test]
    fn api_permissions() {
        use crate::api::{dispatch, Core};
        let core = Core::new(crate::db::open_in_memory().unwrap());
        dispatch(&core, None, "setup.init", json!({"shop_name": "B", "language": "fr", "owner_name": "Owner", "owner_pin": "1234", "demo_users": true, "demo_menu": true})).unwrap();
        let login = |name: &str, pin: &str| -> String {
            let users = dispatch(&core, None, "auth.users", Value::Null).unwrap();
            let id = users.as_array().unwrap().iter().find(|u| u["name"] == name).unwrap()["id"].clone();
            dispatch(&core, None, "auth.login", json!({"user_id": id, "pin": pin})).unwrap()["token"].as_str().unwrap().to_string()
        };
        let cashier = login("Salma", "3333");
        let manager = login("Karim", "2222");
        for m in ["suppliers.list", "purchases.list", "purchases.suggestions"] {
            assert_eq!(dispatch(&core, Some(&cashier), m, Value::Null).unwrap_err().code, "forbidden", "{m}");
        }
        let sups = dispatch(&core, Some(&manager), "suppliers.list", Value::Null).unwrap();
        assert_eq!(sups.as_array().unwrap().len(), 4, "setup with the demo menu seeds suppliers");
        let s = &sups[0];
        let item = &s["items"][0];
        let id = dispatch(&core, Some(&manager), "purchases.save", json!({"supplier_id": s["id"], "lines": [
            {"ingredient_id": item["ingredient_id"], "pack_label": item["pack_label"], "pack_milli": item["pack_milli"], "qty_packs": 2, "pack_cost_cents": item["pack_cost_cents"]}]})).unwrap()["id"].clone();
        dispatch(&core, Some(&manager), "purchases.order", json!({"id": id})).unwrap();
        let po = dispatch(&core, Some(&manager), "purchases.get", json!({"id": id})).unwrap();
        let r = dispatch(&core, Some(&manager), "purchases.receive", json!({"id": id, "lines": [{"line_id": po["lines"][0]["id"], "qty_milli": po["lines"][0]["ordered_milli"]}]})).unwrap();
        assert_eq!(r["status"], "closed");
        assert_eq!(crate::net::topic_of("purchases.receive"), Some("stock"));
        assert_eq!(crate::net::topic_of("purchases.suggestions"), None);
        assert_eq!(crate::net::topic_of("suppliers.save"), Some("stock"));
    }
}
