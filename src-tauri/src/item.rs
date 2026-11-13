//! Server-side port of the item rules in `src/lib/menu.ts`: validate a configuration
//! (variant, modifiers, combo choices) against the stored menu, price it, and describe it.

use crate::error::{ApiError, ApiResult};
use crate::menu;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModSel {
    pub option_id: i64,
    pub qty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemConfig {
    pub product_id: i64,
    pub variant_id: Option<i64>,
    #[serde(default)]
    pub modifiers: Vec<ModSel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combo: Option<Vec<ItemConfig>>,
}

#[derive(Deserialize, Clone)]
struct Variant {
    id: i64,
    names: Value,
    price_cents: i64,
    happy_hour_price_cents: Option<i64>,
}
#[derive(Deserialize, Clone)]
struct Opt {
    id: i64,
    names: Value,
    kind: String,
    price_cents: i64,
    max_qty: i64,
}
#[derive(Deserialize, Clone)]
struct Group {
    id: i64,
    names: Value,
    min_select: i64,
    max_select: i64,
    options: Vec<Opt>,
}
#[derive(Deserialize, Clone)]
struct SlotOpt {
    product_id: i64,
    variant_id: Option<i64>,
    extra_cents: i64,
}
#[derive(Deserialize, Clone)]
struct Slot {
    options: Vec<SlotOpt>,
}
#[derive(Deserialize, Clone)]
pub struct Product {
    pub id: i64,
    pub kind: String,
    pub names: Value,
    price_cents: i64,
    happy_hour_price_cents: Option<i64>,
    pub tax_id: i64,
    pub station_id: Option<i64>,
    available: bool,
    variants: Vec<Variant>,
    group_ids: Vec<i64>,
    slots: Vec<Slot>,
}
#[derive(Deserialize)]
struct MenuData {
    products: Vec<Product>,
    modifier_groups: Vec<Group>,
}

/// Active menu, indexed. Load once per request.
pub struct MenuIndex {
    products: HashMap<i64, Product>,
    groups: HashMap<i64, Group>,
    option_group: HashMap<i64, i64>,
}

impl MenuIndex {
    pub fn load(conn: &Connection) -> ApiResult<Self> {
        let data: MenuData = serde_json::from_value(json!(menu::get_menu(conn, false)?))
            .map_err(|e| ApiError::new("internal", format!("menu decode: {e}")))?;
        let mut option_group = HashMap::new();
        for g in &data.modifier_groups {
            for o in &g.options {
                option_group.insert(o.id, g.id);
            }
        }
        Ok(Self {
            products: data.products.into_iter().map(|p| (p.id, p)).collect(),
            groups: data.modifier_groups.into_iter().map(|g| (g.id, g)).collect(),
            option_group,
        })
    }

    pub fn product(&self, id: i64) -> ApiResult<&Product> {
        self.products.get(&id).ok_or_else(|| ApiError::invalid(format!("product {id} is not on the menu")))
    }

    fn groups_of(&self, p: &Product) -> Vec<&Group> {
        p.group_ids.iter().filter_map(|id| self.groups.get(id)).collect()
    }

    fn option(&self, id: i64) -> Option<(&Opt, &Group)> {
        let g = self.groups.get(self.option_group.get(&id)?)?;
        Some((g.options.iter().find(|o| o.id == id)?, g))
    }
}

pub fn pick(names: &Value, lang: &str) -> String {
    for l in [lang, "fr", "en", "ar"] {
        if let Some(s) = names.get(l).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    String::new()
}

/// Same rules as `validateConfig` in menu.ts. Returns the first problem as an `invalid` error.
pub fn validate(ix: &MenuIndex, cfg: &ItemConfig) -> ApiResult<()> {
    let p = ix.product(cfg.product_id)?;
    let name = pick(&p.names, "fr");
    if !p.available {
        return Err(ApiError::new("unavailable", format!("{name} is out of stock")));
    }
    if !p.variants.is_empty() && !p.variants.iter().any(|v| Some(v.id) == cfg.variant_id) {
        return Err(ApiError::invalid(format!("{name}: choose a size")));
    }
    if p.variants.is_empty() && cfg.variant_id.is_some() {
        return Err(ApiError::invalid(format!("{name}: unknown size")));
    }
    let groups = ix.groups_of(p);
    for m in &cfg.modifiers {
        let ok = ix.option(m.option_id).map(|(_, g)| groups.iter().any(|x| x.id == g.id)).unwrap_or(false);
        if !ok {
            return Err(ApiError::invalid(format!("{name}: option {} not allowed", m.option_id)));
        }
        let (o, _) = ix.option(m.option_id).unwrap();
        if m.qty < 1 || m.qty > o.max_qty {
            return Err(ApiError::invalid(format!("{name}: too many of one option")));
        }
    }
    for g in groups {
        let count: i64 = cfg
            .modifiers
            .iter()
            .filter(|m| ix.option_group.get(&m.option_id) == Some(&g.id))
            .map(|m| m.qty)
            .sum();
        if count < g.min_select || count > g.max_select {
            return Err(ApiError::invalid(format!("{name}: {} needs {}-{}", pick(&g.names, "fr"), g.min_select, g.max_select)));
        }
    }
    if p.kind == "combo" {
        let parts = cfg.combo.as_deref().unwrap_or(&[]);
        if parts.len() != p.slots.len() {
            return Err(ApiError::invalid(format!("{name}: every combo step needs a choice")));
        }
        for (slot, part) in p.slots.iter().zip(parts) {
            let ok = slot
                .options
                .iter()
                .any(|o| o.product_id == part.product_id && (o.variant_id.is_none() || o.variant_id == part.variant_id));
            if !ok {
                return Err(ApiError::invalid(format!("{name}: choice not allowed in this combo")));
            }
            if part.combo.is_some() {
                return Err(ApiError::invalid("nested combo"));
            }
            validate(ix, part)?;
        }
    } else if cfg.combo.is_some() {
        return Err(ApiError::invalid(format!("{name} is not a combo")));
    }
    Ok(())
}

fn base_price(p: &Product, variant_id: Option<i64>, happy: bool) -> i64 {
    match p.variants.iter().find(|v| Some(v.id) == variant_id) {
        Some(v) => if happy { v.happy_hour_price_cents.unwrap_or(v.price_cents) } else { v.price_cents },
        None => if happy { p.happy_hour_price_cents.unwrap_or(p.price_cents) } else { p.price_cents },
    }
}

fn modifiers_total(ix: &MenuIndex, cfg: &ItemConfig) -> i64 {
    cfg.modifiers.iter().map(|m| ix.option(m.option_id).map(|(o, _)| o.price_cents).unwrap_or(0) * m.qty).sum()
}

/// Same as `unitPrice` in menu.ts. Call `validate` first.
pub fn unit_price(ix: &MenuIndex, cfg: &ItemConfig, happy: bool) -> ApiResult<i64> {
    let p = ix.product(cfg.product_id)?;
    if p.kind != "combo" {
        return Ok(base_price(p, cfg.variant_id, happy) + modifiers_total(ix, cfg));
    }
    let mut total = base_price(p, None, happy) + modifiers_total(ix, cfg);
    for (i, part) in cfg.combo.as_deref().unwrap_or(&[]).iter().enumerate() {
        let extra = p.slots[i]
            .options
            .iter()
            .find(|o| o.product_id == part.product_id && (o.variant_id.is_none() || o.variant_id == part.variant_id))
            .map(|o| o.extra_cents)
            .unwrap_or(0);
        total += extra + modifiers_total(ix, part);
    }
    Ok(total)
}

/// Same lines as `describe` in menu.ts, for one language.
pub fn describe(ix: &MenuIndex, cfg: &ItemConfig, lang: &str) -> Vec<String> {
    let Ok(p) = ix.product(cfg.product_id) else { return vec![] };
    let mut out = Vec::new();
    if let Some(v) = p.variants.iter().find(|v| Some(v.id) == cfg.variant_id) {
        out.push(pick(&v.names, lang));
    }
    for m in &cfg.modifiers {
        if let Some((o, _)) = ix.option(m.option_id) {
            let prefix = match o.kind.as_str() {
                "extra" => "+ ",
                "remove" => "− ",
                _ => "",
            };
            let qty = if m.qty > 1 { format!(" ×{}", m.qty) } else { String::new() };
            out.push(format!("{prefix}{}{qty}", pick(&o.names, lang)));
        }
    }
    for part in cfg.combo.as_deref().unwrap_or(&[]) {
        if let Ok(sub) = ix.product(part.product_id) {
            let d = describe(ix, part, lang);
            let detail = if d.is_empty() { String::new() } else { format!(" ({})", d.join(", ")) };
            out.push(format!("• {}{detail}", pick(&sub.names, lang)));
        }
    }
    out
}

pub fn describe_all(ix: &MenuIndex, cfg: &ItemConfig) -> Value {
    json!({ "fr": describe(ix, cfg, "fr"), "ar": describe(ix, cfg, "ar"), "en": describe(ix, cfg, "en") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn demo() -> (Connection, MenuIndex) {
        let c = open_in_memory().unwrap();
        c.execute("INSERT INTO users (id, name, role, pin_hash) VALUES (1, 'O', 'owner', 'x')", []).unwrap();
        crate::seed::seed_demo_menu(&c, 1, 1).unwrap();
        let ix = MenuIndex::load(&c).unwrap();
        (c, ix)
    }
    fn pid(c: &Connection, fr: &str) -> i64 {
        c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')=?1", [fr], |r| r.get(0)).unwrap()
    }
    fn vid(c: &Connection, p: i64, fr: &str) -> i64 {
        c.query_row("SELECT id FROM product_variants WHERE product_id=?1 AND json_extract(names,'$.fr')=?2", rusqlite::params![p, fr], |r| r.get(0)).unwrap()
    }
    fn oid(c: &Connection, fr: &str) -> i64 {
        c.query_row("SELECT id FROM modifier_options WHERE json_extract(names,'$.fr')=?1", [fr], |r| r.get(0)).unwrap()
    }

    #[test]
    fn classic_double_with_extras_is_70() {
        let (c, ix) = demo();
        let classic = pid(&c, "Classic Burger");
        let cfg = ItemConfig {
            product_id: classic,
            variant_id: Some(vid(&c, classic, "Double")),
            modifiers: vec![
                ModSel { option_id: oid(&c, "À point"), qty: 1 },
                ModSel { option_id: oid(&c, "Fromage"), qty: 2 },
                ModSel { option_id: oid(&c, "Sans oignon"), qty: 1 },
            ],
            combo: None,
        };
        validate(&ix, &cfg).unwrap();
        assert_eq!(unit_price(&ix, &cfg, false).unwrap(), 7000);
        assert_eq!(describe(&ix, &cfg, "fr"), vec!["Double", "À point", "+ Fromage ×2", "− Sans oignon"]);
        assert_eq!(describe(&ix, &cfg, "ar")[2], "+ جبن ×2");
    }

    #[test]
    fn combo_maxi_potatoes_is_78_and_rules_hold() {
        let (c, ix) = demo();
        let combo = pid(&c, "Menu Classic");
        let classic = pid(&c, "Classic Burger");
        let potatoes = pid(&c, "Potatoes");
        let coca = pid(&c, "Coca-Cola 33cl");
        let part = |p: i64, v: Option<i64>, m: Vec<ModSel>| ItemConfig { product_id: p, variant_id: v, modifiers: m, combo: None };
        let mut cfg = ItemConfig {
            product_id: combo,
            variant_id: None,
            modifiers: vec![ModSel { option_id: oid(&c, "Maxi"), qty: 1 }],
            combo: Some(vec![
                part(classic, Some(vid(&c, classic, "Simple")), vec![ModSel { option_id: oid(&c, "À point"), qty: 1 }]),
                part(potatoes, None, vec![]),
                part(coca, None, vec![]),
            ]),
        };
        validate(&ix, &cfg).unwrap();
        assert_eq!(unit_price(&ix, &cfg, false).unwrap(), 7800);
        // Triple is not allowed in this combo's burger slot.
        cfg.combo.as_mut().unwrap()[0].variant_id = Some(vid(&c, classic, "Triple"));
        assert!(validate(&ix, &cfg).is_err());
        // Missing doneness on the burger part.
        cfg.combo.as_mut().unwrap()[0] = part(classic, Some(vid(&c, classic, "Simple")), vec![]);
        assert!(validate(&ix, &cfg).is_err());
    }

    #[test]
    fn rejects_bad_configs_and_86() {
        let (c, ix) = demo();
        let classic = pid(&c, "Classic Burger");
        let fries = pid(&c, "Frites");
        let no_size = ItemConfig { product_id: classic, variant_id: None, modifiers: vec![ModSel { option_id: oid(&c, "À point"), qty: 1 }], combo: None };
        assert!(validate(&ix, &no_size).is_err());
        let foreign = ItemConfig { product_id: fries, variant_id: Some(vid(&c, fries, "Petite")), modifiers: vec![ModSel { option_id: oid(&c, "Fromage"), qty: 1 }], combo: None };
        assert!(validate(&ix, &foreign).is_err());
        let ok = ItemConfig { product_id: fries, variant_id: Some(vid(&c, fries, "Petite")), modifiers: vec![], combo: None };
        validate(&ix, &ok).unwrap();
        crate::menu::set_available(&c, 1, fries, false).unwrap();
        let ix = MenuIndex::load(&c).unwrap();
        assert_eq!(validate(&ix, &ok).unwrap_err().code, "unavailable");
    }

    #[test]
    fn happy_hour_price() {
        let (c, ix) = demo();
        let classic = pid(&c, "Classic Burger");
        let cfg = ItemConfig { product_id: classic, variant_id: Some(vid(&c, classic, "Simple")), modifiers: vec![ModSel { option_id: oid(&c, "À point"), qty: 1 }], combo: None };
        assert_eq!(unit_price(&ix, &cfg, true).unwrap(), 3900);
        assert_eq!(unit_price(&ix, &cfg, false).unwrap(), 4500);
    }
}
