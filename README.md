# Aspiral: Time-Series Evolution in PostgreSQL

**Aspiral** is a PostgreSQL extension designed for massive-scale time-series data. It reimagines time as an evolving spiral, optimizing for both storage footprint and hierarchical statistical rollups.

## 🚀 Core Features

### 1. Magic Comments (Zero-Config Pipelines)
Define your entire analytics pipeline directly within your `CREATE TABLE` statement. Aspiral parses SQL comments to automatically generate materialized view hierarchies.

```sql
CREATE TABLE sensor_readings (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    voltage double precision, -- Aspiral: ohlc as v, stats as v_stats
    current double precision, -- Aspiral: stats
    status_code int           -- Aspiral: count as total_events
) WITH (
    aspiral.frames = '1m,5m,1h', 
    aspiral.tenant = 'sensor_id'
);
```

**What happens automatically:**
- **Automated Hierarchy**: Views for `1m`, `5m`, and `1h` are created.
- **Intelligent Naming**: 
    - `voltage` gets `v_o, v_h, v_l, v_c` (from `ohlc as v`) and `v_stats`.
    - `current` keeps its name `current` (single task, no alias).
    - `status_code` becomes `total_events` (via custom alias).
- **Z-Order Clustering**: An index is created to cluster data by `(t, sensor_id)` for high-performance range queries.

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
- **13x Speedup**: Benchmarks show significant I/O reduction for multi-tenant range queries compared to traditional `(tenant_id, time)` indexes.

---

### 4. Reactive Backfill Engine
Aspiral tracks "dirty buckets" in a transactional changelog.

- **ACID Compliance**: Metadata and rollups stay perfectly in sync even during transaction rollbacks.
- **Self-Healing Dashboards**: Adding historical data (audits/corrections) automatically flags those buckets for re-aggregation in the next refresh cycle.
- **Gap-Filling**: Easily generate continuous timelines for frontend charts using standard SQL joins against Aspiral rollups.

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
...
-- Set your "Day Zero"
SET aspiral.kickoff_date = '2026-04-15';

-- Define your data
CREATE TABLE ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric, -- Aspiral: ohlc, stats
    vol int        -- Aspiral: sum
) WITH (aspiral.frames='1m,5m,1h', aspiral.tenant='symbol_id');

-- Ingest data...
INSERT INTO ticks ... ;

-- Refresh the pipeline (cascades to all children)
REFRESH MATERIALIZED VIEW ticks_ohlcv_1m;
```

---
Built with ❤️ using `pgrx` and `ta-statistics`.
