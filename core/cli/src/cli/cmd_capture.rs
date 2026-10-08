use serde_json::json;

use super::Ctx;
use super::args::{CaptureArgs, CaptureKind};
use super::session::{self, Options, Session};
use crate::error::{KbError, Result};
use crate::git::Git;
use crate::knowledge::KnowledgeView;
use crate::model::{Anchor, AnchorKind, Record, Scope};
use crate::output::CommandOutput;
use crate::propose::DraftContext;
use crate::provenance::HostRoots;

pub fn run(ctx: &Ctx, args: &CaptureArgs) -> Result<CommandOutput> {
    let host = session::detect_host(ctx)?;
    let mut session = Session::open(ctx, host.clone(), Options::reading(ctx, false))?;
    let loc = session.loc.clone();
    let plan = session.with_view(|view| {
        let roots = crate::provenance::host_roots(host.as_ref(), view.registry(), args.hosts.repo.as_deref(), &args.hosts.repo_roots, &ctx.env.cwd)?;
        let mut anchors = Vec::new();
        for spec in &args.anchors { anchors.push(anchor(spec, AnchorKind::Source, &roots)?); }
        for spec in &args.test_anchors { anchors.push(anchor(spec, AnchorKind::Test, &roots)?); }
        let owner = match (&args.owner, view.registry().data.owners.as_slice()) {
            (Some(owner), _) => owner.clone(),
            (None, [only]) => only.id.clone(),
            _ => return Err(KbError::invalid_input("capture needs --owner when multiple owners are registered")),
        };
        let mut scope = Scope { product: args.product, modules: args.modules.clone(),
            features: args.features.clone(), change_types: args.change_types.clone(), ..Default::default() };
        if !scope.product {
            scope.repos.extend(args.hosts.repo.iter().cloned());
            if scope.modules.is_empty() && scope.features.is_empty() {
                for a in &anchors {
                    if let (Some(repo), Some(path)) = (&a.repo, &a.path) {
                        if args.hosts.repo.as_ref().is_some_and(|selected| selected != repo) { continue; }
                        if args.hosts.repo.is_none() { scope.repos.push(repo.clone()); }
                        scope.modules.extend(view.registry().modules_for_path(repo, path).into_iter().map(|m| m.id.clone()));
                    }
                }
                if scope.is_unconstrained() && let Some(host) = &host
                    && let Some((repo, _)) = crate::host::identify_repo(host, view.registry()) {
                    scope.repos.push(repo);
                }
            }
        }
        for values in [&mut scope.repos, &mut scope.modules, &mut scope.features, &mut scope.change_types] {
            values.sort(); values.dedup();
        }
        let mut value = json!({"schema": 2, "title": args.title, "status": "draft", "owner": owner,
            "scope": scope, "anchors": anchors});
        if let Some(introduced) = &args.introduced { value["introduced"] = json!(introduced); }
        match args.kind {
            CaptureKind::Decision => {
                if args.reasons.is_empty() { return Err(KbError::invalid_input("decision capture needs --reason; the engine does not invent rationale")); }
                let given = args.given.as_ref().ok_or_else(|| KbError::invalid_input("decision capture needs --given for the prior situation"))?;
                value["kind"] = json!("decision"); value["context"] = json!(given);
                value["decision"] = json!(args.text); value["reasons"] = json!(args.reasons);
            }
            CaptureKind::Gap => {
                value["kind"] = json!("gap"); value["gap"] = json!("missing");
                value["description"] = json!(args.text);
            }
            CaptureKind::Quirk => {
                value["kind"] = json!("reference"); value["summary"] = json!(args.text);
            }
            CaptureKind::Scenario => {
                let [feature] = args.features.as_slice() else { return Err(KbError::invalid_input("scenario capture needs exactly one --feature")); };
                let given = args.given.as_ref().ok_or_else(|| KbError::invalid_input("scenario capture needs --given"))?;
                let expect = args.expect.as_ref().ok_or_else(|| KbError::invalid_input("scenario capture needs --expect"))?;
                value["kind"] = json!("feature"); value["feature"] = json!(feature);
                value["summary"] = json!(args.text);
                value["behaviors"] = json!([{"id": "observed", "text": args.text}]);
                value["scenarios"] = json!([{"id": "captured", "given": given, "expect": expect}]);
            }
        }
        let mut body = String::new();
        if !args.tests.is_empty() {
            body.push_str("## Reported checks\n\nThese command strings were supplied by the caller. kb did not run them or infer success.\n\n");
            for command in &args.tests { body.push_str(&format!("- {}\n", serde_json::to_string(command)?)); }
        }
        let digest = crate::util::sha256_hex(crate::context::canonical_json(&json!({"record": value, "body": body})).as_bytes());
        let kind = value["kind"].as_str().unwrap();
        value["id"] = json!(args.id.clone().unwrap_or_else(|| format!("{}.{kind}.capture-{}", view.config().project.namespace, &digest[..12])));
        let record: Record = serde_json::from_value(value)
            .map_err(|e| KbError::invalid_input(format!("capture fields do not form a valid record: {e}")))?;
        let front = toml::to_string(&record).map_err(|e| KbError::invalid_input(format!("cannot serialize capture: {e}")))?;
        let text = format!("+++\n{front}+++\n{body}");
        crate::propose::prepare(&DraftContext { kb_root: &ctx.env.kb_root, loc: &loc, view, hosts: &roots }, &text)
    })?;
    super::cmd_propose::finish_draft(ctx, &session, &plan, args.apply)
}

fn anchor(spec: &str, kind: AnchorKind, roots: &HostRoots) -> Result<Anchor> {
    let (location, symbol) = spec
        .split_once('#')
        .map_or((spec, None), |(p, s)| (p, Some(s.to_string())));
    let (repo, location) = location
        .split_once(':')
        .ok_or_else(|| KbError::invalid_input("--anchor must be REPO:PATH[@REV][#SYMBOL]"))?;
    let root = roots
        .get(repo)
        .ok_or_else(|| KbError::invalid_input(format!("no host root for anchor repo {repo}")))?;
    let git = Git::new(root);
    let resolve = |revision: &str| -> Result<String> {
        git.resolve_commit(revision)?
            .ok_or_else(|| KbError::invalid_input(format!("anchor revision {revision} is missing")))
    };
    // Paths may contain '@' (icon@2x.png, @types/...). The text after the last '@' is a
    // revision when it resolves; otherwise the whole text is a path that must exist at HEAD.
    let (path, commit) = match location.rsplit_once('@') {
        None => (location, resolve("HEAD")?),
        Some((path, revision)) => match resolve(revision) {
            Ok(commit) => (path, commit),
            Err(missing) => {
                crate::util::check_rel_path(location).map_err(KbError::unsafe_path)?;
                let head = resolve("HEAD")?;
                if !git
                    .output(&["cat-file", "-e", &format!("{head}:{location}")])?
                    .ok()
                {
                    return Err(missing.with_hint(
                        "a PATH containing '@' must exist at HEAD or end with an explicit @REV",
                    ));
                }
                (location, head)
            }
        },
    };
    crate::util::check_rel_path(path).map_err(KbError::unsafe_path)?;
    Ok(Anchor {
        kind,
        repo: Some(repo.into()),
        path: Some(path.into()),
        symbol,
        commit: Some(commit),
        change: None,
        note: None,
        stamp: None,
    })
}
