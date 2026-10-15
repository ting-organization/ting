//! The reference nonce search finds exactly what the plain `valid_pow` loop
//! finds, and how fast it does so.
use num_bigint::BigUint;
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use ting_core::{consensus, hash::hash, meter::Resources, protocol::Header};
use ting_node::mining;

/// A header whose every field depends on `seed`, so the bytes before the
/// nonce (the part a search hashes once) differ from header to header.
fn header(seed: u8, target: BigUint) -> Header {
    let bytes = |field: u8| hash("mining-test", &[seed, field]);
    Header {
        chain_id: bytes(0),
        height: u64::from(seed) * 1_000 + 7,
        parent: bytes(1),
        timestamp: 1_800_000_000 + u64::from(seed),
        target,
        transactions_root: bytes(2),
        receipts_root: bytes(3),
        state_root: bytes(4),
        used: Resources([1, u64::from(seed), u64::MAX - u64::from(seed), 4]),
        votes: [0, 1, 2, seed % 3],
        producer_hash: bytes(5),
        nonce: u128::from(seed) << 100,
    }
}

fn pow2(bits: u32) -> BigUint {
    BigUint::from(1u8) << bits
}

/// The plain method: the first nonce in `attempts` from `start` (wrapping)
/// that `valid_pow` accepts, with the number of attempts it took.
fn reference(header: &Header, start: u128, attempts: u64) -> (Option<u128>, u64) {
    let mut candidate = header.clone();
    for offset in 0..attempts {
        candidate.nonce = start.wrapping_add(u128::from(offset));
        if consensus::valid_pow(&candidate) {
            return (Some(candidate.nonce), offset + 1);
        }
    }
    (None, attempts)
}

const STARTS: [u128; 8] = [
    0,
    (1 << 16) - 7,
    (1 << 32) - 30,
    (1 << 64) - 30,
    (1 << 96) - 30,
    (1 << 127) - 30,
    u128::MAX - 40,
    u128::MAX,
];

#[test]
fn search_finds_the_nonce_the_plain_loop_accepts_first() {
    // Easy targets so most ranges hold a solution, one so hard that none does,
    // and the bounds of what `valid_pow` accepts.
    let targets = [
        pow2(253),
        pow2(250) + 12_345u32,
        pow2(248) - 1u8,
        pow2(160),
        pow2(256) - 1u8,
    ];
    let mut found = 0;
    for seed in 0..6u8 {
        for target in &targets {
            let h = header(seed, target.clone());
            for start in STARTS {
                for attempts in [0, 1, 3, 200, 2_000] {
                    let (expected, used) = reference(&h, start, attempts);
                    let tried = AtomicU64::new(0);
                    let got = mining::search(&h, start, attempts, &AtomicBool::new(false), &tried);
                    assert_eq!(
                        got, expected,
                        "seed {seed} target {target} start {start} attempts {attempts}"
                    );
                    assert_eq!(tried.load(Ordering::Relaxed), used);
                    found += usize::from(got.is_some());
                }
            }
        }
    }
    assert!(found > 250, "the easy targets should be met often: {found}");
}

#[test]
fn search_without_a_valid_target_finds_nothing() {
    // `valid_pow` refuses a zero target and one wider than 256 bits.
    for target in [BigUint::from(0u8), pow2(256), pow2(300)] {
        let h = header(9, target);
        for start in [0, u128::MAX - 3] {
            assert_eq!(reference(&h, start, 50), (None, 50));
            let tried = AtomicU64::new(0);
            assert_eq!(
                mining::search(&h, start, 50, &AtomicBool::new(false), &tried),
                None
            );
            assert_eq!(tried.load(Ordering::Relaxed), 50);
            assert_eq!(
                mining::search_parallel(&h, start, 50, 3, &AtomicBool::new(false), &tried),
                None
            );
            assert_eq!(tried.load(Ordering::Relaxed), 200);
        }
    }
}

#[test]
fn a_raised_stop_ends_the_search_before_any_attempt() {
    let h = header(3, pow2(256) - 1u8);
    let tried = AtomicU64::new(0);
    let stop = AtomicBool::new(true);
    assert_eq!(mining::search(&h, 5, 1_000, &stop, &tried), None);
    assert_eq!(
        mining::search_parallel(&h, 5, 1_000, 4, &stop, &tried),
        None
    );
    assert_eq!(tried.load(Ordering::Relaxed), 0);
}

