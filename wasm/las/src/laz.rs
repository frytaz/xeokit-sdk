//! LASzip (LAZ) decompression.
//!
//! A port of the LASzip decompression algorithms by Martin Isenburg (rapidlasso), following the reference
//! implementations in LASzip (Apache-2.0) and laz-rs (Apache-2.0/MIT). Supports compressors 1 (point-wise),
//! 2 (point-wise chunked) and 3 (layered chunked) with the arithmetic coder, and every item type/version LASzip
//! writes: POINT10/GPSTIME11/RGB12/WAVEPACKET13/BYTE versions 1 and 2, POINT14/RGB14/RGBNIR14/WAVEPACKET14/BYTE14
//! versions 3 and 4. That covers all LAZ files for LAS 1.0 - 1.4 point data record formats 0 - 10.

const AC_MIN_LENGTH: u32 = 0x0100_0000;
const DM_LENGTH_SHIFT: u32 = 15;
const DM_MAX_COUNT: u32 = 1 << DM_LENGTH_SHIFT;
const BM_LENGTH_SHIFT: u32 = 13;
const BM_MAX_COUNT: u32 = 1 << BM_LENGTH_SHIFT;

/// Adaptive multi-symbol model (Amir Said's FastAC).
pub struct Model {
    symbols: u32,
    last_symbol: u32,
    table_size: u32,
    table_shift: u32,
    decoder_table: Vec<u32>,
    distribution: Vec<u32>,
    symbol_count: Vec<u32>,
    total_count: u32,
    update_cycle: u32,
    symbols_until_update: u32,
}

impl Model {
    pub fn new(symbols: u32) -> Model {
        let (table_size, table_shift, decoder_table) = if symbols > 16 {
            let mut table_bits = 3;
            while symbols > (1 << (table_bits + 2)) {
                table_bits += 1;
            }
            let table_size = 1u32 << table_bits;
            (table_size, DM_LENGTH_SHIFT - table_bits, vec![0u32; table_size as usize + 2])
        } else {
            (0, 0, Vec::new())
        };
        let mut m = Model {
            symbols,
            last_symbol: symbols - 1,
            table_size,
            table_shift,
            decoder_table,
            distribution: vec![0; symbols as usize],
            symbol_count: vec![1; symbols as usize],
            total_count: 0,
            update_cycle: symbols,
            symbols_until_update: 0,
        };
        m.update();
        m.update_cycle = (symbols + 6) >> 1;
        m.symbols_until_update = m.update_cycle;
        m
    }

    fn update(&mut self) {
        self.total_count += self.update_cycle;
        if self.total_count > DM_MAX_COUNT {
            self.total_count = 0;
            for c in self.symbol_count.iter_mut() {
                *c = (*c + 1) >> 1;
                self.total_count += *c;
            }
        }
        let scale = 0x8000_0000u32 / self.total_count;
        let mut sum = 0u32;
        let n = self.symbols as usize;
        if self.table_size == 0 {
            for k in 0..n {
                self.distribution[k] = (scale * sum) >> (31 - DM_LENGTH_SHIFT);
                sum += self.symbol_count[k];
            }
        } else {
            let mut s = 0usize;
            for k in 0..n {
                self.distribution[k] = (scale * sum) >> (31 - DM_LENGTH_SHIFT);
                sum += self.symbol_count[k];
                let w = (self.distribution[k] >> self.table_shift) as usize;
                while s < w {
                    s += 1;
                    self.decoder_table[s] = (k - 1) as u32;
                }
            }
            self.decoder_table[0] = 0;
            while s <= self.table_size as usize {
                s += 1;
                self.decoder_table[s] = self.symbols - 1;
            }
        }
        self.update_cycle = (5 * self.update_cycle) >> 2;
        let max_cycle = (self.symbols + 6) << 3;
        if self.update_cycle > max_cycle {
            self.update_cycle = max_cycle;
        }
        self.symbols_until_update = self.update_cycle;
    }
}

/// Adaptive binary model.
pub struct BitModel {
    bit0_count: u32,
    bit_count: u32,
    bit0_prob: u32,
    bits_until_update: u32,
    update_cycle: u32,
}

impl BitModel {
    pub fn new() -> BitModel {
        BitModel { bit0_count: 1, bit_count: 2, bit0_prob: 1 << (BM_LENGTH_SHIFT - 1), bits_until_update: 4, update_cycle: 4 }
    }

    fn update(&mut self) {
        self.bit_count += self.update_cycle;
        if self.bit_count > BM_MAX_COUNT {
            self.bit_count = (self.bit_count + 1) >> 1;
            self.bit0_count = (self.bit0_count + 1) >> 1;
            if self.bit0_count == self.bit_count {
                self.bit_count += 1;
            }
        }
        let scale = 0x8000_0000u32 / self.bit_count;
        self.bit0_prob = (self.bit0_count * scale) >> (31 - BM_LENGTH_SHIFT);
        self.update_cycle = (5 * self.update_cycle) >> 2;
        if self.update_cycle > 64 {
            self.update_cycle = 64;
        }
        self.bits_until_update = self.update_cycle;
    }
}

/// Range decoder over a byte range of the input. Reads past the end yield zero bytes.
#[derive(Clone, Copy)]
pub struct Decoder {
    pos: usize,
    end: usize,
    value: u32,
    length: u32,
}

impl Decoder {
    pub fn new(pos: usize, end: usize) -> Decoder {
        Decoder { pos, end, value: 0, length: 0 }
    }

    #[inline]
    fn next_byte(&mut self, bytes: &[u8]) -> u32 {
        let b = if self.pos < self.end { bytes[self.pos] as u32 } else { 0 };
        self.pos += 1;
        b
    }

    /// Reads the 4 initial bytes of a coded stream.
    pub fn init(&mut self, bytes: &[u8]) {
        let mut v = 0u32;
        for _ in 0..4 {
            v = (v << 8) | self.next_byte(bytes);
        }
        self.value = v;
        self.length = 0xFFFF_FFFF;
    }

    #[inline]
    fn renorm(&mut self, bytes: &[u8]) {
        loop {
            self.value = (self.value << 8) | self.next_byte(bytes);
            self.length <<= 8;
            if self.length >= AC_MIN_LENGTH {
                break;
            }
        }
    }

    pub fn decode_bit(&mut self, bytes: &[u8], m: &mut BitModel) -> u32 {
        let x = m.bit0_prob * (self.length >> BM_LENGTH_SHIFT);
        let sym = if self.value < x {
            self.length = x;
            m.bit0_count += 1;
            0
        } else {
            self.value -= x;
            self.length -= x;
            1
        };
        if self.length < AC_MIN_LENGTH {
            self.renorm(bytes);
        }
        m.bits_until_update -= 1;
        if m.bits_until_update == 0 {
            m.update();
        }
        sym
    }

    pub fn decode_symbol(&mut self, bytes: &[u8], m: &mut Model) -> u32 {
        let mut y = self.length;
        let (sym, x) = if m.table_size != 0 {
            let length = self.length >> DM_LENGTH_SHIFT;
            let dv = self.value / length;
            let t = (dv >> m.table_shift) as usize;
            let mut sym = m.decoder_table[t];
            let mut n = m.decoder_table[t + 1] + 1;
            while n > sym + 1 {
                let k = (sym + n) >> 1;
                if m.distribution[k as usize] > dv {
                    n = k;
                } else {
                    sym = k;
                }
            }
            let x = m.distribution[sym as usize] * length;
            if sym != m.last_symbol {
                y = m.distribution[sym as usize + 1] * length;
            }
            (sym, x)
        } else {
            let length = self.length >> DM_LENGTH_SHIFT;
            let mut x = 0u32;
            let mut sym = 0u32;
            let mut n = m.symbols;
            let mut k = n >> 1;
            loop {
                let z = length * m.distribution[k as usize];
                if z > self.value {
                    n = k;
                    y = z;
                } else {
                    sym = k;
                    x = z;
                }
                k = (sym + n) >> 1;
                if k == sym {
                    break;
                }
            }
            (sym, x)
        };
        self.value -= x;
        self.length = y - x;
        if self.length < AC_MIN_LENGTH {
            self.renorm(bytes);
        }
        m.symbol_count[sym as usize] += 1;
        m.symbols_until_update -= 1;
        if m.symbols_until_update == 0 {
            m.update();
        }
        sym
    }

    pub fn read_bits(&mut self, bytes: &[u8], bits: u32) -> u32 {
        if bits > 19 {
            let tmp = self.read_bits(bytes, 16);
            return (self.read_bits(bytes, bits - 16) << 16) | tmp;
        }
        self.length >>= bits;
        let sym = self.value / self.length;
        self.value -= self.length * sym;
        if self.length < AC_MIN_LENGTH {
            self.renorm(bytes);
        }
        sym
    }

    pub fn read_int(&mut self, bytes: &[u8]) -> u32 {
        let lower = self.read_bits(bytes, 16);
        let upper = self.read_bits(bytes, 16);
        (upper << 16) | lower
    }

    pub fn read_int64(&mut self, bytes: &[u8]) -> u64 {
        let lower = self.read_int(bytes) as u64;
        let upper = self.read_int(bytes) as u64;
        (upper << 32) | lower
    }

    pub fn position(&self) -> usize {
        self.pos
    }
}

/// Integer decompressor: decodes a corrector relative to a prediction.
pub struct IntDecompressor {
    pub k: u32,
    bits_high: u32,
    corr_range: u32,
    corr_min: i32,
    m_bits: Vec<Model>,
    m_corrector0: BitModel,
    m_corrector: Vec<Model>,
}

