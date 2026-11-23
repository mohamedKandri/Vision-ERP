//! Reports (Phase 8): today at a glance and date-range reports, computed from the ledger.
//!
//! Counting rules (the same everywhere, so every section adds up):
//! - a **sale** counts when the order is fully paid (`paid_at`), with the order's current totals;
//! - a **refund** counts when it is made (`refunded_at`), as the same amounts negated;
//! - voided orders are never sales; they are listed under voids.
//! - **Food cost** of a line is the net value of its stock moves (used minus given back). A
//!   refund takes the revenue away but not the cost: if the stock was given back the line's
//!   cost is already zero, otherwise the food was made and is a loss.
//!
//! Dates: the screen sends the range as UTC instants (its local midnight converted) and its
//! UTC offset, used to group by local hour and day.

use crate::error::{ApiError, ApiResult};
use crate::pricing::{self, Discount, LineInput, Tax, TaxBucket};
use crate::stock::{mc_to_cents, value_mc};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Deserialize, Clone)]
pub struct RangeIn {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub tz_offset_min: i64,
}

// ------------------------------------------------------------------ pure helpers

/// `YYYY-MM-DDTHH:MM:SS(.fff)Z`, the format SQLite and JS `toISOString()` both produce,
/// so plain string comparison orders instants correctly.
pub fn valid_instant(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 || b.len() > 30 || *b.last().unwrap() != b'Z' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| r.into_iter().all(|i| b[i].is_ascii_digit());
    digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10) && b[10] == b'T'
        && digits(11..13) && b[13] == b':' && digits(14..16) && b[16] == b':' && digits(17..19)
}

pub fn check_range(r: &RangeIn) -> ApiResult<()> {
    if !valid_instant(&r.from) || !valid_instant(&r.to) || r.from >= r.to {
        return Err(ApiError::invalid("bad date range"));
    }
    if !(-840..=840).contains(&r.tz_offset_min) {
        return Err(ApiError::invalid("bad time zone offset"));
    }
    // A little over a year at most: enough for a yearly report, keeps it fast on a tablet.
    if r.to[..10] > *add_days(&r.from[..10], 400) {
        return Err(ApiError::invalid("range too long"));
    }
    Ok(())
}

/// SQLite date modifier that turns a UTC instant into local time.
pub fn tz_modifier(offset_min: i64) -> String {
    format!("{offset_min:+} minutes")
}

/// `YYYY-MM-DD` + n days (proleptic Gregorian, n >= 0).
fn add_days(day: &str, n: i64) -> String {
    let y: i64 = day[0..4].parse().unwrap_or(2000);
    let m: i64 = day[5..7].parse().unwrap_or(1);
    let d: i64 = day[8..10].parse().unwrap_or(1);
    let days = days_from_civil(y, m, d) + n;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Average rounded half away from zero; 0 when there is nothing to average.
pub fn average(total: i64, count: i64) -> i64 {
    if count == 0 { 0 } else { pricing::div_round(total, count) }
}

/// part / whole in basis points (10000 = 100 %), None when whole is 0.
pub fn ratio_bp(part: i64, whole: i64) -> Option<i64> {
    if whole == 0 { None } else { Some(pricing::div_round(part * 10000, whole)) }
}

/// Add an order's VAT buckets (times `sign`) into the running list, keyed by tax.
pub fn merge_vat(acc: &mut Vec<TaxBucket>, buckets: &[TaxBucket], sign: i64) {
    for b in buckets {
        match acc.iter_mut().find(|x| x.tax_id == b.tax_id) {
            Some(x) => {
                x.base += sign * b.base;
                x.tax += sign * b.tax;
                x.total += sign * b.total;
            }
            None => acc.push(TaxBucket { tax_id: b.tax_id, rate_bp: b.rate_bp, base: sign * b.base, tax: sign * b.tax, total: sign * b.total }),
        }
    }
    acc.sort_by(|a, b| a.rate_bp.cmp(&b.rate_bp).then(a.tax_id.cmp(&b.tax_id)));
}

/// A line's own discount (without its share of an order discount), as the till computed it.
pub fn own_line_discount(unit_cents: i64, qty: i64, rate_bp: i64, include: bool, kind: Option<&str>, value: Option<i64>) -> i64 {
    let gross = unit_cents * qty;
    let discount = match (kind, value) {
        (Some("comp"), _) => return gross,
        (Some("percent"), Some(v)) => Some(Discount::Percent { bp: v }),
        (Some("fixed"), Some(v)) => Some(Discount::Fixed { cents: v }),
        _ => None,
    };
    let l = LineInput { unit_cents, qty, tax: Tax { id: 0, rate_bp, price_include: include }, discount };
    pricing::compute_line(&l).map(|t| t.discount).unwrap_or(0)
}

// ------------------------------------------------------------------ the report

/// Sales events in the range: (order_id, at, sign). Refunds are -1.
const EV: &str = "WITH ev(order_id, at, sign) AS (
    SELECT id, paid_at, 1 FROM orders WHERE paid_at >= ?1 AND paid_at < ?2 AND status <> 'voided'
    UNION ALL
    SELECT id, refunded_at, -1 FROM orders WHERE refunded_at >= ?1 AND refunded_at < ?2)";

/// Net value of the stock a line used (cents, positive = cost).
const LINE_COST: &str = "(SELECT -COALESCE(SUM(m.value_cents), 0) FROM stock_moves m WHERE m.order_line_id = l.id AND m.kind IN ('sale', 'return'))";

