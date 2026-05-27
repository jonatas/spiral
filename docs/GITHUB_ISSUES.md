# Follow-Up Issues

These issue drafts are intended to keep the implementation, docs, and benchmark claims aligned. Each item is written to be pasted directly into GitHub as a new issue.

## 1. Tighten planner rewrite correctness for aggregate mapping [DONE]

**Status Update (2026-05-25):** 
- Implemented support for complex aggregates (Stats, T-Digest, Sketch) with proper type alignment.
- Added recursive aggregate rewriter to support nested functions and arithmetic on top of aggregates.
- Fixed `ohlcv` mapping to correctly route `MIN`/`MAX` to `_l`/`_h` sub-columns.
- Implemented `COUNT(*)` acceleration via a hidden internal `_spiral_count` column.

**Title**
`planner: validate aggregate mapping correctness across rollup and raw fallback segments`

**Description**
The planner hook currently rewrites eligible queries by replacing a base-table RTE with a subquery that unions rollup tiers and raw fallback segments.

We need a correctness audit for aggregate mapping, especially around:

- `AVG` semantics across raw and rollup segments
- `COUNT`, `MIN`, and `MAX` behavior when sourced from materialized columns
- mixed raw/rollup unions with grouping columns
- unsupported or partially supported aggregate functions

The goal is to make the planner's supported surface explicit and testable instead of relying on optimistic fallback assumptions.

**Acceptance criteria**

- Define the supported aggregate matrix in code comments and docs
- Add regression coverage for `SUM`, `COUNT`, `MIN`, `MAX`, and `AVG`
- Ensure unsupported aggregates always fall back cleanly
- Document any cases where exact rewrites are intentionally not attempted

**Relevant files**

- `src/hooks.rs`
- `src/rollup.rs`
- `docs/ARCHITECTURAL_GUIDE.md`
- `docs/QUERY_ACCELERATION.md`

## 2. Formalize sketch/quantile semantics and error bounds

**Title**
`stats: document and verify sketch semantics used for quantile rollups`

**Description**
The current `spiral_sketch_*` implementation uses a compact centroid-based merge strategy, but the docs currently read closer to a full t-digest accuracy claim than the code justifies.

We need to decide whether this structure is:

- a deliberately simple experimental sketch with documented limits, or
- intended to approximate t-digest closely enough for stronger claims

Either way, the implementation needs explicit semantics, test coverage, and benchmark/error characterization.

**Acceptance criteria**

- Document the exact sketch algorithm and its intended guarantees
- Add tests for merge stability and quantile estimation behavior
- Record error characteristics for representative distributions
- Update docs so they do not imply stronger mathematical guarantees than the implementation provides

**Relevant files**

- `src/stats.rs`
- `docs/BENCHMARK.md`
- `README.md`

## 3. Complete or narrow the TAM implementation surface [DONE]

**Status Update (2026-05-27):**
- Completed `tuple_insert`, `tuple_update`, and `tuple_delete`.
- Re-implemented and verified `GenericXLog` for durable WAL logging.
- Integrated `scan_bitmap_next_tuple` to enable standard index usage (B-Tree, Z-order) via Bitmap Heap Scans.
- Verified parallel sequential scans.
- Updated `docs/TAM_AUDIT.md` with final status.

**Title**
`tam: audit unimplemented callbacks and define supported storage semantics`

**Description**
The custom Table Access Method is promising, but several callbacks are currently placeholders or partial implementations. The docs should not imply mature storage semantics until the supported behavior is explicit.

This issue tracks two possible outcomes:

- complete the missing TAM surface needed for a coherent experimental storage engine, or
- narrow the advertised scope and treat the TAM strictly as a prototype path

**Acceptance criteria**

- Audit all registered TAM callbacks and mark each as implemented, partial, or unsupported
- Add tests for insert, scan, MVCC/snapshot behavior, and relation lifecycle where applicable
- Clarify WAL/durability expectations for the compact and block storage paths
- Update docs to match the actual supported semantics

**Relevant files**

- `src/tam.rs`
- `src/storage.rs`
- `README.md`

## 4. Make planner support boundaries explicit for filters, joins, and nested queries [PARTIALLY DONE]

**Status Update (2026-05-25):**
- Implemented **AST Pattern Catalog** for declarative predicate matching.
- Fixed boolean logic awareness: Spiral now correctly identifies safe acceleration paths in `AND` chains and safely ignores `OR`/`NOT` branches.
- Added support for `ScalarArrayOpExpr` (`IN` clauses) in tenant/scope filtering.

**Title**
`planner: define and test support boundaries for filters, joins, CTEs, and nested queries`

**Description**
The planner hook is intentionally conservative, but the repo needs a sharper contract for what query shapes are accelerated versus ignored.

Areas that need explicit validation:

- arbitrary filters on non-scope columns
- join constraint propagation
- nested subqueries and CTEs
- timezone-aware slicing edge cases
- raw fallback behavior when only part of a query is eligible

**Acceptance criteria**

- Enumerate supported and unsupported query shapes
- Add regression tests for each boundary
- Ensure unsafe shapes fall back without altering query semantics
- Reduce ambiguity between README claims and actual hook scope

**Relevant files**

- `src/hooks.rs`
- `tests/pg_regress/sql`
- `docs/ARCHITECTURAL_GUIDE.md`
- `docs/QUERY_ACCELERATION.md`

## 5. Separate benchmark outputs from product claims

**Title**
`benchmarks: keep measured results in benchmark outputs and remove hard-coded performance claims from narrative docs`

**Description**
Performance numbers should live in benchmark artifacts and benchmark-focused docs, not in feature descriptions or architectural overviews.

This issue tracks a stricter documentation policy:

- narrative docs describe goals, mechanisms, and limitations
- benchmark docs contain dated measurements and methodology
- any headline numbers should be traceable to a benchmark script or result file

**Acceptance criteria**

- Remove hard-coded benchmark numbers from README-style feature descriptions
- Keep performance figures in benchmark docs/results only
- Link claims to the relevant benchmark script or result file
- Add a short documentation guideline for future updates

**Relevant files**

- `README.md`
- `docs/BENCHMARK.md`
- `docs/BENCHMARK_RESULTS.md`
- `benchmarks/`

## 7. Implement high-precision 128-bit Z-order and Hilbert Curve [DONE]

**Status Update (2026-05-27):**
- Transitioned from 64-bit to 128-bit bit-budget for Z-order and Hilbert curves.
- Removed the 32-bit timestamp truncation, enabling full 64-bit temporal range.
- Implemented recursive Hilbert Curve encoding for superior spatial locality.
- Aligned return types to `NUMERIC` for all 128-bit space-filling curve results.

**Title**
`math: implement high-precision 128-bit indexing for infinite temporal range`

**Description**
The current 64-bit Z-order implementation truncates timestamps to 32 bits, leading to overflows in 2106 and potential index collisions. We need to expand the bit-budget to 128 bits to support full 64-bit timestamps and high-entropy dimension hashes.

## 8. Implement Z-Order Slice Operators (@>, <@) and quadrant decomposition [DONE]

**Status Update (2026-05-27):**
- Implemented recursive quadrant decomposition for 2D range generation.
- Added `spiral_zorder_contained_by` function and `@>` operator for Z-value/BOX intersection.
- Verified core logic with unit tests in `zorder.rs`.

**Title**
`indexing: add slice operators and range generation for Z-order skip-scans`
