//! Structured control flow, locals and globals, calls and indirect calls,
//! `select`, `unreachable` and the bulk memory instructions (wiki:
//! vm/instructions, with the dispatch rules of vm/metering and the prices of
//! resources/prices): what runs, what is skipped and what each path costs. The
//! instructions each path executes are written out next to each program and
//! priced with `cost`; skipped instructions, `else` and `end` cost nothing.
use crate::{
    dsl::{escaped, import, invoke, memory, module},
    harness::case,
};
use ting_core::protocol::Status;
use vm_model::{Ledger, cost, words};

/// The instructions around an expression that `returning` adds: the address,
/// the store, the span and the return.
const AROUND: &str = "i32.const 0 i64.store i32.const 0 i32.const 8 call $return";
/// Stores the i64 value of `expression` at address 0 and returns those eight
/// bytes.
fn returning(expression: &str) -> String {
    module(&[
        &import("return"),
        &memory(1, 1),
        &invoke(&format!(
            "i32.const 0 {expression} i64.store i32.const 0 i32.const 8 call $return"
        )),
    ])
}
/// `expression` leaves `value`, and the instructions it executes cost
/// `dispatch`.
fn value(expression: &str, dispatch: u64, value: u64) {
    case(&returning(expression))
        .named(expression)
        .call()
        .result(value.to_le_bytes())
        .charged(Ledger {
            dispatch: dispatch + cost(AROUND),
            ..Default::default()
        })
        .run();
}
fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}

