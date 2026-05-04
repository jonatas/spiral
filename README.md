# Spiral: Time-Series Experimental Framework

**Spiral** is a PostgreSQL extension for experimenting with time-series data at scale. It serves as a playground for learning PostgreSQL internals while testing ideas for storage footprints and hierarchical rollups.

## 🚀 Key Features (Experimental)

### 1. Magic Comments (Zero-Config Pipelines)
Define analytics pipelines directly within your `CREATE TABLE` statement. Spiral parses SQL comments to generate persistent metadata and rollup strategies.

```sql
CREATE TABLE sensor_readings (
    t timestamptz NOT NULL,
    sensor_id int REFERENCES sensors(id),
    voltage double precision, -- Spiral: ohlc as v, stats as v_stats
    current double precision, -- Spiral: stats
    status_code int           -- Spiral: sum
) WITH (
    spiral.frames = '1m, 1d, 1mon',
    spiral.tenant = 'sensor_id'
);
```

**What happens automatically:**
- **Smart Defaults & Parsing**:
    - **Time Detection**: Automatically picks the first `timestamptz`, `timestamp`, `date`, or `bigint` column.
    - **Frames & Tenant**: Strictly parsed from the `WITH (spiral.frames='...', spiral.tenant='...')` clause. Without `spiral.frames`, the table will not be tracked by Spiral.
- **Metadata-Driven Hierarchy**: Instead of relying on hardcoded naming conventions (like `_stats`), Spiral now generates and persists a **Rollup Strategy** for each column in `spiral.sources`.
- **Intelligent Naming & Aliasing**: 
    - Single-formula columns keep their original name.
    - Use `as alias` in comments for custom column names.
- **Background Worker**: A built-in worker periodically refreshes root views, ensuring your rollups stay up-to-date automatically.

---

### 2. Advanced Rollup Customization (GSub Strategies)
Spiral's rollup engine is no longer bound by column name suffixes. Every rollup is driven by the `rollup_gsub_strategy` column in `spiral.sources`.

- **Dynamic Substitutions**: Use `rollup("\1")` to refer to the source column (either raw or from a parent tier) and `"\1"` to refer to the target materialized column.
- **Custom Formulas**: You can manually update `spiral.sources` to implement specialized rollup logic.

**Example Strategy Injection:**
```sql
-- Internally generated for 'stats' formula:
-- rollup_gsub_strategy = 'spiral_stats_merge(rollup("\1")) as "\1"'

-- You can replace it with custom logic:
UPDATE spiral.sources 
SET rollup_gsub_strategy = 'my_custom_agg(rollup("\1")) as "\1"' 
WHERE mat_column = 'my_col';
```

---

### 3. Advanced Hierarchical Statistics
Spiral implements **Welford-style parallel merging algorithms** for higher-order moments. This allows complex statistics to be rolled up across timeframes with $O(1)$ efficiency, avoiding full table scans.

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
    spiral_stats_mean(current) as avg_curr,
    spiral_stats_stddev(current) as volatility,
    spiral_stats_kurtosis(current) as risk_factor
FROM sensor_readings_1h
WHERE risk_factor > 3.0; -- Instant detection of extreme anomalies
```

---

### 3. Multi-Dimensional Clustering (Z-Order)
Spiral solves the "Composite Index Trap" by interleaving the bits of Time and Tenant IDs into a single dimension.

- **Fair Performance**: Queries filtering only by Time, only by Tenant, or both, all benefit from the same index.
- **Support for All Types**: Automatically hashes string-based dimensions (like `symbol`) for bit-interleaving.
- **13x Speedup**: Benchmarks show significant I/O reduction for multi-tenant range queries compared to traditional `(tenant_id, time)` indexes.

---

### 4. Transparent Query Acceleration (Hierarchical Slicing)
Spiral includes an experimental **Planner Hook** that intercepts SQL queries against raw tables and attempts to rewrite them using available rollup tiers.

- **Range Slicing**: Automatically attempts to slice a query into segments matching available rollups (e.g., using Daily, Hourly, and Minutely tiers where they align).
- **Timezone Alignment**: Detects session offsets to adjust slicing boundaries for non-UTC queries.
- **Dirty-Data Handling**: Attempts to identify modified buckets in the changelog and routes those specific segments back to the raw table for accuracy.
- **Join Acceleration**: Propagates time constraints across join conditions to try and accelerate multiple tables in a single query.
- **Safety Fallback**: If a query uses unsupported logic (unmapped aggregates, complex functions), the hook stays out of the way and defaults to standard PostgreSQL execution.

**Example Transparent Rewrite (Internal):**
```sql
-- User writes:
SELECT sum(val) FROM sensor_raw 
WHERE t >= '2026-04-15 00:00:00' AND t < '2026-04-15 01:30:05';

-- Spiral executes:
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
Spiral tracks "dirty buckets" in a transactional changelog to provide **Incremental View Maintenance (IVM)**.

- **Surgical Multi-Tenant Healing**: Instead of rebuilding entire views, Spiral identifies exactly which time buckets and **tenants** changed and patches only those records. This provides massive performance gains for backfills or late-arriving data in multi-tenant environments.
- **Tenant-Isolated Acceleration**: Dirty data for one tenant never slows down queries for another. The planner uses scope constraints to surgically fallback to raw data only where necessary.
- **ACID Compliance**: Metadata and rollups stay perfectly in sync even during transaction rollbacks.
- **Cascading Logic**: Refreshing a parent view automatically triggers incremental updates for all downstream children.
- **Self-Healing Dashboards**: Historical data corrections automatically flag those specific buckets/tenants for re-aggregation in the next refresh cycle.

