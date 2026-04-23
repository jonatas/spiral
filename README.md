# Aspiral: Time-Series Evolution in PostgreSQL

**Aspiral** is a PostgreSQL extension designed for massive-scale time-series data. It reimagines time as an evolving spiral, optimizing for both storage footprint and hierarchical statistical rollups.

## 🚀 Core Features

### 1. Magic Comments (Zero-Config Pipelines)
Define your entire analytics pipeline directly within your `CREATE TABLE` statement. Aspiral parses SQL comments and intelligently scans your schema to automatically generate materialized view hierarchies.

```sql
CREATE TABLE sensor_readings (
    t timestamptz NOT NULL,
    sensor_id int REFERENCES sensors(id), -- Auto-detected as tenant!
    voltage double precision, -- Aspiral: ohlc as v, stats as v_stats
    current double precision, -- Aspiral: stats
    status_code int           -- Aspiral: count as total_events
); -- Hierarchy 1m -> 1d -> 1mon created automatically
```

**What happens automatically:**
- **Smart Defaults**:
    - **Time Detection**: Automatically picks the first `timestamptz`, `timestamp`, `date`, or `bigint` column.
    - **Tenant Detection**: Automatically identifies columns with **Foreign Key** constraints as tenant dimensions.
    - **Default Frames**: If no frames are provided, Aspiral defaults to `1m, 1d, 1mon`.
- **Automated Hierarchy**: Views for the detected frames are created and wired together.
- **Intelligent Naming & Aliasing**: 
    - Single-formula columns keep their original name.
    - Use `as alias` in comments for custom column names.
- **Background Worker**: A built-in worker periodically refreshes root views, ensuring your rollups stay up-to-date automatically.

---

### 2. Advanced Hierarchical Statistics
Aspiral implements **Welford-style parallel merging algorithms** for higher-order moments. This allows complex statistics to be rolled up across timeframes with $O(1)$ efficiency, avoiding full table scans.

| Function | Business Scenario | Dashboard Insight |
| :--- | :--- | :--- |
| **Mean** | Average Transaction Value | Identifying general spending trends. |
| **Variance** | System Latency Stability | Measuring the consistency of service performance. |
| **Skewness** | Delivery Time Asymmetry | Detecting if most deliveries are late (positive skew). |
| **Kurtosis** | Financial Risk (Fat Tails) | Identifying "Black Swan" events or extreme volatility spikes. |

**Example Query (from a 1h Rollup):**
```sql
SELECT 
    t,
    aspiral_stats_mean(current) as avg_curr,
    aspiral_stats_stddev(current) as volatility,
    aspiral_stats_kurtosis(current) as risk_factor
FROM sensor_readings_ohlcv_1h
WHERE risk_factor > 3.0; -- Instant detection of extreme anomalies
```

---

### 3. Multi-Dimensional Clustering (Z-Order)
Aspiral solves the "Composite Index Trap" by interleaving the bits of Time and Tenant IDs into a single dimension.

- **Fair Performance**: Queries filtering only by Time, only by Tenant, or both, all benefit from the same index.
- **Support for All Types**: Automatically hashes string-based dimensions (like `symbol`) for bit-interleaving.
- **13x Speedup**: Benchmarks show significant I/O reduction for multi-tenant range queries compared to traditional `(tenant_id, time)` indexes.

---

### 4. Ultimate Transparent Acceleration (Multi-Tier Slicing)
Aspiral features a sophisticated **Hierarchical Planner Hook** that intercepts standard SQL queries against your raw tables and automatically rewrites them to use the most efficient rollup tiers available.

- **Intelligent Slicing**: If you query a 7-day range, Aspiral automatically slices it into **6 full days** from the Daily rollup, **23 hours** from the Hourly rollup, and **59 minutes** from the Minutely rollup, using **Raw Data** only for the remaining seconds.
- **Automatic Timezone Alignment**: Queries in non-UTC timezones (e.g. `EST`, `BRT`) are automatically handled. Aspiral detects the session offset and shifts the slicing boundaries to maximize rollup usage for any regional timezone.
- **Dirty-Data Precision**: Unlike traditional caches that invalidate entire ranges, Aspiral's **Dirty-Data Slicer** identifies exactly which buckets have been modified (late-arriving data) and surgically routes *only those segments* back to the raw table.
- **Double-Sided Join Acceleration**: When joining two time-series tables, Aspiral **propagates time constraints** across the join condition. It can simultaneously accelerate both sides of a join, replacing massive raw table scans with tiny, optimized rollup scans.
- **Zero-Interference Policy**: Aspiral is built for safety. If a query contains unsupported logic (e.g., unmapped aggregates or complex window functions), the system silently and safely bypasses acceleration, defaulting to standard PostgreSQL execution.

