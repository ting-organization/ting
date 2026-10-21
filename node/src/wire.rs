//! The peer protocol's messages and their binary encoding (wiki:
//! reference/peer-protocol). A frame is its length as a `u32`, then a kind byte and
//! the fields: integers little-endian and of fixed width, ids and hashes as
//! their 32 bytes, byte strings after their length, lists after their count.
//! Blocks, headers and transactions travel in their consensus encoding.
//! Decoding is strict: every count and length within its bound, every flag 0
//! or 1, every number in its one shortest form, no byte left over. A message
//! therefore has exactly one encoding.
use crate::{
    Result,
    chain::{HEADERS_PAGE, INVENTORY_PAGE, LOCATOR_MAX, RELAY_PAGE, SLICE_MAX},
    discovery::SHARE_MAX,
};
use num_bigint::BigUint;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use ting_core::{
    hash::{Hash, hex},
    protocol::HEADER_LEN,
};

/// The protocol's version, and the lowest a peer may speak: a later version
/// keeps every message of the earlier ones.
pub const VERSION: u32 = 1;
/// This software, as its hello names it to peers.
pub const AGENT: &str = concat!("ting-node/", env!("CARGO_PKG_VERSION"));
/// The most bytes of an agent's name, of one feature's and of an error.
pub const AGENT_MAX: usize = 64;
pub const FEATURE_MAX: usize = 32;
pub const ERROR_MAX: usize = 1024;
/// The most features a hello lists.
pub const FEATURES_MAX: usize = 16;
/// The most ids a node's state lists of its pooled and of its deferred
/// transactions.
pub const STATE_IDS: usize = 128;
/// The most bytes of accumulated work: far beyond any chain's.
pub const WORK_MAX: usize = 64;

/// A peer's first request: who it is, on which chain, and what it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub version: u32,
    pub chain_id: Hash,
    pub agent: String,
    pub features: Vec<String>,
    /// The node's identity for one run, so that it never keeps a connection
    /// with itself.
    pub nonce: u64,
    /// Whether it asks to be told of this node's changes.
    pub announce: bool,
    /// The port it listens on, which with the address it connects from is a
    /// candidate for others to connect to.
    pub listen: Option<u16>,
}
/// What a node tells of itself, in answer to a hello and to `state`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub version: u32,
    pub chain_id: Hash,
    pub agent: String,
    pub features: Vec<String>,
    pub nonce: u64,
    pub height: u64,
    pub tip: Hash,
    pub work: BigUint,
    /// The best header's branch back from its tip, then side branches, then
    /// genesis (wiki: node/sync).
    pub locator: Vec<Hash>,
    pub pending: Vec<Hash>,
    pub deferred: Vec<Hash>,
}
/// A block (`import`) or transaction (`submit`) pushed whole, or one slice
/// of it: `offset` into the item of `total` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub offset: u64,
    pub total: u64,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Hello(Hello),
    State,
    Headers { locator: Vec<Hash> },
    Raw { id: Hash, offset: u64, length: u32 },
    Import(Part),
    TransactionRaw { id: Hash, offset: u64, length: u32 },
    Submit { part: Part, not_before: Option<u64> },
    Inventory { deferred: bool, after: Option<Hash> },
    Pending { ids: Vec<Hash> },
    Deferred { ids: Vec<Hash> },
    Addresses,
}
/// A pooled or deferred transaction as a `pending` or `deferred` answer lists
/// it: whole, or for one larger than a slice or than the answer has room for,
/// only its size, so that it is fetched with `transaction_raw`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: Hash,
    pub not_before: Option<u64>,
    pub body: Body,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Whole(Vec<u8>),
    Total(u64),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    State(State),
    Headers(Vec<[u8; HEADER_LEN]>),
    Slice {
        offset: u64,
        total: u64,
        bytes: Vec<u8>,
    },
    /// A slice of a push before the last taken: the bytes so far.
    Received(u64),
    /// A push taken whole: the block's or transaction's id, and whether the
    /// node held it already.
    Taken {
        id: Hash,
        known: bool,
    },
    Inventory {
        ids: Vec<Hash>,
        next: Option<Hash>,
    },
    Items(Vec<Item>),
    Addresses(Vec<SocketAddr>),
}
/// What a node tells a peer that asked to be told, between answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub tip: Hash,
    pub height: u64,
    pub work: BigUint,
    pub pending: u64,
    pub deferred: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Request(Request),
    Answer(Answer),
    Error(String),
    Announcement(Announcement),
}

