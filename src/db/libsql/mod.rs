//! libSQL/Turso backend for the Database trait.
//!
//! Provides an embedded SQLite-compatible database using Turso's libSQL fork.
//! Supports three modes:
//! - Local embedded (file-based, no server needed)
//! - Turso cloud with embedded replica (sync to cloud)
//! - In-memory (for testing)

mod ast_graph;
mod conversations;
mod jobs;
mod routines;
mod sandbox;
mod settings;
mod tool_failures;
mod workspace;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use libsql::{Connection, Database as LibSqlDatabase, params};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::agent::routine::{
    NotifyConfig, Routine, RoutineAction, RoutineGuardrails, RoutineRun, RunStatus, Trigger,
};
use crate::agent::{
    ExecutionAttempt, Goal, GoalStatus, Plan, PlanStatus, PlanStep, PlanStepStatus, PolicyDecision,
};
use crate::context::JobState;
use crate::db::{AutonomyExecutionStore, Database, GoalStore, PlanStore};
use crate::error::DatabaseError;
use crate::workspace::MemoryDocument;

use crate::db::libsql_migrations;

/// Explicit column list for routines table (matches positional access in `row_to_routine_libsql`).
pub(crate) const ROUTINE_COLUMNS: &str = "\
    id, name, description, user_id, enabled, \
    trigger_type, trigger_config, action_type, action_config, \
    cooldown_secs, max_concurrent, dedup_window_secs, \
    notify_channel, notify_user, notify_on_success, notify_on_failure, notify_on_attention, \
    state, last_run_at, next_fire_at, run_count, consecutive_failures, \
    created_at, updated_at";

/// Explicit column list for routine_runs table (matches positional access in `row_to_routine_run_libsql`).
pub(crate) const ROUTINE_RUN_COLUMNS: &str = "\
    id, routine_id, trigger_type, trigger_detail, started_at, \
    status, completed_at, result_summary, tokens_used, job_id, created_at";

/// libSQL/Turso database backend.
///
/// Stores the `Database` handle in an `Arc` so that the same underlying
/// database can be shared with stores (SecretsStore, WasmToolStore) that
/// create their own connections per-operation.
pub struct LibSqlBackend {
    db: Arc<LibSqlDatabase>,
}

impl LibSqlBackend {
    /// Create a new local embedded database.
    pub async fn new_local(path: &Path) -> Result<Self, DatabaseError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DatabaseError::Pool(format!("Failed to create database directory: {}", e))
            })?;
        }

        let db = libsql::Builder::new_local(path)
            .build()
            .await
            .map_err(|e| DatabaseError::Pool(format!("Failed to open libSQL database: {}", e)))?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Create a new in-memory database (for testing).
    pub async fn new_memory() -> Result<Self, DatabaseError> {
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| {
                DatabaseError::Pool(format!("Failed to create in-memory database: {}", e))
            })?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Create with Turso cloud sync (embedded replica).
    pub async fn new_remote_replica(
        path: &Path,
        url: &str,
        auth_token: &str,
    ) -> Result<Self, DatabaseError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DatabaseError::Pool(format!("Failed to create database directory: {}", e))
            })?;
        }

        let db = libsql::Builder::new_remote_replica(path, url.to_string(), auth_token.to_string())
            .build()
            .await
            .map_err(|e| DatabaseError::Pool(format!("Failed to open remote replica: {}", e)))?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Get a shared reference to the underlying database handle.
    ///
    /// Use this to pass the database to stores (SecretsStore, WasmToolStore)
    /// that need to create their own connections per-operation.
    pub fn shared_db(&self) -> Arc<LibSqlDatabase> {
        Arc::clone(&self.db)
    }

    /// Create a new connection to the database.
    ///
    /// Sets `PRAGMA busy_timeout = 5000` on every connection so concurrent
    /// writers wait up to 5 seconds instead of failing instantly with
    /// "database is locked".
    pub async fn connect(&self) -> Result<Connection, DatabaseError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DatabaseError::Pool(format!("Failed to create connection: {}", e)))?;
        conn.query("PRAGMA busy_timeout = 5000", ())
            .await
            .map_err(|e| DatabaseError::Pool(format!("Failed to set busy_timeout: {}", e)))?;
        Ok(conn)
    }
}

// ==================== Helper functions ====================

/// Parse an ISO-8601 timestamp string from SQLite into DateTime<Utc>.
///
/// Tries multiple formats in order:
/// 1. RFC 3339 with timezone (e.g. `2024-01-15T10:30:00.123Z`)
/// 2. Naive datetime with fractional seconds (e.g. `2024-01-15 10:30:00.123`)
/// 3. Naive datetime without fractional seconds (e.g. `2024-01-15 10:30:00`)
///
/// Returns an error if none of the formats match.
pub(crate) fn parse_timestamp(s: &str) -> Result<DateTime<Utc>, String> {
    // RFC 3339 (our canonical write format)
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Naive with fractional seconds (legacy or SQLite datetime() output)
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(ndt.and_utc());
    }
    // Naive without fractional seconds (legacy format)
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(ndt.and_utc());
    }
    Err(format!("unparseable timestamp: {:?}", s))
}

