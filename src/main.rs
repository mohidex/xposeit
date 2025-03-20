mod client;
mod server;
mod shared;

use crate::client::cli::XposeCli;
use crate::server::xpose::XposeServer;
use anyhow::Result;
use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand};

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

pub enum Commands {
    Cli(Box<XposeCli>),
    Server(Box<XposeServer>),
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Starts a local proxy to the remote server.
    Xpose {
        /// The local port to expose.
        #[clap(short, long)]
        port: u16,

        /// The local host to expose.
        #[clap(short, long, value_name = "LOCAL_HOST", default_value = "localhost")]
        local_host: String,

        /// Address of the remote server to expose local ports to.
        #[clap(short, long, value_name = "REMOTE_HOST", default_value = "localhost")]
        to: String,
    },

    /// Runs the remote proxy server.
    Server {
        /// Minimum accepted TCP port number.
        #[clap(long, default_value_t = 1024)]
        min_port: u16,

        /// Maximum accepted TCP port number.
        #[clap(long, default_value_t = 65535)]
        max_port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    match Args::parse().command {
        Command::Xpose {
            local_host,
            port,
            to,
        } => {
            let client = XposeCli::new(&local_host, port, &to).await?;
            client.listen().await?;
        }
        Command::Server { min_port, max_port } => {
            let port_range = min_port..=max_port;
            if port_range.is_empty() {
                Args::command()
                    .error(ErrorKind::InvalidValue, "port range is empty")
                    .exit();
            }
            XposeServer::new(port_range).listen().await?;
        }
    }
    Ok(())
}
