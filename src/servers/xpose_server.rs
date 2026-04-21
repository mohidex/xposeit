use std::{net::SocketAddr, ops::RangeInclusive, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Result};
use tokio::{
    io::copy_bidirectional,
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::{interval, sleep, timeout},
};
use tracing::{debug, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::protocol::{
    frame::Delimited,
    messages::{ClientMessage, ServerMessage},
};
use crate::servers::sessions::{ListenerPool, TunnelStreams};

/// How long the server waits for a free port before giving up.
const POOL_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the server sends a heartbeat on the control channel.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// How long an accepted-but-not-claimed forwarded connection lives.
const PENDING_CONN_TTL: Duration = Duration::from_secs(10);

/// Startup aborts if fewer than this many ports could be bound.
const MIN_POOL_SIZE: usize = 1;

/// Binds up to `capacity` listeners drawn randomly from `range`.
/// Returns `Err` if fewer than `MIN_POOL_SIZE` ports were successfully bound.
async fn bind_n_ports(range: RangeInclusive<u16>, capacity: usize) -> Result<Vec<TcpListener>> {
    let mut ports: Vec<u16> = range.collect();
    fastrand::shuffle(&mut ports);

    let mut listeners = Vec::with_capacity(capacity);

    for port in ports {
        if listeners.len() >= capacity {
            break;
        }

        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => {
                debug!(port, "bound forwarding port");
                listeners.push(listener);
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::AddrInUse
                    || e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                // port unavailable — try the next one
            }
            Err(e) => {
                warn!(port, %e, "unexpected bind error — skipping");
            }
        }
    }

    let bound = listeners.len();
    info!(bound, requested = capacity, "listener pool ready");

    if bound < MIN_POOL_SIZE {
        bail!(
            "only bound {bound} port(s), minimum required is {MIN_POOL_SIZE}; \
             check that the port range is wide enough and ports are not all taken"
        );
    }
    if bound < capacity {
        warn!(
            bound,
            capacity, "pool is smaller than requested — consider widening port_range"
        );
    }

    Ok(listeners)
}

pub struct XposeServer {
    /// Pending forwarded streams waiting for a client `Accept`.
    connections: TunnelStreams,

    /// Pre-bound TCP listeners returned to the pool after each session.
    listeners: ListenerPool,

    /// One permit per listener. Callers block here instead of failing fast.
    pool_permits: Arc<Semaphore>,
}

impl XposeServer {
    /// Binds `capacity` ports from `port_range` and returns a ready server.
    ///
    /// Returns `Err` if fewer than `MIN_POOL_SIZE` ports could be bound.
    pub async fn new(port_range: RangeInclusive<u16>, capacity: usize) -> Result<Self> {
        let listeners = bind_n_ports(port_range, capacity).await?;
        let actual = listeners.len();
        Ok(Self {
            connections: TunnelStreams::new(),
            pool_permits: Arc::new(Semaphore::new(actual)),
            listeners: ListenerPool::new(listeners),
        })
    }

    /// Starts accepting control connections on `addr`.
    pub async fn listen(self, addr: SocketAddr) -> Result<()> {
        let this = Arc::new(self);
        let listener = TcpListener::bind(addr).await?;
        info!(?addr, "control server listening");

        loop {
            let (stream, peer) = listener.accept().await?;
            let this = Arc::clone(&this);
            tokio::spawn(
                async move {
                    if let Err(e) = handle_connection(this, stream).await {
                        warn!(%e, "connection handler failed");
                    }
                }
                .instrument(info_span!("control", ?peer)),
            );
        }
    }

    /// Blocks up to `POOL_WAIT_TIMEOUT` for a free listener.
    async fn acquire_listener(&self, stream: &mut Delimited<TcpStream>) -> Result<TcpListener> {
        // Tell the client immediately so it doesn't time out silently
        let _ = stream.send(ServerMessage::Waiting).await;

        let permit = timeout(POOL_WAIT_TIMEOUT, self.pool_permits.acquire())
            .await
            .map_err(|_| anyhow!("timed out waiting for a free port — server is at capacity"))?
            .map_err(|_| anyhow!("semaphore closed"))?;

        // The semaphore and pool are always kept in sync, so a slot is
        // guaranteed to exist. Map to `Err` instead of unwrap/expect.
        let listener = self
            .listeners
            .acquire()
            .await
            .ok_or_else(|| anyhow!("semaphore/pool out of sync — this is a bug"))?;

        // Forget the permit: we manually restore it in `release_listener` so
        // the semaphore count stays perfectly in step with the pool size.
        permit.forget();

        Ok(listener)
    }

    async fn release_listener(&self, listener: TcpListener) {
        self.listeners.release(listener).await;
        self.pool_permits.add_permits(1);
        let available = self.listeners.available().await;
        debug!(available, "listener returned to pool");
    }
}

/// Initial state: control connection accepted, no port assigned yet.
struct Init;

