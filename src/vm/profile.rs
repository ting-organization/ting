//! The execution profile's bounds and summary (rules §18). The binary
//! format, validation and compilation live in `decode`.
use crate::protocol::Error;

/// Bytes of byte work (zeroing, copying, hashing, loading) per compute unit.
pub const WORD: u64 = 32;
/// Compute units for `bytes` of byte work: one per started word.
pub const fn words(bytes: u64) -> u64 {
    bytes.div_ceil(WORD)
}
/// Largest declarable memory: the 32-bit address space.
pub const MAX_PAGES: u64 = 65_536;
/// Largest deployable module.
pub const MAX_MODULE: usize = 1 << 30;
/// Structural bounds on module shape (rules §18). They are allocation
/// bounds far above compiler output, never capacity limits.
pub const MAX_FUNCTIONS: u32 = 1_000_000;
pub const MAX_TYPES: u32 = 1_000_000;
pub const MAX_GLOBALS: u32 = 1_000_000;
pub const MAX_SEGMENTS: u32 = 100_000;
pub const MAX_PARAMETERS: usize = 1_000;
pub const MAX_LOCALS: u32 = 16_384;
pub const MAX_SLOTS: u32 = 32_768;
/// The host imports a module may name.
pub const IMPORTS: u32 = 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FunctionInfo {
    /// Parameters plus declared locals.
    pub locals: u32,
    /// Maximum typed operand-stack height.
    pub operands: u32,
}
impl FunctionInfo {
    /// Slots reserved and charged on entry.
    pub fn slots(self) -> u64 {
        u64::from(self.locals) + u64::from(self.operands)
    }
}
/// What charging needs to know about an accepted module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub functions: Vec<FunctionInfo>,
    pub imports: u32,
    pub types: u32,
    pub invoke: u32,
    /// Initial memory in bytes.
    pub memory: usize,
    pub globals: u64,
    pub data_bytes: u64,
    pub data_segments: u64,
    /// Active data segments holding at least one byte.
    pub data_spans: u64,
    pub table: u64,
}
impl Profile {
    /// Instance preparation debit (rules §15.6): the
    /// start, the initial pages, globals, table entries and data segments, and
    /// the data bytes and their spans at the initial memory's prices.
    pub fn preparation(&self) -> u64 {
        use super::price;
        let pages = self.memory as u64 / super::decode::PAGE;
        price::START
            + price::PAGE * pages
            + price::GLOBAL * self.globals
            + price::TABLE_ENTRY * self.table
            + price::DATA_SEGMENT * self.data_segments
            + price::Tier::of(self.memory as u64).bytes(self.data_bytes, self.data_spans)
    }
}

/// Exact first-byte whitelist (rules §17).
pub fn allowed_opcode(op: u8) -> bool {
    matches!(op,0x00..=0x05|0x0b..=0x11|0x1a|0x1b|0x20..=0x24|0x28|0x29|0x2c..=0x37|0x3a..=0x42|0x45..=0x5a|0x67..=0x8a|0xa7|0xac|0xad|0xc0..=0xc4|0xfc)
}
/// The six accepted `fc`-prefixed operations: the two bulk memory operations
/// and the four wide-arithmetic instructions.
pub fn allowed_prefixed(sub: u32) -> bool {
    matches!(sub, 0x0a | 0x0b | 0x13..=0x16)
}
/// Whether an original instruction byte debits dispatch: `else` and `end` do not.
pub fn charged_opcode(op: u8) -> bool {
    op != 0x05 && op != 0x0b
}
/// The signature of a host import: its number of i32 parameters and whether
/// it returns an i32.
pub fn signature(name: &str) -> Option<(usize, bool)> {
    let import = super::decode::Import::named(name.as_bytes())?;
    Some((import.params(), import.result()))
}

/// Decodes and validates a module against the execution profile.
pub fn validate(bytes: &[u8]) -> Result<Profile, Error> {
    super::decode::decode(bytes).map(|m| m.profile)
}
