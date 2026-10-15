//! Answers to reads of stored chain data: blocks, events, transaction
//! occurrences, contracts and storage for REST, and header pages and block
//! slices for peers. Each comes from one consistent view of the database, so
//! the chain thread and the reader threads that serve them beside it (wiki:
//! node) give the same answers.
use crate::{
    Result,
    chain::{HEADERS_PAGE, LOCATOR_MAX, SLICE_MAX, str_param, u64_param},
    hash_arg,
    store::{Kept, Reader, View},
    wire,
};
use serde_json::{Value, json};
use std::sync::Arc;
use ting_core::{
    hash::{Hash, hex},
    protocol::{Encode, HEADER_LEN, Receipt, Transaction},
    state::{End, manifest_key, prove, record_key},
};
use tokio::sync::Semaphore;

/// The processor cores the node may use.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}
/// Reads of stored data on threads beside the chain thread: as many at once
/// as the machine has processor cores, the rest waiting their turn. REST and
/// peers each have their own.
#[derive(Clone)]
pub struct Readers {
    reader: Reader,
    running: Arc<Semaphore>,
}
impl Readers {
    pub fn new(reader: Reader) -> Self {
        Self {
            reader,
            running: Arc::new(Semaphore::new(cores())),
        }
    }
    /// The answer to a read, from a view of the last commit.
    pub async fn answer(&self, method: &'static str, params: Value) -> Result<Value> {
        self.run(move |view| answer(view, method, &params)).await
    }
    /// A peer's request for a page of headers or a block slice.
    pub async fn wire(&self, request: wire::Request) -> Result<wire::Answer> {
        self.run(move |view| answer_wire(view, &request)).await
    }
    async fn run<T: Send + 'static>(
        &self,
        read: impl FnOnce(&View) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let _running = self
            .running
            .acquire()
            .await
            .map_err(|_| "node worker stopped")?;
        let reader = self.reader.clone();
        tokio::task::spawn_blocking(move || read(&reader.view()?))
            .await
            .map_err(|_| "node worker stopped")?
    }
}
/// A peer's request answered from the database (wiki: node/sync#messages): a
/// page of headers, or a slice of a block.
pub fn answer_wire(view: &View, request: &wire::Request) -> Result<wire::Answer> {
    match request {
        wire::Request::Headers { locator } => {
            Ok(wire::Answer::Headers(header_page(view, locator)?))
        }
        wire::Request::Raw { id, offset, length } => {
            let bytes = served(view, id)?;
            let start = usize::try_from(*offset).unwrap_or(usize::MAX);
            if start > bytes.len() {
                return Err("slice offset out of range".into());
            }
            let end = start.saturating_add(*length as usize).min(bytes.len());
            Ok(wire::Answer::Slice {
                offset: *offset,
                total: bytes.len() as u64,
                bytes: bytes[start..end].to_vec(),
            })
        }
        _ => Err(wire::UNKNOWN_REQUEST.into()),
    }
}
/// The answer to a read of stored data: `block`, `blocks`, `events`, `raw`,
/// `transaction` (its stored part, see [`transaction`]), `contract`,
/// `storage` or `headers`.
pub fn answer(view: &View, method: &str, p: &Value) -> Result<Value> {
    match method {
        "block" => block_json(view, block_id(view, str_param(p, "id")?)?),
        "blocks" | "events" => page(view, method, p),
        "raw" => raw(view, p),
        "headers" => headers(view, p),
        "transaction" => transaction(view, p),
        "contract" => contract(view, p),
        "storage" => storage(view, p),
        "storage_batch" => storage_batch(view, p),
        _ => Err("unknown method".into()),
    }
    .and_then(fits)
}
/// A response small enough for one answer.
pub fn fits(value: Value) -> Result<Value> {
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > crate::FRAME_MAX - 1024 {
        return Err("JSON response size limit; fetch a smaller page or canonical raw block".into());
    }
    Ok(value)
}
/// A height in decimal digits, or a 64-character id of a stored block.
fn block_id(view: &View, text: &str) -> Result<Hash> {
    if text.len() != 64 && !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        let height = text.parse::<u64>().map_err(|_| "block not found")?;
        if height == 0 {
            return Err("genesis has no ordinary block body".into());
        }
        return view
            .canonical(height)?
            .ok_or_else(|| "block not found".into());
    }
    let id = hash_arg(text)?;
    if view.location(&id)?.is_none() {
        return Err("block not found".into());
    }
    Ok(id)
}
fn block_json(view: &View, id: Hash) -> Result<Value> {
    let b = view.block(&id)?;
    let stored = view.stored(&id)?.ok_or("block not found")?;
    let canonical = view.canonical(b.header.height)? == Some(id);
    Ok(
        json!({"id":hex(&id),"height":b.header.height,"parent":hex(&b.header.parent),
        "timestamp":b.header.timestamp,"target":hex(&ting_core::protocol::target_bytes(&b.header.target)),
        "work":stored.work.to_string(),"canonical":canonical,"state_root":hex(&b.header.state_root),
        "transactions_root":hex(&b.header.transactions_root),"receipts_root":hex(&b.header.receipts_root),
        "used":b.header.used.0,"votes":b.header.votes,"producer_hex":hex(&b.producer),"producer_hash":hex(&b.header.producer_hash),
        "nonce":b.header.nonce.to_string(),"bytes":b.encode().len(),
        "transactions":b.transactions.iter().map(|tx| hex(&tx.id(view.chain()))).collect::<Vec<_>>(),
        "receipts":b.receipts.iter().map(receipt_json).collect::<Vec<_>>()}),
    )
}
/// A page of canonical blocks, or of their events, from height `from`.
fn page(view: &View, method: &str, p: &Value) -> Result<Value> {
    let from = u64_param(p, "from")?.unwrap_or(1).max(1);
    let limit = u64_param(p, "limit")?.unwrap_or(20);
    if limit > 100 {
        return Err("pagination limit is 100".into());
    }
    let mut items = Vec::new();
    let mut size = 0;
    for height in (from..).take(limit as usize) {
        let Some(id) = view.canonical(height)? else {
            break;
        };
        let item = if method == "blocks" {
            block_json(view, id)?
        } else {
            let b = view.block(&id)?;
            json!({"block":hex(&id),"height":b.header.height,"timestamp":b.header.timestamp,"events":b.receipts.iter().enumerate().flat_map(|(i,r)|
                r.events.iter().map(move |e| json!({"transaction_index":i,"transaction":hex(&r.transaction),"contract":hex(&e.contract),"payload_hex":hex(&e.payload)}))).collect::<Vec<_>>()})
        };
        if !push_bounded(&mut items, item, &mut size)? {
            break;
        }
    }
    Ok(
        json!({"items":items,"from":from,"next":from.saturating_add(items.len() as u64),"tip":hex(&view.tip()?)}),
    )
}
/// A slice of a block this node has executed. Others it holds, stored but not
/// executed or failed, are not served: a peer is banned for sending a block
/// that fails, so a node passes on only blocks it has checked.
/// One the operator marked invalid, or one above it, is not served either.
fn raw(view: &View, p: &Value) -> Result<Value> {
    let id = block_id(view, str_param(p, "id")?)?;
    slice(&served(view, &id)?, p, "block")
}
/// A block this node passes on: executed and found valid, and neither it nor
/// a block below it marked invalid by the operator.
fn served(view: &View, id: &Hash) -> Result<Arc<Vec<u8>>> {
    if view.stored(id)?.is_none_or(|s| s.kept != Kept::Valid) || !view.unmarked(id)? {
        return Err("block not found".into());
    }
    view.served_bytes(id)
}
fn headers(view: &View, p: &Value) -> Result<Value> {
    let locator = p["locator"]
        .as_array()
        .ok_or("expected locator")?
        .iter()
        .map(|item| hash_arg(item.as_str().ok_or("expected locator hash")?))
        .collect::<Result<Vec<_>>>()?;
    let page = header_page(view, &locator)?;
    Ok(json!({"headers":page.iter().map(|h| hex(h)).collect::<Vec<_>>()}))
}
/// A page of canonical headers after the highest locator entry on the
/// canonical branch, wherever it sits in the list: side-branch heads come
/// after the canonical ones, and a peer on another branch must still move
/// past one page per request.
fn header_page(view: &View, locator: &[Hash]) -> Result<Vec<[u8; HEADER_LEN]>> {
    if locator.is_empty() || locator.len() > LOCATOR_MAX {
        return Err("locator limit".into());
    }
    let mut start = None;
    for id in locator {
        let height = if *id == view.genesis() {
            0
        } else {
            match view.stored(id)? {
                Some(s) if view.canonical(s.header.height)? == Some(*id) => s.header.height,
                _ => continue,
            }
        };
        start = start.max(Some(height));
    }
    let start: u64 = start.ok_or("no common ancestor")?;
    view.canonical_header_bytes(start.saturating_add(1), HEADERS_PAGE)
}
/// The stored part of a transaction lookup: a page of its canonical
/// occurrences, and in `exists` whether it has any at all. [`finish`] adds
/// whether it is pending.
fn transaction(view: &View, p: &Value) -> Result<Value> {
    let id = hash_arg(str_param(p, "id")?)?;
    let from = u64_param(p, "from")?.unwrap_or(0);
    let limit = u64_param(p, "limit")?.unwrap_or(20);
    if limit > 100 {
        return Err("pagination limit is 100".into());
    }
    let mut occurrences = Vec::new();
    let mut size = 0;
    let mut more = false;
    let indexed = view.occurrences(id, from, limit as usize + 1)?;
    let exists = !indexed.is_empty() || !view.occurrences(id, 0, 1)?.is_empty();
    for (block, i) in indexed {
        if occurrences.len() >= limit as usize {
            more = true;
            break;
        }
        let b = view.block(&block)?;
        let tx = b.transactions.get(i).ok_or("invalid transaction index")?;
        if tx.id(view.chain()) != id {
            return Err("transaction index identity mismatch".into());
        }
        let item = json!({"block":hex(&block),"height":b.header.height,"index":i,"receipt":receipt_json(&b.receipts[i]),"hex":inline_hex(tx),"transaction":transaction_summary(tx)});
        if !push_bounded(&mut occurrences, item, &mut size)? {
            more = true;
            break;
        }
    }
    Ok(
        json!({"id":hex(&id),"exists":exists,"occurrences":occurrences,"from":from,
        "next":from.saturating_add(occurrences.len() as u64),"has_more":more,"tip":hex(&view.tip()?)}),
    )
}
/// A transaction lookup: its stored part with what the chain thread knows of
/// it pending, `pending`, `deferred` and `pending_transaction`. A transaction
/// neither stored nor pending is not found.
pub fn finish(mut stored: Value, pending: Value) -> Result<Value> {
    let exists = stored["exists"].as_bool().unwrap_or(false);
    if !exists && pending["pending"] != json!(true) && pending["deferred"].is_null() {
        return Err("transaction not found".into());
    }
    let object = stored.as_object_mut().ok_or("expected a stored lookup")?;
    object.remove("exists");
    for field in ["pending", "deferred", "pending_transaction"] {
        object.insert(field.into(), pending[field].clone());
    }
    Ok(stored)
}
fn contract(view: &View, p: &Value) -> Result<Value> {
    let id = hash_arg(str_param(p, "id")?)?;
    let m = view
        .record(&manifest_key(id))?
        .ok_or("contract not found")?;
    if m.len() != 44 {
        return Err("corrupt contract manifest".into());
    }
    Ok(
        json!({"id":hex(&id),"code_hash":hex(&m[..32]),"code_bytes":u32::from_le_bytes(m[32..36].try_into().unwrap()),"code_price":u64::from_le_bytes(m[36..44].try_into().unwrap()),"state_root":hex(&view.root()?)}),
    )
}
/// A storage record, with `proof` its Merkle proof against the state root.
fn storage(view: &View, p: &Value) -> Result<Value> {
    let id = hash_arg(str_param(p, "id")?)?;
    let key = hash_arg(str_param(p, "key")?)?;
    let mut answer = record_json(view, id, key, p["proof"] == true)?;
    answer["state_root"] = json!(hex(&view.root()?));
    answer["tip"] = json!(hex(&view.tip()?));
    Ok(answer)
}
/// A storage record by contract and key, and with `proof` its proof.
fn record_json(view: &View, contract: Hash, key: Hash, proof: bool) -> Result<Value> {
    let record = record_key(2, contract, key);
    let value = view.record(&record)?;
    let mut answer = json!({"contract":hex(&contract),"key":hex(&key),"value_hex":value.as_ref().map(|v|hex(v))});
    if proof {
        answer["proof"] = proof_json(view, &record)?;
    }
    Ok(answer)
}
/// A record's Merkle proof as the API gives it (wiki: data/state-tree#proofs):
/// its record key, where its path ends, and the other child's hash at each
/// level above that, from the end up to the root.
fn proof_json(view: &View, record: &Hash) -> Result<Value> {
    let proof = prove(view, record)?;
    let mut answer = json!({"record_key":hex(record),
        "siblings":proof.siblings.iter().map(|s| hex(s)).collect::<Vec<_>>()});
    match proof.end {
        End::Present => answer["end"] = json!("present"),
        End::Empty => answer["end"] = json!("empty"),
        End::Other(key, leaf) => {
            answer["end"] = json!("other");
            answer["other_key"] = json!(hex(&key));
            answer["other_leaf"] = json!(hex(&leaf));
        }
    }
    Ok(answer)
}
/// The most records one batch read names.
pub const BATCH_MAX: usize = 1_024;
/// Many records, `reads` naming each by `contract` and `key`, all from one
/// view: one state root, so that records read together never straddle a
/// block. The values come in the order asked, with the root and the tip
/// they were read at.
fn storage_batch(view: &View, p: &Value) -> Result<Value> {
    let reads = p["reads"].as_array().ok_or("expected reads")?;
    if reads.is_empty() || reads.len() > BATCH_MAX {
        return Err(format!("reads must name 1 to {BATCH_MAX} records"));
    }
    let proof = p["proof"] == true;
    let mut values = Vec::with_capacity(reads.len());
    for read in reads {
        let id = hash_arg(str_param(read, "contract")?)?;
        let key = hash_arg(str_param(read, "key")?)?;
        values.push(record_json(view, id, key, proof)?);
    }
    Ok(json!({"values":values,"state_root":hex(&view.root()?),"tip":hex(&view.tip()?)}))
}
pub fn receipt_json(r: &Receipt) -> Value {
    json!({"transaction":hex(&r.transaction),"status":r.status as u8,"status_name":format!("{:?}",r.status),
        "used":r.used.0,"result_hex":hex(&r.result),"events":r.events.iter().map(|e|
            json!({"contract":hex(&e.contract),"payload_hex":hex(&e.payload)})).collect::<Vec<_>>()})
}
pub fn transaction_summary(tx: &Transaction) -> Value {
    let (kind, target, input, code) = match &tx.payload {
        ting_core::protocol::Payload::Call { target, input } => {
            ("call", Some(hex(target)), input.len(), None)
        }
        ting_core::protocol::Payload::Deploy { code, input } => {
            ("deploy", None, input.len(), Some(code.len()))
        }
    };
    json!({"kind":kind,"target":target,"input_bytes":input,"code_bytes":code,"budget":tx.budget.0,"encoded_bytes":tx.encode().len()})
}
/// A transaction's hex in a lookup, or null for one larger than a slice: a
/// canonical one is read from its block's slices (wiki: start/explore).
pub fn inline_hex(tx: &Transaction) -> Value {
    let bytes = tx.encode();
    if bytes.len() > SLICE_MAX {
        Value::Null
    } else {
        json!(hex(&bytes))
    }
}
/// The slice of `bytes`, a block or transaction, that a request's `offset`
/// and `length` name, at most `SLICE_MAX` bytes; without `length`, the whole
/// item when it fits one response.
pub fn slice(bytes: &[u8], p: &Value, what: &str) -> Result<Value> {
    let offset = usize::try_from(u64_param(p, "offset")?.unwrap_or(0)).unwrap_or(usize::MAX);
    if offset > bytes.len() {
        return Err("slice offset out of range".into());
    }
    let end = match u64_param(p, "length")? {
        Some(length) if length as usize > SLICE_MAX => {
            return Err("slice length limit".into());
        }
        Some(length) => offset.saturating_add(length as usize).min(bytes.len()),
        None if bytes.len() > crate::FRAME_MAX / 2 - 4096 => {
            return Err(format!(
                "{what} exceeds one response; fetch slices with offset and length"
            ));
        }
        None => bytes.len(),
    };
    Ok(json!({"hex":hex(&bytes[offset..end]),"offset":offset,"total":bytes.len()}))
}
pub fn push_bounded(items: &mut Vec<Value>, item: Value, size: &mut usize) -> Result<bool> {
    let bytes = serde_json::to_vec(&item).map_err(|e| e.to_string())?.len() + 1;
    if size.saturating_add(bytes) > crate::FRAME_MAX - 4096 {
        if items.is_empty() {
            return Err(
                "object exceeds JSON response limit; fetch canonical raw block instead".into(),
            );
        }
        return Ok(false);
    }
    *size += bytes;
    items.push(item);
    Ok(true)
}