#[test]
fn parallel_search_returns_a_first_solution_of_one_of_its_ranges() {
    for seed in 0..4u8 {
        for target in [pow2(252), pow2(246), pow2(150)] {
            let h = header(seed, target.clone());
            for start in STARTS {
                for threads in [1usize, 2, 3, 5] {
                    let attempts = 40;
                    // Thread i covers `attempts` nonces from start + i × attempts.
                    let firsts: Vec<u128> = (0..threads)
                        .filter_map(|i| {
                            let range = start.wrapping_add(u128::from(attempts) * i as u128);
                            reference(&h, range, attempts).0
                        })
                        .collect();
                    let tried = AtomicU64::new(0);
                    let got = mining::search_parallel(
                        &h,
                        start,
                        attempts,
                        threads,
                        &AtomicBool::new(false),
                        &tried,
                    );
                    match got {
                        Some(nonce) => {
                            assert!(firsts.contains(&nonce), "{nonce} not in {firsts:?}")
                        }
                        None => assert!(firsts.is_empty()),
                    }
                    if threads == 1 {
                        assert_eq!(got, firsts.first().copied());
                    }
                    if got.is_none() {
                        assert_eq!(tried.load(Ordering::Relaxed), attempts * threads as u64);
                    }
                }
            }
        }
    }
}

// ---- Speed ---------------------------------------------------------------------

/// Hashes per second of `run(start, attempts)`, run in slices for about a second.
fn rate(mut run: impl FnMut(u128, u64)) -> f64 {
    let slice = 50_000;
    let began = Instant::now();
    let mut done = 0u64;
    while began.elapsed() < Duration::from_secs(1) {
        run(u128::from(done), slice);
        done += slice;
    }
    done as f64 / began.elapsed().as_secs_f64()
}

/// The same search through `sha2`'s own hasher, cloned after the bytes before
/// the nonce: one compression per nonce on the CPU's SHA extensions where it
/// has them. What the reference search would reach with `sha2` as a node
/// dependency rather than only a test one (`compress256` itself needs the
/// crate's `compress` feature, which nothing enables).
fn sha2_midstate(header: &Header, start: u128, attempts: u64) -> Option<u128> {
    use sha2::{Digest, Sha256};
    use ting_core::protocol::{Encode, target_bytes};
    let mut fixed = Sha256::new();
    fixed.update(b"Ting/");
    fixed.update(5u16.to_le_bytes());
    fixed.update(b"block");
    fixed.update(&header.encode()[..mining::NONCE_OFFSET]);
    let target = target_bytes(&header.target);
    for offset in 0..attempts {
        let nonce = start.wrapping_add(u128::from(offset));
        let mut hasher = fixed.clone();
        hasher.update(nonce.to_le_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        if digest <= target {
            return Some(nonce);
        }
    }
    None
}

#[test]
fn the_sha2_benchmark_variant_finds_what_the_plain_loop_finds() {
    for seed in 0..3u8 {
        let h = header(seed, pow2(251));
        for start in STARTS {
            assert_eq!(sha2_midstate(&h, start, 300), reference(&h, start, 300).0);
        }
    }
}

/// `cargo test -p ting-node --release --test mining -- --ignored --nocapture`
#[test]
#[ignore = "benchmark"]
fn search_speed() {
    // A target nothing meets, so every slice runs to its end.
    let h = header(1, BigUint::from(1u8));
    let stop = AtomicBool::new(false);
    let tried = AtomicU64::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    // The best of three interleaved rounds, against clock and core changes.
    let mut best = [0f64; 4];
    for _ in 0..3 {
        let rates = [
            rate(|start, attempts| {
                assert_eq!(reference(&h, start, attempts).0, None);
            }),
            rate(|start, attempts| {
                assert_eq!(mining::search(&h, start, attempts, &stop, &tried), None);
            }),
            rate(|start, attempts| {
                assert_eq!(sha2_midstate(&h, start, attempts), None);
            }),
            rate(|start, attempts| {
                let start = start * threads as u128;
                assert_eq!(
                    mining::search_parallel(&h, start, attempts, threads, &stop, &tried),
                    None
                );
            }) * threads as f64,
        ];
        for (best, rate) in best.iter_mut().zip(rates) {
            *best = best.max(rate / 1e6);
        }
    }
    let [plain, search, sha2, parallel] = best;
    println!("plain valid_pow loop:   {plain:6.2} MH/s per thread");
    println!("mining::search:         {search:6.2} MH/s per thread");
    println!("sha2 hasher midstate:   {sha2:6.2} MH/s per thread");
    println!("mining::search_parallel {parallel:6.2} MH/s on {threads} threads");
}
