use std::path::Path;

pub struct NpyF32 {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

pub struct NpyI64 {
    pub shape: Vec<usize>,
    pub data: Vec<i64>,
}

pub fn f32(path: &Path) -> NpyF32 {
    let (shape, descr, bytes) = decode(path);
    assert!(descr == "<f4" || descr == "|f4", "{descr} in {}", path.display());
    assert!(bytes.len() % 4 == 0, "{}", path.display());
    let data = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(data.len(), shape.iter().product::<usize>(), "{}", path.display());
    NpyF32 { shape, data }
}

pub fn i64(path: &Path) -> NpyI64 {
    let (shape, descr, bytes) = decode(path);
    assert_eq!(descr, "<i8", "{}", path.display());
    assert!(bytes.len() % 8 == 0, "{}", path.display());
    let data = bytes
        .chunks_exact(8)
        .map(|chunk| i64::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(data.len(), shape.iter().product::<usize>(), "{}", path.display());
    NpyI64 { shape, data }
}

fn decode(path: &Path) -> (Vec<usize>, String, Vec<u8>) {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert!(bytes.len() >= 10 && &bytes[..6] == b"\x93NUMPY", "{}", path.display());
    let major = bytes[6];
    let (header_len, header_at) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => (u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, 12),
        other => panic!("{}: npy version {other}", path.display()),
    };
    let header = std::str::from_utf8(&bytes[header_at..header_at + header_len])
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert!(
        !header.contains("fortran_order': True") && !header.contains("fortran_order\": True"),
        "{} is fortran-order",
        path.display()
    );
    let descr = quoted_after(header, "descr").unwrap_or_else(|| panic!("{}: no descr", path.display()));
    let shape = shape_of(header).unwrap_or_else(|| panic!("{}: no shape", path.display()));
    let data = bytes[header_at + header_len..].to_vec();
    (shape, descr, data)
}

fn quoted_after(header: &str, key: &str) -> Option<String> {
    let needle = format!("'{key}':");
    let at = header.find(&needle)? + needle.len();
    let rest = header[at..].trim_start();
    let rest = rest.strip_prefix('\'')?;
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

fn shape_of(header: &str) -> Option<Vec<usize>> {
    let at = header.find("'shape':")?;
    let open = header[at..].find('(')? + at;
    let close = header[open..].find(')')? + open;
    let inner = &header[open + 1..close];
    Some(
        inner
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| part.parse::<usize>().unwrap())
            .collect(),
    )
}
