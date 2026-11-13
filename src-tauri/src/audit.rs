use crate::error::ApiResult;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

pub fn log(
    conn: &Connection,
    user_id: Option<i64>,
    action: &str,
    entity: Option<&str>,
    entity_id: Option<&str>,
    details: Option<Value>,
) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO audit_log (user_id, action, entity, entity_id, details) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, action, entity, entity_id, details.map(|d| d.to_string())],
    )?;
    Ok(())
}

#[derive(Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: String,
    pub user_id: Option<i64>,
    pub user_name: Option<String>,
    pub action: String,
    pub entity: Option<String>,
    pub entity_id: Option<String>,
    pub details: Option<Value>,
}

pub fn list(conn: &Connection, limit: i64, offset: i64) -> ApiResult<Vec<AuditEntry>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.at, a.user_id, u.name, a.action, a.entity, a.entity_id, a.details
         FROM audit_log a LEFT JOIN users u ON u.id = a.user_id
         ORDER BY a.id DESC LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map(params![limit.clamp(1, 500), offset.max(0)], |r| {
        let details: Option<String> = r.get(7)?;
        Ok(AuditEntry {
            id: r.get(0)?,
            at: r.get(1)?,
            user_id: r.get(2)?,
            user_name: r.get(3)?,
            action: r.get(4)?,
            entity: r.get(5)?,
            entity_id: r.get(6)?,
            details: details.and_then(|d| serde_json::from_str(&d).ok()),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
