//! Routing fixtures (`<profile>/routing-tests/*.toml`): golden context requests with
//! expected mandatory ids, forbidden irrelevant ids, expected status, ambiguities and budget
//! outcomes.
//!
//! Cases are hermetic: host detection is not used (only the snapshot of the given
//! environment), and `expect_status` is compared with [`ContextResult::knowledge_status`],
//! which ignores snapshot provenance (freshness, approval, working tree) so that the same
//! fixtures pass offline and in CI. `forbid` means "not included in any tier".
//!
//! [`ContextResult::knowledge_status`]: super::ContextResult::knowledge_status

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::diag::Diagnostic;
use crate::error::{ErrorCode, KbError, Result};
use crate::knowledge::KnowledgeView;
use crate::model::ProfileLocation;
use crate::model::routing::{RoutingCase, RoutingTestFile};
use crate::normalize;
use crate::output::Format;
use crate::source::SourceTree;

use super::present::safe_text;
use super::{Completeness, ContextRequest, TaskEnv, assemble, parse_host_version};

/// Routing cases as (fixture file path, case), plus load diagnostics.
pub type LoadedCases = (Vec<(String, RoutingCase)>, Vec<Diagnostic>);

/// Load and strictly parse all routing fixtures of a profile. Invalid files and cases are
/// reported as error diagnostics and skipped.
pub fn load_cases(source: &dyn SourceTree, loc: &ProfileLocation) -> Result<LoadedCases> {
    let (entries, issues) = source.list(&[loc.routing_tests_dir()])?;
    let mut diags: Vec<Diagnostic> = issues
        .into_iter()
        .map(|i| Diagnostic::error(i.code, i.message).at_path(i.path))
        .collect();
    let mut files = Vec::new();
    for e in entries {
        let name = e.path.rsplit('/').next().unwrap_or("");
        if e.path.ends_with(".toml") {
            files.push(e);
        } else if !name.eq_ignore_ascii_case("README.md") && name != ".gitkeep" {
            diags.push(
                Diagnostic::warning(
                    "NON_ROUTING_FILE",
                    "ignored: routing fixtures are `*.toml` files",
                )
                .at_path(e.path.clone()),
            );
        }
    }
    let contents = source.read(&files)?;
    let mut cases = Vec::new();
    for (f, bytes) in files.iter().zip(contents) {
        let err =
            |msg: String| Diagnostic::error("ROUTING_TEST_INVALID", msg).at_path(f.path.clone());
        let Ok(text) = std::str::from_utf8(&bytes) else {
            diags.push(err("not UTF-8".into()));
            continue;
        };
        let file: RoutingTestFile = match toml::from_str(text) {
            Ok(x) => x,
            Err(e) => {
                diags.push(err(e.to_string()));
                continue;
            }
        };
        if file.schema != 1 {
            diags.push(
                Diagnostic::error(
                    "UNSUPPORTED_SCHEMA_VERSION",
                    format!("routing fixture schema {} is not supported", file.schema),
                )
                .at_path(f.path.clone()),
            );
            continue;
        }
        let mut names = BTreeSet::new();
        for case in file.case {
            let mut problems = Vec::new();
            if let Err(e) = validate_metrics(&case) {
                problems.push(e);
            }
            if case.name.trim().is_empty() {
                problems.push("case name is empty".to_string());
            }
            if !names.insert(case.name.clone()) {
                problems.push("duplicate case name".to_string());
            }
            for hv in &case.host_versions {
                if let Err(e) = parse_host_version(hv) {
                    problems.push(e.message);
                }
            }
            if let Some(s) = &case.expect_status
                && Completeness::parse(s).is_none()
            {
                problems.push(format!(
                    "expect_status `{s}` is not one of complete, partial, conflict, incomplete"
                ));
            }
            if !problems.is_empty() {
                for p in problems {
                    diags.push(err(format!("case `{}`: {p}", case.name)));
                }
                continue;
            }
            let has_expectation = !case.expect_mandatory.is_empty()
                || !case.expect_included.is_empty()
                || !case.forbid.is_empty()
                || case.expect_status.is_some()
                || !case.expect_ambiguous.is_empty()
                || !case.expect_order.is_empty()
                || case.max_tokens.is_some()
                || case.min_recall_percent.is_some()
                || case.expect_budget_exceeded;
            if !has_expectation {
                diags.push(
                    Diagnostic::warning(
                        "ROUTING_CASE_EMPTY",
                        format!("case `{}` has no expectations", case.name),
                    )
                    .at_path(f.path.clone()),
                );
            }
            cases.push((f.path.clone(), case));
        }
    }
    Ok((cases, diags))
}

