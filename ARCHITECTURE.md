# ARCHITECTURE — memory

Memory is the workspace's typed storage kernel. It is a Rust
library over redb and rkyv. It opens a database file with an
explicit schema, guards the rkyv database format, and provides
typed table operations inside closure-scoped transactions.

Memory is to state what `signal-core` is to wire: the small kernel of
primitives that higher layers build on. Full database-operation
execution lives in `memory-engine`, not in this crate.

The persistent typed-record store is named the **MEMORY database** — not
RAD or other nicknames. Memory is the workspace's pure binary domain-tree
format; the redb-persisted records are conceptually a memory database. It
holds strings (quotes, names, summaries) today, and those reduce as
natural language becomes typed leaves. Internal database logic uses the
same schema-defined message language that component signals use, so a
growing database component can later split into its own daemon without
changing the language pattern.

> **Scope.** Today's `memory` is the Rust-on-redb storage kernel.
> The eventually-self-hosting `Memory` substrate (the universal
> medium for meaning, Memory-on-Memory) is a different artifact at a
> different layer of the stack: it is what makes computation
> itself content-addressable. Today's `memory` is a realization
> step. The eventual versioning model — schema as content-
> addressed Memory source, components carrying multiple versions
> in runtime, translation as reducer work — is named in the
> Versioning section below as **future work, not first-prototype
> work**. See `~/primary/ARCHITECTURE.md` §"Workspace vision and intent".

## Role

The kernel owns:

- The redb file lifecycle: create parent directories, open or create
  the file, create memory-owned internal tables.
- The database header guard: the file records Memory's rkyv format
  identity and refuses incompatible builds.
- The schema-version guard: `Memory::open_with_schema(path, schema)`
  writes the schema on first open and hard-fails on mismatch.
- Closure-scoped transactions: `read(|transaction| ...)` and
  `write(|transaction| ...)`.
- Public transaction type aliases: consumers name `memory::ReadTransaction`
  / `memory::WriteTransaction`, not `redb` transaction types directly, so
  higher layers can type local table reducers without adding their own
  `redb` dependency.
- Typed tables: `Table<K, V>` hides rkyv encode/decode at the table
  boundary and returns owned rows from scans.
- The crate `Error` enum for kernel failures.

Consumers own:

- Record Rust types, usually in Signal contract crates or component
  domain crates.
- Table layouts and schema constants.
- Runtime ordering, actors, authorization, validation, subscriptions,
  and commit-before-effect policy.
- Database-operation execution through `memory-engine`.

## Boundary

```text
component daemon
  owns actors, policy, validation, subscriptions, sockets
  |
  v
memory-engine
  owns Signal verb execution, query/mutation plans, catalog,
  operation log, snapshots, subscription delivery
  |
  v
memory
  owns redb/rkyv typed table storage, schema guard,
  database-format guard, transaction helpers
  |
  v
component.redb
```

The dependency direction is one-way. `memory-engine` may depend on
`memory`. `memory` must not depend on `memory-engine`, Signal contracts,
Kameo, tokio, NOTA, Nexus, Persona, or Criome.

## Non-Goals

Memory does not own:

- Signal verbs or request routing.
- Query planning, mutation planning, validation, operation logs,
  snapshots, or subscriptions.
- Component actors or mailboxes.
- Runtime configuration for any component.
- Raw untyped record storage.
- Peer inspection or daemon sockets.

The retired raw-byte append path is intentionally absent. If a future
engine needs append-only identity or sequence allocation, that lands
as a typed `memory-engine` primitive with its own records and witnesses,
not as a raw storage surface in `memory`.

## Constraints

- Memory opens durable state only through
  `Memory::open_with_schema(path, schema)`.
- Memory stores typed rkyv values through typed tables.
- Memory has no schema-less public open path.
- Memory has no raw byte store API.
- Memory has no Criome read-pool configuration API.
- Memory internal table names use the `__memory_` prefix.
- Memory-owned internal tables are limited to kernel metadata and
  database headers.
- Component table names must not use the `__memory_` prefix.
- Table layout belongs to the component that owns the state.
- Record Rust types live in Signal contract crates or component
  domain crates, not in memory.
