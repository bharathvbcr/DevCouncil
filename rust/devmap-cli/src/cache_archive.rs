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
use std::sync::atomic::{compiler_fence, Ordering};

use anyhow::{anyhow, bail, ensure, Result};
use sha2::{Digest, Sha256};

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

// ---------------------------------------------------------------------------
// Zeroization, CSPRNG, RFC 5869 HKDF-SHA256, RFC 8439 ChaCha20-Poly1305, DMC1
// ---------------------------------------------------------------------------

/// Magic header for authenticated encrypted DevMap cache containers (`DMC1`).
pub const DMC1_MAGIC: &[u8; 4] = b"DMC1";
/// Container format version byte.
pub const DMC1_VERSION: u8 = 1;
/// 12-byte ChaCha20-Poly1305 nonce length.
pub const DMC1_NONCE_LEN: usize = 12;
/// 16-byte Poly1305 authentication tag length.
pub const DMC1_TAG_LEN: usize = 16;
/// Fixed header length: `b"DMC1"` (4) || `version` (1) || `nonce` (12).
pub const DMC1_HEADER_LEN: usize = 4 + 1 + DMC1_NONCE_LEN;
/// Domain-separation prefix for AEAD AAD and HKDF `info`.
pub const DMC1_DOMAIN_PREFIX: &[u8] = b"devmap-cache/v1\0";
/// HKDF-SHA256 salt for deriving the per-repo cache AEAD subkey.
const DMC1_HKDF_SALT: &[u8] = b"devmap-cache-hkdf-v1";

/// Wipe a byte slice in pure safe Rust without `unsafe`, using `black_box` +
/// `SeqCst` compiler fence so the optimizer cannot elide the zeroing stores.
pub(crate) fn zeroize_bytes(buf: &mut [u8]) {
    buf.fill(0);
    let _ = std::hint::black_box(&*buf);
    compiler_fence(Ordering::SeqCst);
}

fn zeroize_u32s(buf: &mut [u32]) {
    buf.fill(0);
    let _ = std::hint::black_box(&*buf);
    compiler_fence(Ordering::SeqCst);
}

/// 32-byte secret key buffer zeroized automatically on drop.
pub struct ZeroizingKey32([u8; 32]);

impl ZeroizingKey32 {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for ZeroizingKey32 {
    fn drop(&mut self) {
        zeroize_bytes(&mut self.0);
    }
}

/// Variable-length secret buffer zeroized automatically on drop.
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        zeroize_bytes(&mut self.0);
    }
}

/// Fill `out` from the OS CSPRNG via `getrandom` (cross-platform, zero `unsafe`).
fn csprng_fill(out: &mut [u8]) -> Result<()> {
    getrandom::fill(out).map_err(|err| anyhow!("OS CSPRNG failed for cache nonce: {err}"))
}

/// Constant-time byte slice equality.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    let _ = std::hint::black_box(diff);
    diff == 0
}

/// Pure-Rust RFC 2104 HMAC-SHA256 returning raw 32-byte tag.
pub(crate) fn hmac_sha256_raw(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let mut digest = Sha256::digest(key);
        k[..32].copy_from_slice(&digest);
        zeroize_bytes(&mut digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut i_key_pad = [0x36u8; BLOCK];
    let mut o_key_pad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        i_key_pad[i] ^= k[i];
        o_key_pad[i] ^= k[i];
    }
    zeroize_bytes(&mut k);

    let mut inner = Sha256::new();
    inner.update(i_key_pad);
    inner.update(data);
    let mut inner_hash = inner.finalize();
    zeroize_bytes(&mut i_key_pad);

    let mut outer = Sha256::new();
    outer.update(o_key_pad);
    outer.update(inner_hash);
    zeroize_bytes(&mut o_key_pad);
    zeroize_bytes(&mut inner_hash);

    let mut out = [0u8; 32];
    let mut finalized = outer.finalize();
    out.copy_from_slice(&finalized);
    zeroize_bytes(&mut finalized);
    out
}

