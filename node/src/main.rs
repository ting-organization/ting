use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use ting_node::{
    Result,
    config::{self, Config, Running},
    daemon, transport,
};

#[derive(Parser)]
#[command(
    name = "ting",
    version,
    about = "Ting node and local administration CLI"
)]
struct Cli {
    /// Node data directory (default: USERPROFILE/.ting on Windows, HOME/.ting elsewhere).
    #[arg(long, global = true, value_name = "DIRECTORY")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Create a node data directory: for the production network, a development
    /// chain with --dev, or another network with --genesis FILE.
    Init {
        #[arg(long, default_value = "Ting node")]
        name: String,
        #[arg(long, default_value = "127.0.0.1:18440")]
        control: SocketAddr,
        /// The peer listener: 0.0.0.0:18442 on the production network,
        /// 127.0.0.1:18442 otherwise.
        #[arg(long)]
        p2p: Option<SocketAddr>,
        #[arg(long, default_value = "127.0.0.1:18441")]
        api_bind: SocketAddr,
        #[arg(long)]
        api: bool,
        /// A private development chain with an easy target.
        #[arg(long, conflicts_with = "genesis")]
        dev: bool,
        #[arg(long)]
        genesis: Option<PathBuf>,
    },
    /// Run the node in the foreground; Ctrl+C stops it.
    Run,
    /// Launch a background node and wait for readiness.
    Start,
    /// Gracefully stop the running node.
    Stop,
    /// Show validated chain and listener status.
    Status,
    /// Show configured and discovered peers and their synchronization status.
    Peers,
    /// Say hello to the node at ADDRESS over the peer protocol, as a peer on
    /// this data directory's network does, and show its answer.
    Probe { address: SocketAddr },
    #[command(subcommand)]
    Add(PeerCommand),
    #[command(subcommand)]
    Remove(PeerCommand),
    /// Mine blocks with a bounded nonce search in one request, for a development chain.
    Mine {
        #[arg(long, default_value_t = 1)]
        count: u64,
        #[arg(long, default_value_t = 1_000_000)]
        attempts: u64,
    },
    /// The built-in CPU miner: start, stop or inspect it.
    #[command(subcommand)]
    Miner(MinerCommand),
    /// Print a block template: a candidate block whose header lacks only the nonce.
    Template {
        /// Include exactly these pooled transactions, in this order (default: first in).
        #[arg(long = "tx", value_name = "ID")]
        transactions: Vec<String>,
    },
    /// Submit a nonce for a template; the node validates, commits and relays the block.
    Solution { template: String, nonce: String },
    /// Submit a canonical binary transaction envelope.
    Submit {
        file: PathBuf,
        /// Defer it: hold until this Unix time, include once it executes successfully.
        #[arg(long)]
        not_before: Option<u64>,
    },
    #[command(subcommand)]
    Api(ApiCommand),
    /// Mark a block invalid: the node leaves it and every block above it, and
    /// never connects them until it is reconsidered.
    Invalidate {
        /// A block id, or the height of a connected block.
        block: String,
    },
    /// Clear invalid marks: of the block, every block above it and every block
    /// below it, marked or failed; the node then follows the best branch again.
    Reconsider {
        /// A block id, or the height of a connected block.
        block: String,
    },
    /// Show enabled wiki and API documentation URLs.
    Docs,
}
#[derive(Subcommand)]
enum PeerCommand {
    Node { address: SocketAddr },
}
#[derive(Subcommand)]
enum MinerCommand {
    /// Start mining with the node's CPU; remembered across restarts.
    Start {
        #[arg(long)]
        threads: Option<u32>,
        /// Least seconds between the miner's own blocks.
        #[arg(long)]
        spacing: Option<u64>,
    },
    Stop,
    Status,
    /// Set the producer data of the blocks this node builds (hex), or clear it.
    Producer {
        hex: Option<String>,
    },
}
#[derive(Subcommand)]
enum ApiCommand {
    Enable {
        /// Answer browser applications from any origin, such as a wallet in a web page.
        #[arg(long)]
        cors: bool,
    },
    Disable,
}
async fn rpc(dir: &Path, method: &str, params: Value) -> Result<Value> {
    let info = Running::load(dir)?;
    transport::control_call(info.control, &config::token(dir)?, method, params).await
}
async fn edit(dir: &Path, method: &str, params: Value) -> Result<Value> {
    let live_error = match rpc(dir, method, params.clone()).await {
        Ok(result) => return Ok(result),
        Err(e) => e,
    };
    // An offline edit is allowed only while holding the same single-writer lock.
    let _lock = config::lock(dir).map_err(|_| live_error)?;
    let mut c = Config::load(dir)?;
    match method {
        "api" => {
            c.api_enabled = params["enabled"].as_bool().unwrap();
            c.api_cors = params["cors"].as_bool().unwrap_or(false);
        }
        "miner" if params["action"] == "producer" => {
            c.producer_hex = params["producer"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
        }
        "miner" => {
            c.miner_enabled = params["action"] == "start";
            if let Some(threads) = params["threads"].as_u64() {
                c.miner_threads = threads as u32;
            }
            if let Some(spacing) = params["spacing"].as_u64() {
                c.miner_spacing = spacing;
            }
        }
        "add_peer" => {
            let a = params["address"]
                .as_str()
                .unwrap()
                .parse()
                .map_err(|_| "invalid address")?;
            if !c.peers.contains(&a) {
                c.peers.push(a);
            }
        }
        "remove_peer" => {
            let a: SocketAddr = params["address"]
                .as_str()
                .unwrap()
                .parse()
                .map_err(|_| "invalid address")?;
            c.peers.retain(|p| *p != a);
        }
        _ => return Err("unknown configuration operation".into()),
    }
    c.save(dir)?;
    Ok(
        json!({"saved":true,"running":false,"api_enabled":c.api_enabled,"peers":c.peers,"miner_enabled":c.miner_enabled}),
    )
}
/// The peer protocol's answer of the node at `address` to a hello on this
/// data directory's network, shown as text (wiki: node/sync#messages).
async fn probe(dir: &Path, address: SocketAddr) -> Result<Value> {
    let hello = ting_node::wire::Hello {
        version: ting_node::wire::VERSION,
        chain_id: config::genesis(dir)?.chain_id(),
        agent: ting_node::wire::AGENT.into(),
        features: Vec::new(),
        nonce: ting_node::discovery::seed(),
        announce: false,
        listen: None,
    };
    let mut stream = ting_node::transport::connect(address).await?;
    let answer =
        ting_node::transport::ask(&mut stream, ting_node::wire::Request::Hello(hello)).await?;
    Ok(ting_node::wire::Frame::Answer(answer).describe())
}
async fn start(dir: &Path) -> Result<Value> {
    if let Ok(status) = rpc(dir, "status", json!({})).await {
        return Ok(status);
    }
    let _ = Config::load(dir)?;
    // A node that has just stopped holds the lock until its process exits, a
    // moment after `ting stop` returns: it is waited for, up to ten seconds.
    let mut waited = 0;
    while let Err(error) = config::lock(dir) {
        if waited >= 100 {
            return Err(error);
        }
        waited += 1;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let absolute = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("node.log"))
        .map_err(|e| e.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--data-dir")
        .arg(&absolute)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        close_stdio_inheritance()?;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    for _ in 0..150 {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "node exited ({status}); see {}",
                absolute.join("node.log").display()
            ));
        }
        if let Ok(status) = rpc(dir, "status", json!({})).await {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "node not ready yet; inspect {}",
        absolute.join("node.log").display()
    ))
}
async fn stop(dir: &Path) -> Result<Value> {
    let result = rpc(dir, "stop", json!({})).await?;
    for _ in 0..150 {
        if config::lock(dir).is_ok() {
            return Ok(result);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("shutdown is still in progress".into())
}

/// std::process currently inherits unrelated inheritable Windows handles.
/// A daemon must not keep a piped CLI's stdout/stderr open after the CLI exits.
/// This runs only in `start`, immediately before spawn; the daemon's three
/// standard streams are explicitly redirected to null/log by Command.
#[cfg(windows)]
#[allow(unsafe_code)]
fn close_stdio_inheritance() -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, HANDLE_FLAG_INHERIT, SetHandleInformation,
    };
    let handles = [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ];
    for handle in handles {
        let mut flags = 0;
        // SAFETY: These are borrowed standard handles owned by the process.
        // Neither API closes them; the output pointer references a live u32.
        if unsafe { GetHandleInformation(handle, &mut flags) } != 0
            && flags & HANDLE_FLAG_INHERIT != 0
        {
            // SAFETY: The preceding query established a valid handle. Only its
            // inheritance flag changes; access and ownership remain unchanged.
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
    }
    Ok(())
}
async fn execute(cli: Cli) -> Result<Option<Value>> {
    let directory = match cli.data_dir {
        Some(path) => path,
        None => config::default_directory()?,
    };
    let dir = &directory;
    // Fail before creating locks or directories for an uninitialized node.
    if !matches!(&cli.command, Action::Init { .. }) {
        Config::load(dir)?;
    }
    let value = match cli.command {
        Action::Init {
            name,
            control,
            p2p,
            api_bind,
            api,
            dev,
            genesis,
        } => {
            let (g, public) = match (&genesis, dev) {
                (Some(path), _) => (
                    ting_core::protocol::Genesis::decode(
                        &std::fs::read(path).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?,
                    false,
                ),
                (None, true) => (config::development_genesis(), false),
                (None, false) => (
                    config::production_genesis().ok_or(
                        "the production network has not launched yet: use `ting init --dev` for a development chain, or `--genesis FILE` to join another network",
                    )?,
                    true,
                ),
            };
            let p2p = p2p.unwrap_or_else(|| {
                let host = if public { "0.0.0.0" } else { "127.0.0.1" };
                format!("{host}:18442").parse().unwrap()
            });
            let c = Config {
                name,
                control,
                p2p,
                api: api_bind,
                api_enabled: api,
                ..Default::default()
            };
            let g = config::initialize_with(dir, &c, g)?;
            json!({"initialized":dir,"chain_id":ting_core::hash::hex(&g.chain_id()),"api_enabled":api})
        }
        Action::Run => {
            daemon::run(dir).await?;
            return Ok(None);
        }
        Action::Start => start(dir).await?,
        Action::Stop => stop(dir).await?,
        Action::Status => rpc(dir, "status", json!({})).await?,
        Action::Peers => rpc(dir, "peers", json!({})).await?,
        Action::Probe { address } => probe(dir, address).await?,
        Action::Add(PeerCommand::Node { address }) => {
            edit(dir, "add_peer", json!({"address":address.to_string()})).await?
        }
        Action::Remove(PeerCommand::Node { address }) => {
            edit(dir, "remove_peer", json!({"address":address.to_string()})).await?
        }
        Action::Api(action) => {
            let (enabled, cors) = match action {
                ApiCommand::Enable { cors } => (true, cors),
                ApiCommand::Disable => (false, false),
            };
            edit(dir, "api", json!({"enabled":enabled,"cors":cors})).await?
        }
        Action::Mine { count, attempts } => {
            rpc(dir, "mine", json!({"count":count,"attempts":attempts})).await?
        }
        Action::Miner(MinerCommand::Start { threads, spacing }) => {
            edit(
                dir,
                "miner",
                json!({"action":"start","threads":threads,"spacing":spacing}),
            )
            .await?
        }
        Action::Miner(MinerCommand::Stop) => edit(dir, "miner", json!({"action":"stop"})).await?,
        Action::Miner(MinerCommand::Status) => {
            rpc(dir, "miner", json!({"action":"status"})).await?
        }
        Action::Miner(MinerCommand::Producer { hex }) => {
            edit(
                dir,
                "miner",
                json!({"action":"producer","producer":hex.unwrap_or_default()}),
            )
            .await?
        }
        Action::Template { transactions } => {
            let params = if transactions.is_empty() {
                json!({})
            } else {
                json!({"transactions":transactions})
            };
            rpc(dir, "template", params).await?
        }
        Action::Solution { template, nonce } => {
            rpc(dir, "solution", json!({"template":template,"nonce":nonce})).await?
        }
        Action::Submit { file, not_before } => {
            // The node refuses one larger than the next block's data capacity.
            rpc(dir,"submit",json!({"hex":ting_core::hash::hex(&std::fs::read(file).map_err(|e|e.to_string())?),"not_before":not_before})).await?
        }
        Action::Invalidate { block } => rpc(dir, "invalidate", json!({"id":block})).await?,
        Action::Reconsider { block } => rpc(dir, "reconsider", json!({"id":block})).await?,
        Action::Docs => {
            let status = rpc(dir, "status", json!({})).await?;
            let address = status["listeners"]["api"]
                .as_str()
                .ok_or("REST is disabled; run ting api enable")?;
            json!({"wiki":format!("http://{address}/wiki"),"api_docs":format!("http://{address}/docs"),"openapi":format!("http://{address}/openapi.json")})
        }
    };
    Ok(Some(value))
}
#[tokio::main]
async fn main() {
    match execute(Cli::parse()).await {
        Ok(Some(value)) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Ok(None) => {}
        Err(error) => {
            eprintln!("ting: {error}");
            std::process::exit(1);
        }
    }
}
