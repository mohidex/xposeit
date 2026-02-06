use anyhow::{anyhow, bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::{io::AsyncWriteExt, net::TcpStream, time::timeout};
use tracing::{error, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::protocol::{
    frame::Delimited,
    messages::{ClientMessage, ServerMessage},
};
use crate::utils::proxy;

/// Client session state machine
struct Session<S> {
    conn: Delimited<TcpStream>,
    state: S,
}

/// Initial state
struct Init;

/// Opened state
struct Opened {
    remote_port: u16,
}

pub struct XposeCli {
    /// The local host to expose.
    local_host: String,

    /// The local port to expose.
    local_port: u16,

    /// Address of the remote server.
    server: String,
}

impl XposeCli {
    pub fn new(local_host: &str, port: u16, server: &str) -> Self {
        Self {
            local_host: local_host.to_string(),
            local_port: port,
            server: server.to_string(),
        }
    }

    pub async fn run(self) -> Result<()> {
        let stream = Delimited::new(connect_with_timeout(&self.server, 7835).await?);

        let session = Session {
            conn: stream,
            state: Init,
        };

        let session = session.open().await?;

        info!("listening at {}:{}", self.server, session.state.remote_port);

        session.listen(Arc::new(self)).await
    }
}

/// Implement session state machine
impl Session<Init> {
    async fn open(mut self) -> Result<Session<Opened>> {
        self.conn.send(ClientMessage::Open).await?;

        match self.conn.recv_timeout().await? {
            Some(ServerMessage::Opened(port)) => {
                info!(remote_port = port, "connected to server");
                Ok(Session {
                    conn: self.conn,
                    state: Opened { remote_port: port },
                })
            }
            Some(ServerMessage::Error(msg)) => bail!("server error: {msg}"),
            Some(_) => bail!("protocol error: unexpected message"),
            None => bail!("unexpected EOF"),
        }
    }
}

impl Session<Opened> {
    async fn listen(mut self, cli: Arc<XposeCli>) -> Result<()> {
        loop {
            match self.conn.recv().await? {
                Some(ServerMessage::Heartbeat) => {
                    // Optionally log heartbeats at trace level
                    // trace!("received heartbeat");
                }

                Some(ServerMessage::Connection(id)) => {
                    info!(%id, "received forwarding request");
                    let cli = Arc::clone(&cli);
                    tokio::spawn(
                        async move {
                            if let Err(err) = handle_proxy(cli, id).await {
                                warn!(%err, "proxy connection failed");
                            }
                        }
                        .instrument(info_span!("proxy", %id)),
                    );
                }

                Some(ServerMessage::Error(err)) => {
                    error!(%err, "server error");
                    return Err(anyhow!("server error: {err}"));
                }

                Some(ServerMessage::Opened(_)) => {
                    warn!("protocol error: unexpected Opened message");
                    return Err(anyhow!("protocol error: duplicate Opened"));
                }

                None => {
                    info!("control connection closed by server");
                    return Ok(());
                }
            }
        }
    }
}

/// Handle a proxy connection
async fn handle_proxy(cli: Arc<XposeCli>, id: Uuid) -> Result<()> {
    info!(%id, "establishing proxy connection");

    // Create a NEW connection specifically for this proxy session
    let mut control = Delimited::new(connect_with_timeout(&cli.server, 7835).await?);

    // Send Accept on this NEW connection
    control.send(ClientMessage::Accept(id)).await?;

    // Connect to local service
    let mut local = connect_with_timeout(&cli.local_host, cli.local_port).await?;

    // Extract the underlying stream from the framed connection
    let parts = control.into_parts();

    debug_assert!(parts.write_buf.is_empty(), "framed write buffer not empty");

    // If there's any buffered data from the server, write it to local
    if !parts.read_buf.is_empty() {
        local.write_all(&parts.read_buf).await?;
    }

    // Proxy bidirectional data between local service and remote client
    info!(%id, "proxying data");
    proxy(local, parts.io).await?;

    info!(%id, "proxy connection closed");
    Ok(())
}

async fn connect_with_timeout(to: &str, port: u16) -> Result<TcpStream> {
    timeout(Duration::from_secs(3), TcpStream::connect((to, port)))
        .await
        .context("connect timeout")?
        .context(format!("could not connect to {to}:{port}"))
}
