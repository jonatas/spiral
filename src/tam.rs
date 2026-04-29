use pgrx::prelude::*;
use pgrx::pg_sys;

// Table Access Method (TAM) Handler for Spiral
#[pg_extern(
    sql = "
        CREATE FUNCTION spiral_tam_handler(internal) RETURNS table_am_handler LANGUAGE c AS 'MODULE_PATHNAME', 'spiral_tam_handler_wrapper' STRICT;
        CREATE ACCESS METHOD spiral TYPE TABLE HANDLER spiral_tam_handler;
    "
)]
pub unsafe fn spiral_tam_handler(_fcinfo: pg_sys::FunctionCallInfo) -> pgrx::datum::Internal {
    let routine = pgrx::PgMemoryContexts::TopMemoryContext.palloc_struct::<pg_sys::TableAmRoutine>();

    (*routine).type_ = pg_sys::NodeTag::T_TableAmRoutine;
    
    // Wire up the O(1) logic callbacks
    (*routine).tuple_insert = Some(spiral_slot_insert);
    (*routine).scan_begin = Some(spiral_scan_begin);
    (*routine).scan_getnextslot = Some(spiral_scan_getnextslot);
    
    pgrx::datum::Internal::from(Some(pg_sys::Datum::from(routine as usize)))
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_slot_insert(
    rel: pg_sys::Relation,
    _slot: *mut pg_sys::TupleTableSlot,
    _cid: pg_sys::CommandId,
    _options: i32,
    _state: *mut pg_sys::BulkInsertStateData,
) {
    info!("Spiral TAM: Routing insert to Delta Store for relation OID {}", (*rel).rd_id);
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_begin(
    rel: pg_sys::Relation,
    snapshot: pg_sys::Snapshot,
    _nkeys: i32,
    _key: *mut pg_sys::ScanKeyData,
    _pscan: pg_sys::ParallelTableScanDesc,
    _flags: u32,
) -> pg_sys::TableScanDesc {
    let scan = pg_sys::palloc0(std::mem::size_of::<pg_sys::TableScanDescData>()) as pg_sys::TableScanDesc;
    if !scan.is_null() {
        (*scan).rs_rd = rel;
        (*scan).rs_snapshot = snapshot;
        // In a full implementation, we would store our file handle/offset in rs_opaque
    }
    scan
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_getnextslot(
    _scan: pg_sys::TableScanDesc,
    _direction: i32,
    _slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    // This is where we would stream from the binary file directly into the slot.
    // By implementing this, SELECT * FROM table automatically becomes an O(N) 
    // stream of our compact 8-byte rows.
    
    // For the prototype, we return false to indicate end of scan,
    // but in production, this would use logic similar to spiral_scan_zero()
    false 
}
