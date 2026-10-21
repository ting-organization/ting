use crate::{Result, daemon::Node, reads, web, wiki};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, FromRequest, Path, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct ApiState {
    node: Node,
    /// Requests in progress.
    slots: Arc<Semaphore>,
    /// REST's own reads off the chain thread.
    readers: reads::Readers,
}
/// Requests REST takes on at once for each processor core.
pub const SLOTS_PER_CORE: usize = 32;
/// How old the published status may be before the chain thread counts as
/// not answering: as long as a request to it may take.
const STALE: Duration = Duration::from_secs(30);
/// The REST router; with `cors`, browser applications from any origin may use it.
pub fn router(node: Node, cors: bool) -> Router {
    let state = ApiState {
        readers: reads::Readers::new(node.handle.reader().clone()),
        node,
        slots: Arc::new(Semaphore::new(
            SLOTS_PER_CORE
                .saturating_mul(reads::cores())
                .min(Semaphore::MAX_PERMITS),
        )),
    };
    let router = Router::new()
        .route("/", get(explorer))
        .route("/blocks", get(explorer))
        .route("/blocks/{id}", get(explorer))
        .route("/transactions/{id}", get(explorer))
        .route("/contracts/{id}", get(explorer))
        .route("/mempool", get(explorer))
        .route("/network", get(explorer))
        .route("/assets/{name}", get(asset))
        .route("/wiki", get(|| async { Html(wiki::page("").unwrap()) }))
        .route("/wiki/{*page}", get(wiki_page))
        .route("/docs", get(|| async { Html(web::api_docs()) }))
        .route("/openapi.json", get(|| async { Json(openapi()) }))
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/blocks", get(blocks))
        .route("/v1/blocks/{id}", get(block))
        .route("/v1/blocks/{id}/raw", get(raw))
        .route("/v1/transactions/{id}", get(transaction))
        // A submission's body is bounded by the data capacity instead.
        .route(
            "/v1/transactions",
            post(submit).layer(DefaultBodyLimit::disable()),
        )
        .route("/v1/mempool", get(mempool))
        .route("/v1/votes", get(votes))
        .route("/v1/events", get(events))
        .route("/v1/contracts/{id}", get(contract))
        .route("/v1/contracts/{id}/storage/{key}", get(storage))
        .route("/v1/storage", post(storage_batch))
        .route("/v1/metrics", get(status))
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"endpoint not found"})),
            )
        })
        .layer(DefaultBodyLimit::max(crate::FRAME_MAX));
    // CORS sits inside the bound, so preflight answers take a slot and carry
    // the same headers as everything else.
    let router = if cors {
        router.layer(CorsLayer::permissive())
    } else {
        router
    };
    router
        .layer(middleware::from_fn_with_state(state.clone(), bounded))
        .with_state(state)
}
async fn bounded(State(s): State<ApiState>, request: Request, next: Next) -> Response {
    let json_endpoint =
        request.uri().path().starts_with("/v1/") || request.uri().path() == "/health";
    let Ok(_permit) = s.slots.clone().try_acquire_owned() else {
        return secured(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"HTTP service busy"})),
            )
                .into_response(),
        );
    };
    // A submission also has the time its body may take to arrive, as a peer
    // has for a transaction of that size.
    let deadline = if request.method() == axum::http::Method::POST {
        let body = submission_limit(s.node.handle.events().borrow().data);
        Duration::from_secs(35) + crate::chain::transfer_time(body as u64)
            - crate::chain::transfer_time(0)
    } else {
        Duration::from_secs(35)
    };
    match tokio::time::timeout(deadline, next.run(request)).await {
        Ok(mut response) => {
            if json_endpoint
                && response.status().is_client_error()
                && !response
                    .headers()
                    .get("content-type")
                    .is_some_and(|h| h.as_bytes().starts_with(b"application/json"))
            {
                let status = response.status();
                let message = match axum::body::to_bytes(response.into_body(), 8192).await {
                    Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).into_owned(),
                    Ok(_) => status
                        .canonical_reason()
                        .unwrap_or("request rejected")
                        .into(),
                    Err(_) => "request rejected".into(),
                };
                response = (status, Json(json!({"error":message}))).into_response();
            }
            secured(response)
        }
        Err(_) => secured(
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({"error":"HTTP deadline exceeded"})),
            )
                .into_response(),
        ),
    }
}
/// Every response carries the same protective headers (wiki: node/security).
fn secured(mut response: Response) -> Response {
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response
}
/// A node error as an HTTP status: what the client asked for is 400 or 404;
/// what the node cannot do right now is 503, or 504 when it ran out of time.
fn respond(result: Result<Value>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            let status = if error.contains("not found") {
                StatusCode::NOT_FOUND
            } else if error.contains("timed out") {
                StatusCode::GATEWAY_TIMEOUT
            } else if error.contains("queue")
                || error.contains("busy")
                || error.contains("is full")
                || error.contains("restart required")
                || error.contains("stopped")
                || error.contains("storage")
            {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(json!({"error":error}))).into_response()
        }
    }
}
async fn explorer() -> Html<String> {
    Html(web::explorer())
}
async fn asset(Path(name): Path<String>) -> Response {
    match web::asset(&name) {
        Some((mime, content)) => ([("content-type", mime)], content).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"asset not found"})),
        )
            .into_response(),
    }
}
async fn wiki_page(Path(page): Path<String>) -> Response {
    if let Some(html) = wiki::page(&page) {
        Html(html).into_response()
    } else if let Some((mime, body)) = wiki::file(&page) {
        ([("content-type", mime)], body).into_response()
    } else if let Some(target) = wiki::redirect(&page) {
        Redirect::permanent(&target).into_response()
    } else {
        (StatusCode::NOT_FOUND, Html(wiki::not_found())).into_response()
    }
}
/// A read of stored chain data, on a thread of its own beside the chain
/// thread, from one consistent view of the database.
async fn read(s: &ApiState, method: &'static str, params: Value) -> Result<Value> {
    s.readers.answer(method, params).await
}
/// The status the chain thread last published.
fn published(s: &ApiState) -> Result<Value> {
    let published = s.node.handle.status();
    if published.at.elapsed() > STALE {
        return Err("node request timed out".into());
    }
    (*published.status).clone()
}
async fn health(State(s): State<ApiState>) -> Response {
    respond(published(&s).map(|v| json!({"ready":true,"height":v["height"],"tip":v["tip"]})))
}
async fn status(State(s): State<ApiState>) -> Response {
    respond(published(&s))
}
async fn mempool(State(s): State<ApiState>) -> Response {
    respond(s.node.handle.call_rest("mempool", json!({})).await)
}
macro_rules! object {
    ($name:ident,$method:literal)=>{async fn $name(State(s):State<ApiState>,Path(id):Path<String>)->Response {respond(read(&s,$method,json!({"id":id})).await)}};
}
object!(block, "block");
object!(contract, "contract");
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Slice {
    offset: Option<u64>,
    length: Option<u64>,
}
/// Canonical block bytes, whole or as a slice: `length` is at most 1 MiB and
/// a block beyond about 8 MiB is only served in slices.
async fn raw(
    State(s): State<ApiState>,
    Path(id): Path<String>,
    Query(q): Query<Slice>,
) -> Response {
    respond(
        read(
            &s,
            "raw",
            json!({"id":id,"offset":q.offset,"length":q.length}),
        )
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    from: Option<u64>,
    limit: Option<u64>,
}
/// A transaction's canonical occurrences, read off the chain thread, and
/// whether it is pending, which only the chain thread knows.
async fn transaction(
    State(s): State<ApiState>,
    Path(id): Path<String>,
    Query(p): Query<Page>,
) -> Response {
    let lookup = async {
        let stored = read(
            &s,
            "transaction",
            json!({"id":id,"from":p.from,"limit":p.limit}),
        )
        .await?;
        let pending = s
            .node
            .handle
            .call_rest("transaction_pending", json!({"id":id}))
            .await?;
        reads::finish(stored, pending)
    };
    respond(lookup.await)
}
/// Where capacity voting stands, from the chain thread, which counts it once
/// per tip.
async fn votes(State(s): State<ApiState>) -> Response {
    respond(s.node.handle.call_rest("votes", json!({})).await)
}
async fn blocks(State(s): State<ApiState>, Query(p): Query<Page>) -> Response {
    respond(read(&s, "blocks", json!({"from":p.from,"limit":p.limit})).await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventQuery {
    from: Option<u64>,
    limit: Option<u64>,
    /// Only the events of this program.
    contract: Option<String>,
    /// Only those whose payload begins with this topic.
    topic: Option<String>,
}
async fn events(State(s): State<ApiState>, Query(p): Query<EventQuery>) -> Response {
    respond(
        read(
            &s,
            "events",
            json!({"from":p.from,"limit":p.limit,"contract":p.contract,"topic":p.topic}),
        )
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proved {
    /// Also the record's Merkle proof against the state root.
    proof: Option<bool>,
}
async fn storage(
    State(s): State<ApiState>,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<Proved>,
) -> Response {
    respond(
        read(
            &s,
            "storage",
            json!({"id":id,"key":key,"proof":q.proof.unwrap_or(false)}),
        )
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageRead {
    contract: String,
    key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageReads {
    reads: Vec<StorageRead>,
    /// Also each record's Merkle proof against the state root.
    proof: Option<bool>,
}
/// Many storage records from one view of the state, read off the chain
/// thread.
async fn storage_batch(State(s): State<ApiState>, Json(body): Json<StorageReads>) -> Response {
    let reads: Vec<Value> = body
        .reads
        .iter()
        .map(|r| json!({"contract":r.contract,"key":r.key}))
        .collect();
    respond(
        read(
            &s,
            "storage_batch",
            json!({"reads":reads,"proof":body.proof.unwrap_or(false)}),
        )
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    hex: String,
    /// Hold the transaction until this time, then include it once it executes
    /// successfully (deferred submission).
    not_before: Option<u64>,
}
/// The largest body a submission may have: a transaction of the next block's
/// data capacity, in hex, and 4 KiB for the rest.
fn submission_limit(capacity: u64) -> usize {
    usize::try_from(capacity)
        .unwrap_or(usize::MAX)
        .saturating_mul(2)
        .saturating_add(4096)
}
/// The address a public REST submission counts against in the pool and the
/// deferred set (wiki: node/mining): the client's own, or, on a connection from
/// this machine, the last address a reverse proxy on it appends to
/// `X-Forwarded-For`. None for a local client without one, which is the
/// operator's own.
fn client_address(request: &Request) -> Option<IpAddr> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()?
        .0
        .ip();
    if !peer.is_loopback() {
        return Some(peer);
    }
    request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .next_back()
        .and_then(|address| address.trim().parse::<IpAddr>().ok())
        .filter(|address| !address.is_loopback())
}
/// A submission: its body is bounded by the transaction it can carry, at most
/// the data capacity of the next block in hex, and a little more for the rest.
async fn submit(State(s): State<ApiState>, request: Request) -> Response {
    let limit = submission_limit(s.node.handle.events().borrow().data);
    let client = client_address(&request).map(|address| address.to_string());
    let (parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, limit).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error":"request body larger than a transaction can be"})),
        )
            .into_response();
    };
    let request = Request::from_parts(parts, Body::from(bytes));
    let p = match Json::<Envelope>::from_request(request, &s).await {
        Ok(Json(p)) => p,
        Err(rejection) => return rejection.into_response(),
    };
    respond(
        s.node
            .handle
            .call_rest(
                "submit",
                json!({"hex":p.hex,"not_before":p.not_before,"client":client}),
            )
            .await,
    )
}
pub fn openapi() -> Value {
    let mut paths = serde_json::Map::new();
    for (path, summary, schema) in [
        ("/health", "Readiness", "Health"),
        ("/v1/status", "Canonical node status", "Status"),
        ("/v1/blocks", "Canonical block page", "BlockPage"),
        ("/v1/blocks/{id}", "Block summary and receipts", "Block"),
        (
            "/v1/blocks/{id}/raw",
            "Canonical block bytes in hex, whole or sliced",
            "RawBlock",
        ),
        (
            "/v1/transactions/{id}",
            "Canonical occurrences and pending status",
            "TransactionLookup",
        ),
        ("/v1/mempool", "Pending transaction inventory", "Mempool"),
        (
            "/v1/votes",
            "Capacity voting: this epoch's votes so far and the capacities decided",
            "Voting",
        ),
        (
            "/v1/events",
            "Events by canonical block, of every program or of one",
            "EventPage",
        ),
        ("/v1/contracts/{id}", "Immutable code manifest", "Manifest"),
        (
            "/v1/contracts/{id}/storage/{key}",
            "Current record, null if absent",
            "Storage",
        ),
        ("/v1/metrics", "Node statistics", "Status"),
    ] {
        let mut parameters = Vec::new();
        if path.contains("{id}") {
            parameters.push(json!({"name":"id","in":"path","required":true,"description":if path.contains("blocks") {"Positive height or 64-character lowercase block hash"} else {"64-character lowercase identifier"},"schema":{"type":"string"}}));
        }
        if path.contains("{key}") {
            parameters.push(json!({"name":"key","in":"path","required":true,"schema":{"$ref":"#/components/schemas/Hash"}}));
        }
        if path == "/v1/contracts/{id}/storage/{key}" {
            parameters.push(json!({"name":"proof","in":"query","description":"Also the record's Merkle proof against state_root","schema":{"type":"boolean","default":false}}));
        }
        if path == "/v1/blocks/{id}/raw" {
            parameters.push(json!({"name":"offset","in":"query","description":"First byte of the slice","schema":{"type":"integer","minimum":0,"default":0}}));
            parameters.push(json!({"name":"length","in":"query","description":"Bytes in the slice, at most 1 MiB; without it the whole block, when it fits one response","schema":{"type":"integer","minimum":0,"maximum":crate::chain::SLICE_MAX}}));
        }
        if path == "/v1/blocks" || path == "/v1/events" || path == "/v1/transactions/{id}" {
            parameters.push(json!({"name":"from","in":"query","schema":{"type":"integer","minimum":if path.contains("transactions") {0} else {1},"default":if path.contains("transactions") {0} else {1}}}));
            parameters.push(json!({"name":"limit","in":"query","schema":{"type":"integer","minimum":0,"maximum":100,"default":20}}));
        }
        if path == "/v1/events" {
            parameters.push(json!({"name":"contract","in":"query","description":"Only the events of this program, and only the blocks that have any; next then continues past them","schema":{"$ref":"#/components/schemas/Hash"}}));
            parameters.push(json!({"name":"topic","in":"query","description":"With contract: only the events whose payload begins with these 32 bytes, the program's topic","schema":{"$ref":"#/components/schemas/Hash"}}));
        }
        let tag = if path.contains("blocks") {
            "Blocks"
        } else if path.contains("transactions") || path.contains("mempool") {
            "Transactions"
        } else if path.contains("contracts") {
            "Contracts"
        } else if path.contains("events") {
            "Events"
        } else {
            "Chain"
        };
        paths.insert(path.into(),json!({"get":{"summary":summary,"tags":[tag],"parameters":parameters,"responses":{
            "200":{"description":"Success","content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{schema}")}}}},
            "400":{"$ref":"#/components/responses/Error"},"404":{"$ref":"#/components/responses/Error"},"503":{"$ref":"#/components/responses/Error"}
        }}}));
    }
    {
        use ting_core::{
            meter::Resources,
            protocol::{Encode, Payload, Transaction},
        };
        let example = Transaction {
            budget: Resources([1_000_000, 10, 1024, 0]),
            payload: Payload::Call {
                target: [7; 32],
                input: vec![],
            },
        };
        paths.insert("/v1/transactions".into(),json!({"post":{"summary":"Submit canonical transaction","tags":["Transactions"],
            "description":"Public admission to the bounded transient mempool. No node token is required. Application authorization is checked by programs during execution. Admission does not imply successful execution. With not_before, the transaction is deferred: held until that time and admitted to the pool once it executes successfully on the tip, surviving restarts and reorganizations until a canonical block includes it or it expires. The example targets a placeholder contract; replace it with a real canonical envelope.",
            "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/Submission"},"example":{"hex":ting_core::hash::hex(&example.encode())}}}},
            "responses":{"200":{"description":"Accepted","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Admission"}}}},
                "400":{"$ref":"#/components/responses/Error"},"413":{"$ref":"#/components/responses/Error"},"503":{"$ref":"#/components/responses/Error"}}}}));
    }
    paths.insert("/v1/storage".into(),json!({"post":{"summary":"Read many storage records from one state","tags":["Contracts"],
        "description":format!("Reads 1 to {} records, each named by a contract and a key, all from the same state: one state root, which the answer gives with the canonical tip it belongs to. Records read together therefore never straddle a block. Values come in the order asked, null for an absent record; with proof, each with its Merkle proof against the state root.", reads::BATCH_MAX),
        "requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":"#/components/schemas/StorageReads"}}}},
        "responses":{"200":{"description":"The records","content":{"application/json":{"schema":{"$ref":"#/components/schemas/StorageValues"}}}},
            "400":{"$ref":"#/components/responses/Error"},"413":{"$ref":"#/components/responses/Error"},"503":{"$ref":"#/components/responses/Error"}}}}));
    let schemas: Value = serde_json::from_str(include_str!("../web/openapi-schemas.json"))
        .expect("bundled OpenAPI schemas are valid JSON");
    json!({"openapi":"3.1.0","info":{"title":"Ting consumer API","version":env!("CARGO_PKG_VERSION"),
    "description":"Consumer interface for validated blocks, contracts, receipts, events and public transaction submission. Resource order is C/S/D/G; binary fields are lowercase hex. Reorganizations can replace canonical history. Node administration uses the local CLI. Pagination defaults to 20 entries, caps at 100 and stops before 16 MiB; follow next, and has_more for occurrence pages."},
    "servers":[{"url":"/","description":"This Ting node"}],"security":[],"paths":paths,"components":{
        "responses":{"Error":{"description":"Request failed; error gives the reason","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Error"}}}}},
        "schemas":schemas
    }})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(peer: [u8; 4], forwarded: Option<&str>) -> Request {
        let mut builder = Request::builder();
        if let Some(value) = forwarded {
            builder = builder.header("x-forwarded-for", value);
        }
        let mut request = builder.body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((peer, 40_000))));
        request
    }

    #[test]
    fn a_submission_counts_against_the_client_or_the_address_a_local_proxy_names() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        // A client on the network is its own address, whatever it claims.
        assert_eq!(
            client_address(&request([192, 0, 2, 1], Some("198.51.100.4"))),
            ip("192.0.2.1")
        );
        // Behind a proxy on this machine, the last address the proxy appended.
        assert_eq!(
            client_address(&request([127, 0, 0, 1], Some("198.51.100.4, 203.0.113.7"))),
            ip("203.0.113.7")
        );
        // A local client without one, or naming this machine, is the operator.
        assert_eq!(client_address(&request([127, 0, 0, 1], None)), None);
        assert_eq!(
            client_address(&request([127, 0, 0, 1], Some("127.0.0.1"))),
            None
        );
        assert_eq!(
            client_address(&request([127, 0, 0, 1], Some("nonsense"))),
            None
        );
        // A request with no connection information, as in a test, is local.
        assert_eq!(client_address(&Request::new(Body::empty())), None);
    }
}
