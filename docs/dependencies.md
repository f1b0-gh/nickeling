# Dependency maintenance

Aim for one compatible resolved version per package identity, source and
maintained resolution context. Shared Rust declarations belong in
`[workspace.dependencies]`, with `workspace = true` in consumers. Preserve
features, optionality, `no_std` where applicable, build/dev dependencies,
target conditions, local paths and existing forks.

Consumer, Nickeling and Nix lockfiles remain independent. Matching version
numbers do not make sources interchangeable. Exact pins require explicit,
reviewed updates; they establish neither compatibility nor security. This policy
does not claim that the current dependency graph is aligned or vulnerability-free.

## Before changing a dependency

Look for an existing solution and reuse shared declarations. Inventory all
maintained manifests, locks, sources, versions, resolution contexts and parent
dependency paths before adding, upgrading, downgrading or aligning a dependency.
Update affected manifests and locks together, using their native tools rather
than manual lockfile edits or scanner-directed automatic fixes.

Prefer a compatible resolution or a maintained upstream parent after reviewing
APIs, MSRV, behavior, security defects, platforms and maintenance cost. Never
merge incompatible features or replace a source merely to improve a count.

For a reduction, report the expected net change in package count, multiversion
families and excess versions, together with additions, affected consumers and
recurring cost. Removing one declaration does not remove a package retained by
another parent. An update without measurable benefit is not a reduction;
a security fix may still be necessary independently of those counts.

Do not introduce forks, wrappers, adapters or modified local copies just to
reduce divergence. An existing fork does not justify unlimited expansion into
transitive dependencies. Record its owner, cumulative patch cost, bounded tests
and condition for returning upstream. Unmodified vendored inputs for reproducible
builds are distinct from maintained forks.

## Detect new divergence

In review and CI, compare exact version sets by package identity, source and
resolution context against a verified baseline with a versioned fingerprint.
A numeric ceiling is insufficient: every new divergence or new version within
an already divergent family requires review, even at an unchanged count.
Do not refresh baselines or exceptions merely to make a check pass.

Keep inherited debt in the backlog; it is not implicitly approved. Every
intentional exception records an owner, exact versions and sources, scope,
reason, evidence, maintenance cost and expiration. Renewal requires explicit
review. Do not hide platforms, suppress findings implicitly or change the
scope to claim non-regression. Incomplete coverage or changed scope blocks that
conclusion. If the CI check is not wired up or qualified, report it as remaining
work; these instructions are not evidence of execution.

## Review current security advisories

Check candidate versions and sources, then final lockfiles, against current
advisories for each relevant ecosystem: RustSec for Cargo, GitHub Advisory
Database for npm including TypeScript tools, the PyPA/PyPI advisory database for
Python, and Go Vulnerability Database for Go. Use a maintained equivalent for
other ecosystems. An OSV feed must retain original advisory IDs and provenance.

Cover direct, transitive, build, development and optional dependencies on every
platform, including Windows, even when inactive locally. Review native versions
supplied by Nix, wrappers and bindings separately: fixing one layer does not
qualify the others. Do not introduce or reintroduce a known affected version to
improve alignment. Retain existing findings in the backlog.

For each check, retain the advisory source, database revision or digest,
retrieval time, tool version, effective configuration, filters, suppressions,
and exact package versions and sources examined. Missing, stale or unsupported
coverage remains incomplete and blocks a favorable conclusion. An advisory
match is a finding to assess, not proof of exploitability.

## Validate and stop unproductive work

Keep initial and final advisory evidence separate from declaration and lockfile
checks. Verify affected consumers' behavior and refusals in the relevant
profiles using pinned native tools and the
[development checks](development.md#required-checks). Compilation and lower
version counts alone are insufficient; record passed, failed, blocked and
deferred checks.

Reuse evidence whose relevant inputs have not changed. Stop a path with no
substantiated gain or disproportionate cost; record the blocker and a concrete
trigger for resuming it. Do not rerun unchanged tests without a reason.

Tool preparation and advisory retrieval are explicit steps separate from static
validation. Static validation does not implicitly authorize network access,
installation or project-code execution. Existing compiler and evaluator
boundaries continue to apply.
