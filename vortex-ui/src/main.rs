use futures_util::stream::StreamExt;
use gloo_net::websocket::Message;
use gloo_net::websocket::futures::WebSocket;
use gloo_utils::format::JsValueSerdeExt;
use leptos::html::Canvas;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, MouseEvent};

macro_rules! console_log {
    ($($t:tt)*) => (web_sys::console::log_1(&format!($($t)*).into()))
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ChangelogEntry {
    event_id: i64,
    base_view: String,
    scope_values: serde_json::Value,
    t_start: i64,
    t_end: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
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
    #[serde(default)]
    data_per_page: i64,
    #[serde(default)]
    page_size: i64,
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

#[component]
fn Heatmap<F>(pages: Signal<i64>, on_click: F) -> impl IntoView
where
    F: Fn(i32) + 'static + Send + Clone,
{
    view! {
        <div class="heatmap-container">
            {move || (0..pages.get().min(100)).map(|i| {
                let idx = i as i32;
                let on_click = on_click.clone();
                view! {
                    <div
                        class="page-block"
                        on:click=move |_| on_click(idx)
                        title=format!("Page {}", idx)
                    ></div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

const HEAP_BYTES_PER_ROW: f64 = 48.0;
const HEAP_ROWS_PER_PAGE: f64 = 8192.0 / HEAP_BYTES_PER_ROW; // ~170
const XOR_BLOCK_BYTES_PER_ROW: f64 = 128.0 / 61.0; // 8B first_val + 60×2B deltas

#[component]
fn Dashboard(stats: Signal<StorageStats>) -> impl IntoView {
    let row_count_exp = RwSignal::new(6.0_f64); // 10^6 = 1M default

    let spiral_bpr = move || {
        let s = stats.get();
        if s.total_rows_capacity > 0 && s.spiral_size_kb > 0 {
            (s.spiral_size_kb * 1024) as f64 / s.total_rows_capacity as f64
        } else {
            8.0
        }
    };

    let spiral_rows_per_page = move || {
        let s = stats.get();
        if s.data_per_page > 0 { s.data_per_page as f64 } else { 1018.0 }
    };

    view! {
        <div class="dashboard">
            <div class="inspector-title">"Storage Compression Analysis"</div>

            // --- Bytes per row bars ---
            <div style="margin: 10px 0 4px 0;">
                <div class="stat-label" style="margin-bottom: 6px; font-size: 0.55rem; letter-spacing: 1px;">"BYTES PER ROW"</div>
                <div class="cmp-bar-row">
                    <span class="cmp-bar-label">"Heap"</span>
                    <div class="cmp-bar-outer">
                        <div class="cmp-bar-inner cmp-heap" style="width:100%;"></div>
                    </div>
                    <span class="cmp-bar-val">"48 B"</span>
                </div>
                <div class="cmp-bar-row">
                    <span class="cmp-bar-label">"Spiral"</span>
                    <div class="cmp-bar-outer">
                        <div class="cmp-bar-inner cmp-spiral" style={move || {
                            format!("width:{:.1}%;", (spiral_bpr() / HEAP_BYTES_PER_ROW * 100.0).min(100.0))
                        }}></div>
                    </div>
                    <span class="cmp-bar-val">{move || format!("{:.1} B", spiral_bpr())}</span>
                </div>
                <div class="cmp-bar-row">
                    <span class="cmp-bar-label">"XOR-Blk"</span>
                    <div class="cmp-bar-outer">
                        <div class="cmp-bar-inner cmp-xor" style={format!(
                            "width:{:.1}%;", XOR_BLOCK_BYTES_PER_ROW / HEAP_BYTES_PER_ROW * 100.0
                        )}></div>
                    </div>
                    <span class="cmp-bar-val">"~2.1 B"</span>
                </div>
            </div>

            // --- IO Tax ---
            <div style="margin-top: 10px; padding-top: 8px; border-top: 1px dashed rgba(255,255,255,0.08);">
                <div class="stat-label" style="margin-bottom: 6px; font-size: 0.55rem; letter-spacing: 1px;">"IO TAX — PAGES PER 1,000 ROWS"</div>
                <div class="stats-grid">
                    <div class="stat-mini">
                        <div class="stat-label">"HEAP"</div>
                        <div class="stat-value" style="color: #f87171;">
                            {format!("{:.0}", (1000.0_f64 / HEAP_ROWS_PER_PAGE).ceil())}
                        </div>
                    </div>
                    <div class="stat-mini">
                        <div class="stat-label">"SPIRAL"</div>
                        <div class="stat-value" style="color: #4ade80;">
                            {move || format!("{:.2}", 1000.0_f64 / spiral_rows_per_page())}
                        </div>
                    </div>
                    <div class="stat-mini">
                        <div class="stat-label">"SPEEDUP"</div>
                        <div class="stat-value" style="color: #818cf8;">
                            {move || format!("{:.0}x", spiral_rows_per_page() / HEAP_ROWS_PER_PAGE)}
                        </div>
                    </div>
                </div>
            </div>

            // --- Savings Calculator ---
            <div style="margin-top: 10px; padding-top: 8px; border-top: 1px dashed rgba(255,255,255,0.08);">
                <div class="stat-label" style="margin-bottom: 6px; font-size: 0.55rem; letter-spacing: 1px;">"SAVINGS CALCULATOR"</div>
                <div style="display:flex; align-items:center; gap:8px; margin-bottom:6px;">
                    <input
                        type="range" min="3" max="12" step="0.05"
                        style="flex:1; accent-color: var(--primary-color); cursor:pointer;"
                        on:input=move |ev| {
                            let val: f64 = event_target_value(&ev).parse().unwrap_or(6.0);
                            row_count_exp.set(val);
                        }
                        prop:value=move || row_count_exp.get().to_string()
                    />
                    <span style="font-family:'JetBrains Mono'; font-size:0.7rem; color:var(--primary-color); min-width:65px; text-align:right;">
                        {move || {
                            let n = 10.0_f64.powf(row_count_exp.get());
                            if n >= 1_000_000_000.0 { format!("{:.1}B rows", n / 1_000_000_000.0) }
                            else if n >= 1_000_000.0 { format!("{:.0}M rows", n / 1_000_000.0) }
                            else { format!("{:.0}K rows", n / 1_000.0) }
                        }}
                    </span>
                </div>
                <div class="stats-grid">
                    <div class="stat-mini">
                        <div class="stat-label">"HEAP"</div>
                        <div class="stat-value" style="color:#f87171;">{move || {
                            let gb = 10.0_f64.powf(row_count_exp.get()) * HEAP_BYTES_PER_ROW / (1024.0 * 1024.0 * 1024.0);
                            if gb < 1.0 { format!("{:.0} MB", gb * 1024.0) } else { format!("{:.1} GB", gb) }
                        }}</div>
                    </div>
                    <div class="stat-mini">
                        <div class="stat-label">"SPIRAL"</div>
                        <div class="stat-value" style="color:#4ade80;">{move || {
                            let gb = 10.0_f64.powf(row_count_exp.get()) * spiral_bpr() / (1024.0 * 1024.0 * 1024.0);
                            if gb < 1.0 { format!("{:.0} MB", gb * 1024.0) } else { format!("{:.1} GB", gb) }
                        }}</div>
                    </div>
                    <div class="stat-mini">
                        <div class="stat-label">"SAVED"</div>
                        <div class="stat-value" style="color:#4ade80;">{move || {
                            let pct = (1.0 - spiral_bpr() / HEAP_BYTES_PER_ROW) * 100.0;
                            format!("{:.1}%", pct)
                        }}</div>
                    </div>
                </div>
            </div>

            // --- 1B Projection ---
            <div class="projection">
                <div class="stat-label">"PROJECTION — 1 BILLION ROWS"</div>
                <div class="stats-grid">
                    <div class="stat-mini">
                        <div class="stat-label">"HEAP"</div>
                        <div class="stat-value" style="color:#f87171;">{move || {
                            let gb = 1_000_000_000.0 * HEAP_BYTES_PER_ROW / (1024.0 * 1024.0 * 1024.0);
                            format!("~{:.0} GB", gb)
                        }}</div>
                    </div>
                    <div class="stat-mini">
                        <div class="stat-label">"SPIRAL"</div>
                        <div class="stat-value">{move || {
                            let gb = 1_000_000_000.0 * spiral_bpr() / (1024.0 * 1024.0 * 1024.0);
                            format!("~{:.1} GB", gb)
                        }}</div>
                    </div>
                    <div class="stat-mini">
                        <div class="stat-label">"XOR-BLK"</div>
                        <div class="stat-value" style="color:#818cf8;">{move || {
                            let gb = 1_000_000_000.0 * XOR_BLOCK_BYTES_PER_ROW / (1024.0 * 1024.0 * 1024.0);
                            format!("~{:.1} GB", gb)
                        }}</div>
                    </div>
                </div>
            </div>
        </div>
    }
}

fn get_color_for_tenant(id: i32, total: i32) -> String {
    if total <= 1 {
        return "#fbbf24".to_string();
    }
    let ratio = id as f64 / (total - 1) as f64;
    let h = 240.0 - (ratio * 195.0);
    let s = 5.0 + (ratio * 91.0);
    let l = 65.0 - (ratio * 9.0);
    format!("hsl({}, {}%, {}%)", h, s, l)
}

fn format_epoch_seconds(epoch: i64) -> String {
    if epoch == 0 {
        return "0".to_string();
    }
    let date = js_sys::Date::new(&JsValue::from_f64(epoch as f64 * 1000.0));
    date.to_utc_string().as_string().unwrap_or_default()
}

fn current_hierarchy(config: &SystemConfig, base_view: &str) -> Option<HierarchyConfig> {
    config
        .hierarchies
        .iter()
        .find(|hierarchy| hierarchy.base_view == base_view)
        .cloned()
}

#[component]
fn App() -> impl IntoView {
    let canvas_ref = NodeRef::<Canvas>::new();
    let stats_by_base = RwSignal::new(BTreeMap::<String, StorageStats>::new());
    let config = RwSignal::new(SystemConfig::default());
    let selected_base_view = RwSignal::new(String::new());
    let last_event = RwSignal::new(None::<String>);
    let selected_block = RwSignal::new(None::<BlockInfo>);
    let is_connected = RwSignal::new(false);
    let is_paused = RwSignal::new(false);
    let hovered_lane = RwSignal::new(None::<usize>);
    let resume_timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);

    // Dynamic host resolution
    let hostname = web_sys::window()
        .map(|w| {
            w.location()
                .hostname()
                .unwrap_or_else(|_| "localhost".to_string())
        })
        .unwrap_or_else(|| "localhost".to_string());

    let server_port = option_env!("VORTEX_SERVER_PORT").unwrap_or("3001");
    let ws_url = Arc::new(format!("ws://{}:{}/ws", hostname, server_port));
    let api_base = Arc::new(format!("http://{}:{}", hostname, server_port));

    // Handle WebSocket
    let ws_url_clone = Arc::clone(&ws_url);
    Effect::new(move |_| {
        let ws_url = ws_url_clone.clone();
        console_log!("Connecting to Vortex WebSocket at {}", ws_url);
        let ws_result = WebSocket::open(&ws_url);
        match ws_result {
            Ok(mut ws) => {
                console_log!("WebSocket opened successfully");
                is_connected.set(true);
                leptos::task::spawn_local(async move {
                    while let Some(msg) = ws.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                match serde_json::from_str::<VortexEvent>(&text) {
                                    Ok(event) => match event {
                                        VortexEvent::StorageStats(s) => {
                                            stats_by_base.update(|stats| {
                                                stats.insert(s.base_view.clone(), s);
                                            });
                                        }
                                        VortexEvent::SystemConfig(c) => {
                                            let selected = selected_base_view.get_untracked();
                                            let first_base = c
                                                .hierarchies
                                                .first()
                                                .map(|hierarchy| hierarchy.base_view.clone())
                                                .unwrap_or_default();
                                            if !c
                                                .hierarchies
                                                .iter()
                                                .any(|h| h.base_view == selected)
                                            {
                                                selected_base_view.set(first_base);
                                                selected_block.set(None);
                                            }
                                            config.set(c);
                                        }
                                        VortexEvent::ChangelogUpdate(entry) => {
                                            last_event.set(Some(format!(
                                                "Update #{}: {} @ {}",
                                                entry.event_id, entry.base_view, entry.t_start
                                            )));
                                        }
                                    },
                                    Err(e) => {
                                        web_sys::console::error_2(
                                            &"Failed to parse event:".into(),
                                            &text.into(),
                                        );
                                        web_sys::console::error_1(
                                            &format!("Serde error: {:?}", e).into(),
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    is_connected.set(false);
                });
            }
            Err(e) => console_log!("Failed to open WebSocket: {:?}", e),
        }
    });

    let api_base_clone = Arc::clone(&api_base);
    let fetch_block_info = move |blkno: i32| {
        let selected = selected_base_view.get_untracked();
        let view_name = stats_by_base
            .get_untracked()
            .get(&selected)
            .map(|stats| stats.view_name.clone())
            .unwrap_or_default();
        if view_name.is_empty() {
            return;
        }
        let url = format!(
            "{}/api/storage/{}/block/{}",
            api_base_clone, view_name, blkno
        );
        leptos::task::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await {
                if let Ok(info) = resp.json::<BlockInfo>().await {
                    selected_block.set(Some(info));
                }
            }
        });
    };

    // Animation Loop
    Effect::new(move |_| {
        if let Some(canvas) = canvas_ref.get() {
            let ctx = canvas
                .get_context("2d")
                .unwrap()
                .expect("Should have context")
                .dyn_into::<CanvasRenderingContext2d>()
                .unwrap();

            let canvas_el: HtmlCanvasElement = canvas.into();
            let width = canvas_el.width() as f64;
            let height = canvas_el.height() as f64;
            let center_x = width / 2.0;
            let center_y = height / 2.0;

            let mut angle = 0.0;

            let mut render = move || {
                // Use untracked access to avoid warnings in the high-frequency loop
                if is_paused.get_untracked() {
                    return;
                }

                let selected = selected_base_view.get_untracked();
                let hierarchy = current_hierarchy(&config.get_untracked(), &selected);
                let levels = hierarchy
                    .as_ref()
                    .map(|hierarchy| hierarchy.aggregation_levels.clone())
                    .unwrap_or_default();
                let tenant_scale = hierarchy
                    .as_ref()
                    .map(|hierarchy| hierarchy.tenant_scale.max(1))
                    .unwrap_or(1);

                ctx.set_fill_style_str("rgba(15, 17, 23, 0.2)");
                ctx.fill_rect(0.0, 0.0, width, height);

                // SLOWER ROTATION (0.002 as requested)
                angle += 0.002;

                // Draw Lanes
                let num_lanes = levels.len() + 1;
                for i in 0..num_lanes {
                    // Radius grows to accommodate more tiers
                    let r = 50.0 + (i as f64 * 40.0);
                    let is_hovered = hovered_lane.get_untracked() == Some(i);

                    ctx.begin_path();
                    ctx.set_line_width(if is_hovered { 3.0 } else { 1.0 });
                    ctx.set_stroke_style_str(if is_hovered {
                        "rgba(92, 158, 255, 0.5)"
                    } else {
                        "rgba(255, 255, 255, 0.05)"
                    });
                    let dash = vec![5.0, 10.0];
                    let dash_js = JsValue::from_serde(&dash).unwrap();
                    ctx.set_line_dash(&dash_js).unwrap();
                    ctx.arc(center_x, center_y, r, 0.0, PI * 2.0).unwrap();
                    ctx.stroke();

                    // Lane Labels
                    ctx.set_fill_style_str(if is_hovered {
                        "#5c9eff"
                    } else {
                        "rgba(255, 255, 255, 0.2)"
                    });
                    ctx.set_font(if is_hovered {
                        "bold 10px 'JetBrains Mono'"
                    } else {
                        "8px 'JetBrains Mono'"
                    });

                    let label = if i == 0 {
                        "RAW".to_string()
                    } else {
                        let sec = levels.get(i - 1).map(|l| l.frame_seconds).unwrap_or(0);
                        if sec < 3600 {
                            format!("{}M", sec / 60)
                        } else if sec < 86400 {
                            format!("{}H", sec / 3600)
                        } else if sec < 604800 {
                            format!("{}D", sec / 86400)
                        } else if sec < 2592000 {
                            format!("{}W", sec / 604800)
                        } else if sec < 31536000 {
                            format!("{}MO", sec / 2592000)
                        } else {
                            format!("{}Y", sec / 31536000)
                        }
                    };
                    ctx.fill_text(&label, center_x + r + 5.0, center_y).unwrap();
                }
                let reset_dash = vec![0.0, 0.0];
                let reset_dash_js = JsValue::from_serde(&reset_dash).unwrap();
                ctx.set_line_dash(&reset_dash_js).unwrap();

                // Draw Physical Page Dots (mapping actual storage pages)
                let num_pages = stats_by_base
                    .get_untracked()
                    .get(&selected)
                    .map(|stats| stats.total_pages)
                    .unwrap_or(0);
                // Represent every page as a dot, but limit for performance
                let max_dots = 150.min(num_pages);

                for i in 0..max_dots {
                    let page_idx = i as i32;
                    // Normalized age that "opens slowly" with time
                    let age_offset = i as f64 / max_dots as f64;
                    let age = (age_offset + (angle * 0.1)) % 1.0;

                    // Radius maps to the age (distance from center)
                    let r = 40.0 + age * 420.0;

                    // Phase represents tenant mapping inside the page
                    let phase = (page_idx as f64 * PI * 2.0) / 16.0;
                    // Spiral rotation
                    let theta = age * PI * 14.0 + phase + angle;

                    let x = center_x + r * theta.cos();
                    let y = center_y + r * theta.sin();

                    // Use tenant scale for color interpolation if relevant, otherwise use page index
                    let color_val = (page_idx as i64 % tenant_scale) as i32;
                    let color = get_color_for_tenant(color_val, tenant_scale as i32);

                    ctx.set_fill_style_str(&color);
                    ctx.begin_path();
                    // Dot size based on records in page (hypothetical fullness)
                    ctx.arc(x, y, 2.5, 0.0, PI * 2.0).unwrap();
                    ctx.fill();

                    // Page Reference Glow
                    ctx.set_shadow_blur(8.0);
                    ctx.set_shadow_color(&color);
                    ctx.fill();
                    ctx.set_shadow_blur(0.0);
                }

                // Draw Dynamic Spiral Thread (Subtle background path)
                ctx.set_stroke_style_str("rgba(92, 158, 255, 0.03)");
                ctx.set_line_width(1.0);
                ctx.begin_path();
                for i in 0..2000 {
                    let t = i as f64 / 2000.0;
                    let r = 30.0 + t * 480.0;
                    let theta = t * PI * 20.0 + angle;
                    let x = center_x + r * theta.cos();
                    let y = center_y + r * theta.sin();
                    if i == 0 {
                        ctx.move_to(x, y);
                    } else {
                        ctx.line_to(x, y);
                    }
                }
                ctx.stroke();
            };

            gloo_timers::callback::Interval::new(16, move || {
                render();
            })
            .forget();
        }
    });

    let on_container_mouse_move = move |e: MouseEvent| {
        // Pause animation when mouse enters or moves in the visualization area
        resume_timer.update_value(|v| *v = None);
        is_paused.set(true);

        if let Some(canvas) = canvas_ref.get_untracked() {
            let rect = canvas.get_bounding_client_rect();

            let scale_x = canvas.width() as f64 / rect.width();
            let scale_y = canvas.height() as f64 / rect.height();

            let x = (e.client_x() as f64 - rect.left()) * scale_x;
            let y = (e.client_y() as f64 - rect.top()) * scale_y;

            let center_x = canvas.width() as f64 / 2.0;
            let center_y = canvas.height() as f64 / 2.0;
            let dist = ((x - center_x).powi(2) + (y - center_y).powi(2)).sqrt();

            let mut found_lane = None;
            let selected = selected_base_view.get_untracked();
            let levels = current_hierarchy(&config.get_untracked(), &selected)
                .map(|hierarchy| hierarchy.aggregation_levels)
                .unwrap_or_default();

            let num_lanes = levels.len() + 1;
            for i in 0..num_lanes {
                let r = 50.0 + (i as f64 * 40.0);
                if (dist - r).abs() < 25.0 {
                    found_lane = Some(i);
                    break;
                }
            }
            hovered_lane.set(found_lane);
        }
    };

    let on_container_mouse_leave = move |_| {
        hovered_lane.set(None);
        let timeout = gloo_timers::callback::Timeout::new(3000, move || {
            is_paused.set(false);
        });
        resume_timer.update_value(|v| *v = Some(timeout));
    };

    view! {
        <div id="canvas-container"
            on:mousemove=on_container_mouse_move
            on:mouseleave=on_container_mouse_leave
        >
            <canvas
                node_ref=canvas_ref
                width="1000"
                height="1000"
            />
            <div class="overlay">
                <span class="badge">"V2.0-VORTEX"</span>
                <h1>"Spiral Monitor" {move || is_paused.get().then(|| " [PAUSED]")}</h1>

                <div class="table-switcher">
                    {move || {
                        let current_config = config.get();
                        if current_config.hierarchies.is_empty() {
                            leptos::either::Either::Left(view! { <div style="font-size: 0.6rem; color: var(--gray-color);">"NO TABLES FOUND"</div> })
                        } else {
                            leptos::either::Either::Right(current_config.hierarchies.into_iter().map(|hierarchy| {
                                let t = hierarchy.base_view.clone();
                                let is_active = selected_base_view.get() == t;
                                view! {
                                    <button
                                        class=move || if is_active { "switcher-btn active" } else { "switcher-btn" }
                                        on:click=move |_| {
                                            selected_base_view.set(t.clone());
                                            selected_block.set(None);
                                        }
                                    >{hierarchy.base_view.to_uppercase()}</button>
                                }
                            }).collect_view())
                        }
                    }}
                </div>

                <div class="stats-container">
                    <div class="stat-item">
                        <span class="stat-label">"PAGES"</span>
                        <span class="stat-value">{move || {
                            stats_by_base
                                .get()
                                .get(&selected_base_view.get())
                                .map(|stats| stats.total_pages)
                                .unwrap_or(0)
                        }}</span>
                    </div>
                    <div class="stat-item">
                        <span class="stat-label">"COMPRESSION"</span>
                        <span class="stat-value">{move || {
                            let ratio = stats_by_base
                                .get()
                                .get(&selected_base_view.get())
                                .map(|stats| stats.compression_ratio)
                                .unwrap_or(0.0);
                            format!("{:.1}x", ratio)
                        }}</span>
                    </div>
                </div>

                {move || hovered_lane.get().map(|lane_idx| {
                    let hierarchy = current_hierarchy(&config.get(), &selected_base_view.get());
                    let Some(hierarchy) = hierarchy else {
                        return view! { <div></div> }.into_any();
                    };
                    let levels = hierarchy.aggregation_levels.clone();

                    let sec = if lane_idx == 0 { 0 } else { levels.get(lane_idx-1).map(|l| l.frame_seconds).unwrap_or(0) };
                    let label = if lane_idx == 0 { "RAW STORAGE".to_string() } else {
                        if sec < 3600 { format!("{} MINUTE ROLLUP", sec/60) }
                        else if sec < 86400 { format!("{} HOUR ROLLUP", sec/3600) }
                        else if sec < 604800 { format!("{} DAY ROLLUP", sec/86400) }
                        else if sec < 2592000 { format!("{} WEEK ROLLUP", sec/604800) }
                        else if sec < 31536000 { format!("{} MONTH ROLLUP", sec/2592000) }
                        else { format!("{} YEAR ROLLUP", sec/31536000) }
                    };

                    let lane_sources: Vec<_> = hierarchy.sources.iter()
                        .filter(|s| {
                            if lane_idx == 0 {
                                false
                            } else {
                                let view_name = levels.get(lane_idx-1).map(|l| l.view_name.as_str()).unwrap_or("");
                                s.view_name == view_name
                            }
                        })
                        .cloned()
                        .collect();

                    view! {
                        <div class="inspector" style="border-left-color: var(--primary-color);">
                            <div class="inspector-title" style="color: var(--primary-color);">{label}</div>
                            <div class="stat-item">
                                <span class="stat-label">"TENANT SCALE"</span>
                                <span class="stat-value">{hierarchy.tenant_scale}</span>
                            </div>
                            {(!lane_sources.is_empty()).then(|| {
                                let sources = lane_sources.clone();
                                view! {
                                    <div style="margin-top: 5px; font-size: 0.6rem; color: rgba(255,255,255,0.4);">
                                        "AGGREGATIONS:"
                                        {sources.into_iter().map(|s| {
                                            let base = s.base_column.clone();
                                            let formula = s.formula.clone();
                                            let mat = s.mat_column.clone();
                                            view! { <div style="margin-left: 5px;">"• " {base} " -> " {formula} " (" {mat} ")"</div> }
                                        }).collect_view()}
                                    </div>
                                }
                            })}
                        </div>
                    }.into_any()
                })}

                <div class="ticker">
                    {move || {
                        if let Some(event) = last_event.get() {
                            event
                        } else if is_connected.get() {
                            "WAITING FOR DATA...".to_string()
                        } else {
                            "CONNECTING...".to_string()
                        }
                    }}
                </div>

                <Heatmap pages=Signal::derive(move || {
                    stats_by_base
                        .get()
                        .get(&selected_base_view.get())
                        .map(|stats| stats.total_pages)
                        .unwrap_or(0)
                }) on_click=fetch_block_info />

                {move || selected_block.get().map(|info| {
                    let kickoff = current_hierarchy(&config.get(), &selected_base_view.get())
                        .map(|hierarchy| hierarchy.kickoff_epoch)
                        .unwrap_or(0);
                    let start_t = kickoff + info.t_range[0];
                    let end_t = kickoff + info.t_range[1];
                    let duration_sec = (end_t - start_t).abs();

                    let duration_fmt = if duration_sec < 60 {
                        format!("{}s", duration_sec)
                    } else if duration_sec < 3600 {
                        format!("{:.1}m", duration_sec as f64 / 60.0)
                    } else if duration_sec < 86400 {
                        format!("{:.1}h", duration_sec as f64 / 3600.0)
                    } else {
                        format!("{:.1}d", duration_sec as f64 / 86400.0)
                    };

                    view! {
                        <div class="inspector">
                            <div class="inspector-title">
                                "Page " {info.blkno} " Inspector "
                                {info.is_boundary.then(|| view! { <span class="badge" style="background: var(--secondary-color); color: white;">"BOUNDARY"</span> })}
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"CAPACITY"</span>
                                <span class="stat-value">{info.tuple_count} " slots"</span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"ALIGNMENT"</span>
                                <span class="stat-value" style=format!("color: {}", if info.alignment_pct > 99.0 { "#4ade80" } else { "#fbbf24" })>
                                    {format!("{:.1}%", info.alignment_pct)}
                                </span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label" title="Drift in records relative to 1s bucket">"DRIFT"</span>
                                <span class="stat-value" style="color: #fbbf24;">{info.drift_records} " records"</span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"TIME SPAN"</span>
                                <span class="stat-value">{duration_fmt}</span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"START"</span>
                                <span class="stat-value" style="font-size: 0.6rem;">{format_epoch_seconds(start_t)}</span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"END"</span>
                                <span class="stat-value" style="font-size: 0.6rem;">{format_epoch_seconds(end_t)}</span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"T_RANGE (REL)"</span>
                                <span class="stat-value">"[" {info.t_range[0]} ".." {info.t_range[1]} "]"</span>
                            </div>
                            <div class="stat-item">
                                <span class="stat-label">"TENANTS"</span>
                                <span class="stat-value">"[" {info.tenant_range[0]} ".." {info.tenant_range[1]} "]"</span>
                            </div>
                        </div>
                    }
                })}

                <Dashboard stats=Signal::derive(move || {
                    stats_by_base
                        .get()
                        .get(&selected_base_view.get())
                        .cloned()
                        .unwrap_or_default()
                }) />
            </div>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