#[test]
fn select_takes_the_first_operand_for_a_nonzero_condition() {
    for cond in [0, 1, -1] {
        // Three constants and the select all run.
        let expression = format!("i64.const 17 i64.const 23 i32.const {cond} select");
        value(
            &expression,
            cost(&expression),
            if cond != 0 { 17 } else { 23 },
        );
    }
}
#[test]
fn br_if_carries_the_block_result_when_taken() {
    for cond in [0, 1, -1] {
        // Taken: block, constant, constant, br_if. Not taken: the drop and the
        // second constant run as well; `end` is free.
        let expression =
            format!("(block (result i64) i64.const 17 i32.const {cond} br_if 0 drop i64.const 23)");
        let executed = if cond != 0 {
            cost(&format!("block i64.const 17 i32.const {cond} br_if 0"))
        } else {
            cost(&expression)
        };
        value(&expression, executed, if cond != 0 { 17 } else { 23 });
    }
}
#[test]
fn if_runs_one_arm_and_its_delimiters_cost_nothing() {
    for cond in [0, 1, -1] {
        // The condition, the if and the constant of the arm taken; `else` and
        // `end` are free.
        let executed = if cond != 0 {
            format!("i32.const {cond} if i64.const 17 end")
        } else {
            format!("i32.const {cond} if else i64.const 23 end")
        };
        value(
            &format!("i32.const {cond} (if (result i64) (then i64.const 17) (else i64.const 23))"),
            cost(&executed),
            if cond != 0 { 17 } else { 23 },
        );
    }
}
#[test]
fn br_table_uses_the_default_for_indices_out_of_range() {
    for index in [0, 1, 2, 123_456, -1] {
        // Label 0 leaves the inner block and the outer block adds one: two
        // blocks, two constants, br_table, constant, add. Label 1, whether
        // named or the default, leaves both blocks with 11.
        let head = format!("block block i64.const 11 i32.const {index} br_table 0 1 1");
        let (executed, expected) = if index == 0 {
            (cost(&head) + cost("end i64.const 1 i64.add end"), 12)
        } else {
            (cost(&head), 11)
        };
        value(
            &format!(
                "(block (result i64) (block (result i64) i64.const 11 i32.const {index} br_table 0 1 1) i64.const 1 i64.add)"
            ),
            executed,
            expected,
        );
    }
}
#[test]
fn a_loop_branch_does_not_re_enter_the_loop_opener() {
    // The counter is set and the block and the loop entered once; each round
    // tests the counter, decrements it and branches back to the loop's body,
    // not to its opener; the exit round tests and leaves; then the counter is
    // returned.
    let test = "local.get 0 i32.eqz br_if 1";
    let step = "local.get 0 i32.const 1 i32.sub local.set 0 br 0";
    let tail = "i32.const 0 local.get 0 i32.store i32.const 0 i32.const 4 call $return";
    for n in [0u32, 1, 3, 7] {
        let setup = format!("i32.const {n} local.set 0");
        let text = module(&[
            &import("return"),
            &memory(1, 1),
            &invoke(&format!(
                "(local i32)
    {setup}
    (block (loop
      {test}
      {step}))
    {tail}"
            )),
        ]);
        let rounds = u64::from(n) * (cost(test) + cost(step));
        case(&text)
            .named(format!("countdown from {n}"))
            .call()
            .result(0u32.to_le_bytes())
            .charged(dispatch(
                cost(&setup) + cost("block loop") + rounds + cost(test) + cost(tail),
            ))
            .run();
    }
}
#[test]
fn local_tee_keeps_the_value_on_the_stack() {
    let body = "i32.const 0 i32.const 17 local.tee 0 local.get 0 i32.add i64.extend_i32_u i64.store
    i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &invoke(&format!("(local i32)\n    {body}")),
    ]);
    case(&text)
        .call()
        .result(34u64.to_le_bytes())
        .charged(dispatch(cost(body)))
        .run();
}
#[test]
fn globals_start_fresh_on_every_invocation() {
    // The harness runs an unchanged step three times; the global is 7 again
    // each time, so the result stays 8.
    let body = "i32.const 0 global.get 0 i32.const 1 i32.add global.set 0 global.get 0 i64.extend_i32_u i64.store
    i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        "(global (mut i32) (i32.const 7))",
        &invoke(body),
    ]);
    case(&text)
        .call()
        .result(8u64.to_le_bytes())
        .charged(dispatch(cost(body)))
        .run();
}
#[test]
fn a_function_returns_explicitly_implicitly_or_through_its_label() {
    let caller = "i32.const 0 call $forty_two i64.store i32.const 0 i32.const 8 call $return";
    for exit in ["", "return", "br 0"] {
        // The caller's instructions, a call of a defined function among them,
        // plus the callee's constant and its exit instruction, if any. The
        // callee reserves one slot for its constant.
        let callee = format!("i64.const 42 {exit}");
        let text = module(&[
            &import("return"),
            &memory(1, 1),
            &format!("(func $forty_two (result i64) {callee})"),
            &invoke(caller),
        ]);
        case(&text)
            .named(format!("exit by {exit:?}"))
            .call()
            .result(42u64.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(caller) + cost(&callee),
                reservations: 1,
                ..Default::default()
            })
            .run();
    }
}
#[test]
fn unreachable_traps_after_its_own_dispatch() {
    case(&module(&[&invoke("nop unreachable nop")]))
        .call()
        .expect(Status::Trap)
        .charged(dispatch(cost("nop unreachable")))
        .run();
}
#[test]
fn select_evaluates_both_operands() {
    // The entry's instructions and those of the two calls of `$next`, which
    // reserves two slots (the global and the constant).
    let next = "global.get $g i64.const 1 i64.add global.set $g global.get $g";
    let entry = "i32.const 0 call $next call $next i32.const 0 select i64.store
    i32.const 8 global.get $g i64.store
    i32.const 0 i32.const 16 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        "(global $g (mut i64) (i64.const 0))",
        &format!("(func $next (result i64) {next})"),
        &invoke(entry),
    ]);
    let mut result = 2u64.to_le_bytes().to_vec();
    result.extend(2u64.to_le_bytes());
    case(&text)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(entry) + 2 * cost(next),
            reservations: 4,
            ..Default::default()
        })
        .run();
}

