use pgrx::pg_sys;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

type MvccStateMap = HashMap<(u32, u32, u16), (u32, f64)>;

// Per-backend pending writes: (rel_oid, blkno, posid) -> (writing_xid, old_value)
// Populated on every write; cleaned up on commit (drop entries) or abort (restore + drop).
static PENDING_WRITES: OnceLock<Mutex<MvccStateMap>> = OnceLock::new();
static CALLBACK_REGISTERED: OnceLock<()> = OnceLock::new();

fn pending_writes() -> &'static Mutex<MvccStateMap> {
    PENDING_WRITES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register the transaction callback exactly once per backend.
pub unsafe fn ensure_callback_registered() {
    CALLBACK_REGISTERED.get_or_init(|| {
        pg_sys::RegisterXactCallback(Some(spiral_xact_callback), std::ptr::null_mut());
    });
}

unsafe extern "C-unwind" fn spiral_xact_callback(
    event: pg_sys::XactEvent::Type,
    _arg: *mut std::ffi::c_void,
) {
    let is_abort = event == pg_sys::XactEvent::XACT_EVENT_ABORT
        || event == pg_sys::XactEvent::XACT_EVENT_PARALLEL_ABORT;

    let current_xid = pg_sys::GetCurrentTransactionIdIfAny().into_inner();
    if current_xid == 0 {
        return;
    }

    if is_abort {
        // Collect entries to restore, then release the lock before doing buffer I/O.
        let to_restore: Vec<(u32, u32, u16, f64)> = {
            let map = match pending_writes().lock() {
                Ok(m) => m,
                Err(_) => return,
            };
            map.iter()
                .filter(|(_, (xid, _))| *xid == current_xid)
                .map(|((oid, blk, pos), (_, old))| (*oid, *blk, *pos, *old))
                .collect()
        };

        for (rel_oid, blkno, posid, old_val) in to_restore {
            restore_slot(rel_oid, blkno, posid, old_val);
        }
    }

    // Remove all entries for this xid regardless of commit or abort.
    if let Ok(mut map) = pending_writes().lock() {
        map.retain(|_, (xid, _)| *xid != current_xid);
    }
}

unsafe fn restore_slot(rel_oid: u32, blkno: u32, posid: u16, value: f64) {
    let rel = pg_sys::RelationIdGetRelation(pg_sys::Oid::from(rel_oid));
    if rel.is_null() {
        return;
    }

    let buffer = pg_sys::ReadBuffer(rel, blkno);
    if buffer == 0 {
        pg_sys::RelationClose(rel);
        return;
    }

    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);
    let page = pg_sys::BufferGetPage(buffer);

    if crate::storage::is_valid_spiral_page(page) {
        let offset = crate::storage::HEADER_SIZE as u32 + (posid as u32 - 1) * 8;
        let ptr = (page as *mut u8).add(offset as usize);
        *(ptr as *mut f64) = value;
        pg_sys::MarkBufferDirty(buffer);
    }

    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
    pg_sys::ReleaseBuffer(buffer);
    pg_sys::RelationClose(rel);
}

/// Record a slot write before modifying the page. The old_value is used to
/// restore the slot on transaction abort. Uses `or_insert` so that the first
/// write in a transaction sets the true "undo" image even if the slot is
/// written multiple times within the same transaction.
pub fn record_write(rel_oid: u32, blkno: u32, posid: u16, xid: u32, old_value: f64) {
    if let Ok(mut map) = pending_writes().lock() {
        map.entry((rel_oid, blkno, posid))
            .or_insert((xid, old_value));
    }
}

/// Returns `Some(false)` if the slot has a pending write not yet visible in
/// `snapshot`, `None` if no pending write exists (caller should use val != 0
/// check), or `Some(true)` for SNAPSHOT_ANY.
pub unsafe fn check_visibility(
    rel_oid: u32,
    blkno: u32,
    posid: u16,
    snapshot: pg_sys::Snapshot,
) -> Option<bool> {
    if snapshot.is_null() {
        return None;
    }

    let snap_type = (*snapshot).snapshot_type;

    if snap_type == pg_sys::SnapshotType::SNAPSHOT_ANY {
        return Some(true);
    }

    // Only perform XidInMVCCSnapshot check for MVCC snapshots.
    let is_mvcc = snap_type == pg_sys::SnapshotType::SNAPSHOT_MVCC
        || snap_type == pg_sys::SnapshotType::SNAPSHOT_HISTORIC_MVCC;
    if !is_mvcc {
        return None;
    }

    let writing_xid = {
        let map = pending_writes().lock().ok()?;
        map.get(&(rel_oid, blkno, posid)).map(|(xid, _)| *xid)?
    };

    if pg_sys::XidInMVCCSnapshot(pg_sys::TransactionId::from(writing_xid), snapshot) {
        Some(false) // still in-progress from snapshot's perspective
    } else {
        None // committed — fall through to val != 0.0 check
    }
}
