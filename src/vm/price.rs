//! The price list (rules §15): every compute price, the memory
//! prices, hashing and the code price. One C is about one dependent integer
//! addition; every other price is measured in those.
use super::{decode::Op, profile::words};

/// A transaction's root dispatch.
pub const ROOT: u64 = 1;
/// Starting and tearing down an instance, whatever the program.
pub const START: u64 = 40_000;
/// One 64 KiB page of memory, initial or grown.
pub const PAGE: u64 = 40_000;
/// Preparation per global, per table entry and per active data segment.
pub const GLOBAL: u64 = 40;
pub const TABLE_ENTRY: u64 = 4;
pub const DATA_SEGMENT: u64 = 12;
/// `call` of a defined function, `call_indirect`, and `call` of a host import.
pub const CALL: u64 = 9;
pub const CALL_INDIRECT: u64 = 50;
pub const HOST_CALL: u64 = 94;
/// `memory.copy` and `memory.fill`, before their bytes.
pub const BULK: u64 = 20;
/// Producing a successful deployment's identifier.
pub const IDENTIFIER: u64 = 32;
/// Slots in 64 KiB of stack, and the slots of the record every active function
/// keeps (rules §15.4).
pub const STACK_PAGE: u64 = 8_192;
pub const ACTIVATION: u64 = 3;

/// The stack pages a height of `h` slots pays for: every started 64 KiB but the
/// first, which the start price covers.
pub const fn stack_pages(h: u64) -> u64 {
    h.div_ceil(STACK_PAGE).saturating_sub(1)
}
/// Entering a function of `slots` reserved slots on a stack of `height` whose
/// mark is `mark`: the debit, its reservation and the stack pages it adds, and
/// the height and mark after it.
pub const fn entry(slots: u64, height: u64, mark: u64) -> (u64, u64, u64) {
    let height = height + slots + ACTIVATION;
    let raised = if height > mark { height } else { mark };
    let debit = slots + PAGE * (stack_pages(raised) - stack_pages(mark));
    (debit, height, raised)
}

/// A SHA-256 the runtime computes over an `m`-byte message.
pub const fn sha(m: u64) -> u64 {
    100 + 900 * (m + 9).div_ceil(64)
}
/// A record's key, hashed with every record read: `H("state-key", 1 + 32 + 32)`.
pub const KEY: u64 = sha(7 + 9 + 65);
/// A deployment's ContractId: `H("contract", chain ‖ transaction ‖ ordinal)`.
pub const CONTRACT_ID: u64 = sha(7 + 8 + 72);
/// A value of `n` bytes hashed into its leaf: `H("state-leaf", key ‖ u32 ‖ value)`.
pub const fn leaf(n: u64) -> u64 {
    sha(7 + 10 + 36 + n)
}
/// `n` bytes of code hashed into its CodeHash: `H("code", code)`.
pub const fn code_hash(n: u64) -> u64 {
    sha(7 + 4 + n)
}

/// The prices that depend on an instance's current memory size (rules
/// §15.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier {
    pub load: u64,
    pub store: u64,
    /// Per started 32-byte word of bytes moved.
    pub word: u64,
    /// Per span of memory, holding at least one byte, that bytes are moved
    /// out of or into.
    pub span: u64,
}
/// log₂ of `pages`, in sixteenths of a doubling, rounded down: the curve's
/// measure of a memory above 2 MiB.
const fn sixteenths(pages: u64) -> u64 {
    let b = 63 - pages.leading_zeros() as u64;
    16 * b + ((16 * pages) >> b) - 16
}
impl Tier {
    /// The prices of a memory of `bytes` bytes: three steps up to 2 MiB, then
    /// a curve over its size in pages.
    pub const fn of(bytes: u64) -> Self {
        let pages = bytes / 65_536;
        let (load, store, word, span) = if pages <= 1 {
            (18, 1, 4, 0)
        } else if pages <= 16 {
            (31, 1, 8, 0)
        } else if pages <= 32 {
            (80, 1, 8, 0)
        } else {
            let d = sixteenths(pages) - 80;
            let store = 16 + (d * d).div_ceil(72);
            (770 + (d * d).div_ceil(48), store, 12, store)
        };
        Self {
            load,
            store,
            word,
            span,
        }
    }
    /// The price of moving `n` bytes in `spans` spans that hold a byte.
    pub const fn bytes(self, n: u64, spans: u64) -> u64 {
        self.word * words(n) + self.span * spans
    }
}

