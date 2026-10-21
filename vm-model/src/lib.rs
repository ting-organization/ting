//! The expected side of the VM tests: the price list and the charging rules of
//! the protocol rules (sections 12, 15, 21 and 22) restated as
//! arithmetic over what a test declares a program does.
//!
//! This crate has no dependencies, and `ting-core` is not one of them, so
//! nothing here can come from the code under test. When a test fails on its
//! counters, either the VM, this model or the reading of the specification is
//! wrong, and the specification decides which.

/// Bytes per word of byte work (rules §15).
pub const WORD: u64 = 32;
/// Started words of `bytes` bytes.
pub const fn words(bytes: u64) -> u64 {
    bytes.div_ceil(WORD)
}

/// A transaction's root dispatch.
pub const ROOT: u64 = 1;
/// The start price of every invocation.
pub const START: u64 = 40_000;
/// One 64 KiB page of memory, initial or grown.
pub const PAGE: u64 = 40_000;
/// Preparation per global, table entry and active data segment.
pub const GLOBAL: u64 = 40;
pub const TABLE_ENTRY: u64 = 4;
pub const DATA_SEGMENT: u64 = 12;
/// Producing a successful deployment's identifier.
pub const IDENTIFIER: u64 = 32;
/// Slots in 64 KiB of stack, and the slots of every active function's record
/// (rules §15.4).
pub const STACK_PAGE: u64 = 8_192;
pub const ACTIVATION: u64 = 3;
/// The 64 KiB pages of stack a height of `h` slots pays for, the first one
/// being part of the start price.
pub const fn stack_pages(h: u64) -> u64 {
    if h <= STACK_PAGE {
        0
    } else {
        (h - 1) / STACK_PAGE
    }
}

/// A SHA-256 over an `m`-byte message: 100, and 900 per 64-byte block of the
/// padded message.
pub const fn sha(m: u64) -> u64 {
    100 + 900 * (m + 9).div_ceil(64)
}
/// A record key: `"Ting/"`, the tag length, `"state-key"`, namespace, contract
/// and logical key, 5 + 2 + 9 + 65 bytes.
pub const KEY: u64 = sha(81);
/// A ContractId: 5 + 2 + `"contract"` + chain, transaction and ordinal.
pub const CONTRACT_ID: u64 = sha(5 + 2 + 8 + 72);
/// A value of `n` bytes hashed into its leaf: 5 + 2 + `"state-leaf"` + key,
/// length and value.
pub const fn leaf(n: u64) -> u64 {
    sha(5 + 2 + 10 + 32 + 4 + n)
}
/// `n` bytes of code hashed into its CodeHash: 5 + 2 + `"code"` + the code.
pub const fn code_hash(n: u64) -> u64 {
    sha(5 + 2 + 4 + n)
}

/// The prices of a memory size (rules §15.2):
/// loads, stores, words of bytes moved and spans moved out of or into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier {
    pub load: u64,
    pub store: u64,
    pub word: u64,
    pub span: u64,
}
impl Tier {
    pub const fn of(memory: u64) -> Self {
        Self::pages(memory / 65_536)
    }
    /// The prices of a memory of `pages` 64 KiB pages: three steps up to
    /// 2 MiB, then the curve over `L`, log₂ of the pages in sixteenths.
    pub const fn pages(pages: u64) -> Self {
        if pages <= 1 {
            return Self {
                load: 18,
                store: 1,
                word: 4,
                span: 0,
            };
        }
        if pages <= 16 {
            return Self {
                load: 31,
                store: 1,
                word: 8,
                span: 0,
            };
        }
        if pages <= 32 {
            return Self {
                load: 80,
                store: 1,
                word: 8,
                span: 0,
            };
        }
        // b: the highest set bit, counted the slow way round.
        let mut b = 0;
        while pages >> (b + 1) != 0 {
            b += 1;
        }
        let l = 16 * b + pages * 16 / (1 << b) - 16;
        let square = (l - 80) * (l - 80);
        let store = 16 + square.div_ceil(72);
        Self {
            load: 770 + square.div_ceil(48),
            store,
            word: 12,
            span: store,
        }
    }
    /// The words of moving `n` bytes.
    pub const fn bytes(self, n: u64) -> u64 {
        self.word * words(n)
    }
    /// The spans of moving bytes: `count` spans that hold a byte.
    pub const fn spans(self, count: u64) -> u64 {
        self.span * count
    }
}
/// The tier of every memory up to one page, the tier most cases run in.
pub const SMALL: Tier = Tier::of(65_536);

