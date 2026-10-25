//! Outbound peer connections (wiki: node/sync). A node keeps one connection
//! open to each configured peer and to each peer discovery chose. The peer
//! announces its tip and pool over it as they change, and the node follows:
//! it asks for the peer's new headers and the blocks it lacks, pushes its own
//! new blocks when it has more work, and exchanges pooled transactions. `run`
//! keeps the connections: it opens them, opens them again after a failure,
//! bans a peer that sent invalid data, and connects one more peer while the
//! tip has not moved for a while.
use crate::{
    FRAME_MAX, Result,
    chain::{
        DEFERRED_FULL, DEFERRED_MAX, Event, HEADERS_PAGE, Handle, INVALID, INVENTORY_PAGE,
        LOCATOR_MAX, NOT_ASKED, POOL_FULL, RELAY_PAGE, SLICE_MAX, TOO_LARGE, is_invalid, is_policy,
        transfer_time,
    },
    config,
    daemon::Node,
    discovery::{self, BAN},
    hash_arg,
    transport::{self, ITSELF},
    wire::{self, Answer, Body, Part, Request},
};
use num_bigint::BigUint;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use ting_core::{
    hash::{Hash, hash, hex},
    protocol::{HEADER_LEN, Header},
};
use tokio::{
    net::tcp::OwnedWriteHalf,
    sync::mpsc,
    task::{AbortHandle, JoinSet},
    time::Instant,
};

/// Most blocks pushed to a peer in one round.
const PUSH_MAX: usize = 32;
/// Header pages asked for in one round before blocks get their turn.
const PAGES: usize = 8;
/// The time a peer has to answer a request.
const REPLY_DEADLINE: Duration = Duration::from_secs(30);
/// Block slices asked of one peer before their answers come (wiki:
/// node/sync#blocks): the peer answers in order, so one round trip carries
/// many blocks.
const PIPELINE: usize = 64;
/// The most bytes the blocks a connection has begun to fetch in more than
/// one slice may come to, each counted at its size once its first slice
/// tells it and at its data capacity until then. One such block is always
/// allowed; past it, no further one is begun until those complete.
const PARTS_MAX: u64 = 32 << 20;
/// A connection with nothing to do asks for headers this often, so that it
/// stays open and misses no tip; while the node looks for more peers, as
/// often as `LOOKING`, asking for addresses too.
const KEEPALIVE: Duration = Duration::from_secs(30);
const LOOKING: Duration = Duration::from_secs(10);
/// Rounds of one connection start at least this far apart, whatever the peer
/// announces.
const ROUND_GAP: Duration = Duration::from_millis(100);
/// Addresses are asked of each peer this often while the node has the
/// connections it wants.
const ASK_INTERVAL: u64 = 600;
/// With every connection slot taken, one unproven address is probed this often.
const PROBE_INTERVAL: u64 = 120;
/// A tip that has not moved for this long may be stale: one more peer is
/// connected until it moves.
const STALE_TIP: u64 = 600;

