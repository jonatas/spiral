use pgrx::prelude::*;
use std::fs::{OpenOptions, File};
use std::io::{Write, Seek, SeekFrom, Read};
use std::path::PathBuf;

pub const ROW_SIZE: usize = 64;
pub const COMPACT_ROW_SIZE: usize = 16;
pub const BLOCK_SIZE: usize = 128;
pub const POINTS_PER_BLOCK: i64 = 64;
pub const MAX_TENANTS: usize = 1000;
pub const BUNDLE_SIZE: usize = MAX_TENANTS * ROW_SIZE;
pub const COMPACT_BUNDLE_SIZE: usize = MAX_TENANTS * COMPACT_ROW_SIZE;
pub const BLOCK_BUNDLE_SIZE: usize = MAX_TENANTS * BLOCK_SIZE;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AspiralingRow {
    pub t: i64,          // 8
    pub tenant_id: i32,  // 4
    pub _align: i32,     // 4 (Total 16)
    pub value: f64,      // 8 (Total 24)
    pub padding: [u8; 40], // 40 (Total 64)
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct PackedRow {
    pub t_delta: i32,    // 4
    pub tenant_id: i32,  // 4
    pub value: f64,      // 8 (Total 16)
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct CompressedBlock {
    pub first_val: f64,
    pub data: [u8; 120], // Compressed XOR deltas
}

pub fn get_storage_path(rel_oid: i32, suffix: &str) -> PathBuf {
    let mut path = PathBuf::from("/tmp/aspiral_main");
    if !path.exists() {
        let _ = std::fs::create_dir_all(&path);
    }
    path.push(format!("{}{}.bin", rel_oid, suffix));
    path
}

#[pg_extern]
pub fn aspiral_pack_delta(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid, "");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(path)
        .expect("Failed to open Main Store file");

    Spi::connect(|client| {
        let query = format!("SELECT t, tenant_id, price FROM {} ORDER BY t ASC", delta_table_name);
        let tuple_table = client.select(&query, None, &[])?;

        for row in tuple_table {
            let t = row.get::<i64>(1)?.unwrap();
            let tenant_id = row.get::<i64>(2)?.unwrap();
            let value = row.get::<f64>(3)?.unwrap();

            let offset = (t * BUNDLE_SIZE as i64) + (tenant_id * ROW_SIZE as i64);
            
            let data = AspiralingRow {
                t,
                tenant_id: tenant_id as i32,
                _align: 0,
                value,
                padding: [0u8; 40],
            };

            let bytes: [u8; ROW_SIZE] = unsafe { std::mem::transmute(data) };
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            file.write_all(&bytes).unwrap();
        }
        
        Ok::<(), spi::Error>(())
    }).unwrap();
}

#[pg_extern]
pub fn aspiral_pack_delta_compact(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid, "_compact");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(path)
        .expect("Failed to open Compact Main Store file");

    Spi::connect(|client| {
        let query = format!("SELECT t, tenant_id, price FROM {} ORDER BY t ASC", delta_table_name);
        let tuple_table = client.select(&query, None, &[])?;

        for row in tuple_table {
            let t = row.get::<i64>(1)?.unwrap();
            let tenant_id = row.get::<i64>(2)?.unwrap();
            let value = row.get::<f64>(3)?.unwrap();

            let offset = (t * COMPACT_BUNDLE_SIZE as i64) + (tenant_id * COMPACT_ROW_SIZE as i64);
            
            let data = PackedRow {
                t_delta: t as i32,
                tenant_id: tenant_id as i32,
                value,
            };

            let bytes: [u8; COMPACT_ROW_SIZE] = unsafe { std::mem::transmute(data) };
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            file.write_all(&bytes).unwrap();
        }
        
        Ok::<(), spi::Error>(())
    }).unwrap();
}

#[pg_extern]
pub fn aspiral_pack_delta_blocks(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid, "_blocks");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(path)
        .expect("Failed to open Blocks Main Store file");

    Spi::connect(|client| {
        // Group by Block and Tenant
        let query = format!(
            "SELECT (t / {0}) as block_id, tenant_id, array_agg(price ORDER BY t) as prices 
             FROM {1} 
             GROUP BY 1, 2", 
            POINTS_PER_BLOCK, delta_table_name
        );
        let tuple_table = client.select(&query, None, &[])?;

        for row in tuple_table {
            let block_id = row.get::<i64>(1)?.unwrap();
            let tenant_id = row.get::<i64>(2)?.unwrap();
            let prices: Vec<f64> = row.get::<Vec<f64>>(3)?.unwrap();

            let mut block = CompressedBlock {
                first_val: prices[0],
                data: [0u8; 120],
            };

            // Simplified XOR Delta-Delta: Store first 60 XORed differences in 120 bytes (2 bytes each)
            // This is a LOSSIVE prototype optimization for the sake of the fixed-size O(1) read.
            // In a real system, we'd use bit-packing to be lossless within the 120 bytes.
            for (i, val) in prices.iter().enumerate().skip(1) {
                if i > 60 { break; }
                let xor_delta = (val.to_bits() ^ prices[i-1].to_bits()) as u16; 
                let bytes = xor_delta.to_le_bytes();
                block.data[(i-1)*2] = bytes[0];
                block.data[(i-1)*2 + 1] = bytes[1];
            }

            let offset = (block_id * BLOCK_BUNDLE_SIZE as i64) + (tenant_id * BLOCK_SIZE as i64);
            let bytes: [u8; BLOCK_SIZE] = unsafe { std::mem::transmute(block) };
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            file.write_all(&bytes).unwrap();
        }
        
        Ok::<(), spi::Error>(())
    }).unwrap();
}

