//! Generated programs (seed section S9): seeded random programs of
//! statements over four i64 locals, a mutable global, a data region of
//! memory, storage, events and calls of a child, with counted loops around
//! them. The generator states each program's outcome by running a model of it
//! in Rust: the expressions with `compositions`' evaluator, the statements by
//! their rules, and every charge on the path as a ledger the model turns into
//! counters. A trap part way through keeps what was charged before it and
//! rolls back the records and the events.
use crate::{
    compositions::{Env, Expr, Rng, Ty, eval, generate, text},
    dsl::{escaped, import, invoke, memory, module},
    harness::{CHILD, deployed as case, deployed_entry_of, deployed_shape, deployed_warm_entry_of},
};
use std::collections::BTreeMap;
use ting_core::protocol::Status;
use vm_model::{EVENT_BASE, Ledger, RECORD_BASE, SMALL, cost, leaf, words};

/// Where things are in memory: the child's identifier and the call's budget,
/// the storage key and value, the call's input and result, the final result,
/// and the data region the expressions, stores, copies and fills use.
const BUDGET_AT: u32 = 32;
const KEY_AT: u32 = 64;
const VALUE_AT: u32 = 96;
const INPUT_AT: u32 = 104;
const RESULT_AT: u32 = 112;
const RETURN_AT: u32 = 1024;
const DATA: u32 = 2048;
const DATA_LEN: u32 = 256;

/// The child returns its eight-byte input plus one.
const CHILD_BODY: &str = "i32.const 0 i32.const 8 call $input drop
    i32.const 0 i32.const 0 i64.load i64.const 1 i64.add i64.store
    i32.const 0 i32.const 8 call $return";
fn child() -> String {
    module(&[
        &import("input"),
        &import("return"),
        &memory(1, 1),
        &invoke(CHILD_BODY),
    ])
}

fn child_charges() -> [u64; 2] {
    static CHARGES: std::sync::OnceLock<[u64; 2]> = std::sync::OnceLock::new();
    *CHARGES.get_or_init(|| {
        let run = SMALL.bytes(8) + cost(CHILD_BODY) + SMALL.bytes(8) + SMALL.bytes(8);
        [
            deployed_entry_of(&child()) + run,
            deployed_warm_entry_of(&child()) + run,
        ]
    })
}

enum Statement {
    SetLocal(usize, Expr),
    Store(u32, Expr),
    SetGlobal(Expr),
    Write(u8, usize),
    Read(u8, usize),
    Delete(u8),
    Emit(usize),
    Call(usize),
    Copy(u32, u32, u32),
    Fill(u32, u8, u32),
    Loop(u32, Vec<Statement>),
}

/// Eight-aligned addresses of the data region, for loads and stores.
fn addresses() -> Vec<u32> {
    (0..DATA_LEN / 8 - 1).map(|i| DATA + 8 * i).collect()
}
fn statement(rng: &mut Rng, in_loop: bool) -> Statement {
    let addresses = addresses();
    let local = |rng: &mut Rng| rng.below(4) as usize;
    let slot = |rng: &mut Rng| rng.below(4) as u8;
    let expr = |rng: &mut Rng| generate(rng, Ty::I64, 1, Some(&addresses));
    match rng.below(if in_loop { 100 } else { 110 }) {
        0..=24 => Statement::SetLocal(local(rng), expr(rng)),
        25..=39 => Statement::Store(*rng.pick(&addresses), expr(rng)),
        40..=47 => Statement::SetGlobal(expr(rng)),
        48..=57 => Statement::Write(slot(rng), local(rng)),
        58..=63 => Statement::Read(slot(rng), local(rng)),
        64..=67 => Statement::Delete(slot(rng)),
        68..=75 => Statement::Emit(local(rng)),
        76..=85 => Statement::Call(local(rng)),
        86..=92 => {
            let len = rng.below(DATA_LEN as u64 / 2) as u32;
            let dst = DATA + rng.below(u64::from(DATA_LEN - len)) as u32;
            let src = DATA + rng.below(u64::from(DATA_LEN - len)) as u32;
            Statement::Copy(dst, src, len)
        }
        93..=99 => {
            let len = rng.below(DATA_LEN as u64 / 2) as u32;
            let dst = DATA + rng.below(u64::from(DATA_LEN - len)) as u32;
            Statement::Fill(dst, rng.next() as u8, len)
        }
        _ => {
            let count = 1 + rng.below(4) as u32;
            let body = (0..1 + rng.below(3))
                .map(|_| statement(rng, true))
                .collect();
            Statement::Loop(count, body)
        }
    }
}

