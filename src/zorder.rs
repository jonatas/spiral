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
}