/// Outcome of one routing case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOutcome {
    pub file: String,
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    /// Knowledge status of the assembled context (absent when assembly failed).
    pub status: Option<Completeness>,
    /// Mandatory-tier ids (mandatory + dependencies).
    pub mandatory: Vec<String>,
    /// Record ids included in any tier.
    pub included: Vec<String>,
    pub delivery_order: Vec<String>,
    pub tokens_est: Option<u64>,
    pub recall_at_k: Option<Ratio>,
    pub recall_k: usize,
    pub mandatory_precision: Option<Ratio>,
    pub mandatory_not_applicable: Vec<String>,
}

/// Exact counts are primary; JSON also exposes their ratio for report consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ratio {
    pub matched: usize,
    pub total: usize,
}

impl Ratio {
    pub fn value(&self) -> Value {
        json!({"matched": self.matched, "total": self.total,
            "fraction": (self.total > 0).then(|| self.matched as f64 / self.total as f64)})
    }
}

fn validate_metrics(case: &RoutingCase) -> std::result::Result<(), String> {
    for (name, ids) in [
        ("expect_order", &case.expect_order),
        ("relevant", &case.relevant),
        ("not_applicable", &case.not_applicable),
    ] {
        if ids.iter().any(|id| id.is_empty())
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        {
            return Err(format!("{name} must contain distinct nonempty record ids"));
        }
    }
    if case.recall_k.is_some_and(|k| k == 0 || k > 10000)
        || case.max_tokens == Some(0)
        || case
            .min_recall_percent
            .is_some_and(|p| p > 100 || case.relevant.is_empty())
    {
        return Err("metrics require recall_k in 1..10000, positive max_tokens and min_recall_percent in 0..100 with relevant labels".into());
    }
    if case
        .relevant
        .iter()
        .chain(&case.expect_mandatory)
        .any(|id| case.not_applicable.contains(id))
    {
        return Err("relevant/mandatory labels contradict not_applicable".into());
    }
    Ok(())
}

/// Outcome of all routing cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingReport {
    pub cases: Vec<CaseOutcome>,
    pub context_format: Format,
}

impl Default for RoutingReport {
    fn default() -> Self {
        Self {
            cases: Vec::new(),
            context_format: Format::Compact,
        }
    }
}

impl RoutingReport {
    pub fn passed(&self) -> usize {
        self.cases.iter().filter(|c| c.passed).count()
    }

    pub fn failed(&self) -> usize {
        self.cases.len() - self.passed()
    }

    pub fn ok(&self) -> bool {
        self.failed() == 0
    }

    /// `ROUTING_TESTS_FAILED` when any case failed.
    pub fn failure(&self) -> Option<KbError> {
        if self.ok() {
            return None;
        }
        let failed: Vec<String> = self
            .cases
            .iter()
            .filter(|c| !c.passed)
            .map(|c| format!("{}#{}", c.file, c.name))
            .collect();
        Some(
            KbError::new(
                ErrorCode::RoutingTestsFailed,
                format!("{} routing case(s) failed", failed.len()),
            )
            .with_details(json!({"failed": failed})),
        )
    }
}

/// Run routing cases against a view. Only `env.snapshot` is used (cases are hermetic).
pub fn run_cases(
    view: &dyn KnowledgeView,
    cases: &[(String, RoutingCase)],
    env: &TaskEnv,
) -> RoutingReport {
    run_cases_in_format(view, cases, env, Format::Compact)
}

/// Compare delivery renderers against exactly the same fixtures and expectations.
pub fn run_cases_in_format(
    view: &dyn KnowledgeView,
    cases: &[(String, RoutingCase)],
    env: &TaskEnv,
    context_format: Format,
) -> RoutingReport {
    let env = TaskEnv::new(env.snapshot.clone());
    RoutingReport {
        context_format,
        cases: cases
            .iter()
            .map(|(file, case)| run_case(view, file, case, &env, context_format))
            .collect(),
    }
}