/// The statement as WebAssembly text; `label` numbers the loops.
fn write(s: &Statement, label: &mut u32) -> String {
    match s {
        Statement::SetLocal(k, e) => format!("(local.set $l{k} {})", text(e)),
        Statement::Store(a, e) => format!("(i64.store (i32.const {a}) {})", text(e)),
        Statement::SetGlobal(e) => format!("(global.set $g {})", text(e)),
        Statement::Write(slot, k) => format!(
            "i32.const {KEY_AT} i32.const {slot} i32.store8
    i32.const {VALUE_AT} local.get $l{k} i64.store
    i32.const {KEY_AT} i32.const {VALUE_AT} i32.const 8 call $storage_set"
        ),
        Statement::Read(slot, k) => format!(
            "i32.const {KEY_AT} i32.const {slot} i32.store8
    i32.const {VALUE_AT} i64.const 0 i64.store
    i32.const {KEY_AT} i32.const {VALUE_AT} i32.const 8 call $storage_get drop
    i32.const {VALUE_AT} i64.load local.set $l{k}"
        ),
        Statement::Delete(slot) => format!(
            "i32.const {KEY_AT} i32.const {slot} i32.store8
             i32.const {KEY_AT} call $storage_delete"
        ),
        Statement::Emit(k) => format!(
            "i32.const {VALUE_AT} local.get $l{k} i64.store
    i32.const {VALUE_AT} i32.const 8 call $emit"
        ),
        Statement::Call(k) => format!(
            "i32.const {INPUT_AT} local.get $l{k} i64.store
    i32.const 0 i32.const {INPUT_AT} i32.const 8 i32.const {BUDGET_AT} i32.const 0 call $call drop
    i32.const {RESULT_AT} i32.const 8 call $call_result drop
    i32.const {RESULT_AT} i64.load local.set $l{k}
    i32.const {BUDGET_AT} i64.const {} i64.store",
            child_charges()[1]
        ),
        Statement::Copy(d, s, n) => {
            format!("i32.const {d} i32.const {s} i32.const {n} memory.copy")
        }
        Statement::Fill(d, b, n) => {
            format!("i32.const {d} i32.const {b} i32.const {n} memory.fill")
        }
        Statement::Loop(count, body) => {
            *label += 1;
            let l = *label;
            let body: Vec<String> = body.iter().map(|s| write(s, label)).collect();
            format!(
                "i32.const 0 local.set $n
    (loop $loop{l}
      {}
      local.get $n i32.const 1 i32.add local.tee $n i32.const {count} i32.lt_u br_if $loop{l})",
                body.join("\n      ")
            )
        }
    }
}