/// Format a DateTime<Utc> for SQLite storage (RFC 3339 with millisecond precision).
pub(crate) fn fmt_ts(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Format an optional DateTime<Utc>.
pub(crate) fn fmt_opt_ts(dt: &Option<DateTime<Utc>>) -> libsql::Value {
    match dt {
        Some(dt) => libsql::Value::Text(fmt_ts(dt)),
        None => libsql::Value::Null,
    }
}

pub(crate) fn parse_job_state(s: &str) -> JobState {
    match s {
        "pending" => JobState::Pending,
        "in_progress" => JobState::InProgress,
        "completed" => JobState::Completed,
        "submitted" => JobState::Submitted,
        "accepted" => JobState::Accepted,
        "failed" => JobState::Failed,
        "stuck" => JobState::Stuck,
        "cancelled" => JobState::Cancelled,
        _ => JobState::Pending,
    }
}

/// Extract a text column from a libsql Row, returning empty string for NULL.
pub(crate) fn get_text(row: &libsql::Row, idx: i32) -> String {
    row.get::<String>(idx).unwrap_or_default()
}

/// Extract an optional text column.
/// Returns None for SQL NULL, preserves empty strings as Some("").
pub(crate) fn get_opt_text(row: &libsql::Row, idx: i32) -> Option<String> {
    row.get::<String>(idx).ok()
}

/// Convert an `Option<&str>` to a `libsql::Value` (Text or Null).
/// Use this instead of `.unwrap_or("")` to preserve NULL semantics.
pub(crate) fn opt_text(s: Option<&str>) -> libsql::Value {
    match s {
        Some(s) => libsql::Value::Text(s.to_string()),
        None => libsql::Value::Null,
    }
}

/// Convert an `Option<String>` to a `libsql::Value` (Text or Null).
pub(crate) fn opt_text_owned(s: Option<String>) -> libsql::Value {
    match s {
        Some(s) => libsql::Value::Text(s),
        None => libsql::Value::Null,
    }
}

/// Extract an i64 column, defaulting to 0.
pub(crate) fn get_i64(row: &libsql::Row, idx: i32) -> i64 {
    row.get::<i64>(idx).unwrap_or(0)
}

/// Extract an optional bool from an integer column.
pub(crate) fn get_opt_bool(row: &libsql::Row, idx: i32) -> Option<bool> {
    row.get::<i64>(idx).ok().map(|v| v != 0)
}

/// Parse a Decimal from a text column.
pub(crate) fn get_decimal(row: &libsql::Row, idx: i32) -> Decimal {
    row.get::<String>(idx)
        .ok()
        .and_then(|s| s.parse::<Decimal>().ok())
        .unwrap_or_default()
}

/// Parse an optional Decimal from a text column.
pub(crate) fn get_opt_decimal(row: &libsql::Row, idx: i32) -> Option<Decimal> {
    row.get::<String>(idx)
        .ok()
        .and_then(|s| s.parse::<Decimal>().ok())
}

/// Parse a JSON value from a text column.
pub(crate) fn get_json(row: &libsql::Row, idx: i32) -> serde_json::Value {
    row.get::<String>(idx)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Parse a timestamp from a text column.
///
/// If the column is NULL or the value cannot be parsed, logs a warning and
/// returns the Unix epoch (1970-01-01T00:00:00Z) so the error is detectable
/// rather than silently replaced by the current time.
pub(crate) fn get_ts(row: &libsql::Row, idx: i32) -> DateTime<Utc> {
    match row.get::<String>(idx) {
        Ok(s) => match parse_timestamp(&s) {
            Ok(dt) => dt,
            Err(e) => {
                tracing::warn!("Timestamp parse failure at column {}: {}", idx, e);
                DateTime::UNIX_EPOCH
            }
        },
        Err(_) => DateTime::UNIX_EPOCH,
    }
}

/// Parse an optional timestamp from a text column.
///
/// Returns None if the column is NULL. Logs a warning and returns None if the
/// value is present but cannot be parsed.
pub(crate) fn get_opt_ts(row: &libsql::Row, idx: i32) -> Option<DateTime<Utc>> {
    match row.get::<String>(idx) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => match parse_timestamp(&s) {
            Ok(dt) => Some(dt),
            Err(e) => {
                tracing::warn!("Timestamp parse failure at column {}: {}", idx, e);
                None
            }
        },
        Err(_) => None,
    }
}

/// Convert a serde-backed enum (with `snake_case` serde names) to DB string.
fn enum_to_snake_case<T: serde::Serialize>(value: &T) -> Result<String, DatabaseError> {
    match serde_json::to_value(value).map_err(|e| DatabaseError::Serialization(e.to_string()))? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(DatabaseError::Serialization(format!(
            "expected enum string, got {}",
            other
        ))),
    }
}

