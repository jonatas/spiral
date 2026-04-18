# Aspiral: Time-Series Evolution in PostgreSQL

**Aspiral** is a PostgreSQL extension built with `pgrx` designed for massive-scale time-series data. It reimagines time as an evolving spiral starting from a fixed "point zero," optimizing for memory footprint and hierarchical statistical rollups.

## Features Implemented

### 1. The `aspiral` Custom Type
- **Memory Efficient**: Uses a 64-bit integer (`i64`) to store offsets from a kickoff date.
- **GUC-Controlled Kickoff**: Configure the "Day Zero" via `aspiral.kickoff_date`.
- **High Resolution**: Default pace is second-level precision.

### 2. Hierarchical Rollup Engine
- **Magic Comments (Zero-Config)**: Define your analytics pipeline directly in the `CREATE TABLE` statement using SQL comments.
  ```sql
  CREATE TABLE ticks (
      t timestamptz NOT NULL,
      symbol_id int NOT NULL,
      price numeric(12,2), -- Aspiral: ohlc as p, stats as p_stats
      vol int              -- Aspiral: sum as total_vol
  ) WITH (aspiral.frames='1m,5m,1h', aspiral.tenant='symbol_id');
  ```
  This automatically generates:
  - Materialized views for all frames (`_1m`, `_5m`, `_1h`).
  - Optimized projections with **Intelligent Naming** and **Custom Aliasing**:
    - Use `as alias` in the comment to override the default naming.
    - Multi-output columns like `ohlc as p` will use the alias as a prefix (`p_o`, `p_h`, etc.).
    - Default names are preserved if no alias is provided and only one task exists.
  - Hierarchical rollup logic (merging 1m stats into 5m, etc.).

- **Automated View Creation**: Using `WITH (aspiral.frames = '1m,5m,1h')` on a manual Materialized View also works for custom complex queries.
- **Efficient Chaining**: Each view sources data from its immediate parent, minimizing raw table scans.
- **Mathematical Precision**: Advanced handling for `aspiral_stats` (Welford-style merging) ensures $O(1)$ hierarchical updates for Mean, Variance, Skewness, and Kurtosis.

### 3. Multi-Dimensional Clustering (Z-Order)
- **Seamless Configuration**: Use the `WITH` clause during table or materialized view creation to automatically enable Z-order clustering.
  ```sql
  CREATE TABLE tenant_logs (
      t timestamptz NOT NULL,
      org_id int NOT NULL,
      user_id int NOT NULL,
      payload jsonb
  ) WITH (
      aspiral.tenant = 'org_id, user_id', -- Up to 3 dimensions
      aspiral.time = 't'                 -- Optional, defaults to 't'
  );
  ```
- **Fair Indexing**: Resolves the "Composite Index Trap" where an index on `(org_id, t)` is fast for specific Orgs but slow for global time-range queries. Z-order preserves spatial locality across all dimensions.

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
