use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, Pool, Postgres, Row};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
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
    t_start: i64,
    t_end: i64,
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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct StorageStats {
    #[serde(default)]
    base_view: String,
    #[serde(default)]
    view_name: String,
    total_pages: i64,
    total_rows_capacity: i64,
    tenant_scale: i64,
    spiral_size_kb: i64,
    projected_heap_size_kb: i64,
    compression_ratio: f64,
    kickoff_epoch: i64,
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

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");

    ensure_changelog_event_ids(&pool)
        .await
        .expect("Failed to prepare changelog schema");

    let (tx, _rx) = broadcast::channel(100);

    let state = Arc::new(AppState {
        pool: pool.clone(),
        tx: tx.clone(),
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
                        for entry in entries {
                            last_processed_event_id = last_processed_event_id.max(entry.event_id);
                            let _ = polling_state.tx.send(VortexEvent::ChangelogUpdate(entry));
                        }
                    }
                }
                Err(e) => tracing::error!("Changelog polling error: {}", e),
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
                        match load_storage_stats(
                            &polling_state.pool,
                            &hierarchy.base_view,
                            &hierarchy.raw_view_name,
                        )
                        .await
                        {
                            Ok(Some(storage_stats)) => {
                                let _ = polling_state
                                    .tx
                                    .send(VortexEvent::StorageStats(storage_stats));
                            }
                            Ok(None) => {}
                            Err(e) => tracing::error!(
                                "Storage stats polling error for {}: {}",
                                hierarchy.base_view,
                                e
                            ),
                        }
                    }
                }
                Err(e) => tracing::error!("Metadata polling error: {}", e),
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    });

    let app = Router::new()
        .route("/api/metadata", get(get_all_metadata))
        .route("/api/metadata/{name}", get(get_metadata))
        .route("/api/changelog", get(get_changelog))
        .route("/api/storage/{name}/block/{blkno}", get(get_block_info))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = "0.0.0.0:3001".parse::<SocketAddr>().unwrap();
    tracing::info!("Vortex Server starting on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn get_block_info(
    State(state): State<Arc<AppState>>,
    Path((name, blkno)): Path<(String, i32)>,
) -> Result<Json<BlockInfo>, (axum::http::StatusCode, String)> {
    let row = sqlx::query(
        "SELECT spiral_blkno_to_tenant_range(oid::int, $2) as info FROM pg_class WHERE relname = $1"
    )
    .bind(name)
    .bind(blkno)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or((axum::http::StatusCode::NOT_FOUND, "Relation not found".into()))?;

    let info_val: Option<serde_json::Value> = row.get("info");
    if let Some(info) = info_val {
        let block_info = serde_json::from_value::<BlockInfo>(info)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(block_info))
    } else {
        Err((
            axum::http::StatusCode::NOT_FOUND,
            "Block info not available".into(),
        ))
    }
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
    let rows = sqlx::query_as::<_, ChangelogEntry>(
        "SELECT event_id, base_view, scope_values, t_start, t_end
         FROM spiral.changelog
         ORDER BY event_id DESC
         LIMIT 100",
    )
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
        if let Ok(msg) = serde_json::to_string(&VortexEvent::SystemConfig(config.clone())) {
            let _ = sender.send(Message::Text(msg.into())).await;
        }

        for hierarchy in &config.hierarchies {
            if let Ok(Some(storage_stats)) =
                load_storage_stats(&state.pool, &hierarchy.base_view, &hierarchy.raw_view_name)
                    .await
            {
                if let Ok(msg) = serde_json::to_string(&VortexEvent::StorageStats(storage_stats)) {
                    let _ = sender.send(Message::Text(msg.into())).await;
                }
            }
        }
    }

    let mut rx = state.tx.subscribe();
    while let Ok(event) = rx.recv().await {
        if let Ok(msg) = serde_json::to_string(&event) {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                tracing::info!("WebSocket client disconnected");
                break;
            }
        }
    }
}

async fn ensure_changelog_event_ids(pool: &Pool<Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE SEQUENCE IF NOT EXISTS spiral.changelog_event_id_seq")
        .execute(pool)
        .await?;
    sqlx::query("ALTER TABLE spiral.changelog ADD COLUMN IF NOT EXISTS event_id BIGINT")
        .execute(pool)
        .await?;
    sqlx::query(
        "ALTER TABLE spiral.changelog
         ALTER COLUMN event_id SET DEFAULT nextval('spiral.changelog_event_id_seq')",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE spiral.changelog
         SET event_id = nextval('spiral.changelog_event_id_seq')
         WHERE event_id IS NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_spiral_changelog_event_id
         ON spiral.changelog (event_id)",
    )
    .execute(pool)
    .await?;
    Ok(())
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

    let mut storage_stats = match serde_json::from_value::<StorageStats>(stats) {
        Ok(storage_stats) => storage_stats,
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

        let raw_view_name = views
            .iter()
            .find(|view| view.view_name == base_view || view.frame_seconds == 0)
            .map(|view| view.view_name.clone())
            .or_else(|| views.first().map(|view| view.view_name.clone()))
            .unwrap_or_else(|| base_view.clone());

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
            aggregation_levels,
            tenant_scale,
            sources: sources_by_base.remove(&base_view).unwrap_or_default(),
            kickoff_epoch,
        });
    }

    Ok(SystemConfig { hierarchies })
}
