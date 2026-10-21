//! Heavy blocks (seed section S10): blocks one transaction fills in one
//! resource, each with its exact charges, as the floor every implementation
//! must validate within the block interval. Sizes follow the capacity of the
//! seed's genesis (`network`), leaving a little of the filled resource unused.
//!
//! - compute spent on 32-byte copies between pseudo-random addresses of a
//!   64 MiB memory, the cheapest work per C measured (calibration R6);
//! - compute spent on one memory growth, as many pages as a block buys;
//! - accesses spent on writing a record each, and on calls nested as deep as
//!   a block allows;
//! - data spent on events, and on one transaction's input;
//! - growth spent over several blocks on one record grown to about 1 MiB.
use crate::{
    dsl::{escaped, import, invoke, memory, module},
    harness::{CHILD, deployed as case, deployed_entry_of, deployed_warm_entry_of},
};
use ting_core::protocol::{BLOCK_BASE, Status};
use vm_model::{
    EVENT_BASE, Ledger, PAGE, RECORD_BASE, SMALL, TRANSACTION_BASE, Tier, call_size, cost, cost_at,
    leaf, words,
};

/// The seed genesis's capacity per block, C/S/D/G.
fn capacity() -> [u64; 4] {
    crate::network::genesis().initial.0
}
/// What the heavy transaction may spend of the resource it fills.
fn share(of: u64) -> u64 {
    of / 100 * 98
}
/// A budget of a whole block.
fn whole_block() -> [u64; 4] {
    capacity()
}

