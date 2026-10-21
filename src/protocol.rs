//! Bounded canonical wire types (rules §2, §4).
use crate::{
    hash::{Hash, hash},
    meter::Resources,
};
use num_bigint::BigUint;
use num_traits::{One, Zero};
use std::fmt;

/// Encoded header length: every header is exactly this long.
pub const HEADER_LEN: usize = 294;
/// Encoded genesis configuration length.
pub const GENESIS_LEN: usize = 150;
/// Data consumed by a block outside transaction budgets: the header, the
/// producer-data length prefix and the transaction count.
pub const BLOCK_BASE: u64 = HEADER_LEN as u64 + 8;
/// Data consumed by a transaction beyond its own bytes: the body length
/// prefixes and the fixed receipt fields.
pub const TRANSACTION_BASE: u64 = 81;
/// The most D a transaction may declare: the body frames the transaction and
/// its receipt each with a `u32` length, and every byte of both is D.
pub const DATA_MAX: u64 = u32::MAX as u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub trait Encode {
    fn encode(&self) -> Vec<u8>;
}
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend(
        u32::try_from(bytes.len())
            .expect("wire length")
            .to_le_bytes(),
    );
    out.extend(bytes);
}
fn put_resources(out: &mut Vec<u8>, r: Resources) {
    for n in r.0 {
        out.extend(n.to_le_bytes());
    }
}
pub fn target_bytes(target: &BigUint) -> Hash {
    let bytes = target.to_bytes_be();
    assert!(bytes.len() <= 32);
    let mut out = [0; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    out
}
pub fn producer_hash(producer: &[u8]) -> Hash {
    hash("producer", producer)
}
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], limit: usize) -> Result<Self> {
        if bytes.len() > limit {
            return Err(Error("wire byte limit"));
        }
        Ok(Self { bytes, pos: 0 })
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or(Error("length overflow"))?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error("truncated field"))?;
        self.pos = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn resources(&mut self) -> Result<Resources> {
        Ok(Resources([
            self.u64()?,
            self.u64()?,
            self.u64()?,
            self.u64()?,
        ]))
    }
    fn target(&mut self) -> Result<BigUint> {
        Ok(BigUint::from_bytes_be(self.take(32)?))
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn finish(&self) -> Result<()> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(Error("trailing bytes"))
        }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
}

