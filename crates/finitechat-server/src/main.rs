use std::env;
use std::net::SocketAddr;

use finitechat_server::{HttpServerState, http_router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("serve") => serve(args.get(1).map(String::as_str)).await,
        Some("smoke") | None => {
            smoke();
            Ok(())
        }
        Some(command) => {
            Err(format!("unknown command '{command}'; expected 'serve' or 'smoke'").into())
        }
    }
}

async fn serve(addr: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let addr = addr.unwrap_or("127.0.0.1:8787").parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("finitechat-darkmatter-server: listening on http://{addr}");
    axum::serve(listener, http_router(HttpServerState::default())).await?;
    Ok(())
}

fn smoke() {
    let ids = finitechat_darkmatter::prove_http_delivery_core_orders_commit_then_message()
        .expect("HTTP delivery core smoke passes");
    println!(
        "finitechat-darkmatter-server: in-memory Darkmatter HTTP delivery core ready ({} smoke messages)",
        ids.len()
    );
}