#[test]
pub(crate) fn several_ordered_transactions_reach_each_exact_block_capacity() {
    use crate::{gate, harness::deployed_shape};
    let prefix = "i32.const 256 i32.const 1 call $input drop i32.const 256 i32.load8_u local.set $mode local.get $mode i32.eqz if";
    let grow_test = "local.get $mode i32.const 4 i32.eq if";
    let grow = "i32.const 0 i32.const 32 i32.const 5 call $storage_set i32.const 0 i32.const 0 call $return";
    let mutate = "i32.const 32 i32.const 0 i32.store
        i32.const 0 i32.const 32 i32.const 4 call $storage_get drop
        i32.const 32 i32.const 32 i32.load i32.const 1 i32.add i32.store
        i32.const 0 i32.const 32 i32.const 4 call $storage_set
        i32.const 32 i32.const 1 call $emit";
    let reverting = "local.get $mode i32.const 2 i32.eq if";
    let trapping = "local.get $mode i32.const 3 i32.eq if";
    let ret = "i32.const 32 i32.const 4 call $return";
    let rev = "i32.const 32 i32.const 4 call $revert";
    let modes = [1u8, 1, 2, 3, 1];
    for dimension in 0..4 {
        let pages = if dimension >= 2 { 17 } else { 1 };
        let tier = Tier::pages(pages.into());
        let ledger_at = |mode, exists| Ledger {
            dispatch: vm_model::cost_at(
                &format!(
                    "{prefix} {grow_test} {mutate} {reverting} {}",
                    if mode == 2 {
                        rev.to_string()
                    } else {
                        format!("{trapping} {}", if mode == 3 { "unreachable" } else { ret })
                    }
                ),
                tier,
            ),
            copies: words(1)
                + words(32)
                + if exists { words(4) } else { 0 }
                + words(36)
                + u64::from(mode != 1),
            reads: 2,
            hashing: vm_model::leaf(4),
            accesses: 3,
            growth: if exists { 0 } else { 40 },
            discarded: if mode == 1 { 0 } else { 37 },
            ..Default::default()
        };
        let text_at = |filler: &str| {
            module(&[
                &import("input"),
                &import("storage_get"),
                &import("storage_set"),
                &import("emit"),
                &import("return"),
                &import("revert"),
                &memory(pages, pages),
                "(data (i32.const 64) \"\\01\")",
                &invoke(&format!(
                    "(local $mode i32) {prefix}
                {filler} else {grow_test} {grow} else {mutate} {reverting} {rev} end {trapping} unreachable end {ret} end end"
                )),
            ])
        };
        let sum_before = |text: &str| {
            let mut sum = [0; 4];
            sum[2] = BLOCK_BASE;
            for (i, &mode) in modes.iter().enumerate() {
                let ledger = ledger_at(mode, i != 0);
                let used = vm_model::call(
                    1,
                    deployed_shape(text).entry(),
                    Ledger {
                        dispatch: ledger.dispatch + cost(gate::ON_CALL),
                        ..ledger
                    },
                    if mode == 3 { 0 } else { 4 },
                    if mode == 1 { &[1] } else { &[] },
                );
                for d in 0..4 {
                    sum[d] += used[d];
                }
            }
            sum
        };
        // S/D/G do not depend on code size. C's filler is fixed before its
        // exact remaining budget is derived from the complete final module.
        let before = sum_before(&text_at("i32.const 0 i32.const 0 call $return"));
        let mut pad_value = Vec::new();
        let filler = match dimension {
            0 => "(loop $spin nop br $spin)".to_string(),
            1 => "i32.const 64 i32.const 0 i32.const 0 call $storage_get drop "
                .repeat((capacity()[1] - before[1] - 2) as usize),
            2 => format!(
                "i32.const 1024 i32.const {} call $emit",
                capacity()[2] - before[2] - TRANSACTION_BASE - call_size(0) - EVENT_BASE
            ),
            _ => {
                pad_value = vec![0; (capacity()[3] - before[3] - RECORD_BASE) as usize];
                format!(
                    "i32.const 64 i32.const 1024 i32.const {} call $storage_set",
                    pad_value.len()
                )
            }
        };
        let filler = if dimension == 0 {
            filler
        } else {
            format!("{filler} i32.const 0 i32.const 0 call $return")
        };
        let text = text_at(&filler);
        let before = sum_before(&text);
        let mut test = case(&text)
            .named(format!("exact-capacity/dimension-{dimension}"))
            .next_block();
        let mut value = 0u32;
        for (i, &mode) in modes.iter().enumerate() {
            let changed = value + 1;
            if mode == 1 {
                value = changed;
            }
            test = test
                .call_with(&[mode])
                .budget(whole_block())
                .expect(match mode {
                    2 => Status::Revert,
                    3 => Status::Trap,
                    _ => Status::Success,
                })
                .records([([0; 32], value.to_le_bytes())])
                .charged(ledger_at(mode, i != 0));
            if mode != 3 {
                test = test.result(changed.to_le_bytes());
            }
            if mode == 1 {
                test = test.events([changed.to_le_bytes()[..1].to_vec()]);
            }
        }
        let n = if dimension == 2 {
            capacity()[2] - before[2] - TRANSACTION_BASE - call_size(0) - EVENT_BASE
        } else {
            pad_value.len() as u64
        };
        let records = |value: u32| {
            let mut out = vec![([0; 32], value.to_le_bytes().to_vec())];
            if dimension == 3 {
                let mut key = [0; 32];
                key[0] = 1;
                out.push((key, pad_value.clone()));
            }
            out
        };
        test = test.call().budget(whole_block()).records(records(value));
        if dimension == 0 {
            let mut budget = whole_block();
            budget[0] -= before[0];
            test = test.budget(budget).expect(Status::Compute).consumed([
                budget[0],
                2,
                TRANSACTION_BASE + call_size(0),
                0,
            ]);
        } else {
            let mut ledger = Ledger {
                dispatch: vm_model::cost_at(&format!("{prefix} {filler}"), tier),
                ..Default::default()
            };
            match dimension {
                1 => {
                    let count = capacity()[1] - before[1] - 2;
                    ledger.copies = count * words(32);
                    ledger.reads = count;
                    ledger.accesses = count;
                }
                2 => {
                    test = test.events([vec![0; n as usize]]);
                }
                _ => {
                    ledger.copies = words(32 + n);
                    ledger.reads = 1;
                    ledger.accesses = 2;
                    ledger.hashing = vm_model::leaf(n);
                    ledger.growth = RECORD_BASE + n;
                }
            }
            test = test.charged(ledger);
        }
        // The same mutating transaction bytes execute once more after the
        // full block and observe its committed state, producing value four.
        test.next_block()
            .call_with(&[1])
            .budget(whole_block())
            .result((value + 1).to_le_bytes())
            .records(records(value + 1))
            .events([vec![(value + 1) as u8]])
            .charged(ledger_at(1, true))
            .next_block()
            .run();
    }
}

