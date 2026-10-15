//! The reference programs published in `programs/` (wiki: programs). They are
//! not part of the protocol, but a change to the protocol or the VM that would
//! break them fails here: every file is listed in the manifest with its exact
//! size and CodeHash, passes the profile, deploys with the canonical budget,
//! and appears in the wiki's table with the same size and hash.
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};
use ting_core::{
    hash::{hash, hex},
    meter::Resources,
    protocol::{Genesis, Payload, Status, Transaction},
    state::State,
    vm::{Context, Runtime, profile},
};

/// The canonical deployment budget (wiki: programs).
const CANONICAL: [u64; 4] = [50_000_000, 64, 16_777_216, 16_777_216];

#[test]
fn every_published_program_is_listed_valid_deployable_and_in_the_wiki() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("programs");
    let manifest: Value =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let entries = manifest.as_array().unwrap();
    let listed: BTreeSet<String> = entries
        .iter()
        .map(|e| format!("{}.wasm", e["name"].as_str().unwrap()))
        .collect();
    let present: BTreeSet<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".wasm"))
        .collect();
    assert_eq!(listed, present, "the manifest and the files differ");
    assert_eq!(listed.len(), entries.len(), "a program is listed twice");
    let wiki = fs::read_to_string(root.join("node/wiki/programs.md")).unwrap();
    let g = Genesis::experimental();
    for entry in entries {
        let name = entry["name"].as_str().unwrap();
        let keys: Vec<&String> = entry.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3, "{name}: only name, bytes and code_hash");
        let code = fs::read(dir.join(format!("{name}.wasm"))).unwrap();
        let code_hash = hex(&hash("code", &code));
        assert_eq!(entry["bytes"], code.len(), "{name}: size");
        assert_eq!(entry["code_hash"], code_hash, "{name}: CodeHash");
        profile::validate(&code).unwrap_or_else(|e| panic!("{name}: {e}"));
        let row = wiki
            .lines()
            .find(|line| line.starts_with(&format!("| [`{name}`]")))
            .unwrap_or_else(|| panic!("{name} is missing from the wiki's table"));
        assert!(
            row.contains(&format!("`{code_hash}`")) && row.contains(&thousands(code.len())),
            "the wiki's row for {name} disagrees with the file: {row}"
        );
        // The canonical deployment has empty input; a coin needs its options.
        let deploy = |input: Vec<u8>| {
            let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
            let tx = Transaction {
                budget: Resources(CANONICAL),
                payload: Payload::Deploy {
                    code: code.clone(),
                    input,
                },
            };
            let receipt = runtime.execute(&tx).unwrap();
            let installed = receipt
                .result
                .clone()
                .try_into()
                .ok()
                .and_then(|id| runtime.code(id));
            (receipt, installed)
        };
        let (receipt, installed) = deploy(Vec::new());
        if name == "coin" {
            assert_eq!(receipt.status, Status::Revert, "{name}");
            assert_eq!(receipt.result, b"bad deploy input");
            let (receipt, installed) = deploy(coin_options());
            assert_eq!(receipt.status, Status::Success, "{name}");
            assert_eq!(installed.as_deref(), Some(&code[..]), "{name}");
        } else {
            assert_eq!(
                receipt.status,
                Status::Success,
                "{name}: {}",
                String::from_utf8_lossy(&receipt.result)
            );
            assert_eq!(installed.as_deref(), Some(&code[..]), "{name}");
        }
    }
}

/// A coin with no cap, no rewards, no minter and no allocations (wiki:
/// programs/coin).
fn coin_options() -> Vec<u8> {
    let bytes = |b: &[u8]| [&(b.len() as u32).to_le_bytes()[..], b].concat();
    [
        &[1][..],
        &bytes(b"Test"),
        &bytes(b"TST"),
        &[0],
        &u128::MAX.to_le_bytes(),
        &0u128.to_le_bytes(),
        &0u64.to_le_bytes(),
        &0u128.to_le_bytes(),
        &[0; 32],
        &0u64.to_le_bytes(),
    ]
    .concat()
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
