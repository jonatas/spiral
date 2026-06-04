use futures_util::StreamExt;
use gloo_net::websocket::Message;
use gloo_net::websocket::futures::WebSocket;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
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
    #[serde(default)]
    heap_bytes_per_row: f64,
    #[serde(default)]
    heap_rows_per_page: f64,
    #[serde(default)]
    xor_bytes_per_row: f64,
    #[serde(default)]
    xor_rows_per_page: f64,
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

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct WorkerInfo {
    pid: i32,
    state: String,
    duration_ms: i64,
    query_snippet: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct ChangelogSummaryRow {
    base_view: String,
    pending_count: i64,
    oldest_age_seconds: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", content = "data")]
enum VortexEvent {
    ChangelogUpdate(ChangelogEntry),
    StorageStats(StorageStats),
    SystemConfig(SystemConfig),
    WorkerUpdate { workers: Vec<WorkerInfo>, summary: Vec<ChangelogSummaryRow> },
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

#[derive(Serialize, Deserialize, Clone, Debug)]
struct PageTimeRange {
    blkno: i32,
    t_start: i64,
    t_end: i64,
}

// No hardcoded storage constants — all values come from the server's spiral_get_storage_stats
// which derives them from pg_attribute + pg_type for heap and from CompressedBlock struct for XOR.

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
        if years < 1.1 {
            "1 year".into()
        } else {
            format!("{:.1}y", years)
        }
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

fn format_tick_label(epoch: i64, span_seconds: i64) -> String {
    if epoch <= 0 {
        return "?".to_string();
    }
    let date = js_sys::Date::new(&JsValue::from_f64(epoch as f64 * 1000.0));
    if span_seconds < 86400 {
        format!("{:02}:{:02}", date.get_utc_hours(), date.get_utc_minutes())
    } else {
        format!(
            "{:04}-{:02}-{:02}",
            date.get_utc_full_year(),
            date.get_utc_month() + 1,
            date.get_utc_date()
        )
    }
}

// Parse a timestamptz string or numeric epoch to seconds-since-epoch.
fn parse_time_col(val: &serde_json::Value) -> Option<f64> {
    if let Some(f) = val.as_f64() {
        return Some(f);
    }
    if let Some(s) = val.as_str() {
        let ms = js_sys::Date::parse(s);
        if ms.is_finite() {
            return Some(ms / 1000.0);
        }
    }
    None
}

// Format a timestamptz string in local timezone as "YYYY-MM-DD HH:MM:SS".
// Returns (short_label, full_tooltip). short_label omits sub-second fraction.
fn format_ts_local(s: &str) -> (String, String) {
    let ms = js_sys::Date::parse(s);
    if !ms.is_finite() {
        return (s.to_string(), s.to_string());
    }
    let date = js_sys::Date::new(&JsValue::from_f64(ms));
    let short = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds(),
    );
    let full = format!("{} ({} ms)", short, date.get_milliseconds());
    (short, full)
}

fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{:.1}m", seconds as f64 / 60.0)
    } else if seconds < 86400 {
        format!("{:.1}h", seconds as f64 / 3600.0)
    } else {
        format!("{:.1}d", seconds as f64 / 86400.0)
    }
}

// High-contrast palette for dark background (#0d1117), all ≥ 4.5:1 contrast ratio
const TABLE_PALETTE: &[&str] = &[
    "#3fb950", "#58a6ff", "#ff7b72", "#d2a8ff", "#ffa657",
    "#79c0ff", "#56d364", "#e3b341", "#f778ba", "#40c4ff",
    "#ff9800", "#b39ddb", "#00e5ff", "#69ff47", "#ff4081",
];

fn palette_index(base_view: &str) -> usize {
    let mut h: u32 = 0x811c9dc5;
    for b in base_view.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h as usize % TABLE_PALETTE.len()
}

fn default_table_color(base_view: &str) -> String {
    TABLE_PALETTE[palette_index(base_view)].to_string()
}

fn resolve_table_color(base_view: &str, overrides: &BTreeMap<String, String>) -> String {
    overrides.get(base_view).cloned().unwrap_or_else(|| default_table_color(base_view))
}

