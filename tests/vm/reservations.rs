//! Function reservations (rules §21): every entry charges its
//! parameters, locals and maximum operand height, and the stack pages it adds
//! past the invocation's mark, atomically and before the body runs, and nothing
//! but the budget bounds the depth.
use crate::{
    dsl::{import, invoke, memory, module},
    harness::{deployed as case, deployed_entry_of as entry_of, deployed_shape as shape},
};
use ting_core::protocol::Status;
use vm_model::{
    ACTIVATION, BUDGET, Ledger, PAGE, STACK_PAGE, TRANSACTION_BASE, call_size, cost, stack_pages,
};

#[test]
pub(crate) fn recursion_reserves_at_every_level() {
    for (n, locals) in [(10u32, 0usize), (100, 0), (10, 100), (300, 20)] {
        let declared = if locals == 0 {
            String::new()
        } else {
            format!("(local {})", "i32 ".repeat(locals))
        };
        let text = module(&[
            &memory(1, 1),
            &format!(
                "(func $down (param i32) {declared}
    local.get 0 (if (then local.get 0 i32.const 1 i32.sub call $down)))"
            ),
            &invoke(&format!("i32.const {n} call $down")),
        ]);
        // The entry calls the first level; each level with a positive counter
        // tests it, takes the branch and calls the next, the last tests and
        // skips. Every level reserves the parameter, the locals and the two
        // operands of the subtraction.
        let descend = cost("local.get 0 if local.get 0 i32.const 1 i32.sub call $down");
        case(&text)
            .named(format!("{n} levels, {locals} locals"))
            .call()
            .charged(Ledger {
                dispatch: cost("i32.const 0 call $down")
                    + u64::from(n) * descend
                    + cost("local.get 0 if end"),
                reservations: (u64::from(n) + 1) * (1 + locals as u64 + 2),
                ..Default::default()
            })
            .run();
    }
}
#[test]
pub(crate) fn parameters_and_locals_are_reserved_together_and_atomically() {
    let arguments = format!("{}call $wide", "i32.const 0 ".repeat(32));
    let text = module(&[
        &memory(1, 1),
        &format!(
            "(func $wide (param {}) (local {}) nop)",
            "i32 ".repeat(32),
            "i32 ".repeat(1024)
        ),
        &invoke(&arguments),
    ]);
    assert_eq!(shape(&text).defined(0), 32 + 1024);
    case(&text)
        .call()
        .charged(Ledger {
            dispatch: cost(&arguments) + cost("nop"),
            reservations: 1056,
            ..Default::default()
        })
        .run();
    // Compute for the thirty-two constants and the call before the callee is
    // entered and nothing more: the reservation is refused as a whole.
    case(&text)
        .named("refused")
        .call()
        .budget([
            1 + entry_of(&text) + cost(&arguments),
            BUDGET[1],
            BUDGET[2],
            BUDGET[3],
        ])
        .expect(Status::Compute)
        .charged(Ledger {
            dispatch: cost(&arguments),
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn the_operand_height_is_reserved_as_the_validator_computes_it() {
    let constants: String = (0..64).map(|i| format!("i64.const {i} ")).collect();
    let tall = format!("{constants}{}", "drop ".repeat(63));
    let body = "i32.const 0 call $tall i64.store i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(func $tall (result i64) {tall})"),
        &invoke(body),
    ]);
    assert_eq!(shape(&text).defined(0), 64);
    case(&text)
        .call()
        .result(0u64.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(body) + cost(&tall),
            reservations: 64,
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn unbounded_recursion_ends_exactly_where_the_budget_does() {
    for locals in [0u64, 1024] {
        let declared = if locals == 0 {
            String::new()
        } else {
            format!("(local {})", "i64 ".repeat(locals as usize))
        };
        let text = module(&[
            &import("input"),
            &memory(1, 1),
            &format!("(func $forever {declared} call $forever)"),
            &invoke("i32.const 0 i32.const 0 call $input (if (then call $forever))"),
        ]);
        let budget = 2_000_000;
        // The input query, which copies nothing into its empty buffer, and the
        // `if` precede the first call. Each level is the call's dispatch and
        // then one debit: the reservation of the locals and the stack pages
        // the level's height, its locals and record, takes past the mark. The
        // first debit that does not fit ends the transaction, and no frame
        // count intervenes.
        let call = cost("call $forever");
        let mut used = 1 + entry_of(&text) + cost("i32.const 0 i32.const 0 call $input if");
        let mut height = shape(&text).entry().slots + ACTIVATION;
        let mut mark = height;
        loop {
            if budget - used < call {
                break;
            }
            used += call;
            let next = height + locals + ACTIVATION;
            let raised = mark.max(next);
            let debit = locals + PAGE * (stack_pages(raised) - stack_pages(mark));
            if budget - used < debit {
                break;
            }
            used += debit;
            (height, mark) = (next, raised);
        }
        assert!(stack_pages(mark) > 0, "the recursion pays for stack");
        case(&text)
            .named(format!("{locals} locals per level"))
            .call_with(b"go")
            .budget([budget, BUDGET[1], BUDGET[2], BUDGET[3]])
            .expect(Status::Compute)
            .consumed([used, 2, TRANSACTION_BASE + call_size(2), 0])
            .run();
    }
}
#[test]
pub(crate) fn the_stack_pays_for_each_64_kib_its_mark_passes_once() {
    // Ten levels of a function reserving 1,003 slots: with its record each adds
    // 1,006 to the height, which passes the first 64 KiB, 8,192 slots, at the
    // ninth level. The second descent stays below the mark and pays no page
    // again.
    let text = module(&[
        &memory(1, 1),
        &format!(
            "(func $down (param i32) (local {})
    local.get 0 (if (then local.get 0 i32.const 1 i32.sub call $down)))",
            "i64 ".repeat(1000)
        ),
        &invoke("i32.const 9 call $down i32.const 9 call $down"),
    ]);
    let shape = shape(&text);
    let (level, start) = (shape.defined(0), shape.entry().slots + ACTIVATION);
    assert_eq!(level, 1 + 1000 + 2);
    assert!(start + 8 * (level + ACTIVATION) <= STACK_PAGE);
    assert!(start + 9 * (level + ACTIVATION) > STACK_PAGE);
    assert_eq!(stack_pages(start + 10 * (level + ACTIVATION)), 1);
    let descend = cost("local.get 0 if local.get 0 i32.const 1 i32.sub call $down");
    let descent = cost("i32.const 9 call $down") + 9 * descend + cost("local.get 0 if end");
    case(&text)
        .call()
        .charged(Ledger {
            dispatch: 2 * descent,
            reservations: 20 * level,
            stack: 1,
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn an_entry_function_beyond_64_kib_pays_its_stack_pages_on_entry() {
    // 9,000 locals and the record take the entry's height past 8,192 slots: one
    // page with its reservation, before the first instruction.
    let text = module(&[
        &memory(1, 1),
        &format!(
            "(func (export \"invoke\") (local {}) nop)",
            "i64 ".repeat(9000)
        ),
    ]);
    let entry = shape(&text).entry();
    assert_eq!(stack_pages(entry.slots + ACTIVATION), 1);
    case(&text)
        .call()
        .charged(Ledger {
            dispatch: cost("nop"),
            ..Default::default()
        })
        .run();
}
