use anyhow::Result;
use clap::{error::ErrorKind, CommandFactory, Parser};
use xposeit::XposeServer;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Minimum accepted TCP port number.
    #[clap(long, default_value_t = 1024)]
    min_port: u16,

    /// Maximum accepted TCP port number.
    #[clap(long, default_value_t = 65535)]
    max_port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let port_range = args.min_port..=args.max_port;
    if port_range.is_empty() {
        Args::command()
            .error(ErrorKind::InvalidValue, "port range is empty")
            .exit();
    }

    let server = XposeServer::new(port_range);
    server.listen().await?;

    Ok(())
}
