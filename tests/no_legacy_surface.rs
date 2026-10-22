//! Architectural truth tests for retired memory surfaces.
//!
//! These tests intentionally inspect the public source because the
//! constraint is negative: the storage kernel must not grow a raw
//! slot-store or Criome read-pool configuration surface again.

fn lib_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lib.rs");
    std::fs::read_to_string(path).expect("src/lib.rs should be readable")
}

#[test]
fn memory_does_not_export_slot() {
    let source = lib_source();
    assert!(!source.contains("pub struct Slot"));
    assert!(!source.contains("impl From<u64> for Slot"));
    assert!(!source.contains("impl From<Slot> for u64"));
}

#[test]
fn memory_does_not_export_legacy_slot_store() {
    let source = lib_source();
    assert!(!source.contains("pub fn open(path: &Path)"));
    assert!(!source.contains("pub fn store(&self"));
    assert!(!source.contains("pub fn get(&self, slot"));
    assert!(!source.contains("pub fn iter(&self) -> Result<Vec<(Slot"));
    assert!(!source.contains("__memory_records"));
    assert!(!source.contains("next_slot"));
}

#[test]
fn memory_does_not_export_reader_count() {
    let source = lib_source();
    assert!(!source.contains("DEFAULT_READER_COUNT"));
    assert!(!source.contains("pub fn reader_count"));
    assert!(!source.contains("pub fn set_reader_count"));
    assert!(!source.contains("reader_count"));
}
