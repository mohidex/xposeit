use anyhow::Result;
use clap::{Parser, Subcommand};
use xposeit::XposeCli;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    #[clap(subcommand)]
    command: Command,
}

pub enum Commands {
    Cli(Box<XposeCli>),
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
    }
    Ok(())
}
