//! Deterministic `.tar.gz` archive codec for `devmap cache` with fail-closed
//! decompression and extraction validation.
//!
//! Zero external compression/tar crates are added to the workspace lockfile:
//! - RFC 1952 gzip framing with IEEE 802.3 CRC-32 and ISIZE verification.
//! - RFC 1951 DEFLATE encoder (deterministic fixed-Huffman LZ77 + stored block
//!   fallback) and full RFC 1951 decoder (stored, fixed Huffman, and dynamic
//!   Huffman) with a hard output cap enforced on every byte to stop gzip bombs.
//! - Deterministic USTAR tar writer (sorted paths, zero mtime/uid/gid, mode 0644)
//!   and strict tar reader that fails closed on symlinks, hardlinks, special
//!   devices, PAX/GNU extended headers, absolute paths, `..` traversal,
//!   backslashes, duplicate paths, or oversized payloads.

use std::collections::BTreeSet;

use anyhow::{anyhow, bail, ensure, Result};

/// Hard ceiling (256 MiB) on compressed archive input, decompressed tar stream,
/// and total extracted payload bytes.
pub const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

/// Cache manifest format version written and accepted by this binary.
pub const CACHE_FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// CRC-32 (IEEE 802.3)
// ---------------------------------------------------------------------------

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// RFC 1951 DEFLATE / RFC 1952 GZIP
// ---------------------------------------------------------------------------

struct BitWriter {
    out: Vec<u8>,
    bit_buf: u64,
    bit_count: u32,
}

