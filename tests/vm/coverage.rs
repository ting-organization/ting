//! The coverage gate, run before a seed is cut: a fresh validator replays the
//! development seed with the VM counting what it runs, and the gate fails with
//! the list of what the seed never exercised. It needs the `coverage` feature:
//!
//! ```sh
//! cargo test --release --features coverage --test vm coverage
//! ```
//!
//! What it asks for: every instruction of the profile (the tables of the
//! protocol rules' instruction reference) executed; every host import returning and ending
//! each way it can; every status of a top-level and of a nested call and
//! deployment; every rejection reason; memory on both sides of each price
//! step and on the curve; stack pages paid on entry and on calls; and a
//! program called in a later block than the one that deployed it.
use crate::seed::{bundle, replay, seed};
use std::collections::{BTreeMap, BTreeSet};
use ting_core::{protocol::Payload, vm::coverage};

/// Every instruction the protocol rules' instruction reference (section 19)
/// lists, from its three kinds of table row: one instruction by its byte, a
/// run of bytes with the operators after the first written without their type,
/// and an i32 and an i64 byte for one operator of both widths.
fn instructions() -> BTreeSet<String> {
    let rules = include_str!("../../ting-protocol-rules.md");
    let page = &rules[rules.find("## 19. Instruction reference").unwrap()
        ..rules.find("## 20. The abstract machine").unwrap()];
    // A cell that opens with bytes in hex, such as `6a` or `fc 13`.
    let opcode = |cell: &str| {
        cell.strip_prefix('`')
            .and_then(|c| c.split('`').next())
            .is_some_and(|bytes| {
                !bytes.is_empty() && bytes.chars().all(|c| c.is_ascii_hexdigit() || c == ' ')
            })
    };
    let mut names = BTreeSet::new();
    for line in page.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 || !opcode(cells[1]) {
            continue;
        }
        if opcode(cells[2]) {
            let operator = cells[3].trim_matches('`');
            names.insert(format!("i32.{operator}"));
            names.insert(format!("i64.{operator}"));
            continue;
        }
        let mut prefix = "";
        for name in cells[2].split(", ").map(|n| n.trim_matches('`')) {
            match name.split_once('.') {
                Some((ty, _)) => {
                    prefix = ty;
                    names.insert(name.to_string());
                }
                None if prefix.is_empty() => {
                    names.insert(name.to_string());
                }
                None => {
                    names.insert(format!("{prefix}.{name}"));
                }
            }
        }
    }
    assert_eq!(names.len(), 115, "the rules list the 115 instructions");
    names
}

/// How each host import can end: every import returns or hands over to a
/// child, and stops its invocation for each failure its rule has.
const IMPORTS: &[(&str, &[&str])] = &[
    ("input", &["returned", "Trap", "Compute"]),
    ("context", &["returned", "Trap", "Compute"]),
    ("storage_get", &["returned", "Trap", "Compute", "Access"]),
    (
        "storage_set",
        &[
            "returned", "Trap", "Compute", "Access", "Growth", "ReadOnly",
        ],
    ),
    (
        "storage_delete",
        &["returned", "Trap", "Compute", "Access", "ReadOnly"],
    ),
    ("code_hash", &["returned", "Trap", "Compute", "Access"]),
    ("call", &["handed over", "returned", "Trap", "Compute"]),
    (
        "deploy",
        &["handed over", "returned", "Trap", "Compute", "ReadOnly"],
    ),
    ("call_result", &["returned", "Trap", "Compute"]),
    ("emit", &["returned", "Trap", "Compute", "Data", "ReadOnly"]),
    ("return", &["Success", "Trap", "Compute", "Data"]),
    ("revert", &["Revert", "Trap", "Compute", "Data"]),
];

/// The statuses each kind of invocation can end with. A top-level call is
/// never read-only, and status 7 needs more deployments than a transaction
/// can make.
const INVOCATIONS: &[(&str, &str, &[&str])] = &[
    (
        "call",
        "top",
        &[
            "Success", "Revert", "Trap", "Compute", "Access", "Data", "Growth", "Missing",
        ],
    ),
    (
        "deploy",
        "top",
        &[
            "Success",
            "Revert",
            "Trap",
            "Compute",
            "Access",
            "Data",
            "Growth",
            "InvalidModule",
            "Exists",
        ],
    ),
    (
        "call",
        "nested",
        &[
            "Success", "Revert", "Trap", "Compute", "Access", "Data", "Growth", "Missing",
            "ReadOnly",
        ],
    ),
    (
        "deploy",
        "nested",
        &[
            "Success",
            "Revert",
            "Trap",
            "Compute",
            "Access",
            "Data",
            "Growth",
            "InvalidModule",
        ],
    ),
];

