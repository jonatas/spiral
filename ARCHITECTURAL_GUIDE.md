# Aspiral: Architectural Guide & Performance Catalog

Aspiral provides transparent hierarchical query acceleration. However, it is designed for specific workloads and has built-in safety fallbacks for unsupported scenarios.

## When to use Aspiral
- **High-Volume Time-Series:** 10M+ rows where sequential scans are too slow.
- **Hierarchical Analysis:** Queries covering mixed ranges (e.g., "Last 7 days" + "Last 10 minutes").
- **Multi-Tenant IoT:** Data scoped by dimensions like `device_id` or `sensor_type`.
- **Read-Heavy Workloads:** Dashboards and analytics where aggregate results are reused.

## When Regular Tables are better
- **Small Datasets:** If your table has < 100k rows, PostgreSQL's Seq Scan is often faster than the overhead of slicing.
- **Frequent Historical Updates:** If you constantly update random points in the past, the `aspiral.changelog` will become fragmented, forcing frequent "Dirty Fallbacks" to the raw table.
- **Complex Relational Queries:** Queries using heavy Window Functions, non-standard aggregates, or complex JOINs that don't involve time.

## Known Architectural Constraints & Issues

### 1. Supported Aggregates
Aspiral currently only accelerates: `SUM`, `COUNT`, `MIN`, `MAX`, `AVG`, and `TDIGEST`.
- **Inconsistency:** Unmapped aggregates (e.g., `STDDEV`, `VARIANCE`) will trigger a **Transparent Fallback** to the raw table. This ensures 100% accuracy but loses the acceleration benefit.

### 2. Filter Propagation
- **Issue:** Filters on columns that are NOT part of the `aspiral_scope` or the Time column (`t`) currently trigger a fallback.
- **Reason:** Pushing arbitrary filters into a `UNION ALL` subquery while maintaining rollup mathematical correctness is a complex task for future versions.

### 3. CTEs and Complex Subqueries
- **Issue:** Tables wrapped in `WITH` (CTEs) or certain nested subqueries are currently bypassed.
- **Reason:** The planner hook presently inspects the top-level Range Table. Deep recursion into the Query AST for CTE acceleration is planned for stabilization.

### 4. Precision & Rounding
- **Constraint:** `AVG` is computed as `SUM(sum_col) / SUM(count_col)`. While mathematically sound, slight floating-point differences may occur compared to a raw scan due to the order of operations in rollups.

## Timezone-Aware Acceleration

Aspiral handles non-UTC queries with surgical precision. Time-series data is stored in UTC, but users often query in their local timezones (e.g., `America/Sao_Paulo`).

### The Slicing Strategy
When a session is set to a non-UTC timezone, Aspiral detects the current offset and shifts its greedy slicer accordingly:
1. **Head Segment:** Uses the finest rollup (Hourly/Minutely) to cover the offset "head" (the hours before the first full UTC day).
2. **Body Segment:** Uses the Daily tier for all full UTC days in the middle of the requested range.
3. **Tail Segment:** Uses the finest rollup to cover the remaining hours at the end.

This ensures that even a query with a `-3h` offset still benefits from Daily rollups for the bulk of its range, rather than falling back to Raw data.

### Example
**Query:** `2026-04-15 00:00:00-03` to `2026-04-17 00:00:00-03`
**Aspiral Plan:**
- 21 Hours from `HOURLY ROLLUP` (Head)
- 1 Day from `DAILY ROLLUP` (Body)
- 3 Hours from `HOURLY ROLLUP` (Tail)

## Safety & Stability Guarantee
Aspiral is designed with a **Zero-Interference** policy. If the system cannot 100% guarantee a correct accelerated plan (due to unmapped logic, dirty regions, or complex ASTs), it will **silently and safely bypass** to standard PostgreSQL execution.
