//! Shop-wide settings stored as JSON values, with an allow-list of keys and types.

use crate::error::{ApiError, ApiResult};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};

#[derive(Clone, Copy)]
enum Kind {
    Str,
    Int { min: i64, max: i64 },
    Bool,
    Enum(&'static [&'static str]),
}

const KEYS: &[(&str, Kind)] = &[
    ("shop_name", Kind::Str),
    ("shop_address", Kind::Str),
    ("shop_phone", Kind::Str),
    ("shop_ice", Kind::Str),
    ("shop_if", Kind::Str),
    ("shop_rc", Kind::Str),
    ("shop_patente", Kind::Str),
    ("shop_logo", Kind::Str), // data: URL, set from the settings screen
    ("receipt_header", Kind::Str),
    ("receipt_footer", Kind::Str),
    ("receipt_show_qr", Kind::Bool),
    ("default_language", Kind::Enum(&["fr", "ar", "en"])),
    ("currency", Kind::Enum(&["MAD"])),
    ("tax_rounding", Kind::Enum(&["per_line", "global"])),
    ("default_tax_id", Kind::Int { min: 1, max: i64::MAX }),
    // 0 = every discount needs a manager PIN.
    ("discount_pin_threshold_bp", Kind::Int { min: 0, max: 10000 }),
    ("auto_lock_minutes", Kind::Int { min: 0, max: 240 }),
    ("order_type_dine_in", Kind::Bool),
    ("order_type_takeaway", Kind::Bool),
    ("order_type_delivery", Kind::Bool),
    ("delivery_fee_cents", Kind::Int { min: 0, max: 100_000 }),
    ("kds_warn_minutes", Kind::Int { min: 1, max: 120 }),
    ("kds_late_minutes", Kind::Int { min: 1, max: 240 }),
    ("loyalty_mode", Kind::Enum(&["none", "stamps", "points"])),
    ("loyalty_stamps_needed", Kind::Int { min: 2, max: 100 }),
    ("loyalty_points_per_mad", Kind::Int { min: 0, max: 100 }),
    ("loyalty_point_value_cents", Kind::Int { min: 0, max: 10_000 }),
    ("stock_restore_on_void", Kind::Bool),
    ("auto_86_on_stockout", Kind::Bool),
    ("backup_daily", Kind::Bool),
    ("backup_keep_days", Kind::Int { min: 1, max: 365 }),
];

pub fn defaults() -> Map<String, Value> {
    let v = json!({
        "shop_name": "BURGER OS",
        "shop_address": "",
        "shop_phone": "",
        "shop_ice": "",
        "shop_if": "",
        "shop_rc": "",
        "shop_patente": "",
        "shop_logo": "",
        "receipt_header": "",
        "receipt_footer": "Merci et à bientôt !",
        "receipt_show_qr": false,
        "default_language": "fr",
        "currency": "MAD",
        "tax_rounding": "per_line",
        "default_tax_id": 1,
        "discount_pin_threshold_bp": 0,
        "auto_lock_minutes": 5,
        "order_type_dine_in": true,
        "order_type_takeaway": true,
        "order_type_delivery": true,
        "delivery_fee_cents": 1000,
        "kds_warn_minutes": 5,
        "kds_late_minutes": 10,
        "loyalty_mode": "none",
        "loyalty_stamps_needed": 10,
        "loyalty_points_per_mad": 1,
        "loyalty_point_value_cents": 10,
        "stock_restore_on_void": true,
        "auto_86_on_stockout": false,
        "backup_daily": true,
        "backup_keep_days": 30
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

fn check(key: &str, value: &Value) -> ApiResult<()> {
    let kind = KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, k)| *k)
        .ok_or_else(|| ApiError::invalid(format!("unknown setting {key}")))?;
    let ok = match kind {
        Kind::Str => value.as_str().map(|s| s.len() <= 2_000_000).unwrap_or(false),
        Kind::Int { min, max } => value.as_i64().map(|n| n >= min && n <= max).unwrap_or(false),
        Kind::Bool => value.is_boolean(),
        Kind::Enum(opts) => value.as_str().map(|s| opts.contains(&s)).unwrap_or(false),
    };
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid(format!("bad value for {key}")))
    }
}

/// All settings, defaults filled in for keys never saved.
pub fn get_all(conn: &Connection) -> ApiResult<Map<String, Value>> {
    let mut out = defaults();
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (k, v) = row?;
        if let Ok(parsed) = serde_json::from_str(&v) {
            out.insert(k, parsed);
        }
    }
    Ok(out)
}

pub fn get(conn: &Connection, key: &str) -> ApiResult<Value> {
    Ok(get_all(conn)?.remove(key).unwrap_or(Value::Null))
}

/// Validates every key first, then writes all of them in one transaction.
/// Returns the keys that actually changed.
pub fn set_many(conn: &Connection, values: &Map<String, Value>) -> ApiResult<Vec<String>> {
    for (k, v) in values {
        check(k, v)?;
    }
    let current = get_all(conn)?;
    // SAVEPOINT nests safely inside a caller's transaction (e.g. first-run setup).
    conn.execute_batch("SAVEPOINT settings_set")?;
    let mut changed = Vec::new();
    let result = (|| -> ApiResult<()> {
        for (k, v) in values {
            if current.get(k) != Some(v) {
                conn.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![k, v.to_string()],
                )?;
                changed.push(k.clone());
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("RELEASE settings_set")?;
            Ok(changed)
        }
        Err(e) => {
            conn.execute_batch("ROLLBACK TO settings_set; RELEASE settings_set")?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn defaults_cover_every_key_and_are_valid() {
        let d = defaults();
        for (k, _) in KEYS {
            let v = d.get(*k).unwrap_or_else(|| panic!("no default for {k}"));
            check(k, v).unwrap_or_else(|_| panic!("default for {k} invalid"));
        }
        assert_eq!(d.len(), KEYS.len());
    }

    #[test]
    fn set_and_get_roundtrip() {
        let c = open_in_memory().unwrap();
        let mut m = Map::new();
        m.insert("shop_name".into(), json!("Burger Lab"));
        m.insert("kds_warn_minutes".into(), json!(7));
        let changed = set_many(&c, &m).unwrap();
        assert_eq!(changed.len(), 2);
        assert_eq!(get(&c, "shop_name").unwrap(), json!("Burger Lab"));
        assert_eq!(get(&c, "kds_warn_minutes").unwrap(), json!(7));
        assert!(set_many(&c, &m).unwrap().is_empty(), "unchanged values are not rewritten");
    }

    #[test]
    fn rejects_unknown_keys_and_bad_types_atomically() {
        let c = open_in_memory().unwrap();
        let mut m = Map::new();
        m.insert("shop_name".into(), json!("X"));
        m.insert("kds_warn_minutes".into(), json!("seven"));
        assert!(set_many(&c, &m).is_err());
        assert_eq!(get(&c, "shop_name").unwrap(), json!("BURGER OS"), "nothing written on error");

        let mut m = Map::new();
        m.insert("hack".into(), json!(1));
        assert!(set_many(&c, &m).is_err());
        let mut m = Map::new();
        m.insert("discount_pin_threshold_bp".into(), json!(10001));
        assert!(set_many(&c, &m).is_err());
        let mut m = Map::new();
        m.insert("default_language".into(), json!("de"));
        assert!(set_many(&c, &m).is_err());
    }
}
