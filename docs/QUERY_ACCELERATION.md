# Spiral Query Acceleration Matrix

Spiral uses a **Constraint Reasoning Engine** to transform standard SQL queries into accelerated hierarchical scans. This document outlines the supported syntaxes, current limitations, and the mathematical roadmap.

## 1. Supported Syntaxes (Accelerated)

| Category | Syntax Example | Mathematical Principle | Status |
| :--- | :--- | :--- | :--- |
| **Simple Aggregates** | `SUM(col)`, `AVG(col)`, `COUNT(*)` | Associativity / Chan-Merge | ✅ Supported |
| **Complex Stats** | `spiral_stats(col)`, `spiral_sketch(col)` | Online Moments / T-Digest | ✅ Supported |
| **OHLCV** | `first(col, t)`, `max(col)`, `min(col)` | Order-dependent / Min-Max | ✅ Supported |
| **Linear Lifting** | `SUM(price * 2 + 10)` | Distributive Property | ✅ Supported |
| **Timestamp Ranges** | `t >= 'A' AND t < 'B'` | Set Intersection | ✅ Supported |
| **Complex Ranges** | `t > 'A' AND t <= 'B'` | Discrete Interval Mapping | ✅ Supported |
| **Scope Filters** | `WHERE tenant_id = 123` | Spatial Partitioning | ✅ Supported |
| **Join Propagation** | `JOIN b ON a.tid = b.tid WHERE a.tid = 1` | Transitive Equality | ✅ Supported |
| **Logical Unions** | `WHERE t < '1h' OR t > '5h'` | **Convex Hull** Bounding | ✅ Supported |
| **IN Clauses** | `WHERE tenant_id IN (1, 2, 3)` | Set Membership | ✅ Supported |

---

## 2. Fallback Scenarios (Standard Execution)

The following syntaxes currently trigger a fallback to the raw base table to maintain 100% accuracy:

| Syntax Pattern | Reason for Fallback |
| :--- | :--- |
| `SUM(col_a * col_b)` | **Non-Linear**: Requires cross-moments (Co-variance) not yet in `StatsState`. |
| `DISTINCT col` | Requires global set tracking (HLL) not yet integrated into rollups. |
| `HAVING SUM(col) > 100` | Planner currently prioritizes `WHERE` clause analysis. |
| `WINDOW` functions | Requires specific frame-aware aggregation logic. |

---

## 3. Mathematical Roadmap (TODO)

### Priority: High (Efficiency & Coverage)
- [ ] **Transitive Inequality Propagation**:
    - *Example*: `JOIN b ON a.t = b.t WHERE a.t > '2026-01-01'`
    - *Reasoning*: If timestamps are joined, the range constraint on `a` mathematically applies to `b`. Implementing this will accelerate large time-series joins by $100\times$.
- [ ] **Canonical Normalization Pass**:
    - *Example*: Convert `col = 1 OR col = 2` into a unified `SetConstraint`.
    - *Reasoning*: Reduces technical debt by allowing the visitor to reason about properties rather than syntax variations.

### Priority: Medium (Functional Expansion)
- [ ] **Non-Linear Cross-Moments**:
    - *Example*: Support `SUM(price * volume)` by storing $\sum(x \cdot y)$ in the rollup.
    - *Reasoning*: Essential for financial metrics like VWAP (Volume Weighted Average Price).
- [ ] **Hyper-box Containment Operators**:
    - *Example*: `zorder_col <@ box(p1, p2)`
    - *Reasoning*: Moving beyond `BETWEEN` to true geometric skip-scans for Z-order curves.

### Priority: Low (Niche Features)
- [ ] **Filter-Clause Lifting**:
    - *Example*: `SUM(val) FILTER (WHERE val > 0)`
    - *Reasoning*: Requires storing partial moments based on value-ranges (Histogram-based rollups).

---

## Logic Policy: "First, Do No Harm"
Spiral follows a **Strict Accuracy** policy. If the AST Visitor cannot prove that a rollup scan will yield the identical result to a raw scan (after applying the remaining PostgreSQL filters), it will **always** fallback to standard execution. We prefer $100ms$ of correct data over $1ms$ of incorrect data.
