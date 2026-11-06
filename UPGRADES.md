# Upgrades

## 0.2.0 — Convert a file written under the kernel's former name

Before it was named Memory, the kernel was named sema, and a file it
wrote holds `__sema_meta` and `__sema_headers`. `Memory::open_with_schema`
does not open such a file. 0.2.0 adds the conversion:

- `SemaFile::open(path)` opens it read-only and checks its database
  header; it refuses a file without `__sema_meta` (`Error::NotSemaFile`)
  and one already holding `__memory_meta` (`Error::AlreadyMemoryFile`).
- `SemaFile::read` reads the consumer's own legacy tables.
- `SemaFile::convert(conversion)` renames the two kernel tables and runs
  `conversion` in one write transaction, committing only when both
  succeed.
- `Table::rename_from(transaction, previous)` moves a consumer table
  stored under `previous` to the table's name.

`Error` gains `NotSemaFile` and `AlreadyMemoryFile`; exhaustive matches
over it need the new arms. Nothing else changes: a memory file opens as
before. memory-engine 0.19.0 builds its store conversion on this.