- Runtime ordering, actor mailboxes, commit-before-effect policy, and
  subscriptions belong to the consuming component or to
  `memory-engine`.

## Public Surface

```rust
pub struct Memory;

impl Memory {
    pub fn open_with_schema(path: &Path, schema: &Schema) -> Result<Self>;
    pub fn read<R>(&self, body: impl FnOnce(&ReadTransaction) -> Result<R>) -> Result<R>;
    pub fn write<R>(&self, body: impl FnOnce(&WriteTransaction) -> Result<R>) -> Result<R>;
    pub fn path(&self) -> &Path;
}

pub struct Table<K, V> { ... }

impl<K, V> Table<K, V> {
    pub const fn new(name: &'static str) -> Self;
    pub fn ensure(&self, transaction: &WriteTransaction) -> Result<()>;
    pub fn get(&self, transaction: &ReadTransaction, key: K) -> Result<Option<V>>;
    pub fn insert(&self, transaction: &WriteTransaction, key: K, value: &V) -> Result<()>;
    pub fn remove(&self, transaction: &WriteTransaction, key: K) -> Result<bool>;
    pub fn iter(&self, transaction: &ReadTransaction) -> Result<Vec<(K::Owned, V)>>;
    pub fn range<R>(&self, transaction: &ReadTransaction, range: R) -> Result<Vec<(K::Owned, V)>>;
}
```

## Deletion durability — copy-on-write page reuse

redb is a copy-on-write B-tree. `Table::remove` — and any `write`
transaction that drops or overwrites a key — does not erase bytes in
place. It commits a new tree and marks the old pages free. A removed
record's rkyv-archived bytes linger in those freed pages **only until a
later write transaction reuses them**; once subsequent commits reclaim
the pages, the bytes are overwritten and the record is gone from the
live file.

So at the kernel layer, **removal is irreversible**. There is no
undelete, no tombstone, and no history beyond redb's single
last-committed / in-progress page pair. A consumer that might need a
removed record back must capture it *before* calling `remove` — the
kernel keeps nothing.

This was confirmed empirically against a memory consumer (the
persona-spirit intent store). Records removed one morning had their
freed pages fully reclaimed within hours by ~74 later record writes: a
forensic `strings` / byte scan of a read-only copy of the live database
found no trace of the removed records' distinctive text, while every
live record's text was still present. A type-aware (rkyv) scan reads
the same overwritten pages and cannot do better — page reuse has
already destroyed the content.

Consequences for the layers above:

- `memory-engine`'s `Retract` verb inherits this: a retracted record is
  unrecoverable from the file once later commits reclaim its pages.
- Components whose records carry durable meaning (intent logs, audit
  trails) must treat removal as destructive and capture-before-remove,
  never relying on forensic recovery.
- Recoverable deletion, where wanted, is a deliberate higher-layer
  feature — a typed `memory-engine` tombstone/archive record, or a
  transaction-coherent copy via redb's backup API or a filesystem
  copy-on-write snapshot — never an implicit property of the store.

The higher-layer direction this serves is **archival lowering before
hard deletion**: a component's data lifecycle prefers marking stale or
low-certainty data and moving it out of the hot working DB before
destroying it, keeping runtime state manageable. Recoverability is
best-effort unless a stronger retention class applies. When a record
moves to the archive its privacy variable moves with it, preserved at
the original privacy level, and archive reads honor the same explicit
privacy discipline as the live store. The archive itself is a
specialized memory-database holding exactly one kind of archived object —
the database name states what it archives — paired with a small
archive-retrieving tool. `memory-engine`'s `CollectRemovalCandidates`
archives the records it collects to a default archive surface before
hot-store removal, with typed retrieval rather than ad-hoc caller files;
when composite intent retires its source records, the sources are
archived and referenced by hash identity from the composite so
provenance survives. This is an engine-layer feature the kernel makes
possible, not kernel behavior.

## Code Map

```text
src/
└── lib.rs    — Memory handle, schema/database header guards,
                closure-scoped transactions, Table<K, V>,
                OwnedTableKey, Error
```

