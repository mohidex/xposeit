use std::{io, net::SocketAddr, ops::RangeInclusive, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    time::{sleep, timeout},
};
use tracing::{info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::shared::{
    frame::Delimited,
    messages::{ClientMessage, ServerMessage},
    proxy::proxy,
};

pub struct XposeServer {
    /// Range of TCP ports that can be forwarded.
    port_range: RangeInclusive<u16>,

    /// Concurrent map of IDs to incoming connections.
    conns: Arc<DashMap<Uuid, TcpStream>>,
}

impl XposeServer {
    pub fn new(port_range: RangeInclusive<u16>) -> Self {
        assert!(!port_range.is_empty(), "must provide at least one port");
        Self {
            port_range,
            conns: Arc::new(DashMap::new()),
        }
    }

    pub async fn listen(self) -> Result<()> {
        let this = Arc::new(self);
        let addr = SocketAddr::from(([0, 0, 0, 0], 7835));
        let listener = TcpListener::bind(addr).await?;

        info!(?addr, "server listening");

        loop {
            let (stream, peer) = listener.accept().await?;
            let this = Arc::clone(&this);

            tokio::spawn(
                async move {
                    let res = handle_connection(this, stream).await;
                    if let Err(err) = res {
                        warn!(%err, "connection exited with error");
                    }
                }
                .instrument(info_span!("control", ?peer)),
            );
        }
    }

    async fn create_listener(&self) -> Result<TcpListener> {
        for _ in 0..150 {
            let port = fastrand::u16(self.port_range.clone());
            match TcpListener::bind(("0.0.0.0", port)).await {
                Ok(l) => return Ok(l),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
                    ) =>
                {
                    continue
                }
                Err(err) => return Err(err.into()),
            }
        }
        Err(anyhow!("failed to find an available port"))
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
            // Heartbeat
            if self.stream.send(ServerMessage::Heartbeat).await.is_err() {
                return Ok(());
            }

            // Accept incoming forwarded connections
            if let Ok(Ok((stream2, addr))) =
                timeout(Duration::from_millis(500), self.state.listener.accept()).await
            {
                let id = Uuid::new_v4();
                info!(%id, ?addr, "incoming forwarded connection");

                server.conns.insert(id, stream2);
                spawn_cleanup(server.conns.clone(), id);

                self.stream.send(ServerMessage::Connection(id)).await?;
            }

            // Control messages - should not receive anything on control connection
            if let Ok(Ok(Some(_msg))) = timeout(
                Duration::from_millis(100),
                self.stream.recv::<ClientMessage>(),
            )
            .await
            {
                return Err(anyhow!(
                    "protocol error: unexpected message on control connection"
                ));
            }
        }
    }
}

async fn handle_connection(server: Arc<XposeServer>, stream: TcpStream) -> Result<()> {
    let mut delimited = Delimited::new(stream);

    // Check what type of connection this is
    match delimited.recv_timeout::<ClientMessage>().await? {
        Some(ClientMessage::Open) => {
            // This is a control connection
            let session = Session {
                stream: delimited,
                state: Init,
            };
            // Don't call recv_open - we already consumed the Open message
            let session = session.open(&server).await?;
            session.run(&server).await
        }
        Some(ClientMessage::Accept(id)) => {
            // This is a proxy connection
            handle_proxy_connection(server, delimited, id).await
        }
        _ => Err(anyhow!("protocol error: expected Open or Accept")),
    }
}

async fn handle_proxy_connection(
    server: Arc<XposeServer>,
    delimited: Delimited<TcpStream>,
    id: Uuid,
) -> Result<()> {
    info!(%id, "forwarding connection");

    let (_, mut stream2) = server
        .conns
        .remove(&id)
        .ok_or_else(|| anyhow!("missing connection"))?;

    let parts = delimited.into_parts();
    debug_assert!(parts.write_buf.is_empty(), "framed write buffer not empty");

    stream2.write_all(&parts.read_buf).await?;
    proxy(parts.io, stream2).await?;
    Ok(())
}

fn spawn_cleanup(conns: Arc<DashMap<Uuid, TcpStream>>, id: Uuid) {
    tokio::spawn(async move {
        sleep(Duration::from_secs(10)).await;
        if conns.remove(&id).is_some() {
            warn!(%id, "removed stale connection");
        }
    });
}