impl BitWriter {
    fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn write_bits(&mut self, value: u32, count: u32) {
        self.bit_buf |= u64::from(value) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    fn write_bits_msb(&mut self, value: u32, count: u32) {
        let mut rev = 0u32;
        for i in 0..count {
            if (value >> (count - 1 - i)) & 1 != 0 {
                rev |= 1 << i;
            }
        }
        self.write_bits(rev, count);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit_count > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
        self.out
    }
}

fn write_fixed_litlen(bw: &mut BitWriter, sym: u16) {
    match sym {
        0..=143 => bw.write_bits_msb(0b0011_0000 + u32::from(sym), 8),
        144..=255 => bw.write_bits_msb(0b1_1001_0000 + u32::from(sym - 144), 9),
        256..=279 => bw.write_bits_msb(u32::from(sym - 256), 7),
        280..=287 => bw.write_bits_msb(0b1100_0000 + u32::from(sym - 280), 8),
        _ => unreachable!("invalid litlen symbol {sym}"),
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn encode_length(length: usize) -> (u16, u32, u32) {
    debug_assert!((3..=258).contains(&length));
    if length == 258 {
        return (285, 0, 0);
    }
    let len = length as u16;
    for i in (0..28).rev() {
        if len >= LEN_BASE[i] {
            return (
                257 + i as u16,
                u32::from(len - LEN_BASE[i]),
                u32::from(LEN_EXTRA[i]),
            );
        }
    }
    unreachable!("length out of range: {length}");
}

fn encode_distance(dist: usize) -> (u16, u32, u32) {
    debug_assert!((1..=32768).contains(&dist));
    let d = dist as u16;
    for i in (0..DIST_BASE.len()).rev() {
        if d >= DIST_BASE[i] {
            return (
                i as u16,
                u32::from(d - DIST_BASE[i]),
                u32::from(DIST_EXTRA[i]),
            );
        }
    }
    unreachable!("distance out of range: {dist}");
}

fn deflate_fixed_lz77(input: &[u8]) -> Vec<u8> {
    const HASH_BITS: usize = 15;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    const WINDOW: usize = 32_768;
    const MAX_MATCH: usize = 258;
    const MAX_CHAIN: usize = 16;

    let mut bw = BitWriter::new(input.len() / 2 + 16);
    // BFINAL = 1, BTYPE = 01 (fixed Huffman)
    bw.write_bits(1, 1);
    bw.write_bits(1, 2);

    let mut head = vec![u32::MAX; HASH_SIZE];
    let mut prev = vec![u32::MAX; WINDOW];

    let hash3 = |slice: &[u8], pos: usize| -> usize {
        let v = (u32::from(slice[pos]) << 16)
            ^ (u32::from(slice[pos + 1]) << 8)
            ^ u32::from(slice[pos + 2]);
        ((v.wrapping_mul(0x1E35_A7BD)) >> (32 - HASH_BITS)) as usize
    };

    let mut pos = 0usize;
    let n = input.len();
    while pos < n {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;

        if pos + 3 <= n {
            let h = hash3(input, pos);
            let mut candidate = head[h];
            prev[pos & (WINDOW - 1)] = candidate;
            head[h] = pos as u32;

            let mut steps = 0usize;
            while candidate != u32::MAX && steps < MAX_CHAIN {
                let cpos = candidate as usize;
                let dist = pos - cpos;
                if dist == 0 || dist > WINDOW {
                    break;
                }
                let max_len = (n - pos).min(MAX_MATCH);
                if max_len >= 3
                    && input[cpos] == input[pos]
                    && input[cpos + 1] == input[pos + 1]
                    && input[cpos + 2] == input[pos + 2]
                {
                    let mut mlen = 3;
                    while mlen < max_len && input[cpos + mlen] == input[pos + mlen] {
                        mlen += 1;
                    }
                    if mlen > best_len {
                        best_len = mlen;
                        best_dist = dist;
                        if mlen == MAX_MATCH {
                            break;
                        }
                    }
                }
                let next_cand = prev[cpos & (WINDOW - 1)];
                if next_cand >= candidate {
                    break;
                }
                candidate = next_cand;
                steps += 1;
            }
        }

        if best_len >= 3 {
            let (lsym, lextra, lbits) = encode_length(best_len);
            write_fixed_litlen(&mut bw, lsym);
            if lbits > 0 {
                bw.write_bits(lextra, lbits);
            }
            let (dsym, dextra, dbits) = encode_distance(best_dist);
            bw.write_bits_msb(u32::from(dsym), 5);
            if dbits > 0 {
                bw.write_bits(dextra, dbits);
            }
            // Insert intermediate positions into hash chain for better ratios.
            let end = (pos + best_len).min(n.saturating_sub(2));
            for p in (pos + 1)..end {
                let h = hash3(input, p);
                prev[p & (WINDOW - 1)] = head[h];
                head[h] = p as u32;
            }
            pos += best_len;
        } else {
            write_fixed_litlen(&mut bw, u16::from(input[pos]));
            pos += 1;
        }
    }

    // End-of-block symbol (256)
    write_fixed_litlen(&mut bw, 256);
    bw.finish()
}

fn deflate_stored(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + (input.len() / 65535 + 1) * 5 + 5);
    if input.is_empty() {
        out.push(0x01); // BFINAL=1, BTYPE=00
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        return out;
    }
    let mut offset = 0;
    while offset < input.len() {
        let chunk_len = (input.len() - offset).min(65535);
        let is_final = offset + chunk_len == input.len();
        out.push(u8::from(is_final));
        let len16 = chunk_len as u16;
        out.extend_from_slice(&len16.to_le_bytes());
        out.extend_from_slice(&(!len16).to_le_bytes());
        out.extend_from_slice(&input[offset..offset + chunk_len]);
        offset += chunk_len;
    }
    out
}

/// Deterministic RFC 1952 gzip compression (mtime=0, OS=Unix, no filename).
pub fn gzip_compress(input: &[u8]) -> Vec<u8> {
    let fixed = deflate_fixed_lz77(input);
    let payload = if fixed.len() <= input.len() + (input.len() / 65535 + 1) * 5 {
        fixed
    } else {
        deflate_stored(input)
    };
    let mut gz = Vec::with_capacity(10 + payload.len() + 8);
    // RFC 1952 header: ID1=1f ID2=8b CM=08 FLG=00 MTIME=0 XFL=00 OS=03 (Unix)
    gz.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0x03]);
    gz.extend_from_slice(&payload);
    gz.extend_from_slice(&crc32(input).to_le_bytes());
    gz.extend_from_slice(&(input.len() as u32).to_le_bytes());
    gz
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buf: u64,
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn ensure_bits(&mut self, n: u32) -> Result<()> {
        while self.bit_count < n {
            if self.pos >= self.data.len() {
                bail!("unexpected end of deflate stream");
            }
            self.bit_buf |= u64::from(self.data[self.pos]) << self.bit_count;
            self.pos += 1;
            self.bit_count += 8;
        }
        Ok(())
    }

