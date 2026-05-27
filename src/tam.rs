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
    let routine = pg_sys::MemoryContextAllocZero(
        pg_sys::TopMemoryContext,
        std::mem::size_of::<pg_sys::TableAmRoutine>(),
    ) as *mut pg_sys::TableAmRoutine;

    (*routine).type_ = pg_sys::NodeTag::T_TableAmRoutine;

    // Wire up the O(1) logic callbacks
    (*routine).slot_callbacks = Some(spiral_slot_callbacks);
    (*routine).tuple_insert = Some(spiral_slot_insert);

    (*routine).scan_begin = Some(spiral_scan_begin);
    (*routine).scan_getnextslot = Some(spiral_scan_getnextslot);
    (*routine).scan_end = Some(spiral_scan_end);
    (*routine).scan_rescan = Some(spiral_scan_rescan);
    (*routine).scan_set_tidrange = Some(spiral_scan_set_tidrange);
    (*routine).scan_getnextslot_tidrange = Some(spiral_scan_getnextslot_tidrange);
    (*routine).parallelscan_estimate = Some(spiral_parallelscan_estimate);
    (*routine).parallelscan_initialize = Some(spiral_parallelscan_initialize);
    (*routine).parallelscan_reinitialize = Some(spiral_parallelscan_reinitialize);
    (*routine).index_fetch_begin = Some(spiral_index_fetch_begin);
    (*routine).index_fetch_reset = Some(spiral_index_fetch_reset);
    (*routine).index_fetch_end = Some(spiral_index_fetch_end);
    (*routine).index_fetch_tuple = Some(spiral_index_fetch_tuple);

    (*routine).relation_size = Some(spiral_relation_size);
    (*routine).relation_estimate_size = Some(spiral_relation_estimate_size);
    (*routine).relation_set_new_filelocator = Some(spiral_relation_set_new_filelocator);
    (*routine).relation_nontransactional_truncate = Some(spiral_relation_nontransactional_truncate);
    (*routine).relation_copy_data = Some(spiral_relation_copy_data);
    (*routine).relation_copy_for_cluster = Some(spiral_relation_copy_for_cluster);
    (*routine).relation_vacuum = Some(spiral_relation_vacuum);
    (*routine).tuple_fetch_row_version = Some(spiral_tuple_fetch_row_version);
    (*routine).tuple_tid_valid = Some(spiral_tuple_tid_valid);
    (*routine).tuple_get_latest_tid = Some(spiral_tuple_get_latest_tid);
    (*routine).tuple_satisfies_snapshot = Some(spiral_tuple_satisfies_snapshot);
    (*routine).index_delete_tuples = Some(spiral_index_delete_tuples);
    (*routine).relation_needs_toast_table = Some(spiral_relation_needs_toast_table);
    (*routine).relation_toast_am = Some(spiral_relation_toast_am);
    (*routine).relation_fetch_toast_slice = Some(spiral_relation_fetch_toast_slice);
    (*routine).tuple_insert_speculative = Some(spiral_tuple_insert_speculative);
    (*routine).tuple_complete_speculative = Some(spiral_tuple_complete_speculative);
    (*routine).multi_insert = Some(spiral_multi_insert);
    (*routine).tuple_delete = Some(spiral_tuple_delete);
    (*routine).tuple_update = Some(spiral_tuple_update);
    (*routine).tuple_lock = Some(spiral_tuple_lock);
    (*routine).finish_bulk_insert = Some(spiral_finish_bulk_insert);
    (*routine).scan_analyze_next_block = Some(spiral_scan_analyze_next_block);
    (*routine).scan_analyze_next_tuple = Some(spiral_scan_analyze_next_tuple);
    (*routine).index_build_range_scan = Some(spiral_index_build_range_scan);
    (*routine).index_validate_scan = Some(spiral_index_validate_scan);
    (*routine).scan_bitmap_next_tuple = Some(spiral_scan_bitmap_next_tuple);
    (*routine).scan_sample_next_block = Some(spiral_scan_sample_next_block);
    (*routine).scan_sample_next_tuple = Some(spiral_scan_sample_next_tuple);

    pgrx::datum::Internal::from(Some(pg_sys::Datum::from(routine as usize)))
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_needs_toast_table(_rel: pg_sys::Relation) -> bool {
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_nontransactional_truncate(rel: pg_sys::Relation) {
    if !rel.is_null() {
        pg_sys::RelationTruncate(rel, 0);
    }
}

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
    warning!("Spiral: CLUSTER is currently a no-op for Spiral TAM relations.");
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_fetch_row_version(
    rel: pg_sys::Relation,
    tid: pg_sys::ItemPointer,
    _snapshot: pg_sys::Snapshot,
    slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    if tid.is_null() || slot.is_null() {
        return false;
    }

    let blkno = pg_sys::ItemPointerGetBlockNumber(tid);
    let posid = pg_sys::ItemPointerGetOffsetNumber(tid);

    if posid < 1 || posid > crate::storage::DATA_PER_PAGE as u16 {
        return false;
    }

    let buffer = pg_sys::ReadBuffer(rel, blkno);
    if buffer == 0 {
        return false;
    }

    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
    let page = pg_sys::BufferGetPage(buffer);

    let mut found = false;
    if crate::storage::is_valid_spiral_page(page) {
        let offset = (crate::storage::HEADER_SIZE as u32) + (posid as u32 - 1) * 8;
        let ptr = (page as *const u8).add(offset as usize);
        let val = *(ptr as *const f64);

        if val != 0.0 {
            let mut tenant_scale = 1024;
            let rel_name_ptr = pg_sys::get_rel_name((*rel).rd_id);
            if !rel_name_ptr.is_null() {
                let rel_name = std::ffi::CStr::from_ptr(rel_name_ptr).to_string_lossy();
                if let Some(m) = crate::catalog::get_metadata(&rel_name) {
                    tenant_scale = crate::catalog::get_tenant_scale(&m);
                }
            }

            let idx = (blkno as i64 * crate::storage::DATA_PER_PAGE as i64) + (posid as i64 - 1);
            let kickoff = crate::get_kickoff_epoch();
            let t = (idx / tenant_scale) + kickoff;
            let tenant_id = (idx % tenant_scale) as i32;

            pg_sys::ExecClearTuple(slot);
            let values = (*slot).tts_values;
            let isnull = (*slot).tts_isnull;

            if !values.is_null() && !isnull.is_null() {
                *values.add(0) = t.into_datum().unwrap();
                *isnull.add(0) = false;
                *values.add(1) = tenant_id.into_datum().unwrap();
                *isnull.add(1) = false;
                *values.add(2) = val.into_datum().unwrap();
                *isnull.add(2) = false;

                pg_sys::ItemPointerCopy(tid, &mut (*slot).tts_tid);
                pg_sys::ExecStoreVirtualTuple(slot);
                found = true;
            }
        }
    }

    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
    pg_sys::ReleaseBuffer(buffer);

    found
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_satisfies_snapshot(
    _rel: pg_sys::Relation,
    _slot: *mut pg_sys::TupleTableSlot,
    _snapshot: pg_sys::Snapshot,
) -> bool {
    // We don't implement MVCC yet, so all non-zero tuples satisfy all snapshots.
    true
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_tid_valid(
    scan: pg_sys::TableScanDesc,
    tid: pg_sys::ItemPointer,
) -> bool {
    if scan.is_null() || tid.is_null() {
        return false;
    }
    let rel = (*scan).rs_rd;
    let blkno = pg_sys::ItemPointerGetBlockNumber(tid);
    let posid = pg_sys::ItemPointerGetOffsetNumber(tid);

    let nblocks =
        unsafe { pg_sys::RelationGetNumberOfBlocksInFork(rel, pg_sys::ForkNumber::MAIN_FORKNUM) };

    blkno < nblocks && posid >= 1 && posid <= crate::storage::DATA_PER_PAGE as u16
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_get_latest_tid(
    _scan: pg_sys::TableScanDesc,
    _tid: pg_sys::ItemPointer,
) {
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
        if !_freeze_xid.is_null() {
            *_freeze_xid = pg_sys::TransactionId::from(0);
        }
        if !_multi_xid.is_null() {
            *_multi_xid = pg_sys::MultiXactId::from(0);
        }
        pg_sys::RelationCreateStorage(*_newrlocator, _persistence, true);
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_copy_data(
    _rel: pg_sys::Relation,
    _newrlocator: *const pg_sys::RelFileLocator,
) {
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
    fork_number: pg_sys::ForkNumber::Type,
) -> u64 {
    if rel.is_null() {
        return 0;
    }
    let smgr = pg_sys::RelationGetSmgr(rel);
    let nblocks = pg_sys::smgrnblocks(smgr, fork_number);
    (nblocks as u64) * (pg_sys::BLCKSZ as u64)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_estimate_size(
    rel: pg_sys::Relation,
    attr_widths: *mut i32,
    pages: *mut pg_sys::BlockNumber,
    tuples: *mut f64,
    allvisfrac: *mut f64,
) {
    if rel.is_null() {
        return;
    }

    let nblocks = pg_sys::RelationGetNumberOfBlocksInFork(rel, pg_sys::ForkNumber::MAIN_FORKNUM);
    *pages = nblocks;

    // Each block is full (mathematically mapped)
    *tuples = (nblocks as f64) * (crate::storage::DATA_PER_PAGE as f64);

    if !allvisfrac.is_null() {
        *allvisfrac = 1.0; // Everything is always visible in Spiral
    }

    if !attr_widths.is_null() {
        let tupdesc = (*rel).rd_att;
        for i in 0..(*tupdesc).natts {
            let attr = pg_sys::TupleDescAttr(tupdesc, i as i32);
            *attr_widths.add(i as usize) =
                pg_sys::get_typavgwidth((*attr).atttypid, (*attr).atttypmod);
        }
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_slot_insert(
    rel: pg_sys::Relation,
    slot: *mut pg_sys::TupleTableSlot,
    _cid: pg_sys::CommandId,
    _options: i32,
    _state: *mut pg_sys::BulkInsertStateData,
) {
    if rel.is_null() || slot.is_null() {
        return;
    }

    let tupdesc = (*slot).tts_tupleDescriptor;
    let mut t_val: Option<i64> = None;
    let mut tenant_id: Option<i32> = None;
    let mut value: Option<f64> = None;

    pg_sys::slot_getallattrs(slot);
    for i in 0..(*tupdesc).natts {
        let name_ptr = pg_sys::get_attname((*rel).rd_id, (i + 1) as i16, true);
        if name_ptr.is_null() {
            continue;
        }
        let name = CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
        warning!("Spiral: found column '{}'", name);
        let is_null = *(*slot).tts_isnull.add(i as usize);
        if is_null {
            continue;
        }
        let datum = *(*slot).tts_values.add(i as usize);

        match name.as_str() {
            "t" => {
                t_val = Some(i64::from_datum(datum, false).unwrap());
            }
            "tenant_id" | "sensor_id" | "symbol_id" => {
                tenant_id = Some(i32::from_datum(datum, false).unwrap());
            }
            "value" | "price" | "reading" | "val" => {
                value = Some(f64::from_datum(datum, false).unwrap());
            }
            _ => {}
        }
    }

    if let (Some(t), Some(tid), Some(v)) = (t_val, tenant_id, value) {
        let mut tenant_scale = 1024;
        let rel_oid = (*rel).rd_id;

        unsafe {
            pg_sys::RelationGetSmgr(rel);
        }

        let relname_ptr = pg_sys::get_rel_name(rel_oid);
        if !relname_ptr.is_null() {
            let name = CStr::from_ptr(relname_ptr).to_string_lossy().into_owned();
            if let Some(m) = crate::catalog::get_metadata(&name) {
                tenant_scale = crate::catalog::get_tenant_scale(&m);
            }
        }

        let kickoff = crate::get_kickoff_epoch();
        let t_rel = t - kickoff;

        if t_rel >= 0 && (0..tenant_scale).contains(&(tid as i64)) {
            let index = (t_rel * tenant_scale) + (tid as i64);
            let (blkno, page_offset) = crate::storage::logical_to_physical_offset(index);

            let smgr = pg_sys::RelationGetSmgr(rel);
            let mut nblocks = pg_sys::smgrnblocks(smgr, pg_sys::ForkNumber::MAIN_FORKNUM);

            // 1. Initialize missing pages
            while nblocks <= blkno {
                let buffer = pg_sys::ReadBuffer(rel, pg_sys::InvalidBlockNumber);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);
                let page = pg_sys::BufferGetPage(buffer);
                crate::storage::initialize_spiral_page(page, tenant_scale as i32);
                pg_sys::MarkBufferDirty(buffer);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
                nblocks += 1;
            }

            // 2. Perform point insertion
            let buffer = pg_sys::ReadBuffer(rel, blkno);
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);

            let page = pg_sys::BufferGetPage(buffer);
            let ptr = (page as *mut u8).add(page_offset as usize);
            *(ptr as *mut f64) = v;

            pg_sys::ItemPointerSet(
                &mut (*slot).tts_tid,
                blkno,
                ((page_offset - crate::storage::HEADER_SIZE as u32) / 8 + 1) as u16,
            );

            pg_sys::MarkBufferDirty(buffer);
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
            pg_sys::ReleaseBuffer(buffer);
        }
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_insert_speculative(
    rel: pg_sys::Relation,
    slot: *mut pg_sys::TupleTableSlot,
    cid: pg_sys::CommandId,
    options: i32,
    state: *mut pg_sys::BulkInsertStateData,
    _spec_token: u32,
) {
    spiral_slot_insert(rel, slot, cid, options, state);
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_complete_speculative(
    _rel: pg_sys::Relation,
    _slot: *mut pg_sys::TupleTableSlot,
    _spec_token: u32,
    _succeeded: bool,
) {
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_multi_insert(
    rel: pg_sys::Relation,
    slots: *mut *mut pg_sys::TupleTableSlot,
    nslots: i32,
    cid: pg_sys::CommandId,
    options: i32,
    state: *mut pg_sys::BulkInsertStateData,
) {
    for idx in 0..nslots {
        let slot = unsafe { *slots.add(idx as usize) };
        spiral_slot_insert(rel, slot, cid, options, state);
    }
}

#[allow(clippy::too_many_arguments)]
#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_delete(
    rel: pg_sys::Relation,
    tid: pg_sys::ItemPointer,
    _cid: pg_sys::CommandId,
    _snapshot: pg_sys::Snapshot,
    _crosscheck: pg_sys::Snapshot,
    _wait: bool,
    _tmfd: *mut pg_sys::TM_FailureData,
    _changing_part: bool,
) -> pg_sys::TM_Result::Type {
    if tid.is_null() {
        return pg_sys::TM_Result::TM_Ok;
    }

    let blkno = pg_sys::ItemPointerGetBlockNumber(tid);
    let posid = pg_sys::ItemPointerGetOffsetNumber(tid); // 1-based

    if posid < 1 || posid > crate::storage::DATA_PER_PAGE as u16 {
        return pg_sys::TM_Result::TM_Ok;
    }

    let buffer = pg_sys::ReadBuffer(rel, blkno);
    if buffer == 0 {
        return pg_sys::TM_Result::TM_Ok;
    }

    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);
    let page = pg_sys::BufferGetPage(buffer);

    if crate::storage::is_valid_spiral_page(page) {
        let offset = (crate::storage::HEADER_SIZE as u32) + (posid as u32 - 1) * 8;
        let ptr = (page as *mut u8).add(offset as usize);
        *(ptr as *mut f64) = 0.0;
        pg_sys::MarkBufferDirty(buffer);
    }

    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
    pg_sys::ReleaseBuffer(buffer);

    pg_sys::TM_Result::TM_Ok
}

#[allow(clippy::too_many_arguments)]
#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_update(
    rel: pg_sys::Relation,
    otid: pg_sys::ItemPointer,
    slot: *mut pg_sys::TupleTableSlot,
    cid: pg_sys::CommandId,
    _snapshot: pg_sys::Snapshot,
    _crosscheck: pg_sys::Snapshot,
    _wait: bool,
    _tmfd: *mut pg_sys::TM_FailureData,
    _lockmode: *mut pg_sys::LockTupleMode::Type,
    _update_indexes: *mut pg_sys::TU_UpdateIndexes::Type,
) -> pg_sys::TM_Result::Type {
    // 1. Delete old
    spiral_tuple_delete(
        rel,
        otid,
        cid,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        true,
        std::ptr::null_mut(),
        false,
    );

    // 2. Insert new
    spiral_slot_insert(rel, slot, cid, 0, std::ptr::null_mut());

    pg_sys::TM_Result::TM_Ok
}

#[allow(clippy::too_many_arguments)]
#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_tuple_lock(
    _rel: pg_sys::Relation,
    _tid: pg_sys::ItemPointer,
    _snapshot: pg_sys::Snapshot,
    _slot: *mut pg_sys::TupleTableSlot,
    _cid: pg_sys::CommandId,
    _mode: pg_sys::LockTupleMode::Type,
    _wait_policy: pg_sys::LockWaitPolicy::Type,
    _flags: u8,
    _tmfd: *mut pg_sys::TM_FailureData,
) -> pg_sys::TM_Result::Type {
    pg_sys::TM_Result::TM_Ok
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_finish_bulk_insert(_rel: pg_sys::Relation, _options: i32) {}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_parallelscan_estimate(
    rel: pg_sys::Relation,
) -> pg_sys::Size {
    pg_sys::table_block_parallelscan_estimate(rel)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_parallelscan_initialize(
    rel: pg_sys::Relation,
    pscan: pg_sys::ParallelTableScanDesc,
) -> pg_sys::Size {
    pg_sys::table_block_parallelscan_initialize(rel, pscan);
    std::mem::size_of::<pg_sys::ParallelBlockTableScanDescData>()
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_parallelscan_reinitialize(
    rel: pg_sys::Relation,
    pscan: pg_sys::ParallelTableScanDesc,
) {
    pg_sys::table_block_parallelscan_reinitialize(rel, pscan);
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_fetch_begin(
    rel: pg_sys::Relation,
) -> *mut pg_sys::IndexFetchTableData {
    let data = pg_sys::palloc0(std::mem::size_of::<pg_sys::IndexFetchTableData>())
        as *mut pg_sys::IndexFetchTableData;
    (*data).rel = rel;
    data
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_fetch_reset(_data: *mut pg_sys::IndexFetchTableData) {}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_fetch_end(data: *mut pg_sys::IndexFetchTableData) {
    if !data.is_null() {
        pg_sys::pfree(data as *mut std::ffi::c_void);
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_fetch_tuple(
    data: *mut pg_sys::IndexFetchTableData,
    tid: pg_sys::ItemPointer,
    snapshot: pg_sys::Snapshot,
    slot: *mut pg_sys::TupleTableSlot,
    call_again: *mut bool,
    all_dead: *mut bool,
) -> bool {
    if data.is_null() || tid.is_null() || slot.is_null() {
        return false;
    }

    if !call_again.is_null() {
        *call_again = false;
    }
    if !all_dead.is_null() {
        *all_dead = false;
    }

    spiral_tuple_fetch_row_version((*data).rel, tid, snapshot, slot)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_delete_tuples(
    _rel: pg_sys::Relation,
    _delstate: *mut pg_sys::TM_IndexDeleteOp,
) -> pg_sys::TransactionId {
    pg_sys::TransactionId::from(0)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_vacuum(
    rel: pg_sys::Relation,
    _params: *mut pg_sys::VacuumParams,
    _bstrategy: pg_sys::BufferAccessStrategy,
) {
    if rel.is_null() {
        return;
    }

    let mut n_pages: pg_sys::BlockNumber = 0;
    let mut n_tuples: f64 = 0.0;

    unsafe {
        pg_sys::RelationGetSmgr(rel);
        if !(*rel).rd_smgr.is_null() {
            n_pages =
                pg_sys::RelationGetNumberOfBlocksInFork(rel, pg_sys::ForkNumber::MAIN_FORKNUM);
        }
    }

    for blkno in 0..n_pages {
        let buffer = unsafe { pg_sys::ReadBuffer(rel, blkno) };
        if buffer == 0 {
            continue;
        }

        unsafe {
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
            let page = pg_sys::BufferGetPage(buffer);

            if crate::storage::is_valid_spiral_page(page) {
                let upper_bound = (crate::storage::BLCKSZ - crate::storage::SPECIAL_SIZE) as u32;
                let mut offset = crate::storage::HEADER_SIZE as u32;
                while offset + 8 <= upper_bound {
                    let ptr = (page as *const u8).add(offset as usize);
                    let val = *(ptr as *const f64);
                    if val != 0.0 {
                        n_tuples += 1.0;
                    }
                    offset += 8;
                }
            }

            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
            pg_sys::ReleaseBuffer(buffer);
        }
    }

    let mut frozenxid_updated = false;
    let mut minmulti_updated = false;

    unsafe {
        pg_sys::vac_update_relstats(
            rel,
            n_pages,
            n_tuples,
            0,
            0,
            false,
            pg_sys::InvalidTransactionId,
            0.into(), // InvalidMultiXactId is 0
            &mut frozenxid_updated,
            &mut minmulti_updated,
            false,
        );
    }

    info!(
        "Spiral: VACUUM finished for '{}'. Pages: {}, Tuples: {}",
        unsafe { std::ffi::CStr::from_ptr(pg_sys::get_rel_name((*rel).rd_id)).to_string_lossy() },
        n_pages,
        n_tuples
    );
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_toast_am(_rel: pg_sys::Relation) -> pg_sys::Oid {
    pg_sys::InvalidOid
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_relation_fetch_toast_slice(
    _toastrel: pg_sys::Relation,
    _valueid: pg_sys::Oid,
    _attrsize: i32,
    _sliceoffset: i32,
    _slicelength: i32,
    _result: *mut pg_sys::varlena,
) {
}

use std::ffi::CStr;

struct SpiralScanState {
    tenant_scale: i64,
    current_blkno: pg_sys::BlockNumber,
    total_blks: pg_sys::BlockNumber,
    /// First page that could contain data within the query's time range.
    scan_first_blk: pg_sys::BlockNumber,
    /// Exclusive upper page bound for the query's time range.
    scan_last_blk: pg_sys::BlockNumber,
    current_offset_in_page: u32,
    phsw: pg_sys::ParallelBlockTableScanWorkerData,
    read_stream: *mut pg_sys::ReadStream,

    // Bitmap scan state
    tbm_res: pg_sys::TBMIterateResult,
    offsets: [pg_sys::OffsetNumber; 1024], // Max tuples per page
    noffsets: i32,
    curr_offset_idx: i32,
}

#[repr(C)]
struct SpiralScanDescData {
    base: pg_sys::TableScanDescData,
    state: *mut SpiralScanState,
}

unsafe extern "C-unwind" fn spiral_read_stream_next_block(
    _stream: *mut pg_sys::ReadStream,
    callback_private: *mut std::ffi::c_void,
    _per_buffer_data: *mut std::ffi::c_void,
) -> pg_sys::BlockNumber {
    let state = callback_private as *mut SpiralScanState;
    if (*state).current_blkno < (*state).scan_last_blk {
        let blk = (*state).current_blkno;
        (*state).current_blkno += 1;
        blk
    } else {
        pg_sys::InvalidBlockNumber
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_begin(
    rel: pg_sys::Relation,
    snapshot: pg_sys::Snapshot,
    nkeys: ::core::ffi::c_int,
    key: *mut pg_sys::ScanKeyData,
    pscan: pg_sys::ParallelTableScanDesc,
    flags: u32,
) -> pg_sys::TableScanDesc {
    let spiral_scan =
        pg_sys::palloc0(std::mem::size_of::<SpiralScanDescData>()) as *mut SpiralScanDescData;

    let scan = &mut (*spiral_scan).base;
    scan.rs_rd = rel;
    scan.rs_snapshot = snapshot;
    scan.rs_nkeys = nkeys;
    scan.rs_key = key;
    scan.rs_parallel = pscan;
    scan.rs_flags = flags;

    let oid = (*rel).rd_id;
    let mut tenant_scale = 1024;
    let relname_ptr = pg_sys::get_rel_name(oid);
    if !relname_ptr.is_null() {
        let name = CStr::from_ptr(relname_ptr).to_string_lossy().into_owned();
        if let Some(m) = crate::catalog::get_metadata(&name) {
            tenant_scale = crate::catalog::get_tenant_scale(&m);
        }
    }

    let smgr = pg_sys::RelationGetSmgr(rel);
    let total_blks = pg_sys::smgrnblocks(smgr, pg_sys::ForkNumber::MAIN_FORKNUM);

    // Consume the time range set by the planner hook (None → full scan).
    let time_range = crate::SCAN_TIME_RANGE.with(|r| r.take());
    let (scan_first_blk, scan_last_blk) = if let Some((ts, te)) = time_range {
        let dpg = crate::storage::DATA_PER_PAGE as i64;
        let first = (ts.saturating_mul(tenant_scale) / dpg).max(0) as u32;
        let last = ((te.saturating_mul(tenant_scale) / dpg) + 1)
            .min(total_blks as i64)
            .max(0) as u32;
        (first, last)
    } else {
        (0, total_blks)
    };

    let state = pg_sys::palloc0(std::mem::size_of::<SpiralScanState>()) as *mut SpiralScanState;
    (*state).tenant_scale = tenant_scale;
    (*state).current_blkno = scan_first_blk;
    (*state).total_blks = total_blks;
    (*state).scan_first_blk = scan_first_blk;
    (*state).scan_last_blk = scan_last_blk;
    (*state).current_offset_in_page = crate::storage::HEADER_SIZE as u32;
    (*state).curr_offset_idx = 0;
    (*state).noffsets = 0;

    if pscan.is_null() {
        // Sequential scan: use relation stream with callback
        let stream = pg_sys::read_stream_begin_relation(
            pg_sys::READ_STREAM_SEQUENTIAL as i32,
            std::ptr::null_mut(),
            rel,
            pg_sys::ForkNumber::MAIN_FORKNUM,
            Some(spiral_read_stream_next_block),
            state as *mut std::ffi::c_void,
            0,
        );
        (*state).read_stream = stream;
    }

    (*spiral_scan).state = state;

    if !pscan.is_null() {
        pg_sys::table_block_parallelscan_startblock_init(
            rel,
            &mut (*state).phsw as *mut pg_sys::ParallelBlockTableScanWorkerData,
            pscan as *mut pg_sys::ParallelBlockTableScanDescData,
        );
    }

    &mut (*spiral_scan).base as pg_sys::TableScanDesc
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_getnextslot(
    scan: pg_sys::TableScanDesc,
    _direction: i32,
    slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    if scan.is_null() || slot.is_null() {
        return false;
    }

    let spiral_scan = scan as *mut SpiralScanDescData;
    if (*spiral_scan).state.is_null() {
        return false;
    }

    let state = &mut *(*spiral_scan).state;
    let rel = (*scan).rs_rd;
    let pscan = (*scan).rs_parallel;

    let blk_limit = state.scan_last_blk.min(state.total_blks);

    loop {
        let buffer = if !state.read_stream.is_null() {
            pg_sys::read_stream_next_buffer(state.read_stream, std::ptr::null_mut())
        } else if !pscan.is_null() {
            if state.current_offset_in_page == crate::storage::HEADER_SIZE as u32 {
                state.current_blkno = pg_sys::table_block_parallelscan_nextpage(
                    rel,
                    &mut state.phsw as *mut pg_sys::ParallelBlockTableScanWorkerData,
                    pscan as *mut pg_sys::ParallelBlockTableScanDescData,
                );
                if state.current_blkno == pg_sys::InvalidBlockNumber {
                    return false;
                }
            }
            pg_sys::ReadBuffer(rel, state.current_blkno)
        } else {
            if state.current_blkno >= blk_limit {
                return false;
            }
            pg_sys::ReadBuffer(rel, state.current_blkno)
        };

        if buffer == 0 {
            return false;
        }

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
        let page = pg_sys::BufferGetPage(buffer);

        if !crate::storage::is_valid_spiral_page(page) {
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
            pg_sys::ReleaseBuffer(buffer);
            if state.read_stream.is_null() && pscan.is_null() {
                state.current_blkno += 1;
            }
            state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
            continue;
        }

        let upper_bound = (crate::storage::BLCKSZ - crate::storage::SPECIAL_SIZE) as u32;
        while state.current_offset_in_page + 8 <= upper_bound {
            let ptr = (page as *const u8).add(state.current_offset_in_page as usize);
            let val = *(ptr as *const f64);
            let offset_in_page = state.current_offset_in_page;
            state.current_offset_in_page += 8;

            if val != 0.0 {
                let items_before = (offset_in_page - crate::storage::HEADER_SIZE as u32) / 8;
                let idx = (pg_sys::BufferGetBlockNumber(buffer) as i64
                    * crate::storage::DATA_PER_PAGE as i64)
                    + items_before as i64;

                let kickoff = crate::get_kickoff_epoch();
                let t = (idx / state.tenant_scale) + kickoff;
                let tenant_id = (idx % state.tenant_scale) as i32;

                pg_sys::ExecClearTuple(slot);
                let values = (*slot).tts_values;
                let isnull = (*slot).tts_isnull;

                if !values.is_null() && !isnull.is_null() {
                    *values.add(0) = t.into_datum().unwrap();
                    *isnull.add(0) = false;
                    *values.add(1) = tenant_id.into_datum().unwrap();
                    *isnull.add(1) = false;
                    *values.add(2) = val.into_datum().unwrap();
                    *isnull.add(2) = false;

                    pg_sys::ItemPointerSet(
                        &mut (*slot).tts_tid,
                        pg_sys::BufferGetBlockNumber(buffer),
                        (items_before + 1) as u16,
                    );
                    pg_sys::ExecStoreVirtualTuple(slot);
                }

                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
                return true;
            }
        }

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);

        if state.read_stream.is_null() && pscan.is_null() {
            state.current_blkno += 1;
        }
        state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_end(scan: pg_sys::TableScanDesc) {
    if !scan.is_null() {
        let spiral_scan = scan as *mut SpiralScanDescData;
        if !(*spiral_scan).state.is_null() {
            let state = &mut *(*spiral_scan).state;
            if !state.read_stream.is_null() {
                pg_sys::read_stream_end(state.read_stream);
                state.read_stream = std::ptr::null_mut();
            }
            pg_sys::pfree((*spiral_scan).state as *mut std::ffi::c_void);
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
            state.current_blkno = state.scan_first_blk;
            state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
            state.curr_offset_idx = 0;
            state.noffsets = 0;

            if !state.read_stream.is_null() {

                pg_sys::read_stream_reset(state.read_stream);
            }
        }
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_set_tidrange(
    _scan: pg_sys::TableScanDesc,
    _mintid: pg_sys::ItemPointer,
    _maxtid: pg_sys::ItemPointer,
) {
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_getnextslot_tidrange(
    scan: pg_sys::TableScanDesc,
    direction: pg_sys::ScanDirection::Type,
    slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    spiral_scan_getnextslot(scan, direction, slot)
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_analyze_next_block(
    scan: pg_sys::TableScanDesc,
    _stream: *mut pg_sys::ReadStream,
) -> bool {
    let spiral_scan = scan as *mut SpiralScanDescData;
    let state = &mut *(*spiral_scan).state;

    state.current_blkno < state.total_blks
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_analyze_next_tuple(
    scan: pg_sys::TableScanDesc,
    _oldest_xmin: pg_sys::TransactionId,
    liverows: *mut f64,
    _deadrows: *mut f64,
    slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    let spiral_scan = scan as *mut SpiralScanDescData;
    let state = &mut *(*spiral_scan).state;
    let rel = (*scan).rs_rd;

    while state.current_blkno < state.total_blks {
        let buffer = pg_sys::ReadBuffer(rel, state.current_blkno);
        if buffer == 0 {
            state.current_blkno += 1;
            state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
            continue;
        }

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
        let page = pg_sys::BufferGetPage(buffer);

        if !crate::storage::is_valid_spiral_page(page) {
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
            pg_sys::ReleaseBuffer(buffer);
            state.current_blkno += 1;
            state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;
            continue;
        }

        let upper_bound = (crate::storage::BLCKSZ - crate::storage::SPECIAL_SIZE) as u32;
        while state.current_offset_in_page + 8 <= upper_bound {
            let ptr = (page as *const u8).add(state.current_offset_in_page as usize);
            let val = *(ptr as *const f64);
            state.current_offset_in_page += 8;

            if val != 0.0 {
                let items_before =
                    (state.current_offset_in_page - 8 - crate::storage::HEADER_SIZE as u32) / 8;
                let idx = (state.current_blkno as i64 * crate::storage::DATA_PER_PAGE as i64)
                    + items_before as i64;

                let kickoff = crate::get_kickoff_epoch();
                let t = (idx / state.tenant_scale) + kickoff;
                let tenant_id = (idx % state.tenant_scale) as i32;

                pg_sys::ExecClearTuple(slot);
                let values = (*slot).tts_values;
                let isnull = (*slot).tts_isnull;

                if !values.is_null() && !isnull.is_null() {
                    *values.add(0) = t.into_datum().unwrap();
                    *isnull.add(0) = false;
                    *values.add(1) = tenant_id.into_datum().unwrap();
                    *isnull.add(1) = false;
                    *values.add(2) = val.into_datum().unwrap();
                    *isnull.add(2) = false;
                    pg_sys::ExecStoreVirtualTuple(slot);
                }

                if !liverows.is_null() {
                    *liverows += 1.0;
                }

                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
                return true;
            }
        }

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);
        state.current_blkno += 1;
        state.current_offset_in_page = crate::storage::HEADER_SIZE as u32;

        return false;
    }
    false
}

#[allow(clippy::too_many_arguments)]
#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_build_range_scan(
    table_rel: pg_sys::Relation,
    index_rel: pg_sys::Relation,
    index_info: *mut pg_sys::IndexInfo,
    _allow_sync: bool,
    _anyvisible: bool,
    _progress: bool,
    _start_blockno: pg_sys::BlockNumber,
    _numblocks: pg_sys::BlockNumber,
    callback: pg_sys::IndexBuildCallback,
    callback_state: *mut std::ffi::c_void,
    scan: pg_sys::TableScanDesc,
) -> f64 {
    let mut reltuples = 0.0;

    let estate = pg_sys::CreateExecutorState();
    let slot = pg_sys::table_slot_create(table_rel, std::ptr::null_mut());

    let my_scan = if scan.is_null() {
        pg_sys::table_beginscan_strat(
            table_rel,
            pg_sys::GetActiveSnapshot(),
            0,
            std::ptr::null_mut(),
            true,
            _allow_sync,
        )
    } else {
        scan
    };

    let num_keys = (*index_info).ii_NumIndexKeyAttrs as usize;
    let values =
        pg_sys::palloc0(std::mem::size_of::<pg_sys::Datum>() * num_keys) as *mut pg_sys::Datum;
    let isnull = pg_sys::palloc0(std::mem::size_of::<bool>() * num_keys) as *mut bool;

    while pg_sys::table_scan_getnextslot(my_scan, pg_sys::ScanDirection::ForwardScanDirection, slot)
    {
        pg_sys::FormIndexDatum(index_info, slot, estate, values, isnull);

        if let Some(cb) = callback {
            cb(
                index_rel,
                &mut (*slot).tts_tid,
                values,
                isnull,
                true,
                callback_state,
            );
        }
        reltuples += 1.0;
    }

    pg_sys::pfree(values as *mut std::ffi::c_void);
    pg_sys::pfree(isnull as *mut std::ffi::c_void);

    if scan.is_null() {
        pg_sys::table_endscan(my_scan);
    }

    pg_sys::ExecDropSingleTupleTableSlot(slot);
    pg_sys::FreeExecutorState(estate);

    reltuples
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_index_validate_scan(
    _table_rel: pg_sys::Relation,
    _index_rel: pg_sys::Relation,
    _index_info: *mut pg_sys::IndexInfo,
    _snapshot: pg_sys::Snapshot,
    _state: *mut pg_sys::ValidateIndexState,
) {
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_bitmap_next_tuple(
    scan: pg_sys::TableScanDesc,
    slot: *mut pg_sys::TupleTableSlot,
    recheck: *mut bool,
    lossy_pages: *mut u64,
    exact_pages: *mut u64,
) -> bool {
    if scan.is_null() || slot.is_null() {
        return false;
    }

    let spiral_scan = scan as *mut SpiralScanDescData;
    let state = &mut *(*spiral_scan).state;
    let rel = (*scan).rs_rd;

    // Use the iterator from the scan descriptor
    let tbm_iterator = &mut (*scan).st.rs_tbmiterator;

    loop {
        // 1. Move to next offset if we have some left in the current page
        if state.curr_offset_idx < state.noffsets {
            let posid = if state.tbm_res.lossy {
                (state.curr_offset_idx + 1) as u16
            } else {
                state.offsets[state.curr_offset_idx as usize]
            };
            state.curr_offset_idx += 1;

            let tid = pg_sys::ItemPointerData {
                ip_blkid: pg_sys::BlockIdData {
                    bi_hi: (state.tbm_res.blockno >> 16) as u16,
                    bi_lo: (state.tbm_res.blockno & 0xffff) as u16,
                },
                ip_posid: posid,
            };

            // Fetch and reconstruct the tuple
            if spiral_tuple_fetch_row_version(
                rel,
                &tid as *const _ as *mut _,
                std::ptr::null_mut(),
                slot,
            ) {
                if !recheck.is_null() {
                    *recheck = state.tbm_res.recheck || state.tbm_res.lossy;
                }
                return true;
            }
            continue;
        }

        // 2. No more offsets in current page, get next page from bitmap
        if !pg_sys::tbm_iterate(tbm_iterator, &mut state.tbm_res) {
            return false;
        }

        state.curr_offset_idx = 0;
        if state.tbm_res.lossy {
            state.noffsets = crate::storage::DATA_PER_PAGE as i32;
            if !lossy_pages.is_null() {
                *lossy_pages += 1;
            }
        } else {
            state.noffsets = pg_sys::tbm_extract_page_tuple(
                &mut state.tbm_res,
                state.offsets.as_mut_ptr(),
                state.offsets.len() as u32,
            );
            if !exact_pages.is_null() {
                *exact_pages += 1;
            }
        }
    }
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_sample_next_block(
    _scan: pg_sys::TableScanDesc,
    _scanstate: *mut pg_sys::SampleScanState,
) -> bool {
    false
}

#[pg_guard]
pub unsafe extern "C-unwind" fn spiral_scan_sample_next_tuple(
    _scan: pg_sys::TableScanDesc,
    _scanstate: *mut pg_sys::SampleScanState,
    _slot: *mut pg_sys::TupleTableSlot,
) -> bool {
    false
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_tam_delete() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_delete_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_delete_test (t, tenant_id, value) VALUES (1, 1, 42.0), (2, 2, 84.0);", None, &[])?;

            let count = Spi::get_one::<i64>("SELECT count(*) FROM tam_delete_test").unwrap().unwrap();
            assert_eq!(count, 2);

            client.update("DELETE FROM tam_delete_test WHERE t = 1;", None, &[])?;

            let count_after = Spi::get_one::<i64>("SELECT count(*) FROM tam_delete_test").unwrap().unwrap();
            assert_eq!(count_after, 1);

            let val = Spi::get_one::<f64>("SELECT value FROM tam_delete_test").unwrap().unwrap();
            assert_eq!(val, 84.0);

            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_update() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_update_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_update_test (t, tenant_id, value) VALUES (1, 1, 10.0);", None, &[])?;

            // Update value only (same TID)
            client.update("UPDATE tam_update_test SET value = 20.0 WHERE t = 1;", None, &[])?;
            let val = Spi::get_one::<f64>("SELECT value FROM tam_update_test").unwrap().unwrap();
            assert_eq!(val, 20.0);

            // Update key (t) - should move it
            client.update("UPDATE tam_update_test SET t = 2 WHERE t = 1;", None, &[])?;
            let t_new = Spi::get_one::<i64>("SELECT t FROM tam_update_test").unwrap().unwrap();
            assert_eq!(t_new, 2);

            let count = Spi::get_one::<i64>("SELECT count(*) FROM tam_update_test").unwrap().unwrap();
            assert_eq!(count, 1);

            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_truncate() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_truncate_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_truncate_test (t, tenant_id, value) VALUES (1, 1, 10.0), (2, 2, 20.0);", None, &[])?;

            client.update("TRUNCATE tam_truncate_test;", None, &[])?;

            let count = Spi::get_one::<i64>("SELECT count(*) FROM tam_truncate_test").unwrap().unwrap();
            assert_eq!(count, 0);

            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_create_index() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_index_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_index_test (t, tenant_id, value) VALUES (1, 1, 10.0), (2, 2, 20.0), (3, 3, 30.0);", None, &[])?;

            // Should now work!
            client.update("CREATE INDEX idx_tam_value ON tam_index_test(value);", None, &[])?;

            // Verify index usage
            client.update("SET enable_seqscan = off;", None, &[])?;
            let val = Spi::get_one::<f64>("SELECT value FROM tam_index_test WHERE value = 20.0").unwrap().unwrap();
            assert_eq!(val, 20.0);

            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_parallel_scan() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_parallel_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;

            // Insert enough data to justify parallel scan (though we'll force it)
            client.update("INSERT INTO tam_parallel_test (t, tenant_id, value)
                           SELECT i, 1, i::double precision FROM generate_series(1, 1000) i;", None, &[])?;

            // Force parallel scan (PG18 uses debug_parallel_query)
            client.update("SET debug_parallel_query = on;", None, &[])?;
            client.update("SET max_parallel_workers_per_gather = 2;", None, &[])?;
            client.update("SET min_parallel_table_scan_size = 0;", None, &[])?;

            let count = Spi::get_one::<i64>("SELECT count(*) FROM tam_parallel_test").unwrap().unwrap();
            assert_eq!(count, 1000);

            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_insert_functional() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_test_functional (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_test_functional (t, tenant_id, value) VALUES (1, 1, 42.0);", None, &[])?;
            let count = Spi::get_one::<i64>("SELECT count(*) FROM tam_test_functional").unwrap().unwrap();
            assert_eq!(count, 1);

            let val = Spi::get_one::<f64>("SELECT value FROM tam_test_functional LIMIT 1").unwrap().unwrap();
            assert_eq!(val, 42.0);
            Ok::<(), spi::Error>(())
        }).unwrap();
    }

    #[pg_test]
    fn test_tam_analyze() {
        Spi::connect_mut(|client| {
            client.update("SET spiral.kickoff_date = '1970-01-01 00:00:00Z';", None, &[])?;
            client.update("CREATE TABLE tam_analyze_test (t bigint, tenant_id int, value double precision) USING spiral;", None, &[])?;
            client.update("INSERT INTO tam_analyze_test (t, tenant_id, value) VALUES (0, 0, 10.0), (0, 1, 20.0), (0, 2, 30.0);", None, &[])?;

            client.update("ANALYZE tam_analyze_test;", None, &[])?;

            // Check pg_stats to verify that ANALYZE successfully scanned the columns
            let row_count = Spi::get_one::<i64>("SELECT count(*) FROM pg_stats WHERE tablename = 'tam_analyze_test'").unwrap().unwrap();
            assert!(row_count > 0);

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

            Ok::<(), spi::Error>(())
        }).unwrap();
    }
}
