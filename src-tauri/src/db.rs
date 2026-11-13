//! SQLite connection setup and forward-only migrations.
//!
//! Migrations are embedded SQL files applied in order inside one transaction each.
//! The applied version is tracked in `schema_migrations`; an already-migrated DB is a no-op.

use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "core", sql: include_str!("../migrations/0001_core.sql") },
    Migration { version: 2, name: "menu", sql: include_str!("../migrations/0002_menu.sql") },
    Migration { version: 3, name: "orders", sql: include_str!("../migrations/0003_orders.sql") },
    Migration { version: 4, name: "printing", sql: include_str!("../migrations/0004_printing.sql") },
    Migration { version: 5, name: "devices_kds", sql: include_str!("../migrations/0005_devices_kds.sql") },
];

/// A nestable transaction built on SAVEPOINT. Commits with `.commit()`; rolls back when
/// dropped without commit (early `return Err`, `?`, panic). Derefs to `Connection`.
pub struct Atomic<'a> {
    conn: &'a Connection,
    done: bool,
}

pub fn atomic(conn: &Connection) -> rusqlite::Result<Atomic<'_>> {
    conn.execute_batch("SAVEPOINT atomic")?;
    Ok(Atomic { conn, done: false })
}

impl Atomic<'_> {
    pub fn commit(mut self) -> rusqlite::Result<()> {
        self.done = true;
        self.conn.execute_batch("RELEASE atomic")
    }
}

impl std::ops::Deref for Atomic<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn
    }
}

impl Drop for Atomic<'_> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.conn.execute_batch("ROLLBACK TO atomic; RELEASE atomic");
        }
    }
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    // WAL survives crashes better and lets reports read while the till writes.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );",
    )?;
    let v: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(v.unwrap_or(0))
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_with(conn, MIGRATIONS)
}

pub fn migrate_with(conn: &Connection, migrations: &[Migration]) -> rusqlite::Result<()> {
    let current = current_version(conn)?;
    for m in migrations.iter().filter(|m| m.version > current) {
        let tx = atomic(conn)?;
        tx.execute_batch(m.sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)",
            rusqlite::params![m.version, m.name],
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_db_to_latest() {
        let conn = open_in_memory().unwrap();
        assert_eq!(current_version(&conn).unwrap(), MIGRATIONS.last().unwrap().version);
        let taxes: i64 = conn.query_row("SELECT COUNT(*) FROM taxes", [], |r| r.get(0)).unwrap();
        assert_eq!(taxes, 4);
        let stations: i64 = conn.query_row("SELECT COUNT(*) FROM stations", [], |r| r.get(0)).unwrap();
        assert_eq!(stations, 4);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
    }

    #[test]
    fn failed_migration_rolls_back_and_keeps_version() {
        let conn = open_in_memory().unwrap();
        let bad = [Migration {
            version: 999,
            name: "bad",
            sql: "CREATE TABLE half_done (id INTEGER); THIS IS NOT SQL;",
        }];
        assert!(migrate_with(&conn, &bad).is_err());
        assert_eq!(current_version(&conn).unwrap(), MIGRATIONS.last().unwrap().version);
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'half_done'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "partial migration must be rolled back");
    }

    #[test]
    fn file_db_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let c = open(&path).unwrap();
            c.execute("INSERT INTO settings (key, value) VALUES ('x', '1')", []).unwrap();
        }
        let c = open(&path).unwrap();
        let v: String = c.query_row("SELECT value FROM settings WHERE key='x'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "1");
    }

    #[test]
    fn atomic_nests_and_rolls_back() {
        let c = open_in_memory().unwrap();
        let count = |c: &Connection| -> i64 { c.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0)).unwrap() };
        {
            let outer = atomic(&c).unwrap();
            outer.execute("INSERT INTO settings VALUES ('a','1')", []).unwrap();
            {
                let inner = atomic(&outer).unwrap();
                inner.execute("INSERT INTO settings VALUES ('b','1')", []).unwrap();
                // dropped without commit -> only 'b' is undone
            }
            assert_eq!(count(&outer), 1);
            let inner = atomic(&outer).unwrap();
            inner.execute("INSERT INTO settings VALUES ('c','1')", []).unwrap();
            inner.commit().unwrap();
            outer.commit().unwrap();
        }
        assert_eq!(count(&c), 2);
        {
            let t = atomic(&c).unwrap();
            t.execute("INSERT INTO settings VALUES ('d','1')", []).unwrap();
        }
        assert_eq!(count(&c), 2, "uncommitted outer rolls back");
    }

    #[test]
    fn foreign_keys_enforced() {
        let conn = open_in_memory().unwrap();
        let r = conn.execute("INSERT INTO audit_log (user_id, action) VALUES (424242, 'x')", []);
        assert!(r.is_err());
    }
}
