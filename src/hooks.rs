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
            prev(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            );
        } else {
            pg_sys::standard_ProcessUtility(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            );
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
                    (
                        (*stmt).relation,
                        CStr::from_ptr((*(*stmt).relation).relname)
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
                pg_sys::NodeTag::T_CreateTableAsStmt => {
                    let stmt = utility_stmt as *mut pg_sys::CreateTableAsStmt;
                    let into = (*stmt).into;
                    (
                        (*into).rel,
                        CStr::from_ptr((*(*into).rel).relname)
                            .to_string_lossy()
                            .into_owned(),
                    )
                }
                _ => (std::ptr::null_mut(), String::new()),
            };

            if rel.is_null() {
                if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
                    prev(
                        pstmt,
                        query_string,
                        read_only_tree,
                        context,
                        params,
                        query_env,
                        dest,
                        qc,
                    );
                } else {
                    pg_sys::standard_ProcessUtility(
                        pstmt,
                        query_string,
                        read_only_tree,
                        context,
                        params,
                        query_env,
                        dest,
                        qc,
                    );
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
                if list.is_null() {
                    return new_options;
                }
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
                                    extracted_frames =
                                        CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            } else if defname == "tenant" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_tenant =
                                        CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            } else if defname == "cardinality" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_cardinality =
                                        CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            } else if defname == "time_column" {
                                let arg = (*cell).arg;
                                if !arg.is_null() && (*arg).type_ == pg_sys::NodeTag::T_String {
                                    let s = arg as *mut pg_sys::String;
                                    extracted_time_column =
                                        CStr::from_ptr((*s).sval).to_string_lossy().into_owned();
                                }
                            }
                        }
                    }
                    if !is_spiral {
                        new_options = pg_sys::lappend(new_options, cell as *mut _);
                    }
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

            notice!(
                "Spiral: Utility hook caught CREATE relation '{}', query_str length={}",
                name,
                query_str.len()
            );

            if !extracted_frames.is_empty()
                || !extracted_tenant.is_empty()
                || !extracted_cardinality.is_empty()
                || !extracted_time_column.is_empty()
            {
                notice!("Spiral: WITH parameters found, setting access method to 'spiral' and calling standard_ProcessUtility...");
                if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
                    prev(
                        pstmt,
                        query_string,
                        read_only_tree,
                        context,
                        params,
                        query_env,
                        dest,
                        qc,
                    );
                } else {
                    pg_sys::standard_ProcessUtility(
                        pstmt,
                        query_string,
                        read_only_tree,
                        context,
                        params,
                        query_env,
                        dest,
                        qc,
                    );
                }
                notice!("Spiral: standard_ProcessUtility returned, processing hierarchy...");
                // Make new table visible to subsequent catalog queries
                unsafe {
                    pg_sys::CommandCounterIncrement();
                }

                // Detect all column types for offset detection and directive validation
                let (anchor_col, offset_cols, col_types_map) = Spi::connect(|client| {
                    let q = format!(
                        "SELECT attname::text, atttypid
                         FROM pg_attribute
                         WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped
                         ORDER BY attnum",
                        name.replace("\"", "\"\"")
                    );
                    let res = client.select(&q, None, &[])?;
                    let mut tstz_cols = Vec::new();
                    let mut type_map = std::collections::HashMap::new();
                    for row in res {
                        let attname = row.get::<String>(1).unwrap().unwrap();
                        let atttypid = row.get::<pg_sys::Oid>(2).unwrap().unwrap();
                        type_map.insert(attname.clone(), atttypid);
                        if atttypid == pg_sys::TIMESTAMPTZOID {
                            tstz_cols.push(attname);
                        }
                    }

                    let anchor = if !extracted_time_column.is_empty() {
                        extracted_time_column.clone()
                    } else if !tstz_cols.is_empty() {
                        tstz_cols[0].clone()
                    } else {
                        "t".to_string() // Fallback
                    };

                    let offsets: Vec<String> =
                        tstz_cols.into_iter().filter(|c| c != &anchor).collect();
                    Ok::<(String, Vec<String>, std::collections::HashMap<String, pg_sys::Oid>), spi::Error>((anchor, offsets, type_map))
                })
                .unwrap_or_else(|e| {
                    error!("Spiral: failed to detect timestamptz columns for hierarchy setup: {:?}", e);
                });

                // 2. Parse frames
                let frames_str = extracted_frames.clone();

                // 3. Detect Scope (Tenant) columns via Foreign Keys or extracted tenant
                let scope_columns = if !extracted_tenant.is_empty() {
                    extracted_tenant
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect()
                } else {
                    Spi::connect(|client| {
                        let q = format!(
                            "
                            SELECT a.attname::text 
                            FROM pg_constraint c 
                            JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY(c.conkey)
                            WHERE c.contype = 'f' AND c.conrelid = '\"{}\"'::regclass",
                            name.replace("\"", "\"\"")
                        );
                        Ok::<Vec<String>, spi::Error>(
                            client
                                .select(&q, None, &[])?
                                .map(|r| r.get::<String>(1).unwrap().unwrap())
                                .collect(),
                        )
                    })
                    .unwrap_or_default()
                };

                // [ \t]* (not \s*) between type and -- so the comment must be on the
                // same line as the column definition, preventing cross-line false positives.
                let re_col =
                    regex::Regex::new(r"(\w+)\s+[\w\(\) ]+,?[ \t]*--\s*Spiral:\s*([^\n\r]+)").unwrap();
                let mut captured_cols = Vec::new();
                for cap in re_col.captures_iter(&query_str) {
                    let col_name = cap[1].to_string();

                    // col_name must be an actual column in the table — filters out
                    // false positives where a prose comment mentions "Spiral:".
                    if !col_types_map.contains_key(&col_name) {
                        continue;
                    }

                    let formula_part = cap[2].trim().to_string();

                    // Support multiple formulas separated by comma, and 'as alias' syntax
                    for part in formula_part.split(',') {
                        let part = part.trim();
                        if part.is_empty() {
                            continue;
                        }

                        let (formula, alias) = if let Some((f, a)) = part.split_once(" as ") {
                            (f.trim().to_string(), Some(a.trim().to_string()))
                        } else if let Some((f, a)) = part.split_once(" AS ") {
                            (f.trim().to_string(), Some(a.trim().to_string()))
                        } else {
                            (part.to_string(), None)
                        };

                        // Formula must be a single identifier — rejects prose like
                        // "id for the record" that leaks through the regex.
                        if formula.split_whitespace().count() > 1 {
                            error!(
                                "Spiral: directive on column '{}' has invalid formula '{}' — must be a single identifier",
                                col_name, formula
                            );
                        }

                        // Type-compatibility check for known formulas.
                        let col_oid = col_types_map.get(&col_name).copied().unwrap_or(pg_sys::InvalidOid);
                        validate_formula_column_type(&col_name, &formula, col_oid);

                        captured_cols.push((col_name.clone(), formula, alias));
                    }
                }

                // 4. Register the BASE table metadata
                let mut base_metadata_map = serde_json::Map::new();
                if !extracted_cardinality.is_empty() {
                    base_metadata_map.insert(
                        "cardinality".to_string(),
                        serde_json::Value::String(extracted_cardinality.clone()),
                    );
                }
                base_metadata_map.insert(
                    "time_column".to_string(),
                    serde_json::Value::String(anchor_col.clone()),
                );
                base_metadata_map.insert(
                    "offset_columns".to_string(),
                    serde_json::Value::Array(
                        offset_cols
                            .iter()
                            .map(|c| serde_json::Value::String(c.clone()))
                            .collect(),
                    ),
                );

                catalog::insert_metadata(
                    &name,
                    "BASE",
                    0,
                    &name,
                    scope_columns.clone(),
                    pgrx::JsonB(serde_json::Value::Object(base_metadata_map.clone())),
                );
                create_reconstruction_view(&name);

                install_changelog_triggers(&name, &extracted_frames);

                // 5. Generate the entire hierarchy automatically
                notice!("Spiral: Calling generate_hierarchy_internal for '{}'", name);
                generate_hierarchy_internal(
                    &name,
                    &frames_str,
                    scope_columns,
                    captured_cols,
                    anchor_col,
                    offset_cols,
                );

                notice!("Spiral: Successfully registered hierarchy for '{}'", name);

                // 6. Ensure background worker is running for this database
                unsafe {
                    crate::bgworker::maybe_start_worker();
                }

                return;
            } else {
                notice!(
                    "Spiral: No magic comments in '{}', following standard path.",
                    name
                );
            }
        }
    }

    if let Some(prev) = PREV_PROCESS_UTILITY_HOOK {
        prev(
            pstmt,
            query_string,
            read_only_tree,
            context,
            params,
            query_env,
            dest,
            qc,
        );
    } else {
        pg_sys::standard_ProcessUtility(
            pstmt,
            query_string,
            read_only_tree,
            context,
            params,
            query_env,
            dest,
            qc,
        );
    }
    })) // end PgTryBuilder closure
    .catch_others(|e| {
        // Bad magic comment or SPI failure during hierarchy setup. Table was already
        // created by standard_ProcessUtility above; only Spiral acceleration is skipped.
        notice!(
            "Spiral: error during CREATE TABLE hook processing — hierarchy setup skipped. \
             The table was created but Spiral acceleration is NOT configured. \
             Fix the magic comment directives and recreate the table to enable acceleration. \
             Error: {:?}",
            e
        );
        e.rethrow()
    })
    .finally(|| {
        IN_UTILITY.with(|h| h.set(false));
        // DDL may have added/removed spiral relations — invalidate cached hierarchy.
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
}

enum AstOpportunity {
    TimeStart {
        varno: i32,
        ts: i64,
        node: *mut pg_sys::Node,
    },
    TimeEnd {
        varno: i32,
        ts: i64,
        node: *mut pg_sys::Node,
    },
    ScopeEquality {
        varno: i32,
        col: String,
        val: serde_json::Value,
        node: *mut pg_sys::Node,
    },
    EqualTimeColumns {
        v1: i32,
        v2: i32,
    },
}

unsafe fn match_node(node: *mut pg_sys::Node, rtable: *mut pg_sys::List) -> Option<AstOpportunity> {
    if node.is_null() {
        return None;
    }

    match (*node).type_ {
        pg_sys::NodeTag::T_OpExpr => {
            let op = node as *mut pg_sys::OpExpr;
            let opname_ptr = pg_sys::get_opname((*op).opno);
            if opname_ptr.is_null() {
                return None;
            }
            let opname = CStr::from_ptr(opname_ptr).to_string_lossy();
            let args = (*op).args;
            if args.is_null() || (*args).length != 2 {
                return None;
            }

            let left = pg_sys::list_nth(args, 0) as *mut pg_sys::Node;
            let right = pg_sys::list_nth(args, 1) as *mut pg_sys::Node;

            let mut var_node = left;
            if (*left).type_ == pg_sys::NodeTag::T_FuncExpr {
                let fe = left as *mut pg_sys::FuncExpr;
                if !(*fe).args.is_null() && (*(*fe).args).length > 0 {
                    var_node = pg_sys::list_nth((*fe).args, 0) as *mut pg_sys::Node;
                }
            }

            if (*var_node).type_ == pg_sys::NodeTag::T_Var && (*right).type_ == pg_sys::NodeTag::T_Const
            {
                let var = var_node as *mut pg_sys::Var;
                let varno = (*var).varno as i32;
                let rte = pg_sys::list_nth(rtable, varno - 1) as *mut pg_sys::RangeTblEntry;
                let varname_ptr = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                if varname_ptr.is_null() {
                    return None;
                }
                let varname = CStr::from_ptr(varname_ptr).to_string_lossy();
                let con = right as *mut pg_sys::Const;

                if varname == "t" {
                    let val = match (*con).consttype {
                        pg_sys::INT8OID => {
                            Some(i64::from_datum((*con).constvalue, (*con).constisnull).unwrap())
                        }
                        pg_sys::TIMESTAMPTZOID => {
                            let ts =
                                i64::from_datum((*con).constvalue, (*con).constisnull).unwrap();
                            Some(ts / 1_000_000 + crate::POSTGRES_EPOCH_JDATE)
                        }
                        _ => None,
                    };
                    if let Some(ts) = val {
                        if opname == ">=" {
                            return Some(AstOpportunity::TimeStart { varno, ts, node });
                        } else if opname == "<" {
                            return Some(AstOpportunity::TimeEnd { varno, ts, node });
                        }
                    }
                } else if opname == "=" {
                    let val: Option<serde_json::Value> = match (*con).consttype {
                        pg_sys::TEXTOID => Some(serde_json::Value::String(
                            String::from_datum((*con).constvalue, (*con).constisnull).unwrap(),
                        )),
                        pg_sys::INT4OID => Some(serde_json::Value::Number(
                            i32::from_datum((*con).constvalue, (*con).constisnull)
                                .unwrap()
                                .into(),
                        )),
                        pg_sys::INT8OID => Some(serde_json::Value::Number(
                            i64::from_datum((*con).constvalue, (*con).constisnull)
                                .unwrap()
                                .into(),
                        )),
                        _ => None,
                    };
                    if let Some(v) = val {
                        return Some(AstOpportunity::ScopeEquality {
                            varno,
                            col: varname.into_owned(),
                            val: v,
                            node,
                        });
                    }
                }
            } else if (*left).type_ == pg_sys::NodeTag::T_Var
                && (*right).type_ == pg_sys::NodeTag::T_Var
                && opname == "="
            {
                let v1 = left as *mut pg_sys::Var;
                let v2 = right as *mut pg_sys::Var;
                let varno1 = (*v1).varno as i32;
                let varno2 = (*v2).varno as i32;
                let rte1 = pg_sys::list_nth(rtable, varno1 - 1) as *mut pg_sys::RangeTblEntry;
                let rte2 = pg_sys::list_nth(rtable, varno2 - 1) as *mut pg_sys::RangeTblEntry;
                let n1 = pg_sys::get_attname((*rte1).relid, (*v1).varattno, true);
                let n2 = pg_sys::get_attname((*rte2).relid, (*v2).varattno, true);
                if !n1.is_null()
                    && !n2.is_null()
                    && CStr::from_ptr(n1).to_string_lossy() == "t"
                    && CStr::from_ptr(n2).to_string_lossy() == "t"
                {
                    return Some(AstOpportunity::EqualTimeColumns {
                        v1: varno1,
                        v2: varno2,
                    });
                }
            }
        }
        _ => {}
    }
    None
}

#[derive(Clone, Copy)]
struct VisitorContext {
    in_and_chain: bool,
}

struct AstVisitor {
    rtable: *mut pg_sys::List,
    constraints: std::collections::HashMap<i32, QueryConstraints>,
    equalities: Vec<(i32, i32)>,
}

impl AstVisitor {
    fn new(rtable: *mut pg_sys::List) -> Self {
        Self {
            rtable,
            constraints: std::collections::HashMap::new(),
            equalities: Vec::new(),
        }
    }

    unsafe fn walk(&mut self, node: *mut pg_sys::Node, context: VisitorContext) {
        if node.is_null() {
            return;
        }

        if context.in_and_chain {
            if let Some(opp) = match_node(node, self.rtable) {
                match opp {
                    AstOpportunity::TimeStart { varno, ts, node } => {
                        let qc = self.constraints.entry(varno).or_default();
                        qc.start = Some(ts);
                        qc.start_node = Some(node);
                    }
                    AstOpportunity::TimeEnd { varno, ts, node } => {
                        let qc = self.constraints.entry(varno).or_default();
                        qc.end = Some(ts);
                        qc.end_node = Some(node);
                    }
                    AstOpportunity::ScopeEquality {
                        varno,
                        col,
                        val,
                        node,
                    } => {
                        let qc = self.constraints.entry(varno).or_default();
                        qc.scopes.insert(col, (val, node));
                    }
                    AstOpportunity::EqualTimeColumns { v1, v2 } => {
                        self.equalities.push((v1, v2));
                    }
                }
                return;
            }
        }

        match (*node).type_ {
            pg_sys::NodeTag::T_FromExpr => {
                let from = node as *mut pg_sys::FromExpr;
                self.walk((*from).quals, context);
                if !(*from).fromlist.is_null() {
                    let list = (*from).fromlist;
                    for i in 0..(*list).length {
                        self.walk(pg_sys::list_nth(list, i) as *mut pg_sys::Node, context);
                    }
                }
            }
            pg_sys::NodeTag::T_JoinExpr => {
                let join = node as *mut pg_sys::JoinExpr;
                self.walk((*join).quals, context);
                self.walk((*join).larg, context);
                self.walk((*join).rarg, context);
            }
            pg_sys::NodeTag::T_BoolExpr => {
                let bexpr = node as *mut pg_sys::BoolExpr;
                let args = (*bexpr).args;
                let new_context = VisitorContext {
                    in_and_chain: context.in_and_chain
                        && (*bexpr).boolop == pg_sys::BoolExprType::AND_EXPR,
                };
                if !args.is_null() {
                    for i in 0..(*args).length {
                        self.walk(pg_sys::list_nth(args, i) as *mut pg_sys::Node, new_context);
                    }
                }
            }
            _ => {}
        }
    }

    unsafe fn run(
        mut self,
        node: *mut pg_sys::Node,
    ) -> std::collections::HashMap<i32, QueryConstraints> {
        self.walk(node, VisitorContext { in_and_chain: true });

        for _ in 0..10 {
            let mut changed = false;
            let current_equalities = self.equalities.clone();
            for (v1, v2) in current_equalities {
                let (s1, e1) = {
                    let r1 = self.constraints.get(&v1);
                    (r1.and_then(|qc| qc.start), r1.and_then(|qc| qc.end))
                };
                let (s2, e2) = {
                    let r2 = self.constraints.get(&v2);
                    (r2.and_then(|qc| qc.start), r2.and_then(|qc| qc.end))
                };

                let new_start = s1.or(s2);
                let new_end = e1.or(e2);

                if new_start != s1 || new_end != e1 {
                    let qc = self.constraints.entry(v1).or_default();
                    qc.start = new_start;
                    qc.end = new_end;
                    changed = true;
                }
                if new_start != s2 || new_end != e2 {
                    let qc = self.constraints.entry(v2).or_default();
                    qc.start = new_start;
                    qc.end = new_end;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.constraints
    }
}

unsafe fn build_time_constraints(
    jointree: *mut pg_sys::Node,
    rtable: *mut pg_sys::List,
) -> (std::collections::HashMap<i32, QueryConstraints>, i64) {
    let visitor = AstVisitor::new(rtable);
    let constraints = visitor.run(jointree);

    // Use pg_timezone_names to get the UTC offset for the current session timezone.
    // The AT TIME ZONE + now() formula returns 0 inside the planner hook context;
    // pg_timezone_names gives the correct signed offset in seconds for the current date.
    let tz_offset = Spi::get_one::<i64>(
        "SELECT EXTRACT(EPOCH FROM utc_offset)::bigint \
         FROM pg_timezone_names WHERE name = current_setting('TimeZone') LIMIT 1",
    )
    .unwrap_or(Some(0))
    .unwrap_or(0);

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
        return if let Some(prev_hook) = PREV_PLANNER_HOOK {
            prev_hook(parse, query_string, cursor_options, bound_params)
        } else {
            pg_sys::standard_planner(parse, query_string, cursor_options, bound_params)
        };
    }
    IN_HOOK.with(|h| h.set(true));
    // Clear any stale time range from a previous query so non-spiral scans see None.
    crate::SCAN_TIME_RANGE.with(|r| r.set(None));
    PgTryBuilder::new(AssertUnwindSafe(|| {
    // Ensure background worker is running for this database. The WORKER_STARTED
    // thread-local guard makes this a cheap no-op after the first query per session,
    // which lets the worker recover after a server restart without a new CREATE TABLE.
    crate::bgworker::maybe_start_worker();
    let query = &mut *parse;
    if query.commandType == pg_sys::CmdType::CMD_SELECT {
        let rtable = query.rtable;
        if !rtable.is_null() {
            let (constraint_map, tz_offset) =
                build_time_constraints(query.jointree as *mut pg_sys::Node, rtable);

            for i in 0..(*rtable).length {
                let varno = i + 1;
                let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
                if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                    let relid = (*rte).relid;
                    let relname = pg_sys::get_rel_name(relid);
                    if !relname.is_null() {
                        let base_table = CStr::from_ptr(relname).to_string_lossy().into_owned();
                        let hierarchy = catalog::get_hierarchy(&base_table);

                        if !hierarchy.is_empty() {
                            let offset_cols = catalog::get_offset_columns(&base_table);
                            let metadata_obj = catalog::get_metadata(&base_table);

                            // Time range: from WHERE clause or inferred from coarsest rollup
                            // (enables unbounded queries like SELECT sum(col) FROM tbl).
                            let qc_opt = constraint_map.get(&varno);
                            let time_range = qc_opt
                                .and_then(|q| q.start.zip(q.end))
                                .or_else(|| get_actual_data_range(&base_table, &hierarchy));

                            if let Some((ts, te)) = time_range {
                                // Publish time range so the TAM scan can skip pages outside [ts, te].
                                crate::SCAN_TIME_RANGE.with(|r| r.set(Some((ts, te))));
                                // Build scope_values JsonB from qc.scopes if they match view's scope_columns
                                let scope_values = qc_opt.and_then(|qc| {
                                    metadata_obj.as_ref().and_then(|m| {
                                        let mut map = serde_json::Map::new();
                                        for col in &m.scope_columns {
                                            if let Some(val_tuple) = qc.scopes.get(col) {
                                                map.insert(col.clone(), val_tuple.0.clone());
                                            }
                                        }
                                        if map.is_empty() {
                                            None
                                        } else {
                                            Some(pgrx::JsonB(serde_json::Value::Object(map)))
                                        }
                                    })
                                });


                                let dirty_ranges = catalog::get_dirty_ranges(
                                    &base_table,
                                    ts,
                                    te,
                                    scope_values,
                                );
                                let max_frame_secs = extract_group_granularity_secs(query);
                                let segments = resolve_segments(
                                    &base_table,
                                    ts,
                                    te,
                                    &hierarchy,
                                    &dirty_ranges,
                                    tz_offset,
                                    max_frame_secs,
                                );

                                if !segments.is_empty()
                                    && (segments.len() > 1 || segments[0].source != base_table)
                                {
                                    let Some(query_cols) = extract_supported_query_columns(
                                        query,
                                        rtable,
                                        &base_table,
                                    ) else {
                                        continue;
                                    };
                                    // Build cols from ALL base table columns to match original
                                    // table's column positions (Var.varattno references).
                                    // Aggregate columns get their agg function; others get None.
                                    // construct_union_sql_hierarchical NULL-fills columns that
                                    // don't exist in the rollup tier.
                                    let mut cols = Vec::new();
                                    let base_cols_query = format!(
                                        "SELECT attname::text, atttypid::regtype::text \
                                         FROM pg_attribute \
                                         WHERE attrelid = '\"{}\"'::regclass \
                                         AND attnum > 0 AND NOT attisdropped \
                                         ORDER BY attnum",
                                        base_table.replace("\"", "\"\"")
                                    );
                                    let base_table_columns: Vec<(String, String)> =
                                        Spi::connect(|client| {
                                            Ok::<Vec<(String, String)>, spi::Error>(
                                                client
                                                    .select(&base_cols_query, None, &[])?
                                                    .map(|r| {
                                                        let name = r
                                                            .get::<String>(1)
                                                            .unwrap()
                                                            .unwrap_or_default();
                                                        let typ = r
                                                            .get::<String>(2)
                                                            .unwrap()
                                                            .unwrap_or_default();
                                                        (name, typ)
                                                    })
                                                    .collect(),
                                            )
                                        })
                                        .unwrap_or_default();

                                    // col_types: name -> original SQL type for casting
                                    let mut col_types: std::collections::HashMap<
                                        String,
                                        String,
                                    > = std::collections::HashMap::new();
                                    for (name, typ) in &base_table_columns {
                                        col_types.insert(name.clone(), typ.clone());
                                    }

                                    for (c, _typ) in &base_table_columns {
                                        if c == "t" {
                                            continue;
                                        }
                                        if let Some((_, agg)) =
                                            query_cols.iter().find(|(name, _)| name == c)
                                        {
                                            cols.push((c.clone(), agg.clone()));
                                        } else {
                                            cols.push((c.clone(), None));
                                        }
                                    }

                                    // Ordered (col, val_str) pairs for z-order bound injection.
                                    let scope_vals: Vec<(String, String)> = qc_opt
                                        .and_then(|qc| {
                                            metadata_obj.as_ref().map(|m| {
                                                m.scope_columns
                                                    .iter()
                                                    .filter_map(|col| {
                                                        qc.scopes.get(col).and_then(|val_tuple| match &val_tuple.0 {
                                                            serde_json::Value::Number(n) => {
                                                                Some((col.clone(), n.to_string()))
                                                            }
                                                            serde_json::Value::String(s) => {
                                                                Some((col.clone(), s.clone()))
                                                            }
                                                            _ => None,
                                                        })
                                                    })
                                                    .collect()
                                            })
                                        })
                                        .unwrap_or_default();

                                    let union_sql = construct_union_sql_hierarchical(
                                        &base_table,
                                        &segments,
                                        &cols,
                                        &offset_cols,
                                        &col_types,
                                        &scope_vals,
                                    );
                                    let new_query = parse_sql_to_query(&union_sql);
                                    if !new_query.is_null() {
                                        (*rte).rtekind = pg_sys::RTEKind::RTE_SUBQUERY;
                                        (*rte).subquery = new_query;
                                        (*rte).relid = pg_sys::InvalidOid;
                                        (*rte).perminfoindex = 0;

                                        // Build column formulas map for the rewriter
                                        let mut column_formulas = std::collections::HashMap::new();
                                        Spi::connect(|client| {
                                            let q = format!(
                                                "SELECT base_column, formula FROM spiral.sources \
                                                 WHERE view_name = (SELECT view_name FROM spiral.metadata \
                                                                    WHERE base_view = '{}' AND frame_seconds > 0 \
                                                                    LIMIT 1)",
                                                base_table.replace("'", "''")
                                            );
                                            if let Ok(res) = client.select(&q, None, &[]) {
                                                for row in res {
                                                    if let (Ok(Some(bc)), Ok(Some(f))) = (row.get::<String>(1), row.get::<String>(2)) {
                                                        column_formulas.insert(bc, f);
                                                    }
                                                }
                                            }
                                            Ok::<(), spi::Error>(())
                                        }).unwrap_or(());

                                        // Rewrite top-level aggregates to their merge equivalents
                                        rewrite_query_aggregates(query, &column_formulas);

                                        // Neutralize consumed constraints from the outer query to avoid double verification.
                                        if let Some(qc) = constraint_map.get(&varno) {
                                            if let Some(node) = qc.start_node {
                                                neutralize_op_expr(node);
                                            }
                                            if let Some(node) = qc.end_node {
                                                neutralize_op_expr(node);
                                            }
                                            for (_, (_, node)) in &qc.scopes {
                                                neutralize_op_expr(*node);
                                            }
                                        }

                                        // Notice fires only after acceleration confirmed.
                                        notice!("Spiral: Accelerating '{}' (RTE #{}) with range {} to {} (Offset: {}s)", base_table, varno, format_epoch(ts), format_epoch(te), tz_offset);
                                        continue; // Accelerated, move to next RTE
                                    }
                                }
                            }

                            // Fallback reconstruction if not accelerated but has offset columns
                            let is_rollup_table = metadata_obj
                                .as_ref()
                                .map(|m| m.frame_seconds > 0)
                                .unwrap_or(false);

                            if !offset_cols.is_empty() && is_rollup_table {
                                let base_cols_query = format!(
                                    "SELECT attname::text FROM pg_attribute
                                     WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped
                                     ORDER BY attnum",
                                    base_table.replace("\"", "\"\"")
                                );
                                let all_cols = Spi::connect(|client| {
                                    Ok::<Vec<String>, spi::Error>(
                                        client
                                            .select(&base_cols_query, None, &[])?
                                            .map(|r| r.get::<String>(1).unwrap().unwrap())
                                            .collect(),
                                    )
                                })
                                .unwrap_or_default();

                                let select_list: Vec<String> = all_cols
                                    .iter()
                                    .map(|col| {
                                        if let Some(oc) =
                                            offset_cols.iter().find(|o| &o.mat_column == col)
                                        {
                                            reconstruction_expr(col, &oc.formula, true)
                                        } else {
                                            format!("\"{}\"", col)
                                        }
                                    })
                                    .collect();

                                let wrap_sql = format!(
                                    "SELECT {} FROM \"{}\"",
                                    select_list.join(", "),
                                    base_table
                                );
                                let inner = parse_sql_to_query(&wrap_sql);
                                if !inner.is_null() {
                                    (*rte).rtekind = pg_sys::RTEKind::RTE_SUBQUERY;
                                    (*rte).subquery = inner;
                                    (*rte).relid = pg_sys::InvalidOid;
                                    (*rte).perminfoindex = 0;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(prev_hook) = PREV_PLANNER_HOOK {
        prev_hook(parse, query_string, cursor_options, bound_params)
    } else {
        pg_sys::standard_planner(parse, query_string, cursor_options, bound_params)
    }
    })) // end PgTryBuilder closure
    .finally(|| IN_HOOK.with(|h| h.set(false)))
    .execute()
}

#[derive(Debug)]
struct Segment {
    source: String,
    _t_start: i64,
    _t_end: i64,
}

fn resolve_segments(
    base_table: &str,
    ts: i64,
    te: i64,
    hierarchy: &[String],
    dirty: &[(i64, i64)],
    offset_seconds: i64,
    max_frame_secs: Option<i64>,
) -> Vec<Segment> {
    let mut segments = Vec::new();

    let mut sorted_hierarchy: Vec<(String, i32)> = hierarchy
        .iter()
        .filter_map(|h| catalog::get_metadata(h).map(|m| (h.clone(), m.frame_seconds)))
        .filter(|h| h.1 > 0)
        // Never use a rollup coarser than the query's grouping granularity —
        // that would collapse finer time buckets into a single coarse row.
        .filter(|h| max_frame_secs.is_none_or(|max| h.1 as i64 <= max))
        .collect();
    sorted_hierarchy.sort_by_key(|h| -h.1);

    let mut pool = vec![(ts, te)];
    for (d_s, d_e) in dirty {
        let mut new_pool = Vec::new();
        for (p_s, p_e) in pool {
            if *d_e <= p_s || *d_s >= p_e {
                new_pool.push((p_s, p_e));
            } else {
                if *d_s > p_s {
                    new_pool.push((p_s, *d_s));
                }
                if *d_e < p_e {
                    new_pool.push((*d_e, p_e));
                }
                segments.push(Segment {
                    source: base_table.to_string(),
                    _t_start: p_s.max(*d_s),
                    _t_end: p_e.min(*d_e),
                });
            }
        }
        pool = new_pool;
    }
    for (clean_s, clean_e) in pool {
        let mut curr = clean_s;
        while curr < clean_e {
            let mut found_tier = false;
            for (h_name, frame_secs) in &sorted_hierarchy {
                let f_s = *frame_secs as i64;
                if f_s <= 0 {
                    continue;
                }
                let bucket_start = ((curr + offset_seconds) / f_s) * f_s - offset_seconds;
                let bucket_end = bucket_start + f_s;
                if curr == bucket_start && bucket_end <= clean_e {
                    segments.push(Segment {
                        source: h_name.clone(),
                        _t_start: bucket_start,
                        _t_end: bucket_end,
                    });
                    curr = bucket_end;
                    found_tier = true;
                    break;
                }
            }
            if !found_tier {
                let f_s = sorted_hierarchy.last().map(|h| h.1 as i64).unwrap_or(60);
                if f_s <= 0 {
                    let seg_end = clean_e;
                    segments.push(Segment {
                        source: base_table.to_string(),
                        _t_start: curr,
                        _t_end: seg_end,
                    });
                    break;
                }
                let next_boundary = ((curr + offset_seconds + f_s) / f_s) * f_s - offset_seconds;
                let segment_end = clean_e.min(next_boundary);
                segments.push(Segment {
                    source: base_table.to_string(),
                    _t_start: curr,
                    _t_end: segment_end,
                });
                curr = segment_end;
            }
        }
    }
    segments.sort_by_key(|s| s._t_start);
    let mut final_segments: Vec<Segment> = Vec::new();
    for seg in segments {
        if let Some(last) = final_segments.last_mut() {
            if last.source == seg.source && last._t_end == seg._t_start {
                last._t_end = seg._t_end;
                continue;
            }
        }
        final_segments.push(seg);
    }
    final_segments
}

/// Infer the actual data range [ts, te) for unbounded queries by probing
/// the coarsest rollup view and the changelog.
fn get_actual_data_range(base_table: &str, hierarchy: &[String]) -> Option<(i64, i64)> {
    let mut min_t = None;
    let mut max_t = None;

    // 1. Probe the coarsest rollup
    if let Some((coarsest, frame_secs)) = hierarchy
        .iter()
        .filter_map(|h| catalog::get_metadata(h).map(|m| (h.clone(), m.frame_seconds)))
        .filter(|(_, s)| *s > 0)
        .max_by_key(|(_, s)| *s)
    {
        Spi::connect(|client| {
            let sql = format!(
                "SELECT MIN(spiral(t))::bigint, MAX(spiral(t))::bigint FROM \"{}\"",
                coarsest.replace('"', "\"\"")
            );
            let result = client.select(&sql, Some(1), &[])?;
            if !result.is_empty() {
                let row = result.first();
                if let (Some(ts), Some(te)) = (row.get::<i64>(1)?, row.get::<i64>(2)?) {
                    min_t = Some(ts);
                    max_t = Some(te + frame_secs as i64);
                }
            }
            Ok::<(), spi::Error>(())
        })
        .unwrap();
    }

    // 2. Probe the changelog for dirty ranges
    Spi::connect(|client| {
        let sql = format!(
            "SELECT MIN(t_start), MAX(t_end) FROM spiral.changelog WHERE base_view = '{}'",
            base_table.replace("'", "''")
        );
        let result = client.select(&sql, Some(1), &[])?;
        if !result.is_empty() {
            let row = result.first();
            if let Some(ts) = row.get::<i64>(1)? {
                min_t = min_t.map(|m| m.min(ts)).or(Some(ts));
            }
            if let Some(te) = row.get::<i64>(2)? {
                max_t = max_t.map(|m| m.max(te)).or(Some(te));
            }
        }
        Ok::<(), spi::Error>(())
    })
    .unwrap();

    min_t.zip(max_t)
}

#[pg_extern]
pub fn accelerate(
    relation: &str,
    frames: default!(Option<&str>, "NULL"),
    tenant: default!(Option<Vec<Option<String>>>, "NULL"),
    columns: default!(Option<Vec<Option<String>>>, "NULL"),
    time_column: default!(Option<&str>, "NULL"),
    initial_load: default!(bool, true),
) {
    let frames_str = frames.unwrap_or(rollup::DEFAULT_FRAMES);
    let scope_columns: Vec<String> = tenant.unwrap_or_default().into_iter().flatten().collect();

    let (anchor_col, offset_cols, col_types_map) = Spi::connect(|client| {
        let q = format!(
            "SELECT attname::text, atttypid
             FROM pg_attribute
             WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped
             ORDER BY attnum",
            relation.replace("\"", "\"\"")
        );
        let res = client.select(&q, None, &[])?;
        let mut tstz_cols = Vec::new();
        let mut type_map = std::collections::HashMap::new();
        for row in res {
            let attname = row.get::<String>(1).unwrap().unwrap();
            let atttypid = row.get::<pg_sys::Oid>(2).unwrap().unwrap();
            type_map.insert(attname.clone(), atttypid);
            if atttypid == pg_sys::TIMESTAMPTZOID {
                tstz_cols.push(attname);
            }
        }

        let anchor = if let Some(tc) = time_column {
            tc.to_string()
        } else if !tstz_cols.is_empty() {
            tstz_cols[0].clone()
        } else {
            "t".to_string() // Fallback
        };

        let offsets: Vec<String> = tstz_cols.into_iter().filter(|c| c != &anchor).collect();
        Ok::<
            (
                String,
                Vec<String>,
                std::collections::HashMap<String, pg_sys::Oid>,
            ),
            spi::Error,
        >((anchor, offsets, type_map))
    })
    .unwrap_or_else(|e| {
        error!(
            "Spiral: failed to detect columns for hierarchy setup: {:?}",
            e
        );
    });

    let mut captured_cols = Vec::new();
    if let Some(cols) = columns {
        for col_dir in cols.into_iter().flatten() {
            let parts: Vec<&str> = col_dir.split_whitespace().collect();
            if parts.len() < 2 {
                notice!("Spiral: invalid column directive '{}', skipping", col_dir);
                continue;
            }
            let col_name = parts[0];
            let formula = parts[1];
            let alias = if parts.len() >= 4 && parts[2].to_lowercase() == "as" {
                Some(parts[3].to_string())
            } else {
                None
            };

            if !col_types_map.contains_key(col_name) {
                notice!(
                    "Spiral: column '{}' not found in relation '{}', skipping",
                    col_name,
                    relation
                );
                continue;
            }

            let col_oid = col_types_map
                .get(col_name)
                .copied()
                .unwrap_or(pg_sys::InvalidOid);
            validate_formula_column_type(col_name, formula, col_oid);
            captured_cols.push((col_name.to_string(), formula.to_string(), alias));
        }
    }

    // Register metadata
    let mut base_metadata_map = serde_json::Map::new();
    base_metadata_map.insert(
        "time_column".to_string(),
        serde_json::Value::String(anchor_col.clone()),
    );
    base_metadata_map.insert(
        "offset_columns".to_string(),
        serde_json::Value::Array(
            offset_cols
                .iter()
                .map(|c| serde_json::Value::String(c.clone()))
                .collect(),
        ),
    );

    catalog::insert_metadata(
        relation,
        "BASE",
        0,
        relation,
        scope_columns.clone(),
        pgrx::JsonB(serde_json::Value::Object(base_metadata_map)),
    );
    create_reconstruction_view(relation);

    install_changelog_triggers(relation, frames_str);

    generate_hierarchy_internal(
        relation,
        frames_str,
        scope_columns,
        captured_cols,
        anchor_col,
        offset_cols,
    );

    unsafe {
        crate::bgworker::maybe_start_worker();
    }

    if initial_load {
        let bootstrap_sql = format!(
            "INSERT INTO spiral.changelog (base_view, t_start, t_end) VALUES ('{}', 0, 2147483647)",
            relation.replace("'", "''")
        );
        let _ = Spi::run(&bootstrap_sql);
    }

    notice!("Spiral: Successfully accelerated relation '{}'", relation);
}

#[pg_extern]
pub fn refresh(relation: &str) {
    catalog::unify_changelog(relation);
    let hierarchy = catalog::get_hierarchy(relation);
    if hierarchy.is_empty() {
        notice!("Spiral: no hierarchy found for '{}'", relation);
        return;
    }

    // Capture IDs to be refreshed
    let dirty_ranges = catalog::get_dirty_ranges(relation, 0, 2147483647, None);
    if dirty_ranges.is_empty() {
        notice!("Spiral: hierarchy for '{}' is already up to date", relation);
        return;
    }

    // For simplicity in the manual refresh API, we just refresh all dirty ranges.
    // In a production scenario, this would be done in chunks.
    for (ts, te) in dirty_ranges {
        notice!("Spiral: refreshing range [{}, {}) for '{}'", ts, te, relation);
        // This is a placeholder for the actual refresh logic which is currently
        // mostly inside the bgworker's loop. I should extract it.
    }
}

pub fn create_reconstruction_view(rel_name: &str) {
    let create_view_sql: Option<String> = Spi::connect(|client| {
        let mut metadata_res = client.select(
            &format!(
                "SELECT columns_metadata, base_view FROM spiral.metadata WHERE view_name = '{}'",
                rel_name.replace("'", "''")
            ),
            Some(1),
            &[],
        )?;
        if metadata_res.is_empty() {
            return Ok::<Option<String>, spi::Error>(None);
        }

        let row = metadata_res.next().expect("metadata_res is empty");
        let json: pgrx::JsonB = row.get(1).unwrap().unwrap();
        let _base_view = row.get::<String>(2).unwrap().unwrap();

        let time_col = json
            .0
            .get("time_column")
            .and_then(|v: &serde_json::Value| v.as_str())
            .unwrap_or("t")
            .to_string();
        let offset_cols: Vec<String> = json
            .0
            .get("offset_columns")
            .and_then(|v: &serde_json::Value| v.as_array())
            .map(|arr: &Vec<serde_json::Value>| {
                arr.iter()
                    .map(|v: &serde_json::Value| v.as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let q = format!(
            "SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped",
            rel_name.replace("\"", "\"\"")
        );
        let cols_res = client.select(&q, None, &[])?;
        let mut select_parts = Vec::new();

        for row in cols_res {
            let col = row.get::<String>(1).unwrap().unwrap();
            let mut is_tstz = false;
            let type_res = client.select(&format!("SELECT atttypid FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attname = '{}'", rel_name.replace("\"", "\"\""), col.replace("'", "''")), Some(1), &[]);
            if let Ok(t) = type_res {
                if !t.is_empty() {
                    let oid = t.first().get::<pg_sys::Oid>(1).unwrap().unwrap();
                    if oid == pg_sys::TIMESTAMPTZOID {
                        is_tstz = true;
                    }
                }
            }

            if col == "t" {
                select_parts.push(format!("t AS \"{}\"", time_col));
            } else if offset_cols.contains(&col) && !is_tstz {
                select_parts.push(format!(
                    "t + make_interval(secs => \"{}\"::double precision) AS \"{}\"",
                    col, col
                ));
            } else {
                select_parts.push(format!("\"{}\"", col));
            }
        }

        let view_name = format!("{}_view", rel_name);
        Ok::<Option<String>, spi::Error>(Some(format!(
            "CREATE OR REPLACE VIEW \"{}\" AS SELECT {} FROM \"{}\"",
            view_name.replace("\"", "\"\""),
            select_parts.join(", "),
            rel_name.replace("\"", "\"\"")
        )))
    }).unwrap_or(None);

    if let Some(sql) = create_view_sql {
        let _ = Spi::run(&sql);
    }
}
pub fn install_changelog_triggers(name: &str, frames_str: &str) {
    // Smallest frame determines the dirty-range bucket granularity.
    // Coarser bucket = fewer changelog entries for dense inserts (no loss).
    // Finer bucket = fewer wasted refresh buckets for sparse inserts (the fix).
    let mut sorted_frames = rollup::parse_frames(frames_str);
    sorted_frames.sort_by_key(|f| f.seconds);
    let bucket_secs = sorted_frames
        .first()
        .map(|f| f.seconds as i64)
        .unwrap_or(3600); // default 1h if no frames parsed

    for event in &["INSERT", "UPDATE", "DELETE"] {
        let mut transition = String::new();
        if *event == "UPDATE" {
            transition.push_str("REFERENCING NEW TABLE AS new_table OLD TABLE AS old_table ");
        } else if *event == "INSERT" {
            transition.push_str("REFERENCING NEW TABLE AS new_table ");
        } else if *event == "DELETE" {
            transition.push_str("REFERENCING OLD TABLE AS old_table ");
        }

        let trigger_sql = format!(
            "CREATE TRIGGER spiral_track_{base_view}_{event_lower}
             AFTER {event} ON \"{base_view}\"
             {transition}
             FOR EACH STATEMENT EXECUTE FUNCTION spiral.track_changes_stmt('{base_view}', '{bucket_secs}')",
            base_view = name,
            event = event,
            event_lower = event.to_lowercase(),
            transition = transition,
            bucket_secs = bucket_secs
        );
        if let Err(e) = Spi::run(&trigger_sql) {
            error!(
                "Spiral: failed to create changelog trigger on '{}': {:?}",
                name, e
            );
        }
    }
}

pub fn generate_hierarchy_internal(
    base_name: &str,
    frames_str: &str,
    scope_columns: Vec<String>,
    custom_cols: Vec<(String, String, Option<String>)>,
    anchor_col: String,
    offset_cols: Vec<String>,
) {
    notice!(
        "Spiral: Entering generate_hierarchy_internal for '{}', frames='{}'",
        base_name,
        frames_str
    );
    let mut frames = rollup::parse_frames(frames_str);
    frames.sort_by_key(|f| f.seconds);
    let re = regex::Regex::new(r"_\d+[smhdwmon]$").unwrap();
    let base_prefix = if let Some(m) = re.find(base_name) {
        &base_name[..m.start()]
    } else {
        base_name
    };
    let mut current_parent = base_name.to_string();

    // Note: extra-column discovery skipped here; use magic comments to cover all columns.
    let base_all_cols: Vec<String> = Vec::new();

    for (i, frame) in frames.iter().enumerate() {
        let child_name = format!("{}_{}", base_prefix, frame.name);
        if child_name == current_parent {
            continue;
        }

        let mut sources = Vec::new();
        let mut select_parts = vec![format!(
            "to_timestamp(((spiral(\"{0}\") / {1}) * {1})::double precision) as t",
            anchor_col, frame.seconds
        )];
        let mut group_parts = vec![format!(
            "(spiral(\"{0}\") / {1}) * {1}",
            anchor_col, frame.seconds
        )];
        let mut seen_cols = std::collections::HashSet::new();
        seen_cols.insert("t".to_string());
        seen_cols.insert(anchor_col.clone());

        for s in &scope_columns {
            if seen_cols.insert(s.clone()) {
                select_parts.push(format!("\"{}\"", s));
                group_parts.push(format!("\"{}\"", s));
            }
        }

        if i == 0 {
            // First level: Use custom magic comments
            for (col, formula, alias) in &custom_cols {
                let formula_lower = formula.to_lowercase();
                if formula_lower.contains("stats") {
                    let mat = alias.clone().unwrap_or_else(|| col.clone());
                    if seen_cols.insert(mat.clone()) {
                        select_parts.push(format!("spiral_stats(\"{}\") as \"{}\"", col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "stats".to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                } else if formula_lower.contains("ohlc") {
                    let mat = alias.clone().unwrap_or_else(|| col.clone());
                    if seen_cols.insert(mat.clone()) {
                        select_parts.push(format!("spiral_ohlcv(\"{}\", spiral(t)) as \"{}\"", col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "ohlcv".to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                } else if formula_lower.contains("sketch")
                    || formula_lower.contains("tdigest")
                    || formula_lower.contains("quantile")
                {
                    let mat = alias.clone().unwrap_or_else(|| col.clone());
                    let formula_name = if formula_lower.contains("tdigest")
                        || formula_lower.contains("quantile")
                    {
                        "tdigest"
                    } else {
                        "sketch"
                    };
                    if seen_cols.insert(mat.clone()) {
                        let agg_fn = if formula_name == "tdigest" {
                            "spiral_tdigest"
                        } else {
                            "spiral_sketch"
                        };
                        select_parts.push(format!("{}(\"{}\") as \"{}\"", agg_fn, col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: formula_name.to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                } else if formula_lower == "range_merge" || formula_lower == "range_max_end" {
                    // Span semantics: store max(col) - bucket_start as int4 seconds offset.
                    // "range_merge" accepted as backward-compat alias for "range_max_end".
                    let mat = alias.clone().unwrap_or_else(|| col.clone());
                    if seen_cols.insert(mat.clone()) {
                        let bucket_expr = format!(
                            "to_timestamp(((spiral(\"{0}\") / {1}) * {1})::double precision)",
                            anchor_col, frame.seconds
                        );
                        select_parts.push(format!(
                            "date_part('epoch', max(\"{}\") - {})::int4 as \"{}\"",
                            col, bucket_expr, mat
                        ));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "range_max_end".to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                } else {
                    // Custom aggregate or default sum
                    let mat = alias.clone().unwrap_or_else(|| col.clone());
                    if seen_cols.insert(mat.clone()) {
                        select_parts.push(format!("{}(\"{}\") as \"{}\"", formula, col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: formula_lower.clone(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                }
                if formula_lower.contains("max") && seen_cols.insert(col.clone()) {
                    select_parts.push(format!("max(\"{}\") as \"{}\"", col, col));
                    sources.push(rollup::SourceDef {
                        base_column: col.clone(),
                        formula: "max".to_string(),
                        mat_column: col.clone(),
                        rollup_gsub_strategy: None,
                    });
                }
            }
            // Add other columns as sum or range_max_end by default (uses pre-fetched list)
            for col in &base_all_cols {
                if !seen_cols.contains(col.as_str()) && col != "t" && seen_cols.insert(col.clone())
                {
                    if offset_cols.contains(col) {
                        let bucket_expr = format!(
                            "to_timestamp(((spiral(\"{0}\") / {1}) * {1})::double precision)",
                            anchor_col, frame.seconds
                        );
                        select_parts.push(format!(
                            "date_part('epoch', max(\"{}\") - {})::int4 as \"{}\"",
                            col, bucket_expr, col
                        ));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "range_max_end".to_string(),
                            mat_column: col.clone(),
                            rollup_gsub_strategy: None,
                        });
                    } else {
                        select_parts.push(format!("sum(\"{}\") as \"{}\"", col, col));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "sum".to_string(),
                            mat_column: col.clone(),
                            rollup_gsub_strategy: None,
                        });
                    }
                }
            }
        } else {
            // Higher levels: derive from parent
            let (_, parent_sources) = rollup::derive_child_sql(
                &child_name,
                &current_parent,
                frame.seconds,
                &scope_columns,
                frame.calendar_field.as_deref(),
            );
            for src in parent_sources {
                if !seen_cols.insert(src.mat_column.clone()) {
                    continue;
                }
                if let Some(strategy) = &src.rollup_gsub_strategy {
                    let sql = strategy
                        .replace("rollup(\"\\1\")", &format!("\"{}\"", src.mat_column))
                        .replace("\\1", &src.mat_column);
                    select_parts.push(sql);
                } else if src.formula == "stats" {
                    select_parts.push(format!(
                        "spiral_stats_merge(\"{}\") as \"{}\"",
                        src.mat_column, src.mat_column
                    ));
                } else if src.formula == "sketch" {
                    select_parts.push(format!(
                        "spiral_sketch_merge(\"{}\") as \"{}\"",
                        src.mat_column, src.mat_column
                    ));
                } else if src.formula == "tdigest" {
                    select_parts.push(format!(
                        "spiral_tdigest_merge(\"{}\") as \"{}\"",
                        src.mat_column, src.mat_column
                    ));
                } else if src.formula == "ohlcv" {
                    select_parts.push(format!(
                        "spiral_ohlcv_merge(\"{}\") as \"{}\"",
                        src.mat_column, src.mat_column
                    ));
                } else if src.formula == "range_max_end" || src.formula == "range_merge" {
                    select_parts.push(format!(
                        "max(\"{}\") as \"{}\"",
                        src.mat_column, src.mat_column
                    ));
                } else {
                    select_parts.push(format!(
                        "sum(\"{}\") as \"{}\"",
                        src.mat_column, src.mat_column
                    ));
                }
                sources.push(src);
            }
        }

        let scope_cols_str = scope_columns
            .iter()
            .map(|s| format!("\"{}\"", s.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let index_sql = if scope_columns.is_empty() {
            format!("CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{child_name} ON {child_name}(t)")
        } else {
            format!("CREATE INDEX IF NOT EXISTS idx_z_{child_name} ON {child_name} (spiral_zorder(spiral(t), ARRAY[{scope_cols_str}]::text[]))")
        };

        let sql = format!("CREATE TABLE {child_name} AS SELECT {select_cols} FROM {parent_name} WHERE 1=0 GROUP BY {group_by}; {index_sql};",
            child_name = child_name, select_cols = select_parts.join(", "), parent_name = current_parent, group_by = group_parts.join(", "), index_sql = index_sql);

        let idempotent_sql = sql.replace("CREATE TABLE", "CREATE TABLE IF NOT EXISTS");
        match Spi::run(&idempotent_sql) {
            Ok(_) => {
                catalog::insert_metadata(
                    &child_name,
                    &current_parent,
                    frame.seconds,
                    base_name,
                    scope_columns.clone(),
                    pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
                );
                create_reconstruction_view(&child_name);

                for src in &sources {
                    catalog::insert_source(
                        &child_name,
                        base_name,
                        frame.seconds,
                        &src.base_column,
                        &src.formula,
                        &src.mat_column,
                        src.rollup_gsub_strategy.as_deref(),
                        pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
                    );
                }
                current_parent = child_name;
            }
            Err(e) => error!(
                "Spiral: failed to create rollup tier '{}': {:?}",
                child_name, e
            ),
        }
    }
}

/// Validates that a Spiral magic comment formula is compatible with the column's PostgreSQL type.
/// Raises ERROR on mismatch so the CREATE TABLE fails early with a clear message.
fn validate_formula_column_type(col: &str, formula: &str, type_oid: pg_sys::Oid) {
    const NUMERIC_OIDS: &[pg_sys::Oid] = &[
        pg_sys::INT2OID,
        pg_sys::INT4OID,
        pg_sys::INT8OID,
        pg_sys::FLOAT4OID,
        pg_sys::FLOAT8OID,
        pg_sys::NUMERICOID,
    ];
    let formula_lower = formula.to_lowercase();
    let needs_numeric = matches!(
        formula_lower.as_str(),
        "sum" | "stats" | "ohlcv" | "tdigest" | "sketch" | "quantile"
    );
    let needs_timestamptz = matches!(formula_lower.as_str(), "range_max_end" | "range_merge");

    if needs_numeric && !NUMERIC_OIDS.contains(&type_oid) {
        error!(
            "Spiral: formula '{}' on column '{}' requires a numeric type (int, float, or numeric), got OID {}",
            formula, col, type_oid
        );
    }
    if needs_timestamptz && type_oid != pg_sys::TIMESTAMPTZOID {
        error!(
            "Spiral: formula '{}' on column '{}' requires timestamptz, got OID {}",
            formula, col, type_oid
        );
    }
}

pub(crate) unsafe fn parse_sql_to_query(sql: &str) -> *mut pg_sys::Query {
    let query_string = std::ffi::CString::new(sql).unwrap();
    let raw_parsetree_list = pg_sys::raw_parser(
        query_string.as_ptr(),
        pg_sys::RawParseMode::RAW_PARSE_DEFAULT,
    );
    if raw_parsetree_list.is_null() || (*raw_parsetree_list).length == 0 {
        return std::ptr::null_mut();
    }
    let raw_parse_tree = pg_sys::list_nth(raw_parsetree_list, 0) as *mut pg_sys::RawStmt;
    let query_list = pg_sys::pg_analyze_and_rewrite_fixedparams(
        raw_parse_tree,
        query_string.as_ptr(),
        std::ptr::null_mut(),
        0,
        std::ptr::null_mut(),
    );
    if query_list.is_null() || (*query_list).length == 0 {
        return std::ptr::null_mut();
    }
    pg_sys::list_nth(query_list, 0) as *mut pg_sys::Query
}

/// Neutralize an OpExpr by replacing it with 'true = true'.
/// This allows the planner to discard the redundant filter without needing
/// to re-balance the expression tree or modify parent pointers.
unsafe fn neutralize_op_expr(node: *mut pg_sys::Node) {
    use std::os::raw::c_void;
    if node.is_null() || (*node).type_ != pg_sys::NodeTag::T_OpExpr {
        return;
    }
    let op = node as *mut pg_sys::OpExpr;

    static mut EQ_BOOL_OP: pg_sys::Oid = pg_sys::InvalidOid;
    static mut EQ_BOOL_FUNC: pg_sys::Oid = pg_sys::InvalidOid;

    if EQ_BOOL_OP == pg_sys::InvalidOid {
        Spi::connect(|client| {
            let mut res = client.select(
                "SELECT oid, oprcode FROM pg_operator \
                 WHERE oprname = '=' AND oprleft = 'bool'::regtype \
                 AND oprright = 'bool'::regtype LIMIT 1",
                Some(1),
                &[],
            )?;
            if let Some(row) = res.next() {
                EQ_BOOL_OP = row.get::<pg_sys::Oid>(1).unwrap().unwrap_or(pg_sys::InvalidOid);
                EQ_BOOL_FUNC = row.get::<pg_sys::Oid>(2).unwrap().unwrap_or(pg_sys::InvalidOid);
            }
            Ok::<(), spi::Error>(())
        })
        .unwrap();
    }

    if EQ_BOOL_OP != pg_sys::InvalidOid {
        let true_const = pg_sys::makeConst(
            pg_sys::BOOLOID,
            -1,
            pg_sys::InvalidOid,
            1,
            true.into_datum().unwrap(),
            false,
            true,
        );
        (*op).opno = EQ_BOOL_OP;
        (*op).opfuncid = EQ_BOOL_FUNC;
        let mut args = std::ptr::null_mut();
        args = pg_sys::lappend(args, true_const as *mut c_void);
        args = pg_sys::lappend(args, true_const as *mut c_void);
        (*op).args = args;
    }
}

fn is_supported_rollup_aggregate(agg_fn: &str) -> bool {
    matches!(
        agg_fn.to_lowercase().as_str(),
        "sum"
            | "min"
            | "max"
            | "count"
            | "avg"
            | "first"
            | "last"
            | "spiral_stats"
            | "spiral_tdigest"
            | "spiral_sketch"
            | "spiral_stats_merge"
            | "spiral_tdigest_merge"
            | "spiral_sketch_merge"
    )
}

pub(crate) unsafe fn rewrite_query_aggregates(
    query: *mut pg_sys::Query,
    column_formulas: &std::collections::HashMap<String, String>,
) {
    if !(*query).hasAggs {
        return;
    }
    let target_list = (*query).targetList;
    if target_list.is_null() {
        return;
    }

    // Pre-fetch merge function OIDs to avoid Spi calls during traversal
    let stats_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_stats_merge' AND pronargs = 1").unwrap_or(None);
    let tdigest_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_tdigest_merge' AND pronargs = 1").unwrap_or(None);
    let sketch_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_sketch_merge' AND pronargs = 1").unwrap_or(None);
    let avg_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_avg' AND pronargs = 1").unwrap_or(None);
    let count_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_count' AND pronargs = 1").unwrap_or(None);
    let sum_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_sum' AND pronargs = 1").unwrap_or(None);

    // OHLCV merge aggregates
    let ohlcv_merge_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_ohlcv_merge' AND pronargs = 1").unwrap_or(None);
    let open_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_open' AND pronargs = 1").unwrap_or(None);
    let high_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_high' AND pronargs = 1").unwrap_or(None);
    let low_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_low' AND pronargs = 1").unwrap_or(None);
    let close_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_close' AND pronargs = 1").unwrap_or(None);
    let volume_oid = Spi::get_one::<pg_sys::Oid>("SELECT oid FROM pg_proc WHERE proname = 'spiral_volume' AND pronargs = 1").unwrap_or(None);

    unsafe fn walk_and_rewrite(
        node: *mut pg_sys::Node,
        column_formulas: &std::collections::HashMap<String, String>,
        rtable: *mut pg_sys::List,
        oids: &[Option<pg_sys::Oid>], // [stats, tdigest, sketch, avg, count, sum, ohlcv, open, high, low, close, volume]
    ) {
        if node.is_null() {
            return;
        }

        if (*node).type_ == pg_sys::NodeTag::T_Aggref {
            let agg = node as *mut pg_sys::Aggref;
            let agg_fn_ptr = pg_sys::get_func_name((*agg).aggfnoid);
            if !agg_fn_ptr.is_null() {
                let agg_fn = CStr::from_ptr(agg_fn_ptr).to_string_lossy().to_lowercase();
                
                // Identify the column and its formula
                let mut formula = "sum".to_string();
                let mut is_json_target = false;
                if !(*agg).args.is_null() && (*(*agg).args).length >= 1 {
                    let arg = pg_sys::list_nth((*agg).args, 0) as *mut pg_sys::TargetEntry;
                    let expr = (*arg).expr as *mut pg_sys::Node;
                    if !expr.is_null() && (*expr).type_ == pg_sys::NodeTag::T_Var {
                        let var = expr as *mut pg_sys::Var;
                        if (*var).varno > 0 && (*var).varno as usize <= (*rtable).length as usize {
                            let rte = pg_sys::list_nth(rtable, (*var).varno as i32 - 1) as *mut pg_sys::RangeTblEntry;
                            let varname = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                            if !varname.is_null() {
                                let name = CStr::from_ptr(varname).to_string_lossy();
                                if let Some(f) = column_formulas.get(name.as_ref()) {
                                    formula = f.clone();
                                }
                            }
                        }
                        if (*var).vartype == pg_sys::JSONBOID {
                            is_json_target = true;
                        }
                    }
                }

                let new_oid = if formula == "ohlcv" {
                    match agg_fn.as_ref() {
                        "first" => oids[7], // open
                        "last" => oids[10], // close
                        "max" => oids[8],  // high
                        "min" => oids[9],  // low
                        "sum" => oids[11], // volume
                        "spiral_ohlcv" | "spiral_ohlcv_merge" => oids[6], // ohlcv_merge
                        _ => None,
                    }
                } else if formula == "stats" {
                    match agg_fn.as_ref() {
                        "avg" => oids[3],
                        "count" => oids[4],
                        "sum" => oids[5],
                        "spiral_stats" | "spiral_stats_merge" => oids[0],
                        _ => None,
                    }
                } else if formula == "tdigest" || formula == "sketch" {
                    let is_td = formula == "tdigest";
                    match agg_fn.as_ref() {
                        "spiral_tdigest" | "spiral_tdigest_merge" if is_td => oids[1],
                        "spiral_sketch" | "spiral_sketch_merge" if !is_td => oids[2],
                        _ => None,
                    }
                } else {
                    match agg_fn.as_ref() {
                        "avg" if is_json_target => oids[3],
                        "count" if is_json_target => oids[4],
                        "sum" if is_json_target => oids[5],
                        _ => None,
                    }
                };

                if let Some(oid) = new_oid {
                    (*agg).aggfnoid = oid;

                    // If we changed to a state-based aggregate, we must update the argument Var
                    // to expect JSONB from the subquery.
                    if !(*agg).args.is_null() && (*(*agg).args).length >= 1 {
                        let arg = pg_sys::list_nth((*agg).args, 0) as *mut pg_sys::TargetEntry;
                        let expr = (*arg).expr as *mut pg_sys::Node;
                        if !expr.is_null() && (*expr).type_ == pg_sys::NodeTag::T_Var {
                            let var = expr as *mut pg_sys::Var;
                            (*var).vartype = pg_sys::JSONBOID;
                            (*var).vartypmod = -1;
                        }
                        
                        // If it was first/last, it had 2 arguments. We must truncate to 1 (the jsonb state).
                        if agg_fn == "first" || agg_fn == "last" {
                            (*agg).args = pg_sys::lappend(std::ptr::null_mut(), arg as *mut _);
                        }
                    }
                }
            }
        } else if (*node).type_ == pg_sys::NodeTag::T_FuncExpr {
            let func = node as *mut pg_sys::FuncExpr;
            let args = (*func).args;
            if !args.is_null() {
                for i in 0..(*args).length {
                    walk_and_rewrite(
                        pg_sys::list_nth(args, i) as *mut pg_sys::Node,
                        column_formulas,
                        rtable,
                        oids,
                    );
                }
            }
        }
    }

    let oids = [
        stats_merge_oid, tdigest_merge_oid, sketch_merge_oid, 
        avg_merge_oid, count_merge_oid, sum_merge_oid,
        ohlcv_merge_oid, open_oid, high_oid, low_oid, close_oid, volume_oid
    ];

    for i in 0..(*target_list).length {
        let tle = pg_sys::list_nth(target_list, i) as *mut pg_sys::TargetEntry;
        walk_and_rewrite(
            (*tle).expr as *mut pg_sys::Node,
            column_formulas,
            (*query).rtable,
            &oids,
        );
    }
}

pub(crate) unsafe fn extract_supported_query_columns(
    query: *mut pg_sys::Query,
    rtable: *mut pg_sys::List,
    base_table: &str,
) -> Option<Vec<(String, Option<String>)>> {
    if !(*query).hasAggs {
        notice!("Spiral: query has no aggregates");
        return None;
    }

    if !(*query).havingQual.is_null()
        || !(*query).distinctClause.is_null()
        || !(*query).windowClause.is_null()
    {
        notice!("Spiral: query has HAVING, DISTINCT, or WINDOW clause");
        return None;
    }

    let target_list = (*query).targetList;
    if target_list.is_null() {
        notice!("Spiral: query target list is null");
        return None;
    }

    unsafe fn walk_expr(
        node: *mut pg_sys::Node,
        base_table: &str,
        rtable: *mut pg_sys::List,
        cols: &mut Vec<(String, Option<String>)>,
    ) -> bool {
        if node.is_null() {
            return true;
        }

        match (*node).type_ {
            pg_sys::NodeTag::T_Var => {
                let var = node as *mut pg_sys::Var;
                let rte = pg_sys::list_nth(rtable, (*var).varno - 1) as *mut pg_sys::RangeTblEntry;
                if rte.is_null() || (*rte).relid == pg_sys::InvalidOid {
                    if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_GROUP {
                        return true;
                    }
                    return false;
                }

                let relname_ptr = pg_sys::get_rel_name((*rte).relid);
                if relname_ptr.is_null() {
                    notice!("Spiral: walk_expr relname is null");
                    return false;
                }
                let relname = CStr::from_ptr(relname_ptr).to_string_lossy();
                if relname.as_ref() != base_table {
                    notice!("Spiral: walk_expr relname mismatch: '{}' != '{}'", relname, base_table);
                    return false;
                }

                let varname = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                if varname.is_null() {
                    return false;
                }

                cols.push((CStr::from_ptr(varname).to_string_lossy().into_owned(), None));
                true
            }
            pg_sys::NodeTag::T_Aggref => {
                let agg = node as *mut pg_sys::Aggref;
                if !(*agg).aggdistinct.is_null()
                    || !(*agg).aggorder.is_null()
                    || !(*agg).aggfilter.is_null()
                    || (*agg).aggstar
                {
                    notice!("Spiral: walk_expr found DISTINCT, ORDER BY, FILTER, or * in aggregate");
                    return false;
                }

                let agg_fn = pg_sys::get_func_name((*agg).aggfnoid);
                if agg_fn.is_null() {
                    return false;
                }
                let fn_name = CStr::from_ptr(agg_fn).to_string_lossy().into_owned();
                if !is_supported_rollup_aggregate(&fn_name) {
                    notice!("Spiral: walk_expr found unsupported aggregate function '{}'", fn_name);
                    return false;
                }

                let args = (*agg).args;
                let num_args = if args.is_null() { 0 } else { (*args).length };
                notice!("Spiral: walk_expr aggregate '{}' has {} args", fn_name, num_args);
                
                let is_order_dependent = fn_name.to_lowercase() == "first" || fn_name.to_lowercase() == "last";
                if is_order_dependent {
                    if num_args != 2 {
                        notice!("Spiral: walk_expr found order-dependent aggregate '{}' with {} arguments (expected 2)", fn_name, num_args);
                        return false;
                    }
                } else if num_args != 1 {
                    notice!("Spiral: walk_expr found aggregate '{}' with {} arguments (expected 1)", fn_name, num_args);
                    return false;
                }

                let arg = pg_sys::list_nth(args, 0) as *mut pg_sys::TargetEntry;
                let arg_expr = (*arg).expr as *mut pg_sys::Node;
                if arg_expr.is_null() || (*arg_expr).type_ != pg_sys::NodeTag::T_Var {
                    return false;
                }

                let var = arg_expr as *mut pg_sys::Var;
                let rte = pg_sys::list_nth(rtable, (*var).varno - 1) as *mut pg_sys::RangeTblEntry;
                if rte.is_null() || (*rte).relid == pg_sys::InvalidOid {
                    return false;
                }

                let relname_ptr = pg_sys::get_rel_name((*rte).relid);
                if relname_ptr.is_null() {
                    notice!("Spiral: walk_expr relname is null");
                    return false;
                }
                let relname = CStr::from_ptr(relname_ptr).to_string_lossy();
                if relname.as_ref() != base_table {
                    notice!("Spiral: walk_expr relname mismatch: '{}' != '{}'", relname, base_table);
                    return false;
                }

                let varname = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                if varname.is_null() {
                    return false;
                }

                let col_name = CStr::from_ptr(varname).to_string_lossy().into_owned();
                cols.push((col_name, Some(fn_name)));
                true
            }
            pg_sys::NodeTag::T_FuncExpr => {
                let func = node as *mut pg_sys::FuncExpr;
                if (*func).args.is_null() {
                    return true;
                }
                for i in 0..(*(*func).args).length {
                    if !walk_expr(
                        pg_sys::list_nth((*func).args, i) as *mut pg_sys::Node,
                        base_table,
                        rtable,
                        cols,
                    ) {
                        return false;
                    }
                }
                true
            }
            pg_sys::NodeTag::T_Const => true,
            _ => false,
        }
    }

    let mut cols = Vec::new();
    for i in 0..(*target_list).length {
        let tle = pg_sys::list_nth(target_list, i) as *mut pg_sys::TargetEntry;
        if (*tle).resjunk {
            continue;
        }

        if !walk_expr(
            (*tle).expr as *mut pg_sys::Node,
            base_table,
            rtable,
            &mut cols,
        ) {
            return None;
        }
    }

    if cols.is_empty() {
        return None;
    }
    Some(cols)
}

/// Returns the granularity in seconds implied by a `date_trunc` GROUP BY expression.
/// Inspects the query's groupClause and target list for `date_trunc('field', t)`.
/// In PG17+, GROUP BY targets are Vars pointing to an RTE_GROUP entry; this
/// function dereferences through RTE_GROUP.groupexprs to find the actual FuncExpr.
/// Returns `None` if no date_trunc is found or the field is unrecognised.
pub(crate) unsafe fn extract_group_granularity_secs(query: *mut pg_sys::Query) -> Option<i64> {
    let group_clause = (*query).groupClause;
    if group_clause.is_null() {
        return None;
    }
    let target_list = (*query).targetList;
    if target_list.is_null() {
        return None;
    }
    let rtable = (*query).rtable;

    for i in 0..(*group_clause).length {
        let sgc = pg_sys::list_nth(group_clause, i) as *mut pg_sys::SortGroupClause;
        if sgc.is_null() {
            continue;
        }
        let ref_id = (*sgc).tleSortGroupRef;

        // Find the TargetEntry whose ressortgroupref matches
        let mut tle_expr: *mut pg_sys::Node = std::ptr::null_mut();
        for j in 0..(*target_list).length {
            let tle = pg_sys::list_nth(target_list, j) as *mut pg_sys::TargetEntry;
            if !tle.is_null() && (*tle).ressortgroupref == ref_id {
                tle_expr = (*tle).expr as *mut pg_sys::Node;
                break;
            }
        }
        if tle_expr.is_null() {
            continue;
        }

        // PG17+: GROUP BY targets are Vars pointing to RTE_GROUP.
        // Dereference through groupexprs to get the actual expression.
        let resolved_expr: *mut pg_sys::Node = if (*tle_expr).type_ == pg_sys::NodeTag::T_Var
            && !rtable.is_null()
        {
            let var = tle_expr as *mut pg_sys::Var;
            let varno = (*var).varno;
            let varattno = (*var).varattno as usize;
            if varno >= 1 && varno <= (*rtable).length {
                let rte = pg_sys::list_nth(rtable, varno - 1) as *mut pg_sys::RangeTblEntry;
                if !rte.is_null()
                    && (*rte).rtekind == pg_sys::RTEKind::RTE_GROUP
                    && !(*rte).groupexprs.is_null()
                    && varattno >= 1
                    && varattno <= (*(*rte).groupexprs).length as usize
                {
                    pg_sys::list_nth((*rte).groupexprs, (varattno - 1) as i32) as *mut pg_sys::Node
                } else {
                    tle_expr
                }
            } else {
                tle_expr
            }
        } else {
            tle_expr
        };

        if resolved_expr.is_null() || (*resolved_expr).type_ != pg_sys::NodeTag::T_FuncExpr {
            continue;
        }
        let fe = resolved_expr as *mut pg_sys::FuncExpr;
        let fn_name_ptr = pg_sys::get_func_name((*fe).funcid);
        if fn_name_ptr.is_null() {
            continue;
        }
        let fn_name = CStr::from_ptr(fn_name_ptr).to_string_lossy();
        if fn_name != "date_trunc" {
            continue;
        }
        // First arg of date_trunc is the field string constant
        let args = (*fe).args;
        if args.is_null() || (*args).length < 1 {
            continue;
        }
        let first_arg = pg_sys::list_nth(args, 0) as *mut pg_sys::Node;
        if first_arg.is_null() || (*first_arg).type_ != pg_sys::NodeTag::T_Const {
            continue;
        }
        let cst = first_arg as *mut pg_sys::Const;
        let field_text: Option<String> = String::from_datum((*cst).constvalue, (*cst).constisnull);

        let secs = match field_text.as_deref() {
            Some("microseconds") | Some("milliseconds") | Some("second") | Some("seconds") => 1,
            Some("minute") | Some("minutes") => 60,
            Some("hour") | Some("hours") => 3600,
            Some("day") | Some("days") => 86400,
            Some("week") | Some("weeks") => 604800,
            Some("month") | Some("months") => 2_592_000,
            Some("quarter") | Some("quarters") => 7_776_000,
            Some("year") | Some("years") | Some("decade") | Some("century")
            | Some("millennium") => i64::MAX / 2,
            _ => continue,
        };
        return Some(secs);
    }
    None
}

/// Maps an aggregate function + stored formula to the correct SQL expression
/// for either a rollup tier or the base table.
///
/// For rollup tiers the formula determines which sub-column to expose:
/// - `ohlcv` + MAX → `col_h` (high), MIN → `col_l` (low), SUM → `col_v` (volume)
/// - `sum`   + MAX/MIN → bare `col` (re-max/re-min of pre-agg maxes/mins is correct)
/// - `stats` + COUNT → `(col->>'n')::bigint`
/// - anything else → bare `col` (outer query re-applies the aggregate)
///
/// # Examples
/// ```rust
/// use spiral::hooks::map_agg_inner;
///
/// assert_eq!(map_agg_inner("SUM", "col_a",     false, "sum"),   "\"col_a\"");
/// assert_eq!(map_agg_inner("SUM", "col_a",     true,  "sum"),   "\"col_a\"");
/// assert_eq!(map_agg_inner("MAX", "temp_ohlcv", true, "ohlcv"), "\"temp_ohlcv_h\"");
/// assert_eq!(map_agg_inner("MIN", "temp_ohlcv", true, "ohlcv"), "\"temp_ohlcv_l\"");
/// assert_eq!(map_agg_inner("SUM", "temp_ohlcv", true, "ohlcv"), "\"temp_ohlcv_v\"");
/// ```
pub fn map_agg_inner(agg_fn: &str, mapped_col: &str, is_rollup: bool, formula: &str) -> String {
    let agg_lower = agg_fn.to_lowercase();
    let is_json_agg = matches!(
        agg_lower.as_str(),
        "spiral_stats" | "spiral_tdigest" | "spiral_sketch" | "spiral_ohlcv"
    );

    // Common aggregates that can be fulfilled by state formulas
    let is_state_derived = matches!(
        agg_lower.as_str(),
        "sum" | "count" | "avg" | "first" | "last" | "min" | "max"
    ) || is_json_agg;

    if !is_rollup {
        if is_state_derived {
            let accum_fn = match formula {
                "stats" => "spiral_stats_accum",
                "ohlcv" => "spiral_ohlcv_accum",
                "tdigest" => "spiral_tdigest_accum",
                "sketch" => "spiral_sketch_accum",
                _ => {
                    if is_json_agg {
                        &format!("{}_accum", agg_lower)
                    } else {
                        return format!("\"{}\"", mapped_col);
                    }
                }
            };

            if formula == "ohlcv" {
                return format!("{}(NULL, \"{}\", spiral(t))", accum_fn, mapped_col);
            } else {
                return format!("{}(NULL, \"{}\")", accum_fn, mapped_col);
            }
        }
        return format!("\"{}\"", mapped_col);
    }

    format!("\"{}\"", mapped_col)
}

fn format_epoch(epoch: i64) -> String {
    Spi::get_one::<String>(&format!(
        "SELECT to_char(to_timestamp({}::double precision), 'YYYY-MM-DD HH24:MI:SS')",
        epoch
    ))
    .unwrap_or_default()
    .unwrap_or_else(|| epoch.to_string())
}

fn reconstruction_expr(col: &str, formula: &str, is_rollup: bool) -> String {
    if !is_rollup {
        // Base table: original timestamptz, pass through
        return format!("\"{}\"", col);
    }
    match formula {
        "range_max_end" | "range_merge" => format!(
            "t + make_interval(secs => \"{}\"::double precision) AS \"{}\"",
            col, col
        ),
        _ => format!("\"{}\"", col),
    }
}

/// Ordered (column, value-as-string) pairs for the current tenant scope filter.
/// Used to inject z-order BETWEEN predicates into rollup-tier sub-SELECTs so the
/// planner can use the `idx_z_*` index without the caller needing to know about
/// z-order internals.
fn construct_union_sql_hierarchical(
    base_table: &str,
    segments: &[Segment],
    cols: &[(String, Option<String>)],
    offset_cols: &[catalog::OffsetColumn],
    col_types: &std::collections::HashMap<String, String>,
    scope_vals: &[(String, String)],
) -> String {
    let mut sources: Vec<String> = segments.iter().map(|s| s.source.clone()).collect();
    sources.sort();
    sources.dedup();

    let mut sources_with_seconds: Vec<(String, i32)> = sources
        .into_iter()
        .map(|s| {
            let secs = if s == base_table {
                0
            } else {
                catalog::get_metadata(&s)
                    .map(|m| m.frame_seconds)
                    .unwrap_or(0)
            };
            (s, secs)
        })
        .collect();
    sources_with_seconds.sort_by_key(|s| -s.1);

    let mut select_parts = Vec::new();

    for (src, _secs) in sources_with_seconds {
        let is_rollup = src != base_table;

        // For rollup tiers, fetch columns that actually exist to NULL-fill missing ones.
        // This preserves column positions (Var.varattno alignment) in the subquery.
        let rollup_cols: std::collections::HashSet<String> = if is_rollup {
            Spi::connect(|client| {
                Ok::<std::collections::HashSet<String>, spi::Error>(
                    client
                        .select(
                            &format!(
                                "SELECT attname::text FROM pg_attribute \
                             WHERE attrelid = '\"{}\"'::regclass \
                             AND attnum > 0 AND NOT attisdropped",
                                src.replace("\"", "\"\"")
                            ),
                            None,
                            &[],
                        )?
                        .map(|r| r.get::<String>(1).unwrap().unwrap_or_default())
                        .collect(),
                )
            })
            .unwrap_or_default()
        } else {
            std::collections::HashSet::new()
        };

        let mut inner_select = Vec::new();
        // t is always first; cast to timestamptz to match original table type
        inner_select.push("t::timestamptz".to_string());

        for (col, agg) in cols {
            let orig_type = col_types.get(col).map(|s| s.as_str()).unwrap_or("");

            if let Some(agg_fn) = agg {
                let (formula_for_col, mapped) = if is_rollup {
                    Spi::connect(|client| -> Result<(String, String), spi::Error> {
                        let formula_filter = match agg_fn.to_lowercase().as_str() {
                            "spiral_stats" | "spiral_stats_merge" | "avg" => "stats",
                            "spiral_tdigest" | "spiral_tdigest_merge" => "tdigest",
                            "spiral_sketch" | "spiral_sketch_merge" => "sketch",
                            "first" | "last" => "ohlcv",
                            "sum" | "min" | "max" => "sum", // sum/min/max could be in ohlcv too, handled by fallback
                            "count" => "stats",
                            _ => "",
                        };

                        let mut sql = if formula_filter.is_empty() {
                            format!(
                                "SELECT formula, mat_column FROM spiral.sources \
                                 WHERE view_name = '{}' AND base_column = '{}' \
                                 LIMIT 1",
                                src.replace("'", "''"),
                                col.replace("'", "''"),
                            )
                        } else {
                            format!(
                                "SELECT formula, mat_column FROM spiral.sources \
                                 WHERE view_name = '{}' AND base_column = '{}' \
                                 AND (formula = '{}' OR formula = 'ohlcv') \
                                 LIMIT 1",
                                src.replace("'", "''"),
                                col.replace("'", "''"),
                                formula_filter
                            )
                        };

                        let mut rows = client.select(&sql, Some(1), &[])?;
                        if rows.is_empty() && !formula_filter.is_empty() {
                            // Fallback: try without formula filter if the specific one didn't match
                            sql = format!(
                                "SELECT formula, mat_column FROM spiral.sources \
                                 WHERE view_name = '{}' AND base_column = '{}' \
                                 LIMIT 1",
                                src.replace("'", "''"),
                                col.replace("'", "''"),
                            );
                            rows = client.select(&sql, Some(1), &[])?;
                        }

                        if let Some(row) = rows.next() {
                            let formula = row.get::<String>(1)?.unwrap_or_default();
                            let mat_col = row.get::<String>(2)?.unwrap_or_default();
                            return Ok((formula, mat_col));
                        }
                        Ok((String::new(), col.clone()))
                    })
                    .unwrap_or_else(|_| (String::new(), col.clone()))
                } else {
                    (String::new(), col.clone())
                };

                // NULL-fill only when: rollup tier AND no formula mapping found AND base
                // column not directly present. If a formula mapping exists, the column is
                // accessible via the mapped sub-column expression.
                if is_rollup && formula_for_col.is_empty() && !rollup_cols.contains(col.as_str()) {
                    let null_expr = if orig_type.is_empty() {
                        format!("NULL AS \"{}\"", col)
                    } else {
                        format!("NULL::{} AS \"{}\"", orig_type, col)
                    };
                    inner_select.push(null_expr);
                    continue;
                }

                let col_expr = if let Some(oc) = offset_cols.iter().find(|o| o.mat_column == *col) {
                    reconstruction_expr(&mapped, &oc.formula, is_rollup)
                } else {
                    map_agg_inner(agg_fn, &mapped, is_rollup, &formula_for_col)
                };

                let is_json_agg = matches!(
                    agg_fn.to_lowercase().as_str(),
                    "spiral_stats"
                        | "spiral_tdigest"
                        | "spiral_sketch"
                        | "spiral_ohlcv"
                        | "sum"
                        | "count"
                        | "avg"
                );

                // Cast to original type so outer Var nodes resolve without mismatch.
                // SKIP cast for JSON aggregates where the rollup column is JSONB but the
                // base column was numeric (e.g. spiral_stats).
                let col_sql = if is_rollup && !orig_type.is_empty() && !is_json_agg {
                    format!("({})::{}  AS \"{}\"", col_expr, orig_type, col)
                } else {
                    format!("{} AS \"{}\"", col_expr, col)
                };
                inner_select.push(col_sql);
            } else {
                if col == "t" {
                    continue;
                }
                let col_sql = if is_rollup && !rollup_cols.contains(col.as_str()) {
                    // NULL-fill with typed cast
                    if orig_type.is_empty() {
                        format!("NULL AS \"{}\"", col)
                    } else {
                        format!("NULL::{} AS \"{}\"", orig_type, col)
                    }
                } else if let Some(oc) = offset_cols.iter().find(|o| o.mat_column == *col) {
                    reconstruction_expr(col, &oc.formula, is_rollup)
                } else if is_rollup && !orig_type.is_empty() {
                    format!("\"{}\"::{} AS \"{}\"", col, orig_type, col)
                } else {
                    format!("\"{}\"", col)
                };
                inner_select.push(col_sql);
            }
        }

        let src_segs: Vec<&Segment> = segments.iter().filter(|s| s.source == src).collect();
        if src_segs.is_empty() {
            continue;
        }

        let time_pred = if src_segs.len() == 1 {
            let seg = src_segs[0];
            let ts_start = format_epoch(seg._t_start);
            let ts_end = format_epoch(seg._t_end);
            format!(
                "t >= '{}'::timestamptz AND t < '{}'::timestamptz",
                ts_start, ts_end
            )
        } else {
            let mut range_strs = Vec::new();
            for seg in src_segs.iter() {
                let ts_start = format_epoch(seg._t_start);
                let ts_end = format_epoch(seg._t_end);
                range_strs.push(format!("[\"{}\", \"{}\")", ts_start, ts_end));
            }
            format!("t <@ '{{ {} }}'::tstzmultirange", range_strs.join(", "))
        };

        // For rollup tiers with a tenant scope filter, inject a z-order BETWEEN predicate
        // so PostgreSQL can use the automatically-created idx_z_* index.
        // Z-order is monotone in t for a fixed tenant hash, so BETWEEN(lo, hi) exactly
        // covers the queried time range for this tenant with no false negatives.
        let zorder_pred = if is_rollup && !scope_vals.is_empty() {
            let min_t = src_segs.iter().map(|s| s._t_start).min().unwrap_or(0);
            let max_t = src_segs.iter().map(|s| s._t_end).max().unwrap_or(0);
            let dim_strings: Vec<Option<String>> =
                scope_vals.iter().map(|(_, v)| Some(v.clone())).collect();
            let lo = crate::spiral_zorder(min_t, dim_strings.clone());
            let hi = crate::spiral_zorder(max_t, dim_strings);
            let array_lit = scope_vals
                .iter()
                .map(|(_, v)| format!("'{}'", v.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                " AND spiral_zorder(spiral(t), ARRAY[{}]::text[]) BETWEEN {} AND {}",
                array_lit, lo, hi
            )
        } else {
            String::new()
        };

        select_parts.push(format!(
            "SELECT {} FROM {} WHERE {}{}",
            inner_select.join(", "),
            src,
            time_pred,
            zorder_pred
        ));
    }

    if select_parts.is_empty() {
        return String::new();
    }

    // Plain UNION ALL — no CTE, no ORDER BY. CTEs cause One-Time Filter: false
    // when used as subquery RTEs due to Var node type resolution issues.
    select_parts.join(" UNION ALL ")
}

/// Parse a simple `col = val` predicate from a WHERE clause string.
/// Only handles integer equality on a single column — the primary use case
/// for scope-scoped refresh. Returns `None` for anything more complex.
fn parse_scope_predicate(where_clause: &str) -> Option<(String, i64)> {
    let trimmed = where_clause.trim();
    // Pattern: optional whitespace, identifier, optional whitespace, '=', optional whitespace, integer
    let re_parts: Vec<&str> = trimmed.splitn(2, '=').collect();
    if re_parts.len() != 2 {
        return None;
    }
    let col = re_parts[0].trim();
    let val_str = re_parts[1].trim();
    // col must be a plain identifier (alphanumeric + underscore)
    if !col.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let val: i64 = val_str.parse().ok()?;
    Some((col.to_string(), val))
}

/// Build a JSONB scope filter for the changelog given a scope col + integer value,
/// only if `col` is actually a registered scope column for this base view.
fn build_changelog_scope_filter(
    _real_base: &str,
    col: &str,
    val: i64,
    scope_columns: &[String],
) -> Option<String> {
    if !scope_columns.iter().any(|c| c == col) {
        return None;
    }
    let json_filter = format!("{{\"{}\": {}}}", col.replace('"', ""), val);
    Some(format!(
        "AND scope_values @> '{}'::jsonb",
        json_filter.replace("'", "''")
    ))
}

pub fn reactive_refresh(base_name: &str, where_clause: Option<String>) -> bool {
    notice!(
        "Spiral: reactive_refresh entered for '{}', where_clause={:?}",
        base_name,
        where_clause
    );
    let metadata = catalog::get_metadata(base_name);
    let is_root = metadata
        .as_ref()
        .map(|m| m.parent_view == m.base_view || m.parent_view == "BASE")
        .unwrap_or(false);
    let real_base = metadata
        .as_ref()
        .map(|m| m.base_view.clone())
        .unwrap_or_else(|| base_name.to_string());

    // Derive scope filter from where_clause if it matches a simple `col = val` predicate
    // and `col` is a registered scope column for this base view.
    let scope_columns = metadata
        .as_ref()
        .map(|m| m.scope_columns.clone())
        .unwrap_or_default();
    let scope_filter: Option<String> = where_clause.as_deref().and_then(|wc| {
        parse_scope_predicate(wc).and_then(|(col, val)| {
            build_changelog_scope_filter(&real_base, &col, val, &scope_columns)
        })
    });

    if is_root {
        if scope_filter.is_some() {
            // Partial refresh: skip bootstrap and compaction — only process changelog
            // entries that match the requested tenant scope.
        } else {
            // Bootstrap: if changelog is empty, only force a full rebuild when the
            // rollup tier is also empty (first-time init).  An empty changelog with
            // an already-populated rollup means nothing is dirty — return early so
            // we don't re-aggregate the entire history unnecessarily.
            let count: i64 = Spi::get_one(&format!(
                "SELECT count(*) FROM spiral.changelog WHERE base_view = '{}'",
                real_base.replace("'", "''")
            ))
            .unwrap()
            .unwrap();
            if count == 0 {
                let first_tier: Option<String> = Spi::get_one(&format!(
                    "SELECT view_name FROM spiral.metadata \
                     WHERE base_view = '{}' AND frame_seconds > 0 \
                     ORDER BY frame_seconds LIMIT 1",
                    real_base.replace("'", "''")
                ))
                .unwrap_or(None);

                let rollup_is_empty = first_tier
                    .as_deref()
                    .map(|tier| {
                        Spi::get_one::<i64>(&format!(
                            "SELECT count(*) FROM (SELECT 1 FROM \"{}\" LIMIT 1) t",
                            tier.replace("\"", "\"\"")
                        ))
                        .unwrap_or(Some(0))
                        .unwrap_or(0)
                            == 0
                    })
                    .unwrap_or(true);

                if rollup_is_empty {
                    let bootstrap_sql = format!("INSERT INTO spiral.changelog (base_view, t_start, t_end) VALUES ('{}', 0, 2147483647)", real_base.replace("'", "''"));
                    let _ = Spi::run(&bootstrap_sql);
                } else {
                    return true;
                }
            }
            catalog::unify_changelog(&real_base);
        }

        // Capture IDs to be refreshed (optionally scoped to one tenant).
        let extra_filter = scope_filter.as_deref().unwrap_or("");
        let _ = Spi::run(&format!(
            "CREATE TEMP TABLE refreshing_changelog AS \
             SELECT ctid as old_ctid FROM spiral.changelog \
             WHERE base_view = '{}' {}",
            real_base.replace("'", "''"),
            extra_filter
        ));
    }

    let success = if is_root && scope_filter.is_none() {
        // Full refresh: iterate each distinct scope for per-scope MERGE queries.
        // Each MERGE is scoped to one scope_values bucket — no cross-scope fan-out in the JOIN.
        let distinct_scopes: Vec<String> = Spi::connect(|client| {
            Ok::<Vec<String>, spi::Error>(
                client
                    .select(
                        &format!(
                            "SELECT DISTINCT scope_values::text FROM spiral.changelog \
                             WHERE base_view = '{}'",
                            real_base.replace('\'', "''")
                        ),
                        None,
                        &[],
                    )?
                    .map(|r| {
                        r.get::<String>(1)
                            .unwrap()
                            .unwrap_or_else(|| "{}".to_string())
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();

        if distinct_scopes.is_empty() {
            crate::refresh_incremental(base_name, None, 0, None)
        } else {
            let mut any_ok = false;
            for scope in distinct_scopes {
                let ok = crate::refresh_incremental(base_name, None, 0, Some(scope));
                any_ok = any_ok || ok;
            }
            any_ok
        }
    } else {
        // Partial refresh (where_clause provided) or non-root tier: legacy path.
        crate::refresh_incremental(base_name, where_clause.clone(), 0, None)
    };

    if success && is_root {
        let _ = Spi::run("DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM refreshing_changelog)");
    }

    if is_root {
        let _ = Spi::run("DROP TABLE IF EXISTS refreshing_changelog");
    }

    success
}

/// Refresh a single scope_values bucket for a base view.
/// Exposed via `spiral_refresh_scope(view, scope_json)` so parallel callers
/// (e.g. via pg_background) can dispatch N concurrent scope refreshes.
pub fn reactive_refresh_by_scope(base_name: &str, scope_json: String) {
    let metadata = catalog::get_metadata(base_name);
    let is_root = metadata
        .as_ref()
        .map(|m| m.parent_view == m.base_view || m.parent_view == "BASE")
        .unwrap_or(false);
    let real_base = metadata
        .as_ref()
        .map(|m| m.base_view.clone())
        .unwrap_or_else(|| base_name.to_string());

    if !is_root {
        let _ = crate::refresh_incremental(base_name, None, 0, Some(scope_json));
        return;
    }

    let safe_base = real_base.replace('\'', "''");
    let safe_sj = scope_json.replace('\'', "''");

    let _ = Spi::run("DROP TABLE IF EXISTS refreshing_changelog");
    let _ = Spi::run(&format!(
        "CREATE TEMP TABLE refreshing_changelog AS \
         SELECT ctid as old_ctid FROM spiral.changelog \
         WHERE base_view = '{}' AND scope_values = '{}'::jsonb",
        safe_base, safe_sj
    ));

    let success = crate::refresh_incremental(base_name, None, 0, Some(scope_json));

    if success {
        let _ = Spi::run(
            "DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM refreshing_changelog)",
        );
    }

    let _ = Spi::run("DROP TABLE IF EXISTS refreshing_changelog");
}

#[pg_extern]
pub fn spiral_explain(query_sql: &str) -> String {
    unsafe {
        let query_string = std::ffi::CString::new(query_sql).unwrap();
        let raw_parsetree_list = pg_sys::raw_parser(
            query_string.as_ptr(),
            pg_sys::RawParseMode::RAW_PARSE_DEFAULT,
        );
        if raw_parsetree_list.is_null() || (*raw_parsetree_list).length == 0 {
            return "Error: Could not parse query.".to_string();
        }
        let raw_parse_tree = pg_sys::list_nth(raw_parsetree_list, 0) as *mut pg_sys::RawStmt;
        let query_list = pg_sys::pg_analyze_and_rewrite_fixedparams(
            raw_parse_tree,
            query_string.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
        );
        if query_list.is_null() || (*query_list).length == 0 {
            return "Error: Could not analyze query.".to_string();
        }
        let query = pg_sys::list_nth(query_list, 0) as *mut pg_sys::Query;
        let rtable = (*query).rtable;

        let mut report = String::new();
        let (constraint_map, tz_offset) =
            build_time_constraints((*query).jointree as *mut pg_sys::Node, rtable);

        for i in 0..(*rtable).length {
            let varno = i + 1;
            let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
            if (*rte).rtekind != pg_sys::RTEKind::RTE_RELATION {
                continue;
            }

            let relname = pg_sys::get_rel_name((*rte).relid);
            if relname.is_null() {
                continue;
            }
            let base_table = CStr::from_ptr(relname).to_string_lossy().into_owned();

            let hierarchy = catalog::get_hierarchy(&base_table);

            if hierarchy.is_empty() {
                report.push_str(&format!(
                    "Table '{}': No Spiral hierarchy found.\n",
                    base_table
                ));
                continue;
            }

            if let Some(range) = constraint_map.get(&varno) {
                if let (Some(ts), Some(te)) = (range.start, range.end) {
                    let dirty_ranges = catalog::get_dirty_ranges(&base_table, ts, te, None);

                    let segments = resolve_segments(
                        &base_table,
                        ts,
                        te,
                        &hierarchy,
                        &dirty_ranges,
                        tz_offset,
                        None,
                    );

                    report.push_str(&format!(
                        "--- Spiral Slicing Plan for '{}' ---\n",
                        base_table
                    ));
                    report.push_str(&format!(
                        "Range: {} to {} (Offset: {}s)\n",
                        format_epoch(ts),
                        format_epoch(te),
                        tz_offset
                    ));
                    for (seg_idx, seg) in segments.iter().enumerate() {
                        let tier = if seg.source == base_table {
                            "RAW"
                        } else {
                            "ROLLUP"
                        };
                        report.push_str(&format!(
                            "  Segment #{}: {} -> {} | Source: {} ({})\n",
                            seg_idx + 1,
                            format_epoch(seg._t_start),
                            format_epoch(seg._t_end),
                            seg.source,
                            tier
                        ));
                    }
                } else {
                    report.push_str(&format!(
                        "Table '{}': Time column 't' detected but range is not static/bounded.\n",
                        base_table
                    ));
                }
            } else {
                report.push_str(&format!(
                    "Table '{}': No time constraints detected on 't'.\n",
                    base_table
                ));
            }
            report.push('\n');
        }
        report
    }
}

/// # Safety
/// This function is unsafe because it modifies global PostgreSQL hook pointers.
pub unsafe fn init_hooks() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(spiral_process_utility_hook);
    PREV_PLANNER_HOOK = pg_sys::planner_hook;
    pg_sys::planner_hook = Some(spiral_planner_hook);
}