/// One outbound connection's transport: requests go out, one at a time or
/// several before their answers, and frames come back through a reader task,
/// so that the announcements a peer sends between answers are noticed while
/// the connection is idle.
struct Conn {
    writer: OwnedWriteHalf,
    frames: mpsc::Receiver<Result<wire::Frame>>,
    reader: tokio::task::JoinHandle<()>,
    /// The peer's latest announcement not yet acted on.
    news: Option<wire::Announcement>,
    /// Why the connection can no longer be used: a request that got no answer
    /// leaves the next answer unmatched.
    broken: Option<String>,
}
impl Drop for Conn {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
impl Conn {
    /// Connects and says hello; with `announce`, asks to be told of the
    /// peer's changes. Returns the connection and the peer's state.
    async fn open(address: SocketAddr, node: &Node, announce: bool) -> Result<(Self, wire::State)> {
        let mut stream = transport::connect(address).await?;
        let chain_id = state(node).await?.chain_id;
        let hello = wire::Hello {
            version: wire::VERSION,
            chain_id,
            agent: wire::AGENT.into(),
            features: Vec::new(),
            nonce: node.nonce,
            announce,
            listen: node.discovery.then(|| node.info.lock().unwrap().p2p.port()),
        };
        let remote = match transport::ask(&mut stream, Request::Hello(hello)).await? {
            Answer::State(state) => state,
            _ => return Err("invalid peer hello".into()),
        };
        if !transport::compatible(remote.version, &remote.chain_id, &chain_id) {
            return Err(transport::MISMATCH.into());
        }
        if remote.nonce == node.nonce {
            return Err(ITSELF.into());
        }
        let (mut read, writer) = stream.into_split();
        let (sender, frames) = mpsc::channel(8);
        let reader = tokio::spawn(async move {
            loop {
                let frame = transport::read_wire(&mut read, None, FRAME_MAX).await;
                let end = frame.is_err();
                if sender.send(frame).await.is_err() || end {
                    break;
                }
            }
        });
        let conn = Self {
            writer,
            frames,
            reader,
            news: None,
            broken: None,
        };
        Ok((conn, remote))
    }
    /// A request's answer; an error answer is the peer's, and the connection
    /// stays usable.
    async fn request(&mut self, request: Request) -> Result<Answer> {
        self.send(request).await?;
        self.next_answer().await
    }
    /// Sends a request whose answer `next_answer` reads later: answers come
    /// in the order the requests went out, so several can be on their way.
    async fn send(&mut self, request: Request) -> Result<()> {
        if let Some(error) = &self.broken {
            return Err(error.clone());
        }
        let sent = transport::write_wire(&mut self.writer, &wire::Frame::Request(request)).await;
        if let Err(error) = &sent {
            self.broken = Some(error.clone());
        }
        sent
    }
    /// The answer to the oldest request sent and not yet answered; an error
    /// answer is the peer's, and the connection stays usable.
    async fn next_answer(&mut self) -> Result<Answer> {
        if let Some(error) = &self.broken {
            return Err(error.clone());
        }
        match self.answer().await {
            Ok(answer) => answer,
            Err(error) => {
                self.broken = Some(error.clone());
                Err(error)
            }
        }
    }
    async fn answer(&mut self) -> Result<Result<Answer>> {
        let deadline = Instant::now() + REPLY_DEADLINE;
        loop {
            let frame = tokio::time::timeout_at(deadline, self.frames.recv())
                .await
                .map_err(|_| "reply deadline exceeded")?
                .ok_or("connection closed")??;
            match frame {
                wire::Frame::Announcement(news) => self.news = Some(news),
                wire::Frame::Answer(answer) => return Ok(Ok(answer)),
                wire::Frame::Error(error) => return Ok(Err(error)),
                wire::Frame::Request(_) => return Err("unexpected frame".into()),
            }
        }
    }
    /// Waits for the peer's next announcement.
    async fn announced(&mut self) -> Result<()> {
        match self.frames.recv().await.ok_or("connection closed")?? {
            wire::Frame::Announcement(news) => {
                self.news = Some(news);
                Ok(())
            }
            _ => Err("unexpected frame".into()),
        }
    }
}
/// This node's state, as its peers are told it.
async fn state(node: &Node) -> Result<wire::State> {
    match node.handle.call_wire(Request::State).await? {
        Answer::State(state) => Ok(state),
        _ => Err("unexpected answer".into()),
    }
}
/// The answer a request expects, or the peer sent something else.
fn unexpected(what: &str) -> String {
    format!("invalid peer answer to {what}")
}

/// What one connection knows of its peer, and what its next round does.
struct Link {
    node: Node,
    address: SocketAddr,
    /// The address as the chain names this peer.
    peer: String,
    source: &'static str,
    conn: Conn,
    looking: Arc<AtomicBool>,
    /// The software the peer's hello named.
    agent: String,
    /// The peer's tip, height, work and pool sizes, as it last told them.
    tip: Hash,
    height: u64,
    work: BigUint,
    pool: (u64, u64),
    /// Where the next header page starts while a peer's chain spans rounds.
    next: Option<Hash>,
    pull: bool,
    fetch: bool,
    push: bool,
    /// The last block the peer took from a push: where the next push starts,
    /// while it is on this node's branch.
    pushed: Option<Hash>,
    swap: bool,
    /// The exchange of pooled and of deferred transactions with the peer.
    relay: [Relay; 2],
    /// The tip at which transactions refused for want of room were last
    /// tried again.
    retried: Option<Hash>,
    asked: u64,
    marked: u64,
    connected: u64,
    delivered: Option<u64>,
}
/// A block on its way from a peer: the bytes so far, its most bytes, when it
/// was asked for and by when it must be complete.
struct Download {
    bytes: Vec<u8>,
    max: u64,
    /// Its size once its first slice tells it, `max` until then.
    size: u64,
    /// Whether its first slice has been asked for.
    begun: bool,
    started: Instant,
    deadline: Instant,
}
/// Why a download stopped asking for more before the chain had nothing left:
/// the peer lacks a block, or the chain refused one for now.
enum Stop {
    Missing,
    Policy,
}
/// What one connection remembers of exchanging a pool with its peer: the
/// transactions sent to it or fetched from it, whatever either side made of
/// them, and of those, the ones refused for want of room, in the order they
/// are tried again.
#[derive(Default)]
struct Relay {
    exchanged: BTreeSet<Hash>,
    refused: VecDeque<Hash>,
}
/// Whether a transaction was refused only because the side that was to take
/// it had no room, which a block may make.
fn no_room(error: &str) -> bool {
    error == POOL_FULL || error == DEFERRED_FULL
}
/// What a push reads of this node's own: a block, or a pooled or deferred
/// transaction.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Own {
    Block,
    Transaction,
}
impl Link {
    /// Takes in an announcement: a new tip is followed, a changed pool swapped.
    fn hear(&mut self, news: wire::Announcement) {
        if news.tip != self.tip {
            self.tip = news.tip;
            self.pull = true;
            self.fetch = true;
        }
        self.height = news.height;
        self.work = news.work;
        let pool = (news.pending, news.deferred);
        if pool != self.pool {
            self.pool = pool;
            self.swap = true;
        }
    }
    /// Does what is due; returns whether more is waiting at once.
    async fn round(&mut self, local: &Event) -> Result<bool> {
        let mut more = false;
        if self.pull {
            self.pull = false;
            more |= self.headers().await?;
        }
        if self.fetch {
            self.blocks().await?;
        }
        if self.push {
            self.push = false;
            more |= self.push_blocks(local).await?;
        }
        if self.swap {
            self.swap = false;
            self.swap_pool(local).await?;
        }
        let interval = if self.looking.load(Ordering::Relaxed) {
            LOOKING.as_secs()
        } else {
            ASK_INTERVAL
        };
        if self.node.discovery && crate::now() >= self.asked + interval {
            self.asked = crate::now();
            self.addresses().await;
        }
        if let Some(error) = &self.conn.broken {
            return Err(error.clone());
        }
        Ok(more)
    }
    /// The peer's headers past the best common block, page after page, each
    /// checked by the chain as it arrives; a chain below the work threshold is
    /// checked without being kept, and fetched again once it reaches it.
    /// Returns whether more pages wait.
    async fn headers(&mut self) -> Result<bool> {
        let handle = self.node.handle.clone();
        for _ in 0..PAGES {
            let locator = match self.next {
                Some(next) => vec![next],
                None => state(&self.node).await?.locator,
            };
            let page = match self.conn.request(Request::Headers { locator }).await {
                Ok(Answer::Headers(page)) => page,
                Ok(_) => return Err(unexpected("headers")),
                // The peer moved off the header its answer was to follow:
                // start again from the best common block.
                Err(e) if e == "no common ancestor" && self.next.is_some() => {
                    self.next = None;
                    continue;
                }
                Err(e) => return Err(e),
            };
            if page.is_empty() {
                self.next = None;
                return Ok(false);
            }
            let full = page.len() == HEADERS_PAGE;
            let checked = match handle
                .call_bytes(
                    "headers_from",
                    json!({"peer":self.peer}),
                    Some(page.concat()),
                )
                .await
            {
                Ok(checked) => checked,
                Err(e) if is_invalid(&e) => return Err(e),
                // Ahead of the clock, too far behind, not following, or a
                // chain fetched again that differs, as after a reorganization
                // on the peer: the rest waits for the peer's next tip.
                Err(_) => {
                    self.next = None;
                    return Ok(false);
                }
            };
            self.next = match checked["next"].as_str() {
                Some(next) if full || checked["refetch"] == true => Some(hash_arg(next)?),
                _ => None,
            };
            if self.next.is_none() {
                return Ok(false);
            }
        }
        self.pull = true;
        Ok(true)
    }
    /// Blocks along the best header's branch, as many as the chain asks of
    /// this peer; other connections fetch others at the same time. Their
    /// requests go out together and are kept topped up, so that the peer is
    /// never waiting for the next.
    async fn blocks(&mut self) -> Result<()> {
        let handle = self.node.handle.clone();
        match self.download(&handle).await {
            // This peer does not have one of them: the others are asked, and
            // this one again once it announces a new tip.
            Ok(Some(Stop::Missing)) => {
                self.fetch = false;
                handle.call("release", json!({"peer":self.peer})).await?;
                Ok(())
            }
            Ok(_) => Ok(()),
            // Answers may still be on their way, and nothing can tell them
            // from the answers to later requests.
            Err(error) => {
                self.conn.broken.get_or_insert_with(|| error.clone());
                Err(error)
            }
        }
    }
    /// The blocks the chain wants of this peer, each with its most bytes,
    /// its data capacity.
    async fn wanted(&self, handle: &Handle) -> Result<Vec<(Hash, u64)>> {
        let wanted = handle
            .call("blocks_wanted", json!({"peer":self.peer}))
            .await?;
        let ids = wanted["ids"].as_array().ok_or("invalid wanted blocks")?;
        ids.iter()
            .enumerate()
            .map(|(k, id)| {
                let id = hash_arg(id.as_str().ok_or("invalid wanted blocks")?)?;
                let max = wanted["max"][k].as_u64().ok_or("invalid wanted blocks")?;
                Ok((id, max))
            })
            .collect()
    }
    /// Fetches the blocks the chain wants of this peer and imports each as it
    /// completes: up to `PIPELINE` slices asked at once, topped up from the
    /// chain as answers come. A block is asked for in slices of at most
    /// `SLICE_MAX` bytes, each from where its bytes so far end, its next slice
    /// before any further block. Each block has its `transfer_time` from when
    /// it was asked. A peer that sends another
    /// block, or claims one larger than its capacity, sent invalid data.
    /// Returns why it stopped early, once every answer on its way has come.
    async fn download(&mut self, handle: &Handle) -> Result<Option<Stop>> {
        // The blocks under way, the slices to ask for, and the slices asked
        // for, in the order their answers come.
        let mut parts: HashMap<Hash, Download> = HashMap::new();
        let mut queue: VecDeque<(Hash, u64)> = VecDeque::new();
        let mut asked: VecDeque<(Hash, u64)> = VecDeque::new();
        let mut more = true;
        let mut stop = None;
        loop {
            if more && stop.is_none() && queue.is_empty() && asked.len() <= PIPELINE / 2 {
                let mut new = false;
                for (id, max) in self.wanted(handle).await? {
                    if parts.contains_key(&id) {
                        continue;
                    }
                    let started = Instant::now();
                    parts.insert(
                        id,
                        Download {
                            bytes: Vec::new(),
                            max,
                            size: max,
                            begun: false,
                            deadline: started + transfer_time(max),
                            started,
                        },
                    );
                    queue.push_back((id, 0));
                    new = true;
                }
                more = new;
            }
            // The bytes the blocks begun in more than one slice will hold
            // until they are whole; a block that fits one slice arrives whole.
            let sliced = |size: u64| size > SLICE_MAX as u64;
            let mut held: u64 = parts
                .values()
                .filter(|part| part.begun && sliced(part.size))
                .map(|part| part.size)
                .sum();
            while stop.is_none() && asked.len() < PIPELINE {
                let Some(&(id, offset)) = queue.front() else {
                    break;
                };
                if offset == 0
                    && let Some(part) = parts.get_mut(&id)
                {
                    if sliced(part.max) {
                        if held > 0 && held.saturating_add(part.max) > PARTS_MAX {
                            break;
                        }
                        held = held.saturating_add(part.max);
                    }
                    part.begun = true;
                }
                queue.pop_front();
                self.conn
                    .send(Request::Raw {
                        id,
                        offset,
                        length: SLICE_MAX as u32,
                    })
                    .await?;
                asked.push_back((id, offset));
            }
            let Some((id, offset)) = asked.pop_front() else {
                return Ok(stop);
            };
            let answer = self.conn.next_answer().await;
            // A block given up on has its last answers read and dropped.
            let Some(part) = parts.get_mut(&id) else {
                if let Err(error) = answer
                    && self.conn.broken.is_some()
                {
                    return Err(error);
                }
                continue;
            };
            let (at, total, bytes) = match answer {
                Ok(Answer::Slice {
                    offset,
                    total,
                    bytes,
                }) => (offset, total, bytes),
                Ok(_) => return Err(unexpected("raw")),
                Err(e) if e == "block not found" && self.conn.broken.is_none() => {
                    parts.remove(&id);
                    queue.retain(|(q, _)| *q != id);
                    stop.get_or_insert(Stop::Missing);
                    continue;
                }
                Err(e) => return Err(e),
            };
            if Instant::now() > part.deadline {
                return Err("block download deadline exceeded".into());
            }
            if at != offset || at != part.bytes.len() as u64 {
                return Err("invalid block slice".into());
            }
            if at.saturating_add(bytes.len() as u64) > total {
                return Err("invalid block slice".into());
            }
            if total > part.max {
                return Err(format!("{INVALID}block: larger than its capacity"));
            }
            if offset == 0 {
                part.size = total;
            }
            let checked = part.bytes.len() >= HEADER_LEN;
            let empty = bytes.is_empty();
            part.bytes.extend(bytes);
            if !checked && part.bytes.len() >= HEADER_LEN {
                let header = Header::decode(&part.bytes[..HEADER_LEN])
                    .map_err(|e| format!("{INVALID}block: {e}"))?;
                if header.id() != id {
                    return Err(format!("{INVALID}block: not the block asked for"));
                }
            }
            let have = part.bytes.len() as u64;
            if have < total {
                if empty {
                    return Err("empty block slice".into());
                }
                // Its next slice is asked for before any further block, from
                // where its bytes end; once its first slice tells its size, its
                // time follows from that.
                if offset == 0 {
                    part.deadline = part.started + transfer_time(total);
                }
                queue.push_front((id, have));
                continue;
            }
            let block = parts.remove(&id).map(|part| part.bytes).unwrap_or_default();
            if matches!(stop, Some(Stop::Policy)) {
                continue;
            }
            match handle
                .call_bytes("import", json!({"peer":self.peer}), Some(block))
                .await
            {
                Ok(imported) => {
                    if imported["known"] == false {
                        self.delivered = Some(crate::now());
                    }
                    // What the chain asks of this peer moves on with it.
                    more = true;
                }
                Err(e) if is_policy(&e) => {
                    stop = Some(Stop::Policy);
                }
                Err(e) => return Err(e),
            }
        }
    }
    /// This node's blocks past the best common block, to a peer with less
    /// work, so that a new tip travels at once, also to a peer that has no
    /// connection to this node. Returns whether more wait.
    async fn push_blocks(&mut self, local: &Event) -> Result<bool> {
        let ours: BigUint = local.work.parse().unwrap_or_default();
        if ours <= self.work {
            return Ok(false);
        }
        let state = match self.conn.request(Request::State).await? {
            Answer::State(state) => state,
            _ => return Err(unexpected("state")),
        };
        self.work = state.work;
        if ours <= self.work {
            return Ok(false);
        }
        // The peer's locator thins out with depth: past a deep fork it may
        // name no shared block but genesis, and the blocks the peer took
        // would be offered again each round. The last one it took comes
        // first, in place of a side branch if the locator is full.
        let mut locator = state.locator;
        if let Some(pushed) = self.pushed {
            if locator.len() >= LOCATOR_MAX {
                locator.remove(locator.len() - 2);
            }
            locator.insert(0, pushed);
        }
        let headers = match self.node.readers.wire(Request::Headers { locator }).await? {
            Answer::Headers(headers) => headers,
            _ => return Err("invalid local headers".into()),
        };
        for header in headers.iter().take(PUSH_MAX) {
            let id = hash("block", header);
            let pushed = self.push(Own::Block, id, None).await;
            match pushed {
                Ok(_) => self.pushed = Some(id),
                // The peer does not take it now (its clock is behind it, or it
                // sits on a branch too far behind the peer's tip), and its
                // descendants would wait too.
                Err(e) if is_policy(&e) => return Ok(false),
                Err(e) => return Err(e),
            }
        }
        let more = headers.len() > PUSH_MAX;
        self.push |= more;
        Ok(more)
    }
    /// A slice of this node's own block or pooled transaction.
    async fn own(&self, own: Own, id: Hash, offset: u64) -> Result<(u64, Vec<u8>)> {
        let length = SLICE_MAX as u32;
        let answer = match own {
            Own::Block => {
                self.node
                    .readers
                    .wire(Request::Raw { id, offset, length })
                    .await?
            }
            Own::Transaction => {
                self.node
                    .handle
                    .call_wire(Request::TransactionRaw { id, offset, length })
                    .await?
            }
        };
        match answer {
            Answer::Slice {
                offset: at,
                total,
                bytes,
            } if at == offset => Ok((total, bytes)),
            _ => Err("invalid local slice".into()),
        }
    }
    /// Pushes this node's own block (`import`) or transaction (`submit`, with
    /// `not_before` for a deferred one): whole when it fits a slice, else
    /// slice by slice, the peer's answer to the last, or to one it stops at,
    /// being the whole push's.
    async fn push(&mut self, own: Own, id: Hash, not_before: Option<u64>) -> Result<Answer> {
        let mut offset = 0;
        loop {
            let (total, bytes) = self.own(own, id, offset).await?;
            let end = offset + bytes.len() as u64;
            let part = Part {
                offset,
                total,
                bytes,
            };
            let request = match own {
                Own::Block => Request::Import(part),
                Own::Transaction => Request::Submit { part, not_before },
            };
            match self.conn.request(request).await? {
                // Each slice is taken whole, or the push ends.
                Answer::Received(received) if received == end && end < total => offset = end,
                Answer::Received(_) => return Err("invalid slice acknowledgement".into()),
                taken @ Answer::Taken { .. } => return Ok(taken),
                _ => return Err(unexpected("a push")),
            }
        }
    }
    /// A transaction listed by its size alone, fetched in slices within its
    /// transfer time. One larger than the next block's data capacity is not
    /// fetched.
    async fn fetch_transaction(&mut self, id: &Hash, total: u64) -> Result<Vec<u8>> {
        if total > self.node.handle.events().borrow().data {
            return Err(TOO_LARGE.into());
        }
        let deadline = Instant::now() + transfer_time(total);
        let mut bytes = Vec::new();
        while (bytes.len() as u64) < total {
            let offset = bytes.len() as u64;
            let request = Request::TransactionRaw {
                id: *id,
                offset,
                length: SLICE_MAX as u32,
            };
            match tokio::time::timeout_at(deadline, self.conn.request(request))
                .await
                .map_err(|_| "transaction download deadline exceeded")??
            {
                Answer::Slice {
                    offset: at,
                    total: listed,
                    bytes: part,
                } if at == offset
                    && listed == total
                    && !part.is_empty()
                    && offset + part.len() as u64 <= total =>
                {
                    bytes.extend(part)
                }
                Answer::Slice { .. } => return Err("invalid transaction slice".into()),
                _ => return Err(unexpected("transaction_raw")),
            }
        }
        Ok(bytes)
    }
    /// Pooled and deferred transactions both ways, by inventory: the ids each
    /// side holds are compared, the peer is sent what it lacks, deferred ones
    /// with their times, and what this node lacks is fetched by id and taken as
    /// relayed by the peer. Each travels once a connection, whatever either
    /// side made of it, so that pools which stay apart, as two full ones do,
    /// cost nothing more until they change. One refused for want of room is
    /// tried again once this node's tip has moved, since a block makes room:
    /// as many bytes of them each way as the next block holds, no more than a
    /// block can have freed.
    async fn swap_pool(&mut self, local: &Event) -> Result<()> {
        let handle = self.node.handle.clone();
        let retry = self.retried.replace(local.tip) != Some(local.tip);
        let mut budget = if retry { [local.data; 2] } else { [0; 2] };
        for deferred in [false, true] {
            let most = self.set_size(deferred);
            let (theirs, cut) = self.inventory(deferred).await?;
            let mut ours = BTreeSet::new();
            let mut after = None;
            loop {
                match handle
                    .call_wire(Request::Inventory { deferred, after })
                    .await?
                {
                    Answer::Inventory { ids, next } => {
                        ours.extend(ids);
                        after = next;
                    }
                    _ => return Err("invalid local inventory".into()),
                }
                if after.is_none() {
                    break;
                }
            }
            // A listing cut short tells nothing of the ids past its end.
            let listed = |id: &Hash| cut.is_none_or(|cut| *id <= cut);
            let Relay { exchanged, refused } = &mut self.relay[usize::from(deferred)];
            // What neither side lists any longer is forgotten, and past the
            // end of a cut listing only what goes beyond twice what a listing
            // holds, the highest first; the rest is not sent or fetched again.
            exchanged.retain(|id| ours.contains(id) || theirs.contains(id) || !listed(id));
            while exchanged.len() > 2 * (most + INVENTORY_PAGE)
                && let Some(&highest) = exchanged.last()
                && !listed(&highest)
            {
                exchanged.remove(&highest);
            }
            refused.retain(|id| exchanged.contains(id));
            let missing: Vec<Hash> = ours
                .iter()
                .filter(|id| listed(id) && !theirs.contains(*id))
                .filter(|id| exchanged.insert(**id))
                .copied()
                .collect();
            let wanted: Vec<Hash> = theirs
                .difference(&ours)
                .filter(|id| exchanged.insert(**id))
                .copied()
                .collect();
            // The refused ones the peer still lacks, or this node does; one
            // past the end of a cut listing waits until the listing reaches it.
            let (mut resend, mut refetch, mut waiting) = (Vec::new(), Vec::new(), Vec::new());
            if retry {
                for id in refused.drain(..) {
                    if !listed(&id) {
                        waiting.push(id);
                    } else if ours.contains(&id) && !theirs.contains(&id) {
                        resend.push(id);
                    } else if theirs.contains(&id) && !ours.contains(&id) {
                        refetch.push(id);
                    }
                }
            }
            let mut unbounded = u64::MAX;
            let (sent, _) = self.send_pooled(deferred, &missing, &mut unbounded).await?;
            let (fetched, _) = self.fetch_pooled(deferred, &wanted, &mut unbounded).await?;
            let (resent, unsent) = self.send_pooled(deferred, &resend, &mut budget[0]).await?;
            let (refetched, unfetched) = self
                .fetch_pooled(deferred, &refetch, &mut budget[1])
                .await?;
            // Those not yet tried again come first next time.
            self.relay[usize::from(deferred)].refused.extend(
                [waiting, unsent, unfetched, sent, fetched, resent, refetched]
                    .into_iter()
                    .flatten(),
            );
        }
        Ok(())
    }
    /// Sends the peer these of this node's pooled or deferred transactions,
    /// deferred ones with their times, while `budget` bytes last. Returns those
    /// the peer had no room for, and those the budget left out.
    async fn send_pooled(
        &mut self,
        deferred: bool,
        ids: &[Hash],
        budget: &mut u64,
    ) -> Result<(Vec<Hash>, Vec<Hash>)> {
        let handle = self.node.handle.clone();
        let (mut refused, mut left) = (Vec::new(), Vec::new());
        for chunk in ids.chunks(RELAY_PAGE) {
            let ids = chunk.to_vec();
            let request = if deferred {
                Request::Deferred { ids }
            } else {
                Request::Pending { ids }
            };
            let Answer::Items(items) = handle.call_wire(request).await? else {
                return Err("invalid local items".into());
            };
            for item in items {
                if *budget == 0 {
                    left.push(item.id);
                    continue;
                }
                // One larger than a slice, or than the answer had room for,
                // travels in slices.
                let (size, sent) = match item.body {
                    Body::Whole(bytes) => {
                        let size = bytes.len() as u64;
                        let part = Part {
                            offset: 0,
                            total: size,
                            bytes,
                        };
                        let not_before = item.not_before;
                        let request = Request::Submit { part, not_before };
                        (size, self.conn.request(request).await)
                    }
                    Body::Total(total) => (
                        total,
                        self.push(Own::Transaction, item.id, item.not_before).await,
                    ),
                };
                *budget = budget.saturating_sub(size);
                if sent.is_err_and(|e| no_room(&e)) {
                    refused.push(item.id);
                }
            }
        }
        Ok((refused, left))
    }
    /// Fetches these of the peer's pooled or deferred transactions and takes
    /// them as relayed by it, while `budget` bytes last. Returns those this
    /// node had no room for, and those the budget left out.
    async fn fetch_pooled(
        &mut self,
        deferred: bool,
        ids: &[Hash],
        budget: &mut u64,
    ) -> Result<(Vec<Hash>, Vec<Hash>)> {
        let handle = self.node.handle.clone();
        let (mut refused, mut left) = (Vec::new(), Vec::new());
        for chunk in ids.chunks(RELAY_PAGE) {
            if *budget == 0 {
                left.extend(chunk);
                continue;
            }
            let ids = chunk.to_vec();
            let request = if deferred {
                Request::Deferred { ids }
            } else {
                Request::Pending { ids }
            };
            let items = match self.conn.request(request).await? {
                Answer::Items(items) => items,
                _ => return Err(unexpected("pending")),
            };
            if items.len() > chunk.len() {
                return Err("relayed items beyond those asked for".into());
            }
            for item in items {
                if deferred != item.not_before.is_some() {
                    return Err("invalid relayed item".into());
                }
                let mut submit =
                    json!({"relayed":true,"peer":self.peer,"not_before":item.not_before});
                // One listed by its size alone is fetched in slices, and must
                // be the transaction asked for.
                let bytes = match item.body {
                    Body::Whole(bytes) => bytes,
                    Body::Total(_) if *budget == 0 => {
                        left.push(item.id);
                        continue;
                    }
                    Body::Total(total) => match self.fetch_transaction(&item.id, total).await {
                        Ok(bytes) => {
                            submit["id"] = json!(hex(&item.id));
                            bytes
                        }
                        Err(e) if e == "transaction not found" || e == TOO_LARGE => {
                            continue;
                        }
                        Err(e) => return Err(e),
                    },
                };
                *budget = budget.saturating_sub(bytes.len() as u64);
                match handle.call_bytes("submit", submit, Some(bytes)).await {
                    // A peer that sent another transaction than the one it
                    // listed is not asked again on this connection.
                    Err(e) if e == NOT_ASKED => return Err(e),
                    Err(e) if no_room(&e) => refused.push(item.id),
                    _ => {}
                }
            }
        }
        Ok((refused, left))
    }
    /// The most transactions this node's pool, or its deferred set, holds.
    fn set_size(&self, deferred: bool) -> usize {
        if deferred {
            DEFERRED_MAX
        } else {
            self.node.mempool
        }
    }
    /// The ids of the peer's pooled or deferred transactions, page by page,
    /// until there are as many as this node's set can hold: a peer that holds
    /// more may, by its own configuration, and this node could take none of
    /// the rest. Each page lists ids in order past the last page's, and names
    /// its own last as where the next starts, so the pages move on. Returns
    /// the ids and, for a listing cut short, its last id, past which what the
    /// peer holds is not known.
    async fn inventory(&mut self, deferred: bool) -> Result<(BTreeSet<Hash>, Option<Hash>)> {
        let most = self.set_size(deferred);
        let mut ids = BTreeSet::new();
        let mut after = None;
        while ids.len() < most {
            let (listed, next) = match self
                .conn
                .request(Request::Inventory { deferred, after })
                .await?
            {
                Answer::Inventory { ids, next } => (ids, next),
                _ => return Err(unexpected("inventory")),
            };
            let ordered = after.iter().chain(&listed).is_sorted_by(|a, b| a < b);
            if listed.len() > INVENTORY_PAGE
                || !ordered
                || next.is_some_and(|next| listed.last() != Some(&next))
            {
                return Err("invalid inventory page".into());
            }
            ids.extend(listed);
            after = next;
            if after.is_none() {
                break;
            }
        }
        Ok((ids, after))
    }
    /// The addresses the peer shares, learned with the limits of the book.
    async fn addresses(&mut self) {
        let Ok(Answer::Addresses(shared)) = self.conn.request(Request::Addresses).await else {
            return;
        };
        self.node
            .book
            .lock()
            .unwrap()
            .learn_shared(&shared, self.address.ip());
    }
    /// A completed round: the peer is proven, and its status current.
    fn report(&mut self) {
        let now = crate::now();
        if now >= self.marked + 60 {
            self.marked = now;
            self.node.book.lock().unwrap().succeeded(self.address, now);
        }
        let status = json!({"address":self.address,"source":self.source,"agent":self.agent,
            "connected":self.connected,"last_success":now,"height":self.height,
            "last_block":self.delivered,"error":null});
        self.node
            .peers
            .lock()
            .unwrap()
            .insert(self.peer.clone(), status);
    }
}

