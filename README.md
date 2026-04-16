# Aspiral: Time-Series Evolution in PostgreSQL

**Aspiral** is a PostgreSQL extension built with `pgrx` designed for massive-scale time-series data. It reimagines time as an evolving spiral starting from a fixed "point zero," optimizing for memory footprint and hierarchical statistical rollups.

## The Core Concept: Aspiraling Time

Time is traditionally handled as a complex `timestamptz` structure. **Aspiral** simplifies this by establishing a **Kickoff Date** (system day zero) and a **Pace** (minimal moment, e.g., 1 second).

- **Integer-Based Representation**: Every moment is stored as a highly efficient integer offset from the kickoff date. This provides a "short number" that represents the exact position in the temporal spiral.
- **Large Scale, Low Memory**: By using integer offsets (32-bit or 64-bit), we drastically reduce the index size and memory footprint compared to standard timestamps.
- **Triangulation**: The constant pace allows for instantaneous calculation of time differences and "triangulation" of events across different time scales using simple arithmetic.
- **Folding/Unfolding Statistics**: Time series are naturally hierarchical. Aspiral allows you to "fold" detailed second-level data into larger frames (minutes, hours, days) and "unfold" them back for deep analysis.

## Technical Architecture

### 1. The `aspiral` Custom Type
Built with `pgrx`, the `aspiral` type maps standard PostgreSQL `timestamptz` to an optimized internal integer offset. 
- **Automatic Conversion**: Seamlessly cast from `timestamptz` while respecting time zones.
- **Efficient Indexing**: Native B-tree support for fast range scans on day-zero-based offsets.

### 2. Native DDL Hooking
Aspiral uses PostgreSQL's `ProcessUtility_hook` to intercept `CREATE MATERIALIZED VIEW` commands. This allows for a native SQL experience with custom extension parameters.

```sql
create table stocks (t timestamptz, price decimal, volume integer);

-- The hook intercepts this DDL and automatically expands it
create materialized view ohlcv AS
select 
  aspiral(t) as t,
  first(price) as o, 
  max(price) as h, 
  min(price) as l, 
  last(price) as c,
  sum(volume) as volume
from stocks
group by 1 
order by 1
with (
  aspiral.frames = '1m,15m,1h,1d,1w',
  aspiral.rollup_rules = 'max, min, last'
);
```

### 3. Hierarchical Rollup Engine
When a view is created with `aspiral.frames`, the extension automatically:
1. Creates the base materialized view (e.g., `ohlcv` at the default second pace).
2. Generates dependent views for each frame: `ohlcv_1m`, `ohlcv_15m`, etc.
3. **Smart Rollups**: Each subsequent view (e.g., `1h`) is computed from the nearest smaller frame (e.g., `15m`), minimizing the need to scan the raw `stocks` table.

## Configuration

- `aspiral.kickoff_date`: The static date set as the center of the spiral (Default: current date of extension initialization).
- `aspiral.default_pace`: The minimal moment resolution (Default: `1s`).

## Roadmap

1. [x] Deep research into `pgrx` custom types and structures.
2. [x] Architectural design for DDL hooking and hierarchical rollups.
3. [ ] Scaffold `pgrx` extension and implement `aspiral` integer-offset type.
4. [ ] Implement `ProcessUtility_hook` for `CREATE MATERIALIZED VIEW` interception.
5. [ ] Build the hierarchical view generation logic.
