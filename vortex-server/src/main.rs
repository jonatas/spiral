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
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, Pool, Postgres, Row};
use std::collections::{BTreeMap, VecDeque};
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
#[serde(tag = "type", content = "data")]
enum VortexEvent {
    ChangelogUpdate(ChangelogEntry),
    StorageStats(StorageStats),
    SystemConfig(SystemConfig),
}

struct AppState {
    pool: Pool<Postgres>,
    tx: broadcast::Sender<VortexEvent>,
    recent_changelog: Mutex<VecDeque<ChangelogEntry>>,
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
    });

    // Start background polling task
    let polling_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut last_processed_event_id = 0i64;
        let mut last_config_signature: Option<String> = None;

        tracing::info!("Starting background polling loop");
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
                        
                        if let Ok(mut buffer) = polling_state.recent_changelog.lock() {
                            for entry in &entries {
                                buffer.push_front(entry.clone());
                            }
                            buffer.truncate(500);
                        }

                        for entry in entries {
                            last_processed_event_id = last_processed_event_id.max(entry.event_id);
                            let _ = polling_state.tx.send(VortexEvent::ChangelogUpdate(entry));
                        }
                    }
                }
                Err(e) => log_db_error("changelog poll", &e),
            }

            // 2. Poll for storage stats and metadata
            match load_system_config(&polling_state.pool).await {
                Ok(config) => {
                    let signature = serde_json::to_string(&config).unwrap_or_default();
                    if last_config_signature.as_deref() != Some(signature.as_str()) {
                        last_config_signature = Some(signature);
                        let _ = polling_state
                            .tx
                            .send(VortexEvent::SystemConfig(config.clone()));
                    }

                    for hierarchy in &config.hierarchies {
                        // Polling for raw view
                        if let Ok(Some(storage_stats)) = load_storage_stats(
                            &polling_state.pool,
                            &hierarchy.base_view,
                            &hierarchy.raw_view_name,
                        )
                        .await
                        {
                            let _ = polling_state
                                .tx
                                .send(VortexEvent::StorageStats(storage_stats));
                        }

                        // Polling for aggregation levels
                        for level in &hierarchy.aggregation_levels {
                            if let Ok(Some(storage_stats)) = load_storage_stats(
                                &polling_state.pool,
                                &hierarchy.base_view,
                                &level.view_name,
                            )
                            .await
                            {
                                let _ = polling_state
                                    .tx
                                    .send(VortexEvent::StorageStats(storage_stats));
                            }
                        }
                    }
                }
                Err(e) => log_db_error("metadata poll", &e),
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    });

    let app = Router::new()
        .route("/api/metadata", get(get_all_metadata))
        .route("/api/metadata/{name}", get(get_metadata))
        .route("/api/changelog", get(get_changelog))
        .route("/api/storage/{name}/block/{blkno}", get(get_block_info))
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
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().map(|c| !c.is_ascii_digit()).unwrap_or(false)
}

async fn get_block_info(
    State(state): State<Arc<AppState>>,
    Path((name, blkno)): Path<(String, i32)>,
) -> Result<Json<BlockInfo>, (axum::http::StatusCode, String)> {
    if !valid_identifier(&name) {
        return Err((axum::http::StatusCode::BAD_REQUEST, "Invalid view name".into()));
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
        return Err((axum::http::StatusCode::NOT_FOUND, "Block info not available".into()));
    };

    let mut block_info = serde_json::from_value::<BlockInfo>(info)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Look up the time column from spiral metadata
    let meta_row = sqlx::query(
        "SELECT base_view, columns_metadata FROM spiral.metadata WHERE view_name = $1",
    )
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
        if let Ok(Some(base_meta)) = sqlx::query(
            "SELECT columns_metadata FROM spiral.metadata WHERE view_name = $1",
        )
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
            "SELECT extract(epoch from min(\"{tc}\"))::bigint, \
                    extract(epoch from max(\"{tc}\"))::bigint \
             FROM \"{view}\" \
             WHERE (ctid::text::point)[0]::int = $1",
            tc = time_col,
            view = name,
        );

        match sqlx::query(&time_sql)
            .bind(blkno)
            .fetch_one(&state.pool)
            .await
        {
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

    Ok(Json(block_info))
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

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    tracing::info!("New WebSocket connection request");
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    tracing::info!("WebSocket upgraded");

    let (mut sender, mut _receiver) = socket.split();

    if let Ok(config) = load_system_config(&state.pool).await {
        if let Ok(msg) = serde_json::to_string(&VortexEvent::SystemConfig(config.clone())) {
            let _ = sender.send(Message::Text(msg.into())).await;
        }

        for hierarchy in &config.hierarchies {
            // Stats for raw view
            if let Ok(Some(storage_stats)) =
                load_storage_stats(&state.pool, &hierarchy.base_view, &hierarchy.raw_view_name)
                    .await
                && let Ok(msg) = serde_json::to_string(&VortexEvent::StorageStats(storage_stats))
            {
                let _ = sender.send(Message::Text(msg.into())).await;
            }

            // Stats for all aggregation levels
            for level in &hierarchy.aggregation_levels {
                if let Ok(Some(storage_stats)) =
                    load_storage_stats(&state.pool, &hierarchy.base_view, &level.view_name).await
                    && let Ok(msg) = serde_json::to_string(&VortexEvent::StorageStats(storage_stats))
                {
                    let _ = sender.send(Message::Text(msg.into())).await;
                }
            }
        }
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
        tracing::warn!("Lock timeout in {context} — another session holds a lock. Will retry next poll cycle.");
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

async fn load_storage_stats(
    pool: &Pool<Postgres>,
    base_view: &str,
    raw_view_name: &str,
) -> Result<Option<StorageStats>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT spiral_get_storage_stats(oid::int) as stats
         FROM pg_class
         WHERE relname = $1",
    )
    .bind(raw_view_name)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let stats_val: Option<serde_json::Value> = row.get("stats");
    let Some(stats) = stats_val else {
        return Ok(None);
    };

    let reltuples: f32 = sqlx::query("SELECT reltuples FROM pg_class WHERE relname = $1")
        .bind(raw_view_name)
        .fetch_one(pool)
        .await?
        .get(0);

    let mut storage_stats = match serde_json::from_value::<StorageStats>(stats) {
        Ok(mut storage_stats) => {
            storage_stats.row_count = if reltuples < 0.0 {
                storage_stats.total_rows_capacity
            } else {
                reltuples as i64
            };
            storage_stats
        }
        Err(_) => return Ok(None),
    };

    storage_stats.base_view = base_view.to_string();
    storage_stats.view_name = raw_view_name.to_string();
    Ok(Some(storage_stats))
}

