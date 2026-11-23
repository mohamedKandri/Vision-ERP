//! Backups (Phase 9): copies of the whole database, made with SQLite's online backup (a
//! consistent snapshot even while the till is writing), each checked after writing.
//!
//! - **Automatic**: once a day on the Host when `backup_daily` is on; automatic copies older
//!   than `backup_keep_days` are deleted. Manual copies are never deleted automatically.
//! - **Manual**: "Back up now", and "copy to Downloads" to carry it on a USB stick.
//! - **Restore** (owner only): from a backup in the backups folder or a `.db` file put in
//!   Downloads/BURGER-OS. The current data is backed up first ("before-restore"), the file is
//!   checked (it must be a BURGER OS database that passes SQLite's integrity check), then
//!   copied over the live database and brought up to the current schema. Everyone logs in again.

use crate::db;
use crate::error::{ApiError, ApiResult};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Backups folder (set at start-up, next to the database).
pub static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn dir() -> ApiResult<&'static Path> {
    DIR.get().map(PathBuf::as_path).ok_or_else(|| ApiError::new("internal", "backup folder not ready"))
}

/// Where a backup comes from: "auto", "manual" or "before-restore".
pub fn kind_of(name: &str) -> Option<&'static str> {
    ["auto", "manual", "before-restore"].into_iter().find(|k| name.starts_with(&format!("{k}_")))
}

/// A backup file name we wrote ourselves: kind_YYYY-MM-DD_HHMMSS(-n).db
pub fn valid_name(name: &str) -> bool {
    let Some(kind) = kind_of(name) else { return false };
    let rest = &name[kind.len() + 1..];
    let Some(stem) = rest.strip_suffix(".db") else { return false };
    let b = stem.as_bytes();
    b.len() >= 17
        && b.len() <= 22
        && b[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
        && b[10] == b'_'
        && b[11..17].iter().all(u8::is_ascii_digit)
        && (b.len() == 17 || (b[17] == b'-' && b[18..].iter().all(u8::is_ascii_digit)))
}

/// A `.db` file someone put in the export folder to restore from: plain characters only.
pub fn valid_import_name(name: &str) -> bool {
    name.len() <= 100
        && name.to_ascii_lowercase().ends_with(".db")
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || " ._-()".contains(c))
}

/// The day part of a backup name ("2026-11-23").
pub fn day_of(name: &str) -> Option<&str> {
    let kind = kind_of(name)?;
    name.get(kind.len() + 1..kind.len() + 11)
}

/// Automatic backups to delete: older than `keep_days` before `today` (YYYY-MM-DD strings).
pub fn to_prune<'a>(names: &'a [String], today: &str, keep_days: i64, days_between: impl Fn(&str, &str) -> i64) -> Vec<&'a String> {
    names
        .iter()
        .filter(|n| kind_of(n) == Some("auto") && valid_name(n))
        .filter(|n| day_of(n).map(|d| days_between(d, today) >= keep_days).unwrap_or(false))
        .collect()
}

fn local_stamp(conn: &Connection) -> ApiResult<(String, String)> {
    Ok(conn.query_row(
        "SELECT strftime('%Y-%m-%d_%H%M%S','now','localtime'), strftime('%Y-%m-%d','now','localtime')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn days_between(conn: &Connection) -> impl Fn(&str, &str) -> i64 + '_ {
    move |a: &str, b: &str| {
        conn.query_row("SELECT CAST(julianday(?2) - julianday(?1) AS INTEGER)", [a, b], |r| r.get(0)).unwrap_or(0)
    }
}

/// Is this a BURGER OS database we can open? Returns its schema version.
pub fn check_file(path: &Path) -> ApiResult<i64> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| ApiError::new("bad_backup", "not a database file"))?;
    let ok: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|_| ApiError::new("bad_backup", "not a database file"))?;
    if ok != "ok" {
        return Err(ApiError::new("bad_backup", format!("damaged file: {ok}")));
    }
    let version: Option<i64> = c
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
        .optional()
        .map_err(|_| ApiError::new("bad_backup", "not a Vision database"))?
        .flatten();
    let has_core: bool = c
        .query_row("SELECT COUNT(*) = 3 FROM sqlite_master WHERE type='table' AND name IN ('users','orders','settings')", [], |r| r.get(0))
        .unwrap_or(false);
    // BURGER OS uses no triggers or views: a file that has some was not made by it, and a
    // trigger would run its own SQL inside the shop's database after a restore.
    let extra: i64 = c
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type IN ('trigger','view')", [], |r| r.get(0))
        .unwrap_or(1);
    if extra > 0 {
        return Err(ApiError::new("bad_backup", "this file contains triggers or views"));
    }
    match version {
        Some(v) if has_core && v >= 1 => {
            if v > db::MIGRATIONS.last().map(|m| m.version).unwrap_or(0) {
                return Err(ApiError::new("bad_backup", "this backup comes from a newer version of Vision"));
            }
            Ok(v)
        }
        _ => Err(ApiError::new("bad_backup", "not a Vision database")),
    }
}