/// Pure-Rust RFC 5869 HKDF-Extract using SHA-256.
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    if salt.is_empty() {
        let zeros = [0u8; 32];
        hmac_sha256_raw(&zeros, ikm)
    } else {
        hmac_sha256_raw(salt, ikm)
    }
}

/// Pure-Rust RFC 5869 HKDF-Expand using SHA-256.
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], okm: &mut [u8]) -> Result<()> {
    const HASH_LEN: usize = 32;
    ensure!(
        okm.len() <= 255 * HASH_LEN,
        "HKDF output length {} exceeds 255 * {HASH_LEN}",
        okm.len()
    );
    if okm.is_empty() {
        return Ok(());
    }

    let mut t_prev = [0u8; HASH_LEN];
    let mut t_len = 0usize;
    let mut offset = 0usize;
    let mut counter: u8 = 1;
    let mut input_buf = Vec::with_capacity(HASH_LEN + info.len() + 1);

    while offset < okm.len() {
        input_buf.clear();
        input_buf.extend_from_slice(&t_prev[..t_len]);
        input_buf.extend_from_slice(info);
        input_buf.push(counter);

        t_prev = hmac_sha256_raw(prk, &input_buf);
        t_len = HASH_LEN;
        let take = (okm.len() - offset).min(HASH_LEN);
        okm[offset..offset + take].copy_from_slice(&t_prev[..take]);
        offset += take;
        counter = counter.wrapping_add(1);
    }

    zeroize_bytes(&mut t_prev);
    zeroize_bytes(&mut input_buf);
    Ok(())
}

/// Pure-Rust RFC 5869 HKDF-SHA256 (Extract + Expand) with intermediate zeroization.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], okm: &mut [u8]) -> Result<()> {
    let mut prk = hkdf_extract(salt, ikm);
    let res = hkdf_expand(&prk, info, okm);
    zeroize_bytes(&mut prk);
    res
}

#[inline]
fn chacha_quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

