//! Single RPC entry point: `dispatch(core, token, method, params)`.
//!
//! The Tauri command on the Host calls this directly. In Phase 5 the LAN HTTP server
//! calls the same function for Client devices, so every rule (auth, permissions, audit)
//! lives in one place.

use crate::auth::{self, SessionUser, PERMISSIONS, ROLES};
use crate::error::{ApiError, ApiResult};
use crate::{audit, cash, menu, orders, printing, settings};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Sessions idle longer than this must log in again (UI auto-lock is usually shorter).
const SESSION_IDLE: Duration = Duration::from_secs(12 * 60 * 60);

pub struct Core {
    pub conn: Arc<Mutex<Connection>>,
    sessions: Mutex<HashMap<String, (i64, Instant)>>,
}

impl Core {
    pub fn new(conn: Connection) -> Self {
        Self { conn: Arc::new(Mutex::new(conn)), sessions: Mutex::new(HashMap::new()) }
    }

    fn session_user_id(&self, token: Option<&str>) -> ApiResult<i64> {
        let token = token.ok_or_else(ApiError::unauthorized)?;
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(token) {
            Some((uid, seen)) if seen.elapsed() < SESSION_IDLE => {
                *seen = Instant::now();
                Ok(*uid)
            }
            Some(_) => {
                sessions.remove(token);
                Err(ApiError::unauthorized())
            }
            None => Err(ApiError::unauthorized()),
        }
    }

    fn drop_sessions_of(&self, user_id: i64) {
        self.sessions.lock().unwrap().retain(|_, (uid, _)| *uid != user_id);
    }
}

fn p<T: for<'de> Deserialize<'de>>(params: Value) -> ApiResult<T> {
    Ok(serde_json::from_value(params)?)
}

fn require(user: &SessionUser, perm: &str) -> ApiResult<()> {
    if user.permissions.iter().any(|p| p == perm) {
        Ok(())
    } else {
        Err(ApiError::forbidden(perm))
    }
}

fn user_count(conn: &Connection) -> ApiResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?)
}

/// Dispatch, then tell every screen (local and paired devices) what changed.
pub fn handle(core: &Core, token: Option<&str>, method: &str, params: Value) -> ApiResult<Value> {
    let r = dispatch(core, token, method, params);
    if r.is_ok() {
        if let Some(topic) = crate::net::topic_of(method) {
            crate::net::publish(topic);
        }
    }
    r
}

