# Skill — working in memory

*What an agent needs to know to be effective in this repo.*

---

## What memory is

Memory is the **workspace's typed-database kernel**. redb-backed;
values are rkyv-archived; tables are typed and version-guarded.
Each ecosystem layers its own typed tables atop memory in a
state-owning component. A separate typed-table crate is only
warranted when more than one component genuinely shares the same
table layer. Do not assume a broad `persona-memory` component exists.

Memory is to **state** what `signal-core` is to **wire**: the
kernel of typed primitives every consumer's typed layer
depends on.

Read `ARCHITECTURE.md` for the role/boundaries summary: what
memory (the kernel) owns, what each consumer-owned typed Memory layer
owns, and the surface each side has.

---

## Intent

**Memory is meant to become impossible to improve as a storage
kernel.**

> *"I am much more interested in a good design than in producing
> it quickly — criome and memory are meant to be eventually
> impossible to improve, so I value clarity, correctness, and
> introspection above production volume, speed, and time to
> market."*
>
> — Li, 2026-04-29

For memory specifically, this commits the project to:

- **The on-disk format reads cleanly.** A future engineer (or
  agent) opening the redb file and the per-kind table layout
  understands what each table is, why each index exists, what
  invariants the change-log preserves.
- **Every wire shape is typed.** No string-tagged columns, no
  opaque blobs the engine treats as untyped. If a value flows
  through memory, it has a closed Rust type with a derived rkyv
  encoding.
- **State is typed at the table boundary.** The kernel stores
  closed Rust values through typed tables. Identity policy,
  content addressing, history, and bitemporal meaning belong to
  `memory-engine` or the consuming domain, not to this kernel.

When a design choice trades clarity for speed of writing, intent
wins. The right format now is worth more than a wrong format
sooner.

---

## Hard invariants for an agent working here

- **One redb file per consumer.** Each state-bearing component
  opens its own file through its local typed Memory layer;
  cross-consumer sharing is not a thing. The kernel doesn't care
  which file; the consumer decides.
- **Values are rkyv-archived.** No JSON, no string-tagged
  blobs, no untyped bytes. The kernel's `Table<K, V: Archive>`
  enforces this.
- **Schema version is checked at open.** The kernel writes
  the consumer's `Schema::version` on first open and refuses
  to open a file whose stored version doesn't match. Schema
  changes are coordinated upgrades, not silent migrations.
- **Database format is checked at open.** The kernel persists a
  `DatabaseHeader` naming Memory's rkyv format identity
  (little-endian, pointer-width-32, unaligned, bytecheck) and
  refuses to open a database whose stored header mismatches this
  build.
- **Internal tables are namespaced.** Memory-owned redb tables use
  the `__memory_` prefix (`__memory_headers`, `__memory_meta`).
  Consumer table names must not use that prefix.
- **Record types live in Signal contracts or component domain crates,
  not in memory.** The consumer's typed-storage layer references those
  records as values; it owns the table layout, not the records.
- **No raw slot allocation surface.** Append-only identity and
  sequence allocation are engine/domain concepts. They do not live
  in this storage kernel.
- **Typed table scans return owned keys.** redb yields borrowed
  keys for borrowed key types (`&str`, `&[u8]`). Memory's
  `Table::iter` and `Table::range` eagerly collect rows and
  return `OwnedTableKey::Owned`, so callers never hold redb
  guards across the transaction boundary.

---

## What this repo is canonical for

Memory (the kernel) owns:

- The redb file lifecycle (open-or-create + parent mkdir +
  ensure_tables).
- Closure-scoped txn helpers (`store.read(|txn| ...)`,
  `store.write(|txn| ...)`).
- The typed `Table<K, V: Archive>` wrapper — hides rkyv
  encode/decode at the table boundary.
- `Table::ensure` for explicit typed-table materialization in
  consumer schema open paths.
- The standard `Error` enum (5 redb-error variants +
  rkyv + io + schema-version mismatch).
- The version-skew guard and database-format guard.
Memory (the kernel) does not own raw slot allocation, read-pool
configuration, query planning, subscriptions, validation, or
operation logs. Those live in `memory-engine` or the consuming
component.

## Test command surface

Use the repo-local scripts through Nix:

```sh
nix run .#test
nix run .#test-kernel-surface
nix run .#test-no-legacy-surface
nix run .#test-doc
```

`nix flake check` remains the pre-commit gate. The scripts are
for named inner-loop surfaces and are exposed as flake apps so
the pinned Rust toolchain is used.

Memory does **not** own:

- Per-consumer schema, table layouts, or migration
  helpers — those live in the state-owning component's typed Memory
  layer, or in a dedicated crate only when sharing earns it.
- Record types — those live in Signal contracts or component domain
  crates.
- Validator pipelines — those live in the consumer's
  daemon (criome, persona-router, etc.).
- Wire format — `signal-core` + `signal-<consumer>`.
- Artifact bytes — `arca`.

---

## See also

- `ARCHITECTURE.md` — memory's role and boundaries.
- `AGENTS.md` — repo-specific carve-outs.
- `memory-engine` — the engine that executes Signal database verbs
  over this storage kernel.
- signal's `skills.md` — the rkyv types of records.
- arca's `skills.md` — the content-addressed artifact store.
- prism's `skills.md` — memory → Rust projector.
- lore's `programming/abstractions.md`,
  `programming/beauty.md`,
  `programming/push-not-pull.md` — cross-language discipline.
- this workspace's `skills/skill-editor.md` — how to edit and
  cross-reference skills.
