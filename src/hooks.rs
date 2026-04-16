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
        
        let sql = rollup::derive_child_sql(&current_parent, frame.seconds);
        
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
    let children = catalog::get_children(base_name);
    for child in children {
        info!("Aspiral cascading refresh to '{}'", child);
        match Spi::run(&format!("REFRESH MATERIALIZED VIEW {}", child)) {
            Ok(_) => reactive_refresh(&child), // Recurse
            Err(e) => warning!("Aspiral failed to refresh child view {}: {:?}", child, e),
        }
    }
}

pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(aspiral_process_utility_hook);
}
