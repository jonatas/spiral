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
