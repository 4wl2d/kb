use std::collections::BTreeMap;

use kb::error::{KbError, Result};
use kb::model::*;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::{Options, frozen::Frozen, native, response, test_candidate};

#[derive(Deserialize)]
struct Node {
    id: String,
    kind: String,
    name: String,
    qualified_name: Option<String>,
    file_path: String,
    start_line: u32,
    end_line: u32,
    signature: Option<String>,
}

#[derive(Deserialize)]
struct Edge {
    source: String,
    target: String,
    kind: String,
    metadata: Option<String>,
    line: Option<u32>,
}

#[derive(Deserialize)]
struct Export {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    unresolved: u32,
    failed_files: u32,
}

fn db_error(error: rusqlite::Error) -> KbError {
    KbError::invalid_input(format!("CodeGraph 1.6.1 database: {error}"))
}

pub(crate) fn load(
    frozen: &Frozen,
    options: &Options,
    request: &CodeRequest,
) -> Result<CodeResponse> {
    native(frozen, options, &["init", "--yes", "."])?;
    let db = Connection::open_with_flags(
        frozen.root.join(".codegraph/codegraph.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(db_error)?;
    let metadata: BTreeMap<String, String> = db
        .prepare("SELECT key,value FROM project_metadata")
        .map_err(db_error)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(db_error)?
        .collect::<std::result::Result<_, _>>()
        .map_err(db_error)?;
    if metadata.get("indexed_with_version").map(String::as_str) != Some("1.6.1")
        || metadata
            .get("indexed_with_extraction_version")
            .map(String::as_str)
            != Some("27")
        || metadata.get("indexed_at_commit") != Some(&request.commit)
        || metadata.get("index_state").map(String::as_str) != Some("complete")
    {
        return Err(KbError::invalid_input(
            "CodeGraph index version, extraction version, commit or completion marker does not match",
        ));
    }
    let schema: u32 = db
        .query_row("SELECT MAX(version) FROM schema_versions", [], |r| r.get(0))
        .map_err(db_error)?;
    if schema != 11 {
        return Err(KbError::invalid_input(
            "unsupported CodeGraph database schema; expected 11",
        ));
    }
    for (sql, cap) in [
        ("SELECT COUNT(*) FROM nodes", kb::code::MAX_SYMBOLS),
        (
            "SELECT COUNT(*) FROM edges WHERE kind <> 'contains'",
            kb::code::MAX_REFS,
        ),
    ] {
        let count: u32 = db.query_row(sql, [], |r| r.get(0)).map_err(db_error)?;
        if count as usize > cap {
            return Err(KbError::invalid_input(
                "CodeGraph index exceeds adapter limits",
            ));
        }
    }
    let nodes = db.prepare("SELECT id,kind,name,qualified_name,file_path,start_line,end_line,signature FROM nodes ORDER BY id").map_err(db_error)?
        .query_map([], |r| Ok(Node { id:r.get(0)?,kind:r.get(1)?,name:r.get(2)?,qualified_name:r.get(3)?,file_path:r.get(4)?,start_line:r.get(5)?,end_line:r.get(6)?,signature:r.get(7)? })).map_err(db_error)?
        .collect::<std::result::Result<Vec<_>,_>>().map_err(db_error)?;
    let edges = db.prepare("SELECT source,target,kind,metadata,line FROM edges WHERE kind <> 'contains' ORDER BY source,target,kind,line").map_err(db_error)?
        .query_map([], |r| Ok(Edge { source:r.get(0)?,target:r.get(1)?,kind:r.get(2)?,metadata:r.get(3)?,line:r.get(4)? })).map_err(db_error)?
        .collect::<std::result::Result<Vec<_>,_>>().map_err(db_error)?;
    let unresolved = db
        .query_row("SELECT COUNT(*) FROM unresolved_refs", [], |r| r.get(0))
        .map_err(db_error)?;
    let failed_files = db.query_row("SELECT COUNT(*) FROM files WHERE errors IS NOT NULL AND errors <> '[]' AND errors <> 'null'", [], |r| r.get(0)).map_err(db_error)?;
    let mut result = convert(
        request,
        &frozen.files,
        Export {
            nodes,
            edges,
            unresolved,
            failed_files,
        },
    )?;
    let dirty: serde_json::Value = serde_json::from_str(
        metadata
            .get("indexed_dirty_paths")
            .map(String::as_str)
            .unwrap_or("null"),
    )?;
    if dirty["commit"].as_str() != Some(&request.commit)
        || dirty["paths"].as_array().is_none_or(|p| !p.is_empty())
    {
        result.complete = false;
        result
            .limitations
            .push("CodeGraph reports omitted or dirty paths in the materialized tree.".into());
    }
    Ok(result)
}

fn convert(
    request: &CodeRequest,
    files: &BTreeMap<String, Vec<u8>>,
    export: Export,
) -> Result<CodeResponse> {
    let mut result = response(request, crate::Backend::Codegraph);
    let mut locations = BTreeMap::new();
    let mut skipped = 0;
    for node in export.nodes {
        let Some(bytes) = files.get(&node.file_path) else {
            skipped += 1;
            continue;
        };
        let is_file = node.kind == "file";
        // Native file nodes count the empty suffix after a trailing newline. The protocol
        // hashes actual inclusive lines; only file extents are normalized this way.
        let end_line = if is_file {
            bytes.split_inclusive(|b| *b == b'\n').count().max(1) as u32
        } else {
            node.end_line
        };
        let start_line = if is_file { 1 } else { node.start_line };
        let span = kb::provenance::line_span(bytes, start_line, end_line)?;
        locations.insert(node.id.clone(), (node.file_path.clone(), start_line));
        result.symbols.push(CodeSymbol {
            id: node.id,
            name: node
                .qualified_name
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| node.name.clone()),
            test: test_candidate(&node.file_path, &node.name),
            kind: node.kind,
            path: node.file_path,
            start_line,
            end_line,
            extent: if is_file {
                CodeExtent::File
            } else {
                CodeExtent::Definition
            },
            sha256: kb::util::sha256_hex(span),
            signature: node.signature,
        });
    }
    for edge in export.edges {
        if edge.kind == "contains" {
            continue;
        }
        let (Some((_, fallback_line)), true) = (
            locations.get(&edge.source),
            locations.contains_key(&edge.target),
        ) else {
            skipped += 1;
            continue;
        };
        let metadata: serde_json::Value = edge
            .metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        let resolved = metadata["confidence"].as_f64().is_some_and(|c| c >= 0.9)
            && metadata["resolvedBy"]
                .as_str()
                .is_some_and(|s| matches!(s, "import" | "scope" | "exact" | "qualified"));
        result.refs.push(CodeRef {
            from: edge.source,
            to: edge.target,
            kind: match edge.kind.as_str() {
                "calls" => CodeRelation::Call,
                "imports" => CodeRelation::Import,
                _ => CodeRelation::Reference,
            },
            confidence: if resolved {
                CodeConfidence::Resolved
            } else {
                CodeConfidence::Possible
            },
            line: edge.line.filter(|n| *n > 0).unwrap_or(*fallback_line),
        });
    }
    if export.unresolved > 0 || export.failed_files > 0 || skipped > 0 {
        result.complete = false;
        result.limitations.push(format!("{} unresolved references (including dependencies), {} failed files, {skipped} external/omitted nodes or edges.", export.unresolved, export.failed_files));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_normalizes_file_end_retains_definition_and_distinguishes_confidence() {
        let request: CodeRequest =
            serde_json::from_str(include_str!("../tests/fixtures/request.json")).unwrap();
        let files = BTreeMap::from([
            (
                "src/lib.rs".into(),
                b"pub fn run() {\n    store::save();\n}\n".to_vec(),
            ),
            ("src/store.rs".into(), b"pub fn save() {}\n".to_vec()),
        ]);
        let result = convert(
            &request,
            &files,
            serde_json::from_slice(include_bytes!("../tests/fixtures/codegraph.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.id == "file:src/lib.rs")
                .unwrap()
                .end_line,
            3
        );
        assert_eq!(
            result
                .symbols
                .iter()
                .find(|s| s.id == "run")
                .unwrap()
                .extent,
            CodeExtent::Definition
        );
        assert_eq!(result.refs[0].confidence, CodeConfidence::Resolved);
        assert_eq!(result.refs[1].confidence, CodeConfidence::Possible);
        assert!(!result.complete);
        assert_eq!(result.refs[0].line, 2);
    }
}
