use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;
use neon_ygo_core::*;

#[derive(Debug, Error)]
pub enum CdbError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid SQLite3 .cdb file")]
    InvalidHeader,
    #[error("corrupt SQLite3 page {0}")]
    CorruptPage(u32),
    #[error("missing required table '{0}' in .cdb")]
    MissingTable(&'static str),
}

#[derive(Debug, Clone)]
enum SqlVal {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    #[allow(dead_code)]
    Blob(Vec<u8>),
}

impl SqlVal {
    fn as_i64(&self) -> i64 {
        match self {
            SqlVal::Int(i) => *i,
            SqlVal::Float(f) => *f as i64,
            SqlVal::Text(s) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }

    fn as_u32(&self) -> u32 {
        self.as_i64() as u32
    }

    fn as_u64(&self) -> u64 {
        self.as_i64() as u64
    }

    fn as_str(&self) -> String {
        match self {
            SqlVal::Text(s) => s.clone(),
            SqlVal::Int(i) => i.to_string(),
            _ => String::new(),
        }
    }
}

/// 100% pure-Rust SQLite3 `.cdb` card database reader (compatible with YGOPro / EDOPro / YGO Omega `cards.cdb`).
pub fn load_cdb_file(path: impl AsRef<Path>) -> Result<Vec<CardData>, CdbError> {
    let bytes = std::fs::read(path)?;
    parse_cdb_bytes(&bytes)
}

pub fn parse_cdb_bytes(data: &[u8]) -> Result<Vec<CardData>, CdbError> {
    if data.len() < 100 || &data[0..16] != b"SQLite format 3\0" {
        return Err(CdbError::InvalidHeader);
    }
    let raw_ps = u16::from_be_bytes([data[16], data[17]]) as usize;
    let page_size = if raw_ps == 1 { 65536 } else { raw_ps };
    if page_size < 512 || data.len() < page_size {
        return Err(CdbError::InvalidHeader);
    }
    let usable_size = page_size.saturating_sub(data[20] as usize);

    // 1. Read sqlite_master (page 1) to find rootpages of `datas` and `texts`
    let master_rows = read_table_rows(data, page_size, usable_size, 1)?;
    let mut datas_root: Option<u32> = None;
    let mut texts_root: Option<u32> = None;

    for (_rowid, cols) in master_rows {
        if cols.len() >= 4 {
            let obj_type = cols[0].as_str();
            let tbl_name = cols[2].as_str();
            let root_page = cols[3].as_u32();
            if obj_type == "table" {
                if tbl_name.eq_ignore_ascii_case("datas") {
                    datas_root = Some(root_page);
                } else if tbl_name.eq_ignore_ascii_case("texts") {
                    texts_root = Some(root_page);
                }
            }
        }
    }

    let datas_page = datas_root.ok_or(CdbError::MissingTable("datas"))?;
    let texts_page = texts_root.ok_or(CdbError::MissingTable("texts"))?;

    // 2. Scan `texts` table: (id, name, desc, str1..str16)
    let mut texts_map: BTreeMap<CardCode, (String, String, Vec<String>)> = BTreeMap::new();
    for (rowid, cols) in read_table_rows(data, page_size, usable_size, texts_page)? {
        let id = if matches!(cols.first(), Some(SqlVal::Null) | None) {
            rowid as CardCode
        } else {
            cols[0].as_u32()
        };
        let name = cols.get(1).map(|v| v.as_str()).unwrap_or_default();
        let desc = cols.get(2).map(|v| v.as_str()).unwrap_or_default();
        let mut strings = Vec::new();
        for c in cols.iter().skip(3).take(16) {
            strings.push(c.as_str());
        }
        texts_map.insert(id, (name, desc, strings));
    }

    // 3. Scan `datas` table: (id, ot, alias, setcode, type, atk, def, level, race, attribute, category)
    let mut cards = Vec::new();
    for (rowid, cols) in read_table_rows(data, page_size, usable_size, datas_page)? {
        if cols.len() < 11 {
            continue;
        }
        let code = if matches!(cols[0], SqlVal::Null) {
            rowid as CardCode
        } else {
            cols[0].as_u32()
        };
        let ot = cols[1].as_u32();
        let alias = cols[2].as_u32();
        let setcode_packed = cols[3].as_u64();
        let card_type = CardType::from_bits_truncate(cols[4].as_u32());
        let atk = cols[5].as_i64() as i32;
        let raw_def = cols[6].as_i64() as i32;
        let raw_level = cols[7].as_u32();
        let level = raw_level & 0xFF;
        let lscale = (raw_level >> 24) & 0xFF;
        let rscale = (raw_level >> 16) & 0xFF;
        let race = Race::from_bits_truncate(cols[8].as_u64());
        let attribute = Attribute::from_bits_truncate(cols[9].as_u32());
        let category = Category::from_bits_truncate(cols[10].as_u64());

        let (def, link_markers) = if card_type.contains(CardType::LINK) {
            (0, LinkMarker::from_bits_truncate(raw_def as u32))
        } else {
            (raw_def, LinkMarker::empty())
        };

        let (name, desc, strings) = texts_map
            .remove(&code)
            .unwrap_or_else(|| (format!("Card #{code}"), String::new(), Vec::new()));

        cards.push(CardData {
            code,
            alias,
            setcodes: CardData::unpack_setcode(setcode_packed),
            card_type,
            atk,
            def,
            level,
            race,
            attribute,
            lscale,
            rscale,
            link_markers,
            ot,
            category,
            name,
            desc,
            strings,
            maximum_atk: None,
            genesys_points: 0,
        });
    }

    Ok(cards)
}

fn page_slice(data: &[u8], page_size: usize, page_num: u32) -> Result<&[u8], CdbError> {
    if page_num == 0 {
        return Err(CdbError::CorruptPage(0));
    }
    let start = (page_num as usize - 1) * page_size;
    let end = start + page_size;
    data.get(start..end).ok_or(CdbError::CorruptPage(page_num))
}

fn read_table_rows(
    data: &[u8],
    page_size: usize,
    usable_size: usize,
    page_num: u32,
) -> Result<Vec<(i64, Vec<SqlVal>)>, CdbError> {
    let mut rows = Vec::new();
    walk_table_btree(data, page_size, usable_size, page_num, &mut rows, 0)?;
    Ok(rows)
}

fn walk_table_btree(
    data: &[u8],
    page_size: usize,
    usable_size: usize,
    page_num: u32,
    out: &mut Vec<(i64, Vec<SqlVal>)>,
    depth: usize,
) -> Result<(), CdbError> {
    if depth > 64 {
        return Err(CdbError::CorruptPage(page_num));
    }
    let page = page_slice(data, page_size, page_num)?;
    let hdr_off = if page_num == 1 { 100 } else { 0 };
    let page_type = page[hdr_off];
    let cell_count = u16::from_be_bytes([page[hdr_off + 3], page[hdr_off + 4]]) as usize;

    match page_type {
        // Table B-tree interior page
        0x05 => {
            let right_child = u32::from_be_bytes([
                page[hdr_off + 8],
                page[hdr_off + 9],
                page[hdr_off + 10],
                page[hdr_off + 11],
            ]);
            let ptr_array = hdr_off + 12;
            for i in 0..cell_count {
                let off = u16::from_be_bytes([page[ptr_array + i * 2], page[ptr_array + i * 2 + 1]]) as usize;
                if off + 4 > page.len() {
                    return Err(CdbError::CorruptPage(page_num));
                }
                let child = u32::from_be_bytes([page[off], page[off + 1], page[off + 2], page[off + 3]]);
                walk_table_btree(data, page_size, usable_size, child, out, depth + 1)?;
            }
            walk_table_btree(data, page_size, usable_size, right_child, out, depth + 1)?;
        }
        // Table B-tree leaf page
        0x0D => {
            let ptr_array = hdr_off + 8;
            let max_local = usable_size.saturating_sub(35);
            let min_local = ((usable_size.saturating_sub(12)) * 32 / 255).saturating_sub(23);

            for i in 0..cell_count {
                let off = u16::from_be_bytes([page[ptr_array + i * 2], page[ptr_array + i * 2 + 1]]) as usize;
                let (payload_len, n1) = read_varint(&page[off..]);
                let (rowid, n2) = read_varint(&page[off + n1..]);
                let p_off = off + n1 + n2;
                let p_len = payload_len as usize;

                let payload: Vec<u8> = if p_len <= max_local {
                    page.get(p_off..p_off + p_len)
                        .ok_or(CdbError::CorruptPage(page_num))?
                        .to_vec()
                } else {
                    let k = min_local + ((p_len - min_local) % (usable_size - 4));
                    let local_len = if k <= max_local { k } else { min_local };
                    let mut buf = Vec::with_capacity(p_len);
                    buf.extend_from_slice(
                        page.get(p_off..p_off + local_len)
                            .ok_or(CdbError::CorruptPage(page_num))?,
                    );
                    let ov_off = p_off + local_len;
                    let mut ov_page = u32::from_be_bytes([
                        page[ov_off],
                        page[ov_off + 1],
                        page[ov_off + 2],
                        page[ov_off + 3],
                    ]);
                    while buf.len() < p_len && ov_page != 0 {
                        let op = page_slice(data, page_size, ov_page)?;
                        ov_page = u32::from_be_bytes([op[0], op[1], op[2], op[3]]);
                        let take = (p_len - buf.len()).min(usable_size - 4);
                        buf.extend_from_slice(&op[4..4 + take]);
                    }
                    buf
                };

                let cols = decode_record(&payload);
                out.push((rowid as i64, cols));
            }
        }
        _ => return Err(CdbError::CorruptPage(page_num)),
    }
    Ok(())
}

fn read_varint(buf: &[u8]) -> (u64, usize) {
    let mut val = 0u64;
    for (i, &b) in buf.iter().take(9).enumerate() {
        if i == 8 {
            val = (val << 8) | (b as u64);
            return (val, 9);
        }
        val = (val << 7) | ((b & 0x7F) as u64);
        if b & 0x80 == 0 {
            return (val, i + 1);
        }
    }
    (val, buf.len().min(9))
}

fn decode_record(payload: &[u8]) -> Vec<SqlVal> {
    if payload.is_empty() {
        return Vec::new();
    }
    let (hdr_len, mut pos) = read_varint(payload);
    let hdr_end = (hdr_len as usize).min(payload.len());
    let mut serial_types = Vec::new();
    while pos < hdr_end {
        let (st, n) = read_varint(&payload[pos..]);
        serial_types.push(st);
        pos += n;
    }

    let mut data_pos = hdr_end;
    let mut cols = Vec::with_capacity(serial_types.len());
    for st in serial_types {
        match st {
            0 => cols.push(SqlVal::Null),
            1 => {
                let v = payload.get(data_pos).copied().unwrap_or(0) as i8 as i64;
                data_pos += 1;
                cols.push(SqlVal::Int(v));
            }
            2 => {
                let v = read_be_signed(payload, data_pos, 2);
                data_pos += 2;
                cols.push(SqlVal::Int(v));
            }
            3 => {
                let v = read_be_signed(payload, data_pos, 3);
                data_pos += 3;
                cols.push(SqlVal::Int(v));
            }
            4 => {
                let v = read_be_signed(payload, data_pos, 4);
                data_pos += 4;
                cols.push(SqlVal::Int(v));
            }
            5 => {
                let v = read_be_signed(payload, data_pos, 6);
                data_pos += 6;
                cols.push(SqlVal::Int(v));
            }
            6 => {
                let v = read_be_signed(payload, data_pos, 8);
                data_pos += 8;
                cols.push(SqlVal::Int(v));
            }
            7 => {
                let b = payload.get(data_pos..data_pos + 8).unwrap_or(&[0; 8]);
                let f = f64::from_be_bytes(b.try_into().unwrap_or([0; 8]));
                data_pos += 8;
                cols.push(SqlVal::Float(f));
            }
            8 => cols.push(SqlVal::Int(0)),
            9 => cols.push(SqlVal::Int(1)),
            n if n >= 12 && n % 2 == 0 => {
                let len = ((n - 12) / 2) as usize;
                let end = (data_pos + len).min(payload.len());
                cols.push(SqlVal::Blob(payload[data_pos..end].to_vec()));
                data_pos = end;
            }
            n if n >= 13 && n % 2 == 1 => {
                let len = ((n - 13) / 2) as usize;
                let end = (data_pos + len).min(payload.len());
                let s = String::from_utf8_lossy(&payload[data_pos..end]).into_owned();
                cols.push(SqlVal::Text(s));
                data_pos = end;
            }
            _ => cols.push(SqlVal::Null),
        }
    }
    cols
}

fn read_be_signed(buf: &[u8], off: usize, len: usize) -> i64 {
    let slice = buf.get(off..off + len).unwrap_or(&[]);
    if slice.is_empty() {
        return 0;
    }
    let neg = (slice[0] & 0x80) != 0;
    let mut acc: i64 = if neg { -1 } else { 0 };
    for &b in slice {
        acc = (acc << 8) | (b as i64);
    }
    acc
}