/// The bodies of the two functions in `table_program`'s table.
const TEN: &str = "i64.const 10";
const WIDEN: &str = "local.get 0 i64.extend_i32_u";
/// Two functions behind a three-entry table whose last entry is null.
fn table_program(entry: &str) -> String {
    module(&[
        "(type $none_i64 (func (result i64)))",
        "(type $i32_i64 (func (param i32) (result i64)))",
        &import("return"),
        &memory(1, 1),
        &format!("(func $ten (type $none_i64) {TEN})"),
        &format!("(func $widen (type $i32_i64) {WIDEN})"),
        "(table 3 3 funcref)",
        "(elem (i32.const 0) $ten $widen)",
        &invoke(entry),
    ])
}
#[test]
fn indirect_calls_check_the_entry_and_the_type_before_reserving() {
    // The caller's instructions, the indirect call among them, and the
    // callee's constant; `$ten` reserves one slot.
    let entry = "i32.const 0 i32.const 0 call_indirect (type $none_i64) i64.store i32.const 0 i32.const 8 call $return";
    case(&table_program(entry))
        .named("entry 0")
        .call()
        .result(10u64.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(entry) + cost(TEN),
            reservations: 1,
            ..Default::default()
        })
        .run();
    // A type mismatch, a null entry and an index past the table each trap
    // after the two constants and the call's own price, reserving nothing.
    for (index, reason) in [(1, "type mismatch"), (2, "null entry"), (3, "out of range")] {
        case(&table_program(&format!(
            "i32.const 0 i32.const {index} call_indirect (type $none_i64) i64.store i32.const 0 i32.const 8 call $return"
        )))
        .named(reason)
        .call()
        .expect(Status::Trap)
        .charged(dispatch(cost(&format!(
            "i32.const 0 i32.const {index} call_indirect"
        ))))
        .run();
    }
    // The caller's instructions and the two of `$widen`, which reserves its
    // parameter and one operand.
    let entry = "i32.const 0 i32.const 7 i32.const 1 call_indirect (type $i32_i64) i64.store i32.const 0 i32.const 8 call $return";
    case(&table_program(entry))
        .named("parameter")
        .call()
        .result(7u64.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(entry) + cost(WIDEN),
            reservations: 2,
            ..Default::default()
        })
        .run();
}
#[test]
fn an_imported_function_in_the_table_pays_the_host_call_and_reserves_nothing() {
    // An indirect call costs the price of `call_indirect`; a host import
    // reached through the table then pays the host call's 94, where a
    // defined function's reservation would be, and charges its own bytes.
    let entry = "i32.const 0 i32.const 8 i32.const 0 call_indirect (type $span)";
    let text = module(&[
        "(type $span (func (param i32 i32)))",
        "(import \"ting\" \"return\" (func $return (type $span)))",
        &memory(1, 1),
        "(table 1 1 funcref)",
        "(elem (i32.const 0) $return)",
        &format!("(data (i32.const 0) {})", escaped(b"indirect")),
        &invoke(entry),
    ]);
    case(&text)
        .call()
        .result(b"indirect")
        .charged(dispatch(cost(entry) + 94))
        .run();
}

