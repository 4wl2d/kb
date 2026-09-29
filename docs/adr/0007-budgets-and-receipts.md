# ADR 0007: Whole-unit packing with byte and estimated-token budgets

Status: accepted

## Decision

Budgets are exact UTF-8 bytes of the rendered payload or a deterministic token estimate
(`ceil(ascii_bytes/4) + ceil(non_ascii_chars/2)`, clearly labeled as an estimate). Mandatory
units are packed first and never truncated; if they do not fit, the command fails with
`CONTEXT_BUDGET_EXCEEDED` and the required amount. Every result carries a receipt id
(SHA-256 of the canonical deterministic result) that proves what was delivered, not that it
was understood or followed.
