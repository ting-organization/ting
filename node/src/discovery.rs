//! Peer discovery: an address book filled from the configured peers, from the
//! listening addresses inbound peers announce, and from the addresses peers
//! share on request. This is node policy; consensus never sees it.
//!
//! An address is shared only after this node has completed a round with it,
//! and never with a peer that could not reach it: loopback addresses stay on
//! the machine and private ranges stay off the public internet. The outbound
//! peers a node chooses come from distinct network groups, so one operator's
//! address range cannot fill them all. A peer that sent invalid data is banned
//! for a day, with its address range: its addresses are dropped and not learned
//! again, it may not connect, and its open connections end.
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
/// An address is shared while its last completed round is this recent.
const SHARE_FRESH: u64 = 24 * 3600;
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

#[derive(Default)]
pub struct AddressBook {
    entries: BTreeMap<SocketAddr, Entry>,
    path: Option<PathBuf>,
    dirty: bool,
    /// Banned ranges, until when, and the same by when they end; kept for
    /// this run only.
    banned: BTreeMap<IpAddr, u64>,
    ban_ends: BTreeSet<(u64, IpAddr)>,
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
            ..Self::default()
        };
        for entry in stored.addresses.into_iter().take(BOOK_MAX) {
            if usable(entry.address) {
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
    /// Adds an address the book does not know; returns whether it did.
    pub fn learn(&mut self, address: SocketAddr, source: Source) -> bool {
        if !usable(address)
            || self.entries.contains_key(&address)
            || self.is_banned(address.ip(), crate::now())
        {
            return false;
        }
        self.insert(address, source)
    }
    /// Bans an address's range until `until`, dropping every address in it;
    /// returns whether it did. A range holding a configured peer is never
    /// banned, nor is this machine: banning it would ban every node on it.
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
        self.entries.retain(|a, _| self::range(a.ip()) != range);
        self.dirty |= self.entries.len() != before;
        true
    }
    pub fn is_banned(&self, ip: IpAddr, now: u64) -> bool {
        self.banned
            .get(&range(ip))
            .is_some_and(|until| *until > now)
    }
    /// Adds the addresses one peer shared: at most `SHARE_MAX` of them, and
    /// only those the peer could legitimately know about from where it is.
    pub fn learn_shared(&mut self, addresses: &[SocketAddr], from: IpAddr) -> usize {
        addresses
            .iter()
            .take(SHARE_MAX)
            .filter(|a| shareable(**a, from))
            .filter(|a| self.learn(**a, Source::Peer))
            .count()
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
    /// Drops the least useful address: never a configured one, an unproven
    /// one before a proven one, the most failed and longest unproven first.
    fn evict(&mut self) -> bool {
        let worst = self
            .entries
            .values()
            .filter(|e| e.source != Source::Config)
            .min_by_key(|e| {
                (
                    e.verified.is_some(),
                    e.verified.unwrap_or(0),
                    u32::MAX - e.failures,
                )
            })
            .map(|e| e.address);
        worst.is_some_and(|a| self.entries.remove(&a).is_some())
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
    /// failure once the wait is over. A configured peer waits a minute, any
    /// other a minute doubling with each further failure up to an hour.
    pub fn due(&self, address: &SocketAddr, now: u64) -> bool {
        self.entries.get(address).is_none_or(|e| {
            if e.source == Source::Config {
                e.failures == 0 || now >= e.attempted.saturating_add(RETRY_BASE)
            } else {
                Self::retry(e, now)
            }
        })
    }
    /// Up to `SHARE_MAX` addresses, in random order, for a peer at `peer`:
    /// those with a recent completed round that the peer can reach,
    /// except the peer's own announced address.
    pub fn share(
        &self,
        peer: IpAddr,
        own: Option<SocketAddr>,
        now: u64,
        seed: u64,
    ) -> Vec<SocketAddr> {
        let mut list: Vec<_> = self
            .entries
            .values()
            .filter(|e| {
                e.verified
                    .is_some_and(|v| now.saturating_sub(v) <= SHARE_FRESH)
            })
            .filter(|e| e.failures == 0 && Some(e.address) != own && shareable(e.address, peer))
            .map(|e| e.address)
            .collect();
        Shuffle(seed).shuffle(&mut list);
        list.truncate(SHARE_MAX);
        list
    }
    /// Up to `want` addresses to connect to, besides `busy`: proven
    /// ones first, most recently proven first, then unproven ones in random
    /// order; each from a network group none of the others is in.
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
                e.source != Source::Config && !busy.contains(&e.address) && Self::retry(e, now)
            })
            .collect();
        Shuffle(seed).shuffle(&mut candidates);
        candidates.sort_by_key(|e| std::cmp::Reverse(e.verified));
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
    /// One unproven address to test with a short connection when every
    /// connection slot is taken, so new nodes still become shareable.
    pub fn probe(&self, busy: &BTreeSet<SocketAddr>, now: u64, seed: u64) -> Option<SocketAddr> {
        let mut candidates: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.verified.is_none() && !busy.contains(&e.address) && Self::retry(e, now))
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
        assert_eq!(book.learn_shared(&sent, ip("9.9.9.9")), SHARE_MAX);
        let mut local = AddressBook::default();
        assert_eq!(
            local.learn_shared(&[a("10.0.0.1:1"), a("127.0.0.1:1")], ip("9.9.9.9")),
            0
        );
        assert_eq!(
            local.learn_shared(&[a("10.0.0.1:1"), a("127.0.0.1:1")], ip("127.0.0.1")),
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
    fn a_full_book_evicts_unproven_addresses_never_configured_ones() {
        let mut book = AddressBook::default();
        book.configure(&[a("8.0.0.1:1")]);
        let proven = a("8.0.0.2:1");
        book.learn(proven, Source::Peer);
        book.succeeded(proven, 10);
        for i in 0..2000 {
            book.learn(a(&format!("9.{}.{}.1:1", i / 256, i % 256)), Source::Peer);
        }
        assert_eq!(book.len(), BOOK_MAX);
        assert!(book.get(&a("8.0.0.1:1")).is_some() && book.get(&proven).is_some());
        book.configure(&[]);
        assert!(book.get(&a("8.0.0.1:1")).is_none());
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
