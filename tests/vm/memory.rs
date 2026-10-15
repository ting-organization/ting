//! Linear memory (wiki: vm/instructions, vm/lifecycle, resources/prices): every
//! load and store width, sign extension and truncation, little-endian order,
//! unaligned access, the exact end of memory, address-plus-offset overflow,
//! growth paid per page, loads, stores and bytes priced by the tier of the
//! memory's current size, declared memory paid per page, and data segments in
//! order.
use crate::{
    dsl::{escaped, import, invoke, memory, module, module_from},
    harness::case,
};
use ting_core::protocol::Status;
use vm_model::{Ledger, SMALL, Tier, cost, cost_at, words};

fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}
/// The entry of [`returning`]: stores the value of `expression` at address 0
/// and returns those eight bytes.
fn returning_body(expression: &str, width: u32) -> String {
    let widen = if width == 32 { " i64.extend_i32_u" } else { "" };
    format!("i32.const 0 {expression}{widen} i64.store i32.const 0 i32.const 8 call $return")
}
/// Returns the i64 value of `expression`, widening a 32-bit one.
fn returning(expression: &str, width: u32, data: &[&str]) -> String {
    let mut parts = vec![import("return"), memory(1, 1)];
    parts.extend(data.iter().map(|d| d.to_string()));
    parts.push(invoke(&returning_body(expression, width)));
    module_from(&parts)
}
/// What [`returning`] dispatches when it runs to its end.
fn returned(expression: &str, width: u32) -> Ledger {
    dispatch(cost(&returning_body(expression, width)))
}
/// What [`returning`] dispatches when `expression` traps at its last
/// instruction: the store's address and the expression, the trapping
/// instruction included, since its price is debited before it executes.
fn trapping(expression: &str) -> Ledger {
    dispatch(cost(&format!("i32.const 0 {expression}")))
}
/// The little-endian value of `payload` as a load of `width` bits sees it.
fn little_endian(payload: &[u8], signed: bool, width: u32) -> u64 {
    let mut value: u64 = 0;
    for (i, b) in payload.iter().enumerate() {
        value |= u64::from(*b) << (8 * i);
    }
    let bits = 8 * payload.len() as u32;
    if signed && bits < 64 && value >> (bits - 1) & 1 == 1 {
        value |= u64::MAX << bits;
    }
    if width == 32 {
        value &= u32::MAX as u64;
    }
    value
}

/// Name, bytes loaded, result width, signed.
const LOADS: [(&str, usize, u32, bool); 12] = [
    ("i32.load", 4, 32, false),
    ("i64.load", 8, 64, false),
    ("i32.load8_s", 1, 32, true),
    ("i32.load8_u", 1, 32, false),
    ("i32.load16_s", 2, 32, true),
    ("i32.load16_u", 2, 32, false),
    ("i64.load8_s", 1, 64, true),
    ("i64.load8_u", 1, 64, false),
    ("i64.load16_s", 2, 64, true),
    ("i64.load16_u", 2, 64, false),
    ("i64.load32_s", 4, 64, true),
    ("i64.load32_u", 4, 64, false),
];
/// Name, bytes stored, operand width.
const STORES: [(&str, usize, u32); 7] = [
    ("i32.store", 4, 32),
    ("i64.store", 8, 64),
    ("i32.store8", 1, 32),
    ("i32.store16", 2, 32),
    ("i64.store8", 1, 64),
    ("i64.store16", 2, 64),
    ("i64.store32", 4, 64),
];