#[test]
fn memory_fill_and_copy_charge_their_bytes_per_word() {
    let body = "i32.const 0 i32.const 171 i32.const 1000 memory.fill
    i32.const 2000 i32.const 0 i32.const 1000 memory.copy
    i32.const 2992 i32.const 8 call $return";
    let text = module(&[&import("return"), &memory(1, 1), &invoke(body)]);
    case(&text)
        .call()
        .result([171u8; 8])
        .charged(Ledger {
            dispatch: cost(body),
            copies: 2 * words(1000),
            ..Default::default()
        })
        .run();
}
#[test]
fn memory_copy_handles_overlap_in_both_directions() {
    let body =
        "i32.const 1 i32.const 0 i32.const 8 memory.copy i32.const 0 i32.const 9 call $return";
    let forward = module(&[
        &import("return"),
        &memory(1, 1),
        &format!(
            "(data (i32.const 0) {})",
            escaped(&[1, 2, 3, 4, 5, 6, 7, 8])
        ),
        &invoke(body),
    ]);
    case(&forward)
        .named("forward")
        .call()
        .result([1, 1, 2, 3, 4, 5, 6, 7, 8])
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(8),
            ..Default::default()
        })
        .run();
    let body =
        "i32.const 0 i32.const 1 i32.const 8 memory.copy i32.const 0 i32.const 9 call $return";
    let backward = module(&[
        &import("return"),
        &memory(1, 1),
        &format!(
            "(data (i32.const 0) {})",
            escaped(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
        ),
        &invoke(body),
    ]);
    case(&backward)
        .named("backward")
        .call()
        .result([2, 3, 4, 5, 6, 7, 8, 9, 9])
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(8),
            ..Default::default()
        })
        .run();
}
#[test]
fn bulk_operations_at_the_end_of_memory() {
    for op in ["memory.copy", "memory.fill"] {
        // A zero-length range at the very end is in bounds and its bytes are
        // free.
        let body = format!(
            "i32.const 65536 i32.const 0 i32.const 0 {op} i32.const 0 i32.const 0 call $return"
        );
        case(&module(&[&import("return"), &memory(1, 1), &invoke(&body)]))
            .named(format!("{op} of nothing at the end"))
            .call()
            .charged(dispatch(cost(&body)))
            .run();
        // One byte past the end traps after the price and the byte debit.
        let body = format!("i32.const 65536 i32.const 0 i32.const 1 {op}");
        case(&module(&[&memory(1, 1), &invoke(&body)]))
            .named(format!("{op} past the end"))
            .call()
            .expect(Status::Trap)
            .charged(Ledger {
                dispatch: cost(&body),
                copies: words(1),
                ..Default::default()
            })
            .run();
    }
    let body = "i32.const 0 i32.const 65536 i32.const 1 memory.copy";
    case(&module(&[&memory(1, 1), &invoke(body)]))
        .named("source past the end")
        .call()
        .expect(Status::Trap)
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(1),
            ..Default::default()
        })
        .run();
    let body =
        "i32.const 0 i32.const 65535 i32.const 1 memory.copy i32.const 0 i32.const 1 call $return";
    case(&module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(data (i32.const 65535) {})", escaped(b"e")),
        &invoke(body),
    ]))
    .named("the last byte")
    .call()
    .result(b"e")
    .charged(Ledger {
        dispatch: cost(body),
        copies: words(1),
        ..Default::default()
    })
    .run();
    let body =
        "i32.const 0 i32.const 0 i32.const 65536 memory.copy i32.const 0 i32.const 0 call $return";
    case(&module(&[&import("return"), &memory(1, 1), &invoke(body)]))
        .named("a whole page onto itself")
        .call()
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(65536),
            ..Default::default()
        })
        .run();
}

// ---- Evaluation order --------------------------------------------------------------------

/// The body of `$mark`, which appends its argument to a log at 1024 (the
/// count, then the entries) and returns it, reserving four slots.
const MARK_BODY: &str =
    "i32.const 1028 i32.const 1024 i32.load i32.const 4 i32.mul i32.add local.get 0 i32.store
    i32.const 1024 i32.const 1024 i32.load i32.const 1 i32.add i32.store
    local.get 0";
