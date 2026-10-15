//! Durable node storage, independent of consensus. See the wiki's node/storage page.
use crate::Result;
use num_bigint::BigUint;
use redb::{
    Database, ReadOnlyTable, ReadTransaction, ReadableDatabase, ReadableTable,
    ReadableTableMetadata, Table, TableDefinition, WriteTransaction,
};
use std::{
    cell::Cell,
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use ting_core::{
    hash::{Hash, hash},
    meter::Resources,
    protocol::{BLOCK_BASE, Block, Encode, Genesis, HEADER_LEN, Header},
    state::{Changes, Entry, Position, State, StateReader},
};

type Blob = TableDefinition<'static, &'static [u8], &'static [u8]>;
pub const META: Blob = TableDefinition::new("metadata");
pub const BLOCKS: Blob = TableDefinition::new("block_locations");
pub const HEADERS: Blob = TableDefinition::new("headers");
pub const RECORDS: Blob = TableDefinition::new("records");
/// Each record's value length, so that a length query never reads the value.
pub const LENGTHS: TableDefinition<&'static [u8], u64> = TableDefinition::new("record_lengths");
pub const NODES: Blob = TableDefinition::new("merkle_nodes");
/// Each executed block's undo data: the previous value of every record it
/// changed, by block id and record key.
pub const DELTAS: Blob = TableDefinition::new("block_changes");
/// Each disconnected block's redo data: the value it left in every record it
/// changed; and which blocks have it, with their heights.
pub const REDO: Blob = TableDefinition::new("block_redo");
pub const REDO_BLOCKS: Blob = TableDefinition::new("redo_blocks");
/// Values too large for one row (wiki: node/storage): a record's value, or one
/// in a block's undo or redo data, above `PART_BYTES` is kept here in parts of
/// that size, by its table's tag, its row's key and the part's number, and its
/// row holds only its length. No database value then nears redb's limit of
/// 3 GiB, whatever size the protocol lets a record reach.
pub const PARTS: Blob = TableDefinition::new("value_parts");
pub const PART_BYTES: usize = 1 << 20;
/// A table whose rows hold values, and its tag among the parts.
#[derive(Clone, Copy)]
struct Rows {
    table: Blob,
    tag: u8,
}
const RECORD_ROWS: Rows = Rows {
    table: RECORDS,
    tag: b'r',
};
const UNDO_ROWS: Rows = Rows {
    table: DELTAS,
    tag: b'u',
};
const REDO_ROWS: Rows = Rows {
    table: REDO,
    tag: b'd',
};
pub const OCCURRENCES: Blob = TableDefinition::new("transaction_occurrences");
pub const CANONICAL: TableDefinition<u64, &'static [u8]> = TableDefinition::new("canonical");
/// Blocks this node's operator marked invalid, by id (wiki: node/storage).
pub const MARKS: Blob = TableDefinition::new("operator_marks");
/// The active limits of each settled canonical block, by id: what the block
/// index needs of a block it no longer holds in memory (wiki: node/storage).
pub const LIMITS: Blob = TableDefinition::new("block_limits");
/// Every stored block off the canonical chain, by id, with its height: with
/// the canonical blocks above the settled height, what a restart indexes.
pub const SIDE: TableDefinition<&'static [u8], u64> = TableDefinition::new("side_blocks");
/// The size a block file grows to before the next begins. A frame larger
/// than that begins a file of its own, which holds it alone: no block is too
/// large to store.
#[cfg(not(test))]
const SEGMENT_SIZE: u64 = 128 * 1024 * 1024;
#[cfg(test)]
const SEGMENT_SIZE: u64 = 64 * 1024;
/// The longest a block's database commit stays unforced to disk. Its frame always
/// is forced, so a crash costs only replaying the frames since (wiki: node/storage).
const DURABLE_INTERVAL: Duration = Duration::from_secs(300);
/// Storage format version, pinned in the database metadata and checked when it is opened.
const FORMAT: u32 = 8;
/// The first 8 bytes of every block segment file, before the ChainId.
const MAGIC: &[u8; 8] = b"TINGBLK\0";
/// A frame is the block's length as a `u64`, the block, and its checksum.
const FRAME_EXTRA: u64 = 8 + 32;

/// What the node holds of a stored block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The body, matching its header, not yet executed.
    Data,
    /// Executed and valid, with its state changes.
    Valid,
    /// Stored, and its execution failed.
    Invalid,
}
impl Kept {
    fn byte(self) -> u8 {
        match self {
            Kept::Data => 1,
            Kept::Valid => 2,
            Kept::Invalid => 3,
        }
    }
    fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            1 => Ok(Kept::Data),
            2 => Ok(Kept::Valid),
            3 => Ok(Kept::Invalid),
            _ => Err("invalid stored block status".into()),
        }
    }
}
pub struct Stored {
    pub header: Header,
    pub work: BigUint,
    pub kept: Kept,
}
/// What the node indexes as it opens: every stored block but the settled
/// canonical ones, at or below `floor`, which it reads from here when it needs
/// them; and the canonical blocks above `floor`, by height.
pub struct Metadata {
    pub blocks: Vec<(Hash, Stored)>,
    pub floor: u64,
    pub canonical: Vec<Hash>,
    pub tip: Hash,
}
#[derive(Clone, Copy, Debug)]
pub struct Location {
    pub segment: u32,
    pub offset: u64,
    pub length: u64,
}
impl Location {
    fn bytes(self) -> Vec<u8> {
        let mut v = self.segment.to_le_bytes().to_vec();
        v.extend(self.offset.to_le_bytes());
        v.extend(self.length.to_le_bytes());
        v
    }
    fn end(self) -> u64 {
        self.offset
            .saturating_add(FRAME_EXTRA)
            .saturating_add(self.length)
    }
}
/// Whether a frame of `length` at `offset` belongs where it is: within a
/// block file's usual size, or alone at the start of a file of its own.
fn frame_fits(offset: u64, length: u64) -> bool {
    offset == 40
        || offset
            .checked_add(FRAME_EXTRA)
            .and_then(|o| o.checked_add(length))
            .is_some_and(|end| end <= SEGMENT_SIZE)
}
/// What is stored, readable from any thread: the database, which serves any
/// number of readers beside its one writer, and the block files. Only the
/// chain thread writes, through its [`Store`]; a reader sees each commit
/// whole (wiki: node/storage).
#[derive(Clone)]
pub struct Reader {
    db: Arc<Database>,
    directory: PathBuf,
    chain: Hash,
    genesis: Hash,
    recent: Arc<Mutex<Recent>>,
}
/// The blocks larger than one slice last read for serving, up to
/// `RECENT_BYTES` in all and always the last one: such a block crosses to a
/// peer in slices, and is read from disk once rather than once a slice.
#[derive(Default)]
struct Recent {
    blocks: VecDeque<(Hash, Arc<Vec<u8>>)>,
    bytes: usize,
}
const RECENT_BYTES: usize = 64 * 1024 * 1024;
impl Recent {
    fn get(&self, id: &Hash) -> Option<Arc<Vec<u8>>> {
        self.blocks
            .iter()
            .find(|(held, _)| held == id)
            .map(|(_, bytes)| bytes.clone())
    }
    fn keep(&mut self, id: Hash, bytes: Arc<Vec<u8>>) {
        self.forget(&id);
        self.bytes = self.bytes.saturating_add(bytes.len());
        self.blocks.push_back((id, bytes));
        while self.bytes > RECENT_BYTES && self.blocks.len() > 1 {
            let (_, gone) = self.blocks.pop_front().expect("more than one");
            self.bytes -= gone.len();
        }
    }
    fn forget(&mut self, id: &Hash) {
        if let Some(at) = self.blocks.iter().position(|(held, _)| held == id) {
            let (_, gone) = self.blocks.remove(at).expect("found above");
            self.bytes -= gone.len();
        }
    }
}
/// One consistent view of what is stored: everything read through it comes
/// from the same commit, however the chain moves meanwhile.
pub struct View<'a> {
    tx: ReadTransaction,
    reader: &'a Reader,
}
/// The default size of the database's page cache.
pub const CACHE_BYTES: u64 = 256 * 1024 * 1024;
pub struct Store {
    reader: Reader,
    /// When a commit was last forced to disk, and the segment of the last
    /// block whose commit was; `None` until the first.
    forced_at: Cell<Option<Instant>>,
    forced_segment: Cell<Option<u32>>,
    /// A commit since the last forced one was not forced.
    lazy: Cell<bool>,
    /// A frame was appended and no commit has indexed it yet.
    unindexed: Cell<bool>,
}
fn err(e: impl std::fmt::Display) -> String {
    format!("storage: {e}")
}
fn hash_bytes(bytes: &[u8]) -> Result<Hash> {
    bytes.try_into().map_err(|_| "invalid stored hash".into())
}
/// A row's value is stored as `0` for none, `1` and the value, or `2` and the
/// value's length as a `u64` when it lies in parts.
fn part_key(tag: u8, key: &[u8], part: usize) -> Vec<u8> {
    let mut k = Vec::with_capacity(key.len() + 5);
    k.push(tag);
    k.extend(key);
    k.extend((part as u32).to_be_bytes());
    k
}
/// The length of a value in parts, or `None` for a row that holds its value.
fn parted_length(row: &[u8]) -> Result<Option<usize>> {
    match row.split_first() {
        Some((0, [])) | Some((1, _)) => Ok(None),
        Some((2, length)) => Ok(Some(
            usize::try_from(u64::from_le_bytes(
                length.try_into().map_err(|_| "invalid stored value")?,
            ))
            .map_err(err)?,
        )),
        _ => Err("invalid stored value".into()),
    }
}
/// Writes row `key` holding `value`, in parts if it is larger than one, and
/// removes the parts the row's previous value had beyond them.
fn put_row(
    rows: &mut Table<&'static [u8], &'static [u8]>,
    parts: &mut Table<&'static [u8], &'static [u8]>,
    tag: u8,
    key: &[u8],
    value: Option<&[u8]>,
) -> Result<()> {
    let (row, count) = match value {
        None => (vec![0], 0),
        Some(v) if v.len() <= PART_BYTES => ([&[1][..], v].concat(), 0),
        Some(v) => (
            [&[2][..], &(v.len() as u64).to_le_bytes()].concat(),
            v.len().div_ceil(PART_BYTES),
        ),
    };
    let old = match rows.insert(key, row.as_slice()).map_err(err)? {
        Some(old) => parted_length(old.value())?.map_or(0, |n| n.div_ceil(PART_BYTES)),
        None => 0,
    };
    if count > 0
        && let Some(value) = value
    {
        for (i, part) in value.chunks(PART_BYTES).enumerate() {
            parts
                .insert(part_key(tag, key, i).as_slice(), part)
                .map_err(err)?;
        }
    }
    for i in count..old {
        parts
            .remove(part_key(tag, key, i).as_slice())
            .map_err(err)?;
    }
    Ok(())
}
/// Removes row `key` and its parts.
fn remove_row(
    rows: &mut Table<&'static [u8], &'static [u8]>,
    parts: &mut Table<&'static [u8], &'static [u8]>,
    tag: u8,
    key: &[u8],
) -> Result<()> {
    let old = match rows.remove(key).map_err(err)? {
        Some(old) => parted_length(old.value())?.map_or(0, |n| n.div_ceil(PART_BYTES)),
        None => 0,
    };
    for i in 0..old {
        parts
            .remove(part_key(tag, key, i).as_slice())
            .map_err(err)?;
    }
    Ok(())
}
/// The value row `key` holds, gathered from its parts if it lies in them.
fn row_value(
    parts: &impl ReadableTable<&'static [u8], &'static [u8]>,
    tag: u8,
    key: &[u8],
    row: &[u8],
) -> Result<Option<Vec<u8>>> {
    let Some(length) = parted_length(row)? else {
        return Ok((row[0] == 1).then(|| row[1..].to_vec()));
    };
    let mut value = Vec::with_capacity(length.min(64 * PART_BYTES));
    for i in 0..length.div_ceil(PART_BYTES) {
        let part = parts
            .get(part_key(tag, key, i).as_slice())
            .map_err(err)?
            .ok_or("missing value part")?;
        value.extend_from_slice(part.value());
    }
    if value.len() != length {
        return Err("stored value length mismatch".into());
    }
    Ok(Some(value))
}
fn frame_length_ok(length: u64) -> bool {
    length >= BLOCK_BASE
}
/// The height in a `headers` row: the header's bytes 34 to 42, after the
/// row's length.
fn row_height(row: &[u8]) -> Result<u64> {
    row.get(36..44)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| "truncated stored header".into())
}
/// The settled floor a read transaction sees; 0 before anything settled.
fn floor_of(tx: &ReadTransaction) -> Result<u64> {
    match tx
        .open_table(META)
        .map_err(err)?
        .get(b"index_floor".as_slice())
        .map_err(err)?
    {
        Some(v) => Ok(u64::from_le_bytes(
            v.value().try_into().map_err(|_| "invalid index floor")?,
        )),
        None => Ok(0),
    }
}
/// A `headers` row: the header's length as a `u16`, the header, its status
/// and its accumulated work.
fn decode_stored(id: Hash, bytes: &[u8], chain: Hash) -> Result<Stored> {
    let n = u16::from_le_bytes(
        bytes
            .get(..2)
            .ok_or("truncated stored header")?
            .try_into()
            .unwrap(),
    ) as usize;
    let header =
        Header::decode(bytes.get(2..2 + n).ok_or("truncated stored header")?).map_err(err)?;
    let kept = Kept::from_byte(*bytes.get(2 + n).ok_or("truncated stored header")?)?;
    if bytes.len().saturating_sub(3 + n) > 40 {
        return Err("invalid stored work".into());
    }
    if header.id() != id || header.chain_id != chain {
        return Err("stored header identity mismatch".into());
    }
    Ok(Stored {
        header,
        work: BigUint::from_bytes_le(&bytes[3 + n..]),
        kept,
    })
}
/// A node is stored as its hash, a leaf as its record key and its hash.
fn entry_bytes(entry: &Entry) -> Vec<u8> {
    match entry {
        Entry::Node(hash) => hash.to_vec(),
        Entry::Leaf(key, hash) => [&key[..], &hash[..]].concat(),
    }
}
fn decode_entry(bytes: &[u8]) -> Result<Entry> {
    match bytes.len() {
        32 => Ok(Entry::Node(hash_bytes(bytes)?)),
        64 => Ok(Entry::Leaf(
            hash_bytes(&bytes[..32])?,
            hash_bytes(&bytes[32..])?,
        )),
        _ => Err("invalid state-tree entry".into()),
    }
}
fn position_bytes(position: &Position) -> [u8; 34] {
    let mut bytes = [0; 34];
    bytes[..2].copy_from_slice(&position.0.to_be_bytes());
    bytes[2..].copy_from_slice(&position.1);
    bytes
}
fn get(tx: &WriteTransaction, table: Blob, key: &[u8]) -> Result<Option<Vec<u8>>> {
    Ok(tx
        .open_table(table)
        .map_err(err)?
        .get(key)
        .map_err(err)?
        .map(|v| v.value().to_vec()))
}
fn put(tx: &WriteTransaction, table: Blob, key: &[u8], value: &[u8]) -> Result<()> {
    tx.open_table(table)
        .map_err(err)?
        .insert(key, value)
        .map_err(err)?;
    Ok(())
}
fn required(tx: &WriteTransaction, key: &[u8]) -> Result<Vec<u8>> {
    get(tx, META, key)?.ok_or_else(|| "missing database metadata".into())
}
struct ReadView {
    records: ReadOnlyTable<&'static [u8], &'static [u8]>,
    parts: ReadOnlyTable<&'static [u8], &'static [u8]>,
    lengths: ReadOnlyTable<&'static [u8], u64>,
    nodes: ReadOnlyTable<&'static [u8], &'static [u8]>,
}
impl StateReader for ReadView {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>> {
        match self.records.get(key.as_slice()).map_err(err)? {
            Some(row) => row_value(&self.parts, RECORD_ROWS.tag, key, row.value()),
            None => Ok(None),
        }
    }
    fn record_len(&self, key: &Hash) -> Result<Option<u64>> {
        Ok(self
            .lengths
            .get(key.as_slice())
            .map_err(err)?
            .map(|v| v.value()))
    }
    fn node(&self, p: &Position) -> Result<Option<Entry>> {
        self.nodes
            .get(position_bytes(p).as_slice())
            .map_err(err)?
            .map(|v| decode_entry(v.value()))
            .transpose()
    }
}
struct WriteView(Rc<WriteTransaction>);
impl StateReader for WriteView {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>> {
        let records = self.0.open_table(RECORDS).map_err(err)?;
        match records.get(key.as_slice()).map_err(err)? {
            Some(row) => {
                let parts = self.0.open_table(PARTS).map_err(err)?;
                row_value(&parts, RECORD_ROWS.tag, key, row.value())
            }
            None => Ok(None),
        }
    }
    fn record_len(&self, key: &Hash) -> Result<Option<u64>> {
        Ok(self
            .0
            .open_table(LENGTHS)
            .map_err(err)?
            .get(key.as_slice())
            .map_err(err)?
            .map(|v| v.value()))
    }
    fn node(&self, p: &Position) -> Result<Option<Entry>> {
        get(&self.0, NODES, &position_bytes(p))?
            .map(|v| decode_entry(&v))
            .transpose()
    }
}

