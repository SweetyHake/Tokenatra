#[path = "../mcp.rs"]
mod mcp;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--install" || a == "--install-mcp") {
        let updated = mcp::register_mcp_clients(true);
        for p in updated {
            println!("Registered Tokenatra MCP in: {p}");
        }
        return;
    }
    std::process::exit(mcp::run_stdio());
}
