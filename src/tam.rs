use pgrx::pg_sys;
use pgrx::prelude::*;

// Table Access Method (TAM) Handler for Spiral
#[pg_extern(sql = "
        CREATE FUNCTION spiral_tam_handler(internal) RETURNS table_am_handler LANGUAGE c AS 'MODULE_PATHNAME', 'spiral_tam_handler_wrapper';
        CREATE ACCESS METHOD spiral TYPE TABLE HANDLER spiral_tam_handler;
    ")]
/// # Safety
/// This function is unsafe because it interacts with PostgreSQL C internals.
pub unsafe fn spiral_tam_handler(_fcinfo: pg_sys::FunctionCallInfo) -> pgrx::datum::Internal {
    let routine =
        pgrx::PgMemoryContexts::TopMemoryContext.palloc0_struct::<pg_sys::TableAmRoutine>();

    (*routine).type_ = pg_sys::NodeTag::T_TableAmRoutine;

    // Wire up the O(1) logic callbacks
    (*routine).slot_callbacks = Some(spiral_slot_callbacks);
    (*routine).tuple_insert = Some(spiral_slot_insert);

    (*routine).scan_begin = Some(spiral_scan_begin);
    (*routine).scan_getnextslot = Some(spiral_scan_getnextslot);
    (*routine).scan_end = Some(spiral_scan_end);
    (*routine).scan_rescan = Some(spiral_scan_rescan);

    (*routine).relation_size = Some(spiral_relation_size);
    (*routine).relation_estimate_size = Some(spiral_relation_estimate_size);
    (*routine).relation_set_new_filelocator = Some(spiral_relation_set_new_filelocator);
    (*routine).relation_nontransactional_truncate = Some(spiral_relation_nontransactional_truncate);
    (*routine).relation_copy_for_cluster = Some(spiral_relation_copy_for_cluster);
    (*routine).tuple_fetch_row_version = Some(spiral_tuple_fetch_row_version);
    (*routine).tuple_tid_valid = Some(spiral_tuple_tid_valid);
    (*routine).tuple_satisfies_snapshot = Some(spiral_tuple_satisfies_snapshot);
    (*routine).relation_needs_toast_table = Some(spiral_relation_needs_toast_table);

    pgrx::datum::Internal::from(Some(pg_sys::Datum::from(routine as usize)))
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_needs_toast_table(_rel: pg_sys::Relation) -> bool {
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_nontransactional_truncate(_rel: pg_sys::Relation) {}

#[pg_guard]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C-unwind" fn spiral_relation_copy_for_cluster(
    _old_heap: pg_sys::Relation,
    _new_heap: pg_sys::Relation,
    _old_index: pg_sys::Relation,
    _use_sort: bool,
    _oldest_xmin: pg_sys::TransactionId,
    _freeze_xid: *mut pg_sys::TransactionId,
    _minmulti: *mut pg_sys::MultiXactId,
    _pages: *mut f64,
    _tuples: *mut f64,
    _allvisfrac: *mut f64,
) {
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_fetch_row_version(
    _rel: pg_sys::Relation,
    _tid: pg_sys::ItemPointer,
    _snapshot: pg_sys::Snapshot,
    _slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_tid_valid(
    _scan: pg_sys::TableScanDesc,
    _tid: pg_sys::ItemPointer,
) -> bool {
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_satisfies_snapshot(
    _rel: pg_sys::Relation,
    _slot: *mut pg_sys::TupleTableSlot,
    _snapshot: pg_sys::Snapshot,
) -> bool {
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_set_new_filelocator(
    _rel: pg_sys::Relation,
    _newrlocator: *const pg_sys::RelFileLocator,
    _persistence: std::os::raw::c_char,
    _freeze_xid: *mut pg_sys::TransactionId,
    _multi_xid: *mut pg_sys::MultiXactId,
) {
    unsafe {
        *_freeze_xid = pg_sys::TransactionId::from(0);
        *_multi_xid = pg_sys::MultiXactId::from(0);
        pg_sys::RelationCreateStorage(*_newrlocator, _persistence, true);
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_slot_callbacks(
    _rel: pg_sys::Relation,
) -> *const pg_sys::TupleTableSlotOps {
    &pg_sys::TTSOpsVirtual
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_size(
    rel: pg_sys::Relation,
    _fork_number: pg_sys::ForkNumber::Type,
) -> u64 {
    unsafe {
        pg_sys::RelationGetSmgr(rel);
        let nblocks = pg_sys::smgrnblocks((*rel).rd_smgr, pg_sys::ForkNumber::MAIN_FORKNUM);
        (nblocks as u64) * 8192
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_estimate_size(
    rel: pg_sys::Relation,
    _attr_widths: *mut i32,
    _pages: *mut pg_sys::BlockNumber,
    _tuples: *mut f64,
    _allvisfrac: *mut f64,
) {
    unsafe {
        pg_sys::RelationGetSmgr(rel);
        let nblocks = pg_sys::smgrnblocks((*rel).rd_smgr, pg_sys::ForkNumber::MAIN_FORKNUM);
        *_pages = nblocks;
        *_tuples = (nblocks as f64) * 1024.0;
        *_allvisfrac = 1.0;
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_slot_insert(
    rel: pg_sys::Relation,
    _slot: *mut pg_sys::TupleTableSlot,
    _cid: pg_sys::CommandId,
    _options: i32,
    _state: *mut pg_sys::BulkInsertStateData,
) {
    info!(
        "Spiral TAM: Routing insert to Delta Store for relation OID {}",
        (*rel).rd_id
    );
}

use std::ffi::CStr;

struct SpiralScanState {
    tenant_scale: i64,
    current_blkno: pg_sys::BlockNumber,
    total_blks: pg_sys::BlockNumber,
    current_offset_in_page: u32,
}

#[repr(C)]
struct SpiralScanDescData {
    base: pg_sys::TableScanDescData,
    state: *mut SpiralScanState,
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
    let spiral_scan =
        pgrx::pg_sys::palloc0(std::mem::size_of::<SpiralScanDescData>()) as *mut SpiralScanDescData;
    let scan = spiral_scan as pg_sys::TableScanDesc;
    if !scan.is_null() {
        (*scan).rs_rd = rel;
        (*scan).rs_snapshot = snapshot;

        let oid = (*rel).rd_id;
        let mut tenant_scale = 1024;
        let relname_ptr = pg_sys::get_rel_name(oid);
        if !relname_ptr.is_null() {
            let name = CStr::from_ptr(relname_ptr).to_string_lossy().into_owned();
            pg_sys::pfree(relname_ptr as *mut std::ffi::c_void);
            if let Some(m) = crate::catalog::get_metadata(&name) {
                tenant_scale = crate::catalog::get_tenant_scale(&m);
            }
        }

        let total_blks =
            pg_sys::RelationGetNumberOfBlocksInFork(rel, pg_sys::ForkNumber::MAIN_FORKNUM);
        let state = Box::new(SpiralScanState {
            tenant_scale,
            current_blkno: 0,
            total_blks,
            current_offset_in_page: crate::storage::HEADER_SIZE as u32,
        });
        (*spiral_scan).state = Box::into_raw(state);
    }
    scan
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_getnextslot(
    scan: pg_sys::TableScanDesc,
    _direction: i32,
    slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    if scan.is_null() {
        return false;
    }

    let spiral_scan = scan as *mut SpiralScanDescData;
    if (*spiral_scan).state.is_null() {
        return false;
    }

    let state = &mut *(*spiral_scan).state;
    let rel = (*scan).rs_rd;

    while state.current_blkno < state.total_blks {
        let buffer = pg_sys::ReadBuffer(rel, state.current_blkno);
        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);

        let page = pg_sys::BufferGetPage(buffer);
        let upper_bound = (crate::storage::BLCKSZ - crate::storage::SPECIAL_SIZE) as u32;

        while state.current_offset_in_page + 8 <= upper_bound {
            let ptr = (page as *const u8).add(state.current_offset_in_page as usize);
            let val = *(ptr as *const f64);
            let offset_in_page = state.current_offset_in_page;
            state.current_offset_in_page += 8;

            if val != 0.0 {
                let items_before = (offset_in_page - crate::storage::HEADER_SIZE as u32) / 8;
                let idx = (state.current_blkno as i64 * crate::storage::DATA_PER_PAGE as i64)
                    + items_before as i64;

                let t = idx / state.tenant_scale;
                let tenant_id = (idx % state.tenant_scale) as i32;

                pg_sys::ExecClearTuple(slot);
                let values = (*slot).tts_values;
                let isnull = (*slot).tts_isnull;

                *values.add(0) = t.into_datum().unwrap();
                *isnull.add(0) = false;
                *values.add(1) = tenant_id.into_datum().unwrap();
                *isnull.add(1) = false;
                *values.add(2) = val.into_datum().unwrap();
                *isnull.add(2) = false;

                pg_sys::ExecStoreVirtualTuple(slot);

                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
                return true;
            }
        }

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);

        state.current_blkno += 1;
        state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
    }
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_end(scan: pg_sys::TableScanDesc) {
    if !scan.is_null() {
        let spiral_scan = scan as *mut SpiralScanDescData;
        if !(*spiral_scan).state.is_null() {
            let _ = Box::from_raw((*spiral_scan).state);
            (*spiral_scan).state = std::ptr::null_mut();
        }
        pg_sys::pfree(scan as *mut std::ffi::c_void);
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_rescan(
    scan: pg_sys::TableScanDesc,
    _key: *mut pg_sys::ScanKeyData,
    _set_params: bool,
    _allow_strat: bool,
    _allow_sync: bool,
    _allow_pagemode: bool,
) {
    if !scan.is_null() {
        let spiral_scan = scan as *mut SpiralScanDescData;
        if !(*spiral_scan).state.is_null() {
            let state = &mut *(*spiral_scan).state;
            state.current_blkno = 0;
            state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
        }
    }
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_tam_insert_placeholder() {
        Spi::connect_mut(|client| {
            client.update("CREATE TABLE tam_test_internal (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_test_internal (t, tenant_id, value) VALUES (1, 1, 42.0);", None, &[])?;
            let count = Spi::get_one::<i64>("SELECT count(*) FROM tam_test_internal").unwrap().unwrap();
            // Since INSERT is a placeholder, count should be 0
            assert_eq!(count, 0);
            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_scan_with_seeded_data() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_scan_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("CREATE TABLE tam_delta_input (t bigint, tenant_id int, price double precision);", None, &[])?;
            client.update("INSERT INTO tam_delta_input (t, tenant_id, price) VALUES (1, 1, 42.0), (2, 2, 84.0);", None, &[])?;

            // Pack data bypassing TAM
            let main_oid = Spi::get_one::<i32>("SELECT 'tam_scan_test'::regclass::oid::int").unwrap().unwrap();
            client.update(&format!("SELECT spiral_pack_delta('tam_delta_input', {});", main_oid), None, &[])?;

            // Now SCAN via TAM
            let mut results = client.select("SELECT t, tenant_id, value FROM tam_scan_test ORDER BY t", None, &[])?;

            let row1 = results.next().unwrap();
            assert_eq!(row1.get::<i64>(1).unwrap().unwrap(), 1);
            assert_eq!(row1.get::<i32>(2).unwrap().unwrap(), 1);
            assert_eq!(row1.get::<f64>(3).unwrap().unwrap(), 42.0);

            let row2 = results.next().unwrap();
            assert_eq!(row2.get::<i64>(1).unwrap().unwrap(), 2);
            assert_eq!(row2.get::<i32>(2).unwrap().unwrap(), 2);
            assert_eq!(row2.get::<f64>(3).unwrap().unwrap(), 84.0);

            // NOTE: UPDATE/DELETE are currently UNSUPPORTED and will fail with
            // "failed to fetch tuple being updated" because tuple_fetch_row_version returns false.
            /*
            client.update("UPDATE tam_scan_test SET value = 99.0 WHERE t = 1", None, &[])?;
            client.update("DELETE FROM tam_scan_test WHERE t = 2", None, &[])?;
            */

            Ok::<(), spi::Error>(())
        }).unwrap();
    }
}
