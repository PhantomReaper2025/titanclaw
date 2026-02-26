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
    ConsolidationRun, ExecutionAttempt, Goal, GoalStatus, Incident, MemoryEvent, MemoryRecord,
    MemoryRecordStatus, Plan, PlanStatus, PlanStep, PlanStepStatus, PlanVerification,
    PolicyDecision, ProceduralPlaybook, ProceduralPlaybookStatus,
};
use crate::context::JobState;
use crate::db::{
    AutonomyExecutionStore, AutonomyMemoryStore, AutonomyReliabilityStore, Database, GoalStore,
    PlanStore,
};
use crate::error::DatabaseError;
use crate::tools::{ToolContractV2Override, ToolReliabilityProfile};
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

async fn ensure_autonomy_incidents_phase3_columns(conn: &Connection) -> Result<(), DatabaseError> {
    for column in [
        "fingerprint TEXT",
        "surface TEXT",
        "tool_name TEXT",
        "occurrence_count INTEGER NOT NULL DEFAULT 1",
        "first_seen_at TEXT",
        "last_seen_at TEXT",
        "last_failure_class TEXT",
    ] {
        let sql = format!("ALTER TABLE autonomy_incidents ADD COLUMN {}", column);
        if let Err(e) = conn.execute(&sql, ()).await {
            let msg = e.to_string().to_lowercase();
            if msg.contains("duplicate column name") || msg.contains("no such table") {
                continue;
            }
            return Err(DatabaseError::Migration(format!(
                "Failed to add autonomy_incidents.{}: {}",
                column.split_whitespace().next().unwrap_or(column),
                e
            )));
        }
    }

    if let Err(e) = conn
        .execute(
            r#"
            UPDATE autonomy_incidents
            SET
                occurrence_count = CASE
                    WHEN occurrence_count IS NULL OR occurrence_count = 0 THEN 1
                    ELSE occurrence_count
                END,
                first_seen_at = COALESCE(first_seen_at, created_at),
                last_seen_at = COALESCE(last_seen_at, updated_at)
            "#,
            (),
        )
        .await
    {
        let msg = e.to_string().to_lowercase();
        if !msg.contains("no such table") && !msg.contains("no such column") {
            return Err(DatabaseError::Migration(format!(
                "Failed to backfill autonomy_incidents Phase 3 columns: {}",
                e
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
        ensure_autonomy_incidents_phase3_columns(&conn).await?;
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

    async fn update_goal_priority(&self, id: Uuid, priority: i32) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let now = fmt_ts(&Utc::now());

        conn.execute(
            r#"
            UPDATE autonomy_goals
            SET
                priority = ?2,
                updated_at = ?3
            WHERE id = ?1
            "#,
            params![id.to_string(), priority as i64, now.as_str()],
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

    async fn list_execution_attempts_for_user(
        &self,
        user_id: &str,
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
                WHERE user_id = ?1
                ORDER BY started_at DESC, id DESC
                "#,
                params![user_id],
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

    async fn list_policy_decisions_for_user(
        &self,
        user_id: &str,
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
                WHERE user_id = ?1
                ORDER BY created_at DESC, id DESC
                "#,
                params![user_id],
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

    async fn record_plan_verification(
        &self,
        verification: &PlanVerification,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&verification.status)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_plan_verifications (
                id, goal_id, plan_id, job_id, user_id, channel, verifier_kind, status,
                completion_claimed, evidence_count, summary, checks, evidence, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                verification.id.to_string(),
                opt_text_owned(verification.goal_id.map(|id| id.to_string())),
                verification.plan_id.to_string(),
                opt_text_owned(verification.job_id.map(|id| id.to_string())),
                verification.user_id.as_str(),
                verification.channel.as_str(),
                verification.verifier_kind.as_str(),
                status,
                verification.completion_claimed as i64,
                verification.evidence_count as i64,
                verification.summary.as_str(),
                verification.checks.to_string(),
                verification.evidence.to_string(),
                fmt_ts(&verification.created_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn list_plan_verifications_for_plan(
        &self,
        plan_id: Uuid,
    ) -> Result<Vec<PlanVerification>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, plan_id, job_id, user_id, channel, verifier_kind, status,
                    completion_claimed, evidence_count, summary, checks, evidence, created_at
                FROM autonomy_plan_verifications
                WHERE plan_id = ?1
                ORDER BY created_at DESC, id DESC
                "#,
                params![plan_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut verifications = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            verifications.push(row_to_plan_verification_libsql(&row)?);
        }
        Ok(verifications)
    }
}

#[async_trait]
impl AutonomyReliabilityStore for LibSqlBackend {
    async fn create_incident(&self, incident: &Incident) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;

        conn.execute(
            r#"
            INSERT INTO autonomy_incidents (
                id, goal_id, plan_id, plan_step_id, execution_attempt_id, policy_decision_id,
                job_id, thread_id, user_id, channel, incident_type, severity, status,
                fingerprint, surface, tool_name, occurrence_count, first_seen_at, last_seen_at,
                last_failure_class, summary, details, created_at, updated_at, resolved_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19,
                ?20, ?21, ?22, ?23, ?24, ?25
            )
            "#,
            params![
                incident.id.to_string(),
                opt_text_owned(incident.goal_id.map(|id| id.to_string())),
                opt_text_owned(incident.plan_id.map(|id| id.to_string())),
                opt_text_owned(incident.plan_step_id.map(|id| id.to_string())),
                opt_text_owned(incident.execution_attempt_id.map(|id| id.to_string())),
                opt_text_owned(incident.policy_decision_id.map(|id| id.to_string())),
                opt_text_owned(incident.job_id.map(|id| id.to_string())),
                opt_text_owned(incident.thread_id.map(|id| id.to_string())),
                incident.user_id.as_str(),
                opt_text(incident.channel.as_deref()),
                incident.incident_type.as_str(),
                incident.severity.as_str(),
                incident.status.as_str(),
                opt_text(incident.fingerprint.as_deref()),
                opt_text(incident.surface.as_deref()),
                opt_text(incident.tool_name.as_deref()),
                incident.occurrence_count as i64,
                fmt_opt_ts(&incident.first_seen_at),
                fmt_opt_ts(&incident.last_seen_at),
                opt_text(incident.last_failure_class.as_deref()),
                incident.summary.as_str(),
                incident.details.to_string(),
                fmt_ts(&incident.created_at),
                fmt_ts(&incident.updated_at),
                fmt_opt_ts(&incident.resolved_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_incident(&self, id: Uuid) -> Result<Option<Incident>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, plan_id, plan_step_id, execution_attempt_id, policy_decision_id,
                    job_id, thread_id, user_id, channel, incident_type, severity, status,
                    fingerprint, surface, tool_name, occurrence_count, first_seen_at, last_seen_at,
                    last_failure_class, summary, details, created_at, updated_at, resolved_at
                FROM autonomy_incidents
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
            Some(row) => Ok(Some(row_to_incident_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_incidents_for_user(&self, user_id: &str) -> Result<Vec<Incident>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, plan_id, plan_step_id, execution_attempt_id, policy_decision_id,
                    job_id, thread_id, user_id, channel, incident_type, severity, status,
                    fingerprint, surface, tool_name, occurrence_count, first_seen_at, last_seen_at,
                    last_failure_class, summary, details, created_at, updated_at, resolved_at
                FROM autonomy_incidents
                WHERE user_id = ?1
                ORDER BY COALESCE(last_seen_at, created_at) DESC, id DESC
                "#,
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut incidents = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            incidents.push(row_to_incident_libsql(&row)?);
        }
        Ok(incidents)
    }

    async fn find_open_incident_by_fingerprint(
        &self,
        user_id: &str,
        fingerprint: &str,
    ) -> Result<Option<Incident>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, goal_id, plan_id, plan_step_id, execution_attempt_id, policy_decision_id,
                    job_id, thread_id, user_id, channel, incident_type, severity, status,
                    fingerprint, surface, tool_name, occurrence_count, first_seen_at, last_seen_at,
                    last_failure_class, summary, details, created_at, updated_at, resolved_at
                FROM autonomy_incidents
                WHERE user_id = ?1
                  AND fingerprint = ?2
                  AND status IN ('open', 'investigating')
                ORDER BY COALESCE(last_seen_at, updated_at, created_at) DESC, id DESC
                LIMIT 1
                "#,
                params![user_id, fingerprint],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_incident_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn increment_incident_occurrence(
        &self,
        id: Uuid,
        observed_at: DateTime<Utc>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let observed = fmt_ts(&observed_at);

        conn.execute(
            r#"
            UPDATE autonomy_incidents
            SET
                occurrence_count = CASE
                    WHEN occurrence_count IS NULL OR occurrence_count = 0 THEN 2
                    ELSE occurrence_count + 1
                END,
                updated_at = ?2,
                first_seen_at = COALESCE(first_seen_at, created_at, ?2),
                last_seen_at = ?2
            WHERE id = ?1
            "#,
            params![id.to_string(), observed.as_str()],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn update_incident_status(
        &self,
        id: Uuid,
        status: &str,
        resolved_at: Option<DateTime<Utc>>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let now = fmt_ts(&Utc::now());

        conn.execute(
            r#"
            UPDATE autonomy_incidents
            SET status = ?2,
                updated_at = ?3,
                resolved_at = ?4
            WHERE id = ?1
            "#,
            params![
                id.to_string(),
                status,
                now.as_str(),
                fmt_opt_ts(&resolved_at)
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn upsert_tool_contract_override(
        &self,
        record: &ToolContractV2Override,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let source = enum_to_snake_case(&record.source)?;
        let descriptor_json = serde_json::to_string(&record.descriptor)
            .map_err(|e| DatabaseError::Serialization(e.to_string()))?;

        conn.execute(
            r#"
            DELETE FROM tool_contract_v2_overrides
            WHERE tool_name = ?1
              AND (
                    (owner_user_id = ?2)
                 OR (owner_user_id IS NULL AND ?2 IS NULL)
              )
            "#,
            params![
                record.tool_name.as_str(),
                opt_text(record.owner_user_id.as_deref())
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        conn.execute(
            r#"
            INSERT INTO tool_contract_v2_overrides (
                id, tool_name, owner_user_id, enabled, descriptor_json, source, notes,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                record.id.to_string(),
                record.tool_name.as_str(),
                opt_text(record.owner_user_id.as_deref()),
                if record.enabled { 1_i64 } else { 0_i64 },
                descriptor_json,
                source,
                opt_text(record.notes.as_deref()),
                fmt_ts(&record.created_at),
                fmt_ts(&record.updated_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_tool_contract_override(
        &self,
        tool_name: &str,
        owner_user_id: Option<&str>,
    ) -> Result<Option<ToolContractV2Override>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = match owner_user_id {
            Some(owner) => {
                conn.query(
                    r#"
                    SELECT
                        id, tool_name, owner_user_id, enabled, descriptor_json, source, notes,
                        created_at, updated_at
                    FROM tool_contract_v2_overrides
                    WHERE tool_name = ?1 AND owner_user_id = ?2
                    ORDER BY updated_at DESC, id DESC
                    LIMIT 1
                    "#,
                    params![tool_name, owner],
                )
                .await
            }
            None => {
                conn.query(
                    r#"
                    SELECT
                        id, tool_name, owner_user_id, enabled, descriptor_json, source, notes,
                        created_at, updated_at
                    FROM tool_contract_v2_overrides
                    WHERE tool_name = ?1 AND owner_user_id IS NULL
                    ORDER BY updated_at DESC, id DESC
                    LIMIT 1
                    "#,
                    params![tool_name],
                )
                .await
            }
        }
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_tool_contract_override_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_tool_contract_overrides(
        &self,
        owner_user_id: Option<&str>,
    ) -> Result<Vec<ToolContractV2Override>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = match owner_user_id {
            Some(owner) => {
                conn.query(
                    r#"
                    SELECT
                        id, tool_name, owner_user_id, enabled, descriptor_json, source, notes,
                        created_at, updated_at
                    FROM tool_contract_v2_overrides
                    WHERE owner_user_id = ?1
                    ORDER BY updated_at DESC, tool_name ASC, id DESC
                    "#,
                    params![owner],
                )
                .await
            }
            None => {
                conn.query(
                    r#"
                    SELECT
                        id, tool_name, owner_user_id, enabled, descriptor_json, source, notes,
                        created_at, updated_at
                    FROM tool_contract_v2_overrides
                    ORDER BY updated_at DESC, tool_name ASC, id DESC
                    "#,
                    (),
                )
                .await
            }
        }
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut overrides = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            overrides.push(row_to_tool_contract_override_libsql(&row)?);
        }
        Ok(overrides)
    }

    async fn delete_tool_contract_override(
        &self,
        tool_name: &str,
        owner_user_id: Option<&str>,
    ) -> Result<bool, DatabaseError> {
        let conn = self.connect().await?;
        let deleted = conn
            .execute(
                r#"
                DELETE FROM tool_contract_v2_overrides
                WHERE tool_name = ?1
                  AND (
                        (owner_user_id = ?2)
                     OR (owner_user_id IS NULL AND ?2 IS NULL)
                  )
                "#,
                params![tool_name, opt_text(owner_user_id)],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(deleted > 0)
    }

    async fn upsert_tool_reliability_profile(
        &self,
        profile: &ToolReliabilityProfile,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let breaker_state = enum_to_snake_case(&profile.breaker_state)?;

        conn.execute(
            r#"
            INSERT INTO tool_reliability_profiles (
                tool_name, window_start, window_end, sample_count, success_count, failure_count,
                timeout_count, blocked_count, success_rate, p50_latency_ms, p95_latency_ms,
                common_failure_modes, recent_incident_count, reliability_score,
                safe_fallback_options, breaker_state, cooldown_until, last_failure_at,
                last_success_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14,
                ?15, ?16, ?17, ?18,
                ?19, ?20
            )
            ON CONFLICT(tool_name) DO UPDATE SET
                window_start = excluded.window_start,
                window_end = excluded.window_end,
                sample_count = excluded.sample_count,
                success_count = excluded.success_count,
                failure_count = excluded.failure_count,
                timeout_count = excluded.timeout_count,
                blocked_count = excluded.blocked_count,
                success_rate = excluded.success_rate,
                p50_latency_ms = excluded.p50_latency_ms,
                p95_latency_ms = excluded.p95_latency_ms,
                common_failure_modes = excluded.common_failure_modes,
                recent_incident_count = excluded.recent_incident_count,
                reliability_score = excluded.reliability_score,
                safe_fallback_options = excluded.safe_fallback_options,
                breaker_state = excluded.breaker_state,
                cooldown_until = excluded.cooldown_until,
                last_failure_at = excluded.last_failure_at,
                last_success_at = excluded.last_success_at,
                updated_at = excluded.updated_at
            "#,
            params![
                profile.tool_name.as_str(),
                fmt_ts(&profile.window_start),
                fmt_ts(&profile.window_end),
                profile.sample_count,
                profile.success_count,
                profile.failure_count,
                profile.timeout_count,
                profile.blocked_count,
                profile.success_rate as f64,
                profile.p50_latency_ms,
                profile.p95_latency_ms,
                profile.common_failure_modes.to_string(),
                profile.recent_incident_count,
                profile.reliability_score as f64,
                profile.safe_fallback_options.to_string(),
                breaker_state,
                fmt_opt_ts(&profile.cooldown_until),
                fmt_opt_ts(&profile.last_failure_at),
                fmt_opt_ts(&profile.last_success_at),
                fmt_ts(&profile.updated_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_tool_reliability_profile(
        &self,
        tool_name: &str,
    ) -> Result<Option<ToolReliabilityProfile>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    tool_name, window_start, window_end, sample_count, success_count, failure_count,
                    timeout_count, blocked_count, success_rate, p50_latency_ms, p95_latency_ms,
                    common_failure_modes, recent_incident_count, reliability_score,
                    safe_fallback_options, breaker_state, cooldown_until, last_failure_at,
                    last_success_at, updated_at
                FROM tool_reliability_profiles
                WHERE tool_name = ?1
                "#,
                params![tool_name],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_tool_reliability_profile_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_tool_reliability_profiles(
        &self,
    ) -> Result<Vec<ToolReliabilityProfile>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    tool_name, window_start, window_end, sample_count, success_count, failure_count,
                    timeout_count, blocked_count, success_rate, p50_latency_ms, p95_latency_ms,
                    common_failure_modes, recent_incident_count, reliability_score,
                    safe_fallback_options, breaker_state, cooldown_until, last_failure_at,
                    last_success_at, updated_at
                FROM tool_reliability_profiles
                ORDER BY updated_at DESC, tool_name ASC
                "#,
                (),
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut profiles = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            profiles.push(row_to_tool_reliability_profile_libsql(&row)?);
        }
        Ok(profiles)
    }
}

#[async_trait]
impl AutonomyMemoryStore for LibSqlBackend {
    async fn create_memory_record(&self, record: &MemoryRecord) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let memory_type = enum_to_snake_case(&record.memory_type)?;
        let source_kind = enum_to_snake_case(&record.source_kind)?;
        let sensitivity = enum_to_snake_case(&record.sensitivity)?;
        let status = enum_to_snake_case(&record.status)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_memory_records (
                id, owner_user_id, goal_id, plan_id, plan_step_id, job_id, thread_id,
                memory_type, source_kind, category, title, summary, payload, provenance,
                confidence, sensitivity, ttl_secs, status, workspace_doc_path,
                workspace_document_id, created_at, updated_at, expires_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19,
                ?20, ?21, ?22, ?23
            )
            "#,
            params![
                record.id.to_string(),
                record.owner_user_id.as_str(),
                opt_text_owned(record.goal_id.map(|id| id.to_string())),
                opt_text_owned(record.plan_id.map(|id| id.to_string())),
                opt_text_owned(record.plan_step_id.map(|id| id.to_string())),
                opt_text_owned(record.job_id.map(|id| id.to_string())),
                opt_text_owned(record.thread_id.map(|id| id.to_string())),
                memory_type,
                source_kind,
                record.category.as_str(),
                record.title.as_str(),
                record.summary.as_str(),
                record.payload.to_string(),
                record.provenance.to_string(),
                record.confidence as f64,
                sensitivity,
                record.ttl_secs,
                status,
                opt_text(record.workspace_doc_path.as_deref()),
                opt_text_owned(record.workspace_document_id.map(|id| id.to_string())),
                fmt_ts(&record.created_at),
                fmt_ts(&record.updated_at),
                fmt_opt_ts(&record.expires_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_memory_record(&self, id: Uuid) -> Result<Option<MemoryRecord>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, goal_id, plan_id, plan_step_id, job_id, thread_id,
                    memory_type, source_kind, category, title, summary, payload, provenance,
                    confidence, sensitivity, ttl_secs, status, workspace_doc_path,
                    workspace_document_id, created_at, updated_at, expires_at
                FROM autonomy_memory_records
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
            Some(row) => Ok(Some(row_to_memory_record_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_memory_records_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<MemoryRecord>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, goal_id, plan_id, plan_step_id, job_id, thread_id,
                    memory_type, source_kind, category, title, summary, payload, provenance,
                    confidence, sensitivity, ttl_secs, status, workspace_doc_path,
                    workspace_document_id, created_at, updated_at, expires_at
                FROM autonomy_memory_records
                WHERE owner_user_id = ?1
                ORDER BY created_at DESC, id DESC
                "#,
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut records = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            records.push(row_to_memory_record_libsql(&row)?);
        }
        Ok(records)
    }

    async fn list_memory_records_for_goal(
        &self,
        goal_id: Uuid,
    ) -> Result<Vec<MemoryRecord>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, goal_id, plan_id, plan_step_id, job_id, thread_id,
                    memory_type, source_kind, category, title, summary, payload, provenance,
                    confidence, sensitivity, ttl_secs, status, workspace_doc_path,
                    workspace_document_id, created_at, updated_at, expires_at
                FROM autonomy_memory_records
                WHERE goal_id = ?1
                ORDER BY created_at DESC, id DESC
                "#,
                params![goal_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut records = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            records.push(row_to_memory_record_libsql(&row)?);
        }
        Ok(records)
    }

    async fn update_memory_record_status(
        &self,
        id: Uuid,
        status: MemoryRecordStatus,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&status)?;

        conn.execute(
            r#"
            UPDATE autonomy_memory_records
            SET status = ?2, updated_at = ?3
            WHERE id = ?1
            "#,
            params![id.to_string(), status, fmt_ts(&Utc::now())],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn record_memory_event(&self, event: &MemoryEvent) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let reason_codes = serde_json::to_string(&event.reason_codes)
            .map_err(|e| DatabaseError::Serialization(e.to_string()))?;
        let action = match event.action {
            Some(action) => libsql::Value::Text(enum_to_snake_case(&action)?),
            None => libsql::Value::Null,
        };

        conn.execute(
            r#"
            INSERT INTO autonomy_memory_events (
                id, memory_record_id, event_kind, actor, reason_codes, action, "before", "after", created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                event.id.to_string(),
                event.memory_record_id.to_string(),
                event.event_kind.as_str(),
                event.actor.as_str(),
                reason_codes,
                action,
                event.before.to_string(),
                event.after.to_string(),
                fmt_ts(&event.created_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_memory_events_for_record(
        &self,
        memory_record_id: Uuid,
    ) -> Result<Vec<MemoryEvent>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, memory_record_id, event_kind, actor, reason_codes, action, "before", "after", created_at
                FROM autonomy_memory_events
                WHERE memory_record_id = ?1
                ORDER BY created_at DESC, id DESC
                "#,
                params![memory_record_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            events.push(row_to_memory_event_libsql(&row)?);
        }
        Ok(events)
    }

    async fn create_or_update_procedural_playbook(
        &self,
        playbook: &ProceduralPlaybook,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&playbook.status)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_procedural_playbooks (
                id, owner_user_id, name, task_class, trigger_signals, steps_template,
                tool_preferences, constraints, success_count, failure_count, confidence,
                status, requires_approval, source_memory_record_ids, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16
            )
            ON CONFLICT(id) DO UPDATE SET
                owner_user_id = excluded.owner_user_id,
                name = excluded.name,
                task_class = excluded.task_class,
                trigger_signals = excluded.trigger_signals,
                steps_template = excluded.steps_template,
                tool_preferences = excluded.tool_preferences,
                constraints = excluded.constraints,
                success_count = excluded.success_count,
                failure_count = excluded.failure_count,
                confidence = excluded.confidence,
                status = excluded.status,
                requires_approval = excluded.requires_approval,
                source_memory_record_ids = excluded.source_memory_record_ids,
                updated_at = excluded.updated_at
            "#,
            params![
                playbook.id.to_string(),
                playbook.owner_user_id.as_str(),
                playbook.name.as_str(),
                playbook.task_class.as_str(),
                playbook.trigger_signals.to_string(),
                playbook.steps_template.to_string(),
                playbook.tool_preferences.to_string(),
                playbook.constraints.to_string(),
                playbook.success_count as i64,
                playbook.failure_count as i64,
                playbook.confidence as f64,
                status,
                if playbook.requires_approval {
                    1_i64
                } else {
                    0_i64
                },
                playbook.source_memory_record_ids.to_string(),
                fmt_ts(&playbook.created_at),
                fmt_ts(&playbook.updated_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_procedural_playbook(
        &self,
        id: Uuid,
    ) -> Result<Option<ProceduralPlaybook>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, name, task_class, trigger_signals, steps_template,
                    tool_preferences, constraints, success_count, failure_count, confidence,
                    status, requires_approval, source_memory_record_ids, created_at, updated_at
                FROM autonomy_procedural_playbooks
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
            Some(row) => Ok(Some(row_to_procedural_playbook_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_procedural_playbooks_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<ProceduralPlaybook>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, name, task_class, trigger_signals, steps_template,
                    tool_preferences, constraints, success_count, failure_count, confidence,
                    status, requires_approval, source_memory_record_ids, created_at, updated_at
                FROM autonomy_procedural_playbooks
                WHERE owner_user_id = ?1
                ORDER BY updated_at DESC, id DESC
                "#,
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut playbooks = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            playbooks.push(row_to_procedural_playbook_libsql(&row)?);
        }
        Ok(playbooks)
    }

    async fn update_procedural_playbook_status(
        &self,
        id: Uuid,
        status: ProceduralPlaybookStatus,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&status)?;

        conn.execute(
            r#"
            UPDATE autonomy_procedural_playbooks
            SET status = ?2, updated_at = ?3
            WHERE id = ?1
            "#,
            params![id.to_string(), status, fmt_ts(&Utc::now())],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn record_consolidation_run(&self, run: &ConsolidationRun) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&run.status)?;

        conn.execute(
            r#"
            INSERT INTO autonomy_consolidation_runs (
                id, owner_user_id, status, started_at, finished_at, batch_size, processed_count,
                promoted_count, playbooks_created_count, archived_count, error_count,
                checkpoint_cursor, notes
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11,
                ?12, ?13
            )
            "#,
            params![
                run.id.to_string(),
                opt_text(run.owner_user_id.as_deref()),
                status,
                fmt_ts(&run.started_at),
                fmt_opt_ts(&run.finished_at),
                run.batch_size as i64,
                run.processed_count as i64,
                run.promoted_count as i64,
                run.playbooks_created_count as i64,
                run.archived_count as i64,
                run.error_count as i64,
                opt_text(run.checkpoint_cursor.as_deref()),
                opt_text(run.notes.as_deref()),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn update_consolidation_run(&self, run: &ConsolidationRun) -> Result<(), DatabaseError> {
        let conn = self.connect().await?;
        let status = enum_to_snake_case(&run.status)?;

        conn.execute(
            r#"
            UPDATE autonomy_consolidation_runs
            SET owner_user_id = ?2,
                status = ?3,
                started_at = ?4,
                finished_at = ?5,
                batch_size = ?6,
                processed_count = ?7,
                promoted_count = ?8,
                playbooks_created_count = ?9,
                archived_count = ?10,
                error_count = ?11,
                checkpoint_cursor = ?12,
                notes = ?13
            WHERE id = ?1
            "#,
            params![
                run.id.to_string(),
                opt_text(run.owner_user_id.as_deref()),
                status,
                fmt_ts(&run.started_at),
                fmt_opt_ts(&run.finished_at),
                run.batch_size as i64,
                run.processed_count as i64,
                run.promoted_count as i64,
                run.playbooks_created_count as i64,
                run.archived_count as i64,
                run.error_count as i64,
                opt_text(run.checkpoint_cursor.as_deref()),
                opt_text(run.notes.as_deref()),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_consolidation_runs_for_user(
        &self,
        user_id: Option<&str>,
    ) -> Result<Vec<ConsolidationRun>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = if let Some(user_id) = user_id {
            conn.query(
                r#"
                SELECT
                    id, owner_user_id, status, started_at, finished_at, batch_size, processed_count,
                    promoted_count, playbooks_created_count, archived_count, error_count,
                    checkpoint_cursor, notes
                FROM autonomy_consolidation_runs
                WHERE owner_user_id = ?1
                ORDER BY started_at DESC, id DESC
                "#,
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        } else {
            conn.query(
                r#"
                SELECT
                    id, owner_user_id, status, started_at, finished_at, batch_size, processed_count,
                    promoted_count, playbooks_created_count, archived_count, error_count,
                    checkpoint_cursor, notes
                FROM autonomy_consolidation_runs
                ORDER BY started_at DESC, id DESC
                "#,
                (),
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        };

        let mut runs = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            runs.push(row_to_consolidation_run_libsql(&row)?);
        }
        Ok(runs)
    }

    async fn list_pending_episodic_for_consolidation(
        &self,
        limit: i64,
    ) -> Result<Vec<MemoryRecord>, DatabaseError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    id, owner_user_id, goal_id, plan_id, plan_step_id, job_id, thread_id,
                    memory_type, source_kind, category, title, summary, payload, provenance,
                    confidence, sensitivity, ttl_secs, status, workspace_doc_path,
                    workspace_document_id, created_at, updated_at, expires_at
                FROM autonomy_memory_records
                WHERE memory_type = ?1
                  AND status = ?2
                  AND (expires_at IS NULL OR expires_at > ?3)
                ORDER BY created_at ASC, id ASC
                LIMIT ?4
                "#,
                params!["episodic", "active", fmt_ts(&Utc::now()), limit.max(0),],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut records = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            records.push(row_to_memory_record_libsql(&row)?);
        }
        Ok(records)
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

pub(crate) fn row_to_plan_verification_libsql(
    row: &libsql::Row,
) -> Result<PlanVerification, DatabaseError> {
    Ok(PlanVerification {
        id: get_text(row, 0).parse().unwrap_or_default(),
        goal_id: get_opt_uuid(row, 1),
        plan_id: get_text(row, 2).parse().unwrap_or_default(),
        job_id: get_opt_uuid(row, 3),
        user_id: get_text(row, 4),
        channel: get_text(row, 5),
        verifier_kind: get_text(row, 6),
        status: enum_from_snake_case(&get_text(row, 7), "PlanVerificationStatus")?,
        completion_claimed: get_i64(row, 8) != 0,
        evidence_count: get_i64(row, 9) as i32,
        summary: get_text(row, 10),
        checks: get_json(row, 11),
        evidence: get_json(row, 12),
        created_at: get_ts(row, 13),
    })
}

pub(crate) fn row_to_incident_libsql(row: &libsql::Row) -> Result<Incident, DatabaseError> {
    Ok(Incident {
        id: get_text(row, 0).parse().unwrap_or_default(),
        goal_id: get_opt_uuid(row, 1),
        plan_id: get_opt_uuid(row, 2),
        plan_step_id: get_opt_uuid(row, 3),
        execution_attempt_id: get_opt_uuid(row, 4),
        policy_decision_id: get_opt_uuid(row, 5),
        job_id: get_opt_uuid(row, 6),
        thread_id: get_opt_uuid(row, 7),
        user_id: get_text(row, 8),
        channel: get_opt_text(row, 9),
        incident_type: get_text(row, 10),
        severity: get_text(row, 11),
        status: get_text(row, 12),
        fingerprint: get_opt_text(row, 13),
        surface: get_opt_text(row, 14),
        tool_name: get_opt_text(row, 15),
        occurrence_count: get_i64(row, 16) as i32,
        first_seen_at: get_opt_ts(row, 17),
        last_seen_at: get_opt_ts(row, 18),
        last_failure_class: get_opt_text(row, 19),
        summary: get_text(row, 20),
        details: get_json(row, 21),
        created_at: get_ts(row, 22),
        updated_at: get_ts(row, 23),
        resolved_at: get_opt_ts(row, 24),
    })
}

pub(crate) fn row_to_tool_contract_override_libsql(
    row: &libsql::Row,
) -> Result<ToolContractV2Override, DatabaseError> {
    let descriptor = serde_json::from_value(get_json(row, 4))
        .map_err(|e| DatabaseError::Serialization(e.to_string()))?;

    Ok(ToolContractV2Override {
        id: get_text(row, 0).parse().unwrap_or_default(),
        tool_name: get_text(row, 1),
        owner_user_id: get_opt_text(row, 2),
        enabled: get_i64(row, 3) != 0,
        descriptor,
        source: enum_from_snake_case(&get_text(row, 5), "ToolContractOverrideSource")?,
        notes: get_opt_text(row, 6),
        created_at: get_ts(row, 7),
        updated_at: get_ts(row, 8),
    })
}

pub(crate) fn row_to_tool_reliability_profile_libsql(
    row: &libsql::Row,
) -> Result<ToolReliabilityProfile, DatabaseError> {
    Ok(ToolReliabilityProfile {
        tool_name: get_text(row, 0),
        window_start: get_ts(row, 1),
        window_end: get_ts(row, 2),
        sample_count: get_i64(row, 3),
        success_count: get_i64(row, 4),
        failure_count: get_i64(row, 5),
        timeout_count: get_i64(row, 6),
        blocked_count: get_i64(row, 7),
        success_rate: get_opt_f32(row, 8).unwrap_or_default(),
        p50_latency_ms: row.get::<i64>(9).ok(),
        p95_latency_ms: row.get::<i64>(10).ok(),
        common_failure_modes: get_json(row, 11),
        recent_incident_count: get_i64(row, 12),
        reliability_score: get_opt_f32(row, 13).unwrap_or_default(),
        safe_fallback_options: get_json(row, 14),
        breaker_state: enum_from_snake_case(&get_text(row, 15), "CircuitBreakerState")?,
        cooldown_until: get_opt_ts(row, 16),
        last_failure_at: get_opt_ts(row, 17),
        last_success_at: get_opt_ts(row, 18),
        updated_at: get_ts(row, 19),
    })
}

pub(crate) fn row_to_memory_record_libsql(
    row: &libsql::Row,
) -> Result<MemoryRecord, DatabaseError> {
    Ok(MemoryRecord {
        id: get_text(row, 0).parse().unwrap_or_default(),
        owner_user_id: get_text(row, 1),
        goal_id: get_opt_uuid(row, 2),
        plan_id: get_opt_uuid(row, 3),
        plan_step_id: get_opt_uuid(row, 4),
        job_id: get_opt_uuid(row, 5),
        thread_id: get_opt_uuid(row, 6),
        memory_type: enum_from_snake_case(&get_text(row, 7), "MemoryType")?,
        source_kind: enum_from_snake_case(&get_text(row, 8), "MemorySourceKind")?,
        category: get_text(row, 9),
        title: get_text(row, 10),
        summary: get_text(row, 11),
        payload: get_json(row, 12),
        provenance: get_json(row, 13),
        confidence: get_opt_f32(row, 14).unwrap_or_default(),
        sensitivity: enum_from_snake_case(&get_text(row, 15), "MemorySensitivity")?,
        ttl_secs: row.get::<i64>(16).ok(),
        status: enum_from_snake_case(&get_text(row, 17), "MemoryRecordStatus")?,
        workspace_doc_path: get_opt_text(row, 18),
        workspace_document_id: get_opt_uuid(row, 19),
        created_at: get_ts(row, 20),
        updated_at: get_ts(row, 21),
        expires_at: get_opt_ts(row, 22),
    })
}

pub(crate) fn row_to_memory_event_libsql(row: &libsql::Row) -> Result<MemoryEvent, DatabaseError> {
    Ok(MemoryEvent {
        id: get_text(row, 0).parse().unwrap_or_default(),
        memory_record_id: get_text(row, 1).parse().unwrap_or_default(),
        event_kind: get_text(row, 2),
        actor: get_text(row, 3),
        reason_codes: get_json_string_array(row, 4),
        action: get_opt_text(row, 5)
            .as_deref()
            .map(|s| enum_from_snake_case(s, "ConsolidationAction"))
            .transpose()?,
        before: get_json(row, 6),
        after: get_json(row, 7),
        created_at: get_ts(row, 8),
    })
}

pub(crate) fn row_to_procedural_playbook_libsql(
    row: &libsql::Row,
) -> Result<ProceduralPlaybook, DatabaseError> {
    Ok(ProceduralPlaybook {
        id: get_text(row, 0).parse().unwrap_or_default(),
        owner_user_id: get_text(row, 1),
        name: get_text(row, 2),
        task_class: get_text(row, 3),
        trigger_signals: get_json(row, 4),
        steps_template: get_json(row, 5),
        tool_preferences: get_json(row, 6),
        constraints: get_json(row, 7),
        success_count: get_i64(row, 8) as i32,
        failure_count: get_i64(row, 9) as i32,
        confidence: get_opt_f32(row, 10).unwrap_or_default(),
        status: enum_from_snake_case(&get_text(row, 11), "ProceduralPlaybookStatus")?,
        requires_approval: get_i64(row, 12) != 0,
        source_memory_record_ids: get_json(row, 13),
        created_at: get_ts(row, 14),
        updated_at: get_ts(row, 15),
    })
}

pub(crate) fn row_to_consolidation_run_libsql(
    row: &libsql::Row,
) -> Result<ConsolidationRun, DatabaseError> {
    Ok(ConsolidationRun {
        id: get_text(row, 0).parse().unwrap_or_default(),
        owner_user_id: get_opt_text(row, 1),
        status: enum_from_snake_case(&get_text(row, 2), "ConsolidationRunStatus")?,
        started_at: get_ts(row, 3),
        finished_at: get_opt_ts(row, 4),
        batch_size: get_i64(row, 5) as i32,
        processed_count: get_i64(row, 6) as i32,
        promoted_count: get_i64(row, 7) as i32,
        playbooks_created_count: get_i64(row, 8) as i32,
        archived_count: get_i64(row, 9) as i32,
        error_count: get_i64(row, 10) as i32,
        checkpoint_cursor: get_opt_text(row, 11),
        notes: get_opt_text(row, 12),
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
