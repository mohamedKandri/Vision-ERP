//! Menu: stations, categories, products (+variants), modifier groups/options, combos.
//!
//! Everything is soft-deleted (`active = 0`) because past orders will reference these rows.

use crate::audit;
use crate::error::{ApiError, ApiResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const ICONS: &[&str] = &["burger", "fries", "drink", "dessert", "sauce", "combo", "chicken", "salad", "coffee", "star"];
const MAX_PRICE: i64 = 10_000_000; // 100 000 MAD
const MAX_PHOTO_BYTES: usize = 400_000;

// ------------------------------------------------------------------ validation helpers

/// Names must be {"fr": non-empty, "ar"?: .., "en"?: ..}, each at most 120 chars.
pub fn check_names(v: &Value, what: &str) -> ApiResult<String> {
    let obj = v.as_object().ok_or_else(|| ApiError::invalid(format!("{what}: names must be an object")))?;
    for (k, val) in obj {
        if !["fr", "ar", "en"].contains(&k.as_str()) {
            return Err(ApiError::invalid(format!("{what}: unknown language {k}")));
        }
        let s = val.as_str().ok_or_else(|| ApiError::invalid(format!("{what}: name must be text")))?;
        if s.chars().count() > 120 {
            return Err(ApiError::invalid(format!("{what}: name too long")));
        }
    }
    let fr = obj.get("fr").and_then(|x| x.as_str()).unwrap_or("").trim();
    if fr.is_empty() {
        return Err(ApiError::invalid(format!("{what}: French name required")));
    }
    // Store trimmed, drop empty translations.
    let mut clean = serde_json::Map::new();
    for (k, val) in obj {
        let s = val.as_str().unwrap_or("").trim();
        if !s.is_empty() {
            clean.insert(k.clone(), json!(s));
        }
    }
    Ok(Value::Object(clean).to_string())
}

fn check_price(p: i64, what: &str) -> ApiResult<()> {
    if (0..=MAX_PRICE).contains(&p) {
        Ok(())
    } else {
        Err(ApiError::invalid(format!("{what}: price out of range")))
    }
}

fn names_of(s: String) -> Value {
    serde_json::from_str(&s).unwrap_or_else(|_| json!({ "fr": s }))
}

fn exists(conn: &Connection, table: &str, id: i64) -> ApiResult<bool> {
    Ok(conn
        .query_row(&format!("SELECT 1 FROM {table} WHERE id = ?1"), [id], |_| Ok(()))
        .optional()?
        .is_some())
}

// ------------------------------------------------------------------ read

#[derive(Serialize)]
pub struct Menu {
    pub stations: Vec<Value>,
    pub categories: Vec<Value>,
    pub products: Vec<Value>,
    pub modifier_groups: Vec<Value>,
}

/// The whole menu. `all = false` hides inactive rows (POS view); back office passes true.
pub fn get_menu(conn: &Connection, all: bool) -> ApiResult<Menu> {
    let act = if all { "" } else { " WHERE active = 1" };

    let stations = conn
        .prepare(&format!("SELECT id, code, names, sort, active FROM stations{act} ORDER BY sort, id"))?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "code": r.get::<_, String>(1)?, "names": names_of(r.get(2)?),
                      "sort": r.get::<_, i64>(3)?, "active": r.get::<_, bool>(4)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let categories = conn
        .prepare(&format!("SELECT id, names, color, icon, sort, active FROM categories{act} ORDER BY sort, id"))?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "color": r.get::<_, String>(2)?,
                      "icon": r.get::<_, String>(3)?, "sort": r.get::<_, i64>(4)?, "active": r.get::<_, bool>(5)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut groups = conn
        .prepare(&format!("SELECT id, names, min_select, max_select, sort, active FROM modifier_groups{act} ORDER BY sort, id"))?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "min_select": r.get::<_, i64>(2)?,
                      "max_select": r.get::<_, i64>(3)?, "sort": r.get::<_, i64>(4)?, "active": r.get::<_, bool>(5)?,
                      "options": []}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut opt_stmt = conn.prepare(&format!(
        "SELECT id, names, kind, price_cents, max_qty, is_default, active, sort FROM modifier_options
         WHERE group_id = ?1{} ORDER BY sort, id",
        if all { "" } else { " AND active = 1" }
    ))?;
    for g in groups.iter_mut() {
        let gid = g["id"].as_i64().unwrap();
        let opts = opt_stmt
            .query_map([gid], |r| {
                Ok(json!({"id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "kind": r.get::<_, String>(2)?,
                          "price_cents": r.get::<_, i64>(3)?, "max_qty": r.get::<_, i64>(4)?,
                          "is_default": r.get::<_, bool>(5)?, "active": r.get::<_, bool>(6)?, "sort": r.get::<_, i64>(7)?}))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        g["options"] = json!(opts);
    }

    let mut products = conn
        .prepare(&format!(
            "SELECT id, category_id, kind, names, description, price_cents, happy_hour_price_cents, tax_id,
                    station_id, photo, active, available, sort, loyalty_stamp FROM products{act} ORDER BY sort, id"
        ))?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "category_id": r.get::<_, i64>(1)?, "kind": r.get::<_, String>(2)?,
                      "names": names_of(r.get(3)?), "description": names_of(r.get(4)?),
                      "price_cents": r.get::<_, i64>(5)?, "happy_hour_price_cents": r.get::<_, Option<i64>>(6)?,
                      "tax_id": r.get::<_, i64>(7)?, "station_id": r.get::<_, Option<i64>>(8)?,
                      "photo": r.get::<_, String>(9)?, "active": r.get::<_, bool>(10)?,
                      "available": r.get::<_, bool>(11)?, "sort": r.get::<_, i64>(12)?, "loyalty_stamp": r.get::<_, bool>(13)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut var_stmt = conn.prepare(&format!(
        "SELECT id, names, price_cents, happy_hour_price_cents, active, sort FROM product_variants
         WHERE product_id = ?1{} ORDER BY sort, id",
        if all { "" } else { " AND active = 1" }
    ))?;
    let mut grp_stmt = conn.prepare("SELECT group_id FROM product_modifier_groups WHERE product_id = ?1 ORDER BY sort, group_id")?;
    let mut slot_stmt = conn.prepare("SELECT id, names, sort FROM combo_slots WHERE product_id = ?1 ORDER BY sort, id")?;
    let mut sopt_stmt = conn.prepare(
        "SELECT product_id, variant_id, extra_cents, is_default FROM combo_slot_options WHERE slot_id = ?1 ORDER BY sort, id",
    )?;
    for p in products.iter_mut() {
        let pid = p["id"].as_i64().unwrap();
        let variants = var_stmt
            .query_map([pid], |r| {
                Ok(json!({"id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "price_cents": r.get::<_, i64>(2)?,
                          "happy_hour_price_cents": r.get::<_, Option<i64>>(3)?, "active": r.get::<_, bool>(4)?,
                          "sort": r.get::<_, i64>(5)?}))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let group_ids = grp_stmt.query_map([pid], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        let mut slots = slot_stmt
            .query_map([pid], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "names": names_of(r.get(1)?), "sort": r.get::<_, i64>(2)?})))?
            .collect::<Result<Vec<_>, _>>()?;
        for s in slots.iter_mut() {
            let sid = s["id"].as_i64().unwrap();
            let opts = sopt_stmt
                .query_map([sid], |r| {
                    Ok(json!({"product_id": r.get::<_, i64>(0)?, "variant_id": r.get::<_, Option<i64>>(1)?,
                              "extra_cents": r.get::<_, i64>(2)?, "is_default": r.get::<_, bool>(3)?}))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            s["options"] = json!(opts);
        }
        p["variants"] = json!(variants);
        p["group_ids"] = json!(group_ids);
        p["slots"] = json!(slots);
    }

    Ok(Menu { stations, categories, products, modifier_groups: groups })
}

// ------------------------------------------------------------------ write: categories

#[derive(Deserialize)]
pub struct CategoryIn {
    pub id: Option<i64>,
    pub names: Value,
    pub color: String,
    pub icon: String,
    #[serde(default)]
    pub sort: i64,
    pub active: bool,
}

fn check_color(c: &str) -> ApiResult<()> {
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid("color must be #rrggbb"))
    }
}

pub fn save_category(conn: &Connection, user_id: i64, c: CategoryIn) -> ApiResult<i64> {
    let names = check_names(&c.names, "category")?;
    check_color(&c.color)?;
    if !ICONS.contains(&c.icon.as_str()) {
        return Err(ApiError::invalid("unknown icon"));
    }
    let id = match c.id {
        Some(id) => {
            if conn.execute(
                "UPDATE categories SET names=?2, color=?3, icon=?4, sort=?5, active=?6 WHERE id=?1",
                params![id, names, c.color, c.icon, c.sort, c.active],
            )? == 0
            {
                return Err(ApiError::not_found("category"));
            }
            id
        }
        None => {
            conn.execute(
                "INSERT INTO categories (names, color, icon, sort, active) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![names, c.color, c.icon, c.sort, c.active],
            )?;
            conn.last_insert_rowid()
        }
    };
    audit::log(conn, Some(user_id), "category.saved", Some("category"), Some(&id.to_string()), Some(json!({"names": c.names, "active": c.active})))?;
    Ok(id)
}

// ------------------------------------------------------------------ write: modifier groups

#[derive(Deserialize)]
pub struct OptionIn {
    pub id: Option<i64>,
    pub names: Value,
    pub kind: String,
    #[serde(default)]
    pub price_cents: i64,
    #[serde(default = "one")]
    pub max_qty: i64,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default = "yes")]
    pub active: bool,
}
fn one() -> i64 {
    1
}
fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct GroupIn {
    pub id: Option<i64>,
    pub names: Value,
    pub min_select: i64,
    pub max_select: i64,
    #[serde(default)]
    pub sort: i64,
    pub active: bool,
    pub options: Vec<OptionIn>,
}

