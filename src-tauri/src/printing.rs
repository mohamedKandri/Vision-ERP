//! Printers, kitchen-ticket routing, and a durable print queue with a background worker.
//!
//! Every job is a row in `print_jobs` written in the same transaction as the change that
//! caused it (e.g. sending lines to the kitchen). The worker sends jobs per printer in order,
//! retries failures with backoff, and never drops a kitchen ticket: a job only leaves the
//! queue when it printed or a manager cancels it.

use crate::audit;
use crate::auth::SessionUser;
use crate::error::{ApiError, ApiResult};
use crate::escpos::{self, Align, Cfg, Codepage, Drawer, Line};
use crate::item::{self, ItemConfig, MenuIndex};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const KINDS: &[&str] = &["network", "bluetooth", "usb", "sunmi"];
/// SUNMI exposes its built-in printer as a paired Bluetooth device with this name
/// (docs.sunmi.com, "Virtual Bluetooth InnerPrinter").
pub const SUNMI_BT_NAME: &str = "InnerPrinter";

// ------------------------------------------------------------------ printers

#[derive(Debug, Clone)]
pub struct Printer {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub address: String,
    pub paper_mm: u32,
    pub codepage: String,
    pub graphic_mode: bool,
    pub cut: bool,
    pub drawer: bool,
    pub receipt_register: Option<String>,
    pub ticket_lang: String,
    pub active: bool,
}

impl Printer {
    pub fn cfg(&self, with_drawer: bool) -> Cfg {
        Cfg {
            paper_mm: self.paper_mm,
            codepage: Codepage::parse(&self.codepage),
            cut: self.cut,
            drawer: if with_drawer && self.drawer { Some(if self.kind == "sunmi" { Drawer::Sunmi } else { Drawer::Generic }) } else { None },
        }
    }
}

const PRINTER_COLS: &str = "id, name, kind, address, paper_mm, codepage, graphic_mode, cut, drawer, receipt_register, ticket_lang, active";

fn printer_row(r: &rusqlite::Row) -> rusqlite::Result<Printer> {
    Ok(Printer {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        address: r.get(3)?,
        paper_mm: r.get(4)?,
        codepage: r.get(5)?,
        graphic_mode: r.get(6)?,
        cut: r.get(7)?,
        drawer: r.get(8)?,
        receipt_register: r.get(9)?,
        ticket_lang: r.get(10)?,
        active: r.get(11)?,
    })
}

pub fn load_printer(conn: &Connection, id: i64) -> ApiResult<Printer> {
    conn.query_row(&format!("SELECT {PRINTER_COLS} FROM printers WHERE id=?1"), [id], printer_row)
        .optional()?
        .ok_or_else(|| ApiError::not_found("printer"))
}

pub fn receipt_printer(conn: &Connection, register: &str) -> ApiResult<Option<Printer>> {
    Ok(conn
        .query_row(&format!("SELECT {PRINTER_COLS} FROM printers WHERE receipt_register=?1 AND active=1"), [register], printer_row)
        .optional()?)
}

#[derive(Deserialize)]
pub struct PrinterIn {
    pub id: Option<i64>,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub address: String,
    pub paper_mm: u32,
    pub codepage: String,
    #[serde(default)]
    pub graphic_mode: bool,
    #[serde(default = "yes")]
    pub cut: bool,
    #[serde(default)]
    pub drawer: bool,
    pub receipt_register: Option<String>,
    #[serde(default = "fr")]
    pub ticket_lang: String,
    #[serde(default = "yes")]
    pub active: bool,
}
fn yes() -> bool {
    true
}
fn fr() -> String {
    "fr".into()
}

/// "host" or "host:port" (port defaults to 9100).
pub fn parse_network_address(a: &str) -> ApiResult<(String, u16)> {
    let a = a.trim();
    if a.is_empty() {
        return Err(ApiError::invalid("printer address required"));
    }
    let (host, port) = match a.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| ApiError::invalid("bad port"))?),
        None => (a, 9100),
    };
    let ok = !host.is_empty() && host.len() <= 253 && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !ok || port == 0 {
        return Err(ApiError::invalid("bad printer address"));
    }
    Ok((host.to_string(), port))
}

pub fn save_printer(conn: &Connection, me: &SessionUser, p: PrinterIn) -> ApiResult<i64> {
    if p.name.trim().is_empty() || p.name.chars().count() > 60 {
        return Err(ApiError::invalid("printer name required"));
    }
    if !KINDS.contains(&p.kind.as_str()) {
        return Err(ApiError::invalid("unknown printer type"));
    }
    if ![58, 80].contains(&p.paper_mm) || !["cp437", "cp850", "cp858", "cp1252", "ascii"].contains(&p.codepage.as_str()) || !["fr", "en"].contains(&p.ticket_lang.as_str()) {
        return Err(ApiError::invalid("bad paper, code page or ticket language"));
    }
    let address = p.address.trim().to_string();
    match p.kind.as_str() {
        "network" => {
            parse_network_address(&address)?;
        }
        "bluetooth" | "usb" if address.is_empty() => return Err(ApiError::invalid("choose the printer")),
        _ => {}
    }
    if address.chars().count() > 200 {
        return Err(ApiError::invalid("address too long"));
    }
    let register = p.receipt_register.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let tx = crate::db::atomic(conn)?;
    if let (Some(r), true) = (&register, p.active) {
        // One receipt printer per register: the new one takes over.
        tx.execute("UPDATE printers SET receipt_register=NULL WHERE receipt_register=?1 AND id IS NOT ?2", params![r, p.id])?;
    }
    let id = match p.id {
        Some(id) => {
            if tx.execute(
                "UPDATE printers SET name=?2, kind=?3, address=?4, paper_mm=?5, codepage=?6, graphic_mode=?7, cut=?8, drawer=?9,
                   receipt_register=?10, ticket_lang=?11, active=?12 WHERE id=?1",
                params![id, p.name.trim(), p.kind, address, p.paper_mm, p.codepage, p.graphic_mode, p.cut, p.drawer, register, p.ticket_lang, p.active],
            )? == 0
            {
                return Err(ApiError::not_found("printer"));
            }
            id
        }
        None => {
            tx.execute(
                "INSERT INTO printers (name, kind, address, paper_mm, codepage, graphic_mode, cut, drawer, receipt_register, ticket_lang, active)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![p.name.trim(), p.kind, address, p.paper_mm, p.codepage, p.graphic_mode, p.cut, p.drawer, register, p.ticket_lang, p.active],
            )?;
            tx.last_insert_rowid()
        }
    };
    audit::log(&tx, Some(me.id), "printer.saved", Some("printer"), Some(&id.to_string()),
        Some(json!({"name": p.name.trim(), "kind": p.kind, "address": address, "receipt_register": register, "active": p.active})))?;
    tx.commit()?;
    Ok(id)
}

