mod client;
mod server;
mod error;

use anyhow::Result;
use clap::{Arg, Command};
use crate::server::{tcp_server::TcpServer, http_server::HttpServer};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Command::new("xposeit")
        .version("0.1.0")
        .author("Mohidul Islam")
        .about("Instantly expose your local apps")
        .subcommand(
            Command::new("server")
                .about("Runs the server")
                .arg(
                    Arg::new("http_addr")
                        .short('w')
                        .long("http-addr")
                        .value_name("HTTP_ADDR")
                        .help("Address to bind the HTTP server to")
                        .default_value("0.0.0.0:8080"),
                )
                .arg(
                    Arg::new("tcp_addr")
                        .short('t')
                        .long("tcp-addr")
                        .value_name("TCP_ADDR")
                        .help("Address to bind the TCP server to")
                        .default_value("0.0.0.0:8081"),
                ),
        )
        .subcommand(
            Command::new("client")
                .about("Runs the client")
                .arg(
                    Arg::new("server_addr")
                        .short('s')
                        .long("server-addr")
                        .value_name("SERVER_ADDR")
                        .help("Address of the server")
                        .default_value("127.0.0.1:8081"),
                )
                .arg(
                    Arg::new("subdomain")
                        .short('d')
                        .long("subdomain")
                        .value_name("SUBDOMAIN")
                        .help("Subdomain for the tunnel")
                        .required(true),
                )
                .arg(
                    Arg::new("local_addr")
                        .short('l')
                        .long("local-addr")
                        .value_name("LOCAL_ADDR")
                        .help("Local address to forward requests to")
                        .default_value("127.0.0.1:3000"),
                ),
        )
        .get_matches();

    match matches.subcommand() {
        Some(("server", sub_matches)) => {
            // Extract values from `sub_matches` and clone them
            let http_addr = sub_matches.get_one::<String>("http_addr").unwrap().clone();
            let tcp_addr = sub_matches.get_one::<String>("tcp_addr").unwrap().clone();

            let tcp_server = Arc::new(TcpServer::new());
            let http_server = HttpServer::new(tcp_server.clone());

            // Spawn the TCP server in a separate task
            let tcp_server_clone = tcp_server.clone();
            tokio::spawn(async move {
                tcp_server_clone.run(&tcp_addr).await.unwrap();
            });

            // Run the HTTP server
            http_server.run(&http_addr).await?;
        }
        Some(("client", sub_matches)) => {
            // Extract values from `sub_matches` and clone them
            let server_addr = sub_matches.get_one::<String>("server_addr").unwrap().clone();
            let subdomain = sub_matches.get_one::<String>("subdomain").unwrap().clone();
            let local_addr = sub_matches.get_one::<String>("local_addr").unwrap().clone();

            // Run the client
            client::client::run_client(&server_addr, &subdomain, &local_addr).await?;
        }
        _ => eprintln!("Invalid subcommand. Use 'server' or 'client'."),
    }

    Ok(())
}