/// Parse a serde-backed enum from DB snake_case string.
fn enum_from_snake_case<T: serde::de::DeserializeOwned>(
    s: &str,
    kind: &str,
) -> Result<T, DatabaseError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| DatabaseError::Serialization(format!("invalid {} '{}': {}", kind, s, e)))
}

/// Convert optional JSON to libSQL text/NULL while preserving NULL semantics.
fn opt_json_text(value: Option<&serde_json::Value>) -> Result<libsql::Value, DatabaseError> {
    match value {
        Some(v) => Ok(libsql::Value::Text(
            serde_json::to_string(v).map_err(|e| DatabaseError::Serialization(e.to_string()))?,
        )),
        None => Ok(libsql::Value::Null),
    }
}

/// Parse an optional JSON value from a text column. Returns None for SQL NULL.
fn get_opt_json(row: &libsql::Row, idx: i32) -> Option<serde_json::Value> {
    match row.get::<String>(idx) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("JSON parse failure at column {}: {}", idx, e);
                None
            }
        },
        Err(_) => None,
    }
}

fn get_opt_f64(row: &libsql::Row, idx: i32) -> Option<f64> {
    row.get::<f64>(idx).ok()
}

fn get_opt_f32(row: &libsql::Row, idx: i32) -> Option<f32> {
    row.get::<f64>(idx).ok().map(|v| v as f32)
}

fn get_opt_uuid(row: &libsql::Row, idx: i32) -> Option<Uuid> {
    get_opt_text(row, idx).and_then(|s| s.parse().ok())
}

