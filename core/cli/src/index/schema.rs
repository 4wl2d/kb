//! Index database schema (index schema 1), connection setup and compatibility checks.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode as SqlCode, TransactionBehavior};

use crate::model::Profile;
use crate::versions::{ENGINE_VERSION, INDEX_SCHEMA, PARSER_VERSION};

/// How long a connection waits for another writer before failing with "database is locked".
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// Tables that must exist in a usable index.
const REQUIRED_TABLES: [&str; 8] = [
    "meta",
    "docs",
    "docs_fts",
    "doc_paths",
    "doc_terms",
    "snapshots",
    "snapshot_docs",
    "snapshot_proposals",
];

/// Schema DDL. `docs` are content-addressed by (content id, parser version); snapshots only
/// reference them, so unchanged files are never parsed twice.
const DDL: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE docs (
    id          INTEGER PRIMARY KEY,
    content_id  TEXT NOT NULL,
    parser      INTEGER NOT NULL,
    record_id   TEXT,
    kind        TEXT,
    status      TEXT,
    title       TEXT,
    meta        TEXT,
    parsed      TEXT,
    raw         TEXT NOT NULL,
    diagnostics TEXT NOT NULL,
    fts_len     INTEGER NOT NULL,
    UNIQUE (content_id, parser)
);
CREATE INDEX docs_record_id ON docs (record_id);

CREATE VIRTUAL TABLE docs_fts USING fts5 (
    ids, title, aliases, normative, body,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE doc_paths (
    doc        INTEGER NOT NULL REFERENCES docs (id) ON DELETE CASCADE,
    repo       TEXT,
    dir_prefix TEXT NOT NULL,
    glob       TEXT NOT NULL
);
CREATE INDEX doc_paths_prefix ON doc_paths (dir_prefix);
CREATE INDEX doc_paths_doc ON doc_paths (doc);

CREATE TABLE doc_terms (
    doc     INTEGER NOT NULL REFERENCES docs (id) ON DELETE CASCADE,
    kind    TEXT NOT NULL,
    value   TEXT NOT NULL,
    pattern TEXT NOT NULL
);
CREATE INDEX doc_terms_value ON doc_terms (kind, value);
CREATE INDEX doc_terms_doc ON doc_terms (doc);

CREATE TABLE snapshots (
    id          INTEGER PRIMARY KEY,
    key         TEXT NOT NULL UNIQUE,
    created     INTEGER NOT NULL,
    config      TEXT NOT NULL,
    registry    TEXT NOT NULL,
    diagnostics TEXT NOT NULL,
    stats       TEXT NOT NULL,
    fts_docs    INTEGER NOT NULL,
    fts_avglen  REAL NOT NULL
);
CREATE INDEX snapshots_created ON snapshots (created);

CREATE TABLE snapshot_docs (
    snapshot  INTEGER NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
    origin    TEXT NOT NULL,
    path      TEXT NOT NULL,
    doc       INTEGER NOT NULL REFERENCES docs (id),
    record_id TEXT,
    kind      TEXT,
    -- 1 when another member of the same snapshot and origin with the same record id has a
    -- smaller path: duplicated ids are served from their first file (set once per build).
    shadowed  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (snapshot, origin, path)
) WITHOUT ROWID;
-- Secondary indexes of a WITHOUT ROWID table carry the primary key, so this one also
-- serves (doc, snapshot, origin) membership checks.
CREATE INDEX snapshot_docs_doc ON snapshot_docs (doc);
CREATE INDEX snapshot_docs_kind ON snapshot_docs (snapshot, origin, kind, doc);
CREATE INDEX snapshot_docs_record ON snapshot_docs (snapshot, origin, record_id, doc);

CREATE TABLE snapshot_proposals (
    snapshot    INTEGER NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    record_id   TEXT,
    change      TEXT NOT NULL,
    doc         INTEGER REFERENCES docs (id),
    stale       INTEGER NOT NULL,
    diagnostics TEXT NOT NULL,
    PRIMARY KEY (snapshot, path)
) WITHOUT ROWID;
CREATE INDEX snapshot_proposals_doc ON snapshot_proposals (doc);
"#;

/// Why an existing database cannot be used as is.
#[derive(Debug)]
pub enum OpenProblem {
    /// Corrupt file, not a database, or a schema this engine cannot interpret: move aside.
    Unusable(String),
    /// A well-formed index whose metadata keys are missing or differ (another engine, index
    /// schema, layout, parser version or profile): rebuild in place.
    Outdated(String),
}

/// Failure to open or check the database.
#[derive(Debug)]
pub enum OpenError {
    Problem(OpenProblem),
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for OpenError {
    /// Corruption signals become [`OpenProblem::Unusable`]; anything else stays an error.
    fn from(e: rusqlite::Error) -> Self {
        if is_corruption(&e) {
            OpenError::Problem(OpenProblem::Unusable(format!("unreadable database: {e}")))
        } else {
            OpenError::Sql(e)
        }
    }
}

/// Is this SQLite error a corruption signal (`SQLITE_CORRUPT` / `SQLITE_NOTADB`)?
pub fn is_corruption(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(SqlCode::DatabaseCorrupt | SqlCode::NotADatabase)
    )
}

/// Open a connection with the index pragmas (WAL, busy timeout, foreign keys).
pub fn connect(path: &Path) -> Result<Connection, OpenError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(OpenError::Problem(OpenProblem::Unusable(format!(
            "cannot enable WAL mode (journal_mode is `{mode}`)"
        ))));
    }
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -16000;",
    )?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(conn)
}

