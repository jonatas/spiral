use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, Pool, Postgres, Row};
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
    base_column: String,
    formula: String,
    mat_column: String,
}

#[derive(Serialize, Deserialize, Debug, FromRow, Clone)]
struct ChangelogEntry {
    base_view: String,
    scope_values: serde_json::Value,
    t_start: i64,
    t_end: i64,
    processed: bool,
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
    total_pages: i64,
    total_rows_capacity: i64,
    tenant_scale: i64,
    spiral_size_kb: i64,
    projected_heap_size_kb: i64,
    compression_ratio: f64,
    kickoff_epoch: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct SystemConfig {
    aggregation_levels: Vec<i32>,
    tenant_scale: i64,
    sources: Vec<SourceInfo>,
    kickoff_epoch: i64,
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

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://localhost/postgres".into());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");

    let (tx, _rx) = broadcast::channel(100);

    let state = Arc::new(AppState {
        pool: pool.clone(),
        tx: tx.clone(),
    });

    // Start background polling task
    let polling_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut last_processed_t = 0i64;
        let mut last_metadata_count = 0;
        
        tracing::info!("Starting background polling loop");
        loop {
            // 1. Poll for new changelog entries
            let result = sqlx::query_as::<_, ChangelogEntry>(
                "SELECT base_view, scope_values, t_start, t_end, processed FROM spiral.changelog WHERE t_start > $1 ORDER BY t_start ASC"
            )
            .bind(last_processed_t)
            .fetch_all(&polling_state.pool)
            .await;

            match result {
                Ok(entries) => {
                    if !entries.is_empty() {
                        tracing::info!("Found {} new changelog entries since {}", entries.len(), last_processed_t);
                        for entry in entries {
                            last_processed_t = last_processed_t.max(entry.t_start);
                            let _ = polling_state.tx.send(VortexEvent::ChangelogUpdate(entry));
                        }
                    }
                }
                Err(e) => tracing::error!("Changelog polling error: {}", e),
            }

            // 2. Poll for storage stats and metadata
            let metadata_result = sqlx::query_as::<_, Metadata>(
                "SELECT view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM spiral.metadata ORDER BY frame_seconds ASC"
            )
            .fetch_all(&polling_state.pool)
            .await;

            if let Ok(views) = metadata_result {
                // If metadata changed, broadcast new system config
                if views.len() != last_metadata_count {
                    last_metadata_count = views.len();
                    let levels: Vec<i32> = views.iter().map(|v| v.frame_seconds).collect();
                    
                    let sources = sqlx::query_as::<_, SourceInfo>(
                        "SELECT view_name, base_column, formula, mat_column FROM spiral.sources"
                    )
                    .fetch_all(&polling_state.pool)
                    .await
                    .unwrap_or_default();

                    let mut tenant_scale = 1024;
                    let mut kickoff_epoch = 0;
                    if let Some(first) = views.first() {
                        let scale_query = sqlx::query(
                            "SELECT (spiral_get_storage_stats(oid::int)->>'tenant_scale')::bigint as scale, (spiral_get_storage_stats(oid::int)->>'kickoff_epoch')::bigint as kickoff FROM pg_class WHERE relname = $1"
                        )
                        .bind(&first.view_name)
                        .fetch_optional(&polling_state.pool)
                        .await;
                        
                        if let Ok(Some(row)) = scale_query {
                            tenant_scale = row.get("scale");
                            kickoff_epoch = row.get("kickoff");
                        }
                    }

                    let _ = polling_state.tx.send(VortexEvent::SystemConfig(SystemConfig {
                        aggregation_levels: levels,
                        tenant_scale,
                        sources,
                        kickoff_epoch,
                    }));
                }

                for view in views {
                    let stats_result = sqlx::query(
                        "SELECT spiral_get_storage_stats(oid::int) as stats FROM pg_class WHERE relname = $1"
                    )
                    .bind(&view.view_name)
                    .fetch_optional(&polling_state.pool)
                    .await;

                    match stats_result {
                        Ok(Some(row)) => {
                            let stats_val: Option<serde_json::Value> = row.get("stats");
                            if let Some(stats) = stats_val {
                                if let Ok(storage_stats) = serde_json::from_value::<StorageStats>(stats) {
                                    let _ = polling_state.tx.send(VortexEvent::StorageStats(storage_stats));
                                }
                            }
                        }
                        _ => {}
                    }
                }
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
        Err((axum::http::StatusCode::NOT_FOUND, "Block info not available".into()))
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
        "SELECT base_view, scope_values, t_start, t_end, processed FROM spiral.changelog ORDER BY t_start DESC LIMIT 100"
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(rows))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    tracing::info!("New WebSocket connection request");
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    tracing::info!("WebSocket upgraded");
    
    // Send initial config if available
    let views = sqlx::query_as::<_, Metadata>(
        "SELECT view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM spiral.metadata ORDER BY frame_seconds ASC"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let sources = sqlx::query_as::<_, SourceInfo>(
        "SELECT view_name, base_column, formula, mat_column FROM spiral.sources"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut tenant_scale = 1024;
    let mut kickoff_epoch = 0;
    if let Some(first) = views.first() {
        let scale_query = sqlx::query(
            "SELECT (spiral_get_storage_stats(oid::int)->>'tenant_scale')::bigint as scale, (spiral_get_storage_stats(oid::int)->>'kickoff_epoch')::bigint as kickoff FROM pg_class WHERE relname = $1"
        )
        .bind(&first.view_name)
        .fetch_optional(&state.pool)
        .await;
        
        if let Ok(Some(row)) = scale_query {
            tenant_scale = row.get("scale");
            kickoff_epoch = row.get("kickoff");
        }
    }

    let levels: Vec<i32> = views.iter().map(|v| v.frame_seconds).collect();
    let event = VortexEvent::SystemConfig(SystemConfig {
        aggregation_levels: levels,
        tenant_scale,
        sources,
        kickoff_epoch,
    });

    let (mut sender, mut _receiver) = socket.split();

    if let Ok(msg) = serde_json::to_string(&event) {
        let _ = sender.send(Message::Text(msg.into())).await;
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
