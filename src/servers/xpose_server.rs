use std::{io, net::SocketAddr, ops::RangeInclusive, sync::Arc, time::Duration};

use anyhow::Result;
use dashmap::DashMap;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};
use tracing::{info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::shared::{
    frame::Delimited, messages::ClientMessage, messages::ServerMessage, proxy::proxy,
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
        XposeServer {
            port_range,
            conns: Arc::new(DashMap::new()),
        }
    }

    /// Start the server, listening for new connections.
    pub async fn listen(self) -> Result<()> {
        let this = Arc::new(self);
        let addr = SocketAddr::from(([0, 0, 0, 0], 7835));
        let listener = TcpListener::bind(&addr).await?;
        info!(?addr, "server listening");

        loop {
            let (stream, addr) = listener.accept().await?;
            let this = Arc::clone(&this);
            tokio::spawn(
                async move {
                    info!("incoming connection");
                    if let Err(err) = this.handle_connection(stream).await {
                        warn!(%err, "connection exited with error");
                    } else {
                        info!("connection exited");
                    }
                }
                .instrument(info_span!("control", ?addr)),
            );
        }
    }

    async fn create_listener(&self) -> Result<TcpListener, &'static str> {
        let try_bind = |port: u16| async move {
            TcpListener::bind(("0.0.0.0", port))
                .await
                .map_err(|err| match err.kind() {
                    io::ErrorKind::AddrInUse => "port already in use",
                    io::ErrorKind::PermissionDenied => "permission denied",
                    _ => "failed to bind to port",
                })
        };
        for _ in 0..150 {
            let port = fastrand::u16(self.port_range.clone());
            match try_bind(port).await {
                Ok(listener) => return Ok(listener),
                Err(_) => continue,
            }
        }
        Err("failed to find an available port")
    }

    async fn handle_connection(&self, stream: TcpStream) -> Result<()> {
        let mut stream = Delimited::new(stream);
        match stream.recv_timeout().await? {
            Some(ClientMessage::Open) => {
                let listener = match self.create_listener().await {
                    Ok(listener) => listener,
                    Err(err) => {
                        stream.send(ServerMessage::Error(err.into())).await?;
                        return Ok(());
                    }
                };
                let port = listener.local_addr()?.port();
                info!(?port, "new client");
                stream.send(ServerMessage::Opened(port)).await?;

                loop {
                    if stream.send(ServerMessage::Heartbeat).await.is_err() {
                        // Assume that the TCP connection has been dropped.
                        return Ok(());
                    }
                    if let Ok(result) = timeout(Duration::from_millis(500), listener.accept()).await
                    {
                        let (stream2, addr) = result?;
                        info!(?addr, ?port, "new connection");

                        let id = Uuid::new_v4();
                        let conns = Arc::clone(&self.conns);

                        conns.insert(id, stream2);
                        tokio::spawn(async move {
                            // Remove stale entries to avoid memory leaks.
                            sleep(Duration::from_secs(10)).await;
                            if conns.remove(&id).is_some() {
                                warn!(%id, "removed stale connection");
                            }
                        });
                        stream.send(ServerMessage::Connection(id)).await?;
                    }
                }
            }
            Some(ClientMessage::Accept(id)) => {
                info!(%id, "forwarding connection");
                match self.conns.remove(&id) {
                    Some((_, mut stream2)) => {
                        let parts = stream.into_parts();
                        debug_assert!(parts.write_buf.is_empty(), "framed write buffer not empty");
                        stream2.write_all(&parts.read_buf).await?;
                        proxy(parts.io, stream2).await?
                    }
                    None => warn!(%id, "missing connection"),
                }
                Ok(())
            }
            None => Ok(()),
        }
    }
}