Internal Memory tables:

- `__memory_headers`
- `__memory_meta`

## Tests

Named Nix surfaces:

```sh
nix run .#test
nix run .#test-kernel-surface
nix run .#test-no-legacy-surface
nix run .#test-doc
nix flake check
```

Load-bearing witnesses:

- `memory_does_not_export_slot`
- `memory_does_not_export_legacy_slot_store`
- `memory_does_not_export_reader_count`
- schema mismatch hard-fails at open
- database format mismatch hard-fails at open
- typed table scans return owned keys and values
- write transactions roll back on typed errors

## Status

Package A of the memory / memory-engine split has **landed**: this crate
is the cleaned storage kernel (no `Slot`, no legacy raw-byte store,
no `reader_count`, no schema-less open). The structural witnesses
for those deletions exist. `memory-engine` has been created as a
sibling library-only repository and is in active development; the
first consumer migration (persona-mind) is in flight.

Ongoing work for this crate is bounded: respond to engine-side
discoveries that require kernel changes (per `~/primary/ARCHITECTURE.md`
§"Workspace vision and intent" — `memory` may break to make the
engine substrate beautiful). Most active development lives in
`memory-engine`.

## Versioning — today and eventually

Today, schema versioning is **manual**: `Memory::open_with_schema`
takes an explicit `SchemaVersion` constant; the file records that
version on first open and hard-fails on subsequent mismatches.
Consumers bump the constant when their typed tables change shape,
and migration is a coordinated rebuild (delete the old database,
recompile the consumer against the new schema, accept the data
loss or run a one-off migrator). The format-identity guard is
separate and protects against rkyv layout drift across builds.

Eventually, when the workspace self-hosts on Memory-on-Memory (per
`~/primary/ARCHITECTURE.md` §"Workspace vision and intent"), versioning becomes
**content-addressed**:

- A schema is identified by the hash of its Memory source. Equal hash
  ⇒ equal schema by construction; no separate `SchemaVersion`
  constant is needed.
- A component's runtime can hold **multiple schema versions
  concurrently**. Records archived under v3's hash decode through
  v3's typed shape; records archived under v4's hash decode through
  v4's. The catalog row carries the schema-hash, not a manually-
  assigned version number.
- Migration becomes a **reducer**: a typed Memory function from
  v3-records to v4-records. The reducer runs over the v3 archive
  to produce a v4 archive; both can coexist in the same store
  under different schema-hash addresses until the v3 archive is
  retired.

This is **future work**, not first-prototype work. The current
manual-`SchemaVersion` mechanic is the realization step for it.
The eventual model retires this section's first paragraph, not
the kernel itself.

### Content-addressed migration — the engine-layer model

The content-addressed model above resolves into a concrete migration
design at the `memory-engine` / contract layer that the kernel serves.
These are engine-layer facts, recorded here because they fix what the
kernel's typed-table storage must support:

- **Schema-layout schema as identity.** Each persona contract carries
  an explicit content-addressable schema-layout schema in a NOTA-based
  language. Its hash is its identity, so any edit changes the address —
  the address is the version. A version-checking pipeline detects
  schema-address mismatches between code and stored data, walks the
  diff, and derives per-type migration operations baked into how types
  are written. `MigrationIndex` is the runtime decoder lookup; migration
  is per type; upgrade and downgrade are one compatibility-projection
  relation, not two mechanisms.
- **Upgrades are typed MEMORY operations.** A protocol or
  database-format change is itself a typed operation/message applied as
  database work, and that same operation is the source for derived
  datatype and upgrade/compatibility code. One library handles
  record-level operations and schema-changing operations alike. Diff
  operations come in three families — Add, Remove, Modify — where Modify
  subdivides into ContainerEmbed, EnumWrap, Reorder, and KeyChange.