/// The twelve host imports; a `call` naming one of them is a host call.
pub const IMPORTS: [&str; 12] = [
    "input",
    "context",
    "storage_get",
    "storage_set",
    "storage_delete",
    "code_hash",
    "call",
    "deploy",
    "call_result",
    "emit",
    "return",
    "revert",
];

/// The price of one instruction by its text name, at `tier`. `call` alone is a
/// call of a defined function; [`cost_at`] tells host calls apart by name.
pub fn price(name: &str, tier: Tier) -> u64 {
    let op = name.split_once('.').map_or(name, |(_, op)| op);
    let bare = matches!(name, "else" | "end");
    if bare {
        return 0;
    }
    match name {
        "call" => return 9,
        "call_indirect" => return 50,
        "memory.copy" | "memory.fill" => return 20,
        "loop" => return 2,
        "select" => return 3,
        "if" | "br_if" => return 18,
        "br_table" => return 37,
        "i64.add128" | "i64.sub128" => return 2,
        "i64.mul_wide_s" | "i64.mul_wide_u" => return 8,
        "memory.size" | "memory.grow" => return 1,
        _ => {}
    }
    if op.starts_with("load") {
        return tier.load;
    }
    if op.starts_with("store") {
        return tier.store;
    }
    match op {
        "eqz" | "eq" | "ne" | "lt_s" | "lt_u" | "gt_s" | "gt_u" | "le_s" | "le_u" | "ge_s"
        | "ge_u" | "clz" | "ctz" | "popcnt" => 4,
        "mul" => 5,
        "rotl" | "rotr" => 2,
        "div_s" | "div_u" | "rem_s" | "rem_u" => 33,
        _ => 1,
    }
}

/// The price of a sequence of instructions in WebAssembly text, at the
/// smallest tier: every word that names an instruction counts; immediates,
/// labels, strings, `offset=`, `align=` and the declaration words of folded
/// text (`local`, `then`, `func`, `param`, `result` and the like) do not.
/// `call $name` is a host call when `name` is one of the twelve imports. The
/// text must be what executes: skipped arms and branches are left out.
pub fn cost(text: &str) -> u64 {
    cost_at(text, SMALL)
}
/// [`cost`] at a given memory tier.
pub fn cost_at(text: &str, tier: Tier) -> u64 {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|w| !w.is_empty())
        .collect();
    let mut total = 0;
    for (k, w) in words.iter().enumerate() {
        let first = w.chars().next().unwrap();
        let immediate = first == '$'
            || first == '-'
            || first.is_ascii_digit()
            || w.starts_with("offset=")
            || w.starts_with("align=")
            || w.starts_with("0x")
            || matches!(
                *w,
                "i32"
                    | "i64"
                    | "result"
                    | "param"
                    | "type"
                    | "local"
                    | "then"
                    | "func"
                    | "module"
                    | "export"
                    | "import"
                    | "mut"
                    | "memory"
                    | "data"
                    | "elem"
                    | "table"
                    | "global"
                    | "funcref"
            )
            || w.starts_with('"');
        if immediate {
            continue;
        }
        if *w == "call" {
            let host = words
                .get(k + 1)
                .and_then(|n| n.strip_prefix('$'))
                .is_some_and(|n| IMPORTS.contains(&n));
            total += if host { 94 } else { 9 };
            continue;
        }
        total += price(w, tier);
    }
    total
}

