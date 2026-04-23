use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::{c_char, c_int};
use std::ffi::CStr;
use crate::{catalog, rollup};
use std::cell::Cell;

static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;
static mut PREV_PLANNER_HOOK: pg_sys::planner_hook_type = None;

thread_local! { 
    static IN_UTILITY: Cell<bool> = Cell::new(false); 
    static IN_HOOK: Cell<bool> = Cell::new(false);
}

#[pg_guard]
pub unsafe extern "C-unwind" fn aspiral_process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion_tag: *mut pg_sys::QueryCompletion,
) {
    let q_str = unsafe { CStr::from_ptr(query_string).to_string_lossy() };
    if IN_UTILITY.with(|h| h.get()) {
        if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
            prev_hook(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
        } else {
            pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
        }
        return;
    }
    IN_UTILITY.with(|h| h.set(true));

    let utility_stmt = (*pstmt).utilityStmt;
    let node_type = if !utility_stmt.is_null() { unsafe { (*(utility_stmt as *mut pg_sys::Node)).type_ } } else { pg_sys::NodeTag::T_Invalid };

    let mut frames_opt: Option<String> = None;
    let mut tenant_opt: Option<String> = None;
    let mut time_col_opt: Option<String> = Some("t".to_string());
    let mut target_name: Option<String> = None;
    let mut scope_columns = Vec::new();
    let mut is_refresh = false;
    let mut is_matview = false;
    let mut aspiral_enabled = false;
    
    if q_str.to_lowercase().contains("-- aspiral: enabled") {
        aspiral_enabled = true;
    }

    if !utility_stmt.is_null() {
        if node_type == pg_sys::NodeTag::T_CreateTableAsStmt {
            let ctas = utility_stmt as *mut pg_sys::CreateTableAsStmt;
            if (*ctas).objtype == pg_sys::ObjectType::OBJECT_MATVIEW {
                is_matview = true;
            }
            let into = (*ctas).into;
            if !into.is_null() {
                let rel = (*into).rel;
                if !rel.is_null() { target_name = Some(CStr::from_ptr((*rel).relname).to_string_lossy().into_owned()); }
                let query = (*ctas).query as *mut pg_sys::Query;
                if !query.is_null() { scope_columns = get_grouping_columns(query); }
                let options = (*into).options;
                if !options.is_null() {
                    let mut new_options: *mut pg_sys::List = std::ptr::null_mut();
                    for i in 0..(*options).length {
                        let def_elem = pg_sys::list_nth(options, i as i32) as *mut pg_sys::DefElem;
                        if !def_elem.is_null() {
                            let defname = CStr::from_ptr((*def_elem).defname).to_string_lossy();
                            let defnamespace = if (*def_elem).defnamespace.is_null() { std::borrow::Cow::Borrowed("") } else { CStr::from_ptr((*def_elem).defnamespace).to_string_lossy() };
                            if defnamespace == "aspiral" || defname == "aspiral" || defname == "aspiral_enabled" || (defnamespace == "aspiral" && defname == "enabled") {
                                aspiral_enabled = true;
                                let arg = (*def_elem).arg as *mut pg_sys::Node;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let val = CStr::from_ptr((*(arg as *mut pg_sys::String)).sval).to_string_lossy().into_owned();
                                    match defname.as_ref() {
                                        "frames" => frames_opt = Some(val),
                                        "tenant" => tenant_opt = Some(val),
                                        "time" => time_col_opt = Some(val),
                                        _ => {}
                                    }
                                }
                            } else {
                                new_options = pg_sys::lappend(new_options, def_elem as *mut std::ffi::c_void);
                            }
                        }
                    }
                    (*into).options = new_options;
                }
            }
        } else if node_type == pg_sys::NodeTag::T_CreateStmt {
            let stmt = utility_stmt as *mut pg_sys::CreateStmt;
            let rel = (*stmt).relation;
            if !rel.is_null() { target_name = Some(CStr::from_ptr((*rel).relname).to_string_lossy().into_owned()); }
            let mut detected_time: Option<String> = None;
            let mut detected_tenants = Vec::new();
            let elements = (*stmt).tableElts;
            if !elements.is_null() {
                for i in 0..(*elements).length {
                    let node = pg_sys::list_nth(elements, i as i32) as *mut pg_sys::Node;
                    if (*node).type_ == pg_sys::NodeTag::T_ColumnDef {
                        let col_def = node as *mut pg_sys::ColumnDef;
                        let col_name = CStr::from_ptr((*col_def).colname).to_string_lossy().into_owned();
                        if detected_time.is_none() {
                            let type_name = (*(*col_def).typeName).names;
                            if !type_name.is_null() {
                                let last_node = pg_sys::list_nth(type_name, (*type_name).length - 1) as *mut pg_sys::Node;
                                if (*last_node).type_ == pg_sys::NodeTag::T_String {
                                    let t_name = CStr::from_ptr((*(last_node as *mut pg_sys::String)).sval).to_string_lossy().to_lowercase();
                                    if ["timestamptz", "timestamp", "date", "int8", "bigint"].contains(&t_name.as_str()) { detected_time = Some(col_name.clone()); }
                                }
                            }
                        }
                        let constraints = (*col_def).constraints;
                        if !constraints.is_null() {
                            for j in 0..(*constraints).length {
                                let constr = pg_sys::list_nth(constraints, j as i32) as *mut pg_sys::Constraint;
                                if (*constr).contype == pg_sys::ConstrType::CONSTR_FOREIGN { detected_tenants.push(col_name.clone()); }
                            }
                        }
                    }
                }
            }
            let options = (*stmt).options;
            if !options.is_null() {
                let mut new_options: *mut pg_sys::List = std::ptr::null_mut();
                for i in 0..(*options).length {
                    let def_elem = pg_sys::list_nth(options, i as i32) as *mut pg_sys::DefElem;
                    if !def_elem.is_null() {
                        let defname = CStr::from_ptr((*def_elem).defname).to_string_lossy();
                        let defnamespace = if (*def_elem).defnamespace.is_null() { std::borrow::Cow::Borrowed("") } else { CStr::from_ptr((*def_elem).defnamespace).to_string_lossy() };
                        if defnamespace == "aspiral" || defname == "aspiral" || defname == "aspiral_enabled" || (defnamespace == "aspiral" && defname == "enabled") {
                            aspiral_enabled = true;
                            let arg = (*def_elem).arg as *mut pg_sys::Node;
                            if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                let val = CStr::from_ptr((*(arg as *mut pg_sys::String)).sval).to_string_lossy().into_owned();
                                match defname.as_ref() {
                                    "tenant" => tenant_opt = Some(val),
                                    "time" => time_col_opt = Some(val),
                                    "frames" => frames_opt = Some(val),
                                    _ => {}
                                }
                            }
                        } else { 
                            new_options = pg_sys::lappend(new_options, def_elem as *mut std::ffi::c_void); 
                        }
                    }
                }
                (*stmt).options = new_options;
            }
            if time_col_opt.is_none() || time_col_opt == Some("t".to_string()) { if let Some(dt) = detected_time { time_col_opt = Some(dt); } }
            if tenant_opt.is_none() && !detected_tenants.is_empty() { tenant_opt = Some(detected_tenants.join(",")); }
        } else if node_type == pg_sys::NodeTag::T_RefreshMatViewStmt {
            let rmv = utility_stmt as *mut pg_sys::RefreshMatViewStmt;
            let rel = (*rmv).relation;
            if !rel.is_null() { target_name = Some(CStr::from_ptr((*rel).relname).to_string_lossy().into_owned()); is_refresh = true; }
        }
    }

    if aspiral_enabled { crate::bgworker::maybe_start_worker(); }

    let mut handled_incrementally = false;
    if let Some(ref name) = target_name { if is_refresh { handled_incrementally = reactive_refresh(name, None); } }

    if !handled_incrementally {
        if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
            prev_hook(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
        } else {
            pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
        }
    }

    if let Some(name) = target_name {
        if !is_refresh {
            if aspiral_enabled && tenant_opt.is_some() {
                let tenant_str = tenant_opt.clone().unwrap();
                let dimensions: Vec<String> = tenant_str.split(',').map(|s| s.trim().to_string()).collect();
                let time_col = time_col_opt.clone().unwrap_or_else(|| "t".to_string());
                crate::cluster_table_internal(&name, &time_col, dimensions);
            }
            if !is_matview && aspiral_enabled {
                let actual_frames = frames_opt.unwrap_or_else(|| rollup::DEFAULT_FRAMES.to_string());
                let table_name = name.clone();
                let time_col = time_col_opt.unwrap_or_else(|| "t".to_string());
                let tenant_cols = tenant_opt.as_ref().map(|s| s.split(',').collect::<Vec<_>>()).unwrap_or_default();
                let mut projections = vec![format!("to_timestamp(((aspiral(\"{time_col}\")/{{0}})*{{0}})::double precision) as \"{time_col}\"")];
                let mut groups = vec![format!("(aspiral(\"{time_col}\")/{{0}})*{{0}}")];
                for tenant in &tenant_cols { let t = tenant.trim(); projections.push(format!("\"{t}\"")); groups.push(format!("\"{t}\"")); }
                for line in q_str.lines() {
                    if let Some(pos) = line.find("-- Aspiral:") {
                        let content_before = &line[..pos].trim();
                        if let Some(col) = content_before.split_whitespace().next() {
                            let tasks = &line[pos + 11..].trim();
                            let clean_col = col.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                            if ["not", "null", "primary", "unique", "check", "default"].contains(&clean_col.to_lowercase().as_str()) { continue; }
                            let tasks_list: Vec<&str> = tasks.split(',').map(|t| t.trim()).collect();
                            let is_dimension = tenant_cols.iter().any(|t| t.trim() == clean_col) || clean_col == time_col;
                            let use_suffix = tasks_list.len() > 1 || tasks_list.contains(&"ohlc") || is_dimension;
                            for task_item in tasks_list {
                                let mut parts = task_item.splitn(2, |c: char| c.is_whitespace());
                                let task_type = parts.next().unwrap_or("").to_lowercase();
                                let remainder = parts.next().unwrap_or("").trim();
                                let custom_alias = if remainder.to_lowercase().starts_with("as ") { Some(remainder[3..].trim().trim_matches(|c: char| !c.is_alphanumeric() && c != '_')) } else { None };
                                match task_type.as_str() {
                                    "ohlc" => {
                                        let prefix = custom_alias.unwrap_or(clean_col);
                                        projections.push(format!("first(\"{clean_col}\", aspiral(\"{time_col}\")) as \"{prefix}_o\""));
                                        projections.push(format!("max(\"{clean_col}\") as \"{prefix}_h\""));
                                        projections.push(format!("min(\"{clean_col}\") as \"{prefix}_l\""));
                                        projections.push(format!("last(\"{clean_col}\", aspiral(\"{time_col}\")) as \"{prefix}_c\""));
                                    },
                                    "stats" => { let alias = custom_alias.map(|s| s.to_string()).unwrap_or_else(|| if use_suffix { format!("{clean_col}_stats") } else { clean_col.to_string() }); projections.push(format!("aspiral_stats(\"{clean_col}\") as \"{alias}\"")); },
                                    "sum" => { let alias = custom_alias.map(|s| s.to_string()).unwrap_or_else(|| if use_suffix { format!("{clean_col}_sum") } else { clean_col.to_string() }); projections.push(format!("sum(\"{clean_col}\") as \"{alias}\"")); },
                                    "count" => { let alias = custom_alias.map(|s| s.to_string()).unwrap_or_else(|| if use_suffix { format!("{clean_col}_count") } else { clean_col.to_string() }); projections.push(format!("count(*) as \"{alias}\"")); },
                                    "sketch" => { let alias = custom_alias.map(|s| s.to_string()).unwrap_or_else(|| if use_suffix { format!("{clean_col}_sketch") } else { clean_col.to_string() }); projections.push(format!("aspiral_sketch(\"{clean_col}\") as \"{alias}\"")); },
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                if projections.len() > 1 {
                    let frames = rollup::parse_frames(&actual_frames);
                    if !frames.is_empty() {
                        let root_frame = &frames[0];
                        let root_view = format!("\"{}_ohlcv_{}\"", table_name, root_frame.name);
                        let select = projections.join(", ").replace("{0}", &root_frame.seconds.to_string());
                        let group_by = groups.join(", ").replace("{0}", &root_frame.seconds.to_string());
                        let index_sql = format!("CREATE INDEX IF NOT EXISTS \"idx_u_{table_name}_root\" ON {root_view}(t)");
                        let root_sql = format!("CREATE TABLE IF NOT EXISTS {root_view} AS SELECT {select} FROM \"{table_name}\" GROUP BY {group_by}; {index_sql};", root_view = root_view, select = select, table_name = table_name, group_by = group_by, index_sql = index_sql);
                        let root_name = root_view.trim_matches('"').to_string();
                        match Spi::run(&root_sql) {
                            Ok(_) => {
                                let (_sql_child, sources) = rollup::derive_child_sql(&root_name, &table_name, root_frame.seconds, &tenant_cols.iter().map(|s| s.trim().to_string()).collect::<Vec<_>>());
                                catalog::insert_metadata(&root_name, "BASE", root_frame.seconds, &table_name, tenant_cols.iter().map(|s| s.trim().to_string()).collect(), pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())));
                                for src in sources { 
                                    catalog::insert_source(&root_name, &table_name, root_frame.seconds, &src.base_column, &src.formula, &src.mat_column, pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new()))); 
                                }
                                generate_hierarchy(&root_name, &actual_frames, tenant_cols.iter().map(|s| s.trim().to_string()).collect());
                                for event in &["INSERT", "UPDATE", "DELETE"] {
                                    let transition = match *event { "INSERT" => "REFERENCING NEW TABLE AS new_table", "UPDATE" => "REFERENCING NEW TABLE AS new_table OLD TABLE AS old_table", "DELETE" => "REFERENCING OLD TABLE AS old_table", _ => "" };
                                    let trigger_sql = format!("CREATE TRIGGER aspiral_track_{base}_{event_lower} AFTER {event} ON \"{base}\" {transition} FOR EACH STATEMENT EXECUTE FUNCTION aspiral.track_changes_stmt('{root}')", base = table_name, event = event, event_lower = event.to_lowercase(), transition = transition, root = root_name);
                                    let _ = Spi::run(&trigger_sql);
                                }
                            },
                            Err(e) => warning!("Aspiral failed to create root view {}: {:?}", root_name, e),
                        }
                    }
                }
            } else if is_matview {
                if let Some(frames_str) = frames_opt {
                    let ctas = utility_stmt as *mut pg_sys::CreateTableAsStmt;
                    let query = (*ctas).query as *mut pg_sys::Query;
                    let rtable = (*query).rtable;

                    let sources = unsafe { extract_aggregate_mappings(query, rtable) };
                    catalog::insert_metadata(&name, "BASE", 0, &name, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())));
                    for src in sources { 
                        catalog::insert_source(&name, &name, 0, &src.base_column, &src.formula, &src.mat_column, pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new()))); 
                    }
                    generate_hierarchy(&name, &frames_str, scope_columns);
                }
            }
        }
    }
    IN_UTILITY.with(|h| h.set(false));
}
#[pg_guard]
pub unsafe extern "C-unwind" fn aspiral_planner_hook(
    parse: *mut pg_sys::Query, query_string: *const c_char, cursor_options: c_int, bound_params: pg_sys::ParamListInfo,
) -> *mut pg_sys::PlannedStmt {
    if IN_HOOK.with(|h| h.get()) {
        return if let Some(prev_hook) = PREV_PLANNER_HOOK { prev_hook(parse, query_string, cursor_options, bound_params) } else { pg_sys::standard_planner(parse, query_string, cursor_options, bound_params) };
    }
    IN_HOOK.with(|h| h.set(true));
    let query = &mut *parse;
    if query.commandType == pg_sys::CmdType::CMD_SELECT {
        let rtable = query.rtable;
        if !rtable.is_null() {
            let mut target_base_table: Option<String> = None;
            let mut target_rte_idx: i32 = -1;
            for i in 0..(*rtable).length {
                let rte = pg_sys::list_nth(rtable, i as i32) as *mut pg_sys::RangeTblEntry;
                if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                    let relid = (*rte).relid;
                    let relname = pg_sys::get_rel_name(relid);
                    if !relname.is_null() {
                        let name = CStr::from_ptr(relname).to_string_lossy().into_owned();
                        let has_rollups = Spi::connect(|client| {
                            let schema_exists = client.select("SELECT 1 FROM pg_namespace WHERE nspname = 'aspiral'", Some(1), &[])?.first().is_empty() == false;
                            if !schema_exists { return Ok::<bool, spi::Error>(false); }
                            let res = client.select("SELECT 1 FROM aspiral.metadata WHERE base_view = $1 LIMIT 1", Some(1), 
                                unsafe { &[pgrx::datum::DatumWithOid::new(name.clone().into_datum().unwrap(), pg_sys::TEXTOID)] })?;
                            Ok::<bool, spi::Error>(!res.is_empty())
                        }).unwrap_or(false);
                        if has_rollups { target_base_table = Some(name); target_rte_idx = i as i32; break; }
                    }
                }
            }

            if let Some(base_table) = target_base_table {
                let (t_start, t_end) = extract_time_range(query.jointree, rtable);
                if let (Some(ts), Some(te)) = (t_start, t_end) {
                    let hierarchy = Spi::connect(|client| {
                        let mut views = Vec::new();
                        let table = client.select("SELECT view_name FROM aspiral.metadata WHERE base_view = $1", None,
                            unsafe { &[pgrx::datum::DatumWithOid::new(base_table.clone().into_datum().unwrap(), pg_sys::TEXTOID)] })?;
                        for row in table { views.push(row.get::<String>(1)?.unwrap()); }
                        Ok::<Vec<String>, spi::Error>(views)
                    }).unwrap_or_default();
                    
                    if !hierarchy.is_empty() {
                         let dirty_ranges = catalog::get_dirty_ranges(&base_table, ts, te);
                         let segments = resolve_segments(&base_table, ts, te, &hierarchy, &dirty_ranges);
                         
                         // SURGICAL SWAP: If we have a single segment that is a rollup, swap the relid!
                         // This only works for very simple queries where the rollup has the SAME column names.
                         if segments.len() == 1 && segments[0].source != base_table {
                             let rollup_relname = std::ffi::CString::new(segments[0].source.clone()).unwrap();
                             let rollup_relid = pg_sys::RelnameGetRelid(rollup_relname.as_ptr());
                             if rollup_relid != pg_sys::InvalidOid {
                                 let rte = pg_sys::list_nth(rtable, target_rte_idx) as *mut pg_sys::RangeTblEntry;
                                 (*rte).relid = rollup_relid;
                                 
                                 // Update permission info for PG 16+
                                 let perminfoindex = (*rte).perminfoindex;
                                 if perminfoindex > 0 && !query.rteperminfos.is_null() {
                                     let perminfo = pg_sys::list_nth(query.rteperminfos, (perminfoindex - 1) as i32) as *mut pg_sys::RTEPermissionInfo;
                                     if !perminfo.is_null() {
                                         (*perminfo).relid = rollup_relid;
                                     }
                                 }
                             }
                         }
                    }
                }
            }
        }
    }
    let result = if let Some(prev_hook) = PREV_PLANNER_HOOK { prev_hook(parse, query_string, cursor_options, bound_params) } else { pg_sys::standard_planner(parse, query_string, cursor_options, bound_params) };
    IN_HOOK.with(|h| h.set(false));
    result
}

