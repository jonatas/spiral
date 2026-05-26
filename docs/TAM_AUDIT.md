# Table Access Method (TAM) Audit

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
| `scan_begin` | **Implemented** | Initializes `SpiralScanState`. Supports `SCAN_TIME_RANGE` optimization and parallel scans. |
| `scan_getnextslot` | **Implemented** | Primary read path. Performs O(1) reconstruction. Assigns `tts_tid`. |
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
| `parallelscan_estimate` | **Implemented** | Uses `table_block_parallelscan_estimate`. |
| `parallelscan_initialize` | **Implemented** | Uses `table_block_parallelscan_initialize`. |
| `parallelscan_reinitialize` | **Implemented** | Uses `table_block_parallelscan_reinitialize`. |

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
| `tuple_satisfies_snapshot` | **Implemented** | Always returns `true` (no MVCC yet). |
| `scan_analyze_next_block` | **Implemented** | Returns `true` if blocks remain. |
| `scan_analyze_next_tuple` | **Implemented** | Full block/tuple sampling for statistical analysis. |

## Summary of Missing Semantics

1.  **WAL Logging**: Not currently implemented. `GenericXLog` usage was attempted but reverted due to same-transaction visibility issues. Marked as TODO.
2.  **MVCC**: Snapshot isolation is completely absent. The TAM currently reads the "current" state of blocks regardless of the transaction snapshot.
3.  **Indices**: Index-organized access is not supported. Only sequential scans (accelerated by `SCAN_TIME_RANGE` in the planner) work.

## Roadmap

- [x] Implement `tuple_insert` by mapping slot columns to logical offsets.
- [x] Implement `tuple_delete` and `tuple_update`.
- [x] Implement `relation_vacuum` and `ANALYZE` support.
- [x] Implement parallel scan support.
- [ ] Add `GenericXLog` support to point-in-time operations for durability.
- [ ] Implement `index_build_range_scan` to allow standard indices on Spiral tables.
