//! `kb tools`: the tool registry, which tools are available on this machine, and which tool
//! answers each request. Routes first (what an agent needs), then the tools.

use std::collections::BTreeMap;

use super::Ctx;
use super::agent;
use super::args::ToolsArgs;
use crate::answer::{Answer, Item, Source};
use crate::error::Result;
use crate::model::tools::{AdapterKind, Capability};
use crate::output::CommandOutput;
use crate::tools::{self, codegraph};

pub fn run(ctx: &Ctx, args: &ToolsArgs) -> Result<CommandOutput> {
    let ws = agent::workspace(ctx)?;
    let lines = agent::lines(args.lines);
    let mut answer = Answer::new("tools", "");
    answer.source(Source {
        id: "registry".into(),
        adapter: "tools".into(),
        version: None,
        state: ws.registry.origin(),
    });
    let probes: BTreeMap<String, tools::Probe> = ws
        .registry
        .config
        .tools
        .iter()
        .map(|t| (t.id.clone(), tools::probe(t, &ws.roots, true)))
        .collect();
    for cap in Capability::ROUTED {
        let route = ws.registry.config.route(cap);
        let order: Vec<&str> = route.iter().map(|t| t.id.as_str()).collect();
        let answering = route.iter().find(|t| probes[&t.id].available);
        let detail = match answering {
            Some(t) => format!("{} (route: {})", t.id, order.join(" → ")),
            None => {
                answer.note(format!("nothing available answers `{}`", cap.as_str()));
                format!(
                    "unavailable (route: {})",
                    if order.is_empty() {
                        "empty".into()
                    } else {
                        order.join(" → ")
                    }
                )
            }
        };
        answer.items.push(
            Item::new("route", "registry")
                .name(cap.as_str())
                .detail(detail),
        );
    }
    answer.items.push(
        Item::new("route", "registry")
            .name("doc")
            .detail("knowledge (built in: feature records and the records scoped to the path)"),
    );
    let checks = ws
        .registry
        .config
        .tools
        .iter()
        .filter(|t| t.adapter == AdapterKind::Command)
        .count();
    answer
        .items
        .push(Item::new("route", "registry").name("check").detail(format!(
            "{checks} command tool(s) by path, plus knowledge validation and declared probes"
        )));
    for tool in &ws.registry.config.tools {
        let p = &probes[&tool.id];
        let caps: Vec<&str> = tool
            .adapter
            .capabilities()
            .iter()
            .map(|c| c.as_str())
            .collect();
        let mut detail = match &p.version {
            Some(v) => format!("{v} · "),
            None => String::new(),
        };
        detail.push_str(&format!(
            "adapter {} [{}]",
            tool.adapter.as_str(),
            caps.join(",")
        ));
        if tool.adapter == AdapterKind::Command {
            detail.push_str(&format!(" paths={}", tool.paths.join(",")));
        }
        if p.available {
            if tool.adapter == AdapterKind::Codegraph
                && let Some(program) = &p.program
            {
                match codegraph::status(std::path::Path::new(program), &ws.roots, tool.timeout()) {
                    Ok(state) => {
                        detail.push_str(&format!(" · {}", state.describe(false)));
                        if !state.initialized {
                            answer.note(format!(
                                "{}: create the index with `codegraph init --yes {}`",
                                tool.id,
                                ws.roots.code.display()
                            ));
                        }
                    }
                    Err(e) => detail.push_str(&format!(" · status failed: {}", e.message)),
                }
            }
            detail.push_str(" · available");
        } else {
            detail.push_str(&format!(
                " · unavailable: {}",
                p.reason.clone().unwrap_or_default()
            ));
        }
        answer.items.push(
            Item::new("tool", "registry")
                .name(tool.id.clone())
                .detail(detail),
        );
    }
    answer.note(format!("code root: {}", ws.roots.code.display()));
    let mut out = agent::output(answer, lines, None);
    out.meta
        .insert("probes".into(), serde_json::to_value(&probes)?);
    Ok(out)
}