pub fn save_group(conn: &Connection, user_id: i64, g: GroupIn) -> ApiResult<i64> {
    let names = check_names(&g.names, "modifier group")?;
    if g.min_select < 0 || g.max_select < 1 || g.min_select > g.max_select || g.max_select > 50 {
        return Err(ApiError::invalid("need 0 ≤ min ≤ max, max between 1 and 50"));
    }
    let active_opts: Vec<&OptionIn> = g.options.iter().filter(|o| o.active).collect();
    if active_opts.is_empty() {
        return Err(ApiError::invalid("a modifier group needs at least one option"));
    }
    let max_reachable: i64 = active_opts.iter().map(|o| o.max_qty).sum();
    if max_reachable < g.min_select {
        return Err(ApiError::invalid("not enough options to reach the minimum"));
    }
    if active_opts.iter().filter(|o| o.is_default).count() as i64 > g.max_select {
        return Err(ApiError::invalid("more defaults than the maximum allows"));
    }
    let mut checked = Vec::new();
    for o in &g.options {
        let n = check_names(&o.names, "modifier option")?;
        if !["choice", "extra", "remove"].contains(&o.kind.as_str()) {
            return Err(ApiError::invalid("option kind must be choice, extra or remove"));
        }
        check_price(o.price_cents, "modifier option")?;
        if o.kind == "remove" && o.price_cents != 0 {
            return Err(ApiError::invalid("a 'remove' option is always free"));
        }
        if !(1..=20).contains(&o.max_qty) {
            return Err(ApiError::invalid("option max quantity must be 1-20"));
        }
        checked.push(n);
    }

    let tx = crate::db::atomic(conn)?;
    let gid = match g.id {
        Some(id) => {
            if tx.execute(
                "UPDATE modifier_groups SET names=?2, min_select=?3, max_select=?4, sort=?5, active=?6 WHERE id=?1",
                params![id, names, g.min_select, g.max_select, g.sort, g.active],
            )? == 0
            {
                return Err(ApiError::not_found("modifier group"));
            }
            id
        }
        None => {
            tx.execute(
                "INSERT INTO modifier_groups (names, min_select, max_select, sort, active) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![names, g.min_select, g.max_select, g.sort, g.active],
            )?;
            tx.last_insert_rowid()
        }
    };
    // Options not in the payload are deactivated (kept for order history).
    let keep: Vec<i64> = g.options.iter().filter_map(|o| o.id).collect();
    let existing: Vec<i64> = tx
        .prepare("SELECT id FROM modifier_options WHERE group_id = ?1")?
        .query_map([gid], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in existing.iter().filter(|id| !keep.contains(id)) {
        tx.execute("UPDATE modifier_options SET active = 0 WHERE id = ?1", [id])?;
    }
    for (i, (o, n)) in g.options.iter().zip(checked).enumerate() {
        match o.id {
            Some(id) => {
                if tx.execute(
                    "UPDATE modifier_options SET names=?3, kind=?4, price_cents=?5, max_qty=?6, is_default=?7, active=?8, sort=?9
                     WHERE id=?1 AND group_id=?2",
                    params![id, gid, n, o.kind, o.price_cents, o.max_qty, o.is_default, o.active, i as i64],
                )? == 0
                {
                    return Err(ApiError::not_found("modifier option"));
                }
            }
            None => {
                tx.execute(
                    "INSERT INTO modifier_options (group_id, names, kind, price_cents, max_qty, is_default, active, sort)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![gid, n, o.kind, o.price_cents, o.max_qty, o.is_default, o.active, i as i64],
                )?;
            }
        }
    }
    audit::log(&tx, Some(user_id), "modifier_group.saved", Some("modifier_group"), Some(&gid.to_string()),
        Some(json!({"names": g.names, "min": g.min_select, "max": g.max_select,
                    "options": g.options.iter().map(|o| json!({"names": o.names, "price_cents": o.price_cents, "active": o.active})).collect::<Vec<_>>()})))?;
    tx.commit()?;
    Ok(gid)
}

// ------------------------------------------------------------------ write: products

#[derive(Deserialize)]
pub struct VariantIn {
    pub id: Option<i64>,
    pub names: Value,
    pub price_cents: i64,
    pub happy_hour_price_cents: Option<i64>,
    #[serde(default = "yes")]
    pub active: bool,
}

#[derive(Deserialize)]
pub struct SlotOptionIn {
    pub product_id: i64,
    pub variant_id: Option<i64>,
    #[serde(default)]
    pub extra_cents: i64,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Deserialize)]
