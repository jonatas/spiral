use crate::{catalog, rollup};
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;
static mut PREV_PLANNER_HOOK: pg_sys::planner_hook_type = None;

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static IN_UTILITY: Cell<bool> = const { Cell::new(false) };
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
                IN_UTILITY.with(|h| h.set(false));
                return;
            }

            let query_str = CStr::from_ptr(query_string).to_string_lossy();
            let mut extracted_frames = String::new();
            let mut extracted_tenant = String::new();

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

            if !extracted_frames.is_empty() {
                notice!("Spiral: WITH parameters found, calling standard_ProcessUtility...");
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

                // 2. Parse frames
                let frames_str = extracted_frames.clone();

                // 3. Detect Scope (Tenant) columns via Foreign Keys or extracted tenant
                let scope_columns = if !extracted_tenant.is_empty() {
                    extracted_tenant.split(',').map(|s| s.trim().to_string()).collect()
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

                let re_col =
                    regex::Regex::new(r"(\w+)\s+[\w\(\) ]+,?\s*--\s*Spiral:\s*([^\n\r]+)").unwrap();
                let mut captured_cols = Vec::new();
                for cap in re_col.captures_iter(&query_str) {
                    captured_cols.push((cap[1].to_string(), cap[2].to_string()));
                }

                // 4. Register the BASE table metadata
                catalog::insert_metadata(
                    &name,
                    "BASE",
                    0,
                    &name,
                    scope_columns.clone(),
                    pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
                );

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
                         FOR EACH STATEMENT EXECUTE FUNCTION spiral.track_changes_stmt('{base_view}')",
                        base_view = name,
                        event = event,
                        event_lower = event.to_lowercase(),
                        transition = transition
                    );
                    if let Err(e) = Spi::run(&trigger_sql) {
                        warning!("Spiral failed to create trigger: {:?}", e);
                    }
                }

                // 5. Generate the entire hierarchy automatically
                notice!("Spiral: Calling generate_hierarchy_internal for '{}'", name);
                generate_hierarchy_internal(&name, &frames_str, scope_columns, captured_cols);

                notice!("Spiral: Successfully registered hierarchy for '{}'", name);

                IN_UTILITY.with(|h| h.set(false));
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
    IN_UTILITY.with(|h| h.set(false));
}

#[derive(Default, Clone, Debug)]
struct QueryConstraints {
    start: Option<i64>,
    end: Option<i64>,
    scopes: std::collections::HashMap<String, String>,
}

