use anyhow::{anyhow, bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::{io::copy_bidirectional, io::AsyncWriteExt, net::TcpStream, time::timeout};
use tracing::{error, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::protocol::{
    frame::JsonTransport,
    messages::{ClientMessage, ServerMessage},
};

/// Timeout for establishing a TCP connection to the server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Timeout waiting for the server to assign a port during the open handshake.
/// Must be greater than the server-side POOL_WAIT_TIMEOUT (30s) so the server
/// always has time to respond before the client gives up.
const OPEN_TIMEOUT: Duration = Duration::from_secs(35);

/// Initial state: TCP connection established, handshake not yet done.
struct Init;

/// Opened state: server has assigned a remote port.
struct Opened {
    remote_port: u16,
}

struct Session<S> {
    conn: JsonTransport<TcpStream>,
    state: S,
}

impl Session<Init> {
    fn new(conn: JsonTransport<TcpStream>) -> Self {
        Self { conn, state: Init }
    }

    /// Sends `Open`, then loops until the server either:
    ///   - sends `Waiting`  → logs progress, keeps waiting
    ///   - sends `Opened`   → transitions to `Session<Opened>`
    ///   - sends `Error`    → returns a descriptive error
    ///   - closes / timeout → returns a descriptive error
    async fn open(mut self) -> Result<Session<Opened>> {
        self.conn.send(ClientMessage::Open).await?;

        let port = loop {
            let msg = timeout(OPEN_TIMEOUT, self.conn.recv())
                .await
                .map_err(|_| {
                    anyhow!(
                        "timed out waiting for a port assignment — server may be at capacity, try again shortly"
                    )
                })?
                .context("unexpected EOF waiting for server handshake")?;

            match msg {
                Some(ServerMessage::Waiting) => {
                    info!("server is at capacity, waiting for a free port...");
                    // Server will send Opened once a port is free; keep looping.
                }
                Some(ServerMessage::Opened(port)) => break port,
                Some(ServerMessage::Error(msg)) => bail!("server error: {msg}"),
                Some(other) => {
                    bail!("protocol error: unexpected message during handshake: {other:?}")
                }
                None => bail!("server closed connection before assigning a port"),
            }
        };

        info!(remote_port = port, "connected to server");
        Ok(Session {
            conn: self.conn,
            state: Opened { remote_port: port },
        })
    }
}

impl Session<Opened> {
    /// Drives the control loop: dispatches incoming forwarding requests and
    /// handles heartbeats until the server closes the connection.
    async fn listen(mut self, cli: Arc<XposeCli>) -> Result<()> {
        info!("listening at {}:{}", cli.server, self.state.remote_port);

        loop {
            match self.conn.recv().await? {
                Some(ServerMessage::Heartbeat) => {
                    // Control link is alive — nothing to do.
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
                    error!(%err, "server reported an error");
                    return Err(anyhow!("server error: {err}"));
                }

                // These messages are only valid during the handshake phase.
                Some(ServerMessage::Opened(_)) => {
                    return Err(anyhow!("protocol error: unexpected Opened on live session"));
                }
                Some(ServerMessage::Waiting) => {
                    return Err(anyhow!(
                        "protocol error: unexpected Waiting on live session"
                    ));
                }

                None => {
                    info!("control connection closed by server");
                    return Ok(());
                }
            }
        }
    }
}

pub struct XposeCli {
    /// The local host to expose.
    local_host: String,

    /// The local port to expose.
    local_port: u16,

    /// Address of the remote server.
    server: String,

    /// Control port on the remote server.
    server_port: u16,
}

impl XposeCli {
    pub fn new(local_host: &str, local_port: u16, server: &str, server_port: u16) -> Self {
        Self {
            local_host: local_host.to_string(),
            local_port,
            server: server.to_string(),
            server_port,
        }
    }

    pub async fn run(self) -> Result<()> {
        let stream = connect_with_timeout(&self.server, self.server_port).await?;
        let session = Session::<Init>::new(JsonTransport::new(stream));
        let session = session.open().await?;
        session.listen(Arc::new(self)).await
    }
}

/// Opens a fresh control connection to the server, sends `Accept(id)`, then
/// splices the resulting stream directly to the local service.
async fn handle_proxy(cli: Arc<XposeCli>, id: Uuid) -> Result<()> {
    info!(%id, "establishing proxy connection");

    // Each proxy uses its own dedicated connection — never the control channel.
    let mut control = JsonTransport::new(connect_with_timeout(&cli.server, cli.server_port).await?);

    // Identify this connection to the server.
    control
        .send(ClientMessage::Accept(id))
        .await
        .context("failed to send Accept to server")?;

    // Connect to the local service that is being exposed.
    let mut local = connect_with_timeout(&cli.local_host, cli.local_port)
        .await
        .with_context(|| {
            format!(
                "could not reach local service at {}:{}",
                cli.local_host, cli.local_port
            )
        })?;

    // Unwrap the framed connection into its raw parts.
    let mut parts = control.into_parts();

    debug_assert!(
        parts.write_buf.is_empty(),
        "framed write buffer unexpectedly non-empty after Accept"
    );

    // Any bytes the server already sent before we switched to raw mode must be
    // forwarded to the local service first.
    if !parts.read_buf.is_empty() {
        local
            .write_all(&parts.read_buf)
            .await
            .context("failed to flush pre-buffered server bytes to local service")?;
        local
            .flush()
            .await
            .context("flush to local service failed")?;
    }

    info!(
        %id,
        local = format!("{}:{}", cli.local_host, cli.local_port),
        "proxy active"
    );

    let (b_up, b_down) = copy_bidirectional(&mut local, &mut parts.io)
        .await
        .context("bidirectional copy failed")?;

    info!(%id, b_up, b_down, "proxy closed gracefully");

    // Best-effort half-close on both sides.
    let _ = local.shutdown().await;
    let _ = parts.io.shutdown().await;

    Ok(())
}

async fn connect_with_timeout(host: &str, port: u16) -> Result<TcpStream> {
    timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("connection to {host}:{port} timed out after {CONNECT_TIMEOUT:?}"))?
        .with_context(|| format!("could not connect to {host}:{port}"))
}
