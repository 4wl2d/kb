# Synthetic context fixture (not real project data)

An invented multi-repository knowledge base ("Acme Shop": repos `mobile`, `backend` and
`shared`) used by `core/cli/tests/context_golden.rs`. It exists only to test context
assembly: routing by task, file or directory path (including a module glob with a wildcard
before its directory part, `mobile.analytics`), module and concept; multilingual aliases;
ambiguity; cross-repo contracts; overrides; version applicability; budgets; search and show.
All names, ids, paths and rules are synthetic and describe no real product.

Layout: `project/` is a profile directory as created by `kb init` (config, registries,
`knowledge/` records and `routing-tests/` golden cases). Tests load it with
`WorkingTreeSource` rooted at this directory.
