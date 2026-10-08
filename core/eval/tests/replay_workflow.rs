use kb_eval::{
    accounting::{Basis, Convention},
    files,
    model::*,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(home: &Path, args: &[&str]) -> serde_json::Value {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kb-eval"));
    files::isolated_env(&mut c, home);
    c.args(args);
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "kb-eval failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
fn git(root: &Path, home: &Path, args: &[&str]) -> String {
    String::from_utf8(files::git(root, home, args).unwrap())
        .unwrap()
        .trim()
        .into()
}
fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn frozen_checkout_has_no_future_objects_or_alternates_and_preserves_input() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let repo = t.path().join("source");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&repo).unwrap();
    git(&repo, &home, &["init", "-q"]);
    fs::write(repo.join("past.txt"), "synthetic past").unwrap();
    git(&repo, &home, &["add", "."]);
    git(&repo, &home, &["commit", "-qm", "synthetic ancestor"]);
    let past = git(&repo, &home, &["rev-parse", "HEAD"]);
    fs::write(repo.join("value.txt"), "before").unwrap();
    git(&repo, &home, &["add", "."]);
    git(&repo, &home, &["commit", "-qm", "synthetic base"]);
    let base = git(&repo, &home, &["rev-parse", "HEAD"]);
    fs::write(repo.join("future.txt"), "future secret").unwrap();
    git(&repo, &home, &["add", "."]);
    git(&repo, &home, &["commit", "-qm", "synthetic future"]);
    let future = git(&repo, &home, &["rev-parse", "HEAD"]);
    fs::write(repo.join("value.txt"), "dirty source").unwrap();
    let out = t.path().join("frozen");
    files::checkout(&repo, &base, &out, &home, History::BaseOnly).unwrap();
    assert_eq!(fs::read_to_string(out.join("value.txt")).unwrap(), "before");
    assert!(!out.join("future.txt").exists());
    assert!(!out.join(".git/objects/info/alternates").exists());
    assert!(files::git(&out, &home, &["cat-file", "-e", &future]).is_err());
    assert!(files::git(&out, &home, &["cat-file", "-e", &past]).is_err());
    let ancestors = t.path().join("with-past");
    files::checkout(&repo, &base, &ancestors, &home, History::Ancestors).unwrap();
    assert!(files::git(&ancestors, &home, &["cat-file", "-e", &past]).is_ok());
    assert!(
        files::git(
            &ancestors,
            &home,
            &["merge-base", "--is-ancestor", &past, &base]
        )
        .is_ok()
    );
    assert!(files::git(&ancestors, &home, &["cat-file", "-e", &future]).is_err());
    assert_eq!(
        fs::read_to_string(repo.join("value.txt")).unwrap(),
        "dirty source"
    );
}