pub fn dispatch(core: &Core, token: Option<&str>, method: &str, params: Value) -> ApiResult<Value> {
    // Methods that work without a session.
    match method {
        "app.status" => {
            let conn = core.conn.lock().unwrap();
            let s = settings::get_all(&conn)?;
            return Ok(json!({
                "setup_done": user_count(&conn)? > 0,
                "shop_name": s["shop_name"],
                "default_language": s["default_language"],
                "version": env!("CARGO_PKG_VERSION"),
            }));
        }
        "setup.init" => {
            let conn = core.conn.lock().unwrap();
            return setup_init(&conn, p(params)?);
        }
        "display.board" => {
            // Public: only order numbers, for the customer-facing screen.
            let conn = core.conn.lock().unwrap();
            return crate::kds::display_board(&conn);
        }
        "auth.users" => {
            let conn = core.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, name, role FROM users WHERE active = 1
                 ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'manager' THEN 1 WHEN 'cashier' THEN 2 ELSE 3 END, name",
            )?;
            let rows = stmt
                .query_map([], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "role": r.get::<_, String>(2)?})))?
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Value::Array(rows));
        }
        "auth.login" => {
            #[derive(Deserialize)]
            struct P {
                user_id: i64,
                pin: String,
            }
            let P { user_id, pin } = p(params)?;
            let conn = core.conn.lock().unwrap();
            let user = auth::check_login(&conn, user_id, &pin)?;
            let token = auth::new_token();
            core.sessions.lock().unwrap().insert(token.clone(), (user.id, Instant::now()));
            return Ok(json!({ "token": token, "user": user }));
        }
        _ => {}
    }

    let uid = core.session_user_id(token)?;
    let conn = core.conn.lock().unwrap();
    // Reload on every call so a deactivated user or a permission change takes effect at once.
    let me = auth::load_session_user(&conn, uid).map_err(|_| ApiError::unauthorized())?;

    match method {
        "auth.me" => Ok(json!(me)),
        "auth.logout" => {
            if let Some(t) = token {
                core.sessions.lock().unwrap().remove(t);
            }
            audit::log(&conn, Some(me.id), "auth.logout", None, None, None)?;
            Ok(Value::Null)
        }
        "auth.authorize" => {
            #[derive(Deserialize)]
            struct P {
                pin: String,
                permission: String,
                reason: Option<String>,
            }
            let P { pin, permission, reason } = p(params)?;
            if !PERMISSIONS.contains(&permission.as_str()) {
                return Err(ApiError::invalid("unknown permission"));
            }
            let approver = auth::authorize_override(&conn, &pin, &permission)?;
            audit::log(
                &conn,
                Some(approver.id),
                "auth.override",
                Some("user"),
                Some(&me.id.to_string()),
                Some(json!({ "permission": permission, "requested_by": me.name, "reason": reason })),
            )?;
            Ok(json!({ "approver_id": approver.id, "approver_name": approver.name }))
        }
        "auth.change_pin" => {
            #[derive(Deserialize)]
            struct P {
                current_pin: String,
                new_pin: String,
            }
            let P { current_pin, new_pin } = p(params)?;
            let hash: String = conn.query_row("SELECT pin_hash FROM users WHERE id=?1", [me.id], |r| r.get(0))?;
            if !auth::verify_pin(&current_pin, &hash) {
                return Err(ApiError::new("wrong_pin", "current PIN is wrong"));
            }
            conn.execute(
                "UPDATE users SET pin_hash=?2, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![me.id, auth::hash_pin(&new_pin)?],
            )?;
            audit::log(&conn, Some(me.id), "user.pin_changed", Some("user"), Some(&me.id.to_string()), None)?;
            Ok(Value::Null)
        }
        "users.set_prefs" => {
            #[derive(Deserialize)]
            struct P {
                language: String,
                theme: String,
            }
            let P { language, theme } = p(params)?;
            conn.execute(
                "UPDATE users SET language=?2, theme=?3, updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![me.id, language, theme],
            )
            .map_err(|_| ApiError::invalid("bad language or theme"))?;
            Ok(json!(auth::load_session_user(&conn, me.id)?))
        }

        "settings.get" => Ok(Value::Object(settings::get_all(&conn)?)),
        "settings.set" => {
            require(&me, "settings.manage")?;
            let values: Map<String, Value> = p(params)?;
            let changed = settings::set_many(&conn, &values)?;
            if !changed.is_empty() {
                audit::log(&conn, Some(me.id), "settings.changed", Some("settings"), None, Some(json!({ "keys": changed })))?;
            }
            Ok(Value::Object(settings::get_all(&conn)?))
        }

        "taxes.list" => {
            let mut stmt = conn.prepare("SELECT id, name, rate_bp, price_include, active FROM taxes ORDER BY active DESC, rate_bp, name")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?,
                        "rate_bp": r.get::<_, i64>(2)?, "price_include": r.get::<_, bool>(3)?,
                        "active": r.get::<_, bool>(4)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
        "taxes.save" => {
            require(&me, "settings.manage")?;
            #[derive(Deserialize)]
            struct P {
                id: Option<i64>,
                name: String,
                rate_bp: i64,
                price_include: bool,
                active: bool,
            }
            let t: P = p(params)?;
            if t.name.trim().is_empty() || !(0..=10000).contains(&t.rate_bp) {
                return Err(ApiError::invalid("tax name and rate 0-100% required"));
            }
            let id = match t.id {
                Some(id) => {
                    let n = conn.execute(
                        "UPDATE taxes SET name=?2, rate_bp=?3, price_include=?4, active=?5 WHERE id=?1",
                        params![id, t.name.trim(), t.rate_bp, t.price_include, t.active],
                    )?;
                    if n == 0 {
                        return Err(ApiError::not_found("tax"));
                    }
                    id
                }
                None => {
                    conn.execute(
                        "INSERT INTO taxes (name, rate_bp, price_include, active) VALUES (?1, ?2, ?3, ?4)",
                        params![t.name.trim(), t.rate_bp, t.price_include, t.active],
                    )?;
                    conn.last_insert_rowid()
                }
            };
            audit::log(
                &conn,
                Some(me.id),
                "tax.saved",
                Some("tax"),
                Some(&id.to_string()),
                Some(json!({ "name": t.name, "rate_bp": t.rate_bp, "price_include": t.price_include, "active": t.active })),
            )?;
            Ok(json!({ "id": id }))
        }

        "users.list" => {
            require(&me, "users.manage")?;
            let mut stmt = conn.prepare(
                "SELECT id, name, role, language, theme, active, created_at FROM users ORDER BY active DESC, name",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "role": r.get::<_, String>(2)?,
                        "language": r.get::<_, String>(3)?, "theme": r.get::<_, String>(4)?,
                        "active": r.get::<_, bool>(5)?, "created_at": r.get::<_, String>(6)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
        "users.save" => {
            require(&me, "users.manage")?;
            let r = save_user(&conn, &me, p(params)?)?;
            if let Some(id) = r.get("deactivated").and_then(|v| v.as_i64()) {
                core.drop_sessions_of(id);
            }
            Ok(r)
        }

        "permissions.get" => {
            let mut roles = Map::new();
            for role in ROLES {
                roles.insert(role.to_string(), json!(auth::permissions_for_role(&conn, role)?));
            }
            Ok(json!({ "permissions": PERMISSIONS, "roles": roles }))
        }
        "permissions.set" => {
            if me.role != "owner" {
                return Err(ApiError::forbidden("owner"));
            }
            #[derive(Deserialize)]
            struct P {
                role: String,
                permissions: Vec<String>,
            }
            let P { role, permissions } = p(params)?;
            if !["manager", "cashier", "kitchen"].contains(&role.as_str()) {
                return Err(ApiError::invalid("only manager, cashier, kitchen roles are editable"));
            }
            if let Some(bad) = permissions.iter().find(|x| !PERMISSIONS.contains(&x.as_str())) {
                return Err(ApiError::invalid(format!("unknown permission {bad}")));
            }
            let tx = crate::db::atomic(&conn)?;
            tx.execute("DELETE FROM role_permissions WHERE role=?1", [&role])?;
            for perm in &permissions {
                tx.execute("INSERT OR IGNORE INTO role_permissions (role, permission) VALUES (?1, ?2)", params![role, perm])?;
            }
            tx.commit()?;
            audit::log(&conn, Some(me.id), "permissions.changed", Some("role"), Some(&role), Some(json!({ "permissions": permissions })))?;
            Ok(json!(auth::permissions_for_role(&conn, &role)?))
        }

        // ---------------------------------------------------------- menu
        "menu.get" => {
            #[derive(Deserialize, Default)]
            struct P {
                #[serde(default)]
                all: bool,
            }
            let P { all } = if params.is_null() { P::default() } else { p(params)? };
            if all {
                require(&me, "menu.manage")?;
            }
            Ok(json!(menu::get_menu(&conn, all)?))
        }
        "categories.save" => {
            require(&me, "menu.manage")?;
            Ok(json!({ "id": menu::save_category(&conn, me.id, p(params)?)? }))
        }
        "modifier_groups.save" => {
            require(&me, "menu.manage")?;
            Ok(json!({ "id": menu::save_group(&conn, me.id, p(params)?)? }))
        }
        "products.save" => {
            require(&me, "menu.manage")?;
            Ok(json!({ "id": menu::save_product(&conn, me.id, p(params)?)? }))
        }
        "products.set_available" => {
            // Cashiers and the kitchen can 86 an item during service.
            if !["menu.manage", "pos.sell", "kds.use"].iter().any(|x| me.permissions.iter().any(|p| p == x)) {
                return Err(ApiError::forbidden("menu.manage"));
            }
            #[derive(Deserialize)]
            struct P {
                id: i64,
                available: bool,
            }
            let P { id, available } = p(params)?;
            menu::set_available(&conn, me.id, id, available)?;
            Ok(Value::Null)
        }
        "menu.reorder" => {
            require(&me, "menu.manage")?;
            #[derive(Deserialize)]
            struct P {
                table: String,
                ids: Vec<i64>,
            }
            let P { table, ids } = p(params)?;
            menu::reorder(&conn, &table, &ids)?;
            Ok(Value::Null)
        }
        "menu.seed_demo" => {
            require(&me, "menu.manage")?;
            let tax = settings::get(&conn, "default_tax_id")?.as_i64().unwrap_or(1);
            let done = crate::seed::seed_demo_menu(&conn, me.id, tax)?;
            if !done {
                return Err(ApiError::new("conflict", "the menu is not empty"));
            }
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- POS
        "pos.context" => {
            require(&me, "pos.sell")?;
            let register = if params.is_null() { main() } else { p::<RegP>(params)?.register };
            let tables = conn
                .prepare("SELECT id, name, seats FROM dining_tables WHERE active=1 ORDER BY sort, id")?
                .query_map([], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "seats": r.get::<_, i64>(2)?})))?
                .collect::<Result<Vec<_>, _>>()?;
            let taxes = conn
                .prepare("SELECT id, name, rate_bp, price_include, active FROM taxes")?
                .query_map([], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "rate_bp": r.get::<_, i64>(2)?,
                                              "price_include": r.get::<_, bool>(3)?, "active": r.get::<_, bool>(4)?})))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({
                "menu": menu::get_menu(&conn, false)?,
                "settings": settings::get_all(&conn)?,
                "tables": tables,
                "taxes": taxes,
                "happy_hour": orders::happy_hour_now(&conn)?,
                "cash_session": cash::current(&conn, &register)?,
                "receipt_printer": printing::receipt_printer(&conn, &register)?.map(|p| json!({
                    "id": p.id, "name": p.name, "paper_mm": p.paper_mm, "graphic_mode": p.graphic_mode, "drawer": p.drawer, "kind": p.kind,
                })),
            }))
        }
        "orders.submit" => {
            require(&me, "pos.sell")?;
            let id = orders::submit(&conn, &me, p(params)?)?;
            orders::get(&conn, id)
        }
        "orders.get" => {
            require(&me, "pos.sell")?;
            orders::get(&conn, p::<IdP>(params)?.id)
        }
        "orders.list" => {
            if !["pos.sell", "kds.use", "reports.view"].iter().any(|x| me.permissions.iter().any(|p| p == x)) {
                return Err(ApiError::forbidden("pos.sell"));
            }
            orders::list(&conn, if params.is_null() { Default::default() } else { p(params)? })
        }
        "orders.update_line" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                line_id: i64,
                qty: Option<i64>,
                note: Option<String>,
            }
            let P { line_id, qty, note } = p(params)?;
            let id = orders::update_line(&conn, line_id, qty, note)?;
            orders::get(&conn, id)
        }
        "orders.remove_line" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                line_id: i64,
                reason: Option<String>,
                approver_pin: Option<String>,
            }
            let P { line_id, reason, approver_pin } = p(params)?;
            let id = orders::remove_line(&conn, &me, line_id, reason, approver_pin.as_deref())?;
            orders::get(&conn, id)
        }
        "orders.discount" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                order_id: Option<i64>,
                line_id: Option<i64>,
                discount: Option<orders::DiscountIn>,
                approver_pin: Option<String>,
            }
            let P { order_id, line_id, discount, approver_pin } = p(params)?;
            let id = match (line_id, order_id) {
                (Some(l), _) => orders::set_line_discount(&conn, &me, l, discount, approver_pin.as_deref())?,
                (None, Some(o)) => orders::set_order_discount(&conn, &me, o, discount, approver_pin.as_deref())?,
                _ => return Err(ApiError::invalid("order_id or line_id required")),
            };
            orders::get(&conn, id)
        }
        "orders.pay" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                order_id: i64,
                payments: Vec<orders::PayIn>,
                #[serde(default = "main")]
                register: String,
            }
            let P { order_id, payments, register } = p(params)?;
            let r = orders::pay(&conn, &me, order_id, payments, &register)?;
            Ok(json!({ "result": r, "order": orders::get(&conn, order_id)? }))
        }
        "orders.set_status" => {
            if !["pos.sell", "kds.use"].iter().any(|x| me.permissions.iter().any(|p| p == x)) {
                return Err(ApiError::forbidden("pos.sell"));
            }
            #[derive(Deserialize)]
            struct P {
                order_id: i64,
                status: String,
            }
            let P { order_id, status } = p(params)?;
            orders::set_status(&conn, order_id, &status)?;
            orders::get(&conn, order_id)
        }
        "orders.void" | "orders.refund" | "orders.reopen" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                order_id: i64,
                #[serde(default)]
                reason: String,
                approver_pin: Option<String>,
                #[serde(default = "main")]
                register: String,
            }
            let P { order_id, reason, approver_pin, register } = p(params)?;
            match method {
                "orders.void" => orders::void(&conn, &me, order_id, &reason, approver_pin.as_deref())?,
                "orders.refund" => {
                    orders::refund(&conn, &me, order_id, &reason, approver_pin.as_deref(), &register)?;
                }
                _ => orders::reopen(&conn, &me, order_id, approver_pin.as_deref())?,
            }
            orders::get(&conn, order_id)
        }
        "orders.split" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                order_id: i64,
                moves: Vec<orders::SplitMove>,
            }
            let P { order_id, moves } = p(params)?;
            let new_id = orders::split(&conn, &me, order_id, moves)?;
            Ok(json!({ "original": orders::get(&conn, order_id)?, "new": orders::get(&conn, new_id)? }))
        }
        "orders.receipt_printed" => {
            require(&me, "pos.sell")?;
            #[derive(Deserialize)]
            struct P {
                order_id: i64,
                #[serde(default)]
                reprint: bool,
            }
            let P { order_id, reprint } = p(params)?;
            if reprint {
                audit::log(&conn, Some(me.id), "receipt.reprint", Some("order"), Some(&order_id.to_string()), None)?;
            }
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- cash
        "cash.current" => {
            require(&me, "pos.sell")?;
            let register = if params.is_null() { main() } else { p::<RegP>(params)?.register };
            cash::current(&conn, &register)
        }
        "cash.open" => {
            require(&me, "cash.session")?;
            #[derive(Deserialize)]
            struct P {
                #[serde(default = "main")]
                register: String,
                float_cents: i64,
            }
            let P { register, float_cents } = p(params)?;
            cash::open(&conn, &me, &register, float_cents)?;
            cash::current(&conn, &register)
        }
        "cash.move" => {
            require(&me, "cash.in_out")?;
            #[derive(Deserialize)]
            struct P {
                #[serde(default = "main")]
                register: String,
                kind: String,
                amount_cents: i64,
                reason: String,
            }
            let P { register, kind, amount_cents, reason } = p(params)?;
            cash::movement(&conn, &me, &register, &kind, amount_cents, &reason)?;
            cash::current(&conn, &register)
        }
        "cash.close" => {
            require(&me, "cash.session")?;
            #[derive(Deserialize)]
            struct P {
                #[serde(default = "main")]
                register: String,
                counted_cents: i64,
                #[serde(default)]
                note: String,
            }
            let P { register, counted_cents, note } = p(params)?;
            cash::close(&conn, &me, &register, counted_cents, &note)
        }
        "cash.sessions" => {
            require(&me, "cash.session")?;
            cash::sessions(&conn, 100)
        }
        "cash.report" => {
            if !["cash.session", "reports.view"].iter().any(|x| me.permissions.iter().any(|p| p == x)) {
                return Err(ApiError::forbidden("cash.session"));
            }
            cash::report(&conn, p::<IdP>(params)?.id)
        }
        "drawer.open" => {
            require(&me, "drawer.open")?;
            let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let register = params.get("register").and_then(|v| v.as_str()).unwrap_or("main").to_string();
            let p = printing::receipt_printer(&conn, &register)?.filter(|p| p.drawer).ok_or_else(|| ApiError::new("no_printer", "no printer with a cash drawer on this register"))?;
            printing::enqueue(&conn, p.id, "drawer", None, None, "drawer", &[], Some(me.id))?;
            audit::log(&conn, Some(me.id), "drawer.open", None, None, Some(json!({ "reason": reason, "register": register })))?;
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- printing
        "printers.list" => {
            require(&me, "settings.manage")?;
            printing::list(&conn)
        }
        "printers.save" => {
            require(&me, "settings.manage")?;
            Ok(json!({ "id": printing::save_printer(&conn, &me, p(params)?)? }))
        }
        "printers.test" => {
            require(&me, "settings.manage")?;
            let printer = printing::load_printer(&conn, p::<IdP>(params)?.id)?;
            let shop = settings::get(&conn, "shop_name")?.as_str().unwrap_or("BURGER OS").to_string();
            let job = printing::enqueue(&conn, printer.id, "test", None, None, "test", &printing::test_doc(&printer, &shop), Some(me.id))?;
            Ok(json!({ "job_id": job }))
        }
        "printers.discover" => {
            require(&me, "settings.manage")?;
            let usb = printing::system_printers();
            let bt = printing::paired_bluetooth();
            Ok(json!({
                "usb": usb.as_ref().ok(), "usb_error": usb.as_ref().err(),
                "bluetooth": bt.as_ref().ok(), "bluetooth_error": bt.as_ref().err(),
            }))
        }
        "stations.routing" => {
            require(&me, "settings.manage")?;
            #[derive(Deserialize)]
            struct P {
                station_id: i64,
                printer_id: Option<i64>,
                show_on_screen: bool,
            }
            let P { station_id, printer_id, show_on_screen } = p(params)?;
            printing::set_station_routing(&conn, &me, station_id, printer_id, show_on_screen)?;
            printing::list(&conn)
        }
        "print.problems" => printing::problems(&conn),
        "print.jobs" => {
            require(&me, "pos.sell")?;
            let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("open").to_string();
            printing::jobs(&conn, &scope)
        }
        "print.job" => printing::job_status(&conn, p::<IdP>(params)?.id),
        "print.retry" => {
            #[derive(Deserialize)]
            struct P {
                job_id: Option<i64>,
                printer_id: Option<i64>,
            }
            let P { job_id, printer_id } = p(params)?;
            Ok(json!({ "jobs": printing::retry(&conn, job_id, printer_id)? }))
        }
        "print.cancel" | "print.reroute" => {
            #[derive(Deserialize)]
            struct P {
                job_id: Option<i64>,
                from: Option<i64>,
                to: Option<i64>,
                approver_pin: Option<String>,
            }
            let P { job_id, from, to, approver_pin } = p(params)?;
            let id = job_id.or(from).unwrap_or(0);
            let by = orders::approve(&conn, &me, "pos.void", approver_pin.as_deref(), method, id)?;
            if method == "print.cancel" {
                printing::cancel(&conn, &me, job_id.ok_or_else(|| ApiError::invalid("job_id required"))?, by)?;
                Ok(Value::Null)
            } else {
                let n = printing::reroute(&conn, &me, from.ok_or_else(|| ApiError::invalid("from required"))?, to.ok_or_else(|| ApiError::invalid("to required"))?)?;
                Ok(json!({ "jobs": n }))
            }
        }
        "print.receipt" | "print.doc" => {
            if !["pos.sell", "cash.session", "reports.view"].iter().any(|x| me.permissions.iter().any(|p| p == x)) {
                return Err(ApiError::forbidden("pos.sell"));
            }
            #[derive(Deserialize)]
            struct P {
                #[serde(default = "main")]
                register: String,
                doc: Vec<crate::escpos::Line>,
                order_id: Option<i64>,
                #[serde(default)]
                reprint: bool,
                #[serde(default)]
                kick: bool,
                #[serde(default)]
                title: String,
            }
            let a: P = p(params)?;
            if a.doc.len() > 2000 {
                return Err(ApiError::invalid("document too long"));
            }
            let printer = printing::receipt_printer(&conn, &a.register)?.ok_or_else(|| ApiError::new("no_printer", "no receipt printer on this register"))?;
            if a.kick && printer.drawer && me.permissions.iter().any(|x| x == "drawer.open") {
                printing::enqueue(&conn, printer.id, "drawer", a.order_id, None, "drawer", &[], Some(me.id))?;
                audit::log(&conn, Some(me.id), "drawer.open", Some("order"), a.order_id.map(|x| x.to_string()).as_deref(), Some(json!({"reason": "cash payment"})))?;
            }
            let kind = if method == "print.receipt" { "receipt" } else { "report" };
            let job = printing::enqueue(&conn, printer.id, kind, a.order_id, None, &a.title, &a.doc, Some(me.id))?;
            if a.reprint {
                audit::log(&conn, Some(me.id), "receipt.reprint", Some("order"), a.order_id.map(|x| x.to_string()).as_deref(), None)?;
            }
            Ok(json!({ "job_id": job, "printer": printer.name, "graphic_mode": printer.graphic_mode, "paper_mm": printer.paper_mm }))
        }
        "print.kitchen_reprint" => {
            require(&me, "pos.sell")?;
            let n = printing::reprint_kitchen(&conn, &me, p::<IdP>(params)?.id)?;
            Ok(json!({ "tickets": n }))
        }

        // ---------------------------------------------------------- tables
        "tables.list" => {
            let rows = conn
                .prepare("SELECT id, name, seats, sort, active FROM dining_tables ORDER BY sort, id")?
                .query_map([], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "seats": r.get::<_, i64>(2)?,
                                              "sort": r.get::<_, i64>(3)?, "active": r.get::<_, bool>(4)?})))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!(rows))
        }
        "tables.save" => {
            require(&me, "settings.manage")?;
            #[derive(Deserialize)]
            struct P {
                id: Option<i64>,
                name: String,
                seats: i64,
                #[serde(default)]
                sort: i64,
                active: bool,
            }
            let t: P = p(params)?;
            if t.name.trim().is_empty() || t.name.chars().count() > 30 || !(1..=50).contains(&t.seats) {
                return Err(ApiError::invalid("table name and 1-50 seats required"));
            }
            match t.id {
                Some(id) => {
                    conn.execute("UPDATE dining_tables SET name=?2, seats=?3, sort=?4, active=?5 WHERE id=?1", params![id, t.name.trim(), t.seats, t.sort, t.active])?;
                }
                None => {
                    conn.execute("INSERT INTO dining_tables (name, seats, sort, active) VALUES (?1,?2,?3,?4)", params![t.name.trim(), t.seats, t.sort, t.active])?;
                }
            }
            audit::log(&conn, Some(me.id), "table.saved", Some("table"), None, Some(json!({"name": t.name.trim(), "active": t.active})))?;
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- kitchen display
        "kds.stations" => {
            require(&me, "kds.use")?;
            crate::kds::screen_stations(&conn)
        }
        "kds.board" => {
            require(&me, "kds.use")?;
            crate::kds::board(&conn, params.get("station_id").and_then(|v| v.as_i64()))
        }
        "kds.toggle" => {
            require(&me, "kds.use")?;
            crate::kds::toggle(&conn, p::<IdP>(params)?.id)?;
            Ok(Value::Null)
        }
        "kds.bump" => {
            require(&me, "kds.use")?;
            #[derive(Deserialize)]
            struct P {
                order_id: i64,
                station_id: i64,
            }
            let P { order_id, station_id } = p(params)?;
            crate::kds::bump(&conn, order_id, station_id)?;
            Ok(Value::Null)
        }
        "kds.recall" => {
            require(&me, "kds.use")?;
            #[derive(Deserialize)]
            struct P {
                station_id: i64,
            }
            Ok(json!({ "order_id": crate::kds::recall(&conn, p::<P>(params)?.station_id)? }))
        }

        // ---------------------------------------------------------- devices (Host)
        "devices.pairing_start" => {
            require(&me, "settings.manage")?;
            audit::log(&conn, Some(me.id), "device.pairing_started", None, None, None)?;
            Ok(crate::net::start_pairing())
        }
        "devices.list" => {
            require(&me, "settings.manage")?;
            crate::net::list_devices(&conn)
        }
        "devices.revoke" => {
            require(&me, "settings.manage")?;
            crate::net::revoke_device(&conn, me.id, p::<IdP>(params)?.id)?;
            Ok(Value::Null)
        }

        "audit.list" => {
            require(&me, "audit.view")?;
            #[derive(Deserialize)]
            struct P {
                limit: Option<i64>,
                offset: Option<i64>,
            }
            let P { limit, offset } = p(params)?;
            Ok(json!(audit::list(&conn, limit.unwrap_or(100), offset.unwrap_or(0))?))
        }

        _ => Err(ApiError::new("unknown_method", method.to_string())),
    }
}

