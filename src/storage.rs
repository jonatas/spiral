use pgrx::prelude::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const BLOCK_SIZE: usize = 128; // 128 bytes per sensor/block
const POINTS_PER_BLOCK: i64 = 64;
const BLOCK_BUNDLE_SIZE: usize = BLOCK_SIZE * 1024; // Pre-allocate for 1024 tenants

#[derive(Clone, Copy)]
#[repr(C)]
struct CompressedBlock {
    first_val: f64,
    data: [u8; 120], // 60 XORed deltas (2 bytes each)
}

fn get_storage_path(rel_oid: i32, suffix: &str) -> PathBuf {
    let mut path = PathBuf::from("/tmp/spiral_main/");
    if !path.exists() {
        let _ = std::fs::create_dir_all(&path);
    }
    path.push(format!("{}{}.bin", rel_oid, suffix));
    path
}

#[pg_extern]
pub fn spiral_pack_delta(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid, "");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("Failed to open Main Store file");

    let kickoff = crate::get_kickoff_epoch();
    notice!("Spiral: packing delta from '{}' to '{}' (kickoff={})", delta_table_name, path.display(), kickoff);
    
    let _ = Spi::connect(|client| {
        let query = format!(
            "SELECT (spiral(t) - {kickoff}) as t_rel, tenant_id, price FROM {delta_table_name} ORDER BY t ASC",
            kickoff = kickoff, delta_table_name = delta_table_name
        );
        let tuple_table = client.select(&query, None, &[])?;
        let mut count = 0;

        for row in tuple_table {
            let t = row.get::<i64>(1)?.unwrap_or(-1);
            let tenant_id = row.get::<i64>(2)?.unwrap_or(-1);
            let price = row.get::<f64>(3)?.unwrap_or(0.0);

            if t < 0 || !(0..1024).contains(&tenant_id) {
                continue;
            }

            let offset = (t * 1024 * 8) + (tenant_id * 8);
            if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
                if file.write_all(&price.to_le_bytes()).is_ok() {
                    count += 1;
                }
            }
        }
        notice!("Spiral: packed {} rows into O(1) store", count);
        Ok::<(), spi::Error>(())
    });
}

#[pg_extern]
pub fn spiral_pack_delta_compact(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid, "_compact");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("Failed to open Compact Store file");

    let kickoff = crate::get_kickoff_epoch();
    let _ = Spi::connect(|client| {
        let query = format!(
            "SELECT (spiral(t) - {}) as t_rel, tenant_id, price FROM {} ORDER BY t ASC",
            kickoff, delta_table_name
        );
        let tuple_table = client.select(&query, None, &[])?;

        for row in tuple_table {
            let t = row.get::<i64>(1)?.unwrap_or(0);
            let tenant_id = row.get::<i64>(2)?.unwrap_or(0);
            let price = row.get::<f64>(3)?.unwrap_or(0.0);

            if t < 0 || !(0..1024).contains(&tenant_id) {
                continue;
            }

            let offset = (t * 1024 * 16) + (tenant_id * 16);
            if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
                let _ = file.write_all(&(t as u32).to_le_bytes());
                let _ = file.write_all(&(price as f32).to_le_bytes());
            }
        }
        Ok::<(), spi::Error>(())
    });
}

#[pg_extern]
pub fn spiral_pack_delta_blocks(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid, "_blocks");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("Failed to open Blocks Main Store file");

    let _ = Spi::connect(|client| {
        let reading_col = client.select(&format!("SELECT attname FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped AND (attname = 'price' OR attname = 'reading' OR attname = 'val') LIMIT 1", delta_table_name.replace("\"", "\"\"")), None, &[])?.get_one::<String>()?.unwrap_or("price".to_string());
        let tenant_col = client.select(&format!("SELECT attname FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped AND (attname = 'tenant_id' OR attname = 'sensor_id') LIMIT 1", delta_table_name.replace("\"", "\"\"")), None, &[])?.get_one::<String>()?.unwrap_or("tenant_id".to_string());

        let kickoff = crate::get_kickoff_epoch();
        let query = format!(
            "SELECT ((spiral(t) - {0}) / {1}) as block_id, {2}, array_agg({3} ORDER BY t) as prices
             FROM {4}
             GROUP BY 1, 2",
            kickoff, POINTS_PER_BLOCK, tenant_col, reading_col, delta_table_name
        );
        let tuple_table = client.select(&query, None, &[])?;

        for row in tuple_table {
            let block_id = row.get::<i64>(1)?.unwrap_or(0);
            let tenant_id = row.get::<i64>(2)?.unwrap_or(0);
            let prices: Vec<f64> = row.get::<Vec<f64>>(3)?.unwrap_or_default();

            if prices.is_empty() || block_id < 0 || !(0..1024).contains(&tenant_id) {
                continue;
            }

            let mut block = CompressedBlock {
                first_val: prices[0],
                data: [0u8; 120],
            };

            for (i, val) in prices.iter().enumerate().skip(1) {
                if i > 60 {
                    break;
                }
                let xor_delta = (val.to_bits() ^ prices[i - 1].to_bits()) as u16;
                let bytes = xor_delta.to_le_bytes();
                block.data[(i - 1) * 2] = bytes[0];
                block.data[(i - 1) * 2 + 1] = bytes[1];
            }

            let offset = (block_id * BLOCK_BUNDLE_SIZE as i64) + (tenant_id * BLOCK_SIZE as i64);
            let bytes: [u8; BLOCK_SIZE] = unsafe { std::mem::transmute(block) };
            if file.seek(SeekFrom::Start(offset as u64)).is_ok() {
                let _ = file.write_all(&bytes);
            }
        }
        Ok::<(), spi::Error>(())
    });
}

