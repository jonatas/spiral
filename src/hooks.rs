use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::{c_char, c_int};
use std::ffi::CStr;
use crate::{catalog, rollup};

static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;
static mut PREV_PLANNER_HOOK: pg_sys::planner_hook_type = None;

thread_local! { 
    static IN_UTILITY: Cell<bool> = Cell::new(false); 
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
    let mut frames_opt: Option<String> = None;
    let mut tenant_opt: Option<String> = None;
    let mut time_col_opt: Option<String> = Some("t".to_string());
    let mut target_name: Option<String> = None;
    let mut scope_columns = Vec::new();
    let mut is_refresh = false;
    let mut is_matview = false;
    
    if !utility_stmt.is_null() {
        let node_type = (*(utility_stmt as *mut pg_sys::Node)).type_;
        if node_type == pg_sys::NodeTag::T_CreateTableAsStmt {
            let ctas = utility_stmt as *mut pg_sys::CreateTableAsStmt;
            if (*ctas).objtype == pg_sys::ObjectType::OBJECT_MATVIEW {
                is_matview = true;
                let into = (*ctas).into;
                if !into.is_null() {
                    let rel = (*into).rel;
                    if !rel.is_null() { target_name = Some(CStr::from_ptr((*rel).relname).to_string_lossy().into_owned()); }
                    let query = (*ctas).query as *mut pg_sys::Query;
                    if !query.is_null() { scope_columns = get_grouping_columns(query); }
                    let options = (*into).options;
                    if !options.is_null() {
                        for i in 0..(*options).length {
                            let def_elem = pg_sys::list_nth(options, i as i32) as *mut pg_sys::DefElem;
                            if !def_elem.is_null() {
                                let defname = CStr::from_ptr((*def_elem).defname).to_string_lossy();
                                let defnamespace = if (*def_elem).defnamespace.is_null() { std::borrow::Cow::Borrowed("") } else { CStr::from_ptr((*def_elem).defnamespace).to_string_lossy() };
                                if defnamespace == "aspiral" {
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
                                }
                            }
                        }
                        (*into).options = std::ptr::null_mut();
                    }
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
                let mut new_options = std::ptr::null_mut();
                for i in 0..(*options).length {
                    let def_elem = pg_sys::list_nth(options, i as i32) as *mut pg_sys::DefElem;
                    if !def_elem.is_null() {
                        let defname = CStr::from_ptr((*def_elem).defname).to_string_lossy();
                        let defnamespace = if (*def_elem).defnamespace.is_null() { std::borrow::Cow::Borrowed("") } else { CStr::from_ptr((*def_elem).defnamespace).to_string_lossy() };
                        if defnamespace == "aspiral" {
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
                        } else { new_options = pg_sys::lappend(new_options, def_elem as *mut std::ffi::c_void); }
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

    let frames_opt_final = frames_opt.clone();
    let tenant_opt_final = tenant_opt.clone();
    let time_col_opt_final = time_col_opt.clone();
    let target_name_final = target_name.clone();
    let is_matview_final = is_matview;
    let scope_columns_final = scope_columns.clone();

    let mut handled_incrementally = false;
    if let Some(name) = target_name_final {
        if is_refresh {
            handled_incrementally = reactive_refresh(&name, None);
        } else {
            if let Some(ref tenant_str) = tenant_opt_final {
                let dimensions: Vec<String> = tenant_str.split(',').map(|s| s.trim().to_string()).collect();
                let time_col = time_col_opt_final.clone().unwrap_or_else(|| "t".to_string());
                crate::cluster_table_internal(&name, &time_col, dimensions);
            }
            let sql_text = unsafe { CStr::from_ptr(query_string).to_string_lossy() };
            let has_magic = sql_text.contains("-- Aspiral:");
            if !is_matview_final && (frames_opt_final.is_some() || has_magic) {
                let actual_frames = frames_opt_final.unwrap_or_else(|| rollup::DEFAULT_FRAMES.to_string());
                let table_name = name.clone();
                let time_col = time_col_opt_final.unwrap_or_else(|| "t".to_string());
                let tenant_cols = tenant_opt_final.as_ref().map(|s| s.split(',').collect::<Vec<_>>()).unwrap_or_default();
                let mut projections = vec![format!("to_timestamptz((aspiral(\"{time_col}\")/{{0}})*{{0}}) as \"{time_col}\"")];
                let mut groups = vec![format!("(aspiral(\"{time_col}\")/{{0}})*{{0}}")];
                for tenant in &tenant_cols { let t = tenant.trim(); projections.push(format!("\"{t}\"")); groups.push(format!("\"{t}\"")); }
                for line in sql_text.lines() {
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
                        let scope_cols_str = tenant_cols.iter().map(|s| format!("\"{}\"", s.trim())).collect::<Vec<_>>().join(", ");
                        
                        let index_sql = format!("CREATE INDEX IF NOT EXISTS \"idx_u_{table_name}_root\" ON {root_view}(t)");

                        let root_sql = format!(
                            "CREATE TABLE IF NOT EXISTS {root_view} AS SELECT {select} FROM \"{table_name}\" GROUP BY {group_by};
                             {index_sql};",
                            root_view = root_view, select = select, table_name = table_name, group_by = group_by, index_sql = index_sql
                        );
                        let root_name = root_view.trim_matches('"').to_string();
                        match Spi::run(&root_sql) {
                            Ok(_) => {
                                catalog::insert_metadata(&root_name, "BASE", 0, &table_name, tenant_cols.iter().map(|s| s.trim().to_string()).collect());
                                generate_hierarchy(&root_name, &actual_frames, tenant_cols.iter().map(|s| s.trim().to_string()).collect());
                                let trigger_sql = format!("CREATE TRIGGER aspiral_track_{} AFTER INSERT OR UPDATE OR DELETE ON \"{}\" FOR EACH ROW EXECUTE FUNCTION aspiral_track_changes('{}')", table_name, table_name, root_name);
                                let _ = Spi::run(&trigger_sql);
                            },
                            Err(e) => warning!("Aspiral failed to create root view {}: {:?}", root_name, e),
                        }
                    }
                }
            } else if is_matview_final {
                if let Some(frames_str) = frames_opt_final {
                    catalog::insert_metadata(&name, "BASE", 0, &name, scope_columns_final.clone());
                    generate_hierarchy(&name, &frames_str, scope_columns_final);
                }
            }
        }
    }

    if !handled_incrementally {
        if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
            prev_hook(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
        } else {
            pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
        }
    }

    IN_UTILITY.with(|h| h.set(false));
}

use std::cell::Cell;
thread_local! { static IN_HOOK: Cell<bool> = Cell::new(false); }

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
            let mut is_aspiral = false;
            for i in 0..(*rtable).length {
                let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
                if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                    let relid = (*rte).relid;
                    let relname = pg_sys::get_rel_name(relid);
                    if !relname.is_null() {
                        let name = CStr::from_ptr(relname).to_string_lossy();
                        if catalog::is_aspiral_relation(&name) { is_aspiral = true; break; }
                    }
                }
            }
            if is_aspiral {
                let target_list = query.targetList;
                for i in 0..(*target_list).length {
                    let tle = pg_sys::list_nth(target_list, i) as *mut pg_sys::TargetEntry;
                    if !(*tle).resname.is_null() {
                        let resname = CStr::from_ptr((*tle).resname).to_string_lossy();
                        if resname == "t" {
                            let expr = (*tle).expr as *mut pg_sys::Expr;
                            let node = expr as *mut pg_sys::Node;
                            if !node.is_null() && (*node).type_ == pg_sys::NodeTag::T_Var {
                                let var = node as *mut pg_sys::Var;
                                if (*var).vartype == pg_sys::INT8OID { info!("Aspiral Planner: Transparently converting 't' to timestamptz for display."); }
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

pub fn generate_hierarchy(base_name: &str, frames_str: &str, scope_columns: Vec<String>) {
    let mut frames = rollup::parse_frames(frames_str);
    frames.sort_by_key(|f| f.seconds);
    let re = regex::Regex::new(r"_\d+[smhdwmon]$").unwrap();
    let base_prefix = if let Some(m) = re.find(base_name) { &base_name[..m.start()] } else { base_name };
    let mut current_parent = base_name.to_string();
    for frame in frames {
        let child_name = format!("{}_{}", base_prefix, frame.name);
        if child_name == current_parent { continue; }
        info!("Aspiral creating child view '{}' from parent '{}'", child_name, current_parent);
        let sql = rollup::derive_child_sql(&child_name, &current_parent, frame.seconds, &scope_columns);
        match Spi::run(&sql) {
            Ok(_) => {
                catalog::insert_metadata(&child_name, &current_parent, frame.seconds, base_name, scope_columns.clone());
                current_parent = child_name;
            },
            Err(e) => warning!("Aspiral failed to create child view {}: {:?}", child_name, e),
        }
    }
}

unsafe fn get_grouping_columns(query: *mut pg_sys::Query) -> Vec<String> {
    let mut names = Vec::new();
    let query_ref = &*query;
    if !query_ref.groupClause.is_null() {
        let group_clause = query_ref.groupClause;
        for i in 0..(*group_clause).length {
            let sg_clause = pg_sys::list_nth(group_clause, i) as *mut pg_sys::SortGroupClause;
            let ref_id = (*sg_clause).tleSortGroupRef;
            let target_list = query_ref.targetList;
            for j in 0..(*target_list).length {
                let tle = pg_sys::list_nth(target_list, j) as *mut pg_sys::TargetEntry;
                if (*tle).ressortgroupref == ref_id {
                    if !(*tle).resname.is_null() { let name = CStr::from_ptr((*tle).resname).to_string_lossy().into_owned(); if name != "t" { names.push(name); } }
                    break;
                }
            }
        }
    }
    names
}

pub fn reactive_refresh(base_name: &str, where_clause: Option<String>) -> bool {
    let metadata = catalog::get_metadata(base_name);
    let is_root = metadata.as_ref().map(|m| m.parent_view == "BASE").unwrap_or(false);

    if crate::refresh_incremental(base_name, where_clause.clone()) {
        if where_clause.is_none() && is_root {
            let _ = Spi::run(&format!("DELETE FROM aspiral.changelog WHERE base_view = '{}'", base_name.replace("'", "''")));
        }
        return true;
    }
    false
}

pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(aspiral_process_utility_hook);
    PREV_PLANNER_HOOK = pg_sys::planner_hook;
    pg_sys::planner_hook = Some(aspiral_planner_hook);
}
