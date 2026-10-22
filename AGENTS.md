# Agent instructions — memory

## Repo role

The workspace's typed storage kernel. Memory opens redb files with an
explicit schema, guards the rkyv database format, and provides typed
`Table<K, V>` access over rkyv-archived values.

Memory is not Criome's records engine and does not own Persona runtime
state. Full database-operation execution lives in `memory-engine`.

---

## Carve-outs worth knowing

- **No schema-less open.** Durable state opens through
  `Memory::open_with_schema(path, schema)`.
- **No raw slot store.** The retired `Slot` / raw bytes / monotone
  counter surface must not reappear in this crate.
- **No Criome read-pool config.** Criome runtime tuning belongs to
  Criome; `reader_count` is not a memory kernel concept.
- **rkyv feature-set** must match the project pin per
  lore/rust/rkyv.md.

This repository is under fast development and constantly breaking.

## Protos estate status

Stack: correct-new destination
Status: component-associated, adoption unresolved
Absence of a direct notation edge is not proof of adoption.