/// Write a backup of `conn` into `dir`. Returns its name.
pub fn create_in(conn: &Connection, dir: &Path, kind: &str) -> ApiResult<String> {
    std::fs::create_dir_all(dir).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
    let (stamp, _) = local_stamp(conn)?;
    let mut name = format!("{kind}_{stamp}.db");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{kind}_{stamp}-{n}.db");
        n += 1;
    }
    let path = dir.join(&name);
    let tmp = dir.join(format!("{name}.part"));
    let _ = std::fs::remove_file(&tmp);
    {
        let mut dst = Connection::open(&tmp).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
        let b = rusqlite::backup::Backup::new(conn, &mut dst).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
        b.run_to_completion(256, Duration::from_millis(0), None).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
    }
    if let Err(e) = check_file(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::new("backup_failed", e.message));
    }
    std::fs::rename(&tmp, &path).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
    Ok(name)
}

fn file_info(path: &Path, location: &str) -> Option<Value> {
    let meta = std::fs::metadata(path).ok()?;
    let name = path.file_name()?.to_str()?.to_string();
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Some(json!({"name": name, "kind": kind_of(&name).unwrap_or("file"), "size": meta.len(), "modified": modified, "location": location}))
}

/// Backups in the backups folder, then `.db` files in the export folder, newest first.
pub fn list_in(dir: &Path, import_dir: Option<&Path>) -> Vec<Value> {
    let mut out = Vec::new();
    for (d, loc, ok) in [(Some(dir), "backups", valid_name as fn(&str) -> bool), (import_dir, "downloads", valid_import_name)] {
        let Some(d) = d else { continue };
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if p.is_file() && ok(&name) {
                if let Some(v) = file_info(&p, loc) {
                    out.push(v);
                }
            }
        }
    }
    out.sort_by(|a, b| b["modified"].as_u64().cmp(&a["modified"].as_u64()).then(b["name"].as_str().cmp(&a["name"].as_str())));
    out
}

/// Daily automatic backup: makes today's if missing, then prunes old automatic ones.
/// Returns the name when one was made.
pub fn daily_in(conn: &Connection, dir: &Path) -> ApiResult<Option<String>> {
    let s = crate::settings::get_all(conn)?;
    if s["backup_daily"] != json!(true) {
        return Ok(None);
    }
    let (_, today) = local_stamp(conn)?;
    let names: Vec<String> = list_in(dir, None).iter().filter_map(|v| v["name"].as_str().map(String::from)).collect();
    let made = if names.iter().any(|n| kind_of(n) == Some("auto") && day_of(n) == Some(today.as_str())) {
        None
    } else {
        Some(create_in(conn, dir, "auto")?)
    };
    let keep = s["backup_keep_days"].as_i64().unwrap_or(30).max(1);
    let names: Vec<String> = list_in(dir, None).iter().filter_map(|v| v["name"].as_str().map(String::from)).collect();
    for n in to_prune(&names, &today, keep, days_between(conn)) {
        let _ = std::fs::remove_file(dir.join(n));
    }
    Ok(made)
}

/// Restore `src` over the live database. The caller has checked permissions.
pub fn restore_from(conn: &mut Connection, dir: &Path, src: &Path) -> ApiResult<String> {
    check_file(src)?;
    let safety = create_in(conn, dir, "before-restore")?;
    conn.restore(rusqlite::MAIN_DB, src, None::<fn(rusqlite::backup::Progress)>)
        .map_err(|e| ApiError::new("restore_failed", e.to_string()))?;
    db::migrate(conn).map_err(|e| ApiError::new("restore_failed", e.to_string()))?;
    conn.pragma_update(None, "foreign_keys", "ON").ok();
    Ok(safety)
}

/// Find a file by name in the backups folder or the export folder (never a path).
pub fn resolve(dir: &Path, import_dir: Option<&Path>, name: &str, location: &str) -> ApiResult<PathBuf> {
    let p = match location {
        "backups" if valid_name(name) => dir.join(name),
        "downloads" if valid_import_name(name) => import_dir.ok_or_else(|| ApiError::not_found("file"))?.join(name),
        _ => return Err(ApiError::invalid("bad file name")),
    };
    if !p.is_file() {
        return Err(ApiError::not_found("file"));
    }
    Ok(p)
}