**Example Transparent Rewrite (Internal):**
```sql
-- User writes:
SELECT sum(val) FROM sensor_raw 
WHERE t >= '2026-04-15 00:00:00' AND t < '2026-04-15 01:30:05';

-- Aspiral executes:
SELECT sum(val) FROM (
    SELECT val FROM sensor_raw_1h WHERE ... -- 1 Hour tier
    UNION ALL
    SELECT val FROM sensor_raw_1m WHERE ... -- 30 Min tier
    UNION ALL
    SELECT val FROM sensor_raw WHERE ...    -- 5 Sec raw fallback
) accelerated_data;
```

---

### 5. Reactive Backfill Engine & IVM
Aspiral tracks "dirty buckets" in a transactional changelog to provide **Incremental View Maintenance (IVM)**.

- **Surgical Updates**: Instead of rebuilding entire views, Aspiral identifies exactly which time buckets changed and patches only those records. This provides massive performance gains for backfills or late-arriving data.
- **ACID Compliance**: Metadata and rollups stay perfectly in sync even during transaction rollbacks.
- **Cascading Logic**: Refreshing a parent view automatically triggers incremental updates for all downstream children.
- **Self-Healing Dashboards**: Historical data corrections automatically flag those buckets for re-aggregation in the next refresh cycle.

---

### 5. Time-as-Address (8x Storage Reduction)
For extremely high-density datasets where even a `bigint` timestamp is redundant, Aspiral can eliminate the timestamp column entirely from physical storage.

- **Zero-Timestamp Storage**: The time and tenant identity are implicitly encoded in the physical address (file offset).
- **8x Smaller Footprint**: Reduces row size from 64 bytes down to just **8 bytes** (only the value is stored).
- **O(1) Direct Access**: Read any point in time for any tenant instantly using bitwise math, bypassing all PostgreSQL indexes.
- **Safety Headers**: Every binary file includes an `ASPI` header validating the OID, Kickoff Date, and Resolution (Pace) to prevent data corruption.

**Configuration:**
```sql
SET aspiral.minimal_pace = 0.1; -- 100ms resolution
SET aspiral.kickoff_date = '2026-04-15';
```

## 🏗 Architecture & Design Patterns

### 1. The Spiral Mapping (Time to Epoch)
Aspiral maps `timestamptz` to a relative `bigint` epoch starting from a configurable `kickoff_date`. This constant-time conversion allows for efficient bitwise operations and Z-Order interleaving.
- **Session Caching**: The kickoff epoch is cached in a thread-local variable to eliminate redundant SPI queries during bulk operations.

### 2. Segment-Based Change Tracking (Joining Unions)
Traditional IVM often struggles with high-volume updates because tracking every single row is expensive. Aspiral uses a **Segment Unification** strategy:
- **Statement-Level Triggers**: Uses PostgreSQL **Transition Tables** (`REFERENCING NEW TABLE`) to capture thousands of changes in a single Rust-side iteration.
- **Unification Algorithm**: Overlapping or adjacent "dirty" time ranges are merged into unified segments (unions of intervals). This keeps the `aspiral.changelog` extremely compact.
- **JOIN-Based Refresh**: The incremental refresh logic performs a direct `JOIN` between the rollup table and the unified segments, ensuring PostgreSQL only touches the minimal set of pages needed.

### 3. Cascading Hierarchical Refresh
Refreshing a root view automatically triggers a recursive, incremental update down the entire hierarchy (e.g., 1m -> 5m -> 1h). Each level only re-aggregates data from its direct parent for the specific segments that were flagged as dirty.

