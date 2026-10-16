# switchboard-macros

`route!` validates a literal route path at compile time and creates typed path
parameters. The Switchboard library reexports the macro as `switchboard::route`.
The macro crate has no production dependency on the Switchboard library.

```rust
use switchboard::{DefinitionError, route};

fn definitions() -> Result<(), DefinitionError> {
    let items = route!(get, "/items/:itemId", "user", String)?;
    assert_eq!(items.definition.path, "/items/:itemId");

    let create = route!(
        post,
        "/items/:itemId/:slug?",
        "user",
        String => String,
        prefixes = ["hu", "de"],
    )?;
    assert_eq!(create.definition.prefixes, ["hu", "de"]);
    Ok(())
}
```

The expression returns `Result<TypedRoute<Params, Body, Output, Permission>,
DefinitionError>`. Each invocation declares its own block-local `Params` struct.
The no-body methods `get`, `delete`, `head`, and `options` use `Body = ()` and
accept only `OutputType`. The body methods `post`, `put`, and `patch` require
`BodyType => OutputType`. Permissions are arbitrary Rust expressions evaluated
once. An optional final comma is accepted.

Path segments beginning with `:` generate public fields:

| Path segment | Rust field type |
| --- | --- |
| `:itemId`, `:item_id` | `f64` |
| `:slug` | `String` |
| `:itemId?`, `:item_id?` | `Option<f64>` |
| `:slug?` | `Option<String>` |

IDs use `f64` to match the reference's numeric schema, including fractional
values. Use an explicit typed struct for integer or constrained ID contracts.
Field spellings retain the path name, including camelCase. Keyword fields use
raw identifiers, for example `:type` becomes `params.r#type`. Serde names always
retain the original path name.

```rust
use switchboard::route;

let keywords = route!(get, "/:type/:gen?", (), String)?;
// A typed handler's inferred params expose `params.r#type` and `params.r#gen`.
# Ok::<(), switchboard::DefinitionError>(())
```

The generated struct derives `Serialize`, `Deserialize`, and `JsonSchema` through
Switchboard's `__private::{serde, schemars}` reexports. Both derives receive
explicit crate-path attributes. Consumers need no direct serde or schemars
dependencies for inferred params. Renaming the dependency in Cargo.toml is
supported:

```toml
[dependencies]
sb = { package = "switchboard", path = "../switchboard/rust" }
```

```rust
use sb::route;
let items = route!(get, "/items/:itemId", "user", String)?;
# Ok::<(), sb::DefinitionError>(())
```

The macro rejects mandatory parameters and static segments after an optional
segment. It follows the reference's segment ordering, including allowing one
trailing slash. It rejects duplicate or empty parameter names, names that cannot
be Rust fields (such as `:item-id`), and `self`, `Self`, `super`, or `crate`, which
Rust cannot use as raw field identifiers. Names starting with `r#` are also
rejected. Use raw `RouteDef` or an explicit `TypedRoute` params struct with serde
renames for these paths. A constant, expression, or `concat!` path cannot supply
compile-time inference; use an explicit typed route instead.

Prefixes must be string literals without a leading slash or any colon. Empty
prefixes and multi-segment static prefixes retain the runtime API's behavior.
The original canonical route string and prefixes are passed unchanged to
`TypedRoute::new`. Runtime schema derivation and definition validation still run
and may return `DefinitionError`. Compile-time path checking does not replace
request parsing, authorization, output validation, or router registration.

The inferred struct is local to its invocation and cannot be named elsewhere.
Use an explicit typed struct when consumers need a reusable named parameter
type or query fields beyond those present in the path. This macro provides no UI,
hydration, browser runtime, or complete reference-parity guarantee.

## Validation

```sh
cargo check --manifest-path rust/macros/Cargo.toml --locked
cargo test --manifest-path rust/macros/Cargo.toml --locked
cargo fmt --manifest-path rust/macros/Cargo.toml --check
cargo clippy --manifest-path rust/macros/Cargo.toml --all-targets --locked -- -D warnings
```

Unit tests cover grammar, field inference, canonical paths, prefixes, keywords,
reference optional ordering, and generated derive paths. Trybuild fixtures check
actual compiler diagnostics without a cyclic library dependency. Compile-pass
integration with the real `TypedRoute`, including renamed consumer dependencies,
is owned by the parent library's integration gate.
