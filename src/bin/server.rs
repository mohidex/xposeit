use anyhow::Result;
use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand};
use xposeit::XposeServer;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

pub enum Commands {
    Server(Box<XposeServer>),
}

#[derive(Subcommand, Debug)]
enum Command {
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
