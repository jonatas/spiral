use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgListener, PgPoolOptions};
use sqlx::{FromRow, Pool, Postgres, Row};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Serialize, Deserialize, Debug, FromRow, Clone)]
struct Metadata {
    view_name: String,
    parent_view: String,
    frame_seconds: i32,
    base_view: String,
    scope_columns: Vec<String>,
    columns_metadata: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, FromRow, Clone)]
struct SourceInfo {
    view_name: String,
    base_view: String,
    frame_seconds: i32,
    base_column: String,
    formula: String,
    mat_column: String,
    rollup_gsub_strategy: Option<String>,
    metadata: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, FromRow, Clone)]
struct ChangelogEntry {
    event_id: i64,
    base_view: String,
    scope_values: serde_json::Value,
    t_start: Option<i64>,
    t_end: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, FromRow, Clone)]
struct ChangelogEntryFull {
    event_id: i64,
    base_view: String,
    scope_values: serde_json::Value,
    t_start: Option<i64>,
    t_end: Option<i64>,
    age_seconds: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct BlockInfo {
    blkno: i32,
    t_range: [i64; 2],
    tenant_range: [i32; 2],
    tuple_count: i64,
    is_boundary: bool,
    drift_records: i64,
    alignment_pct: f64,
    #[serde(default)]
    t_actual_start: i64,
    #[serde(default)]
    t_actual_end: i64,
    #[serde(default)]
    pending_changes: i32,
    #[serde(default)]
    is_stale: bool,
    #[serde(default)]
    last_changelog_ts: i64,
    #[serde(default)]
    kickoff_epoch: i64,
    #[serde(default)]
    fill_pct: f64,
    #[serde(default)]
    live_tuples: i64,
    #[serde(default)]
    dead_tuples: i32,
    #[serde(default)]
    unused_slots: i64,
    #[serde(default)]
    opaque_window_start_t: i64,
    #[serde(default)]
    opaque_window_end_t: i64,
    #[serde(default)]
    opaque_tenant_scale: i32,
    #[serde(default)]
    magic_valid: bool,
    #[serde(default)]
    is_gap_page: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct StorageStats {
    #[serde(default)]
    base_view: String,
    #[serde(default)]
    view_name: String,
    total_pages: i64,
    total_rows_capacity: i64,
    #[serde(default)]
    row_count: i64,
    tenant_scale: i64,
    spiral_size_kb: i64,
    projected_heap_size_kb: i64,
    compression_ratio: f64,
    kickoff_epoch: i64,
    #[serde(default)]
    min_t: i64,
    #[serde(default)]
    max_t: i64,
    #[serde(default)]
    heap_bytes_per_row: f64,
    #[serde(default)]
    heap_rows_per_page: f64,
    #[serde(default)]
    xor_bytes_per_row: f64,
    #[serde(default)]
    xor_rows_per_page: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct AggregationLevel {
    frame_seconds: i32,
    view_name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct HierarchyConfig {
    base_view: String,
    raw_view_name: String,
    time_column: String,
    aggregation_levels: Vec<AggregationLevel>,
    tenant_scale: i64,
    sources: Vec<SourceInfo>,
    kickoff_epoch: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct SystemConfig {
    hierarchies: Vec<HierarchyConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WorkerInfo {
    pid: i32,
    state: String,
    duration_ms: i64,
    query_snippet: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, FromRow)]
struct ChangelogSummaryRow {
    base_view: String,
    pending_count: i64,
    oldest_age_seconds: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data")]
enum VortexEvent {
    ChangelogUpdate(ChangelogEntry),
    StorageStats(StorageStats),
    SystemConfig(SystemConfig),
    WorkerUpdate { workers: Vec<WorkerInfo>, summary: Vec<ChangelogSummaryRow> },
}

struct AppState {
    pool: Pool<Postgres>,
    tx: broadcast::Sender<VortexEvent>,
    recent_changelog: Mutex<VecDeque<ChangelogEntry>>,
    last_worker_update: Mutex<(Vec<WorkerInfo>, Vec<ChangelogSummaryRow>)>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vortex_server=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());

    let connect_opts = database_url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .expect("Invalid DATABASE_URL")
        .options([("lock_timeout", "3000"), ("statement_timeout", "30000")]);

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(connect_opts)
        .await
        .expect("Failed to connect to Postgres");

    let (tx, _rx) = broadcast::channel(100);

    let state = Arc::new(AppState {
        pool: pool.clone(),
        tx: tx.clone(),
        recent_changelog: Mutex::new(VecDeque::with_capacity(500)),
        last_worker_update: Mutex::new((Vec::new(), Vec::new())),
    });

    let poll_secs = std::env::var("VORTEX_POLL_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5);

    // Start background polling task
    let polling_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut last_processed_event_id = 0i64;
        let mut last_config_signature: Option<String> = None;

        tracing::info!("Starting background polling loop (interval={}s)", poll_secs);
        loop {
            // 1. Poll for new changelog entries
            let result = sqlx::query_as::<_, ChangelogEntry>(
                "SELECT event_id, base_view, scope_values, t_start, t_end
                 FROM spiral.changelog
                 WHERE event_id > $1
                 ORDER BY event_id ASC",
            )
            .bind(last_processed_event_id)
            .fetch_all(&polling_state.pool)
            .await;

            match result {
                Ok(entries) => {
                    if !entries.is_empty() {
                        tracing::info!(
                            "Found {} new changelog entries since event_id={}",
                            entries.len(),
                            last_processed_event_id
                        );

                        // Dedup against buffer (pg_notify listener may have already sent these)
                        let already_sent: std::collections::BTreeSet<i64> = polling_state
                            .recent_changelog
                            .lock()
                            .map(|buf| buf.iter().map(|e| e.event_id).collect())
                            .unwrap_or_default();


                        if let Ok(mut buffer) = polling_state.recent_changelog.lock() {
                            for entry in &entries {
                                if !already_sent.contains(&entry.event_id) {
                                    buffer.push_front(entry.clone());
                                }
                            }
                            buffer.truncate(500);
                        }
                        for entry in entries {
                            last_processed_event_id = last_processed_event_id.max(entry.event_id);
                            if !already_sent.contains(&entry.event_id) {
                                let _ = polling_state.tx.send(VortexEvent::ChangelogUpdate(entry));
                            }
                        }
                    }
                }
                Err(e) => log_db_error("changelog poll", &e),
            }

            // 2. Poll metadata; on change broadcast SystemConfig.
            // 3. Batch all storage stats in one query — O(1) round-trips regardless of view count.
            match load_system_config(&polling_state.pool).await {
                Ok(config) => {
                    let signature = serde_json::to_string(&config).unwrap_or_default();
                    if last_config_signature.as_deref() != Some(signature.as_str()) {
                        last_config_signature = Some(signature);
                        let _ = polling_state.tx.send(VortexEvent::SystemConfig(config.clone()));
                    }

                    let all_view_names: Vec<String> = config.hierarchies.iter().flat_map(|h| {
                        std::iter::once(h.raw_view_name.clone())
                            .chain(h.aggregation_levels.iter().map(|l| l.view_name.clone()))
                    }).collect();

                    let mut all_stats = load_all_storage_stats(&polling_state.pool, &all_view_names).await;

                    for hierarchy in &config.hierarchies {
                        for view_name in std::iter::once(&hierarchy.raw_view_name)
                            .chain(hierarchy.aggregation_levels.iter().map(|l| &l.view_name))
                        {
                            if let Some(mut stats) = all_stats.remove(view_name) {
                                stats.base_view = hierarchy.base_view.clone();
                                let _ = polling_state.tx.send(VortexEvent::StorageStats(stats));
                            }
                        }
                    }
                }
                Err(e) => log_db_error("metadata poll", &e),
            }

            // 3. Poll active workers + per-table changelog summary
            let workers: Vec<WorkerInfo> = sqlx::query(
                "SELECT pid::int as pid,
                        COALESCE(state, 'idle') as state,
                        COALESCE(EXTRACT(EPOCH FROM (now() - query_start))::bigint * 1000, 0) AS duration_ms,
                        COALESCE(left(query, 300), '') AS query_snippet
                 FROM pg_stat_activity
                 WHERE application_name LIKE 'spiral%'
                    OR (query ILIKE '%spiral_refresh_scope%' AND state IS DISTINCT FROM 'idle')"
            )
            .fetch_all(&polling_state.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                Some(WorkerInfo {
                    pid: row.try_get::<i32, _>("pid").ok()?,
                    state: row.try_get::<String, _>("state").unwrap_or_default(),
                    duration_ms: row.try_get::<i64, _>("duration_ms").unwrap_or(0),
                    query_snippet: row.try_get::<String, _>("query_snippet").unwrap_or_default(),
                })
            })
            .collect();

            let summary = sqlx::query_as::<_, ChangelogSummaryRow>(
                "SELECT base_view,
                        count(*)::bigint AS pending_count,
                        GREATEST(0, EXTRACT(EPOCH FROM now())::bigint - min(t_end)) AS oldest_age_seconds
                 FROM spiral.changelog
                 GROUP BY base_view
                 ORDER BY oldest_age_seconds DESC"
            )
            .fetch_all(&polling_state.pool)
            .await
            .unwrap_or_default();

            if let Ok(mut wu) = polling_state.last_worker_update.lock() {
                *wu = (workers.clone(), summary.clone());
            }
            let _ = polling_state.tx.send(VortexEvent::WorkerUpdate { workers, summary });

            tokio::time::sleep(tokio::time::Duration::from_secs(poll_secs)).await;
        }
    });

    // Real-time changelog notifications via pg_notify (#63)
    let notify_state = Arc::clone(&state);
    tokio::spawn(async move {
        // Create the trigger function and trigger so INSERTs notify us immediately
        let setup = sqlx::query(
            "CREATE OR REPLACE FUNCTION spiral.spiral_changelog_notify()
             RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
               PERFORM pg_notify('spiral_changelog', json_build_object(
                 'event_id',    NEW.event_id,
                 'base_view',   NEW.base_view,
                 'scope_values', NEW.scope_values,
                 't_start',     EXTRACT(EPOCH FROM NEW.t_start)::bigint,
                 't_end',       EXTRACT(EPOCH FROM NEW.t_end)::bigint
               )::text);
               RETURN NEW;
             END;
             $$;
             DROP TRIGGER IF EXISTS changelog_notify_trigger ON spiral.changelog;
             CREATE TRIGGER changelog_notify_trigger
               AFTER INSERT ON spiral.changelog
               FOR EACH ROW EXECUTE FUNCTION spiral.spiral_changelog_notify();",
        )
        .execute(&notify_state.pool)
        .await;

        if let Err(e) = setup {
            tracing::warn!(
                "pg_notify trigger setup failed (falling back to polling): {}",
                e
            );
            return;
        }
        tracing::info!("pg_notify trigger installed on spiral.changelog");

        let mut listener = match PgListener::connect_with(&notify_state.pool).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("PgListener connect failed: {}", e);
                return;
            }
        };

        if let Err(e) = listener.listen("spiral_changelog").await {
            tracing::warn!("PgListener listen failed: {}", e);
            return;
        }
        tracing::info!("PgListener ready on spiral_changelog channel");

        loop {
            match listener.recv().await {
                Ok(notification) => {
                    match serde_json::from_str::<ChangelogEntry>(notification.payload()) {
                        Ok(entry) => {
                            if let Ok(mut buf) = notify_state.recent_changelog.lock() {
                                if !buf.iter().any(|e| e.event_id == entry.event_id) {
                                    buf.push_front(entry.clone());
                                    buf.truncate(500);
                                }
                            }
                            let _ = notify_state.tx.send(VortexEvent::ChangelogUpdate(entry));
                        }
                        Err(e) => tracing::warn!("pg_notify parse error: {}", e),
                    }
                }
                Err(e) => {
                    tracing::warn!("PgListener recv error: {} — reconnecting", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    if let Err(e) = listener.listen("spiral_changelog").await {
                        tracing::warn!("PgListener re-listen failed: {}", e);
                        break;
                    }
                }
            }
        }
    });

    let app = Router::new()
        .route("/api/metadata", get(get_all_metadata))
        .route("/api/metadata/{name}", get(get_metadata))
        .route("/api/changelog", get(get_changelog))
        .route("/api/storage/{name}/block/{blkno}", get(get_block_info))
        .route("/api/storage/{name}/pagemap", get(get_pagemap))
        .route("/api/storage/{name}/changelog", get(get_storage_changelog))
        .route("/api/slice/{view_name}", get(get_slice_data))
        .route("/api/explain", post(run_explain))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{}", port).parse::<SocketAddr>().unwrap();
    tracing::info!("Vortex Server starting on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars()
            .next()
            .map(|c| !c.is_ascii_digit())
            .unwrap_or(false)
}

async fn get_block_info(
    State(state): State<Arc<AppState>>,
    Path((name, blkno)): Path<(String, i32)>,
) -> Result<Json<BlockInfo>, (axum::http::StatusCode, String)> {
    if !valid_identifier(&name) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid view name".into(),
        ));
    }

