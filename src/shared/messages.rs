use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Represents a message sent from the client to the server over the control connection.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum ClientMessage {
    /// Initial message sent by the client to request the server to open a port for forwarding.
    Open,

    /// Sent by the client to accept an incoming TCP connection and proxy it.
    Accept(Uuid),
}

/// Represents a message sent from the server to the client over the control connection.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum ServerMessage {
    /// Response to the client's `Open` request, indicating the port that has been opened for forwarding.
    Opened(u16),

    /// A heartbeat message sent by the server to check if the client is still reachable.
    Heartbeat,

    /// Sent by the server to request the client to accept a forwarded TCP connection.
    Connection(Uuid),

    /// Indicates an error on the server side that terminates the connection.
    Error(String),
}