#[test]
fn loads_extend_and_read_little_endian_at_any_alignment() {
    for (name, size, width, signed) in LOADS {
        let payloads = [
            vec![0xff; size],
            (1..=size as u8).collect(),
            [vec![0x80], vec![0; size - 1]].concat(),
        ];
        for payload in payloads {
            // Address 31 plus offset 2: unaligned for every width above a byte.
            let load = format!("i32.const 31 {name} offset=2");
            let text = returning(
                &load,
                width,
                &[&format!("(data (i32.const 33) {})", escaped(&payload))],
            );
            case(&text)
                .named(format!("{name} of {payload:02x?}"))
                .call()
                .result(little_endian(&payload, signed, width).to_le_bytes())
                .charged(returned(&load, width))
                .run();
        }
    }
}
#[test]
fn loads_at_the_exact_end_succeed_and_one_byte_past_it_trap() {
    for (name, size, width, _) in LOADS {
        let last = 65536 - size;
        let load = format!("i32.const {last} {name}");
        case(&returning(&load, width, &[]))
            .named(format!("{name} at the end"))
            .call()
            .result(0u64.to_le_bytes())
            .charged(returned(&load, width))
            .run();
        // The address, the constant and the load's own price.
        let load = format!("i32.const {} {name}", last + 1);
        case(&returning(&load, width, &[]))
            .named(format!("{name} past the end"))
            .call()
            .expect(Status::Trap)
            .charged(trapping(&load))
            .run();
        // The effective address is computed without wrapping.
        let load = format!("i32.const -1 {name} offset=1");
        case(&returning(&load, width, &[]))
            .named(format!("{name} with an overflowing offset"))
            .call()
            .expect(Status::Trap)
            .charged(trapping(&load))
            .run();
    }
}
#[test]
fn stores_truncate_and_write_little_endian_at_any_alignment() {
    for (name, size, width) in STORES {
        let value: u64 = if width == 32 {
            0x9abc_def1
        } else {
            0x1234_5678_9abc_def1
        };
        let constant = if width == 32 {
            format!("i32.const {}", value as u32 as i32)
        } else {
            format!("i64.const {}", value as i64)
        };
        // Written at 31 + 2; the bytes on either side stay zero.
        let body = format!(
            "i32.const 31 {constant} {name} offset=2 i32.const 32 i32.const {} call $return",
            size + 2
        );
        let text = module(&[&import("return"), &memory(1, 1), &invoke(&body)]);
        let mut expected = vec![0u8];
        expected.extend(&value.to_le_bytes()[..size]);
        expected.push(0);
        case(&text)
            .named(name)
            .call()
            .result(expected)
            .charged(dispatch(cost(&body)))
            .run();
        // A store that traps has paid its price, like one that succeeds.
        for (past, status) in [(0, Status::Success), (1, Status::Trap)] {
            let body = format!("i32.const {} {constant} {name}", 65536 - size + past);
            case(&module(&[&memory(1, 1), &invoke(&body)]))
                .named(format!("{name} {past} past the end"))
                .call()
                .expect(status)
                .charged(dispatch(cost(&body)))
                .run();
        }
    }
}
#[test]
fn memory_grow_pays_for_its_pages_and_reports_minus_one_beyond_the_maximum() {
    for (request, maximum) in [(0i32, 2u32), (1, 2), (2, 2), (-1, 2), (1, 1), (1023, 1024)] {
        let permitted = request >= 0 && (request as u32) < maximum;
        let body = format!(
            "i32.const 0 i32.const {request} memory.grow i32.store
    i32.const 4 memory.size i32.store
    i32.const 0 i32.const 8 call $return"
        );
        let text = module(&[&import("return"), &memory(1, maximum), &invoke(&body)]);
        let (old, size) = if permitted {
            (1u32, 1 + request as u32)
        } else {
            (u32::MAX, 1)
        };
        let mut result = old.to_le_bytes().to_vec();
        result.extend(size.to_le_bytes());
        // Everything after the growth runs at the tier of the grown memory:
        // its two stores and the copy of the result.
        let grown = if permitted { request as u64 } else { 0 };
        let tier = Tier::pages(1 + grown);
        case(&text)
            .named(format!("grow by {request} of {maximum}"))
            .call()
            .result(result)
            .charged(Ledger {
                dispatch: cost_at(&body, tier),
                pages: grown,
                extra: tier.bytes(8) - SMALL.bytes(8),
                ..Default::default()
            })
            .run();
    }
}
#[test]
fn grown_memory_is_zero_to_its_last_byte() {
    let body = "i32.const 1 memory.grow drop
    i32.const 0 i32.const 131064 i64.load i64.store
    i32.const 0 i32.const 8 call $return";
    let text = module(&[&import("return"), &memory(1, 2), &invoke(body)]);
    let tier = Tier::pages(2);
    case(&text)
        .call()
        .result([0u8; 8])
        .charged(Ledger {
            dispatch: cost_at(body, tier),
            pages: 1,
            extra: tier.bytes(8) - SMALL.bytes(8),
            ..Default::default()
        })
        .run();
}
#[test]
fn loads_and_stores_are_priced_by_the_tier_memory_has_grown_into() {
    // Memory grows from one page to exactly 1 MiB, one page past it, exactly
    // 2 MiB and one page past that, three tier boundaries. At every size the
    // program loads a zero, adds the size and stores it, so each load and
    // store is priced by the tier of the memory at that moment: 18 and 1, 31
    // and 1, 51 and 1 twice, then 690 and 50. The sizes are then copied once
    // more with memory.copy and returned, both at the last tier's 12 C per
    // word, where the model would price the result at the entry's 4.
    let sizes = [1u64, 16, 17, 32, 33];
    let mut body = Vec::new();
    let mut dispatched = 0;
    let mut pages = 1;
    for (stage, &size) in sizes.iter().enumerate() {
        if size > pages {
            let grow = format!("i32.const {} memory.grow drop", size - pages);
            dispatched += cost(&grow);
            body.push(grow);
            pages = size;
        }
        let at = 4 * stage;
        let access =
            format!("i32.const {at} i32.const {at} i32.load memory.size i32.add i32.store");
        dispatched += cost_at(&access, Tier::pages(size));
        body.push(access);
    }
    let stored: Vec<u8> = sizes
        .iter()
        .flat_map(|&p| (p as u32).to_le_bytes())
        .collect();
    let n = stored.len() as u64;
    let finish = format!(
        "i32.const {n} i32.const 0 i32.const {n} memory.copy i32.const 0 i32.const {} call $return",
        2 * n
    );
    dispatched += cost(&finish);
    body.push(finish);
    let text = module(&[
        &import("return"),
        &memory(1, 33),
        &invoke(&body.join("\n    ")),
    ]);
    let top = Tier::pages(33);
    case(&text)
        .call()
        .result([stored.clone(), stored].concat())
        .charged(Ledger {
            dispatch: dispatched,
            pages: 32,
            extra: top.bytes(n) + top.bytes(2 * n) - SMALL.bytes(2 * n),
            ..Default::default()
        })
        .run();
}
#[test]
fn a_declared_memory_sets_the_tier_from_the_first_instruction() {
    // The initial memory's tier prices the data segment's 40 bytes at
    // preparation, which the model reads from the module's shape, the load
    // and the store, and the 48 bytes returned. The sizes are the edges of
    // the first four tiers.
    let pattern: Vec<u8> = (1..=40).collect();
    let body = "i32.const 40 i32.const 0 i64.load i64.store i32.const 0 i32.const 48 call $return";
    let mut expected = pattern.clone();
    expected.extend(&pattern[..8]);
    for pages in [1u32, 2, 16, 17, 32, 33] {
        let text = module(&[
            &import("return"),
            &memory(pages, pages),
            &format!("(data (i32.const 0) {})", escaped(&pattern)),
            &invoke(body),
        ]);
        case(&text)
            .named(format!("{pages} pages"))
            .call()
            .result(&expected)
            .charged(dispatch(cost_at(body, Tier::pages(u64::from(pages)))))
            .run();
    }
}
#[test]
fn data_segments_apply_in_order() {
    let body = "i32.const 0 i32.const 4 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(data (i32.const 0) {})", escaped(b"abcd")),
        &format!("(data (i32.const 1) {})", escaped(b"XY")),
        &invoke(body),
    ]);
    case(&text)
        .call()
        .result(b"aXYd")
        .charged(dispatch(cost(body)))
        .run();
}
#[test]
fn a_large_declared_memory_is_paid_per_page() {
    // 64 MiB of initial memory is 1,024 pages at 40,000 C, 40,960,000 C of
    // preparation, which the model derives from the module's shape.
    let body = "i32.const 0 i32.const 0 call $return";
    let text = module(&[&import("return"), &memory(1024, 1024), &invoke(body)]);
    case(&text).call().charged(dispatch(cost(body))).run();
}

