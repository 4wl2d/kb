use std::collections::{BTreeMap, BTreeSet};

use kb::error::{KbError, Result};
use kb::model::*;
use serde::Deserialize;

use crate::{Options, frozen::Frozen, native, response, test_candidate};

#[derive(Deserialize)]
struct Query<T> {
    count: usize,
    rows: Vec<T>,
}

#[derive(Deserialize)]
struct Symbol {
    name: String,
    qualified_name: Option<String>,
    kind: String,
    line: u32,
    signature: Option<String>,
    path: String,
}

#[derive(Deserialize)]
struct Reference {
    name: String,
    line: u32,
    path: String,
}

pub(crate) fn load(
    frozen: &Frozen,
    options: &Options,
    request: &CodeRequest,
) -> Result<CodeResponse> {
    native(
        frozen,
        options,
        &[
            "rebuild",
            "--include",
            ".",
            "--type",
            "all",
            "--no-deps",
            "--max-files",
            "100000",
        ],
    )?;
    let symbols = native(
        frozen,
        options,
        &[
            "query",
            "SELECT s.name,s.qualified_name,s.kind,s.line,s.signature,f.path FROM symbols s JOIN files f ON f.id=s.file_id ORDER BY f.path,s.line,s.name,s.kind",
            "--limit",
            "100001",
            "--format",
            "json",
        ],
    )?;
    let refs = native(
        frozen,
        options,
        &[
            "query",
            "SELECT r.name,r.line,f.path FROM refs r JOIN files f ON f.id=r.file_id ORDER BY f.path,r.line,r.name",
            "--limit",
            "500001",
            "--format",
            "json",
        ],
    )?;
    convert(request, &frozen.files, &symbols, &refs)
}

fn rows<T: serde::de::DeserializeOwned>(bytes: &[u8], cap: usize) -> Result<Vec<T>> {
    let query: Query<T> = serde_json::from_slice(bytes)?;
    if query.rows.len() > cap || query.count != query.rows.len() {
        return Err(KbError::invalid_input(
            "ast-index query was truncated or exceeded adapter limits",
        ));
    }
    Ok(query.rows)
}

fn convert(
    request: &CodeRequest,
    files: &BTreeMap<String, Vec<u8>>,
    symbols: &[u8],
    refs: &[u8],
) -> Result<CodeResponse> {
    let mut result = response(request, crate::Backend::AstIndex);
    result.complete = false;
    result.limitations.push("ast-index supplies definition lines and name-based references; definition extents, import kinds and semantic resolution are unavailable. Edges are possible and originate at file scope.".into());
    let symbols: Vec<Symbol> = rows(symbols, kb::code::MAX_SYMBOLS)?;
    let refs: Vec<Reference> = rows(refs, kb::code::MAX_REFS)?;
    let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut paths = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for symbol in symbols {
        let bytes = files.get(&symbol.path).ok_or_else(|| {
            KbError::invalid_input("ast-index returned a path outside the frozen tree")
        })?;
        let span = kb::provenance::line_span(bytes, symbol.line, symbol.line)?;
        let name = symbol
            .qualified_name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| symbol.name.clone());
        let id = format!(
            "ast:{}",
            kb::util::sha256_hex(
                format!(
                    "{}\0{}\0{}\0{}",
                    symbol.path, symbol.line, symbol.kind, name
                )
                .as_bytes()
            )
        );
        if !ids.insert(id.clone()) {
            continue;
        }
        names
            .entry(symbol.name.clone())
            .or_default()
            .insert(id.clone());
        names.entry(name.clone()).or_default().insert(id.clone());
        paths.insert(symbol.path.clone());
        result.symbols.push(CodeSymbol {
            id,
            name,
            kind: symbol.kind,
            test: test_candidate(&symbol.path, &symbol.name),
            path: symbol.path,
            start_line: symbol.line,
            end_line: symbol.line,
            extent: CodeExtent::Line,
            sha256: kb::util::sha256_hex(span),
            signature: symbol.signature,
        });
    }
    let mut unmatched = 0;
    for reference in refs {
        if !files.contains_key(&reference.path) {
            return Err(KbError::invalid_input(
                "ast-index reference outside frozen tree",
            ));
        }
        paths.insert(reference.path.clone());
        let Some(targets) = names.get(&reference.name) else {
            unmatched += 1;
            continue;
        };
        for target in targets {
            if result.refs.len() >= kb::code::MAX_REFS {
                return Err(KbError::invalid_input(
                    "ambiguous ast-index references exceed edge limit",
                ));
            }
            result.refs.push(CodeRef {
                from: format!("file:{}", reference.path),
                to: target.clone(),
                kind: CodeRelation::Reference,
                confidence: CodeConfidence::Possible,
                line: reference.line,
            });
        }
    }
    for path in paths {
        let bytes = &files[&path];
        let end = bytes.split_inclusive(|b| *b == b'\n').count().max(1) as u32;
        result.symbols.push(CodeSymbol {
            id: format!("file:{path}"),
            name: path.clone(),
            kind: "file".into(),
            test: test_candidate(&path, ""),
            path,
            start_line: 1,
            end_line: end,
            extent: CodeExtent::File,
            sha256: kb::util::sha256_hex(bytes),
            signature: None,
        });
    }
    if unmatched > 0 {
        result.limitations.push(format!("{unmatched} name references had no indexed definition (including external dependencies)."));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_keeps_ambiguous_references_possible_and_hashes_exact_lines() {
        let req: CodeRequest =
            serde_json::from_str(include_str!("../tests/fixtures/request.json")).unwrap();
        let files = BTreeMap::from([
            ("src/lib.rs".into(), b"pub fn run() { save(); }\n".to_vec()),
            ("src/a.rs".into(), b"pub fn save() {}\n".to_vec()),
            ("src/b.rs".into(), b"pub fn save() {}\n".to_vec()),
        ]);
        let response = convert(
            &req,
            &files,
            include_bytes!("../tests/fixtures/ast-symbols.json"),
            include_bytes!("../tests/fixtures/ast-refs.json"),
        )
        .unwrap();
        assert!(!response.complete);
        assert_eq!(response.refs.len(), 2);
        assert!(
            response
                .refs
                .iter()
                .all(|r| r.confidence == CodeConfidence::Possible && r.from == "file:src/lib.rs")
        );
        assert!(
            response
                .symbols
                .iter()
                .filter(|s| s.extent == CodeExtent::Line)
                .all(|s| s.start_line == s.end_line)
        );
        let dependents = kb::code::dependents(&response, &BTreeSet::from(["src/a.rs".into()]), 2);
        assert_eq!(dependents[0].path, "src/lib.rs");
        assert_eq!(dependents[0].confidence, CodeConfidence::Possible);
    }

    #[test]
    fn refuses_truncated_native_query() {
        assert!(rows::<Reference>(br#"{"count":2,"rows":[]}"#, 10).is_err());
    }
}