#[test]
pub(crate) fn random_small_copies_in_a_large_memory_fill_the_compute() {
    let pages = 1024u64;
    let tier = Tier::pages(pages);
    // An LCG gives the destination and source, each a multiple of 32. Write
    // a nonzero-varying word at the source, copy it and fold the destination
    // into a returned sum. Charging without copying cannot pass this witness.
    let iteration = |n: u64| {
        format!(
            "local.get $x i32.const 1103515245 i32.mul i32.const 12345 i32.add local.tee $x
      i32.const 67108832 i32.and
      local.get $x i32.const 6 i32.shr_u i32.const 67108832 i32.and
      local.tee $src local.get $x i64.extend_i32_u i64.store
      local.get $src i32.const 32 memory.copy
      local.get $sum local.get $x i32.const 67108832 i32.and i64.load i64.add local.set $sum
      local.get $i i32.const 1 i32.add local.tee $i
      i32.const {n} i32.lt_u br_if $again"
        )
    };
    let text = |n: u64| {
        module(&[
            &import("return"),
            &memory(pages as u32, pages as u32),
            &invoke(&format!(
                "(local $x i32) (local $i i32) (local $src i32) (local $sum i64)
    (loop $again
      {})
    i32.const 0 local.get $sum i64.store i32.const 0 i32.const 8 call $return",
                iteration(n)
            )),
        ])
    };
    let per = cost_at(&iteration(0), tier) + tier.bytes(32) + tier.spans(2);
    let finish = "i32.const 0 local.get $sum i64.store i32.const 0 i32.const 8 call $return";
    let fixed = 1
        + deployed_entry_of(&text(0))
        + cost("loop")
        + cost_at(finish, tier)
        + tier.bytes(8)
        + tier.spans(1);
    let n = (share(capacity()[0]) - fixed) / per;
    let (mut x, mut sum) = (0u32, 0u64);
    for _ in 0..n {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        sum = sum.wrapping_add(u64::from(x));
    }
    case(&text(n))
        .named("random small copies")
        .next_block()
        .call()
        .budget(whole_block())
        .result(sum.to_le_bytes())
        .charged(Ledger {
            dispatch: cost("loop") + n * cost_at(&iteration(n), tier) + cost_at(finish, tier),
            copies: n * words(32),
            spans: 2 * n,
            ..Default::default()
        })
        .next_block()
        .run();
}

