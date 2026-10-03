# Symbol resolution

Extraction records what is written: `authorize()`, `self.store.save(bill)`,
`use crate::model::Invoice as Bill`. Resolution decides which definition
each of those refers to, or states why it cannot. The implementation lives
in `crates/analyzer/src/resolver/`.

## Principles

1. **Only statically justified edges.** A `CALLS` edge exists only when one
   definition is determined by name-resolution rules and declared types.
   There is no "closest name" fallback.
2. **Every call site gets exactly one outcome**, and the outcome says why.
3. **Uncertainty is kept, not discarded.** Ambiguous calls keep their
   candidate list; unresolved calls keep their reason. Both are exposed in
   the JSON output and the `--format resolution` report.
4. **Quality is measured** against hand-written ground truth
   (`fixtures/*/expected.json`) with precision and recall per category.

## Outcomes

| Outcome | Meaning | Edge? |
|---|---|---|
| `resolved` | Exactly one repository definition. | `CALLS` |
| `ambiguous` | Several repository definitions are possible (see reasons). | candidates listed |
| `unresolved` | The target may be in the repository but could not be determined. | — |
| `external` | The target is outside the repository: standard library, dependencies, derived methods. | — |
| `constructor` | Tuple-struct or enum-variant construction (`Meters(1)`, `Shape::Circle(r)`). | — |
| `local` | Call of a locally bound closure or function value (`let f = \|x\| ..; f(1)`). The closure's own calls are attributed to the enclosing function. | — |

The **resolution rate** is `resolved / (resolved + ambiguous + unresolved)`:
of the call sites that may target repository code, the share pinned to a
single definition. External calls, constructors and local calls are
excluded because there is no repository target to find.

Ambiguity reasons: `multiple_definitions` (e.g. `#[cfg(unix)]` and
`#[cfg(not(unix))]` versions of one function), `receiver_type_unknown`,
`foreign_trait_method` (every candidate implements a trait from outside the
repository, such as `Iterator::next`; the call most likely targets a
standard-library type), and `generic_parameter` (the receiver's type is a
generic parameter, so any implementation of its bound may run).

Unresolved reasons: `name_not_in_scope` (a repository function has the name
but nothing brings it into scope, typically `#[cfg]`-disabled code),
`path_segment_not_found`, `self_type_unknown`, `dynamic_call` (calls
through closures stored in fields or other expressions).

## Pipeline

1. **Module tree** (`module_tree.rs`). `mod name;` declarations are
   followed from each crate root, including `#[path = "..."]`. This fixes
   cases a path-based mapping gets wrong: `tests/util.rs` declared by
   `tests/tests.rs` is `tests::util`, not its own crate; `src/cli.rs`
   declared only by `main.rs` belongs to the binary.
2. **Symbol table** (`table.rs`). Indexes children by parent and name,
   imports and local bindings by scope, struct field types, type aliases,
   library crate roots and dependency names from every `Cargo.toml`.
3. **Re-homing** (`rehome.rs`). Impl methods are moved under their self
   type once the impl header's path resolves, so
   `impl crate::model::Invoice { fn discounted() }` written in
   `services/billing.rs` becomes `model::Invoice::discounted`. The method
   keeps resolving names lexically in the module its impl is written in.
4. **Trait impls.** `impl Trait for Type` yields `IMPLEMENTS` edges from the
   type to the trait and from each implementing method to the trait method.
5. **Imports**, then **calls**, are classified.

## Name resolution rules

Modelled on Rust 2018+:

* A single name is looked up in the innermost scope first (nested function
  bodies, then the enclosing function), stopping at the first module.
  Modules do not inherit names from their parents.
* Within a scope: items shadow explicit `use` imports, which shadow glob
  imports. Local `let` / parameter / closure bindings shadow items for
  call purposes.
* Types and modules live in the type namespace, functions in the value
  namespace; intermediate path segments are always in the type namespace.
  `use` items bind both namespaces.
* Paths may start with `crate`, `self`, `super` (repeatable), `Self`, a
  library crate of the repository, or `::crate_name`.
* Visibility: accessing module `m` from outside only sees `pub` (or
  `pub(crate)`, …) imports of `m`; from inside `m` or its descendants,
  including through `use super::*`, private imports are visible too.
