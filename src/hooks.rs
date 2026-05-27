use crate::{catalog, rollup};
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::panic::AssertUnwindSafe;

static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;
static mut PREV_PLANNER_HOOK: pg_sys::planner_hook_type = None;

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static IN_UTILITY: Cell<bool> = const { Cell::new(false) };
}

#[cfg(any(test, feature = "pg_test"))]
pub fn is_in_hook_for_test() -> bool {
    IN_HOOK.with(|h| h.get())
}

#[cfg(any(test, feature = "pg_test"))]
pub fn is_in_utility_for_test() -> bool {
    IN_UTILITY.with(|h| h.get())
}

pub fn parse_magic_comments(query_str: &str, col_types_map: &std::collections::HashMap<String, pg_sys::Oid>) -> Vec<(String, String, Option<String>)> {
    let re_col = regex::Regex::new(r"(\w+)\s+[\w\(\) ]+,?[ \t]*--\s*Spiral:\s*([^\n\r]+)").unwrap();
    let mut captured_cols = Vec::new();
    for cap in re_col.captures_iter(query_str) {
        let col_name = cap[1].to_string();
        if !col_types_map.contains_key(&col_name) { continue; }
        let formula_part = cap[2].trim().to_string();
        for part in formula_part.split(',') {
            let part = part.trim();
            if part.is_empty() { continue; }
            let (formula, alias) = if let Some((f, a)) = part.split_once(" as ") {
                (f.trim().to_string(), Some(a.trim().to_string()))
            } else if let Some((f, a)) = part.split_once(" AS ") {
                (f.trim().to_string(), Some(a.trim().to_string()))
            } else {
                (part.to_string(), None)
            };
            if formula.split_whitespace().count() > 1 { continue; }
            captured_cols.push((col_name.clone(), formula, alias));
        }
    }
    captured_cols
}

#[pg_guard]
#[allow(clippy::too_many_arguments)]
unsafe extern "C-unwind" fn spiral_process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    if IN_UTILITY.with(|h| h.get()) {
        if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
            prev(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
        } else {
            pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
        }
        return;
    }
    IN_UTILITY.with(|h| h.set(true));
    PgTryBuilder::new(AssertUnwindSafe(|| {
    let utility_stmt = (*pstmt).utilityStmt;
    if !utility_stmt.is_null() {
        let tag = (*utility_stmt).type_;
        if tag == pg_sys::NodeTag::T_CreateStmt
            || tag == pg_sys::NodeTag::T_ViewStmt
            || tag == pg_sys::NodeTag::T_CreateTableAsStmt
        {
            let (rel, name) = match tag {
                pg_sys::NodeTag::T_CreateStmt => {
                    let stmt = utility_stmt as *mut pg_sys::CreateStmt;
                    ((*stmt).relation, CStr::from_ptr((*(*stmt).relation).relname).to_string_lossy().into_owned())
                }
                pg_sys::NodeTag::T_CreateTableAsStmt => {
                    let stmt = utility_stmt as *mut pg_sys::CreateTableAsStmt;
                    let into = (*stmt).into;
                    ((*into).rel, CStr::from_ptr((*(*into).rel).relname).to_string_lossy().into_owned())
                }
                _ => (std::ptr::null_mut(), String::new()),
            };

            if rel.is_null() {
                if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
                    prev(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
                } else {
                    pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
                }
                return;
            }

            let query_str = CStr::from_ptr(query_string).to_string_lossy();
            let mut extracted_frames = String::new();
            let mut extracted_tenant = String::new();
            let mut extracted_cardinality = String::new();
            let mut extracted_time_column = String::new();

            let mut process_options = |list: *mut pg_sys::List| -> *mut pg_sys::List {
                let mut new_options: *mut pg_sys::List = std::ptr::null_mut();
                if list.is_null() { return new_options; }
                for i in 0..(*list).length {
                    let cell = pg_sys::list_nth(list, i) as *mut pg_sys::DefElem;
                    let mut is_spiral = false;
                    if !(*cell).defnamespace.is_null() {
                        let ns = CStr::from_ptr((*cell).defnamespace).to_string_lossy();
                        if ns == "spiral" {
                            is_spiral = true;
                            let defname = CStr::from_ptr((*cell).defname).to_string_lossy();
                            if defname == "frames" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_frames = CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            } else if defname == "tenant" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_tenant = CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            } else if defname == "cardinality" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_cardinality = CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            } else if defname == "time_column" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_time_column = CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            }
                        }
                    }
                    if !is_spiral { new_options = pg_sys::lappend(new_options, cell as *mut _); }
                }
                new_options
            };

            if tag == pg_sys::NodeTag::T_CreateStmt {
                let stmt = utility_stmt as *mut pg_sys::CreateStmt;
                (*stmt).options = process_options((*stmt).options);
            } else if tag == pg_sys::NodeTag::T_CreateTableAsStmt {
                let stmt = utility_stmt as *mut pg_sys::CreateTableAsStmt;
                let into = (*stmt).into;
                (*into).options = process_options((*into).options);
            }

            if !extracted_frames.is_empty() || !extracted_tenant.is_empty() || !extracted_cardinality.is_empty() || !extracted_time_column.is_empty()
            {
                if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
                    prev(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
                } else {
                    pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
                }
                unsafe { pg_sys::CommandCounterIncrement(); }

                let (anchor_col, offset_cols, col_types_map) = Spi::connect(|client| {
                    let q = format!("SELECT attname::text, atttypid FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped ORDER BY attnum", name.replace("\"", "\"\""));
                    let res = client.select(&q, None, &[])?;
                    let mut tstz_cols = Vec::new();
                    let mut type_map = std::collections::HashMap::new();
                    for row in res {
                        let attname = row.get::<String>(1).unwrap().unwrap();
                        let atttypid = row.get::<pg_sys::Oid>(2).unwrap().unwrap();
                        type_map.insert(attname.clone(), atttypid);
                        if atttypid == pg_sys::TIMESTAMPTZOID { tstz_cols.push(attname); }
                    }
                    let anchor = if !extracted_time_column.is_empty() { extracted_time_column.clone() } else if !tstz_cols.is_empty() { tstz_cols[0].clone() } else { "t".to_string() };
                    let offsets: Vec<String> = tstz_cols.into_iter().filter(|c| c != &anchor).collect();
                    Ok::<(String, Vec<String>, std::collections::HashMap<String, pg_sys::Oid>), spi::Error>((anchor, offsets, type_map))
                }).unwrap();

                let captured_cols = parse_magic_comments(&query_str, &col_types_map);
                for (col, formula, _) in &captured_cols {
                    let col_oid = col_types_map.get(col).copied().unwrap_or(pg_sys::InvalidOid);
                    validate_formula_column_type(col, formula, col_oid);
                }

                let mut base_metadata_map = serde_json::Map::new();
                if !extracted_cardinality.is_empty() { base_metadata_map.insert("cardinality".to_string(), serde_json::Value::String(extracted_cardinality.clone())); }
                base_metadata_map.insert("time_column".to_string(), serde_json::Value::String(anchor_col.clone()));
                base_metadata_map.insert("offset_columns".to_string(), serde_json::Value::Array(offset_cols.iter().map(|c| serde_json::Value::String(c.clone())).collect()));

                let scope_columns = if !extracted_tenant.is_empty() {
                    extracted_tenant.split(',').map(|s| s.trim().to_string()).collect()
                } else {
                    Spi::connect(|client| {
                        let q = format!("SELECT a.attname::text FROM pg_constraint c JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY(c.conkey) WHERE c.contype = 'f' AND c.conrelid = '\"{}\"'::regclass", name.replace("\"", "\"\""));
                        Ok::<Vec<String>, spi::Error>(client.select(&q, None, &[])?.map(|r| r.get::<String>(1).unwrap().unwrap()).collect())
                    }).unwrap_or_default()
                };

                catalog::insert_metadata(&name, "BASE", 0, &name, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(base_metadata_map)));
                create_reconstruction_view(&name);
                install_changelog_triggers(&name, &extracted_frames);
                generate_hierarchy_internal(&name, &extracted_frames, scope_columns, captured_cols, anchor_col, offset_cols);

                unsafe { crate::bgworker::maybe_start_worker(); }
                return;
            }
        }
    }

    if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
        prev(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
    } else {
        pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
    }
    }))
    .finally(|| {
        IN_UTILITY.with(|h| h.set(false));
        catalog::invalidate_catalog_cache();
    })
    .execute()
}

