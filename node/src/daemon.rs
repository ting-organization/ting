use crate::{
    Result, api,
    chain::Handle,
    config::{self, Config, Running},
    discovery::{self, AddressBook},
    mining::{self, Miner, Search, Selector},
    peers, progress, reads,
    transport::{self, Inbound, PeerStats},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use ting_core::protocol::{HEADER_LEN, Header};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
};

struct ApiChange {
    enabled: bool,
    cors: bool,
    reply: oneshot::Sender<Result<Value>>,
}
#[derive(Clone)]
pub struct Node {
    pub handle: Handle,
    pub token: Arc<str>,
    pub shutdown: Arc<AtomicBool>,
    pub info: Arc<Mutex<Running>>,
    pub peers: PeerStats,
    /// Addresses of other nodes, and whether to discover more (read at start).
    pub book: Arc<Mutex<AddressBook>>,
    pub discovery: bool,
    /// Most transactions the pool holds (read at start).
    pub mempool: usize,
    /// Open inbound connections, for bans to end.
    pub inbound: Arc<Mutex<Inbound>>,
    /// This run's identity in the peer protocol.
    pub nonce: u64,
    /// The miner while it runs.
    pub miner: Arc<Mutex<Option<Miner>>>,
    /// Makes the nonce search for a parallelism hint (threads for the CPU).
    pub search: SearchFactory,
    /// Peers' reads of stored data, off the chain thread.
    pub readers: reads::Readers,
    changes: mpsc::Sender<ApiChange>,
}
pub type SearchFactory = Arc<dyn Fn(u32) -> Arc<dyn Search> + Send + Sync>;
/// Resolves when the process is asked to stop: Ctrl-C, or on Unix also
/// SIGTERM, as service managers and container runtimes send it. A signal that
/// cannot be listened for is reported and never resolves, so the node runs on
/// and stops through its control channel.
async fn terminated() {
    let interrupted = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            crate::log!("shutdown signal: {e}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {_=interrupted=>{},_=term.recv()=>{}}
            }
            Err(e) => {
                crate::log!("termination signal: {e}");
                interrupted.await;
            }
        }
    }
    #[cfg(not(unix))]
    interrupted.await;
}
/// The addresses `seeds`, `host:port` names, resolve to, each looked up for at
/// most five seconds; a name that does not resolve is passed over.
pub async fn seed_addresses(seeds: &[&str]) -> Vec<std::net::SocketAddr> {
    let mut addresses = Vec::new();
    for seed in seeds {
        let lookup = tokio::net::lookup_host(*seed);
        match tokio::time::timeout(Duration::from_secs(5), lookup).await {
            Ok(Ok(found)) => {
                let before = addresses.len();
                addresses.extend(found);
                let n = addresses.len() - before;
                crate::log!(
                    "seed node {seed}: {n} address{}",
                    if n == 1 { "" } else { "es" }
                );
            }
            _ => crate::log!("seed node {seed} did not resolve"),
        }
    }
    addresses
}
/// What a fork of this node replaces: how templates pick transactions and how
/// nonces are searched. The reference node uses first-in selection and the CPU.
pub struct Mining {
    pub selector: Box<dyn Selector>,
    pub search: SearchFactory,
}
impl Default for Mining {
    fn default() -> Self {
        Self {
            selector: Box::new(mining::FirstIn::default()),
            search: Arc::new(|threads| Arc::new(mining::Cpu::new(threads))),
        }
    }
}
impl Node {
    pub async fn stopped(&self) {
        transport::wait_stopped(&self.shutdown).await;
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        match method {
            "stop" => Ok(json!({"stopping":true})),
            "api" => {
                let enabled = params["enabled"]
                    .as_bool()
                    .ok_or("expected enabled boolean")?;
                let cors = params["cors"].as_bool().unwrap_or(false);
                let (reply, receiver) = oneshot::channel();
                self.changes
                    .try_send(ApiChange {
                        enabled,
                        cors,
                        reply,
                    })
                    .map_err(|_| "API manager busy")?;
                receiver.await.map_err(|_| "API manager stopped")?
            }
            "status" => {
                let mut status = self.handle.call("status", params).await?;
                status["listeners"] = serde_json::to_value(self.info.lock().unwrap().clone())
                    .map_err(|e| e.to_string())?;
                Ok(status)
            }
            "miner" => self.miner(params).await,
            "votes" => {
                let config = self.handle.call("config", json!({})).await?;
                let mut voting = self.handle.call("votes", json!({})).await?;
                voting["votes"] = config["votes"].clone();
                Ok(voting)
            }
            "mine" => self.mine(params).await,
            "peers" => {
                let config = self.handle.call("config", json!({})).await?;
                let stats = self.peers.lock().unwrap();
                let mut items: Vec<Value> = config["peers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|address| {
                        stats.get(address.as_str().unwrap()).cloned().unwrap_or(
                            json!({"address":address,"source":"config","error":null,"last_success":null}),
                        )
                    })
                    .collect();
                items.extend(
                    stats
                        .values()
                        .filter(|v| v["source"] == "discovered")
                        .cloned(),
                );
                Ok(json!({
                    "items":items,
                    "discovery":config["discovery"],
                    "max_peers":config["max_peers"],
                    "known":self.book.lock().unwrap().len(),
                }))
            }
            _ => self.handle.call(method, params).await,
        }
    }
}
impl Node {
    /// Development mining in one request (`ting mine`): each block from a
    /// template of the chain thread, its nonce searched on a blocking thread
    /// meanwhile, so that the chain answers everyone else, and its solution
    /// handed back. A template the tip moved past is replaced by a new one.
    async fn mine(&self, params: Value) -> Result<Value> {
        let count = params["count"].as_u64().unwrap_or(1);
        let attempts = params["attempts"].as_u64().unwrap_or(1_000_000);
        if !(1..=100).contains(&count) || !(1..=10_000_000).contains(&attempts) {
            return Err("count must be 1..100 and attempts 1..10000000".into());
        }
        let mut mined = Vec::new();
        let tried = Arc::new(AtomicU64::new(0));
        let mut stale = 0;
        while (mined.len() as u64) < count {
            let template = self.handle.call("template", json!({})).await?;
            let header = Header::decode(&crate::unhex(
                template["header_hex"].as_str().ok_or("invalid template")?,
                HEADER_LEN,
            )?)
            .map_err(|e| e.to_string())?;
            let (shutdown, counter) = (self.shutdown.clone(), tried.clone());
            let found = tokio::task::spawn_blocking(move || {
                mining::search(&header, 0, attempts, &shutdown, &counter)
            })
            .await
            .map_err(|e| e.to_string())?;
            let Some(nonce) = found else {
                if self.shutdown.load(Ordering::Relaxed) {
                    return Err("node is stopping".into());
                }
                return Ok(json!({"blocks":mined,"exhausted":true}));
            };
            match self
                .handle
                .call(
                    "solution",
                    json!({"template":template["id"],"nonce":nonce.to_string()}),
                )
                .await
            {
                Ok(imported) => mined.push(imported),
                Err(error) if error == mining::EXPIRED && stale < 10 => stale += 1,
                Err(error) => return Err(error),
            }
        }
        Ok(json!({"blocks":mined,"exhausted":false}))
    }
    /// Starts, stops or describes the built-in miner; start and stop are
    /// remembered in the configuration so a restart resumes the same state.
    async fn miner(&self, params: Value) -> Result<Value> {
        let config: Config = serde_json::from_value(self.handle.call("config", json!({})).await?)
            .map_err(|e| e.to_string())?;
        match params["action"].as_str().unwrap_or("status") {
            "start" => {
                let threads = match params["threads"].as_u64() {
                    None => config.miner_threads,
                    Some(t) => u32::try_from(t)
                        .ok()
                        .filter(|t| (1..=64).contains(t))
                        .ok_or("threads must be 1..64")?,
                };
                let spacing = params["spacing"].as_u64().unwrap_or(config.miner_spacing);
                self.handle
                    .call(
                        "miner_config",
                        json!({"enabled":true,"threads":threads,"spacing":spacing}),
                    )
                    .await?;
                self.stop_miner().await;
                let miner = Miner::start(self.handle.clone(), (self.search)(threads), spacing);
                let status = miner.status();
                *self.miner.lock().unwrap() = Some(miner);
                Ok(status)
            }
            "stop" => {
                self.handle
                    .call("miner_config", json!({"enabled":false}))
                    .await?;
                self.stop_miner().await;
                Ok(json!({"running":false}))
            }
            "producer" => {
                let producer = params["producer"].as_str().unwrap_or("").to_owned();
                self.handle
                    .call(
                        "miner_config",
                        json!({"enabled":config.miner_enabled,"producer":producer}),
                    )
                    .await
            }
            "votes" => {
                self.handle
                    .call(
                        "miner_config",
                        json!({"enabled":config.miner_enabled,"votes":params["votes"]}),
                    )
                    .await
            }
            _ => {
                let mut status = self.miner.lock().unwrap().as_ref().map_or(
                    json!({"running":false,"threads":config.miner_threads,"spacing":config.miner_spacing}),
                    Miner::status,
                );
                status["producer_hex"] = json!(ting_core::hash::hex(&config.producer()));
                status["votes"] = json!(config.votes);
                Ok(status)
            }
        }
    }
    /// Joins the miner thread off the async runtime; it stops within a slice.
    pub async fn stop_miner(&self) {
        let running = self.miner.lock().unwrap().take();
        if let Some(miner) = running {
            let _ = tokio::task::spawn_blocking(move || miner.stop()).await;
        }
    }
}
struct ApiServer {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
fn launch_api(listener: TcpListener, node: Node, cors: bool) -> ApiServer {
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let app =
            api::router(node, cors).into_make_service_with_connect_info::<std::net::SocketAddr>();
        let server = axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = stopped.await;
        });
        if let Err(e) = server.await {
            crate::log!("REST service: {e}");
        }
    });
    ApiServer { stop, task }
}
async fn stop_api(server: ApiServer) {
    let _ = server.stop.send(());
    let mut task = server.task;
    if tokio::time::timeout(Duration::from_secs(2), &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}
async fn api_manager(
    node: Node,
    dir: PathBuf,
    mut changes: mpsc::Receiver<ApiChange>,
    initial: Option<(TcpListener, bool)>,
) {
    let mut server =
        initial.map(|(listener, cors)| (launch_api(listener, node.clone(), cors), cors));
    loop {
        let change = tokio::select! {c=changes.recv()=>c,_=node.stopped()=>break};
        let Some(change) = change else {
            break;
        };
        let result = async {
            // Enabling with the other CORS setting restarts the service.
            if change.enabled
                && server
                    .as_ref()
                    .is_some_and(|(_, cors)| *cors != change.cors)
                && let Some((active, _)) = server.take()
            {
                stop_api(active).await;
            }
            if change.enabled && server.is_none() {
                let c: Config =
                    serde_json::from_value(node.handle.call("config", json!({})).await?)
                        .map_err(|e| e.to_string())?;
                let listener = TcpListener::bind(c.api)
                    .await
                    .map_err(|e| format!("cannot bind REST: {e}"))?;
                let address = listener.local_addr().map_err(|e| e.to_string())?;
                node.handle
                    .call("api_config", json!({"enabled":true,"cors":change.cors}))
                    .await?;
                server = Some((launch_api(listener, node.clone(), change.cors), change.cors));
                node.info.lock().unwrap().api = Some(address);
            } else if !change.enabled && server.is_some() {
                node.handle
                    .call("api_config", json!({"enabled":false,"cors":false}))
                    .await?;
                if let Some((active, _)) = server.take() {
                    stop_api(active).await;
                }
                node.info.lock().unwrap().api = None;
            }
            node.info.lock().unwrap().save(&dir)?;
            Ok(json!({"enabled":server.is_some(),"address":node.info.lock().unwrap().api}))
        }
        .await;
        let _ = change.reply.send(result);
    }
    if let Some((active, _)) = server {
        stop_api(active).await;
    }
}
pub async fn run(dir: &Path) -> Result<()> {
    run_with(dir, Mining::default()).await
}
/// The node with a fork's own mining pieces; everything else is shared.
pub async fn run_with(dir: &Path, mining: Mining) -> Result<()> {
    let lock = config::lock(dir)?;
    let c = Config::load(dir)?;
    let token = config::token(dir)?;
    let control = TcpListener::bind(c.control)
        .await
        .map_err(|e| format!("control listener: {e}"))?;
    let p2p = TcpListener::bind(c.p2p)
        .await
        .map_err(|e| format!("peer listener: {e}"))?;
    let api = if c.api_enabled {
        Some(
            TcpListener::bind(c.api)
                .await
                .map_err(|e| format!("REST listener: {e}"))?,
        )
    } else {
        None
    };
    let info = Running {
        pid: std::process::id(),
        control: control.local_addr().map_err(|e| e.to_string())?,
        p2p: p2p.local_addr().map_err(|e| e.to_string())?,
        api: api
            .as_ref()
            .map(|l| l.local_addr())
            .transpose()
            .map_err(|e| e.to_string())?,
        started: crate::now(),
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let (handle, worker) = Handle::spawn_with(dir.into(), shutdown.clone(), mining.selector)?;
    if let Err(e) = info.save(dir) {
        shutdown.store(true, Ordering::Relaxed);
        let _ = worker.join();
        return Err(e);
    }
    let (changes, receiver) = mpsc::channel(8);
    let mut book = AddressBook::open(dir);
    book.configure(&c.peers);
    let readers = reads::Readers::new(handle.reader().clone());
    let node = Node {
        handle,
        token: token.into(),
        shutdown: shutdown.clone(),
        info: Arc::new(Mutex::new(info.clone())),
        peers: Arc::new(Mutex::new(BTreeMap::new())),
        book: Arc::new(Mutex::new(book)),
        discovery: c.discovery,
        mempool: c.mempool_transactions,
        inbound: Arc::new(Mutex::new(Inbound::default())),
        nonce: discovery::seed(),
        miner: Arc::new(Mutex::new(None)),
        search: mining.search,
        readers,
        changes,
    };
    // On the production network, the built-in seeds are looked up in the
    // background; their addresses join the book like any other.
    let production = config::production_genesis().map(|g| g.chain_id());
    if c.discovery && production == Some(config::genesis(dir)?.chain_id()) {
        let book = node.book.clone();
        tokio::spawn(async move {
            for address in seed_addresses(config::SEEDS).await {
                book.lock().unwrap().learn(address, discovery::Source::Seed);
            }
        });
    }
    if c.miner_enabled {
        *node.miner.lock().unwrap() = Some(Miner::start(
            node.handle.clone(),
            (node.search)(c.miner_threads),
            c.miner_spacing,
        ));
    }
    println!(
        "{} Ting ready: control={} peers={} REST={}",
        crate::log::utc(crate::now()),
        info.control,
        info.p2p,
        info.api
            .map_or("disabled".into(), |a| format!("http://{a}"))
    );
    let control_task = tokio::spawn(transport::control_server(control, node.clone()));
    let peer_task = tokio::spawn(transport::peer_server(p2p, node.clone()));
    let sync_task = tokio::spawn(peers::run(node.clone()));
    let progress_task = tokio::spawn(progress::run(node.clone()));
    let api_task = tokio::spawn(api_manager(
        node.clone(),
        dir.into(),
        receiver,
        api.map(|listener| (listener, c.api_cors)),
    ));
    tokio::select! {_=node.stopped()=>{},_=terminated()=>{}}
    shutdown.store(true, Ordering::Relaxed);
    control_task.abort();
    peer_task.abort();
    sync_task.abort();
    progress_task.abort();
    node.stop_miner().await;
    let _ = api_task.await;
    let _ = worker.join();
    // Addresses learned since the last round are hints worth keeping.
    if let Err(e) = node.book.lock().unwrap().save() {
        crate::log!("peer address book: {e}");
    }
    std::fs::remove_file(dir.join("runtime.json")).map_err(|e| e.to_string())?;
    // The data directory stays locked until the process exits: aborted tasks
    // and reads still running can hold the store until the runtime shuts
    // down, and a node started meanwhile must not find the lock free while the
    // store is still open.
    std::mem::forget(lock);
    Ok(())
}