unsafe fn build_time_constraints(
    jointree: *mut pg_sys::Node,
    rtable: *mut pg_sys::List,
) -> (std::collections::HashMap<i32, QueryConstraints>, i64) {
    let mut constraints: std::collections::HashMap<i32, QueryConstraints> =
        std::collections::HashMap::new();
    let mut equalities: Vec<(i32, i32)> = Vec::new();

    let tz_offset = Spi::get_one::<f64>("SELECT EXTRACT(EPOCH FROM (now() AT TIME ZONE current_setting('TimeZone'))) - EXTRACT(EPOCH FROM (now() AT TIME ZONE 'UTC'))")
        .unwrap_or(Some(0.0)).unwrap_or(0.0) as i64;

    if jointree.is_null() {
        return (constraints, tz_offset);
    }

    let mut stack = vec![jointree];
    while let Some(node) = stack.pop() {
        if node.is_null() {
            continue;
        }
        match (*node).type_ {
            pg_sys::NodeTag::T_FromExpr => {
                let from = node as *mut pg_sys::FromExpr;
                if !(*from).quals.is_null() {
                    stack.push((*from).quals);
                }
                if !(*from).fromlist.is_null() {
                    let list = (*from).fromlist;
                    for i in 0..(*list).length {
                        stack.push(pg_sys::list_nth(list, i) as *mut pg_sys::Node);
                    }
                }
            }
            pg_sys::NodeTag::T_JoinExpr => {
                let join = node as *mut pg_sys::JoinExpr;
                if !(*join).quals.is_null() {
                    stack.push((*join).quals);
                }
                stack.push((*join).larg);
                stack.push((*join).rarg);
            }
            pg_sys::NodeTag::T_BoolExpr => {
                let bexpr = node as *mut pg_sys::BoolExpr;
                let args = (*bexpr).args;
                if !args.is_null() {
                    for i in 0..(*args).length {
                        stack.push(pg_sys::list_nth(args, i) as *mut pg_sys::Node);
                    }
                }
            }
            pg_sys::NodeTag::T_OpExpr => {
                let op = node as *mut pg_sys::OpExpr;
                let opname_ptr = pg_sys::get_opname((*op).opno);
                if opname_ptr.is_null() {
                    continue;
                }
                let opname = CStr::from_ptr(opname_ptr).to_string_lossy();
                let args = (*op).args;
                if args.is_null() || (*args).length != 2 {
                    continue;
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

                if (*var_node).type_ == pg_sys::NodeTag::T_Var
                    && (*right).type_ == pg_sys::NodeTag::T_Const
                {
                    let var = var_node as *mut pg_sys::Var;
                    let rte = pg_sys::list_nth(rtable, ((*var).varno - 1) as i32)
                        as *mut pg_sys::RangeTblEntry;
                    let varname_ptr = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                    if !varname_ptr.is_null() {
                        let varname = CStr::from_ptr(varname_ptr).to_string_lossy();
                        let con = right as *mut pg_sys::Const;

                        if varname == "t" {
                            let val = match (*con).consttype {
                                pg_sys::INT8OID => Some(
                                    i64::from_datum((*con).constvalue, (*con).constisnull).unwrap(),
                                ),
                                pg_sys::TIMESTAMPTZOID => {
                                    let ts = i64::from_datum((*con).constvalue, (*con).constisnull)
                                        .unwrap();
                                    Some(ts / 1000000)
                                }
                                _ => None,
                            };
                            if let Some(v) = val {
                                let qc = constraints.entry((*var).varno).or_default();
                                if opname == ">=" {
                                    qc.start = Some(v);
                                } else if opname == "<" {
                                    qc.end = Some(v);
                                }
                            }
                        } else if opname == "=" {
                            // Possible scope column
                            let val = match (*con).consttype {
                                pg_sys::TEXTOID => Some(
                                    String::from_datum((*con).constvalue, (*con).constisnull)
                                        .unwrap(),
                                ),
                                pg_sys::INT4OID => Some(
                                    i32::from_datum((*con).constvalue, (*con).constisnull)
                                        .unwrap()
                                        .to_string(),
                                ),
                                pg_sys::INT8OID => Some(
                                    i64::from_datum((*con).constvalue, (*con).constisnull)
                                        .unwrap()
                                        .to_string(),
                                ),
                                _ => None,
                            };
                            if let Some(v) = val {
                                let qc = constraints.entry((*var).varno).or_default();
                                qc.scopes.insert(varname.into_owned(), v);
                            }
                        }
                    }
                } else if (*left).type_ == pg_sys::NodeTag::T_Var
                    && (*right).type_ == pg_sys::NodeTag::T_Var
                    && opname == "="
                {
                    let v1 = left as *mut pg_sys::Var;
                    let v2 = right as *mut pg_sys::Var;
                    let rte1 = pg_sys::list_nth(rtable, ((*v1).varno - 1) as i32)
                        as *mut pg_sys::RangeTblEntry;
                    let rte2 = pg_sys::list_nth(rtable, ((*v2).varno - 1) as i32)
                        as *mut pg_sys::RangeTblEntry;
                    let n1 = pg_sys::get_attname((*rte1).relid, (*v1).varattno, true);
                    let n2 = pg_sys::get_attname((*rte2).relid, (*v2).varattno, true);
                    if !n1.is_null()
                        && !n2.is_null()
                        && CStr::from_ptr(n1).to_string_lossy() == "t"
                        && CStr::from_ptr(n2).to_string_lossy() == "t"
                    {
                        equalities.push(((*v1).varno, (*v2).varno));
                    }
                }
            }
            _ => {}
        }
    }

    for _ in 0..(*rtable).length {
        let mut changed = false;
        for (v1, v2) in &equalities {
            let r1 = constraints.get(v1).cloned().unwrap_or_default();
            let r2 = constraints.get(v2).cloned().unwrap_or_default();
            let new_start = r1.start.or(r2.start);
            let new_end = r1.end.or(r2.end);
            if new_start != r1.start || new_end != r1.end {
                let qc = constraints.entry(*v1).or_default();
                qc.start = new_start;
                qc.end = new_end;
                changed = true;
            }
            if new_start != r2.start || new_end != r2.end {
                let qc = constraints.entry(*v2).or_default();
                qc.start = new_start;
                qc.end = new_end;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (constraints, tz_offset)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_planner_hook(
    parse: *mut pg_sys::Query,
    query_string: *const c_char,
    cursor_options: c_int,
    bound_params: pg_sys::ParamListInfo,
) -> *mut pg_sys::PlannedStmt {
    if IN_HOOK.with(|h| h.get()) || crate::SKIP_ACCELERATION.with(|s| s.get()) {
        return if let Some(prev_hook) = PREV_PLANNER_HOOK {
            prev_hook(parse, query_string, cursor_options, bound_params)
        } else {
            pg_sys::standard_planner(parse, query_string, cursor_options, bound_params)
        };
    }
    IN_HOOK.with(|h| h.set(true));
    let query = &mut *parse;
    if query.commandType == pg_sys::CmdType::CMD_SELECT {
        let rtable = query.rtable;
        if !rtable.is_null() {
            let (constraint_map, tz_offset) =
                build_time_constraints(query.jointree as *mut pg_sys::Node, rtable);

            for i in 0..(*rtable).length {
                let varno = i + 1 ;
                let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
                if !rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                    let relid = (*rte).relid;
                    let relname = pg_sys::get_rel_name(relid);
                    if !relname.is_null() {
                        let base_table = CStr::from_ptr(relname).to_string_lossy().into_owned();
                        let hierarchy = Spi::connect(|client| {
                            let mut views = Vec::new();
                            // Safety check: Ensure the metadata table exists before querying
                            let table_exists = !client.select("SELECT 1 FROM information_schema.tables WHERE table_schema = 'spiral' AND table_name = 'metadata' LIMIT 1", Some(1), &[])?.is_empty();
                            if !table_exists { return Ok::<Vec<String>, spi::Error>(views); }

                            let table = client.select("SELECT view_name FROM spiral.metadata WHERE base_view = $1", None,
                                unsafe { &[pgrx::datum::DatumWithOid::new(base_table.clone().into_datum().unwrap(), pg_sys::TEXTOID)] })?;
                            for row in table { views.push(row.get::<String>(1)?.unwrap()); }
                            Ok::<Vec<String>, spi::Error>(views)
                        }).unwrap_or_default();

                        if !hierarchy.is_empty() {
                            if let Some(qc) = constraint_map.get(&varno) {
                                if let (Some(ts), Some(te)) = (qc.start, qc.end) {
                                    // Build scope_values JsonB from qc.scopes if they match view's scope_columns
                                    let metadata_obj = catalog::get_metadata(&base_table);
                                    let scope_values = metadata_obj.as_ref().and_then(|m| {
                                        let mut map = serde_json::Map::new();
                                        for col in &m.scope_columns {
                                            if let Some(val) = qc.scopes.get(col) {
                                                map.insert(
                                                    col.clone(),
                                                    serde_json::Value::String(val.clone()),
                                                );
                                            }
                                        }
                                        if map.is_empty() {
                                            None
                                        } else {
                                            Some(pgrx::JsonB(serde_json::Value::Object(map)))
                                        }
                                    });

                                    let dirty_ranges = catalog::get_dirty_ranges(
                                        &base_table,
                                        ts,
                                        te,
                                        scope_values,
                                    );
                                    let segments = resolve_segments(
                                        &base_table,
                                        ts,
                                        te,
                                        &hierarchy,
                                        &dirty_ranges,
                                        tz_offset,
                                    );

                                    if !segments.is_empty()
                                        && (segments.len() > 1 || segments[0].source != base_table)
                                    {
                                        notice!("Spiral: Accelerating '{}' (RTE #{}) with range {} to {} (Offset: {}s)", base_table, varno, format_epoch(ts), format_epoch(te), tz_offset);

                                        let query_cols =
                                            extract_query_columns_simple(query, rtable);
                                        let mut cols = Vec::new();
                                        let base_cols_query = format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped ORDER BY attnum", base_table.replace("\"", "\"\""));
                                        let base_table_columns = Spi::connect(|client| {
                                            Ok::<Vec<String>, spi::Error>(
                                                client
                                                    .select(&base_cols_query, None, &[])?
                                                    .map(|r| r.get::<String>(1).unwrap().unwrap())
                                                    .collect(),
                                            )
                                        })
                                        .unwrap_or_default();

                                        for c in base_table_columns {
                                            if c == "t" {
                                                continue;
                                            }
                                            if let Some((_, agg)) =
                                                query_cols.iter().find(|(name, _)| name == &c)
                                            {
                                                cols.push((c, agg.clone()));
                                            } else {
                                                cols.push((c, None));
                                            }
                                        }

                                        let union_sql = construct_union_sql_hierarchical(
                                            &base_table,
                                            &segments,
                                            &cols,
                                        );
                                        let new_query = parse_sql_to_query(&union_sql);
                                        if !new_query.is_null() {
                                            (*rte).rtekind = pg_sys::RTEKind::RTE_SUBQUERY;
                                            (*rte).subquery = new_query;
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
) -> Vec<Segment> {
    let mut segments = Vec::new();

    let mut sorted_hierarchy: Vec<(String, i32)> = hierarchy
        .iter()
        .filter_map(|h| catalog::get_metadata(h).map(|m| (h.clone(), m.frame_seconds)))
        .filter(|h| h.1 > 0)
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

pub fn generate_hierarchy_internal(
    base_name: &str,
    frames_str: &str,
    scope_columns: Vec<String>,
    custom_cols: Vec<(String, String)>,
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

    for (i, frame) in frames.iter().enumerate() {
        let child_name = format!("{}_{}", base_prefix, frame.name);
        if child_name == current_parent {
            continue;
        }

        let mut sources = Vec::new();
        let mut select_parts = vec![format!(
            "to_timestamp(((spiral(t) / {0}) * {0} + 946684800)::double precision) as t",
            frame.seconds
        )];
        let mut group_parts = vec![format!("(spiral(t) / {0}) * {0}", frame.seconds)];
        let mut seen_cols = std::collections::HashSet::new();
        seen_cols.insert("t".to_string());

        for s in &scope_columns {
            if seen_cols.insert(s.clone()) {
                select_parts.push(format!("\"{}\"", s));
                group_parts.push(format!("\"{}\"", s));
            }
        }

        if i == 0 {
            // First level: Use custom magic comments
            for (col, formula) in &custom_cols {
                let formula_lower = formula.to_lowercase();
                if formula_lower.contains("stats") {
                    let mat = format!("{}_stats", col);
                    if seen_cols.insert(mat.clone()) {
                        select_parts.push(format!("spiral_stats(\"{}\") as \"{}\"", col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "stats".to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                }
                if formula_lower.contains("ohlc") {
                    let mat = format!("{}_ohlcv", col);
                    if seen_cols.insert(mat.clone()) {
                        select_parts.push(format!("first(\"{}\", spiral(t)) as \"{}_o\", max(\"{}\") as \"{}_h\", min(\"{}\") as \"{}_l\", last(\"{}\", spiral(t)) as \"{}_c\", sum(\"{}\") as \"{}_v\"", col, mat, col, mat, col, mat, col, mat, col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "ohlcv".to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                }
                if formula_lower.contains("sketch") || formula_lower.contains("quantile") {
                    let mat = format!("{}_sketch", col);
                    if seen_cols.insert(mat.clone()) {
                        select_parts.push(format!("spiral_sketch(\"{}\") as \"{}\"", col, mat));
                        sources.push(rollup::SourceDef {
                            base_column: col.clone(),
                            formula: "sketch".to_string(),
                            mat_column: mat,
                            rollup_gsub_strategy: None,
                        });
                    }
                }
            }
            // Add other columns as sum by default
            Spi::connect(|client| {
                let q = format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped", base_name.replace("\"", "\"\""));
                let cols = client.select(&q, None, &[]).unwrap();
                for row in cols {
                    let col = row.get::<String>(1).unwrap().unwrap();
                    if !seen_cols.contains(&col) && col != "t"
                        && seen_cols.insert(col.clone()) {
                            select_parts.push(format!("sum(\"{}\") as \"{}\"", col, col));
                            sources.push(rollup::SourceDef {
                                base_column: col.clone(),
                                formula: "sum".to_string(),
                                mat_column: col.clone(),
                                rollup_gsub_strategy: None,
                            });
                        }
                }
            });
        } else {
            // Higher levels: derive from parent
            let (_, parent_sources) = rollup::derive_child_sql(
                &child_name,
                &current_parent,
                frame.seconds,
                &scope_columns,
            );
            for src in parent_sources {
                if !seen_cols.insert(src.mat_column.clone()) {
                    continue;
                }
                if let Some(strategy) = &src.rollup_gsub_strategy {
                    let sql = strategy.replace("rollup(\"\\1\")", &format!("\"{}\"", src.mat_column))
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
                } else if src.formula == "ohlcv" {
                    let c = &src.mat_column;
                    select_parts.push(format!("first(\"{}_o\", spiral(t)) as \"{}_o\", max(\"{}_h\") as \"{}_h\", min(\"{}_l\") as \"{}_l\", last(\"{}_c\", spiral(t)) as \"{}_c\", sum(\"{}_v\") as \"{}_v\"", c, c, c, c, c, c, c, c, c, c));
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
                for src in sources {
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
            Err(e) => warning!("Spiral failed to create child view {}: {:?}", child_name, e),
        }
    }
}

unsafe fn parse_sql_to_query(sql: &str) -> *mut pg_sys::Query {
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

fn map_agg_inner(agg_fn: &str, mapped_col: &str, is_rollup: bool) -> String {
    let lower = agg_fn.to_lowercase();
    if !is_rollup {
        return format!("{}(\"{}\")", agg_fn, mapped_col);
    }
    match lower.as_str() {
        "sum" | "count" | "min" | "max" | "tdigest" => format!("\"{}\"", mapped_col),
        "avg" => format!("\"{}\"", mapped_col),
        _ => format!("\"{}\"", mapped_col),
    }
}

unsafe fn extract_query_columns_simple(
    query: *mut pg_sys::Query,
    rtable: *mut pg_sys::List,
) -> Vec<(String, Option<String>)> {
    let mut cols = Vec::new();
    let target_list = (*query).targetList;
    if target_list.is_null() {
        return cols;
    }
    for i in 0..(*target_list).length {
        let tle = pg_sys::list_nth(target_list, i) as *mut pg_sys::TargetEntry;
        let node = (*tle).expr as *mut pg_sys::Node;
        if node.is_null() {
            continue;
        }
        match (*node).type_ {
            pg_sys::NodeTag::T_Var => {
                let var = node as *mut pg_sys::Var;
                let rte = pg_sys::list_nth(rtable, ((*var).varno - 1) as i32)
                    as *mut pg_sys::RangeTblEntry;
                let varname = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                if !varname.is_null() {
                    cols.push((CStr::from_ptr(varname).to_string_lossy().into_owned(), None));
                }
            }
            pg_sys::NodeTag::T_Aggref => {
                let agg = node as *mut pg_sys::Aggref;
                let agg_fn = pg_sys::get_func_name((*agg).aggfnoid);
                if !agg_fn.is_null() {
                    let fn_name = CStr::from_ptr(agg_fn).to_string_lossy().into_owned();
                    let args = (*agg).args;
                    if !args.is_null() && (*args).length > 0 {
                        let arg = pg_sys::list_nth(args, 0) as *mut pg_sys::TargetEntry;
                        let arg_expr = (*arg).expr as *mut pg_sys::Node;
                        if !arg_expr.is_null() && (*arg_expr).type_ == pg_sys::NodeTag::T_Var {
                            let var = arg_expr as *mut pg_sys::Var;
                            let rte = pg_sys::list_nth(rtable, ((*var).varno - 1) as i32)
                                as *mut pg_sys::RangeTblEntry;
                            let varname = pg_sys::get_attname((*rte).relid, (*var).varattno, true);
                            if !varname.is_null() {
                                cols.push((
                                    CStr::from_ptr(varname).to_string_lossy().into_owned(),
                                    Some(fn_name),
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    cols
}

fn format_epoch(epoch: i64) -> String {
    let date = Spi::get_one_with_args::<String>(
        "SELECT to_char(to_timestamp($1::double precision), 'YYYY-MM-DD HH24:MI:SS')",
        &[unsafe { pgrx::datum::DatumWithOid::new(epoch.into_datum().unwrap(), pg_sys::INT8OID) }],
    )
    .unwrap()
    .unwrap_or_else(|| epoch.to_string());
    date
}

fn construct_union_sql_hierarchical(
    base_table: &str,
    segments: &[Segment],
    cols: &[(String, Option<String>)],
) -> String {
    let mut union_parts = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let mut inner_select = Vec::new();
        inner_select.push(format!(
            "to_timestamp({}::double precision) as t",
            seg._t_start + 946684800
        ));
        for (col, agg) in cols {
            if let Some(agg_fn) = agg {
                let is_rollup = seg.source != base_table;
                let mapped = if is_rollup {
                    Spi::connect(|client| {
                        client.select("SELECT mat_column FROM spiral.sources WHERE view_name = $1 AND base_column = $2 AND formula = $3 LIMIT 1", None,
                            unsafe { &[
                                pgrx::datum::DatumWithOid::new(seg.source.clone().into_datum().unwrap(), pg_sys::TEXTOID),
                                pgrx::datum::DatumWithOid::new(col.clone().into_datum().unwrap(), pg_sys::TEXTOID),
                                pgrx::datum::DatumWithOid::new(agg_fn.to_lowercase().into_datum().unwrap(), pg_sys::TEXTOID)
                            ]}
                        )?.get_one::<String>()
                    }).unwrap_or(None).unwrap_or_else(|| col.clone())
                } else {
                    col.clone()
                };
                inner_select.push(format!(
                    "{} as \"{}\"",
                    map_agg_inner(agg_fn, &mapped, is_rollup),
                    col
                ));
            } else {
                if col != "t" {
                    inner_select.push(format!("\"{}\"", col));
                }
            }
        }
        let where_clause = format!(
            "spiral(t) >= {} AND spiral(t) < {}",
            seg._t_start, seg._t_end
        );
        let group_by_str = if seg.source == base_table {
            let group_by = cols
                .iter()
                .filter(|(c, agg)| agg.is_none() && c != "t")
                .map(|(col, _)| format!("\"{}\"", col))
                .collect::<Vec<_>>();
            if group_by.is_empty() {
                "".to_string()
            } else {
                format!(" GROUP BY {}", group_by.join(", "))
            }
        } else {
            "".to_string()
        };
        let alias = if seg.source == base_table {
            format!("raw_fallback_{}", i)
        } else {
            let tier = match catalog::get_metadata(&seg.source).map(|m| m.frame_seconds) {
                Some(86400) => "daily",
                Some(3600) => "hourly",
                Some(60) => "minutely",
                _ => "rollup",
            };
            format!("{}_tier_{}", tier, i)
        };
        union_parts.push(format!(
            "SELECT * FROM (SELECT {} FROM {} WHERE {}{}) AS {}",
            inner_select.join(", "),
            seg.source,
            where_clause,
            group_by_str,
            alias
        ));
    }
    union_parts.join(" UNION ALL ")
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
        .map(|m| m.parent_view == m.base_view)
        .unwrap_or(false);
    let real_base = metadata
        .as_ref()
        .map(|m| m.base_view.clone())
        .unwrap_or_else(|| base_name.to_string());

    if is_root {
        // Bootstrap: If changelog is empty for this base table, insert a full range to force initial materialization
        let count: i64 = Spi::get_one_with_args(
            "SELECT count(*) FROM spiral.changelog WHERE base_view = $1",
            &[unsafe {
                pgrx::datum::DatumWithOid::new(
                    real_base.clone().into_datum().unwrap(),
                    pg_sys::TEXTOID,
                )
            }],
        )
        .unwrap()
        .unwrap();
        if count == 0 {
            let bootstrap_sql = format!("INSERT INTO spiral.changelog (base_view, t_start, t_end) VALUES ('{}', 0, 2147483647)", real_base.replace("'", "''"));
            let _ = Spi::run(&bootstrap_sql);
        }
        catalog::unify_changelog(&real_base);

        // Capture IDs to be refreshed
        let _ = Spi::run(&format!("CREATE TEMP TABLE refreshing_changelog AS SELECT ctid as old_ctid FROM spiral.changelog WHERE base_view = '{}'", real_base.replace("'", "''")));
    }

    let success = crate::refresh_incremental(base_name, where_clause.clone(), 0);

    if success && is_root
        && where_clause.is_none() {
            let _ = Spi::run("DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM refreshing_changelog)");
        }

    if is_root {
        let _ = Spi::run("DROP TABLE IF EXISTS refreshing_changelog");
    }

    success
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
            let varno = i + 1 ;
            let rte = pg_sys::list_nth(rtable, i) as *mut pg_sys::RangeTblEntry;
            if (*rte).rtekind != pg_sys::RTEKind::RTE_RELATION {
                continue;
            }

            let relname = pg_sys::get_rel_name((*rte).relid);
            if relname.is_null() {
                continue;
            }
            let base_table = CStr::from_ptr(relname).to_string_lossy().into_owned();

            let hierarchy = Spi::connect(|client| {
                let mut views = Vec::new();
                let table_exists = !client.select("SELECT 1 FROM information_schema.tables WHERE table_schema = 'spiral' AND table_name = 'metadata' LIMIT 1", Some(1), &[])?.is_empty();
                if !table_exists { return Ok::<Vec<String>, spi::Error>(views); }

                let table = client.select("SELECT view_name FROM spiral.metadata WHERE base_view = $1", None,
                    &[pgrx::datum::DatumWithOid::new(base_table.clone().into_datum().unwrap(), pg_sys::TEXTOID)])?;
                for row in table { views.push(row.get::<String>(1)?.unwrap()); }
                Ok::<Vec<String>, spi::Error>(views)
            }).unwrap_or_default();

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

                    let segments =
                        resolve_segments(&base_table, ts, te, &hierarchy, &dirty_ranges, tz_offset);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_agg_inner_not_rollup() {
        assert_eq!(map_agg_inner("SUM", "col_a", false), "SUM(\"col_a\")");
        assert_eq!(map_agg_inner("count", "col_b", false), "count(\"col_b\")");
    }

    #[test]
    fn test_map_agg_inner_rollup() {
        assert_eq!(map_agg_inner("SUM", "col_a", true), "\"col_a\"");
        assert_eq!(map_agg_inner("COUNT", "col_a", true), "\"col_a\"");
        assert_eq!(map_agg_inner("avg", "col_a", true), "\"col_a\"");
        assert_eq!(map_agg_inner("unknown_agg", "col_c", true), "\"col_c\"");
    }
}

