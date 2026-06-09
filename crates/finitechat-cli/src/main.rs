fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("compat-report") => {
            let findings = finitechat_darkmatter::current_port_findings();
            println!(
                "{}",
                serde_json::to_string_pretty(&findings).expect("compat report serializes")
            );
        }
        Some("http-smoke") => {
            let ids = finitechat_darkmatter::prove_http_delivery_core_orders_commit_then_message()
                .expect("HTTP delivery core smoke passes");
            println!(
                "ordered {} messages through Darkmatter HTTP delivery core",
                ids.len()
            );
        }
        _ => {
            eprintln!("usage: finitechat-darkmatter <compat-report|http-smoke>");
            std::process::exit(2);
        }
    }
}