impl IntDecompressor {
    pub fn new(bits: u32, contexts: usize) -> IntDecompressor {
        let bits_high = 8;
        let (corr_bits, corr_range, corr_min) = if bits > 0 && bits < 32 {
            (bits, 1u32 << bits, -(((1u32 << bits) / 2) as i32))
        } else {
            (32, 0, i32::MIN)
        };
        let m_bits = (0..contexts).map(|_| Model::new(corr_bits + 1)).collect();
        let m_corrector = (1..=corr_bits)
            .map(|i| Model::new(if i <= bits_high { 1 << i } else { 1 << bits_high }))
            .collect();
        IntDecompressor { k: 0, bits_high, corr_range, corr_min, m_bits, m_corrector0: BitModel::new(), m_corrector }
    }

    pub fn decompress(&mut self, dec: &mut Decoder, bytes: &[u8], pred: i32, context: usize) -> i32 {
        let k = dec.decode_symbol(bytes, &mut self.m_bits[context]);
        self.k = k;
        let c: i32 = if k != 0 {
            if k < 32 {
                let mut c = dec.decode_symbol(bytes, &mut self.m_corrector[(k - 1) as usize]);
                if k > self.bits_high {
                    let k1 = k - self.bits_high;
                    c = (c << k1) | dec.read_bits(bytes, k1);
                }
                if c >= (1u32 << (k - 1)) {
                    c as i32 + 1
                } else {
                    (c as i32).wrapping_sub(((1u32 << k) - 1) as i32)
                }
            } else {
                self.corr_min
            }
        } else {
            dec.decode_bit(bytes, &mut self.m_corrector0) as i32
        };
        let mut real = pred.wrapping_add(c);
        if self.corr_range != 0 {
            if real < 0 {
                real = real.wrapping_add(self.corr_range as i32);
            } else if real >= self.corr_range as i32 {
                real -= self.corr_range as i32;
            }
        }
        real
    }
}

/// Streaming median of the last five values.
#[derive(Clone, Copy)]
pub struct Median5 {
    values: [i32; 5],
    high: bool,
}

impl Median5 {
    pub fn new() -> Median5 {
        Median5 { values: [0; 5], high: true }
    }

    pub fn add(&mut self, v: i32) {
        let a = &mut self.values;
        if self.high {
            if v < a[2] {
                a[4] = a[3];
                a[3] = a[2];
                if v < a[0] {
                    a[2] = a[1];
                    a[1] = a[0];
                    a[0] = v;
                } else if v < a[1] {
                    a[2] = a[1];
                    a[1] = v;
                } else {
                    a[2] = v;
                }
            } else {
                if v < a[3] {
                    a[4] = a[3];
                    a[3] = v;
                } else {
                    a[4] = v;
                }
                self.high = false;
            }
        } else {
            if a[2] < v {
                a[0] = a[1];
                a[1] = a[2];
                if a[4] < v {
                    a[2] = a[3];
                    a[3] = a[4];
                    a[4] = v;
                } else if a[3] < v {
                    a[2] = a[3];
                    a[3] = v;
                } else {
                    a[2] = v;
                }
            } else {
                if a[1] < v {
                    a[0] = a[1];
                    a[1] = v;
                } else {
                    a[0] = v;
                }
                self.high = true;
            }
        }
    }

    pub fn get(&self) -> i32 {
        self.values[2]
    }
}

const NUMBER_RETURN_MAP: [[u8; 8]; 8] = [
    [15, 14, 13, 12, 11, 10, 9, 8],
    [14, 0, 1, 3, 6, 10, 10, 9],
    [13, 1, 2, 4, 7, 11, 11, 10],
    [12, 3, 4, 5, 8, 12, 12, 11],
    [11, 6, 7, 8, 9, 13, 13, 12],
    [10, 10, 11, 12, 13, 14, 14, 13],
    [9, 10, 11, 12, 13, 14, 15, 14],
    [8, 9, 10, 11, 12, 13, 14, 15],
];

const NUMBER_RETURN_LEVEL: [[u8; 8]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7],
    [1, 0, 1, 2, 3, 4, 5, 6],
    [2, 1, 0, 1, 2, 3, 4, 5],
    [3, 2, 1, 0, 1, 2, 3, 4],
    [4, 3, 2, 1, 0, 1, 2, 3],
    [5, 4, 3, 2, 1, 0, 1, 2],
    [6, 5, 4, 3, 2, 1, 0, 1],
    [7, 6, 5, 4, 3, 2, 1, 0],
];

const NUMBER_RETURN_MAP_6CTX: [[u8; 16]; 16] = [
    [0, 1, 2, 3, 4, 5, 3, 4, 4, 5, 5, 5, 5, 5, 5, 5],
    [1, 0, 1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3],
    [2, 1, 2, 4, 4, 4, 4, 4, 4, 4, 4, 3, 3, 3, 3, 3],
    [3, 3, 4, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4],
    [4, 3, 4, 4, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4],
    [5, 3, 4, 4, 4, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4],
    [3, 3, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4],
    [4, 3, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 4, 4, 4],
    [4, 3, 4, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 4, 4],
    [5, 3, 4, 4, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 4],
    [5, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4],
    [5, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 4, 4, 4],
    [5, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 4, 4],
    [5, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 4],
    [5, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5],
    [5, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5],
];

const NUMBER_RETURN_LEVEL_8CTX: [[u8; 16]; 16] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7],
    [1, 0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7],
    [2, 1, 0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7],
    [3, 2, 1, 0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7],
    [4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7],
    [5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7],
    [6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6, 7, 7, 7],
    [7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6, 7, 7],
    [7, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6, 7],
    [7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6],
    [7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5],
    [7, 7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4],
    [7, 7, 7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3],
    [7, 7, 7, 7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2],
    [7, 7, 7, 7, 7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0, 1],
    [7, 7, 7, 7, 7, 7, 7, 7, 7, 6, 5, 4, 3, 2, 1, 0],
];

#[inline]
fn clamp8(v: i32) -> i32 {
    v.clamp(0, 255)
}

#[inline]
fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[inline]
fn rd_u64(b: &[u8], o: usize) -> u64 {
    (rd_u32(b, o) as u64) | ((rd_u32(b, o + 4) as u64) << 32)
}

#[inline]
fn wr_i32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn lazy(models: &mut [Option<Model>], i: usize, symbols: u32) -> &mut Model {
    models[i].get_or_insert_with(|| Model::new(symbols))
}

//----------------------------------------------------------------------------------------------------------------------
// Point-wise item decoders (compressors 1 and 2, item versions 1 and 2)
//----------------------------------------------------------------------------------------------------------------------

pub trait PointwiseItem {
    fn size(&self) -> usize;
    /// Takes the raw first item of a chunk.
    fn init(&mut self, rec: &[u8]);
    /// Decodes the next item into `rec`.
    fn read(&mut self, dec: &mut Decoder, bytes: &[u8], rec: &mut [u8]);
}

#[derive(Default, Clone, Copy)]
struct Point10 {
    x: i32,
    y: i32,
    z: i32,
    intensity: u16,
    bits: u8,
    classification: u8,
    scan_angle: i8,
    user_data: u8,
    point_source_id: u16,
}

impl Point10 {
    fn unpack(rec: &[u8]) -> Point10 {
        Point10 {
            x: rd_i32(rec, 0),
            y: rd_i32(rec, 4),
            z: rd_i32(rec, 8),
            intensity: rd_u16(rec, 12),
            bits: rec[14],
            classification: rec[15],
            scan_angle: rec[16] as i8,
            user_data: rec[17],
            point_source_id: rd_u16(rec, 18),
        }
    }

    fn pack(&self, rec: &mut [u8]) {
        wr_i32(rec, 0, self.x);
        wr_i32(rec, 4, self.y);
        wr_i32(rec, 8, self.z);
        wr_u16(rec, 12, self.intensity);
        rec[14] = self.bits;
        rec[15] = self.classification;
        rec[16] = self.scan_angle as u8;
        rec[17] = self.user_data;
        wr_u16(rec, 18, self.point_source_id);
    }
}

fn median3(d: &[i32; 3]) -> i32 {
    if d[0] < d[1] {
        if d[1] < d[2] {
            d[1]
        } else if d[0] < d[2] {
            d[2]
        } else {
            d[0]
        }
    } else if d[0] < d[2] {
        d[0]
    } else if d[1] < d[2] {
        d[2]
    } else {
        d[1]
    }
}

pub struct Point10V1 {
    last: Point10,
    last_x_diff: [i32; 3],
    last_y_diff: [i32; 3],
    last_incr: usize,
    ic_dx: IntDecompressor,
    ic_dy: IntDecompressor,
    ic_dz: IntDecompressor,
    ic_intensity: IntDecompressor,
    ic_scan_angle: IntDecompressor,
    ic_point_source_id: IntDecompressor,
    m_changed: Model,
    m_bit_byte: Vec<Option<Model>>,
    m_classification: Vec<Option<Model>>,
    m_user_data: Vec<Option<Model>>,
}

impl Point10V1 {
    pub fn new() -> Point10V1 {
        Point10V1 {
            last: Point10::default(),
            last_x_diff: [0; 3],
            last_y_diff: [0; 3],
            last_incr: 0,
            ic_dx: IntDecompressor::new(32, 1),
            ic_dy: IntDecompressor::new(32, 20),
            ic_dz: IntDecompressor::new(32, 20),
            ic_intensity: IntDecompressor::new(16, 1),
            ic_scan_angle: IntDecompressor::new(8, 2),
            ic_point_source_id: IntDecompressor::new(16, 1),
            m_changed: Model::new(64),
            m_bit_byte: (0..256).map(|_| None).collect(),
            m_classification: (0..256).map(|_| None).collect(),
            m_user_data: (0..256).map(|_| None).collect(),
        }
    }
}

impl PointwiseItem for Point10V1 {
    fn size(&self) -> usize {
        20
    }

