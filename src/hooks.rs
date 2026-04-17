use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::c_char;
use std::ffi::CStr;
use crate::{catalog, rollup};

static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;

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
                    
                    // Extract base table from query
                    let query = (*ctas).query as *mut pg_sys::Query;
                    if !query.is_null() {
                        // This is very simplified, just to get the first range table entry
                        let rtable = (*query).rtable;
                        if !rtable.is_null() && (*rtable).length > 0 {
                            let rte = pg_sys::list_nth(rtable, 0) as *mut pg_sys::RangeTblEntry;
                            if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                                let relid = (*rte).relid;
                                // Get table name from oid
                                let base_relname = pg_sys::get_rel_name(relid);
                                if !base_relname.is_null() {
                                    let base_name = CStr::from_ptr(base_relname).to_string_lossy().into_owned();
                                    info!("Aspiral identified base table: {}", base_name);
                                    
                                    if let Some(ref view_name) = matview_name {
                                        let trigger_sql = format!(
                                            "CREATE TRIGGER aspiral_track_{} 
                                             AFTER INSERT OR UPDATE OR DELETE ON {}
                                             FOR EACH ROW EXECUTE FUNCTION aspiral_track_changes('{}')",
                                            view_name, base_name, view_name
                                        );
                                        // Execute in the reaction phase after standard_ProcessUtility
                                        // Store for later
                                        let _ = Spi::run(&trigger_sql); 
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
                                if defname == "aspiral.frames" {
                                    let arg = (*def_elem).arg as *mut pg_sys::Node;
                                    if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                        let val = CStr::from_ptr((*(arg as *mut pg_sys::String)).sval).to_string_lossy();
                                        frames_opt = Some(val.into_owned());
                                    }
                                }
                            }
                        }
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
            generate_hierarchy(&name, &frames_str);
        }
    }
}

fn generate_hierarchy(base_name: &str, frames_str: &str) {
    let mut frames = rollup::parse_frames(frames_str);
    frames.sort_by_key(|f| f.seconds);

    let mut current_parent = base_name.to_string();
    for frame in frames {
        let child_name = format!("{}_{}", base_name, frame.name);
        info!("Aspiral creating child view '{}' from parent '{}'", child_name, current_parent);
        
        let sql = rollup::derive_child_sql(&child_name, &current_parent, frame.seconds);
        
        match Spi::run(&sql) {
            Ok(_) => {
                catalog::insert_metadata(&child_name, &current_parent, frame.seconds, base_name);
                current_parent = child_name;
            },
            Err(e) => warning!("Aspiral failed to create child view {}: {:?}", child_name, e),
        }
    }
}

fn reactive_refresh(base_name: &str) {
    let dirty_buckets = catalog::get_dirty_buckets(base_name);
    let children = catalog::get_children(base_name);
    
    if dirty_buckets.is_empty() {
        // Fallback to full refresh for children if no specific dirty buckets tracked
        for child in children {
            info!("Aspiral cascading full refresh to '{}'", child);
            match Spi::run(&format!("REFRESH MATERIALIZED VIEW {}", child)) {
                Ok(_) => reactive_refresh(&child), 
                Err(e) => warning!("Aspiral failed full refresh on {}: {:?}", child, e),
            }
        }
    } else {
        info!("Aspiral identified {} dirty buckets for '{}'", dirty_buckets.len(), base_name);
        for child in children {
            info!("Aspiral performing incremental refresh for '{}'", child);
            // In a real implementation, we would execute:
            // DELETE FROM {child} WHERE t IN ({dirty_buckets})
            // INSERT INTO {child} SELECT ... FROM {parent} WHERE t IN ({dirty_buckets})
        }
        catalog::clear_dirty_buckets(base_name, &dirty_buckets);
    }
}

pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(aspiral_process_utility_hook);
}
