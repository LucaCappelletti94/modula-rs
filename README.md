# modula-rs
Crate to compute how modular is your Rust code.

modula scores how well a codebase's declared module tree matches its actual internal dependency graph, producing a single 0 to 1 score plus a report. It analyzes Rust natively through rust-analyzer, and TypeScript, JavaScript, Python, Go, Java, Kotlin, C#, C, and C++ through their SCIP indexers.

## Continuous integration (GitHub Action)

The composite action at `actions/modula` runs the whole flow in CI: it downloads the prebuilt `cargo-modula` binary, scores the project (auto-detecting the language and running the matching SCIP indexer when the project is not Rust), writes the report to the job summary, and fails the job when the score falls below a threshold you set. When a `MODULA_TOKEN` is present in the environment it also uploads the extracted IR to the portal.

The action downloads a released binary, so a `v*` tag must have been published first (see `.github/workflows/release.yml`). It supports Linux and macOS runners.

Score a Rust crate and require a minimum headline score:

```yaml
jobs:
  modularity:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rust-src, rust-analyzer
      - uses: LucaCappelletti94/modula-rs/actions/modula@v1
        with:
          min-headline: "0.6"
        env:
          MODULA_TOKEN: ${{ secrets.MODULA_TOKEN }}
```

The project must be buildable on the runner, because each language's indexer rides that language's own compiler. Add the usual runtime setup before the modula step. TypeScript and Python index through `npx`, so they need Node:

```yaml
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
      - uses: LucaCappelletti94/modula-rs/actions/modula@v1
        with:
          path: ./my-ts-project
```

Go needs the Go toolchain (`actions/setup-go`), the JVM needs a JDK plus `coursier` (`coursier/setup-action`), C# needs the .NET SDK plus `dotnet tool install --global scip-dotnet`, and C and C++ need `scip-clang` on `PATH` together with a `compile_commands.json` (for CMake, configure with `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`).

### Inputs

| Input | Default | Meaning |
|---|---|---|
| `path` | `.` | Project to score. |
| `version` | `latest` | `cargo-modula` release to download (a tag like `v0.1.0`, or `latest`). |
| `repository` | `LucaCappelletti94/modula-rs` | Repository that publishes the release archives. |
| `min-headline` | (none) | Fail the job if the headline score is below this value (0 to 1). |
| `require-acyclic` | `false` | Fail the job if the module dependency graph has any cycle. |
| `max-overexposed` | (none) | Fail the job if the over-exposed item fraction exceeds this value (0 to 1). |
| `upload` | `auto` | `auto` uploads only when `MODULA_TOKEN` is set, `true` requires it, `false` never uploads. |
| `portal-url` | `https://app.modula.rs/api/v1/ir` (provisional) | Portal endpoint that receives the IR upload. |

The upload is best effort: when the portal is unreachable the step logs a warning rather than failing the job, so a not-yet-available portal never breaks your build. The gate (the `min-headline`, `require-acyclic`, and `max-overexposed` checks) is what fails the job.
