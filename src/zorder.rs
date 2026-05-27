use pgrx::datum::AnyNumeric;
use pgrx::prelude::*;

/// FNV-1a 64-bit hash — stable, documented, no external dependency.
#[inline]
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub fn spread_64(x: u64) -> u128 {
    let mut res: u128 = x as u128;
    res = (res | (res << 32)) & 0x00000000FFFFFFFF00000000FFFFFFFF_u128;
    res = (res | (res << 16)) & 0x0000FFFF0000FFFF0000FFFF0000FFFF_u128;
    res = (res | (res << 8)) & 0x00FF00FF00FF00FF00FF00FF00FF00FF_u128;
    res = (res | (res << 4)) & 0x0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F_u128;
    res = (res | (res << 2)) & 0x33333333333333333333333333333333_u128;
    res = (res | (res << 1)) & 0x55555555555555555555555555555555_u128;
    res
}

pub fn interleave_64(x: u64, y: u64) -> u128 {
    spread_64(x) | (spread_64(y) << 1)
}

/// Reverse of spread_64: extracts the bits that were interleaved.
pub fn spread_64_back(mut res: u128) -> u64 {
    res &= 0x55555555555555555555555555555555_u128;
    res = (res | (res >> 1)) & 0x33333333333333333333333333333333_u128;
    res = (res | (res >> 2)) & 0x0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F_u128;
    res = (res | (res >> 4)) & 0x00FF00FF00FF00FF00FF00FF00FF00FF_u128;
    res = (res | (res >> 8)) & 0x0000FFFF0000FFFF0000FFFF0000FFFF_u128;
    res = (res | (res >> 16)) & 0x00000000FFFFFFFF00000000FFFFFFFF_u128;
    res = (res | (res >> 32)) & 0x0000000000000000FFFFFFFFFFFFFFFF_u128;
    res as u64
}

pub fn decode_zorder_2d(z: u128) -> (u64, u64) {
    let x = spread_64_back(z);
    let y = spread_64_back(z >> 1);
    (x, y)
}

/// Generates a set of Z-ranges that cover a 2D box.
/// Uses recursive quadrant decomposition to identify which 1D segments
/// intersect with the target multidimensional box.
pub fn generate_z_ranges_2d(min_x: u64, min_y: u64, max_x: u64, max_y: u64) -> Vec<(u128, u128)> {
    let mut ranges = Vec::new();
    decompose_quadrant(0, 0, 64, min_x, min_y, max_x, max_y, &mut ranges);
    ranges
}

