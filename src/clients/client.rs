use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::{io::AsyncWriteExt, net::TcpStream, time::timeout};
use tracing::{error, info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::shared::frame::Delimited;
use crate::shared::proxy::proxy;

use crate::shared::messages::{ClientMessage, ServerMessage};

/// State structure for the client.
pub struct XposeCli {
    /// Control connection to the server.
    conn: Option<Delimited<TcpStream>>,

    // Local host that is forwarded.
    local_host: String,

    /// Local port that is forwarded.
    port: u16,

    /// Destination address of the server.
    to: String,
}

impl XposeCli {
    /// Create a new client.
    pub async fn new(local_host: &str, port: u16, to: &str) -> Result<Self> {
        let mut stream = Delimited::new(connect_with_timeout(to, 7835).await?);

        stream.send(ClientMessage::Open).await?;
        let remote_port = match stream.recv_timeout().await? {
            Some(ServerMessage::Opened(remote_port)) => remote_port,
            Some(ServerMessage::Error(message)) => bail!("server error: {message}"),
            Some(_) => bail!("unexpected initial non-hello message"),
            None => bail!("unexpected EOF"),
        };
        info!(remote_port, "connected to server");
        info!("listening at {to}:{remote_port}");

        Ok(XposeCli {
            conn: Some(stream),
            to: to.to_string(),
            local_host: local_host.to_string(),
            port,
        })
    }

    /// Start the client, listening for new connections.
    pub async fn listen(mut self) -> Result<()> {
        let mut conn = self.conn.take().unwrap();
        let this = Arc::new(self);
        loop {
            match conn.recv().await? {
                Some(ServerMessage::Opened(_)) => warn!("unexpected request"),
                Some(ServerMessage::Heartbeat) => (),
                Some(ServerMessage::Connection(id)) => {
                    let this = Arc::clone(&this);
                    tokio::spawn(
                        async move {
                            info!("new connection");
                            match this.handle_connection(id).await {
                                Ok(_) => info!("connection exited"),
                                Err(err) => warn!(%err, "connection exited with error"),
                            }
                        }
                        .instrument(info_span!("proxy", %id)),
                    );
                }
                Some(ServerMessage::Error(err)) => error!(%err, "server error"),
                None => return Ok(()),
            }
        }
    }

    async fn handle_connection(&self, id: Uuid) -> Result<()> {
        let mut remote_conn = Delimited::new(connect_with_timeout(&self.to[..], 7835).await?);
        remote_conn.send(ClientMessage::Accept(id)).await?;
        let mut local_conn = connect_with_timeout(&self.local_host, self.port).await?;
        let parts = remote_conn.into_parts();
        debug_assert!(parts.write_buf.is_empty(), "framed write buffer not empty");
        local_conn.write_all(&parts.read_buf).await?; // mostly of the cases, this will be empty
        proxy(local_conn, parts.io).await?;
        Ok(())
    }
}

async fn connect_with_timeout(to: &str, port: u16) -> Result<TcpStream> {
    match timeout(Duration::from_secs(3), TcpStream::connect((to, port))).await {
        Ok(res) => res,
        Err(err) => Err(err.into()),
    }
    .with_context(|| format!("could not connect to {to}:{port}"))
}
