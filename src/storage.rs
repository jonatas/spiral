use pgrx::pg_sys;
use pgrx::prelude::*;
use pgrx::PgRelation;

const BLOCK_SIZE: usize = 128; // 128 bytes per sensor/block
const POINTS_PER_BLOCK: i64 = 64;
pub const BLCKSZ: usize = 8192;
pub const HEADER_SIZE: usize = 24; // std::mem::size_of::<pg_sys::PageHeaderData>()
pub const SPECIAL_SIZE: usize = 24; // std::mem::size_of::<SpiralPageOpaque>()
pub const DATA_PER_PAGE: usize = (BLCKSZ - HEADER_SIZE - SPECIAL_SIZE) / 8;

#[derive(Clone, Copy)]
#[repr(C)]
struct CompressedBlock {
    first_val: f64,
    data: [u8; 120], // 60 XORed deltas (2 bytes each)
}

#[repr(C)]
pub struct SpiralPageOpaque {
    pub window_start_t: i64,
    pub window_end_t: i64,
    pub tenant_scale: i32,
    pub magic: u32, // use 0x50495241 ('SPRA')
}

pub fn logical_to_physical_offset(logical_offset: i64) -> (u32, u32) {
    let index = logical_offset / 8;
    let blkno = (index / DATA_PER_PAGE as i64) as u32;
    let offset_in_page = (HEADER_SIZE as i64 + (index % DATA_PER_PAGE as i64) * 8) as u32;
    (blkno, offset_in_page)
}

unsafe fn get_block_count(rel: pg_sys::Relation) -> u32 {
    pg_sys::RelationGetSmgr(rel);
    pg_sys::smgrnblocks((*rel).rd_smgr, 0)
}

fn get_tenant_scale_for_oid(rel_oid: i32) -> i64 {
    unsafe {
        let relname_ptr = pg_sys::get_rel_name((rel_oid as u32).into());
        if !relname_ptr.is_null() {
            let name = std::ffi::CStr::from_ptr(relname_ptr)
                .to_string_lossy()
                .into_owned();
            pg_sys::pfree(relname_ptr as *mut std::ffi::c_void);
            if let Some(m) = crate::catalog::get_metadata(&name) {
                return crate::catalog::get_tenant_scale(&m);
            }
        }
    }
    1024
}

#[pg_extern]
pub fn spiral_pack_delta(delta_table_name: &str, main_rel_oid: i32) {
    let kickoff = crate::get_kickoff_epoch();
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);

    let rows: Vec<(i64, i64, f64)> = Spi::connect(|client| {
        let query = format!(
            "SELECT (spiral(t) - {kickoff}) as t_rel, tenant_id, price FROM {delta_table_name} ORDER BY t ASC",
            kickoff = kickoff, delta_table_name = delta_table_name
        );
        let tuple_table = client.select(&query, None, &[])?;
        let mut results = Vec::new();

        for row in tuple_table {
            let t = row.get::<i64>(1)?.unwrap_or(-1);
            let tenant_id = row.get::<i64>(2)?.unwrap_or(-1);
            let price = row.get::<f64>(3)?.unwrap_or(0.0);
            if t >= 0 && (0..tenant_scale).contains(&tenant_id) {
                results.push((t, tenant_id, price));
            }
        }
        Ok::<Vec<(i64, i64, f64)>, spi::Error>(results)
    }).unwrap_or_default();

    if rows.is_empty() {
        notice!("Spiral: no rows to pack for {}", delta_table_name);
        return;
    }

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::RowExclusiveLock as i32,
        );
        let rel = pg_rel.as_ptr();
        if rel.is_null() {
            panic!("Spiral: relation pointer is NULL for OID {}", main_rel_oid);
        }

        let mut count = 0;
        for (t, tenant_id, price) in rows {
            let logical_offset = (t * tenant_scale * 8) + (tenant_id * 8);
            let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

            let mut nblocks = get_block_count(rel);
            while nblocks <= blkno {
                let buffer = pg_sys::ReadBuffer(rel, pg_sys::InvalidBlockNumber);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);
                let page = pg_sys::BufferGetPage(buffer);
                pg_sys::PageInit(
                    page,
                    BLCKSZ as pg_sys::Size,
                    std::mem::size_of::<SpiralPageOpaque>() as pg_sys::Size,
                );
                pg_sys::MarkBufferDirty(buffer);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
                nblocks += 1;
                if nblocks > 100000 {
                    break;
                }
            }

            let buffer = pg_sys::ReadBuffer(rel, blkno);
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);

            let page = pg_sys::BufferGetPage(buffer);
            let ptr = (page as *mut u8).add(page_offset as usize);
            *(ptr as *mut f64) = price;

            pg_sys::MarkBufferDirty(buffer);
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
            pg_sys::ReleaseBuffer(buffer);
            count += 1;
        }
        notice!(
            "Spiral: packed {} rows into O(1) buffer-managed store",
            count
        );
    }
}

