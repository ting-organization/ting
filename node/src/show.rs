//! The `ting` command's results as text (wiki: start/node#command-reference):
//! a few lines each, for a person to read. With `--json` the command prints
//! the result itself instead, as scripts read it.
use serde_json::Value;
use ting_node::log::{count, duration, utc};

/// The four resources, in the order of limits and votes.
const RESOURCES: [&str; 4] = ["compute", "state access", "data", "growth"];

fn number(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
/// How long ago a Unix time was, or `never`.
fn ago(v: &Value) -> String {
    match v.as_u64() {
        Some(at) => format!("{} ago", duration(ting_node::now().saturating_sub(at))),
        None => "never".into(),
    }
}
/// A size in bytes, in binary units past a KiB.
fn bytes(n: u64) -> String {
    let units = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    for (size, unit) in units {
        if n >= size {
            let value = format!("{:.1}", n as f64 / size as f64);
            return format!("{} {unit}", value.trim_end_matches(".0"));
        }
    }
    format!("{n} B")
}
fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}
/// A capacity vote, as `ting miner votes` takes it.
fn vote(v: &Value) -> &'static str {
    match v.as_u64() {
        Some(1) => "increase",
        Some(2) => "decrease",
        _ => "hold",
    }
}
/// Four votes, by resource; one word when they agree.
fn votes_text(votes: &Value) -> String {
    let words: Vec<&str> = (0..4).map(|k| vote(&votes[k])).collect();
    if words.iter().all(|w| *w == words[0]) {
        return format!("{} on all four", words[0]);
    }
    RESOURCES
        .iter()
        .zip(&words)
        .map(|(resource, word)| format!("{resource} {word}"))
        .collect::<Vec<_>>()
        .join(", ")
}
/// Labelled lines, their values lined up.
fn rows(rows: &[(&str, String)]) -> String {
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    rows.iter()
        .map(|(label, value)| format!("{label:width$}  {value}\n"))
        .collect()
}
/// A table under a header line, each column as wide as its widest cell.
fn table(header: &[&str], body: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in body {
        for (k, cell) in row.iter().enumerate() {
            widths[k] = widths[k].max(cell.chars().count());
        }
    }
    let line = |cells: Vec<String>| {
        let mut out = String::new();
        for (k, cell) in cells.iter().enumerate() {
            if k + 1 == cells.len() {
                out += cell;
            } else {
                out += &format!("{cell:w$}  ", w = widths[k]);
            }
        }
        out.trim_end().to_owned() + "\n"
    };
    let mut out = line(header.iter().map(|h| h.to_string()).collect());
    for row in body {
        out += &line(row.clone());
    }
    out
}

/// `ting init`: the new data directory and how to start it.
pub fn init(result: &Value, network: &str, height: usize, ting: &str) -> String {
    let what = match network {
        "production" => "a production node",
        "development" => "a development chain",
        _ => "a node for another network",
    };
    let at = if height > 0 {
        format!(
            ", at height {} with the seed's blocks",
            count(height as u64)
        )
    } else {
        String::new()
    };
    let rest = if result["api_enabled"] == true {
        "on when the node runs".to_owned()
    } else {
        format!("off; turn it on with: {ting} api enable")
    };
    format!("Created {what} in {}{at}\n\n", text(&result["initialized"]))
        + &rows(&[
            ("Chain ID", text(&result["chain_id"]).to_owned()),
            ("REST", rest),
        ])
        + &format!("\nStart it with: {ting} start\n")
}

/// `ting start`: whether this command started the node, then its status.
pub fn started(status: &Value, fresh: bool, log: &str, summary: String) -> String {
    let pid = number(&status["listeners"]["pid"]);
    let first = if fresh {
        format!("Started the node (process {pid}); it writes to {log}")
    } else {
        format!("The node is already running (process {pid})")
    };
    format!("{first}\n\n{summary}")
}

