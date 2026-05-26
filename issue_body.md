# Improvement: Transparent Acceleration Module (TAM) Planner Hook Enhancements

The current implementation of the Spiral planner hook is effective but rigid. To support more complex real-world queries and improve performance, we need to transition from procedural AST walking to a more flexible, pattern-based architecture.

## Identified Improvements

### 1. Batch Metadata Fetching
Current behavior executes an SPI query per column in construct_union_sql_hierarchical.
- Action: Fetch all spiral.sources for a table in a single call and cache locally during the planning turn.

### 2. Boolean Logic Correctness
Current build_time_constraints ignores boolop.
- Action: Respect AND_EXPR vs OR_EXPR. Only neutralize/accelerate predicates that are part of a top-level AND chain unless complex disjunctive logic is implemented.

### 3. Support for Array/IN Predicates
- Action: Handle ScalarArrayOpExpr (e.g., tenant_id IN (1, 2, 3)) to allow acceleration for multi-tenant batch queries.

### 4. Expression Support in SELECT
- Action: Allow acceleration of target list expressions like max(temp) * 1.8 + 32 by recursively processing OpExpr and FuncExpr.

### 5. Support for Aggregate FILTER Clauses
- Action: Push FILTER (WHERE ...) clauses down into the generated subqueries.

## Strategic Shift: Node Pattern Catalog
We propose building a Node Pattern Catalog style architecture. This system will:
1. Define declarative patterns for Opportunities (e.g., TimeRangeMatch, TenantInMatch).
2. Use a registry of matchers to scan the Query AST.
3. Decouple AST discovery from SQL generation.

See docs/PLANNER_EVOLUTION.md for the design draft.