/// The code price of a module (rules §15.7) as a debit: the
/// exact price, or 2^64 - 1 for one beyond it, which every budget refuses.
pub fn code_price(module: &[u8]) -> u64 {
    u64::try_from(exact_code_price(module)).unwrap_or(u64::MAX)
}
/// The exact code price, read from the section framing: 14,400, each byte at
/// its section's rate, and 1,215 per function and 446 per data segment that
/// each function and data section declares. Repeated sections can take it
/// past 2^64 - 1.
pub fn exact_code_price(module: &[u8]) -> u128 {
    fn leb(b: &[u8], at: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for k in 0..5 {
            let x = *b.get(*at)?;
            *at += 1;
            v |= u64::from(x & 0x7f) << (7 * k);
            if x < 0x80 {
                return (v <= u64::from(u32::MAX)).then_some(v);
            }
        }
        None
    }
    let mut price: u128 = 14_400;
    let mut at = module.len().min(8);
    let mut rest = module.len() as u128;
    loop {
        if at >= module.len() {
            break;
        }
        let start = at;
        let id = module[at];
        at += 1;
        let Some(size) = leb(module, &mut at) else {
            break;
        };
        let end = at + size as usize;
        if end > module.len() {
            break;
        }
        let rate = match id {
            9 => 175,
            6 => 100,
            11 => 6,
            _ => 60,
        };
        price += (at - start) as u128 * 60 + (end - at) as u128 * rate;
        let count = u128::from(leb(&module[at..end], &mut 0).unwrap_or(0));
        price += match id {
            3 => 1_215 * count,
            11 => 446 * count,
            _ => 0,
        };
        rest -= (end - start) as u128;
        at = end;
    }
    price + rest * 60
}

/// Data debited for every transaction before its payload (rules §12).
pub const TRANSACTION_BASE: u64 = 81;
/// Encoded size of a call: version, kind, budget, target, input length, input.
pub const fn call_size(input: u64) -> u64 {
    1 + 1 + 32 + 32 + 4 + input
}
/// Encoded size of a deployment: version, kind, budget, code length, code,
/// input length, input.
pub const fn deploy_size(code: u64, input: u64) -> u64 {
    1 + 1 + 32 + 4 + code + 4 + input
}
/// Data debited for an event besides its payload: the contract and the length
/// (rules §22).
pub const EVENT_BASE: u64 = 36;
/// Growth a record costs besides its value: its key and its length
/// (rules §12).
pub const RECORD_BASE: u64 = 36;
/// A manifest record: the code hash, the code length and the code price.
pub const MANIFEST: u64 = 32 + 4 + 8;
/// Record accesses of a deployment before its initializer runs: the existence
/// check, then the old-record read and the write of the manifest and of the
/// code (rules §22, with its storage rule).
pub const DEPLOY_ACCESSES: u64 = 1 + 2 + 2;
/// The budget a case runs under unless it says otherwise, C/S/D/G.
pub const BUDGET: [u64; 4] = [100_000_000, 4096, 1_000_000, 1_000_000];

/// What entering a program costs before its first instruction (rules
/// §23, §15): the keys of its manifest and code record,
/// its code price when the transaction has not yet prepared it, the instance's
/// preparation and the entry function's reservation. The harness reads the
/// quantities from the module's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub code_price: u64,
    /// Start, initial pages, globals, table entries, data segments and data bytes.
    pub preparation: u64,
    /// Parameters, locals and the maximum operand height of the entry function.
    pub slots: u64,
    /// The tier of the initial memory, which prices the bytes the program moves.
    pub tier: Tier,
    /// Whether this invocation pays the code price: the first invocation of
    /// the code in its transaction, unless the transaction deployed it.
    pub cold: bool,
}
impl Default for Entry {
    fn default() -> Self {
        Self {
            code_price: 0,
            preparation: 0,
            slots: 0,
            tier: SMALL,
            cold: false,
        }
    }
}
impl Entry {
    pub const fn compute(self) -> u64 {
        2 * KEY
            + if self.cold { self.code_price } else { 0 }
            + self.preparation
            + self.slots
            + PAGE * stack_pages(self.slots + ACTIVATION)
    }
    /// The same entry, for code its transaction has already prepared.
    pub const fn warm(self) -> Self {
        Self {
            cold: false,
            ..self
        }
    }
}