// ---- Encoding ------------------------------------------------------------------

struct Writer(Vec<u8>);
impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend(v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    fn hash(&mut self, v: &Hash) {
        self.0.extend(v);
    }
    fn flag(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    /// A count or length that its bound keeps within its width.
    fn count(&mut self, n: usize) {
        self.u16(u16::try_from(n).expect("a bounded count"));
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(u32::try_from(v.len()).expect("a bounded byte string"));
        self.0.extend(v);
    }
    fn text(&mut self, v: &str) {
        self.count(v.len());
        self.0.extend(v.as_bytes());
    }
    fn hashes(&mut self, v: &[Hash]) {
        self.count(v.len());
        for h in v {
            self.hash(h);
        }
    }
    fn option<T>(&mut self, v: &Option<T>, mut some: impl FnMut(&mut Self, &T)) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                some(self, v);
            }
        }
    }
    fn work(&mut self, v: &BigUint) {
        let bytes = if *v == BigUint::default() {
            Vec::new()
        } else {
            v.to_bytes_be()
        };
        self.u8(u8::try_from(bytes.len()).expect("bounded work"));
        self.0.extend(bytes);
    }
    fn address(&mut self, v: &SocketAddr) {
        match v.ip().to_canonical() {
            IpAddr::V4(ip) => {
                self.u8(4);
                self.0.extend(ip.octets());
            }
            IpAddr::V6(ip) => {
                self.u8(6);
                self.0.extend(ip.octets());
            }
        }
        self.u16(v.port());
    }
    fn names(&mut self, agent: &str, features: &[String]) {
        self.text(agent);
        self.count(features.len());
        for f in features {
            self.text(f);
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
fn invalid(what: &str) -> String {
    format!("invalid peer message: {what}")
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| invalid("length"))?;
        let part = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| invalid("truncated"))?;
        self.at = end;
        Ok(part)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn hash(&mut self) -> Result<Hash> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn flag(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("flag")),
        }
    }
    /// A count of at most `max`.
    fn count(&mut self, max: usize) -> Result<usize> {
        let n = usize::from(self.u16()?);
        if n > max {
            return Err(invalid("count"));
        }
        Ok(n)
    }
    /// A byte string of at most `max` bytes.
    fn bytes(&mut self, max: usize) -> Result<Vec<u8>> {
        let n = usize::try_from(self.u32()?).map_err(|_| invalid("length"))?;
        if n > max {
            return Err(invalid("length"));
        }
        Ok(self.take(n)?.to_vec())
    }
    /// Printable ASCII of at most `max` bytes.
    fn name(&mut self, max: usize) -> Result<String> {
        let n = self.count(max)?;
        let bytes = self.take(n)?;
        if !bytes.iter().all(|b| (0x20..0x7f).contains(b)) {
            return Err(invalid("name"));
        }
        Ok(String::from_utf8(bytes.to_vec()).expect("ASCII"))
    }
    fn hashes(&mut self, max: usize) -> Result<Vec<Hash>> {
        let n = self.count(max)?;
        (0..n).map(|_| self.hash()).collect()
    }
    fn option<T>(&mut self, some: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.flag()? {
            Ok(Some(some(self)?))
        } else {
            Ok(None)
        }
    }
    fn work(&mut self) -> Result<BigUint> {
        let n = usize::from(self.u8()?);
        if n > WORK_MAX {
            return Err(invalid("work"));
        }
        let bytes = self.take(n)?;
        if bytes.first() == Some(&0) {
            return Err(invalid("work"));
        }
        Ok(BigUint::from_bytes_be(bytes))
    }
    fn address(&mut self) -> Result<SocketAddr> {
        let ip = match self.u8()? {
            4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(self.take(4)?).unwrap())),
            6 => {
                let ip = Ipv6Addr::from(<[u8; 16]>::try_from(self.take(16)?).unwrap());
                // An IPv4 address is written as one, never mapped into IPv6.
                if ip.to_ipv4_mapped().is_some() {
                    return Err(invalid("address"));
                }
                IpAddr::V6(ip)
            }
            _ => return Err(invalid("address")),
        };
        Ok(SocketAddr::new(ip, self.u16()?))
    }
    fn names(&mut self) -> Result<(String, Vec<String>)> {
        let agent = self.name(AGENT_MAX)?;
        let n = self.count(FEATURES_MAX)?;
        let features = (0..n)
            .map(|_| self.name(FEATURE_MAX))
            .collect::<Result<_>>()?;
        Ok((agent, features))
    }
    fn finish(&self) -> Result<()> {
        if self.at != self.bytes.len() {
            return Err(invalid("trailing bytes"));
        }
        Ok(())
    }
}

