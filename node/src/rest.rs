//! REST's connections (wiki: node/security#the-rest-interface). The node
//! takes at most so many at once, in all and from one client, and closes one
//! whose client is slow to send a request's header, leaves it idle, or does
//! not read an answer within the time its size allows. What a request may
//! take once its header is in, its body included, is `api`'s own bound.
use crate::{FRAME_MAX, chain::transfer_time, discovery};
use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::ConnectInfo,
};
use hyper::{
    body::{Frame, Incoming, SizeHint},
    server::conn::http1,
    service::Service as _,
    service::service_fn,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::{Pin, pin},
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    time::Instant,
};

/// The time a client has to send a request's header, counted from when its
/// connection opens or the connection's last answer was written, so that a
/// connection left idle for as long closes too. A header is a few hundred
/// bytes, one packet; TCP retransmits a lost one after 1, 2 and 4 seconds
/// (RFC 6298), so ten seconds outlast three losses in a row.
pub const HEADER_TIME: Duration = Duration::from_secs(10);
/// The header time of a connection from this machine, as a reverse proxy's
/// are: five minutes, longer than proxies keep an idle connection to a backend
/// (Caddy two minutes, Go's HTTP client 90 seconds, nginx 60), so that the
/// proxy closes it, never the node while a request of the proxy's is on its
/// way. Only processes on this machine open such connections, and the total
/// still bounds them.
pub const LOCAL_TIME: Duration = Duration::from_secs(300);
/// How long REST, once told to stop, lets its connections finish the
/// requests they are on: a second, in which an answer goes out over this
/// machine or a fast link; then it drops the rest, so that switching REST off
/// or stopping the node never waits on a slow client. An idle connection
/// closes at once.
pub const CLOSING: Duration = Duration::from_secs(1);
/// What a connection buffers: it reads a request's header into about this
/// much, and takes more of an answer to write only while less than this
/// waits, in pieces of as much (`api`), so less than twice this waits. A
/// header is rarely over a few KiB; servers commonly refuse one over 8 to 32
/// KiB. The default is 400 KiB.
pub const BUFFER: usize = 64 * 1024;
/// Most REST connections at once. Each costs the node a descriptor and a few
/// times `BUFFER` of memory, under 200 MiB for all. A browser or a wallet
/// holds a handful while it uses the node and gives them up ten seconds after
/// its last request, so this many serve about a hundred clients at once.
pub const CONNECTIONS_MAX: usize = 512;
/// Most connections at once from one client: an IPv4 address or an IPv6 /64,
/// the ranges peers are counted by. A browser opens up to six to one host and
/// an Android app's HTTP client five; sixteen leave room for a few clients
/// behind one address. Connections from this machine are not counted: behind a
/// reverse proxy on it every connection comes from there, and the proxy limits
/// its own clients.
pub const CLIENT_MAX: usize = 16;
/// Descriptors kept for the rest of the node whatever REST's traffic: one for
/// every peer connection the configuration allows, `max_inbound` in and
/// `max_peers` out, and one to each of up to 32 configured peers; then 64 for
/// everything else: the seeds' links, a probe and the extra peer of a stale
/// tip, 16 control connections, the 16 block files kept open for reading and
/// the one appended to, the database and the one a replay builds beside it,
/// the log, the three listeners, the standard streams and the runtime's own.
pub fn reserved(max_inbound: u32, max_peers: u32) -> u64 {
    u64::from(max_inbound)
        .saturating_add(u64::from(max_peers))
        .saturating_add(32 + 64)
}
/// REST's connections within a limit of `limit` descriptors once `reserved`
/// are kept for the rest of the node; `CONNECTIONS_MAX` where there is no
/// such limit.
pub fn connections(limit: Option<u64>, reserved: u64) -> usize {
    limit.map_or(CONNECTIONS_MAX, |limit| {
        usize::try_from(limit.saturating_sub(reserved))
            .unwrap_or(usize::MAX)
            .min(CONNECTIONS_MAX)
    })
}
/// Raises the process's limit on open descriptors to `wanted` if it is lower,
/// as far as the hard limit allows, and returns the limit then in force.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn descriptors(wanted: u64) -> Option<u64> {
    // `rlim_t` is 32 or 64 bits wide, and signed on some systems; a limit is
    // never negative.
    fn wide<T: TryInto<u64>>(n: T) -> u64 {
        n.try_into().unwrap_or(0)
    }
    fn narrow<T: TryFrom<u64>>(n: u64) -> Option<T> {
        T::try_from(n).ok()
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes only the live struct it is given.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    if wide(limit.rlim_cur) < wanted {
        let raised = libc::rlimit {
            rlim_cur: narrow(wanted).unwrap_or(limit.rlim_max).min(limit.rlim_max),
            rlim_max: limit.rlim_max,
        };
        // SAFETY: setrlimit only reads the struct, which outlives the call.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            limit = raised;
        }
    }
    Some(wide(limit.rlim_cur))
}
/// No limit to raise: Windows bounds a process's sockets and files only by
/// its memory.
#[cfg(not(unix))]
pub fn descriptors(_wanted: u64) -> Option<u64> {
    None
}
/// What REST allows its connections.
#[derive(Clone, Copy)]
pub struct Limits {
    /// Most connections at once, at most `CONNECTIONS_MAX`.
    pub connections: usize,
    /// Most at once from one client other than this machine.
    pub per_client: usize,
    /// The time to send a request's header, or to leave a connection idle.
    pub header: Duration,
    /// The same for a connection from this machine.
    pub local: Duration,
    /// The time an answer of so many bytes has to be written.
    pub send: fn(u64) -> Duration,
}
impl Limits {
    /// `connections` at once, and the other limits as described above. An
    /// answer has the time a peer has to deliver an item of its size, a
    /// minute and ten seconds for every MiB: 220 seconds for the largest, 16
    /// MiB.
    pub fn new(connections: usize) -> Self {
        Self {
            connections,
            per_client: CLIENT_MAX,
            header: HEADER_TIME,
            local: LOCAL_TIME,
            send: transfer_time,
        }
    }
    /// The header time of a connection from `ip`.
    fn header_for(&self, ip: IpAddr) -> Duration {
        if discovery::loopback(ip) {
            self.local
        } else {
            self.header
        }
    }
}
/// The open connections from each client other than this machine, by range.
struct Clients {
    open: Mutex<HashMap<IpAddr, usize>>,
    most: usize,
}
/// A connection's place in its client's count, given back when it ends.
struct Client {
    clients: Arc<Clients>,
    range: Option<IpAddr>,
}
impl Clients {
    /// A place for a connection from `ip`, unless its client has all of its
    /// own; one from this machine always has one.
    fn admit(self: &Arc<Self>, ip: IpAddr) -> Option<Client> {
        let range = (!discovery::loopback(ip)).then(|| discovery::range(ip));
        if let Some(range) = range {
            let mut open = self.open.lock().unwrap();
            let held = open.get(&range).copied().unwrap_or(0);
            if held >= self.most {
                return None;
            }
            open.insert(range, held + 1);
        }
        Some(Client {
            clients: self.clone(),
            range,
        })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let Some(range) = self.range else {
            return;
        };
        let mut open = self.clients.open.lock().unwrap();
        if let Some(held) = open.get_mut(&range) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                open.remove(&range);
            }
        }
    }
}
/// When the answer going out on a connection must have been written; none
/// while a request is read or handled, which `api` bounds. Once all of it is
/// handed to the connection, at least the connection's header time from then,
/// for what is left to write, at most twice `BUFFER`, and for the wait for the
/// next request, so that an answer's time never ends a connection sooner than
/// its idle time. A pipelined request's answer keeps the time left to the one
/// before it, if longer.
#[derive(Clone, Copy, Default)]
struct Due {
    until: Option<Instant>,
    earlier: Option<Instant>,
}
/// Serves `router` on `listener` within `limits` until `stop` resolves; then
/// it accepts no more connections, closes the idle ones, lets the others
/// finish the request they are on for up to `CLOSING`, drops what is left, and
/// returns.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: Limits,
    stop: impl Future<Output = ()>,
) {
    let capacity = limits.connections.min(CONNECTIONS_MAX);
    let room = Arc::new(Semaphore::new(capacity));
    let clients = Arc::new(Clients {
        open: Mutex::new(HashMap::new()),
        most: limits.per_client,
    });
    let (stopping, stopped) = watch::channel(false);
    let (ending, ended) = watch::channel(false);
    let mut stop = pin!(stop);
    loop {
        // A connection is accepted only once there is room for it; until
        // then, new ones wait in the listener's queue.
        let place = tokio::select! {
            place = room.clone().acquire_owned() => place.expect("never closed"),
            () = &mut stop => break,
        };
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut stop => break,
        };
        let Ok((stream, remote)) = accepted else {
            // Out of descriptors or a passing network error: wait, go on.
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        // A client that holds all of its own places gets no other.
        let Some(client) = clients.admit(remote.ip()) else {
            continue;
        };
        tokio::spawn(connection(
            stream,
            remote,
            router.clone(),
            limits,
            (place, client),
            stopped.clone(),
            ended.clone(),
        ));
    }
    let _ = stopping.send(true);
    drop(listener);
    let all = u32::try_from(capacity).expect("CONNECTIONS_MAX fits");
    if tokio::time::timeout(CLOSING, room.acquire_many(all))
        .await
        .is_err()
    {
        let _ = ending.send(true);
        let _ = room.acquire_many(all).await;
    }
}
/// An answer going out on a connection, whose time lasts at least `idle`
/// more once the connection has taken all of it and lets it go.
struct Sending {
    body: Body,
    due: Arc<watch::Sender<Due>>,
    idle: Duration,
}
impl HttpBody for Sending {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
impl Drop for Sending {
    fn drop(&mut self) {
        let rest = Instant::now() + self.idle;
        self.due.send_modify(|d| d.until = d.until.max(Some(rest)));
    }
}
/// One connection, until it closes, its client is too slow, or REST stops:
/// `stopped` asks it to finish the request it is on, `ended` to end at once,
/// as does REST's own end.
async fn connection(
    stream: TcpStream,
    remote: SocketAddr,
    router: Router,
    limits: Limits,
    _held: (OwnedSemaphorePermit, Client),
    mut stopped: watch::Receiver<bool>,
    mut ended: watch::Receiver<bool>,
) {
    let (due, mut watching) = watch::channel(Due::default());
    let due = Arc::new(due);
    let inner = TowerToHyperService::new(router);
    let idle = limits.header_for(remote.ip());
    let service = service_fn(move |mut request: hyper::Request<Incoming>| {
        request.extensions_mut().insert(ConnectInfo(remote));
        due.send_modify(|d| d.earlier = d.earlier.max(d.until.take()));
        let answer = inner.call(request);
        let due = due.clone();
        async move {
            let response = answer.await?;
            // No answer is near 4 GiB: the bound only keeps the time
            // within what a clock reading can add.
            let size = response
                .body()
                .size_hint()
                .upper()
                .unwrap_or(FRAME_MAX as u64)
                .min(u64::from(u32::MAX));
            let until = Instant::now() + (limits.send)(size);
            due.send_modify(|d| d.until = d.earlier.take().max(Some(until)));
            Ok::<_, Infallible>(response.map(|body| Body::new(Sending { body, due, idle })))
        }
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(idle)
        .max_buf_size(BUFFER)
        .serve_connection(TokioIo::new(stream), service);
    let mut connection = pin!(connection);
    let mut closing = false;
    loop {
        let until = watching.borrow_and_update().until;
        tokio::select! {
            _ = connection.as_mut() => break,
            Ok(()) = watching.changed() => {}
            // The answer was not written in time: the connection ends.
            () = tokio::time::sleep_until(until.unwrap_or_else(Instant::now)), if until.is_some() => break,
            _ = stopped.wait_for(|stop| *stop), if !closing => {
                connection.as_mut().graceful_shutdown();
                closing = true;
            }
            _ = ended.wait_for(|end| *end) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const LARGE: usize = 64 << 20;
    /// A body of so many bytes in frames of 64 KiB, as `api` hands answers
    /// to the connection: written a few frames at a time, so that a client
    /// that does not read stops it on every system.
    struct Frames(usize);
    impl HttpBody for Frames {
        type Data = Bytes;
        type Error = Infallible;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            let n = self.0.min(64 << 10);
            if n == 0 {
                return Poll::Ready(None);
            }
            self.0 -= n;
            Poll::Ready(Some(Ok(Frame::data(Bytes::from(vec![7; n])))))
        }
        fn is_end_stream(&self) -> bool {
            self.0 == 0
        }
        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.0 as u64)
        }
    }

    fn app() -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .route("/large", get(|| async { Body::new(Frames(LARGE)) }))
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    "slow"
                }),
            )
    }
    /// A server of `app` within `limits`, and what stops it.
    async fn start(limits: Limits) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(serve(listener, app(), limits, async {
            let _ = stopped.await;
        }));
        (address, stop)
    }
    /// As the node's, an answer has longer to be taken than a connection
    /// may wait idle. The tests connect from this machine, so its header
    /// time is the one they meet, unless they set it apart.
    fn limits(header: Duration) -> Limits {
        Limits {
            connections: 8,
            per_client: CLIENT_MAX,
            header,
            local: header,
            send: |_| Duration::from_secs(30),
        }
    }
    /// Reads until the server closes the connection, or `within` passes;
    /// the bytes read, and whether it closed.
    async fn until_closed(stream: &mut TcpStream, within: Duration) -> (Vec<u8>, bool) {
        let mut bytes = Vec::new();
        let mut buffer = vec![0; 1 << 16];
        let deadline = Instant::now() + within;
        loop {
            match tokio::time::timeout_at(deadline, stream.read(&mut buffer)).await {
                Err(_) => return (bytes, false),
                Ok(Ok(0) | Err(_)) => return (bytes, true),
                Ok(Ok(n)) => bytes.extend(&buffer[..n]),
            }
        }
    }
    /// An answer whose body ends `ok`, within five seconds.
    async fn answer(stream: &mut TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut buffer = vec![0; 512];
        while !bytes.ends_with(b"ok") {
            let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert!(n > 0, "closed");
            bytes.extend(&buffer[..n]);
        }
        bytes
    }
    fn get_request(path: &str) -> Vec<u8> {
        format!("GET {path} HTTP/1.1\r\nHost: test\r\n\r\n").into_bytes()
    }

    #[tokio::test]
    async fn a_header_sent_too_slowly_closes_the_connection() {
        let (address, _stop) = start(limits(Duration::from_millis(400))).await;
        let mut slow = TcpStream::connect(address).await.unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
        let started = Instant::now();
        let (_, closed) = until_closed(&mut slow, Duration::from_secs(5)).await;
        assert!(closed);
        assert!(started.elapsed() >= Duration::from_millis(300));
        // A connection that never sends a byte closes as well.
        let mut silent = TcpStream::connect(address).await.unwrap();
        assert!(until_closed(&mut silent, Duration::from_secs(5)).await.1);
    }

    #[tokio::test]
    async fn an_idle_connection_closes_after_its_last_answer() {
        let (address, _stop) = start(limits(Duration::from_millis(400))).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        // Requests a little apart keep it open...
        for i in 0..3 {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            stream.write_all(&get_request("/")).await.unwrap();
            assert!(answer(&mut stream).await.starts_with(b"HTTP/1.1 200"));
        }
        // ...and with none, it closes.
        let answered = Instant::now();
        let (bytes, closed) = until_closed(&mut stream, Duration::from_secs(5)).await;
        assert!(closed && bytes.is_empty());
        assert!(answered.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn a_connection_from_this_machine_may_wait_idle_longer() {
        let limits = Limits::new(1);
        assert_eq!(limits.header, HEADER_TIME);
        for local in ["127.0.0.1", "127.8.9.1", "::1", "::ffff:127.0.0.1"] {
            assert_eq!(limits.header_for(local.parse().unwrap()), LOCAL_TIME);
        }
        for remote in ["192.0.2.1", "10.0.0.2", "2001:db8::1", "::ffff:192.0.2.1"] {
            assert_eq!(limits.header_for(remote.parse().unwrap()), HEADER_TIME);
        }
    }

    #[tokio::test]
    async fn an_idle_connection_from_this_machine_has_the_local_time() {
        // Longer than the header time of others, and than an answer's own
        // time, which does not end the connection once the answer is out.
        let (address, _stop) = start(Limits {
            local: Duration::from_millis(1_500),
            send: |_| Duration::from_millis(300),
            ..limits(Duration::from_millis(300))
        })
        .await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(&get_request("/")).await.unwrap();
        answer(&mut stream).await;
        // Idle past the header time of others, it still serves a request...
        tokio::time::sleep(Duration::from_millis(900)).await;
        stream.write_all(&get_request("/")).await.unwrap();
        answer(&mut stream).await;
        // ...and closes once idle for its own.
        let answered = Instant::now();
        let (_, closed) = until_closed(&mut stream, Duration::from_secs(5)).await;
        assert!(closed);
        assert!(answered.elapsed() >= Duration::from_millis(1_200));
    }

    #[tokio::test]
    async fn an_answer_not_taken_in_time_closes_the_connection() {
        // One connection at a time: the second is served once the first,
        // whose answer is never read, is closed for it.
        let (address, _stop) = start(Limits {
            connections: 1,
            send: |_| Duration::from_millis(500),
            ..limits(Duration::from_secs(5))
        })
        .await;
        let mut stalled = TcpStream::connect(address).await.unwrap();
        stalled.write_all(&get_request("/large")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut next = TcpStream::connect(address).await.unwrap();
        next.write_all(&get_request("/")).await.unwrap();
        let started = Instant::now();
        answer(&mut next).await;
        assert!(started.elapsed() >= Duration::from_millis(300));
        let (bytes, closed) = until_closed(&mut stalled, Duration::from_secs(10)).await;
        assert!(closed && bytes.len() < LARGE, "{}", bytes.len());
    }

    #[tokio::test]
    async fn connections_beyond_the_limit_wait_their_turn() {
        let (address, _stop) = start(Limits {
            connections: 3,
            ..limits(Duration::from_secs(5))
        })
        .await;
        let mut held = Vec::new();
        for _ in 0..3 {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream.write_all(&get_request("/")).await.unwrap();
            answer(&mut stream).await;
            held.push(stream);
        }
        // A fourth waits while the three stay open...
        let mut fourth = TcpStream::connect(address).await.unwrap();
        fourth.write_all(&get_request("/")).await.unwrap();
        let mut buffer = vec![0; 512];
        let waiting =
            tokio::time::timeout(Duration::from_millis(500), fourth.read(&mut buffer)).await;
        assert!(waiting.is_err());
        // ...and is served once one closes.
        drop(held.pop());
        answer(&mut fourth).await;
    }

    #[test]
    fn each_client_has_its_own_share_and_this_machine_none() {
        let clients = Arc::new(Clients {
            open: Mutex::new(HashMap::new()),
            most: 2,
        });
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let first = clients.admit(ip("192.0.2.1")).unwrap();
        let second = clients.admit(ip("192.0.2.1")).unwrap();
        assert!(clients.admit(ip("192.0.2.1")).is_none());
        // An IPv4 address mapped into IPv6 is the same client.
        assert!(clients.admit(ip("::ffff:192.0.2.1")).is_none());
        // Another address is another client; an IPv6 /64 is one client.
        let other = clients.admit(ip("192.0.2.2")).unwrap();
        let v6 = clients.admit(ip("2001:db8:1:2::1")).unwrap();
        let _v6 = clients.admit(ip("2001:db8:1:2::ffff")).unwrap();
        assert!(clients.admit(ip("2001:db8:1:2:3::")).is_none());
        assert!(clients.admit(ip("2001:db8:1:3::1")).is_some());
        // A place given back is free again.
        drop(first);
        let _again = clients.admit(ip("192.0.2.1")).unwrap();
        assert!(clients.admit(ip("192.0.2.1")).is_none());
        // This machine's connections, a local proxy's among them, are not
        // counted at all.
        let local: Vec<_> = (0..100)
            .map(|i| {
                let ip = if i % 2 == 0 { "127.0.0.1" } else { "::1" };
                clients.admit(ip.parse().unwrap()).unwrap()
            })
            .collect();
        drop((second, other, v6, local));
        let open = clients.open.lock().unwrap();
        assert_eq!(open.values().sum::<usize>(), 2, "{open:?}");
    }

    #[tokio::test]
    async fn a_request_in_progress_finishes_when_rest_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(
            listener,
            app(),
            limits(Duration::from_secs(5)),
            async {
                let _ = stopped.await;
            },
        ));
        let mut idle = TcpStream::connect(address).await.unwrap();
        let mut busy = TcpStream::connect(address).await.unwrap();
        busy.write_all(&get_request("/slow")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop.send(()).unwrap();
        let (answer, closed) = until_closed(&mut busy, Duration::from_secs(5)).await;
        assert!(closed && answer.ends_with(b"slow"));
        assert!(until_closed(&mut idle, Duration::from_secs(5)).await.1);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn rest_stops_within_a_second_whatever_its_clients_do() {
        // An idle connection closes at once, and one whose client stopped
        // reading an answer, which had 30 seconds more, is dropped after a
        // second.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(
            listener,
            app(),
            limits(Duration::from_secs(30)),
            async {
                let _ = stopped.await;
            },
        ));
        let mut idle = TcpStream::connect(address).await.unwrap();
        idle.write_all(&get_request("/")).await.unwrap();
        answer(&mut idle).await;
        let mut stalled = TcpStream::connect(address).await.unwrap();
        stalled.write_all(&get_request("/large")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let stopping = Instant::now();
        stop.send(()).unwrap();
        assert!(until_closed(&mut idle, Duration::from_millis(500)).await.1);
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
        let elapsed = stopping.elapsed();
        assert!(elapsed >= CLOSING && elapsed < CLOSING * 2, "{elapsed:?}");
        let (bytes, closed) = until_closed(&mut stalled, Duration::from_secs(5)).await;
        assert!(closed && bytes.len() < LARGE, "{}", bytes.len());
    }

    #[test]
    fn rest_keeps_the_descriptors_the_rest_of_the_node_needs() {
        let reserved = reserved(125, 8);
        assert_eq!(reserved, 125 + 8 + 32 + 64);
        assert_eq!(connections(None, reserved), CONNECTIONS_MAX);
        assert_eq!(connections(Some(1_024), reserved), CONNECTIONS_MAX);
        assert_eq!(connections(Some(512), reserved), 512 - 229);
        assert_eq!(connections(Some(200), reserved), 0);
        assert_eq!(connections(Some(u64::MAX), u64::MAX), 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_descriptor_limit_is_raised_as_far_as_needed() {
        let before = descriptors(0).unwrap();
        let after = descriptors(before.saturating_add(64)).unwrap();
        assert!(after >= before);
        assert_eq!(descriptors(0), Some(after));
    }
}