fn main() -> String {
    "main".into()
}

#[derive(Deserialize)]
struct IdP {
    id: i64,
}

#[derive(Deserialize)]
struct RegP {
    #[serde(default = "main")]
    register: String,
}

#[derive(Deserialize)]
struct SetupParams {
    shop_name: String,
    language: String,
    owner_name: String,
    owner_pin: String,
    #[serde(default)]
    demo_users: bool,
    #[serde(default)]
    demo_menu: bool,
}

pub const DEMO_USERS: &[(&str, &str, &str)] = &[
    ("Karim", "manager", "2222"),
    ("Salma", "cashier", "3333"),
    ("Youssef", "kitchen", "4444"),
];

fn setup_init(conn: &Connection, s: SetupParams) -> ApiResult<Value> {
    if user_count(conn)? > 0 {
        return Err(ApiError::new("conflict", "setup already done"));
    }
    if s.shop_name.trim().is_empty() || s.owner_name.trim().is_empty() {
        return Err(ApiError::invalid("shop name and owner name required"));
    }
    auth::validate_pin(&s.owner_pin)?;
    let tx = crate::db::atomic(conn)?;
    let mut m = Map::new();
    m.insert("shop_name".into(), json!(s.shop_name.trim()));
    m.insert("default_language".into(), json!(s.language));
    settings::set_many(&tx, &m)?;
    tx.execute(
        "INSERT INTO users (name, role, pin_hash, language) VALUES (?1, 'owner', ?2, ?3)",
        params![s.owner_name.trim(), auth::hash_pin(&s.owner_pin)?, s.language],
    )?;
    let owner_id = tx.last_insert_rowid();
    if s.demo_users {
        for (name, role, pin) in DEMO_USERS {
            tx.execute(
                "INSERT INTO users (name, role, pin_hash, language) VALUES (?1, ?2, ?3, ?4)",
                params![name, role, auth::hash_pin(pin)?, s.language],
            )?;
        }
    }
    if s.demo_menu {
        crate::seed::seed_demo_menu(&tx, owner_id, 1)?;
    }
    audit::log(&tx, Some(owner_id), "setup.init", None, None, Some(json!({ "demo_users": s.demo_users, "demo_menu": s.demo_menu })))?;
    tx.commit()?;
    Ok(json!({ "owner_id": owner_id }))
}

