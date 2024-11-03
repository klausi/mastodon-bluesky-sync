use clap::Parser;
use mastodon_bluesky_sync::{args::Args, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args = Args::parse();

    if let Err(err) = run(args).await {
        eprintln!("Error: {err}");
        for cause in err.chain().skip(1) {
            eprintln!("Because: {cause}");
        }
        std::process::exit(1);
    }
    Ok(())
}
