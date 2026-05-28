use gloo_net::websocket::futures::WebSocket;
use futures_util::StreamExt;
use gloo_net::websocket::Message;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use wasm_bindgen::JsValue;

macro_rules! console_log {
    ($($t:tt)*) => (web_sys::console::log_1(&format!($($t)*).into()))
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct StorageStats {
    base_view: String,
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
    data_per_page: i64,
    #[serde(default)]
    page_size: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct AggregationLevel {
    frame_seconds: i32,
    view_name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct HierarchyConfig {
    base_view: String,
    raw_view_name: String,
    #[serde(default)]
    time_column: String,
    aggregation_levels: Vec<AggregationLevel>,
    tenant_scale: i64,
    sources: Vec<SourceInfo>,
    kickoff_epoch: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct SystemConfig {
    hierarchies: Vec<HierarchyConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", content = "data")]
enum VortexEvent {
    ChangelogUpdate(ChangelogEntry),
    StorageStats(StorageStats),
    SystemConfig(SystemConfig),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct SliceResponse {
    view_name: String,
    #[serde(default)]
    time_col: String,
    #[serde(default)]
    scope_col: String,
    count: usize,
    rows: Vec<serde_json::Value>,
    #[serde(skip)]
    fetch_ms: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct ExplainResult {
    ok: bool,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    duration_ms: f64,
    #[serde(default)]
    error: String,
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
        let years = sec as f64 / 31536000.0;
        if years < 1.1 { "1 year".into() } else { format!("{:.1}y", years) }
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
    if epoch <= 0 {
        return "—".to_string();
    }
    let date = js_sys::Date::new(&JsValue::from_f64(epoch as f64 * 1000.0));
    let iso = date.to_iso_string().as_string().unwrap_or_default();
    iso.trim_end_matches('Z')
        .trim_end_matches(".000")
        .to_string()
        + "Z"
}

fn format_date_short(epoch: i64) -> String {
    if epoch <= 0 {
        return "?".to_string();
    }
    let date = js_sys::Date::new(&JsValue::from_f64(epoch as f64 * 1000.0));
    format!(
        "{:04}-{:02}-{:02}",
        date.get_utc_full_year(),
        date.get_utc_month() + 1,
        date.get_utc_date()
    )
}

fn format_bytes(kb: i64) -> String {
    if kb >= 1_000_000_000 {
        format!("{:.2} TB", kb as f64 / 1_000_000_000.0)
    } else if kb >= 1_000_000 {
        format!("{:.1} GB", kb as f64 / 1_000_000.0)
    } else if kb >= 1_000 {
        format!("{:.1} MB", kb as f64 / 1_000.0)
    } else {
        format!("{} KB", kb)
    }
}

fn format_short_time(epoch: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(epoch * 1000.0));
    format!(
        "{:02}:{:02}:{:02}",
        date.get_utc_hours(),
        date.get_utc_minutes(),
        date.get_utc_seconds()
    )
}

#[derive(Clone, Debug, PartialEq)]
enum ChartDef {
    Line(String),
    Bar(String),
    Candlestick(String),
    Stats(String),
}

fn determine_charts(slice: &SliceResponse, sources: &[SourceInfo]) -> Vec<ChartDef> {
    let Some(first) = slice.rows.first() else { return vec![] };
    let Some(obj) = first.as_object() else { return vec![] };
    
    let mut charts = Vec::new();
    for (k, v) in obj {
        let s = k.as_str();
        if s == "t_epoch" || s == slice.time_col || s == slice.scope_col {
            continue;
        }
        
        // Find source formula if available
        let formula = sources.iter()
            .find(|src| src.mat_column == s || src.base_column == s)
            .map(|src| src.formula.as_str())
            .unwrap_or("");
            
        if let Some(o) = v.as_object() {
            if o.contains_key("open") && o.contains_key("close") {
                charts.push(ChartDef::Candlestick(k.clone()));
            } else if o.contains_key("m1") && o.contains_key("m2") {
                charts.push(ChartDef::Stats(k.clone()));
            }
        } else if v.as_f64().is_some() {
            if formula == "sum" {
                charts.push(ChartDef::Bar(k.clone()));
            } else {
                charts.push(ChartDef::Line(k.clone()));
            }
        }
    }
    charts.sort_by_key(|c| match c {
        ChartDef::Line(s) => (0, s.clone()),
        ChartDef::Bar(s) => (1, s.clone()),
        ChartDef::Candlestick(s) => (2, s.clone()),
        ChartDef::Stats(s) => (3, s.clone()),
    });
    charts
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct SourceInfo {
    view_name: String,
    base_column: String,
    formula: String,
    mat_column: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct ChangelogEntry {
    event_id: i64,
    base_view: String,
    #[serde(default)]
    scope_values: serde_json::Value,
    t_start: i64,
    t_end: i64,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct BlockInfo {
    blkno: i32,
    tuple_count: i64,
    alignment_pct: f64,
    drift_records: i64,
    t_range: [i64; 2],
    tenant_range: [i32; 2],
    is_boundary: bool,
    t_actual_start: i64,
    t_actual_end: i64,
    #[serde(default)]
    kickoff_epoch: i64,
    #[serde(default)]
    pending_changes: i32,
    #[serde(default)]
    is_stale: bool,
    #[serde(default)]
    last_changelog_ts: i64,
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
    kickoff: Signal<i64>,
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

            view! {
                <div class="tree">
                    <div
                        class=move || if selected_tier.get().is_none() { "tree-node tree-node-active" } else { "tree-node" }
                        on:click=move |_| selected_tier.set(None)
                        style="cursor:pointer;"
                    >
                        <span class="node-glyph">"▸"</span>
                        <span class="node-tier node-raw">"RAW"</span>
                        <span class="node-name">{h.base_view.clone()}</span>
                    </div>
                    {levels.into_iter().enumerate().map(|(i, level)| {
                        let sec = level.frame_seconds;
                        let tier_label = format_timespan(sec);
                        let view_name = level.view_name.clone();
                        let vn_click = view_name.clone();
                        let vn_class = view_name.clone();
                        let level_sources: Vec<_> = sources.iter()
                            .filter(|s| s.view_name == view_name)
                            .cloned()
                            .collect();
                        let indent = (i + 1) * 10;
                        view! {
                            <div>
                                <div
                                    class=move || if selected_tier.get().as_deref() == Some(vn_class.as_str()) { "tree-node tree-node-active" } else { "tree-node" }
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
                    {move || {
                        let k = kickoff.get();
                        (k > 0).then(|| view! {
                            <div class="tree-footer">
                                <span class="tf-label">"kickoff"</span>
                                <span class="tf-value tf-ts">{format_epoch_seconds(k)}</span>
                            </div>
                        })
                    }}
                </div>
            }.into_any()
        }}
    }
}

#[component]
fn PageMap(
    total_pages: Signal<i64>,
    tenant_scale: Signal<i64>,
    kickoff: Signal<i64>,
    frame_seconds: Signal<i32>,
    selected_page: Signal<Option<i32>>,
    dirty_blocks: Signal<BTreeSet<i32>>,
    stale_blocks: Signal<BTreeSet<i32>>,
    on_click: impl Fn(i32) + 'static + Send + Clone,
) -> impl IntoView {
    const PAGE_WINDOW: i32 = 1000;
    let page_offset = RwSignal::new(0i32);

    Effect::new(move |_| {
        let _ = total_pages.get();
        page_offset.set(0);
    });

    view! {
        <div class="page-map-wrap">
            {move || {
                let total = total_pages.get() as i32;
                let offset = page_offset.get();
                (total > PAGE_WINDOW).then(|| {
                    let end = (offset + PAGE_WINDOW).min(total);
                    view! {
                        <div class="paginator">
                            <button
                                class="pag-btn"
                                disabled=move || page_offset.get() == 0
                                on:click=move |_| page_offset.update(|o| *o = (*o - PAGE_WINDOW).max(0))
                            >"◀"</button>
                            <span class="pag-info">
                                {format!("pages {}–{} of {}", offset, end - 1, total)}
                            </span>
                            <button
                                class="pag-btn"
                                disabled=move || page_offset.get() + PAGE_WINDOW >= total_pages.get() as i32
                                on:click=move |_| {
                                    let total = total_pages.get() as i32;
                                    page_offset.update(|o| *o = (*o + PAGE_WINDOW).min(total - PAGE_WINDOW).max(0))
                                }
                            >"▶"</button>
                        </div>
                    }
                })
            }}
            <div class="page-map">
                <For
                    each=move || {
                        let total = total_pages.get() as i32;
                        let offset = page_offset.get();
                        let end = (offset + PAGE_WINDOW).min(total);
                        (offset..end).collect::<Vec<i32>>()
                    }
                    key=|idx| *idx
                    children=move |idx| {
                        let on_click = on_click.clone();
                        view! {
                            <div
                                class=move || {
                                    if selected_page.get() == Some(idx) { "page-cell selected" }
                                    else if stale_blocks.get().contains(&idx) { "page-cell stale" }
                                    else if dirty_blocks.get().contains(&idx) { "page-cell dirty" }
                                    else { "page-cell" }
                                }
                                style=move || {
                                    let scale = tenant_scale.get().max(1) as i32;
                                    format!("background:{}", get_color_for_tenant(idx % scale, scale))
                                }
                                on:click=move |_| on_click(idx)
                                title=move || {
                                    let scale = tenant_scale.get().max(1) as i32;
                                    let k = kickoff.get();
                                    let fs = frame_seconds.get().max(1);
                                    let tenant_id = idx % scale;
                                    let time_step = idx / scale;
                                    let est_t = k + (time_step as i64 * fs as i64);
                                    let t_str = if k > 0 {
                                        format!("\nEst. Time: {}", format_epoch_seconds(est_t))
                                    } else { "".into() };
                                    format!("Page {}\nTenant: {}{}", idx, tenant_id, t_str)
                                }
                            ></div>
                        }
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn BlockInspector(block: BlockInfo, kickoff: i64) -> impl IntoView {
    let kb = if block.kickoff_epoch > 0 { block.kickoff_epoch } else { kickoff };
    let start_t = if block.t_actual_start > 0 { block.t_actual_start } else { kb + block.t_range[0] };
    let end_t = if block.t_actual_end > 0 { block.t_actual_end } else { kb + block.t_range[1] };
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
                {block.is_stale.then(|| view! {
                    <span class="badge-dirty">"DIRTY"</span>
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
                <span class="ilabel">"pending"</span>
                <span class={if block.pending_changes > 0 { "ivalue warn" } else { "ivalue" }}>
                    {block.pending_changes} " changes"
                </span>
            </div>
            {(block.last_changelog_ts > 0).then(|| view! {
                <div class="irow-full">
                    <span class="ilabel">"last change"</span>
                    <div class="ivalue ivalue-ts">{format_epoch_seconds(block.last_changelog_ts)}</div>
                </div>
            })}
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

    let xor_rows_per_page = || {
        (1018.0 / 16.0) * 61.0
    };

    view! {
        <div class="cmp-panel">
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

            <div class="cmp-section-label" style="margin-top:10px;">"IO TAX — PAGES / 1K ROWS"</div>
            <div class="three-grid">
                <div class="tg-cell">
                    <span class="tg-label">"HEAP"</span>
                    <span class="tg-value tg-red">{format!("{:.1}", 1000.0_f64 / HEAP_ROWS_PER_PAGE)}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SPIRAL"</span>
                    <span class="tg-value tg-green">{move || format!("{:.1}", 1000.0_f64 / spiral_rows_per_page())}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"XOR-BK"</span>
                    <span class="tg-value tg-purple">{format!("{:.2}", 1000.0_f64 / xor_rows_per_page())}</span>
                </div>
            </div>

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
                    if n >= 1e9 { format!("{:.1} Billion rows", n / 1e9) }
                    else if n >= 1e6 { format!("{:.0} Million rows", n / 1e6) }
                    else { format!("{:.0}K rows", n / 1e3) }
                }}</span>
            </div>
            <div class="four-grid">
                <div class="tg-cell">
                    <span class="tg-label">"HEAP"</span>
                    <span class="tg-value tg-red">{move || {
                        let bytes = 10.0_f64.powf(row_count_exp.get()) * HEAP_BYTES_PER_ROW;
                        if bytes >= 1099511627776.0 { format!("{:.2}TB", bytes / 1099511627776.0) }
                        else if bytes >= 1073741824.0 { format!("{:.1}GB", bytes / 1073741824.0) }
                        else { format!("{:.0}MB", bytes / 1048576.0) }
                    }}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SPIRAL"</span>
                    <span class="tg-value tg-green">{move || {
                        let bytes = 10.0_f64.powf(row_count_exp.get()) * spiral_bpr();
                        if bytes >= 1099511627776.0 { format!("{:.2}TB", bytes / 1099511627776.0) }
                        else if bytes >= 1073741824.0 { format!("{:.1}GB", bytes / 1073741824.0) }
                        else { format!("{:.0}MB", bytes / 1048576.0) }
                    }}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"XOR"</span>
                    <span class="tg-value tg-purple">{move || {
                        let bytes = 10.0_f64.powf(row_count_exp.get()) * XOR_BLOCK_BYTES_PER_ROW;
                        if bytes >= 1099511627776.0 { format!("{:.2}TB", bytes / 1099511627776.0) }
                        else if bytes >= 1073741824.0 { format!("{:.1}GB", bytes / 1073741824.0) }
                        else { format!("{:.0}MB", bytes / 1048576.0) }
                    }}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SAVED"</span>
                    <span class="tg-value tg-green">{move || {
                        let saving = (1.0 - spiral_bpr() / HEAP_BYTES_PER_ROW) * 100.0;
                        format!("{:.1}%", saving)
                    }}</span>
                </div>
            </div>

            <div class="cmp-section-label" style="margin-top:10px;">"1 Billion ROWS PROJECTION (SPIRAL)"</div>
            <div class="ls-row">
                <span class="ls-label">"Storage"</span>
                <span class="ls-value tg-purple">{move || {
                    let kb = (1_000_000_000.0 * spiral_bpr() / 1024.0) as i64;
                    format_bytes(kb)
                }}</span>
            </div>
            <div class="ls-row">
                <span class="ls-label">"Heap equiv"</span>
                <span class="ls-value ls-dim">{move || {
                    let kb = (1_000_000_000.0 * HEAP_BYTES_PER_ROW / 1024.0) as i64;
                    format_bytes(kb)
                }}</span>
            </div>
            <div class="ls-row">
                <span class="ls-label">"Estimated Saving"</span>
                <span class="ls-value ls-green">{move || {
                    let saving = (1.0 - spiral_bpr() / HEAP_BYTES_PER_ROW) * 100.0;
                    format!("{:.1}%", saving)
                }}</span>
            </div>
        </div>
    }
}

#[component]
fn SvgCandlestickChart(
    rows: Vec<serde_json::Value>,
    scope_col: String,
    metric_col: String,
    volume_col: Option<String>,
) -> impl IntoView {
    const W: f64 = 300.0;
    const H: f64 = 140.0;
    const MX: f64 = 6.0;
    const MY: f64 = 16.0;
    const MB: f64 = 24.0;
    let plot_w = W - MX * 2.0;
    let plot_h = H - MY - MB;
    let main_h = if volume_col.is_some() { plot_h * 0.7 } else { plot_h };
    let vol_h = plot_h - main_h - 4.0;

    let mut t_min: f64 = 0.0;
    let mut t_max: f64 = 1.0;
    let mut v_min: f64 = 0.0;
    let mut v_max: f64 = 1.0;
    let mut vol_max: f64 = 1.0;
    let mut has_data = false;
    
    struct Candle {
        t: f64,
        o: f64,
        h: f64,
        l: f64,
        c: f64,
        vol: f64,
    }
    let mut by_tenant: BTreeMap<i64, Vec<Candle>> = BTreeMap::new();

    for row in &rows {
        let Some(t) = row.get("t_epoch").and_then(|v| v.as_f64()) else { continue };
        let Some(m) = row.get(&metric_col).and_then(|v| v.as_object()) else { continue };
        let o = m.get("open").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h = m.get("high").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let l = m.get("low").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let c = m.get("close").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vol = volume_col.as_ref().and_then(|vc| row.get(vc)).and_then(|v| v.as_f64())
            .or_else(|| m.get("volume").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        
        let scope = row.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0);
        if !has_data {
            t_min = t; t_max = t; v_min = l; v_max = h; vol_max = vol;
            has_data = true;
        } else {
            if t < t_min { t_min = t; }
            if t > t_max { t_max = t; }
            if l < v_min { v_min = l; }
            if h > v_max { v_max = h; }
            if vol > vol_max { vol_max = vol; }
        }
        by_tenant.entry(scope).or_default().push(Candle { t, o, h, l, c, vol });
    }

    if t_max <= t_min { t_max = t_min + 1.0; }
    if v_max <= v_min { v_max = v_min + 1.0; }
    if vol_max <= 0.0 { vol_max = 1.0; }

    let t_range = t_max - t_min;
    let v_range = v_max - v_min;
    let n = by_tenant.len() as i32;
    
    let mut tenants_legend = Vec::new();

    let grid_lines = if has_data {
        let steps = 4;
        (0..=steps).map(|i| {
            let t = t_min + (t_range * i as f64 / steps as f64);
            let x = MX + (t - t_min) / t_range * plot_w;
            view! {
                <line x1={x} y1={MY} x2={x} y2={MY + plot_h} stroke="var(--border)" stroke-width="0.5" stroke-dasharray="2,2" />
            }
        }).collect_view()
    } else {
        Vec::new().collect_view()
    };

    let candles_view = by_tenant.into_iter().enumerate().map(|(ti, (tenant_id, candles))| {
        let color = get_color_for_tenant(ti as i32, n);
        tenants_legend.push((tenant_id, color.clone()));
        
        let candle_w = (plot_w / (rows.len() as f64 / n as f64).max(1.0) * 0.8).clamp(1.0, 8.0);

        candles.into_iter().map(|candle| {
            let x = MX + (candle.t - t_min) / t_range * plot_w;
            
            let y_h = MY + (1.0 - (candle.h - v_min) / v_range) * main_h;
            let y_l = MY + (1.0 - (candle.l - v_min) / v_range) * main_h;
            let y_o = MY + (1.0 - (candle.o - v_min) / v_range) * main_h;
            let y_c = MY + (1.0 - (candle.c - v_min) / v_range) * main_h;
            
            let rect_y = y_o.min(y_c);
            let rect_h = (y_o - y_c).abs().max(1.0);
            let is_up = candle.c >= candle.o;
            
            let v_bar_h = (candle.vol / vol_max) * vol_h;
            let v_bar_y = MY + plot_h - v_bar_h;

            view! {
                <g>
                    <line x1={x} y1={y_h} x2={x} y2={y_l} stroke={color.clone()} stroke-width="1" opacity="0.6" />
                    <rect 
                        x={x - candle_w/2.0} 
                        y={rect_y} 
                        width={candle_w} 
                        height={rect_h} 
                        fill={if is_up { color.clone() } else { "none".to_string() }} 
                        stroke={color.clone()} 
                        stroke-width="1" 
                        opacity="0.9" 
                    />
                    {(volume_col.is_some()).then(|| view! {
                        <rect 
                            x={x - candle_w/2.0} 
                            y={v_bar_y} 
                            width={candle_w} 
                            height={v_bar_h} 
                            fill={color.clone()} 
                            opacity="0.3" 
                        />
                    })}
                </g>
            }
        }).collect_view()
    }).collect_view();

    let label = if let Some(ref v) = volume_col {
        format!("{} + {}", metric_col, v)
    } else {
        format!("{} (candles)", metric_col)
    };
    let vmax_s = if has_data { format!("{:.1}", v_max) } else { "—".to_string() };
    let vmin_s = if has_data { format!("{:.1}", v_min) } else { "—".to_string() };
    let tstart_s = if has_data { format_short_time(t_min) } else { "".to_string() };
    let tend_s = if has_data && t_max > t_min { format_short_time(t_max) } else { "".to_string() };
    
    let vb = format!("0 0 {} {}", W, H);

    view! {
        <div class="chart-container">
            <svg viewBox={vb} style="width:100%; height:140px; display:block; overflow:visible;">
                <text x="4" y="11" font-size="9" font-weight="700" fill="var(--muted)">{label}</text>
                <text x="296" y="11" font-size="8" fill="var(--muted)" text-anchor="end">{vmax_s}</text>
                <text x="296" y={MY + main_h} font-size="8" fill="var(--muted)" text-anchor="end">{vmin_s}</text>
                
                {grid_lines}
                
                <text x={MX} y={H - 4.0} font-size="7" fill="var(--blue)">{tstart_s}</text>
                <text x={W - MX} y={H - 4.0} font-size="7" fill="var(--blue)" text-anchor="end">{tend_s}</text>

                {(!has_data).then(|| view! {
                    <text x="150" y="70" font-size="10" fill="var(--muted)" text-anchor="middle">"no data"</text>
                })}
                {candles_view}
                
                <line x1={MX} y1={MY} x2={MX} y2={MY + plot_h} stroke="var(--blue)" stroke-width="1" opacity="0.4" />
                <line x1={W - MX} y1={MY} x2={W - MX} y2={MY + plot_h} stroke="var(--blue)" stroke-width="1" opacity="0.4" />
            </svg>
            <div class="chart-legend">
                {tenants_legend.into_iter().map(|(id, color)| {
                    view! {
                        <div class="legend-item">
                            <span class="legend-dot" style={format!("background: {}", color)}></span>
                            <span class="legend-label">{id}</span>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
fn SvgBarChart(
    rows: Vec<serde_json::Value>,
    scope_col: String,
    metric_col: String,
) -> impl IntoView {
    const W: f64 = 300.0;
    const H: f64 = 120.0;
    const MX: f64 = 6.0;
    const MY: f64 = 16.0;
    const MB: f64 = 24.0;
    let plot_w = W - MX * 2.0;
    let plot_h = H - MY - MB;

    let mut by_time: BTreeMap<i64, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut v_max: f64 = 1.0;
    let mut t_min: f64 = 0.0;
    let mut t_max: f64 = 1.0;
    let mut has_data = false;

    for row in &rows {
        let Some(t) = row.get("t_epoch").and_then(|v| v.as_f64()) else { continue };
        let Some(v) = row.get(&metric_col).and_then(|v| v.as_f64()) else { continue };
        let scope = row.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0);
        
        let t_i = t as i64;
        let entry = by_time.entry(t_i).or_default();
        *entry.entry(scope).or_default() += v;
        
        if !has_data {
            t_min = t; t_max = t;
            has_data = true;
        } else {
            if t < t_min { t_min = t; }
            if t > t_max { t_max = t; }
        }
    }
    
    for scopes_map in by_time.values() {
        let sum: f64 = scopes_map.values().sum();
        if sum > v_max { v_max = sum; }
    }

    let mut unique_scopes: Vec<i64> = rows.iter()
        .map(|r| r.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0))
        .collect();
    unique_scopes.sort();
    unique_scopes.dedup();
    let n_scopes = unique_scopes.len() as i32;
    let scope_colors: BTreeMap<i64, String> = unique_scopes.into_iter().enumerate()
        .map(|(i, s)| (s, get_color_for_tenant(i as i32, n_scopes)))
        .collect();

    if t_max <= t_min { t_max = t_min + 1.0; }
    let t_range = t_max - t_min;
    let bar_w = (plot_w / (by_time.len() as f64).max(1.0) * 0.8).clamp(1.0, 15.0);

    let mut tenants_legend = Vec::new();
    for (id, color) in &scope_colors {
        tenants_legend.push((*id, color.clone()));
    }

    let grid_lines = if has_data {
        let steps = 4;
        (0..=steps).map(|i| {
            let t = t_min + (t_range * i as f64 / steps as f64);
            let x = MX + (t - t_min) / t_range * plot_w;
            view! {
                <line x1={x} y1={MY} x2={x} y2={MY + plot_h} stroke="var(--border)" stroke-width="0.5" stroke-dasharray="2,2" />
            }
        }).collect_view()
    } else {
        Vec::new().collect_view()
    };

    let bars = by_time.into_iter().map(|(t, scopes_map)| {
        let x = MX + (t as f64 - t_min) / t_range * plot_w;
        let mut y_offset = 0.0;
        
        scopes_map.into_iter().map(|(scope, v)| {
            let h = (v / v_max) * plot_h;
            let y = MY + plot_h - y_offset - h;
            y_offset += h;
            let color = scope_colors.get(&scope).cloned().unwrap_or_else(|| "#888".to_string());
            view! {
                <rect x={x - bar_w/2.0} y={y} width={bar_w} height={h} fill={color} opacity="0.8" />
            }
        }).collect_view()
    }).collect_view();

    let label = format!("{} (sum)", metric_col);
    let vmax_s = if has_data { format!("{:.1}", v_max) } else { "—".to_string() };
    let tstart_s = if has_data { format_short_time(t_min) } else { "".to_string() };
    let tend_s = if has_data && t_max > t_min { format_short_time(t_max) } else { "".to_string() };
    let vb = format!("0 0 {} {}", W, H);

    view! {
        <div class="chart-container">
            <svg viewBox={vb} style="width:100%; height:120px; display:block; overflow:visible;">
                <text x="4" y="11" font-size="9" font-weight="700" fill="var(--muted)">{label}</text>
                <text x="296" y="11" font-size="8" fill="var(--muted)" text-anchor="end">{vmax_s}</text>
                
                {grid_lines}
                
                <text x={MX} y={H - 4.0} font-size="7" fill="var(--blue)">{tstart_s}</text>
                <text x={W - MX} y={H - 4.0} font-size="7" fill="var(--blue)" text-anchor="end">{tend_s}</text>

                {(!has_data).then(|| view! {
                    <text x="150" y="60" font-size="10" fill="var(--muted)" text-anchor="middle">"no data"</text>
                })}
                {bars}
                
                <line x1={MX} y1={MY} x2={MX} y2={MY + plot_h} stroke="var(--blue)" stroke-width="1" opacity="0.4" />
                <line x1={W - MX} y1={MY} x2={W - MX} y2={MY + plot_h} stroke="var(--blue)" stroke-width="1" opacity="0.4" />
            </svg>
            <div class="chart-legend">
                {tenants_legend.into_iter().map(|(id, color)| {
                    view! {
                        <div class="legend-item">
                            <span class="legend-dot" style={format!("background: {}", color)}></span>
                            <span class="legend-label">{id}</span>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
fn SvgStatsChart(
    rows: Vec<serde_json::Value>,
    scope_col: String,
    metric_col: String,
) -> impl IntoView {
    let mut rows_mean = Vec::new();
    let mut rows_var = Vec::new();
    let mut rows_max = Vec::new();
    let mut rows_min = Vec::new();
    
    for row in &rows {
        let Some(t_epoch) = row.get("t_epoch").cloned() else { continue };
        let Some(scope) = row.get(&scope_col).cloned() else { continue };
        let Some(obj) = row.get(&metric_col).and_then(|v| v.as_object()) else { continue };
        
        let n = obj.get("n").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let m1 = obj.get("m1").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let m2 = obj.get("m2").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let max = obj.get("max").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let min = obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
        
        let variance = if n > 1.0 { m2 / (n - 1.0) } else { 0.0 };
        
        let mut r_mean = serde_json::Map::new();
        r_mean.insert("t_epoch".to_string(), t_epoch.clone());
        r_mean.insert(scope_col.clone(), scope.clone());
        r_mean.insert("val".to_string(), serde_json::json!(m1));
        rows_mean.push(serde_json::Value::Object(r_mean));
        
        let mut r_var = serde_json::Map::new();
        r_var.insert("t_epoch".to_string(), t_epoch.clone());
        r_var.insert(scope_col.clone(), scope.clone());
        r_var.insert("val".to_string(), serde_json::json!(variance));
        rows_var.push(serde_json::Value::Object(r_var));
        
        let mut r_max = serde_json::Map::new();
        r_max.insert("t_epoch".to_string(), t_epoch.clone());
        r_max.insert(scope_col.clone(), scope.clone());
        r_max.insert("val".to_string(), serde_json::json!(max));
        rows_max.push(serde_json::Value::Object(r_max));
        
        let mut r_min = serde_json::Map::new();
        r_min.insert("t_epoch".to_string(), t_epoch.clone());
        r_min.insert(scope_col.clone(), scope.clone());
        r_min.insert("val".to_string(), serde_json::json!(min));
        rows_min.push(serde_json::Value::Object(r_min));
    }

    view! {
        <div class="stats-group" style="display:grid; grid-template-columns: 1fr 1fr; gap: 8px; border: 1px solid var(--border); padding: 8px; border-radius: 4px; grid-column: 1 / -1;">
            <div style="grid-column: 1 / -1; font-size: 10px; font-weight: 700; color: var(--muted); margin-bottom: 4px;">{format!("{} (stats breakdown)", metric_col)}</div>
            <div class="chart-item">
                <SvgLineChart rows={rows_mean} scope_col={scope_col.clone()} metric_col="val".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"MEAN"</div>
            </div>
            <div class="chart-item">
                <SvgLineChart rows={rows_var} scope_col={scope_col.clone()} metric_col="val".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"VARIANCE"</div>
            </div>
            <div class="chart-item">
                <SvgLineChart rows={rows_max} scope_col={scope_col.clone()} metric_col="val".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"MAX"</div>
            </div>
            <div class="chart-item">
                <SvgLineChart rows={rows_min} scope_col={scope_col.clone()} metric_col="val".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"MIN"</div>
            </div>
        </div>
    }
}

#[component]
fn SvgLineChart(
    rows: Vec<serde_json::Value>,
    scope_col: String,
    metric_col: String,
) -> impl IntoView {
    const W: f64 = 300.0;
    const H: f64 = 120.0;
    const MX: f64 = 6.0;
    const MY: f64 = 16.0;
    const MB: f64 = 24.0;
    let plot_w = W - MX * 2.0;
    let plot_h = H - MY - MB;

    let mut t_min: f64 = 0.0;
    let mut t_max: f64 = 1.0;
    let mut v_min: f64 = 0.0;
    let mut v_max: f64 = 1.0;
    let mut has_data = false;
    let mut by_tenant: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();

    for row in &rows {
        let Some(t) = row.get("t_epoch").and_then(|v| v.as_f64()) else { continue };
        let Some(v) = row.get(&metric_col).and_then(|v| v.as_f64()) else { continue };
        let scope = row.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0);
        if !has_data {
            t_min = t; t_max = t; v_min = v; v_max = v;
            has_data = true;
        } else {
            if t < t_min { t_min = t; }
            if t > t_max { t_max = t; }
            if v < v_min { v_min = v; }
            if v > v_max { v_max = v; }
        }
        by_tenant.entry(scope).or_default().push((t, v));
    }

    if t_max <= t_min { t_max = t_min + 1.0; }
    if v_max <= v_min { v_max = v_min + 1.0; }

    let t_range = t_max - t_min;
    let v_range = v_max - v_min;
    let n = by_tenant.len() as i32;

    let mut tenants_legend = Vec::new();

    let grid_lines = if has_data {
        let steps = 4;
        (0..=steps).map(|i| {
            let t = t_min + (t_range * i as f64 / steps as f64);
            let x = MX + (t - t_min) / t_range * plot_w;
            view! {
                <line x1={x} y1={MY} x2={x} y2={MY + plot_h} stroke="var(--border)" stroke-width="0.5" stroke-dasharray="2,2" />
            }
        }).collect_view()
    } else {
        Vec::new().collect_view()
    };

    let polylines = by_tenant.into_iter().enumerate().map(|(i, (tenant_id, pts))| {
        let color = get_color_for_tenant(i as i32, n);
        tenants_legend.push((tenant_id, color.clone()));
        let pts_str: String = pts
            .iter()
            .map(|(t, v)| {
                let x = MX + (t - t_min) / t_range * plot_w;
                let y = MY + (1.0 - (v - v_min) / v_range) * plot_h;
                format!("{:.1},{:.1}", x, y)
            })
            .collect::<Vec<_>>()
            .join(" ");
        view! {
            <polyline points={pts_str} fill="none" stroke={color} stroke-width="1.5" opacity="0.85" />
        }
    }).collect_view();

    let label = metric_col;
    let vmax_s = if has_data { format!("{:.1}", v_max) } else { "—".to_string() };
    let vmin_s = if has_data { format!("{:.1}", v_min) } else { "—".to_string() };
    let tstart_s = if has_data { format_short_time(t_min) } else { "".to_string() };
    let tend_s = if has_data && t_max > t_min { format_short_time(t_max) } else { "".to_string() };
    
    let vb = format!("0 0 {} {}", W, H);

    view! {
        <div class="chart-container">
            <svg viewBox={vb} style="width:100%; height:120px; display:block; overflow:visible;">
                <text x="4" y="11" font-size="9" font-weight="700" fill="var(--muted)">{label}</text>
                <text x="296" y="11" font-size="8" fill="var(--muted)" text-anchor="end">{vmax_s}</text>
                <text x="296" y={MY + plot_h} font-size="8" fill="var(--muted)" text-anchor="end">{vmin_s}</text>
                
                {grid_lines}

                <text x={MX} y={H - 4.0} font-size="7" fill="var(--blue)">{tstart_s}</text>
                <text x={W - MX} y={H - 4.0} font-size="7" fill="var(--blue)" text-anchor="end">{tend_s}</text>

                {(!has_data).then(|| view! {
                    <text x="150" y="60" font-size="10" fill="var(--muted)" text-anchor="middle">"no data"</text>
                })}
                {polylines}
                
                <line x1={MX} y1={MY} x2={MX} y2={MY + plot_h} stroke="var(--blue)" stroke-width="1" opacity="0.4" />
                <line x1={W - MX} y1={MY} x2={W - MX} y2={MY + plot_h} stroke="var(--blue)" stroke-width="1" opacity="0.4" />
            </svg>
            <div class="chart-legend">
                {tenants_legend.into_iter().map(|(id, color)| {
                    view! {
                        <div class="legend-item">
                            <span class="legend-dot" style={format!("background: {}", color)}></span>
                            <span class="legend-label">{id}</span>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
fn DataTable(slice: SliceResponse) -> impl IntoView {
    let rows = slice.rows;
    if rows.is_empty() {
        return view! { <div class="empty-state">"no rows"</div> }.into_any();
    }
    
    let first = rows.first().unwrap();
    let Some(obj) = first.as_object() else {
        return view! { <div class="empty-state">"invalid data format"</div> }.into_any();
    };
    let mut keys: Vec<String> = obj.keys().cloned().collect();
    // Prioritize t_epoch and common time/scope columns
    keys.sort_by_key(|k| {
        if k == "t_epoch" || k == &slice.time_col { (0, k.clone()) }
        else if k == &slice.scope_col { (1, k.clone()) }
        else { (2, k.clone()) }
    });
    
    view! {
        <div class="data-table-container">
            <table class="data-table">
                <thead>
                    <tr>
                        {keys.clone().into_iter().map(|k| view! { <th>{k}</th> }).collect_view()}
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|row| {
                        let Some(obj) = row.as_object() else { return view! { <tr></tr> }.into_any() };
                        let keys_c = keys.clone();
                        view! {
                            <tr>
                                {keys_c.into_iter().map(|k| {
                                    let val = obj.get(&k).unwrap_or(&serde_json::Value::Null);
                                    let (val_str, cls) = if let Some(f) = val.as_f64() {
                                        (format!("{:.4}", f), "td-num")
                                    } else if let Some(i) = val.as_i64() {
                                        (i.to_string(), "td-num")
                                    } else if val.is_object() {
                                        (serde_json::to_string(val).unwrap_or_default(), "td-obj")
                                    } else {
                                        (val.to_string().replace("\"", ""), "")
                                    };
                                    view! { <td class={cls}>{val_str}</td> }
                                }).collect_view()}
                            </tr>
                        }.into_any()
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }.into_any()
}

#[component]
fn PageDataCharts(slice: SliceResponse, sources: Vec<SourceInfo>) -> impl IntoView {
    let charts = determine_charts(&slice, &sources);
    let rows = slice.rows.clone();
    let sc = slice.scope_col.clone();
    let count = slice.count;
    let fetch_ms = slice.fetch_ms;
    let slice_c = slice.clone();

    view! {
        <div class="charts-section">
            <div class="charts-meta">
                <span>{format!("{} rows · {} charts", count, charts.len())}</span>
                <span style="margin-left: 12px; color: var(--green); opacity: 0.8;">{format!("{:.1}ms fetch", fetch_ms)}</span>
            </div>
            <div class="charts-grid">
                {charts.into_iter().map(|chart| {
                    let rows_c = rows.clone();
                    let sc_c = sc.clone();
                    match chart {
                        ChartDef::Line(col) => view! {
                            <div class="chart-item">
                                <SvgLineChart rows=rows_c scope_col=sc_c metric_col=col />
                            </div>
                        }.into_any(),
                        ChartDef::Bar(col) => view! {
                            <div class="chart-item">
                                <SvgBarChart rows=rows_c scope_col=sc_c metric_col=col />
                            </div>
                        }.into_any(),
                        ChartDef::Candlestick(col) => view! {
                            <div class="chart-item" style="grid-column: 1 / -1;">
                                <SvgCandlestickChart rows=rows_c scope_col=sc_c metric_col=col volume_col=None />
                            </div>
                        }.into_any(),
                        ChartDef::Stats(col) => view! {
                            <div class="chart-item" style="grid-column: 1 / -1;">
                                <SvgStatsChart rows=rows_c scope_col=sc_c metric_col=col />
                            </div>
                        }.into_any(),
                    }
                }).collect_view()}
            </div>
            
            <div class="panel-hdr" style="margin-top: 20px; border-bottom: 1px solid var(--border); padding-bottom: 4px;">"RAW DATA"</div>
            <DataTable slice=slice_c />
        </div>
    }
}

#[component]
fn App() -> impl IntoView {
    let stats_by_view = RwSignal::new(BTreeMap::<String, StorageStats>::new());
    let config = RwSignal::new(SystemConfig::default());
    let selected_base_view = RwSignal::new(String::new());
    let selected_tier_view = RwSignal::new(Option::<String>::None);
    let last_event = RwSignal::new(None::<String>);
    let selected_block = RwSignal::new(None::<BlockInfo>);
    let selected_page_no = RwSignal::new(None::<i32>);
    let is_connected = RwSignal::new(false);
    let slice_data = RwSignal::new(None::<SliceResponse>);
    let explain_result = RwSignal::new(None::<ExplainResult>);
    let explain_query = RwSignal::new(String::new());
    let explain_running = RwSignal::new(false);
    let explain_user_edited = RwSignal::new(false);
    let explain_last_block = RwSignal::new(None::<BlockInfo>);
    let dirty_page_nos = RwSignal::new(BTreeSet::<i32>::new());
    let stale_blocks = RwSignal::new(BTreeSet::<i32>::new());
    let changelog_buffer = RwSignal::new(VecDeque::<ChangelogEntry>::new());
    let changelog_expanded = RwSignal::new(false);
    let pending_page_restore = RwSignal::new(None::<i32>);
    let auto_explain = RwSignal::new({
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|ls| ls.get_item("auto_explain").ok().flatten())
            .map(|v| v == "true")
            .unwrap_or(false)
    });

    // Parse URL hash for state restore: #table=X&tier=Y&page=N
    let (url_table, url_tier, url_page) = {
        let hash = web_sys::window()
            .and_then(|w| w.location().hash().ok())
            .unwrap_or_default();
        let hash = hash.trim_start_matches('#').to_string();
        let mut table = None::<String>;
        let mut tier = None::<String>;
        let mut page = None::<i32>;
        for part in hash.split('&') {
            if let Some(v) = part.strip_prefix("table=") {
                if !v.is_empty() { table = Some(v.to_string()); }
            } else if let Some(v) = part.strip_prefix("tier=") {
                if !v.is_empty() { tier = Some(v.to_string()); }
            } else if let Some(v) = part.strip_prefix("page=") {
                page = v.parse().ok();
            }
        }
        (table, tier, page)
    };

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
        let url_table = url_table.clone();
        let url_tier = url_tier.clone();
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
                                        let t0 = js_sys::Date::now();
                                        stats_by_view.update(|stats| {
                                            stats.insert(s.view_name.clone(), s.clone());
                                        });
                                        let dt = js_sys::Date::now() - t0;
                                        if dt > 5.0 {
                                            console_log!("PERF StorageStats {} took {:.0}ms", s.view_name, dt);
                                        }
                                        last_event.set(Some(format!("stats: {}/{} updated", s.base_view, s.view_name)));
                                    }
                                    VortexEvent::SystemConfig(c) => {
                                        let t0 = js_sys::Date::now();
                                        last_event.set(Some(format!("system config: {} hierarchies", c.hierarchies.len())));
                                        let selected = selected_base_view.get_untracked();
                                        let first_base = c
                                            .hierarchies
                                            .first()
                                            .map(|h| h.base_view.clone())
                                            .unwrap_or_default();

                                        if selected.is_empty() || !c.hierarchies.iter().any(|h| h.base_view == selected) {
                                            let target_base = url_table.as_deref()
                                                .filter(|t| c.hierarchies.iter().any(|h| h.base_view == *t))
                                                .map(|t| t.to_string())
                                                .unwrap_or(first_base);
                                            selected_base_view.set(target_base);
                                            if let Some(ref tier) = url_tier {
                                                selected_tier_view.set(Some(tier.clone()));
                                            }
                                            selected_block.set(None);
                                            selected_page_no.set(url_page);
                                            if url_page.is_some() {
                                                pending_page_restore.set(url_page);
                                            }
                                        }
                                        config.set(c);
                                        let dt = js_sys::Date::now() - t0;
                                        console_log!("PERF SystemConfig took {:.0}ms", dt);
                                    }
                                    VortexEvent::ChangelogUpdate(entry) => {
                                        let t0 = js_sys::Date::now();
                                        changelog_buffer.update(|buf| {
                                            buf.push_front(entry.clone());
                                            buf.truncate(100);
                                        });
                                        last_event.set(Some(format!(
                                            "#{} {} @ {}",
                                            entry.event_id, entry.base_view, entry.t_start
                                        )));
                                        if entry.base_view == selected_base_view.get_untracked() {
                                            let h = config.get_untracked()
                                                .hierarchies
                                                .into_iter()
                                                .find(|h| h.base_view == entry.base_view);
                                            if let Some(h) = h {
                                                let kickoff = h.kickoff_epoch;
                                                let tenant_scale = h.tenant_scale.max(1);
                                                let data_per_page = stats_by_view
                                                    .get_untracked()
                                                    .get(&h.raw_view_name)
                                                    .map(|s| s.data_per_page)
                                                    .filter(|&d| d > 0)
                                                    .unwrap_or(1018);
                                                let t_rel_start = (entry.t_start - kickoff).max(0);
                                                let t_rel_end = (entry.t_end - kickoff).max(0);
                                                let blkno_start = ((t_rel_start * tenant_scale) / data_per_page).max(0) as i32;
                                                let blkno_end = ((t_rel_end * tenant_scale) / data_per_page + 1) as i32;
                                                let dirty_before = dirty_page_nos.get_untracked().len();
                                                dirty_page_nos.update(|set| {
                                                    const MAX_DIRTY: usize = 200;
                                                    for b in blkno_start..=blkno_end {
                                                        if set.len() >= MAX_DIRTY { break; }
                                                        set.insert(b);
                                                    }
                                                });
                                                let dt = js_sys::Date::now() - t0;
                                                if dt > 2.0 {
                                                    console_log!("PERF ChangelogUpdate blks {}-{} dirty_before={} took {:.0}ms", blkno_start, blkno_end, dirty_before, dt);
                                                }
                                            }
                                        }
                                    }
                                },
                                Err(e) => console_log!("VortexEvent deserialization error: {}", e)
                            }
                        }
                    }
                    is_connected.set(false);
                });
            }
            Err(e) => console_log!("WebSocket error: {:?}", e),
        }
    });

    // Write URL hash on state change (#75)
    Effect::new(move |_| {
        let table = selected_base_view.get();
        if table.is_empty() { return; }
        let tier = selected_tier_view.get().unwrap_or_default();
        let page_str = selected_page_no.get().map(|p| p.to_string()).unwrap_or_default();
        let hash = format!("table={}&tier={}&page={}", table, tier, page_str);
        if let Some(w) = web_sys::window() {
            let _ = w.location().set_hash(&hash);
        }
    });

    Effect::new(move |_| {
        let _ = selected_tier_view.get();
        selected_block.set(None);
        selected_page_no.set(None);
        slice_data.set(None);
    });

    Effect::new(move |_| {
        let stats = current_stats.get();
        if selected_page_no.get().is_none() && stats.total_pages > 0 {
            selected_page_no.set(Some(0));
        }
    });

    let api_base_clone = Arc::clone(&api_base);
    let fetch_block_info = move |blkno: i32| {
        selected_block.set(None);
        slice_data.set(None);
        selected_page_no.set(Some(blkno));

        let selected = selected_base_view.get_untracked();
        let view_name = selected_tier_view
            .get_untracked()
            .unwrap_or_else(|| {
                config
                    .get_untracked()
                    .hierarchies
                    .iter()
                    .find(|h| h.base_view == selected)
                    .map(|h| h.raw_view_name.clone())
                    .unwrap_or_default()
            });
        if view_name.is_empty() {
            return;
        }

        let url = format!(
            "{}/api/storage/{}/block/{}",
            api_base_clone, view_name, blkno
        );
        leptos::task::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await
                && let Ok(info) = resp.json::<BlockInfo>().await {
                    console_log!("FETCHED BLOCK INFO: {:?}", info);
                    selected_block.set(Some(info));
                }
        });
    };

    // Auto-restore page from URL hash (#75): fires when base is set + pending page exists
    let api_base_restore = Arc::clone(&api_base);
    Effect::new(move |_| {
        let base = selected_base_view.get();
        if base.is_empty() { return; }
        let Some(blkno) = pending_page_restore.get() else { return; };
        pending_page_restore.set(None);
        let view_name = selected_tier_view.get_untracked().unwrap_or_else(|| {
            config.get_untracked().hierarchies.iter()
                .find(|h| h.base_view == base)
                .map(|h| h.raw_view_name.clone())
                .unwrap_or_default()
        });
        if view_name.is_empty() { return; }
        let url = format!("{}/api/storage/{}/block/{}", api_base_restore, view_name, blkno);
        leptos::task::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await {
                if let Ok(info) = resp.json::<BlockInfo>().await {
                    selected_block.set(Some(info));
                }
            }
        });
    });

    // Sync stale_blocks from server-confirmed is_stale on each block inspect (#60)
    Effect::new(move |_| {
        if let Some(b) = selected_block.get() {
            if b.is_stale {
                stale_blocks.update(|s| { s.insert(b.blkno); });
            } else {
                stale_blocks.update(|s| { s.remove(&b.blkno); });
                dirty_page_nos.update(|s| { s.remove(&b.blkno); });
            }
        }
    });

    // Memo: only propagates to subscribers when the value actually changes (PartialEq check).
    // Without Memo, Signal::derive re-notifies ALL subscribers on every stats_by_view update,
    // causing 1000+ cell re-renders per StorageStats WS event.
    let current_stats: Signal<StorageStats> = Memo::new(move |_| {
        let selected_base = selected_base_view.get();
        let view_name = selected_tier_view.get().unwrap_or_else(|| {
            config
                .get()
                .hierarchies
                .iter()
                .find(|h| h.base_view == selected_base)
                .map(|h| h.raw_view_name.clone())
                .unwrap_or_default()
        });

        stats_by_view
            .get()
            .get(&view_name)
            .cloned()
            .unwrap_or_default()
    }).into();

    let current_hierarchy_opt: Signal<Option<HierarchyConfig>> = Memo::new(move |_| {
        current_hierarchy(&config.get(), &selected_base_view.get())
    }).into();

    let tenant_scale: Signal<i64> = Memo::new(move |_| {
        current_hierarchy_opt
            .get()
            .map(|h| h.tenant_scale.max(1))
            .unwrap_or(1)
    }).into();

    // kickoff_epoch is stable per session; Memo prevents 1000-cell title re-renders on each
    // StorageStats poll cycle even when the epoch value hasn't changed.
    let kickoff: Signal<i64> = Memo::new(move |_| {
        let h_kickoff = current_hierarchy_opt
            .get()
            .map(|h| h.kickoff_epoch)
            .unwrap_or(0);
        let stats = current_stats.get();
        let s_kickoff = stats.kickoff_epoch;

        const PG_EPOCH: i64 = 946684800; // 2000-01-01

        // If we have a selected block with actual time or explicit kickoff, we can infer the real kickoff
        if let Some(b) = selected_block.get() {
            if b.kickoff_epoch > 0 && b.kickoff_epoch != PG_EPOCH {
                return b.kickoff_epoch;
            }
            if b.t_actual_start > 0 {
                let inferred = b.t_actual_start - b.t_range[0];
                if inferred > 0 {
                    return inferred;
                }
            }
        }

        if s_kickoff > 0 && s_kickoff != PG_EPOCH {
            s_kickoff
        } else if h_kickoff > 0 && h_kickoff != PG_EPOCH {
            h_kickoff
        } else {
            if s_kickoff > 0 { s_kickoff } else { h_kickoff }
        }
    }).into();

    let current_frame_seconds: Signal<i32> = Memo::new(move |_| {
        let tier = selected_tier_view.get();
        let h = current_hierarchy_opt.get();
        match (tier, h) {
            (Some(t), Some(h)) => h.aggregation_levels.iter()
                .find(|l| l.view_name == t)
                .map(|l| l.frame_seconds)
                .unwrap_or(0),
            _ => 0,
        }
    }).into();

    let api_base_slice = Arc::clone(&api_base);
    Effect::new(move |_| {
        // Only track selected_block and selected_tier_view — not config or kickoff.
        // kickoff derives from stats_by_view, so tracking it would re-fire this effect
        // (and spawn a new HTTP request) on every StorageStats WS event.
        let block = selected_block.get();
        let selected = selected_base_view.get_untracked();
        let view_name = selected_tier_view
            .get()
            .unwrap_or_else(|| {
                config
                    .get_untracked()
                    .hierarchies
                    .iter()
                    .find(|h| h.base_view == selected)
                    .map(|h| h.raw_view_name.clone())
                    .unwrap_or_default()
            });
        let k = kickoff.get_untracked();

        let Some(b) = block else {
            slice_data.set(None);
            return;
        };

        let kb = if b.kickoff_epoch > 0 { b.kickoff_epoch } else { k };

        let (t_start, t_end) = if b.t_actual_start > 0 {
            (b.t_actual_start as f64, (b.t_actual_end + 120) as f64)
        } else {
            ((kb + b.t_range[0]) as f64, (kb + b.t_range[1] + 120) as f64)
        };

        let new_query = format!(
            "SELECT * FROM {} WHERE t >= to_timestamp({}) AND t < to_timestamp({})",
            view_name,
            t_start as i64,
            t_end as i64
        );
        let block_changed = explain_last_block.get_untracked().as_ref() != Some(&b);
        explain_last_block.set(Some(b));
        if block_changed || !explain_user_edited.get_untracked() {
            explain_query.set(new_query);
            explain_user_edited.set(false);
        }

        if view_name.is_empty() {
            slice_data.set(None);
            return;
        }

        let url = format!(
            "{}/api/slice/{}?t_start={}&t_end={}",
            api_base_slice, view_name, t_start, t_end
        );
        leptos::task::spawn_local(async move {
            let start = js_sys::Date::now();
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await
                && let Ok(mut sr) = resp.json::<SliceResponse>().await {
                    sr.fetch_ms = js_sys::Date::now() - start;
                    if let Some(first) = sr.rows.first() {
                        console_log!("SLICE DATA (first row): {:?}", first);
                    }
                    slice_data.set(Some(sr));
                }
        });
    });

    let api_base_explain = Arc::clone(&api_base);
    let run_explain = move || {
        let q = explain_query.get_untracked();
        if q.is_empty() {
            return;
        }
        explain_running.set(true);
        explain_result.set(None);
        let url = format!("{}/api/explain", api_base_explain);
        leptos::task::spawn_local(async move {
            let res = gloo_net::http::Request::post(&url)
                .json(&serde_json::json!({ "query": q }))
                .unwrap()
                .send()
                .await;
            explain_running.set(false);
            if let Ok(r) = res
                && let Ok(er) = r.json::<ExplainResult>().await {
                    explain_result.set(Some(er));
                }
        });
    };

    // Auto-run EXPLAIN when page selected (#78)
    let run_explain_auto = run_explain.clone();
    Effect::new(move |_| {
        let block = selected_block.get();
        if block.is_some() && auto_explain.get() {
            run_explain_auto();
        }
    });

    view! {
        <div id="app">
            <header class="topbar">
                <div class="brand">"VORTEX"</div>
                <div class="tab-row">
                    {move || {
                        let current_config = config.get();
                        if current_config.hierarchies.is_empty() {
                            view! {
                                <span class="no-tables">"connecting..."</span>
                            }.into_any()
                        } else {
                            current_config.hierarchies.into_iter().map(|h| {
                                let t = h.base_view.clone();
                                let t_class = t.clone();
                                view! {
                                    <button
                                        class=move || if selected_base_view.get() == t_class { "tab tab-active" } else { "tab" }
                                        on:click=move |_| {
                                            selected_base_view.set(t.clone());
                                            selected_tier_view.set(None);
                                            selected_block.set(None);
                                            selected_page_no.set(None);
                                            dirty_page_nos.set(BTreeSet::new());
                                            stale_blocks.set(BTreeSet::new());
                                        }
                                    >{h.base_view.to_uppercase()}</button>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </div>
                <div class="topbar-spacer"></div>
                <div class="topbar-stats">
                    <div class="ts-item">
                        <span class="ts-label">"COUNT"</span>
                        <span class="ts-value">{move || current_stats.get().row_count}</span>
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
                        <HierarchyTree hierarchy=current_hierarchy_opt selected_tier=selected_tier_view kickoff=kickoff />
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
                        {move || {
                            let k = kickoff.get();
                            let s = current_stats.get();
                            let fs = current_frame_seconds.get().max(1) as i64;
                            let scale = s.tenant_scale.max(1);

                            if let Some(b) = selected_block.get() {
                                let kb = if b.kickoff_epoch > 0 { b.kickoff_epoch } else { k };
                                let start_t = if b.t_actual_start > 0 { b.t_actual_start } else { kb + b.t_range[0] };
                                let end_t = if b.t_actual_end > 0 { b.t_actual_end } else { kb + b.t_range[1] };
                                let span = (end_t - start_t).abs() as i32;
                                if start_t > 0 || end_t > 0 {
                                    view! {
                                        <span class="page-meta" style="margin-left:auto; color:var(--blue); font-weight:700;">
                                            {format!("{} → {} ({})", format_epoch_seconds(start_t), format_epoch_seconds(end_t), format_timespan(span))}
                                        </span>
                                    }.into_any()
                                } else {
                                    view! { <span class="page-meta" style="margin-left:auto;"></span> }.into_any()
                                }
                            } else if s.total_pages > 0 && (k > 0 || s.min_t > 0) {
                                let (start_t, end_t, duration) = if s.min_t > 0 && s.max_t > 0 {
                                    (s.min_t, s.max_t, (s.max_t - s.min_t) as i32)
                                } else {
                                    let num_steps = (s.total_pages + scale - 1) / scale;
                                    let total_duration = num_steps * fs;
                                    (k, k + total_duration, total_duration as i32)
                                };

                                if start_t > 0 || end_t > 0 {
                                    view! {
                                        <span class="page-meta" style="margin-left:auto; color:var(--blue); opacity: 0.8;">
                                            {format!("{} → {} ({})", format_epoch_seconds(start_t), format_epoch_seconds(end_t), format_timespan(duration))}
                                        </span>
                                    }.into_any()
                                } else {
                                    view! { <span class="page-meta" style="margin-left:auto;"></span> }.into_any()
                                }
                            } else {
                                view! { <span class="page-meta" style="margin-left:auto;"></span> }.into_any()
                            }
                        }}
                    </div>
                    <div class="map-legend">
                        <span class="legend-title">"COLORS:"</span>
                        {move || {
                            let scale = tenant_scale.get().max(1) as i32;
                            let steps = 8.min(scale);
                            (0..steps).map(|i| {
                                let idx = if steps == 1 { 0 } else { i * (scale - 1) / (steps - 1) };
                                let color = get_color_for_tenant(idx, scale);
                                view! {
                                    <span
                                        class="legend-swatch"
                                        style=format!("background:{}", color)
                                        title=format!("tenant {}", idx)
                                    ></span>
                                }
                            }).collect_view()
                        }}
                        <span class="legend-label">"tenant id"</span>
                        <span class="legend-sep">"·"</span>
                        <span class="legend-swatch legend-dirty-swatch"></span>
                        <span class="legend-label">"dirty"</span>
                        <span class="legend-sep">"·"</span>
                        <span class="legend-swatch legend-stale-swatch"></span>
                        <span class="legend-label">"stale"</span>
                        <span class="legend-sep">"·"</span>
                        <span class="legend-swatch legend-selected-swatch"></span>
                        <span class="legend-label">"selected"</span>
                    </div>

                    // Time-range axis above page map (#73)
                    {move || {
                        let s = current_stats.get();
                        let k = kickoff.get();
                        let fs = current_frame_seconds.get().max(1) as i64;
                        let total = s.total_pages;
                        let scale = s.tenant_scale.max(1);
                        if total <= 0 { return view! { <div></div> }.into_any(); }
                        let (t_start, t_end) = if s.min_t > 0 && s.max_t > 0 {
                            (s.min_t, s.max_t)
                        } else if k > 0 {
                            let num_steps = (total + scale - 1) / scale;
                            (k, k + num_steps * fs)
                        } else {
                            return view! { <div></div> }.into_any();
                        };
                        let range = t_end - t_start;
                        if range <= 0 { return view! { <div></div> }.into_any(); }
                        let labels: Vec<(i32, String)> = (0i64..=4).map(|i| {
                            let t = t_start + range * i / 4;
                            (i as i32 * 25, format_date_short(t))
                        }).collect();
                        view! {
                            <div class="time-axis">
                                {labels.into_iter().map(|(pct, label)| view! {
                                    <span
                                        class="time-axis-label"
                                        style=format!("left:{}%", pct)
                                    >{label}</span>
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }}

                    <PageMap
                        total_pages=Signal::derive(move || current_stats.get().total_pages)
                        tenant_scale=tenant_scale
                        kickoff=kickoff
                        frame_seconds=current_frame_seconds
                        selected_page=Signal::derive(move || selected_page_no.get())
                        dirty_blocks=Signal::derive(move || dirty_page_nos.get())
                        stale_blocks=Signal::derive(move || stale_blocks.get())
                        on_click=fetch_block_info
                    />

                    {move || {
                        let sources = current_hierarchy_opt.get().map(|h| h.sources).unwrap_or_default();
                        slice_data.get().map(|sr| view! {
                            <PageDataCharts slice=sr sources=sources />
                        })
                    }}

                    {move || selected_block.get().map(|_| {
                        let run = run_explain.clone();
                        view! {
                            <div class="query-panel">
                                <div class="qp-hdr">
                                    <span class="panel-hdr" style="border:none;margin:0;padding:0;">"EXPLAIN ANALYZE"</span>
                                    <div style="display:flex;align-items:center;gap:8px;">
                                        <label class="auto-explain-toggle">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || auto_explain.get()
                                                on:change=move |_| {
                                                    let new_val = !auto_explain.get_untracked();
                                                    auto_explain.set(new_val);
                                                    if let Some(ls) = web_sys::window()
                                                        .and_then(|w| w.local_storage().ok().flatten())
                                                    {
                                                        let _ = ls.set_item("auto_explain", if new_val { "true" } else { "false" });
                                                    }
                                                }
                                            />
                                            "auto-run"
                                        </label>
                                        <button
                                            class="run-btn"
                                            on:click=move |_| run()
                                            disabled=move || explain_running.get()
                                        >
                                            {move || if explain_running.get() { "running…" } else { "▶ RUN" }}
                                        </button>
                                    </div>
                                </div>
                                <textarea
                                    class="query-input"
                                    prop:value=move || explain_query.get()
                                    on:input=move |ev| {
                                        explain_query.set(event_target_value(&ev));
                                        explain_user_edited.set(true);
                                    }
                                    rows="4"
                                    spellcheck="false"
                                />
                                {move || explain_result.get().map(|er| {
                                    let dur = er.duration_ms;
                                    let ok = er.ok;
                                    view! {
                                        <div class="explain-out">
                                            <div class={if ok { "er-meta er-ok" } else { "er-meta er-err" }}>
                                                {if ok { format!("✓ {:.1}ms", dur) }
                                                 else { format!("✗ {}", er.error.clone()) }}
                                            </div>
                                            <div class="er-lines">
                                                {er.lines.into_iter().map(|line| {
                                                    let cls = if line.contains("Spiral") || line.contains("spiral") {
                                                        "er-line er-spiral"
                                                    } else if line.contains("Index") {
                                                        "er-line er-index"
                                                    } else if line.contains("Buffers:") {
                                                        "er-line er-buffers"
                                                    } else {
                                                        "er-line"
                                                    };
                                                    view! { <div class={cls}>{line}</div> }
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    }
                                })}
                            </div>
                        }
                    })}

                    <div class="ticker">
                        {move || last_event.get().unwrap_or_else(|| {
                            if is_connected.get() {
                                "waiting for data...".into()
                            } else {
                                "connecting...".into()
                            }
                        })}
                    </div>

                    // Changelog buffer panel (#74)
                    <div class="changelog-panel">
                        <div class="changelog-hdr" on:click=move |_| changelog_expanded.update(|v| *v = !*v)>
                            {move || {
                                let count = changelog_buffer.get().len();
                                if changelog_expanded.get() {
                                    format!("▼ CHANGES ({})", count)
                                } else {
                                    format!("▶ CHANGES ({})", count)
                                }
                            }}
                        </div>
                        {move || changelog_expanded.get().then(|| {
                            let entries: Vec<_> = changelog_buffer.get().into_iter().collect();
                            view! {
                                <div class="changelog-list">
                                    {entries.into_iter().map(|e| view! {
                                        <div class="changelog-row">
                                            <span class="cl-id">"#"{e.event_id}</span>
                                            <span class="cl-base">{e.base_view}</span>
                                            <span class="cl-time">{format_epoch_seconds(e.t_start)}</span>
                                            <span class="cl-sep">"→"</span>
                                            <span class="cl-time">{format_epoch_seconds(e.t_end)}</span>
                                        </div>
                                    }).collect_view()}
                                </div>
                            }
                        })}
                    </div>
                </div>

                <aside class="right-panel">
                    {move || {
                        let block = selected_block.get();
                        let k = kickoff.get();
                        block.map(|b| view! {
                            <BlockInspector block=b kickoff=k />
                        })
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
