# Spiral: Time-Series Experimental Framework

**Spiral** is a PostgreSQL extension for experimenting with time-series data at scale. It serves as a playground for learning PostgreSQL internals while testing ideas for storage footprints and hierarchical rollups.

## 🚀 Key Features (Experimental)

### 1. Magic Comments (Zero-Config Pipelines)
Define analytics pipelines directly within your `CREATE TABLE` statement. Spiral parses SQL comments to generate persistent metadata and rollup strategies.

```sql
CREATE TABLE sensor_readings (
    t timestamptz NOT NULL,
    sensor_id int REFERENCES sensors(id),
    voltage double precision, -- Spiral: ohlcv as v, stats as v_stats
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
Spiral solves the "Composite Index Trap" by interleaving the bits of Time and Tenant IDs into a single dimension using a high-precision **128-bit Morton curve**.

- **Full Precision**: Utilizes the full 64-bit bit-budget for both Time and Tenant hashes, eliminating the "Year 2106" overflow problem.
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

- <details><summary><b>Surgical Multi-Tenant Healing</b></summary>
  
  Instead of rebuilding entire views, Spiral identifies exactly which time buckets and <b>tenants</b> changed and patches only those records. By integrating with the changelog, the IVM (Incremental View Maintenance) engine calculates precise delta updates. This provides massive performance gains for backfills or late-arriving data in multi-tenant environments by ensuring IO operations scale with the number of changed records rather than the total view size.

  **Mechanism & Example:**
  When a late record arrives:
  ```sql
  -- Late arrival for tenant 42
  INSERT INTO sensor_readings (t, sensor_id, voltage) 
  VALUES ('2026-04-15 10:05:00', 42, 220.5);
  ```
  Spiral logs this specific `(sensor_id=42, time_bucket='10:05')` to the changelog. The refresh operation:
  ```sql
  SELECT spiral_refresh('sensor_readings_1m');
  ```
  Executes an internal merge matching only the dirty segment:
  ```text
  Merge on sensor_readings_1m
    -> Nested Loop
       -> Index Scan on spiral.changelog (dirty segments)
       -> Index Scan on sensor_readings (fetch new aggregations)
  ```
  </details>
- <details><summary><b>Tenant-Isolated Acceleration</b></summary>
  
  Dirty data for one tenant never slows down queries for another. The planner uses scope constraints to surgically fallback to raw data only where necessary. When the hierarchical planner slices a query, it evaluates the changelog to inject union operations exclusively for dirty time ranges of the specific tenant, maintaining optimal performance for all other unmodified data.

  **Mechanism & Example:**
  If tenant `99` has un-refreshed dirty data but tenant `42` is clean, queries to tenant `42` use the fast rollup entirely:
  ```sql
  -- Fast Path (Clean Tenant)
  SELECT sum(voltage) FROM sensor_readings WHERE sensor_id = 42;
  -- Plan: Aggregate -> Seq Scan on sensor_readings_1h
  ```
  However, for the dirty tenant, the hook transparently union-appends the raw table *only for the dirty time ranges*:
  ```sql
  -- Transparent Fallback (Dirty Tenant)
  SELECT sum(voltage) FROM sensor_readings WHERE sensor_id = 99;
  -- Plan: Aggregate -> Append
  --         -> Seq Scan on sensor_readings_1h (Clean segments)
  --         -> Seq Scan on sensor_readings (Dirty segments filtering via changelog bounds)
  ```
  </details>
- <details><summary><b>ACID Compliance</b></summary>
  
  Metadata and rollups stay perfectly in sync even during transaction rollbacks. The changelog leverages PostgreSQL's MVCC (Multi-Version Concurrency Control) and transaction ID visibility. If a massive ingest transaction fails and rolls back, the system ensures those dirty markers are invisible to subsequent operations, avoiding wasteful re-computations and maintaining data consistency.

  **Mechanism & Example:**
  ```sql
  BEGIN;
  INSERT INTO sensor_readings (t, sensor_id, voltage) 
  VALUES ('2026-04-15 10:05:00', 42, 220.5);
  -- Changes are written to the table and spiral.changelog but only visible to this TX
  ROLLBACK; 
  ```
  Because the changelog insertions share the same transaction snapshot, the rollback permanently hides the `spiral.changelog` entry. Subsequent `SELECT spiral_refresh('sensor_readings_1m');` will find no new rows and exit cleanly, consuming zero extra IO.
  </details>
- <details><summary><b>Cascading Logic</b></summary>
  
  Refreshing a parent view automatically triggers incremental updates for all downstream children. For example, when the 1-minute rollup is refreshed, the update process identifies the changed segments and seamlessly propagates those specific deltas to the 1-hour and 1-day rollups, maintaining consistent states across the entire rollup hierarchy without re-reading the root table.

  **Mechanism & Example:**
  ```sql
  -- A manual or worker-triggered refresh on the lowest granularity:
  SELECT spiral_refresh('sensor_readings_1m');
  ```
  Internally, the engine tracks which `1m` buckets were recalculated. It then dynamically generates and executes the next tier's refresh by querying the newly generated `1m` data:
  ```sql
  -- Auto-triggered internal operation:
  INSERT INTO sensor_readings_1h (t, sensor_id, voltage)
  SELECT 
    date_trunc('hour', t), sensor_id, spiral_stats_merge(voltage)
  FROM sensor_readings_1m 
  WHERE t IN (/* propagated dirty bounds from the 1m refresh */)
  GROUP BY 1, 2
  ON CONFLICT (t, sensor_id) DO UPDATE ...
  ```
  </details>
- <details><summary><b>Self-Healing Dashboards</b></summary>
  
  Historical data corrections automatically flag those specific buckets/tenants for re-aggregation in the next refresh cycle. When a downstream application sends a late-arriving correction, the background worker automatically identifies the affected historical interval and re-aggregates just that segment, ensuring dashboards reflect accurate data seamlessly on their next reload.

  **Mechanism & Example:**
  Imagine an anomaly detected from last month:
  ```sql
  -- Correcting historical data
  UPDATE sensor_readings 
  SET voltage = 220.0 
  WHERE sensor_id = 42 AND t = '2026-03-01 12:00:00';
  ```
  The update trigger fires and logs the `2026-03-01 12:00:00` bucket to the changelog. The background worker picks this up:
  ```text
  LOG: spiral_worker: found 1 dirty segments for sensor_readings
  LOG: spiral_worker: refreshing sensor_readings_1m (interval: 2026-03-01 12:00:00 to 2026-03-01 12:01:00)
  ```
  The next time the dashboard queries `sensor_readings_1d`, the planner incorporates the healed data without any manual view rebuilds.
  </details>

---

### 6. Time-as-Address (8x Storage Reduction)
For extremely high-density datasets where even a `bigint` timestamp is redundant, Spiral can eliminate the timestamp column entirely from physical storage by leveraging custom **Table Access Methods (TAM)**.

- <details><summary><b>Zero-Timestamp Storage</b></summary>
  
  The time and tenant identity are implicitly encoded in the physical address (block number and offset). By defining a strict minimal pace (resolution) and a constant kickoff date, the exact time is derived mathematically from the buffer location, eliminating the need to physically store timestamp fields on disk.

  **Mechanism & Example:**
  ```sql
  -- Create a pure-value table without 't' or 'tenant_id'
  CREATE TABLE ticks_raw_data (
      price numeric,
      vol int
  ) USING spiral; -- Uses the custom TAM
  ```
  Instead of storing `(t, symbol_id, price, vol)`, we only write `(price, vol)`. When querying, the `spiral` access method reconstructs the timestamp via: `t = kickoff_date + (physical_index) * pace`.
  </details>
- <details><summary><b>8x Smaller Footprint</b></summary>Reduces row size from 64 bytes down to just <b>8 bytes</b> (only the value is stored). This extreme compression means that an entire month of high-frequency sensor data can fit efficiently within PostgreSQL's shared buffers, drastically reducing cache eviction rates and accelerating I/O bound queries.</details>
- <details><summary><b>O(1) Direct Access</b></summary>Read any point in time for any tenant instantly using bitwise math, bypassing all PostgreSQL indexes. Traditional B-trees degrade at scale, but calculating an exact buffer offset requires a constant number of CPU cycles regardless of dataset size, offering unprecedented and predictable lookup speeds.</details>
- <details><summary><b>Native Postgres Integration</b></summary>
  
  Unlike external binary files, the `spiral` TAM is fully integrated with the <b>Postgres Buffer Manager and WAL (Write-Ahead Log)</b>. This means your high-density data is crash-safe, replication-ready, and fully compatible with standard <code>pg_dump</code> and <code>pg_basebackup</code>.

  **Mechanism & Example:**
  ```sql
  -- Standard Postgres operations work out of the box
  SELECT * FROM ticks_raw_data WHERE t > '2026-04-15';
  -- Data is retrieved via O(1) buffer mapping inside the TAM
  ```
  </details>

**Configuration:**
```sql
SET spiral.minimal_pace = 0.1; -- 100ms resolution
SET spiral.kickoff_date = '2026-04-15';
```

## 🏗 Architecture & Design Patterns

### 1. The Spiral Mapping (Time to Epoch)
Spiral maps `timestamptz` to a relative `bigint` epoch starting from a configurable `kickoff_date`. This constant-time conversion allows for efficient bitwise operations and Z-Order interleaving.
- <details><summary><b>Session Caching</b></summary>The kickoff epoch is cached in a thread-local variable to eliminate redundant SPI queries during bulk operations. By caching this configuration within the Rust extension's memory space, it prevents repetitive catalog lookups and significantly reduces query execution latency across the session.</details>

### 2. Segment-Based Change Tracking (Joining Unions)
Traditional IVM often struggles with high-volume updates because tracking every single row is expensive. Spiral uses a **Segment Unification** strategy:
- <details><summary><b>Statement-Level Triggers</b></summary>
  
  Uses PostgreSQL <b>Transition Tables</b> (<code>REFERENCING NEW TABLE</code>) to capture thousands of changes in a single Rust-side iteration. Instead of executing trigger logic per row, this approach processes entire batches of modified records natively, minimizing context switching between SQL and Rust.

  **Mechanism & Example:**
  ```sql
  -- Trigger definition internal to Spiral
  CREATE TRIGGER spiral_changelog_trig
  AFTER INSERT OR UPDATE OR DELETE ON sensor_readings
  REFERENCING NEW TABLE AS new_table
  FOR EACH STATEMENT EXECUTE FUNCTION spiral_process_changes();
  ```
  When an `UPDATE` modifies 10,000 rows, the trigger fires exactly once. The Rust function receives all 10,000 rows as a single virtual table, iterating them efficiently in memory to extract distinct time-buckets.
  </details>
- <details><summary><b>Unification Algorithm</b></summary>Overlapping or adjacent "dirty" time ranges are merged into unified segments (unions of intervals). This keeps the <code>spiral.changelog</code> extremely compact. Even under heavy concurrent workloads with scattered updates, the tracking tables remain small, ensuring that the maintenance phase remains highly performant.</details>
- <details><summary><b>JOIN-Based Refresh</b></summary>The incremental refresh logic performs a direct <code>JOIN</code> between the rollup table and the unified segments, ensuring PostgreSQL only touches the minimal set of pages needed. This set-based operation allows the query planner to utilize index scans effectively, making the IVM refresh cycle highly IO-efficient.</details>

### 3. Cascading Hierarchical Refresh
Refreshing a root view automatically triggers a recursive, incremental update down the entire hierarchy (e.g., 1m -> 5m -> 1h). Each level only re-aggregates data from its direct parent for the specific segments that were flagged as dirty.

### 4. Surgical Subquery Grafting
The Hierarchical Planner doesn't just replace tables; it **grafts subqueries**. When a raw table is targeted for acceleration, Spiral replaces its entry in the PostgreSQL Range Table with an `RTE_SUBQUERY` node. This subquery contains the optimized `UNION ALL` of rollups. This allows the outer query's JOINS, CTEs, and Window Functions to remain perfectly valid while the data source itself is optimized.

### 5. Join Constraint Propagation
In multi-table queries, Spiral performs a recursive walk of the **JoinTree**. If it detects an equijoin on time (e.g., `a.t = b.t`) where only one side has a defined range, Spiral **propagates the constraint** to the other side. This enables the simultaneous acceleration of multiple independent time-series datasets in a single join operation.

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
    price numeric, -- Spiral: ohlcv, stats
    vol int        -- Spiral: sum
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'symbol_id'
);

-- The autonomous background worker automatically detects new inserts and
-- refreshes 'ticks_1m' in the background!
--
-- You can still trigger manual refreshes if desired:
-- SELECT spiral_refresh('ticks');
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

### Configuration (GUCs)

Spiral provides several standard PostgreSQL custom variables (GUCs) to tune the background worker and the query planner. These can be set in `postgresql.conf` or dynamically via `ALTER SYSTEM` (or `SET` for session-local overrides where applicable).

#### Background Worker Settings

Spiral's background worker is fully autonomous. It automatically starts for any database where a table is created using `WITH (spiral.frames = ...)`. No manual database name configuration is required. The worker polls `spiral.changelog` every 1 second and seamlessly triggers cascading incremental view maintenance across all registered hierarchies.

- **`spiral.worker_enabled` (boolean, default `true`)**: Allows you to pause the autonomous worker. This is particularly useful during massive data migrations, schema refactorings, or special maintenance windows where you prefer to delay view refreshes until all transactions are completely finalized.
- **`spiral.worker_debug` (boolean, default `false`)**: By default, the worker logs standard `INFO` messages to the PostgreSQL log when refreshing views. Setting this to `true` switches the output to `DEBUG2` level, silencing the standard logs unless your `log_min_messages` is configured to capture deep debug traces.
- **`spiral.max_workers` (int, default `1`)**: Caps the number of background workers that can refresh materialized views concurrently. Increase this value (e.g., to 2-4) if you have many active tables or high write throughput, provided you have sufficient CPU cores.
- **`spiral.worker_batch_size` (int, default `10`)**: The maximum number of `(base_view, scope)` pairs a worker will process per 1-second tick. Increase this if you have many partitioned tenants/scopes receiving updates simultaneously and want the worker to catch up faster.

##### Testing and Isolation

For testing scenarios or safe manual DDL operations, you can temporarily pause workers for the *current database* using advisory locks, guaranteeing no worker process interferes with your transactions:

```sql
-- Pauses background workers until resumed or the session ends
SELECT spiral.stop_bg_workers();