#[derive(Debug)]
struct Segment { source: String, _t_start: i64, _t_end: i64 }

fn resolve_segments(base_table: &str, ts: i64, te: i64, hierarchy: &[String], dirty: &[(i64, i64)]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current_ts = ts;
    let mut sorted_hierarchy: Vec<(String, i32)> = hierarchy.iter().filter_map(|h| catalog::get_metadata(h).map(|m| (h.clone(), m.frame_seconds))).collect();
    sorted_hierarchy.sort_by_key(|h| -h.1);
    while current_ts < te {
        let is_dirty = dirty.iter().any(|(s, e)| current_ts >= *s && current_ts < *e);
        if is_dirty {
            let dirty_end = dirty.iter().filter(|(s, _)| current_ts >= *s).map(|(_, e)| *e).max().unwrap_or(current_ts);
            let segment_end = te.min(dirty_end);
            segments.push(Segment { source: base_table.to_string(), _t_start: current_ts, _t_end: segment_end });
            current_ts = segment_end;
            continue;
            }

            let mut found_frame = false;
            for (h_name, frame_secs) in &sorted_hierarchy {
            let f_s = *frame_secs as i64; if f_s == 0 { continue; }
            let bucket_start = (current_ts / f_s) * f_s;
            let bucket_end = bucket_start + f_s;
            if current_ts == bucket_start && bucket_end <= te {
                let bucket_dirty = dirty.iter().any(|(s, e)| !(*e <= bucket_start || *s >= bucket_end));
                if !bucket_dirty { segments.push(Segment { source: h_name.clone(), _t_start: bucket_start, _t_end: bucket_end }); current_ts = bucket_end; found_frame = true; break; }
            }
            }
            if !found_frame { let next_ts = te.min(current_ts + 60); segments.push(Segment { source: base_table.to_string(), _t_start: current_ts, _t_end: next_ts }); current_ts = next_ts; }
            }
    segments
}