#[derive(Deserialize)]
struct SaveUser {
    id: Option<i64>,
    name: String,
    role: String,
    #[serde(default)]
    pin: Option<String>,
    language: String,
    theme: String,
    active: bool,
}

fn active_owner_count(conn: &Connection) -> ApiResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM users WHERE role='owner' AND active=1", [], |r| r.get(0))?)
}

fn save_user(conn: &Connection, me: &SessionUser, u: SaveUser) -> ApiResult<Value> {
    if u.name.trim().is_empty() || !ROLES.contains(&u.role.as_str()) {
        return Err(ApiError::invalid("name and valid role required"));
    }
    if u.role == "owner" && me.role != "owner" {
        return Err(ApiError::forbidden("owner"));
    }
    let pin_hash = match u.pin.as_deref().filter(|s| !s.is_empty()) {
        Some(pin) => Some(auth::hash_pin(pin)?),
        None => None,
    };
    let tx = crate::db::atomic(conn)?;
    let (id, action) = match u.id {
        None => {
            let hash = pin_hash.ok_or_else(|| ApiError::invalid("PIN required for a new user"))?;
            tx.execute(
                "INSERT INTO users (name, role, pin_hash, language, theme, active) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![u.name.trim(), u.role, hash, u.language, u.theme, u.active],
            )
            .map_err(|_| ApiError::invalid("bad language or theme"))?;
            (tx.last_insert_rowid(), "user.created")
        }
        Some(id) => {
            let old_role: String = tx
                .query_row("SELECT role FROM users WHERE id=?1", [id], |r| r.get(0))
                .optional()?
                .ok_or_else(|| ApiError::not_found("user"))?;
            if old_role == "owner" && me.role != "owner" {
                return Err(ApiError::forbidden("owner"));
            }
            if id == me.id && (u.role != me.role || !u.active) {
                return Err(ApiError::invalid("you cannot change your own role or deactivate yourself"));
            }
            tx.execute(
                "UPDATE users SET name=?2, role=?3, language=?4, theme=?5, active=?6,
                   updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![id, u.name.trim(), u.role, u.language, u.theme, u.active],
            )
            .map_err(|_| ApiError::invalid("bad language or theme"))?;
            if let Some(hash) = pin_hash {
                tx.execute("UPDATE users SET pin_hash=?2, failed_attempts=0, locked_until=NULL WHERE id=?1", params![id, hash])?;
                audit::log(&tx, Some(me.id), "user.pin_reset", Some("user"), Some(&id.to_string()), None)?;
            }
            if active_owner_count(&tx)? == 0 {
                return Err(ApiError::invalid("there must be at least one active owner"));
            }
            (id, "user.updated")
        }
    };
    audit::log(
        &tx,
        Some(me.id),
        action,
        Some("user"),
        Some(&id.to_string()),
        Some(json!({ "name": u.name.trim(), "role": u.role, "active": u.active })),
    )?;
    tx.commit()?;
    Ok(json!({ "id": id, "deactivated": if u.active { Value::Null } else { json!(id) } }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn core_with_setup() -> Core {
        let core = Core::new(open_in_memory().unwrap());
        dispatch(
            &core,
            None,
            "setup.init",
            json!({"shop_name": "Test Burger", "language": "fr", "owner_name": "Mohamed", "owner_pin": "1234", "demo_users": true}),
        )
        .unwrap();
        core
    }

    fn login(core: &Core, name: &str, pin: &str) -> String {
        let users = dispatch(core, None, "auth.users", Value::Null).unwrap();
        let id = users.as_array().unwrap().iter().find(|u| u["name"] == name).unwrap()["id"].as_i64().unwrap();
        dispatch(core, None, "auth.login", json!({"user_id": id, "pin": pin})).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn setup_once_then_status() {
        let core = Core::new(open_in_memory().unwrap());
        assert_eq!(dispatch(&core, None, "app.status", Value::Null).unwrap()["setup_done"], json!(false));
        let core = core_with_setup();
        let st = dispatch(&core, None, "app.status", Value::Null).unwrap();
        assert_eq!(st["setup_done"], json!(true));
        assert_eq!(st["shop_name"], json!("Test Burger"));
        let again = dispatch(&core, None, "setup.init", json!({"shop_name": "X", "language": "fr", "owner_name": "Y", "owner_pin": "9999"}));
        assert_eq!(again.unwrap_err().code, "conflict");
        assert_eq!(dispatch(&core, None, "auth.users", Value::Null).unwrap().as_array().unwrap().len(), 4);
    }

    #[test]
    fn protected_methods_need_token() {
        let core = core_with_setup();
        assert_eq!(dispatch(&core, None, "settings.get", Value::Null).unwrap_err().code, "unauthorized");
        assert_eq!(dispatch(&core, Some("nope"), "settings.get", Value::Null).unwrap_err().code, "unauthorized");
        let t = login(&core, "Mohamed", "1234");
        assert!(dispatch(&core, Some(&t), "settings.get", Value::Null).is_ok());
        dispatch(&core, Some(&t), "auth.logout", Value::Null).unwrap();
        assert_eq!(dispatch(&core, Some(&t), "settings.get", Value::Null).unwrap_err().code, "unauthorized");
    }

    #[test]
    fn cashier_cannot_change_settings_or_users() {
        let core = core_with_setup();
        let t = login(&core, "Salma", "3333");
        assert_eq!(dispatch(&core, Some(&t), "settings.set", json!({"shop_name": "Hack"})).unwrap_err().code, "forbidden");
        assert_eq!(dispatch(&core, Some(&t), "users.list", Value::Null).unwrap_err().code, "forbidden");
        assert_eq!(dispatch(&core, Some(&t), "audit.list", json!({})).unwrap_err().code, "forbidden");
    }

    #[test]
    fn manager_override_for_cashier_discount_is_audited() {
        let core = core_with_setup();
        let t = login(&core, "Salma", "3333");
        assert_eq!(
            dispatch(&core, Some(&t), "auth.authorize", json!({"pin": "3333", "permission": "pos.discount"})).unwrap_err().code,
            "wrong_pin"
        );
        let r = dispatch(&core, Some(&t), "auth.authorize", json!({"pin": "2222", "permission": "pos.discount", "reason": "regular"})).unwrap();
        assert_eq!(r["approver_name"], json!("Karim"));
        let owner = login(&core, "Mohamed", "1234");
        let log = dispatch(&core, Some(&owner), "audit.list", json!({})).unwrap();
        assert!(log.as_array().unwrap().iter().any(|e| e["action"] == "auth.override"));
    }

    #[test]
    fn manager_cannot_create_or_edit_owner() {
        let core = core_with_setup();
        let owner = login(&core, "Mohamed", "1234");
        // Give manager users.manage first.
        let perms = dispatch(&core, Some(&owner), "permissions.get", Value::Null).unwrap();
        let mut mp: Vec<String> = serde_json::from_value(perms["roles"]["manager"].clone()).unwrap();
        mp.push("users.manage".into());
        dispatch(&core, Some(&owner), "permissions.set", json!({"role": "manager", "permissions": mp})).unwrap();

        let m = login(&core, "Karim", "2222");
        let r = dispatch(&core, Some(&m), "users.save", json!({"name": "Evil", "role": "owner", "pin": "5555", "language": "fr", "theme": "dark", "active": true}));
        assert_eq!(r.unwrap_err().code, "forbidden");
        let r = dispatch(&core, Some(&m), "users.save", json!({"name": "New", "role": "cashier", "pin": "5555", "language": "ar", "theme": "light", "active": true}));
        assert!(r.is_ok());
        assert_eq!(dispatch(&core, Some(&m), "permissions.set", json!({"role": "cashier", "permissions": []})).unwrap_err().code, "forbidden");
    }

    #[test]
    fn cannot_remove_last_owner_or_self() {
        let core = core_with_setup();
        let owner = login(&core, "Mohamed", "1234");
        let me = dispatch(&core, Some(&owner), "auth.me", Value::Null).unwrap();
        let r = dispatch(&core, Some(&owner), "users.save", json!({"id": me["id"], "name": "Mohamed", "role": "manager", "language": "fr", "theme": "dark", "active": true}));
        assert_eq!(r.unwrap_err().code, "invalid");
    }

    #[test]
    fn deactivated_user_session_is_dropped() {
        let core = core_with_setup();
        let owner = login(&core, "Mohamed", "1234");
        let cashier = login(&core, "Salma", "3333");
        let users = dispatch(&core, Some(&owner), "users.list", Value::Null).unwrap();
        let salma = users.as_array().unwrap().iter().find(|u| u["name"] == "Salma").unwrap().clone();
        dispatch(&core, Some(&owner), "users.save", json!({"id": salma["id"], "name": "Salma", "role": "cashier", "language": "fr", "theme": "dark", "active": false})).unwrap();
        assert_eq!(dispatch(&core, Some(&cashier), "settings.get", Value::Null).unwrap_err().code, "unauthorized");
    }

    #[test]
    fn permission_change_applies_to_live_session() {
        let core = core_with_setup();
        let owner = login(&core, "Mohamed", "1234");
        let cashier = login(&core, "Salma", "3333");
        assert_eq!(dispatch(&core, Some(&cashier), "audit.list", json!({})).unwrap_err().code, "forbidden");
        dispatch(&core, Some(&owner), "permissions.set", json!({"role": "cashier", "permissions": ["pos.sell", "audit.view"]})).unwrap();
        assert!(dispatch(&core, Some(&cashier), "audit.list", json!({})).is_ok());
        assert_eq!(dispatch(&core, Some(&owner), "permissions.set", json!({"role": "cashier", "permissions": ["root"]})).unwrap_err().code, "invalid");
    }

    #[test]
    fn taxes_crud_and_validation() {
        let core = core_with_setup();
        let owner = login(&core, "Mohamed", "1234");
        let r = dispatch(&core, Some(&owner), "taxes.save", json!({"name": "TVA 7%", "rate_bp": 700, "price_include": false, "active": true})).unwrap();
        let list = dispatch(&core, Some(&owner), "taxes.list", Value::Null).unwrap();
        let t = list.as_array().unwrap().iter().find(|t| t["id"] == r["id"]).unwrap();
        assert_eq!(t["price_include"], json!(false));
        assert!(dispatch(&core, Some(&owner), "taxes.save", json!({"name": "Bad", "rate_bp": 20000, "price_include": true, "active": true})).is_err());
    }

    #[test]
    fn menu_permissions_and_demo_seed() {
        let core = Core::new(open_in_memory().unwrap());
        dispatch(&core, None, "setup.init", json!({"shop_name": "T", "language": "fr", "owner_name": "O", "owner_pin": "1234", "demo_users": true, "demo_menu": true})).unwrap();
        let cashier = login(&core, "Salma", "3333");
        let kitchen = login(&core, "Youssef", "4444");
        let owner = login(&core, "O", "1234");
        let m = dispatch(&core, Some(&cashier), "menu.get", Value::Null).unwrap();
        let products = m["products"].as_array().unwrap();
        assert!(products.len() >= 30);
        let id = products[0]["id"].clone();
        assert_eq!(dispatch(&core, Some(&cashier), "menu.get", json!({"all": true})).unwrap_err().code, "forbidden");
        assert_eq!(dispatch(&core, Some(&cashier), "categories.save", json!({"names": {"fr": "X"}, "color": "#000000", "icon": "burger", "active": true})).unwrap_err().code, "forbidden");
        dispatch(&core, Some(&kitchen), "products.set_available", json!({"id": id, "available": false})).unwrap();
        let m = dispatch(&core, Some(&owner), "menu.get", json!({"all": true})).unwrap();
        assert_eq!(m["products"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap()["available"], json!(false));
        assert_eq!(dispatch(&core, Some(&owner), "menu.seed_demo", Value::Null).unwrap_err().code, "conflict");
    }

    #[test]
    fn change_own_pin() {
        let core = core_with_setup();
        let t = login(&core, "Salma", "3333");
        assert_eq!(dispatch(&core, Some(&t), "auth.change_pin", json!({"current_pin": "0000", "new_pin": "7777"})).unwrap_err().code, "wrong_pin");
        dispatch(&core, Some(&t), "auth.change_pin", json!({"current_pin": "3333", "new_pin": "7777"})).unwrap();
        login(&core, "Salma", "7777");
    }
}