/// One outbound connection, for as long as it lasts: a round at once, then
/// one whenever this node or the peer changes, or the keepalive is due.
async fn follow(
    node: Node,
    address: SocketAddr,
    source: &'static str,
    looking: Arc<AtomicBool>,
) -> Result<()> {
    let (conn, hello) = Conn::open(address, &node, true).await?;
    let mut events = node.handle.events();
    let mut seen = events.borrow_and_update().clone();
    let mut link = Link {
        node: node.clone(),
        address,
        peer: address.to_string(),
        source,
        conn,
        looking,
        agent: hello.agent,
        tip: hello.tip,
        height: hello.height,
        work: hello.work,
        pool: (0, 0),
        next: None,
        pull: true,
        fetch: true,
        push: true,
        pushed: None,
        swap: true,
        relay: Default::default(),
        retried: None,
        asked: 0,
        marked: 0,
        connected: crate::now(),
        delivered: None,
    };
    let mut last = Instant::now();
    loop {
        tokio::time::sleep_until(last + ROUND_GAP).await;
        last = Instant::now();
        let more = link.round(&seen).await?;
        link.report();
        if !more && link.conn.news.is_none() && !events.has_changed().unwrap_or(true) {
            let wait = if node.discovery && link.looking.load(Ordering::Relaxed) {
                LOOKING
            } else {
                KEEPALIVE
            };
            tokio::select! {
                changed = events.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
                announced = link.conn.announced() => announced?,
                _ = tokio::time::sleep(wait) => {
                    link.pull = true;
                    link.fetch = true;
                    link.swap = true;
                }
                _ = node.stopped() => return Ok(()),
            }
        }
        if let Some(news) = link.conn.news.take() {
            link.hear(news);
        }
        let now = events.borrow_and_update().clone();
        if now.tip != seen.tip {
            link.push = true;
        }
        if (now.pending, now.deferred) != (seen.pending, seen.deferred) {
            link.swap = true;
        }
        seen = now;
    }
}

