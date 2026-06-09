fn main() {
    let ids = finitechat_darkmatter::prove_http_delivery_core_orders_commit_then_message()
        .expect("HTTP delivery core smoke passes");
    println!(
        "finitechat-darkmatter-server: in-memory Darkmatter HTTP delivery core ready ({} smoke messages)",
        ids.len()
    );
}
