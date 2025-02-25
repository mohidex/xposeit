use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::net::{TcpListener, TcpStream};
use crate::error::AppError;

pub struct TcpServer {
    pub tunnels: Arc<Mutex<HashMap<String, TcpStream>>>,
}

impl TcpServer {
    pub fn new() -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn run(&self, addr: &str) -> Result<(), AppError> {
        let listener = TcpListener::bind(addr).await?;
        println!("TCP server running at {}", addr);

        while let Ok((client_socket, _)) = listener.accept().await {
            let tunnels = self.tunnels.clone();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client_connection(client_socket, tunnels).await {
                    eprintln!("Error handling client connection: {}", e);
                }
            });
        }

        Ok(())
    }

    async fn handle_client_connection(
        mut client_socket: TcpStream,
        tunnels: Arc<Mutex<HashMap<String, TcpStream>>>,
    ) -> Result<(), AppError> {
        let mut buffer = [0; 1024];
        let n = client_socket.read(&mut buffer).await?;
        let subdomain = String::from_utf8_lossy(&buffer[..n]).trim().to_string();

        tunnels.lock().await.insert(subdomain.clone(), client_socket);

        println!("Tunnel registered for subdomain: {}", subdomain);
        Ok(())
    }
}