#[test]
#[ignore = "requires a functioning Seatbelt or bubblewrap host; run explicitly in the isolation CI step"]
fn isolated_qualification_coder_judge_and_accounting_workflow() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("controller-home");
    let repo = t.path().join("synthetic-source");
    let hidden = t.path().join("hidden");
    for dir in [&home, &repo, &hidden] {
        fs::create_dir(dir).unwrap();
    }
    git(&repo, &home, &["init", "-q"]);
    fs::write(repo.join("value.txt"), "broken\n").unwrap();
    fs::write(repo.join("stable.txt"), "stable\n").unwrap();
    fs::write(repo.join("stable-reference.txt"), "stable\n").unwrap();
    fs::write(
        repo.join("AGENTS.md"),
        "SYNTHETIC PROJECT RULES. They must not be auto-loaded by the judge.\n",
    )
    .unwrap();
    git(&repo, &home, &["add", "."]);
    git(&repo, &home, &["commit", "-qm", "synthetic base"]);
    let base = git(&repo, &home, &["rev-parse", "HEAD"]);
    fs::write(repo.join("value.txt"), "fixed\n").unwrap();
    git(&repo, &home, &["add", "."]);
    git(&repo, &home, &["commit", "-qm", "synthetic golden"]);
    let golden = git(&repo, &home, &["rev-parse", "HEAD"]);
    fs::write(hidden.join("hidden-expected.txt"), "fixed\n").unwrap();
    let canary = t.path().join("private-future.txt");
    fs::write(&canary, "synthetic future").unwrap();
    // A live host loopback port that no sandboxed stage may reach.
    let denied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let program = |exe: &str, args: Vec<&str>| Program {
        program: exe.into(),
        args: args.into_iter().map(str::to_string).collect(),
        timeout_seconds: 20,
    };
    let task = Task {
        protocol: "kb.eval.task.v1".into(),
        id: "synthetic-fix".into(),
        repository: repo.clone(),
        base: base.clone(),
        golden,
        as_of: base,
        history: History::BaseOnly,
        prompt: "Repair the synthetic value; preserve the stable value.".into(),
        hidden: hidden.clone(),
        hidden_sha256: files::tree_digest(&hidden).unwrap(),
        build: vec![program("/usr/bin/true", vec![])],
        regression: vec![program(
            "/usr/bin/cmp",
            vec!["stable.txt", "stable-reference.txt"],
        )],
        test: vec![program(
            "/usr/bin/cmp",
            vec!["value.txt", "hidden-expected.txt"],
        )],
        quality: vec![Criterion {
            id: "behavior".into(),
            text: "The synthetic value is fixed.".into(),
            weight: 1.0,
            metric: None,
        }],
        rules: vec![Criterion {
            id: "rule".into(),
            text: "Preserve the stable value.".into(),
            weight: 1.0,
            metric: None,
        }],
        judge_context: vec!["stable.txt".into()],
    };
    let task_file = t.path().join("task.json");
    files::save(&task_file, &task).unwrap();
    let task_hash = files::digest(&fs::read(&task_file).unwrap());
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_kb-eval-fixture"))
        .canonicalize()
        .unwrap();
    let agent = Agent {
        family: Family::Codex,
        program: fixture.clone(),
        binary_sha256: files::digest(&fs::read(fixture).unwrap()),
        version: "kb-eval synthetic fixture 1".into(),
        model: "synthetic-coder".into(),
        effort: None,
        timeout_seconds: 20,
        secret_env: vec![],
        usage_basis: Basis::Incremental,
        input_convention: Convention::InputIncludesCache,
    };
    let judge = Agent {
        model: "synthetic-judge".into(),
        ..agent.clone()
    };
    let study = Study {
        protocol: "kb.eval.study.v1".into(),
        id: "synthetic-protocol-check".into(),
        tasks: vec![TaskRef {
            id: task.id.clone(),
            file: task_file,
            sha256: task_hash,
        }],
        arms: vec![
            Arm {
                id: "none".into(),
                instructions: String::new(),
                overlays: vec![],
            },
            Arm {
                id: "candidate".into(),
                instructions: "Synthetic treatment marker.".into(),
                overlays: vec![],
            },
        ],
        blocks: vec![Block {
            id: "offline-fixture".into(),
            agent,
            judges: vec![judge],
        }],
        repetitions: 2,
        seed: 17,
        draws: 1000,
        alpha: 0.05,
        minimum_tasks: 2,
        metrics: [
            (
                "Q".into(),
                Metric {
                    direction: Direction::Higher,
                    sesoi: 5.0,
                },
            ),
            (
                "resolved".into(),
                Metric {
                    direction: Direction::Higher,
                    sesoi: 0.1,
                },
            ),
            (
                "cost_usd".into(),
                Metric {
                    direction: Direction::Lower,
                    sesoi: 0.1,
                },
            ),
            (
                "tokens".into(),
                Metric {
                    direction: Direction::Lower,
                    sesoi: 10.0,
                },
            ),
            (
                "rule_compliance".into(),
                Metric {
                    direction: Direction::Higher,
                    sesoi: 0.01,
                },
            ),
        ]
        .into_iter()
        .collect(),
        comparisons: vec![Comparison {
            id: "quality".into(),
            baseline: "none".into(),
            candidate: "candidate".into(),
            metric: "Q".into(),
        }],
        isolation: Isolation {
            read_roots: vec![],
            environment: BTreeMap::from([
                (
                    "KB_EVAL_FORBIDDEN".into(),
                    canary.to_string_lossy().into_owned(),
                ),
                (
                    "KB_EVAL_DENIED_PORT".into(),
                    denied.local_addr().unwrap().port().to_string(),
                ),
            ]),
            // Starts the egress proxy for the coder and judge; the fixture sends it only
            // an unapproved target, so no stage contacts the network.
            api_hosts: vec!["api.example.invalid".into()],
        },
        decision_rules: "Synthetic protocol smoke only; it cannot establish model quality.".into(),
    };
    let study_path = t.path().join("study.json");
    let registration = t.path().join("registration.json");
    files::save(&study_path, &study).unwrap();
    run(
        &home,
        &[
            "register",
            "--study",
            p(&study_path),
            "--output",
            p(&registration),
        ],
    );
    let preflight = run(
        &home,
        &[
            "preflight",
            "--registration",
            p(&registration),
            "--output",
            p(&t.path().join("preflight")),
        ],
    );
    assert!(
        preflight["pins"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["sandbox_startup"] == true)
    );
    assert_eq!(preflight["model_jobs_started"], 0);
    let qualification = t.path().join("qualification");
    let q = run(
        &home,
        &[
            "qualify",
            "--registration",
            p(&registration),
            "--task",
            "synthetic-fix",
            "--output",
            p(&qualification),
        ],
    );
    assert_eq!(q["accepted"], true);
    assert_eq!(q["base"]["hidden"], false);
    assert_eq!(q["golden"]["hidden"], true);
    // Deep enough that a socket under the stage directory would exceed `sun_path`.
    let trial_root = t
        .path()
        .join("trial-output-with-a-deliberately-long-name-beyond-the-unix-socket-path-limit");
    let trial = run(
        &home,
        &[
            "run",
            "--registration",
            p(&registration),
            "--task",
            "synthetic-fix",
            "--arm",
            "candidate",
            "--block",
            "offline-fixture",
            "--repetition",
            "0",
            "--qualification",
            p(&qualification.join("qualification.json")),
            "--output",
            p(&trial_root),
            "--execute",
        ],
    );
    assert_eq!(trial["objective_metrics"]["resolved"], 1.0);
    assert_eq!(trial["objective_metrics"]["tokens"], 33.0);
    assert!((trial["objective_metrics"]["cost_usd"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    assert_eq!(trial["changed_paths"], serde_json::json!(["value.txt"]));
    assert!(!trial_root.join("agent/work/hidden-expected.txt").exists());
    let judged = run(
        &home,
        &[
            "judge",
            "--registration",
            p(&registration),
            "--trial",
            p(&trial_root.join("trial.json")),
            "--judge",
            "0",
            "--execute",
        ],
    );
    let row: Observation = files::json(Path::new(judged["observation"].as_str().unwrap())).unwrap();
    assert_eq!(row.metrics["Q"], 100.0);
    assert_eq!(row.metrics["rule_compliance"], 1.0);
    let report = run(
        &home,
        &[
            "analyze",
            "--registration",
            p(&registration),
            "--observation",
            judged["observation"].as_str().unwrap(),
            "--output",
            p(&t.path().join("analysis.json")),
        ],
    );
    assert!(
        report["comparisons"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["decision"] == "insufficient-evidence")
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a functioning Seatbelt host; run explicitly in the isolation CI step"]
fn seatbelt_profile_loads_and_admits_only_the_proxy_port() {
    use std::net::TcpListener;
    let t = tempfile::tempdir().unwrap();
    let area = kb_eval::isolation::Area::create(&t.path().join("stage")).unwrap();
    fs::create_dir(&area.work).unwrap();
    let proxy = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let other = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let policy = Isolation {
        read_roots: vec![],
        environment: BTreeMap::new(),
        api_hosts: vec![],
    };
    let profile = t.path().join("stage.sb");
    let text = kb_eval::isolation::seatbelt(
        &area,
        &policy,
        &[],
        Some(proxy.local_addr().unwrap().port()),
    )
    .unwrap();
    fs::write(&profile, text).unwrap();
    let connect = |listener: &TcpListener| {
        Command::new("/usr/bin/sandbox-exec")
            .arg("-f")
            .arg(&profile)
            .args(["/usr/bin/nc", "-z", "-G", "5", "127.0.0.1"])
            .arg(listener.local_addr().unwrap().port().to_string())
            .output()
            .unwrap()
    };
    let allowed = connect(&proxy);
    assert!(
        allowed.status.success(),
        "Seatbelt rejected the profile or the proxy port: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    assert!(!connect(&other).status.success());
}