fn hex_luminance(hex: &str) -> f64 {
    let h = hex.trim_start_matches('#');
    if h.len() < 6 { return 0.0; }
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0) as f64 / 255.0;
    let r = parse(&h[0..2]);
    let g = parse(&h[2..4]);
    let b = parse(&h[4..6]);
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn is_sufficient_contrast(hex: &str) -> bool {
    let bg = hex_luminance("#0d1117");
    let fg = hex_luminance(hex);
    let (l1, l2) = if fg > bg { (fg, bg) } else { (bg, fg) };
    (l1 + 0.05) / (l2 + 0.05) >= 3.0
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
    let Some(first) = slice.rows.first() else {
        return vec![];
    };
    let Some(obj) = first.as_object() else {
        return vec![];
    };

    let mut charts = Vec::new();
    for (k, v) in obj {
        let s = k.as_str();
        if s == "t_epoch" || s == slice.time_col || s == slice.scope_col {
            continue;
        }

        // Find source formula if available
        let formula = sources
            .iter()
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
    page_times: Signal<HashMap<i32, (i64, i64)>>,
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
                                    let tenant_id = idx % scale;
                                    let times = page_times.get();
                                    let t_str = times.get(&idx).map(|(t_start, t_end)| {
                                        format!("\nTime: {} – {}",
                                            format_epoch_seconds(*t_start),
                                            format_epoch_seconds(*t_end))
                                    }).unwrap_or_default();
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
    let kb = if block.kickoff_epoch > 0 {
        block.kickoff_epoch
    } else {
        kickoff
    };
    let start_t = if block.t_actual_start > 0 {
        block.t_actual_start
    } else {
        kb + block.t_range[0]
    };
    let end_t = if block.t_actual_end > 0 {
        block.t_actual_end
    } else {
        kb + block.t_range[1]
    };
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

    let fill_class = if block.fill_pct > 80.0 {
        "ivalue good"
    } else if block.fill_pct > 40.0 {
        "ivalue warn"
    } else if block.fill_pct > 0.0 {
        "ivalue bad"
    } else {
        "ivalue"
    };

    let has_fill = block.fill_pct > 0.0 || block.live_tuples > 0;

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
                {(has_fill && block.fill_pct < 20.0).then(|| view! {
                    <span class="badge-sparse">"SPARSE"</span>
                })}
                {block.is_gap_page.then(|| view! {
                    <span class="badge-gap">"GAP"</span>
                })}
                {(!block.magic_valid && !has_fill).then(|| view! {
                    <span class="badge-gap">"EMPTY"</span>
                })}
            </div>
            <div class="irow">
                <span class="ilabel">"capacity"</span>
                <span class="ivalue">{block.tuple_count} " slots"</span>
            </div>
            {has_fill.then(|| view! {
                <div>
                    <div class="irow">
                        <span class="ilabel">"fill"</span>
                        <span class={fill_class}>{format!("{:.1}%", block.fill_pct)}</span>
                    </div>
                    <div class="irow">
                        <span class="ilabel">"live slots"</span>
                        <span class="ivalue">{block.live_tuples}</span>
                    </div>
                    <div class="irow">
                        <span class="ilabel">"unused"</span>
                        <span class={if block.unused_slots > 500 { "ivalue warn" } else { "ivalue" }}>
                            {block.unused_slots}
                        </span>
                    </div>
                </div>
            })}
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

            // Raw SpiralPageOpaque section (#61)
            {(block.opaque_window_start_t != 0 || block.opaque_window_end_t != 0 || block.magic_valid).then(|| {
                let computed_start = start_t;
                let computed_end = end_t;
                let actual_start = block.opaque_window_start_t;
                let actual_end = block.opaque_window_end_t;
                let start_drift = (actual_start - computed_start).abs();
                let end_drift = (actual_end - computed_end).abs();
                let has_drift = start_drift > 1 || end_drift > 1;
                let scale_mismatch = block.opaque_tenant_scale > 0
                    && block.opaque_tenant_scale != block.tenant_range[1] - block.tenant_range[0] + 1;

                view! {
                    <div class="raw-header-section">
                        <div class="raw-hdr-title">
                            "RAW HEADER"
                            {(!block.magic_valid).then(|| view! {
                                <span class="badge-corrupt">"BAD MAGIC"</span>
                            })}
                            {has_drift.then(|| view! {
                                <span class="badge-corrupt">"DRIFT"</span>
                            })}
                        </div>
                        <div class="irow">
                            <span class="ilabel">"magic"</span>
                            <span class={if block.magic_valid { "ivalue good" } else { "ivalue bad" }}>
                                {if block.magic_valid { "✓ valid" } else { "✗ invalid" }}
                            </span>
                        </div>
                        <div class="irow-full">
                            <span class="ilabel">"stored start"</span>
                            <div class={if start_drift > 1 { "ivalue ivalue-ts ivalue-drift" } else { "ivalue ivalue-ts" }}>
                                {format_epoch_seconds(actual_start)}
                                {(start_drift > 1).then(|| format!(" (Δ{}s)", start_drift))}
                            </div>
                        </div>
                        <div class="irow-full">
                            <span class="ilabel">"stored end"</span>
                            <div class={if end_drift > 1 { "ivalue ivalue-ts ivalue-drift" } else { "ivalue ivalue-ts" }}>
                                {format_epoch_seconds(actual_end)}
                                {(end_drift > 1).then(|| format!(" (Δ{}s)", end_drift))}
                            </div>
                        </div>
                        {scale_mismatch.then(|| view! {
                            <div class="irow">
                                <span class="ilabel">"stored scale"</span>
                                <span class="ivalue bad">{block.opaque_tenant_scale} " ≠ computed"</span>
                            </div>
                        })}
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn CompressionPanel(stats: Signal<StorageStats>) -> impl IntoView {
    let row_count_exp = RwSignal::new(6.0_f64);

    // Actual Spiral bytes/row from live storage data
    let spiral_bpr = move || {
        let s = stats.get();
        if s.total_rows_capacity > 0 && s.spiral_size_kb > 0 {
            (s.spiral_size_kb * 1024) as f64 / s.total_rows_capacity as f64
        } else {
            8.0
        }
    };

    // Heap bytes/row from server (pg_attribute + typalign alignment rules)
    // Falls back to 48 (the MAXALIGN-correct value for a 3-col IoT schema)
    let heap_bpr = move || {
        let s = stats.get();
        if s.heap_bytes_per_row > 0.0 { s.heap_bytes_per_row } else { 48.0 }
    };

    // XOR-Block bytes/row: BLOCK_SIZE / VALUES_PER_XOR_BLOCK = 128 / 61
    // Derived from CompressedBlock struct on the server, not hardcoded here
    let xor_bpr = move || {
        let s = stats.get();
        if s.xor_bytes_per_row > 0.0 { s.xor_bytes_per_row } else { 128.0 / 61.0 }
    };

    let spiral_rows_per_page = move || {
        let s = stats.get();
        if s.data_per_page > 0 { s.data_per_page as f64 } else { 1018.0 }
    };

    // Heap rows/page: (BLCKSZ - PageHeaderData) / (tuple_size + ItemId)
    let heap_rows_per_page = move || {
        let s = stats.get();
        if s.heap_rows_per_page > 0.0 { s.heap_rows_per_page } else { 8168.0 / (heap_bpr() + 4.0) }
    };

    // XOR rows/page: (DATA_PER_PAGE / 16 slots/block) × 61 values/block = 3843
    let xor_rows_per_page = move || {
        let s = stats.get();
        if s.xor_rows_per_page > 0.0 { s.xor_rows_per_page } else { (1018.0_f64 / 16.0).floor() * 61.0 }
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
                <span class="cmp-val">{move || format!("{:.0} B", heap_bpr())}</span>
            </div>
            <div class="cmp-bar-row">
                <span class="cmp-lbl">"Spiral"</span>
                <div class="cmp-outer">
                    <div class="cmp-inner cmp-spiral" style={move || {
                        format!("width:{:.1}%;", (spiral_bpr() / heap_bpr() * 100.0).min(100.0))
                    }}></div>
                </div>
                <span class="cmp-val">{move || format!("{:.1} B", spiral_bpr())}</span>
            </div>
            <div class="cmp-bar-row">
                <span class="cmp-lbl">"XOR"</span>
                <div class="cmp-outer">
                    <div class="cmp-inner cmp-xor" style={move || {
                        format!("width:{:.1}%;", xor_bpr() / heap_bpr() * 100.0)
                    }}></div>
                </div>
                <span class="cmp-val">{move || format!("{:.1} B", xor_bpr())}</span>
            </div>

            <div class="cmp-section-label" style="margin-top:10px;">"IO TAX — PAGES / 1K ROWS"</div>
            <div class="three-grid">
                <div class="tg-cell">
                    <span class="tg-label">"HEAP"</span>
                    <span class="tg-value tg-red">{move || format!("{:.1}", 1000.0_f64 / heap_rows_per_page())}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SPIRAL"</span>
                    <span class="tg-value tg-green">{move || format!("{:.1}", 1000.0_f64 / spiral_rows_per_page())}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"XOR-BK"</span>
                    <span class="tg-value tg-purple">{move || format!("{:.2}", 1000.0_f64 / xor_rows_per_page())}</span>
                </div>
            </div>

        </div>
    }
}

#[component]
fn SavingsCalculatorPanel(stats: Signal<StorageStats>) -> impl IntoView {
    let row_count_exp = RwSignal::new(6.0_f64);

    let spiral_bpr = move || {
        let s = stats.get();
        if s.total_rows_capacity > 0 && s.spiral_size_kb > 0 {
            (s.spiral_size_kb * 1024) as f64 / s.total_rows_capacity as f64
        } else { 8.0 }
    };
    let heap_bpr = move || {
        let s = stats.get();
        if s.heap_bytes_per_row > 0.0 { s.heap_bytes_per_row } else { 48.0 }
    };
    let xor_bpr = move || {
        let s = stats.get();
        if s.xor_bytes_per_row > 0.0 { s.xor_bytes_per_row } else { 2.1 }
    };

    view! {
        <div class="calc-panel">
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
            <div class="four-grid">
                <div class="tg-cell">
                    <span class="tg-label">"HEAP"</span>
                    <span class="tg-value tg-red">{move || {
                        let bytes = 10.0_f64.powf(row_count_exp.get()) * heap_bpr();
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
                        let bytes = 10.0_f64.powf(row_count_exp.get()) * xor_bpr();
                        if bytes >= 1099511627776.0 { format!("{:.2}TB", bytes / 1099511627776.0) }
                        else if bytes >= 1073741824.0 { format!("{:.1}GB", bytes / 1073741824.0) }
                        else { format!("{:.0}MB", bytes / 1048576.0) }
                    }}</span>
                </div>
                <div class="tg-cell">
                    <span class="tg-label">"SAVED"</span>
                    <span class="tg-value tg-green">{move || {
                        format!("{:.1}%", (1.0 - spiral_bpr() / heap_bpr()) * 100.0)
                    }}</span>
                </div>
            </div>
            <div class="cmp-section-label" style="margin-top:8px;">"1B ROWS PROJECTION"</div>
            <div class="ls-row">
                <span class="ls-label">"Storage"</span>
                <span class="ls-value tg-purple">{move || format_bytes((1_000_000_000.0 * spiral_bpr() / 1024.0) as i64)}</span>
            </div>
            <div class="ls-row">
                <span class="ls-label">"Heap equiv"</span>
                <span class="ls-value ls-dim">{move || format_bytes((1_000_000_000.0 * heap_bpr() / 1024.0) as i64)}</span>
            </div>
            <div class="ls-row">
                <span class="ls-label">"Saving"</span>
                <span class="ls-value ls-green">{move || format!("{:.1}%", (1.0 - spiral_bpr() / heap_bpr()) * 100.0)}</span>
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
    time_col: String,
) -> impl IntoView {
    const W: f64 = 300.0;
    const H: f64 = 140.0;
    const MX: f64 = 6.0;
    const MY: f64 = 16.0;
    const MB: f64 = 24.0;
    let plot_w = W - MX * 2.0;
    let plot_h = H - MY - MB;
    let main_h = if volume_col.is_some() {
        plot_h * 0.7
    } else {
        plot_h
    };
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
        let Some(t) = row.get(&time_col).and_then(|v| parse_time_col(v)) else {
            continue;
        };
        let Some(m) = row.get(&metric_col).and_then(|v| v.as_object()) else {
            continue;
        };
        let o = m.get("open").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let h = m.get("high").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let l = m.get("low").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let c = m.get("close").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vol = volume_col
            .as_ref()
            .and_then(|vc| row.get(vc))
            .and_then(|v| v.as_f64())
            .or_else(|| m.get("volume").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);

        let scope = row.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0);
        if !has_data {
            t_min = t;
            t_max = t;
            v_min = l;
            v_max = h;
            vol_max = vol;
            has_data = true;
        } else {
            if t < t_min {
                t_min = t;
            }
            if t > t_max {
                t_max = t;
            }
            if l < v_min {
                v_min = l;
            }
            if h > v_max {
                v_max = h;
            }
            if vol > vol_max {
                vol_max = vol;
            }
        }
        by_tenant
            .entry(scope)
            .or_default()
            .push(Candle { t, o, h, l, c, vol });
    }

    if t_max <= t_min {
        t_max = t_min + 1.0;
    }
    if v_max <= v_min {
        v_max = v_min + 1.0;
    }
    if vol_max <= 0.0 {
        vol_max = 1.0;
    }

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
    let vmax_s = if has_data {
        format!("{:.1}", v_max)
    } else {
        "—".to_string()
    };
    let vmin_s = if has_data {
        format!("{:.1}", v_min)
    } else {
        "—".to_string()
    };
    let tstart_s = if has_data {
        format_short_time(t_min)
    } else {
        "".to_string()
    };
    let tend_s = if has_data && t_max > t_min {
        format_short_time(t_max)
    } else {
        "".to_string()
    };

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
    time_col: String,
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
        let Some(t) = row.get(&time_col).and_then(|v| parse_time_col(v)) else {
            continue;
        };
        let Some(v) = row.get(&metric_col).and_then(|v| v.as_f64()) else {
            continue;
        };
        let scope = row.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0);

        let t_i = t as i64;
        let entry = by_time.entry(t_i).or_default();
        *entry.entry(scope).or_default() += v;

        if !has_data {
            t_min = t;
            t_max = t;
            has_data = true;
        } else {
            if t < t_min {
                t_min = t;
            }
            if t > t_max {
                t_max = t;
            }
        }
    }

    for scopes_map in by_time.values() {
        let sum: f64 = scopes_map.values().sum();
        if sum > v_max {
            v_max = sum;
        }
    }

    let mut unique_scopes: Vec<i64> = rows
        .iter()
        .map(|r| r.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0))
        .collect();
    unique_scopes.sort();
    unique_scopes.dedup();
    let n_scopes = unique_scopes.len() as i32;
    let scope_colors: BTreeMap<i64, String> = unique_scopes
        .into_iter()
        .enumerate()
        .map(|(i, s)| (s, get_color_for_tenant(i as i32, n_scopes)))
        .collect();

    if t_max <= t_min {
        t_max = t_min + 1.0;
    }
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

    let bars = by_time
        .into_iter()
        .map(|(t, scopes_map)| {
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
        })
        .collect_view();

    let label = format!("{} (sum)", metric_col);
    let vmax_s = if has_data {
        format!("{:.1}", v_max)
    } else {
        "—".to_string()
    };
    let tstart_s = if has_data {
        format_short_time(t_min)
    } else {
        "".to_string()
    };
    let tend_s = if has_data && t_max > t_min {
        format_short_time(t_max)
    } else {
        "".to_string()
    };
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
    time_col: String,
) -> impl IntoView {
    let mut rows_mean = Vec::new();
    let mut rows_var = Vec::new();
    let mut rows_max = Vec::new();
    let mut rows_min = Vec::new();

    for row in &rows {
        let Some(t_epoch) = row.get(&time_col).and_then(|v| parse_time_col(v)) else {
            continue;
        };
        let t_val = serde_json::json!(t_epoch);
        let Some(scope) = row.get(&scope_col).cloned() else {
            continue;
        };
        let Some(obj) = row.get(&metric_col).and_then(|v| v.as_object()) else {
            continue;
        };

        let n = obj.get("n").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let m1 = obj.get("m1").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let m2 = obj.get("m2").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let max = obj.get("max").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let min = obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let variance = if n > 1.0 { m2 / (n - 1.0) } else { 0.0 };

        let mut r_mean = serde_json::Map::new();
        r_mean.insert("t_epoch".to_string(), t_val.clone());
        r_mean.insert(scope_col.clone(), scope.clone());
        r_mean.insert("val".to_string(), serde_json::json!(m1));
        rows_mean.push(serde_json::Value::Object(r_mean));

        let mut r_var = serde_json::Map::new();
        r_var.insert("t_epoch".to_string(), t_val.clone());
        r_var.insert(scope_col.clone(), scope.clone());
        r_var.insert("val".to_string(), serde_json::json!(variance));
        rows_var.push(serde_json::Value::Object(r_var));

        let mut r_max = serde_json::Map::new();
        r_max.insert("t_epoch".to_string(), t_val.clone());
        r_max.insert(scope_col.clone(), scope.clone());
        r_max.insert("val".to_string(), serde_json::json!(max));
        rows_max.push(serde_json::Value::Object(r_max));

        let mut r_min = serde_json::Map::new();
        r_min.insert("t_epoch".to_string(), t_val.clone());
        r_min.insert(scope_col.clone(), scope.clone());
        r_min.insert("val".to_string(), serde_json::json!(min));
        rows_min.push(serde_json::Value::Object(r_min));
    }

    view! {
        <div class="stats-group" style="display:grid; grid-template-columns: 1fr 1fr; gap: 8px; border: 1px solid var(--border); padding: 8px; border-radius: 4px; grid-column: 1 / -1;">
            <div style="grid-column: 1 / -1; font-size: 10px; font-weight: 700; color: var(--muted); margin-bottom: 4px;">{format!("{} (stats breakdown)", metric_col)}</div>
            <div class="chart-item">
                <SvgLineChart rows={rows_mean} scope_col={scope_col.clone()} metric_col="val".to_string() time_col="t_epoch".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"MEAN"</div>
            </div>
            <div class="chart-item">
                <SvgLineChart rows={rows_var} scope_col={scope_col.clone()} metric_col="val".to_string() time_col="t_epoch".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"VARIANCE"</div>
            </div>
            <div class="chart-item">
                <SvgLineChart rows={rows_max} scope_col={scope_col.clone()} metric_col="val".to_string() time_col="t_epoch".to_string() />
                <div style="font-size:7px; color:var(--muted); text-align:center; margin-top:-18px; position:relative;">"MAX"</div>
            </div>
            <div class="chart-item">
                <SvgLineChart rows={rows_min} scope_col={scope_col.clone()} metric_col="val".to_string() time_col="t_epoch".to_string() />
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
    time_col: String,
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
        let Some(t) = row.get(&time_col).and_then(|v| parse_time_col(v)) else {
            continue;
        };
        let Some(v) = row.get(&metric_col).and_then(|v| v.as_f64()) else {
            continue;
        };
        let scope = row.get(&scope_col).and_then(|v| v.as_i64()).unwrap_or(0);
        if !has_data {
            t_min = t;
            t_max = t;
            v_min = v;
            v_max = v;
            has_data = true;
        } else {
            if t < t_min {
                t_min = t;
            }
            if t > t_max {
                t_max = t;
            }
            if v < v_min {
                v_min = v;
            }
            if v > v_max {
                v_max = v;
            }
        }
        by_tenant.entry(scope).or_default().push((t, v));
    }

    if t_max <= t_min {
        t_max = t_min + 1.0;
    }
    if v_max <= v_min {
        v_max = v_min + 1.0;
    }

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
    let vmax_s = if has_data {
        format!("{:.1}", v_max)
    } else {
        "—".to_string()
    };
    let vmin_s = if has_data {
        format!("{:.1}", v_min)
    } else {
        "—".to_string()
    };
    let tstart_s = if has_data {
        format_short_time(t_min)
    } else {
        "".to_string()
    };
    let tend_s = if has_data && t_max > t_min {
        format_short_time(t_max)
    } else {
        "".to_string()
    };

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
    let time_col = slice.time_col.clone();
    let scope_col = slice.scope_col.clone();
    let mut keys: Vec<String> = obj.keys()
        .filter(|k| k.as_str() != "t_epoch")
        .cloned()
        .collect();
    keys.sort_by_key(|k| {
        if k == &time_col { (0, k.clone()) }
        else if k == &scope_col { (1, k.clone()) }
        else { (2, k.clone()) }
    });

    // Detect column types by scanning all rows
    let mut col_is_bool: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut col_is_int: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for k in &keys {
        if k == &time_col { continue; }
        let mut all_bool = true;
        let mut all_int = true;
        let mut any_num = false;
        for row in &rows {
            if let Some(f) = row.get(k).and_then(|v| v.as_f64()) {
                any_num = true;
                if f != 0.0 && f != 1.0 { all_bool = false; }
                if f.fract() != 0.0 { all_int = false; }
            }
        }
        col_is_bool.insert(k.clone(), any_num && all_bool);
        col_is_int.insert(k.clone(), any_num && all_int && !all_bool);
    }

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
                        let tc = time_col.clone();
                        let cb = col_is_bool.clone();
                        let ci = col_is_int.clone();
                        view! {
                            <tr>
                                {keys_c.into_iter().map(move |k| {
                                    let val = obj.get(&k).unwrap_or(&serde_json::Value::Null);
                                    if k == tc {
                                        // Timestamp: local timezone, seconds precision, tooltip with ms
                                        let s = val.as_str().unwrap_or("");
                                        let (short, full) = format_ts_local(s);
                                        return view! {
                                            <td class="td-ts" title={full}>{short}</td>
                                        }.into_any();
                                    }
                                    let (val_str, cls) = if cb.get(&k).copied().unwrap_or(false) {
                                        let b = val.as_f64().map(|f| f != 0.0).unwrap_or(false);
                                        (if b { "true".to_string() } else { "false".to_string() }, "td-bool")
                                    } else if ci.get(&k).copied().unwrap_or(false) {
                                        let i = val.as_f64().unwrap_or(0.0) as i64;
                                        (i.to_string(), "td-num")
                                    } else if let Some(f) = val.as_f64() {
                                        (format!("{:.4}", f), "td-num")
                                    } else if let Some(i) = val.as_i64() {
                                        (i.to_string(), "td-num")
                                    } else if val.is_object() {
                                        (serde_json::to_string(val).unwrap_or_default(), "td-obj")
                                    } else {
                                        (val.to_string().replace("\"", ""), "")
                                    };
                                    view! { <td class={cls}>{val_str}</td> }.into_any()
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
    let tc = slice.time_col.clone();
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
                    let tc_c = tc.clone();
                    match chart {
                        ChartDef::Line(col) => view! {
                            <div class="chart-item">
                                <SvgLineChart rows=rows_c scope_col=sc_c metric_col=col time_col=tc_c />
                            </div>
                        }.into_any(),
                        ChartDef::Bar(col) => view! {
                            <div class="chart-item">
                                <SvgBarChart rows=rows_c scope_col=sc_c metric_col=col time_col=tc_c />
                            </div>
                        }.into_any(),
                        ChartDef::Candlestick(col) => view! {
                            <div class="chart-item" style="grid-column: 1 / -1;">
                                <SvgCandlestickChart rows=rows_c scope_col=sc_c metric_col=col volume_col=None time_col=tc_c />
                            </div>
                        }.into_any(),
                        ChartDef::Stats(col) => view! {
                            <div class="chart-item" style="grid-column: 1 / -1;">
                                <SvgStatsChart rows=rows_c scope_col=sc_c metric_col=col time_col=tc_c />
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
fn ChangelogTimeline(
    entries: Signal<VecDeque<ChangelogEntry>>,
    table_colors: Signal<BTreeMap<String, String>>,
    worker_infos: Signal<Vec<WorkerInfo>>,
    selected_base_view: Signal<String>,
    on_bar_click: impl Fn(i64, i64) + 'static + Send + Clone,
) -> impl IntoView {
    let selected_entry = RwSignal::new(None::<ChangelogEntry>);

    view! {
        <div class="changelog-timeline">
        {move || {
            let all_entries: Vec<ChangelogEntry> = entries.get().into_iter().collect();
            let colors = table_colors.get();
            let workers = worker_infos.get();
            let sel_bv = selected_base_view.get();

            if all_entries.is_empty() {
                return view! {
                    <div style="color:var(--muted); font-size:10px; padding:6px 0;">"no pending changelog entries"</div>
                }.into_any();
            }

            // Group by base_view preserving first-seen order
            let mut lanes: Vec<(String, Vec<ChangelogEntry>)> = Vec::new();
            for entry in all_entries.iter() {
                if let Some(lane) = lanes.iter_mut().find(|(bv, _)| bv == &entry.base_view) {
                    lane.1.push(entry.clone());
                } else {
                    lanes.push((entry.base_view.clone(), vec![entry.clone()]));
                }
            }

            let t0 = all_entries.iter().map(|e| e.t_start).filter(|&t| t > 0).min().unwrap_or(0);
            let t1 = all_entries.iter().map(|e| e.t_end).filter(|&t| t > 0).max().unwrap_or(t0 + 3600);
            let t_range = (t1 - t0).max(1) as f64;

            const W: f64 = 600.0;
            const LABEL_W: f64 = 88.0;
            const MX: f64 = 3.0;
            const LANE_H: f64 = 13.0;
            const LANE_GAP: f64 = 2.0;
            const AXIS_H: f64 = 14.0;
            let bar_w = W - LABEL_W - MX;
            let n = lanes.len();
            let total_h = MX + n as f64 * (LANE_H + LANE_GAP) + AXIS_H;

            // Determine which tables have active workers
            let active_tables: std::collections::BTreeSet<String> = workers.iter()
                .filter(|w| w.state == "active")
                .filter_map(|w| {
                    lanes.iter()
                        .find(|(bv, _)| w.query_snippet.contains(bv.as_str()))
                        .map(|(bv, _)| bv.clone())
                })
                .collect();
            let any_active = workers.iter().any(|w| w.state == "active");

            let lane_svgs = lanes.iter().enumerate().map(|(i, (bv, lane_entries))| {
                let lane_y = MX + i as f64 * (LANE_H + LANE_GAP);
                let color = resolve_table_color(bv, &colors);
                let is_sel = sel_bv.is_empty() || bv == &sel_bv;
                let is_active = active_tables.contains(bv) || (any_active && active_tables.is_empty());
                let label_str = if bv.len() > 12 { bv[..12].to_string() } else { bv.clone() };
                let label_weight = if bv == &sel_bv { "700" } else { "400" };
                let label_opacity = if is_sel { "1" } else { "0.45" };

                let dot = is_active.then(|| {
                    let dc = color.clone();
                    view! {
                        <circle cx={LABEL_W - 5.0} cy={lane_y + LANE_H / 2.0} r="3"
                            fill={dc} class="worker-lane-dot" />
                    }
                });

                let lane_bars = lane_entries.iter().map(|entry| {
                    let x0 = LABEL_W + ((entry.t_start - t0) as f64 / t_range * bar_w).clamp(0.0, bar_w);
                    let x1 = LABEL_W + ((entry.t_end - t0) as f64 / t_range * bar_w).clamp(0.0, bar_w);
                    let bw = (x1 - x0).max(2.0);
                    let bc = color.clone();
                    let opacity = if is_sel { "0.85" } else { "0.25" };
                    let ts = entry.t_start;
                    let te = entry.t_end;
                    let entry_cl = entry.clone();
                    let cb = on_bar_click.clone();
                    let tip = format!("#{} {} → {}",
                        entry.event_id,
                        format_epoch_seconds(ts),
                        format_epoch_seconds(te));
                    view! {
                        <rect
                            x={x0} y={lane_y} width={bw} height={LANE_H}
                            fill={bc} opacity={opacity} rx="2"
                            style="cursor:pointer;"
                            title={tip}
                            on:click=move |_| {
                                selected_entry.set(Some(entry_cl.clone()));
                                cb(ts, te);
                            }
                        />
                    }
                }).collect_view();

                let bg_opacity = if bv == &sel_bv { "0.06" } else { "0.01" };
                let lc = color.clone();
                view! {
                    <g>
                        <rect x="0" y={lane_y} width={W} height={LANE_H}
                            fill="white" opacity={bg_opacity} />
                        <text x={MX} y={lane_y + LANE_H - 3.0}
                            font-size="9" font-weight={label_weight}
                            fill={lc} opacity={label_opacity}>{label_str}</text>
                        {dot}
                        {lane_bars}
                    </g>
                }
            }).collect_view();

            let axis_y = MX + n as f64 * (LANE_H + LANE_GAP) + 11.0;
            let axis_lines = (0i64..=4).map(|i| {
                let t = t0 + (t1 - t0) * i / 4;
                let x = LABEL_W + (i as f64 / 4.0) * bar_w;
                let anchor = if i == 4 { "end" } else if i == 0 { "start" } else { "middle" };
                let label = format_tick_label(t, t1 - t0);
                let full_ts = format_epoch_seconds(t);
                view! {
                    <text x={x} y={axis_y} font-size="7" fill="var(--muted)" text-anchor={anchor}>
                        <title>{full_ts}</title>
                        {label}
                    </text>
                }
            }).collect_view();

            let vb = format!("0 0 {} {:.0}", W, total_h);
            view! {
                <svg viewBox={vb} style=format!("width:100%; height:{:.0}px; display:block;", total_h)>
                    <line x1={LABEL_W} y1={MX} x2={LABEL_W} y2={axis_y - 11.0}
                        stroke="var(--border)" stroke-width="0.5"/>
                    <line x1={LABEL_W} y1={axis_y - 11.0} x2={W} y2={axis_y - 11.0}
                        stroke="var(--border)" stroke-width="0.5"/>
                    {lane_svgs}
                    {axis_lines}
                </svg>
            }.into_any()
        }}
        </div>

        // Payload inspector — appears below timeline on bar click
        {move || selected_entry.get().map(|e| {
            let scope = serde_json::to_string_pretty(&e.scope_values).unwrap_or_default();
            let title = format!("#{} {} → {}",
                e.event_id,
                format_epoch_seconds(e.t_start),
                format_epoch_seconds(e.t_end));
            view! {
                <div class="cl-payload-bar">
                    <span class="cl-payload-title">{title}</span>
                    <span class="cl-payload-close" on:click=move |_| selected_entry.set(None)>"×"</span>
                    <div class="cl-payload">{scope}</div>
                </div>
            }
        })}
    }
}

#[component]
fn WorkerStrip(
    workers: Signal<Vec<WorkerInfo>>,
    summary: Signal<Vec<ChangelogSummaryRow>>,
    table_colors: Signal<BTreeMap<String, String>>,
    selected_base_view: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="worker-strip">
            {move || {
                let ws = workers.get();
                let sm = summary.get();
                let colors = table_colors.get();
                let selected = selected_base_view.get();

                if ws.is_empty() && sm.is_empty() {
                    return view! {
                        <span class="worker-idle">"○ no active workers"</span>
                    }.into_any();
                }

                let worker_pills = ws.iter().map(|w| {
                    let bv = w.query_snippet.split("spiral_refresh_scope").nth(1)
                        .and_then(|s| s.split_whitespace().next())
                        .unwrap_or("?")
                        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                        .to_string();
                    let color = resolve_table_color(&bv, &colors);
                    let dur = if w.duration_ms > 1000 {
                        format!("{:.1}s", w.duration_ms as f64 / 1000.0)
                    } else {
                        format!("{}ms", w.duration_ms)
                    };
                    let active = w.state == "active";
                    view! {
                        <span class="worker-pill" class:worker-active={active}>
                            <span class="worker-dot" style=move || format!("background:{}", color)></span>
                            <span class="worker-pid">"["{w.pid}"]"</span>
                            <span class="worker-table">{bv}</span>
                            <span class="worker-dur">{dur}</span>
                        </span>
                    }
                }).collect_view();

                let summary_pills = sm.iter().map(|row| {
                    let color = resolve_table_color(&row.base_view, &colors);
                    let is_sel = row.base_view == selected;
                    let label = format!("{}: {} ({})",
                        row.base_view,
                        row.pending_count,
                        format_age(row.oldest_age_seconds));
                    let color2 = color.clone();
                    view! {
                        <span
                            class="cl-summary-pill"
                            class:cl-summary-selected={is_sel}
                            style=move || format!("border-color:{}", color)
                        >
                            <span class="cl-summary-dot" style=move || format!("background:{}", color2)></span>
                            {label}
                        </span>
                    }
                }).collect_view();

                view! {
                    <div style="display:flex; flex-wrap:wrap; gap:4px; align-items:center;">
                        {worker_pills}
                        <span class="worker-sep">"|"</span>
                        {summary_pills}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[component]
fn TenantLegend(
    tenant_scale: Signal<i64>,
    selected_tenant: RwSignal<Option<i32>>,
    slice_data: Signal<Option<SliceResponse>>,
) -> impl IntoView {
    view! {
        <div class="tenant-legend">
            <span class="legend-title">"TENANTS:"</span>
            <button
                class=move || if selected_tenant.get().is_none() { "tl-all tl-active" } else { "tl-all" }
                on:click=move |_| selected_tenant.set(None)
            >"ALL"</button>
            {move || {
                let scale = tenant_scale.get().max(1) as i32;
                let tenant_ids: Vec<i32> = match slice_data.get() {
                    Some(sr) if !sr.rows.is_empty() && !sr.scope_col.is_empty() => {
                        let mut ids: Vec<i32> = sr.rows.iter()
                            .filter_map(|r| r.get(&sr.scope_col)
                                .and_then(|v| v.as_i64())
                                .map(|v| v as i32))
                            .collect();
                        ids.sort();
                        ids.dedup();
                        ids
                    }
                    _ => {
                        let steps = 16.min(scale);
                        if steps <= 1 { vec![0] }
                        else { (0..steps).map(|i| i * (scale - 1) / (steps - 1)).collect() }
                    }
                };
                tenant_ids.into_iter().map(|tid| {
                    let color = get_color_for_tenant(tid, scale);
                    view! {
                        <button
                            class=move || if selected_tenant.get() == Some(tid) { "tl-item tl-active" } else { "tl-item" }
                            on:click=move |_| selected_tenant.update(|s| {
                                *s = if *s == Some(tid) { None } else { Some(tid) };
                            })
                            title=format!("tenant {}", tid)
                        >
                            <span class="tl-dot" style=format!("background:{}", color)></span>
                            <span class="tl-id">{tid}</span>
                        </button>
                    }
                }).collect_view()
            }}
        </div>
    }
}

#[component]
fn MultiTierPageMap(
    hierarchy: Signal<Option<HierarchyConfig>>,
    stats_by_view: Signal<BTreeMap<String, StorageStats>>,
    tenant_scale: Signal<i64>,
    selected_tenant: Signal<Option<i32>>,
    selected_page: Signal<Option<i32>>,
    selected_tier: Signal<Option<String>>,
    dirty_blocks: Signal<BTreeSet<i32>>,
    stale_blocks: Signal<BTreeSet<i32>>,
    on_click: impl Fn(String, i32) + 'static + Send + Clone,
) -> impl IntoView {
    let tier_offsets = RwSignal::new(BTreeMap::<String, i32>::new());

    Effect::new(move |_| {
        let _ = hierarchy.get();
        tier_offsets.set(BTreeMap::new());
    });

    const CELLS_PER_TIER: i32 = 200;

    let cell_px = |fs: i32| -> usize {
        match fs {
            0             => 4,
            1..=599       => 5,
            600..=7199    => 7,
            7200..=172799 => 10,
            172800..=2591999 => 14,
            _             => 20,
        }
    };

    view! {
        <div class="tier-section">
        {move || {
            let Some(h) = hierarchy.get() else {
                return view! { <div></div> }.into_any();
            };
            let stats = stats_by_view.get();
            let scale = tenant_scale.get().max(1) as i32;

            let mut tiers: Vec<(String, i32, i64)> = h.aggregation_levels.iter()
                .filter(|l| l.frame_seconds > 0)
                .map(|l| {
                    let pages = stats.get(&l.view_name).map(|s| s.total_pages).unwrap_or(0);
                    (l.view_name.clone(), l.frame_seconds, pages)
                })
                .collect();
            tiers.sort_by(|(_, a, _), (_, b, _)| b.cmp(a));

            let raw_pages = stats.get(&h.raw_view_name).map(|s| s.total_pages).unwrap_or(0);
            tiers.push((h.raw_view_name.clone(), 0, raw_pages));

            tiers.into_iter().map(|(view_name, frame_seconds, total_pages)| {
                if total_pages == 0 { return view! { <span></span> }.into_any(); }

                let cpx = cell_px(frame_seconds);
                let tier_label = format_timespan(frame_seconds);
                let is_raw = frame_seconds == 0;

                let vn_pag1 = view_name.clone();
                let vn_pag2 = view_name.clone();
                let vn_cells = view_name.clone();
                let cb_cells = on_click.clone();

                view! {
                    <div class=if is_raw { "tier-row tier-raw" } else { "tier-row" }>
                        <div class="tier-label">
                            <span class="tier-name">{tier_label}</span>
                            <span class="tier-pages">{total_pages}{" p"}</span>
                            {(total_pages > CELLS_PER_TIER as i64).then(|| {
                                let v1a = vn_pag1.clone();
                                let v1b = vn_pag1.clone();
                                let v2a = vn_pag2.clone();
                                let v2b = vn_pag2.clone();
                                view! {
                                    <div class="tier-paginator">
                                        <button
                                            class="tier-pag-btn"
                                            disabled=move || tier_offsets.get().get(&v1a).copied().unwrap_or(0) == 0
                                            on:click=move |_| tier_offsets.update(|m| {
                                                let o = m.entry(v1b.clone()).or_default();
                                                *o = (*o - CELLS_PER_TIER).max(0);
                                            })
                                        >"◀"</button>
                                        <button
                                            class="tier-pag-btn"
                                            disabled=move || {
                                                let o = tier_offsets.get().get(&v2a).copied().unwrap_or(0);
                                                o + CELLS_PER_TIER >= total_pages as i32
                                            }
                                            on:click=move |_| tier_offsets.update(|m| {
                                                let o = m.entry(v2b.clone()).or_default();
                                                *o = (*o + CELLS_PER_TIER).min(total_pages as i32 - CELLS_PER_TIER).max(0);
                                            })
                                        >"▶"</button>
                                    </div>
                                }
                            })}
                        </div>
                        <div class="tier-cells">
                        {move || {
                            let off = tier_offsets.get().get(&vn_cells).copied().unwrap_or(0);
                            let end = (off + CELLS_PER_TIER).min(total_pages as i32);
                            let cb = cb_cells.clone();
                            (off..end).map(|idx| {
                                let tid = idx % scale;
                                let color = get_color_for_tenant(tid, scale);
                                let vn_class = vn_cells.clone();
                                let vn_click = vn_cells.clone();
                                let cb_c = cb.clone();
                                view! {
                                    <div
                                        class=move || {
                                            let sel_t = selected_tenant.get();
                                            let is_match = sel_t.map(|t| t == tid).unwrap_or(false);
                                            let has_filter = sel_t.is_some();
                                            let is_sel = selected_page.get() == Some(idx)
                                                && selected_tier.get().as_deref() == Some(vn_class.as_str());
                                            if is_sel { "page-cell selected" }
                                            else if stale_blocks.get().contains(&idx) { "page-cell stale" }
                                            else if dirty_blocks.get().contains(&idx) { "page-cell dirty" }
                                            else if has_filter && is_match { "page-cell tenant-match" }
                                            else if has_filter { "page-cell tenant-filtered" }
                                            else { "page-cell" }
                                        }
                                        style=format!("background:{}; width:{}px; height:{}px;", color, cpx, cpx)
                                        title=format!("Page {} (tenant {})", idx, tid)
                                        on:click=move |_| cb_c(vn_click.clone(), idx)
                                    ></div>
                                }
                            }).collect_view()
                        }}
                        </div>
                    </div>
                }.into_any()
            }).collect_view().into_any()
        }}
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
    let page_time_map = RwSignal::new(HashMap::<i32, (i64, i64)>::new());
    let changelog_buffer = RwSignal::new(VecDeque::<ChangelogEntry>::new());
    let changelog_expanded = RwSignal::new(false);
    let pending_page_restore = RwSignal::new(None::<i32>);
    let worker_infos = RwSignal::new(Vec::<WorkerInfo>::new());
    // 0 = "all", 300 = 5m, 3600 = 1h, 86400 = 1d
    let changelog_window_secs = RwSignal::new(0i64);
    let worker_target = RwSignal::new(1usize);
    let changelog_summary = RwSignal::new(Vec::<ChangelogSummaryRow>::new());
    let table_colors = RwSignal::new(BTreeMap::<String, String>::new());
    let selected_tenant = RwSignal::new(Option::<i32>::None);
    let suppress_tier_clear = RwSignal::new(false);
    let left_tab = RwSignal::new(0u8); // 0=hierarchy 1=storage 2=calc
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
                if !v.is_empty() {
                    table = Some(v.to_string());
                }
            } else if let Some(v) = part.strip_prefix("tier=") {
                if !v.is_empty() {
                    tier = Some(v.to_string());
                }
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

    // Runtime port: ?port=3010 overrides compile-time default (#71)
    let compile_time_default = option_env!("VORTEX_SERVER_PORT").unwrap_or("3001");
    let server_port = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|search| {
            search
                .trim_start_matches('?')
                .split('&')
                .find(|p| p.starts_with("port="))
                .and_then(|p| p.strip_prefix("port="))
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| compile_time_default.to_string());
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
                                            console_log!(
                                                "PERF StorageStats {} took {:.0}ms",
                                                s.view_name,
                                                dt
                                            );
                                        }
                                        last_event.set(Some(format!(
                                            "stats: {}/{} updated",
                                            s.base_view, s.view_name
                                        )));
                                    }
                                    VortexEvent::SystemConfig(c) => {
                                        let t0 = js_sys::Date::now();
                                        last_event.set(Some(format!(
                                            "system config: {} hierarchies",
                                            c.hierarchies.len()
                                        )));
                                        let selected = selected_base_view.get_untracked();
                                        let first_base = c
                                            .hierarchies
                                            .first()
                                            .map(|h| h.base_view.clone())
                                            .unwrap_or_default();

                                        if selected.is_empty()
                                            || !c
                                                .hierarchies
                                                .iter()
                                                .any(|h| h.base_view == selected)
                                        {
                                            let target_base = url_table
                                                .as_deref()
                                                .filter(|t| {
                                                    c.hierarchies.iter().any(|h| h.base_view == *t)
                                                })
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
                                        // Dedup: pg_notify and polling may both deliver the same entry
                                        let is_new = changelog_buffer.get_untracked()
                                            .iter()
                                            .all(|e| e.event_id != entry.event_id);
                                        if !is_new { continue; }
                                        changelog_buffer.update(|buf| {
                                            buf.push_front(entry.clone());
                                            buf.truncate(100);
                                        });
                                        last_event.set(Some(format!(
                                            "#{} {} @ {}",
                                            entry.event_id, entry.base_view, entry.t_start
                                        )));
                                        if entry.base_view == selected_base_view.get_untracked() {
                                            let h = config
                                                .get_untracked()
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
                                                let blkno_start = ((t_rel_start * tenant_scale)
                                                    / data_per_page)
                                                    .max(0)
                                                    as i32;
                                                let blkno_end = ((t_rel_end * tenant_scale)
                                                    / data_per_page
                                                    + 1)
                                                    as i32;
                                                let dirty_before =
                                                    dirty_page_nos.get_untracked().len();
                                                dirty_page_nos.update(|set| {
                                                    const MAX_DIRTY: usize = 200;
                                                    for b in blkno_start..=blkno_end {
                                                        if set.len() >= MAX_DIRTY {
                                                            break;
                                                        }
                                                        set.insert(b);
                                                    }
                                                });
                                                let dt = js_sys::Date::now() - t0;
                                                if dt > 2.0 {
                                                    console_log!(
                                                        "PERF ChangelogUpdate blks {}-{} dirty_before={} took {:.0}ms",
                                                        blkno_start,
                                                        blkno_end,
                                                        dirty_before,
                                                        dt
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    VortexEvent::WorkerUpdate { workers, summary } => {
                                        worker_infos.set(workers);
                                        changelog_summary.set(summary);
                                    }
                                },
                                Err(e) => console_log!("VortexEvent deserialization error: {}", e),
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
        if table.is_empty() {
            return;
        }
        let tier = selected_tier_view.get().unwrap_or_default();
        let page_str = selected_page_no
            .get()
            .map(|p| p.to_string())
            .unwrap_or_default();
        let hash = format!("table={}&tier={}&page={}", table, tier, page_str);
        if let Some(w) = web_sys::window() {
            let _ = w.location().set_hash(&hash);
        }
    });

    Effect::new(move |_| {
        let _ = selected_tier_view.get();
        if suppress_tier_clear.get_untracked() {
            suppress_tier_clear.set(false);
            return;
        }
        selected_block.set(None);
        selected_page_no.set(None);
        slice_data.set(None);
    });

    // Fetch initial worker count
    let api_base_workers_init = Arc::clone(&api_base);
    leptos::task::spawn_local(async move {
        if let Ok(resp) = gloo_net::http::Request::get(
            &format!("{}/api/workers", api_base_workers_init)
        ).send().await
            && let Ok(v) = resp.json::<serde_json::Value>().await
        {
            if let Some(n) = v["count"].as_u64() {
                worker_target.set(n as usize);
            }
        }
    });

    // POST /api/workers helper
    let api_base_set_workers = Arc::clone(&api_base);
    let set_worker_count = move |delta: i64| {
        let current = worker_target.get_untracked() as i64;
        let next = (current + delta).max(0).min(8) as usize;
        worker_target.set(next);
        let url = format!("{}/api/workers", api_base_set_workers);
        leptos::task::spawn_local(async move {
            let body = format!("{{\"count\":{}}}", next);
            if let Ok(req) = gloo_net::http::Request::post(&url)
                .header("Content-Type", "application/json")
                .body(body)
            {
                let _ = req.send().await;
            }
        });
    };

    let api_base_pagemap = Arc::clone(&api_base);
    Effect::new(move |_| {
        let selected = selected_base_view.get();
        let tier = selected_tier_view.get();
        let conf = config.get_untracked();
        let view_name = tier.unwrap_or_else(|| {
            conf.hierarchies
                .iter()
                .find(|h| h.base_view == selected)
                .map(|h| h.raw_view_name.clone())
                .unwrap_or_default()
        });
        if view_name.is_empty() {
            return;
        }
        let url = format!("{}/api/storage/{}/pagemap", api_base_pagemap, view_name);
        page_time_map.set(HashMap::new());
        leptos::task::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await
                && let Ok(ranges) = resp.json::<Vec<PageTimeRange>>().await
            {
                let map: HashMap<i32, (i64, i64)> = ranges
                    .into_iter()
                    .map(|r| (r.blkno, (r.t_start, r.t_end)))
                    .collect();
                page_time_map.set(map);
            }
        });
    });

    let api_base_clone = Arc::clone(&api_base);
    let _fetch_block_info = move |blkno: i32| {
        selected_block.set(None);
        slice_data.set(None);
        selected_page_no.set(Some(blkno));

        let selected = selected_base_view.get_untracked();
        let view_name = selected_tier_view.get_untracked().unwrap_or_else(|| {
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
                && let Ok(info) = resp.json::<BlockInfo>().await
            {
                console_log!("FETCHED BLOCK INFO: {:?}", info);
                selected_block.set(Some(info));
            }
        });
    };

    let api_base_tier = Arc::clone(&api_base);
    let fetch_block_for_tier = move |view_name: String, blkno: i32| {
        suppress_tier_clear.set(true);
        selected_tier_view.set(Some(view_name.clone()));
        selected_block.set(None);
        slice_data.set(None);
        selected_page_no.set(Some(blkno));
        if view_name.is_empty() { return; }
        let url = format!("{}/api/storage/{}/block/{}", api_base_tier, view_name, blkno);
        leptos::task::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await
                && let Ok(info) = resp.json::<BlockInfo>().await
            {
                selected_block.set(Some(info));
            }
        });
    };

    let api_base_bar_click = Arc::clone(&api_base);
    let on_bar_click = move |ts: i64, te: i64| {
        let base = selected_base_view.get_untracked();
        let conf = config.get_untracked();
        let Some(h) = conf.hierarchies.into_iter().find(|h| h.base_view == base) else {
            return;
        };
        let kickoff = h.kickoff_epoch;
        let tenant_scale = h.tenant_scale.max(1);
        let raw_view = h.raw_view_name.clone();
        let data_per_page = stats_by_view
            .get_untracked()
            .get(&raw_view)
            .map(|s| s.data_per_page)
            .filter(|&d| d > 0)
            .unwrap_or(1018);
        let t_rel_start = (ts - kickoff).max(0);
        let t_rel_end = (te - kickoff).max(0);
        let blkno_start = ((t_rel_start * tenant_scale) / data_per_page).max(0) as i32;
        let blkno_end = ((t_rel_end * tenant_scale) / data_per_page + 1) as i32;
        dirty_page_nos.update(|set| {
            for b in blkno_start..=blkno_end {
                if set.len() < 200 {
                    set.insert(b);
                }
            }
        });
        selected_block.set(None);
        slice_data.set(None);
        selected_page_no.set(Some(blkno_start));
        let view_name = selected_tier_view.get_untracked().unwrap_or(raw_view);
        let url = format!(
            "{}/api/storage/{}/block/{}",
            api_base_bar_click, view_name, blkno_start
        );
        leptos::task::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get(&url).send().await
                && let Ok(info) = resp.json::<BlockInfo>().await
            {
                selected_block.set(Some(info));
            }
        });
    };

    // Auto-restore page from URL hash (#75): fires when base is set + pending page exists
    let api_base_restore = Arc::clone(&api_base);
    Effect::new(move |_| {
        let base = selected_base_view.get();
        if base.is_empty() {
            return;
        }
        let Some(blkno) = pending_page_restore.get() else {
            return;
        };
        pending_page_restore.set(None);
        let view_name = selected_tier_view.get_untracked().unwrap_or_else(|| {
            config
                .get_untracked()
                .hierarchies
                .iter()
                .find(|h| h.base_view == base)
                .map(|h| h.raw_view_name.clone())
                .unwrap_or_default()
        });
        if view_name.is_empty() {
            return;
        }
        let url = format!(
            "{}/api/storage/{}/block/{}",
            api_base_restore, view_name, blkno
        );
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
                stale_blocks.update(|s| {
                    s.insert(b.blkno);
                });
            } else {
                stale_blocks.update(|s| {
                    s.remove(&b.blkno);
                });
                dirty_page_nos.update(|s| {
                    s.remove(&b.blkno);
                });
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
    })
    .into();

    // Auto-select first page when table loads and no page pending from URL
    Effect::new(move |_| {
        let stats = current_stats.get();
        if selected_page_no.get().is_none() && stats.total_pages > 0 {
            selected_page_no.set(Some(0));
        }
    });

    let current_hierarchy_opt: Signal<Option<HierarchyConfig>> =
        Memo::new(move |_| current_hierarchy(&config.get(), &selected_base_view.get())).into();

    let tenant_scale: Signal<i64> = Memo::new(move |_| {
        current_hierarchy_opt
            .get()
            .map(|h| h.tenant_scale.max(1))
            .unwrap_or(1)
    })
    .into();

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
    })
    .into();

    let current_frame_seconds: Signal<i32> = Memo::new(move |_| {
        let tier = selected_tier_view.get();
        let h = current_hierarchy_opt.get();
        match (tier, h) {
            (Some(t), Some(h)) => h
                .aggregation_levels
                .iter()
                .find(|l| l.view_name == t)
                .map(|l| l.frame_seconds)
                .unwrap_or(0),
            _ => 0,
        }
    })
    .into();

    let api_base_slice = Arc::clone(&api_base);
    Effect::new(move |_| {
        // Only track selected_block and selected_tier_view — not config or kickoff.
        // kickoff derives from stats_by_view, so tracking it would re-fire this effect
        // (and spawn a new HTTP request) on every StorageStats WS event.
        let block = selected_block.get();
        let selected = selected_base_view.get_untracked();
        let view_name = selected_tier_view.get().unwrap_or_else(|| {
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

        let kb = if b.kickoff_epoch > 0 {
            b.kickoff_epoch
        } else {
            k
        };

        let (t_start, t_end) = if b.t_actual_start > 0 {
            (b.t_actual_start as f64, (b.t_actual_end + 120) as f64)
        } else {
            ((kb + b.t_range[0]) as f64, (kb + b.t_range[1] + 120) as f64)
        };

        let new_query = format!(
            "SELECT * FROM {} WHERE t >= to_timestamp({}) AND t < to_timestamp({})",
            view_name, t_start as i64, t_end as i64
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
                && let Ok(mut sr) = resp.json::<SliceResponse>().await
            {
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
                && let Ok(er) = r.json::<ExplainResult>().await
            {
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
                                let bv = h.base_view.clone();
                                let bv_click = bv.clone();
                                let bv_class = bv.clone();
                                let label = bv.to_uppercase();
                                view! {
                                    <button
                                        class=move || if selected_base_view.get() == bv_class { "tab tab-active" } else { "tab" }
                                        on:click=move |_| {
                                            selected_base_view.set(bv_click.clone());
                                            selected_tier_view.set(None);
                                            selected_block.set(None);
                                            selected_page_no.set(None);
                                            dirty_page_nos.set(BTreeSet::new());
                                            stale_blocks.set(BTreeSet::new());
                                        }
                                        style=move || {
                                            let color = resolve_table_color(&bv, &table_colors.get());
                                            // Health: 1.0 = fresh, 0.0 = very stale (>1h)
                                            let age = changelog_summary.get()
                                                .iter()
                                                .find(|r| r.base_view == bv)
                                                .map(|r| r.oldest_age_seconds)
                                                .unwrap_or(0);
                                            let health = (1.0 - (age as f64 / 3600.0).min(1.0));
                                            let bar_w = (health * 100.0) as u32;
                                            // health color: green → yellow → red
                                            let bar_color = if health > 0.6 { "#3fb950" }
                                                else if health > 0.3 { "#e3b341" }
                                                else { "#f85149" };
                                            format!(
                                                "--tab-color:{}; --tab-health:{:.0}%; --tab-bar-color:{};",
                                                color, bar_w, bar_color
                                            )
                                        }
                                    >{label}</button>
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
                    <div class="left-tab-bar">
                        <button
                            class=move || if left_tab.get() == 0 { "ltab ltab-active" } else { "ltab" }
                            title="Hierarchy"
                            on:click=move |_| left_tab.set(0)>
                            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                                <circle cx="7" cy="2.5" r="1.5" fill="currentColor"/>
                                <circle cx="3" cy="11" r="1.5" fill="currentColor"/>
                                <circle cx="11" cy="11" r="1.5" fill="currentColor"/>
                                <line x1="7" y1="4" x2="7" y2="8" stroke="currentColor" stroke-width="1.2"/>
                                <line x1="7" y1="8" x2="3" y2="9.5" stroke="currentColor" stroke-width="1.2"/>
                                <line x1="7" y1="8" x2="11" y2="9.5" stroke="currentColor" stroke-width="1.2"/>
                            </svg>
                        </button>
                        <button
                            class=move || if left_tab.get() == 1 { "ltab ltab-active" } else { "ltab" }
                            title="Storage Analysis"
                            on:click=move |_| left_tab.set(1)>
                            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                                <ellipse cx="7" cy="3.5" rx="4.5" ry="1.6" stroke="currentColor" stroke-width="1.2"/>
                                <line x1="2.5" y1="3.5" x2="2.5" y2="10.5" stroke="currentColor" stroke-width="1.2"/>
                                <line x1="11.5" y1="3.5" x2="11.5" y2="10.5" stroke="currentColor" stroke-width="1.2"/>
                                <ellipse cx="7" cy="10.5" rx="4.5" ry="1.6" stroke="currentColor" stroke-width="1.2"/>
                                <ellipse cx="7" cy="7" rx="4.5" ry="1.6" stroke="currentColor" stroke-width="1.2" opacity="0.45"/>
                            </svg>
                        </button>
                        <button
                            class=move || if left_tab.get() == 2 { "ltab ltab-active" } else { "ltab" }
                            title="Savings Calculator"
                            on:click=move |_| left_tab.set(2)>
                            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                                <rect x="2" y="1" width="10" height="12" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                                <rect x="3.5" y="2.5" width="7" height="2.5" rx="0.5" fill="currentColor" opacity="0.65"/>
                                <rect x="3.5" y="6.5" width="2" height="1.5" rx="0.3" fill="currentColor" opacity="0.55"/>
                                <rect x="6"   y="6.5" width="2" height="1.5" rx="0.3" fill="currentColor" opacity="0.55"/>
                                <rect x="8.5" y="6.5" width="2" height="1.5" rx="0.3" fill="currentColor" opacity="0.55"/>
                                <rect x="3.5" y="9"   width="2" height="1.5" rx="0.3" fill="currentColor" opacity="0.55"/>
                                <rect x="6"   y="9"   width="2" height="1.5" rx="0.3" fill="currentColor" opacity="0.55"/>
                                <rect x="8.5" y="9"   width="2" height="1.5" rx="0.3" fill="currentColor" opacity="0.55"/>
                            </svg>
                        </button>
                    </div>
                    <div class="left-tab-content">
                        {move || match left_tab.get() {
                            1 => view! { <CompressionPanel stats=current_stats /> }.into_any(),
                            2 => view! { <SavingsCalculatorPanel stats=current_stats /> }.into_any(),
                            _ => view! { <HierarchyTree hierarchy=current_hierarchy_opt selected_tier=selected_tier_view kickoff=kickoff /> }.into_any(),
                        }}
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
                    <TenantLegend
                        tenant_scale=tenant_scale
                        selected_tenant=selected_tenant
                        slice_data=Signal::derive(move || slice_data.get())
                    />
                    <div class="map-legend">
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
                        let labels: Vec<(i32, String, String)> = (0i64..=4).map(|i| {
                            let t = t_start + range * i / 4;
                            (i as i32 * 25, format_tick_label(t, range), format_epoch_seconds(t))
                        }).collect();
                        view! {
                            <div class="time-axis">
                                {labels.into_iter().map(|(pct, label, full_ts)| view! {
                                    <span
                                        class="time-axis-label"
                                        style=format!("left:{}%", pct)
                                        title={full_ts}
                                    >{label}</span>
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }}

                    <MultiTierPageMap
                        hierarchy=current_hierarchy_opt
                        stats_by_view=Signal::derive(move || stats_by_view.get())
                        tenant_scale=tenant_scale
                        selected_tenant=Signal::derive(move || selected_tenant.get())
                        selected_page=Signal::derive(move || selected_page_no.get())
                        selected_tier=Signal::derive(move || selected_tier_view.get())
                        dirty_blocks=Signal::derive(move || dirty_page_nos.get())
                        stale_blocks=Signal::derive(move || stale_blocks.get())
                        on_click=fetch_block_for_tier
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

                    // Changelog timeline panel (#62)
                    <div class="changelog-panel">
                        <div style="display:flex; align-items:center; gap:6px;">
                            <div class="changelog-hdr" style="flex:1; cursor:pointer;"
                                on:click=move |_| changelog_expanded.update(|v| *v = !*v)>
                                {move || {
                                    let count: i64 = changelog_summary.get().iter().map(|r| r.pending_count).sum();
                                    let w_count = worker_infos.get().len();
                                    if changelog_expanded.get() {
                                        format!("▼ CHANGELOG ({} pending, {} workers)", count, w_count)
                                    } else {
                                        format!("▶ CHANGELOG ({} pending, {} workers)", count, w_count)
                                    }
                                }}
                            </div>
                            // Worker +/- controls
                            <div style="display:flex; align-items:center; gap:2px; font-size:11px;">
                                <button class="ctrl-btn"
                                    on:click={
                                        let swc = set_worker_count.clone();
                                        move |_| swc(-1)
                                    }>"-"</button>
                                <span style="min-width:18px; text-align:center; color:var(--muted);">
                                    {move || worker_target.get()}
                                </span>
                                <button class="ctrl-btn"
                                    on:click={
                                        let swc = set_worker_count.clone();
                                        move |_| swc(1)
                                    }>"+"</button>
                            </div>
                            // Time window filter
                            <div style="display:flex; gap:2px; font-size:10px;">
                                {[("5m", 300i64), ("1h", 3600), ("1d", 86400), ("all", 0)].iter().map(|(label, secs)| {
                                    let secs = *secs;
                                    let label = *label;
                                    view! {
                                        <button class="ctrl-btn"
                                            class:ctrl-btn-active={move || changelog_window_secs.get() == secs}
                                            on:click=move |_| changelog_window_secs.set(secs)>
                                            {label}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </div>

                        // Worker strip + summary pills (always visible)
                        <WorkerStrip
                            workers=Signal::derive(move || worker_infos.get())
                            summary=Signal::derive(move || changelog_summary.get())
                            table_colors=Signal::derive(move || table_colors.get())
                            selected_base_view=Signal::derive(move || selected_base_view.get())
                        />

                        // Multi-lane swim lane timeline: all tables, bars by time range, worker dots
                        <ChangelogTimeline
                            entries=Signal::derive(move || {
                                let buf = changelog_buffer.get();
                                let win = changelog_window_secs.get();
                                if win == 0 {
                                    buf
                                } else {
                                    let cutoff = (js_sys::Date::now() / 1000.0) as i64 - win;
                                    buf.into_iter().filter(|e| e.t_end >= cutoff).collect()
                                }
                            })
                            table_colors=Signal::derive(move || table_colors.get())
                            worker_infos=Signal::derive(move || worker_infos.get())
                            selected_base_view=Signal::derive(move || selected_base_view.get())
                            on_bar_click=on_bar_click
                        />

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