unsafe fn extract_time_range(jointree: *mut pg_sys::FromExpr, rtable: *mut pg_sys::List) -> (Option<i64>, Option<i64>) {
    let mut t_start = None; let mut t_end = None;
    if jointree.is_null() { return (None, None); }
    let quals = (*jointree).quals as *mut pg_sys::Node;
    if quals.is_null() { return (None, None); }
    let mut stack = vec![quals];
    while let Some(node) = stack.pop() {
        if node.is_null() { continue; }
        let tag = (*node).type_;
        if tag == pg_sys::NodeTag::T_OpExpr {
            let op = node as *mut pg_sys::OpExpr;
            let opname = pg_sys::get_opname((*op).opno);
            if !opname.is_null() {
                let name = CStr::from_ptr(opname).to_string_lossy();
                let args = (*op).args;
                if !args.is_null() && (*args).length == 2 {
                    let left = pg_sys::list_nth(args, 0) as *mut pg_sys::Node;
                    let right = pg_sys::list_nth(args, 1) as *mut pg_sys::Node;
                    if (*left).type_ == pg_sys::NodeTag::T_Var && (*right).type_ == pg_sys::NodeTag::T_Const {
                        let var = left as *mut pg_sys::Var;
                        let rte = pg_sys::list_nth(rtable, ((*var).varno - 1) as i32) as *mut pg_sys::RangeTblEntry;
                        let varname_ptr = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                        if !varname_ptr.is_null() {
                            let varname = CStr::from_ptr(varname_ptr).to_string_lossy();
                            if varname == "t" {
                                let con = right as *mut pg_sys::Const;
                                let val = match (*con).consttype {
                                    pg_sys::INT8OID => Some(i64::from_datum((*con).constvalue, (*con).constisnull).unwrap()),
                                    pg_sys::TIMESTAMPTZOID => {
                                        let ts = i64::from_datum((*con).constvalue, (*con).constisnull).unwrap();
                                        Some(ts / 1000000 + 946684800)
                                    },
                                    _ => None,
                                };
                                if let Some(v) = val { if name == ">=" { t_start = Some(v); } else if name == "<" { t_end = Some(v); } }
                            }
                        }
                    }
                }
            }
        } else if tag == pg_sys::NodeTag::T_BoolExpr {
            let bexpr = node as *mut pg_sys::BoolExpr;
            let args = (*bexpr).args;
            for i in 0..(*args).length { stack.push(pg_sys::list_nth(args, i as i32) as *mut pg_sys::Node); }
        }
    }
    (t_start, t_end)
}

