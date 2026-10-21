//! Long, ordinary on-chain histories. A plain Rust map states all records,
//! read results and gross growth; failed steps keep their debit ledger.
use crate::{
    dsl::{import, invoke, memory, module},
    harness::deployed as case,
};
use std::collections::BTreeMap;
use ting_core::protocol::Status;
use vm_model::{Ledger, RECORD_BASE, cost, leaf, words};

const PREFIX: &str = "i32.const 64 i32.const 16 call $input drop
    i32.const 64 i32.load local.set $mode
    i32.const 68 i32.load local.set $i
    i32.const 72 i32.load local.get $i i32.add local.set $stop
    i32.const 76 i32.load local.set $len loop $again";
const KEY: &str = "i32.const 0 local.get $i i32.store
    local.get $mode i32.const 1 i32.eq if";
const DELETE: &str = "i32.const 0 call $storage_delete";
const READ_TEST: &str = "local.get $mode i32.const 2 i32.eq if";
const READ: &str = "i32.const 256 i32.const 0 i32.store8
    local.get $lengths i32.const 0 i32.const 256 i32.const 128 call $storage_get
    i64.extend_i32_u i64.add local.set $lengths
    local.get $values i32.const 256 i32.load8_u i64.extend_i32_u i64.add local.set $values";
// Multiples of four avoid the harness's reserved 0x11/0x12/0x13 symbolic
// identifiers appearing as a run of 32 bytes in a declared record value.
const WRITE: &str = "i32.const 256 local.get $i i32.const 4 i32.mul local.get $len memory.fill
    i32.const 0 i32.const 256 local.get $len call $storage_set";
const NEXT: &str =
    "local.get $i i32.const 1 i32.add local.tee $i local.get $stop i32.lt_u br_if $again";
const RESULTS: &str = "i32.const 512 local.get $lengths i64.store
    i32.const 520 local.get $values i64.store
    local.get $mode i32.const 3 i32.eq if";
const REVERT: &str = "i32.const 512 i32.const 16 call $revert";
const TRAP_TEST: &str = "local.get $mode i32.const 4 i32.eq if";
const RETURN: &str = "i32.const 512 i32.const 16 call $return";

#[test]
pub(crate) fn thousands_of_records_survive_churn_failed_writes_and_later_reads() {
    let body = format!(
        "(local $mode i32) (local $i i32) (local $stop i32) (local $len i32)
        (local $lengths i64) (local $values i64)
        {PREFIX} {KEY} {DELETE} else {READ_TEST} {READ} else {WRITE} end end
        {NEXT} end {RESULTS} {REVERT} end {TRAP_TEST} unreachable end {RETURN}"
    );
    let text = module(&[
        &import("input"),
        &import("storage_set"),
        &import("storage_get"),
        &import("storage_delete"),
        &import("return"),
        &import("revert"),
        &memory(1, 1),
        &invoke(&body),
    ]);
    // (mode, first logical key, number of keys, value length). Writes are
    // bounded by S rather than squeezing setup and observation into one block.
    let mut history = Vec::new();
    for batch in 0..16 {
        history.push((0u32, batch * 256, 256, if batch % 2 == 0 { 33 } else { 65 }));
    }
    for batch in 0..8 {
        history.push((2, batch * 512, 512, 0));
    }
    for batch in 0..8 {
        history.push((1, batch * 512, 128, 0));
    }
    // Removed ranges are recreated as present-empty; the others shrink.
    for batch in 0..16 {
        history.push((0, batch * 256, 256, 0));
    }
    for batch in 0..16 {
        history.push((0, batch * 256, 256, 65));
    }
    // Fail after changing old records and after creating previously absent
    // records. Neither failure may leak a record or refund gross growth.
    history.extend([
        (3, 0, 256, 1),
        (4, 2048, 256, 0),
        (3, 4096, 256, 33),
        (4, 4352, 256, 65),
    ]);
    for batch in 0..9 {
        history.push((2, batch * 512, 512, 0));
    }
    let mut records: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut test = case(&text).named("state-history/4096-live-records");
    for (mode, first, count, len) in history {
        let mut after = records.clone();
        let (mut lengths, mut values, mut copies, mut hashing, mut growth) =
            (0u64, 0u64, words(16), 0u64, 0u64);
        for key in first..first + count {
            match mode {
                1 => {
                    copies += words(32);
                    after.remove(&key);
                }
                2 => {
                    copies += words(32);
                    match records.get(&key) {
                        Some(value) => {
                            lengths += value.len() as u64;
                            values += u64::from(value.first().copied().unwrap_or(0));
                            copies += words(value.len() as u64);
                        }
                        None => lengths += u64::from(u32::MAX),
                    }
                }
                _ => {
                    let value = vec![(key * 4) as u8; len as usize];
                    growth += match after.get(&key) {
                        Some(old) => (value.len() as u64).saturating_sub(old.len() as u64),
                        None => RECORD_BASE + value.len() as u64,
                    };
                    copies += words(u64::from(len)) + words(32 + u64::from(len));
                    hashing += leaf(u64::from(len));
                    after.insert(key, value);
                }
            }
        }
        let iteration = match mode {
            1 => format!("{KEY} {DELETE} {NEXT}"),
            2 => format!("{KEY} {READ_TEST} {READ} {NEXT}"),
            _ => format!("{KEY} {READ_TEST} {WRITE} {NEXT}"),
        };
        let (status, ending) = match mode {
            3 => (Status::Revert, format!("{RESULTS} {REVERT}")),
            4 => (Status::Trap, format!("{RESULTS} {TRAP_TEST} unreachable")),
            _ => (Status::Success, format!("{RESULTS} {TRAP_TEST} {RETURN}")),
        };
        if status == Status::Success {
            records = after;
        }
        let expected: Vec<_> = records
            .iter()
            .map(|(k, v)| {
                let mut key = [0; 32];
                key[..4].copy_from_slice(&k.to_le_bytes());
                (key, v.clone())
            })
            .collect();
        let input: Vec<u8> = [mode, first, count, len]
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect();
        test = test
            .next_block()
            .call_with(&input)
            .expect(status)
            .records(expected)
            .charged(Ledger {
                dispatch: cost(PREFIX) + u64::from(count) * cost(&iteration) + cost(&ending),
                copies,
                hashing,
                growth,
                reads: u64::from(count),
                accesses: u64::from(count) * if mode == 2 { 1 } else { 2 },
                ..Default::default()
            });
        if status != Status::Trap {
            test = test.result([lengths.to_le_bytes(), values.to_le_bytes()].concat());
        }
    }
    test.next_block().run();
}
