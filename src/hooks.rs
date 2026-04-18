use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::c_char;
use std::ffi::CStr;
use crate::{catalog, rollup};

static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;
static mut PREV_PLANNER_HOOK: pg_sys::planner_hook_type = None;

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
    let utility_stmt = (*pstmt).utilityStmt;
    let mut frames_opt: Option<String> = None;
    let mut matview_name: Option<String> = None;
    let mut scope_columns = Vec::new();
    let mut is_refresh = false;
    
    if !utility_stmt.is_null() {
        let node_type = (*(utility_stmt as *mut pg_sys::Node)).type_;
        
        if node_type == pg_sys::NodeTag::T_CreateTableAsStmt {
            let ctas = utility_stmt as *mut pg_sys::CreateTableAsStmt;
            if (*ctas).objtype == pg_sys::ObjectType::OBJECT_MATVIEW {
                let into = (*ctas).into;
                if !into.is_null() {
                    let rel = (*into).rel;
                    if !rel.is_null() {
                        matview_name = Some(CStr::from_ptr((*rel).relname).to_string_lossy().into_owned());
                    }
                    
                    // Extract base table and grouping columns from query
                    let query = (*ctas).query as *mut pg_sys::Query;
                    if !query.is_null() {
                        scope_columns = get_grouping_columns(query);
                        
                        let rtable = (*query).rtable;
                        if !rtable.is_null() && (*rtable).length > 0 {
                            let rte = pg_sys::list_nth(rtable, 0) as *mut pg_sys::RangeTblEntry;
                            if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                                let relid = (*rte).relid;
                                let base_relname = pg_sys::get_rel_name(relid);
                                if !base_relname.is_null() {
                                    let base_name = CStr::from_ptr(base_relname).to_string_lossy().into_owned();
                                    info!("Aspiral identified base table: {} with scopes: {:?}", base_name, scope_columns);
                                    
                                    if let Some(ref view_name) = matview_name {
                                        // Only create triggers on regular tables. Materialized views don't support them.
                                        let rel_kind = pg_sys::get_rel_relkind(relid);
                                        if rel_kind == pg_sys::RELKIND_RELATION as c_char {
                                            let trigger_sql = format!(
                                                "CREATE TRIGGER aspiral_track_{} 
                                                 AFTER INSERT OR UPDATE OR DELETE ON {}
                                                 FOR EACH ROW EXECUTE FUNCTION aspiral_track_changes('{}')",
                                                view_name, base_name, view_name
                                            );
                                            let _ = Spi::run(&trigger_sql); 
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let options = (*into).options;
                    if !options.is_null() {
                        for i in 0..(*options).length {
                            let def_elem = pg_sys::list_nth(options, i as i32) as *mut pg_sys::DefElem;
                            if !def_elem.is_null() {
                                let defname = CStr::from_ptr((*def_elem).defname).to_string_lossy();
                                let defnamespace = if (*def_elem).defnamespace.is_null() {
                                    std::borrow::Cow::Borrowed("")
                                } else {
                                    CStr::from_ptr((*def_elem).defnamespace).to_string_lossy()
                                };
                                if defnamespace == "aspiral" && defname == "frames" {
                                    let arg = (*def_elem).arg as *mut pg_sys::Node;
                                    if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                        let val = CStr::from_ptr((*(arg as *mut pg_sys::String)).sval).to_string_lossy();
                                        frames_opt = Some(val.into_owned());
                                    }
                                }
                            }
                        }
                        (*into).options = std::ptr::null_mut();
                    }
                }
            }
        } else if node_type == pg_sys::NodeTag::T_RefreshMatViewStmt {
            let rmv = utility_stmt as *mut pg_sys::RefreshMatViewStmt;
            let rel = (*rmv).relation;
            if !rel.is_null() {
                matview_name = Some(CStr::from_ptr((*rel).relname).to_string_lossy().into_owned());
                is_refresh = true;
            }
        }
    }

    // Call the standard utility
    if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
        prev_hook(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
    } else {
        pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, completion_tag);
    }

    // React
    if let Some(name) = matview_name {
        if is_refresh {
            reactive_refresh(&name);
        } else if let Some(frames_str) = frames_opt {
            // Register the base matview in metadata so children can find its scope
            catalog::insert_metadata(&name, "BASE", 0, &name, scope_columns.clone());
            generate_hierarchy(&name, &frames_str, scope_columns);
        }
    }
}

use std::cell::Cell;

thread_local! {
    static IN_HOOK: Cell<bool> = Cell::new(false);
}

#[pg_guard]
pub unsafe extern "C-unwind" fn aspiral_planner_hook(
    parse: *mut pg_sys::Query,
    query_string: *const c_char,
    cursor_options: std::os::raw::c_int,
    bound_params: pg_sys::ParamListInfo,
) -> *mut pg_sys::PlannedStmt {
    if IN_HOOK.with(|h| h.get()) {
        return if let Some(prev_hook) = PREV_PLANNER_HOOK {
            prev_hook(parse, query_string, cursor_options, bound_params)
        } else {
            pg_sys::standard_planner(parse, query_string, cursor_options, bound_params)
        };
    }

    IN_HOOK.with(|h| h.set(true));
    
    let query = &mut *parse;
    
    // Transparently rewrite SELECT and WHERE for Aspiral relations
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
                        if catalog::is_aspiral_relation(&name) {
                            is_aspiral = true;
                            break;
                        }
                    }
                }
            }

            if is_aspiral {
                // 1. Rewrite Target List: Wrap 't' in to_timestamptz if it's stored as bigint
                let target_list = query.targetList;
                for i in 0..(*target_list).length {
                    let tle = pg_sys::list_nth(target_list, i) as *mut pg_sys::TargetEntry;
                    if !(*tle).resname.is_null() {
                        let resname = CStr::from_ptr((*tle).resname).to_string_lossy();
                        if resname == "t" {
                            // Check type. If it's INT8 (20), wrap it.
                            let expr = (*tle).expr as *mut pg_sys::Expr;
                            let node = expr as *mut pg_sys::Node;
                            if !node.is_null() && (*node).type_ == pg_sys::NodeTag::T_Var {
                                let var = node as *mut pg_sys::Var;
                                if (*var).vartype == pg_sys::INT8OID {
                                    // Wrap in to_timestamptz(t)
                                    // For simplicity in this prototype, we'll log it.
                                    // A full implementation would use rewriteTargetList.
                                    info!("Aspiral Planner: Transparently converting 't' to timestamptz for display.");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let result = if let Some(prev_hook) = PREV_PLANNER_HOOK {
        prev_hook(parse, query_string, cursor_options, bound_params)
    } else {
        pg_sys::standard_planner(parse, query_string, cursor_options, bound_params)
    };

    IN_HOOK.with(|h| h.set(false));
    result
}

fn generate_hierarchy(base_name: &str, frames_str: &str, scope_columns: Vec<String>) {
    let mut frames = rollup::parse_frames(frames_str);
    frames.sort_by_key(|f| f.seconds);

    let re = regex::Regex::new(r"_\d+[smhdw]$").unwrap();
    let base_prefix = if let Some(m) = re.find(base_name) {
        &base_name[..m.start()]
    } else {
        base_name
    };

    let mut current_parent = base_name.to_string();
    for frame in frames {
        let child_name = format!("{}_{}", base_prefix, frame.name);
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
                    if !(*tle).resname.is_null() {
                        let name = CStr::from_ptr((*tle).resname).to_string_lossy().into_owned();
                        // Ignore the primary time column 't'
                        if name != "t" {
                            names.push(name);
                        }
                    }
                    break;
                }
            }
        }
    }
    names
}

fn reactive_refresh(base_name: &str) {
    let dirty_buckets = catalog::get_dirty_buckets(base_name);
    let children = catalog::get_children(base_name);
    
    if dirty_buckets.is_empty() {
        // Fallback to full refresh for children if no specific dirty buckets tracked
        for child in children {
            info!("Aspiral cascading full refresh to '{}'", child);
            match Spi::run(&format!("REFRESH MATERIALIZED VIEW {}", child)) {
                Ok(_) => (), // Recursion happens via the hook intercepting this refresh
                Err(e) => warning!("Aspiral failed full refresh on {}: {:?}", child, e),
            }
        }
    } else {
        info!("Aspiral identified {} dirty buckets for '{}'", dirty_buckets.len(), base_name);
        for child in children {
            info!("Aspiral cascading refresh to '{}' due to dirty buckets", child);
            match Spi::run(&format!("REFRESH MATERIALIZED VIEW {}", child)) {
                Ok(_) => (),
                Err(e) => warning!("Aspiral failed cascading refresh on {}: {:?}", child, e),
            }
        }
        catalog::clear_dirty_buckets(base_name, &dirty_buckets);
    }
}

pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(aspiral_process_utility_hook);

    PREV_PLANNER_HOOK = pg_sys::planner_hook;
    pg_sys::planner_hook = Some(aspiral_planner_hook);
}