---

### 5. Time-as-Address (8x Storage Reduction)
For extremely high-density datasets where even a `bigint` timestamp is redundant, Spiral can eliminate the timestamp column entirely from physical storage.

- **Zero-Timestamp Storage**: The time and tenant identity are implicitly encoded in the physical address (file offset).
- **8x Smaller Footprint**: Reduces row size from 64 bytes down to just **8 bytes** (only the value is stored).
- **O(1) Direct Access**: Read any point in time for any tenant instantly using bitwise math, bypassing all PostgreSQL indexes.
- **Safety Headers**: Every binary file includes an `ASPI` header validating the OID, Kickoff Date, and Resolution (Pace) to prevent data corruption.

**Configuration:**
```sql
SET spiral.minimal_pace = 0.1; -- 100ms resolution
SET spiral.kickoff_date = '2026-04-15';
```

## 🏗 Architecture & Design Patterns

### 1. The Spiral Mapping (Time to Epoch)
Spiral maps `timestamptz` to a relative `bigint` epoch starting from a configurable `kickoff_date`. This constant-time conversion allows for efficient bitwise operations and Z-Order interleaving.
- **Session Caching**: The kickoff epoch is cached in a thread-local variable to eliminate redundant SPI queries during bulk operations.

### 2. Segment-Based Change Tracking (Joining Unions)
Traditional IVM often struggles with high-volume updates because tracking every single row is expensive. Spiral uses a **Segment Unification** strategy:
- **Statement-Level Triggers**: Uses PostgreSQL **Transition Tables** (`REFERENCING NEW TABLE`) to capture thousands of changes in a single Rust-side iteration.
- **Unification Algorithm**: Overlapping or adjacent "dirty" time ranges are merged into unified segments (unions of intervals). This keeps the `spiral.changelog` extremely compact.
- **JOIN-Based Refresh**: The incremental refresh logic performs a direct `JOIN` between the rollup table and the unified segments, ensuring PostgreSQL only touches the minimal set of pages needed.

### 3. Cascading Hierarchical Refresh
Refreshing a root view automatically triggers a recursive, incremental update down the entire hierarchy (e.g., 1m -> 5m -> 1h). Each level only re-aggregates data from its direct parent for the specific segments that were flagged as dirty.

### 4. Surgical Subquery Grafting
The Hierarchical Planner doesn't just replace tables; it **grafts subqueries**. When a raw table is targeted for acceleration, Spiral replaces its entry in the PostgreSQL Range Table with an `RTE_SUBQUERY` node. This subquery contains the optimized `UNION ALL` of rollups. This allows the outer query's JOINS, CTEs, and Window Functions to remain perfectly valid while the data source itself is optimized.

### 5. Join Constraint Propagation
In multi-table queries, Spiral performs a recursive walk of the **JoinTree**. If it detects an equijoin on time (e.g., `a.t = b.t`) where only one side has a defined range, Spiral **propagates the constraint** to the other side. This enables the simultaneous acceleration of multiple independent time-series datasets in a single join operation.

---

### 7. Backup & Restore (SQL Dump Compatibility)
Because optimized binary files live outside the standard PostgreSQL data directory, standard `pg_dump` will not capture them by default. 

**To perform a backup:**
Materialize the optimized storage back into a standard PostgreSQL table using the provided Set-Returning Function:
```sql
CREATE TABLE backup_ticks AS SELECT * FROM spiral_scan_zero(ticks_oid);
```
Standard backup tools will then see and capture `backup_ticks`.

**To restore:**
After restoring the SQL dump, re-pack the data into the optimized format:
```sql
SELECT spiral_pack_delta_zero('backup_ticks', new_ticks_oid);
```

## 📖 Quick Start

The fastest way to learn Spiral is through the [Short Walkthrough](examples/short_walkthrough.sql).

```bash
cargo pgrx run pg18 < examples/short_walkthrough.sql
```

### Basic Usage

```sql
CREATE EXTENSION spiral;

-- Set your "Day Zero"
SET spiral.kickoff_date = '2026-04-15';

-- Define your data and frames
CREATE TABLE ticks (
    t timestamptz NOT NULL,
    symbol_id int REFERENCES symbols(id),
    price numeric, -- Spiral: ohlc, stats
    vol int        -- Spiral: sum
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'symbol_id'
);

-- Background worker handles refreshes, or manual refresh:
SELECT spiral_refresh('ticks_1m');
```

## 🧪 Benchmarks & Examples

- **[Examples](examples/)**: Hands-on walkthroughs from basic to advanced.
- **[Benchmarks](benchmarks/)**: Comprehensive performance evaluations at different scales.
- **[Documentation](docs/)**: Detailed architectural deep dives.

## ⚙️ Experimental Setup

Spiral is currently being tested on **PostgreSQL 16, 17, and 18**.

### Local Development
To run the framework locally with your preferred PostgreSQL version:

```bash
cargo pgrx run pg18
```

For certain hooks to function correctly, you must add `spiral` to your `shared_preload_libraries` in `postgresql.conf`:

```ini
shared_preload_libraries = 'spiral'
```

### Background Worker Configuration
Spiral's background worker is auto-configured. It will automatically start for any database where a table is created using `WITH (spiral = ...)`. No manual database name configuration is required.

---
Built with ❤️ using `pgrx` and `ta-statistics`.
