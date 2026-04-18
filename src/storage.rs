use pgrx::prelude::*;
use std::fs::{OpenOptions, File};
use std::io::{Write, Seek, SeekFrom, Read};
use std::path::PathBuf;

pub const ROW_SIZE: usize = 64;
pub const MAX_TENANTS: usize = 1000;
pub const BUNDLE_SIZE: usize = MAX_TENANTS * ROW_SIZE;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AspiralingRow {
    pub t: i64,          // 8
    pub tenant_id: i32,  // 4
    pub _align: i32,     // 4 (Total 16)
    pub value: f64,      // 8 (Total 24)
    pub padding: [u8; 40], // 40 (Total 64)
}

pub fn get_storage_path(rel_oid: i32) -> PathBuf {
    let mut path = PathBuf::from("aspiral_main");
    if !path.exists() {
        let _ = std::fs::create_dir_all(&path);
    }
    path.push(format!("{}.bin", rel_oid));
    path
}

#[pg_extern]
pub fn aspiral_pack_delta(delta_table_name: &str, main_rel_oid: i32) {
    let path = get_storage_path(main_rel_oid);
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
pub fn aspiral_read_main(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let path = get_storage_path(main_rel_oid);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_o1_binary_math() {
        assert_eq!(std::mem::size_of::<AspiralingRow>(), 64);
        
        let test_oid = 12345;
        let path = get_storage_path(test_oid);
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