#[inline]
fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Pure-Rust RFC 8439 Section 2.3 ChaCha20 block function.
pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0] = 0x6170_7865;
    state[1] = 0x3320_646e;
    state[2] = 0x7962_2d32;
    state[3] = 0x6b20_6574;
    for i in 0..8 {
        state[4 + i] = read_u32_le(&key[i * 4..i * 4 + 4]);
    }
    state[12] = counter;
    state[13] = read_u32_le(&nonce[0..4]);
    state[14] = read_u32_le(&nonce[4..8]);
    state[15] = read_u32_le(&nonce[8..12]);

    let mut working = state;
    for _ in 0..10 {
        chacha_quarter_round(&mut working, 0, 4, 8, 12);
        chacha_quarter_round(&mut working, 1, 5, 9, 13);
        chacha_quarter_round(&mut working, 2, 6, 10, 14);
        chacha_quarter_round(&mut working, 3, 7, 11, 15);
        chacha_quarter_round(&mut working, 0, 5, 10, 15);
        chacha_quarter_round(&mut working, 1, 6, 11, 12);
        chacha_quarter_round(&mut working, 2, 7, 8, 13);
        chacha_quarter_round(&mut working, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = working[i].wrapping_add(state[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    zeroize_u32s(&mut working);
    zeroize_u32s(&mut state);
    out
}

/// Pure-Rust RFC 8439 Section 2.4 ChaCha20 XOR stream cipher (in-place).
pub fn chacha20_xor_in_place(
    key: &[u8; 32],
    initial_counter: u32,
    nonce: &[u8; 12],
    data: &mut [u8],
) -> Result<()> {
    let mut counter = initial_counter;
    let mut first = true;
    for chunk in data.chunks_mut(64) {
        if !first {
            counter = counter
                .checked_add(1)
                .ok_or_else(|| anyhow!("ChaCha20 block counter overflow"))?;
        }
        first = false;
        let mut keystream = chacha20_block(key, counter, nonce);
        for (dst, &ks) in chunk.iter_mut().zip(keystream.iter()) {
            *dst ^= ks;
        }
        zeroize_bytes(&mut keystream);
    }
    Ok(())
}

/// Pure-Rust RFC 8439 Section 2.5 Poly1305 one-time MAC using 5x26-bit limbs.
pub fn poly1305_mac(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    let mut r_bytes = [0u8; 16];
    r_bytes.copy_from_slice(&key[0..16]);
    r_bytes[3] &= 15;
    r_bytes[7] &= 15;
    r_bytes[11] &= 15;
    r_bytes[15] &= 15;
    r_bytes[4] &= 252;
    r_bytes[8] &= 252;
    r_bytes[12] &= 252;

    let r_u128 = u128::from_le_bytes(r_bytes);
    zeroize_bytes(&mut r_bytes);
    let r0 = (r_u128 & 0x03ff_ffff) as u64;
    let r1 = ((r_u128 >> 26) & 0x03ff_ffff) as u64;
    let r2 = ((r_u128 >> 52) & 0x03ff_ffff) as u64;
    let r3 = ((r_u128 >> 78) & 0x03ff_ffff) as u64;
    let r4 = ((r_u128 >> 104) & 0x03ff_ffff) as u64;

    let s1 = r1 * 5;
    let s2 = r2 * 5;
    let s3 = r3 * 5;
    let s4 = r4 * 5;

    let mut h0: u64 = 0;
    let mut h1: u64 = 0;
    let mut h2: u64 = 0;
    let mut h3: u64 = 0;
    let mut h4: u64 = 0;

    let mut block = [0u8; 17];
    for chunk in msg.chunks(16) {
        block.fill(0);
        block[..chunk.len()].copy_from_slice(chunk);
        block[chunk.len()] = 1;

        let lo = u128::from_le_bytes([
            block[0], block[1], block[2], block[3], block[4], block[5], block[6], block[7],
            block[8], block[9], block[10], block[11], block[12], block[13], block[14], block[15],
        ]);
        let hi = u128::from(block[16]);

        h0 += (lo & 0x03ff_ffff) as u64;
        h1 += ((lo >> 26) & 0x03ff_ffff) as u64;
        h2 += ((lo >> 52) & 0x03ff_ffff) as u64;
        h3 += ((lo >> 78) & 0x03ff_ffff) as u64;
        h4 += (((lo >> 104) | (hi << 24)) & 0x03ff_ffff) as u64;

        let d0 = u128::from(h0) * u128::from(r0)
            + u128::from(h1) * u128::from(s4)
            + u128::from(h2) * u128::from(s3)
            + u128::from(h3) * u128::from(s2)
            + u128::from(h4) * u128::from(s1);
        let mut d1 = u128::from(h0) * u128::from(r1)
            + u128::from(h1) * u128::from(r0)
            + u128::from(h2) * u128::from(s4)
            + u128::from(h3) * u128::from(s3)
            + u128::from(h4) * u128::from(s2);
        let mut d2 = u128::from(h0) * u128::from(r2)
            + u128::from(h1) * u128::from(r1)
            + u128::from(h2) * u128::from(r0)
            + u128::from(h3) * u128::from(s4)
            + u128::from(h4) * u128::from(s3);
        let mut d3 = u128::from(h0) * u128::from(r3)
            + u128::from(h1) * u128::from(r2)
            + u128::from(h2) * u128::from(r1)
            + u128::from(h3) * u128::from(r0)
            + u128::from(h4) * u128::from(s4);
        let mut d4 = u128::from(h0) * u128::from(r4)
            + u128::from(h1) * u128::from(r3)
            + u128::from(h2) * u128::from(r2)
            + u128::from(h3) * u128::from(r1)
            + u128::from(h4) * u128::from(r0);

        let mut c = d0 >> 26;
        h0 = (d0 & 0x03ff_ffff) as u64;
        d1 += c;
        c = d1 >> 26;
        h1 = (d1 & 0x03ff_ffff) as u64;
        d2 += c;
        c = d2 >> 26;
        h2 = (d2 & 0x03ff_ffff) as u64;
        d3 += c;
        c = d3 >> 26;
        h3 = (d3 & 0x03ff_ffff) as u64;
        d4 += c;
        c = d4 >> 26;
        h4 = (d4 & 0x03ff_ffff) as u64;
        h0 += (c as u64) * 5;
        let c0 = h0 >> 26;
        h0 &= 0x03ff_ffff;
        h1 += c0;
    }
    zeroize_bytes(&mut block);

    let mut c = h1 >> 26;
    h1 &= 0x03ff_ffff;
    h2 += c;
    c = h2 >> 26;
    h2 &= 0x03ff_ffff;
    h3 += c;
    c = h3 >> 26;
    h3 &= 0x03ff_ffff;
    h4 += c;
    c = h4 >> 26;
    h4 &= 0x03ff_ffff;
    h0 += c * 5;
    c = h0 >> 26;
    h0 &= 0x03ff_ffff;
    h1 += c;

    let mut g0 = h0 + 5;
    c = g0 >> 26;
    g0 &= 0x03ff_ffff;
    let mut g1 = h1 + c;
    c = g1 >> 26;
    g1 &= 0x03ff_ffff;
    let mut g2 = h2 + c;
    c = g2 >> 26;
    g2 &= 0x03ff_ffff;
    let mut g3 = h3 + c;
    c = g3 >> 26;
    g3 &= 0x03ff_ffff;
    let g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);

    let mask = (g4 >> 63).wrapping_sub(1);
    h0 = (h0 & !mask) | (g0 & mask);
    h1 = (h1 & !mask) | (g1 & mask);
    h2 = (h2 & !mask) | (g2 & mask);
    h3 = (h3 & !mask) | (g3 & mask);
    h4 = (h4 & !mask) | (g4 & mask);

    let f = u128::from(h0)
        | (u128::from(h1) << 26)
        | (u128::from(h2) << 52)
        | (u128::from(h3) << 78)
        | (u128::from(h4) << 104);
    let s = u128::from_le_bytes([
        key[16], key[17], key[18], key[19], key[20], key[21], key[22], key[23], key[24], key[25],
        key[26], key[27], key[28], key[29], key[30], key[31],
    ]);
    f.wrapping_add(s).to_le_bytes()
}

fn poly1305_aead_tag(otk: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let aad_pad = (16 - (aad.len() % 16)) % 16;
    let ct_pad = (16 - (ciphertext.len() % 16)) % 16;
    let total_len = aad.len() + aad_pad + ciphertext.len() + ct_pad + 16;
    let mut mac_data = Vec::with_capacity(total_len);
    mac_data.extend_from_slice(aad);
    mac_data.resize(mac_data.len() + aad_pad, 0);
    mac_data.extend_from_slice(ciphertext);
    mac_data.resize(mac_data.len() + ct_pad, 0);
    mac_data.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    mac_data.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    poly1305_mac(otk, &mac_data)
}

fn poly1305_key_gen(key: &[u8; 32], nonce: &[u8; 12]) -> [u8; 32] {
    let mut block0 = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);
    zeroize_bytes(&mut block0);
    otk
}

/// Pure-Rust RFC 8439 Section 2.8 ChaCha20-Poly1305 AEAD encryption.
/// Returns `ciphertext || 16-byte tag`.
pub fn chacha20_poly1305_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let mut otk = poly1305_key_gen(key, nonce);
    let mut out = Vec::with_capacity(plaintext.len() + DMC1_TAG_LEN);
    out.extend_from_slice(plaintext);
    chacha20_xor_in_place(key, 1, nonce, &mut out)?;
    let tag = poly1305_aead_tag(&otk, aad, &out);
    zeroize_bytes(&mut otk);
    out.extend_from_slice(&tag);
    Ok(out)
}

