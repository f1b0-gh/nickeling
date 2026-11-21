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

Workspace crates declare a local dependency on the exact version `=0.1.0`.
For external consumers, pin a full source commit or use a versioned source
archive as described in the [development guide](../../docs/development.md#rust-consumers).
The package has no CLI feature and does not depend on Clap.

From the workspace root:

```sh
cargo doc -p nickel-util --no-deps --locked
cargo test -p nickel-util --locked
```

Evaluation requires the native tools listed in the
[development guide](../../docs/development.md). Using the JSON or file
primitives does not require running Nickel.

## Guarantees and limits

Inputs are bounded; links and paths outside the allowed scope are rejected.
The evaluator separates explicitly trusted Nix resources from confined project
modules. It exposes no network, host secrets or daemon sockets. An incompatible
host must cause a refusal. Each consumer remains responsible for its application
policy and execution permissions.

[Workspace overview](../../README.md) · [Source](src/lib.rs)