/// What a program does on its path, as the test declares it. Whatever follows
/// from the declared outcome (the debits of the result and the events) or from
/// the module (the entry) is added by the model, never declared twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    /// The prices of the original instructions executed, from [`cost`].
    pub dispatch: u64,
    /// Words of byte work at the program's memory prices: host argument
    /// copies, getter output copies, `memory.copy` and `memory.fill` bytes.
    pub copies: u64,
    /// Spans those copies move bytes out of or into, each holding a byte:
    /// two for a `memory.copy`, one for a `memory.fill` and one for each host
    /// argument or output.
    pub spans: u64,
    /// Leaf hashes of values written by `storage_set`, each [`leaf`] of its length.
    pub hashing: u64,
    /// Record reads by the program's own host calls: `storage_get`,
    /// `storage_set`, `storage_delete` and `code_hash` read one each, and each
    /// pays [`KEY`].
    pub reads: u64,
    /// Slots of every defined function entered through a call.
    pub reservations: u64,
    /// 64 KiB pages of stack paid on entering called functions: every 64 KiB
    /// the stack's mark passes beyond the entry function's own pages.
    pub stack: u64,
    /// Pages added by successful `memory.grow`.
    pub pages: u64,
    /// Record accesses beyond the manifest and code lookups of the entry.
    pub accesses: u64,
    /// Bytes of state growth (rules §12).
    pub growth: u64,
    /// Compute consumed by nested invocations, their entries included.
    pub nested: u64,
    /// Compute of nested deployments' validation: their code price and CodeHash.
    pub validation: u64,
    /// Any other compute a case states explicitly, such as bytes moved at
    /// another tier after memory grew.
    pub extra: u64,
    /// Data debited that the receipt does not show: results of nested
    /// invocations, identifiers of nested deployments, and events or results
    /// rolled back by a later failure. An event the receipt shows is counted
    /// from the receipt instead, its copy included.
    pub discarded: u64,
}
impl Ledger {
    const fn compute(self, tier: Tier) -> u64 {
        self.dispatch
            + self.copies * tier.word
            + self.spans * tier.span
            + self.hashing
            + self.reads * KEY
            + self.reservations
            + self.stack * PAGE
            + self.pages * PAGE
            + self.nested
            + self.validation
            + self.extra
    }
}
fn event_compute(tier: Tier, events: &[u64]) -> u64 {
    events
        .iter()
        .map(|&n| tier.bytes(n) + tier.spans(u64::from(n > 0)))
        .sum()
}
fn event_data(events: &[u64]) -> u64 {
    events.iter().map(|&n| EVENT_BASE + n).sum()
}

/// The counters one top-level call consumes: the root dispatch, the entry, the
/// declared ledger, the debits of a result of `result` bytes and of the given
/// event payloads, the two record lookups, and the transaction envelope.
pub fn call(input: u64, entry: Entry, ledger: Ledger, result: u64, events: &[u64]) -> [u64; 4] {
    [
        ROOT + entry.compute()
            + ledger.compute(entry.tier)
            + entry.tier.bytes(result)
            + entry.tier.spans(u64::from(result > 0))
            + event_compute(entry.tier, events),
        2 + ledger.accesses,
        TRANSACTION_BASE + call_size(input) + result + ledger.discarded + event_data(events),
        ledger.growth,
    ]
}

/// What a deployment of `code` costs before its initializer's entry (rules
/// §23): the ContractId, the existence check, the code price and the
/// CodeHash, and writing the manifest and the code record.
pub fn installation(code: &[u8]) -> u64 {
    let n = code.len() as u64;
    CONTRACT_ID + KEY + code_price(code) + code_hash(n) + KEY + leaf(MANIFEST) + KEY + leaf(n)
}