pub struct SlotIn {
    pub names: Value,
    pub options: Vec<SlotOptionIn>,
}

#[derive(Deserialize)]
pub struct ProductIn {
    pub id: Option<i64>,
    pub category_id: i64,
    pub kind: String,
    pub names: Value,
    #[serde(default = "empty_obj")]
    pub description: Value,
    pub price_cents: i64,
    pub happy_hour_price_cents: Option<i64>,
    pub tax_id: i64,
    pub station_id: Option<i64>,
    #[serde(default)]
    pub photo: String,
    pub active: bool,
    #[serde(default = "yes")]
    pub available: bool,
    #[serde(default)]
    pub sort: i64,
    #[serde(default)]
    pub variants: Vec<VariantIn>,
    #[serde(default)]
    pub group_ids: Vec<i64>,
    #[serde(default)]
    pub slots: Vec<SlotIn>,
    /// Earns a loyalty stamp. None keeps the current value.
    #[serde(default)]
    pub loyalty_stamp: Option<bool>,
}
fn empty_obj() -> Value {
    json!({})
}

fn price_snapshot(conn: &Connection, pid: i64) -> ApiResult<Value> {
    let p: (i64, Option<i64>) = conn.query_row(
        "SELECT price_cents, happy_hour_price_cents FROM products WHERE id=?1",
        [pid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let vars = conn
        .prepare("SELECT id, price_cents, happy_hour_price_cents FROM product_variants WHERE product_id=?1 AND active=1 ORDER BY id")?
        .query_map([pid], |r| Ok(json!([r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?])))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"price_cents": p.0, "happy_hour_price_cents": p.1, "variants": vars}))
}

