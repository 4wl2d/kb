//! `kb schema` handler: report, check (`--check`) or write (`--write`) the generated JSON
//! Schemas in `core/schemas`.

use serde_json::json;

use super::Ctx;
use super::args::SchemaArgs;
use crate::error::{ErrorCode, KbError, Result};
use crate::output::CommandOutput;
use crate::schema_export::{SCHEMAS_DIR, check, generate, write};

pub fn run(ctx: &Ctx, args: &SchemaArgs) -> Result<CommandOutput> {
    let root = &ctx.env.kb_root;
    let schemas: Vec<String> = generate().into_keys().collect();
    if args.write {
        let changes = write(root)?;
        let text = if changes.is_empty() {
            format!(
                "{} schemas in {SCHEMAS_DIR} are up to date\n",
                schemas.len()
            )
        } else {
            lines(&changes)
        };
        let result = json!({"dir": SCHEMAS_DIR, "schemas": schemas, "changes": changes});
        return Ok(CommandOutput::new(result, text));
    }
    let drift = check(root)?;
    let in_sync = drift.is_empty();
    let text = if in_sync {
        format!(
            "{} schemas in {SCHEMAS_DIR} match the model\n",
            schemas.len()
        )
    } else {
        lines(&drift)
    };
    let result =
        json!({"dir": SCHEMAS_DIR, "schemas": schemas, "in_sync": in_sync, "drift": drift});
    let out = CommandOutput::new(result, text);
    if args.check && !in_sync {
        return Ok(out.with_failure(
            KbError::new(
                ErrorCode::DriftDetected,
                format!(
                    "{} file(s) in {SCHEMAS_DIR} differ from the schemas generated from the model",
                    drift.len()
                ),
            )
            .with_details(json!({ "files": drift }))
            .with_hint("run `kbw schema --write` and review the diff"),
        ));
    }
    Ok(out)
}

fn lines(items: &[String]) -> String {
    items.iter().map(|s| format!("{s}\n")).collect()
}
