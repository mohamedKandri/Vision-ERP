//! PIN hashing, login with lockout, sessions and the permission matrix.

use crate::error::{ApiError, ApiResult};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const ROLES: &[&str] = &["owner", "manager", "cashier", "kitchen"];

/// Every permission the app knows. Owner implicitly has all of them.
pub const PERMISSIONS: &[&str] = &[
    "pos.sell",
    "pos.discount",
    "pos.void",
    "pos.refund",
    "pos.reopen",
    "cash.session",
    "cash.in_out",
    "drawer.open",
    "kds.use",
    "menu.manage",
    "inventory.manage",
    "purchasing.manage",
    "customers.manage",
    "reports.view",
    "audit.view",
    "users.manage",
    "settings.manage",
    "backup.manage",
];

pub const MAX_FAILED_ATTEMPTS: i64 = 5;
pub const LOCKOUT_SECS: i64 = 60;

#[derive(Debug, Clone, Serialize)]
pub struct SessionUser {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub language: String,
    pub theme: String,
    pub permissions: Vec<String>,
}

pub fn validate_pin(pin: &str) -> ApiResult<()> {
    let ok = (4..=6).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(ApiError::new("pin_format", "PIN must be 4 to 6 digits"))
    }
}

pub fn hash_pin(pin: &str) -> ApiResult<String> {
    validate_pin(pin)?;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::new("internal", format!("hash: {e}")))
}

