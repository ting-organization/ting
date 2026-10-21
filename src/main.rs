use std::{error::Error, io::Read};
use ting_core::{
    consensus,
    hash::hex,
    meter::Resources,
    protocol::{BLOCK_BASE, Block, Genesis, Header, Payload, Status, Transaction, producer_hash},
    state::State,
    vm::{Context, Runtime, profile},
};

/// A program that stores 40 + 2 and returns the eight bytes, written out in
/// the binary format (rules §17).
fn example_program() -> Vec<u8> {
    let mut m = b"\0asm\x01\0\0\0".to_vec();
    // type: (i32, i32) -> () and () -> ()
    m.extend([1, 9, 2, 0x60, 2, 0x7f, 0x7f, 0, 0x60, 0, 0]);
    // import "ting" "return" of type 0
    m.extend([2, 15, 1, 4]);
    m.extend(b"ting");
    m.extend([6]);
    m.extend(b"return");
    m.extend([0, 0]);
    // one function of type 1; one memory of exactly one page
    m.extend([3, 2, 1, 1, 5, 4, 1, 1, 1, 1]);
    // exports: invoke = function 1, memory = memory 0
    m.extend([7, 19, 2, 6]);
    m.extend(b"invoke");
    m.extend([0, 1, 6]);
    m.extend(b"memory");
    m.extend([2, 0]);
    // code: i32.const 0, i64.const 40, i64.const 2, i64.add, i64.store,
    //       i32.const 0, i32.const 8, call 0, end
    m.extend([10, 20, 1, 18, 0]);
    m.extend([
        0x41, 0, 0x42, 40, 0x42, 2, 0x7c, 0x37, 3, 0, 0x41, 0, 0x41, 8, 0x10, 0, 0x0b,
    ]);
    m
}
fn demo() -> Result<(), Box<dyn Error>> {
    let g = Genesis::experimental();
    g.validate()?;
    let builder = Runtime::new(State::default(), Context::from_genesis(&g));
    let deploy = Transaction {
        budget: g.initial,
        payload: Payload::Deploy {
            code: example_program(),
            input: Vec::new(),
        },
    };
    let installed = builder.execute(&deploy)?;
    if installed.status != Status::Success {
        return Err("demo deployment failed".into());
    }
    let id: [u8; 32] = installed.result.as_slice().try_into()?;
    let call = Transaction {
        budget: g.initial,
        payload: Payload::Call {
            target: id,
            input: Vec::new(),
        },
    };
    let executed = builder.execute(&call)?;
    if executed.status != Status::Success {
        return Err("demo call failed".into());
    }
    let value = u64::from_le_bytes(executed.result.as_slice().try_into()?);
    let transactions = vec![deploy, call];
    let receipts = vec![installed, executed];
    let (txroot, rcroot) = consensus::commitments(&transactions, &receipts);
    let mut used = Resources([0, 0, BLOCK_BASE, 0]);
    for r in &receipts {
        for i in 0..4 {
            used.0[i] += r.used.0[i];
        }
    }
    let mut header = Header {
        chain_id: g.chain_id(),
        height: 1,
        parent: g.block_id(),
        timestamp: g.timestamp + 60,
        target: g.initial_target.clone(),
        transactions_root: txroot,
        receipts_root: rcroot,
        state_root: builder.state_root(),
        used,
        votes: [0; 4],
        producer_hash: producer_hash(&[]),
        nonce: 0,
    };
    while !consensus::valid_pow(&header) {
        header.nonce = header.nonce.checked_add(1).ok_or("nonce exhausted")?;
    }
    let block = Block {
        header,
        producer: Vec::new(),
        transactions,
        receipts,
    };
    let validator = Runtime::new(State::default(), Context::from_genesis(&g));
    validator.validate_and_apply_block(&g, &[], &block)?;
    println!("Program result: {value}");
    println!("Contract: {}", hex(&id));
    println!("Validated local block: {}", hex(&block.header.id()));
    println!("Resources [C, S, D, G]: {:?}", block.header.used.0);
    println!("State root: {}", hex(&validator.state_root()));
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("demo") => demo(),
        Some("inspect") => {
            let path = args.get(2).ok_or("usage: ting-core inspect program.wasm")?;
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(profile::MAX_MODULE as u64 + 1)
                .read_to_end(&mut bytes)?;
            let p = profile::validate(&bytes)?;
            println!(
                "Accepted by the execution profile: {} functions, {} initial memory bytes, {} globals, {} table entries",
                p.functions.len(),
                p.memory,
                p.globals,
                p.table
            );
            Ok(())
        }
        None => {
            println!("Ting reference core\n  ting-core demo\n  ting-core inspect program.wasm");
            Ok(())
        }
        _ => Err("unknown command; use demo or inspect".into()),
    }
}