pub fn list(conn: &Connection) -> ApiResult<Value> {
    let rows = conn
        .prepare(&format!(
            "SELECT {PRINTER_COLS}, last_ok_at, last_error, last_error_at,
               (SELECT COUNT(*) FROM print_jobs j WHERE j.printer_id=printers.id AND j.status='pending'),
               (SELECT COUNT(*) FROM print_jobs j WHERE j.printer_id=printers.id AND j.status='failed')
             FROM printers ORDER BY active DESC, name"
        ))?
        .query_map([], |r| {
            let p = printer_row(r)?;
            Ok(json!({
                "id": p.id, "name": p.name, "kind": p.kind, "address": p.address, "paper_mm": p.paper_mm, "codepage": p.codepage,
                "graphic_mode": p.graphic_mode, "cut": p.cut, "drawer": p.drawer, "receipt_register": p.receipt_register,
                "ticket_lang": p.ticket_lang, "active": p.active,
                "last_ok_at": r.get::<_, Option<String>>(12)?, "last_error": r.get::<_, Option<String>>(13)?,
                "last_error_at": r.get::<_, Option<String>>(14)?, "pending": r.get::<_, i64>(15)?, "failed": r.get::<_, i64>(16)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let stations = conn
        .prepare("SELECT id, code, names, printer_id, show_on_screen FROM stations WHERE active=1 ORDER BY sort, id")?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "code": r.get::<_, String>(1)?,
                      "names": serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
                      "printer_id": r.get::<_, Option<i64>>(3)?, "show_on_screen": r.get::<_, bool>(4)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "printers": rows, "stations": stations }))
}

pub fn set_station_routing(conn: &Connection, me: &SessionUser, station_id: i64, printer_id: Option<i64>, show_on_screen: bool) -> ApiResult<()> {
    if let Some(pid) = printer_id {
        load_printer(conn, pid)?;
    }
    if printer_id.is_none() && !show_on_screen {
        return Err(ApiError::invalid("a station needs a screen, a printer, or both"));
    }
    if conn.execute("UPDATE stations SET printer_id=?2, show_on_screen=?3 WHERE id=?1", params![station_id, printer_id, show_on_screen])? == 0 {
        return Err(ApiError::not_found("station"));
    }
    audit::log(conn, Some(me.id), "station.routing", Some("station"), Some(&station_id.to_string()),
        Some(json!({"printer_id": printer_id, "show_on_screen": show_on_screen})))?;
    Ok(())
}

// ------------------------------------------------------------------ queue

pub fn enqueue(conn: &Connection, printer_id: i64, kind: &str, order_id: Option<i64>, station_id: Option<i64>, title: &str, doc: &[Line], user: Option<i64>) -> ApiResult<i64> {
    conn.execute(
        "INSERT INTO print_jobs (printer_id, kind, order_id, station_id, title, doc, created_by) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![printer_id, kind, order_id, station_id, title, serde_json::to_string(doc).unwrap(), user],
    )?;
    let id = conn.last_insert_rowid();
    wake();
    Ok(id)
}

pub fn jobs(conn: &Connection, scope: &str) -> ApiResult<Value> {
    let filter = match scope {
        "open" => "j.status IN ('pending','failed')",
        _ => "1=1",
    };
    let rows = conn
        .prepare(&format!(
            "SELECT j.id, j.printer_id, p.name, j.kind, j.order_id, j.title, j.status, j.attempts, j.last_error, j.created_at, j.printed_at
             FROM print_jobs j JOIN printers p ON p.id=j.printer_id WHERE {filter} ORDER BY j.id DESC LIMIT 200"
        ))?
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "printer_id": r.get::<_, i64>(1)?, "printer": r.get::<_, String>(2)?,
                      "kind": r.get::<_, String>(3)?, "order_id": r.get::<_, Option<i64>>(4)?, "title": r.get::<_, String>(5)?,
                      "status": r.get::<_, String>(6)?, "attempts": r.get::<_, i64>(7)?, "last_error": r.get::<_, Option<String>>(8)?,
                      "created_at": r.get::<_, String>(9)?, "printed_at": r.get::<_, Option<String>>(10)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

pub fn job_status(conn: &Connection, id: i64) -> ApiResult<Value> {
    conn.query_row("SELECT status, attempts, last_error FROM print_jobs WHERE id=?1", [id], |r| {
        Ok(json!({"status": r.get::<_, String>(0)?, "attempts": r.get::<_, i64>(1)?, "last_error": r.get::<_, Option<String>>(2)?}))
    })
    .optional()?
    .ok_or_else(|| ApiError::not_found("job"))
}

/// Retry now: failed (or pending) jobs of a printer, or one job.
pub fn retry(conn: &Connection, job_id: Option<i64>, printer_id: Option<i64>) -> ApiResult<usize> {
    let n = conn.execute(
        "UPDATE print_jobs SET status='pending', next_attempt_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE status IN ('pending','failed') AND (?1 IS NULL OR id=?1) AND (?2 IS NULL OR printer_id=?2)",
        params![job_id, printer_id],
    )?;
    wake();
    Ok(n)
}

pub fn cancel(conn: &Connection, me: &SessionUser, job_id: i64, approver: i64) -> ApiResult<()> {
    let (kind, title): (String, String) = conn
        .query_row("SELECT kind, title FROM print_jobs WHERE id=?1 AND status IN ('pending','failed')", [job_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| ApiError::not_found("open print job"))?;
    conn.execute("UPDATE print_jobs SET status='cancelled' WHERE id=?1", [job_id])?;
    audit::log(conn, Some(me.id), "print.cancelled", Some("print_job"), Some(&job_id.to_string()), Some(json!({"kind": kind, "title": title, "approved_by": approver})))?;
    Ok(())
}

/// Move every open job of a dead printer to another one (e.g. Grill printer jammed → Drinks printer).
pub fn reroute(conn: &Connection, me: &SessionUser, from: i64, to: i64) -> ApiResult<usize> {
    if from == to {
        return Err(ApiError::invalid("choose another printer"));
    }
    let target = load_printer(conn, to)?;
    if !target.active {
        return Err(ApiError::invalid("target printer is inactive"));
    }
    let n = conn.execute(
        "UPDATE print_jobs SET printer_id=?2, status='pending', next_attempt_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE printer_id=?1 AND status IN ('pending','failed')",
        params![from, to],
    )?;
    audit::log(conn, Some(me.id), "print.rerouted", Some("printer"), Some(&from.to_string()), Some(json!({"to": to, "jobs": n})))?;
    wake();
    Ok(n)
}

/// Problems the till must show: printers with failed jobs.
pub fn problems(conn: &Connection) -> ApiResult<Value> {
    let rows = conn
        .prepare(
            "SELECT p.id, p.name, p.last_error, COUNT(j.id), SUM(j.kind='kitchen')
             FROM printers p JOIN print_jobs j ON j.printer_id=p.id AND j.status='failed'
             GROUP BY p.id ORDER BY p.name",
        )?
        .query_map([], |r| {
            Ok(json!({"printer_id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "error": r.get::<_, Option<String>>(2)?,
                      "failed": r.get::<_, i64>(3)?, "kitchen": r.get::<_, i64>(4)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!(rows))
}

// ------------------------------------------------------------------ kitchen tickets

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TicketMode {
    New,
    Add,
    Void,
    Reprint,
}

fn label(lang: &str, key: &str) -> &'static str {
    let fr = lang != "en";
    match key {
        "new" => if fr { "NOUVELLE" } else { "NEW" },
        "add" => if fr { "AJOUT" } else { "ADDED" },
        "void" => if fr { "ANNULATION" } else { "CANCELLED" },
        "reprint" => if fr { "RÉIMPRESSION" } else { "REPRINT" },
        "dine_in" => if fr { "Sur place" } else { "Dine-in" },
        "takeaway" => if fr { "À emporter" } else { "Takeaway" },
        "delivery" => if fr { "Livraison" } else { "Delivery" },
        "table" => if fr { "Table" } else { "Table" },
        "cancelled_item" => if fr { "ANNULÉ" } else { "VOID" },
        "note" => if fr { "Note" } else { "Note" },
        _ => "",
    }
}

struct Part {
    station_id: Option<i64>,
    qty: i64,
    name: String,
    details: Vec<String>,
    note: String,
}

fn product_station_and_names(conn: &Connection, product_id: i64) -> ApiResult<(Option<i64>, Value)> {
    Ok(conn.query_row("SELECT station_id, names FROM products WHERE id=?1", [product_id], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap_or(Value::Null)))
    })?)
}

/// Explode order lines into per-station parts. A combo is split into its parts so the burger
/// goes to the grill, the fries to the fryer and the drink to the drinks station.
fn parts(conn: &Connection, ix: &MenuIndex, line_ids: &[i64], lang: &str) -> ApiResult<Vec<Part>> {
    let mut out = Vec::new();
    for id in line_ids {
        let (qty, names, details, config, note, station): (i64, String, String, String, String, Option<i64>) = conn.query_row(
            "SELECT qty, names, details, config, note, station_id FROM order_lines WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        let names: Value = serde_json::from_str(&names).unwrap_or(Value::Null);
        let cfg: ItemConfig = serde_json::from_str(&config).map_err(|e| ApiError::new("internal", e.to_string()))?;
        match &cfg.combo {
            Some(combo_parts) if !combo_parts.is_empty() => {
                let combo_name = item::pick(&names, lang);
                // Combo-level options (e.g. "Maxi") are shown on every part.
                let combo_opts = item::describe(ix, &ItemConfig { combo: None, ..cfg.clone() }, lang);
                let tag = if combo_opts.is_empty() { format!("[{combo_name}]") } else { format!("[{combo_name} · {}]", combo_opts.join(", ")) };
                for p in combo_parts {
                    let (st, pnames) = product_station_and_names(conn, p.product_id)?;
                    let mut d = item::describe(ix, p, lang);
                    d.push(tag.clone());
                    out.push(Part { station_id: st, qty, name: item::pick(&pnames, lang), details: d, note: note.clone() });
                }
            }
            _ => {
                let d: Vec<String> = serde_json::from_str::<Value>(&details)
                    .ok()
                    .and_then(|v| v.get(lang).or_else(|| v.get("fr")).cloned())
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                out.push(Part { station_id: station, qty, name: item::pick(&names, lang), details: d, note });
            }
        }
    }
    Ok(out)
}

/// Create kitchen tickets for these lines, one per station that has an active printer.
pub fn enqueue_kitchen(conn: &Connection, order_id: i64, line_ids: &[i64], mode: TicketMode, user: Option<i64>) -> ApiResult<Vec<i64>> {
    if line_ids.is_empty() || !crate::settings::get(conn, "print_kitchen_tickets")?.as_bool().unwrap_or(true) {
        return Ok(vec![]);
    }
    let routes: Vec<(i64, Value, i64)> = conn
        .prepare("SELECT s.id, s.names, s.printer_id FROM stations s JOIN printers p ON p.id=s.printer_id WHERE s.active=1 AND p.active=1")?
        .query_map([], |r| Ok((r.get(0)?, serde_json::from_str::<Value>(&r.get::<_, String>(1)?).unwrap_or(Value::Null), r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    if routes.is_empty() {
        return Ok(vec![]);
    }
    let (number, split, kind, table, customer, order_note, time): (i64, i64, String, Option<String>, String, String, String) = conn.query_row(
        "SELECT o.number, o.split_index, o.type, t.name, o.customer_name, o.note, strftime('%H:%M','now','localtime')
         FROM orders o LEFT JOIN dining_tables t ON t.id=o.table_id WHERE o.id=?1",
        [order_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
    )?;
    let label_no = if split > 0 { format!("#{number}-{}", split + 1) } else { format!("#{number}") };
    let ix = MenuIndex::load(conn)?;
    let mut jobs = Vec::new();
    for (station_id, station_names, printer_id) in routes {
        let printer = load_printer(conn, printer_id)?;
        let lang = printer.ticket_lang.as_str();
        let mine: Vec<Part> = parts(conn, &ix, line_ids, lang)?.into_iter().filter(|p| p.station_id == Some(station_id)).collect();
        if mine.is_empty() {
            continue;
        }
        let head = match mode {
            TicketMode::New => label(lang, "new"),
            TicketMode::Add => label(lang, "add"),
            TicketMode::Void => label(lang, "void"),
            TicketMode::Reprint => label(lang, "reprint"),
        };
        let t = |text: String, align: Align, bold: bool, big: bool| Line::Text { text, align: Some(align), bold: Some(bold), big: Some(big) };
        let mut doc = vec![
            t(label_no.clone(), Align::Center, true, true),
            t(format!("{head} · {}", item::pick(&station_names, lang)), Align::Center, true, false),
            t(
                match kind.as_str() {
                    "dine_in" => format!("{}{}", label(lang, "dine_in"), table.as_ref().map(|x| format!(" · {} {x}", label(lang, "table"))).unwrap_or_default()),
                    "takeaway" => format!("{}{}", label(lang, "takeaway"), if customer.is_empty() { String::new() } else { format!(" · {customer}") }),
                    _ => format!("{}{}", label(lang, "delivery"), if customer.is_empty() { String::new() } else { format!(" · {customer}") }),
                },
                Align::Center,
                true,
                false,
            ),
            t(time.clone(), Align::Center, false, false),
            Line::Rule,
        ];
        for p in &mine {
            let prefix = if mode == TicketMode::Void { format!("{} ", label(lang, "cancelled_item")) } else { String::new() };
            doc.push(t(format!("{prefix}{} x {}", p.qty, p.name), Align::Left, true, true));
            for d in &p.details {
                doc.push(t(format!("   {d}"), Align::Left, true, false));
            }
            if !p.note.is_empty() {
                doc.push(t(format!("   !! {}", p.note), Align::Left, true, false));
            }
            doc.push(Line::Feed);
        }
        if !order_note.is_empty() {
            doc.push(Line::Rule);
            doc.push(t(format!("{}: {order_note}", label(lang, "note")), Align::Left, true, false));
        }
        let title = format!("{label_no} {head} · {}", item::pick(&station_names, lang));
        jobs.push(enqueue(conn, printer_id, "kitchen", Some(order_id), Some(station_id), &title, &doc, user)?);
    }
    Ok(jobs)
}

pub fn reprint_kitchen(conn: &Connection, me: &SessionUser, order_id: i64) -> ApiResult<usize> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM order_lines WHERE order_id=?1 AND voided=0 AND kitchen_status<>'new' ORDER BY id")?
        .query_map([order_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Err(ApiError::invalid("nothing was sent to the kitchen"));
    }
    let jobs = enqueue_kitchen(conn, order_id, &ids, TicketMode::Reprint, Some(me.id))?;
    if jobs.is_empty() {
        return Err(ApiError::new("no_printer", "no kitchen printer is set up"));
    }
    audit::log(conn, Some(me.id), "print.kitchen_reprint", Some("order"), Some(&order_id.to_string()), None)?;
    Ok(jobs.len())
}

pub fn test_doc(p: &Printer, shop: &str) -> Vec<Line> {
    let t = |text: &str, align: Align, bold: bool, big: bool| Line::Text { text: text.into(), align: Some(align), bold: Some(bold), big: Some(big) };
    vec![
        t(shop, Align::Center, true, true),
        t("TEST", Align::Center, true, true),
        Line::Rule,
        t(&format!("{} · {} · {} mm", p.name, p.kind, p.paper_mm), Align::Left, false, false),
        t(&format!("{} · {}", p.address, p.codepage), Align::Left, false, false),
        t("Accents: é è ê à ç ù ô î É À Ç", Align::Left, false, false),
        Line::Row { left: "1 x Classic Burger".into(), right: "45,00 DH".into(), bold: Some(true), big: None },
        Line::Row { left: "TOTAL TTC".into(), right: "45,00 DH".into(), bold: Some(true), big: Some(true) },
        Line::Rule,
        Line::Qr { data: "BURGER OS TEST".into() },
        t("OK", Align::Center, true, true),
    ]
}

// ------------------------------------------------------------------ transports

/// Bluetooth is provided by the Android app (Kotlin plugin). Desktop has none.
pub trait Bluetooth: Send + Sync {
    fn write(&self, target: &str, data: &[u8]) -> Result<(), String>;
    fn paired(&self) -> Result<Vec<Value>, String>;
}

static BLUETOOTH: OnceLock<Arc<dyn Bluetooth>> = OnceLock::new();

pub fn set_bluetooth(b: Arc<dyn Bluetooth>) {
    let _ = BLUETOOTH.set(b);
}

pub fn paired_bluetooth() -> Result<Vec<Value>, String> {
    BLUETOOTH.get().ok_or_else(|| "Bluetooth printers are only available on the Android app".to_string())?.paired()
}

pub fn send_network(address: &str, data: &[u8]) -> Result<(), String> {
    let (host, port) = parse_network_address(address).map_err(|e| e.message)?;
    let addr = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("address: {e}"))?.next().ok_or("address not found")?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_write_timeout(Some(Duration::from_secs(10))).ok();
    s.write_all(data).map_err(|e| format!("write: {e}"))?;
    s.flush().map_err(|e| format!("flush: {e}"))?;
    // Give the printer a moment to take the bytes before closing (some drop the tail otherwise).
    std::thread::sleep(Duration::from_millis(80));
    let _ = s.shutdown(std::net::Shutdown::Both);
    Ok(())
}

#[cfg(windows)]
pub mod spooler {
    //! Raw printing to a Windows printer (USB printers installed with a driver, or "Generic / Text Only").
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
    use windows_sys::Win32::Graphics::Printing::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn print_raw(printer: &str, data: &[u8]) -> Result<(), String> {
        unsafe {
            let name = wide(printer);
            let mut h: HANDLE = std::ptr::null_mut();
            if OpenPrinterW(name.as_ptr(), &mut h, std::ptr::null()) == 0 {
                return Err(format!("open printer '{printer}': error {}", GetLastError()));
            }
            let mut doc_name = wide("BURGER OS");
            let mut datatype = wide("RAW");
            let info = DOC_INFO_1W { pDocName: doc_name.as_mut_ptr(), pOutputFile: std::ptr::null_mut(), pDatatype: datatype.as_mut_ptr() };
            let result = (|| {
                if StartDocPrinterW(h, 1, &info as *const DOC_INFO_1W) == 0 {
                    return Err(format!("start document: error {}", GetLastError()));
                }
                StartPagePrinter(h);
                let mut written = 0u32;
                let ok = WritePrinter(h, data.as_ptr() as *const _, data.len() as u32, &mut written);
                EndPagePrinter(h);
                EndDocPrinter(h);
                if ok == 0 || written as usize != data.len() {
                    return Err(format!("write: error {}", GetLastError()));
                }
                Ok(())
            })();
            ClosePrinter(h);
            result
        }
    }

    pub fn list() -> Result<Vec<String>, String> {
        unsafe {
            let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, std::ptr::null(), 4, std::ptr::null_mut(), 0, &mut needed, &mut count);
            if needed == 0 {
                return Ok(vec![]);
            }
            let mut buf = vec![0u8; needed as usize];
            if EnumPrintersW(flags, std::ptr::null(), 4, buf.as_mut_ptr(), needed, &mut needed, &mut count) == 0 {
                return Err(format!("list printers: error {}", GetLastError()));
            }
            let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize);
            Ok(infos
                .iter()
                .map(|i| {
                    let mut n = 0;
                    while *i.pPrinterName.add(n) != 0 {
                        n += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(i.pPrinterName, n))
                })
                .collect())
        }
    }
}

pub fn system_printers() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        spooler::list()
    }
    #[cfg(not(windows))]
    {
        Err("USB printers are only available on the Windows app".into())
    }
}

pub fn send(p: &Printer, data: &[u8]) -> Result<(), String> {
    match p.kind.as_str() {
        "network" => send_network(&p.address, data),
        "usb" => {
            #[cfg(windows)]
            {
                spooler::print_raw(&p.address, data)
            }
            #[cfg(not(windows))]
            {
                Err("USB printers are only available on the Windows app".into())
            }
        }
        "bluetooth" | "sunmi" => {
            let target = if p.kind == "sunmi" && p.address.trim().is_empty() { SUNMI_BT_NAME } else { p.address.as_str() };
            BLUETOOTH.get().ok_or("Bluetooth printers are only available on the Android app")?.write(target, data)
        }
        other => Err(format!("unknown printer type {other}")),
    }
}

/// Bytes for a job (documents are encoded at print time with the printer's current settings).
pub fn job_bytes(p: &Printer, kind: &str, doc: &str) -> Result<Vec<u8>, String> {
    if kind == "drawer" {
        let cfg = p.cfg(true);
        let mut b = escpos::init_bytes(&cfg);
        b.extend(escpos::drawer_bytes(cfg.drawer.ok_or("this printer has no cash drawer")?));
        return Ok(b);
    }
    let lines: Vec<Line> = serde_json::from_str(doc).map_err(|e| format!("bad document: {e}"))?;
    Ok(escpos::encode(&lines, &p.cfg(false)))
}

// ------------------------------------------------------------------ worker

static WAKE: OnceLock<(Mutex<bool>, std::sync::Condvar)> = OnceLock::new();

fn wake() {
    if let Some((m, c)) = WAKE.get() {
        *m.lock().unwrap() = true;
        c.notify_all();
    }
}

fn backoff_secs(attempts: i64) -> i64 {
    // 2, 4, 8, 16, 30, 30, ... seconds.
    (1i64 << attempts.clamp(1, 5)).min(30)
}

/// Pick the next due job of every idle printer (oldest first per printer, so tickets stay in order).
fn due_jobs(conn: &Connection, busy: &HashSet<i64>) -> ApiResult<Vec<(i64, Printer, String, String)>> {
    let rows: Vec<(i64, i64, String, String)> = conn
        .prepare(
            "SELECT j.id, j.printer_id, j.kind, j.doc FROM print_jobs j JOIN printers p ON p.id=j.printer_id
             WHERE p.active=1 AND j.status IN ('pending','failed')
               AND j.id = (SELECT MIN(id) FROM print_jobs x WHERE x.printer_id=j.printer_id AND x.status IN ('pending','failed'))
               AND j.next_attempt_at <= strftime('%Y-%m-%dT%H:%M:%fZ','now')
             ORDER BY j.id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for (id, pid, kind, doc) in rows {
        if !busy.contains(&pid) {
            out.push((id, load_printer(conn, pid)?, kind, doc));
        }
    }
    Ok(out)
}

pub fn record_result(conn: &Connection, job_id: i64, printer_id: i64, result: &Result<(), String>) -> ApiResult<()> {
    match result {
        Ok(()) => {
            conn.execute("UPDATE print_jobs SET status='done', printed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'), last_error=NULL WHERE id=?1", [job_id])?;
            conn.execute("UPDATE printers SET last_ok_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'), last_error=NULL WHERE id=?1", [printer_id])?;
        }
        Err(e) => {
            let attempts: i64 = conn.query_row("UPDATE print_jobs SET attempts=attempts+1 WHERE id=?1 RETURNING attempts", [job_id], |r| r.get(0))?;
            conn.execute(
                "UPDATE print_jobs SET status='failed', last_error=?2, next_attempt_at=strftime('%Y-%m-%dT%H:%M:%fZ','now', ?3) WHERE id=?1",
                params![job_id, e, format!("+{} seconds", backoff_secs(attempts))],
            )?;
            conn.execute(
                "UPDATE printers SET last_error=?2, last_error_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",
                params![printer_id, e],
            )?;
        }
    }
    Ok(())
}

/// One pass of the worker; `send_fn` is injectable for tests.
pub fn run_once(db: &Arc<Mutex<Connection>>, busy: &Arc<Mutex<HashSet<i64>>>, send_fn: Arc<dyn Fn(&Printer, &[u8]) -> Result<(), String> + Send + Sync>, threaded: bool) -> usize {
    let due = {
        let conn = db.lock().unwrap();
        let b = busy.lock().unwrap();
        due_jobs(&conn, &b).unwrap_or_default()
    };
    let n = due.len();
    for (job_id, printer, kind, doc) in due {
        busy.lock().unwrap().insert(printer.id);
        let (db, busy, send_fn) = (db.clone(), busy.clone(), send_fn.clone());
        let work = move || {
            let result = job_bytes(&printer, &kind, &doc).and_then(|b| send_fn(&printer, &b));
            if let Ok(conn) = db.lock() {
                let _ = record_result(&conn, job_id, printer.id, &result);
            }
            busy.lock().unwrap().remove(&printer.id);
        };
        if threaded {
            std::thread::spawn(work);
        } else {
            work();
        }
    }
    n
}

pub fn start_worker(db: Arc<Mutex<Connection>>) {
    let pair = WAKE.get_or_init(|| (Mutex::new(false), std::sync::Condvar::new()));
    let busy = Arc::new(Mutex::new(HashSet::new()));
    let send_fn: Arc<dyn Fn(&Printer, &[u8]) -> Result<(), String> + Send + Sync> = Arc::new(|p, b| send(p, b));
    std::thread::spawn(move || loop {
        run_once(&db, &busy, send_fn.clone(), true);
        let (m, c) = pair;
        let mut woke = m.lock().unwrap();
        if !*woke {
            woke = c.wait_timeout(woke, Duration::from_millis(1000)).unwrap().0;
        }
        *woke = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::tests::{demo, new_order, simple_line};
    use crate::orders::{submit, SubmitIn};
    use std::io::Read;
    use std::net::TcpListener;

    fn add_printer(c: &Connection, me: &SessionUser, name: &str, address: &str, register: Option<&str>) -> i64 {
        save_printer(c, me, PrinterIn {
            id: None, name: name.into(), kind: "network".into(), address: address.into(), paper_mm: 80, codepage: "cp850".into(),
            graphic_mode: false, cut: true, drawer: true, receipt_register: register.map(String::from), ticket_lang: "fr".into(), active: true,
        }).unwrap()
    }
    fn station(c: &Connection, code: &str) -> i64 {
        c.query_row("SELECT id FROM stations WHERE code=?1", [code], |r| r.get(0)).unwrap()
    }
    fn docs(c: &Connection) -> Vec<(i64, String, String)> {
        c.prepare("SELECT printer_id, title, doc FROM print_jobs ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect()
    }

    #[test]
    fn address_validation() {
        assert_eq!(parse_network_address("192.168.1.50").unwrap(), ("192.168.1.50".into(), 9100));
        assert_eq!(parse_network_address("printer.local:9101").unwrap(), ("printer.local".into(), 9101));
        assert!(parse_network_address("").is_err());
        assert!(parse_network_address("1.2.3.4:0").is_err());
        assert!(parse_network_address("evil host; rm").is_err());
    }

    #[test]
    fn one_receipt_printer_per_register() {
        let (c, owner, _) = demo();
        let a = add_printer(&c, &owner, "Front", "127.0.0.1:9100", Some("main"));
        let b = add_printer(&c, &owner, "Front 2", "127.0.0.1:9101", Some("main"));
        assert_eq!(receipt_printer(&c, "main").unwrap().unwrap().id, b);
        assert!(load_printer(&c, a).unwrap().receipt_register.is_none());
    }

    #[test]
    fn combo_is_split_across_station_printers() {
        let (c, owner, cashier) = demo();
        let grill = add_printer(&c, &owner, "Grill", "127.0.0.1:1", None);
        let drinks = add_printer(&c, &owner, "Drinks", "127.0.0.1:2", None);
        set_station_routing(&c, &owner, station(&c, "grill"), Some(grill), true).unwrap();
        set_station_routing(&c, &owner, station(&c, "drinks"), Some(drinks), false).unwrap();
        // Fryer has no printer -> fries go to the screen only (Phase 5).
        let combo: i64 = c.query_row("SELECT id FROM products WHERE json_extract(names,'$.fr')='Menu Classic'", [], |r| r.get(0)).unwrap();
        let ix = MenuIndex::load(&c).unwrap();
        let mut cfg: ItemConfig = serde_json::from_value(json!({"product_id": combo, "variant_id": null, "modifiers": []})).unwrap();
        // Build defaults like the till does.
        let menu = crate::menu::get_menu(&c, false).unwrap();
        let p = menu.products.iter().find(|p| p["id"] == combo).unwrap().clone();
        let size_normal: i64 = c.query_row("SELECT id FROM modifier_options WHERE json_extract(names,'$.fr')='Normal'", [], |r| r.get(0)).unwrap();
        let a_point: i64 = c.query_row("SELECT id FROM modifier_options WHERE json_extract(names,'$.fr')='À point'", [], |r| r.get(0)).unwrap();
        cfg.modifiers = vec![item::ModSel { option_id: size_normal, qty: 1 }];
        cfg.combo = Some(p["slots"].as_array().unwrap().iter().map(|s| {
            let o = s["options"].as_array().unwrap().iter().find(|o| o["is_default"] == true).unwrap();
            let pid = o["product_id"].as_i64().unwrap();
            let is_classic = c.query_row("SELECT json_extract(names,'$.fr')='Classic Burger' FROM products WHERE id=?1", [pid], |r| r.get::<_, bool>(0)).unwrap();
            ItemConfig { product_id: pid, variant_id: o["variant_id"].as_i64(), modifiers: if is_classic { vec![item::ModSel { option_id: a_point, qty: 1 }] } else { vec![] }, combo: None }
        }).collect());
        item::validate(&ix, &cfg).unwrap();
        let mut note_line = simple_line(&c, "Coca-Cola 33cl", 2);
        note_line.note = "sans glaçons".into();
        submit(&c, &cashier, SubmitIn { order_id: None, kind: Some("dine_in".into()), table_id: Some(3), customer_name: None, customer_phone: None,
            customer_address: None, delivery_fee_cents: None, note: None,
            lines: vec![crate::orders::NewLine { config: cfg, qty: 1, note: String::new() }, note_line], send: true, hold: false, hold_label: None,
            register: "main".into() }).unwrap();
        let d = docs(&c);
        assert_eq!(d.len(), 2, "one ticket per printed station");
        let grill_doc = &d.iter().find(|x| x.0 == grill).unwrap().2;
        assert!(grill_doc.contains("1 x Classic Burger") && grill_doc.contains("À point") && grill_doc.contains("[Menu Classic · Normal]"));
        assert!(grill_doc.contains("NOUVELLE · Grill") && grill_doc.contains("Sur place · Table T3"));
        assert!(!grill_doc.contains("Coca"));
        let drinks_doc = &d.iter().find(|x| x.0 == drinks).unwrap().2;
        assert!(drinks_doc.contains("1 x Coca-Cola 33cl") && drinks_doc.contains("2 x Coca-Cola 33cl") && drinks_doc.contains("!! sans glaçons"));
        assert!(!drinks_doc.contains("Frites"), "fryer has no printer");
    }

    #[test]
    fn add_and_void_tickets_show_only_changes() {
        let (c, owner, cashier) = demo();
        let desk = add_printer(&c, &owner, "Assembly", "127.0.0.1:1", None);
        set_station_routing(&c, &owner, station(&c, "assembly"), Some(desk), true).unwrap();
        let id = new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1)], true);
        submit(&c, &cashier, SubmitIn { order_id: Some(id), kind: None, table_id: None, customer_name: None, customer_phone: None,
            customer_address: None, delivery_fee_cents: None, note: None, lines: vec![simple_line(&c, "Cookie", 2)], send: true,
            hold: false, hold_label: None, register: "main".into() }).unwrap();
        let cookie: i64 = c.query_row("SELECT id FROM order_lines WHERE order_id=?1 AND json_extract(names,'$.fr')='Cookie'", [id], |r| r.get(0)).unwrap();
        crate::orders::remove_line(&c, &cashier, cookie, Some("erreur".into()), Some("2222")).unwrap();
        let d = docs(&c);
        assert_eq!(d.len(), 3);
        assert!(d[0].2.contains("NOUVELLE") && d[0].2.contains("Tiramisu"));
        assert!(d[1].2.contains("AJOUT") && d[1].2.contains("2 x Cookie") && !d[1].2.contains("Tiramisu"));
        assert!(d[2].2.contains("ANNULATION") && d[2].2.contains("ANNULÉ 2 x Cookie"));
        assert_eq!(reprint_kitchen(&c, &owner, id).unwrap(), 1);
        let last = &docs(&c)[3].2;
        assert!(last.contains("RÉIMPRESSION") && last.contains("Tiramisu") && !last.contains("Cookie"));
    }

    #[test]
    fn no_printers_means_no_jobs_and_setting_can_disable() {
        let (c, owner, cashier) = demo();
        new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1)], true);
        assert!(docs(&c).is_empty());
        let p = add_printer(&c, &owner, "Assembly", "127.0.0.1:1", None);
        set_station_routing(&c, &owner, station(&c, "assembly"), Some(p), true).unwrap();
        let mut m = serde_json::Map::new();
        m.insert("print_kitchen_tickets".into(), json!(false));
        crate::settings::set_many(&c, &m).unwrap();
        new_order(&c, &cashier, vec![simple_line(&c, "Tiramisu", 1)], true);
        assert!(docs(&c).is_empty());
        assert!(set_station_routing(&c, &owner, station(&c, "assembly"), None, false).is_err());
    }

    #[test]
    fn queue_retries_in_order_and_never_drops_tickets() {
        let (c, owner, _) = demo();
        let p = add_printer(&c, &owner, "Grill", "127.0.0.1:1", None);
        let doc = vec![Line::Text { text: "A".into(), align: None, bold: None, big: None }];
        let j1 = enqueue(&c, p, "kitchen", None, None, "first", &doc, None).unwrap();
        let j2 = enqueue(&c, p, "kitchen", None, None, "second", &doc, None).unwrap();
        let db = Arc::new(Mutex::new(c));
        let busy = Arc::new(Mutex::new(HashSet::new()));
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let online = Arc::new(Mutex::new(false));
        let (s2, o2) = (sent.clone(), online.clone());
        let f: Arc<dyn Fn(&Printer, &[u8]) -> Result<(), String> + Send + Sync> = Arc::new(move |p, b| {
            if *o2.lock().unwrap() {
                s2.lock().unwrap().push(format!("{}:{}", p.name, b.len()));
                Ok(())
            } else {
                Err("connect refused".into())
            }
        });
        // Offline: only the first job is tried; the second waits behind it.
        assert_eq!(run_once(&db, &busy, f.clone(), false), 1);
        {
            let conn = db.lock().unwrap();
            assert_eq!(job_status(&conn, j1).unwrap()["status"], "failed");
            assert_eq!(job_status(&conn, j2).unwrap()["status"], "pending");
            assert_eq!(problems(&conn).unwrap()[0]["kitchen"], json!(1));
        }
        // Backoff: not due yet.
        assert_eq!(run_once(&db, &busy, f.clone(), false), 0);
        // Printer back + manual retry: both print, in order.
        *online.lock().unwrap() = true;
        retry(&db.lock().unwrap(), None, Some(p)).unwrap();
        assert_eq!(run_once(&db, &busy, f.clone(), false), 1);
        assert_eq!(run_once(&db, &busy, f.clone(), false), 1);
        let conn = db.lock().unwrap();
        assert_eq!(job_status(&conn, j1).unwrap()["status"], "done");
        assert_eq!(job_status(&conn, j2).unwrap()["status"], "done");
        assert_eq!(sent.lock().unwrap().len(), 2);
        assert!(problems(&conn).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn reroute_moves_open_jobs() {
        let (c, owner, _) = demo();
        let a = add_printer(&c, &owner, "Grill", "127.0.0.1:1", None);
        let b = add_printer(&c, &owner, "Drinks", "127.0.0.1:2", None);
        let doc = vec![Line::Feed];
        enqueue(&c, a, "kitchen", None, None, "x", &doc, None).unwrap();
        assert_eq!(reroute(&c, &owner, a, b).unwrap(), 1);
        assert_eq!(docs(&c)[0].0, b);
        assert!(reroute(&c, &owner, b, b).is_err());
    }

    #[test]
    fn network_transport_delivers_exact_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            buf
        });
        let p = Printer { id: 1, name: "t".into(), kind: "network".into(), address: addr.to_string(), paper_mm: 58, codepage: "cp850".into(),
            graphic_mode: false, cut: true, drawer: false, receipt_register: None, ticket_lang: "fr".into(), active: true };
        let bytes = job_bytes(&p, "test", &serde_json::to_string(&test_doc(&p, "Burger Lab")).unwrap()).unwrap();
        send(&p, &bytes).unwrap();
        let got = h.join().unwrap();
        assert_eq!(got, bytes);
        assert!(got.windows(4).any(|w| w == [0x82, b' ', 0x8A, b' '])); // "é è" in CP850
    }

    #[test]
    fn network_transport_reports_refused_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let e = send_network(&addr.to_string(), b"x").unwrap_err();
        assert!(e.contains("connect"), "{e}");
    }

    #[test]
    fn drawer_job_needs_a_drawer() {
        let mut p = Printer { id: 1, name: "t".into(), kind: "sunmi".into(), address: String::new(), paper_mm: 58, codepage: "cp850".into(),
            graphic_mode: false, cut: true, drawer: true, receipt_register: None, ticket_lang: "fr".into(), active: true };
        let b = job_bytes(&p, "drawer", "[]").unwrap();
        assert!(b.ends_with(&[0x10, 0x14, 0, 0, 0]), "SUNMI drawer command");
        p.drawer = false;
        assert!(job_bytes(&p, "drawer", "[]").is_err());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    #[test]
    fn lists_windows_printers() {
        // Every Windows install has at least "Microsoft Print to PDF" or similar.
        let list = super::system_printers().unwrap();
        assert!(!list.is_empty(), "no printers found");
    }
}
