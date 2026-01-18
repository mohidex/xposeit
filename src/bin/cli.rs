use anyhow::Result;
use clap::Parser;
use xposeit::XposeCli;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// The local port to expose.
    #[clap(short, long)]
    port: u16,

    /// The local host to expose.
    #[clap(short, long, value_name = "LOCAL_HOST", default_value = "localhost")]
    local_host: String,

    /// Address of the remote server to expose local ports to.
    #[clap(short, long, value_name = "REMOTE_HOST", default_value = "localhost")]
    to: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let client = XposeCli::new(&args.local_host, args.port, &args.to);
    client.run().await?;

    Ok(())
}