/// Background thread on the Host: try the daily backup now and every 30 minutes.
pub fn start_daily(conn: std::sync::Arc<std::sync::Mutex<Connection>>) {
    std::thread::spawn(move || loop {
        if let (Ok(dir), false) = (dir(), crate::net::is_client()) {
            let c = conn.lock().unwrap();
            if let Ok(Some(name)) = daily_in(&c, dir) {
                let _ = crate::audit::log(&c, None, "backup.auto", None, Some(&name), None);
            }
        }
        std::thread::sleep(Duration::from_secs(30 * 60));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("auto_2026-11-23_093000.db"));
        assert!(valid_name("manual_2026-11-23_093000-2.db"));
        assert!(valid_name("before-restore_2026-11-23_093000.db"));
        for bad in ["auto_2026-11-23.db", "x_2026-11-23_093000.db", "auto_2026-11-23_093000.db.part", "../auto_2026-11-23_093000.db",
            "auto_2026-1a-23_093000.db", "manual_2026-11-23_093000-x.db"] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_import_name("burger-os (1).db"));
        assert!(!valid_import_name("..\\x.db"));
        assert!(!valid_import_name("a/b.db"));
        assert!(!valid_import_name("x.exe"));
        assert_eq!(day_of("auto_2026-11-23_093000.db"), Some("2026-11-23"));
        // Pruning: only automatic backups, only the old ones.
        let names: Vec<String> = ["auto_2026-11-01_090000.db", "auto_2026-11-20_090000.db", "manual_2026-10-01_090000.db", "auto_2026-10-24_090000.db"]
            .iter().map(|s| s.to_string()).collect();
        let days = |a: &str, b: &str| {
            let d = |s: &str| s[0..4].parse::<i64>().unwrap() * 372 + s[5..7].parse::<i64>().unwrap() * 31 + s[8..10].parse::<i64>().unwrap();
            d(b) - d(a)
        };
        let gone: Vec<&str> = to_prune(&names, "2026-11-23", 20, days).into_iter().map(|s| s.as_str()).collect();
        assert_eq!(gone, vec!["auto_2026-11-01_090000.db", "auto_2026-10-24_090000.db"]);
    }

    #[test]
    fn api_permissions() {
        use crate::api::{dispatch, tests::{core_with_setup, login}};
        let tmp = tempfile::tempdir().unwrap();
        let _ = DIR.set(tmp.path().join("backups"));
        let core = core_with_setup();
        let (owner, manager, cashier) = (login(&core, "Mohamed", "1234"), login(&core, "Karim", "2222"), login(&core, "Salma", "3333"));
        assert_eq!(dispatch(&core, Some(&cashier), "backup.list", Value::Null).unwrap_err().code, "forbidden");
        assert_eq!(dispatch(&core, Some(&manager), "backup.create", Value::Null).unwrap_err().code, "forbidden", "needs settings.manage");
        dispatch(&core, Some(&owner), "permissions.set", json!({"role": "manager", "permissions": ["settings.manage", "reports.view"]})).unwrap();
        let made = dispatch(&core, Some(&manager), "backup.create", Value::Null).unwrap();
        let name = made["name"].as_str().unwrap().to_string();
        let items = dispatch(&core, Some(&manager), "backup.list", Value::Null).unwrap();
        assert!(items["items"].as_array().unwrap().iter().any(|i| i["name"] == json!(name)));
        let p = json!({"name": name, "location": "backups"});
        assert_eq!(dispatch(&core, Some(&manager), "backup.restore", p.clone()).unwrap_err().code, "forbidden", "owner only");
        assert_eq!(dispatch(&core, Some(&owner), "backup.restore", json!({"name": "../burger-os.db", "location": "backups"})).unwrap_err().code, "invalid");
        dispatch(&core, Some(&owner), "backup.restore", p).unwrap();
        assert_eq!(dispatch(&core, Some(&owner), "settings.get", Value::Null).unwrap_err().code, "unauthorized", "everyone logs in again");
        assert_eq!(crate::net::topic_of("backup.restore"), Some("all"));
        assert_eq!(crate::net::topic_of("backup.list"), None);
    }

    #[test]
    fn backup_daily_and_restore_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("backups");
        let mut c = db::open(&tmp.path().join("live.db")).unwrap();
        c.execute("INSERT INTO users (name, role, pin_hash) VALUES ('Owner', 'owner', 'x')", []).unwrap();
        crate::settings::set_many(&c, &serde_json::from_value(json!({"shop_name": "Avant"})).unwrap()).unwrap();

        let name = create_in(&c, &dir, "manual").unwrap();
        assert!(valid_name(&name) && kind_of(&name) == Some("manual"));
        assert_eq!(check_file(&dir.join(&name)).unwrap(), db::MIGRATIONS.last().unwrap().version);
        let second = create_in(&c, &dir, "manual").unwrap();
        assert_ne!(name, second, "never overwrites");

        // Daily: one automatic backup per day; turned off = none.
        let a = daily_in(&c, &dir).unwrap();
        assert!(a.is_some());
        assert_eq!(daily_in(&c, &dir).unwrap(), None, "already done today");
        crate::settings::set_many(&c, &serde_json::from_value(json!({"backup_daily": false})).unwrap()).unwrap();
        std::fs::remove_file(dir.join(a.unwrap())).unwrap();
        assert_eq!(daily_in(&c, &dir).unwrap(), None);
        // An old automatic backup is pruned; manual ones stay.
        crate::settings::set_many(&c, &serde_json::from_value(json!({"backup_daily": true, "backup_keep_days": 7})).unwrap()).unwrap();
        std::fs::copy(dir.join(&name), dir.join("auto_2020-01-01_120000.db")).unwrap();
        daily_in(&c, &dir).unwrap();
        let names: Vec<String> = list_in(&dir, None).iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        assert!(!names.contains(&"auto_2020-01-01_120000.db".to_string()));
        assert!(names.contains(&name) && names.contains(&second));

        // Change data, then restore the first backup: the change is gone, a safety copy exists.
        crate::settings::set_many(&c, &serde_json::from_value(json!({"shop_name": "Après"})).unwrap()).unwrap();
        c.execute("INSERT INTO users (name, role, pin_hash) VALUES ('Nouveau', 'cashier', 'x')", []).unwrap();
        let src = resolve(&dir, None, &name, "backups").unwrap();
        let safety = restore_from(&mut c, &dir, &src).unwrap();
        assert_eq!(crate::settings::get(&c, "shop_name").unwrap(), json!("Avant"));
        assert_eq!(c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(kind_of(&safety), Some("before-restore"));
        // ...and restoring the safety copy brings the change back.
        let back = resolve(&dir, None, &safety, "backups").unwrap();
        restore_from(&mut c, &dir, &back).unwrap();
        assert_eq!(crate::settings::get(&c, "shop_name").unwrap(), json!("Après"));

        // Refused: not a database, a damaged file, a non-BURGER OS database, a bad name.
        let imp = tmp.path().join("downloads");
        std::fs::create_dir_all(&imp).unwrap();
        std::fs::write(imp.join("notes.db"), b"hello, not sqlite at all").unwrap();
        let p = resolve(&dir, Some(&imp), "notes.db", "downloads").unwrap();
        assert_eq!(restore_from(&mut c, &dir, &p).unwrap_err().code, "bad_backup");
        let other = imp.join("other.db");
        Connection::open(&other).unwrap().execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
        assert_eq!(check_file(&other).unwrap_err().code, "bad_backup");
        // A real backup with a trigger added is refused.
        let trap = imp.join("trap.db");
        std::fs::copy(dir.join(&name), &trap).unwrap();
        Connection::open(&trap).unwrap()
            .execute_batch("CREATE TRIGGER t AFTER INSERT ON orders BEGIN UPDATE users SET role='owner'; END;").unwrap();
        assert_eq!(check_file(&trap).unwrap_err().code, "bad_backup");
        assert_eq!(resolve(&dir, Some(&imp), "../live.db", "downloads").unwrap_err().code, "invalid");
        assert_eq!(resolve(&dir, None, "live.db", "backups").unwrap_err().code, "invalid");
        assert_eq!(crate::settings::get(&c, "shop_name").unwrap(), json!("Après"), "failed restores change nothing");
        // A good copy in Downloads is listed and can be restored.
        std::fs::copy(dir.join(&name), imp.join("burger-os (1).db")).unwrap();
        let listed = list_in(&dir, Some(&imp));
        assert!(listed.iter().any(|v| v["name"] == "burger-os (1).db" && v["location"] == "downloads"));
        let p = resolve(&dir, Some(&imp), "burger-os (1).db", "downloads").unwrap();
        restore_from(&mut c, &dir, &p).unwrap();
        assert_eq!(crate::settings::get(&c, "shop_name").unwrap(), json!("Avant"));
    }
}
