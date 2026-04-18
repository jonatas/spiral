# Aspiral: Time-Series Evolution in PostgreSQL

**Aspiral** is a PostgreSQL extension built with `pgrx` designed for massive-scale time-series data. It reimagines time as an evolving spiral starting from a fixed "point zero," optimizing for memory footprint and hierarchical statistical rollups.

## Features Implemented

### 1. The `aspiral` Custom Type
- **Memory Efficient**: Uses a 64-bit integer (`i64`) to store offsets from a kickoff date.
- **GUC-Controlled Kickoff**: Configure the "Day Zero" via `aspiral.kickoff_date`.
- **High Resolution**: Default pace is second-level precision.

### 2. Hierarchical Rollup Engine
- **Automated View Creation**: Using `WITH (aspiral.frames = '1m,5m,1h')` automatically generates a chain of materialized views.
- **Efficient Chaining**: Each view sources data from its immediate parent (e.g., `15m` views source from `5m` views), minimizing raw table scans.
- **Mathematical Precision**: Special handling for averages. `avg(x)` is expanded into `sum(x)` and `count(x)` pairs to ensure precision during hierarchical aggregation.

### 3. Multi-Dimensional Clustering (Z-Order)
- **Space-Filling Curves**: Implements Morton Encoding (Z-order curve) via `aspiral_zorder_3d(t, org_id, user_id)` to map 3D coordinates into a 1D linear space.
- **Fair Indexing**: Resolves the "Composite Index Trap" where an index on `(org_id, t)` is fast for specific Orgs but slow for global time-range queries. Z-order preserves spatial locality across all dimensions.
- **Bit-Interleaving**: Uses a high-performance, loop-free bit-shuffling algorithm to interleave bits of time, tenant, and user IDs into a single `bigint` suitable for standard B-Tree indexing.

### 4. Metadata Catalog
- **Tracking**: Internal `aspiral.metadata` table tracks the parent-child relationships and frame sizes.
- **Schema Isolation**: All metadata resides in the `aspiral` schema.

### 4. Reactive Refresh Sync
- **Cascading Updates**: Refreshing a base view automatically triggers a sequential, ordered refresh of the entire dependent hierarchy.
- **Closed Frame Triggers**: Logic to only process "finalized" time buckets (excluding currently open buckets).

### 5. OHLCV Candlesticks & Histograms
- **First/Last Aggregates**: Implementation of `first()` and `last()` aggregates for Open/Close values over time.
- **Histograms**: Utilities for bucketed distribution analysis using `aspiral_sketch()` (t-digest) and `aspiral_quantile()`.

### 6. Dynamic SQL Parser
- **Smart Derivation**: Dynamic derivation of child SQL based on parent view column definitions, correctly resolving aggregations (e.g. `first`, `last`, `max`, `min`, `sum`, `aspiral_sketch_merge`).

## Quick Start & Walkthrough

Aspiral handles the entire lifecycle of time-series data: from raw ingestion to hierarchical rollups and reactive backfills.

### 1. Ingest & Rollup
Define a base table and an intelligent hierarchy in one command:

```sql
CREATE TABLE asset_ticks (t timestamptz NOT NULL, symbol text NOT NULL, price double precision, vol int);

-- Create base 1m view with 5m and 1h children
CREATE MATERIALIZED VIEW asset_ohlcv_1m WITH (aspiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol,
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume,
    aspiral_sketch(price) as price_sketch 
FROM asset_ticks 
GROUP BY 1, 2;
```

### 2. See it in Action
Running `walkthrough.sql` demonstrates the core capabilities.

### 3. Z-Order vs. Composite Index (Performance)
In multi-tenant systems, traditional composite indexes like `(tenant_id, time)` create a "data silo" problem: queries across all tenants for a specific time window are extremely slow because data is physically scattered.

**Benchmark Scenario:**
- 100,000 rows across 100 Organizations.
- Query: "Select 1 hour of data across ALL organizations."

| Index Type | Strategy | Buffers (8KB Pages) | Time |
| :--- | :--- | :--- | :--- |
| **Composite** `(org, t)` | Seq Scan (Index ignored) | 637 | 4.63 ms |
| **Z-Order** `(t, org, u)` | **Index Scan** | 575 | **0.34 ms** |

**Query Plan Detail (Z-Order):**
```text
Index Scan using idx_zorder on zorder_test
  Index Cond: (aspiral_zorder_3d(...) BETWEEN 20582858897002561 AND 20582858898772424)
  Buffers: shared hit=568 read=7
  Execution Time: 0.344 ms
```

**Query Plan Detail (Composite):**
```text
Seq Scan on zorder_test
  Filter: (org_id BETWEEN 0 AND 100 AND t BETWEEN ...)
  Rows Removed by Filter: 99939
  Buffers: shared hit=637
  Execution Time: 4.635 ms
```

**Why it matters:**
The Z-Order index achieves **~13x speedup** by weaving the bits of time and organization together. This ensures that even if you don't filter by a specific organization, the time-proximate data across all organizations remains physically clustered on disk.

### 4. Hierarchical Percentiles (1h)

**Reactive Backfills:**
When you update historical data, Aspiral flags the specific "dirty buckets" and only refreshes what is necessary during the next cycle:
```text
   base_view    |        bucket_t        |   scope_values    
----------------+------------------------+-------------------
 asset_ohlcv_1m | 2026-04-14 21:05:00-03 | {"symbol": "BTC"}
```

## Architecture

Aspiral is built for **Mathematical Addressing**. By mapping `(time, tenant_id)` to a predictable byte offset, it aims to achieve $O(1)$ read performance for time-series "lanes", bypassing traditional B-Tree index bloat.
