//! The configuration file: every bound, every default, and what an invalid
//! one leaves behind (wiki: node).
mod support;
use serde_json::{Value, json};
use support::Directory;
use ting_node::config::{self, Config};

fn base() -> Value {
    json!({
        "version": 1,
        "name": "node",
        "control": "127.0.0.1:0",
        "p2p": "127.0.0.1:0",
        "api": "127.0.0.1:0",
        "api_enabled": false,
        "peers": [],
        "votes": [0, 0, 0, 0]
    })
}
fn load(dir: &Directory, file: &Value) -> Result<Config, String> {
    std::fs::write(dir.0.join("config.json"), serde_json::to_vec(file).unwrap()).unwrap();
    Config::load(&dir.0)
}

#[test]
fn optional_fields_take_their_defaults_and_every_bound_is_enforced() {
    let dir = Directory::new();
    let c = load(&dir, &base()).unwrap();
    assert!(c.discovery && !c.api_cors && !c.miner_enabled);
    assert_eq!((c.max_peers, c.miner_threads, c.miner_spacing), (8, 1, 0));
    assert_eq!((c.db_cache_mib, c.cache_bytes()), (256, 256 << 20));
    assert_eq!(c.producer_hex, None);
    assert!(
        c.producer().is_empty(),
        "no producer data unless configured"
    );
    let mut unknown = base();
    unknown["extra"] = json!(1);
    assert!(
        load(&dir, &unknown)
            .unwrap_err()
            .contains("invalid configuration at")
    );
    let mut missing = base();
    missing.as_object_mut().unwrap().remove("peers");
    assert!(load(&dir, &missing).is_err());
    let faults: [(&str, Value); 14] = [
        ("version", json!(2)),
        ("control", json!("10.0.0.1:18440")),
        ("name", json!("n".repeat(129))),
        ("peers", json!(vec!["10.0.0.1:1"; 33])),
        ("max_peers", json!(0)),
        ("max_peers", json!(65)),
        ("votes", json!([0, 0, 0, 3])),
        ("miner_threads", json!(0)),
        ("miner_threads", json!(65)),
        ("producer_hex", json!("0G")),
        ("producer_hex", json!("0")),
        ("producer_hex", json!("00".repeat(4097))),
        ("producer_hex", json!("AB")),
        ("db_cache_mib", json!(0)),
    ];
    for (field, value) in faults {
        let mut file = base();
        file[field] = value.clone();
        assert!(load(&dir, &file).is_err(), "{field} = {value}");
    }
    let edges: [(&str, Value); 8] = [
        ("name", json!("n".repeat(128))),
        ("peers", json!(vec!["10.0.0.1:1"; 32])),
        ("max_peers", json!(1)),
        ("max_peers", json!(64)),
        ("miner_threads", json!(64)),
        ("producer_hex", json!("00".repeat(4096))),
        ("producer_hex", json!("")),
        ("db_cache_mib", json!(1)),
    ];
    for (field, value) in edges {
        let mut file = base();
        file[field] = value.clone();
        load(&dir, &file).unwrap_or_else(|e| panic!("{field} = {value}: {e}"));
    }
    // Producer data: the hex when set, the name when unset or empty.
    let mut file = base();
    file["producer_hex"] = json!("00ff");
    assert_eq!(load(&dir, &file).unwrap().producer(), [0, 255]);
    file["producer_hex"] = json!("");
    assert!(load(&dir, &file).unwrap().producer().is_empty());
    // What the node writes, it reads back unchanged.
    let written = load(&dir, &file).unwrap();
    written.save(&dir.0).unwrap();
    let again = Config::load(&dir.0).unwrap();
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(&written).unwrap()
    );
}

#[test]
fn an_invalid_configuration_initializes_nothing() {
    let dir = Directory::new();
    let fresh = dir.0.join("fresh");
    let bad = Config {
        control: "10.0.0.1:18440".parse().unwrap(),
        ..Default::default()
    };
    assert!(config::initialize(&fresh, &bad, None).is_err());
    assert!(!fresh.exists());
    let good = Config {
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    config::initialize(&fresh, &good, None).unwrap();
    for file in ["config.json", "genesis.bin", "control.token"] {
        assert!(fresh.join(file).exists(), "{file}");
    }
    assert_eq!(config::token(&fresh).unwrap().len(), 64);
    assert_eq!(config::genesis(&fresh).unwrap().vote_epoch, 20160);
    assert!(
        config::initialize(&fresh, &good, None)
            .unwrap_err()
            .contains("already initialized")
    );
}
#[test]
fn the_production_genesis_is_the_production_profile_at_the_launch_time() {
    use ting_core::{hash::hex, protocol::Genesis};
    // The production genesis is the production profile at the launch time,
    // 2026-10-21 00:00:00 UTC: its ChainId and genesis block are the network's.
    let genesis = config::production_genesis().unwrap();
    assert_eq!(config::LAUNCH, Some(1_792_540_800));
    assert_eq!(genesis, Genesis::production(1_792_540_800));
    assert_eq!(
        hex(&genesis.chain_id()),
        "15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107"
    );
    assert_eq!(
        hex(&genesis.block_id()),
        "24b76a52f3825b921373b6c6a41ced2a74de7dbdb34015669c10594996887459"
    );
    assert_eq!(
        config::SEEDS,
        ["node1.ting-node.com:18442", "node2.ting-node.com:18442"]
    );
}
#[test]
fn the_control_token_is_its_owners_alone() {
    let dir = Directory::new();
    dir.config(false, "token");
    let path = dir.0.join("control.token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = || std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(), 0o600);
        // Opened up after init, it is closed again by the same step.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        config::protect_token(&dir.0).unwrap();
        assert_eq!(mode(), 0o600);
    }
    #[cfg(windows)]
    {
        // One entry, full access for this user, and nothing inherited from
        // the folder.
        let entries = || {
            let out = std::process::Command::new("icacls")
                .arg(&path)
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| l.contains(":("))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let only = entries();
        assert_eq!(only.len(), 1, "{only:?}");
        assert!(
            only[0].contains("(F)") && !only[0].contains("(I)"),
            "{only:?}"
        );
        // Opened to everyone after init, it is closed again by the same step.
        let granted = std::process::Command::new("icacls")
            .arg(&path)
            .args(["/grant", "*S-1-1-0:R"])
            .output()
            .unwrap();
        assert!(granted.status.success());
        assert_eq!(entries().len(), 2);
        config::protect_token(&dir.0).unwrap();
        assert_eq!(entries(), only);
    }
    assert!(config::token(&dir.0).is_ok());
}
