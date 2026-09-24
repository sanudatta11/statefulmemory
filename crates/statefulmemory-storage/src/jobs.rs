use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};

use statefulmemory_core::error::{Error, Result};

pub const JOB_PENDING: &str = "pending";
pub const JOB_RUNNING: &str = "running";
pub const JOB_COMPLETED: &str = "completed";
pub const JOB_DEAD: &str = "dead";
pub const DEFAULT_LIST_LIMIT: usize = 50;
pub const MAX_LIST_LIMIT: usize = 200;
pub const MAX_JOB_LIST_LIMIT: usize = MAX_LIST_LIMIT;
pub const MAX_STATUS_ROWS: usize = 10;

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

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            JOB_PENDING => Some(Self::Pending),
            JOB_RUNNING => Some(Self::Running),
            JOB_COMPLETED => Some(Self::Completed),
            JOB_DEAD => Some(Self::Dead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobStatusCounts {
    pub pending: i64,
    pub running: i64,
    pub completed: i64,
    pub dead: i64,
}

pub type JobCounts = JobStatusCounts;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobQueueStats {
    pub total: i64,
    pub by_status: JobStatusCounts,
    pub oldest_pending_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobFilter {
    pub status: Option<JobStatus>,
    pub kind: Option<String>,
    pub project_name: Option<String>,
    pub limit: usize,
    pub offset: usize,
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

pub fn status_summary(conn: &Connection) -> Result<JobStatusCounts> {
    let mut counts = JobStatusCounts::default();
    let mut stmt = conn
        .prepare("SELECT status, COUNT(*) FROM jobs GROUP BY status")
        .map_err(|e| Error::internal(format!("prepare job status summary: {e}")))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .map_err(|e| Error::internal(format!("query job status summary: {e}")))?;
    for row in rows {
        let (status, count) = row
            .map_err(|e| Error::internal(format!("read job status summary: {e}")))?;
        if let Some(status) = JobStatus::parse(&status) {
            match status {
                JobStatus::Pending => counts.pending = count,
                JobStatus::Running => counts.running = count,
                JobStatus::Completed => counts.completed = count,
                JobStatus::Dead => counts.dead = count,
            }
        }
    }
    Ok(counts)
}

pub fn counts(conn: &Connection) -> Result<JobStatusCounts> {
    status_summary(conn)
}

pub fn get(conn: &Connection, id: i64) -> Result<Job> {
    conn.query_row(
        "SELECT id, kind, project_name, observation_id, dedupe_key, payload, status, \
                attempts, max_attempts, available_at, locked_at, last_error, \
                created_at, updated_at \
         FROM jobs WHERE id = ?1",
        params![id],
        job_from_row,
    )
    .map_err(|e| Error::internal(format!("get job: {e}")))
}

pub fn status(conn: &Connection, id: i64) -> Result<JobStatus> {
    let value: String = conn
        .query_row("SELECT status FROM jobs WHERE id = ?1", params![id], |row| row.get(0))
        .optional()
        .map_err(|e| Error::internal(format!("get job status: {e}")))?
        .ok_or_else(|| Error::not_found(format!("job {id}")))?;
    JobStatus::parse(&value).ok_or_else(|| Error::internal(format!("invalid job status: {value}")))
}

pub fn count(conn: &Connection) -> Result<i64> {
    count_filtered(conn, &JobFilter::default())
}

pub fn count_filtered(conn: &Connection, filter: &JobFilter) -> Result<i64> {
    let (clause, values) = filter_sql(filter, false);
    let sql = format!("SELECT COUNT(*) FROM jobs {clause}");
    let bound: Vec<&dyn ToSql> = values.iter().map(|value| value as &dyn ToSql).collect();
    if bound.is_empty() {
        return conn
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|e| Error::internal(format!("count jobs: {e}")));
    }
    conn.query_row(&sql, params_from_iter(bound), |row| row.get(0))
        .map_err(|e| Error::internal(format!("count jobs: {e}")))
}

pub fn count_for_project(conn: &Connection, project_name: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE project_name = ?1",
        params![project_name],
        |row| row.get(0),
    )
    .map_err(|e| Error::internal(format!("count project jobs: {e}")))
}

pub fn list_filtered(conn: &Connection, filter: &JobFilter) -> Result<Vec<Job>> {
    let (clause, values) = filter_sql(filter, true);
    let sql = format!(
        "SELECT id, kind, project_name, observation_id, dedupe_key, payload, status, \
                attempts, max_attempts, available_at, locked_at, last_error, \
                created_at, updated_at \
         FROM jobs {clause} ORDER BY id DESC"
    );
    let bound: Vec<&dyn ToSql> = values.iter().map(|value| value as &dyn ToSql).collect();
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| Error::internal(format!("prepare list jobs: {e}")))?;
    let rows = if bound.is_empty() {
        stmt.query_map([], job_from_row)
    } else {
        stmt.query_map(params_from_iter(bound), job_from_row)
    }
    .map_err(|e| Error::internal(format!("query list jobs: {e}")))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| Error::internal(format!("read list job: {e}")))?);
    }
    Ok(out)
}

pub fn list(conn: &Connection, limit: usize) -> Result<Vec<Job>> {
    list_filtered(
        conn,
        &JobFilter {
            limit,
            ..JobFilter::default()
        },
    )
}

pub fn list_by_status(conn: &Connection, job_status: JobStatus, limit: usize) -> Result<Vec<Job>> {
    list_filtered(
        conn,
        &JobFilter {
            status: Some(job_status),
            limit,
            ..JobFilter::default()
        },
    )
}

pub fn list_for_kind(conn: &Connection, kind: &str, limit: usize) -> Result<Vec<Job>> {
    list_filtered(
        conn,
        &JobFilter {
            kind: Some(kind.to_string()),
            limit,
            ..JobFilter::default()
        },
    )
}

pub fn list_for_project(conn: &Connection, project_name: &str, limit: usize) -> Result<Vec<Job>> {
    list_filtered(
        conn,
        &JobFilter {
            project_name: Some(project_name.to_string()),
            limit,
            ..JobFilter::default()
        },
    )
}

pub fn queue_stats(conn: &Connection) -> Result<JobQueueStats> {
    let by_status = status_summary(conn)?;
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
        .map_err(|e| Error::internal(format!("count all jobs: {e}")))?;
    let oldest_pending_at: Option<String> = conn
        .query_row(
            "SELECT MIN(created_at) FROM jobs WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| Error::internal(format!("find oldest pending job: {e}")))?;
    Ok(JobQueueStats {
        total,
        by_status,
        oldest_pending_at,
    })
}

fn filter_sql(filter: &JobFilter, bounded: bool) -> (String, Vec<SqlValue>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(job_status) = filter.status {
        clauses.push("status = ?".to_string());
        values.push(SqlValue::Text(job_status.as_str().to_string()));
    }
    if let Some(kind) = &filter.kind {
        clauses.push("kind = ?".to_string());
        values.push(SqlValue::Text(kind.clone()));
    }
    if let Some(project_name) = &filter.project_name {
        clauses.push("project_name = ?".to_string());
        values.push(SqlValue::Text(project_name.clone()));
    }
    let clause = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    if bounded {
        let limit = filter.limit.clamp(1, MAX_LIST_LIMIT);
        values.push(SqlValue::Integer(limit as i64));
        let offset = filter.offset.min(i64::MAX as usize) as i64;
        values.push(SqlValue::Integer(offset));
        (
            format!("{clause} LIMIT ? OFFSET ?"),
            values,
        )
    } else {
        (clause, values)
    }
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