    fn read_bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        self.ensure_bits(n)?;
        let val = (self.bit_buf & ((1u64 << n) - 1)) as u32;
        self.bit_buf >>= n;
        self.bit_count -= n;
        Ok(val)
    }

    fn align_byte(&mut self) {
        let drop = self.bit_count % 8;
        self.bit_buf >>= drop;
        self.bit_count -= drop;
    }

    fn read_exact_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        self.align_byte();
        let mut out = Vec::with_capacity(len);
        while out.len() < len && self.bit_count >= 8 {
            out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
        let rem = len - out.len();
        if self.pos + rem > self.data.len() {
            bail!("unexpected end of stored deflate block");
        }
        out.extend_from_slice(&self.data[self.pos..self.pos + rem]);
        self.pos += rem;
        Ok(out)
    }

    fn byte_offset_after_align(&mut self) -> usize {
        self.align_byte();
        let buffered_bytes = (self.bit_count / 8) as usize;
        self.pos - buffered_bytes
    }
}

struct HuffmanTree {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl HuffmanTree {
    fn from_lengths(lengths: &[u8]) -> Result<Self> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if usize::from(len) >= counts.len() {
                bail!("invalid Huffman code length {len}");
            }
            if len > 0 {
                counts[usize::from(len)] += 1;
            }
        }
        let mut offsets = [0u16; 16];
        let mut total = 0u16;
        for bits in 1..16 {
            offsets[bits] = total;
            total = total
                .checked_add(counts[bits])
                .ok_or_else(|| anyhow!("Huffman symbol count overflow"))?;
        }
        // Check for oversubscribed tree (Kraft inequality)
        let mut left: i32 = 1;
        for &c in counts.iter().skip(1) {
            left <<= 1;
            left -= i32::from(c);
            if left < 0 {
                bail!("oversubscribed Huffman tree");
            }
        }
        let mut symbols = vec![0u16; usize::from(total)];
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let idx = usize::from(offsets[usize::from(len)]);
                symbols[idx] = sym as u16;
                offsets[usize::from(len)] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader<'_>) -> Result<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.read_bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        bail!("invalid Huffman code in deflate stream")
    }
}

fn fixed_trees() -> Result<(HuffmanTree, HuffmanTree)> {
    let mut litlen = [0u8; 288];
    litlen[0..=143].fill(8);
    litlen[144..=255].fill(9);
    litlen[256..=279].fill(7);
    litlen[280..=287].fill(8);
    let dist = [5u8; 32];
    Ok((
        HuffmanTree::from_lengths(&litlen)?,
        HuffmanTree::from_lengths(&dist)?,
    ))
}