fn request(case: &RoutingCase) -> std::result::Result<ContextRequest, String> {
    let mut req = ContextRequest::new(case.intent);
    req.task = case.task.clone();
    req.repos = case.repos.clone();
    req.paths = case.paths.clone();
    req.modules = case.modules.clone();
    req.features = case.features.clone();
    req.concepts = case.concepts.clone();
    req.change_types = case.change_types.clone();
    req.budget = case.budget;
    req.budget_unit = case.budget_unit;
    req.host_versions = case
        .host_versions
        .iter()
        .map(|s| parse_host_version(s).map_err(|e| e.message))
        .collect::<std::result::Result<_, _>>()?;
    Ok(req)
}

fn run_case(
    view: &dyn KnowledgeView,
    file: &str,
    case: &RoutingCase,
    env: &TaskEnv,
    context_format: Format,
) -> CaseOutcome {
    let mut out = CaseOutcome {
        file: file.to_string(),
        name: case.name.clone(),
        passed: false,
        failures: Vec::new(),
        status: None,
        mandatory: Vec::new(),
        included: Vec::new(),
        delivery_order: Vec::new(),
        tokens_est: None,
        recall_at_k: None,
        recall_k: case.recall_k.unwrap_or(10),
        mandatory_precision: None,
        mandatory_not_applicable: Vec::new(),
    };
    if let Err(e) = validate_metrics(case) {
        out.failures.push(e);
        return out;
    }
    let req = match request(case) {
        Ok(r) => r,
        Err(e) => {
            out.failures.push(e);
            return out;
        }
    };
    match assemble(&req, env, view, context_format) {
        Err(e) if e.code == ErrorCode::ContextBudgetExceeded => {
            if !case.expect_budget_exceeded {
                out.failures.push(format!("unexpected {e}"));
            }
        }
        Err(e) => out.failures.push(format!("context assembly failed: {e}")),
        Ok(res) => {
            if case.expect_budget_exceeded {
                out.failures
                    .push("expected CONTEXT_BUDGET_EXCEEDED, but the context fit".into());
            }
            let mandatory: BTreeSet<String> = res
                .mandatory_ids()
                .into_iter()
                .map(str::to_string)
                .collect();
            let included: BTreeSet<String> =
                res.units.iter().map(|u| u.record_id.clone()).collect();
            let mut seen = BTreeSet::new();
            out.delivery_order = res
                .units
                .iter()
                .filter(|u| seen.insert(u.record_id.clone()))
                .map(|u| u.record_id.clone())
                .collect();
            let tokens = super::estimate_tokens(&super::render(&res, context_format, false));
            out.tokens_est = Some(tokens);
            if case.max_tokens.is_some_and(|max| tokens > max) {
                out.failures.push(format!(
                    "response uses {tokens} estimated tokens, exceeding max_tokens {}",
                    case.max_tokens.unwrap_or_default()
                ));
            }
            let positions: Option<Vec<_>> = case
                .expect_order
                .iter()
                .map(|id| out.delivery_order.iter().position(|found| found == id))
                .collect();
            if positions
                .as_ref()
                .is_none_or(|p| p.windows(2).any(|w| w[0] >= w[1]))
            {
                out.failures.push(format!(
                    "expected relative delivery order: {}",
                    case.expect_order.join(", ")
                ));
            }
            if !case.relevant.is_empty() {
                let top: BTreeSet<_> = out.delivery_order.iter().take(out.recall_k).collect();
                let matched = case.relevant.iter().filter(|id| top.contains(id)).count();
                let total = case.relevant.len();
                if case
                    .min_recall_percent
                    .is_some_and(|min| matched * 100 < usize::from(min) * total)
                {
                    out.failures.push(format!(
                        "recall@{} is {matched}/{total}, below {}%",
                        out.recall_k,
                        case.min_recall_percent.unwrap_or_default()
                    ));
                }
                out.recall_at_k = Some(Ratio { matched, total });
            }
            out.mandatory_not_applicable = case
                .not_applicable
                .iter()
                .filter(|id| mandatory.contains(*id))
                .cloned()
                .collect();
            if case.applicability_reviewed {
                out.mandatory_precision = Some(Ratio {
                    matched: mandatory.len() - out.mandatory_not_applicable.len(),
                    total: mandatory.len(),
                });
            }
            for id in &case.expect_mandatory {
                if !mandatory.contains(id) {
                    out.failures
                        .push(format!("expected `{id}` in the mandatory tier"));
                }
            }
            for id in &case.expect_included {
                if !included.contains(id) {
                    out.failures.push(format!("expected `{id}` to be included"));
                }
            }
            for id in &case.forbid {
                if included.contains(id) {
                    out.failures.push(format!("forbidden `{id}` was included"));
                }
            }
            let status = res.knowledge_status();
            if let Some(s) = &case.expect_status
                && status.as_str() != s
            {
                out.failures
                    .push(format!("expected status {s}, got {}", status.as_str()));
            }
            for phrase in &case.expect_ambiguous {
                let norm = normalize::normalize(phrase);
                let found = res
                    .header
                    .scope
                    .ambiguities
                    .iter()
                    .any(|a| a.phrase == norm || a.aliases.iter().any(|k| k == phrase.trim()));
                if !found {
                    out.failures
                        .push(format!("expected `{phrase}` to be reported as ambiguous"));
                }
            }
            out.status = Some(status);
            out.mandatory = mandatory.into_iter().collect();
            out.included = included.into_iter().collect();
        }
    }
    out.passed = out.failures.is_empty();
    out
}