/// `ting status`: where the chain stands, the peers, the pool, the miner and
/// the listeners, from the node's status, peers and miner status.
pub fn status(status: &Value, peers: &Value, miner: &Value) -> String {
    let network = text(&status["network"]);
    let height = number(&status["height"]);
    let best = number(&status["best_height"]);
    let connected = peers["items"].as_array().map_or(0, |items| {
        items.iter().filter(|p| p["connected"].is_number()).count()
    }) as u64;
    let chain = if status["replaying"] == true {
        format!("height {}, rebuilding undo data", count(height))
    } else if best > height + 2 {
        format!(
            "syncing, height {} of {} ({:.0}%)",
            count(height),
            count(best),
            100.0 * height as f64 / best as f64
        )
    } else if connected > 0 {
        format!("height {}, in sync", count(height))
    } else if network == "production" {
        format!("height {}, waiting for peers", count(height))
    } else {
        format!("height {}", count(height))
    };
    let pool = format!(
        "{} ({}), {} deferred",
        plural(number(&status["mempool_count"]), "pending", "pending"),
        bytes(number(&status["mempool_bytes"])),
        count(number(&status["deferred_count"]))
    );
    let miner = if miner["running"] == true {
        let blocks = number(&miner["blocks"]);
        format!(
            "on, {}, {} mined in {}",
            search(&miner["search"]),
            plural(blocks, "block", "blocks"),
            since(&miner["since"])
        )
    } else {
        "off".to_owned()
    };
    let rest = match status["listeners"]["api"].as_str() {
        Some(address) => format!("http://{address}/ (explorer, /wiki, /docs)"),
        None => "off".to_owned(),
    };
    let limits = &status["limits"];
    let mut lines = vec![
        ("Chain", chain),
        ("Tip", text(&status["tip"]).to_owned()),
        ("Peers", format!("{} connected", count(connected))),
        ("Pool", pool),
        (
            "Capacity",
            format!(
                "C {} · S {} · D {} · G {} per block",
                count(number(&limits[0])),
                count(number(&limits[1])),
                count(number(&limits[2])),
                count(number(&limits[3]))
            ),
        ),
        ("Miner", miner),
        ("REST", rest),
        (
            "Listening",
            format!(
                "peers on {}, control on {}",
                text(&status["listeners"]["p2p"]),
                text(&status["listeners"]["control"])
            ),
        ),
    ];
    if let Some(reorganization) = status["reorganization"].as_object() {
        lines.push((
            "Reorganized",
            format!(
                "{} replaced above height {}, {}",
                plural(number(&reorganization["replaced"]), "block", "blocks"),
                count(number(&reorganization["fork_height"])),
                ago(&reorganization["time"])
            ),
        ));
    }
    let marked = status["marked"].as_array().map_or(0, Vec::len) as u64;
    if marked > 0 {
        lines.push((
            "Marked",
            format!(
                "{} invalid by the operator",
                plural(marked, "block", "blocks")
            ),
        ));
    }
    for warning in status["warnings"].as_array().into_iter().flatten() {
        lines.push(("Warning", text(warning).to_owned()));
    }
    format!(
        "{} · {network} · {} · up {}\n\n",
        text(&status["name"]),
        text(&status["version"]),
        duration(number(&status["uptime_seconds"]))
    ) + &rows(&lines)
}

/// The miner's nonce search, as its status describes it: `4 CPU threads`.
fn search(search: &Value) -> String {
    let kind = match text(&search["kind"]) {
        "cpu" => "CPU",
        "" => "search",
        other => other,
    };
    match search["threads"].as_u64() {
        Some(threads) => plural(
            threads,
            &format!("{kind} thread"),
            &format!("{kind} threads"),
        ),
        None => kind.to_owned(),
    }
}
/// How long ago a Unix time was, as a span: `5m`.
fn since(v: &Value) -> String {
    duration(ting_node::now().saturating_sub(number(v)))
}