    fn init(&mut self, rec: &[u8]) {
        self.last = Point10::unpack(rec);
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        let p = &mut self.last;
        let median_x = median3(&self.last_x_diff);
        let median_y = median3(&self.last_y_diff);
        let x_diff = self.ic_dx.decompress(dec, b, median_x, 0);
        p.x = p.x.wrapping_add(x_diff);
        let mut k_bits = self.ic_dx.k;
        let y_diff = self.ic_dy.decompress(dec, b, median_y, k_bits.min(19) as usize);
        p.y = p.y.wrapping_add(y_diff);
        k_bits = (k_bits + self.ic_dy.k) / 2;
        p.z = self.ic_dz.decompress(dec, b, p.z, k_bits.min(19) as usize);
        let changed = dec.decode_symbol(b, &mut self.m_changed);
        if changed != 0 {
            if changed & 32 != 0 {
                p.intensity = self.ic_intensity.decompress(dec, b, p.intensity as i32, 0) as u16;
            }
            if changed & 16 != 0 {
                p.bits = dec.decode_symbol(b, lazy(&mut self.m_bit_byte, p.bits as usize, 256)) as u8;
            }
            if changed & 8 != 0 {
                p.classification = dec.decode_symbol(b, lazy(&mut self.m_classification, p.classification as usize, 256)) as u8;
            }
            if changed & 4 != 0 {
                p.scan_angle = self.ic_scan_angle.decompress(dec, b, p.scan_angle as u8 as i32, if k_bits < 3 { 1 } else { 0 }) as i8;
            }
            if changed & 2 != 0 {
                p.user_data = dec.decode_symbol(b, lazy(&mut self.m_user_data, p.user_data as usize, 256)) as u8;
            }
            if changed & 1 != 0 {
                p.point_source_id = self.ic_point_source_id.decompress(dec, b, p.point_source_id as i32, 0) as u16;
            }
        }
        self.last_x_diff[self.last_incr] = x_diff;
        self.last_y_diff[self.last_incr] = y_diff;
        self.last_incr += 1;
        if self.last_incr > 2 {
            self.last_incr = 0;
        }
        p.pack(rec);
    }
}

pub struct Point10V2 {
    last: Point10,
    last_intensity: [u16; 16],
    last_x_diff_median: [Median5; 16],
    last_y_diff_median: [Median5; 16],
    last_height: [i32; 8],
    ic_intensity: IntDecompressor,
    ic_point_source_id: IntDecompressor,
    ic_dx: IntDecompressor,
    ic_dy: IntDecompressor,
    ic_z: IntDecompressor,
    m_changed: Model,
    m_scan_angle: [Model; 2],
    m_bit_byte: Vec<Option<Model>>,
    m_classification: Vec<Option<Model>>,
    m_user_data: Vec<Option<Model>>,
}

impl Point10V2 {
    pub fn new() -> Point10V2 {
        Point10V2 {
            last: Point10::default(),
            last_intensity: [0; 16],
            last_x_diff_median: [Median5::new(); 16],
            last_y_diff_median: [Median5::new(); 16],
            last_height: [0; 8],
            ic_intensity: IntDecompressor::new(16, 4),
            ic_point_source_id: IntDecompressor::new(16, 1),
            ic_dx: IntDecompressor::new(32, 2),
            ic_dy: IntDecompressor::new(32, 22),
            ic_z: IntDecompressor::new(32, 20),
            m_changed: Model::new(64),
            m_scan_angle: [Model::new(256), Model::new(256)],
            m_bit_byte: (0..256).map(|_| None).collect(),
            m_classification: (0..256).map(|_| None).collect(),
            m_user_data: (0..256).map(|_| None).collect(),
        }
    }
}

impl PointwiseItem for Point10V2 {
    fn size(&self) -> usize {
        20
    }

    fn init(&mut self, rec: &[u8]) {
        self.last = Point10::unpack(rec);
        self.last.intensity = 0;
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        let p = &mut self.last;
        let changed = dec.decode_symbol(b, &mut self.m_changed);
        if changed & 32 != 0 {
            p.bits = dec.decode_symbol(b, lazy(&mut self.m_bit_byte, p.bits as usize, 256)) as u8;
        }
        let r = (p.bits & 7) as usize;
        let n = ((p.bits >> 3) & 7) as usize;
        let m = NUMBER_RETURN_MAP[n][r] as usize;
        let l = NUMBER_RETURN_LEVEL[n][r] as usize;
        if changed != 0 {
            if changed & 16 != 0 {
                p.intensity = self.ic_intensity.decompress(dec, b, self.last_intensity[m] as i32, m.min(3)) as u16;
                self.last_intensity[m] = p.intensity;
            } else {
                p.intensity = self.last_intensity[m];
            }
            if changed & 8 != 0 {
                p.classification = dec.decode_symbol(b, lazy(&mut self.m_classification, p.classification as usize, 256)) as u8;
            }
            if changed & 4 != 0 {
                let val = dec.decode_symbol(b, &mut self.m_scan_angle[((p.bits >> 6) & 1) as usize]) as u8 as i8;
                p.scan_angle = p.scan_angle.wrapping_add(val);
            }
            if changed & 2 != 0 {
                p.user_data = dec.decode_symbol(b, lazy(&mut self.m_user_data, p.user_data as usize, 256)) as u8;
            }
            if changed & 1 != 0 {
                p.point_source_id = self.ic_point_source_id.decompress(dec, b, p.point_source_id as i32, 0) as u16;
            }
        }
        let n1 = if n == 1 { 1 } else { 0 };
        let median = self.last_x_diff_median[m].get();
        let diff = self.ic_dx.decompress(dec, b, median, n1);
        p.x = p.x.wrapping_add(diff);
        self.last_x_diff_median[m].add(diff);
        let median = self.last_y_diff_median[m].get();
        let mut k_bits = self.ic_dx.k;
        let diff = self.ic_dy.decompress(dec, b, median, n1 + if k_bits < 20 { (k_bits & !1) as usize } else { 20 });
        p.y = p.y.wrapping_add(diff);
        self.last_y_diff_median[m].add(diff);
        k_bits = (self.ic_dx.k + self.ic_dy.k) / 2;
        p.z = self.ic_z.decompress(dec, b, self.last_height[l], n1 + if k_bits < 18 { (k_bits & !1) as usize } else { 18 });
        self.last_height[l] = p.z;
        p.pack(rec);
    }
}

const GPS_MULTI_MAX_V1: u32 = 512;

pub struct GpsTimeV1 {
    last: i64,
    last_diff: i32,
    multi_extreme_counter: i32,
    m_multi: Model,
    m_0diff: Model,
    ic: IntDecompressor,
}

impl GpsTimeV1 {
    pub fn new() -> GpsTimeV1 {
        GpsTimeV1 { last: 0, last_diff: 0, multi_extreme_counter: 0, m_multi: Model::new(GPS_MULTI_MAX_V1), m_0diff: Model::new(3), ic: IntDecompressor::new(32, 6) }
    }
}

impl PointwiseItem for GpsTimeV1 {
    fn size(&self) -> usize {
        8
    }

    fn init(&mut self, rec: &[u8]) {
        self.last = rd_u64(rec, 0) as i64;
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        if self.last_diff == 0 {
            let multi = dec.decode_symbol(b, &mut self.m_0diff);
            if multi == 1 {
                self.last_diff = self.ic.decompress(dec, b, 0, 0);
                self.last = self.last.wrapping_add(self.last_diff as i64);
            } else if multi == 2 {
                self.last = dec.read_int64(b) as i64;
            }
        } else {
            let multi = dec.decode_symbol(b, &mut self.m_multi);
            if multi < GPS_MULTI_MAX_V1 - 2 {
                let diff;
                if multi == 1 {
                    diff = self.ic.decompress(dec, b, self.last_diff, 1);
                    self.last_diff = diff;
                    self.multi_extreme_counter = 0;
                } else if multi == 0 {
                    diff = self.ic.decompress(dec, b, self.last_diff / 4, 2);
                    self.multi_extreme_counter += 1;
                    if self.multi_extreme_counter > 3 {
                        self.last_diff = diff;
                        self.multi_extreme_counter = 0;
                    }
                } else if multi < 10 {
                    diff = self.ic.decompress(dec, b, (multi as i32).wrapping_mul(self.last_diff), 3);
                } else if multi < 50 {
                    diff = self.ic.decompress(dec, b, (multi as i32).wrapping_mul(self.last_diff), 4);
                } else {
                    diff = self.ic.decompress(dec, b, (multi as i32).wrapping_mul(self.last_diff), 5);
                    if multi == GPS_MULTI_MAX_V1 - 3 {
                        self.multi_extreme_counter += 1;
                        if self.multi_extreme_counter > 3 {
                            self.last_diff = diff;
                            self.multi_extreme_counter = 0;
                        }
                    }
                }
                self.last = self.last.wrapping_add(diff as i64);
            } else if multi < GPS_MULTI_MAX_V1 - 1 {
                self.last = dec.read_int64(b) as i64;
            }
        }
        wr_u64(rec, 0, self.last as u64);
    }
}

const GPS_MULTI: i32 = 500;
const GPS_MULTI_MINUS: i32 = -10;

/// GPS time sequences shared by GPSTIME11 v2 and POINT14 v3/v4.
#[derive(Clone)]
struct GpsSequences {
    last: usize,
    next: usize,
    times: [i64; 4],
    diffs: [i32; 4],
    multi_extreme_counter: [i32; 4],
}

impl GpsSequences {
    fn new() -> GpsSequences {
        GpsSequences { last: 0, next: 0, times: [0; 4], diffs: [0; 4], multi_extreme_counter: [0; 4] }
    }