const MARK_SLOTS: u64 = 4;
fn mark() -> String {
    format!("(func $mark (param i32) (result i32)\n    {MARK_BODY})")
}
/// Runs `instructions`, which call `$mark` `marks.len()` times, and expects
/// the log to show `marks` in that order. Every instruction runs once, so
/// the dispatch is the price of `instructions` (a leading local declaration
/// is no instruction), of the marks' bodies and of the final return.
fn in_order(parts: &[&str], instructions: &str, marks: &[u32], extra: Ledger) {
    let tail = format!(
        "i32.const 1024 i32.const {} call $return",
        4 + 4 * marks.len()
    );
    let mut all = vec![import("return"), import("emit"), memory(1, 1), mark()];
    all.extend(parts.iter().map(|p| p.to_string()));
    all.push(invoke(&format!("{instructions} {tail}")));
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let mut log = (marks.len() as u32).to_le_bytes().to_vec();
    for m in marks {
        log.extend(m.to_le_bytes());
    }
    let n = marks.len() as u64;
    let executed = instructions.trim_start_matches("(local i32) ");
    case(&module(&all))
        .named(instructions)
        .call()
        .result(log)
        .charged(Ledger {
            dispatch: cost(executed) + n * cost(MARK_BODY) + cost(&tail),
            reservations: n * MARK_SLOTS + extra.reservations,
            copies: extra.copies,
            ..Default::default()
        })
        .run();
}
#[test]
fn operands_are_evaluated_left_to_right() {
    let plain = Ledger::default();
    in_order(
        &[],
        "(drop (i32.sub (call $mark (i32.const 1)) (call $mark (i32.const 2))))",
        &[1, 2],
        plain,
    );
    in_order(
        &[],
        "(drop (i32.add (i32.mul (call $mark (i32.const 1)) (call $mark (i32.const 2))) (call $mark (i32.const 3))))",
        &[1, 2, 3],
        plain,
    );
    in_order(
        &[],
        "(drop (select (call $mark (i32.const 1)) (call $mark (i32.const 2)) (call $mark (i32.const 3))))",
        &[1, 2, 3],
        plain,
    );
    in_order(
        &[],
        "(i32.store (call $mark (i32.const 1)) (call $mark (i32.const 2)))",
        &[1, 2],
        plain,
    );
    in_order(
        &[],
        "(drop (i32.load (call $mark (i32.const 100))))",
        &[100],
        plain,
    );
    in_order(
        &["(func $three (param i32 i32 i32))"],
        "(call $three (call $mark (i32.const 1)) (call $mark (i32.const 2)) (call $mark (i32.const 3)))",
        &[1, 2, 3],
        Ledger {
            reservations: 3,
            ..Default::default()
        },
    );
    // The arguments, then the table index.
    in_order(
        &[
            "(type $t3 (func (param i32 i32 i32)))",
            "(func $three (type $t3))",
            "(table 1 1 funcref)",
            "(elem (i32.const 0) $three)",
        ],
        "(call_indirect (type $t3) (call $mark (i32.const 1)) (call $mark (i32.const 2)) (call $mark (i32.const 3)) (call $mark (i32.const 0)))",
        &[1, 2, 3, 0],
        Ledger {
            reservations: 3,
            ..Default::default()
        },
    );
    in_order(
        &[],
        "(memory.copy (call $mark (i32.const 8)) (call $mark (i32.const 0)) (call $mark (i32.const 4)))",
        &[8, 0, 4],
        Ledger {
            copies: words(4),
            ..Default::default()
        },
    );
    in_order(
        &[],
        "(memory.fill (call $mark (i32.const 16)) (call $mark (i32.const 7)) (call $mark (i32.const 4)))",
        &[16, 7, 4],
        Ledger {
            copies: words(4),
            ..Default::default()
        },
    );
    // The label value, then the condition.
    in_order(
        &[],
        "(drop (block (result i32) (br_if 0 (call $mark (i32.const 1)) (call $mark (i32.const 2)))))",
        &[1, 2],
        plain,
    );
    in_order(
        &[],
        "(local i32) (local.set 0 (call $mark (i32.const 5))) (i32.store (local.get 0) (call $mark (i32.const 6)))",
        &[5, 6],
        plain,
    );
}
#[test]
fn host_arguments_are_evaluated_left_to_right() {
    // emit's span: the pointer, then the length; the event shows three zero bytes.
    let body = "(call $emit (call $mark (i32.const 2048)) (call $mark (i32.const 3))) i32.const 1024 i32.const 12 call $return";
    let text = module(&[
        &import("return"),
        &import("emit"),
        &memory(1, 1),
        &mark(),
        &invoke(body),
    ]);
    let mut log = 2u32.to_le_bytes().to_vec();
    log.extend(2048u32.to_le_bytes());
    log.extend(3u32.to_le_bytes());
    case(&text)
        .call()
        .result(log)
        .events([[0u8; 3]])
        .charged(Ledger {
            dispatch: cost(body) + 2 * cost(MARK_BODY),
            reservations: 2 * MARK_SLOTS,
            ..Default::default()
        })
        .run();
}