pub fn generate_hierarchy(base_name: &str, frames_str: &str, scope_columns: Vec<String>) {
    let mut frames = rollup::parse_frames(frames_str);
    frames.sort_by_key(|f| f.seconds);
    let re = regex::Regex::new(r"_\d+[smhdwmon]$").unwrap();
    let base_prefix = if let Some(m) = re.find(base_name) { &base_name[..m.start()] } else { base_name };
    let mut current_parent = base_name.to_string();
    for frame in frames {
        let child_name = format!("{}_{}", base_prefix, frame.name);
        if child_name == current_parent { continue; }
        let (sql, sources) = rollup::derive_child_sql(&child_name, &current_parent, frame.seconds, &scope_columns);
        match Spi::run(&sql) { 
            Ok(_) => { 
                catalog::insert_metadata(&child_name, &current_parent, frame.seconds, base_name, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new()))); 
                for src in sources {
                    catalog::insert_source(&child_name, base_name, frame.seconds, &src.base_column, &src.formula, &src.mat_column, pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())));
                }
                current_parent = child_name; 
            }, 
            Err(e) => warning!("Aspiral failed to create child view {}: {:?}", child_name, e), 
        }
    }
}

unsafe fn extract_aggregate_mappings(query: *mut pg_sys::Query, rtable: *mut pg_sys::List) -> Vec<rollup::SourceDef> {
    let mut sources = Vec::new();
    let target_list = (*query).targetList;
    if target_list.is_null() { return sources; }
    for i in 0..(*target_list).length {
        let tle = pg_sys::list_nth(target_list, i as i32) as *mut pg_sys::TargetEntry;
        if tle.is_null() || (*tle).resname.is_null() { continue; }
        let mat_column = CStr::from_ptr((*tle).resname).to_string_lossy().into_owned();
        let expr = (*tle).expr as *mut pg_sys::Node;
        if expr.is_null() { continue; }
        if (*expr).type_ == pg_sys::NodeTag::T_Aggref {
            let agg = expr as *mut pg_sys::Aggref;
            let agg_fn = pg_sys::get_func_name((*agg).aggfnoid);
            if !agg_fn.is_null() {
                let formula = CStr::from_ptr(agg_fn).to_string_lossy().into_owned();
                let args = (*agg).args;
                if !args.is_null() && (*args).length > 0 {
                    let arg = pg_sys::list_nth(args, 0) as *mut pg_sys::TargetEntry;
                    let arg_expr = (*arg).expr as *mut pg_sys::Node;
                    if !arg_expr.is_null() && (*arg_expr).type_ == pg_sys::NodeTag::T_Var {
                        let var = arg_expr as *mut pg_sys::Var;
                        let rte = pg_sys::list_nth(rtable, ((*var).varno - 1) as i32) as *mut pg_sys::RangeTblEntry;
                        let base_col_ptr = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                        if !base_col_ptr.is_null() {
                            let base_column = CStr::from_ptr(base_col_ptr).to_string_lossy().into_owned();
                            sources.push(rollup::SourceDef { base_column, formula, mat_column });
                        }
                    }
                }
            }
        }
    }
    sources
}