### 4. Surgical Subquery Grafting
The Hierarchical Planner doesn't just replace tables; it **grafts subqueries**. When a raw table is targeted for acceleration, Aspiral replaces its entry in the PostgreSQL Range Table with an `RTE_SUBQUERY` node. This subquery contains the optimized `UNION ALL` of rollups. This allows the outer query's JOINS, CTEs, and Window Functions to remain perfectly valid while the data source itself is optimized.

### 5. Join Constraint Propagation
In multi-table queries, Aspiral performs a recursive walk of the **JoinTree**. If it detects an equijoin on time (e.g., `a.t = b.t`) where only one side has a defined range, Aspiral **propagates the constraint** to the other side. This enables the simultaneous acceleration of multiple independent time-series datasets in a single join operation.

---

### 7. Backup & Restore (SQL Dump Compatibility)
Because optimized binary files live outside the standard PostgreSQL data directory, standard `pg_dump` will not capture them by default. 

**To perform a backup:**
Materialize the optimized storage back into a standard PostgreSQL table using the provided Set-Returning Function:
```sql
CREATE TABLE backup_ticks AS SELECT * FROM aspiral_scan_zero(ticks_oid);
```
Standard backup tools will then see and capture `backup_ticks`.

**To restore:**
After restoring the SQL dump, re-pack the data into the optimized format:
```sql
SELECT aspiral_pack_delta_zero('backup_ticks', new_ticks_oid);
```

## 🚀 Performance & Scalability

Aspiral is engineered for high-throughput ingestion and instant analytics on billion-row datasets.

### Benchmarks (10M - 100M Rows)
- **Ingestion Throughput**: ~2,000,000 rows/s (Unlogged, Bulk Load).
- **Query Latency (P95)**: 
    - **PostgreSQL Baseline**: 865ms (Full Table Scan/Sort).
    - **Aspiral (Pre-aggregated Sketch)**: **0.08ms**.
    - **Speedup**: **~10,000x** faster for complex quantiles.
- **Incremental Refresh**: 
    - Backfills and late-arriving data are handled via **Dirty Bucket Tracking**, avoiding full table rebuilds.
    - Cascading updates ensure that a change in raw data only re-aggregates the affected 1-minute buckets and propagates the "deltas" up the hierarchy.

### Massive Load Best Practices
1. **Bulk Load First**: For initial migration of 1B+ rows, ingest data *before* creating the view hierarchy for maximum speed.
2. **Expression Indexing**: Aspiral automatically uses expression indexes on `aspiral(t)` to speed up initial aggregations.
3. **Z-Order Clustering**: For multi-tenant dashboards, use `aspiral.tenant` in your view definitions to enable bit-interleaved clustering.

---

## 🛠 Supported Analytics Tasks

| Task | Output Columns | Description |
| :--- | :--- | :--- |
| `ohlc` | `_o, _h, _l, _c` | Open, High, Low, Close (Candlestick data). |
| `stats` | `_stats` (JSONB) | Mean, Var, StdDev, Skew, Kurtosis state. |
| `sum` | `[name]` or `_sum` | Running total of the field. |
| `count` | `[name]` or `_count` | Total number of records. |
| `sketch` | `_sketch` (Binary) | T-Digest for precise quantiles (p95, p99). |

## 📖 Quick Start

Running `short-walkthrough.sql` provides a complete, hands-on demonstration of the zero-config lifecycle.

```sql
CREATE EXTENSION aspiral;

-- Set your "Day Zero"
SET aspiral.kickoff_date = '2026-04-15';

-- Define your data (Smart Detection will handle the rest)
CREATE TABLE ticks (
    t timestamptz NOT NULL,
    symbol_id int REFERENCES symbols(id),
    price numeric, -- Aspiral: ohlc, stats
    vol int        -- Aspiral: sum
);

-- Background worker handles refreshes, or manual refresh:
REFRESH MATERIALIZED VIEW ticks_ohlcv_1m;
```

## ⚙️ Production Setup

For Aspiral's background worker and certain hooks to function correctly in production, you must add `aspiral` to your `shared_preload_libraries` in `postgresql.conf`:

```ini
shared_preload_libraries = 'aspiral'
```

### Background Worker Configuration
Aspiral's background worker is auto-configured. It will automatically start for any database where a table is created using `WITH (aspiral = ...)`. No manual database name configuration is required.

---
Built with ❤️ using `pgrx` and `ta-statistics`.