-- Safe to perform DDLs or isolated testing here
-- ...

-- Resumes background workers
SELECT spiral.start_bg_workers();
```

#### Query Planner Settings

- **`spiral.enable_planner_hook` (boolean, default `true`)**: Enables or disables Spiral's query planner optimizations. When `false`, the standard PostgreSQL planner is used without sub-query flattening, Z-order pushdowns, or Cost-Based Slicing. Useful for debugging plan performance. Can be overridden per-session using `SET spiral.enable_planner_hook = off;`.
- **`spiral.planner_max_segments` (int, default `100`)**: Spiral uses Cost-Based Slicing to rewrite queries into a `UNION ALL` of pre-aggregated rollup segments and raw data. This setting limits the maximum number of fragments the planner will generate. If a query requires more segments than this limit (e.g., querying across many fragmented time ranges), the planner falls back to scanning the RAW table to avoid query complexity and `UNION ALL` overhead.
  - **How to estimate:** A value of `100` works well for most typical dashboards. If you observe long query planning times or PostgreSQL memory limits being hit when querying very wide time ranges, try reducing this number to force the planner to use the RAW table sooner. If your queries consistently span hundreds of fragmented time buckets and you want to enforce strict rollup usage (accepting slightly slower planning times to save I/O), you can increase this value (e.g., to `500` or `1000`).

**Example Configuration:**
```sql
-- Pause the worker globally to perform heavy batch ingestion
ALTER SYSTEM SET spiral.worker_enabled = false;
SELECT pg_reload_conf();