/// A peer sent invalid data: its address range is banned for a day, unless
/// it holds a configured peer or is this machine (wiki: node/sync). Its
/// connections end: inbound ones at once, outbound ones at the next check.
pub fn punish(node: &Node, address: SocketAddr) {
    let now = crate::now();
    let mut book = node.book.lock().unwrap();
    if book.ban(address.ip(), now + BAN) {
        node.inbound
            .lock()
            .unwrap()
            .end(|ip| book.is_banned(ip, now));
    }
}

/// Records how a connection ended, and gives what it was asked to the others.
async fn ended(node: &Node, address: SocketAddr, source: &str, result: Result<()>) {
    if let Err(error) = &result {
        if is_invalid(error) {
            punish(node, address);
        }
        let mut book = node.book.lock().unwrap();
        let mut peers = node.peers.lock().unwrap();
        let key = address.to_string();
        if error == ITSELF {
            book.forget(&address);
            peers.remove(&key);
        } else {
            book.failed(address);
            let mut status = peers
                .remove(&key)
                .unwrap_or_else(|| json!({"address":address,"source":source,"last_success":null}));
            status["error"] = json!(error);
            status["last_attempt"] = json!(crate::now());
            status["connected"] = Value::Null;
            peers.insert(key, status);
        }
    }
    let _ = node
        .handle
        .call("release", json!({"peer":address.to_string()}))
        .await;
}