/// Render compact or human text (JSON: pretty-printed [`report_to_json`]).
pub fn render_report(r: &RoutingReport, format: Format) -> String {
    if format == Format::Json {
        let mut s = serde_json::to_string_pretty(&report_to_json(r)).unwrap_or_default();
        s.push('\n');
        return s;
    }
    let mut o = format!(
        "routing tests: {} passed, {} failed\n",
        r.passed(),
        r.failed()
    );
    for c in &r.cases {
        let mark = if c.passed { "ok  " } else { "FAIL" };
        let status = c.status.map(|s| s.as_str()).unwrap_or("-");
        o.push_str(&format!("{mark} {}: {} [{status}]\n", c.file, c.name));
        for f in &c.failures {
            o.push_str(&format!("  - {f}\n"));
        }
        if format == Format::Human && !c.passed {
            o.push_str(&format!("  mandatory: {}\n", c.mandatory.join(", ")));
            o.push_str(&format!("  included: {}\n", c.included.join(", ")));
        }
    }
    safe_text(o)
}

/// Deterministic JSON report.
pub fn report_to_json(r: &RoutingReport) -> Value {
    let tokens: Vec<_> = r.cases.iter().filter_map(|c| c.tokens_est).collect();
    let covered: BTreeSet<_> = r.cases.iter().flat_map(|c| &c.mandatory).collect();
    json!({
        "passed": r.passed(),
        "failed": r.failed(),
        "context_format": r.context_format,
        "metrics": {"tokens_est": {"calls": tokens.len(), "total": tokens.iter().sum::<u64>(), "max": tokens.iter().max()},
            "mandatory_records_exercised": covered, "recall_labeled_cases": r.cases.iter().filter(|c| c.recall_at_k.is_some()).count(),
            "applicability_reviewed_cases": r.cases.iter().filter(|c| c.mandatory_precision.is_some()).count(),
            "note": "Estimated response tokens in context_format; recall uses distinct records in delivery order. Precision exists only for explicitly adjudicated cases; neither metric proves semantic correctness."},
        "cases": r.cases.iter().map(|c| json!({
            "file": c.file,
            "name": c.name,
            "passed": c.passed,
            "status": c.status.map(|s| s.as_str()),
            "failures": c.failures,
            "mandatory": c.mandatory,
            "included": c.included,
            "delivery_order": c.delivery_order,
            "tokens_est": c.tokens_est,
            "recall_at_k": c.recall_at_k.as_ref().map(Ratio::value),
            "recall_k": c.recall_k,
            "mandatory_precision": c.mandatory_precision.as_ref().map(Ratio::value),
            "mandatory_not_applicable": c.mandatory_not_applicable,
        })).collect::<Vec<_>>(),
    })
}