#[test]
fn parameters_arrive_in_the_order_they_were_pushed() {
    let sub_body = "local.get 0 local.get 1 i32.sub";
    let sub = format!("(func $sub (param i32 i32) (result i32) {sub_body})");
    // Direct: 10 - 3. The callee runs its three instructions and reserves its
    // two parameters and two operands.
    let entry = "i32.const 0 i32.const 10 i32.const 3 call $sub i64.extend_i32_u i64.store i32.const 0 i32.const 8 call $return";
    let text = module(&[&import("return"), &memory(1, 1), &sub, &invoke(entry)]);
    case(&text)
        .named("direct")
        .call()
        .result(7u64.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(entry) + cost(sub_body),
            reservations: 4,
            ..Default::default()
        })
        .run();
    // Indirect: the arguments, then the index.
    let entry = "i32.const 0 i32.const 10 i32.const 3 i32.const 0 call_indirect (type $two) i64.extend_i32_u i64.store i32.const 0 i32.const 8 call $return";
    let text = module(&[
        "(type $two (func (param i32 i32) (result i32)))",
        &import("return"),
        &memory(1, 1),
        &format!("(func $sub (type $two) {sub_body})"),
        "(table 1 1 funcref)",
        "(elem (i32.const 0) $sub)",
        &invoke(entry),
    ]);
    case(&text)
        .named("indirect")
        .call()
        .result(7u64.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(entry) + cost(sub_body),
            reservations: 4,
            ..Default::default()
        })
        .run();
    // Through a second level that swaps them: 3 - 10.
    let swapped_body = "local.get 1 local.get 0 call $sub";
    let entry = "i32.const 0 i32.const 10 i32.const 3 call $swapped i64.extend_i32_u i64.store i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &sub,
        &format!("(func $swapped (param i32 i32) (result i32) {swapped_body})"),
        &invoke(entry),
    ]);
    case(&text)
        .named("swapped through a second level")
        .call()
        .result(((3i32 - 10) as u32 as u64).to_le_bytes())
        .charged(Ledger {
            dispatch: cost(entry) + cost(swapped_body) + cost(sub_body),
            reservations: 8,
            ..Default::default()
        })
        .run();
    // Three parameters of two widths: (5 << 3) + 1.
    let mix_body =
        "local.get 1 local.get 0 i64.extend_i32_u i64.shl local.get 2 i64.extend_i32_u i64.add";
    let entry = "i32.const 0 i32.const 3 i64.const 5 i32.const 1 call $mix i64.store i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(func $mix (param i32 i64 i32) (result i64) {mix_body})"),
        &invoke(entry),
    ]);
    case(&text)
        .named("three parameters of two widths")
        .call()
        .result(41u64.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(entry) + cost(mix_body),
            reservations: 5,
            ..Default::default()
        })
        .run();
}

// ---- Unwinding -----------------------------------------------------------------------------

