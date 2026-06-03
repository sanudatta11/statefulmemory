//! Prompts read-side helpers (writes go through write::WriteRequest).
//!
//! Spec sections: FR6.4 (`SearchPrompts`, `RecentPrompts`).

use rusqlite::{params, Connection};

use memlayer_core::error::{Error, Result};

use crate::models::Prompt;

const SELECT_COLS: &str = "id, sync_id, session_id, content, created_at";

pub fn search(conn: &Connection, query: &str, limit: i32) -> Result<Vec<Prompt>> {
    let limit = limit.clamp(1, 50);
    let safe = sanitize_fts5_query(query);
    let sql = format!(
        "SELECT {SELECT_COLS} FROM user_prompts
          WHERE id IN (
              SELECT rowid FROM prompts_fts WHERE prompts_fts MATCH ?1
              ORDER BY bm25(prompts_fts) ASC LIMIT ?2
          )
          ORDER BY id DESC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare: {e}")))?;
    let rows = stmt
        .query_map(params![safe, limit], Prompt::from_row)
        .map_err(|e| Error::internal(format!("query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("rows: {e}")))?);
    }
    Ok(out)
}

pub fn recent(conn: &Connection, limit: i32) -> Result<Vec<Prompt>> {
    let limit = limit.clamp(1, 50);
    let sql = format!(
        "SELECT {SELECT_COLS} FROM user_prompts ORDER BY created_at DESC LIMIT ?1"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare: {e}")))?;
    let rows = stmt
        .query_map(params![limit], Prompt::from_row)
        .map_err(|e| Error::internal(format!("query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| Error::internal(format!("rows: {e}")))?);
    }
    Ok(out)
}

fn sanitize_fts5_query(q: &str) -> String {
    let escaped = q.replace('"', "\"\"");
    format!("\"{escaped}\"")
}
