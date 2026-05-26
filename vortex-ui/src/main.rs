use futures_util::stream::StreamExt;
use gloo_net::websocket::Message;
use gloo_net::websocket::futures::WebSocket;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

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

const HEAP_BYTES_PER_ROW: f64 = 48.0;
const HEAP_ROWS_PER_PAGE: f64 = 8192.0 / HEAP_BYTES_PER_ROW;
const XOR_BLOCK_BYTES_PER_ROW: f64 = 128.0 / 61.0;

fn format_timespan(sec: i32) -> String {
    if sec == 0 {
        return "raw".to_string();
    }
    if sec < 60 {
        format!("{}s", sec)
    } else if sec < 3600 {
        format!("{}m", sec / 60)
    } else if sec < 86400 {
        format!("{}h", sec / 3600)
    } else if sec < 604800 {
        format!("{}d", sec / 86400)
    } else if sec < 2592000 {
        format!("{}w", sec / 604800)
    } else if sec < 31536000 {
        format!("{}mo", sec / 2592000)
    } else {
        format!("{}y", sec / 31536000)
    }
}

fn get_color_for_tenant(id: i32, total: i32) -> String {
    if total <= 1 {
        return "#fbbf24".to_string();
    }
    let ratio = id as f64 / (total - 1) as f64;
    let h = 240.0 - (ratio * 195.0);
    let s = 20.0 + (ratio * 75.0);
    let l = 55.0 - (ratio * 8.0);
    format!("hsl({:.0}, {:.0}%, {:.0}%)", h, s, l)
}

fn format_epoch_seconds(epoch: i64) -> String {
    if epoch == 0 {
        return "—".to_string();
    }
    let date = js_sys::Date::new(&JsValue::from_f64(epoch as f64 * 1000.0));
    // Use ISO format: 2024-01-15T00:00:00Z
    let iso = date.to_iso_string().as_string().unwrap_or_default();
    // Trim to just the datetime without trailing .000Z
    iso.trim_end_matches('Z')
        .trim_end_matches(".000")
        .to_string()
        + "Z"
}

fn format_bytes(kb: i64) -> String {
    if kb >= 1_000_000 {
        format!("{:.1} GB", kb as f64 / 1_000_000.0)
    } else if kb >= 1_000 {
        format!("{:.1} MB", kb as f64 / 1_000.0)
    } else {
        format!("{} KB", kb)
    }
}

fn current_hierarchy(config: &SystemConfig, base_view: &str) -> Option<HierarchyConfig> {
    config
        .hierarchies
        .iter()
        .find(|h| h.base_view == base_view)
        .cloned()
}

#[component]
fn HierarchyTree(
    hierarchy: Signal<Option<HierarchyConfig>>,
    selected_tier: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        {move || {
            let Some(h) = hierarchy.get() else {
                return view! { <div class="empty-state">"no table selected"</div> }.into_any();
            };
            let levels: Vec<_> = h.aggregation_levels.iter()
                .filter(|l| l.frame_seconds > 0)
                .cloned()
                .collect();
            let sources = h.sources.clone();
            let raw_view = h.raw_view_name.clone();
            let active_tier = selected_tier.get();

            view! {
                <div class="tree">
                    <div
                        class=if active_tier.is_none() { "tree-node tree-node-active" } else { "tree-node" }
                        on:click=move |_| selected_tier.set(None)
                        style="cursor:pointer;"
                    >
                        <span class="node-glyph">"▸"</span>
                        <span class="node-tier node-raw">"RAW"</span>
                        <span class="node-name">{raw_view}</span>
                    </div>
                    {levels.into_iter().enumerate().map(|(i, level)| {
                        let sec = level.frame_seconds;
                        let tier_label = format_timespan(sec);
                        let view_name = level.view_name.clone();
                        let vn_click = view_name.clone();
                        let level_sources: Vec<_> = sources.iter()
                            .filter(|s| s.view_name == view_name)
                            .cloned()
                            .collect();
                        let indent = (i + 1) * 10;
                        let is_active = active_tier.as_deref() == Some(&view_name);
                        view! {
                            <div>
                                <div
                                    class=if is_active { "tree-node tree-node-active" } else { "tree-node" }
                                    style={format!("padding-left: {}px; cursor:pointer;", indent)}
                                    on:click=move |_| selected_tier.set(Some(vn_click.clone()))
                                >
                                    <span class="node-glyph">"└"</span>
                                    <span class="node-tier node-agg">{tier_label}</span>
                                    <span class="node-name">{level.view_name.clone()}</span>
                                </div>
                                {(!level_sources.is_empty()).then(|| {
                                    let srcs = level_sources.clone();
                                    view! {
                                        <div>
                                            {srcs.into_iter().map(|s| view! {
                                                <div class="source-row" style={format!("padding-left: {}px", indent + 16)}>
                                                    <span class="src-col">{s.base_column}</span>
                                                    <span class="src-arrow">"→"</span>
                                                    <span class="src-formula">{s.formula}</span>
                                                    <span class="src-mat">{s.mat_column}</span>
                                                </div>
                                            }).collect_view()}
                                        </div>
                                    }
                                })}
                            </div>
                        }
                    }).collect_view()}
                    <div class="tree-footer">
                        <span class="tf-label">"tenant_scale"</span>
                        <span class="tf-value">{h.tenant_scale}</span>
                    </div>
                    {(h.kickoff_epoch > 0).then(|| view! {
                        <div class="tree-footer">
                            <span class="tf-label">"kickoff"</span>
                            <span class="tf-value tf-ts">{format_epoch_seconds(h.kickoff_epoch)}</span>
                        </div>
                    })}
                </div>
            }.into_any()
        }}
    }
}