/// `ting peers`: the connected peers in a table, then those not connected.
pub fn peers(peers: &Value) -> String {
    let items = peers["items"].as_array().cloned().unwrap_or_default();
    let (up, down): (Vec<&Value>, Vec<&Value>) =
        items.iter().partition(|p| p["connected"].is_number());
    let source = |p: &Value| match text(&p["source"]) {
        "config" => "configured".to_owned(),
        other => other.to_owned(),
    };
    let mut out = String::new();
    if up.is_empty() {
        out += "No peers connected.\n";
    } else {
        let body: Vec<Vec<String>> = up
            .iter()
            .map(|p| {
                vec![
                    text(&p["address"]).to_owned(),
                    text(&p["agent"]).to_owned(),
                    count(number(&p["height"])),
                    ago(&p["connected"]),
                    ago(&p["last_block"]),
                    source(p),
                ]
            })
            .collect();
        out += &table(
            &[
                "ADDRESS",
                "SOFTWARE",
                "HEIGHT",
                "CONNECTED",
                "LAST BLOCK",
                "SOURCE",
            ],
            &body,
        );
    }
    if !down.is_empty() {
        out += "\nNot connected:\n";
        for p in down {
            let why = match p["error"].as_str() {
                Some(error) => match p["last_attempt"].as_u64() {
                    Some(_) => format!("{error}, {}", ago(&p["last_attempt"])),
                    None => error.to_owned(),
                },
                None => "not tried yet".to_owned(),
            };
            out += &format!("  {} ({}): {why}\n", text(&p["address"]), source(p));
        }
    }
    out + &format!(
        "\n{} connected of up to {} · {} known · discovery {}\n",
        count(up_count(&items)),
        count(number(&peers["max_peers"])),
        plural(number(&peers["known"]), "address", "addresses"),
        if peers["discovery"] == true {
            "on"
        } else {
            "off"
        }
    )
}
fn up_count(items: &[Value]) -> u64 {
    items.iter().filter(|p| p["connected"].is_number()).count() as u64
}

/// `ting probe`: what the node at `address` answered.
pub fn probe(address: &str, result: &Value, chain_id: &str) -> String {
    let state = &result["answer"];
    if let Some(error) = state["error"].as_str() {
        return format!("{address} answered with an error: {error}\n");
    }
    let chain = if text(&state["chain_id"]) == chain_id {
        "the same as this node's".to_owned()
    } else {
        format!("another chain, {}", text(&state["chain_id"]))
    };
    let ids = |v: &Value| v.as_array().map_or(0, Vec::len) as u64;
    format!(
        "{address} answered: {}, peer protocol {}\n\n",
        text(&state["agent"]),
        number(&state["version"])
    ) + &rows(&[
        ("Chain", chain),
        ("Height", count(number(&state["height"]))),
        ("Tip", text(&state["tip"]).to_owned()),
        (
            "Pool",
            format!(
                "{} pending, {} deferred",
                count(ids(&state["pending"])),
                count(ids(&state["deferred"]))
            ),
        ),
    ])
}

/// Whether an edit went to config.json because the node was not running.
fn saved(result: &Value) -> bool {
    result["saved"] == true
}

/// `ting add node` and `ting remove node`.
pub fn peer_edit(result: &Value, added: bool, address: &str) -> String {
    match (added, saved(result)) {
        (true, false) => format!("Added peer {address}; the node connects to it now.\n"),
        (true, true) => {
            format!(
                "Added peer {address} to config.json; the node connects to it when it starts.\n"
            )
        }
        (false, false) => format!("Removed peer {address}.\n"),
        (false, true) => format!("Removed peer {address} from config.json.\n"),
    }
}

/// `ting api enable` and `ting api disable`.
pub fn api(result: &Value, enabled: bool, cors: bool) -> String {
    if saved(result) {
        return format!(
            "Saved in config.json: REST is {} when the node starts.\n",
            if enabled { "on" } else { "off" }
        );
    }
    let Some(address) = result["address"].as_str() else {
        return "REST is off.\n".into();
    };
    let mut out = format!("REST is on at http://{address}/\n\n")
        + &rows(&[
            ("Explorer", format!("http://{address}/")),
            ("Wiki", format!("http://{address}/wiki")),
            ("API", format!("http://{address}/docs")),
        ]);
    if cors {
        out += "\nIt answers browser applications from any origin (CORS).\n";
    }
    out
}

/// `ting docs`.
pub fn docs(result: &Value) -> String {
    let wiki = text(&result["wiki"]);
    rows(&[
        ("Explorer", wiki.trim_end_matches("wiki").to_owned()),
        ("Wiki", wiki.to_owned()),
        ("API", text(&result["api_docs"]).to_owned()),
        ("OpenAPI", text(&result["openapi"]).to_owned()),
    ])
}

