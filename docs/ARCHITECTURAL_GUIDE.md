# Spiral: Experimental Architecture Overview

This document outlines the internal ideas being tested in Spiral. These mechanisms are part of an experimental framework for exploring PostgreSQL internals.

## Query Acceleration (Experimental)

Spiral tests ideas for transparent hierarchical query acceleration. It is currently designed for specific research workloads and includes safety fallbacks.

## Performance Catalog (Observations)
- **High-Volume Time-Series:** Testing 10M+ row scenarios where sequential scans are slow.
- **Hierarchical Slicing:** Exploring mixed-range queries (e.g., "7 days" + "10 minutes").
- **Multi-Tenant Scoping:** Testing bit-interleaved clustering (Z-Order) for tenant dimensions.

## Current Limitations & Research Areas

## Consolidated Aggregation States

Spiral uses consolidated JSONB states for complex aggregations. This allows for $O(1)$ merging of statistics and sketches across hierarchical tiers.

### OHLCV (Open-High-Low-Close-Volume)
Previously, OHLCV was stored as five separate columns. It is now consolidated into a single `OHLCVState` JSONB column.

**Heuristic Mapping**:
The query planner automatically maps standard aggregates to the consolidated state:
- `first(col, t)` $\rightarrow$ `spiral_open(col)`
- `max(col)` $\rightarrow$ `spiral_high(col)`
- `min(col)` $\rightarrow$ `spiral_low(col)`
- `last(col, t)` $\rightarrow$ `spiral_close(col)`
- `sum(col)` $\rightarrow$ `spiral_volume(col)`

**Manual Access & Transformation**:
Users can access components independently or transform the state using these helper functions:
- `spiral_ohlcv_open(col)`: Get the Open price.
- `spiral_ohlcv_high(col)`: Get the High price.
- `spiral_ohlcv_low(col)`: Get the Low price.
- `spiral_ohlcv_close(col)`: Get the Close price.
- `spiral_ohlcv_volume(col)`: Get the Volume.
- `spiral_ohlcv_to_array(col)`: Cast the state to a `double precision[]` array: `[open, high, low, close, volume]`.
- `spiral_ohlcv_to_json(col)`: Return the raw JSONB representation.

**Example**:
```sql
-- Accessing as an array for external visualization
SELECT spiral_ohlcv_to_array(price) FROM asset_ohlcv_1m;
-- Result: {100.0, 115.0, 100.0, 112.0, 542.0}
```

### Stats & Sketches
- **Stats**: Consolidated moments (Mean, Variance, Skewness, Kurtosis).
- **T-Digest/Sketch**: Quantile and distribution sketches.


### 2. Filter Push-down
Arbitrary filters on non-scope columns are a complex research area. Currently, these filters trigger a safe fallback to standard PostgreSQL execution.

### 3. Subquery & CTE Handling
The planner hook currently inspects the top-level Range Table. Deep recursion for CTE acceleration is an area for future exploration.

### 4. Custom Table Access Method (TAM)
Spiral includes a prototype Table Access Method for experimental storage. For a detailed audit of supported callbacks and semantics, see [TAM_AUDIT.md](./TAM_AUDIT.md).

## Timezone-Aware Slicing (Experimental)

Spiral explores ways to handle non-UTC queries by shifting slicing boundaries based on session offsets.

### Current Approach
When a session offset is detected, the slicer attempts to:
1. Align the "Body" of the query with full UTC Daily buckets.
2. Use finer rollups (Hourly/Minutely) for the "Head" and "Tail" offsets.

## Safety & Research Policy
Spiral follows a **Zero-Interference** research policy. If the experimental logic cannot guarantee a correct plan, it defaults to standard PostgreSQL execution.
