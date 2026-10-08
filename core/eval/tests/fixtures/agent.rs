//! Synthetic offline protocol fixture, not an AI agent or a benchmark contender.
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|s| s == "--version") {
        println!("kb-eval synthetic fixture 1");
        return;
    }
    if let Ok(forbidden) = std::env::var("KB_EVAL_FORBIDDEN") {
        assert!(
            fs::read(forbidden).is_err(),
            "fixture could read a protected file"
        );
    }
    // Pinned Codex clients refuse to start when CODEX_HOME names a missing directory.
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        assert!(
            std::path::Path::new(&home).is_dir(),
            "CODEX_HOME does not exist"
        );
    }
    // Network stages reach the controller's egress proxy, which refuses unapproved targets.
    // The request never names an approved host, so nothing leaves the machine.
    let mut proxy_port = None;
    if let Ok(url) = std::env::var("HTTPS_PROXY") {
        let address = url.strip_prefix("http://").expect("loopback proxy URL");
        proxy_port = address.rsplit_once(':').map(|(_, port)| port.to_string());
        let mut proxy = TcpStream::connect(address).expect("sandbox blocked the egress proxy");
        proxy
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        proxy
            .write_all(b"CONNECT unapproved.example.invalid:443 HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut reply = Vec::new();
        let _ = proxy.read_to_end(&mut reply);
        assert!(
            !reply.starts_with(b"HTTP/1.1 200"),
            "proxy tunnelled an unapproved host"
        );
    }
    if let Ok(port) = std::env::var("KB_EVAL_DENIED_PORT")
        && proxy_port.as_deref() != Some(port.as_str())
    {
        assert!(
            TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).is_err(),
            "sandbox reached a host loopback port other than the egress proxy"
        );
    }
    assert!(
        !std::path::Path::new("hidden-expected.txt").exists(),
        "hidden tests reached an agent"
    );
    let model = args
        .windows(2)
        .find(|s| s[0] == "--model")
        .map(|s| s[1].as_str())
        .unwrap_or("");
    let message = if model == "synthetic-judge" {
        assert!(!std::path::Path::new("AGENTS.md").exists());
        assert!(!std::path::Path::new(".git").exists());
        let case: serde_json::Value =
            serde_json::from_slice(&fs::read("case.json").unwrap()).unwrap();
        assert!(case.get("arm").is_none());
        serde_json::json!({"protocol":"kb.eval.judgment.v1","quality":[{"id":"behavior","score":1.0,"evidence":"The patch changes the synthetic value to fixed and the hidden check passes."}],"rules":[{"id":"rule","score":1.0,"evidence":"The patch preserves the stable value."}]}).to_string()
    } else {
        assert_eq!(fs::read_to_string("value.txt").unwrap(), "broken\n");
        fs::write("value.txt", "fixed\n").unwrap();
        // A hostile local config must not execute when the controller extracts the patch.
        let mut config = fs::read_to_string(".git/config").unwrap();
        config.push_str("\n[core]\n\tfsmonitor = /does/not/exist\n");
        fs::write(".git/config", config).unwrap();
        "Synthetic fixture completed; no model was called.".into()
    };
    println!(
        "{}",
        serde_json::json!({"type":"turn.completed","turn_id":"segment-one","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":1},"total_cost_usd":0.1})
    );
    println!(
        "{}",
        serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":message}})
    );
    println!(
        "{}",
        serde_json::json!({"type":"turn.completed","turn_id":"segment-two","usage":{"input_tokens":20,"cached_input_tokens":8,"output_tokens":2},"total_cost_usd":0.2})
    );
}
