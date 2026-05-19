# Table Access Method (TAM) Audit

This document tracks the implementation status of the Spiral TAM callbacks as of May 2026.

## Callback Status

| Callback | Status | Notes |
| :--- | :--- | :--- |
| `slot_callbacks` | **Implemented** | Returns `&pg_sys::TTSOpsVirtual`. |
| `tuple_insert` | **Partial (Placeholder)** | Currently only logs routing to Delta Store. Does not perform actual insertion. |
| `scan_begin` | **Implemented** | Correctly initializes `SpiralScanState`. |
| `scan_getnextslot` | **Implemented** | Iterates through blocks and hydrates virtual slots. Supports O(1) logical mapping. |
| `scan_end` | **Implemented** | Cleans up scan state and allocated memory. |
| `scan_rescan` | **Implemented** | Resets scan state for repeat execution. |
| `relation_size` | **Implemented** | Returns size based on physical block count. |
| `relation_estimate_size` | **Implemented** | Provides estimates for the PostgreSQL planner. |
| `relation_set_new_filelocator` | **Implemented** | Initializes physical storage for new relations. |
| `relation_nontransactional_truncate` | **Unsupported** | Placeholder (no-op). |
| `relation_copy_for_cluster` | **Unsupported** | Placeholder (no-op). |
| `tuple_fetch_row_version` | **Unsupported** | Returns `false`. |
| `tuple_tid_valid` | **Unsupported** | Returns `false`. |
| `tuple_satisfies_snapshot` | **Unsupported** | Returns `false` (MVCC not supported in TAM). |
| `relation_needs_toast_table` | **Implemented** | Returns `false` (Fixed-width storage, no TOAST needed). |

## Supported Semantics

### Storage Model
The Spiral TAM uses a **fixed-width, block-oriented storage model**. Data is mapped to physical offsets using a deterministic logical-to-physical mapping.

### Durability & WAL
Currently, the TAM-specific `tuple_insert` does **not** generate WAL records.
Experimental functions in `src/storage.rs` (e.g., `spiral_pack_delta_compact`) utilize `GenericXLog` for WAL-logged updates, but these are not yet integrated into the standard TAM surface.

### MVCC & Isolation
The current implementation **does not support MVCC**. `tuple_satisfies_snapshot` always returns `false` (or rather, the scan bypasses snapshot checks and reads raw blocks). This is suitable for immutable research data but not for general-purpose transactional workloads.

### Relation Lifecycle
`CREATE TABLE ... USING spiral` works and initializes files. `TRUNCATE` and `CLUSTER` are currently no-ops.
