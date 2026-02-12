mod client;
mod conn_pool;
mod connection;
mod crates_api;
mod discovery;
mod hyper_tls;
mod tui;
use client::{Client, Receiver};
pub use crates_api::InfoErr;
pub use tui::FilterReason;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Runtime::new()?;

    let user = std::env::var("CRATES_USER_AGENT").unwrap_or({
        println!("Input a user agent that will be used by the crates.io client");
        println!();
        println!("Otherwise you will get the following response and this service will not work");
        println!("We require that all requests include a `User-Agent` header.");
        println!("To allow us to determine the impact your bot has on our \nservice...Including contact information will also reduce the chance that we \nwill need to take action against your bot");
        println!();
        println!("The full crates.io Crawler Policy can be found at https://crates.io/policies#crawlers");
        println!();
        println!("The `CRATES_USER_AGENT` environment variable can also be set to be used as the used user agent header.");
        println!();
        print!("User-Agent: ");

        use std::io::Write;
        std::io::stdout().flush()?;

        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        buf.trim().to_string()
    });

    let (client, info_rx) = rt.block_on(async move { Client::new(user).await })?;

    let max_recv = 10;

    use tokio::sync::mpsc;
    let (tx, rx) = mpsc::unbounded_channel();

    rt.spawn(async move {
        client.spin(rx).await;
    });

    let tui = tui::Tui::setup(info_rx, tx, max_recv)?;
    Ok(tui.run()?)
}
