# Example: synthetic multi-repository product (SYNTHETIC)

This project was created with `kbw init --example synthetic-multirepo`. Every repository,
team, record and URL in it is invented for demonstration and testing; none of it describes
a real product or organization. Do not build real work on it: initialize a real project with
`kbw init --name <name> --namespace <ns>` in a fresh downstream instead.

The invented product has three repositories (`mobile`, `backend`, `shared-contracts`), two
features (`login`, `checkout`), cross-repository contracts, a product-wide review policy with
an overridable setting and a stricter override for payment code, a superseded decision, a
draft proposal, and a known gap. See `core/templates/examples/synthetic-multirepo/` for the
walkthrough and the expected context queries.