/// The model's state while it runs a program.
struct Model {
    env: Env,
    storage: BTreeMap<u8, u64>,
    events: Vec<[u8; 8]>,
    /// The instructions executed, as text, and every other charge.
    path: Vec<String>,
    ledger: Ledger,
    /// The child's run with its entry cold, as the first invocation of its
    /// code in the transaction, and warm, as every later one.
    child: [u64; 2],
    called: bool,
    label: u32,
}
impl Model {
    fn data(&mut self, at: u32, bytes: &[u8]) {
        self.env.memory[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    }
    /// Runs `s`; `Err` when it traps, its path then ending with the trap.
    fn run(&mut self, s: &Statement) -> Result<(), ()> {
        match s {
            Statement::SetLocal(k, e) => {
                let v = self.eval(e)?;
                self.path.push(format!("local.set $l{k}"));
                self.env.locals[*k] = v;
            }
            Statement::Store(a, e) => {
                self.path.push(format!("i32.const {a}"));
                let v = self.eval(e)?;
                self.path.push("i64.store".into());
                self.data(*a, &v.to_le_bytes());
            }
            Statement::SetGlobal(e) => {
                let v = self.eval(e)?;
                self.path.push("global.set $g".into());
                self.env.global = v;
            }
            Statement::Write(slot, k) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                // The key and the value copied, the old record read with its
                // key, the value's leaf, two accesses, and growth for a new
                // record of eight bytes.
                self.ledger.copies += words(32 + 8);
                self.ledger.hashing += leaf(8);
                self.ledger.reads += 1;
                self.ledger.accesses += 2;
                if !self.storage.contains_key(slot) {
                    self.ledger.growth += RECORD_BASE + 8;
                }
                let v = self.env.locals[*k];
                self.data(VALUE_AT, &v.to_le_bytes());
                self.storage.insert(*slot, v);
            }
            Statement::Read(slot, k) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                // The key copied and the record read with its key; a present
                // value copied out.
                let value = self.storage.get(slot).copied();
                self.ledger.copies += words(32) + if value.is_some() { words(8) } else { 0 };
                self.ledger.reads += 1;
                self.ledger.accesses += 1;
                let v = value.unwrap_or(0);
                self.data(VALUE_AT, &v.to_le_bytes());
                self.env.locals[*k] = v;
            }
            Statement::Emit(k) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                let v = self.env.locals[*k].to_le_bytes();
                self.data(VALUE_AT, &v);
                self.events.push(v);
            }
            Statement::Delete(slot) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                self.ledger.copies += words(32);
                self.ledger.reads += 1;
                self.ledger.accesses += 2;
                self.storage.remove(slot);
            }
            Statement::Call(k) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                // The target and the budget copied; the child, entered with
                // its gate, receives the input, copies it in, adds and returns
                // eight bytes, which the parent copies out and the receipt
                // does not show.
                self.ledger.copies += words(32) + words(32) + words(8);
                self.ledger.nested += self.child[usize::from(self.called)];
                self.called = true;
                self.ledger.accesses += 2;
                self.ledger.discarded += 8;
                let v = self.env.locals[*k];
                self.data(INPUT_AT, &v.to_le_bytes());
                let r = v.wrapping_add(1);
                self.data(RESULT_AT, &r.to_le_bytes());
                self.data(BUDGET_AT, &self.child[1].to_le_bytes());
                self.env.locals[*k] = r;
            }
            Statement::Copy(d, src, n) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                self.ledger.copies += words(u64::from(*n));
                let (d, src, n) = (*d as usize, *src as usize, *n as usize);
                self.env.memory.copy_within(src..src + n, d);
            }
            Statement::Fill(d, b, n) => {
                let mut label = 0;
                self.path.push(write(s, &mut label));
                self.ledger.copies += words(u64::from(*n));
                let (d, n) = (*d as usize, *n as usize);
                self.env.memory[d..d + n].fill(*b);
            }
            Statement::Loop(count, body) => {
                self.label += 1;
                let l = self.label;
                self.path.push("i32.const 0 local.set $n loop".into());
                for _ in 0..*count {
                    for s in body {
                        self.run(s)?;
                    }
                    self.path.push(format!(
                        "local.get $n i32.const 1 i32.add local.tee $n i32.const {count} i32.lt_u br_if $loop{l}"
                    ));
                }
            }
        }
        Ok(())
    }
    fn eval(&mut self, e: &Expr) -> Result<u64, ()> {
        match eval(e, &self.env) {
            Ok((v, path)) => {
                self.path.push(path);
                Ok(v)
            }
            Err(path) => {
                self.path.push(path);
                Err(())
            }
        }
    }
}