fn decompose_quadrant(
    x: u64,
    y: u64,
    level: u32,
    min_x: u64,
    min_y: u64,
    max_x: u64,
    max_y: u64,
    ranges: &mut Vec<(u128, u128)>,
) {
    let side = if level == 64 {
        u64::MAX
    } else {
        (1u64 << level).wrapping_sub(1)
    };
    let x_end = x.saturating_add(side);
    let y_end = y.saturating_add(side);

    // Completely outside
    if x > max_x || x_end < min_x || y > max_y || y_end < min_y {
        return;
    }

    // Completely inside
    if x >= min_x && x_end <= max_x && y >= min_y && y_end <= max_y {
        let start = interleave_64(x, y);
        let end = interleave_64(x_end, y_end);
        ranges.push((start, end));
        return;
    }

    // Leaf node or too deep
    if level == 0 {
        let z = interleave_64(x, y);
        ranges.push((z, z));
        return;
    }

    let next_level = level - 1;
    let next_side = 1u64 << next_level;

    // Subdivide into 4 quadrants in Morton order
    decompose_quadrant(x, y, next_level, min_x, min_y, max_x, max_y, ranges);
    decompose_quadrant(
        x + next_side,
        y,
        next_level,
        min_x,
        min_y,
        max_x,
        max_y,
        ranges,
    );
    decompose_quadrant(
        x,
        y + next_side,
        next_level,
        min_x,
        min_y,
        max_x,
        max_y,
        ranges,
    );
    decompose_quadrant(
        x + next_side,
        y + next_side,
        next_level,
        min_x,
        min_y,
        max_x,
        max_y,
        ranges,
    );
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_zorder_contained_by(z: AnyNumeric, b: pg_sys::BOX) -> bool {
    let z_str = z.to_string();
    let z_val = u128::from_str_radix(z_str.as_str(), 10).unwrap_or(0);
    let (x, y) = decode_zorder_2d(z_val);

    // Postgres BOX: high is (max_x, max_y), low is (min_x, min_y)
    let b_low_x = b.low.x as u64;
    let b_high_x = b.high.x as u64;
    let b_low_y = b.low.y as u64;
    let b_high_y = b.high.y as u64;

    x >= b_low_x && x <= b_high_x && y >= b_low_y && y <= b_high_y
}

extension_sql!(
    r#"
    CREATE OPERATOR <@ (
        LEFTARG = numeric,
        RIGHTARG = box,
        PROCEDURE = spiral_zorder_contained_by
    );
    "#,
    name = "create_zorder_slice_operators",
    requires = [spiral_zorder_contained_by]
);

pub fn spiral_zorder_core(t: i64, dimensions: Vec<Option<String>>) -> u128 {
    let x = t as u64;
    let mut y = 0u64;
    for (i, dim) in dimensions.iter().enumerate() {
        if let Some(d) = dim {
            y ^= fnv1a_64(d.as_bytes()) << (i % 8);
        }
    }
    interleave_64(x, y)
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_zorder(t: i64, dimensions: Vec<Option<String>>) -> AnyNumeric {
    let val = spiral_zorder_core(t, dimensions);
    AnyNumeric::try_from(val.to_string().as_str()).expect("valid numeric")
}

pub fn spiral_zorder_int_array_core(t: i64, dimensions: Vec<i32>) -> u128 {
    let x = t as u64;
    let mut y = 0u64;
    for (i, dim) in dimensions.iter().enumerate() {
        y ^= (*dim as u64) << (i % 8);
    }
    interleave_64(x, y)
}

#[pg_extern(immutable, parallel_safe, name = "spiral_zorder")]
pub fn spiral_zorder_int_array(t: i64, dimensions: Vec<i32>) -> AnyNumeric {
    let val = spiral_zorder_int_array_core(t, dimensions);
    AnyNumeric::try_from(val.to_string().as_str()).expect("valid numeric")
}

pub fn zorder_3d_core(x: i64, y: i64, z: i64) -> u128 {
    let mut res = 0u128;
    for i in 0..42 {
        res |= ((x as u128 >> i) & 1) << (3 * i);
        res |= ((y as u128 >> i) & 1) << (3 * i + 1);
        res |= ((z as u128 >> i) & 1) << (3 * i + 2);
    }
    res
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_zorder_3d(x: i64, y: i64, z: i64) -> AnyNumeric {
    let val = zorder_3d_core(x, y, z);
    AnyNumeric::try_from(val.to_string().as_str()).expect("valid numeric")
}

fn rot(n: u64, x: &mut u64, y: &mut u64, rx: bool, ry: bool) {
    if !ry {
        if rx {
            *x = n.wrapping_sub(1).wrapping_sub(*x);
            *y = n.wrapping_sub(1).wrapping_sub(*y);
        }
        std::mem::swap(x, y);
    }
}

pub fn hilbert_encode(mut x: u64, mut y: u64) -> u128 {
    let mut d = 0u128;
    for s in (0..64).rev() {
        let n = 1u64 << s;
        let rx = (x & n) > 0;
        let ry = (y & n) > 0;
        d += (n as u128 * n as u128) * ((3 * rx as u64) ^ ry as u64) as u128;

        if rx {
            x ^= n;
        }
        if ry {
            y ^= n;
        }

        rot(n, &mut x, &mut y, rx, ry);
    }
    d
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_hilbert_2d(x: i64, y: i64) -> AnyNumeric {
    let d = hilbert_encode(x as u64, y as u64);
    AnyNumeric::try_from(d.to_string().as_str()).expect("valid numeric")
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn test_hilbert_2d_locality() {
        assert_eq!(hilbert_encode(0, 0), 0);
        assert_eq!(hilbert_encode(1, 0), 1);
        assert_eq!(hilbert_encode(1, 1), 2);
        assert_eq!(hilbert_encode(0, 1), 3);
    }

    #[pg_test]
    fn test_zorder_decode() {
        let x = 123456789u64;
        let y = 987654321u64;
        let z = interleave_64(x, y);
        let (dx, dy) = decode_zorder_2d(z);
        assert_eq!(x, dx);
        assert_eq!(y, dy);
    }

    #[pg_test]
    fn test_z_ranges_generation() {
        // Box (0,0) to (1,1) should cover Z-values 0, 1, 2, 3
        // In Morton order: (0,0)->0, (1,0)->2, (0,1)->1, (1,1)->3
        // So the range should be [0, 3] if contiguous or discrete.
        let ranges = generate_z_ranges_2d(0, 0, 1, 1);

        // Decompose quadrant will find the 2x2 block and return it as one range [0, 3]
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], (0, 3));

        // Box (0,0) to (0,0)
        let ranges = generate_z_ranges_2d(0, 0, 0, 0);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], (0, 0));

        // Box (2,0) to (2,0)
        // (2,0) is bits (10, 00)
        // x0=0 at pos 0, y0=0 at pos 1, x1=1 at pos 2, y1=0 at pos 3
        // Result: 2^2 = 4
        let ranges = generate_z_ranges_2d(2, 0, 2, 0);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], (4, 4));
    }
}
