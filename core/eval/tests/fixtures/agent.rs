//! Synthetic offline protocol fixture, not an AI agent or a benchmark contender.
use std::fs;
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
