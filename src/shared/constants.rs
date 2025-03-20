use std::time::Duration;

/// TCP port used for control connections with the server.
pub const CONTROL_PORT: u16 = 7835;

/// Maximum byte length for a JSON frame in the stream.
pub const MAX_FRAME_LENGTH: usize = 256;

/// Timeout for network connections and initial protocol messages.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(3);