pub fn verify_pin(pin: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default().verify_password(pin.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

pub fn new_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub fn permissions_for_role(conn: &Connection, role: &str) -> ApiResult<Vec<String>> {
    if role == "owner" {
        return Ok(PERMISSIONS.iter().map(|s| s.to_string()).collect());
    }
    let mut stmt = conn.prepare("SELECT permission FROM role_permissions WHERE role = ?1 ORDER BY permission")?;
    let rows = stmt.query_map([role], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn load_session_user(conn: &Connection, user_id: i64) -> ApiResult<SessionUser> {
    let row = conn
        .query_row(
            "SELECT id, name, role, language, theme FROM users WHERE id = ?1 AND active = 1",
            [user_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("user"))?;
    let permissions = permissions_for_role(conn, &row.2)?;
    Ok(SessionUser { id: row.0, name: row.1, role: row.2, language: row.3, theme: row.4, permissions })
}

/// Seconds the user must still wait, if locked.
fn lock_remaining(conn: &Connection, user_id: i64) -> ApiResult<Option<i64>> {
    let secs: Option<i64> = conn
        .query_row(
            "SELECT CAST(ROUND((julianday(locked_until) - julianday('now')) * 86400) AS INTEGER)
             FROM users WHERE id = ?1 AND locked_until IS NOT NULL AND locked_until > strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            [user_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(secs.map(|s| s.max(1)))
}

/// Verify a user's PIN, applying the lockout rules. Records failures and success in the audit log.
pub fn check_login(conn: &Connection, user_id: i64, pin: &str) -> ApiResult<SessionUser> {
    let hash: String = conn
        .query_row("SELECT pin_hash FROM users WHERE id = ?1 AND active = 1", [user_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| ApiError::not_found("user"))?;

    if let Some(secs) = lock_remaining(conn, user_id)? {
        return Err(ApiError { retry_after_secs: Some(secs), ..ApiError::new("locked", "too many wrong PINs") });
    }

    if verify_pin(pin, &hash) {
        conn.execute(
            "UPDATE users SET failed_attempts = 0, locked_until = NULL WHERE id = ?1",
            [user_id],
        )?;
        crate::audit::log(conn, Some(user_id), "auth.login", Some("user"), Some(&user_id.to_string()), None)?;
        return load_session_user(conn, user_id);
    }

    let attempts: i64 = conn.query_row(
        "UPDATE users SET failed_attempts = failed_attempts + 1 WHERE id = ?1 RETURNING failed_attempts",
        [user_id],
        |r| r.get(0),
    )?;
    crate::audit::log(
        conn,
        Some(user_id),
        "auth.login_failed",
        Some("user"),
        Some(&user_id.to_string()),
        Some(serde_json::json!({ "attempts": attempts })),
    )?;
    if attempts >= MAX_FAILED_ATTEMPTS {
        conn.execute(
            "UPDATE users SET failed_attempts = 0,
                locked_until = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?2) WHERE id = ?1",
            params![user_id, format!("+{LOCKOUT_SECS} seconds")],
        )?;
        return Err(ApiError {
            retry_after_secs: Some(LOCKOUT_SECS),
            ..ApiError::new("locked", "too many wrong PINs")
        });
    }
    Err(ApiError::new("wrong_pin", format!("{} attempts left", MAX_FAILED_ATTEMPTS - attempts)))
}

/// Manager override: find an active user whose PIN matches and who holds `permission`.
/// Used when a cashier needs approval (discount, void, refund...).
pub fn authorize_override(conn: &Connection, pin: &str, permission: &str) -> ApiResult<SessionUser> {
    validate_pin(pin)?;
    let mut stmt = conn.prepare(
        "SELECT u.id, u.pin_hash FROM users u
         WHERE u.active = 1 AND (u.role = 'owner' OR EXISTS
           (SELECT 1 FROM role_permissions p WHERE p.role = u.role AND p.permission = ?1))",
    )?;
    let candidates = stmt
        .query_map([permission], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, hash) in candidates {
        if lock_remaining(conn, id)?.is_none() && verify_pin(pin, &hash) {
            return load_session_user(conn, id);
        }
    }
    Err(ApiError::new("wrong_pin", "no authorised user with that PIN"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn add_user(conn: &Connection, name: &str, role: &str, pin: &str) -> i64 {
        conn.execute(
            "INSERT INTO users (name, role, pin_hash) VALUES (?1, ?2, ?3)",
            params![name, role, hash_pin(pin).unwrap()],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn pin_format_rules() {
        assert!(validate_pin("1234").is_ok());
        assert!(validate_pin("123456").is_ok());
        assert!(validate_pin("123").is_err());
        assert!(validate_pin("1234567").is_err());
        assert!(validate_pin("12a4").is_err());
        assert!(validate_pin("").is_err());
    }

    #[test]
    fn hash_and_verify() {
        let h = hash_pin("4821").unwrap();
        assert!(h.starts_with("$argon2"));
        assert!(verify_pin("4821", &h));
        assert!(!verify_pin("4822", &h));
        assert!(!verify_pin("4821", "garbage"));
    }

    #[test]
    fn login_success_resets_failures() {
        let c = open_in_memory().unwrap();
        let id = add_user(&c, "Sara", "cashier", "1111");
        assert_eq!(check_login(&c, id, "9999").unwrap_err().code, "wrong_pin");
        let u = check_login(&c, id, "1111").unwrap();
        assert_eq!(u.role, "cashier");
        assert!(u.permissions.contains(&"pos.sell".to_string()));
        assert!(!u.permissions.contains(&"pos.discount".to_string()));
        let f: i64 = c.query_row("SELECT failed_attempts FROM users WHERE id=?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(f, 0);
    }

    #[test]
    fn lockout_after_five_wrong_pins_blocks_even_correct_pin() {
        let c = open_in_memory().unwrap();
        let id = add_user(&c, "Sara", "cashier", "1111");
        for _ in 0..4 {
            assert_eq!(check_login(&c, id, "0000").unwrap_err().code, "wrong_pin");
        }
        let e = check_login(&c, id, "0000").unwrap_err();
        assert_eq!(e.code, "locked");
        assert_eq!(e.retry_after_secs, Some(LOCKOUT_SECS));
        let e = check_login(&c, id, "1111").unwrap_err();
        assert_eq!(e.code, "locked");
        // Expire the lock and the right PIN works again.
        c.execute("UPDATE users SET locked_until = '2000-01-01T00:00:00.000Z' WHERE id=?1", [id]).unwrap();
        assert!(check_login(&c, id, "1111").is_ok());
    }

    #[test]
    fn inactive_user_cannot_login() {
        let c = open_in_memory().unwrap();
        let id = add_user(&c, "Old", "cashier", "1111");
        c.execute("UPDATE users SET active = 0 WHERE id=?1", [id]).unwrap();
        assert_eq!(check_login(&c, id, "1111").unwrap_err().code, "not_found");
    }

    #[test]
    fn owner_has_all_permissions() {
        let c = open_in_memory().unwrap();
        assert_eq!(permissions_for_role(&c, "owner").unwrap().len(), PERMISSIONS.len());
        assert_eq!(permissions_for_role(&c, "kitchen").unwrap(), vec!["kds.use".to_string()]);
    }

    #[test]
    fn override_needs_permission() {
        let c = open_in_memory().unwrap();
        add_user(&c, "Cash", "cashier", "1111");
        let m = add_user(&c, "Boss", "manager", "2222");
        assert_eq!(authorize_override(&c, "1111", "pos.discount").unwrap_err().code, "wrong_pin");
        assert_eq!(authorize_override(&c, "2222", "pos.discount").unwrap().id, m);
        assert_eq!(authorize_override(&c, "2222", "settings.manage").unwrap_err().code, "wrong_pin");
    }

    #[test]
    fn login_writes_audit() {
        let c = open_in_memory().unwrap();
        let id = add_user(&c, "Sara", "cashier", "1111");
        let _ = check_login(&c, id, "0000");
        check_login(&c, id, "1111").unwrap();
        let actions: Vec<String> = c
            .prepare("SELECT action FROM audit_log ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(actions, vec!["auth.login_failed", "auth.login"]);
    }
}
