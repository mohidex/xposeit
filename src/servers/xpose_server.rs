use std::{net::SocketAddr, ops::RangeInclusive, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use tokio::{
    io::copy_bidirectional,
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::{sleep, timeout},
};
use tracing::{debug, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::protocol::{
    frame::Delimited,
    messages::{ClientMessage, ServerMessage},
};

/// Binds exactly `capacity` listeners drawn randomly from `range`.
/// Skips ports that are already in use. Returns however many it managed
/// to bind (could be less than `capacity` if the range is smaller or
/// most ports are taken).
async fn bind_n_ports(range: RangeInclusive<u16>, capacity: usize) -> Vec<TcpListener> {
    // Collect the range into a vec and shuffle so we sample randomly.
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

    info!(
        bound = listeners.len(),
        requested = capacity,
        "listener pool ready"
    );

    listeners
}

/// Pool of pre-bound, ready-to-accept listeners.
/// Sessions check one out and return it when they finish.
struct PortAllocator {
    available: Vec<TcpListener>,
}

impl PortAllocator {
    fn new(listeners: Vec<TcpListener>) -> Self {
        Self {
            available: listeners,
        }
    }

    fn acquire(&mut self) -> Option<TcpListener> {
        self.available.pop()
    }

    fn release(&mut self, listener: TcpListener) {
        self.available.push(listener);
    }

    fn available(&self) -> usize {
        self.available.len()
    }
}

pub struct XposeServer {
    /// Concurrent map of IDs to incoming connections.
    conns: Arc<DashMap<Uuid, TcpStream>>,

    /// Pool of pre-bound listeners for forwarding ports.
    allocator: Arc<Mutex<PortAllocator>>,
}

impl XposeServer {
    /// `capacity` — how many ports to pre-bind from `port_range`.
    pub async fn new(port_range: RangeInclusive<u16>, capacity: usize) -> Self {
        let listeners = bind_n_ports(port_range, capacity).await;
        Self {
            conns: Arc::new(DashMap::new()),
            allocator: Arc::new(Mutex::new(PortAllocator::new(listeners))),
        }
    }

    pub async fn listen(self) -> Result<()> {
        let this = Arc::new(self);
        let addr = SocketAddr::from(([0, 0, 0, 0], 7835));
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

    async fn acquire_listener(&self) -> Option<TcpListener> {
        self.allocator.lock().await.acquire()
    }

    async fn release_listener(&self, listener: TcpListener) {
        let mut pool = self.allocator.lock().await;
        pool.release(listener);
        debug!(available = pool.available(), "listener returned to pool");
    }
}

struct Session<S> {
    stream: Delimited<TcpStream>,
    state: S,
}

/// Initial state
struct Init;

/// Opened state
struct Opened {
    listener: TcpListener,
}

impl Session<Init> {
    async fn open(mut self, server: &XposeServer) -> Result<Session<Opened>> {
        let listener = match server.acquire_listener().await {
            Some(l) => l,
            None => {
                let msg = "all ports are currently allocated, try again later";
                let _ = self.stream.send(ServerMessage::Error(msg.into())).await;
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
    /// Runs the session and always gives the listener back to the caller.
    async fn run(mut self, server: &XposeServer) -> (Result<()>, TcpListener) {
        let result = self.run_inner(server).await;
        (result, self.state.listener)
    }

    async fn run_inner(&mut self, server: &XposeServer) -> Result<()> {
        loop {
            // Send heartbeat (client can detect dead control link)
            if self.stream.send(ServerMessage::Heartbeat).await.is_err() {
                debug!("control stream closed during heartbeat");
                return Ok(());
            }

            tokio::select! {
                // Accept new incoming connection on the forwarded port
                res = timeout(Duration::from_millis(500), self.state.listener.accept()) => {
                    if let Ok(Ok((stream2, addr))) = res {
                        let id = Uuid::new_v4();
                        info!(%id, ?addr, "new forwarded connection");

                        server.conns.insert(id, stream2);
                        spawn_cleanup(server.conns.clone(), id);

                        // Notify client → "you can now Accept(id)"
                        let _ = self.stream.send(ServerMessage::Connection(id)).await;
                    }
                }

                // Should NOT receive client messages on control channel after Open
                res = timeout(
                    Duration::from_millis(100),
                    self.stream.recv::<ClientMessage>(),
                ) => {
                    if let Ok(Ok(Some(_))) = res {
                        return Err(anyhow!("protocol error: unexpected message on control channel"));
                    }
                }

                else => continue,
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
            let session = Session {
                stream: delimited,
                state: Init,
            };
            let session = session.open(&server).await?;
            let (result, listener) = session.run(&server).await;
            server.release_listener(listener).await;
            result?;
        }

        ClientMessage::Accept(id) => {
            handle_proxy_connection(server, delimited, id).await?;
        }
    }

    Ok(())
}

async fn handle_proxy_connection(
    server: Arc<XposeServer>,
    delimited: Delimited<TcpStream>,
    id: Uuid,
) -> Result<()> {
    info!(%id, "starting proxy");

    let (_, mut upstream) = server
        .conns
        .remove(&id)
        .ok_or_else(|| anyhow!("no pending connection for id {id}"))?;

    let mut parts = delimited.into_parts();
    if !parts.write_buf.is_empty() {
        warn!("write buffer was not empty on proxy start — possible bug");
    }

    // Flush any already-read payload that came before the Accept
    if !parts.read_buf.is_empty() {
        upstream.write_all(&parts.read_buf).await?;
        upstream.flush().await?;
    }

    let (b_up, b_down) = copy_bidirectional(&mut parts.io, &mut upstream).await?;
    info!(%id, b_up, b_down, "proxy closed gracefully");

    Ok(())
}

fn spawn_cleanup(conns: Arc<DashMap<Uuid, TcpStream>>, id: Uuid) {
    tokio::spawn(async move {
        sleep(Duration::from_secs(10)).await;
        if conns.remove(&id).is_some() {
            warn!(%id, "cleaned up stale pending connection");
        }
    });
}