    /// `code_full` is the symbol for a full 64-bit time, `unchanged` the end of the multiplier range,
    /// `no_diff_base` the base of the no-diff model (1 for v2, 0 for v3/v4).
    #[allow(clippy::too_many_arguments)]
    fn read(&mut self, dec: &mut Decoder, b: &[u8], ic: &mut IntDecompressor, m_no_diff: &mut Model, m_multi: &mut Model, code_full: i32, unchanged: i32, no_diff_base: i32) {
        loop {
            let last = self.last;
            if self.diffs[last] == 0 {
                let multi = dec.decode_symbol(b, m_no_diff) as i32 - no_diff_base;
                if multi == 0 {
                    self.diffs[last] = ic.decompress(dec, b, 0, 0);
                    self.times[last] = self.times[last].wrapping_add(self.diffs[last] as i64);
                    self.multi_extreme_counter[last] = 0;
                } else if multi == 1 {
                    self.read_full(dec, b, ic);
                } else if multi > 1 {
                    self.last = (last + multi as usize - 1) & 3;
                    continue;
                }
            } else {
                let mut multi = dec.decode_symbol(b, m_multi) as i32;
                if multi == 1 {
                    let d = ic.decompress(dec, b, self.diffs[last], 1);
                    self.times[last] = self.times[last].wrapping_add(d as i64);
                    self.multi_extreme_counter[last] = 0;
                } else if multi < unchanged {
                    let diff;
                    if multi == 0 {
                        diff = ic.decompress(dec, b, 0, 7);
                        self.multi_extreme_counter[last] += 1;
                        if self.multi_extreme_counter[last] > 3 {
                            self.diffs[last] = diff;
                            self.multi_extreme_counter[last] = 0;
                        }
                    } else if multi < GPS_MULTI {
                        diff = ic.decompress(dec, b, multi.wrapping_mul(self.diffs[last]), if multi < 10 { 2 } else { 3 });
                    } else if multi == GPS_MULTI {
                        diff = ic.decompress(dec, b, GPS_MULTI.wrapping_mul(self.diffs[last]), 4);
                        self.multi_extreme_counter[last] += 1;
                        if self.multi_extreme_counter[last] > 3 {
                            self.diffs[last] = diff;
                            self.multi_extreme_counter[last] = 0;
                        }
                    } else {
                        multi = GPS_MULTI - multi;
                        if multi > GPS_MULTI_MINUS {
                            diff = ic.decompress(dec, b, multi.wrapping_mul(self.diffs[last]), 5);
                        } else {
                            diff = ic.decompress(dec, b, GPS_MULTI_MINUS.wrapping_mul(self.diffs[last]), 6);
                            self.multi_extreme_counter[last] += 1;
                            if self.multi_extreme_counter[last] > 3 {
                                self.diffs[last] = diff;
                                self.multi_extreme_counter[last] = 0;
                            }
                        }
                    }
                    self.times[last] = self.times[last].wrapping_add(diff as i64);
                } else if multi == code_full {
                    self.read_full(dec, b, ic);
                } else if multi > code_full {
                    self.last = (last + (multi - code_full) as usize) & 3;
                    continue;
                }
            }
            break;
        }
    }

    fn read_full(&mut self, dec: &mut Decoder, b: &[u8], ic: &mut IntDecompressor) {
        self.next = (self.next + 1) & 3;
        let hi = ic.decompress(dec, b, (self.times[self.last] >> 32) as i32, 8);
        let lo = dec.read_int(b);
        self.times[self.next] = ((hi as i64) << 32) | (lo as i64);
        self.last = self.next;
        self.diffs[self.last] = 0;
        self.multi_extreme_counter[self.last] = 0;
    }
}

pub struct GpsTimeV2 {
    seq: GpsSequences,
    m_multi: Model,
    m_0diff: Model,
    ic: IntDecompressor,
}

impl GpsTimeV2 {
    pub fn new() -> GpsTimeV2 {
        GpsTimeV2 { seq: GpsSequences::new(), m_multi: Model::new((GPS_MULTI - GPS_MULTI_MINUS + 6) as u32), m_0diff: Model::new(6), ic: IntDecompressor::new(32, 9) }
    }
}

impl PointwiseItem for GpsTimeV2 {
    fn size(&self) -> usize {
        8
    }

    fn init(&mut self, rec: &[u8]) {
        self.seq.times[0] = rd_u64(rec, 0) as i64;
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        self.seq.read(dec, b, &mut self.ic, &mut self.m_0diff, &mut self.m_multi, GPS_MULTI - GPS_MULTI_MINUS + 2, GPS_MULTI - GPS_MULTI_MINUS + 1, 1);
        wr_u64(rec, 0, self.seq.times[self.seq.last] as u64);
    }
}

pub struct Rgb12V1 {
    last: [u16; 3],
    m_byte_used: Model,
    ic: IntDecompressor,
}

impl Rgb12V1 {
    pub fn new() -> Rgb12V1 {
        Rgb12V1 { last: [0; 3], m_byte_used: Model::new(64), ic: IntDecompressor::new(8, 6) }
    }
}

impl PointwiseItem for Rgb12V1 {
    fn size(&self) -> usize {
        6
    }

    fn init(&mut self, rec: &[u8]) {
        self.last = [rd_u16(rec, 0), rd_u16(rec, 2), rd_u16(rec, 4)];
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        let sym = dec.decode_symbol(b, &mut self.m_byte_used);
        for c in 0..3 {
            let last = self.last[c] as i32;
            let s = sym >> (2 * c);
            let mut v = if s & 1 != 0 { (self.ic.decompress(dec, b, last & 0xFF, 2 * c) as u16) as i32 } else { last & 0xFF };
            v |= if s & 2 != 0 { (self.ic.decompress(dec, b, last >> 8, 2 * c + 1) & 0xFF) << 8 } else { last & 0xFF00 };
            self.last[c] = v as u16;
            wr_u16(rec, 2 * c, self.last[c]);
        }
    }
}

/// RGB12 v2 models, also used per context by RGB14 v3/v4.
pub struct RgbModels {
    byte_used: Model,
    diff: [Model; 6],
}

impl RgbModels {
    fn new() -> RgbModels {
        RgbModels { byte_used: Model::new(128), diff: [Model::new(256), Model::new(256), Model::new(256), Model::new(256), Model::new(256), Model::new(256)] }
    }
}

/// Decodes an RGB triple relative to `last`.
fn decode_rgb(dec: &mut Decoder, b: &[u8], m: &mut RgbModels, last: &[u16; 3]) -> [u16; 3] {
    let sym = dec.decode_symbol(b, &mut m.byte_used);
    let (lr, lg, lb) = (last[0] as i32, last[1] as i32, last[2] as i32);
    let mut r = if sym & 1 != 0 { (dec.decode_symbol(b, &mut m.diff[0]) as i32 + (lr & 0xFF)) & 0xFF } else { lr & 0xFF };
    r |= if sym & 2 != 0 { ((dec.decode_symbol(b, &mut m.diff[1]) as i32 + (lr >> 8)) & 0xFF) << 8 } else { lr & 0xFF00 };
    let (mut g, mut bl);
    if sym & 64 != 0 {
        let mut diff = (r & 0xFF) - (lr & 0xFF);
        g = if sym & 4 != 0 { (dec.decode_symbol(b, &mut m.diff[2]) as i32 + clamp8(diff + (lg & 0xFF))) & 0xFF } else { lg & 0xFF };
        if sym & 16 != 0 {
            diff = (diff + ((g & 0xFF) - (lg & 0xFF))) / 2;
            bl = (dec.decode_symbol(b, &mut m.diff[4]) as i32 + clamp8(diff + (lb & 0xFF))) & 0xFF;
        } else {
            bl = lb & 0xFF;
        }
        diff = (r >> 8) - (lr >> 8);
        if sym & 8 != 0 {
            g |= ((dec.decode_symbol(b, &mut m.diff[3]) as i32 + clamp8(diff + (lg >> 8))) & 0xFF) << 8;
        } else {
            g |= lg & 0xFF00;
        }
        if sym & 32 != 0 {
            diff = (diff + ((g >> 8) - (lg >> 8))) / 2;
            bl |= ((dec.decode_symbol(b, &mut m.diff[5]) as i32 + clamp8(diff + (lb >> 8))) & 0xFF) << 8;
        } else {
            bl |= lb & 0xFF00;
        }
    } else {
        g = r;
        bl = r;
    }
    [r as u16, g as u16, bl as u16]
}

pub struct Rgb12V2 {
    last: [u16; 3],
    models: RgbModels,
}

impl Rgb12V2 {
    pub fn new() -> Rgb12V2 {
        Rgb12V2 { last: [0; 3], models: RgbModels::new() }
    }
}

impl PointwiseItem for Rgb12V2 {
    fn size(&self) -> usize {
        6
    }

    fn init(&mut self, rec: &[u8]) {
        self.last = [rd_u16(rec, 0), rd_u16(rec, 2), rd_u16(rec, 4)];
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        self.last = decode_rgb(dec, b, &mut self.models, &self.last);
        for c in 0..3 {
            wr_u16(rec, 2 * c, self.last[c]);
        }
    }
}

pub struct ByteV1 {
    last: Vec<u8>,
    ic: IntDecompressor,
}

impl ByteV1 {
    pub fn new(count: usize) -> ByteV1 {
        ByteV1 { last: vec![0; count], ic: IntDecompressor::new(8, count) }
    }
}

impl PointwiseItem for ByteV1 {
    fn size(&self) -> usize {
        self.last.len()
    }

    fn init(&mut self, rec: &[u8]) {
        let n = self.last.len();
        self.last.copy_from_slice(&rec[..n]);
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        for i in 0..self.last.len() {
            self.last[i] = self.ic.decompress(dec, b, self.last[i] as i32, i) as u8;
            rec[i] = self.last[i];
        }
    }
}