#[test]
pub(crate) fn seeded_programs_of_statements_run_as_their_model_says() {
    let child = child();
    let child_run = child_charges();
    let mut initial = vec![0u8; 65_536];
    // Exact child budgets keep the independently modeled path admissible
    // when the parent's own budget is sliced tightly. Later calls replace C
    // with the warm cost; a roomy fixed request would instead be refused.
    let budget: Vec<u8> = [child_run[0], 2, 8, 0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    initial[BUDGET_AT as usize..KEY_AT as usize].copy_from_slice(&budget);
    let (mut traps, mut loops, mut calls) = (0, 0, 0);
    for seed in 1..=192u64 {
        let mut rng = Rng(seed.wrapping_mul(0xd1b5_4a32_d192_ed03) | 1);
        // The locals start from random odd values, so that fewer divisions
        // meet a zero, then come the statements.
        let mut program: Vec<Statement> = (0..4)
            .map(|k| Statement::SetLocal(k, Expr::Const(Ty::I64, rng.next() | 1)))
            .collect();
        program.extend((0..4 + rng.below(7)).map(|_| statement(&mut rng, false)));
        let mut label = 0;
        let statements: Vec<String> = program.iter().map(|s| write(s, &mut label)).collect();
        let text = module(&[
            &import("input"),
            &import("storage_set"),
            &import("storage_get"),
            &import("storage_delete"),
            &import("emit"),
            &import("call"),
            &import("call_result"),
            &import("return"),
            &memory(1, 1),
            "(global $g (mut i64) (i64.const 0))",
            &format!("(data (i32.const 0) {})", escaped(&CHILD)),
            &format!("(data (i32.const {BUDGET_AT}) {})", escaped(&budget)),
            &invoke(&format!(
                "(local $l0 i64) (local $l1 i64) (local $l2 i64) (local $l3 i64) (local $n i32)
    {}
    i32.const {RETURN_AT} local.get $l0 i64.store
    i32.const {RETURN_AT} i32.const 8 call $return",
                statements.join("\n    ")
            )),
        ]);
        loops += usize::from(text.contains("(loop "));
        calls += usize::from(text.contains("call $call "));
        let mut model = Model {
            env: Env {
                locals: [0; 4],
                global: 0,
                memory: initial.clone(),
            },
            storage: BTreeMap::new(),
            events: Vec::new(),
            path: Vec::new(),
            ledger: Ledger::default(),
            child: child_run,
            called: false,
            label: 0,
        };
        let trapped = program.iter().try_for_each(|s| model.run(s)).is_err();
        let c = case(&text)
            .named(format!("program {seed}"))
            .with_program(CHILD, &child)
            .call();
        if trapped {
            traps += 1;
            // The events were copied and their data debited, then rolled
            // back with the records; everything charged before the trap
            // stays.
            let mut ledger = model.ledger;
            ledger.dispatch = cost(&model.path.join(" "));
            ledger.copies += model.events.len() as u64 * words(8);
            ledger.discarded += model.events.len() as u64 * (EVENT_BASE + 8);
            c.expect(Status::Trap).charged(ledger).run();
        } else {
            let finish = format!(
                "i32.const {RETURN_AT} local.get $l0 i64.store i32.const {RETURN_AT} i32.const 8 call $return"
            );
            let mut ledger = model.ledger;
            ledger.dispatch = cost(&format!("{} {finish}", model.path.join(" ")));
            let records: Vec<([u8; 32], [u8; 8])> = model
                .storage
                .iter()
                .map(|(slot, v)| {
                    let mut key = [0u8; 32];
                    key[0] = *slot;
                    (key, v.to_le_bytes())
                })
                .collect();
            c.result(model.env.locals[0].to_le_bytes())
                .events(&model.events)
                .records(records.clone())
                .charged(ledger)
                .run();
            // Slice the final atomic result debit, whose complete preceding
            // path the model has derived. C and D shortages roll back all
            // writes/events, while keeping their earlier charges. A simultaneous
            // shortage must choose Compute. Exact C and D must still succeed.
            let event_sizes = vec![8u64; model.events.len()];
            let needed = vm_model::call(
                0,
                deployed_shape(&text).entry(),
                Ledger {
                    dispatch: ledger.dispatch + cost(crate::gate::ON_CALL),
                    ..ledger
                },
                8,
                &event_sizes,
            );
            for (name, c_short, d_short) in [
                ("exact", false, false),
                ("C-short", true, false),
                ("D-short", false, true),
                ("C-and-D-short", true, true),
            ] {
                let mut budget = vm_model::BUDGET;
                budget[0] = needed[0] - u64::from(c_short);
                budget[2] = needed[2] - u64::from(d_short);
                let test = case(&text)
                    .named(format!("program {seed}/{name}"))
                    .with_program(CHILD, &child)
                    .call()
                    .budget(budget);
                if c_short || d_short {
                    let mut failed = ledger;
                    failed.copies += model.events.len() as u64 * words(8);
                    failed.discarded += model.events.len() as u64 * (EVENT_BASE + 8);
                    test.expect(if c_short {
                        Status::Compute
                    } else {
                        Status::Data
                    })
                    .charged(failed)
                    .run();
                } else {
                    test.result(model.env.locals[0].to_le_bytes())
                        .events(&model.events)
                        .records(records.clone())
                        .charged(ledger)
                        .run();
                }
            }
        }
    }
    assert!(
        traps > 0 && loops > 0 && calls > 0,
        "{traps} traps, {loops} loops, {calls} calls"
    );
}