fn json_or(s: Option<String>) -> Value {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

pub fn report(conn: &Connection, r: &RangeIn) -> ApiResult<Value> {
    check_range(r)?;
    let tz = tz_modifier(r.tz_offset_min);
    let (from, to) = (r.from.as_str(), r.to.as_str());

    // ---- totals
    let t = conn.query_row(
        &format!(
            "{EV} SELECT COUNT(CASE WHEN sign=1 THEN 1 END), COUNT(CASE WHEN sign=-1 THEN 1 END),
               COALESCE(SUM(sign*o.subtotal_cents),0), COALESCE(SUM(sign*o.discount_cents),0), COALESCE(SUM(sign*o.base_cents),0),
               COALESCE(SUM(sign*o.tax_cents),0), COALESCE(SUM(sign*o.total_cents),0),
               COALESCE(SUM(CASE WHEN sign=1 THEN o.total_cents END),0), COALESCE(SUM(CASE WHEN sign=-1 THEN o.total_cents END),0),
               COALESCE(SUM(sign*o.delivery_fee_cents),0)
             FROM ev JOIN orders o ON o.id = ev.order_id"
        ),
        params![from, to],
        |x| Ok([x.get::<_, i64>(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?, x.get(5)?, x.get(6)?, x.get(7)?, x.get(8)?, x.get(9)?]),
    )?;
    let [orders, refunds, gross, discounts, base, tax, net, sales, refunded, fees] = t;

    // ---- by local hour and day
    let mut hours: Vec<Value> = (0..24).map(|h| json!({"hour": h, "orders": 0, "total_cents": 0})).collect();
    let mut stmt = conn.prepare(&format!(
        "{EV} SELECT CAST(strftime('%H', ev.at, ?3) AS INTEGER), COUNT(CASE WHEN sign=1 THEN 1 END), SUM(sign*o.total_cents)
         FROM ev JOIN orders o ON o.id = ev.order_id GROUP BY 1"
    ))?;
    for row in stmt.query_map(params![from, to, tz], |x| Ok((x.get::<_, i64>(0)?, x.get::<_, i64>(1)?, x.get::<_, i64>(2)?)))? {
        let (h, n, v) = row?;
        if let Some(slot) = hours.get_mut(h as usize) {
            *slot = json!({"hour": h, "orders": n, "total_cents": v});
        }
    }
    let by_day = conn
        .prepare(&format!(
            "{EV} SELECT date(ev.at, ?3), COUNT(CASE WHEN sign=1 THEN 1 END), SUM(sign*o.total_cents)
             FROM ev JOIN orders o ON o.id = ev.order_id GROUP BY 1 ORDER BY 1"
        ))?
        .query_map(params![from, to, tz], |x| {
            Ok(json!({"day": x.get::<_, String>(0)?, "orders": x.get::<_, i64>(1)?, "total_cents": x.get::<_, i64>(2)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // ---- by product (and category), with food cost
    let by_product = conn
        .prepare(&format!(
            "{EV} SELECT l.product_id, COALESCE(p.names, MAX(l.names)), p.category_id, p.kind,
               SUM(sign*l.qty), SUM(sign*l.total_cents), SUM(sign*l.base_cents), SUM(sign*l.discount_cents),
               SUM(CASE WHEN sign=1 THEN {LINE_COST} ELSE 0 END)
             FROM ev JOIN order_lines l ON l.order_id = ev.order_id AND l.voided = 0
             LEFT JOIN products p ON p.id = l.product_id
             GROUP BY l.product_id ORDER BY 6 DESC, 5 DESC"
        ))?
        .query_map(params![from, to], |x| {
            let (base, cost): (i64, i64) = (x.get(6)?, x.get(8)?);
            Ok(json!({
                "product_id": x.get::<_, i64>(0)?, "names": json_or(x.get(1)?), "category_id": x.get::<_, Option<i64>>(2)?,
                "kind": x.get::<_, Option<String>>(3)?, "qty": x.get::<_, i64>(4)?, "total_cents": x.get::<_, i64>(5)?,
                "base_cents": base, "discount_cents": x.get::<_, i64>(7)?, "cost_cents": cost,
                "margin_cents": base - cost, "food_cost_bp": ratio_bp(cost, base),
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let cat_names: BTreeMap<i64, Value> = conn
        .prepare("SELECT id, names FROM categories")?
        .query_map([], |x| Ok((x.get::<_, i64>(0)?, json_or(x.get(1)?))))?
        .collect::<Result<_, _>>()?;
    let mut cats: BTreeMap<i64, [i64; 4]> = BTreeMap::new();
    for p in &by_product {
        let c = cats.entry(p["category_id"].as_i64().unwrap_or(0)).or_default();
        for (i, k) in ["qty", "total_cents", "base_cents", "cost_cents"].iter().enumerate() {
            c[i] += p[k].as_i64().unwrap_or(0);
        }
    }
    let mut by_category: Vec<Value> = cats
        .into_iter()
        .map(|(id, [qty, total, base, cost])| {
            json!({"category_id": id, "names": cat_names.get(&id).cloned().unwrap_or(Value::Null), "qty": qty, "total_cents": total,
                   "base_cents": base, "cost_cents": cost, "margin_cents": base - cost, "food_cost_bp": ratio_bp(cost, base)})
        })
        .collect();
    by_category.sort_by_key(|c| -c["total_cents"].as_i64().unwrap_or(0));
    let line_base: i64 = by_product.iter().map(|p| p["base_cents"].as_i64().unwrap_or(0)).sum();
    let line_cost: i64 = by_product.iter().map(|p| p["cost_cents"].as_i64().unwrap_or(0)).sum();

    // ---- by order type and staff (who took the order)
    let by_type = conn
        .prepare(&format!(
            "{EV} SELECT o.type, COUNT(CASE WHEN sign=1 THEN 1 END), SUM(sign*o.total_cents), SUM(CASE WHEN sign=1 THEN o.total_cents ELSE 0 END)
             FROM ev JOIN orders o ON o.id = ev.order_id GROUP BY o.type ORDER BY 3 DESC"
        ))?
        .query_map(params![from, to], |x| {
            let (n, s): (i64, i64) = (x.get(1)?, x.get(3)?);
            Ok(json!({"type": x.get::<_, String>(0)?, "orders": n, "total_cents": x.get::<_, i64>(2)?, "avg_ticket_cents": average(s, n)}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let by_staff = conn
        .prepare(&format!(
            "{EV} SELECT o.created_by, u.name, COUNT(CASE WHEN sign=1 THEN 1 END), SUM(sign*o.total_cents),
               SUM(CASE WHEN sign=1 THEN o.total_cents ELSE 0 END), SUM(CASE WHEN sign=1 THEN o.discount_cents ELSE 0 END)
             FROM ev JOIN orders o ON o.id = ev.order_id JOIN users u ON u.id = o.created_by GROUP BY o.created_by ORDER BY 4 DESC"
        ))?
        .query_map(params![from, to], |x| {
            let (n, s): (i64, i64) = (x.get(2)?, x.get(4)?);
            Ok(json!({"user_id": x.get::<_, i64>(0)?, "name": x.get::<_, String>(1)?, "orders": n, "total_cents": x.get::<_, i64>(3)?,
                      "avg_ticket_cents": average(s, n), "discount_cents": x.get::<_, i64>(5)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // ---- payment methods: the money actually taken in the range (payments table)
    let by_payment = conn
        .prepare(
            "SELECT method, COUNT(CASE WHEN kind='payment' THEN 1 END), COALESCE(SUM(CASE WHEN kind='payment' THEN amount_cents END),0),
               -COALESCE(SUM(CASE WHEN kind='refund' THEN amount_cents END),0), COALESCE(SUM(amount_cents),0)
             FROM payments WHERE at >= ?1 AND at < ?2 GROUP BY method ORDER BY method",
        )?
        .query_map(params![from, to], |x| {
            Ok(json!({"method": x.get::<_, String>(0)?, "count": x.get::<_, i64>(1)?, "paid_cents": x.get::<_, i64>(2)?,
                      "refunded_cents": x.get::<_, i64>(3)?, "net_cents": x.get::<_, i64>(4)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // ---- VAT
    let tax_names: BTreeMap<i64, String> = conn
        .prepare("SELECT id, name FROM taxes")?
        .query_map([], |x| Ok((x.get(0)?, x.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut vat: Vec<TaxBucket> = Vec::new();
    let mut stmt = conn.prepare(&format!("{EV} SELECT o.vat, ev.sign FROM ev JOIN orders o ON o.id = ev.order_id"))?;
    for row in stmt.query_map(params![from, to], |x| Ok((x.get::<_, String>(0)?, x.get::<_, i64>(1)?)))? {
        let (v, sign) = row?;
        merge_vat(&mut vat, &serde_json::from_str::<Vec<TaxBucket>>(&v).unwrap_or_default(), sign);
    }
    let vat: Vec<Value> = vat
        .iter()
        .map(|b| json!({"tax_id": b.tax_id, "name": tax_names.get(&b.tax_id), "rate_bp": b.rate_bp, "base_cents": b.base, "tax_cents": b.tax, "total_cents": b.total}))
        .collect();

    Ok(json!({
        "range": {"from": from, "to": to, "tz_offset_min": r.tz_offset_min},
        "totals": {
            "orders": orders, "refunds": refunds, "gross_cents": gross, "discount_cents": discounts, "base_cents": base,
            "tax_cents": tax, "net_cents": net, "sales_cents": sales, "refunded_cents": refunded, "delivery_fees_cents": fees,
            "avg_ticket_cents": average(sales, orders),
        },
        "by_hour": hours, "by_day": by_day, "by_product": by_product, "by_category": by_category,
        "by_type": by_type, "by_staff": by_staff, "by_payment": by_payment, "vat": vat,
        "food_cost": {"revenue_ht_cents": line_base, "cost_cents": line_cost, "margin_cents": line_base - line_cost, "food_cost_bp": ratio_bp(line_cost, line_base)},
        "discounts": discounts_section(conn, from, to)?,
        "voids": voids_section(conn, from, to)?,
        "stock": stock_section(conn, from, to)?,
        "waste": waste_section(conn, from, to)?,
    }))
}

/// Every discount and comp on orders sold in the range.
fn discounts_section(conn: &Connection, from: &str, to: &str) -> ApiResult<Value> {
    let mut items: Vec<Value> = Vec::new();
    // Line discounts (own amount, without the share of an order discount).
    let mut own_by_order: BTreeMap<i64, i64> = BTreeMap::new();
    let mut stmt = conn.prepare(&format!(
        "{EV} SELECT o.id, o.number, o.split_index, ev.at, l.names, l.qty, l.unit_cents, l.tax_rate_bp, l.price_include,
           l.discount_kind, l.discount_value, l.discount_reason, u.name
         FROM ev JOIN orders o ON o.id = ev.order_id JOIN order_lines l ON l.order_id = o.id
         LEFT JOIN users u ON u.id = l.discount_by
         WHERE ev.sign = 1 AND l.voided = 0 AND l.discount_kind IS NOT NULL ORDER BY ev.at, l.id"
    ))?;
    let rows = stmt.query_map(params![from, to], |x| {
        Ok((x.get::<_, i64>(0)?, x.get::<_, i64>(1)?, x.get::<_, i64>(2)?, x.get::<_, String>(3)?, x.get::<_, String>(4)?, x.get::<_, i64>(5)?,
            x.get::<_, i64>(6)?, x.get::<_, i64>(7)?, x.get::<_, bool>(8)?, x.get::<_, String>(9)?, x.get::<_, Option<i64>>(10)?,
            x.get::<_, Option<String>>(11)?, x.get::<_, Option<String>>(12)?))
    })?;
    for row in rows {
        let (oid, number, split, at, names, qty, unit, rate, incl, kind, value, reason, by) = row?;
        let amount = own_line_discount(unit, qty, rate, incl, Some(&kind), value);
        *own_by_order.entry(oid).or_default() += amount;
        items.push(json!({"order_id": oid, "number": number, "split_index": split, "at": at, "target": "line",
            "names": json_or(Some(names)), "qty": qty, "kind": kind, "value": value, "amount_cents": amount, "reason": reason, "by": by}));
    }
    // Order discounts: the order's total discount minus its lines' own discounts.
    let mut stmt = conn.prepare(&format!(
        "{EV} SELECT o.id, o.number, o.split_index, ev.at, o.discount_kind, o.discount_value, o.discount_reason, u.name, o.discount_cents
         FROM ev JOIN orders o ON o.id = ev.order_id LEFT JOIN users u ON u.id = o.discount_by
         WHERE ev.sign = 1 AND o.discount_kind IS NOT NULL ORDER BY ev.at"
    ))?;
    let rows = stmt.query_map(params![from, to], |x| {
        Ok((x.get::<_, i64>(0)?, x.get::<_, i64>(1)?, x.get::<_, i64>(2)?, x.get::<_, String>(3)?, x.get::<_, String>(4)?,
            x.get::<_, Option<i64>>(5)?, x.get::<_, Option<String>>(6)?, x.get::<_, Option<String>>(7)?, x.get::<_, i64>(8)?))
    })?;
    for row in rows {
        let (oid, number, split, at, kind, value, reason, by, all) = row?;
        let amount = all - own_by_order.get(&oid).copied().unwrap_or(0);
        items.push(json!({"order_id": oid, "number": number, "split_index": split, "at": at, "target": "order",
            "names": Value::Null, "qty": Value::Null, "kind": kind, "value": value, "amount_cents": amount, "reason": reason, "by": by}));
    }
    items.sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
    let total: i64 = items.iter().map(|i| i["amount_cents"].as_i64().unwrap_or(0)).sum();
    let comps: i64 = items.iter().filter(|i| i["kind"] == "comp").map(|i| i["amount_cents"].as_i64().unwrap_or(0)).sum();
    Ok(json!({"count": items.len(), "total_cents": total, "comp_cents": comps, "items": items}))
}

/// Cancelled orders, cancelled lines and refunds in the range.
fn voids_section(conn: &Connection, from: &str, to: &str) -> ApiResult<Value> {
    let orders = conn
        .prepare(
            "SELECT o.id, o.number, o.split_index, o.voided_at, o.total_cents, o.void_reason, v.name, c.name
             FROM orders o LEFT JOIN users v ON v.id = o.voided_by JOIN users c ON c.id = o.created_by
             WHERE o.voided_at >= ?1 AND o.voided_at < ?2 ORDER BY o.voided_at",
        )?
        .query_map(params![from, to], |x| {
            Ok(json!({"order_id": x.get::<_, i64>(0)?, "number": x.get::<_, i64>(1)?, "split_index": x.get::<_, i64>(2)?, "at": x.get::<_, String>(3)?,
                      "amount_cents": x.get::<_, i64>(4)?, "reason": x.get::<_, Option<String>>(5)?, "by": x.get::<_, Option<String>>(6)?,
                      "created_by": x.get::<_, String>(7)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // Lines cancelled after being sent (the till removes unsent lines without a trace).
    let lines = conn
        .prepare(
            "SELECT o.id, o.number, o.split_index, a.at, l.names, l.qty, l.unit_cents * l.qty, l.void_reason, v.name
             FROM audit_log a JOIN order_lines l ON l.id = json_extract(a.details, '$.line_id')
             JOIN orders o ON o.id = l.order_id LEFT JOIN users v ON v.id = l.voided_by
             WHERE a.action = 'order.line_voided' AND a.at >= ?1 AND a.at < ?2 ORDER BY a.at",
        )?
        .query_map(params![from, to], |x| {
            Ok(json!({"order_id": x.get::<_, i64>(0)?, "number": x.get::<_, i64>(1)?, "split_index": x.get::<_, i64>(2)?, "at": x.get::<_, String>(3)?,
                      "names": json_or(x.get(4)?), "qty": x.get::<_, i64>(5)?, "amount_cents": x.get::<_, i64>(6)?,
                      "reason": x.get::<_, Option<String>>(7)?, "by": x.get::<_, Option<String>>(8)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let refunds = conn
        .prepare(
            "SELECT o.id, o.number, o.split_index, o.refunded_at, o.total_cents, o.refund_reason, v.name
             FROM orders o LEFT JOIN users v ON v.id = o.refunded_by
             WHERE o.refunded_at >= ?1 AND o.refunded_at < ?2 ORDER BY o.refunded_at",
        )?
        .query_map(params![from, to], |x| {
            Ok(json!({"order_id": x.get::<_, i64>(0)?, "number": x.get::<_, i64>(1)?, "split_index": x.get::<_, i64>(2)?, "at": x.get::<_, String>(3)?,
                      "amount_cents": x.get::<_, i64>(4)?, "reason": x.get::<_, Option<String>>(5)?, "by": x.get::<_, Option<String>>(6)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let sum = |v: &[Value]| -> i64 { v.iter().map(|x| x["amount_cents"].as_i64().unwrap_or(0)).sum() };
    Ok(json!({
        "orders_cents": sum(&orders), "lines_cents": sum(&lines), "refunds_cents": sum(&refunds),
        "orders": orders, "lines": lines, "refunds": refunds,
    }))
}

/// Stock value now, and what moved in the range by kind (cents, signed: + in, - out).
fn stock_section(conn: &Connection, from: &str, to: &str) -> ApiResult<Value> {
    let mut value_mc_total: i128 = 0;
    let mut count = 0i64;
    let mut stmt = conn.prepare("SELECT unit, cost_cents, stock_milli FROM ingredients WHERE active = 1")?;
    for row in stmt.query_map([], |x| Ok((x.get::<_, String>(0)?, x.get::<_, i64>(1)?, x.get::<_, i64>(2)?)))? {
        let (unit, cost, qty) = row?;
        value_mc_total += value_mc(&unit, cost, qty.max(0));
        count += 1;
    }
    let mut moves = json!({"sale": 0, "return": 0, "waste": 0, "adjust": 0, "count": 0, "purchase": 0, "initial": 0});
    let mut stmt = conn.prepare("SELECT kind, COALESCE(SUM(value_cents),0) FROM stock_moves WHERE at >= ?1 AND at < ?2 GROUP BY kind")?;
    for row in stmt.query_map(params![from, to], |x| Ok((x.get::<_, String>(0)?, x.get::<_, i64>(1)?)))? {
        let (k, v) = row?;
        moves[k] = json!(v);
    }
    Ok(json!({"value_cents": mc_to_cents(value_mc_total), "ingredients": count, "moves": moves}))
}

/// Waste in the range, by ingredient and entry by entry.
fn waste_section(conn: &Connection, from: &str, to: &str) -> ApiResult<Value> {
    let by_ingredient = conn
        .prepare(
            "SELECT i.id, i.names, i.unit, -SUM(m.delta_milli), -SUM(m.value_cents)
             FROM stock_moves m JOIN ingredients i ON i.id = m.ingredient_id
             WHERE m.kind = 'waste' AND m.at >= ?1 AND m.at < ?2 GROUP BY i.id ORDER BY 5 DESC",
        )?
        .query_map(params![from, to], |x| {
            Ok(json!({"ingredient_id": x.get::<_, i64>(0)?, "names": json_or(x.get(1)?), "unit": x.get::<_, String>(2)?,
                      "qty_milli": x.get::<_, i64>(3)?, "value_cents": x.get::<_, i64>(4)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // One entry per waste action: moves recorded together share time, reason, product and user.
    let items = conn
        .prepare(
            "SELECT m.at, m.reason, p.names, u.name, COUNT(*), -SUM(m.value_cents),
               CASE WHEN COUNT(*) = 1 THEN MAX(i.names) END, CASE WHEN COUNT(*) = 1 THEN MAX(i.unit) END, -SUM(m.delta_milli)
             FROM stock_moves m JOIN ingredients i ON i.id = m.ingredient_id
             LEFT JOIN products p ON p.id = m.product_id LEFT JOIN users u ON u.id = m.user_id
             WHERE m.kind = 'waste' AND m.at >= ?1 AND m.at < ?2
             GROUP BY m.at, m.reason, m.product_id, m.user_id ORDER BY m.at DESC LIMIT 500",
        )?
        .query_map(params![from, to], |x| {
            Ok(json!({"at": x.get::<_, String>(0)?, "reason": x.get::<_, Option<String>>(1)?, "product_names": json_or(x.get(2)?),
                      "user": x.get::<_, Option<String>>(3)?, "ingredients": x.get::<_, i64>(4)?, "value_cents": x.get::<_, i64>(5)?,
                      "ingredient_names": json_or(x.get(6)?), "unit": x.get::<_, Option<String>>(7)?, "qty_milli": x.get::<_, i64>(8)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let total: i64 = by_ingredient.iter().map(|x| x["value_cents"].as_i64().unwrap_or(0)).sum();
    Ok(json!({"total_cents": total, "by_ingredient": by_ingredient, "items": items}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::{demo, simple_line};
    use crate::orders::{self, DiscountIn, PayIn, SubmitIn};
    use crate::stock;

    /// From yesterday to tomorrow (UTC), around the test's own "now".
    fn all(c: &Connection) -> RangeIn {
        let (from, to): (String, String) = c
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 day'), strftime('%Y-%m-%dT%H:%M:%fZ','now','+1 day')", [], |x| Ok((x.get(0)?, x.get(1)?)))
            .unwrap();
        range(&from, &to, 60)
    }

    fn range(from: &str, to: &str, tz: i64) -> RangeIn {
        RangeIn { from: from.into(), to: to.into(), tz_offset_min: tz }
    }

    #[test]
    fn pure_helpers() {
        assert!(valid_instant("2026-11-17T08:30:00.000Z"));
        assert!(valid_instant("2026-11-17T08:30:00Z"));
        assert!(!valid_instant("2026-11-17 08:30:00"));
        assert!(!valid_instant("2026-11-17T08:30:00.000+01:00"));
        assert!(!valid_instant("x' OR 1=1 --aaaaaaaaaaZ"));
        assert!(check_range(&range("2026-11-17T00:00:00.000Z", "2026-11-16T00:00:00.000Z", 0)).is_err(), "to before from");
        assert!(check_range(&range("2026-01-01T00:00:00.000Z", "2026-12-31T23:00:00.000Z", 60)).is_ok());
        assert!(check_range(&range("2024-01-01T00:00:00.000Z", "2026-01-01T00:00:00.000Z", 0)).is_err(), "too long");
        assert!(check_range(&range("2026-01-01T00:00:00.000Z", "2026-01-02T00:00:00.000Z", 900)).is_err());
        assert_eq!(tz_modifier(60), "+60 minutes");
        assert_eq!(tz_modifier(-90), "-90 minutes");
        assert_eq!(add_days("2026-02-27", 2), "2026-03-01");
        assert_eq!(add_days("2028-02-28", 1), "2028-02-29");
        assert_eq!(add_days("2026-12-31", 1), "2027-01-01");
        assert_eq!(average(10000, 3), 3333);
        assert_eq!(average(5, 2), 3, "half away from zero");
        assert_eq!(average(-5, 2), -3);
        assert_eq!(average(100, 0), 0);
        assert_eq!(ratio_bp(1871, 4091), Some(4573));
        assert_eq!(ratio_bp(1, 0), None);
        // VAT buckets: sales add, refunds subtract, sorted by rate.
        let b = |id, rate, base, tax| TaxBucket { tax_id: id, rate_bp: rate, base, tax, total: base + tax };
        let mut acc = vec![];
        merge_vat(&mut acc, &[b(2, 2000, 1000, 200)], 1);
        merge_vat(&mut acc, &[b(1, 1000, 4455, 445), b(2, 2000, 500, 100)], 1);
        merge_vat(&mut acc, &[b(1, 1000, 4455, 445)], -1);
        assert_eq!(acc, vec![b(1, 1000, 0, 0), b(2, 2000, 1500, 300)]);
        // A line's own discount, as the till computes it.
        assert_eq!(own_line_discount(1200, 2, 1000, true, Some("percent"), Some(1000)), 240);
        assert_eq!(own_line_discount(1200, 2, 1000, true, Some("fixed"), Some(500)), 500);
        assert_eq!(own_line_discount(1200, 2, 1000, true, Some("comp"), None), 2400);
        assert_eq!(own_line_discount(1200, 2, 1000, true, None, None), 0);
    }

    fn submit(c: &Connection, me: &crate::auth::SessionUser, lines: Vec<orders::NewLine>, kind: &str, send: bool) -> i64 {
        orders::submit(c, me, SubmitIn {
            order_id: None, kind: Some(kind.into()), table_id: None, customer_name: Some("Sara".into()),
            customer_phone: Some("0600000000".into()), customer_address: Some("12 rue X".into()), delivery_fee_cents: None, note: None,
            lines, send, hold: false, hold_label: None, register: "main".into(),
        })
        .unwrap()
    }
    fn total(c: &Connection, id: i64) -> i64 {
        c.query_row("SELECT total_cents FROM orders WHERE id=?1", [id], |r| r.get(0)).unwrap()
    }
    fn pay(c: &Connection, me: &crate::auth::SessionUser, id: i64, method: &str) {
        orders::pay(c, me, id, vec![PayIn { method: method.into(), amount_cents: total(c, id), tendered_cents: None }], "main").unwrap();
    }
    fn at(c: &Connection, id: i64, col: &str, when: &str) {
        c.execute(&format!("UPDATE orders SET {col}=?2 WHERE id=?1"), params![id, when]).unwrap();
        if col == "paid_at" {
            c.execute("UPDATE payments SET at=?2 WHERE order_id=?1 AND kind='payment'", params![id, when]).unwrap();
        }
    }
    fn i(v: &Value) -> i64 {
        v.as_i64().unwrap_or_else(|| panic!("not a number: {v}"))
    }

    #[test]
    fn sales_refunds_vat_and_groupings_add_up() {
        let (c, owner, cashier) = demo();
        stock::seed_demo(&c, owner.id).unwrap();
        // A: 2 cokes + tiramisu, cash (4900). B: cookie, delivery (1200 + 1000 fee), card.
        let a = submit(&c, &cashier, vec![simple_line(&c, "Coca-Cola 33cl", 2), simple_line(&c, "Tiramisu", 1)], "takeaway", true);
        pay(&c, &cashier, a, "cash");
        let b = submit(&c, &owner, vec![simple_line(&c, "Cookie", 1)], "delivery", true);
        pay(&c, &owner, b, "card");
        // C: paid then refunded. D: unpaid (not a sale). E: voided.
        let cc = submit(&c, &cashier, vec![simple_line(&c, "Tiramisu", 2)], "takeaway", true);
        pay(&c, &cashier, cc, "cash");
        orders::refund(&c, &cashier, cc, "plainte", Some("2222"), "main").unwrap();
        submit(&c, &cashier, vec![simple_line(&c, "Cookie", 3)], "takeaway", false);
        let e = submit(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], "takeaway", true);
        orders::void(&c, &cashier, e, "erreur", Some("2222")).unwrap();

        let r = report(&c, &all(&c)).unwrap();
        let t = &r["totals"];
        assert_eq!(i(&t["orders"]), 3, "A, B and C were sold");
        assert_eq!(i(&t["refunds"]), 1);
        assert_eq!(i(&t["sales_cents"]), 4900 + 2200 + 5000);
        assert_eq!(i(&t["refunded_cents"]), 5000);
        assert_eq!(i(&t["net_cents"]), 4900 + 2200);
        assert_eq!(i(&t["delivery_fees_cents"]), 1000);
        assert_eq!(i(&t["avg_ticket_cents"]), (4900 + 2200 + 5000) / 3);
        assert_eq!(i(&t["base_cents"]) + i(&t["tax_cents"]), i(&t["net_cents"]));

        // Every grouping adds up to the net.
        let sum = |k: &str, f: &str| -> i64 { r[k].as_array().unwrap().iter().map(|x| i(&x[f])).sum() };
        assert_eq!(sum("by_hour", "total_cents"), 7100);
        assert_eq!(sum("by_hour", "orders"), 3);
        assert_eq!(sum("by_day", "total_cents"), 7100);
        assert_eq!(sum("by_type", "total_cents"), 7100);
        assert_eq!(sum("by_staff", "total_cents"), 7100);
        assert_eq!(sum("by_product", "total_cents") + 1000, 7100, "products + delivery fee");
        assert_eq!(sum("by_category", "total_cents"), sum("by_product", "total_cents"));
        assert_eq!(sum("vat", "total_cents"), 7100);
        assert_eq!(sum("vat", "tax_cents"), i(&t["tax_cents"]));
        let tira = r["by_product"].as_array().unwrap().iter().find(|p| p["names"]["fr"] == "Tiramisu").unwrap();
        assert_eq!(i(&tira["qty"]), 1, "1 sold in A; C's 2 sold and refunded");
        assert_eq!(i(&tira["total_cents"]), 2500);
        let delivery = r["by_type"].as_array().unwrap().iter().find(|x| x["type"] == "delivery").unwrap();
        assert_eq!((i(&delivery["orders"]), i(&delivery["total_cents"])), (1, 2200));

        // Payment methods: money in and out in the range.
        let pm = |m: &str| r["by_payment"].as_array().unwrap().iter().find(|x| x["method"] == m).unwrap().clone();
        assert_eq!((i(&pm("cash")["paid_cents"]), i(&pm("cash")["refunded_cents"]), i(&pm("cash")["net_cents"])), (9900, 5000, 4900));
        assert_eq!(i(&pm("card")["net_cents"]), 2200);

        // Voids and refunds are listed with their reasons.
        assert_eq!(r["voids"]["orders"][0]["reason"], "erreur");
        assert_eq!(i(&r["voids"]["orders_cents"]), 1200);
        assert_eq!(r["voids"]["refunds"][0]["reason"], "plainte");
        assert_eq!(i(&r["voids"]["refunds_cents"]), 5000);

        // Food cost = the stock that sold lines used. C's refund kept its cost (food was made).
        let used: i64 = c.query_row("SELECT -SUM(value_cents) FROM stock_moves WHERE kind IN ('sale','return') AND order_id IN (?1,?2,?3)",
            params![a, b, cc], |x| x.get(0)).unwrap();
        assert!(used > 0);
        assert_eq!(i(&r["food_cost"]["cost_cents"]), used);
        assert_eq!(i(&r["food_cost"]["revenue_ht_cents"]), sum("by_product", "base_cents"));
        assert_eq!(i(&r["stock"]["moves"]["sale"]) + i(&r["stock"]["moves"]["return"]), -(used + {
            // E was sent then voided: its stock went out and came back (net 0).
            let e_net: i64 = c.query_row("SELECT COALESCE(SUM(value_cents),0) FROM stock_moves WHERE order_id=?1", [e], |x| x.get(0)).unwrap();
            -e_net
        }));
        assert!(i(&r["stock"]["value_cents"]) > 0);
    }

    #[test]
    fn local_hours_days_and_range_edges() {
        let (c, _, cashier) = demo();
        let mk = |when: &str| {
            let id = submit(&c, &cashier, vec![simple_line(&c, "Cookie", 1)], "takeaway", true);
            pay(&c, &cashier, id, "cash");
            at(&c, id, "paid_at", when);
            id
        };
        mk("2026-11-16T22:30:00.000Z"); // 23:30 local (UTC+1) on the 16th
        mk("2026-11-16T23:15:00.000Z"); // 00:15 local on the 17th
        mk("2026-11-17T11:59:59.999Z"); // 12:59 local
        mk("2026-11-17T23:00:00.000Z"); // 00:00 local on the 18th: outside "the 17th"
        // The local day 17 Nov at UTC+1 = [16T23:00Z, 17T23:00Z).
        let r = report(&c, &range("2026-11-16T23:00:00.000Z", "2026-11-17T23:00:00.000Z", 60)).unwrap();
        assert_eq!(i(&r["totals"]["orders"]), 2);
        let h = |n: usize| i(&r["by_hour"][n]["orders"]);
        assert_eq!((h(0), h(12), h(23)), (1, 1, 0));
        assert_eq!(r["by_hour"].as_array().unwrap().len(), 24);
        assert_eq!(r["by_day"], json!([{"day": "2026-11-17", "orders": 2, "total_cents": 2400}]));
        // Two local days.
        let r = report(&c, &range("2026-11-15T23:00:00.000Z", "2026-11-17T23:00:00.000Z", 60)).unwrap();
        let days: Vec<&str> = r["by_day"].as_array().unwrap().iter().map(|d| d["day"].as_str().unwrap()).collect();
        assert_eq!(days, vec!["2026-11-16", "2026-11-17"]);
        // Empty range: zeros, no division by zero.
        let r = report(&c, &range("2020-01-01T00:00:00.000Z", "2020-01-02T00:00:00.000Z", 0)).unwrap();
        assert_eq!(i(&r["totals"]["avg_ticket_cents"]), 0);
        assert_eq!(r["food_cost"]["food_cost_bp"], Value::Null);
        assert_eq!(report(&c, &range("bad", "2020-01-02T00:00:00.000Z", 0)).unwrap_err().code, "invalid");
    }

    #[test]
    fn discounts_comps_line_voids_and_waste() {
        let (c, owner, cashier) = demo();
        stock::seed_demo(&c, owner.id).unwrap();
        let a = submit(&c, &cashier, vec![simple_line(&c, "Coca-Cola 33cl", 2), simple_line(&c, "Tiramisu", 1), simple_line(&c, "Cookie", 1)], "takeaway", true);
        let lines: Vec<i64> = c.prepare("SELECT id FROM order_lines WHERE order_id=?1 ORDER BY id").unwrap()
            .query_map([a], |x| x.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        // 10 % off the cokes, the cookie offered, 5,00 off the order, the tiramisu cancelled after sending.
        orders::set_line_discount(&c, &owner, lines[0], Some(DiscountIn { kind: "percent".into(), value: 1000, reason: "fidèle".into() }), None).unwrap();
        orders::set_line_discount(&c, &owner, lines[2], Some(DiscountIn { kind: "comp".into(), value: 0, reason: "attente".into() }), None).unwrap();
        orders::set_order_discount(&c, &owner, a, Some(DiscountIn { kind: "fixed".into(), value: 500, reason: "geste".into() }), None).unwrap();
        orders::remove_line(&c, &owner, lines[1], Some("client parti".into()), None).unwrap();
        pay(&c, &cashier, a, "cash");
        let bun: i64 = c.query_row("SELECT id FROM ingredients WHERE json_extract(names,'$.fr')='Pain burger'", [], |x| x.get(0)).unwrap();
        let lost = stock::waste(&c, &owner, stock::WasteIn { ingredient_id: Some(bun), qty_milli: Some(3000), product_id: None, variant_id: None, qty: None, reason: "moisi".into() }).unwrap();

        let r = report(&c, &all(&c)).unwrap();
        let d = &r["discounts"];
        let items = d["items"].as_array().unwrap();
        let amount = |target: &str, kind: &str| i(&items.iter().find(|x| x["target"] == target && x["kind"] == kind).unwrap()["amount_cents"]);
        assert_eq!(amount("line", "percent"), 240);
        assert_eq!(amount("line", "comp"), 1200);
        assert_eq!(amount("order", "fixed"), 500);
        assert_eq!(i(&d["comp_cents"]), 1200);
        assert_eq!(i(&d["total_cents"]), 240 + 1200 + 500);
        assert_eq!(i(&d["total_cents"]), i(&r["totals"]["discount_cents"]), "listed discounts = order discounts");
        assert_eq!(items.iter().find(|x| x["kind"] == "comp").unwrap()["reason"], "attente");
        assert_eq!(i(&r["totals"]["net_cents"]), 2400 - 240 - 500);

        let v = &r["voids"]["lines"][0];
        assert_eq!((v["names"]["fr"].as_str(), i(&v["qty"]), i(&v["amount_cents"]), v["reason"].as_str()), (Some("Tiramisu"), 1, 2500, Some("client parti")));

        let w = &r["waste"];
        assert_eq!(i(&w["total_cents"]), lost);
        assert_eq!(w["by_ingredient"][0]["names"]["fr"], "Pain burger");
        assert_eq!(i(&w["by_ingredient"][0]["qty_milli"]), 3000);
        assert_eq!(w["items"][0]["reason"], "moisi");
        assert_eq!(i(&r["stock"]["moves"]["waste"]), -lost);
    }

    #[test]
    fn api_permissions() {
        use crate::api::{dispatch, tests::{core_with_setup, login}};
        let core = core_with_setup();
        let (owner, manager, cashier) = (login(&core, "Mohamed", "1234"), login(&core, "Karim", "2222"), login(&core, "Salma", "3333"));
        let p = json!({"from": "2026-11-16T23:00:00.000Z", "to": "2026-11-17T23:00:00.000Z", "tz_offset_min": 60});
        assert_eq!(dispatch(&core, Some(&cashier), "reports.summary", p.clone()).unwrap_err().code, "forbidden");
        assert!(dispatch(&core, Some(&manager), "reports.summary", p.clone()).is_ok());
        assert!(dispatch(&core, Some(&owner), "reports.summary", p).is_ok());
        assert_eq!(crate::net::topic_of("reports.summary"), None, "a read publishes nothing");
    }
}
