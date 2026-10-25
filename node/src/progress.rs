//! The node's account of its chain as it runs (wiki:
//! start/node#what-the-node-writes): where the chain stands when the node
//! starts, how a download toward the best header goes and when it catches
//! up, each new tip after that, and a while with no peer to follow.
use crate::{daemon::Node, log::duration};
use serde_json::json;
use std::time::Duration;
use ting_core::hash::hex;
use tokio::time::Instant;

/// While the node downloads, its progress is written this often.
const PROGRESS: Duration = Duration::from_secs(10);
/// A node more than this many blocks below its best header is downloading.
/// Following the chain, it is a block below for a moment whenever a new
/// header arrives before its block.
const BEHIND: u64 = 2;
/// With no peer connected for this long, the node says so, and again this
/// often while none is.
const ALONE: Duration = Duration::from_secs(30);
const ALONE_AGAIN: Duration = Duration::from_secs(600);

/// A download toward the best header: the height it began at, when, and
/// when its progress was last written.
struct Download {
    from: u64,
    started: Instant,
    written: Instant,
}

/// Writes the chain's progress until the node stops.
pub async fn run(node: Node) {
    let mut events = node.handle.events();
    let mut last = events.borrow_and_update().clone();
    let network = match node.handle.call("status", json!({})).await {
        Ok(status) => status["network"].as_str().unwrap_or("other").to_owned(),
        Err(_) => "other".into(),
    };
    crate::log!(
        "{network} chain at height {}, tip {}",
        last.height,
        hex(&last.tip)
    );
    let mut download: Option<Download> = None;
    let mut alone: Option<(Instant, Option<Instant>)> = None;
    let mut tick = tokio::time::interval(PROGRESS);
    loop {
        tokio::select! {
            changed = events.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = tick.tick() => {}
            _ = node.stopped() => return,
        }
        let event = events.borrow_and_update().clone();
        let now = Instant::now();
        let behind = event.best.saturating_sub(event.height);
        let following = download.is_none();
        match &mut download {
            None if behind > BEHIND => {
                crate::log!(
                    "downloading blocks from height {}, best header so far {}",
                    event.height + 1,
                    event.best
                );
                download = Some(Download {
                    from: event.height,
                    started: now,
                    written: now,
                });
            }
            Some(d) if behind == 0 => {
                let blocks = event.height.saturating_sub(d.from);
                crate::log!(
                    "caught up at height {}, tip {}: {blocks} blocks in {}",
                    event.height,
                    hex(&event.tip),
                    duration(d.started.elapsed().as_secs())
                );
                download = None;
            }
            Some(d) if now >= d.written + PROGRESS => {
                let done = event.height.saturating_sub(d.from);
                let rate = done as f64 / d.started.elapsed().as_secs_f64().max(1.0);
                let left = if rate > 0.0 {
                    format!(", about {} left", duration((behind as f64 / rate) as u64))
                } else {
                    String::new()
                };
                crate::log!(
                    "height {} of {} ({:.0}%), {rate:.0} blocks/s{left}",
                    event.height,
                    event.best,
                    100.0 * event.height as f64 / event.best.max(1) as f64
                );
                d.written = now;
            }
            _ => {}
        }
        if following && download.is_none() && event.tip != last.tip {
            if event.height == last.height + 1 {
                crate::log!("block {} {}", event.height, hex(&event.tip));
            } else if event.height > last.height {
                crate::log!(
                    "blocks {} to {}, tip {}",
                    last.height + 1,
                    event.height,
                    hex(&event.tip)
                );
            } else {
                crate::log!("tip moved to height {}: {}", event.height, hex(&event.tip));
            }
        }
        last = event;
        // A node with nowhere to look, as a development chain of its own,
        // is not alone.
        let connected = node
            .peers
            .lock()
            .unwrap()
            .values()
            .filter(|status| status["connected"].is_number())
            .count();
        let looking = network == "production" || !node.book.lock().unwrap().is_empty();
        if connected > 0 || !looking {
            alone = None;
            continue;
        }
        let (since, said) = alone.get_or_insert((now, None));
        let due = match said {
            None => now >= *since + ALONE,
            Some(said) => now >= *said + ALONE_AGAIN,
        };
        if due {
            crate::log!(
                "no peers connected for {}; looking for them through {}",
                duration(since.elapsed().as_secs()),
                if network == "production" {
                    "the seed nodes and known addresses"
                } else {
                    "the configured and known addresses"
                }
            );
            *said = Some(now);
        }
    }
}
