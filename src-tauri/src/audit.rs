use crate::error::ApiResult;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
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

/// Filters for the audit log screen and its CSV export. Every field is optional.
#[derive(Deserialize, Default)]
pub struct Filter {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// UTC instants, `from` included, `to` excluded.
    pub from: Option<String>,
    pub to: Option<String>,
    pub user_id: Option<i64>,
    /// Action prefixes, e.g. ["order.", "cash."]; empty = all.
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// Free text searched in the action, entity id and details.
    pub q: Option<String>,
}

pub fn list(conn: &Connection, f: &Filter) -> ApiResult<Vec<AuditEntry>> {
    let mut sql = String::from(
        "SELECT a.id, a.at, a.user_id, u.name, a.action, a.entity, a.entity_id, a.details
         FROM audit_log a LEFT JOIN users u ON u.id = a.user_id WHERE 1=1",
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    // Appends a placeholder bound to `v` and returns its name (?1, ?2, ...).
    fn bind(args: &mut Vec<rusqlite::types::Value>, v: rusqlite::types::Value) -> String {
        args.push(v);
        format!("?{}", args.len())
    }
    for (op, v) in [(">=", &f.from), ("<", &f.to)] {
        if let Some(v) = v {
            if !crate::reports::valid_instant(v) {
                return Err(crate::error::ApiError::invalid("bad date"));
            }
            let n = bind(&mut args, v.clone().into());
            sql.push_str(&format!(" AND a.at {op} {n}"));
        }
    }
    if let Some(u) = f.user_id {
        let n = bind(&mut args, u.into());
        sql.push_str(&format!(" AND a.user_id = {n}"));
    }
    let prefixes: Vec<&String> = f.prefixes.iter().filter(|p| !p.is_empty() && p.len() <= 40).take(20).collect();
    if !prefixes.is_empty() {
        // Prefix match without LIKE, so the user's text has no wildcards.
        let conds: Vec<String> = prefixes
            .into_iter()
            .map(|p| {
                let n = bind(&mut args, p.clone().into());
                format!("substr(a.action, 1, length({n})) = {n}")
            })
            .collect();
        sql.push_str(&format!(" AND ({})", conds.join(" OR ")));
    }
    if let Some(q) = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let n = bind(&mut args, q.chars().take(100).collect::<String>().into());
        sql.push_str(&format!(
            " AND (instr(lower(a.action), lower({n})) > 0 OR instr(lower(COALESCE(a.entity_id,'')), lower({n})) > 0
               OR instr(lower(COALESCE(a.details,'')), lower({n})) > 0 OR instr(lower(COALESCE(u.name,'')), lower({n})) > 0)"
        ));
    }
    let n = bind(&mut args, f.limit.unwrap_or(100).clamp(1, 5000).into());
    let o = bind(&mut args, f.offset.unwrap_or(0).max(0).into());
    sql.push_str(&format!(" ORDER BY a.id DESC LIMIT {n} OFFSET {o}"));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use serde_json::json;

    #[test]
    fn filters_by_prefix_user_text_and_date() {
        let c = open_in_memory().unwrap();
        c.execute("INSERT INTO users (name, role, pin_hash) VALUES ('Karim', 'manager', 'x'), ('Salma', 'cashier', 'x')", []).unwrap();
        log(&c, Some(1), "order.voided", Some("order"), Some("12"), Some(json!({"reason": "erreur client"}))).unwrap();
        log(&c, Some(2), "cash.opened", None, None, None).unwrap();
        log(&c, Some(2), "order.refunded", Some("order"), Some("13"), None).unwrap();
        log(&c, Some(1), "stock.waste", None, None, Some(json!({"reason": "moisi"}))).unwrap();
        c.execute("UPDATE audit_log SET at='2026-11-01T10:00:00.000Z' WHERE action='stock.waste'", []).unwrap();
        let actions = |f: Filter| -> Vec<String> { list(&c, &f).unwrap().into_iter().map(|e| e.action).collect() };
        assert_eq!(actions(Filter::default()).len(), 4);
        assert_eq!(actions(Filter { prefixes: vec!["order.".into()], ..Default::default() }), vec!["order.refunded", "order.voided"]);
        assert_eq!(actions(Filter { prefixes: vec!["order.".into(), "cash.".into()], user_id: Some(2), ..Default::default() }), vec!["order.refunded", "cash.opened"]);
        assert_eq!(actions(Filter { q: Some("ERREUR".into()), ..Default::default() }), vec!["order.voided"]);
        assert_eq!(actions(Filter { q: Some("salma".into()), ..Default::default() }).len(), 2, "searches the user's name too");
        assert_eq!(actions(Filter { q: Some("%".into()), ..Default::default() }).len(), 0, "no wildcards");
        assert_eq!(actions(Filter { prefixes: vec!["%".into()], ..Default::default() }).len(), 0);
        assert_eq!(actions(Filter { to: Some("2026-11-02T00:00:00.000Z".into()), ..Default::default() }), vec!["stock.waste"]);
        assert_eq!(actions(Filter { limit: Some(1), offset: Some(1), ..Default::default() }).len(), 1);
        assert!(list(&c, &Filter { from: Some("yesterday".into()), ..Default::default() }).is_err());
    }
}
