use std::sync::Arc;
use tokio::net::TcpListener;

use dashmap::DashMap;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Pool of pre-bound, ready-to-accept listeners.
/// Sessions check one out and return it when they finish.
#[derive(Debug)]
pub struct ListenerPool(Arc<Mutex<Vec<TcpListener>>>);

impl ListenerPool {
    pub fn new(listeners: Vec<TcpListener>) -> Self {
        Self(Arc::new(Mutex::new(listeners)))
    }

    pub async fn acquire(&self) -> Option<TcpListener> {
        self.0.lock().await.pop()
    }

    pub async fn release(&self, listener: TcpListener) {
        let mut pool = self.0.lock().await;
        pool.push(listener);
    }

    pub async fn available(&self) -> usize {
        self.0.lock().await.len()
    }
}

#[derive(Clone)]
pub struct TunnelStreams(Arc<DashMap<Uuid, TcpStream>>);

impl TunnelStreams {
    pub fn new() -> Self {
        Self(Arc::new(DashMap::new()))
    }

    pub fn insert(&self, id: Uuid, stream: TcpStream) {
        self.0.insert(id, stream);
    }

    pub fn remove(&self, id: &Uuid) -> Option<(Uuid, TcpStream)> {
        self.0.remove(id)
    }
}