fn read_dynamic_trees(br: &mut BitReader<'_>) -> Result<(HuffmanTree, HuffmanTree)> {
    const CL_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let hlit = br.read_bits(5)? as usize + 257;
    let hdist = br.read_bits(5)? as usize + 1;
    let hclen = br.read_bits(4)? as usize + 4;
    ensure!(hlit <= 286 && hdist <= 30, "invalid dynamic Huffman header");

    let mut cl_lengths = [0u8; 19];
    for &idx in CL_ORDER.iter().take(hclen) {
        cl_lengths[idx] = br.read_bits(3)? as u8;
    }
    let cl_tree = HuffmanTree::from_lengths(&cl_lengths)?;

    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_tree.decode(br)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let repeat = br.read_bits(2)? as usize + 3;
                let prev = *lengths
                    .last()
                    .ok_or_else(|| anyhow!("repeat code 16 with no prior length"))?;
                ensure!(
                    lengths.len() + repeat <= total,
                    "dynamic Huffman code lengths overflow"
                );
                lengths.resize(lengths.len() + repeat, prev);
            }
            17 => {
                let repeat = br.read_bits(3)? as usize + 3;
                ensure!(
                    lengths.len() + repeat <= total,
                    "dynamic Huffman code lengths overflow"
                );
                lengths.resize(lengths.len() + repeat, 0);
            }
            18 => {
                let repeat = br.read_bits(7)? as usize + 11;
                ensure!(
                    lengths.len() + repeat <= total,
                    "dynamic Huffman code lengths overflow"
                );
                lengths.resize(lengths.len() + repeat, 0);
            }
            _ => bail!("invalid code length symbol {sym}"),
        }
    }
    let lit_tree = HuffmanTree::from_lengths(&lengths[..hlit])?;
    let dist_tree = HuffmanTree::from_lengths(&lengths[hlit..])?;
    Ok((lit_tree, dist_tree))
}

