use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use statefulmemory_core::error::{Error, Result};

pub const JOB_PENDING: &str = "pending";
pub const JOB_RUNNING: &str = "running";
pub const JOB_COMPLETED: &str = "completed";
pub const JOB_DEAD: &str = "dead";

#[derive(Debug, Clone)]
pub struct NewJob {
    pub kind: String,
    pub project_name: String,
    pub observation_id: Option<i64>,
    pub dedupe_key: Option<String>,
    pub payload: String,
    pub max_attempts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub project_name: String,
    pub observation_id: Option<i64>,
    pub dedupe_key: Option<String>,
    pub payload: String,
    pub status: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub available_at: String,
    pub locked_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Dead,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => JOB_PENDING,
            Self::Running => JOB_RUNNING,
            Self::Completed => JOB_COMPLETED,
            Self::Dead => JOB_DEAD,
        }
    }
}

pub fn enqueue(conn: &Connection, job: &NewJob) -> Result<i64> {
    if job.kind.trim().is_empty() {
        return Err(Error::invalid("job kind is required"));
    }
    if job.project_name.trim().is_empty() {
        return Err(Error::invalid("job project_name is required"));
    }
    serde_json::from_str::<serde_json::Value>(&job.payload)?;
    let max_attempts = job.max_attempts.max(1);
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO jobs \
             (kind, project_name, observation_id, dedupe_key, payload, max_attempts) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.kind,
                job.project_name,
                job.observation_id,
                job.dedupe_key,
                job.payload,
                max_attempts,
            ],
        )
        .map_err(|e| Error::internal(format!("enqueue job: {e}")))?;
    if changed > 0 {
        return Ok(conn.last_insert_rowid());
    }
    if let Some(dedupe_key) = &job.dedupe_key {
        return conn
            .query_row(
                "SELECT id FROM jobs \
                 WHERE kind = ?1 AND dedupe_key = ?2 \
                   AND status IN ('pending', 'running') \
                 ORDER BY id LIMIT 1",
                params![job.kind, dedupe_key],
                |row| row.get(0),
            )
            .map_err(|e| Error::internal(format!("find deduped job: {e}")));
    }
    Err(Error::internal("job insert was ignored without dedupe key"))
}

pub fn claim(conn: &Connection, kind: &str) -> Result<Option<Job>> {
    let selected = conn
        .query_row(
            "SELECT id, kind, project_name, observation_id, dedupe_key, payload, status, \
                    attempts, max_attempts, available_at, locked_at, last_error, \
                    created_at, updated_at \
             FROM jobs \
             WHERE kind = ?1 AND status = 'pending' \
               AND available_at <= datetime('now') \
             ORDER BY id LIMIT 1",
            params![kind],
            job_from_row,
        )
        .optional()
        .map_err(|e| Error::internal(format!("select claimable job: {e}")))?;
    let Some(mut job) = selected else {
        return Ok(None);
    };
    let changed = conn
        .execute(
            "UPDATE jobs \
             SET status = 'running', attempts = attempts + 1, locked_at = datetime('now'), \
                 updated_at = datetime('now') \
             WHERE id = ?1 AND status = 'pending'",
            params![job.id],
        )
        .map_err(|e| Error::internal(format!("mark job running: {e}")))?;
    if changed == 0 {
        return Ok(None);
    }
    job.status = JOB_RUNNING.to_string();
    job.attempts += 1;
    job.locked_at = Some(chrono::Utc::now().to_rfc3339());
    job.updated_at = job.locked_at.clone().unwrap_or_default();
    Ok(Some(job))
}

pub fn complete(conn: &Connection, id: i64) -> Result<bool> {
    let changed = conn
        .execute(
            "UPDATE jobs \
             SET status = 'completed', locked_at = NULL, last_error = NULL, \
                 updated_at = datetime('now') \
             WHERE id = ?1 AND status = 'running'",
            params![id],
        )
        .map_err(|e| Error::internal(format!("complete job: {e}")))?;
    Ok(changed > 0)
}

