use std::time::Duration;

use clap::Parser;
use tokio::io::{self, AsyncReadExt};
use winkeyer_rs::WinKeyer;

#[derive(Debug, Parser)]
#[command(about = "Send stdin text as CW through a WinKeyer 3")]
struct Args {
    /// Serial device path, e.g. /dev/ttyUSB0 or /dev/ttyACM0
    #[arg(short, long)]
    port: String,

    /// Sending speed in words per minute
    #[arg(long, default_value_t = 20)]
    wpm: u8,

    /// Do not wait for the keyer to finish sending before closing
    #[arg(long)]
    no_wait: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut input = String::new();
    io::stdin().read_to_string(&mut input).await?;

    let (mut wk, rev) = WinKeyer::open(&args.port).await?;
    eprintln!("WinKeyer opened, revision byte: {rev}");
    wk.set_timeout(Duration::from_secs(2));
    wk.set_wpm(args.wpm).await?;
    wk.send_text(input).await?;

    if !args.no_wait {
        wk.wait_until_idle().await?;
    }

    wk.close().await?;
    Ok(())
}