#[test]
pub(crate) fn one_memory_growth_fills_the_compute() {
    // As many pages as the block's compute buys at 40,000 C each, then a byte
    // stored in the last of them at the grown memory's prices.
    let grow = |p: u64| format!("i32.const {p} memory.grow drop");
    let touch = |p: u64| {
        let end = (p + 1) * 65_536 - 1;
        format!(
            "i32.const 0 i32.const {end} i32.load8_u i32.store
            i32.const {end} i32.const 1 i32.store8
            i32.const 4 i32.const {end} i32.load8_u i32.store
            i32.const 8 memory.size i32.store
            i32.const 0 i32.const 12 call $return"
        )
    };
    let text = |p: u64| {
        module(&[
            &import("return"),
            &memory(1, 65_536),
            &invoke(&format!("{} {}", grow(p), touch(p))),
        ])
    };
    let fixed = 1 + deployed_entry_of(&text(0)) + cost(&grow(0)) + 1_000;
    let p = (share(capacity()[0]) - fixed) / PAGE;
    let grown = Tier::pages(1 + p);
    case(&text(p))
        .named(format!("one growth of {p} pages"))
        .next_block()
        .call()
        .budget(whole_block())
        .result(
            [0u32, 1, (p + 1) as u32]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .charged(Ledger {
            dispatch: cost(&grow(p)) + cost_at(&touch(p), grown),
            pages: p,
            extra: grown.bytes(12) + grown.spans(1) - SMALL.bytes(12),
            ..Default::default()
        })
        .next_block()
        .run();
}

#[test]
pub(crate) fn a_record_written_per_access_pair_fills_the_accesses() {
    // After the entry's two lookups, each write reads its old record and
    // writes it: two accesses for each of as many records as fit.
    let w = (capacity()[1] - 2) / 2;
    let iteration = format!(
        "i32.const 0 local.get $i i32.store
      i32.const 0 i32.const 32 i32.const 32 call $storage_set
      local.get $i i32.const 1 i32.add local.tee $i
      i32.const {w} i32.lt_u br_if $again"
    );
    let value = [0xa5u8; 32];
    let text = module(&[
        &import("storage_set"),
        &memory(1, 1),
        &format!("(data (i32.const 32) {})", escaped(&value)),
        &invoke(&format!(
            "(local $i i32)
    (loop $again
      {iteration})"
        )),
    ]);
    let records = (0..w as u32).map(|i| {
        let mut key = [0u8; 32];
        key[..4].copy_from_slice(&i.to_le_bytes());
        (key, value)
    });
    case(&text)
        .named(format!("{w} records written"))
        .next_block()
        .call()
        .budget(whole_block())
        .records(records)
        .charged(Ledger {
            dispatch: cost("loop") + w * cost(&iteration),
            copies: w * words(32 + 32),
            hashing: w * leaf(32),
            reads: w,
            accesses: 2 * w,
            growth: w * (RECORD_BASE + 32),
            ..Default::default()
        })
        .next_block()
        .run();
}

#[test]
pub(crate) fn events_fill_the_data() {
    // Events of a thousand bytes until the block's data is nearly spent:
    // each costs its payload and the event's base in the receipt.
    let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let p = payload.len() as u64;
    let room = share(capacity()[2]) - BLOCK_BASE - TRANSACTION_BASE - call_size(0);
    let e = room / (EVENT_BASE + p);
    let iteration = format!(
        "i32.const 0 i32.const {p} call $emit
      local.get $i i32.const 1 i32.add local.tee $i
      i32.const {e} i32.lt_u br_if $again"
    );
    let text = module(&[
        &import("emit"),
        &memory(1, 1),
        &format!("(data (i32.const 0) {})", escaped(&payload)),
        &invoke(&format!(
            "(local $i i32)
    (loop $again
      {iteration})"
        )),
    ]);
    case(&text)
        .named(format!("{e} events"))
        .next_block()
        .call()
        .budget(whole_block())
        .events(std::iter::repeat_n(&payload, e as usize))
        .charged(Ledger {
            dispatch: cost("loop") + e * cost(&iteration),
            ..Default::default()
        })
        .next_block()
        .run();
}

#[test]
pub(crate) fn one_input_fills_the_data() {
    // A call whose input takes the block's data; the program reads its
    // length and copies nothing.
    let body = "i32.const 0 i32.const 0 call $input drop";
    let text = module(&[&import("input"), &memory(1, 1), &invoke(body)]);
    let n = share(capacity()[2]) - BLOCK_BASE - TRANSACTION_BASE - call_size(0);
    let input: Vec<u8> = (0..n).map(|i| (i % 253) as u8).collect();
    case(&text)
        .named(format!("an input of {n} bytes"))
        .next_block()
        .call_with(&input)
        .budget(whole_block())
        .charged(Ledger {
            dispatch: cost(body),
            ..Default::default()
        })
        .next_block()
        .run();
}

/// A level reads its input, the remaining depth as two bytes, and branches.
const ENTER: &str = "i32.const 0 i32.const 2 call $input drop
    i32.const 0 i32.load16_u
    if";
/// A recursing level writes the depth less one as the child's input, reads
/// its own identifier as the child's target, and reads its remaining budget.
const DESCEND: &str = "i32.const 100 i32.const 0 i32.load16_u i32.const 1 i32.sub i32.store16
      i32.const 2 i32.const 128 i32.const 32 call $context drop
      i32.const 8 i32.const 64 i32.const 32 call $context";
/// It then subtracts `spent` from the remaining compute and calls itself with
/// the rest.
fn handing_on(spent: u64) -> String {
    format!(
        "drop i32.const 64 i32.const 64 i64.load i64.const {spent} i64.sub i64.store
      i32.const 200 i32.const 128 i32.const 100 i32.const 2 i32.const 64 i32.const 0 call $call"
    )
}
/// And returns the child's status; the last level returns nothing.
const PASS_BACK: &str = "i32.store i32.const 200 i32.const 4 call $return";
const BOTTOM: &str = "end
    i32.const 0 i32.const 0 call $return";
/// What a level spends from reading its remaining budget to the call's
/// budget check: the getter's copy, the instructions that hand the budget on
/// and the copy of the call's arguments.
fn spent() -> u64 {
    SMALL.bytes(32) + cost(&handing_on(0)) + SMALL.bytes(32 + 2 + 32)
}

#[test]
pub(crate) fn calls_nested_as_deep_as_the_accesses_allow() {
    // Every entered program reads its manifest and its code: a block's
    // accesses hold the top level and 511 levels below it, each passing on
    // exactly what it has left.
    let depth = (capacity()[1] - 2) / 2;
    let text = module(&[
        &import("input"),
        &import("context"),
        &import("call"),
        &import("return"),
        &memory(1, 1),
        &invoke(&format!(
            "{ENTER}
      {DESCEND}
      {}
      {PASS_BACK}
    {BOTTOM}",
            handing_on(spent())
        )),
    ]);
    let recursing = format!("{ENTER} {DESCEND} {} {PASS_BACK}", handing_on(0));
    let level = deployed_warm_entry_of(&text)
        + cost(&recursing)
        + SMALL.bytes(2)
        + 2 * SMALL.bytes(32)
        + SMALL.bytes(32 + 2 + 32)
        + SMALL.bytes(4);
    let last = deployed_warm_entry_of(&text) + cost(&format!("{ENTER} {BOTTOM}")) + SMALL.bytes(2);
    case(&text)
        .named(format!("calls nested {depth} deep"))
        .next_block()
        .call_with(&(depth as u16).to_le_bytes())
        .budget(whole_block())
        .result(0u32.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(&recursing),
            copies: words(2) + words(32) + words(32) + words(32 + 2 + 32),
            nested: (depth - 1) * level + last,
            accesses: 2 * depth,
            discarded: 4 * (depth - 1),
            ..Default::default()
        })
        .next_block()
        .run();
}

