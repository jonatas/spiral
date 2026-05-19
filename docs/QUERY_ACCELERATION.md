# Transparent Query Acceleration in Spiral

Spiral includes an experimental planner hook that can rewrite some aggregate queries against raw tables so they read from compatible rollup tiers where possible. The implementation is intentionally conservative: unsupported query shapes should fall back to standard PostgreSQL planning.

## How it Works

1.  **Lineage Registration**: When a Spiral hierarchy is created, the extension records mappings between base columns, formulas, and materialized columns in `spiral.sources`.
2.  **The `spiral.sources` Registry**: This registry is used by the planner hook to determine whether a requested aggregate can be satisfied from a rollup tier or must stay on the raw table.
3.  **Smart Segment Resolution**: When a `SELECT` query targets a base table with aggregates and a time range:
    *   The system checks the `spiral.changelog` for "dirty" regions (data changed but not yet materialized).
    *   **Multi-Tenant Isolation**: When scope values are available, dirty regions are filtered by the query's specific scope (for example, `tenant_id`).
    *   It decomposes the requested time range into "Clean" segments (mapped to the highest possible rollup tier) and "Dirty/Fragmented" segments (mapped to the raw table).
4.  **Subquery Replacement**: The planner can replace the raw table reference with an `RTE_SUBQUERY` that unions eligible rollup segments with raw fallback segments.

## Current Scope

- The planner hook is designed for selected aggregate/time-range query shapes.
- Exact aggregate rewrite support is currently limited to plain `SUM(col)` over base-table columns.
- `COUNT`, `MIN`, `MAX`, `AVG`, `DISTINCT`, ordered aggregates, filtered aggregates, and more complex expressions fall back to standard PostgreSQL planning.
- Dirty or unsupported regions should fall back to the raw table.
- Support boundaries for aggregates, filters, joins, and nested queries are still an active research area.

## Example

If you have a table `metrics` with a 1-day rollup, and you query:
```sql
SELECT sum(val) FROM metrics WHERE t >= '2026-04-01' AND t < '2026-04-10';
```
Spiral may rewrite this (if the query shape is supported and the relevant segments are clean) to something conceptually similar to:
```sql
SELECT sum(val_sum) FROM metrics_1d WHERE t >= '2026-04-01' AND t < '2026-04-10';
```

## Monitoring

You can see all available accelerated sources by querying the `spiral.available_sources` view:
```sql
SELECT * FROM spiral.available_sources;
```
