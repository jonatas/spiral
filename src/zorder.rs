use pgrx::prelude::*;

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

/// Spreads 64 bits into 128 bits (every other bit is 0).
fn spread_64(x: u64) -> u128 {
    let mut res: u128 = x as u128;
    res = (res | (res << 32)) & 0x00000000FFFFFFFF00000000FFFFFFFF_u128;
    res = (res | (res << 16)) & 0x0000FFFF0000FFFF0000FFFF0000FFFF_u128;
    res = (res | (res << 8)) & 0x00FF00FF00FF00FF00FF00FF00FF00FF_u128;
    res = (res | (res << 4)) & 0x0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F_u128;
    res = (res | (res << 2)) & 0x33333333333333333333333333333333_u128;
    res = (res | (res << 1)) & 0x55555555555555555555555555555555_u128;
    res
}

/// Interleaves two 64-bit values into a 128-bit Morton code.
fn interleave_64(x: u64, y: u64) -> u128 {
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
pub fn spiral_zorder(t: i64, dimensions: Vec<Option<String>>) -> pgrx::datum::AnyNumeric {
    let val = spiral_zorder_core(t, dimensions);
    pgrx::datum::AnyNumeric::try_from(val.to_string().as_str()).expect("valid numeric")
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
pub fn spiral_zorder_int_array(t: i64, dimensions: Vec<i32>) -> pgrx::datum::AnyNumeric {
    let val = spiral_zorder_int_array_core(t, dimensions);
    pgrx::datum::AnyNumeric::try_from(val.to_string().as_str()).expect("valid numeric")
}

pub fn zorder_3d_core(x: i64, y: i32, z: i32) -> u128 {
    let mut res = 0u128;
    for i in 0..42 {
        res |= ((x as u128 >> i) & 1) << (3 * i);
        res |= (((y as u128) >> i) & 1) << (3 * i + 1);
        res |= (((z as u128) >> i) & 1) << (3 * i + 2);
    }
    res
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_zorder_3d(x: i64, y: i32, z: i32) -> pgrx::datum::AnyNumeric {
    let val = zorder_3d_core(x, y, z);
    pgrx::datum::AnyNumeric::try_from(val.to_string().as_str()).expect("valid numeric")
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_hilbert_2d(x: i32, y: i32) -> i32 {
    let mut res = 0i32;
    for i in 0..15 {
        res |= ((x >> i) & 1) << (2 * i);
        res |= ((y >> i) & 1) << (2 * i + 1);
    }
    res
}
