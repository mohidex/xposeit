use std::{net::SocketAddr, ops::RangeInclusive, sync::Arc, sync::Mutex, time::Duration};

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use tokio::{
    io::copy_bidirectional,
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    time::{sleep, timeout},
};
use tracing::{debug, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::protocol::{
    frame::Delimited,
    messages::{ClientMessage, ServerMessage},
};

/// Port pool state for efficient port allocation
struct PortPool {
    /// Shuffled list of ports to try
    ports: Vec<u16>,
    /// Current position in the pool
    current_index: usize,
    /// Number of times the pool has been exhausted and regenerated
    pool_regenerations: usize,
}

impl PortPool {
    fn new(port_range: RangeInclusive<u16>) -> Self {
        let mut ports: Vec<u16> = port_range.collect();
        fastrand::shuffle(&mut ports);

        Self {
            ports,
            current_index: 0,
            pool_regenerations: 0,
        }
    }

    /// Get the next port from the pool. Returns None if pool is exhausted after max regenerations.
    fn next_port(&mut self, max_regenerations: usize) -> Option<u16> {
        if self.current_index >= self.ports.len() {
            if self.pool_regenerations < max_regenerations {
                // Reshuffle and reset
                fastrand::shuffle(&mut self.ports);
                self.current_index = 0;
                self.pool_regenerations += 1;
                warn!(
                    regeneration = self.pool_regenerations,
                    "port pool exhausted, regenerating..."
                );
            } else {
                return None;
            }
        }

        let port = self.ports[self.current_index];
        self.current_index += 1;
        Some(port)
    }
}

pub struct XposeServer {
    /// Concurrent map of IDs to incoming connections.
    conns: Arc<DashMap<Uuid, TcpStream>>,

    /// Thread-safe port allocator
    port_pool: Arc<Mutex<PortPool>>,
}

impl XposeServer {
    pub fn new(port_range: RangeInclusive<u16>) -> Self {
        let port_pool = PortPool::new(port_range);
        Self {
            conns: Arc::new(DashMap::new()),
            port_pool: Arc::new(Mutex::new(port_pool)),
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

    async fn create_listener(&self) -> Result<TcpListener> {
        const MAX_POOL_REGENERATIONS: usize = 3;

        loop {
            let port = {
                let mut pool = self
                    .port_pool
                    .lock()
                    .map_err(|e| anyhow!("port pool mutex poisoned: {e}"))?;

                pool.next_port(MAX_POOL_REGENERATIONS)
                    .ok_or_else(|| anyhow!("exhausted all port attempts after regenerations"))?
            };

            match TcpListener::bind(("0.0.0.0", port)).await {
                Ok(listener) => {
                    let local = listener.local_addr()?;
                    info!(port = local.port(), "bound forwarding listener");
                    return Ok(listener);
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::AddrInUse
                        || e.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    debug!(port, "port in use → skipping");
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Server Session state machine
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
        // Message already received in handle_connection, just create listener
        let listener = server.create_listener().await?;
        let port = listener.local_addr()?.port();

        info!(%port, "opened forwarding port");
        self.stream.send(ServerMessage::Opened(port)).await?;

        Ok(Session {
            stream: self.stream,
            state: Opened { listener },
        })
    }
}

impl Session<Opened> {
    async fn run(mut self, server: &XposeServer) -> Result<()> {
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
            session.run(&server).await?;
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

    // Bidirectional copy
    let (bytes_client_to_upstream, bytes_upstream_to_client) =
        copy_bidirectional(&mut parts.io, &mut upstream).await?;

    info!(
        %id,
        bytes_client_to_upstream,
        bytes_upstream_to_client,
        "proxy closed gracefully"
    );

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
