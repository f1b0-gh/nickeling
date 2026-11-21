# Working on Nickeling

- Use English for code, documentation and reports. Read `README.md` first;
  read `docs/extraction.md` before migrations or distribution changes.
- Keep discussions, plans and working notes in `.local/`; never commit them.
- Keep application logic in Rust, configuration in Nickel and builds in Nix.
  `nickel-util` stays domain-independent; `nickel-k8s` and `nickel-tf` stay
  independent. `nickel-plane` uses optional adapters. Library consumers use
  `default-features = false` to omit Clap.
- Fail closed: reject unknown inputs, symlinks and out-of-scope paths; enforce
  resource budgets before allocation or publication. Separate trusted Nix
  resources from confined, networkless project evaluation. Expose no raw host
  secrets or daemon sockets; never weaken confinement to pass a test.
- Compiler `check`, `render` and `lock` commands must not run project programs,
  OpenTofu or providers. Controller execution uses explicit adapters.
- Preserve state, locks, exports and ownership. No implicit adoption, migration
  or deletion. Test refusals, interruptions, limits and hostile paths with fixtures.
- Preserve versions and locks unless explicitly changing them. Follow
  [dependency policy](docs/dependencies.md) for every dependency change.
- Follow [development checks](docs/development.md#required-checks), including
  libraries without default features. Use pinned native tools, at most two Cargo
  jobs and build outputs outside Nix source trees. Reuse unchanged evidence;
  report failed, blocked or incomplete checks without overstating coverage.
- Keep internal paths repository-relative and external sources explicitly
  versioned. Preserve unique code and inputs; follow the
  [retention rules](docs/development.md#experiment-retention) before cleanup.
  Cleanup eligibility is not permission to delete active or unselected work.