/// A block a command handed the chain, as the chain took it: a new tip, or
/// one already known or stored off the best branch. The chain's height is
/// the new block's when it is the tip.
fn block_line(block: &Value) -> (bool, String) {
    let id = text(&block["id"]);
    let height = count(number(&block["height"]));
    if block["known"] == true {
        (
            false,
            format!("block {id}, already known; the chain is at height {height}"),
        )
    } else if block["canonical"] == false {
        (
            false,
            format!("block {id}, stored off the best branch; the chain is at height {height}"),
        )
    } else {
        (true, format!("block {height} {id}"))
    }
}

/// `ting mine`: each block mined, then why it stopped early, if it did.
pub fn mined(result: &Value, asked: u64, attempts: u64) -> String {
    let blocks = result["blocks"].as_array().cloned().unwrap_or_default();
    let mut out: String = blocks
        .iter()
        .map(|block| format!("Mined {}\n", block_line(block).1))
        .collect();
    if result["exhausted"] == true {
        out += &format!(
            "No nonce found within {} for the next block, after {} of {asked}; \
             more attempts may find one: --attempts N\n",
            plural(attempts, "attempt", "attempts"),
            plural(blocks.len() as u64, "block", "blocks"),
        );
    }
    out
}

/// `ting solution`.
pub fn solution(result: &Value) -> String {
    match block_line(result) {
        (true, line) => format!("Accepted {line}; it is the tip.\n"),
        (false, line) => format!("Took {line}.\n"),
    }
}

/// `ting miner status`.
pub fn miner(result: &Value) -> String {
    let producer = match text(&result["producer_hex"]) {
        "" => "none".to_owned(),
        hex => hex.to_owned(),
    };
    let spacing = number(&result["spacing"]);
    let spacing = if spacing > 0 {
        format!("at least {} between its blocks", duration(spacing))
    } else {
        "no wait between its blocks".to_owned()
    };
    let mut lines = Vec::new();
    if result["running"] == true {
        lines.push((
            "Miner",
            format!(
                "running for {} on {}, {spacing}",
                since(&result["since"]),
                search(&result["search"])
            ),
        ));
        lines.push((
            "Mined",
            format!(
                "{}; {} tried in {}",
                plural(number(&result["blocks"]), "block", "blocks"),
                plural(number(&result["attempts"]), "nonce", "nonces"),
                plural(number(&result["templates"]), "template", "templates")
            ),
        ));
        if let Some(error) = result["last_error"].as_str() {
            lines.push(("Last error", error.to_owned()));
        }
    } else {
        lines.push((
            "Miner",
            format!(
                "stopped; set to {}, {spacing}",
                plural(number(&result["threads"]), "thread", "threads")
            ),
        ));
    }
    lines.push(("Producer", producer));
    lines.push(("Votes", votes_text(&result["votes"])));
    rows(&lines)
}

/// `ting miner start`.
pub fn miner_started(result: &Value) -> String {
    if saved(result) {
        return "Saved in config.json: the miner starts with the node.\n".into();
    }
    let spacing = number(&result["spacing"]);
    let spacing = if spacing > 0 {
        format!(", at least {} between its blocks", duration(spacing))
    } else {
        String::new()
    };
    format!("Miner started on {}{spacing}.\n", search(&result["search"]))
}

/// `ting miner stop`.
pub fn miner_stopped(result: &Value) -> String {
    if saved(result) {
        "Saved in config.json: the miner stays off when the node starts.\n".into()
    } else {
        "Miner stopped.\n".into()
    }
}

/// `ting miner producer`.
pub fn producer(result: &Value, hex: &str) -> String {
    let what = if hex.is_empty() {
        "Producer data cleared".to_owned()
    } else {
        format!("Producer data set to {hex}")
    };
    if saved(result) {
        format!("{what} in config.json.\n")
    } else {
        format!("{what}; templates from now on carry it.\n")
    }
}

