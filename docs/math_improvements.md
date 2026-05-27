
## 11.3 High-Precision 128-bit Indexing
Spiral has moved from 64-bit to **128-bit Morton (Z-order) and Hilbert curves**. This transition provides several critical advantages for high-frequency time-series data:

### A. Removal of the 32-bit Timestamp Truncation
Previously, timestamps were truncated to 32 bits, which limited the temporal range of the index (the "Year 2106" problem) and caused bit collisions in long-lived datasets. Spiral now utilizes the full **64-bit precision** of the Unix epoch timestamp.

### B. Expanded Dimensionality
With a 128-bit bit-budget, Spiral can interleave:
- **Time**: 64 bits (full range)
- **Space/Dimensions**: 64 bits (high-entropy hashes)

This allows for nearly infinite temporal range while maintaining extremely high entropy for tenant and dimension identification, drastically reducing index collisions in multi-tenant environments.

### C. Performance & Type Alignment
The 128-bit Z-values are stored as PostgreSQL **NUMERIC** types. While slightly larger than `bigint`, this allows Spiral to leverage standard B-tree indexing while preserving the full mathematical precision of the space-filling curve.

## 11.4 Decoding the Z-Value Formula (128-bit Precision)
The interleaved Z-value calculation can be expressed as a summation of bits. Spiral now utilizes a full **128-bit bit-budget** to support high-precision indexing without truncation.

<div class="math-formula">
\[ \text{Z}(x, y) = \sum_{i=0}^{n-1} (x_i \cdot 2^{2i+1} + y_i \cdot 2^{2i}) \]
</div>

- **\(x_i, y_i\)**: These represent the individual bits of your coordinates. Spiral uses the full **64 bits** of the timestamp (\(x\)) and a **64-bit hash** of the spatial dimensions (\(y\)).
- **\(2^{2i+1}\) and \(2^{2i}\)**: These powers of 2 act as "masks" that shift the bits into their new interleaved positions, creating a 128-bit scalar result.
- **The Result**: A single `NUMERIC` scalar that increases as you move along the Morton Curve. This move from 64-bit to 128-bit Z-values eliminates the "Year 2106" overflow problem and provides nearly infinite temporal range.

---

## 22.3 Why Welford’s Algorithm Matters
Calculating standard deviation on a rolling stream is numerically unstable if you use the "sum of squares" method. Welford's algorithm provides a way to update the mean and variance in a single pass with high precision.

<div class="math-formula">
\[ \bar{x}_n = \bar{x}_{n-1} + \frac{x_n - \bar{x}_{n-1}}{n} \]
\[ M_{2,n} = M_{2,n-1} + (x_n - \bar{x}_{n-1})(x_n - \bar{x}_n) \]
</div>

- **\(\bar{x}_n\)**: The **New Mean**. It's the old mean plus a fraction of the distance to the new data point \(x_n\).
- **\(M_{2,n}\)**: The **Sum of Squares of Differences** from the mean. Instead of squaring huge numbers, we only track the "unrest" (variance) added by each new point.
- **The Merge Advantage**: Because this is based on moments, we can combine two buckets \(A\) and \(B\) using parallel merge logic without ever needing to see the raw data points that created them. This is the secret to Spiral's \(O(1)\) rollups.
