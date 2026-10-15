//! Bundled learning guide. Pages come only from this registry, never from
//! request-controlled paths or the filesystem.
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
use std::{collections::BTreeSet, fmt::Write};

pub struct Page {
    /// URL below /wiki; a slash makes the page a child of the page named before it.
    pub path: &'static str,
    pub title: &'static str,
    /// One sentence for section cards.
    pub summary: &'static str,
    /// Source file relative to the node crate.
    pub file: &'static str,
    source: &'static str,
}
pub struct Group {
    pub label: &'static str,
    pub pages: &'static [Page],
}

macro_rules! page {
    ($path:literal, $title:literal, $file:literal, $summary:literal $(,)?) => {
        Page {
            path: $path,
            title: $title,
            summary: $summary,
            file: $file,
            source: include_str!(concat!("../", $file)),
        }
    };
}

/// Reading order: sidebar order, previous/next links and section cards follow it.
pub static GROUPS: &[Group] = &[
    Group {
        label: "Start here",
        pages: &[
            page!(
                "",
                "Why Ting",
                "wiki/index.md",
                "The problem Ting solves and what a frozen consensus gives you."
            ),
            page!(
                "tour",
                "A tour of Ting",
                "wiki/tour.md",
                "Follow one transaction from its bytes to a block every node agrees on."
            ),
            page!(
                "faq",
                "Common questions",
                "wiki/faq.md",
                "Short answers to what newcomers ask first."
            ),
            page!(
                "glossary",
                "Glossary",
                "wiki/glossary.md",
                "The terms used throughout the wiki."
            ),
        ],
    },
    Group {
        label: "Hands-on",
        pages: &[
            page!(
                "start/node",
                "Start a node",
                "wiki/start/node.md",
                "Install, initialize and run a node, or a development chain to experiment on."
            ),
            page!(
                "start/explore",
                "Explore the chain",
                "wiki/start/explore.md",
                "Use the explorer and REST API to look at what your node validated."
            ),
            page!(
                "start/first-program",
                "Deploy a first program",
                "wiki/start/first-program.md",
                "Deploy a tiny program on a development chain, then find its receipt and stored value."
            ),
            page!(
                "start/network",
                "Connect two nodes",
                "wiki/start/network.md",
                "Run a small network on one machine or across several."
            ),
        ],
    },
    Group {
        label: "Concepts",
        pages: &[
            page!(
                "consensus",
                "How nodes agree",
                "wiki/consensus.md",
                "Validity, ordering and the branch every node follows."
            ),
            page!(
                "consensus/genesis",
                "Genesis and network identity",
                "wiki/consensus/genesis.md",
                "The fixed configuration that defines a network."
            ),
            page!(
                "consensus/proof-of-work",
                "Proof of work and the heaviest chain",
                "wiki/consensus/proof-of-work.md",
                "How mined work selects one branch, and why there is no fixed finality."
            ),
            page!(
                "consensus/difficulty",
                "Time and difficulty",
                "wiki/consensus/difficulty.md",
                "Timestamp rules and the per-block ASERT retarget."
            ),
            page!(
                "consensus/capacity-voting",
                "Capacity voting",
                "wiki/consensus/capacity-voting.md",
                "How miners move block capacity, and what they cannot change."
            ),
            page!(
                "data",
                "Blocks and state",
                "wiki/data.md",
                "What a block contains and how it commits to everything it changes."
            ),
            page!(
                "data/encoding",
                "Canonical bytes and hashing",
                "wiki/data/encoding.md",
                "One encoding per value, domain-separated SHA-256 and ordered commitments."
            ),
            page!(
                "data/blocks",
                "Blocks, transactions and receipts",
                "wiki/data/blocks.md",
                "The header, the two transaction kinds and the receipts nodes derive."
            ),
            page!(
                "data/records",
                "Contracts and records",
                "wiki/data/records.md",
                "How contracts and their storage are identified in one flat record map."
            ),
            page!(
                "data/state-tree",
                "The state tree",
                "wiki/data/state-tree.md",
                "The sparse Merkle tree behind the state root."
            ),
            page!(
                "resources",
                "Four resources",
                "wiki/resources.md",
                "Why execution is measured with four independent counters."
            ),
            page!(
                "resources/counters",
                "What each counter measures",
                "wiki/resources/counters.md",
                "Compute, state access, data and growth, event by event."
            ),
            page!(
                "resources/prices",
                "The price list",
                "wiki/resources/prices.md",
                "What every compute charge costs, the memory tiers and where the numbers come from."
            ),
            page!(
                "resources/budgets",
                "Budgets and exhaustion",
                "wiki/resources/budgets.md",
                "Nested allowances, atomic debits and why nothing is refunded."
            ),
            page!(
                "resources/growth",
                "Persistent growth",
                "wiki/resources/growth.md",
                "Gross allocation accounting for state that outlives a transaction."
            ),
            page!(
                "resources/capacity",
                "Block capacity and bounds",
                "wiki/resources/capacity.md",
                "Block totals, capacity floors and the bounds that follow from the counters."
            ),
            page!(
                "vm",
                "Programs and the VM",
                "wiki/vm.md",
                "Immutable WebAssembly programs with persistent records."
            ),
            page!(
                "vm/profile",
                "The execution profile",
                "wiki/vm/profile.md",
                "The exact integer WebAssembly subset every node accepts."
            ),
            page!(
                "vm/format",
                "The binary format",
                "wiki/vm/format.md",
                "The exact bytes a module may consist of, with one encoding per construct."
            ),
            page!(
                "vm/validation",
                "Validation",
                "wiki/vm/validation.md",
                "How a module is typed, bounded and measured before it can run."
            ),
            page!(
                "vm/instructions",
                "Instruction reference",
                "wiki/vm/instructions.md",
                "All 111 accepted instructions with their encodings and exact meaning."
            ),
            page!(
                "vm/execution",
                "The abstract machine",
                "wiki/vm/execution.md",
                "Values, the stack, labels, instances, host calls and how an invocation ends."
            ),
            page!(
                "vm/lifecycle",
                "Deploy and invoke",
                "wiki/vm/lifecycle.md",
                "What happens when code is installed and each time it runs."
            ),
            page!(
                "vm/host",
                "The host interface",
                "wiki/vm/host.md",
                "The twelve imports a program uses to talk to the chain."
            ),
            page!(
                "vm/calls",
                "Calls, rollback and read-only",
                "wiki/vm/calls.md",
                "Nested invocations, checkpoints and inherited restrictions."
            ),
            page!(
                "vm/metering",
                "Exact metering",
                "wiki/vm/metering.md",
                "How every instruction is counted the same way on every node."
            ),
        ],
    },
    Group {
        label: "Building applications",
        pages: &[
            page!(
                "applications",
                "What applications build",
                "wiki/applications.md",
                "The boundary between the core and program-defined meaning."
            ),
            page!(
                "applications/authentication",
                "Authentication and replay",
                "wiki/applications/authentication.md",
                "Signed intents and one-time execution are program logic."
            ),
            page!(
                "applications/upgrades",
                "Upgrades through programs",
                "wiki/applications/upgrades.md",
                "Evolving an application when installed code never changes."
            ),
            page!(
                "applications/integrations",
                "Following the chain",
                "wiki/applications/integrations.md",
                "Reading receipts, events and storage from outside, safely across reorganizations."
            ),
            page!(
                "applications/writing-programs",
                "Writing a program",
                "wiki/applications/writing-programs.md",
                "Producing a module the profile accepts, and keeping it cheap to run."
            ),
            page!(
                "applications/security",
                "Security notes",
                "wiki/applications/security.md",
                "Assumptions a program must not make."
            ),
        ],
    },
    Group {
        label: "Reference programs",
        pages: &[
            page!(
                "programs",
                "Reference programs",
                "wiki/programs.md",
                "Frozen programs for common needs, published with Ting but not part of its protocol."
            ),
            page!(
                "programs/conventions",
                "Shared conventions",
                "wiki/programs/conventions.md",
                "Accounts, the policy interface, producer data and events the programs share."
            ),
            page!(
                "programs/policies",
                "Signature policies",
                "wiki/programs/policies.md",
                "secp256k1, multisignature and P-256 authorization as programs."
            ),
            page!(
                "programs/identity",
                "Identity registry",
                "wiki/programs/identity.md",
                "Identities with several keys, guardians, delays and recovery."
            ),
            page!(
                "programs/coin",
                "Coin",
                "wiki/programs/coin.md",
                "One program for every coin: supply, transfers, fees, scheduling and rewards."
            ),
            page!(
                "programs/sha256",
                "SHA-256",
                "wiki/programs/sha256.md",
                "A hash function any program can call."
            ),
            page!(
                "programs/batch",
                "Batch",
                "wiki/programs/batch.md",
                "Several calls in one transaction, so one account can pay for another's operation."
            ),
        ],
    },
    Group {
        label: "Running a node",
        pages: &[
            page!(
                "node",
                "The reference node",
                "wiki/node.md",
                "What ting does, and what is node policy rather than consensus."
            ),
            page!(
                "node/mining",
                "Mining and voting",
                "wiki/node/mining.md",
                "Block assembly, the mempool, producer data and capacity votes."
            ),
            page!(
                "node/sync",
                "Peers and synchronization",
                "wiki/node/sync.md",
                "How a node finds its peers and exchanges blocks and transactions with them."
            ),
            page!(
                "node/storage",
                "Storage and recovery",
                "wiki/node/storage.md",
                "Block files, the state database, restarts and reorganizations."
            ),
            page!(
                "node/security",
                "Access and exposure",
                "wiki/node/security.md",
                "The control channel, the public REST interface and what to expose."
            ),
        ],
    },
    Group {
        label: "Design decisions",
        pages: &[
            page!(
                "decisions",
                "Design commitments",
                "wiki/decisions.md",
                "The ten commitments every other rule follows from."
            ),
            page!(
                "decisions/freeze",
                "Freeze consensus once",
                "wiki/decisions/freeze.md",
                "Why the rules are meant to stop changing."
            ),
            page!(
                "decisions/no-native-coin",
                "No native coin or accounts",
                "wiki/decisions/no-native-coin.md",
                "Why assets, fees and identities are application territory."
            ),
            page!(
                "decisions/crypto-in-programs",
                "Cryptography lives in programs",
                "wiki/decisions/crypto-in-programs.md",
                "Why signature schemes and hash libraries are deployed, not built in."
            ),
            page!(
                "decisions/four-counters",
                "Four counters, not one price",
                "wiki/decisions/four-counters.md",
                "Why compute, state access, data and growth stay separate."
            ),
            page!(
                "decisions/prices-not-caps",
                "Prices, not caps",
                "wiki/decisions/prices-not-caps.md",
                "Why byte work is priced and nothing a counter can measure is capped."
            ),
            page!(
                "decisions/no-refunds",
                "Nothing is refunded",
                "wiki/decisions/no-refunds.md",
                "Why rollback restores state but never consumed resources."
            ),
            page!(
                "decisions/wasm-profile",
                "A dated WebAssembly subset",
                "wiki/decisions/wasm-profile.md",
                "Why instruction meaning is inherited from a fixed standard."
            ),
            page!(
                "decisions/capacity",
                "Capacity moves, rules do not",
                "wiki/decisions/capacity.md",
                "How throughput can change without changing the protocol."
            ),
        ],
    },
    Group {
        label: "Reference",
        pages: &[page!(
            "reference/conformance",
            "Conformance and testing",
            "wiki/reference/conformance.md",
            "How implementations show they agree, byte for byte."
        )],
    },
];