fn get_json_string_array(row: &libsql::Row, idx: i32) -> Vec<String> {
    match row.get::<String>(idx) {
        Ok(s) if s.is_empty() => Vec::new(),
        Ok(s) => match serde_json::from_str::<Vec<String>>(&s) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("JSON string array parse failure at column {}: {}", idx, e);
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

async fn ensure_agent_jobs_autonomy_link_columns(conn: &Connection) -> Result<(), DatabaseError> {
    for column in [
        "autonomy_goal_id",
        "autonomy_plan_id",
        "autonomy_plan_step_id",
    ] {
        let sql = format!("ALTER TABLE agent_jobs ADD COLUMN {} TEXT", column);
        if let Err(e) = conn.execute(&sql, ()).await {
            let msg = e.to_string().to_lowercase();
            if msg.contains("duplicate column name") || msg.contains("no such table") {
                continue;
            }
            return Err(DatabaseError::Migration(format!(
                "Failed to add agent_jobs.{}: {}",
                column, e
            )));
        }
    }

    Ok(())
}

#[async_trait]
impl Database for LibSqlBackend {
    async fn run_migrations(&self) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        // WAL mode persists in the database file: all future connections benefit.
        // Readers no longer block writers and vice versa.
        conn.query("PRAGMA journal_mode=WAL", ())
            .await
            .map_err(|e| DatabaseError::Migration(format!("Failed to enable WAL mode: {}", e)))?;
        // Backward-compatibility for existing databases: SQLite doesn't support
        // `ADD COLUMN IF NOT EXISTS`, so patch missing columns before the
        // consolidated schema runs and creates indexes that depend on them.
        ensure_agent_jobs_autonomy_link_columns(&conn).await?;
        conn.execute_batch(libsql_migrations::SCHEMA)
            .await
            .map_err(|e| DatabaseError::Migration(format!("libSQL migration failed: {}", e)))?;
        Ok(())
    }
}

#[async_trait]
impl GoalStore for LibSqlBackend {
    async fn create_goal(&self, goal: &Goal) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&goal.status)?;
        let risk_class = enum_to_snake_case(&goal.risk_class)?;
        let source = enum_to_snake_case(&goal.source)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_goals (
                id, owner_user_id, channel, thread_id, title, intent, priority,
                status, risk_class, acceptance_criteria, constraints, source,
                created_at, updated_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            "#,
            params![
                goal.id.to_string(),
                goal.owner_user_id.as_str(),
                opt_text(goal.channel.as_deref()),
                opt_text_owned(goal.thread_id.map(|id| id.to_string())),
                goal.title.as_str(),
                goal.intent.as_str(),
                goal.priority as i64,
                status,
                risk_class,
                goal.acceptance_criteria.to_string(),
                goal.constraints.to_string(),
                source,
                fmt_ts(&goal.created_at),
                fmt_ts(&goal.updated_at),
                fmt_opt_ts(&goal.completed_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_goal(&self, id: Uuid) -> Result<Option<Goal>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, channel, thread_id, title, intent, priority,
                    status, risk_class, acceptance_criteria, constraints, source,
                    created_at, updated_at, completed_at
                FROM autonomy_goals
                WHERE id = ?1
                "#,
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_goal_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_goals(&self) -> Result<Vec<Goal>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, channel, thread_id, title, intent, priority,
                    status, risk_class, acceptance_criteria, constraints, source,
                    created_at, updated_at, completed_at
                FROM autonomy_goals
                ORDER BY updated_at DESC, created_at DESC
                "#,
                (),
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut goals = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            goals.push(row_to_goal_libsql(&row)?);
        }
        Ok(goals)
    }

    async fn update_goal_status(&self, id: Uuid, status: GoalStatus) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let now = fmt_ts(&Utc::now());
        let status_str = enum_to_snake_case(&status)?;
        let is_completed = matches!(status, GoalStatus::Completed) as i64;

        conn.execute(
            r#"
            UPDATE autonomy_goals
            SET
                status = ?2,
                updated_at = ?3,
                completed_at = CASE
                    WHEN ?4 = 1 THEN COALESCE(completed_at, ?3)
                    ELSE NULL
                END
            WHERE id = ?1
            "#,
            params![id.to_string(), status_str, now.as_str(), is_completed],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl PlanStore for LibSqlBackend {
    async fn create_plan(&self, plan: &Plan) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&plan.status)?;
        let planner_kind = enum_to_snake_case(&plan.planner_kind)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_plans (
                id, goal_id, revision, status, planner_kind, source_action_plan,
                assumptions, confidence, estimated_cost, estimated_time_secs, summary,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
            params![
                plan.id.to_string(),
                plan.goal_id.to_string(),
                plan.revision as i64,
                status,
                planner_kind,
                opt_json_text(plan.source_action_plan.as_ref())?,
                plan.assumptions.to_string(),
                plan.confidence,
                plan.estimated_cost,
                plan.estimated_time_secs.map(|v| v as i64),
                opt_text(plan.summary.as_deref()),
                fmt_ts(&plan.created_at),
                fmt_ts(&plan.updated_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn list_plans_for_goal(&self, goal_id: Uuid) -> Result<Vec<Plan>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, revision, status, planner_kind, source_action_plan,
                    assumptions, confidence, estimated_cost, estimated_time_secs, summary,
                    created_at, updated_at
                FROM autonomy_plans
                WHERE goal_id = ?1
                ORDER BY revision DESC, created_at DESC
                "#,
                params![goal_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut plans = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            plans.push(row_to_plan_libsql(&row)?);
        }
        Ok(plans)
    }

    async fn get_plan(&self, id: Uuid) -> Result<Option<Plan>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, revision, status, planner_kind, source_action_plan,
                    assumptions, confidence, estimated_cost, estimated_time_secs, summary,
                    created_at, updated_at
                FROM autonomy_plans
                WHERE id = ?1
                "#,
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_plan_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn update_plan_status(&self, id: Uuid, status: PlanStatus) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let now = fmt_ts(&Utc::now());
        let status_str = enum_to_snake_case(&status)?;

        conn.execute(
            r#"
            UPDATE autonomy_plans
            SET status = ?2, updated_at = ?3
            WHERE id = ?1
            "#,
            params![id.to_string(), status_str, now.as_str()],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn create_plan_steps(&self, steps: &[PlanStep]) -> Result<(), DatabaseError> {
        if steps.is_empty() {
            return Ok(());
        }

        let conn = self.connect().await?;
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        for step in steps {
            let kind = enum_to_snake_case(&step.kind)?;
            let status = enum_to_snake_case(&step.status)?;

            if let Err(e) = conn
                .execute(
                    r#"
                    INSERT INTO autonomy_plan_steps (
                        id, plan_id, sequence_num, kind, status, title, description,
                        tool_candidates, inputs, preconditions, postconditions, "rollback",
                        policy_requirements, started_at, completed_at, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                    "#,
                    params![
                        step.id.to_string(),
                        step.plan_id.to_string(),
                        step.sequence_num as i64,
                        kind,
                        status,
                        step.title.as_str(),
                        step.description.as_str(),
                        step.tool_candidates.to_string(),
                        step.inputs.to_string(),
                        step.preconditions.to_string(),
                        step.postconditions.to_string(),
                        opt_json_text(step.rollback.as_ref())?,
                        step.policy_requirements.to_string(),
                        fmt_opt_ts(&step.started_at),
                        fmt_opt_ts(&step.completed_at),
                        fmt_ts(&step.created_at),
                        fmt_ts(&step.updated_at),
                    ],
                )
                .await
            {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(DatabaseError::Query(e.to_string()));
            }
        }

        conn.execute("COMMIT", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn replace_plan_steps_for_plan(
        &self,
        plan_id: Uuid,
        steps: &[PlanStep],
    ) -> Result<(), DatabaseError> {
        if steps.iter().any(|s| s.plan_id != plan_id) {
            return Err(DatabaseError::Query(
                "replace_plan_steps_for_plan received step with mismatched plan_id".to_string(),
            ));
        }

        let conn = self.connect().await?;
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        if let Err(e) = conn
            .execute(
                "DELETE FROM autonomy_plan_steps WHERE plan_id = ?1",
                params![plan_id.to_string()],
            )
            .await
        {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(DatabaseError::Query(e.to_string()));
        }

        for step in steps {
            let kind = enum_to_snake_case(&step.kind)?;
            let status = enum_to_snake_case(&step.status)?;

            if let Err(e) = conn
                .execute(
                    r#"
                    INSERT INTO autonomy_plan_steps (
                        id, plan_id, sequence_num, kind, status, title, description,
                        tool_candidates, inputs, preconditions, postconditions, "rollback",
                        policy_requirements, started_at, completed_at, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                    "#,
                    params![
                        step.id.to_string(),
                        step.plan_id.to_string(),
                        step.sequence_num as i64,
                        kind,
                        status,
                        step.title.as_str(),
                        step.description.as_str(),
                        step.tool_candidates.to_string(),
                        step.inputs.to_string(),
                        step.preconditions.to_string(),
                        step.postconditions.to_string(),
                        opt_json_text(step.rollback.as_ref())?,
                        step.policy_requirements.to_string(),
                        fmt_opt_ts(&step.started_at),
                        fmt_opt_ts(&step.completed_at),
                        fmt_ts(&step.created_at),
                        fmt_ts(&step.updated_at),
                    ],
                )
                .await
            {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(DatabaseError::Query(e.to_string()));
            }
        }

        conn.execute("COMMIT", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_plan_step(&self, id: Uuid) -> Result<Option<PlanStep>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, plan_id, sequence_num, kind, status, title, description,
                    tool_candidates, inputs, preconditions, postconditions, "rollback",
                    policy_requirements, started_at, completed_at, created_at, updated_at
                FROM autonomy_plan_steps
                WHERE id = ?1
                "#,
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_plan_step_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_plan_steps_for_plan(
        &self,
        plan_id: Uuid,
    ) -> Result<Vec<PlanStep>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, plan_id, sequence_num, kind, status, title, description,
                    tool_candidates, inputs, preconditions, postconditions, "rollback",
                    policy_requirements, started_at, completed_at, created_at, updated_at
                FROM autonomy_plan_steps
                WHERE plan_id = ?1
                ORDER BY sequence_num ASC, created_at ASC, id ASC
                "#,
                params![plan_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut steps = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            steps.push(row_to_plan_step_libsql(&row)?);
        }
        Ok(steps)
    }

    async fn update_plan_step_status(
        &self,
        id: Uuid,
        status: PlanStepStatus,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let now = fmt_ts(&Utc::now());
        let status_str = enum_to_snake_case(&status)?;
        let set_started = matches!(status, PlanStepStatus::Running) as i64;
        let set_completed = matches!(
            status,
            PlanStepStatus::Succeeded | PlanStepStatus::Failed | PlanStepStatus::Skipped
        ) as i64;

        conn.execute(
            r#"
            UPDATE autonomy_plan_steps
            SET
                status = ?2,
                updated_at = ?3,
                started_at = CASE WHEN ?4 = 1 THEN COALESCE(started_at, ?3) ELSE started_at END,
                completed_at = CASE WHEN ?5 = 1 THEN COALESCE(completed_at, ?3) ELSE completed_at END
            WHERE id = ?1
            "#,
            params![
                id.to_string(),
                status_str,
                now.as_str(),
                set_started,
                set_completed
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl AutonomyExecutionStore for LibSqlBackend {
    async fn record_execution_attempt(
        &self,
        attempt: &ExecutionAttempt,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&attempt.status)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_execution_attempts (
                id, goal_id, plan_id, plan_step_id, job_id, thread_id, user_id, channel,
                tool_name, tool_call_id, tool_args, status, failure_class, retry_count,
                started_at, finished_at, elapsed_ms, result_summary, error_preview
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
            "#,
            params![
                attempt.id.to_string(),
                opt_text_owned(attempt.goal_id.map(|id| id.to_string())),
                opt_text_owned(attempt.plan_id.map(|id| id.to_string())),
                opt_text_owned(attempt.plan_step_id.map(|id| id.to_string())),
                opt_text_owned(attempt.job_id.map(|id| id.to_string())),
                opt_text_owned(attempt.thread_id.map(|id| id.to_string())),
                attempt.user_id.as_str(),
                attempt.channel.as_str(),
                attempt.tool_name.as_str(),
                opt_text(attempt.tool_call_id.as_deref()),
                opt_json_text(attempt.tool_args.as_ref())?,
                status,
                opt_text(attempt.failure_class.as_deref()),
                attempt.retry_count as i64,
                fmt_ts(&attempt.started_at),
                fmt_opt_ts(&attempt.finished_at),
                attempt.elapsed_ms,
                opt_text(attempt.result_summary.as_deref()),
                opt_text(attempt.error_preview.as_deref()),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn update_execution_attempt(
        &self,
        attempt: &ExecutionAttempt,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&attempt.status)?;

        conn.execute(
            r#"
            UPDATE autonomy_execution_attempts
            SET
                goal_id = ?2,
                plan_id = ?3,
                plan_step_id = ?4,
                job_id = ?5,
                thread_id = ?6,
                user_id = ?7,
                channel = ?8,
                tool_name = ?9,
                tool_call_id = ?10,
                tool_args = ?11,
                status = ?12,
                failure_class = ?13,
                retry_count = ?14,
                started_at = ?15,
                finished_at = ?16,
                elapsed_ms = ?17,
                result_summary = ?18,
                error_preview = ?19
            WHERE id = ?1
            "#,
            params![
                attempt.id.to_string(),
                opt_text_owned(attempt.goal_id.map(|id| id.to_string())),
                opt_text_owned(attempt.plan_id.map(|id| id.to_string())),
                opt_text_owned(attempt.plan_step_id.map(|id| id.to_string())),
                opt_text_owned(attempt.job_id.map(|id| id.to_string())),
                opt_text_owned(attempt.thread_id.map(|id| id.to_string())),
                attempt.user_id.as_str(),
                attempt.channel.as_str(),
                attempt.tool_name.as_str(),
                opt_text(attempt.tool_call_id.as_deref()),
                opt_json_text(attempt.tool_args.as_ref())?,
                status,
                opt_text(attempt.failure_class.as_deref()),
                attempt.retry_count as i64,
                fmt_ts(&attempt.started_at),
                fmt_opt_ts(&attempt.finished_at),
                attempt.elapsed_ms,
                opt_text(attempt.result_summary.as_deref()),
                opt_text(attempt.error_preview.as_deref()),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn record_policy_decision(&self, decision: &PolicyDecision) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let decision_kind = enum_to_snake_case(&decision.decision)?;
        let reason_codes = serde_json::to_string(&decision.reason_codes)
            .map_err(|e| DatabaseError::Serialization(e.to_string()))?;

        conn.execute(
            r#"
            INSERT INTO autonomy_policy_decisions (
                id, goal_id, plan_id, plan_step_id, execution_attempt_id, user_id, channel,
                tool_name, tool_call_id, action_kind, decision, reason_codes, risk_score,
                confidence, requires_approval, auto_approved, evidence_required, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
            "#,
            params![
                decision.id.to_string(),
                opt_text_owned(decision.goal_id.map(|id| id.to_string())),
                opt_text_owned(decision.plan_id.map(|id| id.to_string())),
                opt_text_owned(decision.plan_step_id.map(|id| id.to_string())),
                opt_text_owned(decision.execution_attempt_id.map(|id| id.to_string())),
                decision.user_id.as_str(),
                decision.channel.as_str(),
                opt_text(decision.tool_name.as_deref()),
                opt_text(decision.tool_call_id.as_deref()),
                decision.action_kind.as_str(),
                decision_kind,
                reason_codes,
                decision.risk_score.map(|v| v as f64),
                decision.confidence.map(|v| v as f64),
                decision.requires_approval as i64,
                decision.auto_approved.map(|v| if v { 1_i64 } else { 0_i64 }),
                decision.evidence_required.to_string(),
                fmt_ts(&decision.created_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn list_execution_attempts_for_plan(
        &self,
        plan_id: Uuid,
    ) -> Result<Vec<ExecutionAttempt>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, plan_id, plan_step_id, job_id, thread_id, user_id, channel,
                    tool_name, tool_call_id, tool_args, status, failure_class, retry_count,
                    started_at, finished_at, elapsed_ms, result_summary, error_preview
                FROM autonomy_execution_attempts
                WHERE plan_id = ?1
                ORDER BY started_at DESC, id DESC
                "#,
                params![plan_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut attempts = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            attempts.push(row_to_execution_attempt_libsql(&row)?);
        }
        Ok(attempts)
    }

    async fn list_policy_decisions_for_goal(
        &self,
        goal_id: Uuid,
    ) -> Result<Vec<PolicyDecision>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, plan_id, plan_step_id, execution_attempt_id, user_id, channel,
                    tool_name, tool_call_id, action_kind, decision, reason_codes, risk_score,
                    confidence, requires_approval, auto_approved, evidence_required, created_at
                FROM autonomy_policy_decisions
                WHERE goal_id = ?1
                ORDER BY created_at DESC, id DESC
                "#,
                params![goal_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut decisions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            decisions.push(row_to_policy_decision_libsql(&row)?);
        }
        Ok(decisions)
    }
}

// ==================== Row conversion helpers ====================

pub(crate) fn row_to_memory_document(row: &libsql::Row) -> MemoryDocument {
    MemoryDocument {
        id: get_text(row, 0).parse().unwrap_or_default(),
        user_id: get_text(row, 1),
        agent_id: get_opt_text(row, 2).and_then(|s| s.parse().ok()),
        path: get_text(row, 3),
        content: get_text(row, 4),
        created_at: get_ts(row, 5),
        updated_at: get_ts(row, 6),
        metadata: get_json(row, 7),
    }
}

pub(crate) fn row_to_routine_libsql(row: &libsql::Row) -> Result<Routine, DatabaseError> {
    let trigger_type = get_text(row, 5);
    let trigger_config = get_json(row, 6);
    let action_type = get_text(row, 7);
    let action_config = get_json(row, 8);
    let cooldown_secs = get_i64(row, 9);
    let max_concurrent = get_i64(row, 10);
    let dedup_window_secs: Option<i64> = row.get::<i64>(11).ok();

    let trigger =
        Trigger::from_db(&trigger_type, trigger_config).map_err(DatabaseError::Serialization)?;
    let action = RoutineAction::from_db(&action_type, action_config)
        .map_err(DatabaseError::Serialization)?;

    Ok(Routine {
        id: get_text(row, 0).parse().unwrap_or_default(),
        name: get_text(row, 1),
        description: get_text(row, 2),
        user_id: get_text(row, 3),
        enabled: get_i64(row, 4) != 0,
        trigger,
        action,
        guardrails: RoutineGuardrails {
            cooldown: std::time::Duration::from_secs(cooldown_secs as u64),
            max_concurrent: max_concurrent as u32,
            dedup_window: dedup_window_secs.map(|s| std::time::Duration::from_secs(s as u64)),
        },
        notify: NotifyConfig {
            channel: get_opt_text(row, 12),
            user: get_text(row, 13),
            on_success: get_i64(row, 14) != 0,
            on_failure: get_i64(row, 15) != 0,
            on_attention: get_i64(row, 16) != 0,
        },
        state: get_json(row, 17),
        last_run_at: get_opt_ts(row, 18),
        next_fire_at: get_opt_ts(row, 19),
        run_count: get_i64(row, 20) as u64,
        consecutive_failures: get_i64(row, 21) as u32,
        created_at: get_ts(row, 22),
        updated_at: get_ts(row, 23),
    })
}

pub(crate) fn row_to_routine_run_libsql(row: &libsql::Row) -> Result<RoutineRun, DatabaseError> {
    let status_str = get_text(row, 5);
    let status: RunStatus = status_str
        .parse()
        .map_err(|e: String| DatabaseError::Serialization(e))?;

    Ok(RoutineRun {
        id: get_text(row, 0).parse().unwrap_or_default(),
        routine_id: get_text(row, 1).parse().unwrap_or_default(),
        trigger_type: get_text(row, 2),
        trigger_detail: get_opt_text(row, 3),
        started_at: get_ts(row, 4),
        completed_at: get_opt_ts(row, 6),
        status,
        result_summary: get_opt_text(row, 7),
        tokens_used: row.get::<i64>(8).ok().map(|v| v as i32),
        job_id: get_opt_text(row, 9).and_then(|s| s.parse().ok()),
        created_at: get_ts(row, 10),
    })
}

pub(crate) fn row_to_goal_libsql(row: &libsql::Row) -> Result<Goal, DatabaseError> {
    Ok(Goal {
        id: get_text(row, 0).parse().unwrap_or_default(),
        owner_user_id: get_text(row, 1),
        channel: get_opt_text(row, 2),
        thread_id: get_opt_uuid(row, 3),
        title: get_text(row, 4),
        intent: get_text(row, 5),
        priority: get_i64(row, 6) as i32,
        status: enum_from_snake_case(&get_text(row, 7), "GoalStatus")?,
        risk_class: enum_from_snake_case(&get_text(row, 8), "GoalRiskClass")?,
        acceptance_criteria: get_json(row, 9),
        constraints: get_json(row, 10),
        source: enum_from_snake_case(&get_text(row, 11), "GoalSource")?,
        created_at: get_ts(row, 12),
        updated_at: get_ts(row, 13),
        completed_at: get_opt_ts(row, 14),
    })
}

pub(crate) fn row_to_plan_libsql(row: &libsql::Row) -> Result<Plan, DatabaseError> {
    Ok(Plan {
        id: get_text(row, 0).parse().unwrap_or_default(),
        goal_id: get_text(row, 1).parse().unwrap_or_default(),
        revision: get_i64(row, 2) as i32,
        status: enum_from_snake_case(&get_text(row, 3), "PlanStatus")?,
        planner_kind: enum_from_snake_case(&get_text(row, 4), "PlannerKind")?,
        source_action_plan: get_opt_json(row, 5),
        assumptions: get_json(row, 6),
        confidence: get_opt_f64(row, 7).unwrap_or_default(),
        estimated_cost: get_opt_f64(row, 8),
        estimated_time_secs: row.get::<i64>(9).ok().filter(|v| *v >= 0).map(|v| v as u64),
        summary: get_opt_text(row, 10),
        created_at: get_ts(row, 11),
        updated_at: get_ts(row, 12),
    })
}

#[allow(dead_code)]
pub(crate) fn row_to_plan_step_libsql(row: &libsql::Row) -> Result<PlanStep, DatabaseError> {
    Ok(PlanStep {
        id: get_text(row, 0).parse().unwrap_or_default(),
        plan_id: get_text(row, 1).parse().unwrap_or_default(),
        sequence_num: get_i64(row, 2) as i32,
        kind: enum_from_snake_case(&get_text(row, 3), "PlanStepKind")?,
        status: enum_from_snake_case(&get_text(row, 4), "PlanStepStatus")?,
        title: get_text(row, 5),
        description: get_text(row, 6),
        tool_candidates: get_json(row, 7),
        inputs: get_json(row, 8),
        preconditions: get_json(row, 9),
        postconditions: get_json(row, 10),
        rollback: get_opt_json(row, 11),
        policy_requirements: get_json(row, 12),
        started_at: get_opt_ts(row, 13),
        completed_at: get_opt_ts(row, 14),
        created_at: get_ts(row, 15),
        updated_at: get_ts(row, 16),
    })
}

pub(crate) fn row_to_execution_attempt_libsql(
    row: &libsql::Row,
) -> Result<ExecutionAttempt, DatabaseError> {
    Ok(ExecutionAttempt {
        id: get_text(row, 0).parse().unwrap_or_default(),
        goal_id: get_opt_uuid(row, 1),
        plan_id: get_opt_uuid(row, 2),
        plan_step_id: get_opt_uuid(row, 3),
        job_id: get_opt_uuid(row, 4),
        thread_id: get_opt_uuid(row, 5),
        user_id: get_text(row, 6),
        channel: get_text(row, 7),
        tool_name: get_text(row, 8),
        tool_call_id: get_opt_text(row, 9),
        tool_args: get_opt_json(row, 10),
        status: enum_from_snake_case(&get_text(row, 11), "ExecutionAttemptStatus")?,
        failure_class: get_opt_text(row, 12),
        retry_count: get_i64(row, 13) as i32,
        started_at: get_ts(row, 14),
        finished_at: get_opt_ts(row, 15),
        elapsed_ms: row.get::<i64>(16).ok(),
        result_summary: get_opt_text(row, 17),
        error_preview: get_opt_text(row, 18),
    })
}

pub(crate) fn row_to_policy_decision_libsql(
    row: &libsql::Row,
) -> Result<PolicyDecision, DatabaseError> {
    Ok(PolicyDecision {
        id: get_text(row, 0).parse().unwrap_or_default(),
        goal_id: get_opt_uuid(row, 1),
        plan_id: get_opt_uuid(row, 2),
        plan_step_id: get_opt_uuid(row, 3),
        execution_attempt_id: get_opt_uuid(row, 4),
        user_id: get_text(row, 5),
        channel: get_text(row, 6),
        tool_name: get_opt_text(row, 7),
        tool_call_id: get_opt_text(row, 8),
        action_kind: get_text(row, 9),
        decision: enum_from_snake_case(&get_text(row, 10), "PolicyDecisionKind")?,
        reason_codes: get_json_string_array(row, 11),
        risk_score: get_opt_f32(row, 12),
        confidence: get_opt_f32(row, 13),
        requires_approval: get_i64(row, 14) != 0,
        auto_approved: get_opt_bool(row, 15),
        evidence_required: get_json(row, 16),
        created_at: get_ts(row, 17),
    })
}

#[cfg(test)]
mod tests {
    use crate::db::Database;
    use crate::db::libsql::LibSqlBackend;

    #[tokio::test]
    async fn test_wal_mode_after_migrations() {
        let backend = LibSqlBackend::new_memory().await.unwrap();
        backend.run_migrations().await.unwrap();

        let conn = backend.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA journal_mode", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let mode: String = row.get(0).unwrap();
        // In-memory databases use "memory" journal mode (WAL doesn't apply to :memory:),
        // but the PRAGMA still executes without error. For file-based databases it returns "wal".
        assert!(
            mode == "wal" || mode == "memory",
            "expected wal or memory, got: {}",
            mode,
        );
    }

    #[tokio::test]
    async fn test_busy_timeout_set_on_connect() {
        let backend = LibSqlBackend::new_memory().await.unwrap();
        backend.run_migrations().await.unwrap();

        let conn = backend.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA busy_timeout", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let timeout: i64 = row.get(0).unwrap();
        assert_eq!(timeout, 5000);
    }

    #[tokio::test]
    async fn test_concurrent_writes_succeed() {
        // Use a temp file so connections share state (in-memory DBs are connection-local)
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test_concurrent.db");
        let backend = LibSqlBackend::new_local(&db_path).await.unwrap();
        backend.run_migrations().await.unwrap();

        // Spawn 20 concurrent inserts into the conversations table
        let mut handles = Vec::new();
        for i in 0..20 {
            let conn = backend.connect().await.unwrap();
            let handle = tokio::spawn(async move {
                let id = uuid::Uuid::new_v4().to_string();
                let val = format!("ch_{}", i);
                conn.execute(
                    "INSERT INTO conversations (id, channel, user_id) VALUES (?1, ?2, ?3)",
                    libsql::params![id, val, "test_user"],
                )
                .await
            });
            handles.push(handle);
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(
                result.is_ok(),
                "concurrent write failed: {:?}",
                result.err()
            );
        }

        // Verify all 20 rows landed
        let conn = backend.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM conversations WHERE user_id = ?1",
                libsql::params!["test_user"],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let count: i64 = row.get(0).unwrap();
        assert_eq!(count, 20);
    }
}