pub struct ByteV2 {
    last: Vec<u8>,
    models: Vec<Model>,
}

impl ByteV2 {
    pub fn new(count: usize) -> ByteV2 {
        ByteV2 { last: vec![0; count], models: (0..count).map(|_| Model::new(256)).collect() }
    }
}

impl PointwiseItem for ByteV2 {
    fn size(&self) -> usize {
        self.last.len()
    }

    fn init(&mut self, rec: &[u8]) {
        let n = self.last.len();
        self.last.copy_from_slice(&rec[..n]);
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        for i in 0..self.last.len() {
            self.last[i] = self.last[i].wrapping_add(dec.decode_symbol(b, &mut self.models[i]) as u8);
            rec[i] = self.last[i];
        }
    }
}

/// WAVEPACKET13 v1 (also used for item version 2, and per context by WAVEPACKET14 v3/v4).
pub struct Wavepacket13V1 {
    descriptor: u8,
    offset: u64,
    packet_size: i32,
    return_point: i32,
    dx: i32,
    dy: i32,
    dz: i32,
    last_diff32: i32,
    sym_last_offset_diff: usize,
    m_packet_index: Model,
    m_offset_diff: [Model; 4],
    ic_offset_diff: IntDecompressor,
    ic_packet_size: IntDecompressor,
    ic_return_point: IntDecompressor,
    ic_xyz: IntDecompressor,
}

impl Wavepacket13V1 {
    pub fn new() -> Wavepacket13V1 {
        Wavepacket13V1 {
            descriptor: 0,
            offset: 0,
            packet_size: 0,
            return_point: 0,
            dx: 0,
            dy: 0,
            dz: 0,
            last_diff32: 0,
            sym_last_offset_diff: 0,
            m_packet_index: Model::new(256),
            m_offset_diff: [Model::new(4), Model::new(4), Model::new(4), Model::new(4)],
            ic_offset_diff: IntDecompressor::new(32, 1),
            ic_packet_size: IntDecompressor::new(32, 1),
            ic_return_point: IntDecompressor::new(32, 1),
            ic_xyz: IntDecompressor::new(32, 3),
        }
    }

    fn pack(&self, rec: &mut [u8]) {
        rec[0] = self.descriptor;
        wr_u64(rec, 1, self.offset);
        wr_i32(rec, 9, self.packet_size);
        wr_i32(rec, 13, self.return_point);
        wr_i32(rec, 17, self.dx);
        wr_i32(rec, 21, self.dy);
        wr_i32(rec, 25, self.dz);
    }
}

impl PointwiseItem for Wavepacket13V1 {
    fn size(&self) -> usize {
        29
    }

    fn init(&mut self, rec: &[u8]) {
        self.descriptor = rec[0];
        self.offset = rd_u64(rec, 1);
        self.packet_size = rd_i32(rec, 9);
        self.return_point = rd_i32(rec, 13);
        self.dx = rd_i32(rec, 17);
        self.dy = rd_i32(rec, 21);
        self.dz = rd_i32(rec, 25);
    }

    fn read(&mut self, dec: &mut Decoder, b: &[u8], rec: &mut [u8]) {
        self.descriptor = dec.decode_symbol(b, &mut self.m_packet_index) as u8;
        self.sym_last_offset_diff = dec.decode_symbol(b, &mut self.m_offset_diff[self.sym_last_offset_diff]) as usize;
        match self.sym_last_offset_diff {
            0 => {}
            1 => self.offset = self.offset.wrapping_add(self.packet_size as u32 as u64),
            2 => {
                self.last_diff32 = self.ic_offset_diff.decompress(dec, b, self.last_diff32, 0);
                self.offset = self.offset.wrapping_add(self.last_diff32 as i64 as u64);
            }
            _ => self.offset = dec.read_int64(b),
        }
        self.packet_size = self.ic_packet_size.decompress(dec, b, self.packet_size, 0);
        self.return_point = self.ic_return_point.decompress(dec, b, self.return_point, 0);
        self.dx = self.ic_xyz.decompress(dec, b, self.dx, 0);
        self.dy = self.ic_xyz.decompress(dec, b, self.dy, 1);
        self.dz = self.ic_xyz.decompress(dec, b, self.dz, 2);
        self.pack(rec);
    }
}

//----------------------------------------------------------------------------------------------------------------------
// Layered item decoders (compressor 3, item versions 3 and 4)
//----------------------------------------------------------------------------------------------------------------------

pub trait LayeredItem {
    fn size(&self) -> usize;
    fn num_layers(&self) -> usize;
    /// Takes the raw first item of a chunk; returns the context for the following items.
    fn init_first(&mut self, rec: &[u8], ctx: usize) -> usize;
    fn set_layer_sizes(&mut self, sizes: &[u32]);
    /// Sets up the layer decoders starting at `pos`; returns the position after the layers.
    fn set_layers(&mut self, bytes: &[u8], pos: usize) -> usize;
    /// Decodes the next item into `rec`; returns the context for the following items.
    fn read(&mut self, bytes: &[u8], rec: &mut [u8], ctx: usize) -> usize;
}

fn layer_decoder(bytes: &[u8], pos: usize, size: u32) -> Option<Decoder> {
    if size == 0 {
        return None;
    }
    let end = (pos + size as usize).min(bytes.len());
    let mut dec = Decoder::new(pos, end);
    dec.init(bytes);
    Some(dec)
}

#[derive(Default, Clone, Copy)]
struct Point14 {
    x: i32,
    y: i32,
    z: i32,
    intensity: u16,
    returns: u8,
    flags: u8,
    classification: u8,
    user_data: u8,
    scan_angle: i16,
    point_source_id: u16,
    gps_time: i64,
}

impl Point14 {
    fn unpack(rec: &[u8]) -> Point14 {
        Point14 {
            x: rd_i32(rec, 0),
            y: rd_i32(rec, 4),
            z: rd_i32(rec, 8),
            intensity: rd_u16(rec, 12),
            returns: rec[14],
            flags: rec[15],
            classification: rec[16],
            user_data: rec[17],
            scan_angle: rd_u16(rec, 18) as i16,
            point_source_id: rd_u16(rec, 20),
            gps_time: rd_u64(rec, 22) as i64,
        }
    }

    fn pack(&self, rec: &mut [u8]) {
        wr_i32(rec, 0, self.x);
        wr_i32(rec, 4, self.y);
        wr_i32(rec, 8, self.z);
        wr_u16(rec, 12, self.intensity);
        rec[14] = self.returns;
        rec[15] = self.flags;
        rec[16] = self.classification;
        rec[17] = self.user_data;
        wr_u16(rec, 18, self.scan_angle as u16);
        wr_u16(rec, 20, self.point_source_id);
        wr_u64(rec, 22, self.gps_time as u64);
    }
}

struct Point14Context {
    unused: bool,
    last: Point14,
    gps_time_change: bool,
    last_intensity: [u16; 8],
    last_x_diff_median5: [Median5; 12],
    last_y_diff_median5: [Median5; 12],
    last_z: [i32; 8],
    m_changed_values: Vec<Model>,
    m_scanner_channel: Model,
    m_number_of_returns: Vec<Option<Model>>,
    m_return_number: Vec<Option<Model>>,
    m_return_number_gps_same: Model,
    m_classification: Vec<Option<Model>>,
    m_flags: Vec<Option<Model>>,
    m_user_data: Vec<Option<Model>>,
    m_gps_time_multi: Model,
    m_gps_time_no_diff: Model,
    ic_dx: IntDecompressor,
    ic_dy: IntDecompressor,
    ic_z: IntDecompressor,
    ic_intensity: IntDecompressor,
    ic_scan_angle: IntDecompressor,
    ic_point_source_id: IntDecompressor,
    ic_gps_time: IntDecompressor,
    gps: GpsSequences,
}

impl Point14Context {
    fn new() -> Point14Context {
        Point14Context {
            unused: true,
            last: Point14::default(),
            gps_time_change: false,
            last_intensity: [0; 8],
            last_x_diff_median5: [Median5::new(); 12],
            last_y_diff_median5: [Median5::new(); 12],
            last_z: [0; 8],
            m_changed_values: (0..8).map(|_| Model::new(128)).collect(),
            m_scanner_channel: Model::new(3),
            m_number_of_returns: (0..16).map(|_| None).collect(),
            m_return_number: (0..16).map(|_| None).collect(),
            m_return_number_gps_same: Model::new(13),
            m_classification: (0..64).map(|_| None).collect(),
            m_flags: (0..64).map(|_| None).collect(),
            m_user_data: (0..64).map(|_| None).collect(),
            m_gps_time_multi: Model::new((GPS_MULTI - GPS_MULTI_MINUS + 5) as u32),
            m_gps_time_no_diff: Model::new(5),
            ic_dx: IntDecompressor::new(32, 2),
            ic_dy: IntDecompressor::new(32, 22),
            ic_z: IntDecompressor::new(32, 20),
            ic_intensity: IntDecompressor::new(16, 4),
            ic_scan_angle: IntDecompressor::new(16, 2),
            ic_point_source_id: IntDecompressor::new(16, 1),
            ic_gps_time: IntDecompressor::new(32, 9),
            gps: GpsSequences::new(),
        }
    }

    /// Initializes this context from a last point (the raw first point, or another context's last point).
    fn init_from(&mut self, p: &Point14) {
        *self = Point14Context::new();
        self.unused = false;
        self.last = *p;
        self.gps_time_change = false;
        self.last_intensity = [p.intensity; 8];
        self.last_z = [p.z; 8];
        self.gps.times[0] = p.gps_time;
    }
}

