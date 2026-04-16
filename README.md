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

## How It Works (OHLCV Example)

```sql
SET aspiral.kickoff_date = '2026-04-15';

CREATE TABLE stock_ticks (t timestamptz, price decimal, volume int);

-- Create base 1m view
-- Aspiral will automatically expand this to include child views for 5m and 15m
CREATE MATERIALIZED VIEW ohlcv_1m AS 
SELECT 
    (aspiral(t)/60)*60 as t, 
    sum(price) as price_sum, 
    count(price) as price_count, 
    max(price) as price_max,
    min(price) as price_min
FROM stock_ticks 
GROUP BY 1
WITH (aspiral.frames = '5m,15m');

-- Cascading refresh
REFRESH MATERIALIZED VIEW ohlcv_1m;
-- LOG: Aspiral cascading refresh to 'ohlcv_1m_5m'
-- LOG: Aspiral cascading refresh to 'ohlcv_1m_15m'
```

## Upcoming Scenarios

- [ ] **OHLCV Candlesticks**: Implementation of `first()` and `last()` aggregates for Open/Close.
- [ ] **Histograms**: Utilities for bucketed distribution analysis over time.
- [ ] **Closed Frame Triggers**: Logic to only process "finalized" time buckets.
- [ ] **SQL Parser**: Dynamic derivation of child SQL based on parent view column definitions.