-- ... perform massive ingestion ...

-- Re-enable the worker and let it catch up autonomously
ALTER SYSTEM SET spiral.worker_enabled = true;
SELECT pg_reload_conf();

-- Temporarily bypass the planner in the current session for debugging
SET spiral.enable_planner_hook = false;
```

---
Built with ❤️ using `pgrx` and `ta-statistics`.

## 📦 Releasing

Spiral's CI is manual-only to keep GitHub Actions usage predictable. Normal commits and pushes do not start CI.

### Release Flow
1. Run the full local check: `./scripts/check-all.sh`
2. Create the release tag: `git tag v0.1.0`
3. Push your branch and tag:
   ```bash
   git push origin main
   git push origin v0.1.0
   ```
4. Request CI explicitly for the tag:
   ```bash
   gh workflow run ci.yml --ref v0.1.0
   ```

You can do the same from GitHub Actions by opening the `CI` workflow, clicking `Run workflow`, and selecting the release tag as the ref.

When `ci.yml` is run for a `v*` tag and all jobs pass, the same workflow will:
- Build the extension for PostgreSQL 18 on Ubuntu and macOS.
- Bundle the extension files (`.so`/`.dylib`, `.control`, and `.sql`) into tarballs.
- Create a GitHub Release and upload the tarballs.
- Update the Homebrew formula in `Formula/spiral.rb`.

Pushing a tag by itself does not publish a release. Release publication only happens after you manually dispatch `ci.yml` for that tag and the workflow completes successfully.

### Local Packaging
You can also package the extension locally using:
```bash
./scripts/release.sh
```
This requires `cargo-pgrx` to be installed and initialized.

### Homebrew (macOS)
To install Spiral via Homebrew:
1. Ensure you have `postgresql@18` installed: `brew install postgresql@18`
2. Install Spiral using the local formula:
   ```bash
   brew install --build-from-source ./Formula/spiral.rb
   ```
3. Follow the instructions in the `caveats` section to link the extension files into your PostgreSQL directory.

---
## 📜 License
Apache-2.0
