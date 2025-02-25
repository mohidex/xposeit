use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use hyper::{Body, Request};
use crate::error::AppError;

pub async fn run_client(server_addr: &str, subdomain: &str, local_addr: &str) -> Result<(), AppError> {
    let mut client_socket = TcpStream::connect(server_addr).await?;

    // Register the tunnel with the server
    client_socket.write_all(subdomain.as_bytes()).await?;

    println!("Tunnel registered for subdomain: {}", subdomain);

    // Handle forwarded requests
    let mut buffer = [0; 1024];
    loop {
        let n = client_socket.read(&mut buffer).await?;
        if n == 0 {
            break; // Connection closed
        }

        let request_line = String::from_utf8_lossy(&buffer[..n]);
        println!("Received request: {}", request_line);

        // Forward the request to the local service
        let client = hyper::Client::new();
        let uri = format!("http://{}{}", local_addr, request_line.split_whitespace().nth(1).unwrap_or(""));
        let forwarded_req = Request::builder()
            .method(request_line.split_whitespace().next().unwrap_or("GET"))
            .uri(uri)
            .body(Body::empty())
            .unwrap();

        let response = client.request(forwarded_req).await?;

        // Send the response back to the server
        let response_body = hyper::body::to_bytes(response.into_body()).await?;
        client_socket.write_all(&response_body).await?;
    }

    Ok(())
}
