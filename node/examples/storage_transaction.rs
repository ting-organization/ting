//! Print a canonical deployment envelope as hex for consumer API experiments.
//! This deliberately simple program stores byte 0x2a under the all-zero key.
use ting_core::{
    hash::hex,
    meter::Resources,
    protocol::{Encode, Payload, Transaction},
};
fn main() {
    let code = wat::parse_str(
        r#"(module
        (import "ting" "storage_set" (func (param i32 i32 i32)))
        (memory (export "memory") 1 1)
        (data (i32.const 32) "\2a")
        (func (export "invoke") i32.const 0 i32.const 32 i32.const 1 call 0))"#,
    )
    .unwrap();
    let tx = Transaction {
        budget: Resources([1_000_000, 128, 16_384, 16_384]),
        payload: Payload::Deploy {
            code,
            input: vec![],
        },
    };
    println!("{}", hex(&tx.encode()));
}
