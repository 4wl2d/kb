//! Paired, task-cluster inference. Repetitions never masquerade as independent tasks.
use crate::{
    Result, ensure,
    files::digest,
    model::{Direction, Observation, Registration},
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize)]
pub struct Estimate {
    pub tasks: usize,
    pub cohort: Vec<String>,
    pub cohort_sha256: String,
    pub baseline_mean: Option<f64>,
    pub candidate_mean: Option<f64>,
    /// Positive always means improvement, including lower-is-better metrics.
    pub benefit: Option<f64>,
    pub confidence_interval: Option<[f64; 2]>,
    /// Bonferroni interval across the registered comparison family, used for decisions.
    pub family_interval: Option<[f64; 2]>,
    pub sign_flip_p: Option<f64>,
    pub leave_one_task_out: Option<[f64; 2]>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComparisonReport {
    pub comparison: String,
    pub block: String,
    pub judge: String,
    pub metric: String,
    pub sesoi: f64,
    pub estimate: Estimate,
    pub without_deviations: Estimate,
    pub missing_cells: Vec<String>,
    pub holm_p: f64,
    pub decision: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Repeatability {
    pub block: String,
    pub judge: String,
    pub arm: String,
    pub metric: String,
    pub tasks: usize,
    pub mean_absolute_pair_difference: Option<f64>,
    pub binary_flip_fraction: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Analysis {
    pub protocol: &'static str,
    pub study_sha256: String,
    pub comparisons: Vec<ComparisonReport>,
    pub repeatability: Vec<Repeatability>,
    pub limitations: Vec<&'static str>,
}

struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn index(&mut self, n: usize) -> usize {
        // Rejection avoids modulo bias, including for non-power-of-two cluster counts.
        let bound = n as u64;
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let x = self.next();
            if x >= threshold {
                return (x % bound) as usize;
            }
        }
    }
}
fn average(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let at = q * (sorted.len() - 1) as f64;
    let lo = at.floor() as usize;
    let hi = at.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (at - lo as f64)
}
fn stream_seed(seed: u64, key: &str) -> u64 {
    u64::from_str_radix(&digest(key.as_bytes())[..16], 16).unwrap() ^ seed
}

/// Strong familywise correction, including unobserved preregistered hypotheses as p=1.
pub fn holm(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<_> = (0..values.len()).collect();
    order.sort_by(|a, b| values[*a].total_cmp(&values[*b]).then(a.cmp(b)));
    let mut out = vec![1.0; values.len()];
    let mut maximum: f64 = 0.0;
    for (rank, i) in order.into_iter().enumerate() {
        maximum = maximum.max(((values.len() - rank) as f64 * values[i]).min(1.0));
        out[i] = maximum;
    }
    out
}

fn estimate(
    pairs: &[(String, f64, f64)],
    direction: Direction,
    seed: u64,
    draws: usize,
    alpha: f64,
    family_size: usize,
) -> Estimate {
    let cohort: Vec<_> = pairs.iter().map(|p| p.0.clone()).collect();
    let mut out = Estimate {
        tasks: pairs.len(),
        cohort_sha256: digest(cohort.join("\n").as_bytes()),
        cohort,
        baseline_mean: None,
        candidate_mean: None,
        benefit: None,
        confidence_interval: None,
        family_interval: None,
        sign_flip_p: None,
        leave_one_task_out: None,
    };
    if pairs.is_empty() {
        return out;
    }
    let sign = if direction == Direction::Higher {
        1.0
    } else {
        -1.0
    };
    let diffs: Vec<_> = pairs.iter().map(|p| (p.2 - p.1) * sign).collect();
    let mean = average(&diffs);
    out.baseline_mean = Some(pairs.iter().map(|p| p.1).sum::<f64>() / pairs.len() as f64);
    out.candidate_mean = Some(pairs.iter().map(|p| p.2).sum::<f64>() / pairs.len() as f64);
    out.benefit = Some(mean);
    if pairs.len() < 2 {
        return out;
    }
    let mut rng = Random(seed);
    let mut bootstrap = Vec::with_capacity(draws);
    for _ in 0..draws {
        bootstrap.push(
            (0..diffs.len())
                .map(|_| diffs[rng.index(diffs.len())])
                .sum::<f64>()
                / diffs.len() as f64,
        );
    }
    bootstrap.sort_by(f64::total_cmp);
    out.confidence_interval = Some([
        percentile(&bootstrap, alpha / 2.0),
        percentile(&bootstrap, 1.0 - alpha / 2.0),
    ]);
    let family_alpha = alpha / family_size.max(1) as f64;
    out.family_interval = Some([
        percentile(&bootstrap, family_alpha / 2.0),
        percentile(&bootstrap, 1.0 - family_alpha / 2.0),
    ]);
    let observed = diffs.iter().sum::<f64>().abs();
    let tolerance = 1e-12 * observed.max(1.0);
    let p = if diffs.len() <= 16 {
        let count = 1_usize << diffs.len();
        let extreme = (0..count)
            .filter(|mask| {
                let value = diffs
                    .iter()
                    .enumerate()
                    .map(|(i, d)| if mask & (1 << i) == 0 { *d } else { -*d })
                    .sum::<f64>();
                value.abs() + tolerance >= observed
            })
            .count();
        extreme as f64 / count as f64
    } else {
        let extreme = (0..draws)
            .filter(|_| {
                let value = diffs
                    .iter()
                    .map(|d| if rng.next() & 1 == 0 { *d } else { -*d })
                    .sum::<f64>();
                value.abs() + tolerance >= observed
            })
            .count();
        (extreme + 1) as f64 / (draws + 1) as f64
    };
    out.sign_flip_p = Some(p);
    let total = diffs.iter().sum::<f64>();
    let leave: Vec<_> = diffs
        .iter()
        .map(|d| (total - d) / (diffs.len() - 1) as f64)
        .collect();
    out.leave_one_task_out = Some([
        leave.iter().copied().fold(f64::INFINITY, f64::min),
        leave.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ]);
    out
}

pub fn analyze(reg: &Registration, observations: &[Observation]) -> Result<Analysis> {
    reg.study.validate()?;
    ensure(
        reg.sha256 == digest(&serde_json::to_vec(&reg.study)?),
        "registration digest mismatch",
    )?;
    let study = &reg.study;
    let mut rows = BTreeMap::new();
    for o in observations {
        ensure(
            o.study_sha256 == reg.sha256,
            "observation belongs to a different registration",
        )?;
        crate::model::hash(&o.evidence_sha256)?;
        ensure(
            study.tasks.iter().any(|t| t.id == o.task)
                && study.arms.iter().any(|a| a.id == o.arm)
                && study.blocks.iter().any(|b| b.id == o.block)
                && o.repetition < study.repetitions,
            "unplanned task/arm/block/repetition",
        )?;
        let block = study.blocks.iter().find(|b| b.id == o.block).unwrap();
        ensure(
            o.judge == "objective"
                || block
                    .judges
                    .iter()
                    .any(|a| crate::adapters::identity(a) == o.judge),
            "unregistered judge identity",
        )?;
        for (name, value) in &o.metrics {
            ensure(
                study.metrics.contains_key(name) && value.is_finite(),
                "unknown metric or nonfinite value",
            )?;
            if name == "resolved" {
                ensure(
                    *value == 0.0 || *value == 1.0,
                    "resolved must be a binary executable outcome",
                )?;
            }
        }
        let key = (
            o.block.as_str(),
            o.judge.as_str(),
            o.task.as_str(),
            o.arm.as_str(),
            o.repetition,
        );
        ensure(rows.insert(key, o).is_none(), "duplicate observation cell")?;
    }
    let mut comparisons = Vec::new();
    let mut repeats = Vec::new();
    for block in &study.blocks {
        let judges: BTreeSet<_> = std::iter::once("objective".to_string())
            .chain(block.judges.iter().map(crate::adapters::identity))
            .collect();
        for judge in judges {
            for c in &study.comparisons {
                let metric = &study.metrics[&c.metric];
                let mut pairs = Vec::new();
                let mut clean = Vec::new();
                let mut missing = Vec::new();
                for task in &study.tasks {
                    let mut left = Vec::new();
                    let mut right = Vec::new();
                    let mut deviated = false;
                    for r in 0..study.repetitions {
                        for (arm, dst) in [(&c.baseline, &mut left), (&c.candidate, &mut right)] {
                            let row = rows.get(&(
                                block.id.as_str(),
                                judge.as_str(),
                                task.id.as_str(),
                                arm.as_str(),
                                r,
                            ));
                            if let Some(v) = row.and_then(|o| o.metrics.get(&c.metric)) {
                                dst.push(*v);
                                deviated |= !row.unwrap().deviations.is_empty();
                            } else {
                                missing.push(format!("{}/{arm}/{r}/{}", task.id, c.metric));
                            }
                        }
                    }
                    if left.len() == study.repetitions as usize
                        && right.len() == study.repetitions as usize
                    {
                        let pair = (task.id.clone(), average(&left), average(&right));
                        if !deviated {
                            clean.push(pair.clone());
                        }
                        pairs.push(pair);
                    }
                }
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                clean.sort_by(|a, b| a.0.cmp(&b.0));
                missing.sort();
                let seed = stream_seed(study.seed, &format!("{}/{}/{}", block.id, judge, c.id));
                comparisons.push(ComparisonReport {
                    comparison: c.id.clone(),
                    block: block.id.clone(),
                    judge: judge.clone(),
                    metric: c.metric.clone(),
                    sesoi: metric.sesoi,
                    estimate: estimate(
                        &pairs,
                        metric.direction,
                        seed,
                        study.draws,
                        study.alpha,
                        study.comparisons.len(),
                    ),
                    without_deviations: estimate(
                        &clean,
                        metric.direction,
                        seed,
                        study.draws,
                        study.alpha,
                        study.comparisons.len(),
                    ),
                    missing_cells: missing,
                    holm_p: 1.0,
                    decision: "insufficient-evidence".into(),
                });
            }
            for arm in &study.arms {
                for metric in study.metrics.keys() {
                    let mut differences = Vec::new();
                    let mut flips = Vec::new();
                    let mut tasks = 0;
                    for task in &study.tasks {
                        let values: Vec<_> = (0..study.repetitions)
                            .filter_map(|r| {
                                rows.get(&(
                                    block.id.as_str(),
                                    judge.as_str(),
                                    task.id.as_str(),
                                    arm.id.as_str(),
                                    r,
                                ))
                                .and_then(|o| o.metrics.get(metric))
                                .copied()
                            })
                            .collect();
                        if values.len() != study.repetitions as usize || values.len() < 2 {
                            continue;
                        }
                        tasks += 1;
                        let mut ds = Vec::new();
                        let mut fs = Vec::new();
                        for i in 0..values.len() {
                            for j in i + 1..values.len() {
                                ds.push((values[i] - values[j]).abs());
                                if metric == "resolved" {
                                    fs.push(f64::from(values[i] != values[j]));
                                }
                            }
                        }
                        differences.push(average(&ds));
                        if !fs.is_empty() {
                            flips.push(average(&fs));
                        }
                    }
                    repeats.push(Repeatability {
                        block: block.id.clone(),
                        judge: judge.clone(),
                        arm: arm.id.clone(),
                        metric: metric.clone(),
                        tasks,
                        mean_absolute_pair_difference: (!differences.is_empty())
                            .then(|| average(&differences)),
                        binary_flip_fraction: (!flips.is_empty()).then(|| average(&flips)),
                    });
                }
            }
        }
    }
    // Different harness/model/judge blocks are independent registered families; do not pool them.
    let families: BTreeSet<_> = comparisons
        .iter()
        .map(|c| (c.block.clone(), c.judge.clone()))
        .collect();
    for family in families {
        let indexes: Vec<_> = comparisons
            .iter()
            .enumerate()
            .filter(|(_, c)| (&c.block, &c.judge) == (&family.0, &family.1))
            .map(|(i, _)| i)
            .collect();
        let p: Vec<_> = indexes
            .iter()
            .map(|i| comparisons[*i].estimate.sign_flip_p.unwrap_or(1.0))
            .collect();
        for (i, p) in indexes.into_iter().zip(holm(&p)) {
            let c = &mut comparisons[i];
            c.holm_p = p;
            if c.estimate.tasks < study.minimum_tasks || !c.missing_cells.is_empty() {
                continue;
            }
            if let Some([lo, hi]) = c.estimate.family_interval {
                c.decision = if lo > c.sesoi && p < study.alpha {
                    "superior-beyond-sesoi"
                } else if hi < -c.sesoi && p < study.alpha {
                    "inferior-beyond-sesoi"
                } else if lo > -c.sesoi && hi < c.sesoi {
                    "equivalent-within-sesoi"
                } else {
                    "inconclusive"
                }
                .into();
            }
        }
    }
    comparisons.sort_by(|a, b| {
        (&a.block, &a.judge, &a.comparison).cmp(&(&b.block, &b.judge, &b.comparison))
    });
    repeats.sort_by(|a, b| {
        (&a.block, &a.judge, &a.arm, &a.metric).cmp(&(&b.block, &b.judge, &b.arm, &b.metric))
    });
    Ok(Analysis {
        protocol: "kb.eval.analysis.v1",
        study_sha256: reg.sha256.clone(),
        comparisons,
        repeatability: repeats,
        limitations: vec![
            "Percentile intervals resample whole tasks after averaging paired repetitions. confidence_interval is marginal; family_interval uses Bonferroni alpha/family-size and governs all SESOI decisions. Bootstrap coverage remains an approximation, especially with few tasks or sparse tails.",
            "Two-sided sign-flip p-values assume exchangeability/symmetry of task effects under the null. Holm is applied within each preregistered block/judge comparison family, including missing hypotheses.",
            "Every reported arm mean uses the identical paired task/repetition cohort. Compare cost and quality conclusions only when cohort hashes match; missing planned cells prevent acceptance decisions.",
            "Equivalence requires the entire confidence interval inside the preregistered SESOI. A nonsignificant superiority test does not establish equivalence.",
            "No automatic best-on-all-criteria claim: review guardrails, exclusions, leave-one-task-out sensitivity, second-judge agreement and repeatability separately.",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn fixture() -> (Registration, Vec<Observation>) {
        let a = Agent {
            family: Family::Codex,
            program: "/synthetic/agent".into(),
            binary_sha256: "0".repeat(64),
            version: "fixture-1".into(),
            model: "fixture-model".into(),
            effort: None,
            timeout_seconds: 10,
            secret_env: vec![],
            usage_basis: crate::accounting::Basis::Incremental,
            input_convention: crate::accounting::Convention::InputIncludesCache,
        };
        let judge = crate::adapters::identity(&a);
        let study = Study {
            protocol: "kb.eval.study.v1".into(),
            id: "synthetic-statistics".into(),
            tasks: (0..8)
                .map(|i| TaskRef {
                    id: format!("task-{i}"),
                    file: format!("/synthetic/task-{i}.json").into(),
                    sha256: "0".repeat(64),
                })
                .collect(),
            arms: ["baseline", "candidate"]
                .into_iter()
                .map(|id| Arm {
                    id: id.into(),
                    instructions: String::new(),
                    overlays: vec![],
                })
                .collect(),
            blocks: vec![Block {
                id: "block".into(),
                agent: a.clone(),
                judges: vec![a],
            }],
            repetitions: 2,
            seed: 7,
            draws: 2000,
            alpha: 0.05,
            minimum_tasks: 8,
            metrics: [
                (
                    "Q".into(),
                    Metric {
                        direction: Direction::Higher,
                        sesoi: 5.0,
                    },
                ),
                (
                    "cost_usd".into(),
                    Metric {
                        direction: Direction::Lower,
                        sesoi: 0.1,
                    },
                ),
            ]
            .into_iter()
            .collect(),
            comparisons: [("quality", "Q"), ("cost", "cost_usd")]
                .into_iter()
                .map(|(id, metric)| Comparison {
                    id: id.into(),
                    baseline: "baseline".into(),
                    candidate: "candidate".into(),
                    metric: metric.into(),
                })
                .collect(),
            isolation: Isolation {
                read_roots: vec![],
                environment: Default::default(),
                api_hosts: vec![],
            },
            decision_rules: "Synthetic unit fixture; no empirical quality claim.".into(),
        };
        let reg = Registration {
            protocol: "kb.eval.registration.v1".into(),
            sha256: digest(&serde_json::to_vec(&study).unwrap()),
            study,
        };
        let mut rows = Vec::new();
        for i in 0..8 {
            for arm in ["baseline", "candidate"] {
                for repetition in 0..2 {
                    let mut metrics = BTreeMap::from([
                        (
                            "Q".into(),
                            40.0 + f64::from(repetition) * 2.0
                                + if arm == "candidate" { 10.0 } else { 0.0 },
                        ),
                        (
                            "cost_usd".into(),
                            if arm == "candidate" { 0.5 } else { 1.0 },
                        ),
                    ]);
                    if i == 7 && arm == "candidate" {
                        metrics.remove("cost_usd");
                    }
                    rows.push(Observation {
                        study_sha256: reg.sha256.clone(),
                        task: format!("task-{i}"),
                        arm: arm.into(),
                        block: "block".into(),
                        repetition,
                        judge: judge.clone(),
                        metrics,
                        deviations: if i == 0 {
                            vec!["synthetic deviation".into()]
                        } else {
                            vec![]
                        },
                        evidence_sha256: "1".repeat(64),
                    });
                }
            }
        }
        (reg, rows)
    }
    #[test]
    fn analysis_pairs_cells_preserves_repetitions_and_never_changes_cohort_for_cost() {
        let (reg, mut rows) = fixture();
        let report = analyze(&reg, &rows).unwrap();
        let quality = report
            .comparisons
            .iter()
            .find(|c| c.comparison == "quality" && c.judge != "objective")
            .unwrap();
        let cost = report
            .comparisons
            .iter()
            .find(|c| c.comparison == "cost" && c.judge != "objective")
            .unwrap();
        assert_eq!(quality.estimate.tasks, 8);
        assert_eq!(quality.estimate.benefit, Some(10.0));
        assert_eq!(quality.decision, "superior-beyond-sesoi");
        assert_eq!(quality.without_deviations.tasks, 7);
        assert_eq!(cost.estimate.tasks, 7);
        assert_eq!(cost.missing_cells.len(), 2);
        assert_eq!(cost.decision, "insufficient-evidence");
        assert_ne!(quality.estimate.cohort_sha256, cost.estimate.cohort_sha256);
        assert_eq!(
            report
                .repeatability
                .iter()
                .find(|r| r.judge != "objective" && r.metric == "Q")
                .unwrap()
                .mean_absolute_pair_difference,
            Some(2.0)
        );
        rows.reverse();
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::to_value(analyze(&reg, &rows).unwrap()).unwrap()
        );
        rows.push(rows[0].clone());
        assert!(analyze(&reg, &rows).is_err());
    }
    #[test]
    fn equivalence_requires_its_interval_inside_the_preregistered_margin() {
        let (reg, mut rows) = fixture();
        for row in &mut rows {
            if row.arm == "candidate" {
                *row.metrics.get_mut("Q").unwrap() -= 8.0;
            }
        }
        let report = analyze(&reg, &rows).unwrap();
        let q = report
            .comparisons
            .iter()
            .find(|c| c.judge != "objective" && c.comparison == "quality")
            .unwrap();
        assert_eq!(q.decision, "equivalent-within-sesoi");
        for row in &mut rows {
            if row.arm == "candidate" && row.task == "task-0" {
                *row.metrics.get_mut("Q").unwrap() += 80.0;
            }
        }
        let report = analyze(&reg, &rows).unwrap();
        let q = report
            .comparisons
            .iter()
            .find(|c| c.judge != "objective" && c.comparison == "quality")
            .unwrap();
        assert_eq!(q.decision, "inconclusive");
    }
    #[test]
    fn known_holm_family_and_missing_hypothesis() {
        assert_eq!(holm(&[0.01, 0.04, 0.03, 1.0]), vec![0.04, 0.09, 0.09, 1.0]);
    }
    #[test]
    fn repeated_tasks_are_clusters_and_sign_flip_is_exact() {
        let pairs: Vec<_> = (0..8).map(|i| (format!("t{i}"), 40.0, 50.0)).collect();
        let e = estimate(&pairs, Direction::Higher, 7, 2000, 0.05, 1);
        assert_eq!(e.confidence_interval, Some([10.0, 10.0]));
        assert_eq!(e.sign_flip_p, Some(2.0 / 256.0));
        assert_eq!(e.leave_one_task_out, Some([10.0, 10.0]));
        let lower = estimate(&pairs, Direction::Lower, 7, 2000, 0.05, 1);
        assert_eq!(lower.benefit, Some(-10.0));
    }
    #[test]
    fn cluster_bootstrap_is_deterministic_and_detects_a_dominating_task() {
        let pairs: Vec<_> = (0..8)
            .map(|i| (format!("t{i}"), 0.0, if i == 7 { 40.0 } else { 0.0 }))
            .collect();
        let a = estimate(&pairs, Direction::Higher, 9, 2000, 0.05, 8);
        let b = estimate(&pairs, Direction::Higher, 9, 2000, 0.05, 8);
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert_eq!(a.leave_one_task_out.unwrap()[0], 0.0);
        assert!(a.confidence_interval.unwrap()[0] <= 0.0);
        assert!(a.family_interval.unwrap()[1] >= a.confidence_interval.unwrap()[1]);
    }
}
