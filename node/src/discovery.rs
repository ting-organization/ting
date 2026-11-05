//! Peer discovery: an address book filled from the configured peers, from the
//! listening addresses inbound peers announce, and from the addresses peers
//! share on request. This is node policy; consensus never sees it.
//!
//! An address is shared only after this node has completed a round with it,
//! and never with a peer that could not reach it: loopback addresses stay on
//! the machine and private ranges stay off the public internet. The outbound
//! peers a node chooses come from distinct network groups, so one operator's
//! address range cannot fill them all, and no one address or network holds
//! more than its share of the book. Every peer asking for addresses is given
//! the same answer for an hour. A peer that sent invalid data is banned
//! for a day, with its address range: its addresses are dropped, a seed's
//! aside, and not learned again, it may not connect, and its open connections
//! end.
use crate::{Result, config::atomic_write};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
};

/// Addresses in one answer, and the most accepted from one answer.
pub const SHARE_MAX: usize = 32;
/// Addresses the book keeps.
pub const BOOK_MAX: usize = 1024;
/// Discovered addresses the book keeps of one public IP address, room for a
/// node and a second one, or a port it moved to; and of one network group,
/// a 32nd of the book, as Bitcoin's tried table keeps one group to 8 of its
/// 256 buckets: an attacker needs 32 networks to fill it.
const PER_IP: usize = 2;
const PER_GROUP: usize = BOOK_MAX / 32;
/// Addresses one network group may add to the book in an hour, announcing
/// its own or sharing others: one answer's worth, as many as an honest peer's
/// answer changes by in that time.
const ADDED_PER_HOUR: usize = SHARE_MAX;
const HOUR: u64 = 3600;
/// An address is shared while its last completed round is this recent.
const SHARE_FRESH: u64 = 24 * 3600;
/// Every peer is given the same answer for an hour: asking again tells
/// nothing new, so a peer cannot map the book or this node's connections by
/// asking often. Bitcoin keeps its answer about a day, beside the addresses
/// its peers relay as they hear of them; here answers are the only way
/// addresses spread, so a new node becomes known within hours, not a day.
const SHARE_KEPT: u64 = HOUR;
/// A failed address waits a minute, doubling with each further failure up to an hour.
const RETRY_BASE: u64 = 60;
const RETRY_MAX: u64 = 3600;
/// How long a peer that sent invalid data stays banned.
pub const BAN: u64 = 24 * 3600;
/// Most bans kept at once; past it, the ban that ends soonest gives way.
pub const BAN_MAX: usize = 4_096;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Listed in the configuration.
    Config,
    /// Announced by a peer that connected to this node.
    Inbound,
    /// Shared by a peer on request.
    Peer,
    /// A seed node of the production network.
    Seed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub address: SocketAddr,
    pub source: Source,
    /// When a round with it last completed.
    #[serde(default)]
    pub verified: Option<u64>,
    /// When a connection to it was last started.
    #[serde(default)]
    pub attempted: u64,
    /// Connections failed since a round last completed.
    #[serde(default)]
    pub failures: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    addresses: Vec<Entry>,
}
/// Whether an address was discovered, from a peer, rather than configured or
/// a seed's: only discovered ones are limited, and give way.
fn discovered(entry: &Entry) -> bool {
    matches!(entry.source, Source::Inbound | Source::Peer)
}

#[derive(Default)]
pub struct AddressBook {
    entries: BTreeMap<SocketAddr, Entry>,
    path: Option<PathBuf>,
    dirty: bool,
    /// Banned ranges, until when, and the same by when they end; kept for
    /// this run only.
    banned: BTreeMap<IpAddr, u64>,
    ban_ends: BTreeSet<(u64, IpAddr)>,
    /// The `addresses` answer for peers of each reach.
    shared: BTreeMap<Reach, Shared>,
    /// By network group, when its hour of adding addresses began and how
    /// many it has added since.
    added: BTreeMap<IpAddr, (u64, usize)>,
    /// Which of the addresses alike gives way when one must.
    random: Shuffle,
}

/// An `addresses` answer, when it was drawn, and whether it holds every
/// address there was to share then.
struct Shared {
    drawn: u64,
    list: Vec<SocketAddr>,
    whole: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Reach {
    Unusable,
    Loopback,
    Private,
    Public,
}
fn reach(ip: IpAddr) -> Reach {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            if ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
            {
                Reach::Unusable
            } else if ip.is_loopback() {
                Reach::Loopback
            } else if ip.is_private() || ip.is_link_local() || (o[0] == 100 && o[1] & 0xc0 == 64) {
                Reach::Private
            } else {
                Reach::Public
            }
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return reach(IpAddr::V4(v4));
            }
            let first = ip.segments()[0];
            if ip.is_unspecified()
                || ip.is_multicast()
                || first == 0x2001 && ip.segments()[1] == 0x0db8
            {
                Reach::Unusable
            } else if ip.is_loopback() {
                Reach::Loopback
            } else if first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80 {
                Reach::Private
            } else {
                Reach::Public
            }
        }
    }
}
/// An address's range: an IPv4 address, or the /64 of an IPv6 address, the
/// block one subscriber usually holds whole. Bans and inbound limits count
/// by it.
pub fn range(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(ip) => {
            let s = ip.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        ip => ip,
    }
}
/// Whether an address is on this machine.
pub fn loopback(ip: IpAddr) -> bool {
    reach(ip) == Reach::Loopback
}
/// Whether an address names a reachable listener at all.
pub fn usable(address: SocketAddr) -> bool {
    address.port() != 0 && reach(address.ip()) != Reach::Unusable
}
/// Whether `address` may be shared with, or accepted from, a peer at `peer`:
/// public addresses with anyone, private ones only off the public internet,
/// loopback ones only on the same machine.
pub fn shareable(address: SocketAddr, peer: IpAddr) -> bool {
    match reach(address.ip()) {
        Reach::Public => true,
        Reach::Private => reach(peer) != Reach::Public,
        Reach::Loopback => reach(peer) == Reach::Loopback,
        Reach::Unusable => false,
    }
}
/// The network group of a public address (an IPv4 /16, an IPv6 /32), from
/// which a node keeps at most one discovered peer, and by which inbound
/// connections give way to newcomers. Local addresses have none.
pub fn group(ip: IpAddr) -> Option<IpAddr> {
    if reach(ip) != Reach::Public {
        return None;
    }
    Some(match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            IpAddr::V4(Ipv4Addr::new(o[0], o[1], 0, 0))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], 0, 0, 0, 0, 0, 0))
        }
    })
}

