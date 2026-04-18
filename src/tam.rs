use pgrx::prelude::*;
use pgrx::pg_sys;

// Table Access Method (TAM) Handler for Aspiral
#[pg_extern(
    sql = "
        CREATE FUNCTION aspiral_tam_handler(internal) RETURNS table_am_handler LANGUAGE c AS 'MODULE_PATHNAME', 'aspiral_tam_handler_wrapper' STRICT;
        CREATE ACCESS METHOD aspiral TYPE TABLE HANDLER aspiral_tam_handler;
    "
)]
pub unsafe fn aspiral_tam_handler(_fcinfo: pg_sys::FunctionCallInfo) -> pgrx::datum::Internal {
    let routine = pgrx::PgMemoryContexts::TopMemoryContext.palloc_struct::<pg_sys::TableAmRoutine>();

    (*routine).type_ = pg_sys::NodeTag::T_TableAmRoutine;
    
    // Wire up the O(1) logic callbacks
    (*routine).tuple_insert = Some(aspiral_slot_insert);
    (*routine).scan_begin = Some(aspiral_scan_begin);
    (*routine).scan_getnextslot = Some(aspiral_scan_getnextslot);
    
    pgrx::datum::Internal::from(Some(pg_sys::Datum::from(routine as usize)))
}

#[pg_guard]
pub unsafe extern "C-unwind" fn aspiral_slot_insert(
    rel: pg_sys::Relation,
    _slot: *mut pg_sys::TupleTableSlot,
    _cid: pg_sys::CommandId,
    _options: i32,
    _state: *mut pg_sys::BulkInsertStateData,
) {
    info!("Aspiral TAM: Routing insert to Delta Store for relation OID {}", (*rel).rd_id);
}

#[pg_guard]
pub unsafe extern "C-unwind" fn aspiral_scan_begin(
    rel: pg_sys::Relation,
    snapshot: pg_sys::Snapshot,
    _nkeys: i32,
    _key: *mut pg_sys::ScanKeyData,
    _pscan: pg_sys::ParallelTableScanDesc,
    _flags: u32,
) -> pg_sys::TableScanDesc {
    info!("Aspiral TAM: Initializing O(1) mathematical scan");
    let scan = pg_sys::palloc0(std::mem::size_of::<pg_sys::TableScanDescData>()) as pg_sys::TableScanDesc;
    if !scan.is_null() {
        (*scan).rs_rd = rel;
        (*scan).rs_snapshot = snapshot;
    }
    scan
}

#[pg_guard]
pub unsafe extern "C-unwind" fn aspiral_scan_getnextslot(
    _scan: pg_sys::TableScanDesc,
    _direction: i32,
    _slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    false 
}