/// Revision of the table layout; bump when tables change so older caches are rebuilt.
const LAYOUT: u32 = 2;

/// Expected `meta` rows for this engine and profile.
fn expected_meta(profile: Profile) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("index_schema", INDEX_SCHEMA.to_string()),
        // Table layout revision within this index schema (derived data: a mismatch rebuilds).
        ("layout", LAYOUT.to_string()),
        ("parser_version", PARSER_VERSION.to_string()),
        ("engine_version", ENGINE_VERSION.to_string()),
        ("profile", profile.as_str().to_string()),
    ])
}

/// Check that the database holds a compatible index, creating the schema in an empty
/// database. Must be called with the initialization lock held.
pub fn check_or_init(conn: &mut Connection, profile: Profile) -> Result<(), OpenError> {
    let unusable = |msg: String| Err(OpenError::Problem(OpenProblem::Unusable(msg)));
    let tables = table_names(conn)?;
    if tables.is_empty() {
        return Ok(create(conn, profile)?);
    }
    if !tables.iter().any(|t| t == "meta") {
        return unusable("database has no kb index metadata".into());
    }
    let found = match read_meta(conn) {
        Ok(m) => m,
        Err(e) if is_corruption(&e) => return Err(e.into()),
        Err(e) => return unusable(format!("index metadata is unreadable: {e}")),
    };
    // A kb index with a missing or different metadata key was written by another engine,
    // index schema, layout revision or profile (older engines lack keys added later): it
    // is well-formed derived data and is rebuilt in place, not moved aside as corrupt.
    let mut outdated = Vec::new();
    for (key, want) in expected_meta(profile) {
        match found.get(key) {
            None => outdated.push(format!("{key} missing -> {want}")),
            Some(have) if *have != want => outdated.push(format!("{key} {have} -> {want}")),
            Some(_) => {}
        }
    }
    if !outdated.is_empty() {
        return Err(OpenError::Problem(OpenProblem::Outdated(
            outdated.join(", "),
        )));
    }
    if let Some(missing) = REQUIRED_TABLES
        .iter()
        .find(|t| !tables.iter().any(|have| have == *t))
    {
        return unusable(format!("index table `{missing}` is missing"));
    }
    Ok(())
}

fn read_meta(conn: &Connection) -> Result<BTreeMap<String, String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT key, value FROM meta")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect()
}

fn table_names(conn: &Connection) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

/// Create the schema and metadata in one transaction (no-op if another process won).
fn create(conn: &mut Connection, profile: Profile) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        tx.execute_batch(DDL)?;
        for (k, v) in expected_meta(profile) {
            tx.execute("INSERT INTO meta (key, value) VALUES (?1, ?2)", (k, v))?;
        }
    }
    tx.commit()
}