/// The counters one successful top-level deployment consumes (rules
/// §23, §22): the installation, the initializer's entry, warm since
/// the deployment prepared its code, its ledger and events, and the identifier.
/// The initializer's own result is discarded, so a result it returns is
/// declared as `discarded`.
pub fn deploy(code: &[u8], input: u64, entry: Entry, ledger: Ledger, events: &[u64]) -> [u64; 4] {
    let n = code.len() as u64;
    [
        ROOT + installation(code)
            + entry.warm().compute()
            + ledger.compute(entry.tier)
            + event_compute(entry.tier, events)
            + IDENTIFIER,
        DEPLOY_ACCESSES + 2 + ledger.accesses,
        TRANSACTION_BASE + deploy_size(n, input) + ledger.discarded + event_data(events) + 32,
        RECORD_BASE + MANIFEST + RECORD_BASE + n + ledger.growth,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_charge_started_words() {
        assert_eq!(words(0), 0);
        assert_eq!(words(1), 1);
        assert_eq!(words(32), 1);
        assert_eq!(words(33), 2);
    }
    #[test]
    fn hashes_are_priced_by_their_padded_blocks() {
        // 55 bytes fit one block with the padding, 56 need two.
        assert_eq!(sha(55), 1_000);
        assert_eq!(sha(56), 1_900);
        assert_eq!(KEY, 1_900);
        assert_eq!(CONTRACT_ID, 1_900);
        assert_eq!(leaf(0), 1_000);
        assert_eq!(leaf(2), 1_000);
        assert_eq!(leaf(3), 1_900);
        assert_eq!(leaf(MANIFEST), 1_900);
        assert_eq!(code_hash(44), 1_000);
        assert_eq!(code_hash(45), 1_900);
    }
    #[test]
    fn memory_prices_step_then_follow_the_curve() {
        assert_eq!(Tier::pages(0), SMALL);
        assert_eq!(Tier::pages(1), SMALL);
        assert_eq!(Tier::pages(2).load, 31);
        assert_eq!(Tier::pages(16).load, 31);
        assert_eq!(Tier::pages(17).load, 80);
        assert_eq!(
            Tier::pages(32),
            Tier {
                load: 80,
                store: 1,
                word: 8,
                span: 0
            }
        );
        // The table of the price list.
        for (pages, load, store) in [
            (33, 770, 16),
            (48, 772, 17),
            (256, 818, 48),
            (1_024, 904, 105),
            (4_096, 1_032, 191),
            (16_384, 1_202, 304),
            (65_536, 1_416, 447),
        ] {
            assert_eq!(
                Tier::pages(pages),
                Tier {
                    load,
                    store,
                    word: 12,
                    span: store
                },
                "{pages} pages"
            );
        }
        assert_eq!(SMALL.bytes(33) + SMALL.spans(2), 8);
        assert_eq!(
            Tier::pages(48).bytes(33) + Tier::pages(48).spans(2),
            24 + 34
        );
    }
    #[test]
    fn instruction_text_is_priced_word_by_word() {
        assert_eq!(cost("i32.const 0 i64.const 42 i64.store"), 3);
        assert_eq!(cost("i32.const 0 i32.const 8 call $return"), 96);
        assert_eq!(cost("local.get 0 call $f"), 10);
        assert_eq!(cost("i64.mul i64.div_s i32.lt_u br_if 0"), 5 + 33 + 4 + 18);
        assert_eq!(cost("i32.load offset=4 i64.store8"), 19);
        assert_eq!(cost_at("i32.load", Tier::pages(2)), 31);
        assert_eq!(cost("block loop br 1 end end"), 1 + 2 + 1);
        assert_eq!(cost("i64.add128 i64.mul_wide_u"), 10);
        assert_eq!(cost("(local i32) (then (local.get 0) (drop))"), 2);
    }
    #[test]
    fn the_code_price_reads_section_framing_only() {
        // The preamble alone, and a module with an empty type section.
        let preamble = b"\0asm\x01\0\0\0";
        assert_eq!(code_price(preamble), 14_400 + 8 * 60);
        let mut types = preamble.to_vec();
        types.extend([1, 1, 0]);
        assert_eq!(code_price(&types), 14_400 + 11 * 60);
        // A function section declaring two functions, a data section with one
        // segment of three bytes, and a truncated section at the end.
        let mut m = preamble.to_vec();
        m.extend([3, 3, 2, 0, 0]);
        m.extend([11, 7, 1, 0, 0x41, 0, 0x0b, 3, 9]);
        m.extend([10, 9, 0]);
        assert_eq!(
            code_price(&m),
            14_400 + 8 * 60 + (2 + 3) * 60 + 2 * 1_215 + 2 * 60 + 7 * 6 + 446 + 3 * 60
        );
    }
    #[test]
    fn a_bare_call_pays_root_entry_lookups_and_envelope() {
        let entry = Entry {
            code_price: 15_000,
            preparation: 80_000,
            slots: 2,
            tier: SMALL,
            cold: true,
        };
        assert_eq!(
            call(0, entry, Ledger::default(), 0, &[]),
            [
                1 + 2 * 1_900 + 15_000 + 80_000 + 2,
                2,
                TRANSACTION_BASE + 70,
                0
            ]
        );
    }
    #[test]
    fn results_and_events_add_their_bytes() {
        let entry = Entry::default();
        assert_eq!(
            call(3, entry, Ledger::default(), 40, &[5]),
            [1 + 2 * 1_900 + 8 + 4, 2, TRANSACTION_BASE + 73 + 40 + 41, 0]
        );
    }
}
