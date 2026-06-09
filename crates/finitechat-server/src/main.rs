use std::env;
use std::net::SocketAddr;

use finitechat_server::{HttpServerState, http_router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]).await,
        Some("smoke") | None => {
            smoke();
            Ok(())
        }
        Some(command) => Err(format!(
            "unknown command '{command}'; expected 'serve [addr] [--sqlite PATH]' or 'smoke'"
        )
        .into()),
    }
}

async fn serve(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let options = ServeOptions::parse(args)?;
    let addr = options.addr.parse::<SocketAddr>()?;
    let state = match options.sqlite_path {
        Some(path) => HttpServerState::from_sqlite_path(path)?,
        None => HttpServerState::default(),
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("finitechat-darkmatter-server: listening on http://{addr}");
    axum::serve(listener, http_router(state)).await?;
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

#[derive(Debug)]
struct ServeOptions {
    addr: String,
    sqlite_path: Option<String>,
}

impl ServeOptions {
    fn parse(args: &[String]) -> Result<Self, Box<dyn std::error::Error>> {
        let mut addr = None;
        let mut sqlite_path = None;
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--sqlite" => {
                    index += 1;
                    let Some(path) = args.get(index) else {
                        return Err("missing value for --sqlite".into());
                    };
                    sqlite_path = Some(path.clone());
                }
                value if value.starts_with("--") => {
                    return Err(format!("unknown serve option '{value}'").into());
                }
                value => {
                    if addr.replace(value.to_owned()).is_some() {
                        return Err("serve accepts at most one address".into());
                    }
                }
            }
            index += 1;
        }
        Ok(Self {
            addr: addr.unwrap_or_else(|| "127.0.0.1:8787".to_owned()),
            sqlite_path,
        })
    }
}