/// The price of an original instruction, before it executes. `imports` is the
/// module's number of imported functions, which decides what a `call` costs;
/// loads and stores take the current tier's prices.
#[inline(always)]
pub(super) fn instruction(op: &Op, imports: u32, tier: Tier) -> u64 {
    use Op::*;
    match op {
        Else { .. } | End => 0,
        I32Load(_) | I64Load(_) | I32Load8S(_) | I32Load8U(_) | I32Load16S(_) | I32Load16U(_)
        | I64Load8S(_) | I64Load8U(_) | I64Load16S(_) | I64Load16U(_) | I64Load32S(_)
        | I64Load32U(_) => tier.load,
        I32Store(_) | I64Store(_) | I32Store8(_) | I32Store16(_) | I64Store8(_) | I64Store16(_)
        | I64Store32(_) => tier.store,
        Call(f) => {
            if *f < imports {
                HOST_CALL
            } else {
                CALL
            }
        }
        CallIndirect(_) => CALL_INDIRECT,
        MemoryCopy | MemoryFill => BULK,
        Loop | I32Rotl | I32Rotr | I64Rotl | I64Rotr | I64Add128 | I64Sub128 => 2,
        Select => 3,
        I32Eqz | I32Eq | I32Ne | I32LtS | I32LtU | I32GtS | I32GtU | I32LeS | I32LeU | I32GeS
        | I32GeU | I64Eqz | I64Eq | I64Ne | I64LtS | I64LtU | I64GtS | I64GtU | I64LeS | I64LeU
        | I64GeS | I64GeU | I32Clz | I32Ctz | I32Popcnt | I64Clz | I64Ctz | I64Popcnt => 4,
        I32Mul | I64Mul => 5,
        I64MulWideS | I64MulWideU => 8,
        If { .. } | BrIf(_) => 18,
        I32DivS | I32DivU | I32RemS | I32RemU | I64DivS | I64DivU | I64RemS | I64RemU => 33,
        BrTable(_) => 37,
        Unreachable | Nop | Block | Br(_) | Return | Drop | LocalGet(_) | LocalSet(_)
        | LocalTee(_) | GlobalGet(_) | GlobalSet(_) | MemorySize | MemoryGrow | I32Const(_)
        | I64Const(_) | I32Add | I32Sub | I32And | I32Or | I32Xor | I32Shl | I32ShrS | I32ShrU
        | I64Add | I64Sub | I64And | I64Or | I64Xor | I64Shl | I64ShrS | I64ShrU | I32WrapI64
        | I64ExtendI32S | I64ExtendI32U | I32Extend8S | I32Extend16S | I64Extend8S
        | I64Extend16S | I64Extend32S => 1,
    }
}

/// Per-byte rates of the code price, by section id, and its per-item prices.
const CODE_BASE: u64 = 14_400;
const BYTE: u64 = 60;
const ELEMENT_BYTE: u64 = 175;
const GLOBAL_BYTE: u64 = 100;
const DATA_BYTE: u64 = 6;
const FUNCTION: u64 = 1_215;
const SEGMENT: u64 = 446;

/// An unsigned LEB128 of at most five bytes whose value fits 32 bits, as the
/// code price's framing scan reads it; `None` ends the scan.
fn leb(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0u64;
    for k in 0..5 {
        let b = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(b & 0x7f) << (7 * k);
        if b & 0x80 == 0 {
            return u32::try_from(value).ok();
        }
    }
    None
}

/// The code price of a module, from its section framing alone (rules
/// §15.7): known before validation, paid by invalid code too.
/// Repeated sections can declare more than 2^64 - 1 C; the count stops there,
/// where every budget refuses it, as the exact price would be.
pub fn code(module: &[u8]) -> u64 {
    let mut price = CODE_BASE;
    let mut at = 8.min(module.len());
    let mut priced = 0u64;
    while at < module.len() {
        let start = at;
        let id = module[at];
        at += 1;
        // A section whose size cannot be read or whose contents run past the
        // end ends the scan; its bytes and the rest are priced at the default.
        let Some(size) = leb(module, &mut at) else {
            break;
        };
        let size = size as usize;
        let Some(end) = at.checked_add(size).filter(|end| *end <= module.len()) else {
            break;
        };
        let header = (at - start) as u64;
        let contents = &module[at..end];
        let rate = match id {
            9 => ELEMENT_BYTE,
            6 => GLOBAL_BYTE,
            11 => DATA_BYTE,
            _ => BYTE,
        };
        price = price
            .saturating_add(header * BYTE)
            .saturating_add((contents.len() as u64).saturating_mul(rate));
        let declared = || u64::from(leb(contents, &mut 0).unwrap_or(0));
        match id {
            3 => price = price.saturating_add(FUNCTION * declared()),
            11 => price = price.saturating_add(SEGMENT * declared()),
            _ => {}
        }
        priced += (end - start) as u64;
        at = end;
    }
    let preamble = 8.min(module.len()) as u64;
    let unscanned = module.len() as u64 - preamble - priced;
    price.saturating_add((preamble + unscanned).saturating_mul(BYTE))
}