const VECTORS: &str = "reference/protocol-vectors.json";
/// Diagrams, by file name, drawn into the page where a document shows them as
/// an image (`![Caption](../diagrams/name.svg)`), so that they take the page's
/// colors in light and dark mode. Each is a plain SVG with its own fallback
/// colors, so the same file also shows as an image outside the wiki.
static DIAGRAMS: &[(&str, &str)] = &[
    ("sync.svg", include_str!("../wiki/diagrams/sync.svg")),
    (
        "connection.svg",
        include_str!("../wiki/diagrams/connection.svg"),
    ),
    (
        "reorganization.svg",
        include_str!("../wiki/diagrams/reorganization.svg"),
    ),
    (
        "block-status.svg",
        include_str!("../wiki/diagrams/block-status.svg"),
    ),
];
fn diagram(url: &str) -> Option<&'static str> {
    let name = url.rsplit('/').next()?;
    DIAGRAMS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, svg)| *svg)
}

impl Page {
    pub fn url(&self) -> String {
        if self.path.is_empty() {
            "/wiki".into()
        } else {
            format!("/wiki/{}", self.path)
        }
    }
    pub fn parent(&self) -> Option<&'static Page> {
        find(self.path.rsplit_once('/')?.0)
    }
    pub fn children(&self) -> impl Iterator<Item = &'static Page> + '_ {
        all().filter(|p| p.parent().is_some_and(|parent| parent.path == self.path))
    }
    fn group(&self) -> &'static Group {
        GROUPS
            .iter()
            .find(|g| g.pages.iter().any(|p| p.path == self.path))
            .expect("registered page")
    }
}
pub fn all() -> impl Iterator<Item = &'static Page> {
    GROUPS.iter().flat_map(|g| g.pages.iter())
}
pub fn find(path: &str) -> Option<&'static Page> {
    all().find(|p| p.path == path)
}
/// A page path with a trailing slash points at the page.
pub fn redirect(path: &str) -> Option<String> {
    Some(find(path.strip_suffix('/')?)?.url())
}
/// Non-page files linked from bundled documents: the protocol vectors, and
/// the reference programs with their manifest, from the repository's
/// `programs/` folder.
pub fn file(path: &str) -> Option<(&'static str, &'static [u8])> {
    const JSON: &str = "application/json; charset=utf-8";
    const WASM: &str = "application/wasm";
    Some(match path {
        VECTORS => (
            JSON,
            &include_bytes!("../wiki/reference/protocol-vectors.json")[..],
        ),
        "programs/manifest.json" => (JSON, &include_bytes!("../../programs/manifest.json")[..]),
        "programs/policy-secp256k1.wasm" => (
            WASM,
            &include_bytes!("../../programs/policy-secp256k1.wasm")[..],
        ),
        "programs/policy-multisig-secp256k1.wasm" => (
            WASM,
            &include_bytes!("../../programs/policy-multisig-secp256k1.wasm")[..],
        ),
        "programs/policy-p256.wasm" => {
            (WASM, &include_bytes!("../../programs/policy-p256.wasm")[..])
        }
        "programs/id-registry.wasm" => {
            (WASM, &include_bytes!("../../programs/id-registry.wasm")[..])
        }
        "programs/coin.wasm" => (WASM, &include_bytes!("../../programs/coin.wasm")[..]),
        "programs/sha256.wasm" => (WASM, &include_bytes!("../../programs/sha256.wasm")[..]),
        "programs/batch.wasm" => (WASM, &include_bytes!("../../programs/batch.wasm")[..]),
        _ => return None,
    })
}