#[component]
fn PageMap(
    total_pages: Signal<i64>,
    tenant_scale: Signal<i64>,
    selected_page: Signal<Option<i32>>,
    on_click: impl Fn(i32) + 'static + Send + Clone,
) -> impl IntoView {
    view! {
        <div class="page-map">
            {move || {
                let total = total_pages.get();
                let scale = tenant_scale.get().max(1) as i32;
                let sel = selected_page.get();
                (0..total.min(800) as i32).map(|idx| {
                    let color = get_color_for_tenant(idx % scale, scale);
                    let on_click = on_click.clone();
                    let is_sel = sel == Some(idx);
                    view! {
                        <div
                            class=if is_sel { "page-cell selected" } else { "page-cell" }
                            style=format!("background:{}", color)
                            on:click=move |_| on_click(idx)
                            title=format!("Page {}", idx)
                        ></div>
                    }
                }).collect::<Vec<_>>()
            }}
        </div>
    }
}

#[component]
fn BlockInspector(block: BlockInfo, kickoff: i64) -> impl IntoView {
    let start_t = kickoff + block.t_range[0];
    let end_t = kickoff + block.t_range[1];
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

    let align_class = if block.alignment_pct > 99.0 {
        "ivalue good"
    } else if block.alignment_pct > 95.0 {
        "ivalue warn"
    } else {
        "ivalue bad"
    };

    view! {
        <div class="block-inspector">
            <div class="bi-title">
                "PAGE " {block.blkno}
                {block.is_boundary.then(|| view! {
                    <span class="badge-boundary">"BOUNDARY"</span>
                })}
            </div>
            <div class="irow">
                <span class="ilabel">"capacity"</span>
                <span class="ivalue">{block.tuple_count} " slots"</span>
            </div>
            <div class="irow">
                <span class="ilabel">"alignment"</span>
                <span class={align_class}>{format!("{:.1}%", block.alignment_pct)}</span>
            </div>
            <div class="irow">
                <span class="ilabel">"drift"</span>
                <span class={if block.drift_records > 0 { "ivalue warn" } else { "ivalue" }}>
                    {block.drift_records} " rec"
                </span>
            </div>
            <div class="irow">
                <span class="ilabel">"span"</span>
                <span class="ivalue">{duration_fmt}</span>
            </div>
            <div class="irow">
                <span class="ilabel">"t_range"</span>
                <span class="ivalue ivalue-sm">
                    "[" {block.t_range[0]} ".." {block.t_range[1]} "]"
                </span>
            </div>
            <div class="irow">
                <span class="ilabel">"tenants"</span>
                <span class="ivalue">
                    "[" {block.tenant_range[0]} ".." {block.tenant_range[1]} "]"
                </span>
            </div>
            <div class="irow-full">
                <span class="ilabel">"start"</span>
                <div class="ivalue ivalue-ts">{format_epoch_seconds(start_t)}</div>
            </div>
            <div class="irow-full">
                <span class="ilabel">"end"</span>
                <div class="ivalue ivalue-ts">{format_epoch_seconds(end_t)}</div>
            </div>
        </div>
    }
}