/// Network parameters. Capacities have floors but no ceilings: block capacity
/// follows miner votes for as long as the chain exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis {
    pub timestamp: u64,
    pub initial_target: BigUint,
    pub pow_limit: BigUint,
    pub interval: u32,
    /// ASERT half-life in seconds (rules §9).
    pub half_life: u32,
    pub vote_epoch: u32,
    pub initial: Resources,
    pub minimum: Resources,
}
impl Genesis {
    pub fn validate(&self) -> Result<()> {
        if self.initial_target.is_zero()
            || self.initial_target > self.pow_limit
            || self.pow_limit.bits() > 256
        {
            return Err(Error("invalid target range"));
        }
        if self.interval == 0 || self.half_life == 0 || self.vote_epoch == 0 {
            return Err(Error("invalid interval"));
        }
        if self.minimum.0.contains(&0)
            || !self.minimum.fits(self.initial)
            || self.minimum.0[2] < BLOCK_BASE
        {
            return Err(Error("invalid capacity range"));
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes, GENESIS_LEN)?;
        if r.u16()? != 1 {
            return Err(Error("genesis version"));
        }
        let g = Self {
            timestamp: r.u64()?,
            initial_target: r.target()?,
            pow_limit: r.target()?,
            interval: r.u32()?,
            half_life: r.u32()?,
            vote_epoch: r.u32()?,
            initial: r.resources()?,
            minimum: r.resources()?,
        };
        r.finish()?;
        g.validate()?;
        Ok(g)
    }
    pub fn chain_id(&self) -> Hash {
        hash("chain", &self.encode())
    }
    pub fn block_id(&self) -> Hash {
        hash("genesis", &self.chain_id())
    }
    /// The production profile (rules §3), launched at
    /// `timestamp`.
    pub fn production(timestamp: u64) -> Self {
        Self {
            timestamp,
            initial_target: (BigUint::one() << 224usize) - BigUint::one(),
            pow_limit: (BigUint::one() << 228usize) - BigUint::one(),
            interval: 60,
            half_life: 43_200,
            vote_epoch: 20_160,
            initial: Resources([450_000_000, 2_048, 262_144, 131_072]),
            minimum: Resources([40_000_000, 64, 98_304, 98_304]),
        }
    }
    /// A development chain: the experimental profile with a two-week voting
    /// epoch, the genesis of `ting init` without a network and of the
    /// development seed.
    pub fn development() -> Self {
        Self {
            vote_epoch: 20_160,
            ..Self::experimental()
        }
    }
    pub fn experimental() -> Self {
        Self {
            timestamp: 1_700_000_000,
            initial_target: (BigUint::one() << 254usize) - BigUint::one(),
            pow_limit: (BigUint::one() << 256usize) - BigUint::one(),
            interval: 60,
            half_life: 172_800,
            vote_epoch: 4,
            initial: Resources([100_000_000, 1024, 1_048_576, 524_288]),
            minimum: Resources([100_000, 16, 1024, 4096]),
        }
    }
}
impl Encode for Genesis {
    fn encode(&self) -> Vec<u8> {
        let mut out = 1u16.to_le_bytes().to_vec();
        out.extend(self.timestamp.to_le_bytes());
        out.extend(target_bytes(&self.initial_target));
        out.extend(target_bytes(&self.pow_limit));
        out.extend(self.interval.to_le_bytes());
        out.extend(self.half_life.to_le_bytes());
        out.extend(self.vote_epoch.to_le_bytes());
        for r in [self.initial, self.minimum] {
            put_resources(&mut out, r);
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub chain_id: Hash,
    pub height: u64,
    pub parent: Hash,
    pub timestamp: u64,
    pub target: BigUint,
    pub transactions_root: Hash,
    pub receipts_root: Hash,
    pub state_root: Hash,
    pub used: Resources,
    pub votes: [u8; 4],
    /// `H("producer", producer_data)`; the data itself travels in the body.
    pub producer_hash: Hash,
    pub nonce: u128,
}
impl Header {
    fn read(r: &mut Reader<'_>) -> Result<Self> {
        if r.u16()? != 1 {
            return Err(Error("header version"));
        }
        let h = Self {
            chain_id: r.array()?,
            height: r.u64()?,
            parent: r.array()?,
            timestamp: r.u64()?,
            target: r.target()?,
            transactions_root: r.array()?,
            receipts_root: r.array()?,
            state_root: r.array()?,
            used: r.resources()?,
            votes: r.array()?,
            producer_hash: r.array()?,
            nonce: u128::from_le_bytes(r.array()?),
        };
        if h.votes.iter().any(|v| *v > 2) {
            return Err(Error("vote value"));
        }
        Ok(h)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes, HEADER_LEN)?;
        let h = Self::read(&mut r)?;
        r.finish()?;
        Ok(h)
    }
    pub fn id(&self) -> Hash {
        hash("block", &self.encode())
    }
}
impl Encode for Header {
    fn encode(&self) -> Vec<u8> {
        let mut out = 1u16.to_le_bytes().to_vec();
        out.extend(self.chain_id);
        out.extend(self.height.to_le_bytes());
        out.extend(self.parent);
        out.extend(self.timestamp.to_le_bytes());
        out.extend(target_bytes(&self.target));
        for root in [self.transactions_root, self.receipts_root, self.state_root] {
            out.extend(root);
        }
        put_resources(&mut out, self.used);
        out.extend(self.votes);
        out.extend(self.producer_hash);
        out.extend(self.nonce.to_le_bytes());
        debug_assert_eq!(out.len(), HEADER_LEN);
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Call { target: Hash, input: Vec<u8> },
    Deploy { code: Vec<u8>, input: Vec<u8> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub budget: Resources,
    pub payload: Payload,
}
impl Transaction {
    pub fn decode(bytes: &[u8], limit: usize) -> Result<Self> {
        let mut r = Reader::new(bytes, limit)?;
        if r.u8()? != 1 {
            return Err(Error("transaction version"));
        }
        let kind = r.u8()?;
        let budget = r.resources()?;
        let payload = match kind {
            0 => Payload::Call {
                target: r.array()?,
                input: r.bytes()?,
            },
            1 => Payload::Deploy {
                code: r.bytes()?,
                input: r.bytes()?,
            },
            _ => return Err(Error("transaction kind")),
        };
        r.finish()?;
        let t = Self { budget, payload };
        if t.budget.0[2] < bytes.len() as u64 + TRANSACTION_BASE {
            return Err(Error("data budget below envelope base"));
        }
        if t.budget.0[2] > DATA_MAX {
            return Err(Error("data budget above its frame"));
        }
        Ok(t)
    }
    pub fn id(&self, chain: Hash) -> Hash {
        let mut bytes = chain.to_vec();
        bytes.extend(self.encode());
        hash("transaction", &bytes)
    }
}
impl Encode for Transaction {
    fn encode(&self) -> Vec<u8> {
        let mut out = vec![
            1,
            if matches!(self.payload, Payload::Call { .. }) {
                0
            } else {
                1
            },
        ];
        put_resources(&mut out, self.budget);
        match &self.payload {
            Payload::Call { target, input } => {
                out.extend(target);
                put_bytes(&mut out, input);
            }
            Payload::Deploy { code, input } => {
                put_bytes(&mut out, code);
                put_bytes(&mut out, input);
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Success = 0,
    Revert = 1,
    Trap = 2,
    Compute = 3,
    Access = 4,
    Data = 5,
    Growth = 6,
    Structure = 7,
    Missing = 8,
    InvalidModule = 9,
    Exists = 10,
    ReadOnly = 11,
}
impl Status {
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => Self::Success,
            1 => Self::Revert,
            2 => Self::Trap,
            3 => Self::Compute,
            4 => Self::Access,
            5 => Self::Data,
            6 => Self::Growth,
            7 => Self::Structure,
            8 => Self::Missing,
            9 => Self::InvalidModule,
            10 => Self::Exists,
            11 => Self::ReadOnly,
            _ => return Err(Error("receipt status")),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub contract: Hash,
    pub payload: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub transaction: Hash,
    pub status: Status,
    pub used: Resources,
    pub result: Vec<u8>,
    pub events: Vec<Event>,
}
impl Receipt {
    pub fn decode(bytes: &[u8], limit: usize) -> Result<Self> {
        let mut r = Reader::new(bytes, limit)?;
        let transaction = r.array()?;
        let status = Status::from_byte(r.u8()?)?;
        let used = r.resources()?;
        let result = r.bytes()?;
        let count = r.u32()? as usize;
        if count > r.remaining() / 36 {
            return Err(Error("event count"));
        }
        let mut events = Vec::with_capacity(count);
        for _ in 0..count {
            events.push(Event {
                contract: r.array()?,
                payload: r.bytes()?,
            });
        }
        r.finish()?;
        if status != Status::Success && !events.is_empty() {
            return Err(Error("failed receipt events"));
        }
        Ok(Self {
            transaction,
            status,
            used,
            result,
            events,
        })
    }
}
impl Encode for Receipt {
    fn encode(&self) -> Vec<u8> {
        let mut out = self.transaction.to_vec();
        out.push(self.status as u8);
        put_resources(&mut out, self.used);
        put_bytes(&mut out, &self.result);
        out.extend((self.events.len() as u32).to_le_bytes());
        for e in &self.events {
            out.extend(e.contract);
            put_bytes(&mut out, &e.payload);
        }
        out
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    /// Miner-selected producer data, committed by the header's producer hash.
    pub producer: Vec<u8>,
    pub transactions: Vec<Transaction>,
    pub receipts: Vec<Receipt>,
}
impl Block {
    /// Decodes a block whose length is bounded by `limit` (the active data capacity).
    pub fn decode(bytes: &[u8], limit: usize) -> Result<Self> {
        let mut r = Reader::new(bytes, limit)?;
        let header = Header::read(&mut r)?;
        let producer = r.bytes()?;
        if producer_hash(&producer) != header.producer_hash {
            return Err(Error("producer data commitment"));
        }
        let n = r.u32()? as usize;
        if n > r.remaining() / 123 {
            return Err(Error("transaction count"));
        }
        let mut transactions = Vec::with_capacity(n);
        for _ in 0..n {
            transactions.push(Transaction::decode(&r.bytes()?, limit)?);
        }
        let mut receipts = Vec::with_capacity(n);
        for _ in 0..n {
            receipts.push(Receipt::decode(&r.bytes()?, limit)?);
        }
        r.finish()?;
        Ok(Self {
            header,
            producer,
            transactions,
            receipts,
        })
    }
}
impl Encode for Block {
    fn encode(&self) -> Vec<u8> {
        assert_eq!(self.transactions.len(), self.receipts.len());
        let mut out = self.header.encode();
        put_bytes(&mut out, &self.producer);
        out.extend((self.transactions.len() as u32).to_le_bytes());
        for t in &self.transactions {
            put_bytes(&mut out, &t.encode());
        }
        for r in &self.receipts {
            put_bytes(&mut out, &r.encode());
        }
        out
    }
}