/// Opened state: a forwarding port has been assigned and the client notified.
struct Opened {
    listener: TcpListener,
}

/// A control-channel session parameterised over its state.
///
/// Because `S` is stored as a concrete `state` field, the compiler tracks the
/// exact type at every call site — no `PhantomData` required.
struct Session<S> {
    stream: Delimited<TcpStream>,
    state: S,
}

impl Session<Init> {
    fn new(stream: Delimited<TcpStream>) -> Self {
        Self {
            stream,
            state: Init,
        }
    }

    /// Acquires a forwarding port, notifies the client, and transitions to
    /// `Opened`. On failure the error is forwarded to the client before returning.
    async fn open(mut self, server: &XposeServer) -> Result<Session<Opened>> {
        let listener = match server.acquire_listener(&mut self.stream).await {
            Ok(l) => l,
            Err(e) => {
                let msg = e.to_string();
                let _ = self.stream.send(ServerMessage::Error(msg.clone())).await;
                return Err(anyhow!(msg));
            }
        };

        let port = listener.local_addr()?.port();
        info!(%port, "checked out forwarding port");
        self.stream.send(ServerMessage::Opened(port)).await?;

        Ok(Session {
            stream: self.stream,
            state: Opened { listener },
        })
    }
}

impl Session<Opened> {
    /// Drives the session to completion and always returns the listener so the
    /// caller can put it back in the pool regardless of outcome.
    async fn run(mut self, server: &XposeServer) -> (Result<()>, TcpListener) {
        let result = self.run_inner(server).await;
        // Direct struct-field access — no Option, no unwrap, no expect.
        (result, self.state.listener)
    }

    async fn run_inner(&mut self, server: &XposeServer) -> Result<()> {
        let mut heartbeat = interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased; // explicit priority: heartbeat > new conn > unexpected msg

                _ = heartbeat.tick() => {
                    if self.stream.send(ServerMessage::Heartbeat).await.is_err() {
                        debug!("control stream closed — ending session");
                        return Ok(());
                    }
                }

                res = self.state.listener.accept() => {
                    let (stream2, addr) = res?;
                    let id = Uuid::new_v4();
                    info!(%id, ?addr, "new forwarded connection");

                    server.connections.insert(id, stream2);
                    spawn_cleanup(server.connections.clone(), id);

                    self.stream.send(ServerMessage::Connection(id)).await?;
                }

                // Any client message on the control channel after Open is a
                // protocol violation.
                res = self.stream.recv::<ClientMessage>() => {
                    match res? {
                        Some(msg) => {
                            return Err(anyhow!(
                                "protocol error: unexpected {msg:?} on control channel"
                            ));
                        }
                        // Client closed the control connection cleanly.
                        None => return Ok(()),
                    }
                }
            }
        }
    }
}

async fn handle_connection(server: Arc<XposeServer>, stream: TcpStream) -> Result<()> {
    let mut delimited = Delimited::new(stream);

    let msg = delimited
        .recv_timeout::<ClientMessage>()
        .await?
        .ok_or_else(|| anyhow!("connection closed before first message"))?;

    match msg {
        ClientMessage::Open => {
            let session = Session::<Init>::new(delimited);
            let session = session.open(&server).await?;
            let (result, listener) = session.run(&server).await;
            server.release_listener(listener).await;
            result?;
        }

        ClientMessage::Accept(id) => {
            handle_proxy_connection(&server, delimited, id).await?;
        }
    }

    Ok(())
}

async fn handle_proxy_connection(
    server: &XposeServer,
    delimited: Delimited<TcpStream>,
    id: Uuid,
) -> Result<()> {
    info!(%id, "starting proxy");

    let (_, mut upstream) = server
        .connections
        .remove(&id)
        .ok_or_else(|| anyhow!("no pending connection for id {id}"))?;

    let mut parts = delimited.into_parts();

    // Data in the write buffer means the framing layer buffered bytes that
    // were never sent to the client — dropping them would silently corrupt
    // the proxied stream, so we treat this as a hard error.
    if !parts.write_buf.is_empty() {
        bail!(
            "non-empty write buffer ({} bytes) on proxy start — aborting to avoid data loss",
            parts.write_buf.len()
        );
    }

    // Flush any bytes already read past the framing header before handing off
    // to the raw bidirectional copy.
    if !parts.read_buf.is_empty() {
        upstream.write_all(&parts.read_buf).await?;
        upstream.flush().await?;
    }

    let (b_up, b_down) = copy_bidirectional(&mut parts.io, &mut upstream).await?;
    info!(%id, b_up, b_down, "proxy closed gracefully");

    Ok(())
}

fn spawn_cleanup(conns: TunnelStreams, id: Uuid) {
    tokio::spawn(async move {
        sleep(PENDING_CONN_TTL).await;
        if conns.remove(&id).is_some() {
            warn!(%id, "cleaned up stale pending connection");
        }
    });
}