pub fn save_product(conn: &Connection, user_id: i64, p: ProductIn) -> ApiResult<i64> {
    let names = check_names(&p.names, "product")?;
    let description = if p.description.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        "{}".to_string()
    } else {
        check_names(&p.description, "description")?
    };
    if !["simple", "combo"].contains(&p.kind.as_str()) {
        return Err(ApiError::invalid("kind must be simple or combo"));
    }
    check_price(p.price_cents, "product")?;
    if let Some(h) = p.happy_hour_price_cents {
        check_price(h, "happy hour")?;
    }
    if !exists(conn, "categories", p.category_id)? {
        return Err(ApiError::invalid("unknown category"));
    }
    if !exists(conn, "taxes", p.tax_id)? {
        return Err(ApiError::invalid("unknown tax"));
    }
    if let Some(s) = p.station_id {
        if !exists(conn, "stations", s)? {
            return Err(ApiError::invalid("unknown station"));
        }
    }
    if !p.photo.is_empty() && (!p.photo.starts_with("data:image/") || p.photo.len() > MAX_PHOTO_BYTES) {
        return Err(ApiError::invalid("photo must be a small image"));
    }
    for v in &p.variants {
        check_names(&v.names, "variant")?;
        check_price(v.price_cents, "variant")?;
        if let Some(h) = v.happy_hour_price_cents {
            check_price(h, "variant happy hour")?;
        }
    }
    for gid in &p.group_ids {
        if !exists(conn, "modifier_groups", *gid)? {
            return Err(ApiError::invalid("unknown modifier group"));
        }
    }
    if p.kind == "combo" {
        if !p.variants.is_empty() {
            return Err(ApiError::invalid("a combo cannot have variants; use a size modifier group"));
        }
        if p.slots.is_empty() {
            return Err(ApiError::invalid("a combo needs at least one slot"));
        }
        for s in &p.slots {
            check_names(&s.names, "combo slot")?;
            if s.options.is_empty() {
                return Err(ApiError::invalid("each combo slot needs at least one choice"));
            }
            if s.options.iter().filter(|o| o.is_default).count() > 1 {
                return Err(ApiError::invalid("only one default per combo slot"));
            }
            for o in &s.options {
                check_price(o.extra_cents, "combo extra")?;
                let kind: Option<String> = conn
                    .query_row("SELECT kind FROM products WHERE id=?1", [o.product_id], |r| r.get(0))
                    .optional()?;
                match kind.as_deref() {
                    Some("simple") => {}
                    Some(_) => return Err(ApiError::invalid("a combo cannot contain another combo")),
                    None => return Err(ApiError::invalid("unknown product in combo")),
                }
                if Some(o.product_id) == p.id {
                    return Err(ApiError::invalid("a combo cannot contain itself"));
                }
                if let Some(vid) = o.variant_id {
                    let ok = conn
                        .query_row("SELECT 1 FROM product_variants WHERE id=?1 AND product_id=?2", params![vid, o.product_id], |_| Ok(()))
                        .optional()?
                        .is_some();
                    if !ok {
                        return Err(ApiError::invalid("variant does not belong to that product"));
                    }
                }
            }
        }
    } else if !p.slots.is_empty() {
        return Err(ApiError::invalid("only combos have slots"));
    }

    let tx = crate::db::atomic(conn)?;
    let before = match p.id {
        Some(id) => {
            let k: Option<String> = tx.query_row("SELECT kind FROM products WHERE id=?1", [id], |r| r.get(0)).optional()?;
            match k {
                None => return Err(ApiError::not_found("product")),
                Some(k) if k != p.kind => return Err(ApiError::invalid("cannot change a product into a combo or back")),
                _ => {}
            }
            Some(price_snapshot(&tx, id)?)
        }
        None => None,
    };
    let pid = match p.id {
        Some(id) => {
            tx.execute(
                "UPDATE products SET category_id=?2, names=?3, description=?4, price_cents=?5, happy_hour_price_cents=?6,
                   tax_id=?7, station_id=?8, photo=?9, active=?10, available=?11, sort=?12,
                   updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![id, p.category_id, names, description, p.price_cents, p.happy_hour_price_cents, p.tax_id,
                        p.station_id, p.photo, p.active, p.available, p.sort],
            )?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO products (category_id, kind, names, description, price_cents, happy_hour_price_cents, tax_id,
                   station_id, photo, active, available, sort) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![p.category_id, p.kind, names, description, p.price_cents, p.happy_hour_price_cents, p.tax_id,
                        p.station_id, p.photo, p.active, p.available, p.sort],
            )?;
            tx.last_insert_rowid()
        }
    };
    if let Some(v) = p.loyalty_stamp {
        tx.execute("UPDATE products SET loyalty_stamp=?2 WHERE id=?1", params![pid, v])?;
    }

    // Variants: upsert, keep ids stable, deactivate the ones removed.
    let keep: Vec<i64> = p.variants.iter().filter_map(|v| v.id).collect();
    let existing: Vec<i64> = tx
        .prepare("SELECT id FROM product_variants WHERE product_id=?1")?
        .query_map([pid], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in existing.iter().filter(|id| !keep.contains(id)) {
        tx.execute("UPDATE product_variants SET active=0 WHERE id=?1", [id])?;
    }
    for (i, v) in p.variants.iter().enumerate() {
        let n = check_names(&v.names, "variant")?;
        match v.id {
            Some(id) => {
                if tx.execute(
                    "UPDATE product_variants SET names=?3, price_cents=?4, happy_hour_price_cents=?5, active=?6, sort=?7
                     WHERE id=?1 AND product_id=?2",
                    params![id, pid, n, v.price_cents, v.happy_hour_price_cents, v.active, i as i64],
                )? == 0
                {
                    return Err(ApiError::not_found("variant"));
                }
            }
            None => {
                tx.execute(
                    "INSERT INTO product_variants (product_id, names, price_cents, happy_hour_price_cents, active, sort)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![pid, n, v.price_cents, v.happy_hour_price_cents, v.active, i as i64],
                )?;
            }
        }
    }

    tx.execute("DELETE FROM product_modifier_groups WHERE product_id=?1", [pid])?;
    for (i, gid) in p.group_ids.iter().enumerate() {
        tx.execute(
            "INSERT OR IGNORE INTO product_modifier_groups (product_id, group_id, sort) VALUES (?1, ?2, ?3)",
            params![pid, gid, i as i64],
        )?;
    }

    tx.execute("DELETE FROM combo_slots WHERE product_id=?1", [pid])?;
    for (i, s) in p.slots.iter().enumerate() {
        tx.execute(
            "INSERT INTO combo_slots (product_id, names, sort) VALUES (?1, ?2, ?3)",
            params![pid, check_names(&s.names, "combo slot")?, i as i64],
        )?;
        let sid = tx.last_insert_rowid();
        for (j, o) in s.options.iter().enumerate() {
            tx.execute(
                "INSERT INTO combo_slot_options (slot_id, product_id, variant_id, extra_cents, is_default, sort)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![sid, o.product_id, o.variant_id, o.extra_cents, o.is_default, j as i64],
            )?;
        }
    }

    let after = price_snapshot(&tx, pid)?;
    audit::log(&tx, Some(user_id), if p.id.is_some() { "product.updated" } else { "product.created" },
        Some("product"), Some(&pid.to_string()), Some(json!({"names": p.names, "active": p.active})))?;
    if let Some(b) = before {
        if b != after {
            audit::log(&tx, Some(user_id), "product.price_changed", Some("product"), Some(&pid.to_string()),
                Some(json!({"names": p.names, "before": b, "after": after})))?;
        }
    }
    tx.commit()?;
    Ok(pid)
}