#[test]
fn a_branch_carries_the_top_value_and_discards_the_rest() {
    // Every instruction runs.
    let expression = "(block (result i64) i64.const 1 i64.const 2 i64.const 3 br 0)";
    value(expression, cost(expression), 3);
    for (cond, expected) in [(1, 9), (0, 7)] {
        // Taken: the branch carries 9 over the i32 and the 7. Not taken: the
        // two drops run and leave the 7.
        let expression = format!(
            "(block (result i64) i64.const 7 i32.const 1 i64.const 9 i32.const {cond} br_if 0 drop drop)"
        );
        let executed = if cond != 0 {
            cost(&format!(
                "block i64.const 7 i32.const 1 i64.const 9 i32.const {cond} br_if 0"
            ))
        } else {
            cost(&expression)
        };
        value(&expression, executed, expected);
    }
    // Label 0 leaves the inner block with 20; every instruction runs.
    let expression = "(block (result i64) (block (result i64) i64.const 10 i64.const 20 i32.const 0 br_table 0 1) i64.const 1 i64.add)";
    value(expression, cost(expression), 21);
    // The loop is entered once; the branch to the block leaves it.
    let expression = "(block (result i64) (loop (result i64) i64.const 5 i64.const 6 br 1))";
    value(expression, cost(expression), 6);
    for (cond, expected) in [(1, 12), (0, 11)] {
        // Taken: the branch carries 12 out of both blocks. Not taken: the
        // drop runs and the outer block ends with 11.
        let expression = format!(
            "(block (result i64) i64.const 11 (block i64.const 12 i32.const {cond} br_if 1 drop))"
        );
        let executed = if cond != 0 {
            cost(&format!(
                "block i64.const 11 block i64.const 12 i32.const {cond} br_if 1"
            ))
        } else {
            cost(&expression)
        };
        value(&expression, executed, expected);
    }
    // From the then arm the branch carries 3 over the 1 and the 2, skipping
    // the addition; the else arm falls through to the addition.
    for (cond, expected) in [(1, 3), (0, 5)] {
        let executed = if cond != 0 {
            format!("block i64.const 1 i32.const {cond} if i64.const 2 i64.const 3 br 1")
        } else {
            format!("block i64.const 1 i32.const {cond} if else i64.const 4 end i64.add end")
        };
        value(
            &format!(
                "(block (result i64) i64.const 1 i32.const {cond} (if (result i64) (then i64.const 2 i64.const 3 br 1) (else i64.const 4)) i64.add)"
            ),
            cost(&executed),
            expected,
        );
    }
}
#[test]
fn a_function_returns_its_top_value_and_discards_the_rest() {
    let caller = "i32.const 0 call $f i64.store i32.const 0 i32.const 8 call $return";
    for (exit, name) in [("return", "return"), ("br 0", "the function label")] {
        // The caller's instructions and the callee's two constants and its
        // exit, which carries the 42 over the 1.
        let callee = format!("i64.const 1 i64.const 42 {exit}");
        let text = module(&[
            &import("return"),
            &memory(1, 1),
            &format!("(func $f (result i64) {callee})"),
            &invoke(caller),
        ]);
        case(&text)
            .named(name)
            .call()
            .result(42u64.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(caller) + cost(&callee),
                reservations: 2,
                ..Default::default()
            })
            .run();
    }
}
#[test]
fn a_br_table_switch_falls_through_the_blocks_it_does_not_leave() {
    // Label 0 is innermost; each block the branch does not leave adds its
    // number on the way out, one constant and one addition each.
    for (index, expected, additions) in [
        (0, 110, 4),
        (1, 109, 3),
        (2, 107, 2),
        (3, 104, 1),
        (4, 100, 0),
        (5, 100, 0),
        (-1, 100, 0),
    ] {
        let head = format!(
            "block block block block block i64.const 100 i32.const {index} br_table 0 1 2 3 4"
        );
        value(
            &format!(
                "(block (result i64) (block (result i64) (block (result i64) (block (result i64) (block (result i64)
      i64.const 100 i32.const {index} br_table 0 1 2 3 4)
      i64.const 1 i64.add) i64.const 2 i64.add) i64.const 3 i64.add) i64.const 4 i64.add)"
            ),
            cost(&head) + additions * cost("i64.const 1 i64.add"),
            expected,
        );
    }
}