#[derive(Clone, Default)]
struct QueryConstraints {
    start: Option<i64>,
    end: Option<i64>,
    start_node: Option<*mut pg_sys::Node>,
    end_node: Option<*mut pg_sys::Node>,
    scopes: std::collections::HashMap<String, (serde_json::Value, *mut pg_sys::Node)>,
    in_clauses: std::collections::HashMap<String, (Vec<serde_json::Value>, *mut pg_sys::Node)>,
}

enum AstOpportunity {
    TimeStart { varno: i32, ts: i64, node: *mut pg_sys::Node },
    TimeEnd { varno: i32, ts: i64, node: *mut pg_sys::Node },
    ScopeEquality { varno: i32, col: String, val: serde_json::Value, node: *mut pg_sys::Node },
    ScopeSet { varno: i32, col: String, vals: Vec<serde_json::Value>, node: *mut pg_sys::Node },
    EqualColumns { v1: i32, v2: i32, col: String },
}

unsafe fn match_node(node: *mut pg_sys::Node, rtable: *mut pg_sys::List) -> Option<AstOpportunity> {
    if node.is_null() { return None; }
    match (*node).type_ {
        pg_sys::NodeTag::T_OpExpr => {
            let op = node as *mut pg_sys::OpExpr;
            let opname_ptr = pg_sys::get_opname((*op).opno); if opname_ptr.is_null() { return None; }
            let opname = CStr::from_ptr(opname_ptr).to_string_lossy();
            let args = (*op).args; if args.is_null() || (*args).length != 2 { return None; }
            let left = pg_sys::list_nth(args, 0) as *mut pg_sys::Node;
            let right = pg_sys::list_nth(args, 1) as *mut pg_sys::Node;
            let (mut left, mut right, mut opname) = (left, right, opname.into_owned());
            if (*left).type_ == pg_sys::NodeTag::T_Const && (*right).type_ != pg_sys::NodeTag::T_Const {
                std::mem::swap(&mut left, &mut right);
                opname = match opname.as_str() { ">" => "<".to_string(), ">=" => "<=".to_string(), "<" => ">".to_string(), "<=" => ">=".to_string(), other => other.to_string() };
            }
            let mut var_node = left;
            if (*left).type_ == pg_sys::NodeTag::T_FuncExpr {
                let fe = left as *mut pg_sys::FuncExpr;
                if !(*fe).args.is_null() && (*(*fe).args).length > 0 { var_node = pg_sys::list_nth((*fe).args, 0) as *mut pg_sys::Node; }
            }
            if (*var_node).type_ == pg_sys::NodeTag::T_Var && (*right).type_ == pg_sys::NodeTag::T_Const {
                let var = var_node as *mut pg_sys::Var; let varno = (*var).varno as i32;
                let rte = pg_sys::list_nth(rtable, varno - 1) as *mut pg_sys::RangeTblEntry;
                let varname_ptr = pg_sys::get_attname((*rte).relid, (*var).varattno, true); if varname_ptr.is_null() { return None; }
                let varname = CStr::from_ptr(varname_ptr).to_string_lossy();
                let con = right as *mut pg_sys::Const;
                if varname == "t" {
                    let val = match (*con).consttype {
                        pg_sys::INT8OID => Some(i64::from_datum((*con).constvalue, (*con).constisnull).unwrap()),
                        pg_sys::TIMESTAMPTZOID => {
                            let ts = i64::from_datum((*con).constvalue, (*con).constisnull).unwrap();
                            Some(ts / 1_000_000 + crate::POSTGRES_EPOCH_JDATE)
                        }
                        _ => None,
                    };
                    if let Some(ts) = val { if opname == ">=" { return Some(AstOpportunity::TimeStart { varno, ts, node }); } else if opname == "<" { return Some(AstOpportunity::TimeEnd { varno, ts, node }); } }
                } else if opname == "=" {
                    let val = match (*con).consttype {
                        pg_sys::TEXTOID => Some(serde_json::Value::String(String::from_datum((*con).constvalue, (*con).constisnull).unwrap())),
                        pg_sys::INT4OID => Some(serde_json::Value::Number(i32::from_datum((*con).constvalue, (*con).constisnull).unwrap().into())),
                        pg_sys::INT8OID => Some(serde_json::Value::Number(i64::from_datum((*con).constvalue, (*con).constisnull).unwrap().into())),
                        _ => None,
                    };
                    if let Some(v) = val { return Some(AstOpportunity::ScopeEquality { varno, col: varname.into_owned(), val: v, node }); }
                }
            } else if (*left).type_ == pg_sys::NodeTag::T_Var && (*right).type_ == pg_sys::NodeTag::T_Var && opname == "=" {
                let v1 = left as *mut pg_sys::Var; let v2 = right as *mut pg_sys::Var;
                let varno1 = (*v1).varno as i32; let varno2 = (*v2).varno as i32;
                let rte1 = pg_sys::list_nth(rtable, varno1 - 1) as *mut pg_sys::RangeTblEntry;
                let rte2 = pg_sys::list_nth(rtable, varno2 - 1) as *mut pg_sys::RangeTblEntry;
                let n1 = pg_sys::get_attname((*rte1).relid, (*v1).varattno, true);
                let n2 = pg_sys::get_attname((*rte2).relid, (*v2).varattno, true);
                if !n1.is_null() && !n2.is_null() {
                    let name1 = CStr::from_ptr(n1).to_string_lossy(); let name2 = CStr::from_ptr(n2).to_string_lossy();
                    if name1 == name2 { return Some(AstOpportunity::EqualColumns { v1: varno1, v2: varno2, col: name1.into_owned() }); }
                }
            }
        }
        pg_sys::NodeTag::T_ScalarArrayOpExpr => {
            let sao = node as *mut pg_sys::ScalarArrayOpExpr;
            let opname_ptr = pg_sys::get_opname((*sao).opno); if opname_ptr.is_null() { return None; }
            let opname = CStr::from_ptr(opname_ptr).to_string_lossy();
            if opname == "=" && (*sao).useOr {
                let args = (*sao).args; if !args.is_null() && (*args).length == 2 {
                    let left = pg_sys::list_nth(args, 0) as *mut pg_sys::Node;
                    let right = pg_sys::list_nth(args, 1) as *mut pg_sys::Node;
                    if (*left).type_ == pg_sys::NodeTag::T_Var && (*right).type_ == pg_sys::NodeTag::T_Const {
                        let var = left as *mut pg_sys::Var; let varno = (*var).varno as i32;
                        let rte = pg_sys::list_nth(rtable, varno - 1) as *mut pg_sys::RangeTblEntry;
                        let varname_ptr = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                        if !varname_ptr.is_null() {
                            let varname = CStr::from_ptr(varname_ptr).to_string_lossy();
                            let con = right as *mut pg_sys::Const;
                            let mut vals = Vec::new();
                            if !(*con).constisnull {
                                match (*con).consttype {
                                    pg_sys::INT4ARRAYOID => {
                                        if let Some(arr) = Array::<i32>::from_datum((*con).constvalue, false) {
                                            for v in arr { if let Some(v) = v { vals.push(serde_json::Value::Number(v.into())); } }
                                        }
                                    }
                                    pg_sys::TEXTARRAYOID => {
                                        if let Some(arr) = Array::<String>::from_datum((*con).constvalue, false) {
                                            for v in arr { if let Some(v) = v { vals.push(serde_json::Value::String(v)); } }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if !vals.is_empty() { return Some(AstOpportunity::ScopeSet { varno, col: varname.into_owned(), vals, node }); }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    None
}

#[derive(Clone, Copy)]
struct VisitorContext { in_and_chain: bool }

struct AstVisitor {
    rtable: *mut pg_sys::List,
    constraints: std::collections::HashMap<i32, QueryConstraints>,
    equalities: Vec<(i32, i32, String)>,
}

impl AstVisitor {
    fn new(rtable: *mut pg_sys::List) -> Self { Self { rtable, constraints: std::collections::HashMap::new(), equalities: Vec::new() } }
    unsafe fn walk(&mut self, node: *mut pg_sys::Node, context: VisitorContext) {
        if node.is_null() { return; }
        if context.in_and_chain {
            if let Some(opp) = match_node(node, self.rtable) {
                match opp {
                    AstOpportunity::TimeStart { varno, ts, node } => { let qc = self.constraints.entry(varno).or_default(); qc.start = Some(ts); qc.start_node = Some(node); }
                    AstOpportunity::TimeEnd { varno, ts, node } => { let qc = self.constraints.entry(varno).or_default(); qc.end = Some(ts); qc.end_node = Some(node); }
                    AstOpportunity::ScopeEquality { varno, col, val, node } => { let qc = self.constraints.entry(varno).or_default(); qc.scopes.insert(col, (val, node)); }
                    AstOpportunity::ScopeSet { varno, col, vals, node } => { let qc = self.constraints.entry(varno).or_default(); qc.in_clauses.insert(col, (vals, node)); }
                    AstOpportunity::EqualColumns { v1, v2, col } => { self.equalities.push((v1, v2, col)); }
                }
                return;
            }
        }
        match (*node).type_ {
            pg_sys::NodeTag::T_FromExpr => {
                let from = node as *mut pg_sys::FromExpr; self.walk((*from).quals, context);
                if !(*from).fromlist.is_null() {
                    let list = (*from).fromlist;
                    for i in 0..(*list).length { self.walk(pg_sys::list_nth(list, i) as *mut pg_sys::Node, context); }
                }
            }
            pg_sys::NodeTag::T_JoinExpr => {
                let join = node as *mut pg_sys::JoinExpr; self.walk((*join).quals, context);
                self.walk((*join).larg, context); self.walk((*join).rarg, context);
            }
            pg_sys::NodeTag::T_BoolExpr => {
                let bexpr = node as *mut pg_sys::BoolExpr; let args = (*bexpr).args;
                if (*bexpr).boolop == pg_sys::BoolExprType::AND_EXPR {
                    if !args.is_null() { for i in 0..(*args).length { self.walk(pg_sys::list_nth(args, i) as *mut pg_sys::Node, context); } }
                } else if (*bexpr).boolop == pg_sys::BoolExprType::OR_EXPR {
                    let mut hull_constraints = Vec::new();
                    if !args.is_null() {
                        for i in 0..(*args).length {
                            let mut branch_visitor = AstVisitor::new(self.rtable);
                            branch_visitor.walk(pg_sys::list_nth(args, i) as *mut pg_sys::Node, VisitorContext { in_and_chain: true });
                            hull_constraints.push(branch_visitor.constraints);
                        }
                    }
                    if !hull_constraints.is_empty() {
                        let mut merged: std::collections::HashMap<i32, QueryConstraints> = std::collections::HashMap::new();
                        let mut all_varnos = std::collections::HashSet::new();
                        for branch in &hull_constraints { for varno in branch.keys() { all_varnos.insert(*varno); } }
                        for varno in all_varnos {
                            let mut min_start: Option<i64> = None; let mut max_end: Option<i64> = None;
                            let mut common_scopes = std::collections::HashMap::new();
                            let mut first = true; let mut all_branches_have_varno = true;
                            for branch in &hull_constraints {
                                if let Some(qc) = branch.get(&varno) {
                                    if let Some(s) = qc.start { min_start = Some(min_start.map_or(s, |ms| ms.min(s))); } else { min_start = None; }
                                    if let Some(e) = qc.end { max_end = Some(max_end.map_or(e, |me| me.max(e))); } else { max_end = None; }
                                    if first { common_scopes = qc.scopes.clone(); } else { common_scopes.retain(|k, (v, _)| qc.scopes.get(k).map_or(false, |(bv, _)| v == bv)); }
                                    first = false;
                                } else { all_branches_have_varno = false; break; }
                            }
                            if all_branches_have_varno { let qc = merged.entry(varno).or_default(); qc.start = min_start; qc.end = max_end; qc.scopes = common_scopes; }
                        }
                        for (varno, qc) in merged {
                            let main_qc = self.constraints.entry(varno).or_default();
                            if let Some(s) = qc.start { main_qc.start = Some(main_qc.start.map_or(s, |ms| ms.max(s))); }
                            if let Some(e) = qc.end { main_qc.end = Some(main_qc.end.map_or(e, |me| me.min(e))); }
                            for (k, v) in qc.scopes { main_qc.scopes.insert(k, v); }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    unsafe fn run(mut self, node: *mut pg_sys::Node) -> std::collections::HashMap<i32, QueryConstraints> {
        self.walk(node, VisitorContext { in_and_chain: true });
        for _ in 0..10 {
            let mut changed = false;
            let current_equalities = self.equalities.clone();
            for (v1, v2, col) in current_equalities {
                if col == "t" {
                    let (s1, e1) = { let r = self.constraints.get(&v1); (r.and_then(|qc| qc.start), r.and_then(|qc| qc.end)) };
                    let (s2, e2) = { let r = self.constraints.get(&v2); (r.and_then(|qc| qc.start), r.and_then(|qc| qc.end)) };
                    let new_start = s1.or(s2); let new_end = e1.or(e2);
                    if new_start != s1 || new_end != e1 { let qc = self.constraints.entry(v1).or_default(); qc.start = new_start; qc.end = new_end; changed = true; }
                    if new_start != s2 || new_end != e2 { let qc = self.constraints.entry(v2).or_default(); qc.start = new_start; qc.end = new_end; changed = true; }
                } else {
                    let val1 = self.constraints.get(&v1).and_then(|qc| qc.scopes.get(&col).cloned());
                    let val2 = self.constraints.get(&v2).and_then(|qc| qc.scopes.get(&col).cloned());
                    if val1.is_some() && val2.is_none() { self.constraints.entry(v2).or_default().scopes.insert(col.clone(), val1.unwrap()); changed = true; }
                    else if val2.is_some() && val1.is_none() { self.constraints.entry(v1).or_default().scopes.insert(col.clone(), val2.unwrap()); changed = true; }
                }
            }
            if !changed { break; }
        }
        self.constraints
    }
}

unsafe fn build_time_constraints(jointree: *mut pg_sys::Node, rtable: *mut pg_sys::List) -> (std::collections::HashMap<i32, QueryConstraints>, i64) {
    let visitor = AstVisitor::new(rtable);
    let constraints = visitor.run(jointree);
    let tz_offset = Spi::get_one::<i64>("SELECT EXTRACT(EPOCH FROM utc_offset)::bigint FROM pg_timezone_names WHERE name = current_setting('TimeZone') LIMIT 1").unwrap_or(Some(0)).unwrap_or(0);
    (constraints, tz_offset)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_planner_hook(
    parse: *mut pg_sys::Query,
    query_string: *const c_char,
    cursor_options: c_int,
    bound_params: pg_sys::ParamListInfo,
) -> *mut pg_sys::PlannedStmt {
    if IN_HOOK.with(|h| h.get()) || crate::SKIP_ACCELERATION.with(|s| s.get()) || !crate::ENABLE_PLANNER_HOOK.get() {
        return if let Some(prev) = PREV_PLANNER_HOOK { prev(parse, query_string, cursor_options, bound_params) } else { pg_sys::standard_planner(parse, query_string, cursor_options, bound_params) };
    }
    IN_HOOK.with(|h| h.set(true));
    crate::SCAN_TIME_RANGE.with(|r| r.set(None));
    PgTryBuilder::new(AssertUnwindSafe(|| {
        crate::bgworker::maybe_start_worker();
        let tz_offset = Spi::get_one::<i64>("SELECT EXTRACT(EPOCH FROM utc_offset)::bigint FROM pg_timezone_names WHERE name = current_setting('TimeZone') LIMIT 1").unwrap_or(Some(0)).unwrap_or(0);
        process_query_recursive(parse, tz_offset);
        if let Some(prev) = PREV_PLANNER_HOOK { prev(parse, query_string, cursor_options, bound_params) } else { pg_sys::standard_planner(parse, query_string, cursor_options, bound_params) }
    }))
    .finally(|| IN_HOOK.with(|h| h.set(false)))
    .execute()
}

unsafe fn process_query_recursive(query: *mut pg_sys::Query, tz_offset: i64) {
    if query.is_null() || (*query).commandType != pg_sys::CmdType::CMD_SELECT { return; }
    let rtable = (*query).rtable; if rtable.is_null() { return; }
    let (constraint_map, _) = build_time_constraints((*query).jointree as *mut pg_sys::Node, rtable);

    for i in 0..(*rtable).length {
        let varno = (i + 1) as i32;
        let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
        if rte.is_null() { continue; }
        if (*rte).rtekind == pg_sys::RTEKind::RTE_SUBQUERY { process_query_recursive((*rte).subquery, tz_offset); continue; }
        if (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
            let relname = pg_sys::get_rel_name((*rte).relid);
            if !relname.is_null() {
                let base_table = CStr::from_ptr(relname).to_string_lossy().into_owned();
                let hierarchy = catalog::get_hierarchy(&base_table);
                if !hierarchy.is_empty() {
                    let offset_cols = catalog::get_offset_columns(&base_table);
                    let metadata_obj = catalog::get_metadata(&base_table);
                    let qc_opt = constraint_map.get(&varno);
                    let time_range = qc_opt.and_then(|q| q.start.zip(q.end)).or_else(|| get_actual_data_range(&base_table, &hierarchy));

                    if let Some((ts, te)) = time_range {
                        crate::SCAN_TIME_RANGE.with(|r| r.set(Some((ts, te))));
                        let scope_values = qc_opt.and_then(|qc| {
                            metadata_obj.as_ref().and_then(|m| {
                                let mut map = serde_json::Map::new();
                                for col in &m.scope_columns { if let Some(val_tuple) = qc.scopes.get(col) { map.insert(col.clone(), val_tuple.0.clone()); } }
                                if map.is_empty() { None } else { Some(pgrx::JsonB(serde_json::Value::Object(map))) }
                            })
                        });
                        let dirty_ranges = catalog::get_dirty_ranges(&base_table, ts, te, scope_values);
                        let max_frame_secs = extract_group_granularity_secs(query);
                        let segments = resolve_segments(&base_table, ts, te, &hierarchy, &dirty_ranges, tz_offset, max_frame_secs);

                        if !segments.is_empty() && (segments.len() > 1 || segments[0].source != base_table) {
                            if segments.len() > 100 { continue; }
                            
                            let base_cols_query = format!("SELECT attname::text, atttypid::regtype::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped ORDER BY attnum", base_table.replace("\"", "\"\""));
                            let base_table_columns: Vec<(String, String)> = Spi::connect(|client| {
                                Ok::<Vec<(String, String)>, spi::Error>(client.select(&base_cols_query, None, &[])?.map(|r| (r.get::<String>(1).unwrap().unwrap(), r.get::<String>(2).unwrap().unwrap())).collect())
                            }).unwrap_or_default();

                            let mut cols = Vec::new();
                            for (c, _typ) in &base_table_columns { if c != "t" { cols.push((c.clone(), None)); } }
                            if let Some(query_cols) = extract_supported_query_columns(query, rtable, &base_table) {
                                for (c, agg) in query_cols { if agg.is_some() && !cols.iter().any(|(cc, aa)| cc == &c && aa == &agg) { cols.push((c, agg)); } }
                            }

                            let col_types: std::collections::HashMap<String, String> = base_table_columns.into_iter().collect();
                            let scope_vals: Vec<(String, String)> = qc_opt.and_then(|qc| metadata_obj.as_ref().map(|m| m.scope_columns.iter().filter_map(|col| qc.scopes.get(col).and_then(|v| match &v.0 { serde_json::Value::Number(n) => Some((col.clone(), n.to_string())), serde_json::Value::String(s) => Some((col.clone(), s.clone())), _ => None })).collect())).unwrap_or_default();
                            let in_vals: Vec<(String, Vec<String>)> = qc_opt.map(|qc| qc.in_clauses.iter().map(|(col, (vals, _))| (col.clone(), vals.iter().filter_map(|v| match v { serde_json::Value::Number(n) => Some(n.to_string()), serde_json::Value::String(s) => Some(s.clone()), _ => None }).collect())).collect()).unwrap_or_default();

                            let union_sql = construct_union_sql_hierarchical(&base_table, &segments, &cols, &offset_cols, &col_types, &scope_vals, &in_vals);
                            let new_query = parse_sql_to_query(&union_sql);
                            if !new_query.is_null() {
                                (*rte).rtekind = pg_sys::RTEKind::RTE_SUBQUERY;
                                (*rte).subquery = new_query;
                                (*rte).relid = pg_sys::InvalidOid;
                                (*rte).perminfoindex = 0;

                                rewrite_query_aggregates(query, &base_table, rtable, &cols, varno);
                                if let Some(qc) = qc_opt {
                                    if let Some(node) = qc.start_node { neutralize_op_expr(node); }
                                    if let Some(node) = qc.end_node { neutralize_op_expr(node); }
                                    for (_, (_, node)) in &qc.scopes { neutralize_op_expr(*node); }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
struct Segment { source: String, _t_start: i64, _t_end: i64 }

fn resolve_segments(base_table: &str, ts: i64, te: i64, hierarchy: &[String], dirty: &[(i64, i64)], offset_seconds: i64, max_frame_secs: Option<i64>) -> Vec<Segment> {
    let mut segments = Vec::new();
    let (raw_rows, _) = catalog::get_table_stats(base_table);
    let mut sorted_hierarchy: Vec<(String, i32, f64)> = hierarchy.iter().filter_map(|h| catalog::get_metadata(h).map(|m| (h.clone(), m.frame_seconds))).filter(|h| h.1 > 0).filter(|h| max_frame_secs.is_none_or(|max| h.1 as i64 <= max)).map(|(name, secs)| { let (rows, _) = catalog::get_table_stats(&name); (name, secs, rows) }).filter(|(_, _, rows)| raw_rows <= 0.0 || *rows < raw_rows * 0.9).collect();
    sorted_hierarchy.sort_by_key(|h| -h.1);

    let mut pool = vec![(ts, te)];
    for (d_s, d_e) in dirty {
        let mut new_pool = Vec::new();
        for (p_s, p_e) in pool {
            if *d_e <= p_s || *d_s >= p_e { new_pool.push((p_s, p_e)); }
            else {
                if *d_s > p_s { new_pool.push((p_s, *d_s)); }
                if *d_e < p_e { new_pool.push((*d_e, p_e)); }
                segments.push(Segment { source: base_table.to_string(), _t_start: p_s.max(*d_s), _t_end: p_e.min(*d_e) });
            }
        }
        pool = new_pool;
    }
    for (clean_s, clean_e) in pool {
        let mut curr = clean_s;
        while curr < clean_e {
            let mut found_tier = false;
            for (h_name, frame_secs, _) in &sorted_hierarchy {
                let f_s = *frame_secs as i64;
                let bucket_start = ((curr + offset_seconds) / f_s) * f_s - offset_seconds;
                let bucket_end = bucket_start + f_s;
                if curr == bucket_start && bucket_end <= clean_e {
                    segments.push(Segment { source: h_name.clone(), _t_start: bucket_start, _t_end: bucket_end });
                    curr = bucket_end; found_tier = true; break;
                }
            }
            if !found_tier {
                let f_s = sorted_hierarchy.last().map(|h| h.1 as i64).unwrap_or(60);
                let next_boundary = ((curr + offset_seconds + f_s) / f_s) * f_s - offset_seconds;
                let segment_end = clean_e.min(next_boundary);
                segments.push(Segment { source: base_table.to_string(), _t_start: curr, _t_end: segment_end });
                curr = segment_end;
            }
        }
    }
    segments.sort_by_key(|s| s._t_start);
    let mut final_segments: Vec<Segment> = Vec::new();
    for seg in segments {
        if let Some(last) = final_segments.last_mut() {
            if last.source == seg.source && last._t_end == seg._t_start { last._t_end = seg._t_end; continue; }
        }
        final_segments.push(seg);
    }
    final_segments
}

fn get_actual_data_range(base_table: &str, hierarchy: &[String]) -> Option<(i64, i64)> {
    let mut min_t = None; let mut max_t = None;
    Spi::connect(|client| {
        for tier in hierarchy {
            if let Some(m) = catalog::get_metadata(tier) {
                if m.frame_seconds <= 0 { continue; }
                if let Ok(res) = client.select(&format!("SELECT to_regclass('\"{}\"') IS NOT NULL", tier.replace('"', "\"\"")), Some(1), &[]) {
                    if res.first().get::<bool>(1).unwrap().unwrap_or(false) {
                        if let Ok(result) = client.select(&format!("SELECT MIN(spiral(t))::bigint, MAX(spiral(t))::bigint FROM \"{}\"", tier.replace('"', "\"\"")), Some(1), &[]) {
                            if let (Some(ts), Some(te)) = (result.first().get::<i64>(1).unwrap(), result.first().get::<i64>(2).unwrap()) {
                                min_t = min_t.map(|cur| cur.min(ts)).or(Some(ts));
                                max_t = max_t.map(|cur| cur.max(te + m.frame_seconds as i64)).or(Some(te + m.frame_seconds as i64));
                            }
                        }
                    }
                }
            }
        }
        if let Ok(result) = client.select(&format!("SELECT MIN(t_start), MAX(t_end) FROM spiral.changelog WHERE base_view = '{}'", base_table.replace("'", "''")), Some(1), &[]) {
            if let (Some(ts), Some(te)) = (result.first().get::<i64>(1).unwrap(), result.first().get::<i64>(2).unwrap()) {
                min_t = min_t.map(|cur| cur.min(ts)).or(Some(ts));
                max_t = max_t.map(|cur| cur.max(te)).or(Some(te));
            }
        }
        Ok::<(), spi::Error>(())
    }).unwrap();
    min_t.zip(max_t)
}

#[pg_extern]
pub fn accelerate(relation: &str, frames: default!(Option<&str>, "NULL"), tenant: default!(Option<Vec<Option<String>>>, "NULL"), columns: default!(Option<Vec<Option<String>>>, "NULL"), time_column: default!(Option<&str>, "NULL"), initial_load: default!(bool, true)) {
    let frames_str = frames.unwrap_or(rollup::DEFAULT_FRAMES);
    let scope_columns: Vec<String> = tenant.unwrap_or_default().into_iter().flatten().collect();
    let (anchor_col, offset_cols, col_types_map) = Spi::connect(|client| {
        let res = client.select(&format!("SELECT attname::text, atttypid FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped ORDER BY attnum", relation.replace("\"", "\"\"")), None, &[])?;
        let mut tstz_cols = Vec::new(); let mut type_map = std::collections::HashMap::new();
        for row in res { let name = row.get::<String>(1).unwrap().unwrap(); let oid = row.get::<pg_sys::Oid>(2).unwrap().unwrap(); type_map.insert(name.clone(), oid); if oid == pg_sys::TIMESTAMPTZOID { tstz_cols.push(name); } }
        let anchor = time_column.map(|tc| tc.to_string()).or_else(|| tstz_cols.first().cloned()).unwrap_or_else(|| "t".to_string());
        let offsets: Vec<String> = tstz_cols.into_iter().filter(|c| c != &anchor).collect();
        Ok::<(String, Vec<String>, std::collections::HashMap<String, pg_sys::Oid>), spi::Error>((anchor, offsets, type_map))
    }).unwrap();

    let mut captured_cols = Vec::new();
    if let Some(cols) = columns {
        for col_dir in cols.into_iter().flatten() {
            let parts: Vec<&str> = col_dir.split_whitespace().collect(); if parts.len() < 2 { continue; }
            let (name, formula) = (parts[0], parts[1]);
            let alias = if parts.len() >= 4 && parts[2].to_lowercase() == "as" { Some(parts[3].to_string()) } else { None };
            if let Some(&oid) = col_types_map.get(name) { validate_formula_column_type(name, formula, oid); captured_cols.push((name.to_string(), formula.to_string(), alias)); }
        }
    }

    let mut metadata = serde_json::Map::new();
    metadata.insert("time_column".to_string(), serde_json::Value::String(anchor_col.clone()));
    metadata.insert("offset_columns".to_string(), serde_json::Value::Array(offset_cols.iter().map(|c| serde_json::Value::String(c.clone())).collect()));

    catalog::insert_metadata(relation, "BASE", 0, relation, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(metadata)));
    create_reconstruction_view(relation);
    install_changelog_triggers(relation, frames_str);
    generate_hierarchy_internal(relation, frames_str, scope_columns, captured_cols, anchor_col, offset_cols);

    unsafe { crate::bgworker::maybe_start_worker(); }
    if initial_load { let _ = Spi::run(&format!("INSERT INTO spiral.changelog (base_view, t_start, t_end) VALUES ('{}', 0, 2147483647)", relation.replace("'", "''"))); }
}

#[pg_extern]
pub fn refresh(relation: &str) {
    catalog::unify_changelog(relation);
    for tier in catalog::get_hierarchy(relation) { reactive_refresh(&tier, None); }
}

pub fn create_reconstruction_view(rel_name: &str) {
    let sql = Spi::connect(|client| {
        let mut meta = client.select(&format!("SELECT columns_metadata FROM spiral.metadata WHERE view_name = '{}'", rel_name.replace("'", "''")), Some(1), &[])?;
        if meta.is_empty() { return Ok::<Option<String>, spi::Error>(None); }
        let json: pgrx::JsonB = meta.next().unwrap().get(1).unwrap().unwrap();
        let time_col = json.0.get("time_column").and_then(|v| v.as_str()).unwrap_or("t").to_string();
        let offsets: Vec<String> = json.0.get("offset_columns").and_then(|v| v.as_array()).map(|arr| arr.iter().map(|v| v.as_str().unwrap().to_string()).collect()).unwrap_or_default();
        let cols = client.select(&format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped", rel_name.replace("\"", "\"\"")), None, &[])?;
        let mut select = Vec::new();
        for row in cols {
            let col = row.get::<String>(1).unwrap().unwrap();
            if col == "t" { select.push(format!("t AS \"{}\"", time_col)); }
            else if offsets.contains(&col) { select.push(format!("t + make_interval(secs => \"{}\"::double precision) AS \"{}\"", col, col)); }
            else { select.push(format!("\"{}\"", col)); }
        }
        Ok(Some(format!("CREATE OR REPLACE VIEW \"{}_view\" AS SELECT {} FROM \"{}\"", rel_name, select.join(", "), rel_name)))
    }).unwrap();
    if let Some(s) = sql { let _ = Spi::run(&s); }
}

pub fn install_changelog_triggers(name: &str, frames_str: &str) {
    let mut frames = rollup::parse_frames(frames_str); frames.sort_by_key(|f| f.seconds);
    let bucket = frames.first().map(|f| f.seconds as i64).unwrap_or(3600);
    for event in &["INSERT", "UPDATE", "DELETE"] {
        let transition = match *event { "UPDATE" => "REFERENCING NEW TABLE AS new_table OLD TABLE AS old_table ", "INSERT" => "REFERENCING NEW TABLE AS new_table ", "DELETE" => "REFERENCING OLD TABLE AS old_table ", _ => "" };
        let sql = format!("CREATE OR REPLACE TRIGGER spiral_track_{name}_{} AFTER {event} ON \"{name}\" {transition} FOR EACH STATEMENT EXECUTE FUNCTION spiral.track_changes_stmt('{name}', '{bucket}')", event.to_lowercase(), name=name, event=event, transition=transition, bucket=bucket);
        let _ = Spi::run(&sql);
    }
}

pub fn generate_hierarchy_internal(base_name: &str, frames_str: &str, scope_columns: Vec<String>, custom_cols: Vec<(String, String, Option<String>)>, anchor_col: String, _offset_cols: Vec<String>) {
    let mut frames = rollup::parse_frames(frames_str); frames.sort_by_key(|f| f.seconds);
    let re = regex::Regex::new(r"_\d+[smhdwmon]$").unwrap();
    let base_prefix = re.find(base_name).map_or(base_name, |m| &base_name[..m.start()]);
    let mut current_parent = base_name.to_string();

    for (i, frame) in frames.iter().enumerate() {
        let child_name = format!("{}_{}", base_prefix, frame.name); if child_name == current_parent { continue; }
        let mut select = vec![format!("to_timestamp(((spiral(\"{}\") / {f}) * {f})::double precision) as t", anchor_col, f=frame.seconds)];
        let mut group = vec![format!("(spiral(\"{}\") / {f}) * {f}", anchor_col, f=frame.seconds)];
        let mut seen = std::collections::HashSet::new(); seen.insert("t".to_string()); seen.insert(anchor_col.clone());
        let mut sources = Vec::new();

        for s in &scope_columns { if seen.insert(s.clone()) { select.push(format!("\"{}\"", s)); group.push(format!("\"{}\"", s)); } }
        if i == 0 {
            for (col, formula, alias) in &custom_cols {
                let mat = alias.clone().unwrap_or_else(|| col.clone()); if !seen.insert(mat.clone()) { continue; }
                let f_lower = formula.to_lowercase();
                let sql = if f_lower.contains("stats") { format!("spiral_stats(\"{}\") as \"{}\"", col, mat) }
                          else if f_lower.contains("ohlc") { format!("spiral_ohlcv(\"{}\", spiral(\"{}\")) as \"{}\"", col, anchor_col, mat) }
                          else if f_lower.contains("sketch") { format!("spiral_sketch(\"{}\") as \"{}\"", col, mat) }
                          else if f_lower.contains("tdigest") { format!("spiral_tdigest(\"{}\") as \"{}\"", col, mat) }
                          else { format!("{}(\"{}\") as \"{}\"", formula, col, mat) };
                select.push(sql);
                let f_name = if f_lower.contains("stats") { "stats" } else if f_lower.contains("ohlc") { "ohlcv" } else if f_lower.contains("sketch") { "sketch" } else if f_lower.contains("tdigest") { "tdigest" } else { &f_lower };
                sources.push(rollup::SourceDef { base_column: col.clone(), formula: f_name.to_string(), mat_column: mat, rollup_gsub_strategy: None });
            }
        } else {
            let (_, parent_sources) = rollup::derive_child_sql(&child_name, &current_parent, frame.seconds, &scope_columns, frame.calendar_field.as_deref());
            for src in parent_sources {
                if !seen.insert(src.mat_column.clone()) { continue; }
                let sql = match src.formula.as_str() { "stats" => "spiral_stats_merge", "sketch" => "spiral_sketch_merge", "tdigest" => "spiral_tdigest_merge", "ohlcv" => "spiral_ohlcv_merge", "range_max_end" | "range_merge" => "max", _ => "sum" };
                select.push(format!("{}(\"{}\") as \"{}\"", sql, src.mat_column, src.mat_column));
                sources.push(src);
            }
        }

        let scope_str = scope_columns.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ");
        let idx_sql = if scope_columns.is_empty() { format!("CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{child_name} ON {child_name}(t)") }
                      else { format!("CREATE INDEX IF NOT EXISTS idx_z_{child_name} ON {child_name} (spiral_zorder(spiral(t), ARRAY[{scope_str}]::text[]))") };
        let sql = format!("CREATE TABLE IF NOT EXISTS {child_name} AS SELECT {} FROM {current_parent} WHERE 1=0 GROUP BY {}; {idx_sql};", select.join(", "), group.join(", "));
        if Spi::run(&sql).is_ok() {
            catalog::insert_metadata(&child_name, &current_parent, frame.seconds, base_name, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())));
            create_reconstruction_view(&child_name);
            for src in sources { catalog::insert_source(&child_name, base_name, frame.seconds, &src.base_column, &src.formula, &src.mat_column, None, pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new()))); }
            current_parent = child_name;
        }
    }
}

fn validate_formula_column_type(col: &str, formula: &str, type_oid: pg_sys::Oid) {
    const NUMERIC: &[pg_sys::Oid] = &[pg_sys::INT2OID, pg_sys::INT4OID, pg_sys::INT8OID, pg_sys::FLOAT4OID, pg_sys::FLOAT8OID, pg_sys::NUMERICOID];
    let f = formula.to_lowercase();
    if matches!(f.as_str(), "sum" | "stats" | "ohlcv" | "tdigest" | "sketch") && !NUMERIC.contains(&type_oid) { error!("Spiral: formula '{}' on column '{}' requires numeric", formula, col); }
    if matches!(f.as_str(), "range_max_end" | "range_merge") && type_oid != pg_sys::TIMESTAMPTZOID { error!("Spiral: formula '{}' on column '{}' requires timestamptz", formula, col); }
}

pub(crate) unsafe fn parse_sql_to_query(sql: &str) -> *mut pg_sys::Query {
    let cstr = std::ffi::CString::new(sql).unwrap();
    let list = pg_sys::raw_parser(cstr.as_ptr(), pg_sys::RawParseMode::RAW_PARSE_DEFAULT);
    if list.is_null() || (*list).length == 0 { return std::ptr::null_mut(); }
    let raw = pg_sys::list_nth(list, 0) as *mut pg_sys::RawStmt;
    let queries = pg_sys::pg_analyze_and_rewrite_fixedparams(raw, cstr.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut());
    if queries.is_null() || (*queries).length == 0 { return std::ptr::null_mut(); }
    pg_sys::list_nth(queries, 0) as *mut pg_sys::Query
}

unsafe fn neutralize_op_expr(node: *mut pg_sys::Node) {
    if node.is_null() || (*node).type_ != pg_sys::NodeTag::T_OpExpr { return; }
    let op = node as *mut pg_sys::OpExpr;
    let (oid, func) = Spi::connect(|client| {
        let row = client.select("SELECT oid, oprcode FROM pg_operator WHERE oprname = '=' AND oprleft = 'bool'::regtype AND oprright = 'bool'::regtype LIMIT 1", Some(1), &[])?.next();
        Ok::<(pg_sys::Oid, pg_sys::Oid), spi::Error>(row.map(|r| (r.get(1).unwrap().unwrap(), r.get(2).unwrap().unwrap())).unwrap_or((pg_sys::InvalidOid, pg_sys::InvalidOid)))
    }).unwrap();
    if oid != pg_sys::InvalidOid {
        let c = pg_sys::makeConst(pg_sys::BOOLOID, -1, pg_sys::InvalidOid, 1, true.into_datum().unwrap(), false, true);
        (*op).opno = oid; (*op).opfuncid = func; (*op).args = pg_sys::lappend(pg_sys::lappend(std::ptr::null_mut(), c as *mut _), c as *mut _);
    }
}

fn is_supported_rollup_aggregate(agg: &str) -> bool {
    matches!(agg.to_lowercase().as_str(), "sum" | "min" | "max" | "count" | "avg" | "first" | "last" | "spiral_stats" | "spiral_tdigest" | "spiral_sketch" | "spiral_stats_merge" | "spiral_tdigest_merge" | "spiral_sketch_merge")
}

pub(crate) unsafe fn rewrite_query_aggregates(
    query: *mut pg_sys::Query,
    base_table: &str,
    rtable: *mut pg_sys::List,
    detected_cols: &[(String, Option<String>)],
    varno: i32,
) {
    if !(*query).hasAggs { return; }
    let oids = [
        Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_stats_merge' AND pronargs = 1").unwrap_or(None),
        Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_tdigest_merge' AND pronargs = 1").unwrap_or(None),
        Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_sketch_merge' AND pronargs = 1").unwrap_or(None),
        Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_ohlcv_merge' AND pronargs = 1").unwrap_or(None),
    ];
    unsafe fn walk_and_rewrite(node: *mut pg_sys::Node, base_table: &str, rtable: *mut pg_sys::List, detected_cols: &[(String, Option<String>)], varno: i32, oids: &[Option<pg_sys::Oid>]) {
        if node.is_null() { return; }
        if (*node).type_ == pg_sys::NodeTag::T_Aggref {
            let agg = node as *mut pg_sys::Aggref;
            let mut current_cols = Vec::new();
            if walk_expr(node, base_table, rtable, &mut current_cols) && !current_cols.is_empty() {
                let (expr_str, found_agg) = &current_cols[0];
                if let Some(idx) = detected_cols.iter().position(|(c, a)| c == expr_str && a.as_ref().map(|s| s.to_lowercase()) == found_agg.as_ref().map(|s| s.to_lowercase())) {
                    let attno = (idx + 2) as i16;
                    let formula = Spi::get_one_with_args::<String>("SELECT formula FROM spiral.sources WHERE base_view = $1 AND base_column = $2 LIMIT 1", vec![(pg_sys::TEXTOID, base_table.into_datum()), (pg_sys::TEXTOID, expr_str.into_datum())]).unwrap_or(None).unwrap_or_else(|| "sum".to_string());
                    let mut new_fn_oid = None;
                    let mut is_json_target = false;
                    if formula == "ohlcv" { new_fn_oid = oids[3]; is_json_target = true; }
                    else if formula == "stats" { new_fn_oid = oids[0]; is_json_target = true; }
                    else if formula == "tdigest" { new_fn_oid = oids[1]; is_json_target = true; }
                    else if formula == "sketch" { new_fn_oid = oids[2]; is_json_target = true; }
                    if let Some(oid) = new_fn_oid { (*agg).aggfnoid = oid; }
                    let var = pg_sys::makeVar(varno, attno, if is_json_target { pg_sys::JSONBOID } else { (*agg).aggtype }, -1, pg_sys::InvalidOid, 0);
                    let tle = pg_sys::makeTargetEntry(var as *mut pg_sys::Expr, 1, std::ptr::null_mut(), false);
                    (*agg).args = pg_sys::lappend(std::ptr::null_mut(), tle as *mut _);
                    (*agg).aggstar = false;
                }
            }
        } else if (*node).type_ == pg_sys::NodeTag::T_FuncExpr { let f = node as *mut pg_sys::FuncExpr; if !(*f).args.is_null() { for i in 0..(*(*f).args).length { walk_and_rewrite(pg_sys::list_nth((*f).args, i) as *mut pg_sys::Node, base_table, rtable, detected_cols, varno, oids); } } }
        else if (*node).type_ == pg_sys::NodeTag::T_OpExpr { let o = node as *mut pg_sys::OpExpr; if !(*o).args.is_null() { for i in 0..(*(*o).args).length { walk_and_rewrite(pg_sys::list_nth((*o).args, i) as *mut pg_sys::Node, base_table, rtable, detected_cols, varno, oids); } } }
    }
    let target_list = (*query).targetList;
    if !target_list.is_null() { for i in 0..(*target_list).length { let tle = pg_sys::list_nth(target_list, i) as *mut pg_sys::TargetEntry; walk_and_rewrite((*tle).expr as *mut pg_sys::Node, base_table, rtable, detected_cols, varno, &oids); } }
}

pub(crate) unsafe fn walk_expr(node: *mut pg_sys::Node, base_table: &str, rtable: *mut pg_sys::List, cols: &mut Vec<(String, Option<String>)>) -> bool {
    if node.is_null() { return true; }
    match (*node).type_ {
        pg_sys::NodeTag::T_Var => {
            let var = node as *mut pg_sys::Var; if (*var).varno == 0 || (*var).varno as usize > (*rtable).length as usize { return false; }
            let rte = pg_sys::list_nth(rtable, (*var).varno as i32 - 1) as *mut pg_sys::RangeTblEntry;
            if rte.is_null() || (*rte).relid == pg_sys::InvalidOid { return (*rte).rtekind == pg_sys::RTEKind::RTE_GROUP; }
            let name = pg_sys::get_rel_name((*rte).relid); if name.is_null() || CStr::from_ptr(name).to_string_lossy() != base_table { return false; }
            let att = pg_sys::get_attname((*rte).relid, (*var).varattno, true); if att.is_null() { return false; }
            cols.push((CStr::from_ptr(att).to_string_lossy().into_owned(), None)); true
        }
        pg_sys::NodeTag::T_Aggref => {
            let agg = node as *mut pg_sys::Aggref;
            if !(*agg).aggdistinct.is_null() || !(*agg).aggorder.is_null() || !(*agg).aggfilter.is_null() { return false; }
            let f = pg_sys::get_func_name((*agg).aggfnoid); if f.is_null() { return false; }
            let name = CStr::from_ptr(f).to_string_lossy().to_lowercase();
            if !is_supported_rollup_aggregate(&name) { return false; }
            if (*agg).aggstar { if name == "count" { cols.push(("*".to_string(), Some("count".to_string()))); return true; } return false; }
            let args = (*agg).args; if args.is_null() || (name != "first" && name != "last" && (*args).length != 1) || ((name == "first" || name == "last") && (*args).length != 2) { return false; }
            if !walk_expr((* (pg_sys::list_nth(args, 0) as *mut pg_sys::TargetEntry)).expr as *mut _, base_table, rtable, cols) { return false; }
            if let Some(c) = cols.last_mut() { c.1 = Some(name); } true
        }
        pg_sys::NodeTag::T_OpExpr => {
            let op = node as *mut pg_sys::OpExpr; let args = (*op).args; if args.is_null() || (*args).length != 2 { return false; }
            let (l, r) = (pg_sys::list_nth(args, 0) as *mut pg_sys::Node, pg_sys::list_nth(args, 1) as *mut pg_sys::Node);
            let name = CStr::from_ptr(pg_sys::get_opname((*op).opno)).to_string_lossy();
            if matches!(name.as_ref(), "+" | "-" | "*") { if (*l).type_ == pg_sys::NodeTag::T_Var { return walk_expr(l, base_table, rtable, cols); } else if (*r).type_ == pg_sys::NodeTag::T_Var { return walk_expr(r, base_table, rtable, cols); } }
            for i in 0..(*args).length { if !walk_expr(pg_sys::list_nth(args, i) as *mut _, base_table, rtable, cols) { return false; } } true
        }
        pg_sys::NodeTag::T_FuncExpr => {
            let f = node as *mut pg_sys::FuncExpr; if (*f).args.is_null() { return true; }
            for i in 0..(*(*f).args).length { if !walk_expr(pg_sys::list_nth((*f).args, i) as *mut _, base_table, rtable, cols) { return false; } } true
        }
        pg_sys::NodeTag::T_Const => true,
        _ => false
    }
}

pub(crate) unsafe fn extract_supported_query_columns(query: *mut pg_sys::Query, rtable: *mut pg_sys::List, base_table: &str) -> Option<Vec<(String, Option<String>)>> {
    if !(*query).hasAggs || !(*query).havingQual.is_null() || !(*query).distinctClause.is_null() || !(*query).windowClause.is_null() { return None; }
    let mut cols = Vec::new();
    for i in 0..(*(*query).targetList).length {
        let tle = pg_sys::list_nth((*query).targetList, i) as *mut pg_sys::TargetEntry;
        if !(*tle).resjunk && !walk_expr((*tle).expr as *mut _, base_table, rtable, &mut cols) { return None; }
    }
    if cols.is_empty() { None } else { Some(cols) }
}

pub(crate) unsafe fn extract_group_granularity_secs(query: *mut pg_sys::Query) -> Option<i64> {
    let group = (*query).groupClause; if group.is_null() { return None; }
    for i in 0..(*group).length {
        let sgc = pg_sys::list_nth(group, i) as *mut pg_sys::SortGroupClause;
        for j in 0..(*(*query).targetList).length {
            let tle = pg_sys::list_nth((*query).targetList, j) as *mut pg_sys::TargetEntry;
            if !tle.is_null() && (*tle).ressortgroupref == (*sgc).tleSortGroupRef {
                let expr = (*tle).expr as *mut pg_sys::Node;
                if (*expr).type_ == pg_sys::NodeTag::T_FuncExpr {
                    let fe = expr as *mut pg_sys::FuncExpr;
                    if CStr::from_ptr(pg_sys::get_func_name((*fe).funcid)).to_string_lossy() == "date_trunc" {
                        let arg = pg_sys::list_nth((*fe).args, 0) as *mut pg_sys::Const;
                        let field: Option<String> = String::from_datum((*arg).constvalue, (*arg).constisnull);
                        return match field.as_deref() { Some("second") => Some(1), Some("minute") => Some(60), Some("hour") => Some(3600), Some("day") => Some(86400), _ => None };
                    }
                }
            }
        }
    }
    None
}

pub fn map_agg_inner(agg: &str, col: &str, rollup: bool, formula: &str) -> String {
    let a = agg.to_lowercase();
    if !rollup {
        let f = match formula { "stats" => "spiral_stats_accum", "ohlcv" => "spiral_ohlcv_accum", "tdigest" => "spiral_tdigest_accum", "sketch" => "spiral_sketch_accum", _ => if matches!(a.as_str(), "spiral_stats" | "spiral_tdigest" | "spiral_sketch" | "spiral_ohlcv") { return format!("{}_accum(NULL, \"{}\")", a, col); } else { return format!("\"{}\"", col); } };
        return if formula == "ohlcv" { format!("{}(NULL, \"{}\", spiral(t))", f, col) } else { format!("{}(NULL, \"{}\")", f, col) };
    }
    match (a.as_str(), formula) { ("sum", "ohlcv") => format!("(\"{}\"->>'v')::float8", col), ("max", "ohlcv") => format!("(\"{}\"->>'h')::float8", col), ("min", "ohlcv") => format!("(\"{}\"->>'l')::float8", col), ("first", "ohlcv") => format!("(\"{}\"->>'o')::float8", col), ("last", "ohlcv") => format!("(\"{}\"->>'c')::float8", col), ("min", "stats") => format!("(\"{}\"->>'min')::numeric", col), ("max", "stats") => format!("(\"{}\"->>'max')::numeric", col), _ => format!("\"{}\"", col) }
}

fn format_epoch(e: i64) -> String { Spi::get_one::<String>(&format!("SELECT to_char(to_timestamp({}::double precision), 'YYYY-MM-DD HH24:MI:SS')", e)).unwrap().unwrap() }
fn reconstruction_expr(col: &str, f: &str, rollup: bool) -> String { if !rollup { format!("\"{}\"", col) } else { match f { "range_max_end" | "range_merge" => format!("t + make_interval(secs => \"{}\"::double precision) AS \"{}\"", col, col), _ => format!("\"{}\"", col) } } }

fn construct_union_sql_hierarchical(base_table: &str, segments: &[Segment], cols: &[(String, Option<String>)], offset_cols: &[catalog::OffsetColumn], col_types: &std::collections::HashMap<String, String>, scope_vals: &[(String, String)], in_vals: &[(String, Vec<String>)]) -> String {
    let mut sources: Vec<String> = segments.iter().map(|s| s.source.clone()).collect(); sources.sort(); sources.dedup();
    let mut s_secs: Vec<(String, i32)> = sources.into_iter().map(|s| (s.clone(), if s == base_table { 0 } else { catalog::get_metadata(&s).unwrap().frame_seconds })).collect();
    s_secs.sort_by_key(|s| -s.1);
    let mut parts = Vec::new();
    for (src, _) in s_secs {
        let rollup = src != base_table;
        let mut inner = vec!["t::timestamptz".to_string()];
        for (i, (col, agg)) in cols.iter().enumerate() {
            let alias = format!("spiral_col_{}", i);
            let (formula, mapped) = if rollup {
                Spi::connect(|client| {
                    let row = client.select(&format!("SELECT formula, mat_column FROM spiral.sources WHERE view_name = '{}' AND base_column = '{}' LIMIT 1", src.replace("'", "''"), col.replace("'", "''")), Some(1), &[])?.next();
                    Ok::<(String, String), spi::Error>(row.map(|r| (r.get(1).unwrap().unwrap(), r.get(2).unwrap().unwrap())).unwrap_or((String::new(), col.clone())))
                }).unwrap()
            } else { (String::new(), col.clone()) };
            let expr = if let Some(oc) = offset_cols.iter().find(|o| o.mat_column == *col) { reconstruction_expr(&mapped, &oc.formula, rollup) } else if let Some(a) = agg { map_agg_inner(a, &mapped, rollup, &formula) } else { format!("\"{}\"", mapped) };
            inner.push(format!("{} AS {}", expr, alias));
        }
        let segs: Vec<&Segment> = segments.iter().filter(|s| s.source == src).collect(); if segs.is_empty() { continue; }
        let t_pred = if segs.len() == 1 { format!("t >= '{}'::timestamptz AND t < '{}'::timestamptz", format_epoch(segs[0]._t_start), format_epoch(segs[0]._t_end)) } else { format!("t <@ '{{ {} }}'::tstzmultirange", segs.iter().map(|s| format!("[\"{}\", \"{}\")", format_epoch(s._t_start), format_epoch(s._t_end))).collect::<Vec<_>>().join(", ")) };
        let s_pred = scope_vals.iter().map(|(c, v)| format!(" AND \"{}\" = '{}'", c, v.replace("'", "''"))).collect::<String>();
        parts.push(format!("SELECT {} FROM \"{}\" WHERE {}{}", inner.join(", "), src, t_pred, s_pred));
    }
    parts.join(" UNION ALL ")
}

fn parse_scope_predicate(wc: &str) -> Option<(String, i64)> { let p: Vec<&str> = wc.trim().splitn(2, '=').collect(); if p.len() == 2 { Some((p[0].trim().to_string(), p[1].trim().parse().ok()?)) } else { None } }
fn build_changelog_scope_filter(_rel: &str, col: &str, val: i64, scopes: &[String]) -> Option<String> { if scopes.contains(&col.to_string()) { Some(format!("AND scope_values @> '{{\"{}\": {}}}'::jsonb", col, val)) } else { None } }

pub fn reactive_refresh(base_name: &str, where_clause: Option<String>) -> bool {
    let meta = catalog::get_metadata(base_name); if meta.is_none() { return false; }
    let m = meta.unwrap(); let root = m.parent_view == m.base_view || m.parent_view == "BASE";
    let filter = where_clause.as_deref().and_then(|wc| parse_scope_predicate(wc).and_then(|(c, v)| build_changelog_scope_filter(&m.base_view, &c, v, &m.scope_columns)));
    if root { let _ = Spi::run(&format!("CREATE TEMP TABLE refreshing_changelog AS SELECT ctid as old_ctid FROM spiral.changelog WHERE base_view = '{}' {}", m.base_view.replace("'", "''"), filter.as_deref().unwrap_or(""))); }
    let success = crate::refresh_incremental(base_name, where_clause, 0, None);
    if success && root { let _ = Spi::run("DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM refreshing_changelog)"); }
    if root { let _ = Spi::run("DROP TABLE IF EXISTS refreshing_changelog"); }
    success
}

pub fn reactive_refresh_by_scope(base_name: &str, scope: String) {
    let _ = Spi::run(&format!("CREATE TEMP TABLE refreshing_changelog AS SELECT ctid as old_ctid FROM spiral.changelog WHERE base_view = (SELECT base_view FROM spiral.metadata WHERE view_name='{base_name}') AND scope_values = '{scope}'::jsonb"));
    if crate::refresh_incremental(base_name, None, 0, Some(scope)) { let _ = Spi::run("DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM refreshing_changelog)"); }
    let _ = Spi::run("DROP TABLE IF EXISTS refreshing_changelog");
}

#[pg_extern]
pub fn spiral_explain(sql: &str) -> String {
    unsafe {
        let q = parse_sql_to_query(sql); if q.is_null() { return "Parse error".into(); }
        let mut report = String::new(); explain_query_recursive(q, &mut report); report
    }
}

unsafe fn explain_query_recursive(query: *mut pg_sys::Query, report: &mut String) {
    if query.is_null() || (*query).commandType != pg_sys::CmdType::CMD_SELECT { return; }
    let rtable = (*query).rtable; if rtable.is_null() { return; }
    let (constraints, tz) = build_time_constraints((*query).jointree as *mut _, rtable);
    for i in 0..(*rtable).length {
        let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
        if (*rte).rtekind == pg_sys::RTEKind::RTE_SUBQUERY { explain_query_recursive((*rte).subquery, report); continue; }
        if (*rte).rtekind != pg_sys::RTEKind::RTE_RELATION { continue; }
        let name = CStr::from_ptr(pg_sys::get_rel_name((*rte).relid)).to_string_lossy().into_owned();
        let h = catalog::get_hierarchy(&name); if h.is_empty() { continue; }
        if let Some(qc) = constraints.get(&((i+1) as i32)) {
            if let (Some(ts), Some(te)) = (qc.start, qc.end) {
                let segments = resolve_segments(&name, ts, te, &h, &[], tz, None);
                report.push_str(&format!("Accelerating {name}: {} segments\n", segments.len()));
            }
        }
    }
}

pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook; pg_sys::ProcessUtility_hook = Some(spiral_process_utility_hook);
    PREV_PLANNER_HOOK = pg_sys::planner_hook; pg_sys::planner_hook = Some(spiral_planner_hook);
}