#[component]
fn CompressionPanel(stats: Signal<StorageStats>) -> impl IntoView {
    let row_count_exp = RwSignal::new(6.0_f64);

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
        if s.data_per_page > 0 {
            s.data_per_page as f64
        } else {
            1018.0
        }
    };

    view! {
        <div class="cmp-panel">
            // live stats from server
            {move || {
                let s = stats.get();
                (s.spiral_size_kb > 0).then(|| view! {
                    <div class="live-stats">
                        <div class="ls-row">
                            <span class="ls-label">"spiral"</span>
                            <span class="ls-value">{format_bytes(s.spiral_size_kb)}</span>
                        </div>
                        <div class="ls-row">
                            <span class="ls-label">"heap est."</span>
                            <span class="ls-value ls-dim">{format_bytes(s.projected_heap_size_kb)}</span>
                        </div>
                        <div class="ls-row">
                            <span class="ls-label">"ratio"</span>
                            <span class="ls-value ls-green">{format!("{:.1}x", s.compression_ratio)}</span>
                        </div>
                        <div class="ls-row">
                            <span class="ls-label">"rows"</span>
                            <span class="ls-value">{s.total_rows_capacity}</span>
                        </div>
                    </div>
                })
            }}

            // bytes per row bars
            <div class="cmp-section-label">"BYTES / ROW"</div>
            <div class="cmp-bar-row">
                <span class="cmp-lbl">"Heap"</span>
                <div class="cmp-outer"><div class="cmp-inner cmp-heap" style="width:100%;"></div></div>
                <span class="cmp-val">"48 B"</span>
            </div>
            <div class="cmp-bar-row">
                <span class="cmp-lbl">"Spiral"</span>
                <div class="cmp-outer">
                    <div class="cmp-inner cmp-spiral" style={move || {
                        format!("width:{:.1}%;", (spiral_bpr() / HEAP_BYTES_PER_ROW * 100.0).min(100.0))
                    }}></div>
                </div>
                <span class="cmp-val">{move || format!("{:.1} B", spiral_bpr())}</span>
            </div>
            <div class="cmp-bar-row">
                <span class="cmp-lbl">"XOR"</span>
                <div class="cmp-outer">
                    <div class="cmp-inner cmp-xor" style={format!(
                        "width:{:.1}%;", XOR_BLOCK_BYTES_PER_ROW / HEAP_BYTES_PER_ROW * 100.0
                    )}></div>
                </div>
                <span class="cmp-val">"~2.1 B"</span>
            </div>

            // IO tax
            <div class="cmp-section-label" style="margin-top:10px;">"IO TAX — PG / 1K ROWS"</div>
            <div class="three-grid">
                <div class="tg-cell">
                    <span class="tg-label">"HEAP"</span>
                    <span class="tg-value tg-red">{format!("{:.0}", (1000.0_f64 / HEAP_ROWS_PER_PAGE).ceil())}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SPIRAL"</span>
                    <span class="tg-value tg-green">{move || format!("{:.2}", 1000.0_f64 / spiral_rows_per_page())}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SPEEDUP"</span>
                    <span class="tg-value tg-blue">{move || format!("{:.0}x", spiral_rows_per_page() / HEAP_ROWS_PER_PAGE)}</span>
                </div>
            </div>

            // savings calculator
            <div class="cmp-section-label" style="margin-top:10px;">"SAVINGS CALCULATOR"</div>
            <div class="slider-row">
                <input
                    type="range" min="3" max="12" step="0.05"
                    on:input=move |ev| {
                        let val: f64 = event_target_value(&ev).parse().unwrap_or(6.0);
                        row_count_exp.set(val);
                    }
                    prop:value=move || row_count_exp.get().to_string()
                />
                <span class="slider-label">{move || {
                    let n = 10.0_f64.powf(row_count_exp.get());
                    if n >= 1e9 { format!("{:.1}B rows", n / 1e9) }
                    else if n >= 1e6 { format!("{:.0}M rows", n / 1e6) }
                    else { format!("{:.0}K rows", n / 1e3) }
                }}</span>
            </div>
            <div class="three-grid">
                <div class="tg-cell">
                    <span class="tg-label">"HEAP"</span>
                    <span class="tg-value tg-red">{move || {
                        let gb = 10.0_f64.powf(row_count_exp.get()) * HEAP_BYTES_PER_ROW / (1024.0 * 1024.0 * 1024.0);
                        if gb < 1.0 { format!("{:.0}MB", gb * 1024.0) } else { format!("{:.1}GB", gb) }
                    }}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SPIRAL"</span>
                    <span class="tg-value tg-green">{move || {
                        let gb = 10.0_f64.powf(row_count_exp.get()) * spiral_bpr() / (1024.0 * 1024.0 * 1024.0);
                        if gb < 1.0 { format!("{:.0}MB", gb * 1024.0) } else { format!("{:.1}GB", gb) }
                    }}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SAVED"</span>
                    <span class="tg-value tg-green">{move || format!("{:.1}%", (1.0 - spiral_bpr() / HEAP_BYTES_PER_ROW) * 100.0)}</span>
                </div>
            </div>
        </div>
    }
}