#[pg_extern]
pub fn spiral_pack_delta_compact(delta_table_name: &str, main_rel_oid: i32) {
    let kickoff = crate::get_kickoff_epoch();
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::RowExclusiveLock as i32,
        );
        let rel = pg_rel.as_ptr();

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

                if t < 0 || !(0..tenant_scale).contains(&tenant_id) {
                    continue;
                }
                let logical_offset = (t * tenant_scale * 16) + (tenant_id * 16);
                let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

                let mut nblocks = pg_sys::RelationGetNumberOfBlocksInFork(rel, 0);
                while nblocks <= blkno {
                    let buffer = pg_sys::ReadBuffer(rel, pg_sys::InvalidBlockNumber);
                    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);
                    let page = pg_sys::BufferGetPage(buffer);

                    pg_sys::PageInit(
                        page,
                        BLCKSZ as pg_sys::Size,
                        std::mem::size_of::<SpiralPageOpaque>() as pg_sys::Size,
                    );

                    pg_sys::MarkBufferDirty(buffer);
                    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                    pg_sys::ReleaseBuffer(buffer);
                    nblocks += 1;
                    if nblocks > 200000 {
                        break;
                    } // Safety break
                }

                let state = pg_sys::GenericXLogStart(rel);
                let buffer = pg_sys::ReadBuffer(rel, blkno);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);

                let page = pg_sys::GenericXLogRegisterBuffer(state, buffer, 0);
                let ptr = (page as *mut u8).add(page_offset as usize);

                *(ptr as *mut u32) = t as u32;
                *(ptr.add(4) as *mut f32) = price as f32;

                pg_sys::MarkBufferDirty(buffer);
                pg_sys::GenericXLogFinish(state);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
            }
            Ok::<(), spi::Error>(())
        });
    }
}

#[pg_extern]
pub fn spiral_pack_delta_blocks(delta_table_name: &str, main_rel_oid: i32) {
    let kickoff = crate::get_kickoff_epoch();
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::RowExclusiveLock as i32,
        );
        let rel = pg_rel.as_ptr();

        let _ = Spi::connect(|client| {
            let reading_col = client.select(&format!("SELECT attname FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped AND (attname = 'price' OR attname = 'reading' OR attname = 'val') LIMIT 1", delta_table_name.replace("\"", "\"\"")), None, &[])?.get_one::<String>()?.unwrap_or("price".to_string());
            let tenant_col = client.select(&format!("SELECT attname FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped AND (attname = 'tenant_id' OR attname = 'sensor_id') LIMIT 1", delta_table_name.replace("\"", "\"\"")), None, &[])?.get_one::<String>()?.unwrap_or("tenant_id".to_string());

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

                if prices.is_empty() || block_id < 0 || !(0..tenant_scale).contains(&tenant_id) {
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

                let bundle_size = BLOCK_SIZE as i64 * tenant_scale;
                let logical_offset = (block_id * bundle_size) + (tenant_id * BLOCK_SIZE as i64);
                let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

                let mut nblocks = pg_sys::RelationGetNumberOfBlocksInFork(rel, 0);
                while nblocks <= blkno {
                    let buffer = pg_sys::ReadBuffer(rel, pg_sys::InvalidBlockNumber);
                    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);
                    let page = pg_sys::BufferGetPage(buffer);
                    pg_sys::PageInit(
                        page,
                        BLCKSZ as pg_sys::Size,
                        std::mem::size_of::<SpiralPageOpaque>() as pg_sys::Size,
                    );
                    pg_sys::MarkBufferDirty(buffer);
                    pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                    pg_sys::ReleaseBuffer(buffer);
                    nblocks += 1;
                    if nblocks > 200000 {
                        break;
                    } // Safety break
                }

                let state = pg_sys::GenericXLogStart(rel);
                let buffer = pg_sys::ReadBuffer(rel, blkno);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_EXCLUSIVE as i32);

                let page = pg_sys::GenericXLogRegisterBuffer(state, buffer, 0);
                let ptr = (page as *mut u8).add(page_offset as usize);
                let bytes: [u8; BLOCK_SIZE] = std::mem::transmute(block);
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, BLOCK_SIZE);

                pg_sys::MarkBufferDirty(buffer);
                pg_sys::GenericXLogFinish(state);
                pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
                pg_sys::ReleaseBuffer(buffer);
            }
            Ok::<(), spi::Error>(())
        });
    }
}

#[pg_extern]
pub fn spiral_read_main(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let kickoff = crate::get_kickoff_epoch();
    let t_rel = t - kickoff;
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);
    if t_rel < 0 || !(0..tenant_scale).contains(&tenant_id) {
        return None;
    }

    let logical_offset = (t_rel * tenant_scale * 8) + (tenant_id * 8);
    let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::AccessShareLock as i32,
        );
        let rel = pg_rel.as_ptr();

        if blkno >= get_block_count(rel) {
            return None;
        }

        let buffer = pg_sys::ReadBuffer(rel, blkno);
        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
        let page = pg_sys::BufferGetPage(buffer);
        let ptr = (page as *const u8).add(page_offset as usize);
        let val = *(ptr as *const f64);

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);

        Some(val)
    }
}

