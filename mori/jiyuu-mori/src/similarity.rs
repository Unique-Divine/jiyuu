//! Generate near-clone edges from the extractor's MinHash fingerprints.

use std::collections::{HashMap, HashSet};

#[cfg(feature = "native-engine-abi")]
mod native;

const WORDS: usize = 64;
const BAND_ROWS: usize = 2;
const BANDS: usize = WORDS / BAND_ROWS;
const MAX_BUCKET_SIZE: usize = 200;
const MAX_CANDIDATES: usize = 4096;
const MAX_EDGES_PER_SOURCE: usize = 10;
// 61/64 is the first agreement score at or above the reference's 0.95.
const MIN_AGREEMENT: usize = 61;

type Fingerprint = [u32; WORDS];

struct Entry {
    id: i64,
    name: Option<Vec<u8>>,
    path: Option<Vec<u8>>,
    fingerprint: Fingerprint,
    bands: [u16; BANDS],
}

#[derive(Debug, PartialEq, Eq)]
struct SimilarEdge {
    source: i64,
    target: i64,
    agreement: usize,
    same_file: bool,
}

fn fingerprint(properties: &[u8]) -> Option<Fingerprint> {
    const KEY: &[u8] = b"\"fp\":\"";
    const HEX_PER_WORD: usize = 8;
    let start = properties
        .windows(KEY.len())
        .position(|window| window == KEY)?;
    let value = &properties[start + KEY.len()..];
    let end = value.iter().position(|&byte| byte == b'"')?;
    if end != WORDS * HEX_PER_WORD {
        return None;
    }

    let mut fingerprint = [0; WORDS];
    for (num, chunk) in fingerprint
        .iter_mut()
        .zip(value[..end].chunks_exact(HEX_PER_WORD))
    {
        *num = hex_word(chunk);
    }
    Some(fingerprint)
}

// The C decoder uses strtoul on each fixed-width word. A nonhex suffix does
// not invalidate a word, and a word without digits decodes to zero.
fn hex_word(mut bytes: &[u8]) -> u32 {
    let first = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace() && *byte != 0x0b);
    bytes = first.map_or(&[], |first| &bytes[first..]);
    let negative = bytes.first() == Some(&b'-');
    if matches!(bytes.first(), Some(b'-' | b'+')) {
        bytes = &bytes[1..];
    }
    bytes = bytes
        .strip_prefix(b"0x")
        .or_else(|| bytes.strip_prefix(b"0X"))
        .unwrap_or(bytes);
    let value = bytes
        .iter()
        .map_while(|&byte| char::from(byte).to_digit(16))
        .fold(0, |word, digit| (word << 4) | digit);
    if negative {
        0u32.wrapping_sub(value)
    } else {
        value
    }
}

fn extension(path: Option<&[u8]>) -> &[u8] {
    let path = path.unwrap_or_default();
    path.iter()
        .rposition(|&byte| byte == b'.')
        .map_or(&[], |dot| &path[dot..])
}

enum Bucket {
    One(usize),
    Many(Vec<usize>),
    Overfull,
}

impl Bucket {
    fn push(&mut self, index: usize) {
        match self {
            Self::One(first) => *self = Self::Many(vec![*first, index]),
            Self::Many(indices) if indices.len() < MAX_BUCKET_SIZE => {
                indices.push(index)
            }
            Self::Many(_) => *self = Self::Overfull,
            Self::Overfull => {}
        }
    }

    fn candidates(&self) -> &[usize] {
        match self {
            Self::One(index) => std::slice::from_ref(index),
            Self::Many(indices) => indices,
            Self::Overfull => &[],
        }
    }
}

struct Index {
    entries: Vec<Entry>,
    buckets: HashMap<(usize, u16), Bucket>,
}

impl Index {
    fn new(mut entries: Vec<Entry>) -> Self {
        // Bucket insertion order and pair ownership both use canonical names.
        entries.sort_unstable_by(|a, b| {
            a.name
                .as_deref()
                .unwrap_or_default()
                .cmp(b.name.as_deref().unwrap_or_default())
                .then(a.id.cmp(&b.id))
        });
        let mut buckets = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            for (band, key) in entry.bands.into_iter().enumerate() {
                buckets
                    .entry((band, key))
                    .and_modify(|bucket: &mut Bucket| bucket.push(index))
                    .or_insert(Bucket::One(index));
            }
        }
        Self { entries, buckets }
    }

    fn collect_edges(
        &self,
        source: usize,
        seen: &mut HashSet<i64>,
        out: &mut Vec<SimilarEdge>,
    ) {
        seen.clear();
        let src = &self.entries[source];
        let Some(name) = &src.name else { return };
        let suffix = extension(src.path.as_deref());
        let mut emitted = 0;
        for (band, key) in src.bands.into_iter().enumerate() {
            let Some(bucket) = self.buckets.get(&(band, key)) else {
                continue;
            };
            for &target in bucket.candidates() {
                let target = &self.entries[target];
                if !seen.insert(target.id) {
                    continue;
                }
                if target.id != src.id
                    && target.name.as_ref().is_some_and(|other| name < other)
                    && extension(target.path.as_deref()) == suffix
                {
                    let agreement = src
                        .fingerprint
                        .iter()
                        .zip(&target.fingerprint)
                        .filter(|(left, right)| left == right)
                        .count();
                    if agreement >= MIN_AGREEMENT {
                        out.push(SimilarEdge {
                            source: src.id,
                            target: target.id,
                            agreement,
                            same_file: src.path.is_some()
                                && src.path == target.path,
                        });
                        emitted += 1;
                    }
                }
                if seen.len() == MAX_CANDIDATES
                    || emitted == MAX_EDGES_PER_SOURCE
                {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
