# memory

The memory database kernel: typed, version-guarded table access over
[redb](https://github.com/cberner/redb) with rkyv-archived Rust values.
This crate owns the database kernel only. Component-specific tables and
relations live in the state-bearing component that owns that database.

The typed-kernel surface stores a Memory database header alongside
the schema version, so each database records the rkyv format
identity it was written with.

A `Graph` record is the database-level compilation unit — a
flow-graph of `Node` records connected by `Edge` records that
compile together to one artifact. The flow-graph IS the
program.

## Test Commands

The repo carries shell scripts for the named test surfaces. Run
them through Nix so the pinned Rust toolchain is used:

```sh
nix run .#test
nix run .#test-kernel-surface
nix run .#test-no-legacy-surface
nix run .#test-doc
```

## License

[License of Non-Authority](LICENSE.md).

## Reference

`reference/Vision.md` — aspirational description
of the mature memory format (universal typed-binary of meaning,
self-transforming via quorum-signed spec changes, criome as the
distributed web). Reference material for future direction; current
implementation is the pseudo-memory layer above.
