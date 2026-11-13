//! Demo menu so the shop can test immediately. Built through the normal validated save
//! functions, so the seed also proves the validation accepts a realistic menu.

use crate::error::ApiResult;
use crate::menu::{self, CategoryIn, GroupIn, ProductIn};
use rusqlite::Connection;
use serde_json::{json, Value};

fn n(fr: &str, ar: &str, en: &str) -> Value {
    json!({ "fr": fr, "ar": ar, "en": en })
}

fn station(conn: &Connection, code: &str) -> ApiResult<i64> {
    Ok(conn.query_row("SELECT id FROM stations WHERE code=?1", [code], |r| r.get(0))?)
}

fn variant_id(conn: &Connection, product: i64, fr: &str) -> ApiResult<i64> {
    Ok(conn.query_row(
        "SELECT id FROM product_variants WHERE product_id=?1 AND json_extract(names,'$.fr')=?2",
        rusqlite::params![product, fr],
        |r| r.get(0),
    )?)
}

/// Insert the demo menu. Does nothing and returns false if any product already exists.
pub fn seed_demo_menu(conn: &Connection, user_id: i64, tax_id: i64) -> ApiResult<bool> {
    if menu::product_count(conn)? > 0 {
        return Ok(false);
    }
    let tx = crate::db::atomic(conn)?;
    let c: &Connection = &tx;
    let grill = station(c, "grill")?;
    let fryer = station(c, "fryer")?;
    let drinks = station(c, "drinks")?;
    let assembly = station(c, "assembly")?;

    let cat = |names: Value, color: &str, icon: &str, sort: i64| -> ApiResult<i64> {
        menu::save_category(c, user_id, CategoryIn { id: None, names, color: color.into(), icon: icon.into(), sort, active: true })
    };
    let c_burgers = cat(n("Burgers", "برغر", "Burgers"), "#f59e0b", "burger", 1)?;
    let c_sides = cat(n("Accompagnements", "مقبلات", "Sides"), "#eab308", "fries", 2)?;
    let c_drinks = cat(n("Boissons", "مشروبات", "Drinks"), "#0ea5e9", "drink", 3)?;
    let c_desserts = cat(n("Desserts", "حلويات", "Desserts"), "#ec4899", "dessert", 4)?;
    let c_sauces = cat(n("Sauces", "صلصات", "Sauces"), "#ef4444", "sauce", 5)?;
    let c_combos = cat(n("Menus", "وجبات", "Combos"), "#22c55e", "combo", 0)?;

    let group = |v: Value| -> ApiResult<i64> { menu::save_group(c, user_id, serde_json::from_value::<GroupIn>(v)?) };
    let g_cook = group(json!({"names": n("Cuisson", "درجة الطهي", "Doneness"), "min_select": 1, "max_select": 1, "active": true, "sort": 1,
        "options": [
            {"names": n("Saignant", "نصف مطهو", "Rare"), "kind": "choice"},
            {"names": n("À point", "متوسط", "Medium"), "kind": "choice", "is_default": true},
            {"names": n("Bien cuit", "مطهو جيدًا", "Well done"), "kind": "choice"}]}))?;
    let g_extras = group(json!({"names": n("Suppléments", "إضافات", "Extras"), "min_select": 0, "max_select": 5, "active": true, "sort": 2,
        "options": [
            {"names": n("Fromage", "جبن", "Cheese"), "kind": "extra", "price_cents": 500, "max_qty": 3},
            {"names": n("Bacon de bœuf", "لحم مقدد بقري", "Beef bacon"), "kind": "extra", "price_cents": 800, "max_qty": 2},
            {"names": n("Œuf", "بيض", "Egg"), "kind": "extra", "price_cents": 500},
            {"names": n("Jalapeños", "هالبينو", "Jalapeños"), "kind": "extra", "price_cents": 400},
            {"names": n("Oignons caramélisés", "بصل مكرمل", "Caramelised onions"), "kind": "extra", "price_cents": 400},
            {"names": n("Avocat", "أفوكادو", "Avocado"), "kind": "extra", "price_cents": 800}]}))?;
    let g_remove = group(json!({"names": n("Retirer", "بدون", "Remove"), "min_select": 0, "max_select": 6, "active": true, "sort": 3,
        "options": [
            {"names": n("Sans oignon", "بدون بصل", "No onion"), "kind": "remove"},
            {"names": n("Sans sauce", "بدون صلصة", "No sauce"), "kind": "remove"},
            {"names": n("Sans cornichons", "بدون مخلل", "No pickles"), "kind": "remove"},
            {"names": n("Sans tomate", "بدون طماطم", "No tomato"), "kind": "remove"},
            {"names": n("Sans salade", "بدون خس", "No lettuce"), "kind": "remove"},
            {"names": n("Sans fromage", "بدون جبن", "No cheese"), "kind": "remove"}]}))?;
    let g_fry_sauce = group(json!({"names": n("Sauces offertes", "صلصات مجانية", "Free sauces"), "min_select": 0, "max_select": 2, "active": true, "sort": 4,
        "options": [
            {"names": n("Ketchup", "كاتشب", "Ketchup"), "kind": "choice"},
            {"names": n("Mayonnaise", "مايونيز", "Mayonnaise"), "kind": "choice"},
            {"names": n("Algérienne", "جزائرية", "Algerian"), "kind": "choice"},
            {"names": n("Harissa", "هريسة", "Harissa"), "kind": "choice"},
            {"names": n("Barbecue", "باربكيو", "Barbecue"), "kind": "choice"},
            {"names": n("Samouraï", "ساموراي", "Samurai"), "kind": "choice"}]}))?;
    let g_size = group(json!({"names": n("Taille du menu", "حجم الوجبة", "Combo size"), "min_select": 1, "max_select": 1, "active": true, "sort": 5,
        "options": [
            {"names": n("Normal", "عادي", "Regular"), "kind": "choice", "is_default": true},
            {"names": n("Maxi", "ماكسي", "Large"), "kind": "choice", "price_cents": 1000}]}))?;
    let g_shake = group(json!({"names": n("Parfum", "النكهة", "Flavour"), "min_select": 1, "max_select": 1, "active": true, "sort": 6,
        "options": [
            {"names": n("Vanille", "فانيليا", "Vanilla"), "kind": "choice", "is_default": true},
            {"names": n("Chocolat", "شوكولاتة", "Chocolate"), "kind": "choice"},
            {"names": n("Fraise", "فراولة", "Strawberry"), "kind": "choice"}]}))?;

    let mut sort = 0;
    let mut product = |cat: i64, names: Value, price: i64, hh: Option<i64>, station: i64, variants: Value, groups: Vec<i64>| -> ApiResult<i64> {
        sort += 1;
        menu::save_product(c, user_id, serde_json::from_value::<ProductIn>(json!({
            "category_id": cat, "kind": "simple", "names": names, "price_cents": price, "happy_hour_price_cents": hh,
            "tax_id": tax_id, "station_id": station, "active": true, "sort": sort, "variants": variants, "group_ids": groups
        }))?)
    };
    // `hh`: happy-hour price of the Single size (the size price always wins over the product price).
    let sdt = |s: i64, d: i64, t: Option<i64>, hh: Option<i64>| {
        let mut v = vec![
            json!({"names": n("Simple", "عادي", "Single"), "price_cents": s, "happy_hour_price_cents": hh}),
            json!({"names": n("Double", "مزدوج", "Double"), "price_cents": d, "happy_hour_price_cents": null}),
        ];
        if let Some(t) = t {
            v.push(json!({"names": n("Triple", "ثلاثي", "Triple"), "price_cents": t, "happy_hour_price_cents": null}));
        }
        json!(v)
    };
    let beef = vec![g_cook, g_extras, g_remove];
    let chicken = vec![g_extras, g_remove];
    let none = json!([]);

    // 10 burgers
    let classic = product(c_burgers, n("Classic Burger", "برغر كلاسيك", "Classic Burger"), 4500, None, grill, sdt(4500, 6000, Some(7500), Some(3900)), beef.clone())?;
    let cheese = product(c_burgers, n("Cheeseburger", "تشيز برغر", "Cheeseburger"), 5000, None, grill, sdt(5000, 6500, Some(8000), Some(4200)), beef.clone())?;
    let bacon = product(c_burgers, n("Bacon Cheese", "بيكون تشيز", "Bacon Cheese"), 6000, None, grill, sdt(6000, 7500, Some(9000), None), beef.clone())?;
    let bbq = product(c_burgers, n("BBQ Smoky", "باربكيو مدخن", "BBQ Smoky"), 6000, None, grill, sdt(6000, 7500, None, None), beef.clone())?;
    let spicy = product(c_burgers, n("Spicy Jalapeño", "حار هالبينو", "Spicy Jalapeño"), 5500, None, grill, sdt(5500, 7000, None, None), beef.clone())?;
    let mushroom = product(c_burgers, n("Mushroom Swiss", "فطر وجبن سويسري", "Mushroom Swiss"), 5800, None, grill, sdt(5800, 7300, None, None), beef.clone())?;
    let kefta = product(c_burgers, n("Kefta Burger", "برغر كفتة", "Kefta Burger"), 5000, None, grill, none.clone(), beef.clone())?;
    let crispy = product(c_burgers, n("Crispy Chicken", "دجاج مقرمش", "Crispy Chicken"), 4500, None, fryer, none.clone(), chicken.clone())?;
    let grilled = product(c_burgers, n("Grilled Chicken", "دجاج مشوي", "Grilled Chicken"), 4500, None, grill, none.clone(), chicken.clone())?;
    let veggie = product(c_burgers, n("Veggie Burger", "برغر نباتي", "Veggie Burger"), 4200, None, grill, none.clone(), vec![g_extras, g_remove])?;

    // Sides
    let fries = product(c_sides, n("Frites", "بطاطس مقلية", "Fries"), 1500, None, fryer, json!([
        {"names": n("Petite", "صغير", "Small"), "price_cents": 1200, "happy_hour_price_cents": null},
        {"names": n("Moyenne", "متوسط", "Medium"), "price_cents": 1500, "happy_hour_price_cents": null},
        {"names": n("Grande", "كبير", "Large"), "price_cents": 2000, "happy_hour_price_cents": null}]), vec![g_fry_sauce])?;
    let potatoes = product(c_sides, n("Potatoes", "بطاطس ودجز", "Potato wedges"), 1800, None, fryer, none.clone(), vec![g_fry_sauce])?;
    let rings = product(c_sides, n("Onion rings", "حلقات البصل", "Onion rings"), 2000, None, fryer, none.clone(), vec![g_fry_sauce])?;
    let _nuggets = product(c_sides, n("Nuggets x6", "ناغتس ×6", "Nuggets x6"), 2800, None, fryer, none.clone(), vec![g_fry_sauce])?;
    let coleslaw = product(c_sides, n("Salade coleslaw", "سلطة كولسلو", "Coleslaw"), 1500, None, assembly, none.clone(), vec![])?;

    // Drinks
    let coca = product(c_drinks, n("Coca-Cola 33cl", "كوكا كولا 33 سل", "Coca-Cola 33cl"), 1200, None, drinks, none.clone(), vec![])?;
    let zero = product(c_drinks, n("Coca-Cola Zero 33cl", "كوكا كولا زيرو 33 سل", "Coke Zero 33cl"), 1200, None, drinks, none.clone(), vec![])?;
    let fanta = product(c_drinks, n("Fanta 33cl", "فانتا 33 سل", "Fanta 33cl"), 1200, None, drinks, none.clone(), vec![])?;
    let sprite = product(c_drinks, n("Sprite 33cl", "سبرايت 33 سل", "Sprite 33cl"), 1200, None, drinks, none.clone(), vec![])?;
    let hawai = product(c_drinks, n("Hawaï 33cl", "هاواي 33 سل", "Hawai 33cl"), 1200, None, drinks, none.clone(), vec![])?;
    let water = product(c_drinks, n("Eau minérale 50cl", "ماء معدني 50 سل", "Still water 50cl"), 700, None, drinks, none.clone(), vec![])?;
    let juice = product(c_drinks, n("Jus d'orange frais", "عصير برتقال طازج", "Fresh orange juice"), 2000, None, drinks, none.clone(), vec![])?;
    let _shake = product(c_drinks, n("Milkshake", "ميلك شيك", "Milkshake"), 3000, None, drinks, none.clone(), vec![g_shake])?;

    // Desserts
    product(c_desserts, n("Tiramisu", "تيراميسو", "Tiramisu"), 2500, None, assembly, none.clone(), vec![])?;
    product(c_desserts, n("Cookie", "كوكي", "Cookie"), 1200, None, assembly, none.clone(), vec![])?;
    product(c_desserts, n("Sundae caramel", "سانداي كراميل", "Caramel sundae"), 2000, None, assembly, none.clone(), vec![])?;
    product(c_desserts, n("Brownie", "براوني", "Brownie"), 1800, None, assembly, none.clone(), vec![])?;

    // Sauces (paid extras sold on their own)
    product(c_sauces, n("Ketchup", "كاتشب", "Ketchup"), 300, None, assembly, none.clone(), vec![])?;
    product(c_sauces, n("Mayonnaise", "مايونيز", "Mayonnaise"), 300, None, assembly, none.clone(), vec![])?;
    product(c_sauces, n("Algérienne", "جزائرية", "Algerian sauce"), 400, None, assembly, none.clone(), vec![])?;
    product(c_sauces, n("Barbecue", "باربكيو", "Barbecue sauce"), 400, None, assembly, none.clone(), vec![])?;
    product(c_sauces, n("Cheddar fondu", "شيدر ذائب", "Melted cheddar"), 600, None, assembly, none.clone(), vec![])?;

    // Combos: burger + side + drink, with a Maxi size upgrade.
    let fries_m = variant_id(c, fries, "Moyenne")?;
    let side_slot = json!({"names": n("Accompagnement", "المقبلات", "Side"), "options": [
        {"product_id": fries, "variant_id": fries_m, "is_default": true},
        {"product_id": potatoes, "extra_cents": 300},
        {"product_id": rings, "extra_cents": 500},
        {"product_id": coleslaw}]});
    let drink_slot = json!({"names": n("Boisson", "المشروب", "Drink"), "options": [
        {"product_id": coca, "is_default": true}, {"product_id": zero}, {"product_id": fanta},
        {"product_id": sprite}, {"product_id": hawai}, {"product_id": water},
        {"product_id": juice, "extra_cents": 600}]});
    let mut combo = |names: Value, price: i64, burgers: Value| -> ApiResult<i64> {
        sort += 1;
        menu::save_product(c, user_id, serde_json::from_value::<ProductIn>(json!({
            "category_id": c_combos, "kind": "combo", "names": names, "price_cents": price, "happy_hour_price_cents": null,
            "tax_id": tax_id, "station_id": null, "active": true, "sort": sort, "group_ids": [g_size],
            "slots": [{"names": n("Burger", "البرغر", "Burger"), "options": burgers}, side_slot.clone(), drink_slot.clone()]
        }))?)
    };
    let v = |p: i64, fr: &str| variant_id(c, p, fr);
    combo(n("Menu Classic", "وجبة كلاسيك", "Classic Combo"), 6500, json!([
        {"product_id": classic, "variant_id": v(classic, "Simple")?, "is_default": true},
        {"product_id": classic, "variant_id": v(classic, "Double")?, "extra_cents": 1500}]))?;
    combo(n("Menu Cheese", "وجبة تشيز", "Cheese Combo"), 7000, json!([
        {"product_id": cheese, "variant_id": v(cheese, "Simple")?, "is_default": true},
        {"product_id": cheese, "variant_id": v(cheese, "Double")?, "extra_cents": 1500}]))?;
    combo(n("Menu Chicken", "وجبة دجاج", "Chicken Combo"), 6500, json!([
        {"product_id": crispy, "is_default": true}, {"product_id": grilled}]))?;
    combo(n("Menu Signature", "وجبة سيغنتشر", "Signature Combo"), 8500, json!([
        {"product_id": bacon, "variant_id": v(bacon, "Simple")?, "is_default": true},
        {"product_id": bbq, "variant_id": v(bbq, "Simple")?},
        {"product_id": spicy, "variant_id": v(spicy, "Simple")?},
        {"product_id": mushroom, "variant_id": v(mushroom, "Simple")?},
        {"product_id": kefta}, {"product_id": veggie}]))?;

    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn demo_menu_seeds_once_and_is_complete() {
        let c = open_in_memory().unwrap();
        c.execute("INSERT INTO users (id, name, role, pin_hash) VALUES (1, 'O', 'owner', 'x')", []).unwrap();
        assert!(seed_demo_menu(&c, 1, 1).unwrap());
        assert!(!seed_demo_menu(&c, 1, 1).unwrap(), "second call is a no-op");
        let m = menu::get_menu(&c, false).unwrap();
        assert_eq!(m.categories.len(), 6);
        let burgers_cat = m.categories.iter().find(|c| c["names"]["fr"] == "Burgers").unwrap()["id"].clone();
        assert_eq!(m.products.iter().filter(|p| p["category_id"] == burgers_cat).count(), 10);
        assert_eq!(m.products.iter().filter(|p| p["kind"] == "combo").count(), 4);
        assert_eq!(m.modifier_groups.len(), 6);
        // Every product has Arabic and English names.
        for p in &m.products {
            assert!(p["names"]["ar"].is_string() && p["names"]["en"].is_string(), "{}", p["names"]);
        }
        // Every combo slot has a default.
        for p in m.products.iter().filter(|p| p["kind"] == "combo") {
            for s in p["slots"].as_array().unwrap() {
                assert!(s["options"].as_array().unwrap().iter().any(|o| o["is_default"] == true));
            }
        }
    }
}