fn decode_huffman_block(
    br: &mut BitReader<'_>,
    lit_tree: &HuffmanTree,
    dist_tree: &HuffmanTree,
    out: &mut Vec<u8>,
    max_out: usize,
) -> Result<()> {
    loop {
        let sym = lit_tree.decode(br)?;
        match sym {
            0..=255 => {
                if out.len() >= max_out {
                    bail!(
                        "decompressed cache archive exceeds maximum allowed size of {max_out} bytes"
                    );
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let lidx = usize::from(sym - 257);
                let length = usize::from(LEN_BASE[lidx])
                    + br.read_bits(u32::from(LEN_EXTRA[lidx]))? as usize;
                let dsym = dist_tree.decode(br)?;
                ensure!(
                    usize::from(dsym) < DIST_BASE.len(),
                    "invalid distance symbol {dsym}"
                );
                let didx = usize::from(dsym);
                let dist = usize::from(DIST_BASE[didx])
                    + br.read_bits(u32::from(DIST_EXTRA[didx]))? as usize;
                ensure!(
                    dist > 0 && dist <= out.len(),
                    "deflate back-reference out of range"
                );
                if out.len().saturating_add(length) > max_out {
                    bail!(
                        "decompressed cache archive exceeds maximum allowed size of {max_out} bytes"
                    );
                }
                for _ in 0..length {
                    let b = out[out.len() - dist];
                    out.push(b);
                }
            }
            _ => bail!("invalid literal/length symbol {sym}"),
        }
    }
}

/// Decompress an RFC 1952 gzip archive with a strict output size cap and
/// mandatory CRC-32 + ISIZE verification.
pub fn gzip_decompress_bounded(input: &[u8], max_out: usize) -> Result<Vec<u8>> {
    ensure!(
        input.len() <= max_out,
        "compressed cache archive ({} bytes) exceeds maximum allowed size ({max_out} bytes)",
        input.len()
    );
    ensure!(input.len() >= 18, "truncated gzip archive");
    ensure!(
        input[0] == 0x1f && input[1] == 0x8b && input[2] == 0x08,
        "invalid gzip header (expected RFC 1952 deflate)"
    );
    let flg = input[3];
    ensure!(flg & 0xE0 == 0, "reserved gzip header flags are set");
    let mut idx = 10usize;
    if flg & 0x04 != 0 {
        // FEXTRA
        ensure!(idx + 2 <= input.len(), "truncated gzip FEXTRA length");
        let xlen = u16::from_le_bytes([input[idx], input[idx + 1]]) as usize;
        idx += 2;
        ensure!(idx + xlen <= input.len(), "truncated gzip FEXTRA payload");
        idx += xlen;
    }
    if flg & 0x08 != 0 {
        // FNAME
        while idx < input.len() && input[idx] != 0 {
            idx += 1;
        }
        ensure!(idx < input.len(), "unterminated gzip FNAME");
        idx += 1;
    }
    if flg & 0x10 != 0 {
        // FCOMMENT
        while idx < input.len() && input[idx] != 0 {
            idx += 1;
        }
        ensure!(idx < input.len(), "unterminated gzip FCOMMENT");
        idx += 1;
    }
    if flg & 0x02 != 0 {
        // FHCRC
        ensure!(idx + 2 <= input.len(), "truncated gzip FHCRC");
        let expected_hcrc = u16::from_le_bytes([input[idx], input[idx + 1]]);
        let actual_hcrc = (crc32(&input[..idx]) & 0xFFFF) as u16;
        ensure!(expected_hcrc == actual_hcrc, "gzip header CRC-16 mismatch");
        idx += 2;
    }
    ensure!(idx + 8 <= input.len(), "truncated gzip deflate payload");

    let deflate_slice = &input[idx..input.len() - 8];
    let mut br = BitReader::new(deflate_slice);
    let mut out = Vec::new();
    let (fixed_lit, fixed_dist) = fixed_trees()?;

    loop {
        let bfinal = br.read_bits(1)?;
        let btype = br.read_bits(2)?;
        match btype {
            0 => {
                let hdr = br.read_exact_bytes(4)?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]);
                let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
                ensure!(len == !nlen, "corrupt stored deflate block length");
                let len = usize::from(len);
                if out.len().saturating_add(len) > max_out {
                    bail!(
                        "decompressed cache archive exceeds maximum allowed size of {max_out} bytes"
                    );
                }
                let block = br.read_exact_bytes(len)?;
                out.extend_from_slice(&block);
            }
            1 => {
                decode_huffman_block(&mut br, &fixed_lit, &fixed_dist, &mut out, max_out)?;
            }
            2 => {
                let (dyn_lit, dyn_dist) = read_dynamic_trees(&mut br)?;
                decode_huffman_block(&mut br, &dyn_lit, &dyn_dist, &mut out, max_out)?;
            }
            _ => bail!("invalid deflate block type {btype}"),
        }
        if bfinal == 1 {
            break;
        }
    }

    let consumed = br.byte_offset_after_align();
    ensure!(
        consumed == deflate_slice.len(),
        "unexpected trailing bytes in gzip archive ({} trailing)",
        deflate_slice.len().saturating_sub(consumed)
    );

    let trailer = &input[input.len() - 8..];
    let expected_crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let expected_isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    let actual_crc = crc32(&out);
    let actual_isize = out.len() as u32;
    ensure!(
        actual_crc == expected_crc,
        "gzip CRC-32 mismatch (expected {expected_crc:#010x}, got {actual_crc:#010x})"
    );
    ensure!(
        actual_isize == expected_isize,
        "gzip ISIZE mismatch (expected {expected_isize}, got {actual_isize})"
    );
    Ok(out)
}

// ---------------------------------------------------------------------------
// Deterministic USTAR Tar
// ---------------------------------------------------------------------------

/// Validate a relative archive entry path, rejecting absolute paths, `..`
/// traversal, backslashes, NUL/control bytes, and Windows drive prefixes.
pub fn validate_archive_entry_path(raw: &str) -> Result<()> {
    if raw.is_empty() {
        bail!("refusing empty path in cache archive");
    }
    if raw.starts_with('/') {
        bail!("refusing absolute path in cache archive: {raw:?}");
    }
    if raw.contains('\\') {
        bail!("refusing backslash path separator in cache archive: {raw:?}");
    }
    if raw.bytes().any(|b| b == 0 || b < 0x20 || b == 0x7f) {
        bail!("refusing control character in cache archive path: {raw:?}");
    }
    for (idx, seg) in raw.split('/').enumerate() {
        if seg.is_empty() {
            bail!("refusing empty path segment in cache archive: {raw:?}");
        }
        if seg == "." || seg == ".." {
            bail!("refusing path traversal segment in cache archive: {raw:?}");
        }
        if idx == 0 && seg.contains(':') {
            bail!("refusing drive-prefixed path in cache archive: {raw:?}");
        }
    }
    Ok(())
}

