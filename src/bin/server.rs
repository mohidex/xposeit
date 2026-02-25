use anyhow::Result;
use clap::{error::ErrorKind, CommandFactory, Parser};
use xposeit::XposeServer;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Minimum port in the forwarding range.
    #[clap(long, default_value_t = 9000)]
    min_port: u16,

    /// Maximum port in the forwarding range.
    #[clap(long, default_value_t = 9999)]
    max_port: u16,

    /// How many ports to pre-bind from the range.
    /// This is the maximum number of simultaneous clients.
    #[clap(long, default_value_t = 100)]
    capacity: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if args.min_port > args.max_port {
        Args::command()
            .error(ErrorKind::InvalidValue, "min_port must be <= max_port")
            .exit();
    }

    if args.capacity == 0 {
        Args::command()
            .error(ErrorKind::InvalidValue, "capacity must be > 0")
            .exit();
    }

    let port_range = args.min_port..=args.max_port;
    let server = XposeServer::new(port_range, args.capacity).await;
    server.listen().await?;

    Ok(())
}