pub struct Point14V3 {
    version: u16,
    layer_sizes: [u32; 9],
    contexts: Vec<Point14Context>,
    current_context: usize,
    dec_channel_returns_xy: Option<Decoder>,
    dec_z: Option<Decoder>,
    dec_classification: Option<Decoder>,
    dec_flags: Option<Decoder>,
    dec_intensity: Option<Decoder>,
    dec_scan_angle: Option<Decoder>,
    dec_user_data: Option<Decoder>,
    dec_point_source: Option<Decoder>,
    dec_gps_time: Option<Decoder>,
}

impl Point14V3 {
    pub fn new(version: u16) -> Point14V3 {
        Point14V3 {
            version,
            layer_sizes: [0; 9],
            contexts: (0..4).map(|_| Point14Context::new()).collect(),
            current_context: 0,
            dec_channel_returns_xy: None,
            dec_z: None,
            dec_classification: None,
            dec_flags: None,
            dec_intensity: None,
            dec_scan_angle: None,
            dec_user_data: None,
            dec_point_source: None,
            dec_gps_time: None,
        }
    }
}

impl LayeredItem for Point14V3 {
    fn size(&self) -> usize {
        30
    }

    fn num_layers(&self) -> usize {
        9
    }

    fn init_first(&mut self, rec: &[u8], _ctx: usize) -> usize {
        let p = Point14::unpack(rec);
        for c in self.contexts.iter_mut() {
            c.unused = true;
        }
        self.current_context = ((p.flags >> 4) & 3) as usize;
        self.contexts[self.current_context].init_from(&p);
        self.current_context
    }

    fn set_layer_sizes(&mut self, sizes: &[u32]) {
        self.layer_sizes.copy_from_slice(&sizes[..9]);
    }

    fn set_layers(&mut self, bytes: &[u8], mut pos: usize) -> usize {
        let s = self.layer_sizes;
        let mut next = |size: u32| {
            let d = layer_decoder(bytes, pos, size);
            pos += size as usize;
            d
        };
        self.dec_channel_returns_xy = next(s[0]);
        self.dec_z = next(s[1]);
        self.dec_classification = next(s[2]);
        self.dec_flags = next(s[3]);
        self.dec_intensity = next(s[4]);
        self.dec_scan_angle = next(s[5]);
        self.dec_user_data = next(s[6]);
        self.dec_point_source = next(s[7]);
        self.dec_gps_time = next(s[8]);
        pos
    }

    fn read(&mut self, b: &[u8], rec: &mut [u8], mut shared_ctx: usize) -> usize {
        let dec_xy = self.dec_channel_returns_xy.as_mut().expect("layer");
        let ctx_i = self.current_context;
        let (last_r, last_n, lpr) = {
            let ctx = &self.contexts[ctx_i];
            let last_r = (ctx.last.returns & 0x0F) as u32;
            let last_n = (ctx.last.returns >> 4) as u32;
            (last_r, last_n, (if last_r == 1 { 1 } else { 0 }) + (if last_r >= last_n { 2 } else { 0 }) + (if ctx.gps_time_change { 4 } else { 0 }))
        };
        let changed = dec_xy.decode_symbol(b, &mut self.contexts[ctx_i].m_changed_values[lpr]);
        if changed & 64 != 0 {
            let diff = dec_xy.decode_symbol(b, &mut self.contexts[ctx_i].m_scanner_channel) as usize;
            let scanner_channel = (self.current_context + diff + 1) % 4;
            if self.contexts[scanner_channel].unused {
                let mut snapshot = self.contexts[self.current_context].last;
                let g = &self.contexts[self.current_context].gps;
                snapshot.gps_time = g.times[g.last];
                self.contexts[scanner_channel].init_from(&snapshot);
            }
            self.current_context = scanner_channel;
            if self.version == 3 {
                shared_ctx = self.current_context;
            }
            let ctx = &mut self.contexts[scanner_channel];
            ctx.last.flags = (ctx.last.flags & 0xCF) | ((scanner_channel as u8) << 4);
        }
        if self.version == 4 {
            shared_ctx = self.current_context;
        }
        let ctx = &mut self.contexts[self.current_context];
        let point_source_changed = changed & 32 != 0;
        let gps_time_changed = changed & 16 != 0;
        let scan_angle_changed = changed & 8 != 0;
        let gtc = if gps_time_changed { 1usize } else { 0 };
        let n = if changed & 4 != 0 {
            dec_xy.decode_symbol(b, lazy(&mut ctx.m_number_of_returns, last_n as usize, 16))
        } else {
            last_n
        };
        let r = match changed & 3 {
            0 => last_r,
            1 => (last_r + 1) % 16,
            2 => (last_r + 15) % 16,
            _ => {
                if gps_time_changed {
                    dec_xy.decode_symbol(b, lazy(&mut ctx.m_return_number, last_r as usize, 16))
                } else {
                    (last_r + dec_xy.decode_symbol(b, &mut ctx.m_return_number_gps_same) + 2) % 16
                }
            }
        };
        ctx.last.returns = ((n << 4) | r) as u8;
        let m = NUMBER_RETURN_MAP_6CTX[n as usize][r as usize] as usize;
        let l = NUMBER_RETURN_LEVEL_8CTX[n as usize][r as usize] as usize;
        let cpr = (if r == 1 { 2 } else { 0 }) + (if r >= n { 1 } else { 0 });
        let n1 = if n == 1 { 1usize } else { 0 };
        let idx = (m << 1) | gtc;
        let median = ctx.last_x_diff_median5[idx].get();
        let diff = ctx.ic_dx.decompress(dec_xy, b, median, n1);
        ctx.last.x = ctx.last.x.wrapping_add(diff);
        ctx.last_x_diff_median5[idx].add(diff);
        let median = ctx.last_y_diff_median5[idx].get();
        let mut k_bits = ctx.ic_dx.k;
        let diff = ctx.ic_dy.decompress(dec_xy, b, median, n1 + if k_bits < 20 { (k_bits & !1) as usize } else { 20 });
        ctx.last.y = ctx.last.y.wrapping_add(diff);
        ctx.last_y_diff_median5[idx].add(diff);
        if let Some(dec) = self.dec_z.as_mut() {
            k_bits = (ctx.ic_dx.k + ctx.ic_dy.k) / 2;
            ctx.last.z = ctx.ic_z.decompress(dec, b, ctx.last_z[l], n1 + if k_bits < 18 { (k_bits & !1) as usize } else { 18 });
            ctx.last_z[l] = ctx.last.z;
        }
        if let Some(dec) = self.dec_classification.as_mut() {
            let ccc = (((ctx.last.classification & 0x1F) as usize) << 1) + if cpr == 3 { 1 } else { 0 };
            ctx.last.classification = dec.decode_symbol(b, lazy(&mut ctx.m_classification, ccc, 256)) as u8;
        }
        if let Some(dec) = self.dec_flags.as_mut() {
            let f = ctx.last.flags;
            let last_flags = (((f >> 7) as usize) << 5) | ((((f >> 6) & 1) as usize) << 4) | (f & 0x0F) as usize;
            let flags = dec.decode_symbol(b, lazy(&mut ctx.m_flags, last_flags, 64)) as u8;
            ctx.last.flags = (((flags >> 5) & 1) << 7) | (((flags >> 4) & 1) << 6) | (f & 0x30) | (flags & 0x0F);
        }
        if let Some(dec) = self.dec_intensity.as_mut() {
            let iidx = (cpr << 1) | gtc;
            ctx.last.intensity = ctx.ic_intensity.decompress(dec, b, ctx.last_intensity[iidx] as i32, cpr) as u16;
            ctx.last_intensity[iidx] = ctx.last.intensity;
        }
        if scan_angle_changed {
            if let Some(dec) = self.dec_scan_angle.as_mut() {
                ctx.last.scan_angle = ctx.ic_scan_angle.decompress(dec, b, ctx.last.scan_angle as i32, gtc) as i16;
            }
        }
        if let Some(dec) = self.dec_user_data.as_mut() {
            let uidx = (ctx.last.user_data >> 2) as usize;
            ctx.last.user_data = dec.decode_symbol(b, lazy(&mut ctx.m_user_data, uidx, 256)) as u8;
        }
        if point_source_changed {
            if let Some(dec) = self.dec_point_source.as_mut() {
                ctx.last.point_source_id = ctx.ic_point_source_id.decompress(dec, b, ctx.last.point_source_id as i32, 0) as u16;
            }
        }
        if gps_time_changed {
            if let Some(dec) = self.dec_gps_time.as_mut() {
                ctx.gps.read(dec, b, &mut ctx.ic_gps_time, &mut ctx.m_gps_time_no_diff, &mut ctx.m_gps_time_multi, GPS_MULTI - GPS_MULTI_MINUS + 1, GPS_MULTI - GPS_MULTI_MINUS + 1, 0);
            }
        }
        ctx.gps_time_change = gps_time_changed;
        ctx.last.gps_time = ctx.gps.times[ctx.gps.last];
        ctx.last.pack(rec);
        shared_ctx
    }
}

/// Per-context state shared by the single-layer items (RGB14, NIR14, WAVEPACKET14).
struct ContextItem<T, M> {
    version: u16,
    layer_size: u32,
    dec: Option<Decoder>,
    last_context_used: usize,
    unused: [bool; 4],
    models: [Option<M>; 4],
    last: [T; 4],
}

impl<T: Copy + Default, M> ContextItem<T, M> {
    fn new(version: u16) -> ContextItem<T, M> {
        ContextItem { version, layer_size: 0, dec: None, last_context_used: 0, unused: [true; 4], models: [None, None, None, None], last: [T::default(); 4] }
    }

    fn init_first(&mut self, value: T, ctx: usize) -> usize {
        self.unused = [true; 4];
        self.models = [None, None, None, None];
        self.last[ctx] = value;
        self.unused[ctx] = false;
        self.last_context_used = ctx;
        ctx
    }