/// Pure-Rust RFC 8439 Section 2.8 ChaCha20-Poly1305 AEAD decryption.
/// Verifies the 16-byte Poly1305 tag in constant time before decrypting.
pub fn chacha20_poly1305_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext_and_tag: &[u8],
) -> Result<Vec<u8>> {
    ensure!(
        ciphertext_and_tag.len() >= DMC1_TAG_LEN,
        "ChaCha20-Poly1305 ciphertext truncated (< {DMC1_TAG_LEN} bytes)"
    );
    let split = ciphertext_and_tag.len() - DMC1_TAG_LEN;
    let ciphertext = &ciphertext_and_tag[..split];
    let tag = &ciphertext_and_tag[split..];

    let mut otk = poly1305_key_gen(key, nonce);
    let expected_tag = poly1305_aead_tag(&otk, aad, ciphertext);
    zeroize_bytes(&mut otk);

    ensure!(
        constant_time_eq(&expected_tag, tag),
        "ChaCha20-Poly1305 authentication failed (wrong key, wrong repo_root_hash AAD, or tampered archive)"
    );

    let mut plaintext = ciphertext.to_vec();
    chacha20_xor_in_place(key, 1, nonce, &mut plaintext)?;
    Ok(plaintext)
}

/// Returns `true` when `bytes` begins with the `DMC1` container magic.
pub fn is_dmc1_container(bytes: &[u8]) -> bool {
    bytes.starts_with(DMC1_MAGIC)
}

