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

mod show;

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
    /// Print each result as JSON, for scripts, instead of as text.
    #[arg(long, global = true)]
    json: bool,
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
    /// Add a configured peer, which the node always keeps a connection to.
    #[command(subcommand)]
    Add(PeerCommand),
    /// Remove a configured peer.
    #[command(subcommand)]
    Remove(PeerCommand),
    /// Mine blocks with a bounded nonce search in one request, for a development chain.
    Mine {
        #[arg(long, default_value_t = 1)]
        count: u64,
        #[arg(long, default_value_t = 1_000_000)]
        attempts: u64,
    },
    /// The built-in CPU miner: start, stop or inspect it, and what the blocks
    /// this node builds carry.
    #[command(subcommand)]
    Miner(MinerCommand),
    /// Show where capacity voting stands: the share of this epoch's work voting
    /// each way so far, the capacities already decided, and this node's votes.
    Votes,
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
    /// Turn the REST interface, which serves the explorer and the wiki, on or off.
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
    /// The node at IP:PORT.
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
    /// Stop mining; remembered across restarts.
    Stop,
    /// Show whether the miner runs, what it has mined, and what its blocks carry.
    Status,
    /// Set the producer data of the blocks this node builds (hex), or clear it.
    Producer { hex: Option<String> },
    /// Set the capacity votes of the blocks this node builds: compute, state
    /// access, data and growth, each hold, increase or decrease.
    Votes {
        #[arg(num_args = 4, required = true, value_names = ["COMPUTE", "STATE", "DATA", "GROWTH"])]
        votes: Vec<Vote>,
    },
}
#[derive(Clone, Copy, clap::ValueEnum)]
enum Vote {
    Hold,
    Increase,
    Decrease,
}
#[derive(Subcommand)]
enum ApiCommand {
    /// Serve REST, by default at 127.0.0.1:18441.
    Enable {
        /// Answer browser applications from any origin, such as a wallet in a web page.
        #[arg(long)]
        cors: bool,
    },
    /// Stop serving REST; the CLI keeps working over its own channel.
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
        "miner" if params["action"] == "votes" => {
            c.votes = serde_json::from_value(params["votes"].clone()).map_err(|e| e.to_string())?;
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
/// data directory's network, shown as text (wiki: reference/peer-protocol).
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
/// Starts the node in the background; returns its status and whether this
/// call started it.
async fn start(dir: &Path) -> Result<(Value, bool)> {
    if let Ok(status) = rpc(dir, "status", json!({})).await {
        return Ok((status, false));
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
            return Ok((status, true));
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
/// The command as the user would type it again: `ting`, with the data
/// directory they named, for the hints in a result's text.
fn invocation(data_dir: Option<&Path>) -> String {
    match data_dir {
        Some(path) => {
            let path = path.display().to_string();
            if path.contains(char::is_whitespace) {
                format!("ting --data-dir \"{path}\"")
            } else {
                format!("ting --data-dir {path}")
            }
        }
        None => "ting".to_owned(),
    }
}
/// The network of a genesis, as the node's status names it.
fn network(g: &ting_core::protocol::Genesis) -> &'static str {
    let id = g.chain_id();
    if config::production_genesis().is_some_and(|p| p.chain_id() == id) {
        "production"
    } else if config::development_genesis().chain_id() == id {
        "development"
    } else {
        "other"
    }
}
/// The text of `ting status`, which also shows the peers and the miner.
async fn status_text(dir: &Path, status: &Value) -> Result<String> {
    let peers = rpc(dir, "peers", json!({})).await?;
    let miner = rpc(dir, "miner", json!({"action":"status"})).await?;
    Ok(show::status(status, &peers, &miner))
}
/// Runs the command; returns its result and the text that shows it, or
/// nothing for `ting run`.
async fn execute(cli: Cli) -> Result<Option<(Value, String)>> {
    let ting = invocation(cli.data_dir.as_deref());
    let directory = match cli.data_dir {
        Some(path) => path,
        None => config::default_directory()?,
    };
    let dir = &directory;
    // Fail before creating locks or directories for an uninitialized node.
    if !matches!(&cli.command, Action::Init { .. }) {
        Config::load(dir)?;
    }
    let shown = match cli.command {
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
            let value = json!({"initialized":dir,"chain_id":ting_core::hash::hex(&g.chain_id()),"api_enabled":api});
            let height = ting_node::seed::ids(&g).len();
            let text = show::init(&value, network(&g), height, &ting);
            (value, text)
        }
        Action::Run => {
            daemon::run(dir).await?;
            return Ok(None);
        }
        Action::Start => {
            let (status, fresh) = start(dir).await?;
            let summary = status_text(dir, &status).await?;
            let log = std::fs::canonicalize(dir)
                .unwrap_or_else(|_| dir.clone())
                .join("node.log");
            let text = show::started(&status, fresh, &log.display().to_string(), summary);
            (status, text)
        }
        Action::Stop => (stop(dir).await?, "Stopped the node.\n".into()),
        Action::Status => {
            let status = rpc(dir, "status", json!({})).await?;
            let text = status_text(dir, &status).await?;
            (status, text)
        }
        Action::Peers => {
            let peers = rpc(dir, "peers", json!({})).await?;
            let text = show::peers(&peers);
            (peers, text)
        }
        Action::Probe { address } => {
            let answer = probe(dir, address).await?;
            let chain_id = ting_core::hash::hex(&config::genesis(dir)?.chain_id());
            let text = show::probe(&address.to_string(), &answer, &chain_id);
            (answer, text)
        }
        Action::Add(PeerCommand::Node { address }) => {
            let result = edit(dir, "add_peer", json!({"address":address.to_string()})).await?;
            let text = show::peer_edit(&result, true, &address.to_string());
            (result, text)
        }
        Action::Remove(PeerCommand::Node { address }) => {
            let result = edit(dir, "remove_peer", json!({"address":address.to_string()})).await?;
            let text = show::peer_edit(&result, false, &address.to_string());
            (result, text)
        }
        Action::Api(action) => {
            let (enabled, cors) = match action {
                ApiCommand::Enable { cors } => (true, cors),
                ApiCommand::Disable => (false, false),
            };
            let result = edit(dir, "api", json!({"enabled":enabled,"cors":cors})).await?;
            let text = show::api(&result, enabled, cors);
            (result, text)
        }
        Action::Mine { count, attempts } => {
            let result = rpc(dir, "mine", json!({"count":count,"attempts":attempts})).await?;
            let text = show::mined(&result, count, attempts);
            (result, text)
        }
        Action::Miner(MinerCommand::Start { threads, spacing }) => {
            let result = edit(
                dir,
                "miner",
                json!({"action":"start","threads":threads,"spacing":spacing}),
            )
            .await?;
            let text = show::miner_started(&result);
            (result, text)
        }
        Action::Miner(MinerCommand::Stop) => {
            let result = edit(dir, "miner", json!({"action":"stop"})).await?;
            let text = show::miner_stopped(&result);
            (result, text)
        }
        Action::Miner(MinerCommand::Status) => {
            let result = rpc(dir, "miner", json!({"action":"status"})).await?;
            let text = show::miner(&result);
            (result, text)
        }
        Action::Miner(MinerCommand::Votes { votes }) => {
            let votes: Vec<u8> = votes
                .iter()
                .map(|vote| match vote {
                    Vote::Hold => 0,
                    Vote::Increase => 1,
                    Vote::Decrease => 2,
                })
                .collect();
            let result = edit(dir, "miner", json!({"action":"votes","votes":votes})).await?;
            let text = show::votes_set(&result, &json!(votes));
            (result, text)
        }
        Action::Votes => {
            let result = rpc(dir, "votes", json!({})).await?;
            let text = show::votes(&result);
            (result, text)
        }
        Action::Miner(MinerCommand::Producer { hex }) => {
            let hex = hex.unwrap_or_default();
            let result = edit(dir, "miner", json!({"action":"producer","producer":hex})).await?;
            let text = show::producer(&result, &hex);
            (result, text)
        }
        Action::Template { transactions } => {
            let params = if transactions.is_empty() {
                json!({})
            } else {
                json!({"transactions":transactions})
            };
            let result = rpc(dir, "template", params).await?;
            let text = show::template(&result, &ting);
            (result, text)
        }
        Action::Solution { template, nonce } => {
            let result = rpc(dir, "solution", json!({"template":template,"nonce":nonce})).await?;
            let text = show::solution(&result);
            (result, text)
        }
        Action::Submit { file, not_before } => {
            // The node refuses one larger than the next block's data capacity.
            let result = rpc(dir,"submit",json!({"hex":ting_core::hash::hex(&std::fs::read(file).map_err(|e|e.to_string())?),"not_before":not_before})).await?;
            let text = show::submitted(&result);
            (result, text)
        }
        Action::Invalidate { block } => {
            let result = rpc(dir, "invalidate", json!({"id":block})).await?;
            let text = show::invalidated(&result);
            (result, text)
        }
        Action::Reconsider { block } => {
            let result = rpc(dir, "reconsider", json!({"id":block})).await?;
            let text = show::reconsidered(&result);
            (result, text)
        }
        Action::Docs => {
            let status = rpc(dir, "status", json!({})).await?;
            let address = status["listeners"]["api"]
                .as_str()
                .ok_or_else(|| format!("REST is disabled; run {ting} api enable"))?;
            let result = json!({"wiki":format!("http://{address}/wiki"),"api_docs":format!("http://{address}/docs"),"openapi":format!("http://{address}/openapi.json")});
            let text = show::docs(&result);
            (result, text)
        }
    };
    Ok(Some(shown))
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    match execute(cli).await {
        Ok(Some((value, _))) if json => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap())
        }
        Ok(Some((_, text))) => print!("{text}"),
        Ok(None) => {}
        Err(error) => {
            eprintln!("ting: {error}");
            std::process::exit(1);
        }
    }
}