    /// Context-switch bookkeeping; returns the index of the "last" value to predict from.
    fn switch(&mut self, ctx: usize) -> usize {
        let mut last_idx = self.last_context_used;
        if last_idx != ctx {
            self.last_context_used = ctx;
            if self.unused[ctx] {
                self.last[ctx] = self.last[last_idx];
                self.unused[ctx] = false;
                self.models[ctx] = None;
                if self.version == 3 {
                    last_idx = ctx;
                }
            }
            if self.version == 4 {
                last_idx = ctx;
            }
        }
        last_idx
    }
}

pub struct Rgb14V3(ContextItem<[u16; 3], RgbModels>);

impl Rgb14V3 {
    pub fn new(version: u16) -> Rgb14V3 {
        Rgb14V3(ContextItem::new(version))
    }
}

impl LayeredItem for Rgb14V3 {
    fn size(&self) -> usize {
        6
    }

    fn num_layers(&self) -> usize {
        1
    }

    fn init_first(&mut self, rec: &[u8], ctx: usize) -> usize {
        self.0.init_first([rd_u16(rec, 0), rd_u16(rec, 2), rd_u16(rec, 4)], ctx)
    }

    fn set_layer_sizes(&mut self, sizes: &[u32]) {
        self.0.layer_size = sizes[0];
    }

    fn set_layers(&mut self, bytes: &[u8], pos: usize) -> usize {
        self.0.dec = layer_decoder(bytes, pos, self.0.layer_size);
        pos + self.0.layer_size as usize
    }

    fn read(&mut self, b: &[u8], rec: &mut [u8], ctx: usize) -> usize {
        let last_idx = self.0.switch(ctx);
        if let Some(dec) = self.0.dec.as_mut() {
            let models = self.0.models[ctx].get_or_insert_with(RgbModels::new);
            self.0.last[last_idx] = decode_rgb(dec, b, models, &self.0.last[last_idx]);
        }
        for c in 0..3 {
            wr_u16(rec, 2 * c, self.0.last[last_idx][c]);
        }
        ctx
    }
}

pub struct NirModels {
    bytes_used: Model,
    lower: Model,
    upper: Model,
}

pub struct Nir14V3(ContextItem<u16, NirModels>);

impl Nir14V3 {
    pub fn new(version: u16) -> Nir14V3 {
        Nir14V3(ContextItem::new(version))
    }
}

impl LayeredItem for Nir14V3 {
    fn size(&self) -> usize {
        2
    }

    fn num_layers(&self) -> usize {
        1
    }

    fn init_first(&mut self, rec: &[u8], ctx: usize) -> usize {
        self.0.init_first(rd_u16(rec, 0), ctx)
    }

    fn set_layer_sizes(&mut self, sizes: &[u32]) {
        self.0.layer_size = sizes[0];
    }

    fn set_layers(&mut self, bytes: &[u8], pos: usize) -> usize {
        self.0.dec = layer_decoder(bytes, pos, self.0.layer_size);
        pos + self.0.layer_size as usize
    }

    fn read(&mut self, b: &[u8], rec: &mut [u8], ctx: usize) -> usize {
        let last_idx = self.0.switch(ctx);
        if let Some(dec) = self.0.dec.as_mut() {
            let m = self.0.models[ctx].get_or_insert_with(|| NirModels { bytes_used: Model::new(4), lower: Model::new(256), upper: Model::new(256) });
            let last = self.0.last[last_idx] as u32;
            let sym = dec.decode_symbol(b, &mut m.bytes_used);
            let mut nir = if sym & 1 != 0 { (dec.decode_symbol(b, &mut m.lower) + (last & 0xFF)) & 0xFF } else { last & 0xFF };
            nir |= if sym & 2 != 0 { ((dec.decode_symbol(b, &mut m.upper) + (last >> 8)) & 0xFF) << 8 } else { last & 0xFF00 };
            self.0.last[last_idx] = nir as u16;
        }
        wr_u16(rec, 0, self.0.last[last_idx]);
        ctx
    }
}

pub struct Wavepacket14V3(ContextItem<[u8; 29], Wavepacket13V1>);

impl Wavepacket14V3 {
    pub fn new(version: u16) -> Wavepacket14V3 {
        Wavepacket14V3(ContextItem::new(version))
    }
}

impl LayeredItem for Wavepacket14V3 {
    fn size(&self) -> usize {
        29
    }

    fn num_layers(&self) -> usize {
        1
    }

    fn init_first(&mut self, rec: &[u8], ctx: usize) -> usize {
        let mut v = [0u8; 29];
        v.copy_from_slice(&rec[..29]);
        self.0.init_first(v, ctx)
    }

    fn set_layer_sizes(&mut self, sizes: &[u32]) {
        self.0.layer_size = sizes[0];
    }

    fn set_layers(&mut self, bytes: &[u8], pos: usize) -> usize {
        self.0.dec = layer_decoder(bytes, pos, self.0.layer_size);
        pos + self.0.layer_size as usize
    }

    fn read(&mut self, b: &[u8], rec: &mut [u8], ctx: usize) -> usize {
        let last_idx = self.0.switch(ctx);
        if let Some(dec) = self.0.dec.as_mut() {
            let wp = self.0.models[ctx].get_or_insert_with(Wavepacket13V1::new);
            let last = self.0.last[last_idx];
            wp.init(&last);
            wp.read(dec, b, &mut rec[..29]);
            self.0.last[last_idx].copy_from_slice(&rec[..29]);
        } else {
            rec[..29].copy_from_slice(&self.0.last[last_idx]);
        }
        ctx
    }
}

pub struct Byte14V3 {
    version: u16,
    count: usize,
    layer_sizes: Vec<u32>,
    decs: Vec<Option<Decoder>>,
    last: [Vec<u8>; 4],
    unused: [bool; 4],
    models: [Option<Vec<Model>>; 4],
    last_context_used: usize,
}

impl Byte14V3 {
    pub fn new(version: u16, count: usize) -> Byte14V3 {
        Byte14V3 {
            version,
            count,
            layer_sizes: vec![0; count],
            decs: (0..count).map(|_| None).collect(),
            last: [vec![0; count], vec![0; count], vec![0; count], vec![0; count]],
            unused: [true; 4],
            models: [None, None, None, None],
            last_context_used: 0,
        }
    }
}

impl LayeredItem for Byte14V3 {
    fn size(&self) -> usize {
        self.count
    }

    fn num_layers(&self) -> usize {
        self.count
    }

    fn init_first(&mut self, rec: &[u8], ctx: usize) -> usize {
        self.unused = [true; 4];
        self.models = [None, None, None, None];
        self.last[ctx].copy_from_slice(&rec[..self.count]);
        self.unused[ctx] = false;
        self.last_context_used = ctx;
        ctx
    }

    fn set_layer_sizes(&mut self, sizes: &[u32]) {
        self.layer_sizes.copy_from_slice(&sizes[..self.count]);
    }

    fn set_layers(&mut self, bytes: &[u8], mut pos: usize) -> usize {
        for i in 0..self.count {
            self.decs[i] = layer_decoder(bytes, pos, self.layer_sizes[i]);
            pos += self.layer_sizes[i] as usize;
        }
        pos
    }

    fn read(&mut self, b: &[u8], rec: &mut [u8], ctx: usize) -> usize {
        let mut last_idx = self.last_context_used;
        if last_idx != ctx {
            self.last_context_used = ctx;
            if self.unused[ctx] {
                let (src, dst) = if last_idx < ctx {
                    let (a, bb) = self.last.split_at_mut(ctx);
                    (&a[last_idx], &mut bb[0])
                } else {
                    let (a, bb) = self.last.split_at_mut(last_idx);
                    (&bb[0], &mut a[ctx])
                };
                dst.copy_from_slice(src);
                self.unused[ctx] = false;
                self.models[ctx] = None;
                if self.version == 3 {
                    last_idx = ctx;
                }
            }
            if self.version == 4 {
                last_idx = ctx;
            }
        }
        let count = self.count;
        let models = self.models[ctx].get_or_insert_with(|| (0..count).map(|_| Model::new(256)).collect());
        let last = &mut self.last[last_idx];
        for i in 0..count {
            if let Some(dec) = self.decs[i].as_mut() {
                last[i] = last[i].wrapping_add(dec.decode_symbol(b, &mut models[i]) as u8);
            }
            rec[i] = last[i];
        }
        ctx
    }
}

//----------------------------------------------------------------------------------------------------------------------
// LASzip VLR and chunk driver
//----------------------------------------------------------------------------------------------------------------------

pub const COMPRESSOR_NONE: u16 = 0;
const COMPRESSOR_POINTWISE: u16 = 1;
const COMPRESSOR_POINTWISE_CHUNKED: u16 = 2;
const COMPRESSOR_LAYERED_CHUNKED: u16 = 3;
const VARIABLE_CHUNK_SIZE: u32 = 0xFFFF_FFFF;

#[derive(Clone, Copy)]
pub struct LazItem {
    pub item_type: u16,
    pub size: u16,
    pub version: u16,
}

pub struct LazVlr {
    pub compressor: u16,
    pub coder: u16,
    pub chunk_size: u32,
    pub items: Vec<LazItem>,
}

/// Parses the record data of a "laszip encoded" VLR (record ID 22204).
pub fn parse_laszip_vlr(data: &[u8]) -> Result<LazVlr, String> {
    if data.len() < 34 {
        return Err("LAZ: LASzip VLR is too short".into());
    }
    let num_items = rd_u16(data, 32) as usize;
    if data.len() < 34 + num_items * 6 {
        return Err("LAZ: LASzip VLR is truncated".into());
    }
    let items = (0..num_items)
        .map(|i| LazItem { item_type: rd_u16(data, 34 + i * 6), size: rd_u16(data, 36 + i * 6), version: rd_u16(data, 38 + i * 6) })
        .collect();
    Ok(LazVlr { compressor: rd_u16(data, 0), coder: rd_u16(data, 2), chunk_size: rd_u32(data, 12), items })
}

