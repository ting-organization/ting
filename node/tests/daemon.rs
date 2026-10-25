mod support;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::Path, process::Command, time::Duration};
use support::Directory;
use ting_core::{
    consensus,
    hash::{Hash, hex},
    protocol::{Encode, HEADER_LEN},
    state::State,
    vm::{Context, Runtime},
};
use ting_node::{
    config::{self, Running},
    transport, wire,
};
/// A test's own hello to a node on `chain_id`.
fn hello(chain_id: Hash) -> wire::Hello {
    wire::Hello {
        version: 1,
        chain_id,
        agent: "test".into(),
        features: vec![],
        nonce: 1,
        announce: false,
        listen: None,
    }
}
/// Connects to a node's peer listener and says `hello`.
async fn greet(
    address: SocketAddr,
    hello: wire::Hello,
) -> (TcpStream, Result<wire::Answer, String>) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let answer = transport::ask(&mut stream, wire::Request::Hello(hello)).await;
    (stream, answer)
}
/// A frame of any bytes, as a peer that does not follow the protocol sends.
async fn raw_frame(stream: &mut TcpStream, bytes: &[u8]) {
    stream.write_u32_le(bytes.len() as u32).await.unwrap();
    stream.write_all(bytes).await.unwrap();
}
/// An item pushed whole, in one part.
fn whole(bytes: &[u8]) -> wire::Part {
    wire::Part {
        offset: 0,
        total: bytes.len() as u64,
        bytes: bytes.to_vec(),
    }
}
/// A fake peer's state: three headers ahead, at `tip`.
fn fake_state(chain_id: Hash, tip: Hash, nonce: u64) -> wire::State {
    wire::State {
        version: 1,
        chain_id,
        agent: "fake".into(),
        features: vec![],
        nonce,
        height: 3,
        tip,
        work: 12u32.into(),
        locator: vec![],
        pending: vec![],
        deferred: vec![],
    }
}
/// Serves a fake peer's connections: each request answered by `answer`, until
/// the connection ends.
fn fake_peer(
    listener: tokio::net::TcpListener,
    answer: impl Fn(&wire::Request, &mut bool) -> Result<wire::Answer, String>
    + Clone
    + Send
    + Sync
    + 'static,
) {
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let answer = answer.clone();
            tokio::spawn(async move {
                let mut sent = false;
                while let Ok(wire::Frame::Request(request)) =
                    transport::read_wire(&mut stream, None, ting_node::FRAME_MAX).await
                {
                    let frame = match answer(&request, &mut sent) {
                        Ok(answer) => wire::Frame::Answer(answer),
                        Err(error) => wire::Frame::Error(error),
                    };
                    if transport::write_wire(&mut stream, &frame).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
}
/// The transactions a fake peer pools, by id: their bytes, or none for one it
/// only lists.
type Pool = std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<Hash, Option<Vec<u8>>>>>;
/// The requests a fake peer was sent, in order.
type Asked = std::sync::Arc<std::sync::Mutex<Vec<wire::Request>>>;
/// Serves a fake peer at `state` that pools `pool`, pages its inventory and
/// answers with a slice's worth of transactions whole as a node does, takes
/// none of the transactions it is sent, and after each exchange announces
/// that its pool changed, so that the node exchanges again at once.
fn pool_peer(listener: tokio::net::TcpListener, state: wire::State, pool: Pool, asked: Asked) {
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (state, pool, asked) = (state.clone(), pool.clone(), asked.clone());
            tokio::spawn(async move {
                let mut changes = 0;
                while let Ok(wire::Frame::Request(request)) =
                    transport::read_wire(&mut stream, None, ting_node::FRAME_MAX).await
                {
                    asked.lock().unwrap().push(request.clone());
                    let answer = match &request {
                        wire::Request::Hello(_) | wire::Request::State => {
                            Ok(wire::Answer::State(state.clone()))
                        }
                        wire::Request::Inventory {
                            deferred: false,
                            after,
                        } => {
                            let pool = pool.lock().unwrap();
                            let mut ids: Vec<Hash> = pool
                                .keys()
                                .filter(|id| after.is_none_or(|after| **id > after))
                                .take(1025)
                                .copied()
                                .collect();
                            let next = (ids.len() > 1024).then(|| ids[1023]);
                            ids.truncate(1024);
                            Ok(wire::Answer::Inventory { ids, next })
                        }
                        wire::Request::Inventory { deferred: true, .. } => {
                            Ok(wire::Answer::Inventory {
                                ids: vec![],
                                next: None,
                            })
                        }
                        wire::Request::Pending { ids } => {
                            let pool = pool.lock().unwrap();
                            let mut whole = 0;
                            Ok(wire::Answer::Items(
                                ids.iter()
                                    .filter_map(|id| {
                                        let bytes = pool.get(id)?.clone()?;
                                        whole += bytes.len();
                                        let body = if whole <= ting_node::chain::SLICE_MAX {
                                            wire::Body::Whole(bytes)
                                        } else {
                                            wire::Body::Total(bytes.len() as u64)
                                        };
                                        Some(wire::Item {
                                            id: *id,
                                            not_before: None,
                                            body,
                                        })
                                    })
                                    .collect(),
                            ))
                        }
                        wire::Request::TransactionRaw { id, offset, length } => {
                            let pool = pool.lock().unwrap();
                            match pool.get(id).cloned().flatten() {
                                Some(bytes) => {
                                    let from = (*offset as usize).min(bytes.len());
                                    let to = (from + *length as usize).min(bytes.len());
                                    Ok(wire::Answer::Slice {
                                        offset: *offset,
                                        total: bytes.len() as u64,
                                        bytes: bytes[from..to].to_vec(),
                                    })
                                }
                                None => Err("transaction not found".to_string()),
                            }
                        }
                        wire::Request::Submit { .. } => Err("mempool is full".to_string()),
                        wire::Request::Headers { .. } => Ok(wire::Answer::Headers(vec![])),
                        wire::Request::Addresses => Ok(wire::Answer::Addresses(vec![])),
                        _ => Err("not served".to_string()),
                    };
                    let mut frames = vec![match answer {
                        Ok(answer) => wire::Frame::Answer(answer),
                        Err(error) => wire::Frame::Error(error),
                    }];
                    // The deferred inventory ends an exchange.
                    if matches!(request, wire::Request::Inventory { deferred: true, .. }) {
                        changes += 1;
                        frames.push(wire::Frame::Announcement(wire::Announcement {
                            tip: state.tip,
                            height: state.height,
                            work: state.work.clone(),
                            pending: changes,
                            deferred: 0,
                        }));
                    }
                    for frame in frames {
                        if transport::write_wire(&mut stream, &frame).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
}
/// What a fake peer was asked so far: the exchanges begun, the transactions
/// sent to it, and the ids of those asked of it.
fn exchanges(asked: &Asked) -> (usize, usize, usize) {
    let asked = asked.lock().unwrap();
    let begun = asked
        .iter()
        .filter(|r| {
            matches!(
                r,
                wire::Request::Inventory {
                    deferred: false,
                    ..
                }
            )
        })
        .count();
    let sent = asked
        .iter()
        .filter(|r| matches!(r, wire::Request::Submit { .. }))
        .count();
    let fetched = asked
        .iter()
        .map(|r| match r {
            wire::Request::Pending { ids } => ids.len(),
            _ => 0,
        })
        .sum();
    (begun, sent, fetched)
}
/// Waits until a fake peer has seen `rounds` exchanges begin.
async fn wait_exchanges(n: &Node, asked: &Asked, rounds: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while exchanges(asked).0 < rounds {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?} {}",
            exchanges(asked),
            n.call("peers", json!({})).await
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
/// A node's ChainId, as its status gives it.
async fn chain_id_of(n: &Node) -> Hash {
    ting_node::hash_arg(
        n.call("status", json!({})).await["chain_id"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

fn cli(dir: &Path, args: &[&str]) -> std::process::Output {
    // Rust's Windows process spawner can inherit pipes from concurrent spawns.
    // Serialize test subprocess launches; peer/HTTP traffic remains concurrent.
    static LAUNCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _launch = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    let mut command = Command::new(env!("CARGO_BIN_EXE_ting"));
    command.arg("--data-dir").arg(dir).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}
fn command(dir: &Path, args: &[&str]) -> Value {
    let output = cli(dir, args);
    assert!(
        output.status.success(),
        "CLI {:?}: {}{}",
        args,
        String::from_utf8_lossy(&output.stderr),
        node_log(dir)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
/// The end of a node's log, for a failure's message.
fn node_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("node.log")).map_or_else(
        |_| String::new(),
        |log| {
            let lines: Vec<_> = log.lines().collect();
            format!(
                "\nnode.log:\n{}",
                lines[lines.len().saturating_sub(20)..].join("\n")
            )
        },
    )
}
struct Node {
    dir: Directory,
    info: Running,
}
impl Node {
    fn start(api: bool, name: &str) -> Self {
        Self::start_on(None, api, name)
    }
    /// A node on the genesis in the file `genesis`, or on the test genesis,
    /// the development chain's rules without its seed, so that the node
    /// starts empty.
    fn start_on(genesis: Option<&Path>, api: bool, name: &str) -> Self {
        let dir = Directory::new();
        let genesis = match genesis {
            Some(path) => path.to_path_buf(),
            None => {
                let path = dir.0.join("test.genesis");
                std::fs::write(&path, support::test_genesis().encode()).unwrap();
                path
            }
        };
        let genesis = genesis.to_str().unwrap().to_owned();
        let mut args = vec!["init", "--genesis", genesis.as_str()];
        args.extend([
            "--name",
            name,
            "--control",
            "127.0.0.1:0",
            "--p2p",
            "127.0.0.1:0",
            "--api-bind",
            "127.0.0.1:0",
        ]);
        if api {
            args.push("--api");
        }
        command(&dir.0, &args);
        command(&dir.0, &["start"]);
        let info = Running::load(&dir.0).unwrap();
        Self { dir, info }
    }
    async fn call(&self, method: &str, params: Value) -> Value {
        transport::control_call(
            self.info.control,
            &config::token(&self.dir.0).unwrap(),
            method,
            params,
        )
        .await
        .unwrap()
    }
    async fn height(&self) -> u64 {
        self.call("status", json!({})).await["height"]
            .as_u64()
            .unwrap()
    }
    async fn wait_height(&self, height: u64) {
        // Every imported block is a durable commit, which takes far longer on
        // some disks than on others. Wait as long as synchronization makes
        // progress, measured by stored blocks or height; a sync that stalls
        // for 30 seconds fails, and so does one still running after 10 minutes.
        let start = tokio::time::Instant::now();
        let mut last_progress = start;
        let mut seen = (0, 0);
        while last_progress.elapsed() < Duration::from_secs(30)
            && start.elapsed() < Duration::from_secs(600)
        {
            let status = self.call("status", json!({})).await;
            let now = (
                status["height"].as_u64().unwrap(),
                status["stored_blocks"].as_u64().unwrap(),
            );
            if now.0 >= height {
                return;
            }
            if now != seen {
                seen = now;
                last_progress = tokio::time::Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!(
            "node did not synchronize to height {height}: {}, peers {}",
            self.call("status", json!({})).await,
            self.call("peers", json!({})).await
        );
    }
    /// A node of a development chain: with its seed imported as `ting init
    /// --dev` does, or empty, to take the seed from its peers.
    fn start_development(seeded: bool, name: &str) -> Self {
        let dir = Directory::new();
        if seeded {
            command(
                &dir.0,
                &[
                    "init",
                    "--dev",
                    "--name",
                    name,
                    "--control",
                    "127.0.0.1:0",
                    "--p2p",
                    "127.0.0.1:0",
                    "--api-bind",
                    "127.0.0.1:0",
                ],
            );
        } else {
            let c = config::Config {
                name: name.into(),
                control: "127.0.0.1:0".parse().unwrap(),
                p2p: "127.0.0.1:0".parse().unwrap(),
                api: "127.0.0.1:0".parse().unwrap(),
                ..Default::default()
            };
            config::initialize_empty(&dir.0, &c, config::development_genesis()).unwrap();
        }
        command(&dir.0, &["start"]);
        let info = Running::load(&dir.0).unwrap();
        Self { dir, info }
    }
    /// A node on the test genesis, with its configuration changed by
    /// `configure` before it starts, such as its pool's size.
    fn start_configured(name: &str, configure: impl FnOnce(&mut config::Config)) -> Self {
        let dir = Directory::new();
        let mut c = config::Config {
            name: name.into(),
            control: "127.0.0.1:0".parse().unwrap(),
            p2p: "127.0.0.1:0".parse().unwrap(),
            api: "127.0.0.1:0".parse().unwrap(),
            ..Default::default()
        };
        configure(&mut c);
        config::initialize_with(&dir.0, &c, support::test_genesis()).unwrap();
        command(&dir.0, &["start"]);
        let info = Running::load(&dir.0).unwrap();
        Self { dir, info }
    }
    /// Starts a stopped node again; its listeners get new ports.
    fn restart(&mut self) {
        command(&self.dir.0, &["start"]);
        self.info = Running::load(&self.dir.0).unwrap();
    }
    async fn stop(&self) {
        self.call("stop", json!({})).await;
        for _ in 0..50 {
            if !self.dir.0.join("runtime.json").exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("daemon did not stop");
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        if self.dir.0.join("runtime.json").exists() {
            let _ = cli(&self.dir.0, &["stop"]);
            for _ in 0..50 {
                if !self.dir.0.join("runtime.json").exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
async fn http(
    address: SocketAddr,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let body = body.map(|v| v.to_string()).unwrap_or_default();
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8(bytes).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.into(),
    )
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_cli_rest_wiki_sync_and_restart_work_together() {
    let a = Node::start(false, "node A");
    let b = Node::start(true, "node B");
    assert_eq!(
        a.call("status", json!({})).await["listeners"]["api"],
        Value::Null
    );
    assert_eq!(command(&a.dir.0, &["status"])["height"], 0);
    let enabled = command(&a.dir.0, &["api", "enable"]);
    let api = enabled["address"].as_str().unwrap().parse().unwrap();
    let docs = command(&a.dir.0, &["docs"]);
    assert!(docs["wiki"].as_str().unwrap().ends_with("/wiki"));
    for page in ting_node::wiki::all() {
        let (status, html) = http(api, "GET", &page.url(), None, None).await;
        assert_eq!(status, 200, "{}", page.url());
        assert!(html.contains("<img src=\"/assets/logo.svg\" alt=\"\"><span>Ting</span>"));
        assert!(html.contains("<h1"));
    }
    let (status, logo) = http(api, "GET", "/assets/logo.svg", None, None).await;
    assert_eq!(status, 200);
    assert!(logo.starts_with("<svg"));
    assert_eq!(
        http(api, "GET", "/wiki/consensus/", None, None).await.0,
        308
    );
    for path in [
        "/",
        "/wiki",
        "/docs",
        "/health",
        "/v1/status",
        "/v1/metrics",
        "/v1/events",
        "/v1/events?contract=0000000000000000000000000000000000000000000000000000000000000000&topic=abababababababababababababababababababababababababababababababab&from=1&limit=5",
        "/v1/mempool",
        "/v1/votes",
        "/blocks",
        "/mempool",
        "/network",
        "/assets/site.css",
        "/assets/site.js",
        "/assets/explorer.js",
        "/assets/swagger-ui.css",
        "/assets/swagger-ui-bundle.js",
        "/assets/swagger-init.js",
    ] {
        assert_eq!(http(api, "GET", path, None, None).await.0, 200, "{path}");
    }
    let (status, schema) = http(api, "GET", "/openapi.json", None, None).await;
    assert_eq!(status, 200);
    let schema: Value = serde_json::from_str(&schema).unwrap();
    assert_eq!(schema["openapi"], "3.1.0");
    assert!(schema["paths"].get("/v1/peers").is_none());
    assert!(schema["components"].get("securitySchemes").is_none());
    assert!(
        schema["paths"]["/v1/transactions"]["post"]
            .get("security")
            .is_none()
    );
    let (_, consumer) = http(api, "GET", "/v1/status", None, None).await;
    let consumer: Value = serde_json::from_str(&consumer).unwrap();
    assert!(consumer.get("listeners").is_none());
    assert!(consumer.get("pid").is_none());
    assert!(a.call("status", json!({})).await.get("listeners").is_some());
    for path in ["/", "/blocks", "/mempool", "/network", "/docs"] {
        let (_, html) = http(api, "GET", path, None, None).await;
        assert!(
            !html.contains("Wiki chapters"),
            "{path} unexpectedly includes the wiki sidebar"
        );
        assert_eq!(
            html.contains("Explorer sections"),
            path != "/docs",
            "{path} explorer sidebar presence"
        );
    }
    let (_, wiki) = http(api, "GET", "/wiki", None, None).await;
    assert!(!wiki.contains("Explorer sections"));
    let (_, docs_page) = http(api, "GET", "/docs", None, None).await;
    assert!(docs_page.contains("id=\"swagger-ui\""));
    assert!(docs_page.contains("/assets/swagger-ui-bundle.js"));
    assert_eq!(http(api, "GET", "/wiki/missing", None, None).await.0, 404);
    assert_eq!(
        http(api, "GET", "/v1/blocks?limit=101", None, None).await.0,
        400
    );
    let peer = json!({"address":b.info.p2p.to_string()});
    assert_eq!(
        http(api, "POST", "/v1/peers", Some(peer.clone()), None)
            .await
            .0,
        404
    );
    let token = config::token(&a.dir.0).unwrap();
    assert_eq!(
        http(api, "POST", "/v1/peers", Some(peer), Some(&token))
            .await
            .0,
        404
    );
    assert_eq!(http(api, "GET", "/v1/peers", None, None).await.0, 404);
    // A connects to B: A's blocks reach B by push, and B's reach A because B
    // announces its new tip over A's connection; neither waits for the
    // 30-second keepalive.
    command(&a.dir.0, &["add", "node", &b.info.p2p.to_string()]);
    command(&a.dir.0, &["mine", "--count", "2"]);
    tokio::time::timeout(Duration::from_secs(10), b.wait_height(2))
        .await
        .unwrap();
    command(&b.dir.0, &["mine", "--count", "1"]);
    tokio::time::timeout(Duration::from_secs(10), a.wait_height(3))
        .await
        .unwrap();
    assert_eq!(
        a.call("status", json!({})).await["tip"],
        b.call("status", json!({})).await["tip"]
    );
    let tx = support::tx();
    assert_eq!(
        http(
            api,
            "POST",
            "/v1/transactions",
            Some(json!({"hex":hex(&tx.encode())})),
            None
        )
        .await
        .0,
        200
    );
    let (status, admission) = http(
        api,
        "POST",
        "/v1/transactions",
        Some(json!({"hex":hex(&tx.encode())})),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let id = serde_json::from_str::<Value>(&admission).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, pending_lookup) = http(api, "GET", &format!("/v1/transactions/{id}"), None, None).await;
    let pending_lookup: Value = serde_json::from_str(&pending_lookup).unwrap();
    assert_eq!(pending_lookup["pending"], true);
    assert_eq!(
        pending_lookup["pending_transaction"]["transaction"]["kind"],
        "deploy"
    );
    assert!(
        pending_lookup["pending_transaction"]["transaction"]["code_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        http(
            api,
            "POST",
            "/v1/transactions",
            Some(json!({"hex":"zz"})),
            None
        )
        .await
        .0,
        400
    );
    for _ in 0..100 {
        if b.call("status", json!({})).await["mempool_count"] == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(b.call("status", json!({})).await["mempool_count"], 1);
    command(&b.dir.0, &["mine"]);
    a.wait_height(4).await;
    let (_, lookup) = http(api, "GET", &format!("/v1/transactions/{id}"), None, None).await;
    let lookup: Value = serde_json::from_str(&lookup).unwrap();
    assert_eq!(lookup["occurrences"][0]["transaction"]["kind"], "deploy");
    assert_eq!(lookup["pending_transaction"], Value::Null);
    let contract = lookup["occurrences"][0]["receipt"]["result_hex"]
        .as_str()
        .unwrap();
    assert_eq!(
        http(api, "GET", &format!("/v1/contracts/{contract}"), None, None)
            .await
            .0,
        200
    );
    let (_, value) = http(
        api,
        "GET",
        &format!("/v1/contracts/{contract}/storage/{}", hex(&[0; 32])),
        None,
        None,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&value).unwrap()["value_hex"],
        "2a"
    );
    // Many records at once come from one state, in the order asked, absent
    // ones as null; an empty or overlong list, or an unknown field, is refused.
    let read = |key: [u8; 32]| json!({"contract":contract,"key":hex(&key)});
    let (status, batch) = http(
        api,
        "POST",
        "/v1/storage",
        Some(json!({"reads":[read([0; 32]), read([1; 32]), read([0; 32])]})),
        None,
    )
    .await;
    assert_eq!(status, 200, "{batch}");
    let batch: Value = serde_json::from_str(&batch).unwrap();
    let values: Vec<&Value> = batch["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| &v["value_hex"])
        .collect();
    assert_eq!(values, [&json!("2a"), &Value::Null, &json!("2a")]);
    let status = a.call("status", json!({})).await;
    assert_eq!(batch["state_root"], status["state_root"]);
    assert_eq!(batch["tip"], status["tip"]);
    // Asked for, each record comes with its Merkle proof; the proof flag
    // takes only true or false.
    let (status, proved) = http(
        api,
        "GET",
        &format!(
            "/v1/contracts/{contract}/storage/{}?proof=true",
            hex(&[0; 32])
        ),
        None,
        None,
    )
    .await;
    assert_eq!(status, 200, "{proved}");
    let proved: Value = serde_json::from_str(&proved).unwrap();
    assert_eq!(proved["proof"]["end"], "present");
    let (status, proved) = http(
        api,
        "POST",
        "/v1/storage",
        Some(json!({"reads":[read([1; 32])],"proof":true})),
        None,
    )
    .await;
    assert_eq!(status, 200, "{proved}");
    let proved: Value = serde_json::from_str(&proved).unwrap();
    assert!(proved["values"][0]["proof"]["siblings"].is_array());
    let (status, _) = http(
        api,
        "GET",
        &format!(
            "/v1/contracts/{contract}/storage/{}?proof=yes",
            hex(&[0; 32])
        ),
        None,
        None,
    )
    .await;
    assert_eq!(status, 400);
    for refused in [
        json!({"reads":[]}),
        json!({"reads":vec![read([0; 32]); 1_025]}),
        json!({"reads":[{"contract":contract,"key":hex(&[0; 32]),"extra":1}]}),
        json!({"reads":[{"contract":contract,"key":"zz"}]}),
    ] {
        let (status, body) = http(api, "POST", "/v1/storage", Some(refused), None).await;
        assert!((400..500).contains(&status), "{status} {body}");
        assert!(serde_json::from_str::<Value>(&body).unwrap()["error"].is_string());
    }
    // The raw block comes whole, or in slices that add up to it.
    let (status, whole) = http(api, "GET", "/v1/blocks/4/raw", None, None).await;
    assert_eq!(status, 200);
    let whole: Value = serde_json::from_str(&whole).unwrap();
    let total = whole["total"].as_u64().unwrap() as usize;
    assert_eq!(whole["hex"].as_str().unwrap().len(), 2 * total);
    assert_eq!(whole["offset"], 0);
    let (mut pieces, mut offset) = (String::new(), 0);
    while offset < total {
        let (status, slice) = http(
            api,
            "GET",
            &format!("/v1/blocks/4/raw?offset={offset}&length=100"),
            None,
            None,
        )
        .await;
        assert_eq!(status, 200);
        let slice: Value = serde_json::from_str(&slice).unwrap();
        assert_eq!(slice["offset"], offset);
        assert_eq!(slice["total"], total);
        let part = slice["hex"].as_str().unwrap();
        assert!(!part.is_empty() && part.len() <= 200);
        pieces.push_str(part);
        offset += part.len() / 2;
    }
    assert_eq!(whole["hex"], pieces);
    for bad in [
        "?length=1048577".to_owned(),
        format!("?offset={}", total + 1),
        "?slice=1".to_owned(),
    ] {
        let (status, body) = http(api, "GET", &format!("/v1/blocks/4/raw{bad}"), None, None).await;
        assert_eq!(status, 400, "{bad}");
        assert!(
            serde_json::from_str::<Value>(&body).unwrap()["error"].is_string(),
            "{bad}: {body}"
        );
    }
    // The operator leaves the tip and comes back to it.
    let tip = a.call("status", json!({})).await["tip"].clone();
    let left = command(&a.dir.0, &["invalidate", "4"]);
    assert_eq!(left["height"], 3);
    let back = command(&a.dir.0, &["reconsider", tip.as_str().unwrap()]);
    assert_eq!(back["height"], 4);
    assert_eq!(back["tip"], tip);
    let before = a.call("status", json!({})).await;
    command(&a.dir.0, &["api", "disable"]);
    assert!(TcpStream::connect(api).await.is_err());
    assert_eq!(command(&a.dir.0, &["status"])["height"], 4);
    a.stop().await;
    command(&a.dir.0, &["start"]);
    let restarted = command(&a.dir.0, &["status"]);
    assert_eq!(restarted["tip"], before["tip"]);
    assert_eq!(restarted["state_root"], before["state_root"]);
    assert_eq!(restarted["listeners"]["api"], Value::Null);
    // Removed, b is no longer a configured peer. With discovery on, the node
    // may still learn its address again and connect to it as a discovered one.
    command(&a.dir.0, &["remove", "node", &b.info.p2p.to_string()]);
    let peers = command(&a.dir.0, &["peers"]);
    assert!(
        peers["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["source"] != "config"),
        "{peers}"
    );
    command(&a.dir.0, &["stop"]);
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_genesis_control_auth_and_frame_bounds_are_enforced() {
    let n = Node::start(false, "test");
    // A wrong token: the node's proof does not check, and nothing is sent.
    assert_eq!(
        transport::control_call(n.info.control, "wrong", "mine", json!({}))
            .await
            .unwrap_err(),
        transport::NOT_THIS_NODE
    );
    // A client that skips that check and sends a request with a wrong proof
    // is refused.
    let mut stream = TcpStream::connect(n.info.control).await.unwrap();
    transport::write_frame(&mut stream, &json!({"nonce":hex(&[7; 32])}))
        .await
        .unwrap();
    let answer = transport::read_frame(&mut stream).await.unwrap();
    assert_eq!(answer["proof"].as_str().unwrap().len(), 64);
    transport::write_frame(
        &mut stream,
        &json!({"request":r#"{"method":"stop","params":{}}"#,"proof":hex(&[0; 32])}),
    )
    .await
    .unwrap();
    assert_eq!(
        transport::read_frame(&mut stream).await.unwrap()["error"],
        "unauthorized"
    );
    // Whatever listens where a stale runtime.json points learns nothing: it
    // receives a nonce, cannot prove it holds the token, and gets no request.
    let impostor = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = impostor.local_addr().unwrap();
    let received = tokio::spawn(async move {
        let (mut stream, _) = impostor.accept().await.unwrap();
        let mut frames = vec![transport::read_frame(&mut stream).await.unwrap()];
        let answer = json!({"nonce":hex(&[1; 32]),"proof":hex(&[2; 32])});
        transport::write_frame(&mut stream, &answer).await.unwrap();
        while let Ok(Ok(frame)) =
            tokio::time::timeout(Duration::from_secs(1), transport::read_frame(&mut stream)).await
        {
            frames.push(frame);
        }
        frames
    });
    let token = config::token(&n.dir.0).unwrap();
    assert_eq!(
        transport::control_call(address, &token, "stop", json!({}))
            .await
            .unwrap_err(),
        transport::NOT_THIS_NODE
    );
    let frames = received.await.unwrap();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert!(!frames[0].to_string().contains(&token));
    // A connection that says nothing is closed within the frame deadline, and
    // holds no inbound place.
    let mut silent = TcpStream::connect(n.info.p2p).await.unwrap();
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(10), silent.read(&mut byte)).await;
    assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))), "{closed:?}");
    let chain_id = chain_id_of(&n).await;
    let (_, mismatch) = greet(n.info.p2p, hello([0; 32])).await;
    assert!(mismatch.unwrap_err().contains("genesis"));
    let mut stream = TcpStream::connect(n.info.p2p).await.unwrap();
    stream
        .write_u32_le(ting_node::FRAME_MAX as u32 + 1)
        .await
        .unwrap();
    assert!(
        transport::read_wire(&mut stream, None, ting_node::FRAME_MAX)
            .await
            .is_err()
    );
    // A request the node does not know ends the connection.
    let (mut stream, answer) = greet(n.info.p2p, hello(chain_id)).await;
    assert!(matches!(answer, Ok(wire::Answer::State(_))));
    raw_frame(&mut stream, &[1, 99]).await;
    assert!(
        transport::read_wire(&mut stream, None, ting_node::FRAME_MAX)
            .await
            .is_err()
    );
    // A valid block from more than two hours ahead is refused for now, and
    // that one refusal keeps the connection open, unlike every other failure.
    let g = config::genesis(&n.dir.0).unwrap();
    let mut early = Runtime::new(State::default(), Context::from_genesis(&g))
        .build_candidate(
            &g,
            &[],
            vec![],
            ting_node::now() + ting_node::chain::CLOCK_SLACK + 60,
            [0; 4],
            vec![],
        )
        .unwrap();
    while !consensus::valid_pow(&early.header) {
        early.header.nonce += 1;
    }
    let (mut stream, _) = greet(n.info.p2p, hello(chain_id)).await;
    assert_eq!(
        transport::ask(&mut stream, wire::Request::Import(whole(&early.encode())))
            .await
            .unwrap_err(),
        ting_node::chain::AHEAD_OF_CLOCK
    );
    let headers = transport::ask(
        &mut stream,
        wire::Request::Headers {
            locator: vec![g.block_id()],
        },
    )
    .await
    .unwrap();
    assert_eq!(headers, wire::Answer::Headers(vec![]));
    // An error that is an answer keeps the connection: a block it does not have.
    assert_eq!(
        transport::ask(
            &mut stream,
            wire::Request::Raw {
                id: [7; 32],
                offset: 0,
                length: 1024
            }
        )
        .await
        .unwrap_err(),
        "block not found"
    );
    let Ok(wire::Answer::State(state)) = transport::ask(&mut stream, wire::Request::State).await
    else {
        panic!("no state");
    };
    assert_eq!(state.height, 0);
    // A frame that is not a message, here one byte too many, ends the
    // connection before anything is done with it.
    let mut padded = wire::Frame::Request(wire::Request::State).encode();
    padded.push(0);
    raw_frame(&mut stream, &padded).await;
    assert!(
        transport::read_wire(&mut stream, None, ting_node::FRAME_MAX)
            .await
            .is_err()
    );
    // A first frame larger than a hello needs is not read.
    let mut stream = TcpStream::connect(n.info.p2p).await.unwrap();
    raw_frame(&mut stream, &vec![1; 5_000]).await;
    assert!(
        transport::read_wire(&mut stream, None, ting_node::FRAME_MAX)
            .await
            .is_err()
    );
    assert_eq!(n.height().await, 0);
    assert_eq!(n.call("status", json!({})).await["stored_blocks"], 0);
    // Discovery over the same protocol: a connecting peer's announced
    // listening port is recorded at the address it came from, an unusable
    // one is not, a node refuses its own nonce, and a peer without
    // discovery still gets an (empty) answer to `addresses`.
    let known = n.call("peers", json!({})).await["known"].as_u64().unwrap();
    let listening = |port: u16| wire::Hello {
        listen: Some(port),
        ..hello(chain_id)
    };
    let (mut stream, remote) = greet(n.info.p2p, listening(0)).await;
    let Ok(wire::Answer::State(remote)) = remote else {
        panic!("no hello");
    };
    assert_ne!(remote.nonce, 0);
    assert_eq!(
        n.call("peers", json!({})).await["known"].as_u64().unwrap(),
        known
    );
    let shared = transport::ask(&mut stream, wire::Request::Addresses)
        .await
        .unwrap();
    assert_eq!(shared, wire::Answer::Addresses(vec![]));
    let (mut stream, _) = greet(n.info.p2p, listening(18442)).await;
    let peers = n.call("peers", json!({})).await;
    assert_eq!(peers["known"].as_u64().unwrap(), known + 1);
    // An unverified address is never shared.
    assert_eq!(
        transport::ask(&mut stream, wire::Request::Addresses)
            .await
            .unwrap(),
        wire::Answer::Addresses(vec![])
    );
    let own = wire::Hello {
        nonce: remote.nonce,
        ..hello(chain_id)
    };
    let (_, itself) = greet(n.info.p2p, own).await;
    assert_eq!(itself.unwrap_err(), "connected to itself");
    // Invalid data ends the connection (a peer on this machine is not banned).
    let (mut stream, _) = greet(n.info.p2p, hello(chain_id)).await;
    assert_eq!(
        transport::ask(&mut stream, wire::Request::Import(whole(&[0])))
            .await
            .unwrap_err(),
        "invalid block: truncated header"
    );
    assert!(
        transport::ask(&mut stream, wire::Request::State)
            .await
            .is_err()
    );
    // A peer that asks for announcements is told of a new tip between
    // answers; `state` is its own hello again.
    let (mut stream, _) = greet(
        n.info.p2p,
        wire::Hello {
            announce: true,
            ..hello(chain_id)
        },
    )
    .await;
    let Ok(wire::Answer::State(state)) = transport::ask(&mut stream, wire::Request::State).await
    else {
        panic!("no state");
    };
    assert_eq!(state.height, 0);
    n.call("mine", json!({})).await;
    let told = transport::read_wire(&mut stream, None, ting_node::FRAME_MAX)
        .await
        .unwrap();
    let wire::Frame::Announcement(told) = told else {
        panic!("{told:?}");
    };
    assert_eq!(told.height, 1);
    assert_eq!(
        json!(hex(&told.tip)),
        n.call("status", json!({})).await["tip"]
    );
    let running = config::lock(&n.dir.0).err().unwrap();
    assert!(running.contains("locked"));
    n.stop().await;
    // The announced address reached the book on disk.
    let book: Value =
        serde_json::from_slice(&std::fs::read(n.dir.0.join("peers.json")).unwrap()).unwrap();
    let entry = book["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["address"] == "127.0.0.1:18442")
        .unwrap_or_else(|| panic!("{book}"));
    assert_eq!(entry["source"], "inbound");
    assert_eq!(entry["verified"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nodes_discover_each_other_through_a_common_peer() {
    // A and C are each configured with B only. B learns their listening
    // addresses when they connect, synchronizes with them, and shares each
    // with the other, so A and C end up synchronizing directly.
    let a = Node::start(false, "discovery A");
    let b = Node::start(false, "discovery B");
    let c = Node::start(false, "discovery C");
    command(&a.dir.0, &["add", "node", &b.info.p2p.to_string()]);
    command(&c.dir.0, &["add", "node", &b.info.p2p.to_string()]);
    let synced_with = |peers: &Value, other: &Node| {
        peers["items"].as_array().unwrap().iter().any(|p| {
            p["address"] == other.info.p2p.to_string()
                && p["source"] == "discovered"
                && p["last_success"].is_u64()
        })
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let (pa, pc) = (
            a.call("peers", json!({})).await,
            c.call("peers", json!({})).await,
        );
        if synced_with(&pa, &c) && synced_with(&pc, &a) {
            assert_eq!(pa["discovery"], true);
            assert!(pa["known"].as_u64().unwrap() >= 2, "{pa}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no discovery: A {pa}, C {pc}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // A block mined on C reaches A.
    command(&c.dir.0, &["mine"]);
    a.wait_height(1).await;
    // No node lists itself, and the book survives in the data directory.
    for node in [&a, &b, &c] {
        let peers = node.call("peers", json!({})).await;
        assert!(
            !peers["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["address"] == node.info.p2p.to_string()),
            "{peers}"
        );
    }
    a.stop().await;
    let book: Value =
        serde_json::from_slice(&std::fs::read(a.dir.0.join("peers.json")).unwrap()).unwrap();
    assert!(book["addresses"].as_array().unwrap().len() >= 2, "{book}");
    b.stop().await;
    c.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_forks_sync_across_more_than_one_header_page() {
    let a = Node::start(false, "fork A");
    let b = Node::start(false, "fork B");
    // Without producer data of its own, B would mine A's very blocks.
    command(&b.dir.0, &["miner", "producer", "bb"]);
    command(&a.dir.0, &["mine", "--count", "100"]);
    command(&b.dir.0, &["mine", "--count", "100"]);
    assert_ne!(
        a.call("status", json!({})).await["tip"],
        b.call("status", json!({})).await["tip"]
    );
    command(&a.dir.0, &["mine", "--count", "40"]);
    command(&b.dir.0, &["mine", "--count", "41"]);
    let target = b.call("status", json!({})).await["tip"].clone();
    command(&a.dir.0, &["add", "node", &b.info.p2p.to_string()]);
    a.wait_height(141).await;
    assert_eq!(a.call("status", json!({})).await["tip"], target);
    assert_eq!(b.call("status", json!({})).await["tip"], target);
    a.stop().await;
    b.stop().await;
}
#[tokio::test]
async fn a_new_node_syncs_a_long_chain_and_carries_on_after_a_restart() {
    // A new node follows a peer 1,000 blocks ahead over one connection.
    // Stopped in the middle and started again, it carries on from what it
    // stored, and its connection never fails.
    let a = Node::start(false, "long A");
    for _ in 0..10 {
        command(&a.dir.0, &["mine", "--count", "100"]);
    }
    let mut b = Node::start(false, "long B");
    command(&b.dir.0, &["add", "node", &a.info.p2p.to_string()]);
    b.wait_height(300).await;
    b.stop().await;
    b.restart();
    let resumed = b.height().await;
    assert!(resumed >= 300, "{resumed}");
    b.wait_height(1_000).await;
    assert_eq!(
        b.call("status", json!({})).await["tip"],
        a.call("status", json!({})).await["tip"]
    );
    let peers = b.call("peers", json!({})).await;
    assert!(
        peers["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["error"].is_null()),
        "{peers}"
    );
    a.stop().await;
    b.stop().await;
}
#[tokio::test]
async fn built_in_miner_starts_stops_and_mines_through_templates() {
    let n = Node::start(false, "miner");
    assert_eq!(n.call("miner", json!({})).await["running"], false);
    // Votes set on the running node go into every block it builds next.
    let set = n
        .call("miner", json!({"action":"votes","votes":[0, 1, 0, 0]}))
        .await;
    assert_eq!(set["votes"], json!([0, 1, 0, 0]));
    assert_eq!(
        n.call("miner", json!({})).await["votes"],
        json!([0, 1, 0, 0])
    );
    let started = n
        .call("miner", json!({"action":"start","threads":1,"spacing":0}))
        .await;
    assert_eq!(started["running"], true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = n.call("miner", json!({"action":"status"})).await;
        if status["blocks"].as_u64().unwrap_or(0) >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "miner found nothing: {status}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let stopped = n.call("miner", json!({"action":"stop"})).await;
    assert_eq!(stopped["running"], false);
    let voting = n.call("votes", json!({})).await;
    assert_eq!(voting["votes"], json!([0, 1, 0, 0]));
    assert_eq!(
        voting["this_epoch"]["increase_percent"],
        json!([0.0, 100.0, 0.0, 0.0])
    );
    let height = n.height().await;
    assert!(height >= 2, "height {height}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(n.height().await, height, "stopped miner keeps mining");
    assert!(!config::Config::load(&n.dir.0).unwrap().miner_enabled);
    // A template can be asked for and solved by hand, like an external miner would.
    let template = n.call("template", json!({})).await;
    assert_eq!(template["height"].as_u64().unwrap(), height + 1);
    n.stop().await;
}

/// A request with its own headers; the answer's status, its header block in
/// lower case, and its body.
async fn http_full(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8(bytes).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        head.to_ascii_lowercase(),
        body.into(),
    )
}
fn error_of(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap_or_else(|_| panic!("not JSON: {body}"))["error"]
        .as_str()
        .unwrap_or_else(|| panic!("no error string: {body}"))
        .to_owned()
}
/// A submission's headers and body, and the status it should get.
type Submission<'a> = (&'a [(&'a str, &'a str)], String, u16);
const PROTECTED: [&str; 3] = [
    "x-content-type-options: nosniff",
    "referrer-policy: no-referrer",
    "cache-control: no-store",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_answers_with_the_documented_statuses_headers_and_cors() {
    let n = Node::start(true, "rest");
    let api = n.info.api.unwrap();
    command(&n.dir.0, &["mine"]);
    let json = [("Content-Type", "application/json")];
    // Not found, as JSON.
    let missing = hex(&[9; 32]);
    for path in [
        "/v1/blocks/99".to_owned(),
        format!("/v1/transactions/{missing}"),
        format!("/v1/contracts/{missing}"),
        "/v1/nothing".to_owned(),
        "/assets/nothing.css".to_owned(),
    ] {
        let (status, head, body) = http_full(api, "GET", &path, &[], "").await;
        assert_eq!(status, 404, "{path}");
        assert!(!error_of(&body).is_empty(), "{path}");
        for header in PROTECTED {
            assert!(head.contains(header), "{path}: {head}");
        }
    }
    // Malformed requests, as JSON.
    for path in [
        "/v1/blocks/zz".to_owned(),
        "/v1/blocks/0".to_owned(),
        "/v1/contracts/abc".to_owned(),
        "/v1/blocks?from=x".to_owned(),
        "/v1/events?page=2".to_owned(),
        "/v1/events?topic=abababababababababababababababababababababababababababababababab"
            .to_owned(),
        "/v1/events?contract=zz".to_owned(),
        "/v1/transactions/{missing}?limit=101".replace("{missing}", &missing),
        format!("/v1/contracts/{missing}/storage/zz"),
    ] {
        let (status, _, body) = http_full(api, "GET", &path, &[], "").await;
        assert_eq!(status, 400, "{path}: {body}");
        assert!(!error_of(&body).is_empty(), "{path}");
    }
    // Submissions that are not JSON, or JSON of the wrong shape.
    let tx = hex(&support::tx().encode());
    let cases: [Submission; 4] = [
        (&json, format!(r#"{{"hex":"{tx}","extra":1}}"#), 422),
        (&json, r#"{"hex":5}"#.to_owned(), 422),
        (&json, "not json".to_owned(), 400),
        (
            &[("Content-Type", "text/plain")],
            format!(r#"{{"hex":"{tx}"}}"#),
            415,
        ),
    ];
    for (headers, body, expected) in cases {
        let (status, head, answer) =
            http_full(api, "POST", "/v1/transactions", headers, &body).await;
        assert_eq!(status, expected, "{body}: {answer}");
        assert!(!error_of(&answer).is_empty(), "{body}");
        assert!(head.contains("content-type: application/json"), "{head}");
    }
    // A body larger than any transaction the next block could hold, as hex.
    let capacity = n.call("status", json!({})).await["limits"][2]
        .as_u64()
        .unwrap() as usize;
    let oversized = format!(r#"{{"hex":"{}"}}"#, "0".repeat(2 * capacity + 4096));
    let (status, _, answer) = http_full(api, "POST", "/v1/transactions", &json, &oversized).await;
    assert_eq!(status, 413, "{answer}");
    assert!(!error_of(&answer).is_empty());
    let (status, _, body) = http_full(api, "GET", "/v1/transactions", &[], "").await;
    assert_eq!(status, 405);
    assert_eq!(error_of(&body), "Method Not Allowed");
    // A good submission is known the second time, and deferred with a time.
    let envelope = format!(r#"{{"hex":"{tx}"}}"#);
    let (_, _, first) = http_full(api, "POST", "/v1/transactions", &json, &envelope).await;
    let (_, _, second) = http_full(api, "POST", "/v1/transactions", &json, &envelope).await;
    let (first, second): (Value, Value) = (
        serde_json::from_str(&first).unwrap(),
        serde_json::from_str(&second).unwrap(),
    );
    assert_eq!(
        (first["known"].clone(), second["known"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(first["id"], second["id"]);
    let later = {
        let mut tx = support::tx();
        if let ting_core::protocol::Payload::Deploy { input, .. } = &mut tx.payload {
            input.push(1);
        }
        hex(&tx.encode())
    };
    let not_before = ting_node::now() + 3600;
    let (status, _, deferred) = http_full(
        api,
        "POST",
        "/v1/transactions",
        &json,
        &format!(r#"{{"hex":"{later}","not_before":{not_before}}}"#),
    )
    .await;
    assert_eq!(status, 200);
    let deferred: Value = serde_json::from_str(&deferred).unwrap();
    assert_eq!(deferred["deferred"], true);
    assert_eq!(deferred["not_before"], not_before);
    let (_, _, health) = http_full(api, "GET", "/health", &[], "").await;
    let health: Value = serde_json::from_str(&health).unwrap();
    assert_eq!(health["ready"], true);
    assert_eq!(health["height"], 1);
    assert!(health["tip"].is_string());
    let (_, _, mempool) = http_full(api, "GET", "/v1/mempool", &[], "").await;
    let mempool: Value = serde_json::from_str(&mempool).unwrap();
    assert_eq!(mempool["items"].as_array().unwrap().len(), 1);
    assert_eq!(mempool["deferred"].as_array().unwrap().len(), 1);
    // Every answer carries the protective headers; none allows other origins
    // until CORS is turned on, which works while REST is running.
    let (_, head, _) =
        http_full(api, "GET", "/v1/status", &[("Origin", "http://a.test")], "").await;
    for header in PROTECTED {
        assert!(head.contains(header), "{head}");
    }
    assert!(!head.contains("access-control-allow-origin"), "{head}");
    let enabled = command(&n.dir.0, &["api", "enable", "--cors"]);
    assert_eq!(enabled["enabled"], true);
    let api: SocketAddr = enabled["address"].as_str().unwrap().parse().unwrap();
    assert!(config::Config::load(&n.dir.0).unwrap().api_cors);
    let (_, head, _) =
        http_full(api, "GET", "/v1/status", &[("Origin", "http://a.test")], "").await;
    assert!(head.contains("access-control-allow-origin: *"), "{head}");
    let (status, head, _) = http_full(
        api,
        "OPTIONS",
        "/v1/transactions",
        &[
            ("Origin", "http://a.test"),
            ("Access-Control-Request-Method", "POST"),
            ("Access-Control-Request-Headers", "content-type"),
        ],
        "",
    )
    .await;
    assert!(status == 200 || status == 204, "{status}");
    assert!(head.contains("access-control-allow-origin: *"), "{head}");
    for header in PROTECTED {
        assert!(head.contains(header), "{head}");
    }
    let plain = command(&n.dir.0, &["api", "enable"]);
    let api: SocketAddr = plain["address"].as_str().unwrap().parse().unwrap();
    assert!(!config::Config::load(&n.dir.0).unwrap().api_cors);
    let (status, head, _) =
        http_full(api, "GET", "/v1/status", &[("Origin", "http://a.test")], "").await;
    assert_eq!(status, 200);
    assert!(!head.contains("access-control-allow-origin"), "{head}");
    assert_eq!(n.height().await, 1);
    n.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cli_edits_a_stopped_node_and_reports_what_it_cannot_do() {
    let dir = Directory::new();
    command(
        &dir.0,
        &[
            "init",
            "--dev",
            "--name",
            "offline",
            "--control",
            "127.0.0.1:0",
            "--p2p",
            "127.0.0.1:0",
            "--api-bind",
            "127.0.0.1:0",
        ],
    );
    let again = cli(&dir.0, &["init", "--dev"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already initialized"));
    // Edits while stopped go to the file.
    let saved = command(&dir.0, &["add", "node", "127.0.0.1:9"]);
    assert_eq!(
        (saved["saved"].clone(), saved["running"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(saved["peers"], json!(["127.0.0.1:9"]));
    let load = || config::Config::load(&dir.0).unwrap();
    assert_eq!(load().peers.len(), 1);
    command(&dir.0, &["remove", "node", "127.0.0.1:9"]);
    assert!(load().peers.is_empty());
    command(&dir.0, &["miner", "producer", "00ff"]);
    assert_eq!(load().producer_hex.as_deref(), Some("00ff"));
    command(&dir.0, &["miner", "producer"]);
    assert_eq!(load().producer_hex, None);
    command(
        &dir.0,
        &["miner", "votes", "increase", "hold", "hold", "decrease"],
    );
    assert_eq!(load().votes, [1, 0, 0, 2]);
    for bad in [
        &["miner", "votes", "up", "hold", "hold", "hold"][..],
        &["miner", "votes", "hold", "hold", "hold"],
    ] {
        assert!(!cli(&dir.0, bad).status.success(), "{bad:?}");
    }
    assert_eq!(load().votes, [1, 0, 0, 2]);
    command(&dir.0, &["api", "enable", "--cors"]);
    assert!(load().api_enabled && load().api_cors);
    command(&dir.0, &["api", "disable"]);
    assert!(!load().api_enabled && !load().api_cors);
    let bad = cli(&dir.0, &["miner", "start", "--threads", "0"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("invalid configuration"));
    assert!(!load().miner_enabled);
    // What needs a running node says so.
    for args in [&["stop"][..], &["docs"], &["status"], &["mine"]] {
        let out = cli(&dir.0, args);
        assert!(!out.status.success(), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}");
    }
    // Running: `start` again is the status, `docs` needs REST, and the
    // miner's state is remembered across a restart.
    // A development chain starts with its seed.
    let seed = ting_node::seed::ids(&config::development_genesis()).len();
    let started = command(&dir.0, &["start"]);
    assert_eq!(started["height"], seed);
    assert_eq!(command(&dir.0, &["start"])["height"], seed);
    let docs = cli(&dir.0, &["docs"]);
    assert!(!docs.status.success());
    assert!(String::from_utf8_lossy(&docs.stderr).contains("REST is disabled"));
    let miner = command(&dir.0, &["miner", "start", "--spacing", "3600"]);
    assert_eq!(miner["running"], true);
    assert!(load().miner_enabled);
    command(&dir.0, &["stop"]);
    command(&dir.0, &["start"]);
    assert_eq!(command(&dir.0, &["miner", "status"])["running"], true);
    assert_eq!(command(&dir.0, &["miner", "stop"])["running"], false);
    assert!(!load().miner_enabled);
    command(&dir.0, &["stop"]);
    assert!(!dir.0.join("runtime.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_that_answers_with_another_block_is_caught_at_the_first_slice() {
    // A fake peer offers three valid headers, then answers the request for
    // the first block with the second. The node must not take it, nor ask
    // again in a loop: the connection ends with the reason.
    let b = Node::start(false, "victim");
    let g = config::genesis(&b.dir.0).unwrap();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = Vec::new();
    let mut blocks = Vec::new();
    let start = ting_node::now();
    for i in 0..3 {
        let mut block = runtime
            .build_candidate(&g, &history, vec![], start + i, [0; 4], vec![])
            .unwrap();
        while !consensus::valid_pow(&block.header) {
            block.header.nonce += 1;
        }
        runtime
            .validate_and_apply_block(&g, &history, &block)
            .unwrap();
        history.push(block.header.clone());
        blocks.push(block);
    }
    let chain_id = g.chain_id();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = listener.local_addr().unwrap();
    let served = blocks.clone();
    fake_peer(listener, move |request, sent_headers| match request {
        wire::Request::Hello(_) | wire::Request::State => Ok(wire::Answer::State(fake_state(
            chain_id,
            served[2].header.id(),
            0xaa,
        ))),
        wire::Request::Headers { .. } if !*sent_headers => {
            *sent_headers = true;
            Ok(wire::Answer::Headers(
                served
                    .iter()
                    .map(|b| b.header.encode().try_into().unwrap())
                    .collect(),
            ))
        }
        wire::Request::Headers { .. } => Ok(wire::Answer::Headers(vec![])),
        // Asked for the first block, it sends the second.
        wire::Request::Raw { .. } => {
            let bytes = served[1].encode();
            Ok(wire::Answer::Slice {
                offset: 0,
                total: bytes.len() as u64,
                bytes,
            })
        }
        wire::Request::Inventory { .. } => Ok(wire::Answer::Inventory {
            ids: vec![],
            next: None,
        }),
        wire::Request::Addresses => Ok(wire::Answer::Addresses(vec![])),
        _ => Err("not served".into()),
    });
    command(&b.dir.0, &["add", "node", &fake.to_string()]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let peers = b.call("peers", json!({})).await;
        let error = peers["items"][0]["error"].as_str().unwrap_or("").to_owned();
        if error == "invalid block: not the block asked for" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{peers}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(b.height().await, 0);
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_block_pushed_in_slices_is_assembled_in_order_within_its_capacity() {
    let n = Node::start(false, "assembler");
    let g = config::genesis(&n.dir.0).unwrap();
    let mut block = Runtime::new(State::default(), Context::from_genesis(&g))
        .build_candidate(&g, &[], vec![], ting_node::now(), [0; 4], vec![])
        .unwrap();
    while !consensus::valid_pow(&block.header) {
        block.header.nonce += 1;
    }
    let bytes = block.encode();
    let total = bytes.len();
    let (mut stream, _) = greet(n.info.p2p, hello(chain_id_of(&n).await)).await;
    let slice = |from: usize, to: usize| {
        wire::Request::Import(wire::Part {
            offset: from as u64,
            total: total as u64,
            bytes: bytes[from..to.min(total)].to_vec(),
        })
    };
    // The header first, then the rest four bytes at a time.
    let mut cuts: Vec<usize> = (HEADER_LEN..total).step_by(4).collect();
    cuts.insert(0, 0);
    cuts.push(total);
    assert!(cuts.len() > 3);
    // A slice with nothing before it, and one that skips ahead, are refused
    // without ending the connection.
    assert_eq!(
        transport::ask(&mut stream, slice(cuts[1], cuts[2]))
            .await
            .unwrap_err(),
        "nothing being pushed"
    );
    transport::ask(&mut stream, slice(0, cuts[1]))
        .await
        .unwrap();
    assert_eq!(
        transport::ask(&mut stream, slice(cuts[2], cuts[3]))
            .await
            .unwrap_err(),
        "slice out of order"
    );
    // In order, the last slice imports the block.
    let mut answer = wire::Answer::Received(0);
    for pair in cuts.windows(2) {
        answer = transport::ask(&mut stream, slice(pair[0], pair[1]))
            .await
            .unwrap();
        if pair[1] < total {
            assert_eq!(answer, wire::Answer::Received(pair[1] as u64));
        }
    }
    assert_eq!(
        answer,
        wire::Answer::Taken {
            id: block.header.id(),
            known: false
        }
    );
    assert_eq!(n.height().await, 1);
    // Pushed again, the first slice answers that the block is known.
    assert_eq!(
        transport::ask(&mut stream, slice(0, cuts[1]))
            .await
            .unwrap(),
        wire::Answer::Taken {
            id: block.header.id(),
            known: true
        }
    );
    // A block claiming more bytes than its capacity is invalid data.
    let next = Runtime::new(State::default(), Context::from_genesis(&g));
    next.validate_and_apply_block(&g, &[], &block).unwrap();
    let mut second = next
        .build_candidate(
            &g,
            &[block.header.clone()],
            vec![],
            block.header.timestamp + 1,
            [0; 4],
            vec![],
        )
        .unwrap();
    while !consensus::valid_pow(&second.header) {
        second.header.nonce += 1;
    }
    let capacity = n.call("status", json!({})).await["limits"][2]
        .as_u64()
        .unwrap();
    let second = second.encode();
    let error = transport::ask(
        &mut stream,
        wire::Request::Import(wire::Part {
            offset: 0,
            total: capacity + 1,
            bytes: second,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error, "invalid block: larger than its capacity");
    n.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_to_a_peer_on_a_deep_fork_moves_past_the_blocks_it_shares() {
    // B shares A's first 50 blocks, then runs 80 deep on a branch of its own.
    // B never connects to A, so A's blocks reach it only by A's push, and B's
    // locator names no shared block but genesis: past the first 32 shared
    // blocks the push must carry on from the last one B took.
    let a = Node::start(false, "push A");
    let mut b = Node::start(false, "push B");
    command(&a.dir.0, &["mine", "--count", "50"]);
    command(&b.dir.0, &["add", "node", &a.info.p2p.to_string()]);
    b.wait_height(50).await;
    b.stop().await;
    let path = b.dir.0.join("config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["peers"] = json!([]);
    config["discovery"] = json!(false);
    // Without producer data of its own, B would mine A's very blocks.
    config["producer_hex"] = json!("bb");
    std::fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    b.restart();
    command(&b.dir.0, &["mine", "--count", "80"]);
    command(&a.dir.0, &["mine", "--count", "90"]);
    let target = a.call("status", json!({})).await["tip"].clone();
    command(&a.dir.0, &["add", "node", &b.info.p2p.to_string()]);
    b.wait_height(140).await;
    assert_eq!(b.call("status", json!({})).await["tip"], target);
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_node_answers_while_ting_mine_searches() {
    // The nonce search runs off the chain thread: a status asked in the
    // middle of a hundred blocks sees some of them, not none or all.
    let n = Node::start(false, "mine");
    let dir = n.dir.0.clone();
    let mining = std::thread::spawn(move || command(&dir, &["mine", "--count", "100"]));
    let mut seen = std::collections::BTreeSet::new();
    while !mining.is_finished() {
        seen.insert(n.height().await);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        mining.join().unwrap()["blocks"].as_array().unwrap().len(),
        100
    );
    assert!(seen.iter().any(|h| (1..100).contains(h)), "{seen:?}");
    let token = config::token(&n.dir.0).unwrap();
    for params in [
        json!({"count":0}),
        json!({"count":101}),
        json!({"attempts":0}),
    ] {
        assert_eq!(
            transport::control_call(n.info.control, &token, "mine", params)
                .await
                .unwrap_err(),
            "count must be 1..100 and attempts 1..10000000"
        );
    }
    n.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_built_in_miner_waits_while_the_node_is_behind_its_best_header() {
    // A fake peer shows three valid headers and never serves their blocks:
    // the node stays at height 0 behind a best header at 3, and its miner
    // waits rather than mine a block about to be replaced.
    let n = Node::start(false, "behind");
    let g = config::genesis(&n.dir.0).unwrap();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = Vec::new();
    let start = ting_node::now();
    for i in 0..3 {
        let mut block = runtime
            .build_candidate(&g, &history, vec![], start + i, [0; 4], vec![])
            .unwrap();
        while !consensus::valid_pow(&block.header) {
            block.header.nonce += 1;
        }
        runtime
            .validate_and_apply_block(&g, &history, &block)
            .unwrap();
        history.push(block.header.clone());
    }
    let chain_id = g.chain_id();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = listener.local_addr().unwrap();
    fake_peer(listener, move |request, _| match request {
        wire::Request::Hello(_) | wire::Request::State => Ok(wire::Answer::State(fake_state(
            chain_id,
            history[2].id(),
            0xbb,
        ))),
        wire::Request::Headers { .. } => Ok(wire::Answer::Headers(
            history
                .iter()
                .map(|h| h.encode().try_into().unwrap())
                .collect(),
        )),
        wire::Request::Raw { .. } => Err("block not found".into()),
        wire::Request::Inventory { .. } => Ok(wire::Answer::Inventory {
            ids: vec![],
            next: None,
        }),
        wire::Request::Addresses => Ok(wire::Answer::Addresses(vec![])),
        _ => Err("not served".into()),
    });
    command(&n.dir.0, &["add", "node", &fake.to_string()]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while n.call("status", json!({})).await["best_height"] != 3 {
        assert!(tokio::time::Instant::now() < deadline, "no headers");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    n.call("miner", json!({"action":"start","threads":1,"spacing":0}))
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = n.call("miner", json!({"action":"status"})).await;
        if status["last_error"]
            .as_str()
            .is_some_and(|e| e.starts_with("waiting"))
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{status}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(n.height().await, 0);
    n.call("miner", json!({"action":"stop"})).await;
    n.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transactions_larger_than_a_frame_travel_in_slices() {
    // A chain whose blocks hold 24 MiB of data: a 10 MiB transaction is 20 MiB
    // of hex, more than one 16 MiB frame. One goes in through REST at A and
    // B, connected to A, fetches it in slices; one deferred goes in through
    // the control channel at B and B pushes it to A in slices.
    use ting_core::protocol::{Payload, Transaction};
    let files = Directory::new();
    let mut g = config::development_genesis();
    g.initial.0[2] = 24 << 20;
    let path = files.0.join("genesis.bin");
    std::fs::write(&path, g.encode()).unwrap();
    let a = Node::start_on(Some(&path), true, "large A");
    let b = Node::start_on(Some(&path), false, "large B");
    let large = |byte: u8| Transaction {
        budget: ting_core::meter::Resources([1_000_000, 1, 11 << 20, 1]),
        payload: Payload::Call {
            target: [1; 32],
            input: vec![byte; 10 << 20],
        },
    };
    let (pending, deferred) = (large(1), large(2));
    let body = format!(r#"{{"hex":"{}"}}"#, hex(&pending.encode()));
    let (status, _, answer) = http_full(
        a.info.api.unwrap(),
        "POST",
        "/v1/transactions",
        &[("Content-Type", "application/json")],
        &body,
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    let token = config::token(&b.dir.0).unwrap();
    let not_before = ting_node::now() + 3_600;
    transport::control_call(
        b.info.control,
        &token,
        "submit",
        json!({"hex":hex(&deferred.encode()),"not_before":not_before}),
    )
    .await
    .unwrap();
    command(&b.dir.0, &["add", "node", &a.info.p2p.to_string()]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    for node in [&a, &b] {
        loop {
            let status = node.call("status", json!({})).await;
            if status["mempool_count"] == 1 && status["deferred_count"] == 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{status} {}",
                node.call("peers", json!({})).await
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    // A block holding the pending one, over 10 MiB, reaches B in slices.
    command(&a.dir.0, &["mine"]);
    b.wait_height(1).await;
    assert_eq!(
        b.call("status", json!({})).await["tip"],
        a.call("status", json!({})).await["tip"]
    );
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_takes_later_versions_and_names_the_software() {
    let n = Node::start(false, "hello");
    let chain_id = chain_id_of(&n).await;
    // A later version keeps every earlier message, so it is taken; one below
    // the first is not.
    let later = wire::Hello {
        version: 2,
        agent: "other/9".into(),
        features: vec!["later".into()],
        ..hello(chain_id)
    };
    let (_, answer) = greet(n.info.p2p, later).await;
    let Ok(wire::Answer::State(state)) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(
        (state.version, state.agent.as_str(), state.features.len()),
        (1, wire::AGENT, 0)
    );
    let (_, answer) = greet(
        n.info.p2p,
        wire::Hello {
            version: 0,
            ..hello(chain_id)
        },
    )
    .await;
    assert!(answer.unwrap_err().contains("version"));
    // A software name that is not plain text of at most 64 characters makes
    // the hello no message at all: the connection ends unanswered.
    for agent in ["bad\nline".to_string(), "x".repeat(65)] {
        let mut stream = TcpStream::connect(n.info.p2p).await.unwrap();
        let mut frame = wire::Frame::Request(wire::Request::Hello(hello(chain_id))).encode();
        // The agent's length and bytes follow the kind, the request, the
        // version and the ChainId.
        let at = 2 + 4 + 32;
        frame.splice(
            at..at + 2 + 4,
            [
                (agent.len() as u16).to_le_bytes().as_slice(),
                agent.as_bytes(),
            ]
            .concat(),
        );
        raw_frame(&mut stream, &frame).await;
        assert!(
            transport::read_wire(&mut stream, None, ting_node::FRAME_MAX)
                .await
                .is_err()
        );
    }
    // `ting probe` says hello as a peer does and shows the answer as text.
    let probed = command(&n.dir.0, &["probe", &n.info.p2p.to_string()]);
    assert_eq!(probed["answer"]["agent"], wire::AGENT);
    assert_eq!(probed["answer"]["height"], 0);
    assert_eq!(probed["answer"]["chain_id"], json!(hex(&chain_id)));
    n.stop().await;
}
#[tokio::test]
async fn seed_names_resolve_to_addresses_and_bad_ones_are_passed_over() {
    let found =
        ting_node::daemon::seed_addresses(&["127.0.0.1:18442", "no port", "localhost:18443"]).await;
    assert!(
        found.contains(&"127.0.0.1:18442".parse().unwrap()),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|a| a.port() == 18443 && a.ip().is_loopback()),
        "{found:?}"
    );
    assert!(found.iter().all(|a| a.port() != 0));
    assert!(ting_node::config::SEEDS.iter().all(|s| s.contains(':')));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocks_are_asked_for_many_at_once_and_assembled_from_their_slices() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    // A peer with 24 blocks that answers each request a little late and in
    // slices of 100 bytes, so that every block takes several: the node keeps
    // many requests on their way, and assembles each block from its slices.
    let b = Node::start(false, "fetcher");
    let g = config::genesis(&b.dir.0).unwrap();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let (mut history, mut blocks) = (Vec::new(), Vec::new());
    let start = ting_node::now();
    for i in 0..24 {
        let mut block = runtime
            .build_candidate(&g, &history, vec![], start + i, [0; 4], vec![])
            .unwrap();
        while !consensus::valid_pow(&block.header) {
            block.header.nonce += 1;
        }
        runtime
            .validate_and_apply_block(&g, &history, &block)
            .unwrap();
        history.push(block.header.clone());
        blocks.push(block);
    }
    let tip = blocks[23].header.id();
    let state = wire::State {
        version: 1,
        chain_id: g.chain_id(),
        agent: "slow".into(),
        features: vec![],
        nonce: 0xbb,
        height: 24,
        tip,
        work: history.iter().map(|h| consensus::work(&h.target)).sum(),
        locator: vec![],
        pending: vec![],
        deferred: vec![],
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (waiting, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (served, seen) = (blocks.clone(), (waiting.clone(), most.clone()));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (mut read, mut write) = stream.into_split();
            let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
            let (waiting, most) = seen.clone();
            tokio::spawn(async move {
                while let Ok(wire::Frame::Request(request)) =
                    transport::read_wire(&mut read, None, ting_node::FRAME_MAX).await
                {
                    let now = waiting.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    if sender.send(request).is_err() {
                        return;
                    }
                }
            });
            let (served, state, waiting) = (served.clone(), state.clone(), seen.0.clone());
            tokio::spawn(async move {
                let mut sent_headers = false;
                while let Some(request) = requests.recv().await {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let answer = match request {
                        wire::Request::Hello(_) | wire::Request::State => {
                            Ok(wire::Answer::State(state.clone()))
                        }
                        wire::Request::Headers { .. } if !sent_headers => {
                            sent_headers = true;
                            Ok(wire::Answer::Headers(
                                served
                                    .iter()
                                    .map(|b| b.header.encode().try_into().unwrap())
                                    .collect(),
                            ))
                        }
                        wire::Request::Headers { .. } => Ok(wire::Answer::Headers(vec![])),
                        wire::Request::Raw { id, offset, .. } => {
                            let block = served.iter().find(|b| b.header.id() == id).unwrap();
                            let bytes = block.encode();
                            let from = offset as usize;
                            let to = (from + 100).min(bytes.len());
                            Ok(wire::Answer::Slice {
                                offset,
                                total: bytes.len() as u64,
                                bytes: bytes[from..to].to_vec(),
                            })
                        }
                        wire::Request::Inventory { .. } => Ok(wire::Answer::Inventory {
                            ids: vec![],
                            next: None,
                        }),
                        wire::Request::Addresses => Ok(wire::Answer::Addresses(vec![])),
                        _ => Err("not served".to_string()),
                    };
                    waiting.fetch_sub(1, Ordering::SeqCst);
                    let frame = match answer {
                        Ok(answer) => wire::Frame::Answer(answer),
                        Err(error) => wire::Frame::Error(error),
                    };
                    if transport::write_wire(&mut write, &frame).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    command(&b.dir.0, &["add", "node", &address.to_string()]);
    b.wait_height(24).await;
    assert_eq!(b.call("status", json!({})).await["tip"], hex(&tip));
    let most = most.load(Ordering::SeqCst);
    assert!(
        most >= 16,
        "at most {most} requests were on their way at once"
    );
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_syncs_the_seed_from_a_peer_restarts_and_carries_on() {
    // The launch gate (wiki: reference/conformance#the-seed). A starts its
    // development chain with the seed; B starts empty, and a block it mines
    // on its own is refused at the seed's first height. B then takes the
    // seed's blocks from A, each checked against the list and executed,
    // restarts on its store and follows A's next block.
    let seed = ting_node::seed::ids(&config::development_genesis());
    let n = seed.len() as u64;
    let tip = ting_core::hash::hex(seed.last().unwrap());
    let a = Node::start_development(true, "seeded A");
    assert_eq!(a.height().await, n);
    assert_eq!(a.call("status", json!({})).await["tip"], tip);
    let mut b = Node::start_development(false, "empty B");
    assert_eq!(b.height().await, 0);
    let mined = cli(&b.dir.0, &["mine"]);
    assert!(!mined.status.success());
    assert!(
        String::from_utf8_lossy(&mined.stderr).contains("not the seed's block at height 1"),
        "{}",
        String::from_utf8_lossy(&mined.stderr)
    );
    assert_eq!(b.height().await, 0);
    command(&b.dir.0, &["add", "node", &a.info.p2p.to_string()]);
    b.wait_height(n).await;
    assert_eq!(b.call("status", json!({})).await["tip"], tip);
    b.stop().await;
    b.restart();
    assert_eq!(b.height().await, n);
    command(&b.dir.0, &["add", "node", &a.info.p2p.to_string()]);
    command(&a.dir.0, &["mine"]);
    b.wait_height(n + 1).await;
    assert_eq!(
        b.call("status", json!({})).await["tip"],
        a.call("status", json!({})).await["tip"]
    );
    a.stop().await;
    b.stop().await;
}

/// A transaction of the test chain, one for each `input`.
fn pooled(input: u8) -> ting_core::protocol::Transaction {
    let mut tx = support::tx();
    if let ting_core::protocol::Payload::Deploy { input: bytes, .. } = &mut tx.payload {
        *bytes = vec![input];
    }
    tx
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_transaction_travels_once_a_connection_however_full_the_pools() {
    // Two full pools that stay apart: the node's own transactions, which never
    // give way, fill its pool of two, and the peer takes none of those it is
    // sent. Exchange after exchange, the node sends each of its own once and
    // fetches each of the peer's once, until the peer holds another.
    let n = Node::start_configured("full pool", |c| c.mempool_transactions = 2);
    let g = config::genesis(&n.dir.0).unwrap();
    let chain_id = g.chain_id();
    for input in 0..2 {
        n.call("submit", json!({"hex":hex(&pooled(input).encode())}))
            .await;
    }
    let held = |input: u8| (pooled(input).id(chain_id), Some(pooled(input).encode()));
    let (pool, asked): (Pool, Asked) = Default::default();
    pool.lock().unwrap().extend([held(10), held(11)]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    pool_peer(
        listener,
        fake_state(chain_id, g.block_id(), 0xcc),
        pool.clone(),
        asked.clone(),
    );
    command(&n.dir.0, &["add", "node", &address.to_string()]);
    wait_exchanges(&n, &asked, 5).await;
    let (_, sent, fetched) = exchanges(&asked);
    assert_eq!((sent, fetched), (2, 2));
    // One the peer pools later is fetched in turn, and only that one.
    pool.lock().unwrap().extend([held(12)]);
    wait_exchanges(&n, &asked, exchanges(&asked).0 + 3).await;
    let (_, sent, fetched) = exchanges(&asked);
    assert_eq!((sent, fetched), (2, 3));
    let status = n.call("status", json!({})).await;
    assert_eq!(status["mempool_count"], 2);
    let peers = n.call("peers", json!({})).await;
    assert!(peers["items"][0]["error"].is_null(), "{peers}");
    n.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_pooling_more_than_the_node_can_hold_keeps_its_connection() {
    // The peer lists 1,500 transactions, more than the node's pool of two can
    // hold, as a peer configured with a larger pool may: the node reads one
    // page of its inventory, fetches what that lists once, and stays connected.
    // Its own two lie past the page's last id, where it cannot tell whether the
    // peer holds them, so it does not send them.
    let n = Node::start_configured("small pool", |c| c.mempool_transactions = 2);
    let g = config::genesis(&n.dir.0).unwrap();
    for input in 0..2 {
        n.call("submit", json!({"hex":hex(&pooled(input).encode())}))
            .await;
    }
    let id = |i: u16| {
        let mut id = [0; 32];
        id[30..].copy_from_slice(&i.to_be_bytes());
        id
    };
    let (pool, asked): (Pool, Asked) = Default::default();
    pool.lock()
        .unwrap()
        .extend((1000..2500).map(|i| (id(i), None)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    pool_peer(
        listener,
        fake_state(g.chain_id(), g.block_id(), 0xcd),
        pool.clone(),
        asked.clone(),
    );
    command(&n.dir.0, &["add", "node", &address.to_string()]);
    wait_exchanges(&n, &asked, 3).await;
    let (_, sent, fetched) = exchanges(&asked);
    assert_eq!((sent, fetched), (0, 1024));
    let paged = asked.lock().unwrap().iter().any(|r| {
        matches!(
            r,
            wire::Request::Inventory {
                deferred: false,
                after: Some(_)
            }
        )
    });
    assert!(!paged, "the node read past what its pool holds");
    // Ten lower ids push the top of the page past its end, and leave again:
    // only the ten are fetched, and those the page lists again are not.
    pool.lock().unwrap().extend((0..10).map(|i| (id(i), None)));
    wait_exchanges(&n, &asked, exchanges(&asked).0 + 3).await;
    assert_eq!(exchanges(&asked).2, 1034);
    pool.lock()
        .unwrap()
        .retain(|id, _| u16::from_be_bytes([id[30], id[31]]) >= 1000);
    wait_exchanges(&n, &asked, exchanges(&asked).0 + 3).await;
    let (_, sent, fetched) = exchanges(&asked);
    assert_eq!((sent, fetched), (0, 1034));
    let peers = n.call("peers", json!({})).await;
    assert!(peers["items"][0]["error"].is_null(), "{peers}");
    n.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transactions_refused_for_want_of_room_are_tried_again_once_a_block_makes_room() {
    // The node's pool of two is full of its own when the peer offers four
    // transactions of 400 kB, so it takes none. Once a block includes its own,
    // it fetches the peer's again, but no more bytes than the next block holds,
    // 1 MiB: three of the four, of which its pool takes two. The rest wait for
    // the next block.
    use ting_core::{
        meter::Resources,
        protocol::{Payload, Transaction},
    };
    let n = Node::start_configured("refused", |c| c.mempool_transactions = 2);
    let g = config::genesis(&n.dir.0).unwrap();
    let chain_id = g.chain_id();
    for input in 0..2 {
        n.call("submit", json!({"hex":hex(&pooled(input).encode())}))
            .await;
    }
    let (pool, asked): (Pool, Asked) = Default::default();
    pool.lock().unwrap().extend((1..=4).map(|byte| {
        let tx = Transaction {
            budget: Resources([1_000_000, 1, 450_000, 1]),
            payload: Payload::Call {
                target: [1; 32],
                input: vec![byte; 400_000],
            },
        };
        (tx.id(chain_id), Some(tx.encode()))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    pool_peer(
        listener,
        fake_state(chain_id, g.block_id(), 0xcf),
        pool,
        asked.clone(),
    );
    // Slices asked for, of those listed by size alone.
    let raw = |asked: &Asked| {
        let asked = asked.lock().unwrap();
        asked
            .iter()
            .filter(|r| matches!(r, wire::Request::TransactionRaw { .. }))
            .count()
    };
    command(&n.dir.0, &["add", "node", &address.to_string()]);
    wait_exchanges(&n, &asked, 3).await;
    // Two came whole within the answer's slice, two by size alone.
    assert_eq!((exchanges(&asked).2, raw(&asked)), (4, 2));
    n.call("mine", json!({"count":1})).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while raw(&asked) < 3 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            exchanges(&asked)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    wait_exchanges(&n, &asked, exchanges(&asked).0 + 3).await;
    assert_eq!((exchanges(&asked).2, raw(&asked)), (8, 3));
    let status = n.call("status", json!({})).await;
    assert_eq!(
        (status["height"].as_u64(), status["mempool_count"].as_u64()),
        (Some(1), Some(2))
    );
    let peers = n.call("peers", json!({})).await;
    assert!(peers["items"][0]["error"].is_null(), "{peers}");
    n.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inventory_whose_pages_do_not_move_on_ends_the_connection() {
    // Every page of the peer's inventory lists the same two ids and says more
    // follow: the node does not ask forever, it ends the connection.
    let n = Node::start(false, "pages");
    let g = config::genesis(&n.dir.0).unwrap();
    let (chain_id, genesis) = (g.chain_id(), g.block_id());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    fake_peer(listener, move |request, _| match request {
        wire::Request::Hello(_) | wire::Request::State => {
            Ok(wire::Answer::State(fake_state(chain_id, genesis, 0xce)))
        }
        wire::Request::Headers { .. } => Ok(wire::Answer::Headers(vec![])),
        wire::Request::Inventory { .. } => Ok(wire::Answer::Inventory {
            ids: vec![[1; 32], [2; 32]],
            next: Some([2; 32]),
        }),
        wire::Request::Addresses => Ok(wire::Answer::Addresses(vec![])),
        _ => Err("not served".into()),
    });
    command(&n.dir.0, &["add", "node", &address.to_string()]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let peers = n.call("peers", json!({})).await;
        if peers["items"][0]["error"] == "invalid inventory page" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{peers}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    n.stop().await;
}