impl Store {
    /// Opens the store in `dir`, with a page cache of up to `cache` bytes.
    pub fn open(dir: &Path, g: &Genesis, cache: u64) -> Result<Self> {
        let db = Database::builder()
            .set_cache_size(usize::try_from(cache).unwrap_or(usize::MAX))
            .create(dir.join("chain.redb"))
            .map_err(err)?;
        let store = Self {
            reader: Reader {
                db: Arc::new(db),
                directory: dir.to_owned(),
                chain: g.chain_id(),
                genesis: g.block_id(),
                recent: Arc::default(),
            },
            forced_at: Cell::new(None),
            forced_segment: Cell::new(None),
            lazy: Cell::new(false),
            unindexed: Cell::new(false),
        };
        let tx = store.begin()?;
        for table in [
            META,
            BLOCKS,
            HEADERS,
            RECORDS,
            NODES,
            DELTAS,
            REDO,
            REDO_BLOCKS,
            PARTS,
            OCCURRENCES,
            MARKS,
            LIMITS,
        ] {
            tx.open_table(table).map_err(err)?;
        }
        tx.open_table(CANONICAL).map_err(err)?;
        tx.open_table(LENGTHS).map_err(err)?;
        tx.open_table(SIDE).map_err(err)?;
        if get(&tx, META, b"format")?.is_none() {
            if tx.open_table(BLOCKS).map_err(err)?.len().map_err(err)? != 0 {
                return Err("missing database format".into());
            }
            put(&tx, META, b"format", &FORMAT.to_le_bytes())?;
            put(&tx, META, b"chain", &g.chain_id())?;
            put(&tx, META, b"tip", &g.block_id())?;
            put(&tx, META, b"root", &State::default().root())?;
            put(&tx, META, b"count", &0u64.to_le_bytes())?;
            put(
                &tx,
                META,
                b"file_cursor",
                &Location {
                    segment: 0,
                    offset: 40,
                    length: 0,
                }
                .bytes(),
            )?;
        } else if required(&tx, b"format")? != FORMAT.to_le_bytes()
            || required(&tx, b"chain")? != g.chain_id()
        {
            return Err("database genesis or format mismatch".into());
        }
        // A database from before the side-block table: it is filled once,
        // from every stored header off the canonical chain.
        if get(&tx, META, b"side_blocks")?.is_none() {
            let side = {
                let headers = tx.open_table(HEADERS).map_err(err)?;
                let canonical = tx.open_table(CANONICAL).map_err(err)?;
                let mut side = Vec::new();
                for row in headers.iter().map_err(err)? {
                    let (id, value) = row.map_err(err)?;
                    let height = row_height(value.value())?;
                    if canonical
                        .get(height)
                        .map_err(err)?
                        .is_none_or(|c| c.value() != id.value())
                    {
                        side.push((id.value().to_vec(), height));
                    }
                }
                side
            };
            let mut table = tx.open_table(SIDE).map_err(err)?;
            for (id, height) in side {
                table.insert(id.as_slice(), height).map_err(err)?;
            }
            drop(table);
            put(&tx, META, b"side_blocks", &[1])?;
        }
        tx.commit().map_err(err)?;
        Ok(store)
    }
    /// A reader of this store for other threads.
    pub fn reader(&self) -> Reader {
        self.reader.clone()
    }
    pub fn begin(&self) -> Result<WriteTransaction> {
        let mut tx = self.db.begin_write().map_err(err)?;
        tx.set_durability(redb::Durability::Immediate)
            .map_err(err)?;
        tx.set_quick_repair(true);
        Ok(tx)
    }
    /// Commits the transaction that stored a block whose frame is at `location`.
    /// The commit is forced to disk for the first block after opening, the first
    /// block of each segment, and then at least every `DURABLE_INTERVAL`. After a
    /// crash the database holds the last forced commit, and every frame written
    /// since lies in its segment or the next, where recovery replays them.
    pub fn commit_block(&self, mut tx: WriteTransaction, location: Location) -> Result<()> {
        let forced = self.forced_segment.get() != Some(location.segment) || self.due();
        if !forced {
            tx.set_durability(redb::Durability::None).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        self.unindexed.set(false);
        self.committed(forced);
        if forced {
            self.forced_segment.set(Some(location.segment));
        }
        Ok(())
    }
    /// Commits a transaction that moved state without storing a new block:
    /// forced to disk for the first commit after opening, and then at least
    /// every `DURABLE_INTERVAL`, like block commits.
    pub fn commit_state(&self, mut tx: WriteTransaction) -> Result<()> {
        let forced = self.due();
        if !forced {
            tx.set_durability(redb::Durability::None).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        self.committed(forced);
        Ok(())
    }
    /// Forces to disk the commits made since the last forced one, if any.
    pub fn flush(&self) -> Result<()> {
        if self.lazy.get() {
            self.begin()?.commit().map_err(err)?;
            self.committed(true);
        }
        Ok(())
    }
    /// The same once `DURABLE_INTERVAL` has passed since the last forced
    /// commit, so that commits made while no block arrives reach the disk.
    pub fn flush_due(&self) -> Result<()> {
        if self.due() {
            self.flush()?;
        }
        Ok(())
    }
    /// Whether the next commit is forced to disk: none has been since opening,
    /// or the last was `DURABLE_INTERVAL` ago.
    pub fn due(&self) -> bool {
        self.forced_at
            .get()
            .is_none_or(|at| at.elapsed() >= DURABLE_INTERVAL)
    }
    /// Whether a commit was made since the last forced one.
    pub fn lazy(&self) -> bool {
        self.lazy.get()
    }
    fn committed(&self, forced: bool) {
        if forced {
            self.forced_at.set(Some(Instant::now()));
        }
        self.lazy.set(!forced);
    }
    pub fn metadata(&self) -> Result<Metadata> {
        let tx = self.db.begin_read().map_err(err)?;
        let floor = floor_of(&tx)?;
        let locations = tx.open_table(BLOCKS).map_err(err)?;
        let canonical_rows = tx.open_table(CANONICAL).map_err(err)?;
        if locations.len().map_err(err)?
            != tx.open_table(HEADERS).map_err(err)?.len().map_err(err)?
        {
            return Err("block metadata count mismatch".into());
        }
        // The canonical blocks above the floor, then every stored side block:
        // what the index holds. A settled block is not read at all.
        let heights = canonical_rows.len().map_err(err)?;
        if heights < floor {
            return Err("settled blocks above the canonical tip".into());
        }
        if floor > 0 && canonical_rows.get(floor).map_err(err)?.is_none() {
            return Err("canonical height gap".into());
        }
        let mut canonical = Vec::new();
        for row in canonical_rows.range(floor + 1..).map_err(err)? {
            let (height, id) = row.map_err(err)?;
            if height.value() != floor + canonical.len() as u64 + 1 {
                return Err("canonical height gap".into());
            }
            canonical.push(hash_bytes(id.value())?);
        }
        if floor + canonical.len() as u64 != heights {
            return Err("canonical height gap".into());
        }
        let mut ids = canonical.clone();
        for row in tx.open_table(SIDE).map_err(err)?.iter().map_err(err)? {
            ids.push(hash_bytes(row.map_err(err)?.0.value())?);
        }
        let header_rows = tx.open_table(HEADERS).map_err(err)?;
        let mut headers = Vec::with_capacity(ids.len());
        for id in ids {
            let row = header_rows
                .get(id.as_slice())
                .map_err(err)?
                .ok_or("missing stored header")?;
            let stored = decode_stored(id, row.value(), self.chain)?;
            let location = decode_location(
                locations
                    .get(id.as_slice())
                    .map_err(err)?
                    .ok_or("missing block location")?
                    .value(),
            )?;
            if !frame_length_ok(location.length) {
                return Err("invalid indexed block length".into());
            }
            headers.push((id, stored));
        }
        let tip = hash_bytes(
            tx.open_table(META)
                .map_err(err)?
                .get(b"tip".as_slice())
                .map_err(err)?
                .ok_or("missing tip")?
                .value(),
        )?;
        Ok(Metadata {
            blocks: headers,
            floor,
            canonical,
            tip,
        })
    }
    /// Settles canonical blocks: the block index reads them from here from now
    /// on, by their limits and the new floor, the height at or below which
    /// every canonical block is settled. A lower floor unsettles the blocks
    /// above it.
    pub fn settle(tx: &WriteTransaction, limits: &[(Hash, Resources)], floor: u64) -> Result<()> {
        let mut table = tx.open_table(LIMITS).map_err(err)?;
        for (id, limits) in limits {
            let bytes: Vec<u8> = limits.0.iter().flat_map(|v| v.to_le_bytes()).collect();
            table.insert(id.as_slice(), bytes.as_slice()).map_err(err)?;
        }
        drop(table);
        put(tx, META, b"index_floor", &floor.to_le_bytes())
    }
    pub fn snapshot(&self) -> Result<State> {
        let tx = self.db.begin_read().map_err(err)?;
        let meta = tx.open_table(META).map_err(err)?;
        let root = hash_bytes(
            meta.get(b"root".as_slice())
                .map_err(err)?
                .ok_or("missing state root")?
                .value(),
        )?;
        let count = u64::from_le_bytes(
            meta.get(b"count".as_slice())
                .map_err(err)?
                .ok_or("missing count")?
                .value()
                .try_into()
                .map_err(|_| "invalid count")?,
        );
        let records = tx.open_table(RECORDS).map_err(err)?;
        let parts = tx.open_table(PARTS).map_err(err)?;
        let lengths = tx.open_table(LENGTHS).map_err(err)?;
        let nodes = tx.open_table(NODES).map_err(err)?;
        if records.len().map_err(err)? != count || lengths.len().map_err(err)? != count {
            return Err("state record count mismatch".into());
        }
        let actual = nodes
            .get(position_bytes(&(0, [0; 32])).as_slice())
            .map_err(err)?
            .map(|v| decode_entry(v.value()))
            .transpose()?
            .map_or(State::default().root(), |entry| entry.hash());
        if actual != root {
            return Err("persisted Merkle root mismatch".into());
        }
        Ok(State::backed(
            Rc::new(ReadView {
                records,
                parts,
                lengths,
                nodes,
            }),
            usize::try_from(count).map_err(err)?,
            root,
        ))
    }
    pub fn state(tx: Rc<WriteTransaction>) -> Result<State> {
        let root = hash_bytes(&required(&tx, b"root")?)?;
        let count = u64::from_le_bytes(
            required(&tx, b"count")?
                .try_into()
                .map_err(|_| "invalid count")?,
        );
        Ok(State::backed(
            Rc::new(WriteView(tx)),
            usize::try_from(count).map_err(err)?,
            root,
        ))
    }
    pub fn apply(tx: &WriteTransaction, changes: &Changes) -> Result<()> {
        {
            let mut table = tx.open_table(RECORDS).map_err(err)?;
            let mut parts = tx.open_table(PARTS).map_err(err)?;
            let mut lengths = tx.open_table(LENGTHS).map_err(err)?;
            let tag = RECORD_ROWS.tag;
            for change in &changes.records {
                let key = change.key.as_slice();
                if let Some(value) = &change.after {
                    put_row(&mut table, &mut parts, tag, key, Some(value))?;
                    lengths.insert(key, value.len() as u64).map_err(err)?;
                } else {
                    remove_row(&mut table, &mut parts, tag, key)?;
                    lengths.remove(key).map_err(err)?;
                }
            }
        }
        {
            let mut table = tx.open_table(NODES).map_err(err)?;
            for (position, value) in &changes.nodes {
                let key = position_bytes(position);
                if let Some(entry) = value {
                    table
                        .insert(key.as_slice(), entry_bytes(entry).as_slice())
                        .map_err(err)?;
                } else {
                    table.remove(key.as_slice()).map_err(err)?;
                }
            }
        }
        put(tx, META, b"root", &changes.root)?;
        put(tx, META, b"count", &(changes.count as u64).to_le_bytes())
    }
    /// A block's undo data, by record key.
    fn undo_rows(tx: &WriteTransaction, id: Hash) -> Result<Vec<(Hash, Option<Vec<u8>>)>> {
        let table = tx.open_table(DELTAS).map_err(err)?;
        let parts = tx.open_table(PARTS).map_err(err)?;
        rows(&table, &parts, UNDO_ROWS.tag, id)
    }
    /// The undo data of a block, from a store opened for reading only, such as
    /// the replay's.
    pub fn undo_of(&self, id: Hash) -> Result<Vec<(Hash, Option<Vec<u8>>)>> {
        let tx = self.db.begin_read().map_err(err)?;
        let table = tx.open_table(DELTAS).map_err(err)?;
        let parts = tx.open_table(PARTS).map_err(err)?;
        rows(&table, &parts, UNDO_ROWS.tag, id)
    }
    /// Keeps a block's undo data.
    pub fn put_undo(
        tx: &WriteTransaction,
        id: Hash,
        undo: &[(Hash, Option<Vec<u8>>)],
    ) -> Result<()> {
        Self::put_rows(tx, UNDO_ROWS, id, undo)
    }
    /// Keeps a block's undo or redo data, a row per record key.
    fn put_rows(
        tx: &WriteTransaction,
        rows: Rows,
        id: Hash,
        values: &[(Hash, Option<Vec<u8>>)],
    ) -> Result<()> {
        let mut table = tx.open_table(rows.table).map_err(err)?;
        let mut parts = tx.open_table(PARTS).map_err(err)?;
        for (key, value) in values {
            let k = [&id[..], &key[..]].concat();
            put_row(&mut table, &mut parts, rows.tag, &k, value.as_deref())?;
        }
        Ok(())
    }
    /// Disconnects a block by its undo data, keeping first, as its redo data,
    /// the values it left. The root reached must be `parent_root`.
    pub fn undo_block(
        tx: Rc<WriteTransaction>,
        id: Hash,
        height: u64,
        parent_root: Hash,
    ) -> Result<()> {
        let undo = Self::undo_rows(&tx, id)?;
        let mut state = Self::state(tx.clone())?;
        let mut redo = Vec::with_capacity(undo.len());
        for (key, before) in undo {
            let after = state.get(&key);
            if let Some(error) = state.storage_error() {
                return Err(error);
            }
            redo.push((key, after));
            state.set(key, before);
        }
        let changes = state.changes()?;
        if changes.root != parent_root {
            return Err("restored state root mismatch".into());
        }
        Self::apply(&tx, &changes)?;
        Self::put_rows(&tx, REDO_ROWS, id, &redo)?;
        put(&tx, REDO_BLOCKS, &id, &height.to_le_bytes())
    }
    /// Connects a block again by its redo data, if it has any; returns
    /// whether it did. The root reached must be `root`, the block's own.
    pub fn redo_block(tx: Rc<WriteTransaction>, id: Hash, root: Hash) -> Result<bool> {
        if get(&tx, REDO_BLOCKS, &id)?.is_none() {
            return Ok(false);
        }
        let redo = {
            let table = tx.open_table(REDO).map_err(err)?;
            let parts = tx.open_table(PARTS).map_err(err)?;
            rows(&table, &parts, REDO_ROWS.tag, id)?
        };
        let mut state = Self::state(tx.clone())?;
        for (key, after) in &redo {
            state.set(*key, after.clone());
        }
        let changes = state.changes()?;
        if changes.root != root {
            return Err("redone state root mismatch".into());
        }
        Self::apply(&tx, &changes)?;
        Self::drop_rows(&tx, REDO_ROWS, id)?;
        tx.open_table(REDO_BLOCKS)
            .map_err(err)?
            .remove(id.as_slice())
            .map_err(err)?;
        Ok(true)
    }
    /// Deletes a block's rows of undo or redo data, with their parts.
    fn drop_rows(tx: &WriteTransaction, rows: Rows, id: Hash) -> Result<()> {
        let mut table = tx.open_table(rows.table).map_err(err)?;
        let mut parts = tx.open_table(PARTS).map_err(err)?;
        let low = [&id[..], &[0; 32]].concat();
        let high = [&id[..], &[255; 32]].concat();
        let keys: Vec<Vec<u8>> = table
            .range(low.as_slice()..=high.as_slice())
            .map_err(err)?
            .map(|row| row.map(|(k, _)| k.value().to_vec()))
            .collect::<std::result::Result<_, _>>()
            .map_err(err)?;
        for key in keys {
            remove_row(&mut table, &mut parts, rows.tag, &key)?;
        }
        Ok(())
    }
    /// Deletes the undo data of canonical blocks below the window, and notes
    /// that heights up to `pruned` have none (wiki: node/storage).
    pub fn prune_undo(tx: &WriteTransaction, ids: &[Hash], pruned: u64) -> Result<()> {
        for id in ids {
            Self::drop_rows(tx, UNDO_ROWS, *id)?;
        }
        put(tx, META, b"pruned", &pruned.to_le_bytes())
    }
    /// Notes how far undo data is pruned, as after a replay restored it.
    pub fn set_pruned(tx: &WriteTransaction, pruned: u64) -> Result<()> {
        put(tx, META, b"pruned", &pruned.to_le_bytes())
    }
    /// Deletes the undo and redo data of disconnected blocks at heights up to
    /// `height`: connected again, they are executed again.
    pub fn prune_redo(tx: &WriteTransaction, height: u64) -> Result<()> {
        let old: Vec<Hash> = {
            let table = tx.open_table(REDO_BLOCKS).map_err(err)?;
            let mut old = Vec::new();
            for row in table.iter().map_err(err)? {
                let (id, at) = row.map_err(err)?;
                let at =
                    u64::from_le_bytes(at.value().try_into().map_err(|_| "invalid redo height")?);
                if at <= height {
                    old.push(hash_bytes(id.value())?);
                }
            }
            old
        };
        for id in old {
            Self::drop_rows(tx, REDO_ROWS, id)?;
            Self::drop_rows(tx, UNDO_ROWS, id)?;
            tx.open_table(REDO_BLOCKS)
                .map_err(err)?
                .remove(id.as_slice())
                .map_err(err)?;
        }
        Ok(())
    }
    /// The height up to which canonical blocks have no undo data.
    pub fn pruned(&self) -> Result<u64> {
        let tx = self.db.begin_read().map_err(err)?;
        let meta = tx.open_table(META).map_err(err)?;
        Ok(match meta.get(b"pruned".as_slice()).map_err(err)? {
            Some(v) => {
                u64::from_le_bytes(v.value().try_into().map_err(|_| "invalid pruned height")?)
            }
            None => 0,
        })
    }
    /// Deletes what is stored of a block whose body cannot be read, so that it
    /// is fetched again; its frame stays in its file, unreferenced.
    pub fn forget_block(&self, tx: &WriteTransaction, id: &Hash) -> Result<()> {
        self.reader
            .recent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .forget(id);
        tx.open_table(SIDE)
            .map_err(err)?
            .remove(id.as_slice())
            .map_err(err)?;
        tx.open_table(BLOCKS)
            .map_err(err)?
            .remove(id.as_slice())
            .map_err(err)?;
        tx.open_table(HEADERS)
            .map_err(err)?
            .remove(id.as_slice())
            .map_err(err)?;
        Ok(())
    }
    /// Deletes everything stored of a side-branch block the node no longer
    /// keeps (wiki: node/storage): its location and header, the occurrences of
    /// its transactions, and any undo and redo data. Its frame stays in its
    /// file, unreferenced.
    pub fn forget_side(&self, tx: &WriteTransaction, id: &Hash) -> Result<()> {
        if let Ok(block) = self.block(id) {
            let mut table = tx.open_table(OCCURRENCES).map_err(err)?;
            for (index, transaction) in block.transactions.iter().enumerate() {
                let mut key = transaction.id(self.chain).to_vec();
                key.extend(block.header.height.to_be_bytes());
                key.extend(id);
                key.extend((index as u32).to_be_bytes());
                table.remove(key.as_slice()).map_err(err)?;
            }
        }
        Self::drop_rows(tx, UNDO_ROWS, *id)?;
        Self::drop_rows(tx, REDO_ROWS, *id)?;
        tx.open_table(REDO_BLOCKS)
            .map_err(err)?
            .remove(id.as_slice())
            .map_err(err)?;
        self.forget_block(tx, id)
    }
    /// Whether a frame was appended that no commit indexed: a failed import
    /// that leaves one must stop the node.
    pub fn has_unindexed(&self) -> bool {
        self.unindexed.get()
    }
    fn put_header(
        tx: &WriteTransaction,
        header: &Header,
        work: &BigUint,
        kept: Kept,
    ) -> Result<()> {
        let encoded = header.encode();
        let mut value = (encoded.len() as u16).to_le_bytes().to_vec();
        value.extend(encoded);
        value.push(kept.byte());
        value.extend(work.to_bytes_le());
        put(tx, HEADERS, &header.id(), &value)
    }
    /// A block's body is stored at `location`, not yet executed.
    pub fn save_data(
        tx: &WriteTransaction,
        header: &Header,
        work: &BigUint,
        location: Location,
    ) -> Result<()> {
        put(tx, BLOCKS, &header.id(), &location.bytes())?;
        Self::put_header(tx, header, work, Kept::Data)?;
        // Stored, a block is off the canonical chain until it is connected.
        tx.open_table(SIDE)
            .map_err(err)?
            .insert(header.id().as_slice(), header.height)
            .map_err(err)?;
        // A frame recovered out of order lies before the cursor: it stays.
        let cursor = decode_location(&required(tx, b"file_cursor")?)?;
        if (location.segment, location.end()) <= (cursor.segment, cursor.offset) {
            return Ok(());
        }
        put(
            tx,
            META,
            b"file_cursor",
            &Location {
                offset: location.end(),
                length: 0,
                ..location
            }
            .bytes(),
        )
    }
    /// A stored block executed and valid: its record changes and the
    /// occurrences of its transactions.
    pub fn save_valid(
        &self,
        tx: &WriteTransaction,
        block: &Block,
        work: &BigUint,
        changes: &Changes,
    ) -> Result<()> {
        let id = block.header.id();
        Self::put_header(tx, &block.header, work, Kept::Valid)?;
        let undo: Vec<(Hash, Option<Vec<u8>>)> = changes
            .records
            .iter()
            .map(|c| (c.key, c.before.clone()))
            .collect();
        Self::put_undo(tx, id, &undo)?;
        for (index, transaction) in block.transactions.iter().enumerate() {
            let mut key = transaction.id(self.chain).to_vec();
            key.extend(block.header.height.to_be_bytes());
            key.extend(id);
            key.extend((index as u32).to_be_bytes());
            put(tx, OCCURRENCES, &key, &[])?;
        }
        Ok(())
    }
    /// A stored block whose execution failed.
    pub fn save_invalid(tx: &WriteTransaction, header: &Header, work: &BigUint) -> Result<()> {
        Self::put_header(tx, header, work, Kept::Invalid)
    }
    /// A stored block given back its status before it failed, which keeps
    /// whatever else is stored of it.
    pub fn save_kept(
        tx: &WriteTransaction,
        header: &Header,
        work: &BigUint,
        kept: Kept,
    ) -> Result<()> {
        Self::put_header(tx, header, work, kept)
    }
    /// The blocks the operator marked invalid.
    pub fn marks(&self) -> Result<Vec<Hash>> {
        let tx = self.db.begin_read().map_err(err)?;
        let table = tx.open_table(MARKS).map_err(err)?;
        let mut marks = Vec::new();
        for row in table.iter().map_err(err)? {
            let (id, _) = row.map_err(err)?;
            marks.push(hash_bytes(id.value())?);
        }
        Ok(marks)
    }
    pub fn mark(tx: &WriteTransaction, id: &Hash, marked: bool) -> Result<()> {
        let mut table = tx.open_table(MARKS).map_err(err)?;
        if marked {
            table.insert(id.as_slice(), [].as_slice()).map_err(err)?;
        } else {
            table.remove(id.as_slice()).map_err(err)?;
        }
        Ok(())
    }
    pub fn canonical(
        tx: &WriteTransaction,
        common: usize,
        branch: &[Hash],
        tip: Hash,
    ) -> Result<()> {
        let mut table = tx.open_table(CANONICAL).map_err(err)?;
        let mut side = tx.open_table(SIDE).map_err(err)?;
        let old = table.len().map_err(err)?;
        // What leaves the canonical chain is a side block, and what joins it
        // is one no longer.
        for height in common as u64 + 1..=old {
            if let Some(id) = table.remove(height).map_err(err)? {
                side.insert(id.value(), height).map_err(err)?;
            }
        }
        for (index, id) in branch.iter().enumerate() {
            table
                .insert((common + index + 1) as u64, id.as_slice())
                .map_err(err)?;
            side.remove(id.as_slice()).map_err(err)?;
        }
        drop((table, side));
        put(tx, META, b"tip", &tip)
    }
    fn cursor(&self) -> Result<Location> {
        let tx = self.db.begin_read().map_err(err)?;
        let meta = tx.open_table(META).map_err(err)?;
        let cursor = decode_location(
            meta.get(b"file_cursor".as_slice())
                .map_err(err)?
                .ok_or("missing file cursor")?
                .value(),
        )?;
        if cursor.length != 0 {
            return Err("invalid file cursor length".into());
        }
        Ok(cursor)
    }
    fn repair_new_header(&self, file: &mut std::fs::File, cursor: Location) -> Result<()> {
        if cursor.offset == 40 && file.metadata().map_err(err)?.len() < 40 {
            file.seek(SeekFrom::Start(0)).map_err(err)?;
            let mut magic = MAGIC.to_vec();
            magic.extend(self.chain);
            file.write_all(&magic)
                .and_then(|()| file.sync_all())
                .map_err(err)?;
        }
        Ok(())
    }
    fn read_frame(&self, file: &mut std::fs::File, offset: u64) -> Result<Option<Vec<u8>>> {
        self.check_magic(file)?;
        if offset < 40 || offset > file.metadata().map_err(err)?.len() {
            return Err("invalid block file cursor".into());
        }
        file.seek(SeekFrom::Start(offset)).map_err(err)?;
        let size = file.metadata().map_err(err)?.len();
        let mut length = [0; 8];
        if let Err(e) = file.read_exact(&mut length) {
            if e.kind() != std::io::ErrorKind::UnexpectedEof {
                return Err(err(e));
            }
            file.set_len(offset)
                .and_then(|()| file.sync_all())
                .map_err(err)?;
            return Ok(None);
        }
        let length = u64::from_le_bytes(length);
        // Unindexed, the tail holds nothing a row refers to: a damaged frame,
        // such as one a power loss left zero-filled, ends it, and its block
        // and any after it are fetched again (wiki: node/storage).
        let truncate = |file: &mut std::fs::File| -> Result<Option<Vec<u8>>> {
            eprintln!("block file: a damaged unindexed frame at {offset} is truncated");
            file.set_len(offset)
                .and_then(|()| file.sync_all())
                .map_err(err)?;
            Ok(None)
        };
        if !frame_length_ok(length) || !frame_fits(offset, length) {
            return truncate(file);
        }
        // A frame the file does not hold whole was cut short by a crash.
        if (Location {
            segment: 0,
            offset,
            length,
        })
        .end()
            > size
        {
            file.set_len(offset)
                .and_then(|()| file.sync_all())
                .map_err(err)?;
            return Ok(None);
        }
        let mut bytes = vec![0; usize::try_from(length).map_err(err)?];
        let mut checksum = [0; 32];
        if let Err(e) = file
            .read_exact(&mut bytes)
            .and_then(|()| file.read_exact(&mut checksum))
        {
            if e.kind() != std::io::ErrorKind::UnexpectedEof {
                return Err(err(e));
            }
            file.set_len(offset)
                .and_then(|()| file.sync_all())
                .map_err(err)?;
            return Ok(None);
        }
        if checksum != hash("node-block", &bytes) {
            return truncate(file);
        }
        Ok(Some(bytes))
    }
    pub fn append(&self, bytes: &[u8]) -> Result<Location> {
        if !frame_length_ok(bytes.len() as u64) {
            return Err("block shorter than any block".into());
        }
        fs::create_dir_all(self.directory.join("blocks")).map_err(err)?;
        let mut cursor = self.cursor()?;
        if !frame_fits(cursor.offset, bytes.len() as u64) {
            cursor.segment = cursor.segment.checked_add(1).ok_or("segment overflow")?;
            cursor.offset = 40;
        }
        #[cfg(unix)]
        let new_segment = !self.segment(cursor.segment).exists();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.segment(cursor.segment))
            .map_err(err)?;
        if file.metadata().map_err(err)?.len() == 0 {
            let mut magic = MAGIC.to_vec();
            magic.extend(self.chain);
            file.write_all(&magic)
                .and_then(|()| file.sync_all())
                .map_err(err)?;
        }
        self.check_magic(&mut file)?;
        if file.metadata().map_err(err)?.len() != cursor.offset {
            return Err("unindexed block tail: reopen node for recovery".into());
        }
        file.seek(SeekFrom::Start(cursor.offset)).map_err(err)?;
        let result = file
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .and_then(|()| file.write_all(bytes))
            .and_then(|()| file.write_all(&hash("node-block", bytes)))
            .and_then(|()| file.sync_all());
        if let Err(e) = result {
            file.set_len(cursor.offset)
                .and_then(|()| file.sync_all())
                .map_err(|r| format!("block append failed: {e}; repair failed: {r}"))?;
            return Err(err(e));
        }
        #[cfg(unix)]
        if new_segment {
            std::fs::File::open(self.directory.join("blocks"))
                .and_then(|dir| dir.sync_all())
                .map_err(err)?;
            std::fs::File::open(&self.directory)
                .and_then(|dir| dir.sync_all())
                .map_err(err)?;
        }
        self.unindexed.set(true);
        Ok(Location {
            length: bytes.len() as u64,
            ..cursor
        })
    }
    pub fn pending(&self) -> Result<Option<(Vec<u8>, Location)>> {
        let mut cursor = self.cursor()?;
        let path = self.segment(cursor.segment);
        if !path.exists() {
            if cursor.segment == 0 && cursor.offset == 40 {
                return Ok(None);
            }
            return Err("missing block segment".into());
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(err)?;
        self.repair_new_header(&mut file, cursor)?;
        if let Some(bytes) = self.read_frame(&mut file, cursor.offset)? {
            return Ok(Some((
                bytes.clone(),
                Location {
                    length: bytes.len() as u64,
                    ..cursor
                },
            )));
        }
        cursor.segment = cursor.segment.checked_add(1).ok_or("segment overflow")?;
        cursor.offset = 40;
        let path = self.segment(cursor.segment);
        if !path.exists() {
            return Ok(None);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(err)?;
        self.repair_new_header(&mut file, cursor)?;
        Ok(self.read_frame(&mut file, 40)?.map(|bytes| {
            let length = bytes.len() as u64;
            (bytes, Location { length, ..cursor })
        }))
    }
    pub fn file_progress(&self, location: Location) -> Result<()> {
        self.unindexed.set(false);
        let tx = self.begin()?;
        put(
            &tx,
            META,
            b"file_cursor",
            &Location {
                offset: location.end(),
                length: 0,
                ..location
            }
            .bytes(),
        )?;
        tx.commit().map_err(err)
    }
}
impl std::ops::Deref for Store {
    type Target = Reader;
    fn deref(&self) -> &Reader {
        &self.reader
    }
}
impl Reader {
    /// A view of the last commit.
    pub fn view(&self) -> Result<View<'_>> {
        Ok(View {
            tx: self.db.begin_read().map_err(err)?,
            reader: self,
        })
    }
    fn segment(&self, number: u32) -> PathBuf {
        self.directory
            .join("blocks")
            .join(format!("{number:08}.dat"))
    }
    fn check_magic(&self, file: &mut std::fs::File) -> Result<()> {
        file.seek(SeekFrom::Start(0)).map_err(err)?;
        let mut magic = [0; 40];
        file.read_exact(&mut magic).map_err(err)?;
        if &magic[..8] != MAGIC || magic[8..] != self.chain {
            return Err("block file genesis or format mismatch".into());
        }
        Ok(())
    }
    /// Where a stored block's frame is.
    pub fn location(&self, id: &Hash) -> Result<Location> {
        self.view()?
            .location(id)?
            .ok_or_else(|| "block not found".into())
    }
    pub fn block(&self, id: &Hash) -> Result<Block> {
        self.view()?.block(id)
    }
    pub fn block_bytes(&self, id: &Hash) -> Result<Vec<u8>> {
        self.view()?.block_bytes(id)
    }
    pub fn succeeded_above(&self, id: Hash, height: u64) -> Result<bool> {
        self.view()?.succeeded_above(id, height)
    }
    pub fn included_above(&self, id: Hash, height: u64) -> Result<bool> {
        self.view()?.included_above(id, height)
    }
    pub fn occurrences(&self, id: Hash, from: u64, limit: usize) -> Result<Vec<(Hash, usize)>> {
        self.view()?.occurrences(id, from, limit)
    }
}
/// A view reads the state tree as it was at its commit, for proofs.
impl StateReader for View<'_> {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>> {
        View::record(self, key)
    }
    fn node(&self, position: &Position) -> Result<Option<Entry>> {
        View::node(self, position)
    }
}
impl View<'_> {
    /// The ChainId of the stored chain.
    pub fn chain(&self) -> Hash {
        self.reader.chain
    }
    /// The genesis block's id.
    pub fn genesis(&self) -> Hash {
        self.reader.genesis
    }
    fn meta_hash(&self, key: &[u8]) -> Result<Hash> {
        hash_bytes(
            self.tx
                .open_table(META)
                .map_err(err)?
                .get(key)
                .map_err(err)?
                .ok_or("missing database metadata")?
                .value(),
        )
    }
    /// The canonical tip.
    pub fn tip(&self) -> Result<Hash> {
        self.meta_hash(b"tip")
    }
    /// The state root at the canonical tip.
    pub fn root(&self) -> Result<Hash> {
        self.meta_hash(b"root")
    }
    /// The height at or below which canonical blocks are settled.
    pub fn floor(&self) -> Result<u64> {
        floor_of(&self.tx)
    }
    /// A settled block's active limits.
    pub fn limits(&self, id: &Hash) -> Result<Option<Resources>> {
        let table = self.tx.open_table(LIMITS).map_err(err)?;
        let Some(row) = table.get(id.as_slice()).map_err(err)? else {
            return Ok(None);
        };
        let bytes = row.value();
        if bytes.len() != 32 {
            return Err("invalid stored limits".into());
        }
        let mut limits = Resources([0; 4]);
        for (i, value) in bytes.as_chunks::<8>().0.iter().enumerate() {
            limits.0[i] = u64::from_le_bytes(*value);
        }
        Ok(Some(limits))
    }
    /// The canonical block at `height`, counted from 1.
    pub fn canonical(&self, height: u64) -> Result<Option<Hash>> {
        self.tx
            .open_table(CANONICAL)
            .map_err(err)?
            .get(height)
            .map_err(err)?
            .map(|id| hash_bytes(id.value()))
            .transpose()
    }
    /// A stored block's header, with its status and accumulated work.
    pub fn stored(&self, id: &Hash) -> Result<Option<Stored>> {
        self.tx
            .open_table(HEADERS)
            .map_err(err)?
            .get(id.as_slice())
            .map_err(err)?
            .map(|row| decode_stored(*id, row.value(), self.reader.chain))
            .transpose()
    }
    /// Where a stored block's frame is.
    pub fn location(&self, id: &Hash) -> Result<Option<Location>> {
        self.tx
            .open_table(BLOCKS)
            .map_err(err)?
            .get(id.as_slice())
            .map_err(err)?
            .map(|row| decode_location(row.value()))
            .transpose()
    }
    /// Up to `count` canonical headers from height `from` on, in their
    /// encoding as stored, each checked against its id.
    pub fn canonical_header_bytes(&self, from: u64, count: usize) -> Result<Vec<[u8; HEADER_LEN]>> {
        let canonical = self.tx.open_table(CANONICAL).map_err(err)?;
        let headers = self.tx.open_table(HEADERS).map_err(err)?;
        let mut page = Vec::new();
        for row in canonical.range(from..).map_err(err)?.take(count) {
            let (_, id) = row.map_err(err)?;
            let id = hash_bytes(id.value())?;
            let row = headers
                .get(id.as_slice())
                .map_err(err)?
                .ok_or("missing canonical header")?;
            let header: [u8; HEADER_LEN] = row
                .value()
                .get(2..2 + HEADER_LEN)
                .and_then(|h| h.try_into().ok())
                .ok_or("truncated stored header")?;
            if hash("block", &header) != id {
                return Err("stored header identity mismatch".into());
            }
            page.push(header);
        }
        Ok(page)
    }
    /// Whether the operator's marks leave a stored block valid: neither it nor
    /// any ancestor is marked. Its own branch is walked down to the canonical
    /// one, and a marked canonical block below that, as while the node is
    /// still leaving it, counts too.
    pub fn unmarked(&self, id: &Hash) -> Result<bool> {
        let marks = self.tx.open_table(MARKS).map_err(err)?;
        if marks.is_empty().map_err(err)? {
            return Ok(true);
        }
        let mut marked = Vec::new();
        for row in marks.iter().map_err(err)? {
            marked.push(hash_bytes(row.map_err(err)?.0.value())?);
        }
        let mut at = *id;
        let height = loop {
            if marked.contains(&at) {
                return Ok(false);
            }
            if at == self.reader.genesis {
                return Ok(true);
            }
            let stored = self.stored(&at)?.ok_or("missing stored ancestor")?;
            if self.canonical(stored.header.height)? == Some(at) {
                break stored.header.height;
            }
            at = stored.header.parent;
        };
        for mark in marked {
            if let Some(stored) = self.stored(&mark)?
                && stored.header.height <= height
                && self.canonical(stored.header.height)? == Some(mark)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// A stored block's bytes for serving in slices: one larger than a slice
    /// is kept among the last read ([`Recent`]).
    pub fn served_bytes(&self, id: &Hash) -> Result<Arc<Vec<u8>>> {
        let recent = || {
            self.reader
                .recent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        };
        if let Some(bytes) = recent().get(id) {
            return Ok(bytes);
        }
        let bytes = Arc::new(self.block_bytes(id)?);
        if bytes.len() > crate::chain::SLICE_MAX {
            recent().keep(*id, bytes.clone());
        }
        Ok(bytes)
    }
    /// The stored state-tree subtree at a position, at the canonical tip.
    pub fn node(&self, position: &Position) -> Result<Option<Entry>> {
        self.tx
            .open_table(NODES)
            .map_err(err)?
            .get(position_bytes(position).as_slice())
            .map_err(err)?
            .map(|v| decode_entry(v.value()))
            .transpose()
    }
    /// The value of a state record at the canonical tip.
    pub fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>> {
        let records = self.tx.open_table(RECORDS).map_err(err)?;
        match records.get(key.as_slice()).map_err(err)? {
            Some(row) => {
                let parts = self.tx.open_table(PARTS).map_err(err)?;
                row_value(&parts, RECORD_ROWS.tag, key, row.value())
            }
            None => Ok(None),
        }
    }
    pub fn block(&self, id: &Hash) -> Result<Block> {
        let bytes = self.block_bytes(id)?;
        let block = Block::decode(&bytes, bytes.len()).map_err(err)?;
        if block.header.id() != *id {
            return Err("stored block identity mismatch".into());
        }
        Ok(block)
    }
    /// A stored block's bytes, checked against their checksum and their
    /// header against the id, without decoding the body.
    pub fn block_bytes(&self, id: &Hash) -> Result<Vec<u8>> {
        let location = self.location(id)?.ok_or("block not found")?;
        let mut file = OpenOptions::new()
            .read(true)
            .open(self.reader.segment(location.segment))
            .map_err(err)?;
        self.reader.check_magic(&mut file)?;
        file.seek(SeekFrom::Start(location.offset)).map_err(err)?;
        let mut length = [0; 8];
        file.read_exact(&mut length).map_err(err)?;
        if u64::from_le_bytes(length) != location.length || !frame_length_ok(location.length) {
            return Err("stored block length mismatch".into());
        }
        let mut bytes = vec![0; usize::try_from(location.length).map_err(err)?];
        let mut checksum = [0; 32];
        file.read_exact(&mut bytes)
            .and_then(|()| file.read_exact(&mut checksum))
            .map_err(err)?;
        if checksum != hash("node-block", &bytes) {
            return Err("corrupt block checksum".into());
        }
        let header = bytes
            .get(..ting_core::protocol::HEADER_LEN)
            .ok_or("stored block identity mismatch")?;
        if Header::decode(header).map_err(err)?.id() != *id {
            return Err("stored block identity mismatch".into());
        }
        Ok(bytes)
    }
    /// A transaction's canonical occurrences, above `above` if given, as
    /// block and position, in order of height.
    fn canonical_occurrences(
        &self,
        id: Hash,
        above: Option<u64>,
    ) -> Result<impl Iterator<Item = Result<(Hash, usize)>>> {
        let table = self.tx.open_table(OCCURRENCES).map_err(err)?;
        let canonical = self.tx.open_table(CANONICAL).map_err(err)?;
        let mut low = id.to_vec();
        match above {
            Some(height) => {
                low.extend(height.saturating_add(1).to_be_bytes());
                low.extend([0; 36]);
            }
            None => low.extend([0; 44]),
        }
        let mut high = id.to_vec();
        high.extend([255; 44]);
        let rows = table.range(low.as_slice()..=high.as_slice()).map_err(err)?;
        Ok(rows.filter_map(move |row| {
            let found = (|| -> Result<Option<(Hash, usize)>> {
                let (key, _) = row.map_err(err)?;
                let key = key.value();
                if key.len() != 76 {
                    return Err("invalid occurrence key".into());
                }
                let height = u64::from_be_bytes(key[32..40].try_into().unwrap());
                let block = hash_bytes(&key[40..72])?;
                let index = u32::from_be_bytes(key[72..76].try_into().unwrap()) as usize;
                let on = canonical
                    .get(height)
                    .map_err(err)?
                    .is_some_and(|v| v.value() == block);
                Ok(on.then_some((block, index)))
            })();
            found.transpose()
        }))
    }
    /// Whether a canonical block above `height` includes the transaction with
    /// a successful receipt.
    pub fn succeeded_above(&self, id: Hash, height: u64) -> Result<bool> {
        for found in self.canonical_occurrences(id, Some(height))? {
            let (block, index) = found?;
            if self
                .block(&block)?
                .receipts
                .get(index)
                .is_some_and(|r| r.status == ting_core::protocol::Status::Success)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// Whether a canonical block above `height` includes the transaction.
    pub fn included_above(&self, id: Hash, height: u64) -> Result<bool> {
        Ok(self
            .canonical_occurrences(id, Some(height))?
            .next()
            .transpose()?
            .is_some())
    }
    /// A page of a transaction's canonical occurrences, as block and
    /// position: up to `limit` of them after the first `from`.
    pub fn occurrences(&self, id: Hash, from: u64, limit: usize) -> Result<Vec<(Hash, usize)>> {
        self.canonical_occurrences(id, None)?
            .skip(usize::try_from(from).unwrap_or(usize::MAX))
            .take(limit)
            .collect()
    }
}
/// The rows of one block in an undo or redo table, by record key.
fn rows(
    table: &impl ReadableTable<&'static [u8], &'static [u8]>,
    parts: &impl ReadableTable<&'static [u8], &'static [u8]>,
    tag: u8,
    id: Hash,
) -> Result<Vec<(Hash, Option<Vec<u8>>)>> {
    let low = [&id[..], &[0; 32]].concat();
    let high = [&id[..], &[255; 32]].concat();
    let mut result = Vec::new();
    for row in table.range(low.as_slice()..=high.as_slice()).map_err(err)? {
        let (key, value) = row.map_err(err)?;
        if key.value().len() != 64 {
            return Err("invalid change key".into());
        }
        let record = row_value(parts, tag, key.value(), value.value())?;
        result.push((hash_bytes(&key.value()[32..])?, record));
    }
    Ok(result)
}
/// A block's frame at `location` among the block files of the data
/// directory `dir`, checked against its checksum, read without the database.
pub fn frame_at(dir: &Path, location: Location) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(
            dir.join("blocks")
                .join(format!("{:08}.dat", location.segment)),
        )
        .map_err(err)?;
    file.seek(SeekFrom::Start(location.offset)).map_err(err)?;
    let mut length = [0; 8];
    file.read_exact(&mut length).map_err(err)?;
    if u64::from_le_bytes(length) != location.length {
        return Err("stored block length mismatch".into());
    }
    let mut bytes = vec![0; usize::try_from(location.length).map_err(err)?];
    let mut checksum = [0; 32];
    file.read_exact(&mut bytes)
        .and_then(|()| file.read_exact(&mut checksum))
        .map_err(err)?;
    if checksum != hash("node-block", &bytes) {
        return Err("corrupt block checksum".into());
    }
    Ok(bytes)
}
fn decode_location(bytes: &[u8]) -> Result<Location> {
    if bytes.len() != 20 {
        return Err("invalid block location".into());
    }
    let location = Location {
        segment: u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        offset: u64::from_le_bytes(bytes[4..12].try_into().unwrap()),
        length: u64::from_le_bytes(bytes[12..].try_into().unwrap()),
    };
    // A cursor, of length 0, may follow a frame larger than a file's usual
    // size, alone in its own file.
    if location.offset < 40
        || (location.length != 0 && !frame_fits(location.offset, location.length))
    {
        return Err("invalid block location bounds".into());
    }
    Ok(location)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_blocks_are_bounded_and_keep_the_last_one() {
        let mut recent = Recent::default();
        let block = |n: usize| Arc::new(vec![0; n]);
        recent.keep([1; 32], block(40 << 20));
        recent.keep([2; 32], block(20 << 20));
        assert_eq!(recent.bytes, 60 << 20);
        // A third over the bound pushes out the oldest.
        recent.keep([3; 32], block(10 << 20));
        assert!(recent.get(&[1; 32]).is_none());
        assert_eq!(recent.bytes, 30 << 20);
        // One larger than the bound alone is still kept, as the only one.
        recent.keep([4; 32], block(100 << 20));
        assert_eq!(recent.blocks.len(), 1);
        assert_eq!(recent.get(&[4; 32]).unwrap().len(), 100 << 20);
        recent.forget(&[4; 32]);
        assert_eq!((recent.blocks.len(), recent.bytes), (0, 0));
    }

    #[test]
    fn a_frame_larger_than_a_block_file_takes_a_file_of_its_own() {
        let dir = std::env::temp_dir().join(format!("ting-segments-{}", crate::discovery::seed()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, &crate::config::development_genesis(), CACHE_BYTES).unwrap();
        let frame = |n: usize, byte: u8| vec![byte; n];
        let stored = |bytes: &[u8]| {
            let location = store.append(bytes).unwrap();
            store.file_progress(location).unwrap();
            location
        };
        // Files are 64 KiB in tests: a small frame, one of 100 KiB, another
        // small one, then 100 KiB again left unindexed, as by a crash.
        let first = stored(&frame(1_000, 1));
        let large = stored(&frame(100 * 1024, 2));
        let after = stored(&frame(1_000, 3));
        assert_eq!((first.segment, first.offset), (0, 40));
        assert_eq!((large.segment, large.offset), (1, 40));
        assert_eq!((after.segment, after.offset), (2, 40));
        assert_eq!(frame_at(&dir, large).unwrap(), frame(100 * 1024, 2));
        assert!(decode_location(&large.bytes()).is_ok());
        assert!(
            decode_location(
                &Location {
                    offset: 41,
                    ..large
                }
                .bytes()
            )
            .is_err(),
            "only the first frame of a file may exceed its usual size"
        );
        let crashed = store.append(&frame(100 * 1024, 4)).unwrap();
        assert_eq!((crashed.segment, crashed.offset), (3, 40));
        let (bytes, found) = store.pending().unwrap().unwrap();
        assert_eq!(bytes, frame(100 * 1024, 4));
        assert_eq!((found.segment, found.length), (3, 100 * 1024));
        drop(store);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
