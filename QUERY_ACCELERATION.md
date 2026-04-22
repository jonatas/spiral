# Transparent Query Acceleration in Aspiral

Aspiral features an "Ultimate Caching System" that transparently rewrites queries against raw tables to leverage the hierarchical rollups. This ensures that users always get the fastest possible response without needing to know which rollup tier to query.

## How it Works

1.  **Lineage Rescue**: When an Aspiral hierarchy is created, the system performs a deep walk of the PostgreSQL Query AST to "rescue" the mapping between raw columns, aggregate formulas, and materialized columns.
2.  **The `aspiral.sources` Registry**: These mappings are stored in a system catalog. This registry knows exactly which base column and formula (e.g., `sum`, `count`, `avg`) are satisfied by which materialized view.
3.  **Smart Segment Resolution**: When a `SELECT` query targets a base table with aggregates and a time range:
    *   The system checks the `aspiral.changelog` for "dirty" regions (data changed but not yet materialized).
    *   It decomposes the requested time range into "Clean" segments (mapped to the highest possible rollup tier) and "Dirty/Fragmented" segments (mapped to the raw table).
4.  **AST Swapping**: The query planner transparently replaces the raw table scan with a `UNION ALL` of these segments.

## Benefits

*   **Zero-Lag Accuracy**: Unlike standard materialized views, Aspiral automatically falls back to raw data for any time slice that is "dirty" or hasn't been processed yet. You get the speed of rollups with the accuracy of raw data.
*   **Mathematical Correctness**: Complex aggregates like `AVG` are automatically decomposed into their parts (e.g., `SUM` / `COUNT`) across the union segments to ensure the final result is 100% correct.
*   **Sub-Millisecond Latency**: Queries that would normally scan millions of rows in the raw table are resolved by scanning a few dozen rows in the daily or hourly rollups.

## Example

If you have a table `metrics` with a 1-day rollup, and you query:
```sql
SELECT sum(val) FROM metrics WHERE t >= '2026-04-01' AND t < '2026-04-10';
```
Aspiral will transparently rewrite this (if the data is clean) to:
```sql
SELECT sum(val_sum) FROM metrics_1d WHERE t >= '2026-04-01' AND t < '2026-04-10';
```

## Monitoring

You can see all available accelerated sources by querying the `aspiral.available_sources` view:
```sql
SELECT * FROM aspiral.available_sources;
```