fn split_ustar_name(path: &str) -> Result<(&str, &str)> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    // Find a '/' split point where prefix <= 155 and name <= 100.
    for (idx, ch) in path.char_indices() {
        if ch == '/' {
            let prefix = &path[..idx];
            let name = &path[idx + 1..];
            if !prefix.is_empty() && !name.is_empty() && prefix.len() <= 155 && name.len() <= 100 {
                return Ok((prefix, name));
            }
        }
    }
    bail!("archive entry path exceeds USTAR length limit: {path:?}")
}

fn write_octal(field: &mut [u8], value: u64) -> Result<()> {
    let width = field.len().saturating_sub(1);
    let formatted = format!("{value:0width$o}");
    ensure!(
        formatted.len() <= width,
        "tar octal field overflow for value {value}"
    );
    field[..width].copy_from_slice(formatted.as_bytes());
    field[width] = 0;
    Ok(())
}

/// Build a deterministic USTAR tar stream from `(path, bytes)` entries.
pub fn write_tar(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&(String, Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (path, data) in sorted {
        validate_archive_entry_path(path)?;
        ensure!(
            seen.insert(path.as_str()),
            "duplicate entry path in cache archive: {path:?}"
        );
        let (prefix, name) = split_ustar_name(path)?;
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_octal(&mut header[100..108], 0o644)?; // mode
        write_octal(&mut header[108..116], 0)?; // uid
        write_octal(&mut header[116..124], 0)?; // gid
        write_octal(&mut header[124..136], data.len() as u64)?; // size
        write_octal(&mut header[136..148], 0)?; // mtime (deterministic)
        header[148..156].fill(b' '); // checksum placeholder
        header[156] = b'0'; // regular file
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        if !prefix.is_empty() {
            header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        }
        let cksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let cksum_str = format!("{cksum:06o}\0 ");
        header[148..156].copy_from_slice(cksum_str.as_bytes());

        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        let rem = data.len() % 512;
        if rem != 0 {
            out.resize(out.len() + (512 - rem), 0);
        }
    }
    // Two 512-byte zero blocks terminate the tar archive.
    out.resize(out.len() + 1024, 0);
    Ok(out)
}

fn parse_octal_field(field: &[u8]) -> Result<u64> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let text = std::str::from_utf8(&field[..end])
        .map_err(|_| anyhow!("non-UTF-8 tar numeric field"))?
        .trim();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|e| anyhow!("invalid tar octal field {text:?}: {e}"))
}

fn read_cstr(field: &[u8]) -> Result<&str> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end]).map_err(|_| anyhow!("non-UTF-8 tar header string"))
}

