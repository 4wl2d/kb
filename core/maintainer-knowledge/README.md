# Maintainer knowledge (opt-in profile)

Typed knowledge about the kb engine itself, for people and agents changing the engine.
It is **never** part of a project's context: normal indexing reads only `project/`.
Use it explicitly with `--profile maintainer`, for example:

```sh
./kbw validate --profile maintainer
./kbw context --profile maintainer --snapshot working-tree --offline \
  --intent implement --path core/cli/src/index/mod.rs --repo kb
```

Records follow the same strict format as project knowledge (namespace `kb`).
