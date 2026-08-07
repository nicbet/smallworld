//! Region file format for persistent brick caching.
//!
//! Each region covers a 16×16×16 cube of grid cells. File layout:
//!
//! ```text
//! Header: 4096 entries × 4 bytes = 16 KB
//!   Each entry: offset (24 bits) + sector_count (8 bits)
//!   offset = sector index (× SECTOR_SIZE), 0 = empty/air
//! Sectors: 4 KB each, zstd-compressed brick payloads
//! ```

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use smallworld_engine::brick_data::BrickData;

const REGION_EDGE: u32 = 16;
const HEADER_ENTRIES: usize = (REGION_EDGE * REGION_EDGE * REGION_EDGE) as usize;
const HEADER_SIZE: u64 = (HEADER_ENTRIES * 4) as u64;
const SECTOR_SIZE: u64 = 4096;
const ZSTD_LEVEL: i32 = 3;

/// Sentinel stored in the header to mark a cell as confirmed air (not just
/// "never written"). Sector count = 0xFF with offset = 0 can't occur naturally
/// since offset 0 is the header itself.
const AIR_SENTINEL: u32 = 0xFF;

/// A region file storing up to 16³ bricks.
pub struct RegionFile {
    file: fs::File,
    header: [u32; HEADER_ENTRIES],
}

impl RegionFile {
    /// Opens an existing region file or creates a new one.
    pub fn open_or_create(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let exists = path.exists();
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let mut header = [0u32; HEADER_ENTRIES];

        if exists && file.metadata()?.len() >= HEADER_SIZE {
            file.seek(SeekFrom::Start(0))?;
            let header_bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut header);
            file.read_exact(header_bytes)?;
        } else {
            file.set_len(HEADER_SIZE)?;
            file.seek(SeekFrom::Start(0))?;
            let header_bytes: &[u8] = bytemuck::cast_slice(&header);
            file.write_all(header_bytes)?;
        }

