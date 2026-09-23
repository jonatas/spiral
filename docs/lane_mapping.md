# Spiral Lane Mapping & Space Reclamation Architecture

Spiral uses a custom Table Access Method (TAM) that organizes data into Z-ordered multidimensional arrays. Historically, tenants were directly mapped to physical lanes in the array using their logical `tenant_id`. 

This direct mapping caused two major issues:
1. **Sparsity:** If a system had `tenant_id = 1` and `tenant_id = 10000`, the array would allocate 10,000 lanes even if only two were used.
2. **Fragmentation:** If a tenant was deleted, its physical lane would be permanently left empty.

To solve this, Spiral implements an **Indirection Layer** and a **Custom Garbage Collector (Vacuum)**.

## 1. The Indirection Catalog
We decouple the logical `tenant_id` from the physical block layout (`lane_id`).
Two tables maintain this mapping:
- `spiral.lane_mapping (table_oid OID, tenant_id INT, lane_id INT)`
  - *Constraints:* `PRIMARY KEY (table_oid, tenant_id)`, `UNIQUE (table_oid, lane_id)`
- `spiral.free_lanes (table_oid OID, lane_id INT)`

When an `INSERT` occurs:
1. We check if `tenant_id` has an assigned `lane_id`.
2. If not, we attempt to `SELECT lane_id FROM spiral.free_lanes ... FOR UPDATE SKIP LOCKED`.
3. If no free lanes exist, we assign `MAX(lane_id) + 1`.

## 2. Thread-Local Caching & Concurrency
Because looking up the catalog on every `INSERT` and `SELECT` would incur catastrophic overhead, we aggressively cache the mappings in thread-local memory (`LANE_MAPPING_CACHE` and `LANE_REVERSE_CACHE`).

### Concurrency Guarantees
- **Lane Collisions:** Guarded by `FOR UPDATE SKIP LOCKED` on the free lanes table and a `UNIQUE` index on `lane_id`. If two transactions race for the same lane, one will get a constraint violation and retry.
- **Rollback Safety:** If a transaction aborts, the database state rolls back. We register a `pg_sys::RegisterXactCallback` to flush the thread-local caches on `ROLLBACK`, preventing dirty reads.
- **Global Invalidation:** `VACUUM FULL` aggressively rewrites lanes. To ensure all other connections recognize the new mapping, we register a `pg_sys::CacheRegisterRelcacheCallback`. When the system triggers a relcache invalidation, all thread-local caches are dropped, forcing a fresh read from the catalog.

## 3. Garbage Collection (VACUUM)
During a standard `VACUUM`, `spiral_relation_vacuum` scans all pages. It tracks which lanes contain live tuples.
Any lane that has *zero* live tuples across all blocks is considered "Dead" and is inserted into `spiral.free_lanes`.

## 4. Physical Repacking (CLUSTER / VACUUM FULL)
When `VACUUM FULL` runs, `spiral_relation_copy_for_cluster` intercepts the rewrite:
1. It queries the active mappings.
2. It assigns a dense, contiguous sequence of new `lane_id`s `(0..N)`.
3. It copies tuples from the old heap into the new heap using the densely packed lanes.
4. It updates the catalog's `tenant_scale` to match `N` and atomically bulk-updates `spiral.lane_mapping`.

Because `VACUUM FULL` runs with an `AccessExclusiveLock`, no inserts can happen concurrently, and its commit triggers the global relcache invalidation.
