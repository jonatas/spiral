# Dynamic Tenant Timeline

> **Status:** Experimental (Implemented in PR #100+)
> **Component:** Table Access Method (TAM) & Catalog

## The Problem: Reactive Migration
Historically, Spiral achieved its $O(1)$ read/write performance by establishing a rigid mathematical offset based on a global `tenant_scale`:
`Logical Offset = (Time_Relative * tenant_scale) + tenant_id`

While extremely fast, this architecture was brittle for exponentially growing datasets. If a table was initialized with a `tenant_scale` of 1,000, but the platform acquired 1,001 tenants, the entire historical dataset had to be migrated to re-space the memory slots. This caused severe blocking and "reactive" migrations.

## The Solution: Logarithmic Capacity Lanes
The **Dynamic Tenant Timeline** shifts Spiral from a single global scaler to a piecewise step-function over time using `spiral.tenants_timeline`.

1. **Epochs:** A table's timeline is divided into Epochs (`start_t` to `end_t`).
2. **Logarithmic Lanes:** Instead of strictly matching the current tenant count, the system rounds up to the `next_power_of_two()`. 
   - E.g., 100 tenants allocate a lane of 128.
   - This provides "phantom slots" for new tenants.
   - When tenant 129 arrives, the system smoothly transitions to a new epoch with a scale of 256.

Growing from 1,000 to 1,000,000 tenants only triggers ~10 epoch boundaries, eliminating reactive historical migrations.

## Performance Impact Matrix & Backfill Scenarios

This architecture maintains $O(1)$ performance for almost all operations. The only complexity arises during "Historical Backfills"—when data arrives late.

| Scenario | % of Workload (Est.) | Storage Path | Read/Write Performance | Impact Description |
| :--- | :--- | :--- | :--- | :--- |
| **Real-time Ingest (Current Epoch)** | ~98.0% | TAM $O(1)$ Offset (Current Lane) | $O(1)$ | No change. Offsets are calculated directly using the current epoch's localized `tenant_scale`. |
| **Historical Backfill (Known Tenant)** | ~1.9% | TAM $O(1)$ Offset (Historical Lane) | $O(1)$ | Minimal. TAM reverses the logical slot index using the timeline map to find the exact historical page and offset. |
| **Out-of-Bounds Backfill (Late Arrival Anomaly)** | ~0.1% | *OOB Heap (B-Tree)* | $O(\log N)$ | **The Edge Case:** A *newly created* tenant (e.g., ID 150) backfills data into an *old* epoch (which only had capacity for 128). Because `150 > 128`, it cannot fit in the $O(1)$ page. It routes to a standard Postgres Heap (OOB). Queries UNION this small heap with the main TAM. |

### Why the OOB Anomaly is Acceptable
The Out-Of-Bounds (OOB) backfill scenario is extremely rare. Time-series data is overwhelmingly append-mostly. Late-arriving data for a tenant that *did not exist* during that historical epoch represents an insignificant fraction of total data volume. 

By accepting a standard B-Tree $O(\log N)$ degradation for $< 0.1\%$ of anomalies, we preserve true $O(1)$ mathematical performance for $99.9\%$ of the dataset, achieving infinite exponential scaling with zero blocking migrations.