//! Native log accounting. A reconnect is a segment, never a replacement for earlier usage.
use crate::{Result, ensure, model::Family};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Basis {
    Incremental,
    SessionCumulative,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Convention {
    InputIncludesCache,
    InputExcludesCache,
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Totals {
    pub native_input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Segment {
    pub id: String,
    pub session: String,
    pub sequence: u32,
    pub basis: Basis,
    pub log_sha256: String,
    pub complete: bool,
    pub terminal_events: usize,
    pub usage: Totals,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Accounting {
    pub complete: bool,
    pub segments: usize,
    pub usage: Totals,
    pub total_tokens: Option<u64>,
    pub issues: Vec<String>,
}

impl Totals {
    fn add(&mut self, other: &Self) -> Result<()> {
        self.native_input = self
            .native_input
            .checked_add(other.native_input)
            .ok_or("input overflow")?;
        self.cache_read = self
            .cache_read
            .checked_add(other.cache_read)
            .ok_or("cache overflow")?;
        self.cache_write = self
            .cache_write
            .checked_add(other.cache_write)
            .ok_or("cache overflow")?;
        self.output = self
            .output
            .checked_add(other.output)
            .ok_or("output overflow")?;
        self.cost_usd = self.cost_usd.zip(other.cost_usd).map(|(a, b)| a + b);
        ensure(self.cost_usd.is_none_or(|c| c.is_finite()), "cost overflow")
    }
    fn monotonic(&self, old: &Self) -> bool {
        self.native_input >= old.native_input
            && self.cache_read >= old.cache_read
            && self.cache_write >= old.cache_write
            && self.output >= old.output
            && self.cost_usd.zip(old.cost_usd).is_none_or(|(a, b)| a >= b)
    }
}

fn counter(value: &Value, key: &str, required: bool) -> Result<u64> {
    match value.get(key) {
        Some(v) => v
            .as_u64()
            .ok_or_else(|| format!("invalid usage counter {key}").into()),
        None if !required => Ok(0),
        None => Err(format!("missing usage counter {key}").into()),
    }
}

pub fn parse(family: Family, bytes: &[u8], basis: Basis) -> Result<Segment> {
    let mut segment = Segment {
        id: String::new(),
        session: String::new(),
        sequence: 0,
        basis,
        log_sha256: crate::files::digest(bytes),
        complete: true,
        terminal_events: 0,
        usage: Totals {
            cost_usd: Some(0.0),
            ..Totals::default()
        },
        issues: vec![],
    };
    let mut seen = BTreeMap::<String, String>::new();
    let mut anonymous_events = BTreeSet::new();
    let mut terminal_tail = false;
    let mut observed = false;
    for line in bytes.split(|b| *b == b'\n').filter(|s| !s.is_empty()) {
        let event: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => {
                segment
                    .issues
                    .push("unparsed log line; accounting may be truncated".into());
                continue;
            }
        };
        let kind = event["type"].as_str().unwrap_or("");
        if matches!(kind, "turn.failed" | "error") {
            segment
                .issues
                .push("native failure event without guaranteed final usage".into());
        }
        if event
            .get("parent_tool_use_id")
            .is_some_and(|v| !v.is_null())
            || event.get("subagent_id").is_some()
        {
            segment.issues.push(
                "child session observed; reconcile its separate usage before cost comparisons"
                    .into(),
            );
        }
        let terminal = match family {
            Family::Codex => kind == "turn.completed",
            _ => kind == "result",
        };
        terminal_tail = terminal;
        if !terminal {
            continue;
        }
        let event_id = event["uuid"]
            .as_str()
            .or(event["event_id"].as_str())
            .or(event["turn_id"].as_str());
        if let Some(event_id) = event_id {
            let hash = crate::files::digest(line);
            if let Some(previous) = seen.insert(event_id.to_string(), hash.clone()) {
                ensure(
                    previous == hash,
                    "same terminal event id has conflicting payloads",
                )?;
                continue;
            }
        } else if !anonymous_events.insert(crate::files::digest(line)) {
            segment.issues.push(
                "ambiguous repeated terminal event without an id; reconcile original segments"
                    .into(),
            );
            continue;
        }
        segment.terminal_events += 1;
        let Some(u) = event.get("usage").filter(|v| v.is_object()) else {
            segment.issues.push("terminal event lacks usage".into());
            continue;
        };
        let keys = if family == Family::Cursor {
            [
                "inputTokens",
                "cacheReadTokens",
                "cacheWriteTokens",
                "outputTokens",
            ]
        } else if family == Family::Codex {
            [
                "input_tokens",
                "cached_input_tokens",
                "cache_creation_input_tokens",
                "output_tokens",
            ]
        } else {
            [
                "input_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
                "output_tokens",
            ]
        };
        let parsed = (|| -> Result<Totals> {
            let cost = event
                .get("total_cost_usd")
                .or(event.get("cost_usd"))
                .map(|v| {
                    v.as_f64()
                        .filter(|n| n.is_finite() && *n >= 0.0)
                        .ok_or("invalid USD cost")
                })
                .transpose()?;
            Ok(Totals {
                native_input: counter(u, keys[0], true)?,
                cache_read: counter(u, keys[1], false)?,
                cache_write: counter(u, keys[2], false)?,
                output: counter(u, keys[3], true)?,
                cost_usd: cost,
            })
        })();
        match parsed {
            Err(e) => segment.issues.push(e.to_string()),
            Ok(t) => {
                if basis == Basis::SessionCumulative {
                    ensure(
                        !observed || t.monotonic(&segment.usage),
                        "cumulative usage decreased; supply incremental segments with documented semantics",
                    )?;
                    segment.usage = t;
                } else {
                    segment.usage.add(&t)?;
                }
                observed = true;
            }
        }
        if event["is_error"].as_bool() == Some(true) {
            segment.issues.push("native result reports an error".into());
        }
    }
    if !observed || segment.terminal_events == 0 {
        segment.issues.push("no complete native usage event".into());
        segment.usage.cost_usd = None;
    }
    if !terminal_tail {
        segment
            .issues
            .push("log ends before a terminal usage event".into());
    }
    segment.issues.sort();
    segment.issues.dedup();
    segment.complete = segment.issues.is_empty();
    Ok(segment)
}

pub fn reconcile(segments: &[Segment], convention: Convention) -> Result<Accounting> {
    let mut identities = BTreeSet::new();
    let mut log_ids = BTreeSet::new();
    let mut groups = BTreeMap::<&str, Vec<&Segment>>::new();
    for s in segments {
        crate::model::id(&s.id)?;
        crate::model::id(&s.session)?;
        crate::model::hash(&s.log_sha256)?;
        ensure(identities.insert(&s.id), "duplicate segment id")?;
        ensure(
            log_ids.insert((&s.session, &s.log_sha256)),
            "same log charged twice within a session",
        )?;
        groups.entry(&s.session).or_default().push(s);
    }
    let mut total = Totals {
        cost_usd: Some(0.0),
        ..Totals::default()
    };
    let mut issues = Vec::new();
    for (session, mut group) in groups {
        group.sort_by_key(|s| s.sequence);
        for pair in group.windows(2) {
            ensure(
                pair[0].sequence != pair[1].sequence,
                "duplicate session sequence",
            )?;
            ensure(
                pair[0].basis == pair[1].basis,
                "mixed cumulative and incremental session usage",
            )?;
        }
        for (i, s) in group.iter().enumerate() {
            if s.sequence as usize != i {
                issues.push(format!("session {session}: missing segment sequence"));
            }
            if !s.complete {
                issues.push(format!("segment {}: incomplete", s.id));
            }
            issues.extend(s.issues.iter().map(|v| format!("{}: {v}", s.id)));
        }
        if group[0].basis == Basis::SessionCumulative {
            for pair in group.windows(2) {
                ensure(
                    pair[1].usage.monotonic(&pair[0].usage),
                    "session cumulative counters decreased",
                )?;
            }
            total.add(&group.last().unwrap().usage)?;
        } else {
            for s in group {
                total.add(&s.usage)?;
            }
        }
    }
    if segments.is_empty() {
        issues.push("no segments".into());
        total.cost_usd = None;
    }
    issues.sort();
    issues.dedup();
    let complete = issues.is_empty();
    // Preserve known lower bounds, but never present them as complete cost/token estimates.
    if !complete {
        total.cost_usd = None;
    }
    let mut total_tokens = match convention {
        Convention::InputIncludesCache => total.native_input.checked_add(total.output),
        Convention::InputExcludesCache => total
            .native_input
            .checked_add(total.output)
            .and_then(|v| v.checked_add(total.cache_read))
            .and_then(|v| v.checked_add(total.cache_write)),
        Convention::Unknown => None,
    };
    if !complete {
        total_tokens = None;
    }
    Ok(Accounting {
        complete,
        segments: segments.len(),
        usage: total,
        total_tokens,
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn segment(seq: u32, input: u64, cost: f64) -> Segment {
        let raw = format!(
            r#"{{"type":"result","usage":{{"input_tokens":{input},"output_tokens":2}},"total_cost_usd":{cost}}}"#
        );
        let mut s = parse(Family::Claude, raw.as_bytes(), Basis::Incremental).unwrap();
        s.id = format!("seg-{seq}");
        s.session = "trial".into();
        s.sequence = seq;
        s
    }
    #[test]
    fn reconnects_sum_and_missing_tail_does_not_become_free() {
        let mut parts = vec![segment(0, 10, 0.1), segment(1, 20, 0.2)];
        let all = reconcile(&parts, Convention::InputExcludesCache).unwrap();
        assert_eq!(all.usage.native_input, 30);
        assert_eq!(all.total_tokens, Some(34));
        assert!((all.usage.cost_usd.unwrap() - 0.3).abs() < 1e-9);
        parts[1].complete = false;
        let partial = reconcile(&parts, Convention::InputExcludesCache).unwrap();
        assert!(!partial.complete);
        assert_eq!(partial.usage.cost_usd, None);
        assert_eq!(partial.total_tokens, None);
    }
    #[test]
    fn cumulative_segments_are_not_double_charged_and_duplicate_events_are_rejected() {
        let mut parts = vec![segment(0, 10, 0.1), segment(1, 20, 0.2)];
        for s in &mut parts {
            s.basis = Basis::SessionCumulative;
        }
        assert_eq!(
            reconcile(&parts, Convention::InputIncludesCache)
                .unwrap()
                .usage
                .native_input,
            20
        );
        parts[1].usage.native_input = 5;
        assert!(reconcile(&parts, Convention::InputIncludesCache).is_err());
        assert!(reconcile(&[parts[0].clone(), parts[0].clone()], Convention::Unknown).is_err());
    }
    #[test]
    fn codex_turns_and_cursor_native_fields_are_preserved() {
        let log=b"{\"type\":\"turn.completed\",\"turn_id\":\"one\",\"usage\":{\"input_tokens\":100,\"cached_input_tokens\":70,\"output_tokens\":4}}\n{\"type\":\"turn.completed\",\"turn_id\":\"two\",\"usage\":{\"input_tokens\":200,\"output_tokens\":6}}\n";
        let c = parse(Family::Codex, log, Basis::Incremental).unwrap();
        assert_eq!(c.usage.native_input, 300);
        assert_eq!(c.usage.cache_read, 70);
        assert_eq!(c.usage.cost_usd, None);
        let c = parse(
            Family::Cursor,
            br#"{"type":"result","usage":{"inputTokens":12,"cacheReadTokens":8,"outputTokens":3}}"#,
            Basis::Incremental,
        )
        .unwrap();
        assert_eq!(c.usage.native_input, 12);
        assert_eq!(c.usage.cache_read, 8);
        assert!(c.complete);
    }
    #[test]
    fn a_reconnect_tail_or_ambiguous_duplicate_is_incomplete() {
        let event = "{\"type\":\"result\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2},\"total_cost_usd\":0.1}";
        let repeated = format!("{event}\n{event}\n");
        let parsed = parse(Family::Claude, repeated.as_bytes(), Basis::Incremental).unwrap();
        assert!(!parsed.complete);
        assert_eq!(parsed.usage.native_input, 10);
        let tail = format!("{event}\n{{\"type\":\"assistant\",\"message\":{{}}}}\n");
        assert!(
            !parse(Family::Claude, tail.as_bytes(), Basis::Incremental)
                .unwrap()
                .complete
        );
    }
}