/// Strictly parse a tar archive, failing closed on symlinks, hardlinks,
/// devices, extended headers, path traversal, absolute paths, duplicates, or
/// total extracted bytes exceeding `max_total_bytes`.
pub fn read_tar_strict(tar_bytes: &[u8], max_total_bytes: usize) -> Result<Vec<(String, Vec<u8>)>> {
    ensure!(
        tar_bytes.len() <= max_total_bytes,
        "tar stream ({} bytes) exceeds maximum allowed size ({max_total_bytes} bytes)",
        tar_bytes.len()
    );
    ensure!(
        tar_bytes.len().is_multiple_of(512),
        "tar archive length ({}) is not a multiple of 512 bytes",
        tar_bytes.len()
    );

    let mut pos = 0usize;
    let mut total_payload = 0usize;
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();

    while pos + 512 <= tar_bytes.len() {
        let header = &tar_bytes[pos..pos + 512];
        pos += 512;
        if header.iter().all(|&b| b == 0) {
            // End-of-archive marker: verify every remaining byte is zero so no
            // smuggled payload can hide past the EOF zero block.
            ensure!(
                tar_bytes[pos..].iter().all(|&b| b == 0),
                "refusing non-zero trailing data after tar end-of-archive marker"
            );
            break;
        }

        // Verify USTAR/V7 checksum.
        let expected_cksum = parse_octal_field(&header[148..156])?;
        let mut computed_cksum: u64 = 0;
        for (i, &b) in header.iter().enumerate() {
            computed_cksum += if (148..156).contains(&i) {
                u64::from(b' ')
            } else {
                u64::from(b)
            };
        }
        ensure!(
            computed_cksum == expected_cksum,
            "tar header checksum mismatch (expected {expected_cksum:#o}, got {computed_cksum:#o})"
        );

        let name = read_cstr(&header[0..100])?;
        let prefix = read_cstr(&header[345..500])?;
        let full_path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let typeflag = header[156];
        let linkname = read_cstr(&header[157..257])?;
        let size = parse_octal_field(&header[124..136])? as usize;

        match typeflag {
            b'2' => bail!("refusing symlink entry in cache archive: {full_path:?}"),
            b'1' => bail!("refusing hardlink entry in cache archive: {full_path:?}"),
            b'3' | b'4' | b'6' => {
                bail!("refusing special device/fifo entry in cache archive: {full_path:?}")
            }
            b'x' | b'g' | b'L' | b'K' => {
                bail!(
                    "refusing extended tar header (typeflag {:?}) in cache archive",
                    typeflag as char
                )
            }
            b'5' => {
                let trimmed = full_path.trim_end_matches('/');
                validate_archive_entry_path(trimmed)?;
                ensure!(size == 0, "directory entry {full_path:?} has non-zero size");
                continue;
            }
            b'0' | 0 => {}
            other => bail!("refusing unsupported tar entry typeflag {other:#x} for {full_path:?}"),
        }

        ensure!(
            linkname.is_empty(),
            "refusing non-empty linkname on regular file entry {full_path:?}"
        );
        validate_archive_entry_path(&full_path)?;
        ensure!(
            seen.insert(full_path.clone()),
            "refusing duplicate entry in cache archive: {full_path:?}"
        );

        total_payload = total_payload
            .checked_add(size)
            .ok_or_else(|| anyhow!("archive payload size overflow"))?;
        ensure!(
            total_payload <= max_total_bytes,
            "extracted cache payload exceeds maximum allowed size of {max_total_bytes} bytes"
        );
        let padded = size
            .checked_add((512 - (size % 512)) % 512)
            .ok_or_else(|| anyhow!("tar entry padding overflow"))?;
        ensure!(
            pos.checked_add(padded)
                .is_some_and(|end| end <= tar_bytes.len()),
            "truncated tar entry payload for {full_path:?}"
        );
        let data = tar_bytes[pos..pos + size].to_vec();
        pos += padded;
        entries.push((full_path, data));
    }

    Ok(entries)
}

/// Pack deterministic `(path, bytes)` entries into a deterministic `.tar.gz`.
pub fn pack_tar_gz(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let tar = write_tar(entries)?;
    ensure!(
        tar.len() <= MAX_ARCHIVE_BYTES,
        "packed tar archive ({} bytes) exceeds maximum allowed size ({MAX_ARCHIVE_BYTES} bytes)",
        tar.len()
    );
    Ok(gzip_compress(&tar))
}