- **rkyv headroom enables zero-cost changes.** rkyv's storage layout
  reserves more namespace than data needs: a bool occupies a byte, a
  small enum's discriminator fits in a byte. This headroom makes a class
  of schema changes migration-free — adding a unit variant that still
  fits the byte, appending struct fields under append-only encoding,
  widening fixed-width ints — provided variant order is preserved. The
  schema-layout schema exposes this headroom so derivation can skip
  zero-cost changes. Cap'n-Proto-style structural-compatibility
  discipline lives at the rkyv layer, not the NOTA text layer.

## Version control — the reusable system

The content-addressed versioning and migration the kernel supports is
the storage face of a larger goal: a **reusable component
version-control system**, foundational to the whole meta-work. The
facts below are system-level direction — they live in `memory-engine` and
its supporting libraries, not in this kernel — but they fix what the
typed-table storage underneath must serve.

- **Native version-controlled durability.** Component Memory databases
  (each component's durable daemon state) get native version-controlled,
  server-backed atomic durability with no state loss, built once as a
  reusable library of generic types and traits every component opts
  into. The design is Dolt-informed; strict-typed
  hard-migration-per-schema-change is the core constraint, with the
  exact generic mechanism settled by prior-art research.
- **Full distributed-version-control memoryntics.** The system supports
  branching, forking, rebasing, and merging over the typed database,
  with per-component customizable intake, merge, and rebase policy: a
  default implementation plus a per-component override. For example,
  Spirit's guardian mediates rebase by admitting, rejecting, or
  transforming each incoming entry.
- **Operation log is authoritative; redb is a materialized view.** The
  versioned operation log is the authoritative source of truth for
  component Memory state, and the redb store becomes a rebuildable
  materialized view folded from the log. This kernel inversion is chosen
  for the first version-control implementation rather than deferred —
  which is why this kernel keeps no history of its own beyond redb's
  last-committed page pair (see Deletion durability): durable history
  lives in the log above it.
- **The remote is a mirror triad.** The version-control remote is a
  dedicated mirror component triad (`mirror`, `signal-mirror`,
  `meta-signal-mirror`). One payload-blind append-ingest mirror daemon
  on the ouranos tailnet host serves every component store: it validates
  sequence continuity and the expected head, deduplicates idempotently,
  fsyncs before acking, and carries retention and privacy behind its
  meta signal. Its own state is itself a memory-engine store.
- **Cross-host transport.** Cross-host component transport for the
  mirror is a tailnet-bound TCP listener in `triad-runtime`, reusing the
  length-prefixed frame codec, with peer identity as a typed closed sum
  that distinguishes kernel-vouched Unix-socket peers from tailnet TCP
  peers. Ssh-forwarded sockets are rejected as a transport shape.
- **One cryptographic basis.** A single consistent cryptographic basis
  spans the entire version-control and backup system: blake3 for all
  content addressing and Criome BLS (BLS12-381 curve, BLS signature
  scheme) for signing and attesting history; no component diverges. The
  pre-Criome agent-identity path already uses BLS12-381 keypairs (not
  Ed25519) from day one, so identity transitions cleanly when Criome
  lands.

## Macro-pattern integration

**Status:** integrated into the brilliant macro library pattern per `reports/designer/326-v13-spirit-complete-schema-vision.md §3` (schemas as macro-pattern instance).

**Role:** this crate is the storage kernel — the typed-value substrate `memory-engine` builds upon. Component schemas never reference `memory` directly; they reference `memory-engine`, which composes this kernel under the hood.

**Integration target:** storage kernel; consumed by `memory-engine`. Under the schema-engine upgrade, this crate's surface does not change. The macro-emitted storage descriptors call into `memory-engine`'s typed-table API, which in turn calls into this kernel's typed-value primitives. The reducer-based migration model described above in the "future work" section becomes more natural under the schema-engine upgrade: the schema language gives `.schema` files first-class schema-hash identity, and reducers between two schema-hash versions become regular schema-declared transformations the macro can emit code for.

**References:**
- `reports/designer/326-v13-spirit-complete-schema-vision.md` — schema language + macro pattern
- `reports/designer/324-migration-mvp-spirit-handover-re-specification.md` — migration MVP
- `reports/operator/174-schema-import-header-design-critique-2026-05-24.md` — lowering + AssembledSchema form
