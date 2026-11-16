//! Inventory: ingredients, recipes, the stock ledger, automatic deduction from orders,
//! waste, manual adjustments, stock counts and food cost.
//!
//! Quantities are integer milli-units of the ingredient's unit (150 g = 150_000,
//! 1 piece = 1_000). Ingredient cost is in cents per kg (g), per litre (ml) or per piece.
//! `ingredients.stock_milli` is a cache of the ledger (`stock_moves`), always updated in
//! the same transaction as the move.

use crate::auth::SessionUser;
use crate::db::atomic;
use crate::error::{ApiError, ApiResult};
use crate::item::ItemConfig;
use crate::pricing::{div_round, split_included};
use crate::{audit, menu, settings};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// ingredient id -> quantity in milli-units.
pub type Qty = BTreeMap<i64, i64>;

pub const UNITS: &[&str] = &["g", "ml", "piece"];
const MAX_QTY: i64 = 1_000_000_000_000; // 1 million kg / L / pieces

// ------------------------------------------------------------------ pure helpers

/// Value in milli-cents of `qty_milli` of an ingredient costing `cost_cents` per kg / L / piece.
pub fn value_mc(unit: &str, cost_cents: i64, qty_milli: i64) -> i128 {
    let v = qty_milli as i128 * cost_cents as i128;
    if unit == "piece" {
        v
    } else {
        div_round_128(v, 1000)
    }
}

fn div_round_128(n: i128, d: i128) -> i128 {
    let q = n / d;
    let r = n % d;
    if 2 * r.abs() >= d.abs() {
        q + if (n < 0) != (d < 0) { -1 } else { 1 }
    } else {
        q
    }
}

/// Milli-cents to cents, rounded half away from zero.
pub fn mc_to_cents(mc: i128) -> i64 {
    div_round_128(mc, 1000) as i64
}

/// Recipes of the whole menu, indexed.
#[derive(Default, Debug)]
pub struct Recipes {
    pub product: HashMap<i64, Qty>,
    pub variant: HashMap<i64, Qty>,
    pub option: HashMap<i64, Qty>,
}

fn add(into: &mut Qty, from: &Qty, times: i64) {
    for (ing, q) in from {
        *into.entry(*ing).or_insert(0) += q * times;
    }
}

fn collect(r: &Recipes, cfg: &ItemConfig, out: &mut Qty) {
    // A size with its own recipe replaces the product's recipe.
    let base = cfg.variant_id.and_then(|v| r.variant.get(&v)).or_else(|| r.product.get(&cfg.product_id));
    if let Some(b) = base {
        add(out, b, 1);
    }
    for m in &cfg.modifiers {
        if let Some(o) = r.option.get(&m.option_id) {
            add(out, o, m.qty.max(0));
        }
    }
    if let Some(children) = &cfg.combo {
        for c in children {
            collect(r, c, out);
        }
    }
}

/// What ONE unit of a configured item consumes. Removals ("No onion") can't take an
/// ingredient below zero.
pub fn consumption(r: &Recipes, cfg: &ItemConfig) -> Qty {
    let mut out = Qty::new();
    collect(r, cfg, &mut out);
    out.retain(|_, q| *q > 0);
    out
}

pub fn times(q: &Qty, n: i64) -> Qty {
    q.iter().map(|(k, v)| (*k, v * n)).filter(|(_, v)| *v != 0).collect()
}

/// Cost in milli-cents of a quantity map, given each ingredient's (unit, cost per unit).
pub fn cost_mc(q: &Qty, costs: &HashMap<i64, (String, i64)>) -> i128 {
    q.iter().map(|(ing, milli)| costs.get(ing).map(|(u, c)| value_mc(u, *c, *milli)).unwrap_or(0)).sum()
}

/// Margin in basis points of the price excluding VAT. 0 when the price is 0.
pub fn margin_bp(price_ht: i64, cost: i64) -> i64 {
    if price_ht <= 0 {
        0
    } else {
        div_round((price_ht - cost) * 10000, price_ht)
    }
}

/// Price excluding VAT.
pub fn price_ht(price: i64, rate_bp: i64, price_include: bool) -> i64 {
    if price_include {
        split_included(price, rate_bp).0
    } else {
        price
    }
}

/// Stock difference to record so the line's recorded consumption equals `want`.
/// Returns ingredient -> delta (negative = stock goes out).
pub fn diff_to(want: &Qty, have: &Qty) -> Qty {
    let mut out = Qty::new();
    for ing in want.keys().chain(have.keys()) {
        let d = have.get(ing).copied().unwrap_or(0) - want.get(ing).copied().unwrap_or(0);
        if d != 0 {
            out.insert(*ing, d);
        }
    }
    out
}

// ------------------------------------------------------------------ loading

impl Recipes {
    pub fn load(conn: &Connection) -> ApiResult<Self> {
        let mut r = Recipes::default();
        let mut stmt = conn.prepare("SELECT product_id, variant_id, option_id, ingredient_id, qty_milli FROM recipe_lines")?;
        let rows = stmt.query_map([], |x| {
            Ok((x.get::<_, Option<i64>>(0)?, x.get::<_, Option<i64>>(1)?, x.get::<_, Option<i64>>(2)?, x.get::<_, i64>(3)?, x.get::<_, i64>(4)?))
        })?;
        for row in rows {
            let (p, v, o, ing, q) = row?;
            let map = match (p, v, o) {
                (Some(id), _, _) => r.product.entry(id).or_default(),
                (_, Some(id), _) => r.variant.entry(id).or_default(),
                (_, _, Some(id)) => r.option.entry(id).or_default(),
                _ => continue,
            };
            *map.entry(ing).or_insert(0) += q;
        }
        Ok(r)
    }
}

fn costs(conn: &Connection) -> ApiResult<HashMap<i64, (String, i64)>> {
    let mut stmt = conn.prepare("SELECT id, unit, cost_cents FROM ingredients")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, String>(1)?, r.get::<_, i64>(2)?))))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn names_of(s: String) -> Value {
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

// ------------------------------------------------------------------ ledger

pub struct Move<'a> {
    pub ingredient_id: i64,
    pub delta_milli: i64,
    pub kind: &'a str,
    pub reason: &'a str,
    pub order_id: Option<i64>,
    pub order_line_id: Option<i64>,
    pub count_id: Option<i64>,
    pub product_id: Option<i64>,
    pub user_id: Option<i64>,
    pub po_id: Option<i64>,
    /// Value to record instead of the current cost (goods received at their real price).
    pub value_cents: Option<i64>,
}

impl<'a> Move<'a> {
    pub fn new(ingredient_id: i64, delta_milli: i64, kind: &'a str) -> Self {
        Self { ingredient_id, delta_milli, kind, reason: "", order_id: None, order_line_id: None, count_id: None, product_id: None, user_id: None, po_id: None, value_cents: None }
    }
}