/// Unpack a `.tar.gz` with strict bounds and security validation.
pub fn unpack_tar_gz_strict(gz_bytes: &[u8], max_bytes: usize) -> Result<Vec<(String, Vec<u8>)>> {
    let tar = gzip_decompress_bounded(gz_bytes, max_bytes)?;
    read_tar_strict(&tar, max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_tar_entry(name: &str, typeflag: u8, linkname: &str, data: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_octal(&mut header[100..108], 0o644).unwrap();
        write_octal(&mut header[108..116], 0).unwrap();
        write_octal(&mut header[116..124], 0).unwrap();
        write_octal(&mut header[124..136], data.len() as u64).unwrap();
        write_octal(&mut header[136..148], 0).unwrap();
        header[148..156].fill(b' ');
        header[156] = typeflag;
        header[157..157 + linkname.len()].copy_from_slice(linkname.as_bytes());
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let cksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let cksum_str = format!("{cksum:06o}\0 ");
        header[148..156].copy_from_slice(cksum_str.as_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        let rem = data.len() % 512;
        if rem != 0 {
            out.resize(out.len() + (512 - rem), 0);
        }
        out.resize(out.len() + 1024, 0);
        out
    }

    #[test]
    fn gzip_roundtrip_is_deterministic_and_compresses() {
        let data = b"devmap cache test payload repeated ".repeat(200);
        let gz1 = gzip_compress(&data);
        let gz2 = gzip_compress(&data);
        assert_eq!(gz1, gz2, "gzip output must be deterministic");
        assert!(gz1.len() < data.len() / 2, "expected LZ77 compression");
        let decoded = gzip_decompress_bounded(&gz1, MAX_ARCHIVE_BYTES).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn gzip_rejects_corrupt_crc_and_oversized_output() {
        let data = vec![b'A'; 4096];
        let mut gz = gzip_compress(&data);
        // Reject gzip bomb / oversized output
        let err = gzip_decompress_bounded(&gz, 1024).unwrap_err();
        assert!(err.to_string().contains("exceeds maximum"), "{err}");
        // Corrupt CRC-32 byte
        let len = gz.len();
        gz[len - 5] ^= 0xFF;
        let err = gzip_decompress_bounded(&gz, MAX_ARCHIVE_BYTES).unwrap_err();
        assert!(err.to_string().contains("CRC-32 mismatch"), "{err}");
    }

    #[test]
    fn tar_rejects_symlink_hardlink_traversal_and_absolute_paths() {
        let symlink_tar = raw_tar_entry("evil_link", b'2', "/etc/passwd", b"");
        let err = read_tar_strict(&symlink_tar, MAX_ARCHIVE_BYTES).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");

        let hardlink_tar = raw_tar_entry("evil_hard", b'1', "devmap.sqlite", b"");
        let err = read_tar_strict(&hardlink_tar, MAX_ARCHIVE_BYTES).unwrap_err();
        assert!(err.to_string().contains("hardlink"), "{err}");

        let traversal_tar = raw_tar_entry("../escape.txt", b'0', "", b"pwn");
        let err = read_tar_strict(&traversal_tar, MAX_ARCHIVE_BYTES).unwrap_err();
        assert!(err.to_string().contains("traversal"), "{err}");

        let nested_traversal = raw_tar_entry("a/../../escape.txt", b'0', "", b"pwn");
        let err = read_tar_strict(&nested_traversal, MAX_ARCHIVE_BYTES).unwrap_err();
        assert!(err.to_string().contains("traversal"), "{err}");

        let absolute_tar = raw_tar_entry("/etc/passwd", b'0', "", b"pwn");
        let err = read_tar_strict(&absolute_tar, MAX_ARCHIVE_BYTES).unwrap_err();
        assert!(err.to_string().contains("absolute"), "{err}");
    }

    #[test]
    fn tar_gz_roundtrip_sorts_deterministically() {
        let a = vec![
            ("b.txt".to_string(), b"beta".to_vec()),
            ("a.txt".to_string(), b"alpha".to_vec()),
        ];
        let b = vec![
            ("a.txt".to_string(), b"alpha".to_vec()),
            ("b.txt".to_string(), b"beta".to_vec()),
        ];
        let gz_a = pack_tar_gz(&a).unwrap();
        let gz_b = pack_tar_gz(&b).unwrap();
        assert_eq!(gz_a, gz_b, "entry ordering must not change archive bytes");
        let entries = unpack_tar_gz_strict(&gz_a, MAX_ARCHIVE_BYTES).unwrap();
        assert_eq!(entries, b);
    }
}