const REJECTIONS: &[&str] = &[
    "module outside frozen VM profile",
    "noncanonical Wasm encoding",
    "function body fails Core validation",
];

/// Memory sizes in pages: both sides of the steps at 64 KiB, 1 MiB and
/// 2 MiB, and the curve above them.
const PAGES: &[u64] = &[1, 2, 16, 17, 32, 33, 1024];

#[test]
fn the_seed_exercises_every_rule() {
    let (blocks, manifest, _) = seed();
    let bytes = bundle(&blocks);
    coverage::take();
    replay(&bytes);
    let tally = coverage::take();
    let mut missing = Vec::new();

    for name in instructions() {
        if !tally.instructions.contains_key(name.as_str()) {
            missing.push(format!("instruction {name}"));
        }
    }
    for (import, endings) in IMPORTS {
        for ending in *endings {
            if !tally.imports.contains_key(&(*import, ending.to_string())) {
                missing.push(format!("{import} ending {ending}"));
            }
        }
    }
    for (kind, depth, statuses) in INVOCATIONS {
        for status in *statuses {
            if !tally
                .invocations
                .contains_key(&(*kind, *depth, status.to_string()))
            {
                missing.push(format!("{depth} {kind} ending {status}"));
            }
        }
    }
    for reason in REJECTIONS {
        if !tally.rejections.contains_key(reason) {
            missing.push(format!("rejection: {reason}"));
        }
    }
    for pages in PAGES {
        if !tally.pages.contains_key(pages) {
            missing.push(format!("memory of {pages} pages"));
        }
    }
    for place in ["entry", "call"] {
        if !tally.stack.contains_key(place) {
            missing.push(format!("stack pages on {place}"));
        }
    }

    // A program deployed at the top level in one block and called at the
    // top level in a later one.
    let mut deployed: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
    let mut later = false;
    for block in &blocks {
        let height = block.header.height;
        for (tx, receipt) in block.transactions.iter().zip(&block.receipts) {
            match &tx.payload {
                Payload::Deploy { .. } if receipt.result.len() == 32 => {
                    deployed.insert(receipt.result.clone(), height);
                }
                Payload::Call { target, .. } => {
                    later |= deployed.get(&target[..]).is_some_and(|&h| h < height);
                }
                _ => {}
            }
        }
    }
    if !later {
        missing.push("a call in a later block than its program's deployment".into());
    }

    let unexpected: Vec<String> = tally
        .imports
        .keys()
        .filter(|(import, ending)| {
            !IMPORTS
                .iter()
                .any(|(i, endings)| i == import && endings.contains(&ending.as_str()))
        })
        .map(|(import, ending)| format!("{import} ending {ending}"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "endings the gate does not list: {unexpected:?}"
    );
    assert!(
        missing.is_empty(),
        "the seed never exercises:\n  {}",
        missing.join("\n  ")
    );
    let report = serde_json::json!({
        "scope": "Broad execution requirements, plus source/variant provenance. Not a claim that every protocol edge is covered.",
        "instructions": tally.instructions,
        "imports": tally.imports.iter().map(|((name, ending), count)| serde_json::json!({"name":name,"ending":ending,"count":count})).collect::<Vec<_>>(),
        "invocations": tally.invocations.iter().map(|((kind, depth, status), count)| serde_json::json!({"kind":kind,"depth":depth,"status":status,"count":count})).collect::<Vec<_>>(),
        "rejections": tally.rejections,
        "pages": tally.pages,
        "stack": tally.stack,
        "blocks": manifest.iter().map(|b| serde_json::json!({"height":b["height"],"id":b["id"]})).collect::<Vec<_>>(),
        "missing": missing,
    });
    let dir = crate::seed::artifact_dir("development");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("coverage.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn refused_instruction_dispatch_is_not_execution_coverage() {
    use crate::{
        dsl::{invoke, memory, module},
        harness::{deployed as case, deployed_entry_of},
    };
    use ting_core::protocol::Status;
    let text = module(&[&memory(1, 1), &invoke("nop")]);
    coverage::take();
    let mut budget = vm_model::BUDGET;
    budget[0] = 1 + deployed_entry_of(&text);
    case(&text)
        .call()
        .budget(budget)
        .expect(Status::Compute)
        .charged(vm_model::Ledger::default())
        .run();
    assert!(!coverage::take().instructions.contains_key("nop"));
    case(&text)
        .call()
        .charged(vm_model::Ledger {
            dispatch: 1,
            ..Default::default()
        })
        .run();
    assert!(coverage::take().instructions.contains_key("nop"));
}
