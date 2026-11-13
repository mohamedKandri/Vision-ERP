//! Orders: create/append lines, discounts (with manager approval), payments, status flow,
//! split, void, refund, reopen. Totals are always recomputed here from stored lines.

use crate::auth::{self, SessionUser};
use crate::db::atomic;
use crate::error::{ApiError, ApiResult};
use crate::item::{self, ItemConfig, MenuIndex};
use crate::pricing::{self, Discount, ExtraFee, LineInput, OrderOpts, Rounding, Tax};
use crate::{audit, settings};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

pub const TYPES: &[&str] = &["dine_in", "takeaway", "delivery"];

// ------------------------------------------------------------------ helpers

pub fn next_seq(conn: &Connection, name: &str) -> ApiResult<i64> {
    Ok(conn.query_row(
        "UPDATE sequences SET next_value = next_value + 1 WHERE name = ?1 RETURNING next_value - 1",
        [name],
        |r| r.get(0),
    )?)
}

fn now_minutes(conn: &Connection) -> ApiResult<i64> {
    Ok(conn.query_row(
        "SELECT CAST(strftime('%H','now','localtime') AS INTEGER) * 60 + CAST(strftime('%M','now','localtime') AS INTEGER)",
        [],
        |r| r.get(0),
    )?)
}

/// Same rule as `isHappyHour` in menu.ts, evaluated on the Host's clock.
pub fn happy_hour_now(conn: &Connection) -> ApiResult<bool> {
    let s = settings::get_all(conn)?;
    if !s["happy_hour_enabled"].as_bool().unwrap_or(false) {
        return Ok(false);
    }
    let (start, end) = (s["happy_hour_start_min"].as_i64().unwrap_or(0), s["happy_hour_end_min"].as_i64().unwrap_or(0));
    let m = now_minutes(conn)?;
    Ok(if start == end { false } else if start < end { m >= start && m < end } else { m >= start || m < end })
}

/// Who approved a sensitive action: the user themself if they hold `perm`, otherwise a
/// manager PIN typed on the same screen. Every override is audited.
pub fn approve(conn: &Connection, me: &SessionUser, perm: &str, approver_pin: Option<&str>, action: &str, entity_id: i64) -> ApiResult<i64> {
    if me.permissions.iter().any(|p| p == perm) {
        return Ok(me.id);
    }
    let pin = approver_pin.filter(|p| !p.is_empty()).ok_or_else(|| ApiError::new("approval_required", perm.to_string()))?;
    let approver = auth::authorize_override(conn, pin, perm)?;
    audit::log(conn, Some(approver.id), "auth.override", Some("order"), Some(&entity_id.to_string()),
        Some(json!({ "permission": perm, "action": action, "requested_by": me.name })))?;
    Ok(approver.id)
}

#[allow(dead_code)]
struct OrderRow {
    id: i64,
    status: String,
    paid_cents: i64,
    total_cents: i64,
    payment_status: String,
}

fn load_order(conn: &Connection, id: i64) -> ApiResult<OrderRow> {
    conn.query_row(
        "SELECT id, status, paid_cents, total_cents, payment_status FROM orders WHERE id = ?1",
        [id],
        |r| Ok(OrderRow { id: r.get(0)?, status: r.get(1)?, paid_cents: r.get(2)?, total_cents: r.get(3)?, payment_status: r.get(4)? }),
    )
    .optional()?
    .ok_or_else(|| ApiError::not_found("order"))
}

fn ensure_editable(o: &OrderRow) -> ApiResult<()> {
    match o.status.as_str() {
        "closed" => Err(ApiError::new("order_closed", "reopen the order first")),
        "voided" => Err(ApiError::new("order_voided", "order is voided")),
        _ if o.payment_status == "refunded" => Err(ApiError::new("order_closed", "order was refunded")),
        _ => Ok(()),
    }
}

fn touch(conn: &Connection, id: i64) -> ApiResult<()> {
    conn.execute("UPDATE orders SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1", [id])?;
    Ok(())
}

fn tax_of(conn: &Connection, id: i64) -> ApiResult<Tax> {
    conn.query_row("SELECT id, rate_bp, price_include FROM taxes WHERE id = ?1", [id], |r| {
        Ok(Tax { id: r.get(0)?, rate_bp: r.get(1)?, price_include: r.get(2)? })
    })
    .optional()?
    .ok_or_else(|| ApiError::invalid("unknown tax"))
}

fn to_discount(kind: Option<String>, value: Option<i64>) -> Option<Discount> {
    match (kind.as_deref(), value) {
        (Some("percent"), Some(bp)) => Some(Discount::Percent { bp }),
        (Some("fixed"), Some(cents)) => Some(Discount::Fixed { cents }),
        (Some("comp"), _) => Some(Discount::Percent { bp: 10000 }),
        _ => None,
    }
}