#[pg_extern]
pub fn spiral_read_main_compact(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);
    if t < 0 || !(0..tenant_scale).contains(&tenant_id) {
        return None;
    }
    let logical_offset = (t * tenant_scale * 16) + (tenant_id * 16);
    let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::AccessShareLock as i32,
        );
        let rel = pg_rel.as_ptr();

        if blkno >= get_block_count(rel) {
            return None;
        }

        let buffer = pg_sys::ReadBuffer(rel, blkno);
        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
        let page = pg_sys::BufferGetPage(buffer);
        let ptr = (page as *const u8).add(page_offset as usize + 4);
        let val = *(ptr as *const f32) as f64;

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);

        Some(val)
    }
}

#[pg_extern]
pub fn spiral_read_main_block_point(main_rel_oid: i32, t: i64, tenant_id: i64) -> Option<f64> {
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);
    if t < 0 || !(0..tenant_scale).contains(&tenant_id) {
        return None;
    }
    let block_id = t / POINTS_PER_BLOCK;
    let step = (t % POINTS_PER_BLOCK) as usize;

    let bundle_size = BLOCK_SIZE as i64 * tenant_scale;
    let logical_offset = (block_id * bundle_size) + (tenant_id * BLOCK_SIZE as i64);
    let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::AccessShareLock as i32,
        );
        let rel = pg_rel.as_ptr();

        if blkno >= get_block_count(rel) {
            return None;
        }

        let buffer = pg_sys::ReadBuffer(rel, blkno);
        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
        let page = pg_sys::BufferGetPage(buffer);
        let ptr = (page as *const u8).add(page_offset as usize);

        let mut buf = [0u8; BLOCK_SIZE];
        std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), BLOCK_SIZE);
        let block: CompressedBlock = std::mem::transmute(buf);

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);

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
}

#[pg_extern]
pub fn spiral_read_main_block_range(main_rel_oid: i32, block_id: i64, tenant_id: i64) -> Vec<f64> {
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);
    if block_id < 0 || !(0..tenant_scale).contains(&tenant_id) {
        return vec![];
    }
    let bundle_size = BLOCK_SIZE as i64 * tenant_scale;
    let logical_offset = (block_id * bundle_size) + (tenant_id * BLOCK_SIZE as i64);
    let (blkno, page_offset) = logical_to_physical_offset(logical_offset);

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::AccessShareLock as i32,
        );
        let rel = pg_rel.as_ptr();

        if blkno >= pg_sys::RelationGetNumberOfBlocksInFork(rel, 0) {
            return vec![];
        }

        let buffer = pg_sys::ReadBuffer(rel, blkno);
        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
        let page = pg_sys::BufferGetPage(buffer);
        let ptr = (page as *const u8).add(page_offset as usize);

        let mut buf = [0u8; BLOCK_SIZE];
        std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), BLOCK_SIZE);
        let block: CompressedBlock = std::mem::transmute(buf);

        pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
        pg_sys::ReleaseBuffer(buffer);

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
}

#[pg_extern]
pub fn spiral_scan_zero(
    main_rel_oid: i32,
) -> TableIterator<'static, (name!(t, i64), name!(tenant_id, i32), name!(value, f64))> {
    let tenant_scale = get_tenant_scale_for_oid(main_rel_oid);
    let mut results = Vec::new();

    unsafe {
        let pg_rel = PgRelation::with_lock(
            pg_sys::Oid::from(main_rel_oid as u32),
            pg_sys::AccessShareLock as i32,
        );
        let rel = pg_rel.as_ptr();

        let nblocks = get_block_count(rel);

        for blkno in 0..nblocks {
            let buffer = pg_sys::ReadBuffer(rel, blkno);
            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_SHARE as i32);
            let page = pg_sys::BufferGetPage(buffer);

            let mut offset = HEADER_SIZE;
            while offset + 8 <= (BLCKSZ - SPECIAL_SIZE) {
                let ptr = (page as *const u8).add(offset);
                let val = *(ptr as *const f64);
                if val != 0.0 {
                    let items_before = (offset - HEADER_SIZE) / 8;
                    let idx = (blkno as i64 * DATA_PER_PAGE as i64) + items_before as i64;
                    let t = idx / tenant_scale;
                    let tenant_id = (idx % tenant_scale) as i32;
                    results.push((t, tenant_id, val));
                }
                offset += 8;
            }

            pg_sys::LockBuffer(buffer, pg_sys::BUFFER_LOCK_UNLOCK as i32);
            pg_sys::ReleaseBuffer(buffer);
        }
    }

    TableIterator::new(results)
}
