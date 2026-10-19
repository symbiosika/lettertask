//! `graph-fake [--port 8099] [--empty]` – runs the fake Graph server standalone.

use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let mut port: u16 = 8099;
    let mut seed_demo = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).expect("--port needs a number"),
            "--empty" => seed_demo = false,
            "-h" | "--help" => {
                println!("graph-fake [--port 8099] [--empty]\n\nFake Microsoft Graph v1.0 + Entra OAuth for development and tests.");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let opts = graph_fake::FakeOptions { seed_demo, now: chrono::Utc::now() };
    let server = graph_fake::spawn_on(SocketAddr::from(([127, 0, 0, 1], port)), opts).await.expect("bind");
    println!("graph-fake listening on {}", server.base_url);
    println!("  Graph:     {}/v1.0", server.base_url);
    println!("  Authority: {}/organizations", server.base_url);
    println!("  Test token: {}", server.issue_token());
    let _ = tokio::signal::ctrl_c().await;
}