/// `ting miner votes`.
pub fn votes_set(result: &Value, votes: &Value) -> String {
    let what = format!("Votes set: {}", votes_text(votes));
    if saved(result) {
        format!("{what}, in config.json.\n")
    } else {
        format!("{what}; templates from now on carry them.\n")
    }
}

/// `ting votes`: this epoch's voting by resource, then the capacities
/// already decided.
pub fn votes(result: &Value) -> String {
    let epoch = &result["this_epoch"];
    let height = number(&result["height"]);
    let capacities = result["capacities"].as_array().cloned().unwrap_or_default();
    // The capacities in force: of the last epoch begun by the next block.
    let current = capacities
        .iter()
        .rfind(|c| number(&c["from"]) <= height + 1)
        .map_or(Value::Null, |c| c["limits"].clone());
    let percent = |v: &Value| format!("{:.1}%", v.as_f64().unwrap_or(0.0));
    let body: Vec<Vec<String>> = (0..4)
        .map(|k| {
            vec![
                RESOURCES[k].to_owned(),
                count(number(&current[k])),
                percent(&epoch["increase_percent"][k]),
                percent(&epoch["decrease_percent"][k]),
                text(&epoch["outcome_so_far"][k]).to_owned(),
            ]
        })
        .collect();
    let mut out = format!(
        "Capacity voting, epoch {}: {} of {} voted so far; the outcome takes effect from height {}\n\n",
        number(&result["epoch"]),
        count(number(&epoch["blocks"])),
        plural(number(&result["epoch_blocks"]), "block", "blocks"),
        count(number(&epoch["takes_effect_from"]))
    );
    out += &table(
        &["RESOURCE", "CAPACITY", "INCREASE", "DECREASE", "SO FAR"],
        &body,
    );
    out += &format!(
        "\nBlocks this node builds vote {}.\n\nDecided capacities:\n",
        votes_text(&result["votes"])
    );
    let decided: Vec<Vec<String>> = capacities
        .iter()
        .map(|c| {
            let mut row = vec![number(&c["epoch"]).to_string(), count(number(&c["from"]))];
            row.extend((0..4).map(|k| count(number(&c["limits"][k]))));
            row
        })
        .collect();
    out + &table(
        &[
            "EPOCH",
            "FROM HEIGHT",
            "COMPUTE",
            "STATE ACCESS",
            "DATA",
            "GROWTH",
        ],
        &decided,
    )
}

/// `ting template`.
pub fn template(result: &Value, ting: &str) -> String {
    let transactions = result["transactions"].as_array().map_or(0, Vec::len) as u64;
    let mining = &result["mining"];
    let offset = number(&mining["nonce_offset"]);
    format!(
        "Template {} for block {}\n\n",
        text(&result["id"]),
        count(number(&result["height"]))
    ) + &rows(&[
        ("Parent", text(&result["parent"]).to_owned()),
        (
            "Holds",
            format!(
                "{}, {} in all",
                plural(transactions, "transaction", "transactions"),
                bytes(number(&result["bytes"]))
            ),
        ),
        ("Target", text(&mining["target_hex"]).to_owned()),
        ("Header", text(&result["header_hex"]).to_owned()),
    ]) + &format!(
        "\nThe nonce is bytes {offset} to {} of the header, {}-endian. \
         Submit one with: {ting} solution {} NONCE\n",
        offset + number(&mining["nonce_bytes"]) - 1,
        text(&mining["nonce_endian"]),
        text(&result["id"])
    )
}

/// `ting submit`.
pub fn submitted(result: &Value) -> String {
    let id = text(&result["id"]);
    let known = result["known"] == true;
    if result["deferred"] == true {
        let until = utc(number(&result["not_before"]));
        let expires = utc(number(&result["expires"]));
        let held = if known { "was already held" } else { "is held" };
        format!(
            "Transaction {id} {held} until {until}, then included once it executes \
             successfully; it expires at {expires}.\n"
        )
    } else if known {
        format!("Transaction {id} was already known.\n")
    } else {
        format!("Transaction {id} is in the pool.\n")
    }
}