/// The entry of [`over_pattern`]: `instructions`, then a return of the first
/// `window` bytes.
fn and_return(instructions: &str, window: usize) -> String {
    format!("{instructions} i32.const 0 i32.const {window} call $return")
}
/// A program whose memory starts with `pattern` and runs `body`.
fn over_pattern(pattern: &[u8], body: &str) -> String {
    module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(data (i32.const 0) {})", escaped(pattern)),
        &invoke(body),
    ])
}

#[test]
fn memory_copy_over_every_small_overlap() {
    // Every destination, source and length over a 32-byte pattern: the
    // expected bytes are a Rust copy within a vector, direction included.
    let pattern: Vec<u8> = (1..=32).collect();
    for dst in 0..=8usize {
        for src in 0..=8usize {
            for len in [0usize, 1, 2, 3, 5, 8, 9, 16] {
                let mut expected = pattern.clone();
                expected.copy_within(src..src + len, dst);
                let body = and_return(
                    &format!("i32.const {dst} i32.const {src} i32.const {len} memory.copy"),
                    32,
                );
                case(&over_pattern(&pattern, &body))
                    .named(format!("copy {len} from {src} to {dst}"))
                    .call()
                    .result(expected)
                    .charged(Ledger {
                        dispatch: cost(&body),
                        copies: words(len as u64),
                        ..Default::default()
                    })
                    .run();
            }
        }
    }
}
#[test]
fn memory_copy_and_fill_across_word_boundaries() {
    let pattern: Vec<u8> = (0..=255u8).cycle().take(1200).collect();
    for (dst, src, len) in [
        (0usize, 1usize, 33usize),
        (1, 0, 33),
        (0, 2, 1000),
        (2, 0, 1000),
        (100, 0, 31),
        (0, 100, 32),
        (33, 1, 64),
        (5, 37, 63),
        (1100, 1000, 100),
        (1000, 1100, 100),
    ] {
        let mut expected = pattern.clone();
        expected.copy_within(src..src + len, dst);
        let body = and_return(
            &format!("i32.const {dst} i32.const {src} i32.const {len} memory.copy"),
            1200,
        );
        case(&over_pattern(&pattern, &body))
            .named(format!("copy {len} from {src} to {dst}"))
            .call()
            .result(expected)
            .charged(Ledger {
                dispatch: cost(&body),
                copies: words(len as u64),
                ..Default::default()
            })
            .run();
    }
    for dst in [0usize, 1, 31] {
        for value in [0i32, 0x5a, 0xff, 0x1ff, -1] {
            for len in [0usize, 1, 31, 32, 33, 64] {
                let mut expected = pattern[..128].to_vec();
                expected[dst..dst + len].fill(value as u8);
                let body = and_return(
                    &format!("i32.const {dst} i32.const {value} i32.const {len} memory.fill"),
                    128,
                );
                case(&over_pattern(&pattern[..128], &body))
                    .named(format!("fill {len} at {dst} with {value}"))
                    .call()
                    .result(expected)
                    .charged(Ledger {
                        dispatch: cost(&body),
                        copies: words(len as u64),
                        ..Default::default()
                    })
                    .run();
            }
        }
    }
}
#[test]
fn a_zero_length_bulk_operation_still_needs_a_range_inside_memory() {
    for (name, instructions, status) in [
        (
            "copy from the end",
            "i32.const 0 i32.const 65536 i32.const 0 memory.copy",
            Status::Success,
        ),
        (
            "copy from past the end",
            "i32.const 0 i32.const 65537 i32.const 0 memory.copy",
            Status::Trap,
        ),
        (
            "copy to past the end",
            "i32.const 65537 i32.const 0 i32.const 0 memory.copy",
            Status::Trap,
        ),
        (
            "copy from the top address",
            "i32.const 0 i32.const -1 i32.const 0 memory.copy",
            Status::Trap,
        ),
        (
            "fill at the end",
            "i32.const 65536 i32.const 0 i32.const 0 memory.fill",
            Status::Success,
        ),
        (
            "fill past the end",
            "i32.const 65537 i32.const 0 i32.const 0 memory.fill",
            Status::Trap,
        ),
        (
            "fill at the top address",
            "i32.const -1 i32.const 0 i32.const 0 memory.fill",
            Status::Trap,
        ),
    ] {
        // The operation's price is paid, and its zero bytes cost nothing,
        // before the range is checked.
        case(&module(&[&memory(1, 1), &invoke(instructions)]))
            .named(name)
            .call()
            .expect(status)
            .charged(dispatch(cost(instructions)))
            .run();
    }
}
#[test]
fn every_load_and_store_at_every_edge_of_memory() {
    // The effective address is the 33-bit sum of the address and the offset;
    // an access traps exactly when it ends past the one page.
    let traps = |address: u32, offset: u32, size: usize| {
        u64::from(address) + u64::from(offset) + size as u64 > 65536
    };
    for (name, size, width, _) in LOADS {
        for address in [
            65536 - size as u32,
            65536 - size as u32 + 1,
            65535,
            65536,
            u32::MAX,
        ] {
            for offset in [0u32, 1, size as u32 - 1, size as u32, 65535, u32::MAX] {
                let load = format!("i32.const {} {name} offset={offset}", address as i32);
                let c = case(&returning(&load, width, &[]))
                    .named(format!("{name} at {address} offset {offset}"))
                    .call();
                if traps(address, offset, size) {
                    c.expect(Status::Trap).charged(trapping(&load)).run();
                } else {
                    c.result(0u64.to_le_bytes())
                        .charged(returned(&load, width))
                        .run();
                }
            }
        }
    }
    for (name, size, width) in STORES {
        let constant = if width == 32 {
            "i32.const 1"
        } else {
            "i64.const 1"
        };
        for address in [
            65536 - size as u32,
            65536 - size as u32 + 1,
            65535,
            65536,
            u32::MAX,
        ] {
            for offset in [0u32, 1, size as u32 - 1, size as u32, 65535, u32::MAX] {
                let body = format!(
                    "i32.const {} {constant} {name} offset={offset}",
                    address as i32
                );
                case(&module(&[&memory(1, 1), &invoke(&body)]))
                    .named(format!("{name} at {address} offset {offset}"))
                    .call()
                    .expect(if traps(address, offset, size) {
                        Status::Trap
                    } else {
                        Status::Success
                    })
                    .charged(dispatch(cost(&body)))
                    .run();
            }
        }
    }
}
#[test]
fn loads_read_the_same_bytes_whatever_the_offset_split_and_alignment_hint() {
    let pattern: Vec<u8> = (0x11..=0x11 + 15).collect();
    for (name, size, width, signed) in LOADS {
        for base in 0..4usize {
            for offset in [0usize, 1, 2, 5] {
                let mut align = 1;
                while align <= size {
                    let data = format!("(data (i32.const 0) {})", escaped(&pattern));
                    let load = format!("i32.const {base} {name} offset={offset} align={align}");
                    let text = returning(&load, width, &[&data]);
                    let at = base + offset;
                    case(&text)
                        .named(format!("{name} at {base}+{offset} align {align}"))
                        .call()
                        .result(little_endian(&pattern[at..at + size], signed, width).to_le_bytes())
                        .charged(returned(&load, width))
                        .run();
                    align *= 2;
                }
            }
        }
    }
}
