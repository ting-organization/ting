//! Canonical SHA-256 commitments (wiki: data/encoding, data/state-tree).
use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

pub fn hash(tag: &str, bytes: &[u8]) -> Hash {
    hash_parts(tag, &[bytes])
}

/// `hash(tag, concatenation of parts)`, without building the concatenation.
pub fn hash_parts(tag: &str, parts: &[&[u8]]) -> Hash {
    assert!(tag.is_ascii() && tag.len() <= u16::MAX as usize);
    let mut digest = Sha256::new();
    digest.update(b"Ting/");
    digest.update((tag.len() as u16).to_le_bytes());
    digest.update(tag.as_bytes());
    for part in parts {
        digest.update(part);
    }
    digest.finalize().into()
}

pub fn vector_root(tag: &str, items: &[Vec<u8>]) -> Hash {
    assert!(items.len() <= u32::MAX as usize);
    let count = (items.len() as u32).to_le_bytes();
    if items.is_empty() {
        return hash(&format!("{tag}-root"), &count);
    }
    let size = items.len().next_power_of_two();
    let mut nodes = Vec::with_capacity(size);
    for (i, item) in items.iter().enumerate() {
        let mut bytes = (i as u64).to_le_bytes().to_vec();
        bytes.extend((item.len() as u32).to_le_bytes());
        bytes.extend(item);
        nodes.push(hash(&format!("{tag}-leaf"), &bytes));
    }
    for i in items.len()..size {
        nodes.push(hash(&format!("{tag}-pad"), &(i as u64).to_le_bytes()));
    }
    while nodes.len() > 1 {
        nodes = nodes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let mut bytes = pair[0].to_vec();
                bytes.extend(pair[1]);
                hash(&format!("{tag}-node"), &bytes)
            })
            .collect();
    }
    let mut bytes = count.to_vec();
    bytes.extend(nodes[0]);
    hash(&format!("{tag}-root"), &bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[usize::from(b >> 4)]);
        out.push(DIGITS[usize::from(b & 15)]);
    }
    String::from_utf8(out).expect("hex digits are ASCII")
}