        Ok(Self { file, header })
    }

    /// Reads a brick from the region. Returns `None` for air or unwritten cells.
    pub fn read_brick(&mut self, local_pos: [u32; 3]) -> io::Result<Option<BrickData>> {
        let idx = Self::header_index(local_pos);
        let entry = self.header[idx];

        if entry == 0 {
            return Ok(None); // never written
        }
        if entry == AIR_SENTINEL {
            return Ok(None); // confirmed air
        }

        let offset = (entry >> 8) as u64;
        let sector_count = (entry & 0xFF) as u64;
        if offset == 0 || sector_count == 0 {
            return Ok(None);
        }

        let file_offset = offset * SECTOR_SIZE;
        self.file.seek(SeekFrom::Start(file_offset))?;

        let mut len_bytes = [0u8; 4];
        self.file.read_exact(&mut len_bytes)?;
        let payload_len = u32::from_le_bytes(len_bytes) as usize;

        let mut compressed = vec![0u8; payload_len];
        self.file.read_exact(&mut compressed)?;

        let decompressed = zstd::decode_all(&compressed[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        deserialize_brick(&decompressed)
    }

    /// Writes a brick to the region. Pass `None` to mark a cell as confirmed air.
    pub fn write_brick(&mut self, local_pos: [u32; 3], data: Option<&BrickData>) -> io::Result<()> {
        let idx = Self::header_index(local_pos);

        let data = match data {
            None => {
                self.header[idx] = AIR_SENTINEL;
                self.flush_header_entry(idx)?;
                return Ok(());
            }
            Some(d) => d,
        };

        let raw = serialize_brick(data);
        let compressed = zstd::encode_all(&raw[..], ZSTD_LEVEL).map_err(io::Error::other)?;

        let payload_len = compressed.len() as u32;
        let total_bytes = 4 + compressed.len() as u64;
        let sector_count = total_bytes.div_ceil(SECTOR_SIZE);

        let file_len = self.file.metadata()?.len();
        let offset = file_len.div_ceil(SECTOR_SIZE).max(1); // skip header sector(s)

        let padded_len = sector_count * SECTOR_SIZE;
        self.file.seek(SeekFrom::Start(offset * SECTOR_SIZE))?;
        self.file.write_all(&payload_len.to_le_bytes())?;
        self.file.write_all(&compressed)?;

        let written = 4 + compressed.len() as u64;
        if written < padded_len {
            let padding = vec![0u8; (padded_len - written) as usize];
            self.file.write_all(&padding)?;
        }

        let entry = ((offset as u32) << 8) | (sector_count as u32 & 0xFF);
        self.header[idx] = entry;
        self.flush_header_entry(idx)?;

        Ok(())
    }

    /// Whether this cell has been written (data or air).
    pub fn has_entry(&self, local_pos: [u32; 3]) -> bool {
        let idx = Self::header_index(local_pos);
        self.header[idx] != 0
    }

    fn flush_header_entry(&mut self, idx: usize) -> io::Result<()> {
        let offset = (idx * 4) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(&self.header[idx].to_le_bytes())?;
        Ok(())
    }

    fn header_index(pos: [u32; 3]) -> usize {
        debug_assert!(
            pos[0] < REGION_EDGE && pos[1] < REGION_EDGE && pos[2] < REGION_EDGE,
            "local pos {:?} out of region bounds",
            pos
        );
        (pos[0] + REGION_EDGE * (pos[1] + REGION_EDGE * pos[2])) as usize
    }
}

/// Builds a region file path from region coordinates.
pub fn region_path(base_dir: &Path, region_pos: [u32; 3]) -> PathBuf {
    base_dir.join(format!(
        "r.{}.{}.{}.swr",
        region_pos[0], region_pos[1], region_pos[2]
    ))
}

/// Splits a grid position into region + local coordinates.
pub fn split_grid_pos(grid_pos: [u32; 3]) -> ([u32; 3], [u32; 3]) {
    let region = [
        grid_pos[0] / REGION_EDGE,
        grid_pos[1] / REGION_EDGE,
        grid_pos[2] / REGION_EDGE,
    ];
    let local = [
        grid_pos[0] % REGION_EDGE,
        grid_pos[1] % REGION_EDGE,
        grid_pos[2] % REGION_EDGE,
    ];
    (region, local)
}

fn serialize_brick(data: &BrickData) -> Vec<u8> {
    let palette_len = data.palette.len() as u8;
    let mut buf = Vec::with_capacity(4096 + 1 + data.palette.len() * 4);
    buf.extend_from_slice(&data.voxels);
    buf.push(palette_len);
    for entry in &data.palette {
        buf.extend_from_slice(entry);
    }
    buf
}

fn deserialize_brick(bytes: &[u8]) -> io::Result<Option<BrickData>> {
    if bytes.len() < 4097 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "brick payload too short",
        ));
    }

    let mut voxels = [0u8; 4096];
    voxels.copy_from_slice(&bytes[..4096]);

    let palette_len = bytes[4096] as usize;
    let palette_bytes = &bytes[4097..];
    if palette_bytes.len() < palette_len * 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "palette truncated",
        ));
    }

    let mut palette = Vec::with_capacity(palette_len);
    for i in 0..palette_len {
        let off = i * 4;
        palette.push([
            palette_bytes[off],
            palette_bytes[off + 1],
            palette_bytes[off + 2],
            palette_bytes[off + 3],
        ]);
    }

    Ok(Some(BrickData { voxels, palette }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_brick() {
        let dir = std::env::temp_dir().join("smallworld_test_region");
        let _ = fs::remove_dir_all(&dir);
        let path = region_path(&dir, [0, 0, 0]);

        let mut voxels = [0u8; 4096];
        voxels[0] = 1;
        voxels[100] = 2;
        let palette = vec![[0, 0, 0, 0], [255, 0, 0, 255], [0, 255, 0, 255]];
        let data = BrickData { voxels, palette };

        {
            let mut rf = RegionFile::open_or_create(&path).unwrap();
            rf.write_brick([0, 0, 0], Some(&data)).unwrap();
            rf.write_brick([1, 0, 0], None).unwrap(); // air
        }

        {
            let mut rf = RegionFile::open_or_create(&path).unwrap();
            let loaded = rf.read_brick([0, 0, 0]).unwrap().unwrap();
            assert_eq!(loaded.voxels[0], 1);
            assert_eq!(loaded.voxels[100], 2);
            assert_eq!(loaded.palette.len(), 3);
            assert_eq!(loaded.palette[1], [255, 0, 0, 255]);

            assert!(rf.read_brick([1, 0, 0]).unwrap().is_none()); // air
            assert!(rf.read_brick([2, 0, 0]).unwrap().is_none()); // never written
            assert!(rf.has_entry([0, 0, 0]));
            assert!(rf.has_entry([1, 0, 0])); // air is an entry
            assert!(!rf.has_entry([2, 0, 0])); // never written
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_grid_pos_works() {
        let (region, local) = split_grid_pos([17, 5, 32]);
        assert_eq!(region, [1, 0, 2]);
        assert_eq!(local, [1, 5, 0]);
    }
}