#[test]
pub(crate) fn one_record_grows_over_several_blocks_to_about_a_mebibyte() {
    // Each block rewrites the record 128,000 bytes longer, within the growth
    // a production block allows: the input names the length, the program
    // fills that many bytes and writes them.
    let body = "i32.const 64 i32.const 4 call $input drop
    i32.const 128 i32.const 7 i32.const 64 i32.load memory.fill
    i32.const 0 i32.const 128 i32.const 64 i32.load call $storage_set";
    let pages = 17u64;
    let tier = Tier::pages(pages);
    let text = module(&[
        &import("input"),
        &import("storage_set"),
        &memory(pages as u32, pages as u32),
        &invoke(body),
    ]);
    let step = 128_000u64;
    let mut c = case(&text).named("one record grown over blocks");
    for k in 1..=8u64 {
        let n = k * step;
        assert!(128 + n <= pages * 65_536);
        c = c
            .next_block()
            .call_with(&(n as u32).to_le_bytes())
            .budget(whole_block())
            .records([([0u8; 32], vec![7u8; n as usize])])
            .charged(Ledger {
                dispatch: cost_at(body, tier),
                copies: words(4) + words(n) + words(32 + n),
                hashing: leaf(n),
                reads: 1,
                accesses: 2,
                growth: if k == 1 { RECORD_BASE + n } else { step },
                ..Default::default()
            });
    }
    c.next_block().run();
}

#[test]
pub(crate) fn discarded_child_outputs_fill_data_even_when_the_block_is_small() {
    for events in [false, true] {
        let size = if events { 2048u64 } else { 4096 };
        let child_body = if events {
            format!(
                "i32.const 0 i32.const {size} call $emit i32.const 0 i32.const {size} call $return"
            )
        } else {
            format!("i32.const 0 i32.const {size} call $return")
        };
        let child = module(&[
            &import("emit"),
            &import("return"),
            &memory(1, 1),
            &format!(
                "(data (i32.const 0) {})",
                escaped(&vec![0xa7; size as usize])
            ),
            &invoke(&child_body),
        ]);
        let child_d = size + if events { EVENT_BASE + size } else { 0 };
        let room = share(capacity()[2]) - BLOCK_BASE - TRANSACTION_BASE - call_size(0) - 4;
        let count = (room / child_d).min((capacity()[1] - 2) / 2);
        let iteration = format!(
            "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
            local.get $i i32.const 1 i32.add local.tee $i i32.const {count} i32.lt_u br_if $again"
        );
        let finish = if events { "revert" } else { "return" };
        let tail = format!(
            "i32.const 128 local.get $i i32.store i32.const 128 i32.const 4 call ${finish}"
        );
        let body = format!("(local $i i32) (loop $again {iteration}) {tail}");
        let b: Vec<u8> = [4_000_000u64, 2, child_d, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let text = module(&[
            &import("call"),
            &import(finish),
            &memory(1, 1),
            &format!("(data (i32.const 0) {})", escaped(&CHILD)),
            &format!("(data (i32.const 64) {})", escaped(&b)),
            &invoke(&body),
        ]);
        let work = cost(&child_body) + SMALL.bytes(size) * if events { 2 } else { 1 };
        case(&text)
            .named(format!("discarded-data/events-{events}/{count}-children"))
            .with_program(CHILD, &child)
            .next_block()
            .call()
            .budget(whole_block())
            .expect(if events {
                Status::Revert
            } else {
                Status::Success
            })
            .result((count as u32).to_le_bytes())
            .charged(Ledger {
                dispatch: cost("loop") + count * cost(&iteration) + cost(&tail),
                copies: count * words(64),
                nested: deployed_entry_of(&child)
                    + (count - 1) * deployed_warm_entry_of(&child)
                    + count * work,
                accesses: 2 * count,
                discarded: count * child_d,
                ..Default::default()
            })
            .next_block()
            .run();
    }
}