    let row = sqlx::query(
        "SELECT spiral_blkno_to_tenant_range(oid::int, $2) as info FROM pg_class WHERE relname = $1"
    )
    .bind(&name)
    .bind(blkno)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or((axum::http::StatusCode::NOT_FOUND, "Relation not found".into()))?;

    let info_val: Option<serde_json::Value> = row.get("info");
    let Some(info) = info_val else {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            "Block info not available".into(),
        ));
    };

    let mut block_info = serde_json::from_value::<BlockInfo>(info)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Look up the time column from spiral metadata
    let meta_row =
        sqlx::query("SELECT base_view, columns_metadata FROM spiral.metadata WHERE view_name = $1")
            .bind(&name)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut time_col = meta_row.as_ref().and_then(|row| {
        let cm: serde_json::Value = row.get("columns_metadata");
        cm.get("time_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    if time_col.is_none()
        && let Some(ref row) = meta_row
    {
        let base_view: String = row.get("base_view");
        if let Ok(Some(base_meta)) =
            sqlx::query("SELECT columns_metadata FROM spiral.metadata WHERE view_name = $1")
                .bind(&base_view)
                .fetch_optional(&state.pool)
                .await
        {
            let cm: serde_json::Value = base_meta.get("columns_metadata");
            time_col = cm
                .get("time_column")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    let time_col = time_col.unwrap_or_else(|| "t".to_string());

    // time_col comes from our own metadata — safe to interpolate after identifier check
    if valid_identifier(&time_col) {
        let time_sql = format!(
            "SELECT 
                (SELECT extract(epoch from \"{tc}\")::bigint FROM \"{view}\" WHERE ctid >= '({blkno},0)'::tid AND ctid < '({next_blkno},0)'::tid ORDER BY \"{tc}\" ASC LIMIT 1),
                (SELECT extract(epoch from \"{tc}\")::bigint FROM \"{view}\" WHERE ctid >= '({blkno},0)'::tid AND ctid < '({next_blkno},0)'::tid ORDER BY \"{tc}\" DESC LIMIT 1)",
            tc = time_col,
            view = name,
            blkno = blkno,
            next_blkno = blkno + 1
        );

        match sqlx::query(&time_sql).fetch_one(&state.pool).await {
            Ok(time_row) => {
                let t_min: Option<i64> = time_row.get(0);
                let t_max: Option<i64> = time_row.get(1);
                block_info.t_actual_start = t_min.unwrap_or(0);
                block_info.t_actual_end = t_max.unwrap_or(0);
            }
            Err(e) => {
                tracing::error!("time_sql failed: {}", e);
            }
        }
    }

    // Count pending changelog entries overlapping this block's time range
    if block_info.t_actual_start > 0 && block_info.t_actual_end > 0 {
        let base_view_opt = sqlx::query_scalar::<_, String>(
            "SELECT base_view FROM spiral.metadata WHERE view_name = $1",
        )
        .bind(&name)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();

        if let Some(base_view) = base_view_opt
            && let Ok(cl_row) = sqlx::query(
                "SELECT COUNT(*)::int, COALESCE(MAX(t_end), 0)::bigint
                 FROM spiral.changelog
                 WHERE base_view = $1
                   AND t_start <= $2
                   AND t_end >= $3",
            )
            .bind(&base_view)
            .bind(block_info.t_actual_end)
            .bind(block_info.t_actual_start)
            .fetch_one(&state.pool)
            .await
        {
            let count: i32 = cl_row.get(0);
            let last_ts: i64 = cl_row.get(1);
            block_info.pending_changes = count;
            block_info.is_stale = count > 0;
            block_info.last_changelog_ts = last_ts;
        }
    }

    // Raw SpiralPageOpaque from disk for corruption detection (#61)
    if let Ok(Some(opaque_row)) = sqlx::query(
        "SELECT spiral_read_page_opaque(oid::int, $2) AS opaque FROM pg_class WHERE relname = $1",
    )
    .bind(&name)
    .bind(blkno)
    .fetch_optional(&state.pool)
    .await
    {
        if let Some(v) = opaque_row.get::<Option<serde_json::Value>, _>("opaque") {
            if v.get("found").and_then(|x| x.as_bool()).unwrap_or(false) {
                block_info.opaque_window_start_t = v
                    .get("window_start_t")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                block_info.opaque_window_end_t =
                    v.get("window_end_t").and_then(|x| x.as_i64()).unwrap_or(0);
                block_info.opaque_tenant_scale =
                    v.get("tenant_scale").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
                block_info.magic_valid = v
                    .get("magic_valid")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
            }
        }
    }

    // Physical fill stats: live/unused 8-byte slots in this spiral page
    if let Ok(Some(fill_row)) = sqlx::query(
        "SELECT spiral_page_fill_stats(oid::int, $2) AS stats FROM pg_class WHERE relname = $1",
    )
    .bind(&name)
    .bind(blkno)
    .fetch_optional(&state.pool)
    .await
    {
        if let Some(stats_val) = fill_row.get::<Option<serde_json::Value>, _>("stats") {
            block_info.fill_pct = stats_val
                .get("fill_pct")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            block_info.live_tuples = stats_val
                .get("live_tuples")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            block_info.dead_tuples = stats_val
                .get("dead_tuples")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            block_info.unused_slots = stats_val
                .get("unused_slots")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
        }
    }

    // Spiral gap page: valid spiral magic but all data slots zero (never written).
    // Created by the allocation loop in spiral_pack_delta* when skipping to a target blkno.
    block_info.is_gap_page = block_info.magic_valid && block_info.live_tuples == 0;

    Ok(Json(block_info))
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PageTimeRange {
    blkno: i32,
    t_start: i64,
    t_end: i64,
}

async fn get_pagemap(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Vec<PageTimeRange>>, (axum::http::StatusCode, String)> {
    if !valid_identifier(&name) {
        return Err((axum::http::StatusCode::BAD_REQUEST, "Invalid view name".into()));
    }

    let meta_row =
        sqlx::query("SELECT base_view, columns_metadata FROM spiral.metadata WHERE view_name = $1")
            .bind(&name)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut time_col = meta_row.as_ref().and_then(|row| {
        let cm: serde_json::Value = row.get("columns_metadata");
        cm.get("time_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    if time_col.is_none()
        && let Some(ref row) = meta_row
    {
        let base_view: String = row.get("base_view");
        if let Ok(Some(base_meta)) =
            sqlx::query("SELECT columns_metadata FROM spiral.metadata WHERE view_name = $1")
                .bind(&base_view)
                .fetch_optional(&state.pool)
                .await
        {
            let cm: serde_json::Value = base_meta.get("columns_metadata");
            time_col = cm
                .get("time_column")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    let time_col = time_col.unwrap_or_else(|| "t".to_string());
    if !valid_identifier(&time_col) {
        return Err((axum::http::StatusCode::BAD_REQUEST, "Invalid time column".into()));
    }

    let sql = format!(
        "SELECT (ctid::text::point)[0]::int AS blkno, \
         extract(epoch from min(\"{tc}\"))::bigint AS t_start, \
         extract(epoch from max(\"{tc}\"))::bigint AS t_end \
         FROM \"{view}\" GROUP BY 1 ORDER BY 1",
        tc = time_col,
        view = name,
    );

    let rows = sqlx::query(&sql)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let result = rows
        .iter()
        .map(|row| PageTimeRange {
            blkno: row.get::<i32, _>("blkno"),
            t_start: row.get::<i64, _>("t_start"),
            t_end: row.get::<i64, _>("t_end"),
        })
        .collect();

    Ok(Json(result))
}

async fn get_all_metadata(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Metadata>>, (axum::http::StatusCode, String)> {
    let rows = sqlx::query_as::<_, Metadata>(
        "SELECT view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM spiral.metadata ORDER BY frame_seconds ASC"
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(rows))
}

async fn get_metadata(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Metadata>, (axum::http::StatusCode, String)> {
    let row = sqlx::query_as::<_, Metadata>(
        "SELECT view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM spiral.metadata WHERE view_name = $1"
    )
    .bind(name)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or((axum::http::StatusCode::NOT_FOUND, "Metadata not found".into()))?;

    Ok(Json(row))
}

async fn get_changelog(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ChangelogEntry>>, (axum::http::StatusCode, String)> {
    let rows = state
        .recent_changelog
        .lock()
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .iter()
        .cloned()
        .collect();

    Ok(Json(rows))
}

#[derive(Deserialize)]
struct ChangelogQueryParams {
    limit: Option<i64>,
}

async fn get_storage_changelog(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(params): Query<ChangelogQueryParams>,
) -> Result<Json<Vec<ChangelogEntryFull>>, (axum::http::StatusCode, String)> {
    if !valid_identifier(&name) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid view name".into(),
        ));
    }
    let limit = params.limit.unwrap_or(100).min(500);
    let rows = sqlx::query_as::<_, ChangelogEntryFull>(
        "SELECT event_id, base_view, scope_values, t_start, t_end,
                GREATEST(0, EXTRACT(EPOCH FROM NOW())::bigint - COALESCE(t_end, 0)) AS age_seconds
         FROM spiral.changelog
         WHERE base_view = $1
         ORDER BY event_id DESC
         LIMIT $2",
    )
    .bind(&name)
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(rows))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    tracing::info!("New WebSocket connection request");
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    tracing::info!("WebSocket upgraded");

    let (mut sender, mut _receiver) = socket.split();

    if let Ok(config) = load_system_config(&state.pool).await {
        // Send SystemConfig first — fast (metadata only, no per-view stats queries).
        if let Ok(msg) = serde_json::to_string(&VortexEvent::SystemConfig(config.clone())) {
            let _ = sender.send(Message::Text(msg.into())).await;
        }

        // Batch all storage stats in one round-trip.
        let all_view_names: Vec<String> = config.hierarchies.iter().flat_map(|h| {
            std::iter::once(h.raw_view_name.clone())
                .chain(h.aggregation_levels.iter().map(|l| l.view_name.clone()))
        }).collect();

        let mut all_stats = load_all_storage_stats(&state.pool, &all_view_names).await;

        for hierarchy in &config.hierarchies {
            for view_name in std::iter::once(&hierarchy.raw_view_name)
                .chain(hierarchy.aggregation_levels.iter().map(|l| &l.view_name))
            {
                if let Some(mut stats) = all_stats.remove(view_name) {
                    stats.base_view = hierarchy.base_view.clone();
                    if let Ok(msg) = serde_json::to_string(&VortexEvent::StorageStats(stats)) {
                        let _ = sender.send(Message::Text(msg.into())).await;
                    }
                }
            }
        }
    }

    // Send last known worker status so client doesn't wait for next poll cycle
    let worker_update_msg = state.last_worker_update.lock().ok().and_then(|wu| {
        let (workers, summary) = wu.clone();
        serde_json::to_string(&VortexEvent::WorkerUpdate { workers, summary }).ok()
    });
    if let Some(msg) = worker_update_msg {
        let _ = sender.send(Message::Text(msg.into())).await;
    }

    let mut rx = state.tx.subscribe();
    while let Ok(event) = rx.recv().await {
        if let Ok(msg) = serde_json::to_string(&event)
            && sender.send(Message::Text(msg.into())).await.is_err()
        {
            tracing::info!("WebSocket client disconnected");
            break;
        }
    }
}

fn log_db_error(context: &str, e: &sqlx::Error) {
    let msg = e.to_string();
    if msg.contains("lock timeout") || msg.contains("55P03") {
        tracing::warn!(
            "Lock timeout in {context} — another session holds a lock. Will retry next poll cycle."
        );
    } else if msg.contains("statement timeout") || msg.contains("57014") {
        tracing::warn!("Statement timeout in {context} — query exceeded 30s limit.");
    } else {
        tracing::error!("DB error in {context}: {e}");
    }
}

async fn load_metadata(pool: &Pool<Postgres>) -> Result<Vec<Metadata>, sqlx::Error> {
    sqlx::query_as::<_, Metadata>(
        "SELECT view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata
         FROM spiral.metadata
         ORDER BY base_view ASC, frame_seconds ASC, view_name ASC",
    )
    .fetch_all(pool)
    .await
}

async fn load_sources(pool: &Pool<Postgres>) -> Result<Vec<SourceInfo>, sqlx::Error> {
    sqlx::query_as::<_, SourceInfo>(
        "SELECT view_name, base_view, frame_seconds, base_column, formula, mat_column,
                rollup_gsub_strategy, metadata
         FROM spiral.sources
         ORDER BY base_view ASC, frame_seconds ASC, view_name ASC, base_column ASC, formula ASC",
    )
    .fetch_all(pool)
    .await
}

/// Fetch storage stats for all named views in a single DB round-trip.
/// Returns only what succeeds — one failing view does not affect others.
async fn load_all_storage_stats(
    pool: &Pool<Postgres>,
    view_names: &[String],
) -> HashMap<String, StorageStats> {
    if view_names.is_empty() {
        return HashMap::new();
    }
    let rows = sqlx::query(
        "SELECT relname::text, spiral_get_storage_stats(oid::int) AS stats, reltuples::float8
         FROM pg_class
         WHERE relname = ANY($1)",
    )
    .bind(view_names)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut result = HashMap::new();
    for row in rows {
        let relname: String = row.get("relname");
        let stats_val: Option<serde_json::Value> = row.get("stats");
        let reltuples: f64 = row.try_get("reltuples").unwrap_or(-1.0);
        if let Some(stats) = stats_val {
            if let Ok(mut s) = serde_json::from_value::<StorageStats>(stats) {
                s.row_count = if reltuples < 0.0 { s.total_rows_capacity } else { reltuples as i64 };
                s.view_name = relname.clone();
                result.insert(relname, s);
            }
        }
    }
    result
}

async fn load_system_config(pool: &Pool<Postgres>) -> Result<SystemConfig, sqlx::Error> {
    let metadata = load_metadata(pool).await?;
    let sources = load_sources(pool).await?;

    let mut metadata_by_base: BTreeMap<String, Vec<Metadata>> = BTreeMap::new();
    for view in metadata {
        metadata_by_base.entry(view.base_view.clone()).or_default().push(view);
    }
    for views in metadata_by_base.values_mut() {
        views.sort_by(|a, b| a.frame_seconds.cmp(&b.frame_seconds).then_with(|| a.view_name.cmp(&b.view_name)));
    }

    // Collect raw view names, then fetch all storage stats in one round-trip.
    let raw_view_names: Vec<String> = metadata_by_base
        .iter()
        .map(|(base_view, views)| {
            views.iter()
                .find(|v| v.view_name == *base_view || v.frame_seconds == 0)
                .or_else(|| views.first())
                .map(|v| v.view_name.clone())
                .unwrap_or_else(|| base_view.clone())
        })
        .collect();
    let all_stats = load_all_storage_stats(pool, &raw_view_names).await;

    let mut sources_by_base: BTreeMap<String, Vec<SourceInfo>> = BTreeMap::new();
    for source in sources {
        sources_by_base.entry(source.base_view.clone()).or_default().push(source);
    }

    let mut hierarchies = Vec::new();
    for (base_view, views) in metadata_by_base {
        let raw_view = views
            .iter()
            .find(|view| view.view_name == base_view || view.frame_seconds == 0)
            .cloned()
            .or_else(|| views.first().cloned());

        let raw_view_name = raw_view.as_ref().map(|v| v.view_name.clone()).unwrap_or_else(|| base_view.clone());
        let time_column = raw_view.as_ref()
            .and_then(|v| v.columns_metadata.get("time_column").and_then(|tc| tc.as_str()))
            .unwrap_or("t")
            .to_string();

        let (tenant_scale, kickoff_epoch) = all_stats
            .get(&raw_view_name)
            .map(|s| (s.tenant_scale, s.kickoff_epoch))
            .unwrap_or((1024, 0));

        let aggregation_levels = views.iter()
            .map(|view| AggregationLevel { frame_seconds: view.frame_seconds, view_name: view.view_name.clone() })
            .collect();

        hierarchies.push(HierarchyConfig {
            base_view: base_view.clone(),
            raw_view_name,
            time_column,
            aggregation_levels,
            tenant_scale,
            sources: sources_by_base.remove(&base_view).unwrap_or_default(),
            kickoff_epoch,
        });
    }

    Ok(SystemConfig { hierarchies })
}

#[derive(Deserialize)]
struct SliceParams {
    t_start: f64,
    t_end: f64,
    limit: Option<i64>,
}

async fn get_slice_data(
    State(state): State<Arc<AppState>>,
    Path(view_name): Path<String>,
    Query(params): Query<SliceParams>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let limit = params.limit.unwrap_or(2000).min(5000);

    // Look up metadata to find time_col and scope_col
    let meta_row = sqlx::query(
        "SELECT columns_metadata, scope_columns, base_view
         FROM spiral.metadata
         WHERE view_name = $1",
    )
    .bind(&view_name)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // view_name must be a registered spiral view — prevents SQL injection
    let meta_row = meta_row.ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            format!("View '{}' not in spiral.metadata", view_name),
        )
    })?;

    let columns_metadata: serde_json::Value = meta_row.get("columns_metadata");
    let scope_columns: Vec<String> = meta_row.get("scope_columns");

    let mut time_col = columns_metadata
        .get("time_column")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if time_col.is_none() {
        let base_view: String = meta_row.get("base_view");
        if let Ok(Some(base_meta)) =
            sqlx::query("SELECT columns_metadata FROM spiral.metadata WHERE view_name = $1")
                .bind(&base_view)
                .fetch_optional(&state.pool)
                .await
        {
            let cm: serde_json::Value = base_meta.get("columns_metadata");
            time_col = cm
                .get("time_column")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    let time_col = time_col.unwrap_or_else(|| "t".to_string());
    let scope_col = scope_columns
        .into_iter()
        .next()
        .unwrap_or_else(|| "tenant_id".to_string());

    // Build query with safe identifiers (all from our own metadata, not user input)
    let sql = format!(
        "SELECT json_agg(row_to_json(d)) FROM (
           SELECT *
           FROM {view_name}
           WHERE {time_col} >= to_timestamp($1)
             AND {time_col} < to_timestamp($2)
           ORDER BY {time_col}, {scope_col}
           LIMIT $3
         ) d",
        time_col = time_col,
        view_name = view_name,
    );

    let row = sqlx::query(&sql)
        .bind(params.t_start)
        .bind(params.t_end)
        .bind(limit)
        .fetch_one(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let agg: Option<serde_json::Value> = row.get(0);
    let rows = agg.unwrap_or(serde_json::Value::Array(vec![]));
    let count = rows.as_array().map(|a| a.len()).unwrap_or(0);

    Ok(Json(serde_json::json!({
        "view_name": view_name,
        "time_col": time_col,
        "scope_col": scope_col,
        "count": count,
        "rows": rows,
    })))
}

#[derive(Deserialize)]
struct ExplainRequest {
    query: String,
}

async fn run_explain(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ExplainRequest>,
) -> Json<serde_json::Value> {
    let trimmed = payload.query.trim().to_string();
    let upper = trimmed.to_uppercase();

    if !upper.starts_with("SELECT") && !upper.starts_with("WITH") && !upper.starts_with("EXPLAIN") {
        return Json(serde_json::json!({
            "ok": false,
            "error": "Only SELECT/WITH/EXPLAIN allowed",
            "lines": [],
            "duration_ms": 0,
        }));
    }

    let final_query = if upper.starts_with("EXPLAIN") {
        trimmed
    } else {
        format!("EXPLAIN (ANALYZE, BUFFERS) {}", trimmed)
    };

    let start = std::time::Instant::now();
    let result = sqlx::query(&final_query).fetch_all(&state.pool).await;
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(rows) => {
            let lines: Vec<String> = rows.iter().map(|row| row.get::<String, _>(0)).collect();
            Json(serde_json::json!({
                "ok": true,
                "lines": lines,
                "duration_ms": duration_ms,
            }))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
            "lines": [],
            "duration_ms": duration_ms,
        })),
    }
}