/// Build the domain-separated AEAD AAD / HKDF info: `b"devmap-cache/v1\0" || repo_root_hash.as_bytes()`.
pub fn build_cache_aad(repo_root_hash: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(DMC1_DOMAIN_PREFIX.len() + repo_root_hash.len());
    aad.extend_from_slice(DMC1_DOMAIN_PREFIX);
    aad.extend_from_slice(repo_root_hash.as_bytes());
    aad
}

/// Derive the 32-byte ChaCha20-Poly1305 subkey via RFC 5869 HKDF-SHA256.
pub fn derive_cache_subkey(key_material: &[u8], repo_root_hash: &str) -> Result<ZeroizingKey32> {
    ensure!(
        !key_material.is_empty(),
        "cache encryption key material must not be empty"
    );
    ensure!(
        !repo_root_hash.is_empty(),
        "repo_root_hash must not be empty for cache key derivation"
    );
    let info = build_cache_aad(repo_root_hash);
    let mut subkey = [0u8; 32];
    hkdf_sha256(DMC1_HKDF_SALT, key_material, &info, &mut subkey)?;
    Ok(ZeroizingKey32(subkey))
}

/// Encrypt `.tar.gz` bytes into an authenticated `DMC1` container:
/// `b"DMC1"` (4 B) || `1u8` (1 B) || 12-byte CSPRNG nonce || ChaCha20-Poly1305 ciphertext+tag.
pub fn encrypt_cache_archive(
    gz_bytes: &[u8],
    key_material: &[u8],
    repo_root_hash: &str,
) -> Result<Vec<u8>> {
    let total_len = gz_bytes
        .len()
        .checked_add(DMC1_HEADER_LEN + DMC1_TAG_LEN)
        .ok_or_else(|| anyhow!("encrypted cache archive size overflow"))?;
    ensure!(
        total_len <= MAX_ARCHIVE_BYTES,
        "encrypted cache archive ({total_len} bytes) exceeds maximum allowed size ({MAX_ARCHIVE_BYTES} bytes)"
    );

    let subkey = derive_cache_subkey(key_material, repo_root_hash)?;
    let aad = build_cache_aad(repo_root_hash);
    let mut nonce = [0u8; DMC1_NONCE_LEN];
    csprng_fill(&mut nonce)?;

    let ct_and_tag = chacha20_poly1305_seal(subkey.as_bytes(), &nonce, &aad, gz_bytes)?;
    let mut out = Vec::with_capacity(DMC1_HEADER_LEN + ct_and_tag.len());
    out.extend_from_slice(DMC1_MAGIC);
    out.push(DMC1_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct_and_tag);
    Ok(out)
}