/// Write one move and update the cached stock. Returns the value in cents.
pub fn record(conn: &Connection, m: &Move) -> ApiResult<i64> {
    if m.delta_milli == 0 {
        return Ok(0);
    }
    let (unit, cost): (String, i64) = conn
        .query_row("SELECT unit, cost_cents FROM ingredients WHERE id=?1", [m.ingredient_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| ApiError::not_found("ingredient"))?;
    let value = m.value_cents.unwrap_or_else(|| mc_to_cents(value_mc(&unit, cost, m.delta_milli)));
    conn.execute(
        "INSERT INTO stock_moves (ingredient_id, delta_milli, kind, reason, value_cents, order_id, order_line_id, count_id, product_id, user_id, po_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![m.ingredient_id, m.delta_milli, m.kind, m.reason, value, m.order_id, m.order_line_id, m.count_id, m.product_id, m.user_id, m.po_id],
    )?;
    conn.execute(
        "UPDATE ingredients SET stock_milli = stock_milli + ?2, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
        params![m.ingredient_id, m.delta_milli],
    )?;
    Ok(value)
}

fn recorded_for_line(conn: &Connection, line_id: i64) -> ApiResult<Qty> {
    let mut stmt = conn.prepare("SELECT ingredient_id, -SUM(delta_milli) FROM stock_moves WHERE order_line_id=?1 GROUP BY ingredient_id")?;
    let rows = stmt.query_map([line_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let mut q: Qty = rows.collect::<Result<_, _>>()?;
    q.retain(|_, v| *v != 0);
    Ok(q)
}

// ------------------------------------------------------------------ orders

fn flag(cfg: &Map<String, Value>, key: &str, default: bool) -> bool {
    cfg.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Bring the stock ledger in line with an order's current state. Idempotent: call it after
/// any change to the order (send, pay, edit, void, refund, split).
///
/// - A line uses stock once it is sent to the kitchen or the order is fully paid.
/// - Its consumption per unit is frozen the first time, so recipe edits never rewrite
///   past sales.
/// - A voided line or order gives its stock back when `stock_restore_on_void` is on (else
///   what was used stays used). A refund gives it back when `stock_restore_on_refund` is on.
pub fn sync_order(conn: &Connection, order_id: i64, user_id: Option<i64>) -> ApiResult<()> {
    let Some((status, payment_status, paid)): Option<(String, String, bool)> = conn
        .query_row("SELECT status, payment_status, paid_at IS NOT NULL FROM orders WHERE id=?1", [order_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?
    else {
        return Ok(());
    };
    let cfg = settings::get_all(conn)?;
    let restore_void = flag(&cfg, "stock_restore_on_void", true);
    let restore_refund = flag(&cfg, "stock_restore_on_refund", false);
    let order_voided = status == "voided";
    let refunded = payment_status == "refunded";

    struct L {
        id: i64,
        config: String,
        qty: i64,
        sent: bool,
        voided: bool,
        per_unit: Option<String>,
    }
    let lines: Vec<L> = conn
        .prepare("SELECT id, config, qty, kitchen_status <> 'new', voided, stock_per_unit FROM order_lines WHERE order_id=?1 ORDER BY id")?
        .query_map([order_id], |r| {
            Ok(L { id: r.get(0)?, config: r.get(1)?, qty: r.get(2)?, sent: r.get(3)?, voided: r.get(4)?, per_unit: r.get(5)? })
        })?
        .collect::<Result<_, _>>()?;

    let mut recipes: Option<Recipes> = None;
    let mut changed = false;
    for l in &lines {
        let cancelled = l.voided || order_voided;
        let uses = l.sent || paid;
        let want: Qty = if cancelled {
            if !restore_void {
                continue; // used stock stays used
            }
            Qty::new()
        } else if refunded {
            if !restore_refund {
                continue;
            }
            Qty::new()
        } else if uses {
            let per_unit: Qty = match &l.per_unit {
                Some(s) => serde_json::from_str::<BTreeMap<String, i64>>(s)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(k, v)| k.parse().ok().map(|k| (k, v)))
                    .collect(),
                None => {
                    if recipes.is_none() {
                        recipes = Some(Recipes::load(conn)?);
                    }
                    let q = match serde_json::from_str::<ItemConfig>(&l.config) {
                        Ok(c) => consumption(recipes.as_ref().unwrap(), &c),
                        Err(_) => Qty::new(),
                    };
                    let snap: BTreeMap<String, i64> = q.iter().map(|(k, v)| (k.to_string(), *v)).collect();
                    conn.execute("UPDATE order_lines SET stock_per_unit=?2 WHERE id=?1", params![l.id, json!(snap).to_string()])?;
                    q
                }
            };
            times(&per_unit, l.qty)
        } else {
            Qty::new()
        };
        let have = recorded_for_line(conn, l.id)?;
        let kind = if cancelled || refunded { "return" } else { "sale" };
        for (ing, delta) in diff_to(&want, &have) {
            let mut m = Move::new(ing, delta, kind);
            m.order_id = Some(order_id);
            m.order_line_id = Some(l.id);
            m.user_id = user_id;
            record(conn, &m)?;
            changed = true;
        }
    }

    // Lines deleted after they used stock (an unsent line removed from a paid order).
    let orphans: Vec<(i64, i64, i64)> = conn
        .prepare(
            "SELECT order_line_id, ingredient_id, SUM(delta_milli) FROM stock_moves
             WHERE order_id=?1 AND order_line_id IS NOT NULL AND order_line_id NOT IN (SELECT id FROM order_lines)
             GROUP BY order_line_id, ingredient_id HAVING SUM(delta_milli) <> 0",
        )?
        .query_map([order_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for (line, ing, sum) in orphans {
        let mut m = Move::new(ing, -sum, "return");
        m.order_id = Some(order_id);
        m.order_line_id = Some(line);
        m.user_id = user_id;
        record(conn, &m)?;
        changed = true;
    }
    if changed {
        refresh_86(conn, user_id)?;
    }
    Ok(())
}

/// Mark products unavailable when an ingredient of their recipe is out (setting
/// `auto_86_on_stockout`), and bring them back once every ingredient is in stock again.
/// Products 86'd by hand are never touched. Returns how many products changed.
pub fn refresh_86(conn: &Connection, user_id: Option<i64>) -> ApiResult<usize> {
    let on = flag(&settings::get_all(conn)?, "auto_86_on_stockout", false);
    let out: HashSet<i64> = conn
        .prepare("SELECT id FROM ingredients WHERE active=1 AND stock_milli <= 0")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut uses: HashMap<i64, HashSet<i64>> = HashMap::new();
    for row in conn
        .prepare(
            "SELECT COALESCE(rl.product_id, v.product_id), rl.ingredient_id FROM recipe_lines rl
             LEFT JOIN product_variants v ON v.id = rl.variant_id
             WHERE rl.option_id IS NULL AND rl.qty_milli > 0 AND (rl.product_id IS NOT NULL OR v.active = 1)",
        )?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
    {
        let (p, i) = row?;
        uses.entry(p).or_default().insert(i);
    }
    let products: Vec<(i64, bool, bool)> = conn
        .prepare("SELECT id, available, stock_out FROM products WHERE kind='simple'")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut n = 0;
    for (pid, available, stock_out) in products {
        let short = on && uses.get(&pid).map(|s| s.iter().any(|i| out.contains(i))).unwrap_or(false);
        if short && available && !stock_out {
            conn.execute("UPDATE products SET available=0, stock_out=1, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", [pid])?;
            audit::log(conn, user_id, "product.stock_out", Some("product"), Some(&pid.to_string()), None)?;
            n += 1;
        } else if !short && stock_out {
            conn.execute("UPDATE products SET available=1, stock_out=0, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", [pid])?;
            audit::log(conn, user_id, "product.back_in_stock", Some("product"), Some(&pid.to_string()), Some(json!({"auto": true})))?;
            n += 1;
        }
    }
    if n > 0 {
        crate::net::publish("menu");
    }
    Ok(n)
}

// ------------------------------------------------------------------ ingredients

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct IngredientIn {
    pub id: Option<i64>,
    pub names: Value,
    pub unit: String,
    #[serde(default)]
    pub cost_cents: i64,
    #[serde(default)]
    pub min_milli: i64,
    #[serde(default = "yes")]
    pub active: bool,
    /// New ingredient only: stock on hand.
    #[serde(default)]
    pub initial_milli: i64,
}

pub fn save_ingredient(conn: &Connection, me: &SessionUser, i: IngredientIn) -> ApiResult<i64> {
    let names = menu::check_names(&i.names, "ingredient name")?;
    if !UNITS.contains(&i.unit.as_str()) {
        return Err(ApiError::invalid("unit must be g, ml or piece"));
    }
    if !(0..=10_000_000).contains(&i.cost_cents) {
        return Err(ApiError::invalid("cost out of range"));
    }
    if !(0..=MAX_QTY).contains(&i.min_milli) || !(0..=MAX_QTY).contains(&i.initial_milli) {
        return Err(ApiError::invalid("quantity out of range"));
    }
    let tx = atomic(conn)?;
    let id = match i.id {
        None => {
            tx.execute(
                "INSERT INTO ingredients (names, unit, cost_cents, min_milli, active, sort)
                 VALUES (?1,?2,?3,?4,?5,(SELECT COALESCE(MAX(sort),0)+1 FROM ingredients))",
                params![names, i.unit, i.cost_cents, i.min_milli, i.active],
            )?;
            let id = tx.last_insert_rowid();
            if i.initial_milli > 0 {
                let mut m = Move::new(id, i.initial_milli, "initial");
                m.user_id = Some(me.id);
                record(&tx, &m)?;
            }
            audit::log(&tx, Some(me.id), "ingredient.created", Some("ingredient"), Some(&id.to_string()),
                Some(json!({"names": names_of(names), "unit": i.unit, "cost_cents": i.cost_cents})))?;
            id
        }
        Some(id) => {
            let (unit, cost): (String, i64) = tx
                .query_row("SELECT unit, cost_cents FROM ingredients WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
                .ok_or_else(|| ApiError::not_found("ingredient"))?;
            if unit != i.unit {
                let used: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM recipe_lines WHERE ingredient_id=?1) OR EXISTS(SELECT 1 FROM stock_moves WHERE ingredient_id=?1)",
                    [id],
                    |r| r.get(0),
                )?;
                if used {
                    return Err(ApiError::new("unit_locked", "the unit can't change once the ingredient is used"));
                }
            }
            tx.execute(
                "UPDATE ingredients SET names=?2, unit=?3, cost_cents=?4, min_milli=?5, active=?6,
                   updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![id, names, i.unit, i.cost_cents, i.min_milli, i.active],
            )?;
            audit::log(&tx, Some(me.id), "ingredient.updated", Some("ingredient"), Some(&id.to_string()),
                Some(json!({"names": names_of(names), "cost_before": cost, "cost_after": i.cost_cents, "active": i.active})))?;
            id
        }
    };
    refresh_86(&tx, Some(me.id))?;
    tx.commit()?;
    Ok(id)
}

pub fn list_ingredients(conn: &Connection) -> ApiResult<Value> {
    let rows = conn
        .prepare(
            "SELECT i.id, i.names, i.unit, i.cost_cents, i.stock_milli, i.min_milli, i.active,
               (SELECT COUNT(*) FROM recipe_lines WHERE ingredient_id=i.id)
             FROM ingredients i ORDER BY i.active DESC, i.sort, i.id",
        )?
        .query_map([], |r| {
            let unit: String = r.get(2)?;
            let cost: i64 = r.get(3)?;
            let stock: i64 = r.get(4)?;
            let min: i64 = r.get(5)?;
            let used: i64 = r.get(7)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "unit": unit, "cost_cents": cost,
                "stock_milli": stock, "min_milli": min, "active": r.get::<_, bool>(6)?, "recipe_count": used,
                "low": min > 0 && stock <= min, "out": stock <= 0 && used > 0,
                "value_cents": mc_to_cents(value_mc(&unit, cost, stock.max(0))),
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

/// Low (at or under the minimum) and out-of-stock ingredients that are active.
pub fn alerts(conn: &Connection) -> ApiResult<Value> {
    let all = list_ingredients(conn)?;
    let rows: Vec<Value> = all
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["active"] == true && (i["low"] == true || i["out"] == true))
        .cloned()
        .collect();
    Ok(json!(rows))
}

// ------------------------------------------------------------------ recipes

#[derive(Deserialize)]
pub struct RecipeLineIn {
    pub ingredient_id: i64,
    pub qty_milli: i64,
}

#[derive(Deserialize)]
pub struct RecipeIn {
    /// "product", "variant" or "option"
    pub target: String,
    pub id: i64,
    pub lines: Vec<RecipeLineIn>,
}

pub fn recipes_list(conn: &Connection) -> ApiResult<Value> {
    let r = Recipes::load(conn)?;
    let conv = |m: &HashMap<i64, Qty>| -> Value {
        let mut o = Map::new();
        for (k, q) in m {
            o.insert(k.to_string(), json!(q.iter().map(|(i, v)| json!({"ingredient_id": i, "qty_milli": v})).collect::<Vec<_>>()));
        }
        Value::Object(o)
    };
    Ok(json!({"product": conv(&r.product), "variant": conv(&r.variant), "option": conv(&r.option)}))
}

pub fn save_recipe(conn: &Connection, me: &SessionUser, rin: RecipeIn) -> ApiResult<()> {
    let (col, table) = match rin.target.as_str() {
        "product" => ("product_id", "products"),
        "variant" => ("variant_id", "product_variants"),
        "option" => ("option_id", "modifier_options"),
        _ => return Err(ApiError::invalid("target must be product, variant or option")),
    };
    let tx = atomic(conn)?;
    if tx.query_row(&format!("SELECT 1 FROM {table} WHERE id=?1"), [rin.id], |_| Ok(())).optional()?.is_none() {
        return Err(ApiError::not_found(table));
    }
    if rin.lines.len() > 60 {
        return Err(ApiError::invalid("too many recipe lines"));
    }
    let mut seen = HashSet::new();
    for l in &rin.lines {
        if !seen.insert(l.ingredient_id) {
            return Err(ApiError::invalid("an ingredient is listed twice"));
        }
        let ok = if rin.target == "option" { l.qty_milli != 0 } else { l.qty_milli > 0 };
        if !ok || l.qty_milli.abs() > MAX_QTY {
            return Err(ApiError::invalid("bad recipe quantity"));
        }
        if tx.query_row("SELECT 1 FROM ingredients WHERE id=?1", [l.ingredient_id], |_| Ok(())).optional()?.is_none() {
            return Err(ApiError::not_found("ingredient"));
        }
    }
    tx.execute(&format!("DELETE FROM recipe_lines WHERE {col}=?1"), [rin.id])?;
    for l in &rin.lines {
        tx.execute(
            &format!("INSERT INTO recipe_lines ({col}, ingredient_id, qty_milli) VALUES (?1,?2,?3)"),
            params![rin.id, l.ingredient_id, l.qty_milli],
        )?;
    }
    audit::log(&tx, Some(me.id), "recipe.changed", Some(&rin.target), Some(&rin.id.to_string()),
        Some(json!({"lines": rin.lines.iter().map(|l| json!([l.ingredient_id, l.qty_milli])).collect::<Vec<_>>()})))?;
    refresh_86(&tx, Some(me.id))?;
    tx.commit()?;
    Ok(())
}

/// Food cost and margin of every active product (each size), and of each combo with its
/// default choices. Prices compared excluding VAT.
pub fn food_cost(conn: &Connection) -> ApiResult<Value> {
    let r = Recipes::load(conn)?;
    let c = costs(conn)?;
    let taxes: HashMap<i64, (i64, bool)> = conn
        .prepare("SELECT id, rate_bp, price_include FROM taxes")?
        .query_map([], |x| Ok((x.get::<_, i64>(0)?, (x.get::<_, i64>(1)?, x.get::<_, bool>(2)?))))?
        .collect::<Result<_, _>>()?;
    let m = menu::get_menu(conn, false)?;
    let mut out = Vec::new();
    for p in &m.products {
        let pid = p["id"].as_i64().unwrap();
        let (rate, incl) = taxes.get(&p["tax_id"].as_i64().unwrap_or(0)).copied().unwrap_or((0, true));
        let mut row = |variant: Option<&Value>, cfg: ItemConfig, price: i64| {
            let q = consumption(&r, &cfg);
            let cost = mc_to_cents(cost_mc(&q, &c));
            let ht = price_ht(price, rate, incl);
            out.push(json!({
                "product_id": pid, "kind": p["kind"], "category_id": p["category_id"], "names": p["names"],
                "variant_id": variant.map(|v| v["id"].clone()), "variant_names": variant.map(|v| v["names"].clone()),
                "price_cents": price, "price_ht_cents": ht, "cost_cents": cost, "margin_bp": margin_bp(ht, cost),
                "has_recipe": !q.is_empty(),
            }));
        };
        let base = |variant_id: Option<i64>| ItemConfig { product_id: pid, variant_id, modifiers: vec![], combo: None };
        if p["kind"] == "combo" {
            let children: Vec<ItemConfig> = p["slots"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|s| s["options"].as_array().unwrap().iter().find(|o| o["is_default"] == true).or(s["options"].get(0)).cloned())
                .map(|o| ItemConfig { product_id: o["product_id"].as_i64().unwrap(), variant_id: o["variant_id"].as_i64(), modifiers: vec![], combo: None })
                .collect();
            let mut cfg = base(None);
            cfg.combo = Some(children);
            row(None, cfg, p["price_cents"].as_i64().unwrap());
            continue;
        }
        let variants = p["variants"].as_array().unwrap();
        if variants.is_empty() {
            row(None, base(None), p["price_cents"].as_i64().unwrap());
        } else {
            for v in variants {
                row(Some(v), base(v["id"].as_i64()), v["price_cents"].as_i64().unwrap());
            }
        }
    }
    Ok(json!(out))
}

// ------------------------------------------------------------------ waste, adjustments

fn check_reason(reason: &str) -> ApiResult<&str> {
    let r = reason.trim();
    if r.is_empty() {
        return Err(ApiError::new("reason_required", "a reason is required"));
    }
    if r.chars().count() > 200 {
        return Err(ApiError::invalid("reason too long"));
    }
    Ok(r)
}

pub fn adjust(conn: &Connection, me: &SessionUser, ingredient_id: i64, delta_milli: i64, reason: &str) -> ApiResult<i64> {
    let reason = check_reason(reason)?;
    if delta_milli == 0 || delta_milli.abs() > MAX_QTY {
        return Err(ApiError::invalid("bad quantity"));
    }
    let tx = atomic(conn)?;
    let mut m = Move::new(ingredient_id, delta_milli, "adjust");
    m.reason = reason;
    m.user_id = Some(me.id);
    let value = record(&tx, &m)?;
    audit::log(&tx, Some(me.id), "stock.adjusted", Some("ingredient"), Some(&ingredient_id.to_string()),
        Some(json!({"delta_milli": delta_milli, "value_cents": value, "reason": reason})))?;
    refresh_86(&tx, Some(me.id))?;
    tx.commit()?;
    Ok(value)
}

#[derive(Deserialize)]
pub struct WasteIn {
    pub ingredient_id: Option<i64>,
    pub qty_milli: Option<i64>,
    pub product_id: Option<i64>,
    pub variant_id: Option<i64>,
    pub qty: Option<i64>,
    pub reason: String,
}

/// Waste/loss of an ingredient, or of finished products (their recipe is taken out).
/// Returns the value lost in cents (positive).
pub fn waste(conn: &Connection, me: &SessionUser, w: WasteIn) -> ApiResult<i64> {
    let reason = check_reason(&w.reason)?;
    let tx = atomic(conn)?;
    let (q, product): (Qty, Option<i64>) = match (w.ingredient_id, w.product_id) {
        (Some(ing), None) => {
            let n = w.qty_milli.unwrap_or(0);
            if n <= 0 || n > MAX_QTY {
                return Err(ApiError::invalid("bad quantity"));
            }
            (Qty::from([(ing, n)]), None)
        }
        (None, Some(pid)) => {
            let n = w.qty.unwrap_or(0);
            if !(1..=999).contains(&n) {
                return Err(ApiError::invalid("quantity must be 1-999"));
            }
            if let Some(v) = w.variant_id {
                let owner: Option<i64> = tx.query_row("SELECT product_id FROM product_variants WHERE id=?1", [v], |r| r.get(0)).optional()?;
                if owner != Some(pid) {
                    return Err(ApiError::invalid("size does not belong to the product"));
                }
            }
            let cfg = ItemConfig { product_id: pid, variant_id: w.variant_id, modifiers: vec![], combo: None };
            let q = times(&consumption(&Recipes::load(&tx)?, &cfg), n);
            if q.is_empty() {
                return Err(ApiError::new("no_recipe", "this product has no recipe"));
            }
            (q, Some(pid))
        }
        _ => return Err(ApiError::invalid("give an ingredient or a product")),
    };
    let mut value = 0;
    for (ing, n) in &q {
        let mut m = Move::new(*ing, -n, "waste");
        m.reason = reason;
        m.product_id = product;
        m.user_id = Some(me.id);
        value += record(&tx, &m)?;
    }
    audit::log(&tx, Some(me.id), "stock.waste", product.map(|_| "product").or(Some("ingredient")),
        Some(&product.or(w.ingredient_id).unwrap().to_string()),
        Some(json!({"qty": w.qty, "qty_milli": w.qty_milli, "variant_id": w.variant_id, "value_cents": -value, "reason": reason})))?;
    refresh_86(&tx, Some(me.id))?;
    tx.commit()?;
    Ok(-value)
}

#[derive(Deserialize, Default)]
pub struct HistoryIn {
    pub ingredient_id: Option<i64>,
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

pub fn history(conn: &Connection, h: HistoryIn) -> ApiResult<Value> {
    let limit = h.limit.unwrap_or(100).clamp(1, 500);
    let offset = h.offset.unwrap_or(0).max(0);
    let rows = conn
        .prepare(
            "SELECT m.id, m.at, m.ingredient_id, i.names, i.unit, m.delta_milli, m.kind, m.reason, m.value_cents,
               m.order_id, o.number, m.count_id, m.product_id, p.names, u.name, po.number
             FROM stock_moves m JOIN ingredients i ON i.id = m.ingredient_id
             LEFT JOIN purchase_orders po ON po.id = m.po_id
             LEFT JOIN orders o ON o.id = m.order_id
             LEFT JOIN products p ON p.id = m.product_id
             LEFT JOIN users u ON u.id = m.user_id
             WHERE (?1 IS NULL OR m.ingredient_id = ?1) AND (?2 IS NULL OR m.kind = ?2)
             ORDER BY m.id DESC LIMIT ?3 OFFSET ?4",
        )?
        .query_map(params![h.ingredient_id, h.kind, limit, offset], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "at": r.get::<_, String>(1)?, "ingredient_id": r.get::<_, i64>(2)?,
                "ingredient_names": names_of(r.get(3)?), "unit": r.get::<_, String>(4)?, "delta_milli": r.get::<_, i64>(5)?,
                "kind": r.get::<_, String>(6)?, "reason": r.get::<_, String>(7)?, "value_cents": r.get::<_, i64>(8)?,
                "order_id": r.get::<_, Option<i64>>(9)?, "order_number": r.get::<_, Option<i64>>(10)?,
                "count_id": r.get::<_, Option<i64>>(11)?, "product_id": r.get::<_, Option<i64>>(12)?,
                "product_names": r.get::<_, Option<String>>(13)?.map(names_of), "user_name": r.get::<_, Option<String>>(14)?, "po_number": r.get::<_, Option<i64>>(15)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

// ------------------------------------------------------------------ stock counts

pub fn counts_list(conn: &Connection) -> ApiResult<Value> {
    let rows = conn
        .prepare(
            "SELECT c.id, c.status, c.note, c.created_at, c.closed_at, u.name,
               (SELECT COUNT(*) FROM stock_count_lines WHERE count_id=c.id AND counted_milli IS NOT NULL),
               (SELECT COUNT(*) FROM stock_count_lines WHERE count_id=c.id),
               (SELECT COALESCE(SUM(value_cents),0) FROM stock_moves WHERE count_id=c.id)
             FROM stock_counts c JOIN users u ON u.id=c.created_by ORDER BY c.id DESC LIMIT 50",
        )?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "status": r.get::<_, String>(1)?, "note": r.get::<_, String>(2)?,
                      "created_at": r.get::<_, String>(3)?, "closed_at": r.get::<_, Option<String>>(4)?,
                      "created_by": r.get::<_, String>(5)?, "counted": r.get::<_, i64>(6)?, "lines": r.get::<_, i64>(7)?,
                      "value_cents": r.get::<_, i64>(8)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

pub fn count_get(conn: &Connection, id: i64) -> ApiResult<Value> {
    let (status, note, created_at, closed_at): (String, String, String, Option<String>) = conn
        .query_row("SELECT status, note, created_at, closed_at FROM stock_counts WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .optional()?
        .ok_or_else(|| ApiError::not_found("count"))?;
    let lines = conn
        .prepare(
            "SELECT l.ingredient_id, i.names, i.unit, i.cost_cents, i.stock_milli, l.counted_milli, l.expected_milli
             FROM stock_count_lines l JOIN ingredients i ON i.id = l.ingredient_id WHERE l.count_id=?1 ORDER BY i.sort, i.id",
        )?
        .query_map([id], |r| {
            Ok(json!({"ingredient_id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "unit": r.get::<_, String>(2)?,
                      "cost_cents": r.get::<_, i64>(3)?, "stock_milli": r.get::<_, i64>(4)?,
                      "counted_milli": r.get::<_, Option<i64>>(5)?, "expected_milli": r.get::<_, Option<i64>>(6)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"id": id, "status": status, "note": note, "created_at": created_at, "closed_at": closed_at, "lines": lines}))
}

pub fn count_start(conn: &Connection, me: &SessionUser, note: &str) -> ApiResult<i64> {
    if note.chars().count() > 200 {
        return Err(ApiError::invalid("note too long"));
    }
    let tx = atomic(conn)?;
    if tx.query_row("SELECT 1 FROM stock_counts WHERE status='open'", [], |_| Ok(())).optional()?.is_some() {
        return Err(ApiError::new("count_open", "a stock count is already open"));
    }
    tx.execute("INSERT INTO stock_counts (note, created_by) VALUES (?1, ?2)", params![note.trim(), me.id])?;
    let id = tx.last_insert_rowid();
    tx.execute("INSERT INTO stock_count_lines (count_id, ingredient_id) SELECT ?1, id FROM ingredients WHERE active=1", [id])?;
    audit::log(&tx, Some(me.id), "stock.count_started", Some("count"), Some(&id.to_string()), None)?;
    tx.commit()?;
    Ok(id)
}

fn open_count(conn: &Connection, id: i64) -> ApiResult<()> {
    let status: String = conn
        .query_row("SELECT status FROM stock_counts WHERE id=?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| ApiError::not_found("count"))?;
    if status != "open" {
        return Err(ApiError::new("count_closed", "this count is closed"));
    }
    Ok(())
}

pub fn count_set(conn: &Connection, id: i64, ingredient_id: i64, counted_milli: Option<i64>) -> ApiResult<()> {
    open_count(conn, id)?;
    if let Some(n) = counted_milli {
        if !(0..=MAX_QTY).contains(&n) {
            return Err(ApiError::invalid("bad quantity"));
        }
    }
    let n = conn.execute(
        "UPDATE stock_count_lines SET counted_milli=?3 WHERE count_id=?1 AND ingredient_id=?2",
        params![id, ingredient_id, counted_milli],
    )?;
    if n == 0 {
        // Ingredient created after the count started.
        conn.execute(
            "INSERT INTO stock_count_lines (count_id, ingredient_id, counted_milli)
             SELECT ?1, id, ?3 FROM ingredients WHERE id=?2",
            params![id, ingredient_id, counted_milli],
        )?;
    }
    Ok(())
}

/// Set every counted ingredient to its counted quantity. Uncounted lines are left alone.
pub fn count_validate(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<Value> {
    let tx = atomic(conn)?;
    open_count(&tx, id)?;
    let lines: Vec<(i64, i64, i64)> = tx
        .prepare(
            "SELECT l.ingredient_id, l.counted_milli, i.stock_milli FROM stock_count_lines l
             JOIN ingredients i ON i.id = l.ingredient_id WHERE l.count_id=?1 AND l.counted_milli IS NOT NULL",
        )?
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut value = 0;
    let mut changed = 0;
    for (ing, counted, stock) in &lines {
        tx.execute("UPDATE stock_count_lines SET expected_milli=?3 WHERE count_id=?1 AND ingredient_id=?2", params![id, ing, stock])?;
        let delta = counted - stock;
        if delta != 0 {
            let mut m = Move::new(*ing, delta, "count");
            m.count_id = Some(id);
            m.user_id = Some(me.id);
            value += record(&tx, &m)?;
            changed += 1;
        }
    }
    tx.execute("UPDATE stock_counts SET status='done', closed_by=?2, closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", params![id, me.id])?;
    audit::log(&tx, Some(me.id), "stock.count_validated", Some("count"), Some(&id.to_string()),
        Some(json!({"counted": lines.len(), "changed": changed, "value_cents": value})))?;
    refresh_86(&tx, Some(me.id))?;
    tx.commit()?;
    Ok(json!({"counted": lines.len(), "changed": changed, "value_cents": value}))
}

pub fn count_cancel(conn: &Connection, me: &SessionUser, id: i64) -> ApiResult<()> {
    open_count(conn, id)?;
    conn.execute("UPDATE stock_counts SET status='cancelled', closed_by=?2, closed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", params![id, me.id])?;
    audit::log(conn, Some(me.id), "stock.count_cancelled", Some("count"), Some(&id.to_string()), None)?;
    Ok(())
}

// ------------------------------------------------------------------ demo data

fn n3(fr: &str, ar: &str, en: &str) -> Value {
    json!({"fr": fr, "ar": ar, "en": en})
}

/// Demo ingredients and recipes for the demo menu (products matched by their French name).
/// Does nothing when ingredients already exist. Products missing from the menu are skipped.
pub fn seed_demo(conn: &Connection, user_id: i64) -> ApiResult<bool> {
    let any: i64 = conn.query_row("SELECT COUNT(*) FROM ingredients", [], |r| r.get(0))?;
    if any > 0 {
        return Ok(false);
    }
    let tx = atomic(conn)?;
    // (key, names, unit, cost per kg/L/piece in cents, stock, minimum) - stock and minimum in units.
    #[rustfmt::skip]
    let ings: &[(&str, Value, &str, i64, i64, i64)] = &[
        ("bun", n3("Pain burger", "خبز البرغر", "Burger bun"), "piece", 250, 200, 40),
        ("beef", n3("Viande hachée de bœuf", "لحم بقري مفروم", "Ground beef"), "g", 9000, 20000, 5000),
        ("kefta", n3("Kefta", "كفتة", "Kefta"), "g", 9500, 5000, 1500),
        ("chicken", n3("Filet de poulet", "فيليه دجاج", "Chicken fillet"), "g", 6500, 8000, 2000),
        ("crispy", n3("Poulet pané", "دجاج مقرمش", "Breaded chicken"), "piece", 600, 80, 20),
        ("veggie", n3("Galette végétale", "قرص نباتي", "Veggie patty"), "piece", 900, 30, 10),
        ("cheddar", n3("Cheddar (tranche)", "شيدر (شريحة)", "Cheddar slice"), "piece", 120, 300, 60),
        ("swiss", n3("Emmental (tranche)", "إمنتال (شريحة)", "Swiss slice"), "piece", 150, 100, 20),
        ("bacon", n3("Bacon de bœuf", "لحم مقدد بقري", "Beef bacon"), "g", 18000, 2000, 500),
        ("egg", n3("Œuf", "بيض", "Egg"), "piece", 150, 60, 12),
        ("onion", n3("Oignon", "بصل", "Onion"), "g", 800, 5000, 1000),
        ("tomato", n3("Tomate", "طماطم", "Tomato"), "g", 900, 5000, 1000),
        ("lettuce", n3("Salade", "خس", "Lettuce"), "g", 1500, 2000, 500),
        ("pickles", n3("Cornichons", "مخلل", "Pickles"), "g", 3000, 2000, 400),
        ("mushroom", n3("Champignons", "فطر", "Mushrooms"), "g", 4000, 2000, 500),
        ("jalapeno", n3("Jalapeños", "هالبينو", "Jalapeños"), "g", 6000, 1000, 200),
        ("avocado", n3("Avocat", "أفوكادو", "Avocado"), "g", 3500, 2000, 500),
        ("burger_sauce", n3("Sauce burger", "صلصة البرغر", "Burger sauce"), "ml", 4000, 5000, 1000),
        ("bbq", n3("Sauce barbecue", "صلصة باربكيو", "Barbecue sauce"), "ml", 4500, 3000, 500),
        ("ketchup", n3("Ketchup", "كاتشب", "Ketchup"), "ml", 2500, 5000, 1000),
        ("mayo", n3("Mayonnaise", "مايونيز", "Mayonnaise"), "ml", 3000, 5000, 1000),
        ("algerian", n3("Sauce algérienne", "صلصة جزائرية", "Algerian sauce"), "ml", 3500, 3000, 500),
        ("harissa", n3("Harissa", "هريسة", "Harissa"), "ml", 3000, 2000, 300),
        ("samurai", n3("Sauce samouraï", "صلصة ساموراي", "Samurai sauce"), "ml", 3500, 2000, 300),
        ("cheddar_sauce", n3("Sauce cheddar", "صلصة شيدر", "Cheddar sauce"), "ml", 6000, 2000, 300),
        ("fries", n3("Frites surgelées", "بطاطس مجمدة", "Frozen fries"), "g", 1800, 25000, 5000),
        ("wedges", n3("Potatoes surgelées", "بطاطس ودجز مجمدة", "Frozen wedges"), "g", 2600, 10000, 2000),
        ("rings", n3("Rings d'oignon surgelés", "حلقات بصل مجمدة", "Frozen onion rings"), "g", 4000, 5000, 1000),
        ("nuggets", n3("Nuggets", "ناغتس", "Nuggets"), "piece", 250, 200, 48),
        ("coleslaw", n3("Coleslaw maison", "كولسلو", "Coleslaw"), "g", 2000, 3000, 600),
        ("coca", n3("Canette Coca-Cola 33cl", "علبة كوكا كولا", "Coca-Cola can"), "piece", 450, 96, 24),
        ("zero", n3("Canette Coca-Cola Zero 33cl", "علبة كوكا زيرو", "Coke Zero can"), "piece", 450, 48, 12),
        ("fanta", n3("Canette Fanta 33cl", "علبة فانتا", "Fanta can"), "piece", 450, 48, 12),
        ("sprite", n3("Canette Sprite 33cl", "علبة سبرايت", "Sprite can"), "piece", 450, 48, 12),
        ("hawai", n3("Canette Hawaï 33cl", "علبة هاواي", "Hawai can"), "piece", 400, 48, 12),
        ("water", n3("Eau minérale 50cl", "ماء معدني", "Water bottle"), "piece", 250, 48, 12),
        ("orange", n3("Oranges", "برتقال", "Oranges"), "g", 800, 10000, 2000),
        ("milk", n3("Lait", "حليب", "Milk"), "ml", 900, 10000, 2000),
        ("ice_cream", n3("Glace vanille", "مثلجات فانيليا", "Vanilla ice cream"), "ml", 3000, 5000, 1000),
        ("syrup", n3("Sirop (chocolat, fraise)", "شراب منكه", "Flavour syrup"), "ml", 5000, 2000, 300),
        ("caramel", n3("Sauce caramel", "صلصة كراميل", "Caramel sauce"), "ml", 5000, 1000, 200),
        ("tiramisu", n3("Tiramisu (portion)", "تيراميسو", "Tiramisu portion"), "piece", 900, 20, 5),
        ("cookie", n3("Cookie", "كوكي", "Cookie"), "piece", 400, 40, 10),
        ("brownie", n3("Brownie (portion)", "براوني", "Brownie portion"), "piece", 600, 20, 5),
        ("box", n3("Boîte burger", "علبة برغر", "Burger box"), "piece", 80, 500, 100),
        ("cone", n3("Cornet frites", "علبة بطاطس", "Fries cone"), "piece", 40, 500, 100),
        ("cup", n3("Gobelet", "كوب", "Cup"), "piece", 60, 300, 50),
        ("bag", n3("Sachet", "كيس", "Bag"), "piece", 50, 300, 50),
        ("pot", n3("Pot à sauce", "علبة صلصة", "Sauce pot"), "piece", 20, 500, 100),
    ];
    let mut id = HashMap::new();
    for (sort, (key, names, unit, cost, stock, min)) in ings.iter().enumerate() {
        tx.execute(
            "INSERT INTO ingredients (names, unit, cost_cents, min_milli, sort) VALUES (?1,?2,?3,?4,?5)",
            params![names.to_string(), unit, cost, min * 1000, sort as i64 + 1],
        )?;
        let iid = tx.last_insert_rowid();
        let mut m = Move::new(iid, stock * 1000, "initial");
        m.user_id = Some(user_id);
        record(&tx, &m)?;
        id.insert(*key, iid);
    }

    let product = |fr: &str| -> ApiResult<Option<i64>> {
        Ok(tx.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')=?1 ORDER BY id LIMIT 1", [fr], |r| r.get(0)).optional()?)
    };
    let variant = |pid: i64, fr: &str| -> ApiResult<Option<i64>> {
        Ok(tx
            .query_row("SELECT id FROM product_variants WHERE product_id=?1 AND json_extract(names,'$.fr')=?2", params![pid, fr], |r| r.get(0))
            .optional()?)
    };
    let option = |group_fr: &str, fr: &str| -> ApiResult<Option<i64>> {
        Ok(tx
            .query_row(
                "SELECT o.id FROM modifier_options o JOIN modifier_groups g ON g.id=o.group_id
                 WHERE json_extract(g.names,'$.fr')=?1 AND json_extract(o.names,'$.fr')=?2",
                params![group_fr, fr],
                |r| r.get(0),
            )
            .optional()?)
    };
    // Quantities in units (g, ml, pieces); stored in milli-units.
    let put = |col: &str, target: i64, lines: &[(&str, f64)]| -> ApiResult<()> {
        for (k, q) in lines {
            tx.execute(
                &format!("INSERT INTO recipe_lines ({col}, ingredient_id, qty_milli) VALUES (?1,?2,?3)"),
                params![target, id[k], (q * 1000.0).round() as i64],
            )?;
        }
        Ok(())
    };

    // Burgers: "patty" lines are multiplied for Double / Triple sizes.
    type Lines = &'static [(&'static str, f64)];
    #[rustfmt::skip]
    let burgers: &[(&str, Lines, Lines)] = &[
        ("Classic Burger", &[("beef", 150.0)], &[("bun", 1.0), ("onion", 20.0), ("tomato", 30.0), ("lettuce", 15.0), ("pickles", 15.0), ("burger_sauce", 20.0), ("box", 1.0)]),
        ("Cheeseburger", &[("beef", 150.0), ("cheddar", 1.0)], &[("bun", 1.0), ("onion", 20.0), ("pickles", 15.0), ("ketchup", 10.0), ("burger_sauce", 15.0), ("box", 1.0)]),
        ("Bacon Cheese", &[("beef", 150.0), ("cheddar", 1.0)], &[("bun", 1.0), ("bacon", 30.0), ("onion", 20.0), ("burger_sauce", 20.0), ("box", 1.0)]),
        ("BBQ Smoky", &[("beef", 150.0), ("cheddar", 1.0)], &[("bun", 1.0), ("onion", 30.0), ("bbq", 25.0), ("box", 1.0)]),
        ("Spicy Jalapeño", &[("beef", 150.0), ("cheddar", 1.0)], &[("bun", 1.0), ("jalapeno", 20.0), ("tomato", 30.0), ("lettuce", 15.0), ("samurai", 20.0), ("box", 1.0)]),
        ("Mushroom Swiss", &[("beef", 150.0), ("swiss", 1.0)], &[("bun", 1.0), ("mushroom", 50.0), ("burger_sauce", 20.0), ("box", 1.0)]),
        ("Kefta Burger", &[("kefta", 150.0)], &[("bun", 1.0), ("onion", 20.0), ("tomato", 30.0), ("harissa", 15.0), ("box", 1.0)]),
        ("Crispy Chicken", &[("crispy", 1.0)], &[("bun", 1.0), ("lettuce", 20.0), ("mayo", 20.0), ("box", 1.0)]),
        ("Grilled Chicken", &[("chicken", 140.0)], &[("bun", 1.0), ("lettuce", 20.0), ("tomato", 30.0), ("burger_sauce", 20.0), ("box", 1.0)]),
        ("Veggie Burger", &[("veggie", 1.0)], &[("bun", 1.0), ("lettuce", 20.0), ("tomato", 30.0), ("avocado", 40.0), ("burger_sauce", 20.0), ("box", 1.0)]),
    ];
    for (fr, patty, rest) in burgers {
        let Some(pid) = product(*fr)? else { continue };
        let scaled = |n: f64| -> Vec<(&str, f64)> { patty.iter().map(|(k, q)| (*k, q * n)).chain(rest.iter().copied()).collect() };
        put("product_id", pid, &scaled(1.0))?;
        for (size, n) in [("Simple", 1.0), ("Double", 2.0), ("Triple", 3.0)] {
            if let Some(v) = variant(pid, size)? {
                put("variant_id", v, &scaled(n))?;
            }
        }
    }
    #[rustfmt::skip]
    let simple: &[(&str, Lines)] = &[
        ("Potatoes", &[("wedges", 150.0), ("cone", 1.0)]),
        ("Onion rings", &[("rings", 120.0), ("cone", 1.0)]),
        ("Nuggets x6", &[("nuggets", 6.0), ("box", 1.0)]),
        ("Salade coleslaw", &[("coleslaw", 120.0), ("pot", 1.0)]),
        ("Coca-Cola 33cl", &[("coca", 1.0)]),
        ("Coca-Cola Zero 33cl", &[("zero", 1.0)]),
        ("Fanta 33cl", &[("fanta", 1.0)]),
        ("Sprite 33cl", &[("sprite", 1.0)]),
        ("Hawaï 33cl", &[("hawai", 1.0)]),
        ("Eau minérale 50cl", &[("water", 1.0)]),
        ("Jus d'orange frais", &[("orange", 400.0), ("cup", 1.0)]),
        ("Milkshake", &[("milk", 200.0), ("ice_cream", 100.0), ("cup", 1.0)]),
        ("Tiramisu", &[("tiramisu", 1.0)]),
        ("Cookie", &[("cookie", 1.0)]),
        ("Brownie", &[("brownie", 1.0)]),
        ("Sundae caramel", &[("ice_cream", 120.0), ("caramel", 20.0), ("cup", 1.0)]),
        ("Ketchup", &[("ketchup", 30.0), ("pot", 1.0)]),
        ("Mayonnaise", &[("mayo", 30.0), ("pot", 1.0)]),
        ("Algérienne", &[("algerian", 30.0), ("pot", 1.0)]),
        ("Barbecue", &[("bbq", 30.0), ("pot", 1.0)]),
        ("Cheddar fondu", &[("cheddar_sauce", 50.0), ("pot", 1.0)]),
        ("Menu Classic", &[("bag", 1.0)]),
        ("Menu Cheese", &[("bag", 1.0)]),
        ("Menu Chicken", &[("bag", 1.0)]),
        ("Menu Signature", &[("bag", 1.0)]),
    ];
    for (fr, lines) in simple {
        if let Some(pid) = product(*fr)? {
            put("product_id", pid, *lines)?;
        }
    }
    if let Some(f) = product("Frites")? {
        for (size, g) in [("Petite", 100.0), ("Moyenne", 150.0), ("Grande", 200.0)] {
            if let Some(v) = variant(f, size)? {
                put("variant_id", v, &[("fries", g), ("cone", 1.0)])?;
            }
        }
    }
    #[rustfmt::skip]
    let options: &[(&str, &str, Lines)] = &[
        ("Suppléments", "Fromage", &[("cheddar", 1.0)]),
        ("Suppléments", "Bacon de bœuf", &[("bacon", 30.0)]),
        ("Suppléments", "Œuf", &[("egg", 1.0)]),
        ("Suppléments", "Jalapeños", &[("jalapeno", 20.0)]),
        ("Suppléments", "Oignons caramélisés", &[("onion", 40.0)]),
        ("Suppléments", "Avocat", &[("avocado", 40.0)]),
        ("Retirer", "Sans oignon", &[("onion", -20.0)]),
        ("Retirer", "Sans sauce", &[("burger_sauce", -20.0)]),
        ("Retirer", "Sans cornichons", &[("pickles", -15.0)]),
        ("Retirer", "Sans tomate", &[("tomato", -30.0)]),
        ("Retirer", "Sans salade", &[("lettuce", -15.0)]),
        ("Retirer", "Sans fromage", &[("cheddar", -1.0)]),
        ("Sauces offertes", "Ketchup", &[("ketchup", 20.0), ("pot", 1.0)]),
        ("Sauces offertes", "Mayonnaise", &[("mayo", 20.0), ("pot", 1.0)]),
        ("Sauces offertes", "Algérienne", &[("algerian", 20.0), ("pot", 1.0)]),
        ("Sauces offertes", "Harissa", &[("harissa", 20.0), ("pot", 1.0)]),
        ("Sauces offertes", "Barbecue", &[("bbq", 20.0), ("pot", 1.0)]),
        ("Sauces offertes", "Samouraï", &[("samurai", 20.0), ("pot", 1.0)]),
        ("Taille du menu", "Maxi", &[("fries", 50.0)]),
        ("Parfum", "Chocolat", &[("syrup", 20.0)]),
        ("Parfum", "Fraise", &[("syrup", 20.0)]),
    ];
    for (g, o, lines) in options {
        if let Some(oid) = option(*g, *o)? {
            put("option_id", oid, *lines)?;
        }
    }
    crate::purchasing::seed_demo(&tx)?;
    audit::log(&tx, Some(user_id), "stock.seed_demo", None, None, Some(json!({"ingredients": ings.len()})))?;
    tx.commit()?;
    Ok(true)
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ModSel;
    use crate::orders::tests::{demo, simple_line};
    use crate::orders::{self, NewLine, PayIn, SubmitIn};

    fn q(pairs: &[(i64, i64)]) -> Qty {
        pairs.iter().copied().collect()
    }

    fn cfg(pid: i64, vid: Option<i64>, mods: &[(i64, i64)], combo: Option<Vec<ItemConfig>>) -> ItemConfig {
        ItemConfig { product_id: pid, variant_id: vid, modifiers: mods.iter().map(|(o, n)| ModSel { option_id: *o, qty: *n }).collect(), combo }
    }

    #[test]
    fn consumption_rules() {
        let mut r = Recipes::default();
        r.product.insert(1, q(&[(10, 1000), (11, 150_000), (12, 20_000)])); // bun, beef 150 g, onion 20 g
        r.variant.insert(5, q(&[(10, 1000), (11, 300_000), (12, 20_000)])); // Double
        r.option.insert(100, q(&[(13, 1000)])); // extra cheese
        r.option.insert(101, q(&[(12, -20_000)])); // no onion
        r.option.insert(102, q(&[(14, -1000)])); // no pickles - not in this recipe
        r.product.insert(2, q(&[(20, 1000)])); // combo bag
        r.product.insert(3, q(&[(21, 150_000)])); // fries
        assert_eq!(consumption(&r, &cfg(1, None, &[], None)), q(&[(10, 1000), (11, 150_000), (12, 20_000)]));
        // A size with a recipe replaces the product recipe; a size without one falls back.
        assert_eq!(consumption(&r, &cfg(1, Some(5), &[], None))[&11], 300_000);
        assert_eq!(consumption(&r, &cfg(1, Some(6), &[], None))[&11], 150_000);
        // Extras multiply by their quantity; removals subtract but never go below zero.
        let c = consumption(&r, &cfg(1, None, &[(100, 2), (101, 1), (102, 1)], None));
        assert_eq!(c.get(&13), Some(&2000));
        assert_eq!(c.get(&12), None, "no onion removes it");
        assert_eq!(c.get(&14), None, "removing something absent is ignored");
        // Combo: own recipe + every chosen part (with the parts' modifiers).
        let combo = cfg(2, None, &[], Some(vec![cfg(1, Some(5), &[(100, 1)], None), cfg(3, None, &[], None)]));
        assert_eq!(consumption(&r, &combo), q(&[(10, 1000), (11, 300_000), (12, 20_000), (13, 1000), (20, 1000), (21, 150_000)]));
        // No recipe -> nothing.
        assert!(consumption(&r, &cfg(9, None, &[], None)).is_empty());
    }

    #[test]
    fn cost_and_margin() {
        // 150 g at 90.00/kg = 13.50; 1 bun at 2.50; 0.5 piece at 1.21 = 0.605 -> rounded at the end.
        assert_eq!(mc_to_cents(value_mc("g", 9000, 150_000)), 1350);
        assert_eq!(mc_to_cents(value_mc("piece", 250, 1000)), 250);
        assert_eq!(mc_to_cents(value_mc("piece", 121, 500)), 61);
        assert_eq!(mc_to_cents(value_mc("ml", 4000, 20_000)), 80);
        assert_eq!(mc_to_cents(value_mc("g", 9000, -150_000)), -1350);
        // Tiny quantities don't vanish when summed: 3 x 0.3 cent = 0.9 cent -> 1 cent.
        let costs: HashMap<i64, (String, i64)> = [(1, ("g".to_string(), 300)), (2, ("g".to_string(), 300)), (3, ("g".to_string(), 300))].into();
        assert_eq!(mc_to_cents(cost_mc(&q(&[(1, 1000), (2, 1000), (3, 1000)]), &costs)), 1);
        // Price 45.00 TTC at 10% -> 40.91 HT; cost 16.00 -> margin 60.89%.
        assert_eq!(price_ht(4500, 1000, true), 4091);
        assert_eq!(price_ht(4500, 1000, false), 4500);
        assert_eq!(margin_bp(4091, 1600), 6089);
        assert_eq!(margin_bp(0, 100), 0);
        assert_eq!(margin_bp(1000, 1500), -5000, "selling under cost shows a negative margin");
    }

    #[test]
    fn diff_to_targets() {
        assert_eq!(diff_to(&q(&[(1, 300)]), &q(&[])), q(&[(1, -300)]));
        assert_eq!(diff_to(&q(&[(1, 100)]), &q(&[(1, 300), (2, 50)])), q(&[(1, 200), (2, 50)]));
        assert!(diff_to(&q(&[(1, 5)]), &q(&[(1, 5)])).is_empty());
    }

    // ----- with the database

    fn ing(c: &Connection, fr: &str) -> (i64, i64) {
        c.query_row("SELECT id, stock_milli FROM ingredients WHERE json_extract(names,'$.fr')=?1", [fr], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }
    fn stock(c: &Connection, fr: &str) -> i64 {
        ing(c, fr).1
    }
    fn ledger_matches_cache(c: &Connection) {
        let bad: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM ingredients i WHERE stock_milli <> (SELECT COALESCE(SUM(delta_milli),0) FROM stock_moves WHERE ingredient_id=i.id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bad, 0, "cached stock = sum of the ledger");
    }
    fn submit(c: &Connection, me: &SessionUser, order_id: Option<i64>, lines: Vec<NewLine>, send: bool) -> i64 {
        orders::submit(c, me, SubmitIn {
            order_id, kind: Some("takeaway".into()), table_id: None, customer_name: None, customer_phone: None, customer_address: None,
            delivery_fee_cents: None, note: None, lines, send, hold: false, hold_label: None, register: "main".into(),
        })
        .unwrap()
    }
    fn total(c: &Connection, id: i64) -> i64 {
        c.query_row("SELECT total_cents FROM orders WHERE id=?1", [id], |r| r.get(0)).unwrap()
    }
    fn pay_cash(c: &Connection, me: &SessionUser, id: i64) {
        orders::pay(c, me, id, vec![PayIn { method: "cash".into(), amount_cents: total(c, id), tendered_cents: None }], "main").unwrap();
    }
    fn set(c: &Connection, k: &str, v: Value) {
        let mut m = Map::new();
        m.insert(k.into(), v);
        settings::set_many(c, &m).unwrap();
    }
    /// A burger line with the required doneness (and the Simple size when it has sizes).
    fn burger(c: &Connection, fr: &str, qty: i64) -> NewLine {
        let mut l = simple_line(c, fr, qty);
        l.config.variant_id = c
            .query_row("SELECT id FROM product_variants WHERE product_id=?1 AND json_extract(names,'$.fr')='Simple'", [l.config.product_id], |r| r.get(0))
            .optional()
            .unwrap();
        l.config.modifiers = vec![ModSel { option_id: opt(c, "À point"), qty: 1 }];
        l
    }
    fn opt(c: &Connection, fr: &str) -> i64 {
        c.query_row("SELECT id FROM modifier_options WHERE json_extract(names,'$.fr')=?1 ORDER BY id LIMIT 1", [fr], |r| r.get(0)).unwrap()
    }

    #[test]
    fn demo_recipes_seed_once() {
        let (c, owner, _) = demo();
        assert!(seed_demo(&c, owner.id).unwrap());
        assert!(!seed_demo(&c, owner.id).unwrap());
        assert_eq!(stock(&c, "Pain burger"), 200_000);
        // Every active simple product of the demo menu has a recipe, and every combo too.
        let fc = food_cost(&c).unwrap();
        let missing: Vec<_> = fc.as_array().unwrap().iter().filter(|r| r["has_recipe"] == false).map(|r| r["names"]["fr"].clone()).collect();
        assert!(missing.is_empty(), "no recipe: {missing:?}");
        let classic = fc.as_array().unwrap().iter().find(|r| r["names"]["fr"] == "Classic Burger" && r["variant_names"]["fr"] == "Simple").unwrap();
        // bun 2.50 + beef 13.50 + onion 0.16 + tomato 0.27 + lettuce 0.225 + pickles 0.45 + sauce 0.80 + box 0.80
        assert_eq!(classic["cost_cents"], 250 + 1350 + 16 + 27 + 23 + 45 + 80 + 80, "{classic}");
        assert_eq!(classic["price_ht_cents"], 4091);
        let double = fc.as_array().unwrap().iter().find(|r| r["names"]["fr"] == "Classic Burger" && r["variant_names"]["fr"] == "Double").unwrap();
        assert_eq!(double["cost_cents"].as_i64().unwrap() - classic["cost_cents"].as_i64().unwrap(), 1350, "double = one more patty");
        ledger_matches_cache(&c);
    }

    #[test]
    fn selling_deducts_and_void_restores() {
        let (c, owner, cashier) = demo();
        seed_demo(&c, owner.id).unwrap();
        let bun0 = stock(&c, "Pain burger");
        let beef0 = stock(&c, "Viande hachée de bœuf");
        // Not sent, not paid: nothing used yet.
        let id = submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 2)], false);
        assert_eq!(stock(&c, "Pain burger"), bun0);
        // Sent to the kitchen: 2 buns, 300 g kefta.
        submit(&c, &cashier, Some(id), vec![], true);
        assert_eq!(stock(&c, "Pain burger"), bun0 - 2000);
        assert_eq!(stock(&c, "Kefta"), 5_000_000 - 300_000);
        // Syncing again (payment) doesn't deduct twice.
        pay_cash(&c, &cashier, id);
        assert_eq!(stock(&c, "Pain burger"), bun0 - 2000);

        // An order paid without going to the kitchen uses stock at payment.
        let mut line = simple_line(&c, "Classic Burger", 1);
        let v: i64 = c.query_row("SELECT id FROM product_variants WHERE product_id=?1 AND json_extract(names,'$.fr')='Double'", [line.config.product_id], |r| r.get(0)).unwrap();
        line.config.variant_id = Some(v);
        line.config.modifiers = vec![ModSel { option_id: opt(&c, "À point"), qty: 1 }, ModSel { option_id: opt(&c, "Fromage"), qty: 2 }, ModSel { option_id: opt(&c, "Sans oignon"), qty: 1 }];
        let o2 = submit(&c, &cashier, None, vec![line], false);
        let onion0 = stock(&c, "Oignon");
        let cheddar0 = stock(&c, "Cheddar (tranche)");
        pay_cash(&c, &cashier, o2);
        assert_eq!(stock(&c, "Viande hachée de bœuf"), beef0 - 300_000, "double = 300 g");
        assert_eq!(stock(&c, "Cheddar (tranche)"), cheddar0 - 2000, "2 extra cheese");
        assert_eq!(stock(&c, "Oignon"), onion0, "no onion");

        // Voiding a sent line gives the stock back (setting on by default).
        let o3 = submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 1)], true);
        let kefta_after_sale = stock(&c, "Kefta");
        let line_id: i64 = c.query_row("SELECT id FROM order_lines WHERE order_id=?1", [o3], |r| r.get(0)).unwrap();
        orders::submit(&c, &cashier, SubmitIn {
            order_id: Some(o3), kind: None, table_id: None, customer_name: None, customer_phone: None, customer_address: None,
            delivery_fee_cents: None, note: None, lines: vec![simple_line(&c, "Coca-Cola 33cl", 1)], send: true, hold: false,
            hold_label: None, register: "main".into(),
        }).unwrap();
        orders::remove_line(&c, &cashier, line_id, Some("client parti".into()), Some("2222")).unwrap();
        assert_eq!(stock(&c, "Kefta"), kefta_after_sale + 150_000);
        // Voiding the whole order returns the drink too.
        let coca_before_void = stock(&c, "Canette Coca-Cola 33cl");
        orders::void(&c, &cashier, o3, "erreur", Some("2222")).unwrap();
        assert_eq!(stock(&c, "Canette Coca-Cola 33cl"), coca_before_void + 1000);
        let kinds: Vec<String> = c.prepare("SELECT DISTINCT kind FROM stock_moves WHERE order_id=?1 ORDER BY kind").unwrap()
            .query_map([o3], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(kinds, ["return", "sale"]);
        ledger_matches_cache(&c);
    }

    #[test]
    fn void_without_restore_keeps_stock_used_and_refund_setting() {
        let (c, owner, cashier) = demo();
        seed_demo(&c, owner.id).unwrap();
        set(&c, "stock_restore_on_void", json!(false));
        let bun0 = stock(&c, "Pain burger");
        let id = submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 1)], true);
        orders::void(&c, &cashier, id, "jeté", Some("2222")).unwrap();
        assert_eq!(stock(&c, "Pain burger"), bun0 - 1000, "food was made: stays used");

        // Refund: kept by default, given back when the setting is on.
        let r1 = submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 1)], true);
        pay_cash(&c, &cashier, r1);
        orders::refund(&c, &cashier, r1, "plainte", Some("2222"), "main").unwrap();
        assert_eq!(stock(&c, "Pain burger"), bun0 - 2000);
        set(&c, "stock_restore_on_refund", json!(true));
        let r2 = submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 1)], true);
        pay_cash(&c, &cashier, r2);
        orders::refund(&c, &cashier, r2, "erreur de caisse", Some("2222"), "main").unwrap();
        assert_eq!(stock(&c, "Pain burger"), bun0 - 2000);
        ledger_matches_cache(&c);
    }

    #[test]
    fn recipe_change_does_not_rewrite_past_sales_and_split_moves_nothing() {
        let (c, owner, cashier) = demo();
        seed_demo(&c, owner.id).unwrap();
        let kefta = ing(&c, "Kefta").0;
        let pid: i64 = c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')='Kefta Burger'", [], |r| r.get(0)).unwrap();
        let id = submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 3)], true);
        let after_sale = stock(&c, "Kefta");
        assert_eq!(after_sale, 5_000_000 - 450_000);
        save_recipe(&c, &owner, RecipeIn { target: "product".into(), id: pid, lines: vec![RecipeLineIn { ingredient_id: kefta, qty_milli: 200_000 }] }).unwrap();
        // Splitting one burger to another check re-syncs both orders: no change in stock.
        let line_id: i64 = c.query_row("SELECT id FROM order_lines WHERE order_id=?1", [id], |r| r.get(0)).unwrap();
        submit(&c, &cashier, Some(id), vec![simple_line(&c, "Coca-Cola 33cl", 1)], true);
        let coca = stock(&c, "Canette Coca-Cola 33cl");
        let new_id = orders::split(&c, &cashier, id, vec![orders::SplitMove { line_id, qty: 1 }]).unwrap();
        assert_eq!(stock(&c, "Kefta"), after_sale, "old sale keeps its frozen recipe (150 g)");
        assert_eq!(stock(&c, "Canette Coca-Cola 33cl"), coca);
        // A new sale uses the new recipe.
        submit(&c, &cashier, Some(new_id), vec![burger(&c, "Kefta Burger", 1)], true);
        assert_eq!(stock(&c, "Kefta"), after_sale - 200_000);
        ledger_matches_cache(&c);
    }

    #[test]
    fn waste_adjust_and_count() {
        let (c, owner, _) = demo();
        seed_demo(&c, owner.id).unwrap();
        let (beef, beef0) = ing(&c, "Viande hachée de bœuf");
        // Ingredient waste: 500 g at 90/kg = 45.00 lost.
        let lost = waste(&c, &owner, WasteIn { ingredient_id: Some(beef), qty_milli: Some(500_000), product_id: None, variant_id: None, qty: None, reason: "périmé".into() }).unwrap();
        assert_eq!(lost, 4500);
        assert_eq!(stock(&c, "Viande hachée de bœuf"), beef0 - 500_000);
        // Product waste takes the recipe out.
        let pid: i64 = c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')='Kefta Burger'", [], |r| r.get(0)).unwrap();
        let bun0 = stock(&c, "Pain burger");
        waste(&c, &owner, WasteIn { ingredient_id: None, qty_milli: None, product_id: Some(pid), variant_id: None, qty: Some(2), reason: "tombé".into() }).unwrap();
        assert_eq!(stock(&c, "Pain burger"), bun0 - 2000);
        assert_eq!(waste(&c, &owner, WasteIn { ingredient_id: Some(beef), qty_milli: Some(1), product_id: None, variant_id: None, qty: None, reason: " ".into() }).unwrap_err().code, "reason_required");
        // Adjustment.
        adjust(&c, &owner, beef, 1_000_000, "livraison hors commande").unwrap();
        assert_eq!(stock(&c, "Viande hachée de bœuf"), beef0 + 500_000);
        assert_eq!(adjust(&c, &owner, beef, 0, "x").unwrap_err().code, "invalid");

        // Stock count: only counted lines change; one count open at a time.
        let cid = count_start(&c, &owner, "fin de mois").unwrap();
        assert_eq!(count_start(&c, &owner, "").unwrap_err().code, "count_open");
        count_set(&c, cid, beef, Some(10_000_000)).unwrap();
        let bun = ing(&c, "Pain burger").0;
        count_set(&c, cid, bun, Some(bun0 - 2000)).unwrap(); // matches: no move
        let tomato_before = stock(&c, "Tomate");
        let res = count_validate(&c, &owner, cid).unwrap();
        assert_eq!(res["counted"], 2);
        assert_eq!(res["changed"], 1);
        assert_eq!(stock(&c, "Viande hachée de bœuf"), 10_000_000);
        assert_eq!(stock(&c, "Tomate"), tomato_before, "uncounted: untouched");
        assert_eq!(count_set(&c, cid, beef, Some(1)).unwrap_err().code, "count_closed");
        let g = count_get(&c, cid).unwrap();
        let beef_line = g["lines"].as_array().unwrap().iter().find(|l| l["ingredient_id"] == beef).unwrap();
        assert_eq!(beef_line["expected_milli"], beef0 + 500_000);
        // History lists every move with its reason.
        let h = history(&c, HistoryIn { ingredient_id: Some(beef), ..Default::default() }).unwrap();
        let kinds: Vec<&str> = h.as_array().unwrap().iter().map(|m| m["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["count", "adjust", "waste", "initial"]);
        ledger_matches_cache(&c);
    }

    #[test]
    fn auto_86_when_an_ingredient_runs_out_and_back_on_restock() {
        let (c, owner, cashier) = demo();
        seed_demo(&c, owner.id).unwrap();
        set(&c, "auto_86_on_stockout", json!(true));
        let (kefta, k0) = ing(&c, "Kefta");
        let pid: i64 = c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')='Kefta Burger'", [], |r| r.get(0)).unwrap();
        let avail = |c: &Connection, id: i64| -> bool { c.query_row("SELECT available FROM products WHERE id=?1", [id], |r| r.get(0)).unwrap() };
        adjust(&c, &owner, kefta, -(k0 - 150_000), "test").unwrap();
        assert!(avail(&c, pid));
        submit(&c, &cashier, None, vec![burger(&c, "Kefta Burger", 1)], true);
        assert!(!avail(&c, pid), "last kefta used: product 86'd");
        let classic: i64 = c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')='Classic Burger'", [], |r| r.get(0)).unwrap();
        assert!(avail(&c, classic), "other products untouched");
        adjust(&c, &owner, kefta, 2_000_000, "livraison").unwrap();
        assert!(avail(&c, pid), "back on restock");
        // A product 86'd by hand stays 86'd after a restock.
        crate::menu::set_available(&c, owner.id, classic, false).unwrap();
        adjust(&c, &owner, ing(&c, "Pain burger").0, 1000, "x").unwrap();
        assert!(!avail(&c, classic));
        // Setting off: no automatic 86.
        set(&c, "auto_86_on_stockout", json!(false));
        adjust(&c, &owner, kefta, -stock(&c, "Kefta"), "x").unwrap();
        assert!(avail(&c, pid));
    }

    #[test]
    fn recipe_and_ingredient_validation() {
        let (c, owner, _) = demo();
        let id = save_ingredient(&c, &owner, IngredientIn { id: None, names: json!({"fr": "Sel"}), unit: "g".into(), cost_cents: 500, min_milli: 0, active: true, initial_milli: 1_000_000 }).unwrap();
        assert_eq!(stock(&c, "Sel"), 1_000_000);
        let pid: i64 = c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')='Kefta Burger'", [], |r| r.get(0)).unwrap();
        let bad = |lines: Vec<(i64, i64)>, target: &str| {
            save_recipe(&c, &owner, RecipeIn { target: target.into(), id: pid, lines: lines.into_iter().map(|(i, q)| RecipeLineIn { ingredient_id: i, qty_milli: q }).collect() })
        };
        assert_eq!(bad(vec![(id, -5)], "product").unwrap_err().code, "invalid", "a product recipe can't remove");
        assert_eq!(bad(vec![(id, 5), (id, 6)], "product").unwrap_err().code, "invalid");
        assert_eq!(bad(vec![(9999, 5)], "product").unwrap_err().code, "not_found");
        assert_eq!(bad(vec![(id, 5)], "nope").unwrap_err().code, "invalid");
        bad(vec![(id, 2000)], "product").unwrap();
        // The unit is locked once used.
        let e = save_ingredient(&c, &owner, IngredientIn { id: Some(id), names: json!({"fr": "Sel"}), unit: "piece".into(), cost_cents: 500, min_milli: 0, active: true, initial_milli: 0 }).unwrap_err();
        assert_eq!(e.code, "unit_locked");
        assert_eq!(save_ingredient(&c, &owner, IngredientIn { id: None, names: json!({"fr": ""}), unit: "g".into(), cost_cents: 0, min_milli: 0, active: true, initial_milli: 0 }).unwrap_err().code, "invalid");
        // Alerts: low when at or under the minimum.
        save_ingredient(&c, &owner, IngredientIn { id: Some(id), names: json!({"fr": "Sel"}), unit: "g".into(), cost_cents: 500, min_milli: 2_000_000, active: true, initial_milli: 0 }).unwrap();
        let a = alerts(&c).unwrap();
        assert!(a.as_array().unwrap().iter().any(|i| i["id"] == id && i["low"] == true));
    }

    #[test]
    fn api_permissions_setup_seed_and_topics() {
        use crate::api::{dispatch, Core};
        let core = Core::new(crate::db::open_in_memory().unwrap());
        dispatch(&core, None, "setup.init", json!({"shop_name": "B", "language": "fr", "owner_name": "Owner", "owner_pin": "1234", "demo_users": true, "demo_menu": true})).unwrap();
        let login = |name: &str, pin: &str| -> String {
            let users = dispatch(&core, None, "auth.users", Value::Null).unwrap();
            let id = users.as_array().unwrap().iter().find(|u| u["name"] == name).unwrap()["id"].clone();
            dispatch(&core, None, "auth.login", json!({"user_id": id, "pin": pin})).unwrap()["token"].as_str().unwrap().to_string()
        };
        let owner = login("Owner", "1234");
        let cashier = login("Salma", "3333");
        let kitchen = login("Youssef", "4444");
        let ings = dispatch(&core, Some(&cashier), "ingredients.list", Value::Null).unwrap();
        assert!(ings.as_array().unwrap().len() > 40, "setup with the demo menu also seeds ingredients and recipes");
        let beef = ings.as_array().unwrap().iter().find(|i| i["names"]["fr"] == "Viande hachée de bœuf").unwrap()["id"].clone();
        let forbidden = |tok: &str, m: &str, p: Value| dispatch(&core, Some(tok), m, p).unwrap_err().code;
        assert_eq!(forbidden(&cashier, "ingredients.save", json!({"names": {"fr": "X"}, "unit": "g"})), "forbidden");
        // Cashiers have the kitchen screen by default, so they can declare waste (dropped drink).
        dispatch(&core, Some(&cashier), "stock.waste", json!({"ingredient_id": beef, "qty_milli": 1000, "reason": "x"})).unwrap();
        assert_eq!(forbidden(&kitchen, "stock.adjust", json!({"ingredient_id": beef, "delta_milli": 1000, "reason": "x"})), "forbidden");
        assert_eq!(forbidden(&kitchen, "counts.start", Value::Null), "forbidden");
        assert_eq!(forbidden(&cashier, "stock.food_cost", Value::Null), "forbidden");
        // The kitchen declares waste; the history shows who did it.
        dispatch(&core, Some(&kitchen), "stock.waste", json!({"ingredient_id": beef, "qty_milli": 200_000, "reason": "tombé"})).unwrap();
        let h = dispatch(&core, Some(&owner), "stock.history", json!({"ingredient_id": beef})).unwrap();
        assert_eq!(h[0]["kind"], "waste");
        assert_eq!(h[0]["user_name"], "Youssef");
        assert_eq!(h[0]["value_cents"], -1800);
        assert_eq!(forbidden(&owner, "stock.seed_demo", Value::Null), "conflict");
        // A count through the API.
        let cid = dispatch(&core, Some(&owner), "counts.start", json!({"note": "test"})).unwrap()["id"].clone();
        dispatch(&core, Some(&owner), "counts.set", json!({"id": cid, "ingredient_id": beef, "counted_milli": 1_000_000})).unwrap();
        dispatch(&core, Some(&owner), "counts.validate", json!({"id": cid})).unwrap();
        let after = dispatch(&core, Some(&owner), "ingredients.list", Value::Null).unwrap();
        assert_eq!(after.as_array().unwrap().iter().find(|i| i["id"] == beef).unwrap()["stock_milli"], 1_000_000);
        // Live updates: writes publish "stock", reads publish nothing.
        assert_eq!(crate::net::topic_of("stock.waste"), Some("stock"));
        assert_eq!(crate::net::topic_of("counts.validate"), Some("stock"));
        assert_eq!(crate::net::topic_of("ingredients.save"), Some("stock"));
        for read in ["stock.history", "stock.alerts", "stock.food_cost", "ingredients.list", "recipes.list", "counts.get"] {
            assert_eq!(crate::net::topic_of(read), None, "{read}");
        }
    }
}