#[pg_extern]
pub fn aspiral_read_main(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid, "");
    let mut file = File::open(path).ok()?;
    
    let offset = (t * BUNDLE_SIZE as i64) + (tenant_id * ROW_SIZE as i64);
    let mut buffer = [0u8; ROW_SIZE];
    
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    file.read_exact(&mut buffer).ok()?;
    
    let row: AspiralingRow = unsafe { std::mem::transmute(buffer) };
    
    if row.t == t && row.tenant_id == tenant_id as i32 {
        Some(row.value)
    } else {
        None
    }
}

#[pg_extern]
pub fn aspiral_read_main_compact(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid, "_compact");
    let mut file = File::open(path).ok()?;
    
    let offset = (t * COMPACT_BUNDLE_SIZE as i64) + (tenant_id * COMPACT_ROW_SIZE as i64);
    let mut buffer = [0u8; COMPACT_ROW_SIZE];
    
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    file.read_exact(&mut buffer).ok()?;
    
    let row: PackedRow = unsafe { std::mem::transmute(buffer) };
    
    if row.t_delta == t as i32 && row.tenant_id == tenant_id as i32 {
        Some(row.value)
    } else {
        None
    }
}

#[pg_extern]
pub fn aspiral_read_main_block_point(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid, "_blocks");
    let mut file = File::open(path).ok()?;
    
    let block_id = t / POINTS_PER_BLOCK;
    let offset_in_block = t % POINTS_PER_BLOCK;
    
    let offset = (block_id * BLOCK_BUNDLE_SIZE as i64) + (tenant_id * BLOCK_SIZE as i64);
    let mut buffer = [0u8; BLOCK_SIZE];
    
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    file.read_exact(&mut buffer).ok()?;
    
    let block: CompressedBlock = unsafe { std::mem::transmute(buffer) };
    
    if offset_in_block == 0 {
        return Some(block.first_val);
    }

    let mut current_bits = block.first_val.to_bits();
    for i in 1..=offset_in_block {
        if i > 60 { break; } // Prototype limit
        let low_bits = u16::from_le_bytes([block.data[(i as usize -1)*2], block.data[(i as usize -1)*2+1]]);
        current_bits ^= low_bits as u64;
    }
    
    Some(f64::from_bits(current_bits))
}

#[pg_extern]
pub fn aspiral_read_main_block_range(main_rel_oid: i32, block_id: i64, tenant_id: i64) -> Vec<f64> {
    let mut res = Vec::with_capacity(POINTS_PER_BLOCK as usize);
    let path = get_storage_path(main_rel_oid, "_blocks");
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return res,
    };
    
    let offset = (block_id * BLOCK_BUNDLE_SIZE as i64) + (tenant_id * BLOCK_SIZE as i64);
    let mut buffer = [0u8; BLOCK_SIZE];
    
    if file.seek(SeekFrom::Start(offset as u64)).is_err() { return res; }
    if file.read_exact(&mut buffer).is_err() { return res; }
    
    let block: CompressedBlock = unsafe { std::mem::transmute(buffer) };
    
    let mut current_bits = block.first_val.to_bits();
    res.push(block.first_val);

    for i in 1..POINTS_PER_BLOCK {
        if i > 60 { 
            res.push(0.0); // Padding for prototype limit
            continue; 
        }
        let low_bits = u16::from_le_bytes([block.data[(i as usize -1)*2], block.data[(i as usize -1)*2+1]]);
        current_bits ^= low_bits as u64;
        res.push(f64::from_bits(current_bits));
    }
    
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_o1_binary_math() {
        assert_eq!(std::mem::size_of::<AspiralingRow>(), 64);
        
        let test_oid = 12345;
        let path = get_storage_path(test_oid, "");
        if path.exists() { let _ = fs::remove_file(&path); }

        let mut file = OpenOptions::new().write(true).create(true).open(&path).unwrap();
        
        let test_t = 100;
        let test_tenant = 5;
        let test_val = 99.99;

        let offset = (test_t * BUNDLE_SIZE as i64) + (test_tenant * ROW_SIZE as i64);
        let data = AspiralingRow {
            t: test_t,
            tenant_id: test_tenant as i32,
            _align: 0,
            value: test_val,
            padding: [0u8; 40],
        };

        let bytes: [u8; ROW_SIZE] = unsafe { std::mem::transmute(data) };
        file.seek(SeekFrom::Start(offset as u64)).unwrap();
        file.write_all(&bytes).unwrap();
        drop(file);

        let mut read_file = File::open(&path).unwrap();
        let mut buffer = [0u8; ROW_SIZE];
        read_file.seek(SeekFrom::Start(offset as u64)).unwrap();
        read_file.read_exact(&mut buffer).unwrap();
        let row: AspiralingRow = unsafe { std::mem::transmute(buffer) };
        
        assert_eq!(row.t, test_t);
        assert_eq!(row.tenant_id, test_tenant as i32);
        assert_eq!(row.value, test_val);
        
        let _ = fs::remove_file(&path);
    }
}
