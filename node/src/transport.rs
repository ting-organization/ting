use crate::{
    FRAME_MAX, Result,
    chain::{Event, INVALID, TOO_LARGE, is_invalid, transfer_time},
    config,
    daemon::Node,
    discovery::{self, Source},
    peers, wire,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use ting_core::{
    hash::{Hash, hex},
    protocol::HEADER_LEN,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, oneshot},
    time::Instant,
};

pub type PeerStats = Arc<Mutex<BTreeMap<String, Value>>>;
/// Open inbound connections and their remote addresses, so that a ban ends
/// the ones it covers at once and no one network takes every slot.
#[derive(Default)]
pub struct Inbound {
    next: u64,
    open: BTreeMap<u64, (IpAddr, oneshot::Sender<()>)>,
}
/// Most inbound connections at once, and from one address range (wiki:
/// node/sync), this machine aside.
const INBOUND_MAX: usize = 125;
const INBOUND_PER_RANGE: usize = 8;
/// What inbound connections give way by: a public address's network group (an
/// IPv4 /16, an IPv6 /32), and a local one's range.
fn network(ip: IpAddr) -> IpAddr {
    discovery::group(ip).unwrap_or_else(|| discovery::range(ip))
}
impl Inbound {
    /// Takes a connection from `ip` if the limits allow: under its range's
    /// share, and with every slot taken, in place of the newest connection of
    /// the network holding the most, if that network holds at least two more
    /// than `ip`'s. Many addresses of one network therefore cannot hold every
    /// slot, and networks of one connection each are never churned.
    pub fn admit(&mut self, ip: IpAddr) -> Option<(u64, oneshot::Receiver<()>)> {
        let range = discovery::range(ip);
        let ranged = self
            .open
            .values()
            .filter(|(open, _)| discovery::range(*open) == range)
            .count();
        if ranged >= INBOUND_PER_RANGE && !discovery::loopback(ip) {
            return None;
        }
        if self.open.len() >= INBOUND_MAX {
            let mut held: BTreeMap<IpAddr, usize> = BTreeMap::new();
            for (open, _) in self.open.values() {
                *held.entry(network(*open)).or_default() += 1;
            }
            let own = held.get(&network(ip)).copied().unwrap_or(0);
            let (&largest, &most) = held.iter().max_by_key(|(_, n)| **n)?;
            if most <= own + 1 {
                return None;
            }
            let newest = self
                .open
                .iter()
                .rev()
                .find(|(_, (open, _))| network(*open) == largest)
                .map(|(key, _)| *key)?;
            if let Some((_, end)) = self.open.remove(&newest) {
                let _ = end.send(());
            }
        }
        Some(self.open(ip))
    }
    fn open(&mut self, ip: IpAddr) -> (u64, oneshot::Receiver<()>) {
        let (end, ended) = oneshot::channel();
        self.next += 1;
        self.open.insert(self.next, (ip, end));
        (self.next, ended)
    }
    /// Ends every connection from an address `banned` covers.
    pub fn end(&mut self, banned: impl Fn(IpAddr) -> bool) {
        let ending: Vec<u64> = self
            .open
            .iter()
            .filter(|(_, (ip, _))| banned(*ip))
            .map(|(key, _)| *key)
            .collect();
        for key in ending {
            if let Some((_, end)) = self.open.remove(&key) {
                let _ = end.send(());
            }
        }
    }
}
/// An inbound connection's place in `Inbound`, given up when it ends.
struct Registered<'a>(&'a Mutex<Inbound>, u64);
impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().open.remove(&self.1);
    }
}
/// The time a frame may take once it has started.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(5);
pub async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Value> {
    read_frame_within(stream, FRAME_MAX).await
}
/// A frame of at most `max` bytes, such as the 4 KiB a `hello` needs before a
/// peer has said who it is.
pub async fn read_frame_within<S: AsyncRead + Unpin>(stream: &mut S, max: usize) -> Result<Value> {
    tokio::time::timeout(FRAME_DEADLINE, read_frame_sized(stream, None, max))
        .await
        .map_err(|_| "read deadline exceeded")?
}
/// The most bytes a peer's first frame, its `hello`, may have.
const HELLO_MAX: usize = 4096;
/// The most JSON structure, brackets, braces, commas and colons, one control
/// frame may hold: a parsed value of many tiny elements is many times its
/// bytes.
const STRUCTURE_MAX: usize = 65_536;
async fn read_frame_sized<S: AsyncRead + Unpin>(
    stream: &mut S,
    idle: Option<Duration>,
    max: usize,
) -> Result<Value> {
    let bytes = read_bytes(stream, idle, max).await?;
    let structure = bytes
        .iter()
        .filter(|b| matches!(b, b'[' | b'{' | b',' | b':'))
        .count();
    if structure > STRUCTURE_MAX {
        return Err("frame structure limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
/// A frame's bytes: its length, as a `u32`, within `idle` if given, then the
/// rest within the frame deadline.
async fn read_bytes<S: AsyncRead + Unpin>(
    stream: &mut S,
    idle: Option<Duration>,
    max: usize,
) -> Result<Vec<u8>> {
    let size = match idle {
        Some(idle) => tokio::time::timeout(idle, stream.read_u32_le())
            .await
            .map_err(|_| "idle connection closed")?,
        None => stream.read_u32_le().await,
    }
    .map_err(|e| e.to_string())? as usize;
    if size == 0 || size > max {
        return Err("frame size limit".into());
    }
    // The buffer grows as the bytes arrive: a length alone reserves nothing.
    let mut bytes = Vec::with_capacity(size.min(64 * 1024));
    tokio::time::timeout(
        FRAME_DEADLINE,
        (&mut *stream).take(size as u64).read_to_end(&mut bytes),
    )
    .await
    .map_err(|_| "read deadline exceeded")?
    .map_err(|e| e.to_string())?;
    if bytes.len() != size {
        return Err("connection closed within a frame".into());
    }
    Ok(bytes)
}
pub async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, value: &Value) -> Result<()> {
    write_frame_within(stream, value, FRAME_MAX).await
}
/// A frame of at most `max` bytes, such as a control request carrying a
/// transaction larger than an ordinary frame.
async fn write_frame_within<S: AsyncWrite + Unpin>(
    stream: &mut S,
    value: &Value,
    max: usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    write_bytes(stream, &bytes, max).await
}
async fn write_bytes<S: AsyncWrite + Unpin>(
    stream: &mut S,
    bytes: &[u8],
    max: usize,
) -> Result<()> {
    if bytes.is_empty() || bytes.len() > max || u32::try_from(bytes.len()).is_err() {
        return Err("frame size limit".into());
    }
    // The length and the frame in one write: written apart, the frame would
    // wait for the peer to acknowledge the length, a round trip each time.
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend((bytes.len() as u32).to_le_bytes());
    frame.extend(bytes);
    tokio::time::timeout(FRAME_DEADLINE, stream.write_all(&frame))
        .await
        .map_err(|_| "write deadline exceeded")?
        .map_err(|e| e.to_string())
}
/// A connection sends each frame as soon as it is written: requests and
/// answers are small and wait for each other, so holding one back for more
/// data (Nagle's algorithm) only adds round trips.
fn prompt(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
}
fn reply(result: Result<Value>) -> Value {
    match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error}),
    }
}
pub async fn connect(address: SocketAddr) -> Result<TcpStream> {
    let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(address))
        .await
        .map_err(|_| "connect deadline exceeded")?
        .map_err(|e| e.to_string())?;
    prompt(&stream);
    Ok(stream)
}
pub async fn control_call(
    address: SocketAddr,
    token: &str,
    method: &str,
    params: Value,
) -> Result<Value> {
    if !address.ip().is_loopback() {
        return Err("control address must be loopback".into());
    }
    let mut s = connect(address).await?;
    // The node proves it holds the token before anything else is sent, so a
    // stale `runtime.json` pointing at another listener reveals nothing.
    let ours = fresh_nonce()?;
    write_frame(&mut s, &json!({"nonce":hex(&ours)})).await?;
    let hello = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame_within(&mut s, CONTROL_HELLO_MAX),
    )
    .await
    .map_err(|_| "control handshake timed out")??;
    let Ok(theirs) = nonce_of(&hello["nonce"]) else {
        return Err(format!(
            "control handshake failed: {}",
            hello["error"].as_str().unwrap_or("no nonce")
        ));
    };
    let expected = hex(&mac(token, &[NODE_PROOF, &ours, &theirs]));
    if !config::authorized(&expected, hello["proof"].as_str().unwrap_or("")) {
        return Err(NOT_THIS_NODE.into());
    }
    let request = json!({"method":method,"params":params}).to_string();
    let proof = hex(&mac(
        token,
        &[CLIENT_PROOF, &theirs, &ours, request.as_bytes()],
    ));
    // The node bounds the request by its data capacity.
    write_frame_within(
        &mut s,
        &json!({"request":request,"proof":proof}),
        u32::MAX as usize,
    )
    .await?;
    // Mining can occupy the actor longer than a peer I/O deadline.
    let r = tokio::time::timeout(Duration::from_secs(35), async {
        let size = s.read_u32_le().await.map_err(|e| e.to_string())? as usize;
        if size == 0 || size > FRAME_MAX {
            return Err("control frame size limit".into());
        }
        let mut b = vec![0; size];
        s.read_exact(&mut b).await.map_err(|e| e.to_string())?;
        serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "control request timed out")??;
    if r["ok"] != true {
        return Err(r["error"]
            .as_str()
            .unwrap_or("invalid control reply")
            .into());
    }
    Ok(r["result"].clone())
}
/// What the CLI says when the listener at the control address cannot prove it
/// holds this data directory's token.
pub const NOT_THIS_NODE: &str = "the control listener did not prove it holds this data \
     directory's token: runtime.json may be stale, or the token another node's";
