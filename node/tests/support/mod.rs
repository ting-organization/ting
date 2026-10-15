#![allow(dead_code)]
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".node-tests")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                ting_node::now(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    pub fn config(&self, api: bool, name: &str) {
        let c = ting_node::config::Config {
            name: name.into(),
            control: "127.0.0.1:0".parse().unwrap(),
            p2p: "127.0.0.1:0".parse().unwrap(),
            api: "127.0.0.1:0".parse().unwrap(),
            api_enabled: api,
            ..Default::default()
        };
        ting_node::config::initialize(&self.0, &c, None).unwrap();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".node-tests");
        if let Ok(path) = self.0.canonicalize() {
            assert!(
                path.starts_with(root.canonicalize().unwrap())
                    && path != root.canonicalize().unwrap()
            );
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
pub fn tx() -> ting_core::protocol::Transaction {
    use ting_core::{
        meter::Resources,
        protocol::{Payload, Transaction},
    };
    Transaction {
        budget: Resources([1_000_000, 128, 16_384, 16_384]),
        payload: Payload::Deploy {
            code: wat::parse_str(
                r#"(module
            (import "ting" "storage_set" (func (param i32 i32 i32)))
            (memory (export "memory") 1 1) (data (i32.const 32) "\2a")
            (func (export "invoke") i32.const 0 i32.const 32 i32.const 1 call 0))"#,
            )
            .unwrap(),
            input: vec![],
        },
    }
}
