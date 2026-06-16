# ISSUE-001: Optimize Statistical State Storage: Replace JSONB with Internal Binary Representation

## Description
During recent benchmarking with a 1M-row dataset, a significant performance bottleneck was identified in Spiral's statistical aggregation path. While high-level rollups targeting a few rows are exceptionally fast (sub-2ms), queries that require merging states across many rollup rows (e.g., >300k rows) suffer from high CPU overhead due to serialization costs.

## Root Cause
Currently, `spiral_stats_accum`, `spiral_stats_combine`, and their counterparts for sketches and t-digests use `pgrx::JsonB` as the transition state type. This requires the following for every single row in a scan:
1. **Deserialization**: Converting the JSONB blob into a Rust struct (e.g., `StatsState`).
2. **Merging**: Performing the mathematical aggregation logic.
3. **Reserialization**: Converting the updated Rust struct back into a JSONB blob to pass to the next row.

This JSON overhead is dominant when scanning large rollup tables, negating some of the performance benefits of pre-aggregation.

## Benchmark Evidence
- **Scenario**: 1,000,000 raw rows rolled up into 317,665 daily rows grouped by multiple dimensions.
- **Query**: `SELECT regionid, spiral_stats_sum_final(spiral_stats_merge(adv_engine_stats)) FROM hits_1d GROUP BY regionid`
- **Current Latency**: **~1.2 seconds** (Release mode).
- **Bottleneck**: `EXPLAIN ANALYZE` confirms that `Partial HashAggregate` and `Finalize GroupAggregate` spend ~90% of their execution time in the transition/merge functions.

## Proposed Solution
1. **Binary Format**: Define a stable, compact binary format for all internal states (`StatsState`, `SketchState`, `OHLCVState`, `TDigestState`).
2. **Transition Type**: Update the `STYPE` of Spiral aggregates from `jsonb` to `bytea` or a custom internal PostgreSQL type.
3. **Zero-Copy**: Utilize zero-copy deserialization where possible to minimize memory allocations during the aggregation loop.
4. **Compatibility**: Maintain `jsonb` as an output format for the `_final` functions (like `spiral_stats_mean`) to keep the API user-friendly for non-Spiral queries.

## Expected Impact
Removing the JSON serialization layer is expected to improve grouped aggregation performance by **5x to 10x**, bringing Spiral's performance on par with native PostgreSQL aggregates while maintaining its unique hierarchical acceleration features.