const NODE_PROOF: &[u8] = b"ting-control-node";
const CLIENT_PROOF: &[u8] = b"ting-control-client";
/// The first frame of a control connection carries a nonce, and its answer a
/// nonce and a proof.
const CONTROL_HELLO_MAX: usize = 1024;
/// HMAC-SHA-256 (RFC 2104) of the concatenated `parts`, keyed with the token's
/// 64 characters.
fn mac(token: &str, parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut key = [0u8; 64];
    match token.as_bytes() {
        long if long.len() > 64 => key[..32].copy_from_slice(&Sha256::digest(long)),
        short => key[..short.len()].copy_from_slice(short),
    }
    let mut inner = Sha256::new();
    inner.update(key.map(|b| b ^ 0x36));
    for part in parts {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(key.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}
fn fresh_nonce() -> Result<[u8; 32]> {
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(|e| format!("no randomness for a nonce: {e}"))?;
    Ok(nonce)
}
fn nonce_of(value: &Value) -> Result<[u8; 32]> {
    let bytes = crate::unhex(value.as_str().ok_or("expected nonce")?, 32)?;
    bytes
        .try_into()
        .map_err(|_| "expected a 32-byte nonce".into())
}
/// The node's half of a control connection's handshake (wiki: node/security):
/// it proves it holds the token over both nonces, and takes the request only
/// with the client's proof over both nonces and the request's exact text. The
/// token itself never crosses the connection.
async fn accept_control(stream: &mut TcpStream, token: &str, data: u64) -> Result<Value> {
    let hello = read_frame_within(stream, CONTROL_HELLO_MAX).await?;
    let theirs = nonce_of(&hello["nonce"])?;
    let ours = fresh_nonce()?;
    let proof = mac(token, &[NODE_PROOF, &theirs, &ours]);
    write_frame(stream, &json!({"nonce":hex(&ours),"proof":hex(&proof)})).await?;
    // A request may carry a transaction as large as a block holds, in hex.
    let data = usize::try_from(data).unwrap_or(usize::MAX);
    let most = FRAME_MAX.max(data.saturating_mul(2).saturating_add(65_536));
    let frame = read_frame_within(stream, most).await?;
    let request = frame["request"].as_str().unwrap_or("");
    let expected = hex(&mac(
        token,
        &[CLIENT_PROOF, &ours, &theirs, request.as_bytes()],
    ));
    if !config::authorized(&expected, frame["proof"].as_str().unwrap_or("")) {
        let _ = write_frame(stream, &reply(Err("unauthorized".into()))).await;
        return Err("unauthorized".into());
    }
    serde_json::from_str(request).map_err(|e| e.to_string())
}
pub async fn control_server(listener: TcpListener, node: Node) {
    let slots = Arc::new(Semaphore::new(16));
    loop {
        let accepted = tokio::select! { a=listener.accept()=>a, _=node.stopped()=>break };
        let Ok((mut stream, _)) = accepted else {
            // Out of descriptors or a passing network error: wait, go on.
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        prompt(&stream);
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let node = node.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let data = node.handle.events().borrow().data;
            let Ok(request) = accept_control(&mut stream, &node.token, data).await else {
                return;
            };
            let method = request["method"].as_str().unwrap_or("");
            let result = if matches!(
                method,
                "status"
                    | "peers"
                    | "add_peer"
                    | "remove_peer"
                    | "mine"
                    | "template"
                    | "solution"
                    | "miner"
                    | "votes"
                    | "submit"
                    | "api"
                    | "invalidate"
                    | "reconsider"
                    | "stop"
            ) {
                node.call(method, request["params"].clone()).await
            } else {
                Err("unknown control method".into())
            };
            let stopping = method == "stop" && result.is_ok();
            let _ = write_frame(&mut stream, &reply(result)).await;
            if stopping {
                node.shutdown.store(true, Ordering::Relaxed);
            }
        });
    }
}
/// A peer frame of at most `max` bytes, its length coming within `idle` if
/// given and the rest within the frame deadline (wiki: reference/peer-protocol).
pub async fn read_wire<S: AsyncRead + Unpin>(
    stream: &mut S,
    idle: Option<Duration>,
    max: usize,
) -> Result<wire::Frame> {
    let bytes = read_bytes(stream, idle, max).await?;
    wire::Frame::decode(&bytes)
}
pub async fn write_wire<S: AsyncWrite + Unpin>(stream: &mut S, frame: &wire::Frame) -> Result<()> {
    write_bytes(stream, &frame.encode(), FRAME_MAX).await
}
/// Sends a request and waits for its answer, within the frame deadline: a
/// hello, or a test or tool talking to a peer directly.
pub async fn ask(stream: &mut TcpStream, request: wire::Request) -> Result<wire::Answer> {
    write_wire(stream, &wire::Frame::Request(request)).await?;
    tokio::time::timeout(FRAME_DEADLINE, async {
        loop {
            match read_wire(stream, None, FRAME_MAX).await? {
                wire::Frame::Answer(answer) => return Ok(answer),
                wire::Frame::Error(error) => return Err(error),
                wire::Frame::Announcement(_) => continue,
                wire::Frame::Request(_) => return Err("unexpected frame".into()),
            }
        }
    })
    .await
    .map_err(|_| "read deadline exceeded")?
}
pub const ITSELF: &str = "connected to itself";
/// What a node answers a hello it does not take.
pub const MISMATCH: &str = "peer version or genesis mismatch";
/// Whether a peer's hello may be taken: a version this node speaks, and its
/// own genesis.
pub fn compatible(version: u32, chain_id: &Hash, own: &Hash) -> bool {
    version >= MIN_VERSION && chain_id == own
}
const MIN_VERSION: u32 = 1;
/// An inbound connection with no request for this long is closed; a node
/// sends at least one every 30 seconds.
const IDLE: Duration = Duration::from_secs(120);
/// Announcements wait this long, so that a burst of changes is told once.
const ANNOUNCE_DELAY: Duration = Duration::from_millis(200);
/// Requests one connection may make, counted by cost (most 1, a header page
/// 8, and a request that carries a block or a transaction 1 more for every
/// `CARRIED` bytes): this many a second, in bursts of up to `BURST`, so that a
/// connection draws at most about 12 MiB a second of blocks, many small ones
/// or a few large. Requests beyond the rate wait.
const RATE: f64 = 200.0;
const BURST: f64 = 2_000.0;
const CARRIED: usize = 64 * 1024;
/// What carrying `bytes` of a block or transaction adds to a request's cost.
fn carried(bytes: usize) -> f64 {
    bytes.div_ceil(CARRIED) as f64
}
/// What a peer is told when this node's tip or pool changes.
pub fn announcement(event: &Event) -> wire::Announcement {
    wire::Announcement {
        tip: event.tip,
        height: event.height,
        work: event.work.parse().unwrap_or_default(),
        pending: event.pending as u64,
        deferred: event.deferred as u64,
    }
}
pub async fn peer_server(listener: TcpListener, node: Node) {
    loop {
        let accepted =
            tokio::select! { a=listener.accept()=>a, _=wait_stopped(&node.shutdown)=>break };
        let Ok((stream, remote)) = accepted else {
            // Out of descriptors or a passing network error: wait, go on.
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        prompt(&stream);
        let remote = SocketAddr::new(remote.ip().to_canonical(), remote.port());
        if node
            .book
            .lock()
            .unwrap()
            .is_banned(remote.ip(), crate::now())
        {
            continue;
        }
        let Some((key, ended)) = node.inbound.lock().unwrap().admit(remote.ip()) else {
            continue;
        };
        let node = node.clone();
        tokio::spawn(async move {
            let _registered = Registered(&node.inbound, key);
            serve(stream, remote, node.clone(), ended).await;
        });
    }
}
/// A block (`import`) or transaction (`submit`) a peer pushes in slices, as
/// far as it has come, and the request that takes it in once it is whole.
struct Pushed {
    method: &'static str,
    params: Value,
    bytes: Vec<u8>,
    total: u64,
    deadline: Instant,
}
/// The chain's answer to a push taken whole.
fn taken(answer: Value) -> Result<wire::Answer> {
    Ok(wire::Answer::Taken {
        id: crate::hash_arg(answer["id"].as_str().ok_or("invalid chain answer")?)?,
        known: answer["known"] == true,
    })
}
/// A block (`import`) or transaction (`submit`, with `not_before` for a
/// deferred one) a peer pushes (wiki: node/sync). One that comes whole, in a
/// single part, is taken at once. Otherwise the first part, at offset 0, sets
/// the size: a block's header, which it holds, bounds it by the data capacity
/// at its height, and a transaction is bounded by the next block's. The
/// others follow in order, within the item's transfer time, and the last
/// takes the whole item in.
async fn pushed(
    node: &Node,
    peer: &str,
    pushed: &mut Option<Pushed>,
    method: &'static str,
    part: wire::Part,
    not_before: Option<u64>,
) -> Result<wire::Answer> {
    let params = if method == "import" {
        json!({"peer":peer})
    } else {
        json!({"relayed":true,"peer":peer,"not_before":not_before})
    };
    if part.offset == 0 && part.bytes.len() as u64 == part.total {
        return taken(
            node.handle
                .call_external_bytes(method, params, Some(part.bytes))
                .await?,
        );
    }
    if part.offset == 0 {
        *pushed = None;
        if method == "import" {
            let header = part
                .bytes
                .get(..HEADER_LEN)
                .ok_or(format!("{INVALID}block: truncated header"))?;
            let checked = node
                .handle
                .call_external_bytes("check_header", Value::Null, Some(header.to_vec()))
                .await?;
            if checked["known"] == true {
                return taken(checked);
            }
            if part.total > checked["max"].as_u64().ok_or("invalid header check")? {
                return Err(format!("{INVALID}block: larger than its capacity"));
            }
        } else if part.total > node.handle.events().borrow().data {
            return Err(TOO_LARGE.into());
        }
        *pushed = Some(Pushed {
            method,
            params,
            bytes: Vec::new(),
            total: part.total,
            deadline: Instant::now() + transfer_time(part.total),
        });
    }
    let item = pushed.as_mut().ok_or("nothing being pushed")?;
    let end = part.offset.saturating_add(part.bytes.len() as u64);
    if item.method != method
        || part.offset != item.bytes.len() as u64
        || part.total != item.total
        || end > part.total
        || Instant::now() > item.deadline
    {
        *pushed = None;
        return Err("slice out of order".into());
    }
    if end < part.total && part.bytes.is_empty() {
        *pushed = None;
        return Err("empty slice".into());
    }
    item.bytes.extend(part.bytes);
    if end < part.total {
        return Ok(wire::Answer::Received(end));
    }
    let item = pushed.take().ok_or("nothing being pushed")?;
    taken(
        node.handle
            .call_external_bytes(item.method, item.params, Some(item.bytes))
            .await?,
    )
}
/// One inbound connection: `hello`, then requests answered in order for as
/// long as the peer keeps sending them. A peer that asked for announcements in
/// its hello is also told, between answers, when this node's tip or pool
/// changes.
async fn serve(
    mut stream: TcpStream,
    remote: SocketAddr,
    node: Node,
    mut ended: oneshot::Receiver<()>,
) {
    // The hello, whole, within the frame deadline: a connection that says
    // nothing holds no slot for long.
    let Ok(Ok(first)) =
        tokio::time::timeout(FRAME_DEADLINE, read_wire(&mut stream, None, HELLO_MAX)).await
    else {
        return;
    };
    let Ok(wire::Answer::State(mut local)) =
        node.handle.call_external_wire(wire::Request::State).await
    else {
        return;
    };
    let hello = match first {
        wire::Frame::Request(wire::Request::Hello(hello))
            if compatible(hello.version, &hello.chain_id, &local.chain_id) =>
        {
            hello
        }
        _ => {
            let _ = write_wire(&mut stream, &wire::Frame::Error(MISMATCH.into())).await;
            return;
        }
    };
    if hello.nonce == node.nonce {
        let _ = write_wire(&mut stream, &wire::Frame::Error(ITSELF.into())).await;
        return;
    }
    // The listening port a peer announces, at the address it connected from,
    // is a candidate for others to connect to.
    let announced = hello.listen.map(|port| SocketAddr::new(remote.ip(), port));
    if node.discovery
        && let Some(address) = announced
    {
        node.book.lock().unwrap().learn(address, Source::Inbound);
    }
    let mut events = node.handle.events();
    let mut told = announcement(&events.borrow_and_update());
    local.nonce = node.nonce;
    if write_wire(
        &mut stream,
        &wire::Frame::Answer(wire::Answer::State(local)),
    )
    .await
    .is_err()
    {
        return;
    }
    let (mut reader, mut writer) = stream.into_split();
    // Answers and announcements leave through one writer, in order.
    let (out, mut outgoing) = mpsc::channel::<wire::Frame>(8);
    let writing = tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            if write_wire(&mut writer, &frame).await.is_err() {
                break;
            }
        }
    });
    let announcing = hello.announce.then(|| {
        let out = out.clone();
        tokio::spawn(async move {
            while events.changed().await.is_ok() {
                tokio::time::sleep(ANNOUNCE_DELAY).await;
                let now = announcement(&events.borrow_and_update());
                if now != told {
                    told = now.clone();
                    if out.send(wire::Frame::Announcement(now)).await.is_err() {
                        break;
                    }
                }
            }
        })
    });
    let peer = remote.to_string();
    let (mut tokens, mut filled) = (BURST, Instant::now());
    let mut pushing = None;
    loop {
        let frame = tokio::select! {
            r=read_wire(&mut reader, Some(IDLE), FRAME_MAX)=>r,
            _=&mut ended=>break,
            _=wait_stopped(&node.shutdown)=>break,
        };
        // Anything but a request this node knows ends the connection.
        let Ok(wire::Frame::Request(request)) = frame else {
            break;
        };
        // What a request takes from the allowance follows what it costs the
        // node: a page of headers is many small requests, and one that
        // carries a block or a transaction counts its bytes, the slices asked
        // for once they are answered.
        let cost = match &request {
            wire::Request::Import(part) | wire::Request::Submit { part, .. } => {
                1.0 + carried(part.bytes.len())
            }
            wire::Request::Headers { .. } => 8.0,
            wire::Request::State
            | wire::Request::Pending { .. }
            | wire::Request::Deferred { .. }
            | wire::Request::Inventory { .. } => 2.0,
            _ => 1.0,
        };
        tokens = (tokens + filled.elapsed().as_secs_f64() * RATE).min(BURST);
        filled = Instant::now();
        if tokens < cost {
            tokio::time::sleep(Duration::from_secs_f64((cost - tokens) / RATE)).await;
            tokens = cost;
            filled = Instant::now();
        }
        tokens -= cost;
        let result = match request {
            wire::Request::Hello(_) => Err(wire::UNKNOWN_REQUEST.into()),
            wire::Request::Addresses => {
                let addresses = if node.discovery {
                    let book = node.book.lock().unwrap();
                    book.share(remote.ip(), announced, crate::now(), discovery::seed())
                } else {
                    Vec::new()
                };
                Ok(wire::Answer::Addresses(addresses))
            }
            wire::Request::State => {
                match node.handle.call_external_wire(wire::Request::State).await {
                    Ok(wire::Answer::State(mut state)) => {
                        state.nonce = node.nonce;
                        Ok(wire::Answer::State(state))
                    }
                    other => other,
                }
            }
            wire::Request::Import(part) => {
                pushed(&node, &peer, &mut pushing, "import", part, None).await
            }
            wire::Request::Submit { part, not_before } => {
                pushed(&node, &peer, &mut pushing, "submit", part, not_before).await
            }
            // Header pages and block slices come from the database, beside
            // the chain thread.
            request @ (wire::Request::Headers { .. } | wire::Request::Raw { .. }) => {
                node.readers.wire(request).await
            }
            request @ (wire::Request::TransactionRaw { .. }
            | wire::Request::Inventory { .. }
            | wire::Request::Pending { .. }
            | wire::Request::Deferred { .. }) => node.handle.call_external_wire(request).await,
        };
        // An error is an answer like any other (a block it does not have, a
        // full pool, a transaction not taken) and the connection stays open,
        // unless the peer sent a second hello or invalid data, which also
        // bans it.
        if let Ok(wire::Answer::Slice { bytes, .. }) = &result {
            tokens -= carried(bytes.len());
        }
        let failed = match &result {
            Err(e) if is_invalid(e) => {
                peers::punish(&node, remote);
                true
            }
            Err(e) => e == wire::UNKNOWN_REQUEST,
            Ok(_) => false,
        };
        let frame = match result {
            Ok(answer) => wire::Frame::Answer(answer),
            Err(error) => wire::Frame::Error(error),
        };
        if out.send(frame).await.is_err() || failed {
            break;
        }
    }
    if let Some(task) = announcing {
        task.abort();
    }
    drop(out);
    let _ = tokio::time::timeout(FRAME_DEADLINE, writing).await;
}
pub async fn wait_stopped(shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ban_ends_the_open_connections_it_covers_and_no_others() {
        let inbound = Mutex::new(Inbound::default());
        let (bad, good) = ("8.8.1.1".parse().unwrap(), "9.9.1.1".parse().unwrap());
        let (first, mut first_ended) = inbound.lock().unwrap().open(bad);
        let (second, mut second_ended) = inbound.lock().unwrap().open(bad);
        let (third, mut third_ended) = inbound.lock().unwrap().open(good);
        let registered = [first, second, third].map(|key| Registered(&inbound, key));
        inbound.lock().unwrap().end(|ip| ip == bad);
        assert!(first_ended.try_recv().is_ok() && second_ended.try_recv().is_ok());
        assert!(third_ended.try_recv().is_err());
        assert_eq!(inbound.lock().unwrap().open.len(), 1);
        // A connection that ends gives up its place.
        drop(registered);
        assert!(inbound.lock().unwrap().open.is_empty());
    }

    #[test]
    fn one_address_range_cannot_take_every_inbound_slot() {
        let mut inbound = Inbound::default();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // Eight from one address, or one IPv6 /64; the ninth is refused.
        for _ in 0..INBOUND_PER_RANGE {
            assert!(inbound.admit(ip("8.8.1.1")).is_some());
        }
        assert!(inbound.admit(ip("8.8.1.1")).is_none());
        for i in 0..INBOUND_PER_RANGE {
            assert!(
                inbound
                    .admit(ip(&format!("2a00:1:2:3::{}", i + 1)))
                    .is_some()
            );
        }
        assert!(inbound.admit(ip("2a00:1:2:3:ffff::1")).is_none());
        // This machine is not held to the share.
        let mut ends = Vec::new();
        for _ in 0..INBOUND_MAX - 2 * INBOUND_PER_RANGE {
            ends.push(inbound.admit(ip("127.0.0.1")).unwrap().1);
        }
        assert_eq!(inbound.open.len(), INBOUND_MAX);
        // Full: a newcomer takes the place of the newest connection of the
        // network holding the most, here this machine's.
        assert!(inbound.admit(ip("9.9.1.1")).is_some());
        assert_eq!(inbound.open.len(), INBOUND_MAX);
        assert!(ends.last_mut().unwrap().try_recv().is_ok());
        assert!(ends[0].try_recv().is_err());
        // One whose network already holds as many as any other is refused:
        // networks of one connection each are never churned.
        let mut full = Inbound::default();
        for i in 0..INBOUND_MAX {
            assert!(full.admit(IpAddr::from([11, i as u8, 0, 1])).is_some());
        }
        assert!(full.admit(ip("12.0.0.1")).is_none());
    }

    #[test]
    fn many_addresses_of_one_network_cannot_hold_every_inbound_slot() {
        // Every slot taken by addresses of one network, one connection each,
        // as one operator's IPv4 /16 or IPv6 /32 provides them: each newcomer
        // from elsewhere takes the place of the newest of them, and the network
        // cannot take it back.
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let crowds: [fn(usize) -> IpAddr; 2] = [
            |i| IpAddr::from([11, 0, (i / 250) as u8, (i % 250) as u8 + 1]),
            |i| IpAddr::from([0x2a00, 1, i as u16, 0, 0, 0, 0, 1]),
        ];
        for crowd in crowds {
            let mut inbound = Inbound::default();
            let mut ends: Vec<_> = (0..INBOUND_MAX)
                .map(|i| inbound.admit(crowd(i)).unwrap().1)
                .collect();
            let newcomers: Vec<_> = (0..8)
                .map(|k| inbound.admit(IpAddr::from([20 + k, 0, 0, 1])).unwrap().1)
                .collect();
            assert_eq!(inbound.open.len(), INBOUND_MAX);
            for end in &mut ends[INBOUND_MAX - 8..] {
                assert!(end.try_recv().is_ok());
            }
            assert!(ends[0].try_recv().is_err());
            // The crowd comes back from yet another of its addresses, and from
            // a network of its own: neither takes a newcomer's place.
            assert!(inbound.admit(crowd(INBOUND_MAX)).is_none());
            assert!(inbound.admit(ip("30.0.0.1")).is_some());
            assert!(ends[INBOUND_MAX - 9].try_recv().is_ok());
            for mut end in newcomers {
                assert!(end.try_recv().is_err());
            }
        }
    }

    #[test]
    fn the_control_proof_is_hmac_sha_256() {
        // RFC 4231, test case 2.
        assert_eq!(
            hex(&mac("Jefe", &[b"what do ya ", b"want for nothing?"])),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