const REQUEST: u8 = 1;
const ANSWER: u8 = 2;
const ERROR: u8 = 3;
const ANNOUNCEMENT: u8 = 4;

impl Request {
    /// The request's name, as the wiki lists it and its cost is counted.
    pub fn method(&self) -> &'static str {
        match self {
            Request::Hello(_) => "hello",
            Request::State => "state",
            Request::Headers { .. } => "headers",
            Request::Raw { .. } => "raw",
            Request::Import(_) => "import",
            Request::TransactionRaw { .. } => "transaction_raw",
            Request::Submit { .. } => "submit",
            Request::Inventory { .. } => "inventory",
            Request::Pending { .. } => "pending",
            Request::Deferred { .. } => "deferred",
            Request::Addresses => "addresses",
        }
    }
}
fn part(w: &mut Writer, p: &Part) {
    w.u64(p.offset);
    w.u64(p.total);
    w.bytes(&p.bytes);
}
fn read_part(r: &mut Reader) -> Result<Part> {
    Ok(Part {
        offset: r.u64()?,
        total: r.u64()?,
        bytes: r.bytes(SLICE_MAX)?,
    })
}
/// A slice request's length: 1 to `SLICE_MAX` bytes.
fn read_length(r: &mut Reader) -> Result<u32> {
    let length = r.u32()?;
    if length == 0 || length as usize > SLICE_MAX {
        return Err(invalid("slice length"));
    }
    Ok(length)
}
fn read_ids(r: &mut Reader, max: usize) -> Result<Vec<Hash>> {
    let ids = r.hashes(max)?;
    if ids.is_empty() {
        return Err(invalid("no ids"));
    }
    Ok(ids)
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        match self {
            Frame::Request(request) => {
                w.u8(REQUEST);
                match request {
                    Request::Hello(h) => {
                        w.u8(0);
                        w.u32(h.version);
                        w.hash(&h.chain_id);
                        w.names(&h.agent, &h.features);
                        w.u64(h.nonce);
                        w.flag(h.announce);
                        w.option(&h.listen, |w, port| w.u16(*port));
                    }
                    Request::State => w.u8(1),
                    Request::Headers { locator } => {
                        w.u8(2);
                        w.hashes(locator);
                    }
                    Request::Raw { id, offset, length } => {
                        w.u8(3);
                        w.hash(id);
                        w.u64(*offset);
                        w.u32(*length);
                    }
                    Request::Import(p) => {
                        w.u8(4);
                        part(&mut w, p);
                    }
                    Request::TransactionRaw { id, offset, length } => {
                        w.u8(5);
                        w.hash(id);
                        w.u64(*offset);
                        w.u32(*length);
                    }
                    Request::Submit {
                        part: p,
                        not_before,
                    } => {
                        w.u8(6);
                        part(&mut w, p);
                        w.option(not_before, |w, t| w.u64(*t));
                    }
                    Request::Inventory { deferred, after } => {
                        w.u8(7);
                        w.flag(*deferred);
                        w.option(after, |w, id| w.hash(id));
                    }
                    Request::Pending { ids } => {
                        w.u8(8);
                        w.hashes(ids);
                    }
                    Request::Deferred { ids } => {
                        w.u8(9);
                        w.hashes(ids);
                    }
                    Request::Addresses => w.u8(10),
                }
            }
            Frame::Answer(answer) => {
                w.u8(ANSWER);
                match answer {
                    Answer::State(s) => {
                        w.u8(0);
                        w.u32(s.version);
                        w.hash(&s.chain_id);
                        w.names(&s.agent, &s.features);
                        w.u64(s.nonce);
                        w.u64(s.height);
                        w.hash(&s.tip);
                        w.work(&s.work);
                        w.hashes(&s.locator);
                        w.hashes(&s.pending);
                        w.hashes(&s.deferred);
                    }
                    Answer::Headers(headers) => {
                        w.u8(1);
                        w.count(headers.len());
                        for h in headers {
                            w.0.extend(h);
                        }
                    }
                    Answer::Slice {
                        offset,
                        total,
                        bytes,
                    } => {
                        w.u8(2);
                        w.u64(*offset);
                        w.u64(*total);
                        w.bytes(bytes);
                    }
                    Answer::Received(n) => {
                        w.u8(3);
                        w.u64(*n);
                    }
                    Answer::Taken { id, known } => {
                        w.u8(4);
                        w.hash(id);
                        w.flag(*known);
                    }
                    Answer::Inventory { ids, next } => {
                        w.u8(5);
                        w.hashes(ids);
                        w.option(next, |w, id| w.hash(id));
                    }
                    Answer::Items(items) => {
                        w.u8(6);
                        w.count(items.len());
                        for item in items {
                            w.hash(&item.id);
                            w.option(&item.not_before, |w, t| w.u64(*t));
                            match &item.body {
                                Body::Whole(bytes) => {
                                    w.u8(0);
                                    w.bytes(bytes);
                                }
                                Body::Total(total) => {
                                    w.u8(1);
                                    w.u64(*total);
                                }
                            }
                        }
                    }
                    Answer::Addresses(addresses) => {
                        w.u8(7);
                        w.count(addresses.len());
                        for a in addresses {
                            w.address(a);
                        }
                    }
                }
            }
            Frame::Error(error) => {
                w.u8(ERROR);
                // A long error is cut at a character boundary.
                let mut end = error.len().min(ERROR_MAX);
                while !error.is_char_boundary(end) {
                    end -= 1;
                }
                w.text(&error[..end]);
            }
            Frame::Announcement(a) => {
                w.u8(ANNOUNCEMENT);
                w.hash(&a.tip);
                w.u64(a.height);
                w.work(&a.work);
                w.u64(a.pending);
                w.u64(a.deferred);
            }
        }
        w.0
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader { bytes, at: 0 };
        let frame = match r.u8()? {
            REQUEST => Frame::Request(match r.u8()? {
                0 => {
                    let version = r.u32()?;
                    let chain_id = r.hash()?;
                    let (agent, features) = r.names()?;
                    Request::Hello(Hello {
                        version,
                        chain_id,
                        agent,
                        features,
                        nonce: r.u64()?,
                        announce: r.flag()?,
                        listen: r.option(|r| r.u16())?,
                    })
                }
                1 => Request::State,
                2 => Request::Headers {
                    locator: read_ids(&mut r, LOCATOR_MAX)?,
                },
                3 => Request::Raw {
                    id: r.hash()?,
                    offset: r.u64()?,
                    length: read_length(&mut r)?,
                },
                4 => Request::Import(read_part(&mut r)?),
                5 => Request::TransactionRaw {
                    id: r.hash()?,
                    offset: r.u64()?,
                    length: read_length(&mut r)?,
                },
                6 => Request::Submit {
                    part: read_part(&mut r)?,
                    not_before: r.option(|r| r.u64())?,
                },
                7 => Request::Inventory {
                    deferred: r.flag()?,
                    after: r.option(|r| r.hash())?,
                },
                8 => Request::Pending {
                    ids: read_ids(&mut r, RELAY_PAGE)?,
                },
                9 => Request::Deferred {
                    ids: read_ids(&mut r, RELAY_PAGE)?,
                },
                10 => Request::Addresses,
                _ => return Err(UNKNOWN_REQUEST.into()),
            }),
            ANSWER => Frame::Answer(match r.u8()? {
                0 => {
                    let version = r.u32()?;
                    let chain_id = r.hash()?;
                    let (agent, features) = r.names()?;
                    Answer::State(State {
                        version,
                        chain_id,
                        agent,
                        features,
                        nonce: r.u64()?,
                        height: r.u64()?,
                        tip: r.hash()?,
                        work: r.work()?,
                        locator: r.hashes(LOCATOR_MAX)?,
                        pending: r.hashes(STATE_IDS)?,
                        deferred: r.hashes(STATE_IDS)?,
                    })
                }
                1 => {
                    let n = r.count(HEADERS_PAGE)?;
                    Answer::Headers(
                        (0..n)
                            .map(|_| Ok(r.take(HEADER_LEN)?.try_into().unwrap()))
                            .collect::<Result<_>>()?,
                    )
                }
                2 => Answer::Slice {
                    offset: r.u64()?,
                    total: r.u64()?,
                    bytes: r.bytes(SLICE_MAX)?,
                },
                3 => Answer::Received(r.u64()?),
                4 => Answer::Taken {
                    id: r.hash()?,
                    known: r.flag()?,
                },
                5 => Answer::Inventory {
                    ids: r.hashes(INVENTORY_PAGE)?,
                    next: r.option(|r| r.hash())?,
                },
                6 => {
                    let n = r.count(RELAY_PAGE)?;
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        let id = r.hash()?;
                        let not_before = r.option(|r| r.u64())?;
                        let body = match r.u8()? {
                            0 => Body::Whole(r.bytes(SLICE_MAX)?),
                            1 => Body::Total(r.u64()?),
                            _ => return Err(invalid("item body")),
                        };
                        items.push(Item {
                            id,
                            not_before,
                            body,
                        });
                    }
                    Answer::Items(items)
                }
                7 => {
                    let n = r.count(SHARE_MAX)?;
                    Answer::Addresses((0..n).map(|_| r.address()).collect::<Result<_>>()?)
                }
                _ => return Err(invalid("answer")),
            }),
            ERROR => {
                let n = r.count(ERROR_MAX)?;
                let text = std::str::from_utf8(r.take(n)?).map_err(|_| invalid("error text"))?;
                Frame::Error(text.to_string())
            }
            ANNOUNCEMENT => Frame::Announcement(Announcement {
                tip: r.hash()?,
                height: r.u64()?,
                work: r.work()?,
                pending: r.u64()?,
                deferred: r.u64()?,
            }),
            _ => return Err(invalid("frame kind")),
        };
        r.finish()?;
        Ok(frame)
    }
    /// The frame as JSON, with bytes in hex: for reading peer traffic while
    /// debugging, never sent.
    pub fn describe(&self) -> Value {
        let ids = |v: &[Hash]| v.iter().map(|h| hex(h)).collect::<Vec<_>>();
        match self {
            Frame::Request(request) => {
                let params = match request {
                    Request::Hello(h) => json!({"version":h.version,"chain_id":hex(&h.chain_id),
                        "agent":h.agent,"features":h.features,"nonce":format!("{:016x}",h.nonce),
                        "announce":h.announce,"listen":h.listen}),
                    Request::State | Request::Addresses => json!({}),
                    Request::Headers { locator } => json!({"locator":ids(locator)}),
                    Request::Raw { id, offset, length }
                    | Request::TransactionRaw { id, offset, length } => {
                        json!({"id":hex(id),"offset":offset,"length":length})
                    }
                    Request::Import(p) => {
                        json!({"offset":p.offset,"total":p.total,"bytes":p.bytes.len()})
                    }
                    Request::Submit {
                        part: p,
                        not_before,
                    } => {
                        json!({"offset":p.offset,"total":p.total,"bytes":p.bytes.len(),"not_before":not_before})
                    }
                    Request::Inventory { deferred, after } => {
                        json!({"kind":if *deferred {"deferred"} else {"pending"},"after":after.map(|a| hex(&a))})
                    }
                    Request::Pending { ids: list } | Request::Deferred { ids: list } => {
                        json!({"ids":ids(list)})
                    }
                };
                json!({"request":request.method(),"params":params})
            }
            Frame::Answer(answer) => json!({"answer":match answer {
                Answer::State(s) => json!({"version":s.version,"chain_id":hex(&s.chain_id),
                    "agent":s.agent,"features":s.features,"nonce":format!("{:016x}",s.nonce),
                    "height":s.height,"tip":hex(&s.tip),"work":s.work.to_string(),
                    "locator":ids(&s.locator),"pending":ids(&s.pending),"deferred":ids(&s.deferred)}),
                Answer::Headers(h) => json!({"headers":h.iter().map(|h| hex(h)).collect::<Vec<_>>()}),
                Answer::Slice { offset, total, bytes } => {
                    json!({"offset":offset,"total":total,"bytes":bytes.len()})
                }
                Answer::Received(n) => json!({"received":n}),
                Answer::Taken { id, known } => json!({"id":hex(id),"known":known}),
                Answer::Inventory { ids: list, next } => {
                    json!({"ids":ids(list),"next":next.map(|n| hex(&n))})
                }
                Answer::Items(items) => json!(items.iter().map(|i| json!({"id":hex(&i.id),
                    "not_before":i.not_before,"bytes":match &i.body {
                        Body::Whole(b) => b.len() as u64, Body::Total(t) => *t },
                    "whole":matches!(i.body, Body::Whole(_))})).collect::<Vec<_>>()),
                Answer::Addresses(a) => json!(a.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
            }}),
            Frame::Error(error) => json!({"error":error}),
            Frame::Announcement(a) => json!({"announce":{"tip":hex(&a.tip),"height":a.height,
                "work":a.work.to_string(),"pending":a.pending,"deferred":a.deferred}}),
        }
    }
}
/// What a node answers a request it does not know; the connection then ends.
pub const UNKNOWN_REQUEST: &str = "unknown peer method";

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            version: VERSION,
            chain_id: [3; 32],
            agent: AGENT.into(),
            features: vec!["later".into()],
            nonce: 0x0123_4567_89ab_cdef,
            announce: true,
            listen: Some(18442),
        }
    }
    fn state() -> State {
        State {
            version: VERSION,
            chain_id: [3; 32],
            agent: AGENT.into(),
            features: vec![],
            nonce: 7,
            height: 9,
            tip: [4; 32],
            work: BigUint::from(1u32) << 200u32,
            locator: vec![[5; 32]; LOCATOR_MAX],
            pending: vec![[6; 32]; STATE_IDS],
            deferred: vec![],
        }
    }
    fn part() -> Part {
        Part {
            offset: 5,
            total: 9,
            bytes: vec![1, 2, 3, 4],
        }
    }
    fn every_frame() -> Vec<Frame> {
        let requests = [
            Request::Hello(hello()),
            Request::Hello(Hello {
                listen: None,
                announce: false,
                ..hello()
            }),
            Request::State,
            Request::Headers {
                locator: vec![[1; 32]; 3],
            },
            Request::Raw {
                id: [2; 32],
                offset: 1 << 40,
                length: SLICE_MAX as u32,
            },
            Request::Import(part()),
            Request::TransactionRaw {
                id: [2; 32],
                offset: 0,
                length: 1,
            },
            Request::Submit {
                part: part(),
                not_before: Some(1_800_000_000),
            },
            Request::Submit {
                part: part(),
                not_before: None,
            },
            Request::Inventory {
                deferred: true,
                after: Some([8; 32]),
            },
            Request::Inventory {
                deferred: false,
                after: None,
            },
            Request::Pending {
                ids: vec![[9; 32]; RELAY_PAGE],
            },
            Request::Deferred { ids: vec![[9; 32]] },
            Request::Addresses,
        ];
        let answers = [
            Answer::State(state()),
            Answer::State(State {
                work: BigUint::default(),
                ..state()
            }),
            Answer::Headers(vec![[7; HEADER_LEN]; 3]),
            Answer::Headers(vec![]),
            Answer::Slice {
                offset: 3,
                total: 10,
                bytes: vec![0; 7],
            },
            Answer::Received(4),
            Answer::Taken {
                id: [1; 32],
                known: true,
            },
            Answer::Inventory {
                ids: vec![[2; 32]; 2],
                next: Some([2; 32]),
            },
            Answer::Items(vec![
                Item {
                    id: [1; 32],
                    not_before: None,
                    body: Body::Whole(vec![5; 20]),
                },
                Item {
                    id: [2; 32],
                    not_before: Some(3),
                    body: Body::Total(1 << 30),
                },
            ]),
            Answer::Addresses(vec![
                "203.0.113.5:18442".parse().unwrap(),
                "[2001:db8::1]:18442".parse().unwrap(),
            ]),
        ];
        requests
            .into_iter()
            .map(Frame::Request)
            .chain(answers.into_iter().map(Frame::Answer))
            .chain([
                Frame::Error("block not found".into()),
                Frame::Announcement(Announcement {
                    tip: [1; 32],
                    height: 2,
                    work: BigUint::from(3u32),
                    pending: 4,
                    deferred: 5,
                }),
            ])
            .collect()
    }

    #[test]
    fn every_message_decodes_to_itself_and_only_its_own_bytes_do() {
        for frame in every_frame() {
            let bytes = frame.encode();
            assert_eq!(Frame::decode(&bytes).unwrap(), frame, "{frame:?}");
            // Not one byte more, nor one less.
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(Frame::decode(&longer).is_err(), "{frame:?}");
            assert!(
                Frame::decode(&bytes[..bytes.len() - 1]).is_err(),
                "{frame:?}"
            );
            assert!(frame.describe().is_object());
        }
    }

    #[test]
    fn blocks_headers_and_transactions_travel_in_their_own_bytes() {
        // A full page of headers is their bytes, a count and two kind bytes.
        let page = Frame::Answer(Answer::Headers(vec![[0; HEADER_LEN]; HEADERS_PAGE])).encode();
        assert_eq!(page.len(), 2 + 2 + HEADERS_PAGE * HEADER_LEN);
        // A slice is its bytes and 22 more.
        let slice = Frame::Answer(Answer::Slice {
            offset: 0,
            total: SLICE_MAX as u64,
            bytes: vec![0; SLICE_MAX],
        })
        .encode();
        assert_eq!(slice.len(), SLICE_MAX + 22);
        assert!(slice.len() <= crate::FRAME_MAX);
    }

    #[test]
    fn every_bound_and_form_is_enforced() {
        let refused = |bytes: Vec<u8>, what: &str| {
            assert!(Frame::decode(&bytes).is_err(), "{what}");
        };
        let request = |tag: u8, fields: &[u8]| [&[REQUEST, tag][..], fields].concat();
        let count = |n: u16| n.to_le_bytes().to_vec();
        let ids = |n: u16| [count(n), vec![1; 32 * n as usize]].concat();
        refused(vec![], "no kind");
        refused(vec![9], "unknown kind");
        refused(request(11, &[]), "unknown request");
        refused(vec![ANSWER, 8], "unknown answer");
        // Counts: locators, relayed ids, pages, items and addresses.
        refused(request(2, &ids(0)), "empty locator");
        refused(request(2, &ids(LOCATOR_MAX as u16 + 1)), "long locator");
        refused(request(8, &ids(0)), "no ids");
        refused(request(9, &ids(RELAY_PAGE as u16 + 1)), "too many ids");
        refused(
            [vec![ANSWER, 1], count(HEADERS_PAGE as u16 + 1)].concat(),
            "long header page",
        );
        refused(
            [vec![ANSWER, 5], ids(INVENTORY_PAGE as u16 + 1), vec![0]].concat(),
            "long inventory",
        );
        refused(
            [vec![ANSWER, 6], count(RELAY_PAGE as u16 + 1)].concat(),
            "too many items",
        );
        refused(
            [vec![ANSWER, 7], count(SHARE_MAX as u16 + 1)].concat(),
            "too many addresses",
        );
        // Slices: a length of nothing or of more than a slice, and bytes
        // beyond a slice.
        let raw = |length: u32| {
            request(
                3,
                &[
                    [0; 32].as_slice(),
                    &0u64.to_le_bytes(),
                    &length.to_le_bytes(),
                ]
                .concat(),
            )
        };
        assert!(Frame::decode(&raw(1)).is_ok());
        refused(raw(0), "empty slice length");
        refused(raw(SLICE_MAX as u32 + 1), "long slice length");
        let mut long = Frame::Request(Request::Import(part())).encode();
        long.truncate(2 + 16);
        long.extend((SLICE_MAX as u32 + 1).to_le_bytes());
        refused(long, "bytes beyond a slice");
        // Flags are 0 or 1, options too.
        let mut flag = Frame::Request(Request::Inventory {
            deferred: true,
            after: None,
        })
        .encode();
        flag[2] = 2;
        refused(flag.clone(), "flag");
        flag[2] = 1;
        flag[3] = 2;
        refused(flag, "option");
        // Work in its shortest form; an IPv4 address never mapped into IPv6.
        let mut work = Frame::Announcement(Announcement {
            tip: [0; 32],
            height: 0,
            work: BigUint::from(1u32),
            pending: 0,
            deferred: 0,
        })
        .encode();
        let at = 1 + 32 + 8;
        work.splice(at..at + 2, [2, 0, 1]);
        refused(work, "work with a leading zero");
        let mapped: SocketAddr = "[::ffff:203.0.113.5]:1".parse().unwrap();
        let mut address = Frame::Answer(Answer::Addresses(vec![mapped])).encode();
        assert_eq!(address[4], 4, "written as IPv4");
        address.splice(
            4..9,
            [[6].as_slice(), &[0; 10], &[0xff, 0xff], &[203, 0, 113, 5]].concat(),
        );
        refused(address, "mapped address");
        // Names are printable ASCII, and an error is UTF-8.
        let mut name = Frame::Request(Request::Hello(hello())).encode();
        name[2 + 4 + 32 + 2] = 10;
        refused(name, "control character in the agent");
        refused(
            [vec![ERROR], count(2), vec![0xff, 0xfe]].concat(),
            "error text",
        );
    }

    #[test]
    fn the_peer_protocol_pages_example_frames_are_the_encoders() {
        // The page's examples, hex with spaces and line breaks, each a whole
        // frame with its length first.
        let page = include_str!("../wiki/reference/peer-protocol.md");
        let examples = page
            .split("## Example frames")
            .nth(1)
            .and_then(|s| s.split("\n## ").next())
            .expect("the page has its example frames");
        let blocks: Vec<String> = examples
            .split("```text")
            .skip(1)
            .map(|block| {
                block
                    .split("```")
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .collect::<String>()
            })
            .collect();
        let hash = |text: &str| -> Hash { crate::hash_arg(text).unwrap() };
        let framed = |frame: Frame| {
            let bytes = frame.encode();
            hex(&[(bytes.len() as u32).to_le_bytes().as_slice(), &bytes].concat())
        };
        let genesis = hash("24b76a52f3825b921373b6c6a41ced2a74de7dbdb34015669c10594996887459");
        let expected = [
            framed(Frame::Request(Request::Hello(Hello {
                version: VERSION,
                chain_id: hash("15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107"),
                agent: "ting-node/0.1.0".into(),
                features: vec![],
                nonce: 0x0123_4567_89ab_cdef,
                announce: true,
                listen: Some(18442),
            }))),
            framed(Frame::Request(Request::Headers {
                locator: vec![genesis],
            })),
            framed(Frame::Request(Request::Raw {
                id: hash("000000004eba8c8bd3488debecab68017d3720b9f5a0da24ab80ffce4b57d1ea"),
                offset: 0,
                length: SLICE_MAX as u32,
            })),
        ];
        assert_eq!(blocks, expected);
    }

    #[test]
    fn a_long_error_is_cut_at_a_character_boundary() {
        let long = "\u{e9}".repeat(ERROR_MAX);
        let Frame::Error(cut) = Frame::decode(&Frame::Error(long).encode()).unwrap() else {
            panic!("not an error");
        };
        assert!(cut.len() <= ERROR_MAX && cut.len() >= ERROR_MAX - 1);
        assert!(cut.chars().all(|c| c == '\u{e9}'));
    }
}
