use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;
use tokio_util::codec::{AnyDelimiterCodec, Framed, FramedParts};
use tracing::trace;

pub struct Delimited<U>(Framed<U, AnyDelimiterCodec>);

impl<U: AsyncRead + AsyncWrite + Unpin> Delimited<U> {
    /// Construct a new delimited stream.
    pub fn new(stream: U) -> Self {
        let codec = AnyDelimiterCodec::new_with_max_length(vec![0], vec![0], 256);
        Self(Framed::new(stream, codec))
    }

    /// Read the next null-delimited JSON instruction from a stream.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        trace!("waiting to receive json message");
        if let Some(next_message) = self.0.next().await {
            let byte_message = next_message.context("frame error, invalid byte length")?;
            let serialized_obj =
                serde_json::from_slice(&byte_message).context("unable to parse message")?;
            Ok(serialized_obj)
        } else {
            Ok(None)
        }
    }

    /// Read the next null-delimited JSON instruction, with a default timeout.
    pub async fn recv_timeout<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        timeout(Duration::from_secs(3), self.recv())
            .await
            .context("timed out waiting for initial message")?
    }

    /// Send a null-terminated JSON instruction on a stream.
    pub async fn send<T: Serialize>(&mut self, msg: T) -> Result<()> {
        trace!("sending json message");
        self.0.send(serde_json::to_string(&msg)?).await?;
        Ok(())
    }

    /// Consume this object, returning current buffers and the inner transport.
    pub fn into_parts(self) -> FramedParts<U, AnyDelimiterCodec> {
        self.0.into_parts()
    }
}

// Unit tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame::Delimited;
    use crate::protocol::messages::{ClientMessage, ServerMessage};
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn test_send_and_recv() -> Result<()> {
        // Create a TCP listener
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // Spawn a server task
        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut delimited = Delimited::new(socket);

            // Receive a message from the client
            let msg: Option<ClientMessage> = delimited.recv().await?;
            assert_eq!(msg, Some(ClientMessage::Open)); // This will now work

            // Send a response to the client
            delimited.send(ServerMessage::Opened(8080)).await?;
            Ok::<(), anyhow::Error>(())
        });

        // Connect to the server
        let stream = TcpStream::connect(addr).await?;
        let mut delimited = Delimited::new(stream);

        // Send a message to the server
        delimited.send(ClientMessage::Open).await?;

        // Receive a response from the server
        let msg: Option<ServerMessage> = delimited.recv().await?;
        assert_eq!(msg, Some(ServerMessage::Opened(8080))); // This will now work

        // Wait for the server task to complete
        server_task.await??;

        Ok(())
    }

    #[tokio::test]
    async fn test_into_parts() -> Result<()> {
        // Create a TCP listener
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // Spawn a server task
        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let delimited = Delimited::new(socket);

            // Consume the delimited stream and return its parts
            let parts = delimited.into_parts();
            assert!(parts.read_buf.is_empty()); // Ensure the read buffer is empty
            assert!(parts.write_buf.is_empty()); // Ensure the write buffer is empty
            Ok::<(), anyhow::Error>(())
        });

        // Connect to the server
        let stream = TcpStream::connect(addr).await?;
        let delimited = Delimited::new(stream);

        // Consume the delimited stream and return its parts
        let parts = delimited.into_parts();
        assert!(parts.read_buf.is_empty()); // Ensure the read buffer is empty
        assert!(parts.write_buf.is_empty()); // Ensure the write buffer is empty

        // Wait for the server task to complete
        server_task.await??;

        Ok(())
    }

    #[tokio::test]
    async fn test_serialization_and_deserialization() -> Result<()> {
        // Create a TCP listener
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // Spawn a server task
        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut delimited = Delimited::new(socket);

            // Receive a message from the client
            let msg: Option<String> = delimited.recv().await?;
            let client_msg: ClientMessage = serde_json::from_str(&msg.unwrap())?;
            assert_eq!(client_msg, ClientMessage::Open);

            // Send a response to the client
            let server_msg = serde_json::to_string(&ServerMessage::Opened(8080))?;
            delimited.send(server_msg).await?;

            Ok::<(), anyhow::Error>(())
        });

        // Connect to the server
        let stream = TcpStream::connect(addr).await?;
        let mut delimited = Delimited::new(stream);

        // Send a message to the server
        let client_msg = serde_json::to_string(&ClientMessage::Open)?;
        delimited.send(client_msg).await?;

        // Receive a response from the server
        let msg: Option<String> = delimited.recv().await?;
        let server_msg: ServerMessage = serde_json::from_str(&msg.unwrap())?;
        assert_eq!(server_msg, ServerMessage::Opened(8080));

        // Wait for the server task to complete
        server_task.await??;

        Ok(())
    }
}
