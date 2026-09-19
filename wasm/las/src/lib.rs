//! LAS 1.0 - 1.4 (R15) point cloud reader for the xeokit SDK, with LASzip (LAZ) decompression.
//!
//! Compiled to WebAssembly with a plain C ABI (see the `extern "C"` functions at the bottom); the JavaScript side
//! copies the file into WebAssembly memory, calls `las_open`, then `las_read` repeatedly (so it can yield to the
//! browser's event loop between calls), and finally reads the decoded arrays described by `las_info`.

pub mod laz;

use laz::LazReader;

/// Public header block, LASzip VLR (if any) and EPSG code (if any) of a LAS/LAZ file.
pub struct Header {
    pub version_major: u8,
    pub version_minor: u8,
    pub file_source_id: u16,
    pub global_encoding: u16,
    pub system_identifier: String,
    pub generating_software: String,
    pub creation_day: u16,
    pub creation_year: u16,
    pub header_size: u16,
    pub offset_to_point_data: u32,
    pub num_vlrs: u32,
    pub point_data_format: u8,
    pub compressed: bool,
    pub point_data_record_length: u16,
    pub num_points: u64,
    pub legacy_num_points: u32,
    pub num_points_by_return: [u32; 5],
    pub scale: [f64; 3],
    pub offset: [f64; 3],
    pub max: [f64; 3],
    pub min: [f64; 3],
    pub start_of_waveform: Option<u64>,
    pub start_of_first_evlr: Option<u64>,
    pub num_evlrs: Option<u32>,
    pub num_point_records: Option<u64>,
    pub num_points_by_return_ext: Option<[u64; 15]>,
    pub epsg: Option<u16>,
    pub laz_vlr: Option<Vec<u8>>,
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn rd_u64(b: &[u8], o: usize) -> u64 {
    (rd_u32(b, o) as u64) | ((rd_u32(b, o + 4) as u64) << 32)
}

fn rd_f64(b: &[u8], o: usize) -> f64 {
    f64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn rd_i32(b: &[u8], o: usize) -> i32 {
    rd_u32(b, o) as i32
}

fn ascii(b: &[u8], start: usize, len: usize) -> String {
    let slice = &b[start..start + len];
    let end = slice.iter().position(|&c| c == 0).unwrap_or(len);
    slice[..end].iter().map(|&c| c as char).collect::<String>().trim().to_string()
}

/// Reads the EPSG code from a GeoTIFF GeoKeyDirectoryTag (ProjectedCSTypeGeoKey, else GeographicTypeGeoKey).
fn read_epsg(data: &[u8]) -> Option<u16> {
    if data.len() < 8 {
        return None;
    }
    let num_keys = rd_u16(data, 6) as usize;
    let mut geographic = None;
    for i in 0..num_keys {
        let pos = 8 + i * 8;
        if pos + 8 > data.len() {
            break;
        }
        let key_id = rd_u16(data, pos);
        let tag_location = rd_u16(data, pos + 2);
        let value = rd_u16(data, pos + 6);
        if tag_location == 0 {
            if key_id == 3072 {
                return Some(value);
            }
            if key_id == 2048 {
                geographic = Some(value);
            }
        }
    }
    geographic
}

pub fn parse_header(bytes: &[u8]) -> Result<Header, String> {
    if bytes.len() < 227 || &bytes[0..4] != b"LASF" {
        return Err("Invalid FileSignature. Is this a LAS/LAZ file?".into());
    }
    let version_major = bytes[24];
    let version_minor = bytes[25];
    let header_size = rd_u16(bytes, 94);
    let pdrf = bytes[104];
    let mut h = Header {
        version_major,
        version_minor,
        file_source_id: rd_u16(bytes, 4),
        global_encoding: rd_u16(bytes, 6),
        system_identifier: ascii(bytes, 26, 32),
        generating_software: ascii(bytes, 58, 32),
        creation_day: rd_u16(bytes, 90),
        creation_year: rd_u16(bytes, 92),
        header_size,
        offset_to_point_data: rd_u32(bytes, 96),
        num_vlrs: rd_u32(bytes, 100),
        point_data_format: pdrf & 0x3F,
        compressed: pdrf & 0xC0 != 0,
        point_data_record_length: rd_u16(bytes, 105),
        num_points: rd_u32(bytes, 107) as u64,
        legacy_num_points: rd_u32(bytes, 107),
        num_points_by_return: [rd_u32(bytes, 111), rd_u32(bytes, 115), rd_u32(bytes, 119), rd_u32(bytes, 123), rd_u32(bytes, 127)],
        scale: [rd_f64(bytes, 131), rd_f64(bytes, 139), rd_f64(bytes, 147)],
        offset: [rd_f64(bytes, 155), rd_f64(bytes, 163), rd_f64(bytes, 171)],
        max: [rd_f64(bytes, 179), rd_f64(bytes, 195), rd_f64(bytes, 211)],
        min: [rd_f64(bytes, 187), rd_f64(bytes, 203), rd_f64(bytes, 219)],
        start_of_waveform: None,
        start_of_first_evlr: None,
        num_evlrs: None,
        num_point_records: None,
        num_points_by_return_ext: None,
        epsg: None,
        laz_vlr: None,
    };
    if version_major == 1 && version_minor >= 3 && header_size >= 235 && bytes.len() >= 235 {
        h.start_of_waveform = Some(rd_u64(bytes, 227));
    }
    if version_major == 1 && version_minor >= 4 && header_size >= 375 && bytes.len() >= 375 {
        h.start_of_first_evlr = Some(rd_u64(bytes, 235));
        h.num_evlrs = Some(rd_u32(bytes, 243));
        let n = rd_u64(bytes, 247);
        h.num_point_records = Some(n);
        let mut by_return = [0u64; 15];
        for (i, v) in by_return.iter_mut().enumerate() {
            *v = rd_u64(bytes, 255 + i * 8);
        }
        h.num_points_by_return_ext = Some(by_return);
        if n > 0 {
            h.num_points = n;
        }
    }
    if h.offset_to_point_data as usize > bytes.len() {
        return Err("LAS: offset to point data lies beyond the end of the file".into());
    }
    // Variable length records
    let mut pos = header_size as usize;
    for _ in 0..h.num_vlrs {
        if pos + 54 > bytes.len() {
            break;
        }
        let user_id = ascii(bytes, pos + 2, 16);
        let record_id = rd_u16(bytes, pos + 18);
        let length = rd_u16(bytes, pos + 20) as usize;
        let data = &bytes[pos + 54..(pos + 54 + length).min(bytes.len())];
        if user_id == "laszip encoded" && record_id == 22204 {
            h.laz_vlr = Some(data.to_vec());
        } else if user_id == "LASF_Projection" && record_id == 34735 {
            if let Some(epsg) = read_epsg(data) {
                h.epsg = Some(epsg);
            }
        }
        pos += 54 + length;
    }
    Ok(h)
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_f64(v: f64) -> String {
    if v.is_finite() {
        format!("{}", v)
    } else {
        "null".into()
    }
}

impl Header {
    /// The header as JSON, with the attribute names used by `LASLoaderPlugin` metadata.
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(1024);
        s.push_str("{\"FileSignature\":\"LASF\"");
        s.push_str(&format!(",\"FileSoureceID\":{}", self.file_source_id));
        s.push_str(&format!(",\"GlobalEncoding\":{}", self.global_encoding));
        s.push_str(&format!(",\"VersionMajor\":{},\"VersionMinor\":{}", self.version_major, self.version_minor));
        s.push_str(&format!(",\"SystemIdentifier\":{}", json_string(&self.system_identifier)));
        s.push_str(&format!(",\"GeneratingSoftware\":{}", json_string(&self.generating_software)));
        s.push_str(&format!(",\"CreationDay\":{},\"CreationYear\":{}", self.creation_day, self.creation_year));
        s.push_str(&format!(",\"HeaderSize\":{},\"OffsetToPointData\":{}", self.header_size, self.offset_to_point_data));
        s.push_str(&format!(",\"NumberOfVariableLengthRecords\":{}", self.num_vlrs));
        s.push_str(&format!(",\"PointDataFormatID\":{},\"Compressed\":{}", self.point_data_format, self.compressed));
        s.push_str(&format!(",\"PointDataRecordLength\":{}", self.point_data_record_length));
        s.push_str(&format!(",\"NumberOfPoints\":{}", self.num_points));
        s.push_str(&format!(",\"NumberOfPointByReturn\":[{}]", self.num_points_by_return.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")));
        for (name, v) in [("ScaleFactor", self.scale), ("Offset", self.offset)] {
            s.push_str(&format!(",\"{0}X\":{1},\"{0}Y\":{2},\"{0}Z\":{3}", name, json_f64(v[0]), json_f64(v[1]), json_f64(v[2])));
        }
        s.push_str(&format!(",\"MaxX\":{},\"MinX\":{},\"MaxY\":{},\"MinY\":{},\"MaxZ\":{},\"MinZ\":{}", json_f64(self.max[0]), json_f64(self.min[0]), json_f64(self.max[1]), json_f64(self.min[1]), json_f64(self.max[2]), json_f64(self.min[2])));
        if let Some(v) = self.start_of_waveform {
            s.push_str(&format!(",\"StartOfWaveformDataPacketRecord\":{}", v));
        }
        if let Some(v) = self.start_of_first_evlr {
            s.push_str(&format!(",\"StartOfFirstExtendedVariableLengthRecord\":{}", v));
        }
        if let Some(v) = self.num_evlrs {
            s.push_str(&format!(",\"NumberOfExtendedVariableLengthRecords\":{}", v));
        }
        if let Some(v) = self.num_point_records {
            s.push_str(&format!(",\"NumberOfPointRecords\":{}", v));
        }
        if let Some(v) = &self.num_points_by_return_ext {
            s.push_str(&format!(",\"NumberOfPointsByReturn\":[{}]", v.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")));
        }
        if let Some(epsg) = self.epsg {
            s.push_str(&format!(",\"epsg\":{}", epsg));
        }
        s.push('}');
        s
    }
}

// Byte offset of the RGB triple within a point data record, per point data record format (LAS 1.4 R15, tables 7-17)
fn rgb_offset(format: u8) -> Option<usize> {
    match format {
        2 => Some(20),
        3 | 5 => Some(28),
        7 | 8 | 10 => Some(30),
        _ => None,
    }
}

/// Decoded point attributes, as the `LASLoaderPlugin` needs them.
#[derive(Default)]
pub struct Points {
    pub positions_f32: Vec<f32>,
    pub positions_f64: Vec<f64>,
    pub intensities: Vec<u16>,
    pub classifications: Vec<u8>,
    /// Raw 16-bit RGB triples (empty when the format has no colors)
    pub rgb16: Vec<u16>,
    /// 8-bit RGB triples, filled in when reading has finished
    pub colors: Vec<u8>,
    pub kept: usize,
}

/// Field layout of the file's point data records
struct Layout {
    scale: [f64; 3],
    offset: [f64; 3],
    rgb_offset: Option<usize>,
    class_offset: usize,
    class_mask: u8,
    fp64: bool,
}

#[inline]
fn extract(rec: &[u8], out: &mut Points, layout: &Layout) {
    let j = out.kept;
    let x = rd_i32(rec, 0) as f64 * layout.scale[0] + layout.offset[0];
    let y = rd_i32(rec, 4) as f64 * layout.scale[1] + layout.offset[1];
    let z = rd_i32(rec, 8) as f64 * layout.scale[2] + layout.offset[2];
    if layout.fp64 {
        out.positions_f64[j * 3] = x;
        out.positions_f64[j * 3 + 1] = y;
        out.positions_f64[j * 3 + 2] = z;
    } else {
        out.positions_f32[j * 3] = x as f32;
        out.positions_f32[j * 3 + 1] = y as f32;
        out.positions_f32[j * 3 + 2] = z as f32;
    }
    out.intensities[j] = rd_u16(rec, 12);
    out.classifications[j] = rec[layout.class_offset] & layout.class_mask;
    if let Some(o) = layout.rgb_offset {
        out.rgb16[j * 3] = rd_u16(rec, o);
        out.rgb16[j * 3 + 1] = rd_u16(rec, o + 2);
        out.rgb16[j * 3 + 2] = rd_u16(rec, o + 4);
    }
    out.kept = j + 1;
}

/// Incremental reader: `read` decodes a bounded number of source points per call.
pub struct LasReader {
    bytes: Vec<u8>,
    pub header: Header,
    pub header_json: String,
    laz: Option<LazReader>,
    layout: Layout,
    record_length: usize,
    skip: u64,
    color_depth: u32,
    pub num_points: u64,
    pub num_kept: usize,
    next_index: u64,
    pub points: Points,
    pub finished: bool,
}

impl LasReader {
    /// `color_depth`: 8, 16, or 0 for automatic detection.
    pub fn open(bytes: Vec<u8>, skip: u32, fp64: bool, color_depth: u32) -> Result<LasReader, String> {
        let header = parse_header(&bytes)?;
        let format = header.point_data_format;
        if format > 10 {
            return Err(format!("LAS: unsupported point data record format {}", format));
        }
        let record_length = header.point_data_record_length as usize;
        if record_length < [20, 28, 26, 34, 57, 63, 30, 36, 38, 59, 67][format as usize] {
            return Err(format!("LAS: point data record length {} is too short for format {}", record_length, format));
        }
        let offset = header.offset_to_point_data as usize;
        let mut num_points = header.num_points;
        let laz = if header.compressed {
            let vlr = header.laz_vlr.as_ref().ok_or("LAZ: compressed file without a 'laszip encoded' VLR")?;
            let reader = LazReader::new(&bytes, offset, num_points, vlr)?;
            if reader.record_size != record_length {
                return Err(format!("LAZ: LASzip record size {} does not match the point data record length {}", reader.record_size, record_length));
            }
            Some(reader)
        } else {
            let available = ((bytes.len() - offset) / record_length) as u64;
            if available < num_points {
                num_points = available;
            }
            None
        };
        let skip = (skip.max(1)) as u64;
        let num_kept = num_points.div_ceil(skip) as usize;
        let rgb = rgb_offset(format);
        let layout = Layout {
            scale: header.scale,
            offset: header.offset,
            rgb_offset: rgb,
            class_offset: if format >= 6 { 16 } else { 15 },
            class_mask: if format >= 6 { 0xFF } else { 0x1F },
            fp64,
        };
        let points = Points {
            positions_f32: if fp64 { Vec::new() } else { vec![0.0; num_kept * 3] },
            positions_f64: if fp64 { vec![0.0; num_kept * 3] } else { Vec::new() },
            intensities: vec![0; num_kept],
            classifications: vec![0; num_kept],
            rgb16: if rgb.is_some() { vec![0; num_kept * 3] } else { Vec::new() },
            colors: Vec::new(),
            kept: 0,
        };
        let header_json = header.to_json();
        Ok(LasReader { bytes, header, header_json, laz, layout, record_length, skip, color_depth, num_points, num_kept, next_index: 0, points, finished: false })
    }

    /// Decodes up to `max_points` further source points; returns the number of source points still to read.
    pub fn read(&mut self, max_points: u64) -> Result<u64, String> {
        let end = self.next_index.saturating_add(max_points).min(self.num_points);
        match self.laz.as_mut() {
            Some(laz) => {
                while self.next_index < end {
                    laz.read_point(&self.bytes)?;
                    if self.next_index % self.skip == 0 {
                        extract(&laz.record, &mut self.points, &self.layout);
                    }
                    self.next_index += 1;
                }
            }
            None => {
                while self.next_index < end {
                    let off = self.header.offset_to_point_data as usize + self.next_index as usize * self.record_length;
                    extract(&self.bytes[off..off + self.record_length], &mut self.points, &self.layout);
                    self.next_index += self.skip;
                }
                if self.next_index >= self.num_points {
                    self.next_index = self.num_points;
                }
            }
        }
        if self.next_index >= self.num_points && !self.finished {
            self.finish();
        }
        Ok(self.num_points - self.next_index)
    }

    fn finish(&mut self) {
        self.finished = true;
        let rgb16 = &self.points.rgb16;
        if rgb16.is_empty() {
            return;
        }
        let two_byte = match self.color_depth {
            16 => true,
            8 => false,
            _ => rgb16.iter().any(|&v| v > 255),
        };
        self.points.colors = rgb16.iter().map(|&v| if two_byte { (v >> 8) as u8 } else { v.min(255) as u8 }).collect();
        self.points.rgb16 = Vec::new();
    }
}

//----------------------------------------------------------------------------------------------------------------------
// C ABI for the WebAssembly build
//----------------------------------------------------------------------------------------------------------------------

use std::cell::RefCell;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_error(msg: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = msg);
}

/// Result description read by JavaScript (all fields are 32-bit; pointers are offsets into the module memory).
#[repr(C)]
pub struct Info {
    pub num_kept: u32,
    pub points_done: u32,
    pub point_format: u32,
    pub fp64: u32,
    pub positions_ptr: u32,
    pub positions_len: u32,
    pub colors_ptr: u32,
    pub colors_len: u32,
    pub intensities_ptr: u32,
    pub intensities_len: u32,
    pub classifications_ptr: u32,
    pub classifications_len: u32,
    pub header_ptr: u32,
    pub header_len: u32,
    pub finished: u32,
}

pub struct Handle {
    reader: LasReader,
    info: Info,
}

/// Allocates `len` bytes in module memory (freed by `dealloc`, or taken over by `las_open`).
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> *mut u8 {
    let mut v: Vec<u8> = Vec::with_capacity(len as usize);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from `alloc(len)` and not have been freed or passed to `las_open`.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: u32) {
    drop(Vec::from_raw_parts(ptr, 0, len as usize));
}

#[no_mangle]
pub extern "C" fn error_ptr() -> *const u8 {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn error_len() -> u32 {
    LAST_ERROR.with(|e| e.borrow().len() as u32)
}

/// Opens a LAS/LAZ file held in memory allocated with `alloc` (ownership passes to the reader).
/// Returns 0 on error (see `error_ptr`/`error_len`).
///
/// # Safety
/// `ptr`/`len` must describe an allocation made by `alloc`.
#[no_mangle]
pub unsafe extern "C" fn las_open(ptr: *mut u8, len: u32, skip: u32, fp64: u32, color_depth: u32) -> *mut Handle {
    let bytes = Vec::from_raw_parts(ptr, len as usize, len as usize);
    match LasReader::open(bytes, skip, fp64 != 0, color_depth) {
        Ok(reader) => {
            let info = Info { num_kept: 0, points_done: 0, point_format: 0, fp64: 0, positions_ptr: 0, positions_len: 0, colors_ptr: 0, colors_len: 0, intensities_ptr: 0, intensities_len: 0, classifications_ptr: 0, classifications_len: 0, header_ptr: 0, header_len: 0, finished: 0 };
            let mut handle = Box::new(Handle { reader, info });
            update_info(&mut handle);
            Box::into_raw(handle)
        }
        Err(e) => {
            set_error(e);
            std::ptr::null_mut()
        }
    }
}

fn update_info(h: &mut Handle) {
    let r = &h.reader;
    let p = &r.points;
    h.info = Info {
        num_kept: r.num_kept as u32,
        points_done: p.kept as u32,
        point_format: r.header.point_data_format as u32,
        fp64: r.layout.fp64 as u32,
        positions_ptr: if r.layout.fp64 { p.positions_f64.as_ptr() as usize as u32 } else { p.positions_f32.as_ptr() as usize as u32 },
        positions_len: if r.layout.fp64 { p.positions_f64.len() as u32 } else { p.positions_f32.len() as u32 },
        colors_ptr: p.colors.as_ptr() as usize as u32,
        colors_len: p.colors.len() as u32,
        intensities_ptr: p.intensities.as_ptr() as usize as u32,
        intensities_len: p.intensities.len() as u32,
        classifications_ptr: p.classifications.as_ptr() as usize as u32,
        classifications_len: p.classifications.len() as u32,
        header_ptr: r.header_json.as_ptr() as usize as u32,
        header_len: r.header_json.len() as u32,
        finished: r.finished as u32,
    };
}

/// Decodes up to `max_points` further source points. Returns the number of source points still to read,
/// or 0xFFFFFFFF on error.
///
/// # Safety
/// `handle` must come from `las_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn las_read(handle: *mut Handle, max_points: u32) -> u32 {
    let h = &mut *handle;
    match h.reader.read(max_points as u64) {
        Ok(remaining) => {
            update_info(h);
            remaining.min(0xFFFF_FFFE) as u32
        }
        Err(e) => {
            set_error(e);
            0xFFFF_FFFF
        }
    }
}

/// # Safety
/// `handle` must come from `las_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn las_info(handle: *mut Handle) -> *const Info {
    &(*handle).info
}

/// # Safety
/// `handle` must come from `las_open` and not have been closed.
#[no_mangle]
pub unsafe extern "C" fn las_close(handle: *mut Handle) {
    drop(Box::from_raw(handle));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv1a64(data: &[u8]) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Decodes every record of a LAZ file and returns the FNV-1a 64 hash of the concatenated raw records.
    fn laz_record_hash(path: &str) -> (u64, u64) {
        let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("missing test file {}", path));
        let header = parse_header(&bytes).unwrap();
        let mut reader = LazReader::new(&bytes, header.offset_to_point_data as usize, header.num_points, header.laz_vlr.as_ref().unwrap()).unwrap();
        assert_eq!(reader.record_size, header.point_data_record_length as usize);
        let mut all = Vec::with_capacity(header.num_points as usize * reader.record_size);
        for _ in 0..header.num_points {
            reader.read_point(&bytes).unwrap();
            all.extend_from_slice(&reader.record);
        }
        (header.num_points, fnv1a64(&all))
    }

    // Expected hashes were produced by an independent decoder verified byte-for-byte against laz-perf.

    #[test]
    fn laz_pointwise_v1_items() {
        // LAS 1.2 format 3, LASzip 1.2r0 compressor 1 (point-wise), POINT10/GPSTIME11/RGB12 version 1
        assert_eq!(laz_record_hash("tests/data/simple-laszip-1.2r0.laz"), (1065, 0xac5ceed2077b659c));
    }

    #[test]
    fn laz_layered_v3_items() {
        // LAS 1.4 format 7 COPC, compressor 3 (layered), POINT14/RGB14 version 3, variable-size chunks
        assert_eq!(laz_record_hash("tests/data/autzen.copc.laz"), (107, 0xad692d2414c2d186));
    }

    #[test]
    fn laz_chunked_v2_items() {
        // LAS 1.2 format 3, compressor 2 (chunked), POINT10/GPSTIME11/RGB12 version 2 (repo example asset)
        assert_eq!(laz_record_hash("../../assets/models/las/indoor.0.1.laz"), (808042, 0xefe6328ea49f30ca));
    }

    #[test]
    fn las_reader_laz_positions_within_bounds() {
        let bytes = std::fs::read("../../assets/models/las/indoor.0.1.laz").unwrap();
        let mut reader = LasReader::open(bytes, 7, false, 0).unwrap();
        while reader.read(100_000).unwrap() > 0 {}
        let h = &reader.header;
        assert_eq!(reader.num_kept, (808042 + 6) / 7);
        assert_eq!(reader.points.kept, reader.num_kept);
        assert_eq!(reader.points.colors.len(), reader.num_kept * 3);
        for (i, chunk) in reader.points.positions_f32.chunks(3).enumerate() {
            for c in 0..3 {
                assert!((chunk[c] as f64) >= h.min[c] - 0.01 && (chunk[c] as f64) <= h.max[c] + 0.01, "point {} out of bounds", i);
            }
        }
        assert!(reader.header_json.contains("\"PointDataFormatID\":3,\"Compressed\":true"));
    }

    /// Builds an uncompressed LAS 1.4 file with two format-8 points (RGB + NIR).
    fn synthetic_las_1_4() -> Vec<u8> {
        let mut b = vec![0u8; 375];
        b[0..4].copy_from_slice(b"LASF");
        b[24] = 1;
        b[25] = 4;
        b[26..32].copy_from_slice(b"xeokit");
        b[94..96].copy_from_slice(&375u16.to_le_bytes());
        b[96..100].copy_from_slice(&375u32.to_le_bytes());
        b[104] = 8;
        b[105..107].copy_from_slice(&38u16.to_le_bytes());
        for (i, v) in [0.01f64, 0.01, 0.001, 100.0, 200.0, 300.0].iter().enumerate() {
            b[131 + i * 8..139 + i * 8].copy_from_slice(&v.to_le_bytes());
        }
        b[247..255].copy_from_slice(&2u64.to_le_bytes());
        for (x, intensity, class, rgb) in [(1000i32, 500u16, 2u8, [65535u16, 0, 256]), (-1000, 7, 6, [1000, 2000, 3000])] {
            let mut rec = vec![0u8; 38];
            rec[0..4].copy_from_slice(&x.to_le_bytes());
            rec[4..8].copy_from_slice(&(2 * x).to_le_bytes());
            rec[8..12].copy_from_slice(&(3 * x).to_le_bytes());
            rec[12..14].copy_from_slice(&intensity.to_le_bytes());
            rec[16] = class;
            for c in 0..3 {
                rec[30 + c * 2..32 + c * 2].copy_from_slice(&rgb[c].to_le_bytes());
            }
            b.extend_from_slice(&rec);
        }
        b
    }

    #[test]
    fn las_1_4_uncompressed_format_8() {
        let mut reader = LasReader::open(synthetic_las_1_4(), 1, true, 0).unwrap();
        assert_eq!(reader.read(10).unwrap(), 0);
        let p = &reader.points;
        assert_eq!(p.positions_f64, vec![110.0, 220.0, 303.0, 90.0, 180.0, 297.0]);
        assert_eq!(p.intensities, vec![500, 7]);
        assert_eq!(p.classifications, vec![2, 6]);
        assert_eq!(p.colors, vec![255, 0, 1, 3, 7, 11]); // 16-bit colors detected automatically
        assert!(reader.header_json.contains("\"VersionMinor\":4"));
        assert!(reader.header_json.contains("\"NumberOfPointRecords\":2"));
        assert!(reader.header_json.contains("\"SystemIdentifier\":\"xeokit\""));
        let mut reader8 = LasReader::open(synthetic_las_1_4(), 1, false, 8).unwrap();
        reader8.read(10).unwrap();
        assert_eq!(reader8.points.colors, vec![255, 0, 255, 255, 255, 255]); // clamped 8-bit interpretation
    }

    #[test]
    fn rejects_non_las() {
        assert!(LasReader::open(b"not a las file at all, definitely not".repeat(10), 1, false, 0).is_err());
    }
}
