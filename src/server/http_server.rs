use std::sync::Arc;
use hyper::header::HeaderValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};
use crate::error::AppError;
use crate::server::tcp_server::TcpServer;

pub struct HttpServer {
    pub tcp_server: Arc<TcpServer>,
}

impl HttpServer {
    pub fn new(tcp_server: Arc<TcpServer>) -> Self {
        Self { tcp_server }
    }

    pub async fn run(&self, addr: &str) -> Result<(), AppError> {
        let tcp_server = self.tcp_server.clone();
        let make_svc = make_service_fn(move |_conn| {
            let tcp_server = tcp_server.clone();
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    let tcp_server = tcp_server.clone();
                    async move { Self::handle_http_request(req, tcp_server).await }
                }))
            }
        });

        let server = Server::bind(&addr.parse()?).serve(make_svc);
        println!("HTTP server running at {}", addr);

        // Await the server future
        if let Err(e) = server.await {
            eprintln!("Server error: {}", e);
        }

        Ok(())
    }

    fn get_host(req: &Request<Body>) -> Option<&str> {
        req.headers()
        .get("host")
        .and_then(|hv: &HeaderValue| hv.to_str().ok())
    }

    async fn handle_http_request(
        req: Request<Body>,
        tcp_server: Arc<TcpServer>,
    ) -> Result<Response<Body>, hyper::Error> {
        let host = Self::get_host(&req).unwrap_or("").to_string();

        let subdomain = host.split('.').next().unwrap_or("");

        let mut tunnels = tcp_server.tunnels.lock().await;
        if let Some(client_socket) = tunnels.get_mut(subdomain) {
            // Forward the HTTP request to the client over the TCP connection
            let request_line = format!("{} {} {:?}", req.method(), req.uri().path(), req.headers());
            client_socket.write_all(request_line.as_bytes()).await.unwrap();

            // Wait for the client's response
            let mut buffer = [0; 1024];
            let n = client_socket.read(&mut buffer).await.unwrap();
            let response_body = String::from_utf8(buffer[..n].to_vec()).unwrap();

            let response = Response::builder()
                .status(200)
                .body(Body::from(response_body))
                .unwrap();
            Ok(response)
        } else {
            Ok(Response::builder()
                .status(404)
                .body(Body::from("Tunnel not found"))
                .unwrap())
        }
    }
}