pub fn fail(conn: &Connection, id: i64, error: &str, retry_delay_secs: i64) -> Result<JobStatus> {
    let row = conn
        .query_row(
            "SELECT status, attempts, max_attempts FROM jobs WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| Error::internal(format!("read failed job: {e}")))?
        .ok_or_else(|| Error::not_found(format!("job {id}")))?;
    if row.0 != JOB_RUNNING {
        return Err(Error::FailedPrecondition(format!(
            "job {id} is {}, not running",
            row.0
        )));
    }
    let status = if row.1 >= row.2 {
        JOB_DEAD
    } else {
        JOB_PENDING
    };
    let delay = retry_delay_secs.max(0);
    conn.execute(
        "UPDATE jobs \
         SET status = ?1, available_at = datetime('now', ?2), locked_at = NULL, \
             last_error = ?3, updated_at = datetime('now') \
         WHERE id = ?4",
        params![status, format!("+{delay} seconds"), error, id],
    )
    .map_err(|e| Error::internal(format!("fail job: {e}")))?;
    Ok(match status {
        JOB_DEAD => JobStatus::Dead,
        _ => JobStatus::Pending,
    })
}

pub fn pending_count(conn: &Connection, kind: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE kind = ?1 AND status = 'pending'",
        params![kind],
        |row| row.get(0),
    )
    .map_err(|e| Error::internal(format!("count pending jobs: {e}")))
}

pub fn status_counts(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn
        .prepare("SELECT status, COUNT(*) FROM jobs GROUP BY status ORDER BY status")
        .map_err(|e| Error::internal(format!("prepare job status counts: {e}")))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| Error::internal(format!("query job status counts: {e}")))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| Error::internal(format!("read job status counts: {e}")))?);
    }
    Ok(out)
}

fn job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        kind: row.get(1)?,
        project_name: row.get(2)?,
        observation_id: row.get(3)?,
        dedupe_key: row.get(4)?,
        payload: row.get(5)?,
        status: row.get(6)?,
        attempts: row.get(7)?,
        max_attempts: row.get(8)?,
        available_at: row.get(9)?,
        locked_at: row.get(10)?,
        last_error: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_write;
    use tempfile::TempDir;

    fn job(kind: &str, dedupe: Option<&str>) -> NewJob {
        NewJob {
            kind: kind.into(),
            project_name: "p".into(),
            observation_id: Some(7),
            dedupe_key: dedupe.map(str::to_string),
            payload: "{}".into(),
            max_attempts: 2,
        }
    }

    #[test]
    fn enqueue_claim_complete_round_trip() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("jobs.db")).unwrap();
        let id = enqueue(&conn, &job("embed", Some("embed:7"))).unwrap();
        assert!(id > 0);
        let claimed = claim(&conn, "embed").unwrap().unwrap();
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.status, JOB_RUNNING);
        assert_eq!(claimed.attempts, 1);
        assert!(complete(&conn, id).unwrap());
        assert_eq!(pending_count(&conn, "embed").unwrap(), 0);
    }

    #[test]
    fn enqueue_deduplicates_active_job() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("jobs.db")).unwrap();
        let first = enqueue(&conn, &job("extract", Some("extract:7"))).unwrap();
        let second = enqueue(&conn, &job("extract", Some("extract:7"))).unwrap();
        assert_eq!(first, second);
        assert_eq!(pending_count(&conn, "extract").unwrap(), 1);
    }

    #[test]
    fn failed_job_retries_then_dead_letters() {
        let dir = TempDir::new().unwrap();
        let conn = open_write(&dir.path().join("jobs.db")).unwrap();
        let mut input = job("extract", None);
        input.max_attempts = 2;
        let id = enqueue(&conn, &input).unwrap();
        let first = claim(&conn, "extract").unwrap().unwrap();
        assert_eq!(first.id, id);
        assert_eq!(fail(&conn, id, "temporary", 0).unwrap(), JobStatus::Pending);
        let second = claim(&conn, "extract").unwrap().unwrap();
        assert_eq!(second.attempts, 2);
        assert_eq!(fail(&conn, id, "permanent", 0).unwrap(), JobStatus::Dead);
        assert!(claim(&conn, "extract").unwrap().is_none());
    }
}