struct Heading {
    level: usize,
    id: String,
    text: String,
}
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').into()
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
        .replace('"', "&quot;")
}
/// Relative links between repository documents become wiki links.
fn rewrite(url: &str) -> Option<String> {
    let (base, fragment) = url
        .split_once('#')
        .map_or((url, None), |(b, f)| (b, Some(f)));
    if base.is_empty() || base.starts_with('/') || base.contains(':') {
        return None;
    }
    let target = if base == "protocol-vectors.json" {
        format!("/wiki/{VECTORS}")
    } else {
        all()
            .find(|p| p.file.rsplit('/').next() == Some(base))?
            .url()
    };
    Some(fragment.map_or(target.clone(), |f| format!("{target}#{f}")))
}
fn render(source: &str) -> (String, Vec<Heading>) {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_GFM;
    let mut events: Vec<Event> = Parser::new_ext(source, options).collect();
    let mut headings = Vec::new();
    let mut ids = BTreeSet::new();
    let mut i = 0;
    while i < events.len() {
        // A diagram shown as an image is drawn into the page as a figure with
        // its caption; a paragraph holding only the diagram is replaced whole.
        if let Event::Start(Tag::Image { dest_url, .. }) = &events[i]
            && let Some(svg) = diagram(dest_url)
        {
            let end = i + events[i..]
                .iter()
                .position(|e| matches!(e, Event::End(TagEnd::Image)))
                .expect("closed image");
            let caption: String = events[i + 1..end]
                .iter()
                .filter_map(|e| match e {
                    Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
                    _ => None,
                })
                .collect();
            let alone = i > 0
                && matches!(events[i - 1], Event::Start(Tag::Paragraph))
                && matches!(events.get(end + 1), Some(Event::End(TagEnd::Paragraph)));
            let (from, to) = if alone { (i - 1, end + 1) } else { (i, end) };
            let figure = format!(
                "<figure class='diagram'>{}<figcaption>{}</figcaption></figure>",
                svg.trim(),
                escape(&caption)
            );
            events.splice(from..=to, [Event::Html(figure.into())]);
            i = from + 1;
            continue;
        }
        if let Event::Start(Tag::Link { dest_url, .. }) = &mut events[i]
            && let Some(url) = rewrite(dest_url)
        {
            *dest_url = url.into();
        }
        if let Event::Start(Tag::Heading { level, .. }) = events[i] {
            let end = i + events[i..]
                .iter()
                .position(|e| matches!(e, Event::End(TagEnd::Heading(_))))
                .expect("closed heading");
            let text: String = events[i + 1..end]
                .iter()
                .filter_map(|e| match e {
                    Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
                    _ => None,
                })
                .collect();
            let base = slug(&text);
            let mut id = base.clone();
            for n in 2.. {
                if ids.insert(id.clone()) {
                    break;
                }
                id = format!("{base}-{n}");
            }
            if let Event::Start(Tag::Heading { id: slot, .. }) = &mut events[i] {
                *slot = Some(id.clone().into());
            }
            if level != HeadingLevel::H1 {
                events.insert(
                    end,
                    Event::InlineHtml(
                        format!(
                            "<a class='anchor' href='#{id}' aria-label='Link to this section'>#</a>"
                        )
                        .into(),
                    ),
                );
            }
            headings.push(Heading {
                level: level as usize,
                id,
                text,
            });
        }
        i += 1;
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    (body, headings)
}
fn link(page: &Page, selected: &str, class: &str) -> String {
    format!(
        "<a href='{}'{}{}>{}</a>",
        page.url(),
        if class.is_empty() {
            String::new()
        } else {
            format!(" class='{class}'")
        },
        if page.path == selected {
            " aria-current='page'"
        } else {
            ""
        },
        escape(page.title)
    )
}
/// Sidebar for wide screens and a collapsible contents list for narrow ones.
pub fn navigation(selected: &str) -> String {
    let current = find(selected);
    let section = current.map(|p| p.parent().map_or(p.path, |parent| parent.path));
    let mut links = String::new();
    for group in GROUPS {
        write!(links, "<p class='nav-label'>{}</p>", group.label).unwrap();
        for page in group.pages.iter().filter(|p| p.parent().is_none()) {
            let children: Vec<_> = page.children().collect();
            let open = !children.is_empty() && section == Some(page.path);
            let class = if children.is_empty() || open {
                ""
            } else {
                "has-children"
            };
            links += &link(page, selected, class);
            if open {
                links += "<div class='nav-children'>";
                for child in children {
                    links += &link(child, selected, "");
                }
                links += "</div>";
            }
        }
    }
    format!(
        "<nav class='side-nav wiki-nav' aria-label='Wiki chapters'>{links}</nav>\
         <details class='wiki-mobile-nav'><summary>Contents<span>{}</span></summary>\
         <nav aria-label='Wiki chapters, compact'>{links}</nav></details>",
        escape(current.map_or("Wiki", |p| p.title))
    )
}

fn contents(headings: &[Heading], depth: usize) -> String {
    let items: String = headings
        .iter()
        .filter(|h| h.level >= 2 && h.level <= depth)
        .map(|h| {
            format!(
                "<li{}><a href='#{}'>{}</a></li>",
                if h.level > 2 { " class='toc-sub'" } else { "" },
                h.id,
                escape(&h.text)
            )
        })
        .collect();
    format!("<ol>{items}</ol>")
}
fn pager(index: usize) -> String {
    let pages: Vec<_> = all().collect();
    let mut out = String::from("<nav class='pager' aria-label='Previous and next page'>");
    if let Some(prev) = index.checked_sub(1).map(|i| pages[i]) {
        write!(
            out,
            "<a class='prev' href='{}'><span>Previous</span>{}</a>",
            prev.url(),
            escape(prev.title)
        )
        .unwrap();
    }
    if let Some(next) = pages.get(index + 1) {
        write!(
            out,
            "<a class='next' href='{}'><span>Next</span>{}</a>",
            next.url(),
            escape(next.title)
        )
        .unwrap();
    }
    out + "</nav>"
}

pub fn shell(title: &str, selected: &str, body: &str) -> String {
    crate::web::page(title, crate::web::Section::Wiki, selected, body, "", "")
}
pub fn not_found() -> String {
    shell(
        "Page not found",
        "\0",
        "<div class='wiki-page'><article class='wiki-article'><h1>Page not found</h1>\
         <p>This wiki page does not exist. <a href='/wiki'>Start from the beginning</a> \
         or pick a chapter from the contents.</p></article></div>",
    )
}
pub fn page(path: &str) -> Option<String> {
    let page = find(path)?;
    let index = all().position(|p| p.path == path)?;
    let (mut body, headings) = render(page.source);
    let toc = !page.path.is_empty() && headings.iter().filter(|h| h.level == 2).count() >= 3;
    let mut html = String::new();
    if page.path.is_empty() {
        html += "<div class='wiki-page wiki-home'><article class='wiki-article'>";
        html += &body;
    } else {
        if toc {
            let inline = format!(
                "<details class='toc-inline'><summary>On this page</summary>{}</details>",
                contents(&headings, 2)
            );
            if let Some(end) = body.find("</h1>") {
                body.insert_str(end + 5, &inline);
            }
        }
        let minutes = page.source.split_whitespace().count().div_ceil(220).max(1);
        write!(
            html,
            "<div class='wiki-page{}'><article class='wiki-article'><nav class='wiki-meta' aria-label='Breadcrumb'><span>{}</span>",
            if toc { " has-toc" } else { "" },
            page.group().label
        )
        .unwrap();
        if let Some(parent) = page.parent() {
            write!(
                html,
                "<span class='sep'>›</span><a href='{}'>{}</a>",
                parent.url(),
                escape(parent.title)
            )
            .unwrap();
        }
        write!(
            html,
            "<span class='sep'>·</span><span>{minutes} min read</span></nav>"
        )
        .unwrap();
        html += &body;
    }
    let children: Vec<_> = page.children().collect();
    if !children.is_empty() {
        html += "<section class='subpages' aria-label='In this section'><h2>In this section</h2><div class='cards'>";
        for child in children {
            write!(
                html,
                "<a class='card' href='{}'><strong>{}</strong><span>{}</span></a>",
                child.url(),
                escape(child.title),
                escape(child.summary)
            )
            .unwrap();
        }
        html += "</div></section>";
    }
    html += &pager(index);
    html += "</article>";
    if toc {
        write!(
            html,
            "<nav class='toc' aria-label='On this page'><p class='toc-label'>On this page</p>{}</nav>",
            contents(&headings, 2)
        )
        .unwrap();
    }
    html += "</div>";
    Some(shell(page.title, path, &html))
}
