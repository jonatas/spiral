# Table Access Method (TAM) Audit

This document tracks the implementation status of all Table Access Method (TAM) callbacks registered in `src/tam.rs`.

## Core Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `slot_callbacks` | **Implemented** | Returns `&pg_sys::TTSOpsVirtual`. Suitable for our virtual reconstruction. |
| `tuple_insert` | **Partial** | Extracts `t`, `tenant_id`, and `value` from slot. No WAL logging yet. |
| `tuple_insert_speculative` | **Placeholder** | No-op. |
| `multi_insert` | **Placeholder** | No-op. |
| `tuple_delete` | **Partial** | Returns `TM_Ok` but does not actually delete data. |
| `tuple_update` | **Partial** | Returns `TM_Ok` but does not actually update data. |
| `tuple_lock` | **Partial** | Returns `TM_Ok` without performing any locking. |

## Scan Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `scan_begin` | **Implemented** | Initializes `SpiralScanState`. Supports `SCAN_TIME_RANGE` optimization from planner. |
| `scan_getnextslot` | **Implemented** | Primary read path. Performs O(1) reconstruction. |
| `scan_end` | **Implemented** | Cleans up scan state. |
| `scan_rescan` | **Implemented** | Resets scan pointers. |
| `scan_set_tidrange` | **Placeholder** | No-op. Required for `TID Scan`. |
| `scan_getnextslot_tidrange` | **Partial** | Calls standard `scan_getnextslot`. |
| `scan_bitmap_next_tuple` | **Unsupported** | Returns `false`. Required for `Bitmap Scan`. |
| `scan_sample_next_block` | **Unsupported** | Returns `false`. Required for `TABLESAMPLE`. |
| `scan_sample_next_tuple` | **Unsupported** | Returns `false`. Required for `TABLESAMPLE`. |

## Parallel Scan Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `parallelscan_estimate` | **Unsupported** | Returns `0`. |
| `parallelscan_initialize` | **Unsupported** | Returns `0`. |
| `parallelscan_reinitialize` | **Placeholder** | No-op. |

## Index Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `index_fetch_begin` | **Unsupported** | Returns `NULL`. Indices cannot be used to fetch rows from the TAM. |
| `index_fetch_reset` | **Placeholder** | No-op. |
| `index_fetch_end` | **Placeholder** | No-op. |
| `index_fetch_tuple` | **Unsupported** | Returns `false`. |
| `index_delete_tuples` | **Unsupported** | Returns `0`. |
| `index_build_range_scan` | **Unsupported** | Returns `0.0`. Required for `CREATE INDEX`. |
| `index_validate_scan` | **Placeholder** | No-op. |

## Relation & Lifecycle Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `relation_size` | **Implemented** | Correctly reports MAIN fork size. |
| `relation_estimate_size` | **Implemented** | Uses `table_block_relation_estimate_size` with Spiral-specific widths. |
| `relation_set_new_filelocator` | **Implemented** | Creates physical storage on disk. |
| `relation_nontransactional_truncate`| **Placeholder** | No-op. |
| `relation_copy_data` | **Placeholder** | No-op. |
| `relation_copy_for_cluster` | **Placeholder** | No-op. `CLUSTER` will not work. |
| `relation_vacuum` | **Placeholder** | No-op. `VACUUM` will not work. |
| `relation_needs_toast_table` | **Implemented** | Returns `false`. |
| `relation_toast_am` | **Unsupported** | Returns `InvalidOid`. |
| `relation_fetch_toast_slice` | **Placeholder** | No-op. |

## Tuple/MVCC Callbacks

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `tuple_fetch_row_version` | **Unsupported** | Returns `false`. |
| `tuple_tid_valid` | **Unsupported** | Returns `false`. |
| `tuple_get_latest_tid` | **Placeholder** | No-op. |
| `tuple_satisfies_snapshot` | **Unsupported** | Returns `false`. Snapshot isolation is NOT implemented. |
| `scan_analyze_next_block` | **Unsupported** | Returns `false`. |
| `scan_analyze_next_tuple` | **Unsupported** | Returns `false`. |

## Summary of Missing Semantics

1.  **WAL Logging**: Most callbacks are not WAL-logged. Using `GenericXLog` (already used in `src/storage.rs`) would be the correct approach for durability.
2.  **MVCC**: Snapshot isolation is completely absent. The TAM currently reads the "current" state of blocks regardless of the transaction snapshot.
3.  **Indices**: Index-organized access is not supported. Only sequential scans (accelerated by `SCAN_TIME_RANGE` in the planner) work.

## Roadmap

- [x] Implement `tuple_insert` by mapping slot columns to logical offsets.
- [ ] Add `GenericXLog` support to `tuple_insert` for durability.
- [ ] Implement `relation_vacuum` to allow reclaiming space (or at least reporting stats).
- [ ] Implement `scan_analyze_next_block/tuple` to enable `ANALYZE` and better planner estimates.
