# Spiral: Experimental Architecture Overview

This document outlines the internal ideas being tested in Spiral. These mechanisms are part of an experimental framework for exploring PostgreSQL internals.

## Query Acceleration (Experimental)

Spiral tests ideas for transparent hierarchical query acceleration. It is currently designed for specific research workloads and includes safety fallbacks.

## Performance Catalog (Observations)
- **High-Volume Time-Series:** Testing 10M+ row scenarios where sequential scans are slow.
- **Hierarchical Slicing:** Exploring mixed-range queries (e.g., "7 days" + "10 minutes").
- **Multi-Tenant Scoping:** Testing bit-interleaved clustering (Z-Order) for tenant dimensions.

## Current Limitations & Research Areas

### 1. Supported Aggregates
Current exact planner rewrite support is limited to plain `SUM` over columns that have `sum`-materialized rollup state.
- `COUNT`, `MIN`, `MAX`, `AVG`, `TDIGEST`, and other unmapped aggregates currently trigger a fallback to raw-table planning to maintain accuracy.

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
