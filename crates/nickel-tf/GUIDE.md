# nickel-tf

Compile Nickel configuration into OpenTofu `.tf.json`, using contracts generated
from frozen provider schemas. `nickel-tf` works independently of
the Kubernetes compiler and the controller. The library has no CLI dependency when
built with `--no-default-features`.

This is a focused rework of [tf-ncl](https://github.com/nickel-lang/tf-ncl): direct
Rust schema conversion, explicit deferred expressions, confined evaluation and
input locks. It does not use tf-ncl's Go schema merger or copy its unconditional
removal of empty records. Empty resources and empty map values remain intact.

## Local example

Run from the workspace root. The pinned flake selects Nickel 1.18.0 and
OpenTofu 1.13.2 on x86_64 Linux. Nix, Bubblewrap and `prlimit` must be available,
with unprivileged user namespaces enabled.

```sh
NICKELING_ROOT=$(pwd -P)
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-tf#nickel \
  --no-link --no-update-lock-file --no-write-lock-file
TF_RESOURCES="$NICKELING_ROOT/crates/nickel-tf"
TF_DEMO=$(mktemp -d)
cargo run -p nickel-tf --locked -- import-schema \
  --schema "$TF_RESOURCES/examples/local/schema.json" \
  --selection "$TF_RESOURCES/examples/local/selection.json" \
  --output "$TF_DEMO/bundle"
cargo run -p nickel-tf --locked -- check \
  --config "$TF_RESOURCES/examples/local/main.ncl" \
  --resources "$TF_RESOURCES" --bundle "$TF_DEMO/bundle"
cargo run -p nickel-tf --locked -- lock \
  --config "$TF_RESOURCES/examples/local/main.ncl" \
  --resources "$TF_RESOURCES" --bundle "$TF_DEMO/bundle" \
  --output "$TF_DEMO/config.lock.json"
cargo run -p nickel-tf --locked -- render \
  --config "$TF_RESOURCES/examples/local/main.ncl" \
  --resources "$TF_RESOURCES" --bundle "$TF_DEMO/bundle" \
  --lock "$TF_DEMO/config.lock.json" --output "$TF_DEMO/main.tf.json"
```

This produces configuration without running OpenTofu. The fresh output directory
avoids overwriting earlier results. Bundle import refuses an existing destination;
`render` and `lock` require `--replace` to replace files. Omit `render --output`
for stdout. The example uses built-in `terraform_data`, without an external
provider or cloud account.

To validate the generated configuration and inspect a native plan, continue
explicitly with OpenTofu in the same shell:

```sh
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-tf#opentofu \
  --no-link --no-update-lock-file --no-write-lock-file
TOFU_BIN="$(nix --extra-experimental-features 'nix-command flakes' eval --raw \
  --no-update-lock-file --no-write-lock-file path:./crates/nickel-tf#opentofu.outPath)/bin/tofu"
"$TOFU_BIN" -chdir="$TF_DEMO" init -backend=false
"$TOFU_BIN" -chdir="$TF_DEMO" validate
"$TOFU_BIN" -chdir="$TF_DEMO" plan
```

These native commands initialize a disposable working directory and prepare a
plan; they do not apply it.

## Provider schemas and pins

Provider acquisition is an explicit preparation step in a separate OpenTofu
working directory. Select exact versions, run `tofu init`, then export:

```sh
tofu providers schema -json > schema.json
```

Supply a separate `selection.json` recording the engine and **provider release**
versions, not resource schema versions:

```json
{
  "engine": {"name": "opentofu", "version": "1.13.2"},
  "providers": {
    "random": {
      "source": "registry.opentofu.org/hashicorp/random",
      "version": "3.9.1"
    }
  }
}
```

The version above is illustrative; use the exact release installed in your
preparation directory. The built-in provider uses source
`terraform.io/builtin/terraform` and version `null`; its version follows OpenTofu.
All selected sources must match the exported schema catalog. Each resource/data
block is bound to the local provider name that owns its schema. If that name
differs from the resource type prefix, the compiler supplies the `provider`
meta-argument; an explicit reference to another schema owner is rejected. Keep the generated
bundle outside the directory containing your entry `.ncl` file: that directory
is the confined project module tree, subject to its own module and naming limits.

```sh
nickel-tf import-schema --schema schema.json --selection selection.json \
  --provider-lock .terraform.lock.hcl --output provider-bundle
```

External providers require a native `.terraform.lock.hcl`. Its bytes are preserved
in the bundle and included in the configuration receipt. Copy that native lock
alongside the rendered JSON and use `tofu init -lockfile=readonly`. OpenTofu owns
native lock syntax, provider checksum verification and installation. The compiler
emits exact version constraints but does not parse HCL locks or prove that an
export was produced by the asserted provider binary. Review schema provenance
when preparing bundles; bundle hashes detect later drift, not authenticity.

## Values and expressions

```nickel
let tf = import "/libraries/nickel-tf/expressions.ncl" in
{
  resource.terraform_data.example.input = {
    literal = "$" ++ "{this_is_text}",
    expression = tf.expr "upper(var.name)",
    reference = tf.ref ["terraform_data", "other", "output"],
  },
}
```

Ordinary strings are literal. In expression positions the renderer escapes HCL
`${...}` and `%{...}` template sequences. `tf.expr` and `tf.ref` explicitly defer
a value to OpenTofu. `tf.ref` accepts identifier path segments; use `tf.expr` for
indexes or more complex syntax. `tf.literal value` preserves data containing the
reserved `__nickel_tf` marker. Marker objects must have exactly the documented
expression/literal shape. Static labels, descriptions and variable defaults have
separate rendering rules. OpenTofu validates the syntax and result types of HCL
expressions during validate/plan; an offline check cannot establish those.

## Scope and boundaries

The initial compiler supports provider/resource/data schemas, primitive and
collection/object/tuple types, required and optional attributes, optional-computed
attributes and nested blocks. Computed-only attributes cannot be configured.
Core support covers `resource`, `data`, `provider`, `variable`, `locals`, `output`
and exact engine/provider constraints in `terraform`. Unsupported core fields,
provider schema shapes and nesting modes fail explicitly. Nested map-block labels
containing HCL template delimiters are rejected because they are static block
labels, while ordinary attribute map keys are expression positions. Modules, remote
backends, provisioners, dynamic blocks and infrastructure execution are outside
this initial version.

`import-schema`, `check`, `render` and `lock` do not run OpenTofu, provider plugins,
project commands or network resolution. Nickel runs through the existing
confined `nickel-util` evaluator. Only selected public recipes reach the Nix
store; project configuration, provider locks and schema inputs are staged
separately. The `--resources` directory is explicitly trusted code.

Bundles store the schema, selection, native lock and deterministic generated
contracts. Before evaluation the compiler checks hashes and regenerates contracts
to reject weakened generated code. Input files are limited to 16 MiB and depth
64. All libraries share a budget of 2048 modules and 64 MiB, with 1 MiB per
module. Import reserves the three bundled helper modules and their bytes, allowing
at most 2045 generated modules. Custom trusted resource libraries may consume
more of the evaluator budget. Symlink
traversal is refused and bundle publication is atomic without replacement.
Configuration locks cover the staged project modules, libraries, schema bundle,
evaluator recipes and rendered output digest. They contain hashes, not evaluated
configuration values. Rendered configuration may contain secrets supplied by you;
OpenTofu manages state and its protection separately.

## Verification

Use the pinned tool preparation and workspace checks in the
[development guide](../../docs/development.md#required-checks), from the workspace root.

The OpenTofu integration tests use disposable local state and apply only
built-in `terraform_data` and local Random resources to check actual expression
results. They block HTTP(S) access and do not deploy cloud infrastructure.
`examples/random` uses Random 3.9.1 from the pinned Nix package under the local
name `entropy`, exercising explicit binding of schemas to selected providers. Its native lock
contains the checksum of that Nix wrapper on Linux x86_64, not an official registry
download; use the matching `path:./crates/nickel-tf#opentofu-local` package with this fixture. Generate
your own native lock when using providers installed from their registry. Provider schema and JSON behavior follow the
[OpenTofu schema format](https://opentofu.org/docs/cli/commands/providers/schema/)
and [JSON syntax](https://opentofu.org/docs/language/syntax/json/).

## Reusable compositions

`nickel/composition.ncl` provides `define Input build` for typed Nickel functions
and `compose {componentName = configuration, ...}` to assemble named components.
Resource/data types merge by type and instance name; other blocks merge by name.
Duplicate identities are rejected even when their values are equal. Native
OpenTofu expressions still belong in `expressions.ncl` and resolve in OpenTofu.

```nickel
let c = import "/libraries/nickel-tf/composition.ncl" in
let settings = c.define {message | String} (fun p => {
  resource.terraform_data.settings.input = p.message,
}) in
c.compose {settings = settings {message="hello"}}
```

Rust callers can use `check_value` for already evaluated JSON; it uses the same
confined evaluator, schema contracts and receipt as source compilation. Execution
and bindings across units belong to [nickel-plane](../nickel-plane/README.md).
The existing `check`, `render` and `lock` commands remain offline.
