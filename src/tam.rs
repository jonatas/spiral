use pgrx::pg_sys;
use pgrx::prelude::*;

// Table Access Method (TAM) Handler for Spiral
#[pg_extern(sql = "
        CREATE FUNCTION spiral_tam_handler(internal) RETURNS table_am_handler LANGUAGE c AS 'MODULE_PATHNAME', 'spiral_tam_handler_wrapper' STRICT;
        CREATE ACCESS METHOD spiral TYPE TABLE HANDLER spiral_tam_handler;
    ")]
/// # Safety
/// This function is unsafe because it interacts with PostgreSQL C internals.
pub unsafe fn spiral_tam_handler(_fcinfo: pg_sys::FunctionCallInfo) -> pgrx::datum::Internal {
    let routine =
        pgrx::PgMemoryContexts::TopMemoryContext.palloc_struct::<pg_sys::TableAmRoutine>();

    (*routine).type_ = pg_sys::NodeTag::T_TableAmRoutine;

    // Wire up the O(1) logic callbacks
    (*routine).tuple_insert = Some(spiral_slot_insert);
    (*routine).scan_begin = Some(spiral_scan_begin);
    (*routine).scan_getnextslot = Some(spiral_scan_getnextslot);
    (*routine).scan_end = Some(spiral_scan_end);
    (*routine).scan_rescan = Some(spiral_scan_rescan);

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
    info!(
        "Spiral TAM: Routing insert to Delta Store for relation OID {}",
        (*rel).rd_id
    );
}

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Seek};

struct SpiralScanState {
    file: File,
    tenant_scale: i64,
    current_index: u64,
    total_slots: u64,
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
    let spiral_scan = pgrx::pg_sys::palloc0(std::mem::size_of::<SpiralScanDescData>()) as *mut SpiralScanDescData;
    let scan = spiral_scan as pg_sys::TableScanDesc;
    if !scan.is_null() {
        (*scan).rs_rd = rel;
        (*scan).rs_snapshot = snapshot;

        let oid = (*rel).rd_id;
        let mut path = std::path::PathBuf::from("/tmp/spiral_main/");
        path.push(format!("{}.bin", oid));

        let mut tenant_scale = 1024;
        let relname_ptr = pg_sys::get_rel_name(oid);
        if !relname_ptr.is_null() {
            let name = CStr::from_ptr(relname_ptr).to_string_lossy();
            if let Some(m) = crate::catalog::get_metadata(&name) {
                tenant_scale = crate::catalog::get_tenant_scale(&m);
            }
        }

        if let Ok(file) = File::open(&path) {
            let total_size = file.metadata().map(|m| m.len()).unwrap_or(0);
            let state = Box::new(SpiralScanState {
                file,
                tenant_scale,
                current_index: 0,
                total_slots: total_size / 8,
            });
            (*spiral_scan).state = Box::into_raw(state);
        } else {
            (*spiral_scan).state = std::ptr::null_mut();
        }
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
    let mut buf = [0u8; 8];

    while state.current_index < state.total_slots {
        if state.file.read_exact(&mut buf).is_ok() {
            let val = f64::from_le_bytes(buf);
            let idx = state.current_index;
            state.current_index += 1;

            if val != 0.0 {
                let t = (idx as i64 / state.tenant_scale) as i64;
                let tenant_id = (idx as i64 % state.tenant_scale) as i32;

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
                return true;
            }
        } else {
            break;
        }
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
            state.current_index = 0;
            let _ = state.file.seek(std::io::SeekFrom::Start(0));
        }
    }
}