enum Items {
    Pointwise(Vec<Box<dyn PointwiseItem>>),
    Layered(Vec<Box<dyn LayeredItem>>),
}

fn create_items(items: &[LazItem], layered: bool) -> Result<Items, String> {
    let unsupported = |it: &LazItem| Err(format!("LAZ: unsupported LASzip item type {} version {}", it.item_type, it.version));
    if layered {
        let mut out: Vec<Box<dyn LayeredItem>> = Vec::new();
        for it in items {
            let v = it.version;
            if v != 3 && v != 4 {
                return unsupported(it);
            }
            match it.item_type {
                10 => out.push(Box::new(Point14V3::new(v))),
                11 => out.push(Box::new(Rgb14V3::new(v))),
                12 => {
                    out.push(Box::new(Rgb14V3::new(v)));
                    out.push(Box::new(Nir14V3::new(v)));
                }
                13 => out.push(Box::new(Wavepacket14V3::new(v))),
                14 => out.push(Box::new(Byte14V3::new(v, it.size as usize))),
                _ => return unsupported(it),
            }
        }
        Ok(Items::Layered(out))
    } else {
        let mut out: Vec<Box<dyn PointwiseItem>> = Vec::new();
        for it in items {
            let v = it.version;
            let d: Box<dyn PointwiseItem> = match (it.item_type, v) {
                (6, 1) => Box::new(Point10V1::new()),
                (6, 2) => Box::new(Point10V2::new()),
                (7, 1) => Box::new(GpsTimeV1::new()),
                (7, 2) => Box::new(GpsTimeV2::new()),
                (8, 1) => Box::new(Rgb12V1::new()),
                (8, 2) => Box::new(Rgb12V2::new()),
                (0, 1) => Box::new(ByteV1::new(it.size as usize)),
                (0, 2) => Box::new(ByteV2::new(it.size as usize)),
                (9, 1) | (9, 2) => Box::new(Wavepacket13V1::new()),
                _ => return unsupported(it),
            };
            if d.size() != it.size as usize {
                return Err(format!("LAZ: unexpected size {} for LASzip item type {}", it.size, it.item_type));
            }
            out.push(d);
        }
        Ok(Items::Pointwise(out))
    }
}

struct ChunkEntry {
    point_count: u32,
}

fn read_chunk_table(bytes: &[u8], table_offset: usize) -> Result<Vec<ChunkEntry>, String> {
    if table_offset + 8 > bytes.len() {
        return Err("LAZ: chunk table lies beyond the end of the file".into());
    }
    if rd_u32(bytes, table_offset) != 0 {
        return Err(format!("LAZ: unsupported chunk table version {}", rd_u32(bytes, table_offset)));
    }
    let num_chunks = rd_u32(bytes, table_offset + 4) as usize;
    let mut dec = Decoder::new(table_offset + 8, bytes.len());
    dec.init(bytes);
    let mut ic = IntDecompressor::new(32, 2);
    let mut table = Vec::with_capacity(num_chunks.min(1 << 20));
    let mut prev_count = 0i32;
    let mut prev_bytes = 0i32;
    for _ in 0..num_chunks {
        let point_count = ic.decompress(&mut dec, bytes, prev_count, 0);
        let byte_count = ic.decompress(&mut dec, bytes, prev_bytes, 1);
        table.push(ChunkEntry { point_count: point_count as u32 });
        prev_count = point_count;
        prev_bytes = byte_count;
    }
    Ok(table)
}

/// Sequential reader of LASzip-compressed point records. Each call to `read_point` decodes one raw point data
/// record (same layout as an uncompressed LAS record, extra bytes included) into `record`.
pub struct LazReader {
    vlr: LazVlr,
    layered: bool,
    num_points: u64,
    points_read: u64,
    pub record_size: usize,
    pub record: Vec<u8>,
    pos: usize,
    chunk_size: u32,
    chunk_remaining: u32,
    chunk_index: usize,
    chunk_table: Option<Vec<ChunkEntry>>,
    items: Option<Items>,
    dec: Decoder,
}

impl LazReader {
    pub fn new(bytes: &[u8], point_data_offset: usize, num_points: u64, laz_vlr_data: &[u8]) -> Result<LazReader, String> {
        let vlr = parse_laszip_vlr(laz_vlr_data)?;
        if vlr.coder != 0 {
            return Err(format!("LAZ: unsupported coder {}", vlr.coder));
        }
        if vlr.compressor != COMPRESSOR_POINTWISE && vlr.compressor != COMPRESSOR_POINTWISE_CHUNKED && vlr.compressor != COMPRESSOR_LAYERED_CHUNKED {
            return Err(format!("LAZ: unsupported compressor {}", vlr.compressor));
        }
        if vlr.items.is_empty() {
            return Err("LAZ: no LASzip items".into());
        }
        let layered = vlr.compressor == COMPRESSOR_LAYERED_CHUNKED;
        let record_size: usize = vlr.items.iter().map(|i| i.size as usize).sum();
        let mut reader = LazReader {
            layered,
            num_points,
            points_read: 0,
            record_size,
            record: vec![0; record_size],
            pos: point_data_offset,
            chunk_size: 0,
            chunk_remaining: 0,
            chunk_index: 0,
            chunk_table: None,
            items: None,
            dec: Decoder::new(0, 0),
            vlr,
        };
        if reader.vlr.compressor == COMPRESSOR_POINTWISE {
            reader.chunk_size = num_points.min(u32::MAX as u64) as u32;
        } else {
            reader.chunk_size = if reader.vlr.chunk_size == 0 { VARIABLE_CHUNK_SIZE } else { reader.vlr.chunk_size };
            if point_data_offset + 8 > bytes.len() {
                return Err("LAZ: unexpected end of file".into());
            }
            let mut table_offset = rd_u64(bytes, point_data_offset) as i64;
            reader.pos += 8;
            if reader.chunk_size == VARIABLE_CHUNK_SIZE && !layered {
                if table_offset <= point_data_offset as i64 && bytes.len() >= 8 {
                    table_offset = rd_u64(bytes, bytes.len() - 8) as i64;
                }
                if table_offset <= point_data_offset as i64 || table_offset as u64 + 8 > bytes.len() as u64 {
                    return Err("LAZ: variable-size chunks without a chunk table".into());
                }
                reader.chunk_table = Some(read_chunk_table(bytes, table_offset as usize)?);
            }
        }
        Ok(reader)
    }

    /// Decodes the next point record into `self.record`.
    pub fn read_point(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.chunk_remaining == 0 {
            self.start_chunk(bytes)?; // Leaves the chunk's raw first point in self.record
        } else {
            match self.items.as_mut().expect("items") {
                Items::Layered(items) => {
                    let mut ctx = 0;
                    let mut off = 0;
                    for d in items.iter_mut() {
                        let size = d.size();
                        ctx = d.read(bytes, &mut self.record[off..off + size], ctx);
                        off += size;
                    }
                }
                Items::Pointwise(items) => {
                    let mut off = 0;
                    for d in items.iter_mut() {
                        let size = d.size();
                        d.read(&mut self.dec, bytes, &mut self.record[off..off + size]);
                        off += size;
                    }
                }
            }
        }
        self.chunk_remaining -= 1;
        self.points_read += 1;
        Ok(())
    }

    fn start_chunk(&mut self, bytes: &[u8]) -> Result<(), String> {
        if !self.layered && self.items.is_some() {
            self.pos = self.dec.position(); // Continue after the bytes consumed by the previous chunk's decoder
        }
        if self.pos + self.record_size > bytes.len() {
            return Err("LAZ: unexpected end of file".into());
        }
        let remaining = self.num_points - self.points_read;
        let mut chunk_points = match &self.chunk_table {
            Some(table) => table.get(self.chunk_index).map(|e| e.point_count).ok_or("LAZ: chunk table is too short")?,
            None => (self.chunk_size as u64).min(remaining) as u32,
        };
        self.chunk_index += 1;
        let mut items = create_items(&self.vlr.items, self.layered)?;
        self.record.copy_from_slice(&bytes[self.pos..self.pos + self.record_size]);
        let mut pos = self.pos + self.record_size;
        match &mut items {
            Items::Layered(items) => {
                let mut ctx = 0;
                let mut off = 0;
                for d in items.iter_mut() {
                    let size = d.size();
                    ctx = d.init_first(&self.record[off..off + size], ctx);
                    off += size;
                }
                let total_layers: usize = items.iter().map(|d| d.num_layers()).sum();
                if pos + 4 + 4 * total_layers > bytes.len() {
                    return Err("LAZ: unexpected end of file in chunk header".into());
                }
                let count = rd_u32(bytes, pos);
                pos += 4;
                for d in items.iter_mut() {
                    let n = d.num_layers();
                    let sizes: Vec<u32> = (0..n).map(|i| rd_u32(bytes, pos + 4 * i)).collect();
                    d.set_layer_sizes(&sizes);
                    pos += 4 * n;
                }
                for d in items.iter_mut() {
                    pos = d.set_layers(bytes, pos);
                }
                self.pos = pos;
                if self.chunk_size == VARIABLE_CHUNK_SIZE || count < chunk_points {
                    chunk_points = (count as u64).min(remaining) as u32;
                }
            }
            Items::Pointwise(items) => {
                let mut off = 0;
                for d in items.iter_mut() {
                    let size = d.size();
                    d.init(&self.record[off..off + size]);
                    off += size;
                }
                self.dec = Decoder::new(pos, bytes.len());
                self.dec.init(bytes);
            }
        }
        self.items = Some(items);
        self.chunk_remaining = chunk_points.max(1);
        Ok(())
    }
}