/// A small generator for sampling; `seed` comes from the operating system.
struct Shuffle(u64);
/// A fixed seed, so that a book made in a test gives way the same each run;
/// a node's own book is seeded from the operating system.
impl Default for Shuffle {
    fn default() -> Self {
        Self(0x9e37_79b9_7f4a_7c15)
    }
}
impl Shuffle {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}
pub fn seed() -> u64 {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).expect("operating system randomness");
    u64::from_le_bytes(bytes) | 1
}

impl AddressBook {
    /// The book kept in `peers.json` of a data directory; a missing or
    /// unreadable file starts it empty, since it only holds hints.
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("peers.json");
        let stored: Stored = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut book = Self {
            path: Some(path),
            random: Shuffle(seed()),
            ..Self::default()
        };
        // Best first, so that a book saved past the shares of one address
        // and one network keeps what is worth keeping: the configured peers
        // and the seeds, then the most recently proven, then the least failed.
        let mut addresses = stored.addresses;
        addresses.sort_by_key(|e| (discovered(e), std::cmp::Reverse(e.verified), e.failures));
        for entry in addresses {
            if book.entries.len() >= BOOK_MAX {
                break;
            }
            if usable(entry.address)
                && !book.entries.contains_key(&entry.address)
                && (!discovered(&entry) || book.fits(entry.address))
            {
                book.entries.insert(entry.address, entry);
            }
        }
        book
    }
    pub fn save(&mut self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        let stored = Stored {
            addresses: self.entries.values().cloned().collect(),
        };
        atomic_write(
            path,
            &serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?,
        )?;
        self.dirty = false;
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// The configured peers the book holds.
    pub fn configured(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.source == Source::Config)
            .count()
    }
    pub fn get(&self, address: &SocketAddr) -> Option<&Entry> {
        self.entries.get(address)
    }
    /// Marks exactly `peers` as the configured ones; an address no longer
    /// configured is forgotten, so the node does not connect to it again
    /// unless a peer shares it later.
    pub fn configure(&mut self, peers: &[SocketAddr]) {
        let before = self.entries.len();
        self.entries
            .retain(|a, e| e.source != Source::Config || peers.contains(a));
        self.dirty |= self.entries.len() != before;
        for address in peers {
            // A configured peer is never banned: configuring one lifts a ban
            // on its range from before.
            let range = range(address.ip());
            if let Some(until) = self.banned.remove(&range) {
                self.ban_ends.remove(&(until, range));
            }
            match self.entries.get_mut(address) {
                Some(entry) if entry.source != Source::Config => {
                    entry.source = Source::Config;
                    self.dirty = true;
                }
                Some(_) => {}
                None => {
                    self.insert(*address, Source::Config);
                }
            }
        }
    }
    /// Adds an address the book does not know; returns whether it did. A
    /// discovered one takes its place within the share of its IP address
    /// and network group.
    pub fn learn(&mut self, address: SocketAddr, source: Source) -> bool {
        if !usable(address)
            || self.entries.contains_key(&address)
            || self.is_banned(address.ip(), crate::now())
        {
            return false;
        }
        if matches!(source, Source::Inbound | Source::Peer) && !self.make_room(address) {
            return false;
        }
        self.insert(address, source)
    }
    /// Adds the listening address a peer that connected announced, at the IP
    /// address it came from, unless its network group has added its hour's
    /// worth; returns whether it did.
    pub fn learn_announced(&mut self, address: SocketAddr, now: u64) -> bool {
        let source = group(address.ip());
        if !self.may_add(source, now) || !self.learn(address, Source::Inbound) {
            return false;
        }
        self.count_added(source, now);
        true
    }
    /// Whether the discovered addresses of `address`'s IP address and network
    /// group leave room for it. Local addresses have no share: they come only
    /// from this machine or its own network.
    fn fits(&self, address: SocketAddr) -> bool {
        let Some(own) = group(address.ip()) else {
            return true;
        };
        let ip = address.ip().to_canonical();
        let (mut same_ip, mut same_group) = (0, 0);
        for e in self.entries.values().filter(|e| discovered(e)) {
            if group(e.address.ip()) == Some(own) {
                same_group += 1;
                same_ip += usize::from(e.address.ip().to_canonical() == ip);
            }
        }
        same_ip < PER_IP && same_group < PER_GROUP
    }
    /// Makes room for `address` within the shares of its IP address and
    /// network group if either is full: the least worth keeping of that
    /// address or group gives way, never a proven one. Returns whether there
    /// is room.
    fn make_room(&mut self, address: SocketAddr) -> bool {
        let Some(own) = group(address.ip()) else {
            return true;
        };
        let ip = address.ip().to_canonical();
        let same_ip = move |e: &Entry| e.address.ip().to_canonical() == ip;
        let same_group = move |e: &Entry| group(e.address.ip()) == Some(own);
        if self.held(same_ip) >= PER_IP && !self.give_way(same_ip, false) {
            return false;
        }
        if self.held(same_group) >= PER_GROUP && !self.give_way(same_group, false) {
            return false;
        }
        self.fits(address)
    }
    /// How many discovered addresses `same` picks.
    fn held(&self, same: impl Fn(&Entry) -> bool) -> usize {
        self.entries
            .values()
            .filter(|e| discovered(e) && same(e))
            .count()
    }
    /// Whether network group `source` may add another address at `now`: it
    /// has not added `ADDED_PER_HOUR` in the hour since its first. Local
    /// sources have no limit.
    fn may_add(&self, source: Option<IpAddr>, now: u64) -> bool {
        let Some(source) = source else {
            return true;
        };
        self.added.get(&source).is_none_or(|&(began, count)| {
            now < began || now - began >= HOUR || count < ADDED_PER_HOUR
        })
    }
    /// Counts an address network group `source` added at `now`. The groups
    /// counted are at most as many as the book holds: past that, the one
    /// whose hour began first gives way.
    fn count_added(&mut self, source: Option<IpAddr>, now: u64) {
        let Some(source) = source else {
            return;
        };
        if !self.added.contains_key(&source) && self.added.len() >= BOOK_MAX {
            self.added
                .retain(|_, &mut (began, _)| now >= began && now - began < HOUR);
            let first = self.added.iter().min_by_key(|(_, (began, _))| *began);
            if self.added.len() >= BOOK_MAX
                && let Some((&first, _)) = first
            {
                self.added.remove(&first);
            }
        }
        let (began, count) = self.added.entry(source).or_insert((now, 0));
        if now < *began || now - *began >= HOUR {
            (*began, *count) = (now, 0);
        }
        *count = count.saturating_add(1);
    }
    /// Adds a seed node's address, or marks a known one as a seed's unless it
    /// is configured: a seed is where a node starts, picked after every other
    /// address. A banned one is kept too, to be connected once the ban ends.
    pub fn learn_seed(&mut self, address: SocketAddr) {
        match self.entries.get_mut(&address) {
            Some(entry) if !matches!(entry.source, Source::Config | Source::Seed) => {
                entry.source = Source::Seed;
                self.dirty = true;
            }
            Some(_) => {}
            None => {
                if usable(address) {
                    self.insert(address, Source::Seed);
                }
            }
        }
    }
    pub fn is_seed(&self, address: &SocketAddr) -> bool {
        self.entries
            .get(address)
            .is_some_and(|e| e.source == Source::Seed)
    }
    /// Whether the node may connect to one of the seed nodes' addresses at
    /// `now`: one that is not banned, and not waiting after failures.
    pub fn seed_due(&self, now: u64) -> bool {
        self.seeds().any(|a| self.due(&a, now))
    }
    /// The seed nodes' addresses.
    pub fn seeds(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.entries
            .values()
            .filter(|e| e.source == Source::Seed)
            .map(|e| e.address)
    }
    /// Bans an address's range until `until`, dropping every address in it but
    /// a seed's, which is not connected until the ban ends; returns whether it
    /// did. A range holding a configured peer is never banned, nor is this
    /// machine: banning it would ban every node on it.
    pub fn ban(&mut self, ip: IpAddr, until: u64) -> bool {
        let range = range(ip);
        if loopback(ip)
            || self
                .entries
                .values()
                .any(|e| e.source == Source::Config && self::range(e.address.ip()) == range)
        {
            return false;
        }
        let now = until.saturating_sub(BAN);
        while let Some(&(end, ended)) = self.ban_ends.first()
            && end <= now
        {
            self.ban_ends.pop_first();
            self.banned.remove(&ended);
        }
        let until = match self.banned.get(&range) {
            Some(&old) => {
                self.ban_ends.remove(&(old, range));
                old.max(until)
            }
            None => {
                if self.banned.len() >= BAN_MAX
                    && let Some((_, soonest)) = self.ban_ends.pop_first()
                {
                    self.banned.remove(&soonest);
                }
                until
            }
        };
        self.banned.insert(range, until);
        self.ban_ends.insert((until, range));
        let before = self.entries.len();
        self.entries
            .retain(|a, e| e.source == Source::Seed || self::range(a.ip()) != range);
        self.dirty |= self.entries.len() != before;
        for shared in self.shared.values_mut() {
            shared.list.retain(|a| self::range(a.ip()) != range);
        }
        true
    }
    pub fn is_banned(&self, ip: IpAddr, now: u64) -> bool {
        self.banned
            .get(&range(ip))
            .is_some_and(|until| *until > now)
    }
    /// Adds the addresses one peer shared: at most `SHARE_MAX` of them, only
    /// those the peer could legitimately know about from where it is, and
    /// none once the peer's network group has added its hour's worth.
    pub fn learn_shared(&mut self, addresses: &[SocketAddr], from: IpAddr, now: u64) -> usize {
        let source = group(from);
        let mut learned = 0;
        for address in addresses.iter().take(SHARE_MAX) {
            if !self.may_add(source, now) {
                break;
            }
            if shareable(*address, from) && self.learn(*address, Source::Peer) {
                self.count_added(source, now);
                learned += 1;
            }
        }
        learned
    }
    fn insert(&mut self, address: SocketAddr, source: Source) -> bool {
        if self.entries.len() >= BOOK_MAX && !self.evict() {
            return false;
        }
        self.entries.insert(
            address,
            Entry {
                address,
                source,
                verified: None,
                attempted: 0,
                failures: 0,
            },
        );
        self.dirty = true;
        true
    }
    /// Drops the least useful address: never a configured one or a seed's, an
    /// unproven one before a proven one, the most failed and longest unproven
    /// first. A seed's is proven no more once the node leaves it, and it is
    /// where the node goes back to.
    fn evict(&mut self) -> bool {
        self.give_way(|_| true, true)
    }
    /// Drops the least useful of the discovered addresses `among` picks, and
    /// only an unproven one unless `proven`: an unproven one before a proven
    /// one, of unproven ones the most failed first, of proven ones the one
    /// proven longest ago, and of those alike one at random, so that no
    /// address can be chosen to stay. Returns whether one was dropped.
    fn give_way(&mut self, among: impl Fn(&Entry) -> bool, proven: bool) -> bool {
        let rank = |e: &Entry| {
            (
                e.verified.is_some(),
                e.verified.unwrap_or(0),
                u32::MAX - e.failures,
            )
        };
        let candidates: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| discovered(e) && among(e) && (proven || e.verified.is_none()))
            .collect();
        let Some(worst) = candidates.iter().map(|e| rank(e)).min() else {
            return false;
        };
        let alike: Vec<SocketAddr> = candidates
            .iter()
            .filter(|e| rank(e) == worst)
            .map(|e| e.address)
            .collect();
        let gone = alike[self.random.below(alike.len())];
        self.entries.remove(&gone);
        self.dirty = true;
        true
    }
    pub fn forget(&mut self, address: &SocketAddr) {
        if self.entries.remove(address).is_some() {
            self.dirty = true;
        }
    }
    pub fn attempted(&mut self, address: SocketAddr, now: u64) {
        if let Some(entry) = self.entries.get_mut(&address) {
            entry.attempted = now;
            self.dirty = true;
        }
    }
    pub fn succeeded(&mut self, address: SocketAddr, now: u64) {
        if let Some(entry) = self.entries.get_mut(&address) {
            entry.verified = Some(now);
            entry.failures = 0;
            self.dirty = true;
        }
    }
    pub fn failed(&mut self, address: SocketAddr) {
        if let Some(entry) = self.entries.get_mut(&address) {
            entry.failures = entry.failures.saturating_add(1);
            self.dirty = true;
        }
    }
    /// Whether a new connection with a discovered address may be tried.
    fn retry(entry: &Entry, now: u64) -> bool {
        entry.failures == 0
            || now
                >= entry
                    .attempted
                    .saturating_add((RETRY_BASE << (entry.failures - 1).min(6)).min(RETRY_MAX))
    }
    /// Whether a connection to an address may be tried: at once, or after a
    /// failure once the wait is over, and not while it is banned. A configured
    /// peer waits a minute, any other a minute doubling with each further
    /// failure up to an hour.
    pub fn due(&self, address: &SocketAddr, now: u64) -> bool {
        !self.is_banned(address.ip(), now)
            && self.entries.get(address).is_none_or(|e| {
                if e.source == Source::Config {
                    e.failures == 0 || now >= e.attempted.saturating_add(RETRY_BASE)
                } else {
                    Self::retry(e, now)
                }
            })
    }
    /// Up to `SHARE_MAX` addresses, in random order, for a peer at `peer`:
    /// those with a recent completed round that the peer can reach, except
    /// the peer's own announced address. Every peer that can reach the same
    /// is given the same, drawn at most `SHARE_KEPT` before `now`; a list
    /// that held every address there was to share is drawn again as soon as
    /// there is another.
    pub fn share(
        &mut self,
        peer: IpAddr,
        own: Option<SocketAddr>,
        now: u64,
        seed: u64,
    ) -> Vec<SocketAddr> {
        let mut fresh: Vec<_> = self
            .entries
            .values()
            .filter(|e| {
                e.verified
                    .is_some_and(|v| now.saturating_sub(v) <= SHARE_FRESH)
            })
            .filter(|e| e.failures == 0 && shareable(e.address, peer))
            .filter(|e| !self.is_banned(e.address.ip(), now))
            .map(|e| e.address)
            .collect();
        let kept = self.shared.get(&reach(peer)).filter(|kept| {
            now >= kept.drawn
                && now - kept.drawn < SHARE_KEPT
                && !(kept.whole && fresh.iter().any(|a| !kept.list.contains(a)))
        });
        let list = match kept {
            Some(kept) => kept.list.clone(),
            None => {
                let whole = fresh.len() <= SHARE_MAX;
                Shuffle(seed).shuffle(&mut fresh);
                fresh.truncate(SHARE_MAX);
                let (drawn, list) = (now, fresh.clone());
                self.shared
                    .insert(reach(peer), Shared { drawn, list, whole });
                fresh
            }
        };
        list.into_iter().filter(|a| Some(*a) != own).collect()
    }
    /// Up to `want` addresses to connect to, besides `busy`: proven
    /// ones first, most recently proven first, then unproven ones in random
    /// order, and seeds after every other; each from a network group none of
    /// the others is in.
    pub fn pick(
        &self,
        want: usize,
        busy: &BTreeSet<SocketAddr>,
        now: u64,
        seed: u64,
    ) -> Vec<SocketAddr> {
        let mut groups: BTreeSet<IpAddr> = busy.iter().filter_map(|a| group(a.ip())).collect();
        let mut candidates: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| {
                e.source != Source::Config
                    && !busy.contains(&e.address)
                    && Self::retry(e, now)
                    && !self.is_banned(e.address.ip(), now)
            })
            .collect();
        Shuffle(seed).shuffle(&mut candidates);
        candidates.sort_by_key(|e| (e.source == Source::Seed, std::cmp::Reverse(e.verified)));
        let mut chosen = Vec::new();
        for entry in candidates {
            if chosen.len() == want {
                break;
            }
            if let Some(g) = group(entry.address.ip())
                && !groups.insert(g)
            {
                continue;
            }
            chosen.push(entry.address);
        }
        chosen
    }
    /// One unproven address, not a seed's, to test with a short connection
    /// when every connection slot is taken, so new nodes still become
    /// shareable.
    pub fn probe(&self, busy: &BTreeSet<SocketAddr>, now: u64, seed: u64) -> Option<SocketAddr> {
        let mut candidates: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.verified.is_none() && e.source != Source::Seed)
            .filter(|e| !busy.contains(&e.address) && Self::retry(e, now))
            .map(|e| e.address)
            .collect();
        Shuffle(seed).shuffle(&mut candidates);
        candidates.first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn addresses_travel_only_where_they_are_reachable() {
        let (public, private, cgnat, loopback) = (
            a("8.8.4.4:18442"),
            a("192.168.1.5:18442"),
            a("100.64.0.9:18442"),
            a("127.0.0.1:18442"),
        );
        for peer in ["9.9.9.9", "10.0.0.2", "127.0.0.1"] {
            assert!(shareable(public, ip(peer)), "{peer}");
        }
        assert!(!shareable(private, ip("9.9.9.9")) && !shareable(cgnat, ip("9.9.9.9")));
        assert!(shareable(private, ip("10.0.0.2")) && shareable(private, ip("127.0.0.1")));
        assert!(shareable(loopback, ip("127.0.0.1")) && !shareable(loopback, ip("10.0.0.2")));
        for bad in [
            "0.0.0.0:1",
            "224.0.0.1:1",
            "192.0.2.1:1",
            "8.8.8.8:0",
            "[2001:db8::1]:1",
            "[::]:1",
        ] {
            assert!(!usable(a(bad)), "{bad}");
        }
        assert!(usable(a("[2a00::1]:18442")) && !shareable(a("[fd00::1]:1"), ip("2a00::2")));
    }

    #[test]
    fn only_proven_fresh_addresses_are_shared_and_never_back_to_their_owner() {
        let mut book = AddressBook::default();
        let (x, y, z) = (a("8.1.0.1:1"), a("8.2.0.1:1"), a("8.3.0.1:1"));
        for address in [x, y, z] {
            book.learn(address, Source::Peer);
        }
        book.succeeded(x, 1000);
        book.succeeded(y, 1000);
        book.failed(y);
        assert_eq!(book.share(ip("9.9.9.9"), None, 1000, 7), vec![x]);
        assert!(book.share(ip("9.9.9.9"), Some(x), 1000, 7).is_empty());
        assert!(
            book.share(ip("9.9.9.9"), None, 1000 + SHARE_FRESH + 1, 7)
                .is_empty()
        );
        let many: Vec<_> = (0..100).map(|i| a(&format!("8.{i}.0.1:1"))).collect();
        for m in &many {
            book.learn(*m, Source::Peer);
            book.succeeded(*m, 1000);
        }
        assert_eq!(book.share(ip("9.9.9.9"), None, 1000, 7).len(), SHARE_MAX);
    }

    #[test]
    fn shared_lists_are_capped_and_filtered_by_the_sender() {
        let mut book = AddressBook::default();
        let sent: Vec<_> = (0..40).map(|i| a(&format!("8.8.{i}.1:1"))).collect();
        assert_eq!(book.learn_shared(&sent, ip("9.9.9.9"), 0), SHARE_MAX);
        let mut local = AddressBook::default();
        assert_eq!(
            local.learn_shared(&[a("10.0.0.1:1"), a("127.0.0.1:1")], ip("9.9.9.9"), 0),
            0
        );
        assert_eq!(
            local.learn_shared(&[a("10.0.0.1:1"), a("127.0.0.1:1")], ip("127.0.0.1"), 0),
            2
        );
    }

    #[test]
    fn picks_prefer_proven_peers_from_distinct_groups_and_back_off_failures() {
        let mut book = AddressBook::default();
        let (proven, same_group, other, failing) = (
            a("8.8.1.1:1"),
            a("8.8.2.2:1"),
            a("9.9.1.1:1"),
            a("7.7.1.1:1"),
        );
        for address in [proven, same_group, other, failing] {
            book.learn(address, Source::Peer);
        }
        book.succeeded(proven, 50);
        book.attempted(failing, 100);
        book.failed(failing);
        let chosen = book.pick(8, &BTreeSet::new(), 120, 3);
        assert_eq!(chosen[0], proven);
        assert!(
            chosen.contains(&other) && !chosen.contains(&same_group) && !chosen.contains(&failing)
        );
        // A minute after a failure the address may be tried again; after a
        // second failure it waits two.
        assert!(book.pick(8, &BTreeSet::new(), 160, 3).contains(&failing));
        book.attempted(failing, 160);
        book.failed(failing);
        assert!(!book.pick(8, &BTreeSet::new(), 270, 3).contains(&failing));
        assert!(book.pick(8, &BTreeSet::new(), 280, 3).contains(&failing));
        // Local addresses have no group limit.
        let mut local = AddressBook::default();
        for port in 1..=3 {
            local.learn(a(&format!("127.0.0.1:{port}")), Source::Inbound);
        }
        assert_eq!(local.pick(8, &BTreeSet::new(), 0, 3).len(), 3);
    }

    #[test]
    fn seeds_are_picked_after_every_other_address() {
        let mut book = AddressBook::default();
        let (seed, proven, unproven) = (a("8.8.1.1:1"), a("9.9.1.1:1"), a("7.7.1.1:1"));
        book.learn(seed, Source::Seed);
        book.learn(proven, Source::Peer);
        book.learn(unproven, Source::Inbound);
        // The seed was proven last, and still comes after an untried address.
        book.succeeded(proven, 50);
        book.succeeded(seed, 100);
        let none = BTreeSet::new();
        assert_eq!(book.pick(8, &none, 120, 3), vec![proven, unproven, seed]);
        assert_eq!(book.pick(2, &none, 120, 3), vec![proven, unproven]);
        // A node with nothing else to try picks the seeds.
        assert_eq!(
            book.pick(8, &BTreeSet::from([proven, unproven]), 120, 3),
            vec![seed]
        );
        // A seed's address known from a peer is a seed's all the same, unless
        // it is configured.
        let (other_seed, configured) = (a("6.6.1.1:1"), a("5.5.1.1:1"));
        book.learn(other_seed, Source::Inbound);
        book.succeeded(other_seed, 110);
        book.configure(&[configured]);
        for address in [other_seed, configured] {
            book.learn_seed(address);
        }
        assert!(book.is_seed(&other_seed) && !book.is_seed(&configured));
        assert_eq!(book.pick(8, &none, 120, 3)[2..], [other_seed, seed]);
        // A seed is never probed: it needs no proving to be shared.
        book.learn_seed(a("4.4.1.1:1"));
        let busy = BTreeSet::from([configured]);
        assert_eq!(book.probe(&busy, 120, 3), Some(unproven));
        let busy = BTreeSet::from([configured, unproven]);
        assert_eq!(book.probe(&busy, 120, 3), None);
    }

    #[test]
    fn a_full_book_evicts_unproven_addresses_never_configured_ones_or_seeds() {
        let mut book = AddressBook::default();
        book.configure(&[a("8.0.0.1:1")]);
        let proven = a("8.0.0.2:1");
        book.learn(proven, Source::Peer);
        book.succeeded(proven, 10);
        for i in 0..2000 {
            book.learn(
                a(&format!("{}.{}.0.1:1", 11 + i / 256, i % 256)),
                Source::Peer,
            );
        }
        assert_eq!(book.len(), BOOK_MAX);
        assert!(book.get(&a("8.0.0.1:1")).is_some() && book.get(&proven).is_some());
        book.configure(&[]);
        assert!(book.get(&a("8.0.0.1:1")).is_none());
        // A seed's address stays, however long ago it was proven, while
        // addresses proven since take each other's places.
        let mut book = AddressBook::default();
        let seed = a("8.0.0.3:1");
        book.learn_seed(seed);
        book.succeeded(seed, 1);
        for i in 0..2000 {
            let address = a(&format!("{}.{}.0.1:1", 11 + i / 256, i % 256));
            book.learn(address, Source::Peer);
            book.succeeded(address, 100);
        }
        assert_eq!(book.len(), BOOK_MAX);
        assert!(book.is_seed(&seed));
    }

    #[test]
    fn a_banned_address_is_dropped_and_not_learned_again_until_the_ban_ends() {
        let mut book = AddressBook::default();
        let (bad, other_port, good, configured) = (
            a("8.8.1.1:1"),
            a("8.8.1.1:2"),
            a("9.9.1.1:1"),
            a("7.7.1.1:1"),
        );
        book.configure(&[configured]);
        for address in [bad, other_port, good] {
            book.learn(address, Source::Peer);
        }
        let now = crate::now();
        book.ban(bad.ip(), now + BAN);
        assert!(book.is_banned(bad.ip(), now) && !book.is_banned(bad.ip(), now + BAN));
        assert!(book.get(&bad).is_none() && book.get(&other_port).is_none());
        assert!(!book.learn(bad, Source::Peer) && book.get(&good).is_some());
        assert!(!book.pick(8, &BTreeSet::new(), now, 3).contains(&bad));
        // A configured peer and this machine are never banned.
        book.ban(configured.ip(), now + BAN);
        book.ban(ip("127.0.0.1"), now + BAN);
        assert!(!book.is_banned(configured.ip(), now) && !book.is_banned(ip("127.0.0.1"), now));
        assert!(book.get(&configured).is_some());
        // Configuring a banned peer lifts its range's ban.
        book.configure(&[configured, bad]);
        assert!(!book.is_banned(bad.ip(), now) && book.get(&bad).is_some());
        assert!(book.ban_ends.is_empty());
    }

    #[test]
    fn a_ban_keeps_a_seeds_address_to_connect_to_once_it_ends() {
        // A proven seed shares a /64 with a peer that sent invalid data: the
        // ban covers it, so it is not connected, picked or shared, but it
        // stays in the book and is connected again once the ban ends.
        let now = crate::now();
        let mut book = AddressBook::default();
        let (seed, neighbour) = (a("[2a00:1:2:3::1]:18442"), a("[2a00:1:2:3::2]:18442"));
        book.learn_seed(seed);
        book.succeeded(seed, now - 10);
        book.learn(neighbour, Source::Inbound);
        assert!(book.ban(ip("2a00:1:2:3::bad"), now + BAN));
        assert!(book.is_seed(&seed) && book.get(&neighbour).is_none());
        let none = BTreeSet::new();
        assert!(book.pick(8, &none, now, 3).is_empty());
        assert!(!book.due(&seed, now) && !book.seed_due(now));
        assert!(book.share(ip("9.9.9.9"), None, now, 7).is_empty());
        assert_eq!(book.pick(8, &none, now + BAN, 3), [seed]);
        assert!(book.due(&seed, now + BAN) && book.seed_due(now + BAN));
        // A seed's name that resolves into a banned range is kept the same
        // way; any other address there is still not learned.
        let mut book = AddressBook::default();
        book.ban(ip("8.8.1.1"), now + BAN);
        book.learn_seed(a("8.8.1.1:18442"));
        assert!(!book.learn(a("8.8.1.1:1"), Source::Peer));
        assert!(book.is_seed(&a("8.8.1.1:18442")));
        assert!(!book.seed_due(now) && book.seed_due(now + BAN));
    }

    #[test]
    fn a_ban_covers_an_ipv6_slash_64_and_the_bans_kept_are_capped() {
        let mut book = AddressBook::default();
        let now = 1_000_000;
        assert!(book.ban(ip("2a00:1:2:3:4:5:6:7"), now + BAN));
        assert!(book.is_banned(ip("2a00:1:2:3:ffff::1"), now));
        assert!(!book.is_banned(ip("2a00:1:2:4::1"), now));
        // An IPv4 address is its own range, also written as mapped IPv6.
        assert!(book.ban(ip("8.8.1.1"), now + BAN + 2));
        assert!(book.is_banned(ip("::ffff:8.8.1.1"), now));
        assert!(!book.is_banned(ip("8.8.1.2"), now));
        // Full, a new ban replaces the one that ends soonest: here the first.
        for i in 0..BAN_MAX as u32 - 2 {
            let address = IpAddr::V4(std::net::Ipv4Addr::from(0x0b00_0000 + i));
            assert!(book.ban(address, now + BAN + 1));
        }
        assert_eq!(book.banned.len(), BAN_MAX);
        assert!(book.ban(ip("9.9.9.9"), now + BAN + 1));
        assert_eq!(book.banned.len(), BAN_MAX);
        assert!(!book.is_banned(ip("2a00:1:2:3::1"), now));
        assert!(book.is_banned(ip("8.8.1.1"), now));
        // Bans that have ended give way first, without a scan.
        assert!(book.ban(ip("9.9.9.10"), now + 2 * BAN + 1));
        assert_eq!(book.banned.len(), 2);
        assert_eq!(book.ban_ends.len(), 2);
        assert!(book.is_banned(ip("8.8.1.1"), now + BAN + 1));
    }

    #[test]
    fn one_address_or_network_cannot_take_over_the_book() {
        // Two honest nodes have announced themselves, still unproven, and the
        // book holds a configured peer and a seed in the attacker's network.
        let now = 1_000_000;
        let mut book = AddressBook::default();
        let (configured, seed) = (a("8.8.0.1:1"), a("8.8.0.2:1"));
        let honest = [a("9.9.1.1:18442"), a("7.7.1.1:18442")];
        book.configure(&[configured]);
        book.learn_seed(seed);
        for address in honest {
            assert!(book.learn_announced(address, now));
        }
        let kept = |book: &AddressBook| {
            honest
                .iter()
                .chain([&configured, &seed])
                .all(|a| book.get(a).is_some())
        };
        let held = |book: &AddressBook, of: &dyn Fn(IpAddr) -> bool| {
            book.entries.keys().filter(|a| of(a.ip())).count()
        };
        // One address announcing 2,000 ports keeps two places, and its
        // network stops adding after an hour's worth.
        let flood = ip("8.8.8.8");
        let learned = (1..=2000)
            .filter(|port| book.learn_announced(SocketAddr::new(flood, *port), now))
            .count();
        assert_eq!(learned, ADDED_PER_HOUR);
        assert_eq!(held(&book, &|a| a == flood), PER_IP);
        assert!(kept(&book));
        // Hour after hour, 2,000 addresses of its network keep 32 places,
        // which its own unproven ones take from each other.
        let network = |a: IpAddr| a.to_string().starts_with("8.8.");
        for hour in 1..=3 {
            for i in 0..2000 {
                let address = IpAddr::from([8, 8, 1 + (i / 250) as u8, 1 + (i % 250) as u8]);
                book.learn_announced(SocketAddr::new(address, 18442), now + hour * 3600);
            }
            // The configured peer and the seed are not counted in its share.
            assert_eq!(held(&book, &network), PER_GROUP + 2);
            assert!(kept(&book));
        }
        // So, too, for an IPv6 /64, whose addresses are one network's.
        let v6 = |a: IpAddr| a.to_string().starts_with("2a00:1:");
        for i in 0..2000u16 {
            let address = IpAddr::from([0x2a00, 1, 2, 3, 0, 0, i >> 8, i]);
            book.learn_announced(SocketAddr::new(address, 18442), now);
        }
        assert_eq!(held(&book, &v6), PER_GROUP);
        // A peer sharing addresses of every network, answer after answer,
        // adds an hour's worth an hour.
        let liar = ip("6.6.6.6");
        let mut shared = 0;
        for answer in 0..100u32 {
            let list: Vec<_> = (0..SHARE_MAX as u32)
                .map(|k| (Ipv4Addr::from(0x2000_0000 + (answer * 32 + k) * 65_536), 1).into())
                .collect();
            shared += book.learn_shared(&list, liar, now);
        }
        assert_eq!(shared, ADDED_PER_HOUR);
        assert!(kept(&book));
        // Whatever the attacker does, a proven address of its network keeps
        // its place, and none other of the network takes it.
        let proven = a("8.8.250.1:1");
        let mut book = AddressBook::default();
        book.learn(proven, Source::Peer);
        book.succeeded(proven, now);
        for i in 0..2000 {
            let address = IpAddr::from([8, 8, 1 + (i / 250) as u8, 1 + (i % 250) as u8]);
            book.learn(SocketAddr::new(address, 1), Source::Inbound);
        }
        assert!(book.get(&proven).is_some());
        assert_eq!(held(&book, &network), PER_GROUP);
    }

    #[test]
    fn a_full_book_gives_way_at_random_among_the_least_worth_keeping() {
        // A full book of unproven addresses, each of its own network: the
        // one that gives way is not the lowest, nor one an attacker could
        // name, but one at random. One that failed goes first.
        let mut book = AddressBook::default();
        let address = |i: usize| a(&format!("{}.{}.0.1:1", 11 + i / 256, i % 256));
        for i in 0..BOOK_MAX {
            assert!(book.learn(address(i), Source::Peer));
        }
        let failed = address(500);
        book.failed(failed);
        assert!(book.learn(address(BOOK_MAX), Source::Peer));
        assert!(book.get(&failed).is_none() && book.get(&address(0)).is_some());
        let before: BTreeSet<_> = book.entries.keys().copied().collect();
        for i in BOOK_MAX + 1..BOOK_MAX + 11 {
            assert!(book.learn(address(i), Source::Peer));
        }
        let gone: Vec<_> = before.iter().filter(|a| book.get(a).is_none()).collect();
        assert_eq!(gone.len(), 10);
        assert!(book.get(&address(0)).is_some(), "{gone:?}");
        assert!(gone.iter().any(|a| **a > address(512)), "{gone:?}");
    }

    #[test]
    fn every_peer_is_given_the_same_addresses_for_an_hour() {
        let mut book = AddressBook::default();
        for i in 0..100 {
            let address = a(&format!("8.{i}.0.1:1"));
            book.learn(address, Source::Peer);
            book.succeeded(address, 1000);
        }
        let first = book.share(ip("9.9.9.9"), None, 1000, 7);
        assert_eq!(first.len(), SHARE_MAX);
        // Asked again within the hour, by the same peer or by another, the
        // node gives the same list; a peer whose own address is in it is
        // given the rest.
        for (peer, at, seed) in [("9.9.9.9", 1001, 8), ("5.5.5.5", 1000 + SHARE_KEPT - 1, 9)] {
            assert_eq!(book.share(ip(peer), None, at, seed), first);
        }
        let rest = book.share(ip("9.9.9.9"), Some(first[3]), 1002, 10);
        assert_eq!(rest.len(), SHARE_MAX - 1);
        assert!(!rest.contains(&first[3]));
        // An hour on, it is drawn again; a banned range leaves it at once.
        let later = book.share(ip("9.9.9.9"), None, 1000 + SHARE_KEPT, 11);
        assert_eq!(later.len(), SHARE_MAX);
        assert_ne!(later, first);
        book.ban(later[0].ip(), 1000 + SHARE_KEPT + BAN);
        let now = book.share(ip("9.9.9.9"), None, 1000 + SHARE_KEPT + 1, 12);
        assert_eq!(now[..], later[1..]);
        // A list that holds every address there is to share grows as soon as
        // there is another, and peers on a private network have their own.
        let mut small = AddressBook::default();
        let (x, y, private) = (a("8.1.0.1:1"), a("8.2.0.1:1"), a("192.168.1.5:1"));
        for address in [x, y, private] {
            small.learn(address, Source::Peer);
        }
        small.succeeded(x, 1000);
        assert_eq!(small.share(ip("9.9.9.9"), None, 1000, 7), [x]);
        small.succeeded(y, 1001);
        small.succeeded(private, 1001);
        let mut grown = small.share(ip("9.9.9.9"), None, 1002, 7);
        grown.sort();
        assert_eq!(grown, [x, y]);
        assert_eq!(small.share(ip("10.0.0.2"), None, 1002, 7).len(), 3);
    }

    #[test]
    fn a_book_saved_past_the_shares_loads_what_is_worth_keeping() {
        // A book saved by a node without the shares, one address holding
        // nearly all of it: the configured peer, the seed, the honest
        // addresses and, of the one address, its proven one are kept.
        let dir = std::env::temp_dir().join(format!("ting-book-{}", seed()));
        std::fs::create_dir_all(&dir).unwrap();
        let entry = |address: String, source: &str, verified: Option<u64>| serde_json::json!({"address":address,"source":source,"verified":verified});
        let mut addresses = vec![
            entry("8.8.0.1:1".into(), "config", None),
            entry("8.8.8.8:1".into(), "seed", None),
            entry("9.9.1.1:18442".into(), "inbound", None),
            entry("7.7.1.1:18442".into(), "peer", Some(5)),
        ];
        addresses.extend((2..1020).map(|port| entry(format!("8.8.8.8:{port}"), "inbound", None)));
        addresses.push(entry("8.8.8.8:5000".into(), "peer", Some(9)));
        let file = serde_json::json!({ "addresses": addresses }).to_string();
        std::fs::write(dir.join("peers.json"), file).unwrap();
        let book = AddressBook::open(&dir);
        for kept in ["8.8.0.1:1", "8.8.8.8:1", "9.9.1.1:18442", "7.7.1.1:18442"] {
            assert!(book.get(&a(kept)).is_some(), "{kept}");
        }
        assert!(book.is_seed(&a("8.8.8.8:1")) && book.configured() == 1);
        assert_eq!(book.get(&a("8.8.8.8:5000")).unwrap().verified, Some(9));
        assert_eq!(book.len(), 4 + PER_IP);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_book_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("ting-book-{}", seed()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut book = AddressBook::open(&dir);
        book.learn(a("8.8.8.8:18442"), Source::Peer);
        book.succeeded(a("8.8.8.8:18442"), 5);
        book.save().unwrap();
        let reopened = AddressBook::open(&dir);
        assert_eq!(reopened.get(&a("8.8.8.8:18442")).unwrap().verified, Some(5));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