/// `ting invalidate`.
pub fn invalidated(result: &Value) -> String {
    let id = text(&result["id"]);
    let first = if result["known"] == true {
        format!("Marked block {id} invalid.")
    } else {
        format!("Marked block {id} invalid; the node refuses it if it arrives.")
    };
    format!("{first}\n{}", chain_now(result))
}

/// `ting reconsider`.
pub fn reconsidered(result: &Value) -> String {
    format!(
        "Cleared {} at and around block {}.\n{}",
        plural(number(&result["cleared"]), "invalid mark", "invalid marks"),
        text(&result["id"]),
        chain_now(result)
    )
}
fn chain_now(result: &Value) -> String {
    format!(
        "The chain is at height {}, tip {}.\n",
        count(number(&result["height"])),
        text(&result["tip"])
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes_tables_and_votes_read_plainly() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1_023), "1023 B");
        assert_eq!(bytes(1_048_576), "1 MiB");
        assert_eq!(bytes(1_572_864), "1.5 MiB");
        assert_eq!(
            table(&["A", "LONGER"], &[vec!["wide cell".into(), "x".into()]]),
            "A          LONGER\nwide cell  x\n"
        );
        assert_eq!(votes_text(&json!([0, 0, 0, 0])), "hold on all four");
        assert_eq!(
            votes_text(&json!([0, 1, 2, 0])),
            "compute hold, state access increase, data decrease, growth hold"
        );
    }

    #[test]
    fn a_status_says_where_the_chain_stands() {
        let status = |height: u64, best: u64| {
            json!({"name":"Ting node","version":"0.1.0","network":"production","height":height,
                "best_height":best,"tip":"ab","mempool_count":1,"mempool_bytes":2048,"deferred_count":0,
                "limits":[100000000,1024,1048576,524288],"uptime_seconds":75,"replaying":false,
                "reorganization":null,"marked":[],"warnings":["check for an update"],
                "listeners":{"p2p":"0.0.0.0:18442","control":"127.0.0.1:18440","api":null}})
        };
        let none = json!({"items":[]});
        let one = json!({"items":[{"connected":1}]});
        let off = json!({"running":false});
        let syncing = super::status(&status(970, 5_420), &one, &off);
        assert!(syncing.starts_with("Ting node · production · 0.1.0 · up 1m 15s\n"));
        assert!(syncing.contains("Chain      syncing, height 970 of 5,420 (18%)\n"));
        assert!(syncing.contains("Peers      1 connected\n"));
        assert!(syncing.contains("Pool       1 pending (2 KiB), 0 deferred\n"));
        assert!(syncing.contains("REST       off\n"));
        assert!(syncing.contains("Warning    check for an update\n"));
        let synced = super::status(&status(5_420, 5_421), &one, &off);
        assert!(synced.contains("Chain      height 5,420, in sync\n"));
        let alone = super::status(&status(325, 325), &none, &off);
        assert!(alone.contains("Chain      height 325, waiting for peers\n"));
    }

    #[test]
    fn peers_are_a_table_and_the_rest_say_why() {
        let now = ting_node::now();
        let text = peers(&json!({"discovery":true,"known":3,"max_peers":8,"items":[
            {"address":"203.0.113.4:18442","agent":"ting-node/0.1.0","connected":now - 7_200,
             "height":5420,"last_block":now - 10_800,"source":"discovered","error":null},
            {"address":"198.51.100.7:18442","source":"config","connected":null,
             "error":"connection refused","last_attempt":now - 86_400},
            {"address":"198.51.100.8:18442","source":"config","error":null,"last_success":null}]}));
        assert_eq!(
            text,
            "ADDRESS            SOFTWARE         HEIGHT  CONNECTED  LAST BLOCK  SOURCE\n\
             203.0.113.4:18442  ting-node/0.1.0  5,420   2h ago     3h ago      discovered\n\
             \n\
             Not connected:\n  \
             198.51.100.7:18442 (configured): connection refused, 1d ago\n  \
             198.51.100.8:18442 (configured): not tried yet\n\
             \n\
             1 connected of up to 8 · 3 addresses known · discovery on\n"
        );
        assert!(
            peers(&json!({"discovery":true,"known":0,"max_peers":8,"items":[]}))
                .starts_with("No peers connected.\n")
        );
    }
}
