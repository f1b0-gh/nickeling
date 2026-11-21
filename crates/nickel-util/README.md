# nickel-util

Shared Rust primitives for the Nickeling tools.

This library provides generic operations reused by the compilers and controller.
Application models and execution policy belong to its consumers. It has no CLI.

| Module | Purpose |
| --- | --- |
| `files` | Bounded reads and safe file publication |
| `sources` | Source selection, snapshots and fingerprints |
| `json` | JSON parsing and structural checks |
| `process` | Processes with resource limits and interruption handling |
| `evaluation` | Trusted resource preparation and confined Nickel evaluation |
| `error` | Shared errors |

## Usage

The library targets Linux, including its file primitives, which use `O_PATH`
and `/proc/self/fd`.

The package has no CLI feature and does not depend on Clap or any other
Nickeling crate. It is a library, so `cargo install` does not apply.

After version 0.1.0 is published on crates.io, add:

```toml
[dependencies]
nickel-util = "=0.1.0"
```

Before publication, use the Git source pinned to a reviewed full commit:

```toml
[dependencies.nickel-util]
git = "https://github.com/f1b0-gh/nickeling"
rev = "<full-commit-sha>"
```

Cargo fetches the selected source and builds only the selected library and
its dependencies. File, JSON and process operations do not require Nickel or
Nix; the confined evaluation module requires the native evaluation tools.

From the workspace root:

```sh
cargo doc -p nickel-util --no-deps --locked
cargo test -p nickel-util --locked
```

Evaluation requires the native tools listed in the
[development guide](https://github.com/f1b0-gh/nickeling/blob/main/docs/development.md). Using the JSON or file
primitives does not require running Nickel.

## Guarantees and limits

Inputs are bounded; links and paths outside the allowed scope are rejected.
The evaluator separates explicitly trusted Nix resources from confined project
modules. It exposes no network, host secrets or daemon sockets. An incompatible
host must cause a refusal. Each consumer remains responsible for its application
policy and execution permissions.

[Workspace overview](https://github.com/f1b0-gh/nickeling/blob/main/README.md) · [Source](https://github.com/f1b0-gh/nickeling/blob/main/crates/nickel-util/src/lib.rs)