type Ended = (SocketAddr, Result<()>);

/// Keeps the outbound connections, checking every two seconds.
pub async fn run(node: Node) {
    let looking = Arc::new(AtomicBool::new(true));
    let mut tasks: JoinSet<Ended> = JoinSet::new();
    let mut links: BTreeMap<SocketAddr, (AbortHandle, &'static str)> = BTreeMap::new();
    let mut probes: JoinSet<Ended> = JoinSet::new();
    let mut last_probe = 0;
    let events = node.handle.events();
    let (mut tip, mut tip_since) = (events.borrow().tip, crate::now());
    // Addresses that reached this node itself, configured ones included: not
    // connected again in this run.
    let mut itself: BTreeSet<SocketAddr> = BTreeSet::new();
    loop {
        // A busy or slow chain thread is waited out, never a reason to stop
        // keeping the connections.
        let config =
            node.handle.call("config", json!({})).await.and_then(|c| {
                serde_json::from_value::<config::Config>(c).map_err(|e| e.to_string())
            });
        let config = match config {
            Ok(config) => config,
            Err(_) => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(2)) => continue,
                    _ = node.stopped() => break,
                }
            }
        };
        let own = node.info.lock().unwrap().p2p;
        let configured: Vec<SocketAddr> = config
            .peers
            .iter()
            .copied()
            .filter(|a| *a != own && !itself.contains(a))
            .collect();
        let now = crate::now();
        node.book.lock().unwrap().configure(&config.peers);
        while let Some(done) = tasks.try_join_next() {
            if let Ok((address, result)) = done
                && let Some((_, source)) = links.remove(&address)
            {
                if matches!(&result, Err(e) if e == ITSELF) {
                    itself.insert(address);
                }
                ended(&node, address, source, result).await;
            }
        }
        while let Some(done) = probes.try_join_next() {
            if let Ok((address, result)) = done {
                let mut book = node.book.lock().unwrap();
                match result {
                    Ok(()) => book.succeeded(address, crate::now()),
                    Err(e) if e == ITSELF => {
                        itself.insert(address);
                        book.forget(&address);
                    }
                    Err(_) => book.failed(address),
                }
            }
        }
        // Peers whose blocks failed once connected sent invalid data too.
        if let Ok(offenders) = node.handle.call("offenders", json!({})).await {
            for peer in offenders["peers"].as_array().into_iter().flatten() {
                if let Some(address) = peer.as_str().and_then(|p| p.parse().ok()) {
                    punish(&node, address);
                    if let Some((link, source)) = links.remove(&address) {
                        link.abort();
                        let error = format!("{INVALID}block: it failed validation");
                        ended(&node, address, source, Err(error)).await;
                    }
                }
            }
        }
        // Connections that should end: to a banned address, to a configured
        // peer no longer configured, or one discovered peer too many once the
        // tip moves again.
        let current = events.borrow().tip;
        if current != tip {
            (tip, tip_since) = (current, now);
        }
        let stale = now >= tip_since + STALE_TIP;
        let want = if node.discovery {
            (config.max_peers as usize).saturating_sub(configured.len()) + usize::from(stale)
        } else {
            0
        };
        let mut ending: Vec<SocketAddr> = {
            let book = node.book.lock().unwrap();
            links
                .iter()
                .filter(|(address, (_, source))| {
                    book.is_banned(address.ip(), now)
                        || (*source == "config") != configured.contains(address)
                })
                .map(|(address, _)| *address)
                .collect()
        };
        let discovered: Vec<SocketAddr> = links
            .iter()
            .filter(|(a, (_, source))| *source == "discovered" && !ending.contains(a))
            .map(|(a, _)| *a)
            .collect();
        if discovered.len() > want {
            let peers = node.peers.lock().unwrap();
            let last_block = |a: &SocketAddr| {
                let status = peers.get(&a.to_string());
                (
                    status.and_then(|s| s["last_block"].as_u64()),
                    std::cmp::Reverse(status.and_then(|s| s["connected"].as_u64())),
                )
            };
            let mut by_block = discovered.clone();
            by_block.sort_by_key(last_block);
            ending.extend(by_block.into_iter().take(discovered.len() - want));
        }
        for address in ending {
            if let Some((link, _)) = links.remove(&address) {
                link.abort();
                node.peers.lock().unwrap().remove(&address.to_string());
                let _ = node
                    .handle
                    .call("release", json!({"peer":address.to_string()}))
                    .await;
            }
        }
        // New connections: configured peers not connected, and discovered
        // ones up to the number wanted, each when due after a failure.
        let mut start = Vec::new();
        {
            let mut book = node.book.lock().unwrap();
            for address in &configured {
                if !links.contains_key(address) && book.due(address, now) {
                    start.push((*address, "config"));
                }
            }
            if node.discovery {
                let busy: BTreeSet<SocketAddr> = links
                    .keys()
                    .copied()
                    .chain(configured.iter().copied())
                    .chain([own])
                    .chain(itself.iter().copied())
                    .collect();
                let connected = links.values().filter(|(_, s)| *s == "discovered").count();
                let missing = want.saturating_sub(connected);
                for address in book.pick(missing, &busy, now, discovery::seed()) {
                    start.push((address, "discovered"));
                }
                if missing == 0
                    && probes.is_empty()
                    && now >= last_probe + PROBE_INTERVAL
                    && let Some(address) = book.probe(&busy, now, discovery::seed())
                {
                    last_probe = now;
                    book.attempted(address, now);
                    let node = node.clone();
                    probes.spawn(async move {
                        let opened = Conn::open(address, &node, false).await;
                        (address, opened.map(|_| ()))
                    });
                }
            }
            for (address, _) in &start {
                book.attempted(*address, now);
            }
        }
        for (address, source) in start {
            let (node, looking) = (node.clone(), looking.clone());
            let link =
                tasks.spawn(async move { (address, follow(node, address, source, looking).await) });
            links.insert(address, (link, source));
        }
        looking.store(links.len() < config.max_peers as usize, Ordering::Relaxed);
        node.peers.lock().unwrap().retain(|a, _| {
            a.parse()
                .is_ok_and(|a: SocketAddr| configured.contains(&a) || links.contains_key(&a))
        });
        if let Err(e) = node.book.lock().unwrap().save() {
            eprintln!("peer address book: {e}");
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = node.stopped() => break,
        }
    }
    tasks.abort_all();
    probes.abort_all();
}
