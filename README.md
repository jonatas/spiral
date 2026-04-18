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

### 3. Metadata Catalog
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

## How It Works (OHLCV Example)

```sql
SET aspiral.kickoff_date = '2026-04-15';

CREATE TABLE asset_ticks (t timestamptz NOT NULL, symbol text NOT NULL, price double precision, vol int);

-- Create base 1m view
-- Aspiral will automatically expand this to include child views for 5m and 1h
CREATE MATERIALIZED VIEW asset_ohlcv_1m WITH (aspiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol,
    first(price, aspiral(t)) as o, 
    max(price) as h, 
    min(price) as l, 
    last(price, aspiral(t)) as c,
    sum(vol) as volume,
    aspiral_sketch(price) as price_sketch 
FROM asset_ticks 
GROUP BY 1, 2;

-- Cascading refresh
REFRESH MATERIALIZED VIEW asset_ohlcv_1m;
-- LOG: Aspiral cascading refresh to 'asset_ohlcv_1m_5m'
-- LOG: Aspiral cascading refresh to 'asset_ohlcv_1m_1h'
```