/// 86 / un-86 a product (out of stock today).
pub fn set_available(conn: &Connection, user_id: i64, id: i64, available: bool) -> ApiResult<()> {
    if conn.execute(
        "UPDATE products SET available=?2, stock_out=0, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
        params![id, available],
    )? == 0
    {
        return Err(ApiError::not_found("product"));
    }
    audit::log(conn, Some(user_id), if available { "product.back_in_stock" } else { "product.86" },
        Some("product"), Some(&id.to_string()), None)?;
    Ok(())
}

/// Set `sort` = position for the given ids.
pub fn reorder(conn: &Connection, table: &str, ids: &[i64]) -> ApiResult<()> {
    let table = match table {
        "categories" | "products" | "modifier_groups" => table,
        _ => return Err(ApiError::invalid("cannot reorder that")),
    };
    let tx = crate::db::atomic(conn)?;
    for (i, id) in ids.iter().enumerate() {
        tx.execute(&format!("UPDATE {table} SET sort=?2 WHERE id=?1"), params![id, i as i64])?;
    }
    tx.commit()?;
    Ok(())
}

pub fn product_count(conn: &Connection) -> ApiResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM products", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn cat(c: &Connection) -> i64 {
        save_category(c, 0_i64.max(1), CategoryIn { id: None, names: json!({"fr": "Burgers"}), color: "#ff0000".into(), icon: "burger".into(), sort: 0, active: true })
            .unwrap()
    }
    fn setup() -> Connection {
        let c = open_in_memory().unwrap();
        c.execute("INSERT INTO users (id, name, role, pin_hash) VALUES (1, 'O', 'owner', 'x')", []).unwrap();
        c
    }
    fn simple(c: &Connection, cat: i64, name: &str, price: i64) -> i64 {
        save_product(c, 1, serde_json::from_value(json!({
            "category_id": cat, "kind": "simple", "names": {"fr": name}, "price_cents": price,
            "happy_hour_price_cents": null, "tax_id": 1, "station_id": 1, "active": true
        })).unwrap()).unwrap()
    }

    #[test]
    fn names_validation() {
        assert!(check_names(&json!({"fr": "Classic"}), "x").is_ok());
        assert_eq!(check_names(&json!({"fr": " Classic ", "ar": "", "en": "C"}), "x").unwrap(), r#"{"en":"C","fr":"Classic"}"#);
        assert!(check_names(&json!({"ar": "كلاسيك"}), "x").is_err());
        assert!(check_names(&json!({"fr": "  "}), "x").is_err());
        assert!(check_names(&json!({"fr": "x", "de": "y"}), "x").is_err());
        assert!(check_names(&json!("Classic"), "x").is_err());
    }

    #[test]
    fn category_validation() {
        let c = setup();
        let bad_color = CategoryIn { id: None, names: json!({"fr": "A"}), color: "red".into(), icon: "burger".into(), sort: 0, active: true };
        assert!(save_category(&c, 1, bad_color).is_err());
        let bad_icon = CategoryIn { id: None, names: json!({"fr": "A"}), color: "#000000".into(), icon: "rocket".into(), sort: 0, active: true };
        assert!(save_category(&c, 1, bad_icon).is_err());
    }

    #[test]
    fn product_with_variants_and_groups_roundtrip() {
        let c = setup();
        let k = cat(&c);
        let g = save_group(&c, 1, serde_json::from_value(json!({
            "names": {"fr": "Cuisson"}, "min_select": 1, "max_select": 1, "active": true,
            "options": [{"names": {"fr": "Saignant"}, "kind": "choice"}, {"names": {"fr": "À point"}, "kind": "choice", "is_default": true}]
        })).unwrap()).unwrap();
        let pid = save_product(&c, 1, serde_json::from_value(json!({
            "category_id": k, "kind": "simple", "names": {"fr": "Classic", "ar": "كلاسيك"}, "price_cents": 4500,
            "happy_hour_price_cents": 3900, "tax_id": 1, "station_id": 1, "active": true,
            "variants": [{"names": {"fr": "Simple"}, "price_cents": 4500, "happy_hour_price_cents": null},
                         {"names": {"fr": "Double"}, "price_cents": 6000, "happy_hour_price_cents": null}],
            "group_ids": [g]
        })).unwrap()).unwrap();
        let m = get_menu(&c, true).unwrap();
        let p = m.products.iter().find(|p| p["id"] == pid).unwrap();
        assert_eq!(p["names"]["ar"], "كلاسيك");
        assert_eq!(p["variants"].as_array().unwrap().len(), 2);
        assert_eq!(p["group_ids"], json!([g]));
        assert_eq!(m.modifier_groups[0]["options"].as_array().unwrap().len(), 2);
        assert_eq!(m.stations.len(), 4);
    }

    #[test]
    fn variant_ids_are_stable_and_removed_ones_deactivated() {
        let c = setup();
        let k = cat(&c);
        let pid = save_product(&c, 1, serde_json::from_value(json!({
            "category_id": k, "kind": "simple", "names": {"fr": "B"}, "price_cents": 100, "happy_hour_price_cents": null,
            "tax_id": 1, "station_id": null, "active": true,
            "variants": [{"names": {"fr": "S"}, "price_cents": 100, "happy_hour_price_cents": null},
                         {"names": {"fr": "D"}, "price_cents": 200, "happy_hour_price_cents": null}]
        })).unwrap()).unwrap();
        let ids: Vec<i64> = c.prepare("SELECT id FROM product_variants ORDER BY id").unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        save_product(&c, 1, serde_json::from_value(json!({
            "id": pid, "category_id": k, "kind": "simple", "names": {"fr": "B"}, "price_cents": 100, "happy_hour_price_cents": null,
            "tax_id": 1, "station_id": null, "active": true,
            "variants": [{"id": ids[0], "names": {"fr": "S"}, "price_cents": 150, "happy_hour_price_cents": null}]
        })).unwrap()).unwrap();
        let rows: Vec<(i64, i64, bool)> = c.prepare("SELECT id, price_cents, active FROM product_variants ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(rows, vec![(ids[0], 150, true), (ids[1], 200, false)]);
        let pos = get_menu(&c, false).unwrap();
        assert_eq!(pos.products[0]["variants"].as_array().unwrap().len(), 1, "POS view hides inactive variants");
    }

    #[test]
    fn price_change_is_audited_only_when_price_changes() {
        let c = setup();
        let k = cat(&c);
        let pid = simple(&c, k, "Fries", 1500);
        let body = |price: i64, name: &str| serde_json::from_value::<ProductIn>(json!({
            "id": pid, "category_id": k, "kind": "simple", "names": {"fr": name}, "price_cents": price,
            "happy_hour_price_cents": null, "tax_id": 1, "station_id": 2, "active": true
        })).unwrap();
        save_product(&c, 1, body(1500, "Frites")).unwrap();
        let count = |c: &Connection| -> i64 { c.query_row("SELECT COUNT(*) FROM audit_log WHERE action='product.price_changed'", [], |r| r.get(0)).unwrap() };
        assert_eq!(count(&c), 0);
        save_product(&c, 1, body(1800, "Frites")).unwrap();
        assert_eq!(count(&c), 1);
        let d: String = c.query_row("SELECT details FROM audit_log WHERE action='product.price_changed'", [], |r| r.get(0)).unwrap();
        assert!(d.contains("1500") && d.contains("1800"));
    }

    #[test]
    fn combo_rules() {
        let c = setup();
        let k = cat(&c);
        let burger = simple(&c, k, "Classic", 4500);
        let fries = simple(&c, k, "Frites", 1500);
        let combo = |slots: Value| serde_json::from_value::<ProductIn>(json!({
            "category_id": k, "kind": "combo", "names": {"fr": "Menu"}, "price_cents": 6500, "happy_hour_price_cents": null,
            "tax_id": 1, "station_id": null, "active": true, "slots": slots
        })).unwrap();
        assert!(save_product(&c, 1, combo(json!([]))).is_err(), "needs slots");
        assert!(save_product(&c, 1, combo(json!([{"names": {"fr": "Burger"}, "options": []}]))).is_err(), "slot needs options");
        assert!(save_product(&c, 1, combo(json!([{"names": {"fr": "Burger"}, "options": [
            {"product_id": burger, "is_default": true}, {"product_id": fries, "is_default": true}]}]))).is_err(), "one default");
        assert!(save_product(&c, 1, combo(json!([{"names": {"fr": "X"}, "options": [{"product_id": 999}]}]))).is_err());
        let m1 = save_product(&c, 1, combo(json!([
            {"names": {"fr": "Burger"}, "options": [{"product_id": burger, "is_default": true}]},
            {"names": {"fr": "Accompagnement"}, "options": [{"product_id": fries}]}
        ]))).unwrap();
        assert!(save_product(&c, 1, combo(json!([{"names": {"fr": "X"}, "options": [{"product_id": m1}]}]))).is_err(), "no nested combos");
        let menu = get_menu(&c, true).unwrap();
        let p = menu.products.iter().find(|p| p["id"] == m1).unwrap();
        assert_eq!(p["slots"].as_array().unwrap().len(), 2);
        assert_eq!(p["slots"][0]["options"][0]["is_default"], json!(true));
        // A simple product can't have slots and can't become a combo.
        let mut bad = combo(json!([{"names": {"fr": "Burger"}, "options": [{"product_id": burger}]}]));
        bad.kind = "simple".into();
        assert!(save_product(&c, 1, bad).is_err());
    }

    #[test]
    fn group_rules() {
        let c = setup();
        let g = |min: i64, max: i64, opts: Value| serde_json::from_value::<GroupIn>(json!({
            "names": {"fr": "G"}, "min_select": min, "max_select": max, "active": true, "options": opts
        })).unwrap();
        assert!(save_group(&c, 1, g(2, 1, json!([{"names": {"fr": "a"}, "kind": "choice"}]))).is_err(), "min > max");
        assert!(save_group(&c, 1, g(0, 1, json!([]))).is_err(), "no options");
        assert!(save_group(&c, 1, g(2, 2, json!([{"names": {"fr": "a"}, "kind": "choice"}]))).is_err(), "min unreachable");
        assert!(save_group(&c, 1, g(0, 3, json!([{"names": {"fr": "a"}, "kind": "remove", "price_cents": 100}]))).is_err(), "remove is free");
        assert!(save_group(&c, 1, g(0, 3, json!([{"names": {"fr": "a"}, "kind": "bogus"}]))).is_err());
        assert!(save_group(&c, 1, g(0, 5, json!([{"names": {"fr": "Fromage"}, "kind": "extra", "price_cents": 500, "max_qty": 3}]))).is_ok());
    }

    #[test]
    fn eighty_six_and_reorder() {
        let c = setup();
        let k = cat(&c);
        let a = simple(&c, k, "A", 100);
        let b = simple(&c, k, "B", 100);
        set_available(&c, 1, a, false).unwrap();
        let m = get_menu(&c, false).unwrap();
        assert_eq!(m.products.iter().find(|p| p["id"] == a).unwrap()["available"], json!(false));
        reorder(&c, "products", &[b, a]).unwrap();
        let m = get_menu(&c, false).unwrap();
        assert_eq!(m.products[0]["id"], json!(b));
        assert!(reorder(&c, "users", &[1]).is_err());
        assert!(set_available(&c, 1, 9999, false).is_err());
    }

    #[test]
    fn unknown_refs_rejected() {
        let c = setup();
        let k = cat(&c);
        let base = |tax: i64, station: Option<i64>, cat: i64| serde_json::from_value::<ProductIn>(json!({
            "category_id": cat, "kind": "simple", "names": {"fr": "X"}, "price_cents": 100,
            "happy_hour_price_cents": null, "tax_id": tax, "station_id": station, "active": true
        })).unwrap();
        assert!(save_product(&c, 1, base(99, None, k)).is_err());
        assert!(save_product(&c, 1, base(1, Some(99), k)).is_err());
        assert!(save_product(&c, 1, base(1, None, 99)).is_err());
        let mut neg = base(1, None, k);
        neg.price_cents = -1;
        assert!(save_product(&c, 1, neg).is_err());
        let mut photo = base(1, None, k);
        photo.photo = "http://evil/x.png".into();
        assert!(save_product(&c, 1, photo).is_err());
    }
}