#[pg_extern]
pub fn spiral_read_main(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid, "");
    let mut file = File::open(path).ok()?;
    let kickoff = crate::get_kickoff_epoch();
    let t_rel = t - kickoff;

    if t_rel < 0 || !(0..1024).contains(&tenant_id) {
        return None;
    }
    let offset = (t_rel * 1024 * 8) + (tenant_id * 8);
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).ok()?;
    Some(f64::from_le_bytes(buf))
}

#[pg_extern]
pub fn spiral_read_main_compact(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid, "_compact");
    let mut file = File::open(path).ok()?;
    if t < 0 || !(0..1024).contains(&tenant_id) {
        return None;
    }
    let offset = (t * 1024 * 16) + (tenant_id * 16);
    file.seek(SeekFrom::Start(offset as u64 + 4)).ok()?;
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).ok()?;
    Some(f32::from_le_bytes(buf) as f64)
}

#[pg_extern]
pub fn spiral_read_main_block_point(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid, "_blocks");
    let mut file = File::open(path).ok()?;
    if t < 0 || !(0..1024).contains(&tenant_id) {
        return None;
    }
    let block_id = t / POINTS_PER_BLOCK;
    let step = (t % POINTS_PER_BLOCK) as usize;

    let offset = (block_id * BLOCK_BUNDLE_SIZE as i64) + (tenant_id * BLOCK_SIZE as i64);
    file.seek(SeekFrom::Start(offset as u64)).ok()?;

    let mut buf = [0u8; BLOCK_SIZE];
    file.read_exact(&mut buf).ok()?;
    let block: CompressedBlock = unsafe { std::mem::transmute(buf) };

    let mut current_bits = block.first_val.to_bits();
    if step == 0 {
        return Some(block.first_val);
    }

    for i in 0..step {
        if i >= 60 {
            break;
        }
        let xor_delta = u16::from_le_bytes([block.data[i * 2], block.data[i * 2 + 1]]) as u64;
        current_bits ^= xor_delta;
    }

    Some(f64::from_bits(current_bits))
}

#[pg_extern]
pub fn spiral_read_main_block_range(main_rel_oid: i32, block_id: i64, tenant_id: i64) -> Vec<f64> {
    let path = get_storage_path(main_rel_oid, "_blocks");
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    if block_id < 0 || !(0..1024).contains(&tenant_id) {
        return vec![];
    }
    let offset = (block_id * BLOCK_BUNDLE_SIZE as i64) + (tenant_id * BLOCK_SIZE as i64);
    if file.seek(SeekFrom::Start(offset as u64)).is_err() {
        return vec![];
    }

    let mut buf = [0u8; BLOCK_SIZE];
    if file.read_exact(&mut buf).is_err() {
        return vec![];
    }
    let block: CompressedBlock = unsafe { std::mem::transmute(buf) };

    let mut results = Vec::with_capacity(64);
    let mut current_bits = block.first_val.to_bits();
    results.push(block.first_val);

    for i in 0..60 {
        let xor_delta = u16::from_le_bytes([block.data[i * 2], block.data[i * 2 + 1]]) as u64;
        current_bits ^= xor_delta;
        results.push(f64::from_bits(current_bits));
    }

    results
}

#[pg_extern]
pub fn spiral_scan_zero(
    main_rel_oid: i32,
) -> TableIterator<'static, (name!(t, i64), name!(tenant_id, i32), name!(value, f64))> {
    let path = get_storage_path(main_rel_oid, "");
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return TableIterator::new(Vec::new()),
    };
    let metadata = file.metadata().unwrap();
    let total_size = metadata.len();

    let mut results = Vec::new();
    let mut buf = [0u8; 8];

    let total_slots = total_size / 8;
    for i in 0..total_slots {
        if file.read_exact(&mut buf).is_ok() {
            let val = f64::from_le_bytes(buf);
            if val != 0.0 {
                let t = (i / 1024) as i64;
                let tenant_id = (i % 1024) as i32;
                results.push((t, tenant_id, val));
            }
        }
    }

    TableIterator::new(results)
}
