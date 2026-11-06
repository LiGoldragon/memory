//! A file the kernel wrote under its former name, `sema`, converts to a
//! memory file in one write transaction, and a refused conversion leaves it
//! a sema file.
//!
//! The sema file is written here through redb exactly as the sema kernel
//! (51d7927) wrote it: `__sema_meta` with the schema version, `__sema_headers`
//! with the archived database header, and one consumer table.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use memory::{DatabaseHeader, Error, Memory, Schema, SchemaVersion, SemaFile, Table};
use redb::TableDefinition;
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct Toy {
    name: String,
}

const SCHEMA: Schema = Schema {
    version: SchemaVersion::new(3),
};
const TOYS: Table<&str, Toy> = Table::new("toys");
const LEGACY_TOYS: &str = "__sema_toys";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh path under the system temporary directory, removed on drop.
struct ScratchPath {
    path: PathBuf,
}

impl ScratchPath {
    fn new(extension: &str) -> Self {
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "memory_sema_file_test_{}_{}.{extension}",
            std::process::id(),
            counter
        ));
        let _ = std::fs::remove_file(&path);
        Self { path }
    }
}

impl Drop for ScratchPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct LegacyFile {
    scratch: ScratchPath,
}

impl LegacyFile {
    fn written() -> Self {
        let file = Self {
            scratch: ScratchPath::new("sema"),
        };
        let database = redb::Database::create(file.path()).expect("redb creates");
        let transaction = database.begin_write().expect("write begins");
        {
            let mut meta = transaction
                .open_table(TableDefinition::<&str, u64>::new("__sema_meta"))
                .expect("meta table");
            meta.insert("schema_version", 3).expect("schema version");
            let mut headers = transaction
                .open_table(TableDefinition::<&str, &[u8]>::new("__sema_headers"))
                .expect("header table");
            let header = rkyv::to_bytes::<rkyv::rancor::Error>(&DatabaseHeader::current())
                .expect("header encodes");
            headers
                .insert("database", header.as_slice())
                .expect("header row");
            let mut toys = transaction
                .open_table(TableDefinition::<&str, &[u8]>::new(LEGACY_TOYS))
                .expect("toy table");
            let toy = rkyv::to_bytes::<rkyv::rancor::Error>(&Toy {
                name: "kite".to_owned(),
            })
            .expect("toy encodes");
            toys.insert("kite", toy.as_slice()).expect("toy row");
        }
        transaction.commit().expect("legacy file commits");
        file
    }

    fn path(&self) -> PathBuf {
        self.scratch.path.clone()
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.path()).expect("file reads")
    }
}

fn toys(path: &Path) -> Vec<(String, Toy)> {
    Memory::open_with_schema(path, &SCHEMA)
        .expect("converted file opens as memory")
        .read(|transaction| TOYS.iter(transaction))
        .expect("toys read")
}

#[test]
fn a_sema_file_converts_its_kernel_and_consumer_tables_in_one_transaction() {
    let file = LegacyFile::written();
    let renamed = SemaFile::open(&file.path())
        .expect("sema file opens")
        .convert(|transaction| TOYS.rename_from(transaction, LEGACY_TOYS))
        .expect("conversion commits");
    assert!(renamed);
    assert_eq!(
        toys(&file.path()),
        vec![(
            "kite".to_owned(),
            Toy {
                name: "kite".to_owned()
            }
        )]
    );
    assert!(matches!(
        SemaFile::open(&file.path()),
        Err(Error::AlreadyMemoryFile { .. })
    ));
}

#[test]
fn opening_a_sema_file_writes_nothing() {
    let file = LegacyFile::written();
    let before = file.bytes();
    let sema = SemaFile::open(&file.path()).expect("sema file opens");
    let rows = sema
        .read(|transaction| {
            Table::<&str, Toy>::new(LEGACY_TOYS)
                .iter(transaction)
                .map(|rows| rows.len())
        })
        .expect("legacy rows read");
    drop(sema);
    assert_eq!(rows, 1);
    assert_eq!(file.bytes(), before);
}

#[test]
fn a_refused_conversion_leaves_a_sema_file() {
    let file = LegacyFile::written();
    let refusal = SemaFile::open(&file.path())
        .expect("sema file opens")
        .convert(|transaction| {
            TOYS.rename_from(transaction, LEGACY_TOYS)?;
            Err::<(), _>(Error::Io(std::io::Error::other("consumer refuses")))
        })
        .expect_err("the consumer refused");
    assert!(matches!(refusal, Error::Io(_)));
    let sema = SemaFile::open(&file.path()).expect("still a sema file");
    let rows = sema
        .read(|transaction| Table::<&str, Toy>::new(LEGACY_TOYS).iter(transaction))
        .expect("legacy rows read");
    assert_eq!(rows.len(), 1);
}

#[test]
fn a_file_without_sema_tables_is_not_a_sema_file() {
    let memory_file = ScratchPath::new("memory");
    drop(Memory::open_with_schema(&memory_file.path, &SCHEMA).expect("memory file"));
    assert!(matches!(
        SemaFile::open(&memory_file.path),
        Err(Error::AlreadyMemoryFile { .. })
    ));
    let empty = ScratchPath::new("redb");
    drop(redb::Database::create(&empty.path).expect("empty redb"));
    assert!(matches!(
        SemaFile::open(&empty.path),
        Err(Error::NotSemaFile { .. })
    ));
    let missing = ScratchPath::new("sema");
    assert!(SemaFile::open(&missing.path).is_err());
    assert!(!missing.path.exists());
}

#[test]
fn renaming_from_an_absent_table_answers_false() {
    let file = LegacyFile::written();
    let renamed = SemaFile::open(&file.path())
        .expect("sema file opens")
        .convert(|transaction| TOYS.rename_from(transaction, "__sema_absent"))
        .expect("conversion commits");
    assert!(!renamed);
}
