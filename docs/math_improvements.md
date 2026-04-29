
## 11.3 Decoding the Z-Value Formula
The interleaved Z-value calculation can be expressed as a summation of bits. Let's break down the variables to understand how we map 2D space into a 1D line:

<div class="math-formula">
\[ \text{Z}(x, y) = \sum_{i=0}^{n-1} (x_i \cdot 2^{2i+1} + y_i \cdot 2^{2i}) \]
</div>

- **\(x_i, y_i\)**: These represent the individual bits of your coordinates (e.g., Time and Tenant ID).
- **\(2^{2i+1}\) and \(2^{2i}\)**: These powers of 2 act as "masks" that shift the bits into their new interleaved positions.
- **The Result**: A single scalar that increases as you move along the Morton Curve, ensuring that points with similar bit patterns (close in 2D) have similar Z-values (close on disk).

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