/// Recompute every stored total for an order from its (non-voided) lines.
pub fn recompute(conn: &Connection, order_id: i64) -> ApiResult<()> {
    let s = settings::get_all(conn)?;
    let rounding = if s["tax_rounding"] == "global" { Rounding::Global } else { Rounding::PerLine };
    let (dk, dv, fee, fee_tax): (Option<String>, Option<i64>, i64, Option<i64>) = conn.query_row(
        "SELECT discount_kind, discount_value, delivery_fee_cents, delivery_tax_id FROM orders WHERE id=?1",
        [order_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let rows: Vec<(i64, i64, i64, i64, i64, bool, Option<String>, Option<i64>)> = conn
        .prepare(
            "SELECT id, unit_cents, qty, tax_id, tax_rate_bp, price_include, discount_kind, discount_value
             FROM order_lines WHERE order_id=?1 AND voided=0 ORDER BY id",
        )?
        .query_map([order_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?
        .collect::<Result<_, _>>()?;
    let inputs: Vec<LineInput> = rows
        .iter()
        .map(|r| LineInput {
            unit_cents: r.1,
            qty: r.2,
            tax: Tax { id: r.3, rate_bp: r.4, price_include: r.5 },
            discount: to_discount(r.6.clone(), r.7),
        })
        .collect();
    let extra_fee = match (fee > 0, fee_tax) {
        (true, Some(t)) => Some(ExtraFee { cents: fee, tax: tax_of(conn, t)? }),
        _ => None,
    };
    let totals = pricing::compute_order(&inputs, &OrderOpts { rounding, order_discount: to_discount(dk, dv), extra_fee })
        .map_err(|e| ApiError::invalid(e.0))?;
    // Discounts given on lines (before the order discount), in each line's price basis.
    let line_discounts: i64 = inputs.iter().map(|l| pricing::compute_line(l).map(|t| t.discount).unwrap_or(0)).sum();
    for (row, lt) in rows.iter().zip(&totals.lines) {
        conn.execute(
            "UPDATE order_lines SET gross_cents=?2, discount_cents=?3, base_cents=?4, tax_cents=?5, total_cents=?6 WHERE id=?1",
            params![row.0, lt.gross, lt.discount, lt.base, lt.tax, lt.total],
        )?;
    }
    conn.execute(
        "UPDATE order_lines SET gross_cents=0, discount_cents=0, base_cents=0, tax_cents=0, total_cents=0 WHERE order_id=?1 AND voided=1",
        [order_id],
    )?;

    let (paid, refunds, paid_at): (i64, i64, Option<String>) = conn.query_row(
        "SELECT COALESCE(SUM(p.amount_cents),0), COUNT(CASE WHEN p.kind='refund' THEN 1 END), o.paid_at
         FROM orders o LEFT JOIN payments p ON p.order_id = o.id WHERE o.id=?1",
        [order_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let pay_status = if refunds > 0 && paid <= 0 {
        "refunded"
    } else if paid == 0 {
        if totals.total == 0 && paid_at.is_some() { "paid" } else { "unpaid" }
    } else if paid >= totals.total {
        "paid"
    } else {
        "partial"
    };
    conn.execute(
        "UPDATE orders SET subtotal_cents=?2, discount_cents=?3, base_cents=?4, tax_cents=?5, total_cents=?6,
           vat=?7, paid_cents=?8, payment_status=?9, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
        params![
            order_id,
            totals.subtotal + fee,
            line_discounts + totals.order_discount,
            totals.base,
            totals.tax,
            totals.total,
            serde_json::to_string(&totals.vat).unwrap(),
            paid,
            pay_status
        ],
    )?;
    auto_close(conn, order_id)?;
    Ok(())
}

/// Paid + served = closed.
fn auto_close(conn: &Connection, order_id: i64) -> ApiResult<()> {
    conn.execute(
        "UPDATE orders SET status='closed', closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id=?1 AND status='served' AND payment_status='paid'",
        [order_id],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ read

pub fn get(conn: &Connection, id: i64) -> ApiResult<Value> {
    let mut o = conn
        .query_row(
            "SELECT o.id, o.number, o.split_index, o.parent_id, o.type, o.table_id, t.name, o.customer_name, o.customer_phone,
                    o.customer_address, o.delivery_fee_cents, o.note, o.status, o.payment_status, o.held, o.hold_label,
                    o.discount_kind, o.discount_value, o.discount_reason, o.subtotal_cents, o.discount_cents, o.base_cents,
                    o.tax_cents, o.total_cents, o.paid_cents, o.vat, o.register, o.created_by, u.name, o.created_at,
                    o.sent_at, o.ready_at, o.served_at, o.paid_at, o.closed_at, o.voided_at, o.void_reason, o.refunded_at, o.refund_reason
             FROM orders o LEFT JOIN dining_tables t ON t.id = o.table_id LEFT JOIN users u ON u.id = o.created_by
             WHERE o.id = ?1",
            [id],
            |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "number": r.get::<_, i64>(1)?, "split_index": r.get::<_, i64>(2)?,
                    "parent_id": r.get::<_, Option<i64>>(3)?, "type": r.get::<_, String>(4)?, "table_id": r.get::<_, Option<i64>>(5)?,
                    "table_name": r.get::<_, Option<String>>(6)?, "customer_name": r.get::<_, String>(7)?,
                    "customer_phone": r.get::<_, String>(8)?, "customer_address": r.get::<_, String>(9)?,
                    "delivery_fee_cents": r.get::<_, i64>(10)?, "note": r.get::<_, String>(11)?, "status": r.get::<_, String>(12)?,
                    "payment_status": r.get::<_, String>(13)?, "held": r.get::<_, bool>(14)?, "hold_label": r.get::<_, String>(15)?,
                    "discount_kind": r.get::<_, Option<String>>(16)?, "discount_value": r.get::<_, Option<i64>>(17)?,
                    "discount_reason": r.get::<_, Option<String>>(18)?, "subtotal_cents": r.get::<_, i64>(19)?,
                    "discount_cents": r.get::<_, i64>(20)?, "base_cents": r.get::<_, i64>(21)?, "tax_cents": r.get::<_, i64>(22)?,
                    "total_cents": r.get::<_, i64>(23)?, "paid_cents": r.get::<_, i64>(24)?,
                    "vat": serde_json::from_str::<Value>(&r.get::<_, String>(25)?).unwrap_or(json!([])),
                    "register": r.get::<_, String>(26)?, "created_by": r.get::<_, i64>(27)?, "created_by_name": r.get::<_, Option<String>>(28)?,
                    "created_at": r.get::<_, String>(29)?, "sent_at": r.get::<_, Option<String>>(30)?, "ready_at": r.get::<_, Option<String>>(31)?,
                    "served_at": r.get::<_, Option<String>>(32)?, "paid_at": r.get::<_, Option<String>>(33)?,
                    "closed_at": r.get::<_, Option<String>>(34)?, "voided_at": r.get::<_, Option<String>>(35)?,
                    "void_reason": r.get::<_, Option<String>>(36)?, "refunded_at": r.get::<_, Option<String>>(37)?,
                    "refund_reason": r.get::<_, Option<String>>(38)?,
                }))
            },
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("order"))?;
    let lines = conn
        .prepare(
            "SELECT id, product_id, variant_id, config, names, details, qty, unit_cents, happy_hour, tax_id, tax_rate_bp,
                    price_include, discount_kind, discount_value, discount_reason, gross_cents, discount_cents, base_cents,
                    tax_cents, total_cents, note, station_id, kitchen_status, voided, void_reason
             FROM order_lines WHERE order_id = ?1 ORDER BY id",
        )?
        .query_map([id], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "product_id": r.get::<_, i64>(1)?, "variant_id": r.get::<_, Option<i64>>(2)?,
                "config": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                "names": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                "details": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
                "qty": r.get::<_, i64>(6)?, "unit_cents": r.get::<_, i64>(7)?, "happy_hour": r.get::<_, bool>(8)?,
                "tax_id": r.get::<_, i64>(9)?, "tax_rate_bp": r.get::<_, i64>(10)?, "price_include": r.get::<_, bool>(11)?,
                "discount_kind": r.get::<_, Option<String>>(12)?, "discount_value": r.get::<_, Option<i64>>(13)?,
                "discount_reason": r.get::<_, Option<String>>(14)?, "gross_cents": r.get::<_, i64>(15)?,
                "discount_cents": r.get::<_, i64>(16)?, "base_cents": r.get::<_, i64>(17)?, "tax_cents": r.get::<_, i64>(18)?,
                "total_cents": r.get::<_, i64>(19)?, "note": r.get::<_, String>(20)?, "station_id": r.get::<_, Option<i64>>(21)?,
                "kitchen_status": r.get::<_, String>(22)?, "voided": r.get::<_, bool>(23)?, "void_reason": r.get::<_, Option<String>>(24)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let payments = conn
        .prepare("SELECT id, kind, method, amount_cents, tendered_cents, change_cents, at FROM payments WHERE order_id=?1 ORDER BY id")?
        .query_map([id], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?, "method": r.get::<_, String>(2)?,
                      "amount_cents": r.get::<_, i64>(3)?, "tendered_cents": r.get::<_, Option<i64>>(4)?,
                      "change_cents": r.get::<_, Option<i64>>(5)?, "at": r.get::<_, String>(6)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    o["lines"] = json!(lines);
    o["payments"] = json!(payments);
    o["due_cents"] = json!(o["total_cents"].as_i64().unwrap_or(0) - o["paid_cents"].as_i64().unwrap_or(0));
    Ok(o)
}

#[derive(Deserialize, Default)]
pub struct ListIn {
    #[serde(default)]
    pub scope: String, // "active" (default), "held", "ready", "today", "recent"
    #[serde(default)]
    pub limit: Option<i64>,
}

pub fn list(conn: &Connection, f: ListIn) -> ApiResult<Value> {
    let filter = match f.scope.as_str() {
        "held" => "held = 1 AND status NOT IN ('closed','voided')",
        "ready" => "status = 'ready'",
        "today" => "date(created_at,'localtime') = date('now','localtime')",
        "recent" => "1=1",
        _ => "status NOT IN ('closed','voided') AND payment_status <> 'refunded'",
    };
    let rows = conn
        .prepare(&format!(
            "SELECT o.id, o.number, o.split_index, o.type, t.name, o.customer_name, o.status, o.payment_status, o.held,
                    o.hold_label, o.total_cents, o.paid_cents, o.created_at,
                    (SELECT COUNT(*) FROM order_lines l WHERE l.order_id=o.id AND l.voided=0)
             FROM orders o LEFT JOIN dining_tables t ON t.id=o.table_id
             WHERE {filter} ORDER BY o.id DESC LIMIT ?1"
        ))?
        .query_map([f.limit.unwrap_or(200).clamp(1, 1000)], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "number": r.get::<_, i64>(1)?, "split_index": r.get::<_, i64>(2)?,
                      "type": r.get::<_, String>(3)?, "table_name": r.get::<_, Option<String>>(4)?,
                      "customer_name": r.get::<_, String>(5)?, "status": r.get::<_, String>(6)?,
                      "payment_status": r.get::<_, String>(7)?, "held": r.get::<_, bool>(8)?, "hold_label": r.get::<_, String>(9)?,
                      "total_cents": r.get::<_, i64>(10)?, "paid_cents": r.get::<_, i64>(11)?, "created_at": r.get::<_, String>(12)?,
                      "line_count": r.get::<_, i64>(13)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

// ------------------------------------------------------------------ write

#[derive(Deserialize)]
pub struct NewLine {
    pub config: ItemConfig,
    pub qty: i64,
    #[serde(default)]
    pub note: String,
}

#[derive(Deserialize)]
pub struct SubmitIn {
    pub order_id: Option<i64>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub table_id: Option<i64>,
    #[serde(default)]
    pub customer_name: Option<String>,
    #[serde(default)]
    pub customer_phone: Option<String>,
    #[serde(default)]
    pub customer_address: Option<String>,
    pub delivery_fee_cents: Option<i64>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub lines: Vec<NewLine>,
    #[serde(default)]
    pub send: bool,
    #[serde(default)]
    pub hold: bool,
    #[serde(default)]
    pub hold_label: Option<String>,
    #[serde(default = "main_register")]
    pub register: String,
}
fn main_register() -> String {
    "main".into()
}

fn check_text(s: &str, max: usize, what: &str) -> ApiResult<()> {
    if s.chars().count() > max {
        return Err(ApiError::invalid(format!("{what} too long")));
    }
    Ok(())
}

/// Create an order or append lines to one; optionally send to kitchen or park it.
pub fn submit(conn: &Connection, me: &SessionUser, s: SubmitIn) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let cfg = settings::get_all(&tx)?;
    let order_id = match s.order_id {
        Some(id) => {
            let o = load_order(&tx, id)?;
            ensure_editable(&o)?;
            id
        }
        None => {
            let kind = s.kind.clone().unwrap_or_else(|| "takeaway".into());
            if !TYPES.contains(&kind.as_str()) {
                return Err(ApiError::invalid("unknown order type"));
            }
            if cfg[format!("order_type_{kind}").as_str()] == json!(false) {
                return Err(ApiError::invalid("this order type is disabled"));
            }
            let table = if kind == "dine_in" { s.table_id } else { None };
            if let Some(t) = table {
                if tx.query_row("SELECT 1 FROM dining_tables WHERE id=?1 AND active=1", [t], |_| Ok(())).optional()?.is_none() {
                    return Err(ApiError::invalid("unknown table"));
                }
            }
            let name = s.customer_name.clone().unwrap_or_default();
            let phone = s.customer_phone.clone().unwrap_or_default();
            let address = s.customer_address.clone().unwrap_or_default();
            check_text(&name, 80, "name")?;
            check_text(&phone, 30, "phone")?;
            check_text(&address, 300, "address")?;
            if kind == "delivery" && (phone.trim().is_empty() || address.trim().is_empty()) {
                return Err(ApiError::new("delivery_details", "delivery needs a phone and an address"));
            }
            let fee = if kind == "delivery" {
                s.delivery_fee_cents.unwrap_or(cfg["delivery_fee_cents"].as_i64().unwrap_or(0))
            } else {
                0
            };
            if !(0..=100_000).contains(&fee) {
                return Err(ApiError::invalid("delivery fee out of range"));
            }
            let fee_tax = cfg["default_tax_id"].as_i64().unwrap_or(1);
            let number = next_seq(&tx, "order_number")?;
            let note = s.note.clone().unwrap_or_default();
            check_text(&note, 500, "note")?;
            tx.execute(
                "INSERT INTO orders (number, type, table_id, customer_name, customer_phone, customer_address,
                   delivery_fee_cents, delivery_tax_id, note, register, created_by)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![number, kind, table, name.trim(), phone.trim(), address.trim(), fee, fee_tax, note.trim(), s.register, me.id],
            )?;
            tx.last_insert_rowid()
        }
    };

    if !s.lines.is_empty() {
        let ix = MenuIndex::load(&tx)?;
        let happy = happy_hour_now(&tx)?;
        for l in &s.lines {
            if !(1..=999).contains(&l.qty) {
                return Err(ApiError::invalid("quantity must be 1-999"));
            }
            check_text(&l.note, 200, "note")?;
            item::validate(&ix, &l.config)?;
            let unit = item::unit_price(&ix, &l.config, happy)?;
            let p = ix.product(l.config.product_id)?;
            let tax = tax_of(&tx, p.tax_id)?;
            tx.execute(
                "INSERT INTO order_lines (order_id, product_id, variant_id, config, names, details, qty, unit_cents, happy_hour,
                   tax_id, tax_rate_bp, price_include, note, station_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    order_id, p.id, l.config.variant_id, serde_json::to_string(&l.config).unwrap(), p.names.to_string(),
                    item::describe_all(&ix, &l.config).to_string(), l.qty, unit, happy, tax.id, tax.rate_bp, tax.price_include,
                    l.note.trim(), p.station_id
                ],
            )?;
        }
    }

    if s.send {
        let to_send: Vec<i64> = tx
            .prepare("SELECT id FROM order_lines WHERE order_id=?1 AND kitchen_status='new' AND voided=0 ORDER BY id")?
            .query_map([order_id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let first_send: bool = tx.query_row("SELECT sent_at IS NULL FROM orders WHERE id=?1", [order_id], |r| r.get(0))?;
        let n = tx.execute(
            "UPDATE order_lines SET kitchen_status='sent', sent_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE order_id=?1 AND kitchen_status='new' AND voided=0",
            [order_id],
        )?;
        if n > 0 {
            tx.execute(
                "UPDATE orders SET status = CASE WHEN status IN ('open','ready','served') THEN 'sent' ELSE status END,
                   sent_at = COALESCE(sent_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')), held = 0 WHERE id=?1",
                [order_id],
            )?;
            // Kitchen tickets are queued in this same transaction: they can't be lost.
            let mode = if first_send { crate::printing::TicketMode::New } else { crate::printing::TicketMode::Add };
            crate::printing::enqueue_kitchen(&tx, order_id, &to_send, mode, Some(me.id))?;
        }
    }
    if s.hold {
        let label = s.hold_label.clone().unwrap_or_default();
        check_text(&label, 40, "label")?;
        tx.execute("UPDATE orders SET held=1, hold_label=?2 WHERE id=?1", params![order_id, label.trim()])?;
    } else if s.order_id.is_some() || s.send {
        tx.execute("UPDATE orders SET held=0 WHERE id=?1", [order_id])?;
    }
    recompute(&tx, order_id)?;
    tx.commit()?;
    Ok(order_id)
}

/// Change qty or note of a line not yet sent to the kitchen.
pub fn update_line(conn: &Connection, line_id: i64, qty: Option<i64>, note: Option<String>) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let (order_id, status, voided): (i64, String, bool) = tx
        .query_row("SELECT order_id, kitchen_status, voided FROM order_lines WHERE id=?1", [line_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
        .ok_or_else(|| ApiError::not_found("line"))?;
    ensure_editable(&load_order(&tx, order_id)?)?;
    if voided {
        return Err(ApiError::invalid("line is voided"));
    }
    if let Some(q) = qty {
        if status != "new" {
            return Err(ApiError::new("line_sent", "already sent to the kitchen; void it instead"));
        }
        if !(1..=999).contains(&q) {
            return Err(ApiError::invalid("quantity must be 1-999"));
        }
        tx.execute("UPDATE order_lines SET qty=?2 WHERE id=?1", params![line_id, q])?;
    }
    if let Some(n) = note {
        check_text(&n, 200, "note")?;
        tx.execute("UPDATE order_lines SET note=?2 WHERE id=?1", params![line_id, n.trim()])?;
    }
    recompute(&tx, order_id)?;
    tx.commit()?;
    Ok(order_id)
}

/// Remove a line. Unsent lines are deleted; sent lines are voided and need `pos.void` approval.
pub fn remove_line(conn: &Connection, me: &SessionUser, line_id: i64, reason: Option<String>, approver_pin: Option<&str>) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let (order_id, status, voided, qty, names): (i64, String, bool, i64, String) = tx
        .query_row("SELECT order_id, kitchen_status, voided, qty, names FROM order_lines WHERE id=?1", [line_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .optional()?
        .ok_or_else(|| ApiError::not_found("line"))?;
    ensure_editable(&load_order(&tx, order_id)?)?;
    if voided {
        return Err(ApiError::invalid("line already voided"));
    }
    if status == "new" {
        tx.execute("DELETE FROM order_lines WHERE id=?1", [line_id])?;
    } else {
        let reason = reason.unwrap_or_default();
        if reason.trim().is_empty() {
            return Err(ApiError::new("reason_required", "a reason is required"));
        }
        let by = approve(&tx, me, "pos.void", approver_pin, "line.void", order_id)?;
        tx.execute(
            "UPDATE order_lines SET voided=1, void_reason=?2, voided_by=?3 WHERE id=?1",
            params![line_id, reason.trim(), by],
        )?;
        crate::printing::enqueue_kitchen(&tx, order_id, &[line_id], crate::printing::TicketMode::Void, Some(me.id))?;
        audit::log(&tx, Some(me.id), "order.line_voided", Some("order"), Some(&order_id.to_string()),
            Some(json!({"line_id": line_id, "qty": qty, "names": serde_json::from_str::<Value>(&names).unwrap_or(Value::Null), "reason": reason.trim(), "approved_by": by})))?;
    }
    recompute(&tx, order_id)?;
    tx.commit()?;
    Ok(order_id)
}

#[derive(Deserialize)]
pub struct DiscountIn {
    pub kind: String, // percent | fixed | comp (comp: lines only)
    #[serde(default)]
    pub value: i64,
    #[serde(default)]
    pub reason: String,
}

/// Discounts are checked against `discount_pin_threshold_bp`: above it (or any comp) the
/// cashier needs `pos.discount` or a manager PIN.
fn discount_needs_approval(conn: &Connection, d: &DiscountIn, gross: i64) -> ApiResult<bool> {
    let threshold = settings::get(conn, "discount_pin_threshold_bp")?.as_i64().unwrap_or(0);
    let bp = match d.kind.as_str() {
        "comp" => 10000,
        "percent" => d.value,
        "fixed" => if gross > 0 { (d.value * 10000 + gross - 1) / gross } else { 10000 },
        _ => return Err(ApiError::invalid("discount kind must be percent, fixed or comp")),
    };
    Ok(d.kind == "comp" || bp > threshold)
}

fn check_discount(d: &DiscountIn, allow_comp: bool) -> ApiResult<()> {
    match d.kind.as_str() {
        "percent" if (1..=10000).contains(&d.value) => {}
        "fixed" if d.value > 0 && d.value <= 10_000_000 => {}
        "comp" if allow_comp => {}
        _ => return Err(ApiError::invalid("bad discount")),
    }
    if d.reason.trim().is_empty() && d.kind == "comp" {
        return Err(ApiError::new("reason_required", "a comp needs a reason"));
    }
    check_text(&d.reason, 200, "reason")
}

pub fn set_line_discount(conn: &Connection, me: &SessionUser, line_id: i64, d: Option<DiscountIn>, approver_pin: Option<&str>) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let (order_id, gross, voided): (i64, i64, bool) = tx
        .query_row("SELECT order_id, unit_cents*qty, voided FROM order_lines WHERE id=?1", [line_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
        .ok_or_else(|| ApiError::not_found("line"))?;
    let o = load_order(&tx, order_id)?;
    ensure_editable(&o)?;
    if voided {
        return Err(ApiError::invalid("line is voided"));
    }
    match d {
        None => {
            tx.execute("UPDATE order_lines SET discount_kind=NULL, discount_value=NULL, discount_reason=NULL, discount_by=NULL WHERE id=?1", [line_id])?;
            audit::log(&tx, Some(me.id), "order.discount_removed", Some("order"), Some(&order_id.to_string()), Some(json!({"line_id": line_id})))?;
        }
        Some(d) => {
            check_discount(&d, true)?;
            let by = if discount_needs_approval(&tx, &d, gross)? {
                approve(&tx, me, "pos.discount", approver_pin, "line.discount", order_id)?
            } else {
                me.id
            };
            tx.execute(
                "UPDATE order_lines SET discount_kind=?2, discount_value=?3, discount_reason=?4, discount_by=?5 WHERE id=?1",
                params![line_id, d.kind, if d.kind == "comp" { None } else { Some(d.value) }, d.reason.trim(), by],
            )?;
            audit::log(&tx, Some(me.id), if d.kind == "comp" { "order.comp" } else { "order.discount" }, Some("order"), Some(&order_id.to_string()),
                Some(json!({"line_id": line_id, "kind": d.kind, "value": d.value, "reason": d.reason.trim(), "approved_by": by})))?;
        }
    }
    recompute(&tx, order_id)?;
    tx.commit()?;
    Ok(order_id)
}

pub fn set_order_discount(conn: &Connection, me: &SessionUser, order_id: i64, d: Option<DiscountIn>, approver_pin: Option<&str>) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    ensure_editable(&o)?;
    match d {
        None => {
            tx.execute("UPDATE orders SET discount_kind=NULL, discount_value=NULL, discount_reason=NULL, discount_by=NULL WHERE id=?1", [order_id])?;
            audit::log(&tx, Some(me.id), "order.discount_removed", Some("order"), Some(&order_id.to_string()), None)?;
        }
        Some(d) => {
            check_discount(&d, false)?;
            let gross: i64 = tx.query_row(
                "SELECT COALESCE(SUM(total_cents),0) FROM order_lines WHERE order_id=?1 AND voided=0",
                [order_id],
                |r| r.get(0),
            )?;
            let by = if discount_needs_approval(&tx, &d, gross)? {
                approve(&tx, me, "pos.discount", approver_pin, "order.discount", order_id)?
            } else {
                me.id
            };
            tx.execute(
                "UPDATE orders SET discount_kind=?2, discount_value=?3, discount_reason=?4, discount_by=?5 WHERE id=?1",
                params![order_id, d.kind, d.value, d.reason.trim(), by],
            )?;
            audit::log(&tx, Some(me.id), "order.discount", Some("order"), Some(&order_id.to_string()),
                Some(json!({"kind": d.kind, "value": d.value, "reason": d.reason.trim(), "approved_by": by})))?;
        }
    }
    recompute(&tx, order_id)?;
    tx.commit()?;
    Ok(order_id)
}

#[derive(Deserialize)]
pub struct PayIn {
    pub method: String,
    pub amount_cents: i64,
    pub tendered_cents: Option<i64>,
}

pub fn open_session_id(conn: &Connection, register: &str) -> ApiResult<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM cash_sessions WHERE register=?1 AND closed_at IS NULL", [register], |r| r.get(0))
        .optional()?)
}

/// Record payments. Card amounts can't exceed what's due; cash can be over-tendered and
/// the change is stored. Needs an open cash session on that register.
pub fn pay(conn: &Connection, me: &SessionUser, order_id: i64, pays: Vec<PayIn>, register: &str) -> ApiResult<Value> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    if o.status == "voided" || o.payment_status == "refunded" {
        return Err(ApiError::new("order_voided", "cannot pay this order"));
    }
    let session = open_session_id(&tx, register)?.ok_or_else(|| ApiError::new("no_cash_session", "open the till first"))?;
    let mut due = o.total_cents - o.paid_cents;
    if due < 0 {
        return Err(ApiError::invalid("nothing due"));
    }
    let mut change_total = 0;
    for p in &pays {
        if due <= 0 {
            return Err(ApiError::new("overpaid", "the order is already fully paid"));
        }
        match p.method.as_str() {
            "card" => {
                if p.amount_cents <= 0 || p.amount_cents > due {
                    return Err(ApiError::new("overpaid", "card amount must be between 0 and what is due"));
                }
                tx.execute(
                    "INSERT INTO payments (order_id, kind, method, amount_cents, cash_session_id, user_id) VALUES (?1,'payment','card',?2,?3,?4)",
                    params![order_id, p.amount_cents, session, me.id],
                )?;
                due -= p.amount_cents;
            }
            "cash" => {
                // amount_cents = what this cash payment covers; tendered = notes handed over.
                let tendered = p.tendered_cents.unwrap_or(p.amount_cents);
                if p.amount_cents <= 0 || tendered < p.amount_cents || tendered > 10_000_000 {
                    return Err(ApiError::invalid("cash amount out of range"));
                }
                let applied = p.amount_cents.min(due);
                let change = tendered - applied;
                tx.execute(
                    "INSERT INTO payments (order_id, kind, method, amount_cents, tendered_cents, change_cents, cash_session_id, user_id)
                     VALUES (?1,'payment','cash',?2,?3,?4,?5,?6)",
                    params![order_id, applied, tendered, change, session, me.id],
                )?;
                due -= applied;
                change_total += change;
            }
            _ => return Err(ApiError::invalid("payment method must be cash or card")),
        }
    }
    if due == 0 {
        tx.execute("UPDATE orders SET paid_at = COALESCE(paid_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')), held=0 WHERE id=?1", [order_id])?;
    }
    recompute(&tx, order_id)?;
    touch(&tx, order_id)?;
    tx.commit()?;
    Ok(json!({ "order_id": order_id, "change_cents": change_total, "due_cents": due }))
}

/// Kitchen / service flow. Paid + served closes the order.
pub fn set_status(conn: &Connection, order_id: i64, status: &str) -> ApiResult<()> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    ensure_editable(&o)?;
    let col = match status {
        "preparing" => None,
        "ready" => Some("ready_at"),
        "served" => Some("served_at"),
        _ => return Err(ApiError::invalid("status must be preparing, ready or served")),
    };
    tx.execute("UPDATE orders SET status=?2, held=0 WHERE id=?1", params![order_id, status])?;
    if let Some(c) = col {
        tx.execute(&format!("UPDATE orders SET {c}=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1"), [order_id])?;
    }
    if status == "ready" || status == "served" {
        tx.execute("UPDATE order_lines SET kitchen_status='done' WHERE order_id=?1 AND kitchen_status='sent'", [order_id])?;
    }
    auto_close(&tx, order_id)?;
    touch(&tx, order_id)?;
    tx.commit()?;
    Ok(())
}

pub fn void(conn: &Connection, me: &SessionUser, order_id: i64, reason: &str, approver_pin: Option<&str>) -> ApiResult<()> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    ensure_editable(&o)?;
    if o.paid_cents != 0 {
        return Err(ApiError::new("refund_first", "this order has payments; refund it instead"));
    }
    if reason.trim().is_empty() {
        return Err(ApiError::new("reason_required", "a reason is required"));
    }
    check_text(reason, 200, "reason")?;
    let by = approve(&tx, me, "pos.void", approver_pin, "order.void", order_id)?;
    tx.execute(
        "UPDATE orders SET status='voided', held=0, voided_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'), voided_by=?2, void_reason=?3 WHERE id=?1",
        params![order_id, by, reason.trim()],
    )?;
    audit::log(&tx, Some(me.id), "order.voided", Some("order"), Some(&order_id.to_string()),
        Some(json!({"reason": reason.trim(), "total_cents": o.total_cents, "approved_by": by})))?;
    tx.commit()?;
    Ok(())
}

/// Full refund: every payment is mirrored by a refund of the same method (cash from this register's drawer).
pub fn refund(conn: &Connection, me: &SessionUser, order_id: i64, reason: &str, approver_pin: Option<&str>, register: &str) -> ApiResult<Value> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    if o.paid_cents <= 0 {
        return Err(ApiError::invalid("nothing to refund"));
    }
    if reason.trim().is_empty() {
        return Err(ApiError::new("reason_required", "a reason is required"));
    }
    check_text(reason, 200, "reason")?;
    let by = approve(&tx, me, "pos.refund", approver_pin, "order.refund", order_id)?;
    let session = open_session_id(&tx, register)?;
    let by_method: Vec<(String, i64)> = tx
        .prepare("SELECT method, SUM(amount_cents) FROM payments WHERE order_id=?1 GROUP BY method HAVING SUM(amount_cents) > 0")?
        .query_map([order_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    if by_method.iter().any(|(m, _)| m == "cash") && session.is_none() {
        return Err(ApiError::new("no_cash_session", "open the till to refund cash"));
    }
    for (method, amount) in &by_method {
        tx.execute(
            "INSERT INTO payments (order_id, kind, method, amount_cents, cash_session_id, user_id) VALUES (?1,'refund',?2,?3,?4,?5)",
            params![order_id, method, -amount, session, me.id],
        )?;
    }
    tx.execute(
        "UPDATE orders SET refunded_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'), refunded_by=?2, refund_reason=?3,
           status = CASE WHEN status='closed' THEN 'closed' ELSE status END WHERE id=?1",
        params![order_id, by, reason.trim()],
    )?;
    recompute(&tx, order_id)?;
    audit::log(&tx, Some(me.id), "order.refunded", Some("order"), Some(&order_id.to_string()),
        Some(json!({"reason": reason.trim(), "amount_cents": o.paid_cents, "methods": by_method, "approved_by": by})))?;
    tx.commit()?;
    Ok(json!({ "refunded_cents": o.paid_cents }))
}

pub fn reopen(conn: &Connection, me: &SessionUser, order_id: i64, approver_pin: Option<&str>) -> ApiResult<()> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    if o.status != "closed" && o.payment_status != "paid" {
        return Err(ApiError::invalid("only paid orders can be reopened"));
    }
    if o.payment_status == "refunded" {
        return Err(ApiError::invalid("order was refunded"));
    }
    let by = approve(&tx, me, "pos.reopen", approver_pin, "order.reopen", order_id)?;
    tx.execute("UPDATE orders SET status='open', closed_at=NULL WHERE id=?1", [order_id])?;
    audit::log(&tx, Some(me.id), "order.reopened", Some("order"), Some(&order_id.to_string()), Some(json!({"approved_by": by, "total_cents": o.total_cents})))?;
    tx.commit()?;
    Ok(())
}

#[derive(Deserialize)]
pub struct SplitMove {
    pub line_id: i64,
    pub qty: i64,
}

/// Move some lines (or part of a line's quantity) to a new check with the same order number.
pub fn split(conn: &Connection, me: &SessionUser, order_id: i64, moves: Vec<SplitMove>) -> ApiResult<i64> {
    let tx = atomic(conn)?;
    let o = load_order(&tx, order_id)?;
    ensure_editable(&o)?;
    if o.paid_cents != 0 {
        return Err(ApiError::invalid("split before taking any payment"));
    }
    if moves.is_empty() {
        return Err(ApiError::invalid("choose items to move"));
    }
    let (number, root): (i64, i64) = tx.query_row("SELECT number, COALESCE(parent_id, id) FROM orders WHERE id=?1", [order_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let next_index: i64 = tx.query_row("SELECT MAX(split_index)+1 FROM orders WHERE number=?1", [number], |r| r.get(0))?;
    tx.execute(
        "INSERT INTO orders (number, split_index, parent_id, type, table_id, customer_name, customer_phone, customer_address,
           delivery_fee_cents, delivery_tax_id, note, status, register, created_by, sent_at)
         SELECT number, ?2, ?3, type, table_id, customer_name, customer_phone, customer_address, 0, delivery_tax_id, note,
           CASE WHEN status='open' THEN 'open' ELSE status END, register, ?4, sent_at FROM orders WHERE id=?1",
        params![order_id, next_index, root, me.id],
    )?;
    let new_id = tx.last_insert_rowid();
    let mut remaining: i64 = tx.query_row("SELECT COUNT(*) FROM order_lines WHERE order_id=?1 AND voided=0", [order_id], |r| r.get(0))?;
    for m in &moves {
        let (line_order, qty, voided): (i64, i64, bool) = tx
            .query_row("SELECT order_id, qty, voided FROM order_lines WHERE id=?1", [m.line_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?
            .ok_or_else(|| ApiError::not_found("line"))?;
        if line_order != order_id || voided || m.qty < 1 || m.qty > qty {
            return Err(ApiError::invalid("bad split line"));
        }
        if m.qty == qty {
            tx.execute("UPDATE order_lines SET order_id=?2 WHERE id=?1", params![m.line_id, new_id])?;
            remaining -= 1;
        } else {
            tx.execute("UPDATE order_lines SET qty=qty-?2 WHERE id=?1", params![m.line_id, m.qty])?;
            tx.execute(
                "INSERT INTO order_lines (order_id, product_id, variant_id, config, names, details, qty, unit_cents, happy_hour, tax_id,
                   tax_rate_bp, price_include, discount_kind, discount_value, discount_reason, discount_by, note, station_id,
                   kitchen_status, sent_at)
                 SELECT ?2, product_id, variant_id, config, names, details, ?3, unit_cents, happy_hour, tax_id, tax_rate_bp, price_include,
                   CASE WHEN discount_kind='fixed' THEN NULL ELSE discount_kind END,
                   CASE WHEN discount_kind='fixed' THEN NULL ELSE discount_value END,
                   CASE WHEN discount_kind='fixed' THEN NULL ELSE discount_reason END,
                   CASE WHEN discount_kind='fixed' THEN NULL ELSE discount_by END, note, station_id, kitchen_status, sent_at
                 FROM order_lines WHERE id=?1",
                params![m.line_id, new_id, m.qty],
            )?;
            // A fixed discount stays on the original line only, so it is never counted twice.
        }
    }
    if remaining <= 0 {
        return Err(ApiError::invalid("leave at least one item on the original check"));
    }
    recompute(&tx, order_id)?;
    recompute(&tx, new_id)?;
    audit::log(&tx, Some(me.id), "order.split", Some("order"), Some(&order_id.to_string()), Some(json!({"new_order_id": new_id})))?;
    tx.commit()?;
    Ok(new_id)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::db::open_in_memory;

    pub fn user(c: &Connection, role: &str) -> SessionUser {
        let pin = match role { "owner" => "1111", "manager" => "2222", "cashier" => "3333", _ => "4444" };
        c.execute("INSERT INTO users (name, role, pin_hash) VALUES (?1, ?2, ?3)", params![role, role, auth::hash_pin(pin).unwrap()]).unwrap();
        let id: i64 = c.query_row("SELECT id FROM users WHERE name=?1", [role], |r| r.get(0)).unwrap();
        auth::load_session_user(c, id).unwrap()
    }
    pub fn demo() -> (Connection, SessionUser, SessionUser) {
        let c = open_in_memory().unwrap();
        let owner = user(&c, "owner");
        let cashier = user(&c, "cashier");
        user(&c, "manager");
        crate::seed::seed_demo_menu(&c, owner.id, 1).unwrap();
        c.execute("INSERT INTO cash_sessions (register, opened_by, float_cents) VALUES ('main', ?1, 50000)", [owner.id]).unwrap();
        (c, owner, cashier)
    }
    fn pid(c: &Connection, fr: &str) -> i64 {
        c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')=?1", [fr], |r| r.get(0)).unwrap()
    }
    pub fn simple_line(c: &Connection, fr: &str, qty: i64) -> NewLine {
        NewLine { config: ItemConfig { product_id: pid(c, fr), variant_id: None, modifiers: vec![], combo: None }, qty, note: String::new() }
    }
    pub fn new_order(c: &Connection, me: &SessionUser, lines: Vec<NewLine>, send: bool) -> i64 {
        submit(c, me, SubmitIn { order_id: None, kind: Some("takeaway".into()), table_id: None, customer_name: Some("Ali".into()),
            customer_phone: None, customer_address: None, delivery_fee_cents: None, note: None, lines, send, hold: false,
            hold_label: None, register: "main".into() }).unwrap()
    }
    fn total(c: &Connection, id: i64) -> (i64, i64, String, String) {
        c.query_row("SELECT total_cents, paid_cents, payment_status, status FROM orders WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
    }

    #[test]
    fn numbers_never_reset_and_totals_are_server_side() {
        let (c, _, cashier) = demo();
        let a = new_order(&c, &cashier, vec![simple_line(&c, "Coca-Cola 33cl", 2), simple_line(&c, "Tiramisu", 1)], false);
        let b = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], false);
        let (na, nb): (i64, i64) = (
            c.query_row("SELECT number FROM orders WHERE id=?1", [a], |r| r.get(0)).unwrap(),
            c.query_row("SELECT number FROM orders WHERE id=?1", [b], |r| r.get(0)).unwrap(),
        );
        assert_eq!(nb, na + 1);
        assert_eq!(total(&c, a).0, 2 * 1200 + 2500);
        let o = get(&c, a).unwrap();
        assert_eq!(o["vat"][0]["tax"], json!(445)); // 4900 TTC at 10%: base 4455, VAT 445
        assert_eq!(o["lines"][0]["details"]["fr"], json!([]));
    }

    #[test]
    fn invalid_config_rejected_and_nothing_written() {
        let (c, _, cashier) = demo();
        let mut bad = simple_line(&c, "Classic Burger", 1); // missing size + doneness
        bad.config.variant_id = None;
        let r = submit(&c, &cashier, SubmitIn { order_id: None, kind: Some("takeaway".into()), table_id: None, customer_name: None,
            customer_phone: None, customer_address: None, delivery_fee_cents: None, note: None, lines: vec![bad], send: true,
            hold: false, hold_label: None, register: "main".into() });
        assert!(r.is_err());
        let n: i64 = c.query_row("SELECT COUNT(*) FROM orders", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "rolled back, order number not consumed either");
        let next: i64 = c.query_row("SELECT next_value FROM sequences WHERE name='order_number'", [], |r| r.get(0)).unwrap();
        assert_eq!(next, 1);
    }

    #[test]
    fn delivery_needs_details_and_adds_fee() {
        let (c, _, cashier) = demo();
        let mk = |phone: Option<&str>| SubmitIn { order_id: None, kind: Some("delivery".into()), table_id: None, customer_name: Some("Sara".into()),
            customer_phone: phone.map(String::from), customer_address: Some("12 rue X, Casa".into()), delivery_fee_cents: None, note: None,
            lines: vec![simple_line(&c, "Cookie", 1)], send: false, hold: false, hold_label: None, register: "main".into() };
        assert_eq!(submit(&c, &cashier, mk(None)).unwrap_err().code, "delivery_details");
        let id = submit(&c, &cashier, mk(Some("0600000000"))).unwrap();
        assert_eq!(total(&c, id).0, 1200 + 1000);
    }

    #[test]
    fn cashier_discount_needs_manager_pin() {
        let (c, _, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 2)], false);
        let line: i64 = c.query_row("SELECT id FROM order_lines WHERE order_id=?1", [id], |r| r.get(0)).unwrap();
        let d = || Some(DiscountIn { kind: "percent".into(), value: 1000, reason: "habitué".into() });
        assert_eq!(set_line_discount(&c, &cashier, line, d(), None).unwrap_err().code, "approval_required");
        assert_eq!(set_line_discount(&c, &cashier, line, d(), Some("3333")).unwrap_err().code, "wrong_pin");
        set_line_discount(&c, &cashier, line, d(), Some("2222")).unwrap();
        assert_eq!(total(&c, id).0, 4500);
        // Comp needs a reason.
        let comp = DiscountIn { kind: "comp".into(), value: 0, reason: "".into() };
        assert_eq!(set_line_discount(&c, &cashier, line, Some(comp), Some("2222")).unwrap_err().code, "reason_required");
        let n: i64 = c.query_row("SELECT COUNT(*) FROM audit_log WHERE action='auth.override'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn threshold_lets_cashier_give_small_discounts() {
        let (c, _, cashier) = demo();
        let mut m = serde_json::Map::new();
        m.insert("discount_pin_threshold_bp".into(), json!(1000));
        settings::set_many(&c, &m).unwrap();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1)], false);
        set_order_discount(&c, &cashier, id, Some(DiscountIn { kind: "percent".into(), value: 1000, reason: String::new() }), None).unwrap();
        assert_eq!(total(&c, id).0, 2250);
        // 300 off 2500 is 12% > 10% -> needs approval.
        let r = set_order_discount(&c, &cashier, id, Some(DiscountIn { kind: "fixed".into(), value: 300, reason: String::new() }), None);
        assert_eq!(r.unwrap_err().code, "approval_required");
    }

    #[test]
    fn mixed_payment_change_and_close_flow() {
        let (c, _, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1), simple_line(&c, "Cookie", 1)], true); // 37.00
        assert_eq!(total(&c, id).3, "sent");
        assert_eq!(pay(&c, &cashier, id, vec![PayIn { method: "card".into(), amount_cents: 5000, tendered_cents: None }], "main").unwrap_err().code, "overpaid");
        let r = pay(&c, &cashier, id, vec![
            PayIn { method: "card".into(), amount_cents: 2000, tendered_cents: None },
            PayIn { method: "cash".into(), amount_cents: 1700, tendered_cents: Some(2000) },
        ], "main").unwrap();
        assert_eq!(r["change_cents"], json!(300));
        assert_eq!(total(&c, id), (3700, 3700, "paid".into(), "sent".into()));
        set_status(&c, id, "ready").unwrap();
        set_status(&c, id, "served").unwrap();
        assert_eq!(total(&c, id).3, "closed");
        assert_eq!(update_line(&c, 0, Some(1), None).unwrap_err().code, "not_found");
        let line: i64 = c.query_row("SELECT id FROM order_lines WHERE order_id=?1 LIMIT 1", [id], |r| r.get(0)).unwrap();
        assert_eq!(update_line(&c, line, None, Some("x".into())).unwrap_err().code, "order_closed");
    }

    #[test]
    fn payment_requires_open_till() {
        let (c, _, cashier) = demo();
        c.execute("UPDATE cash_sessions SET closed_at='2026-01-01'", []).unwrap();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], false);
        assert_eq!(pay(&c, &cashier, id, vec![PayIn { method: "cash".into(), amount_cents: 1200, tendered_cents: None }], "main").unwrap_err().code, "no_cash_session");
    }

    #[test]
    fn sent_lines_need_void_approval_unsent_are_deleted() {
        let (c, _, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 2)], true);
        submit(&c, &cashier, SubmitIn { order_id: Some(id), kind: None, table_id: None, customer_name: None, customer_phone: None,
            customer_address: None, delivery_fee_cents: None, note: None, lines: vec![simple_line(&c, "Tiramisu", 1)], send: false,
            hold: false, hold_label: None, register: "main".into() }).unwrap();
        let lines: Vec<(i64, String)> = c.prepare("SELECT id, kitchen_status FROM order_lines WHERE order_id=?1 ORDER BY id").unwrap()
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(lines[0].1, "sent");
        assert_eq!(lines[1].1, "new");
        assert_eq!(update_line(&c, lines[0].0, Some(5), None).unwrap_err().code, "line_sent");
        update_line(&c, lines[1].0, Some(2), None).unwrap();
        assert_eq!(total(&c, id).0, 2400 + 5000);
        remove_line(&c, &cashier, lines[1].0, None, None).unwrap();
        assert_eq!(remove_line(&c, &cashier, lines[0].0, None, Some("2222")).unwrap_err().code, "reason_required");
        assert_eq!(remove_line(&c, &cashier, lines[0].0, Some("client parti".into()), None).unwrap_err().code, "approval_required");
        remove_line(&c, &cashier, lines[0].0, Some("client parti".into()), Some("2222")).unwrap();
        assert_eq!(total(&c, id).0, 0);
    }

    #[test]
    fn void_refund_reopen() {
        let (c, owner, cashier) = demo();
        let a = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], false);
        assert_eq!(void(&c, &cashier, a, "erreur", None).unwrap_err().code, "approval_required");
        void(&c, &cashier, a, "erreur", Some("2222")).unwrap();
        assert_eq!(total(&c, a).3, "voided");
        assert_eq!(pay(&c, &cashier, a, vec![], "main").unwrap_err().code, "order_voided");

        let b = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1)], false);
        pay(&c, &cashier, b, vec![PayIn { method: "cash".into(), amount_cents: 2500, tendered_cents: Some(5000) }], "main").unwrap();
        assert_eq!(void(&c, &owner, b, "x", None).unwrap_err().code, "refund_first");
        reopen(&c, &cashier, b, Some("2222")).unwrap();
        submit(&c, &cashier, SubmitIn { order_id: Some(b), kind: None, table_id: None, customer_name: None, customer_phone: None,
            customer_address: None, delivery_fee_cents: None, note: None, lines: vec![simple_line(&c, "Cookie", 1)], send: false,
            hold: false, hold_label: None, register: "main".into() }).unwrap();
        assert_eq!(total(&c, b), (3700, 2500, "partial".into(), "open".into()));
        refund(&c, &owner, b, "client mécontent", None, "main").unwrap();
        assert_eq!(total(&c, b).1, 0);
        assert_eq!(total(&c, b).2, "refunded");
        let actions: Vec<String> = c.prepare("SELECT action FROM audit_log WHERE action LIKE 'order.%' ORDER BY id").unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(actions, vec!["order.voided", "order.reopened", "order.refunded"]);
    }

    #[test]
    fn split_by_items_keeps_number_and_totals() {
        let (c, _, cashier) = demo();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Cookie", 3), simple_line(&c, "Tiramisu", 1)], true);
        let lines: Vec<i64> = c.prepare("SELECT id FROM order_lines WHERE order_id=?1 ORDER BY id").unwrap()
            .query_map([id], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert!(split(&c, &cashier, id, vec![SplitMove { line_id: lines[0], qty: 3 }, SplitMove { line_id: lines[1], qty: 1 }]).is_err(), "can't empty the original");
        let child = split(&c, &cashier, id, vec![SplitMove { line_id: lines[0], qty: 1 }, SplitMove { line_id: lines[1], qty: 1 }]).unwrap();
        let (n1, s1): (i64, i64) = c.query_row("SELECT number, split_index FROM orders WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let (n2, s2): (i64, i64) = c.query_row("SELECT number, split_index FROM orders WHERE id=?1", [child], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((n1, s1, n2, s2), (n1, 0, n1, 1));
        assert_eq!(total(&c, id).0, 2 * 1200);
        assert_eq!(total(&c, child).0, 1200 + 2500);
    }

    #[test]
    fn hold_and_resume() {
        let (c, _, cashier) = demo();
        let id = submit(&c, &cashier, SubmitIn { order_id: None, kind: Some("dine_in".into()), table_id: Some(3), customer_name: None,
            customer_phone: None, customer_address: None, delivery_fee_cents: None, note: None, lines: vec![simple_line(&c, "Cookie", 1)],
            send: false, hold: true, hold_label: Some("Table 3".into()), register: "main".into() }).unwrap();
        let held = list(&c, ListIn { scope: "held".into(), limit: None }).unwrap();
        assert_eq!(held.as_array().unwrap().len(), 1);
        submit(&c, &cashier, SubmitIn { order_id: Some(id), kind: None, table_id: None, customer_name: None, customer_phone: None,
            customer_address: None, delivery_fee_cents: None, note: None, lines: vec![], send: true, hold: false, hold_label: None,
            register: "main".into() }).unwrap();
        assert_eq!(list(&c, ListIn { scope: "held".into(), limit: None }).unwrap().as_array().unwrap().len(), 0);
    }
}