#[component]
fn App() -> impl IntoView {
    let stats_by_base = RwSignal::new(BTreeMap::<String, StorageStats>::new());
    let config = RwSignal::new(SystemConfig::default());
    let selected_base_view = RwSignal::new(String::new());
    let selected_tier_view = RwSignal::new(Option::<String>::None); // None = raw
    let last_event = RwSignal::new(None::<String>);
    let selected_block = RwSignal::new(None::<BlockInfo>);
    let selected_page_no = RwSignal::new(None::<i32>);
    let is_connected = RwSignal::new(false);

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

    let ws_url_clone = Arc::clone(&ws_url);
    Effect::new(move |_| {
        let ws_url = ws_url_clone.clone();
        console_log!("Connecting to {}", ws_url);
        match WebSocket::open(&ws_url) {
            Ok(mut ws) => {
                is_connected.set(true);
                leptos::task::spawn_local(async move {
                    while let Some(msg) = ws.next().await {
                        if let Ok(Message::Text(text)) = msg {
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
                                            .map(|h| h.base_view.clone())
                                            .unwrap_or_default();
                                        if !c.hierarchies.iter().any(|h| h.base_view == selected) {
                                            selected_base_view.set(first_base);
                                            selected_block.set(None);
                                            selected_page_no.set(None);
                                        }
                                        config.set(c);
                                    }
                                    VortexEvent::ChangelogUpdate(entry) => {
                                        last_event.set(Some(format!(
                                            "#{} {} @ {}",
                                            entry.event_id, entry.base_view, entry.t_start
                                        )));
                                    }
                                },
                                Err(_) => {}
                            }
                        }
                    }
                    is_connected.set(false);
                });
            }
            Err(e) => console_log!("WebSocket error: {:?}", e),
        }
    });

    let api_base_clone = Arc::clone(&api_base);
    let fetch_block_info = move |blkno: i32| {
        let selected = selected_base_view.get_untracked();
        // Use selected tier view if set, otherwise fall back to base view's raw view
        let view_name = selected_tier_view
            .get_untracked()
            .unwrap_or_else(|| {
                stats_by_base
                    .get_untracked()
                    .get(&selected)
                    .map(|s| s.view_name.clone())
                    .unwrap_or_default()
            });
        if view_name.is_empty() {
            return;
        }
        selected_page_no.set(Some(blkno));
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

    let current_stats = Signal::derive(move || {
        stats_by_base
            .get()
            .get(&selected_base_view.get())
            .cloned()
            .unwrap_or_default()
    });

    let current_hierarchy_opt = Signal::derive(move || {
        current_hierarchy(&config.get(), &selected_base_view.get())
    });

    let tenant_scale = Signal::derive(move || {
        current_hierarchy_opt
            .get()
            .map(|h| h.tenant_scale.max(1))
            .unwrap_or(1)
    });

    let kickoff = Signal::derive(move || {
        current_hierarchy_opt
            .get()
            .map(|h| h.kickoff_epoch)
            .unwrap_or(0)
    });

    view! {
        <div id="app">
            <header class="topbar">
                <div class="brand">"VORTEX"</div>
                <div class="tab-row">
                    {move || {
                        let current_config = config.get();
                        if current_config.hierarchies.is_empty() {
                            leptos::either::Either::Left(view! {
                                <span class="no-tables">"connecting..."</span>
                            })
                        } else {
                            leptos::either::Either::Right(
                                current_config.hierarchies.into_iter().map(|h| {
                                    let t = h.base_view.clone();
                                    let is_active = selected_base_view.get() == t;
                                    view! {
                                        <button
                                            class=if is_active { "tab tab-active" } else { "tab" }
                                            on:click=move |_| {
                                                selected_base_view.set(t.clone());
                                                selected_block.set(None);
                                                selected_page_no.set(None);
                                            }
                                        >{h.base_view.to_uppercase()}</button>
                                    }
                                }).collect_view()
                            )
                        }
                    }}
                </div>
                <div class="topbar-spacer"></div>
                <div class="topbar-stats">
                    <div class="ts-item">
                        <span class="ts-label">"PAGES"</span>
                        <span class="ts-value">{move || current_stats.get().total_pages}</span>
                    </div>
                    <div class="ts-item">
                        <span class="ts-label">"COMP"</span>
                        <span class="ts-value ts-green">{move || {
                            let r = current_stats.get().compression_ratio;
                            if r > 0.0 { format!("{:.1}x", r) } else { "—".into() }
                        }}</span>
                    </div>
                    <div class="ts-item">
                        <span class="ts-label">"TENANTS"</span>
                        <span class="ts-value">{move || current_stats.get().tenant_scale}</span>
                    </div>
                </div>
                <div class=move || if is_connected.get() { "conn-dot conn-live" } else { "conn-dot conn-dead" }>
                    {move || if is_connected.get() { "● LIVE" } else { "○ OFFLINE" }}
                </div>
            </header>

            <div class="main-grid">
                <aside class="left-panel">
                    <div class="panel-block">
                        <div class="panel-hdr">"HIERARCHY"</div>
                        <HierarchyTree hierarchy=current_hierarchy_opt selected_tier=selected_tier_view />
                    </div>
                    <div class="panel-block">
                        <div class="panel-hdr">"STORAGE ANALYSIS"</div>
                        <CompressionPanel stats=current_stats />
                    </div>
                </aside>

                <div class="center-panel">
                    <div class="center-hdr">
                        <span class="panel-hdr" style="border:none; margin:0; padding:0;">
                            {move || {
                                let tier = selected_tier_view.get();
                                match tier {
                                    None => "PAGE MAP".to_string(),
                                    Some(ref v) => format!("PAGE MAP — {}", v.to_uppercase()),
                                }
                            }}
                        </span>
                        <span class="page-meta">
                            {move || {
                                let s = current_stats.get();
                                format!("{} pages · {} tenant scale", s.total_pages, s.tenant_scale)
                            }}
                        </span>
                    </div>
                    <PageMap
                        total_pages=Signal::derive(move || current_stats.get().total_pages)
                        tenant_scale=tenant_scale
                        selected_page=Signal::derive(move || selected_page_no.get())
                        on_click=fetch_block_info
                    />
                    <div class="ticker">
                        {move || last_event.get().unwrap_or_else(|| {
                            if is_connected.get() {
                                "waiting for data...".into()
                            } else {
                                "connecting...".into()
                            }
                        })}
                    </div>
                </div>

                <aside class="right-panel">
                    {move || {
                        let block = selected_block.get();
                        let k = kickoff.get();
                        match block {
                            Some(b) => leptos::either::Either::Left(view! {
                                <div class="panel-block">
                                    <div class="panel-hdr">"BLOCK INSPECTOR"</div>
                                    <BlockInspector block=b kickoff=k />
                                </div>
                            }),
                            None => leptos::either::Either::Right(view! {
                                <div class="empty-state">"Click a page cell to inspect"</div>
                            }),
                        }
                    }}
                </aside>
            </div>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