/// Decrypt and authenticate a `DMC1` container back into `.tar.gz` bytes.
pub fn decrypt_cache_archive(
    container: &[u8],
    key_material: &[u8],
    repo_root_hash: &str,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    ensure!(
        container.len() <= max_bytes,
        "encrypted cache archive ({} bytes) exceeds maximum allowed size ({max_bytes} bytes)",
        container.len()
    );
    ensure!(
        container.starts_with(DMC1_MAGIC),
        "cache archive is not an encrypted DMC1 container"
    );
    ensure!(
        container.len() >= DMC1_HEADER_LEN + DMC1_TAG_LEN,
        "truncated DMC1 cache archive ({} bytes)",
        container.len()
    );
    ensure!(
        container[4] == DMC1_VERSION,
        "unsupported DMC1 container version {} (expected {DMC1_VERSION})",
        container[4]
    );

    let nonce: [u8; DMC1_NONCE_LEN] = container[5..DMC1_HEADER_LEN]
        .try_into()
        .map_err(|_| anyhow!("invalid DMC1 nonce slice"))?;
    let ct_and_tag = &container[DMC1_HEADER_LEN..];

    let subkey = derive_cache_subkey(key_material, repo_root_hash)?;
    let aad = build_cache_aad(repo_root_hash);
    chacha20_poly1305_open(subkey.as_bytes(), &nonce, &aad, ct_and_tag)
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

    fn decode_hex(s: &str) -> Vec<u8> {
        let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        assert_eq!(clean.len() % 2, 0);
        clean
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn rfc8439_section_2_3_2_chacha20_block_kat() {
        let key: [u8; 32] =
            decode_hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
                .try_into()
                .unwrap();
        let nonce: [u8; 12] = decode_hex("000000090000004a00000000").try_into().unwrap();
        let expected = decode_hex(
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
             d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e",
        );
        let block = chacha20_block(&key, 1, &nonce);
        assert_eq!(block.as_slice(), expected.as_slice());
    }

    #[test]
    fn rfc8439_section_2_5_2_poly1305_mac_kat() {
        let key: [u8; 32] =
            decode_hex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b")
                .try_into()
                .unwrap();
        let msg = b"Cryptographic Forum Research Group";
        let expected = decode_hex("a8061dc1305136c6c22b8baf0c0127a9");
        let tag = poly1305_mac(&key, msg);
        assert_eq!(tag.as_slice(), expected.as_slice());
    }

    #[test]
    fn rfc8439_section_2_8_2_aead_chacha20_poly1305_kat() {
        let key: [u8; 32] =
            decode_hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
                .try_into()
                .unwrap();
        let nonce: [u8; 12] = decode_hex("070000004041424344454647").try_into().unwrap();
        let aad = decode_hex("50515253c0c1c2c3c4c5c6c7");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let expected = decode_hex(
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6\
             3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
             92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc\
             3ff4def08e4b7a9de576d26586cec64b6116\
             1ae10b594f09e26a7e902ecbd0600691",
        );

        let sealed = chacha20_poly1305_seal(&key, &nonce, &aad, plaintext).unwrap();
        assert_eq!(sealed, expected);

        let opened = chacha20_poly1305_open(&key, &nonce, &aad, &sealed).unwrap();
        assert_eq!(opened, plaintext);

        // Wrong AAD must fail closed.
        assert!(chacha20_poly1305_open(&key, &nonce, b"wrong-aad", &sealed).is_err());

        // Tampered tag or ciphertext must fail closed.
        let mut tampered_tag = sealed.clone();
        *tampered_tag.last_mut().unwrap() ^= 0x01;
        assert!(chacha20_poly1305_open(&key, &nonce, &aad, &tampered_tag).is_err());

        let mut tampered_ct = sealed;
        tampered_ct[0] ^= 0x80;
        assert!(chacha20_poly1305_open(&key, &nonce, &aad, &tampered_ct).is_err());
    }

    #[test]
    fn rfc5869_hkdf_sha256_kat() {
        // RFC 5869 Appendix A.1 (Test Case 1)
        let ikm = decode_hex("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let salt = decode_hex("000102030405060708090a0b0c");
        let info = decode_hex("f0f1f2f3f4f5f6f7f8f9");
        let expected = decode_hex(
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
        );
        let mut okm = [0u8; 42];
        hkdf_sha256(&salt, &ikm, &info, &mut okm).unwrap();
        assert_eq!(okm.as_slice(), expected.as_slice());
    }

    #[test]
    fn dmc1_container_roundtrip_and_fail_closed_tamper_aad_key() {
        let entries = vec![("a.txt".to_string(), b"secret-cache-payload".to_vec())];
        let gz = pack_tar_gz(&entries).unwrap();
        let key = b"super-secret-32-byte-key-material";
        let repo_hash = "a".repeat(64);

        let c1 = encrypt_cache_archive(&gz, key, &repo_hash).unwrap();
        let c2 = encrypt_cache_archive(&gz, key, &repo_hash).unwrap();
        assert!(is_dmc1_container(&c1));
        assert_eq!(&c1[..4], b"DMC1");
        assert_eq!(c1[4], 1);
        assert_eq!(c1.len(), DMC1_HEADER_LEN + gz.len() + DMC1_TAG_LEN);
        // Fresh CSPRNG nonces make ciphertexts distinct.
        assert_ne!(c1[5..17], c2[5..17]);
        assert_ne!(c1, c2);

        // Decryption restores exact tar.gz bytes.
        let dec = decrypt_cache_archive(&c1, key, &repo_hash, MAX_ARCHIVE_BYTES).unwrap();
        assert_eq!(dec, gz);

        // Wrong key fails closed.
        assert!(decrypt_cache_archive(&c1, b"wrong-key", &repo_hash, MAX_ARCHIVE_BYTES).is_err());

        // Wrong repo_root_hash AAD fails closed.
        let wrong_repo = "b".repeat(64);
        assert!(decrypt_cache_archive(&c1, key, &wrong_repo, MAX_ARCHIVE_BYTES).is_err());

        // Tampered ciphertext / tag / nonce / version fail closed.
        let mut bad_ct = c1.clone();
        bad_ct[DMC1_HEADER_LEN] ^= 0x01;
        assert!(decrypt_cache_archive(&bad_ct, key, &repo_hash, MAX_ARCHIVE_BYTES).is_err());

        let mut bad_tag = c1.clone();
        *bad_tag.last_mut().unwrap() ^= 0x01;
        assert!(decrypt_cache_archive(&bad_tag, key, &repo_hash, MAX_ARCHIVE_BYTES).is_err());

        let mut bad_nonce = c1.clone();
        bad_nonce[5] ^= 0x01;
        assert!(decrypt_cache_archive(&bad_nonce, key, &repo_hash, MAX_ARCHIVE_BYTES).is_err());

        let mut bad_ver = c1.clone();
        bad_ver[4] = 2;
        assert!(decrypt_cache_archive(&bad_ver, key, &repo_hash, MAX_ARCHIVE_BYTES).is_err());

        // Plaintext .tar.gz rejected as non-DMC1.
        assert!(!is_dmc1_container(&gz));
        assert!(decrypt_cache_archive(&gz, key, &repo_hash, MAX_ARCHIVE_BYTES).is_err());
    }
}