/// Drop every object of an outdated index and recreate the schema, in one transaction.
/// Other connections see either the old or the new index, never a mix.
pub fn reset(conn: &mut Connection, profile: Profile) -> Result<(), rusqlite::Error> {
    // Tables of an unknown older schema are dropped in arbitrary order; foreign keys can
    // only be switched off outside a transaction.
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let result = drop_and_create(conn, profile);
    let restored = conn.execute_batch("PRAGMA foreign_keys = ON");
    result.and(restored)
}

fn drop_and_create(conn: &mut Connection, profile: Profile) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Virtual tables first: dropping them removes their shadow tables.
    for virtual_only in [true, false] {
        let names: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                   AND (sql LIKE 'CREATE VIRTUAL TABLE%') = ?1",
            )?;
            stmt.query_map([virtual_only], |r| r.get::<_, String>(0))?
                .collect::<Result<_, _>>()?
        };
        for name in names {
            tx.execute_batch(&format!("DROP TABLE {}", quote_ident(&name)))?;
        }
    }
    tx.execute_batch(DDL)?;
    for (k, v) in expected_meta(profile) {
        tx.execute("INSERT INTO meta (key, value) VALUES (?1, ?2)", (k, v))?;
    }
    tx.commit()
}

/// Quote an SQL identifier (names read from `sqlite_master` of an untrusted cache file).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Cheap structural check used after a failed operation: `None` when healthy.
pub fn health_problem(conn: &Connection) -> Option<String> {
    match conn.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0)) {
        Ok(s) if s == "ok" => None,
        Ok(s) => Some(s),
        Err(e) if is_corruption(&e) => Some(e.to_string()),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_accepts_own_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.sqlite");
        let mut conn = connect(&path).unwrap();
        check_or_init(&mut conn, Profile::Project).unwrap();
        check_or_init(&mut conn, Profile::Project).unwrap();
        match check_or_init(&mut conn, Profile::Maintainer) {
            Err(OpenError::Problem(OpenProblem::Outdated(r))) => {
                assert!(r.contains("profile"), "{r}")
            }
            other => panic!("expected outdated, got {other:?}"),
        }
        reset(&mut conn, Profile::Maintainer).unwrap();
        check_or_init(&mut conn, Profile::Maintainer).unwrap();
        assert!(health_problem(&conn).is_none());
    }

    #[test]
    fn quotes_identifiers() {
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
    }

    /// Historical slices rank in Rust (`lexical::rank_corpus`); its term folding must agree
    /// with the FTS5 tokenizer of `docs_fts` for every character that normalizes to a token.
    #[test]
    fn slice_term_folding_matches_the_full_text_tokenizer() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(DDL).unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE temp.vocab USING fts5vocab(main, docs_fts, instance)",
        )
        .unwrap();
        let mut expected = BTreeMap::new();
        let mut insert = conn
            .prepare("INSERT INTO docs_fts (rowid, ids) VALUES (?1, ?2)")
            .unwrap();
        for c in (0x80..=0x10FFFF).filter_map(char::from_u32) {
            if let [token] = crate::normalize::tokens(&c.to_string()).as_slice() {
                // Surrounding ASCII keeps every character inside one FTS token.
                let folded = crate::context::lexical::fts_term(token);
                insert.execute((c as i64, format!("x{token}x"))).unwrap();
                expected.insert(c as i64, (token.clone(), format!("x{folded}x")));
            }
        }
        let mut terms: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        let mut rows = conn.prepare("SELECT doc, term FROM temp.vocab").unwrap();
        for row in rows.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() {
            let (doc, term) = row.unwrap();
            terms.entry(doc).or_default().push(term);
        }
        let mut folded = 0;
        for (doc, (token, want)) in &expected {
            // Characters unicode61 treats as separators (or as unassigned in its Unicode
            // version) split or drop the token; that involves no folding.
            if let Some([got]) = terms.get(doc).map(Vec::as_slice) {
                assert_eq!(got, want, "U+{doc:04X}");
                folded += usize::from(*want != format!("x{token}x"));
            }
        }
        assert!(
            folded > 400,
            "only {folded} folded characters were compared"
        );
    }
}