* `extern crate a as b;` binds `b` like an import. `type A = B;` makes `A`
  resolve to `B`.
* Associated functions follow Rust's priority: inherent methods, then
  methods from trait impls for the type, then default methods of traits the
  type implements.

A path whose root is `std`/`core`/`alloc`, a declared dependency, a common
prelude type (`Vec`, `String`, `HashMap`, …), a name bound by an import of
an external item, or a name that no repository module or type has, is
**external**. A path that leaves the repository through a `pub use` of an
external item or a type alias of an external type is also external.

## Receiver-type inference

Method calls are resolved through the receiver's type when it can be
determined locally:

| Receiver | Source of the type |
|---|---|
| `self` | the method's owning type (or trait, in default methods) |
| `self.field` | the declared field type |
| parameter, `let x: T`, typed closure parameter | the written type |
| `let x = Type::new(..)` / `let x = f(..)?` | the callee's declared return type; `Self` maps to the callee's owner; `?` unwraps `Result<T, _>` / `Option<T>` |
| `let x = Type { .. }`, `vec![..]`, `format!(..)` | the literal's type, `Vec`, `String` |
| `a.b(..).c(..)` | the declared return type of each repository method in the chain |

References, `Box`, `Rc`, `Arc`, `dyn Trait` and `impl Trait` are looked
through, since method calls auto-deref. A trait-typed receiver resolves to
the trait method; the `IMPLEMENTS` edges then lead to implementations.

Return types of standard-library methods are not modelled, so a chain that
passes through one (`self.map.get(k).unwrap().run()`) stays unknown. If the
receiver type stays unknown, the call is **ambiguous** with every
repository method of that name as candidates, unless no repository method
has that name, in which case it is external.

## Measured quality

### Fixtures (ground truth)

`codeatlas evaluate fixtures/<name>` compares output with the hand-written
`expected.json`; `cargo test` fails on any difference.

| Fixture | Calls (expected) | Implements | Ambiguous | Unresolved | Result |
|---|---|---|---|---|---|
| simple-repo | 8 | 0 | 0 | 0 | exact |
| duplicate-symbols | 4 | 2 | 1 | 0 | exact |
| cross-module | 15 | 3 | 1 | 0 | exact |
| unresolved | 0 | 4 | 2 | 3 | exact |
| change-impact | 11 | 6 | 0 | 0 | exact |
| circular-dependency | 7 | 0 | 0 | 0 | exact |

These fixtures were written alongside the resolver, so an exact match shows
the rules behave as designed. It is not evidence of accuracy on unfamiliar
code.

### ripgrep (no ground truth)

One run of `codeatlas analyze <ripgrep> --format resolution` on ripgrep
commit `3fce3b5` (110 files, 50,953 non-blank lines), macOS on Apple
Silicon, release build:

| | Call sites |
|---|---|
| resolved | 9,248 |
| ambiguous | 2,873 (receiver type unknown 2,566; foreign trait method 250; generic parameter 29; multiple definitions 28) |
| unresolved | 6 (5 dynamic calls, 1 pattern-bound variable) |
| external | 6,225 |
| constructors / local | 353 / 330 |
| **resolution rate** | **76.3%** |

To check the precision of the largest strategy, 30 `receiver_type` edges
were drawn at random (seed 7) and checked by reading the source; all 30
were correct, including one that depends on a type alias
(`type Range = Match;`). A 30-edge sample bounds precision only loosely.
A systematic precision/recall measurement against compiler-derived ground
truth is planned with the benchmark suite.

## Known limitations

* No general type inference: element types of iterators and `for` loops,
  `match` / `if let` bindings, generic instantiation and standard-library
  return types are not modelled.
* Macro-generated items are invisible; calls to them are classified as
  external.
* `#[cfg]` is not evaluated; every configuration is analysed and duplicates
  become `multiple_definitions` ambiguity.
* Trait-method calls on trait-typed receivers resolve to the trait
  declaration, not to a specific implementation.
* Bindings are flow-insensitive within a function: the latest binding of a
  name at or before the call line is used, regardless of block scope.
* Glob imports of enums (`use Enum::*`) are recorded but variants are not
  symbols.
