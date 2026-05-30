# Table Access Method (TAM) Audit

> **⚠️ EXPERIMENTAL — NOT ACID-SAFE**
>
> The Spiral TAM does **not** implement MVCC, WAL-backed rollback, or snapshot
> isolation. Writes bypass WAL and are immediately visible to all readers regardless
> of transaction state. `ROLLBACK` and `ROLLBACK TO SAVEPOINT` do **not** undo TAM
> writes. Do not use TAM-backed tables in production paths that require transactional
> correctness. See [issue #65](https://github.com/jonatas/spiral/issues/65).
>
> A `WARNING` is emitted on the first TAM write per session by default.
> Set `spiral.warn_on_tam_writes = false` to suppress after acknowledging the limitation.

This document tracks the implementation status of all Table Access Method (TAM) callbacks registered in `src/tam.rs`.

## Core Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `slot_callbacks` | **Implemented** | Returns `&pg_sys::TTSOpsVirtual`. Suitable for our virtual reconstruction. |
| `tuple_insert` | **Implemented** | Extracts `t`, `tenant_id`, and `value` from slot. Mathematical `v = (t * scale) + tid` mapping. |
| `tuple_insert_speculative` | **Placeholder** | No-op. |
| `multi_insert` | **Placeholder** | No-op. |
| `tuple_delete` | **Implemented** | Point-in-place zeroing of data at specified TID. |
| `tuple_update` | **Implemented** | Logical delete of old TID followed by re-insertion of new slot values. |
| `tuple_lock` | **Partial** | Returns `TM_Ok` without performing any locking. |

## Scan Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `scan_begin` | **Implemented** | Initializes `SpiralScanState`. Supports `SCAN_TIME_RANGE`, parallel scans, and **ReadStream prefetching**. |
| `scan_getnextslot` | **Implemented** | Primary read path. Performs O(1) reconstruction. Assigns `tts_tid`. |
| `scan_end` | **Implemented** | Cleans up scan state. |
| `scan_rescan` | **Implemented** | Resets scan pointers. |
| `scan_set_tidrange` | **Placeholder** | No-op. Required for `TID Scan`. |
| `scan_getnextslot_tidrange` | **Partial** | Calls standard `scan_getnextslot`. |
| `scan_bitmap_next_tuple` | **Implemented** | Iterates over `TBMIterator` for Bitmap Heap Scans. |
| `scan_sample_next_block` | **Unsupported** | Returns `false`. Required for `TABLESAMPLE`. |
| `scan_sample_next_tuple` | **Unsupported** | Returns `false`. Required for `TABLESAMPLE`. |

## Parallel Scan Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `parallelscan_estimate` | **Implemented** | Uses `table_block_parallelscan_estimate`. |
| `parallelscan_initialize` | **Implemented** | Uses `table_block_parallelscan_initialize`. |
| `parallelscan_reinitialize` | **Implemented** | Uses `table_block_parallelscan_reinitialize`. |

## Index Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `index_fetch_begin` | **Implemented** | Allocates per-scan state for index-based row retrieval. |
| `index_fetch_reset` | **Implemented** | Resets index fetch state. |
| `index_fetch_end` | **Implemented** | Releases index fetch state. |
| `index_fetch_tuple` | **Implemented** | Uses TID to mathematically reconstruct rows for standard indices. |
| `index_delete_tuples` | **Unsupported** | Returns `0`. |
| `index_build_range_scan` | **Implemented** | Standard index builder. feeds reconstructed tuples to B-Tree builder. |
| `index_validate_scan` | **Placeholder** | No-op. |

## Relation & Lifecycle Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `relation_size` | **Implemented** | Correctly reports MAIN fork size. |
| `relation_estimate_size` | **Implemented** | Uses `table_block_relation_estimate_size` with Spiral-specific widths. |
| `relation_set_new_filelocator` | **Implemented** | Creates physical storage on disk. |
| `relation_nontransactional_truncate`| **Implemented** | Physically truncates relation MAIN fork to 0 blocks. |
| `relation_copy_data` | **Placeholder** | No-op. |
| `relation_copy_for_cluster` | **Placeholder** | No-op. `CLUSTER` will not work. |
| `relation_vacuum` | **Implemented** | Scans all blocks and updates `pg_class` stats (reltuples, relpages). |
| `relation_needs_toast_table` | **Implemented** | Returns `false`. |
| `relation_toast_am` | **Unsupported** | Returns `InvalidOid`. |
| `relation_fetch_toast_slice` | **Placeholder** | No-op. |

## Tuple/MVCC Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `tuple_fetch_row_version` | **Implemented** | Reconstructs tuple from TID and physical block. |
| `tuple_tid_valid` | **Implemented** | Returns `true` if position is within Spiral page limits. |
| `tuple_get_latest_tid` | **Placeholder** | No-op. |
| `tuple_satisfies_snapshot` | **Implemented** | Snapshot-aware: returns `false` for slots whose writing xid is still in-progress per the MVCC snapshot. Backed by a per-backend pending-writes map registered via `RegisterXactCallback`; aborts restore old values, commits clear the map. |
| `scan_analyze_next_block` | **Implemented** | Returns `true` if blocks remain. |
| `scan_analyze_next_tuple` | **Implemented** | Full block/tuple sampling for statistical analysis. |

## Summary of Missing Semantics

1.  **MVCC (partial)**: Snapshot isolation is implemented via an in-memory pending-writes map (`src/mvcc.rs`). Uncommitted inserts/deletes are hidden from concurrent snapshots and rolled back on abort. Limitation: the undo log is in-memory only — a crash during a write transaction leaves stale data (WAL-based undo is a future work item). Tuple-level locking remains a no-op (`TM_Ok` always).
2.  **TABLESAMPLE**: `TABLESAMPLE` clauses are currently unsupported.

## Roadmap

- [x] Implement `tuple_insert` by mapping slot columns to logical offsets.
- [x] Implement `tuple_delete` and `tuple_update`.
- [x] Implement `relation_vacuum` and `ANALYZE` support.
- [x] Implement parallel scan support.
- [x] Implement `index_build_range_scan` and fetch callbacks for standard indices.
- [x] Add `GenericXLog` support to point-in-time operations for durability.
- [x] Implement `scan_bitmap_next_tuple` for Bitmap Scan support.
- [x] Implement snapshot-aware `tuple_satisfies_snapshot` with in-memory undo for rollback (closes #65, partial MVCC).
- [ ] WAL-log individual slot writes so crash recovery can undo in-flight transactions.