async fn load_system_config(pool: &Pool<Postgres>) -> Result<SystemConfig, sqlx::Error> {
    let metadata = load_metadata(pool).await?;
    let sources = load_sources(pool).await?;

    let mut metadata_by_base: BTreeMap<String, Vec<Metadata>> = BTreeMap::new();
    for view in metadata {
        metadata_by_base
            .entry(view.base_view.clone())
            .or_default()
            .push(view);
    }

    let mut sources_by_base: BTreeMap<String, Vec<SourceInfo>> = BTreeMap::new();
    for source in sources {
        sources_by_base
            .entry(source.base_view.clone())
            .or_default()
            .push(source);
    }

    let mut hierarchies = Vec::new();
    for (base_view, mut views) in metadata_by_base {
        views.sort_by(|a, b| {
            a.frame_seconds
                .cmp(&b.frame_seconds)
                .then_with(|| a.view_name.cmp(&b.view_name))
        });

        let raw_view = views
            .iter()
            .find(|view| view.view_name == base_view || view.frame_seconds == 0)
            .cloned()
            .or_else(|| views.first().cloned());

        let raw_view_name = raw_view
            .as_ref()
            .map(|v| v.view_name.clone())
            .unwrap_or_else(|| base_view.clone());

        let time_column = raw_view
            .as_ref()
            .and_then(|v| {
                v.columns_metadata
                    .get("time_column")
                    .and_then(|tc| tc.as_str())
            })
            .unwrap_or("t")
            .to_string();

        let storage_stats = load_storage_stats(pool, &base_view, &raw_view_name).await?;
        let tenant_scale = storage_stats
            .as_ref()
            .map(|stats| stats.tenant_scale)
            .unwrap_or(1024);
        let kickoff_epoch = storage_stats
            .as_ref()
            .map(|stats| stats.kickoff_epoch)
            .unwrap_or(0);

        let aggregation_levels = views
            .iter()
            .map(|view| AggregationLevel {
                frame_seconds: view.frame_seconds,
                view_name: view.view_name.clone(),
            })
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
        (axum::http::StatusCode::NOT_FOUND, format!("View '{}' not in spiral.metadata", view_name))
    })?;

    let columns_metadata: serde_json::Value = meta_row.get("columns_metadata");
    let scope_columns: Vec<String> = meta_row.get("scope_columns");

    let mut time_col = columns_metadata
        .get("time_column")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if time_col.is_none() {
        let base_view: String = meta_row.get("base_view");
        if let Ok(Some(base_meta)) = sqlx::query(
            "SELECT columns_metadata FROM spiral.metadata WHERE view_name = $1",
        )
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
           SELECT *, extract(epoch from {time_col})::bigint AS t_epoch
           FROM {view_name}
           WHERE {time_col} >= to_timestamp($1)
             AND {time_col} < to_timestamp($2)
           ORDER BY {time_col}
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

    if !upper.starts_with("SELECT")
        && !upper.starts_with("WITH")
        && !upper.starts_with("EXPLAIN")
    {
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
            let lines: Vec<String> = rows
                .iter()
                .map(|row| row.get::<String, _>(0))
                .collect();
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