unsafe fn get_grouping_columns(query: *mut pg_sys::Query) -> Vec<String> {
    let mut names = Vec::new();
    let query_ref = &*query;
    if !query_ref.groupClause.is_null() {
        let group_clause = query_ref.groupClause;
        for i in 0..(*group_clause).length {
            let sg_clause = pg_sys::list_nth(group_clause, i as i32) as *mut pg_sys::SortGroupClause;
            let ref_id = (*sg_clause).tleSortGroupRef;
            let target_list = query_ref.targetList;
            for j in 0..(*target_list).length {
                let tle = pg_sys::list_nth(target_list, j as i32) as *mut pg_sys::TargetEntry;
                if (*tle).ressortgroupref == ref_id { if !(*tle).resname.is_null() { let name = CStr::from_ptr((*tle).resname).to_string_lossy().into_owned(); if name != "t" { names.push(name); } } break; }
            }
        }
    }
    names
}

pub fn reactive_refresh(base_name: &str, where_clause: Option<String>) -> bool {
    let metadata = catalog::get_metadata(base_name);
    let is_root = metadata.as_ref().map(|m| m.parent_view == "BASE").unwrap_or(false);
    if is_root { catalog::unify_changelog(base_name); }
    if crate::refresh_incremental(base_name, where_clause.clone()) { if where_clause.is_none() && is_root { let _ = Spi::run(&format!("DELETE FROM aspiral.changelog WHERE base_view = '{}'", base_name.replace("'", "''"))); } return true; }
    false
}

pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(aspiral_process_utility_hook);
    PREV_PLANNER_HOOK = pg_sys::planner_hook;
    pg_sys::planner_hook = Some(aspiral_planner_hook);
}
