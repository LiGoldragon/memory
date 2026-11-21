//! Kernel-mode tests — exercise the Schema / Table&lt;K, V&gt; /
//! version-guard surface consumed by component-owned Sema layers.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use redb::{ReadableDatabase, TableDefinition, TableHandle};
use rkyv::{Archive, Deserialize, Serialize};
use sema::{
    DatabaseHeader, Error, RkyvEndian, RkyvPointerWidth, Schema, SchemaVersion, Sema, Table,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!(
        "sema_kernel_test_{}_{}.redb",
        std::process::id(),
        counter
    ));
    let _ = std::fs::remove_file(&path);
    path
}

// A small typed record to round-trip through Table<K, V>.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
struct ToyRecord {
    name: String,
    value: u32,
}

const SCHEMA_V1: Schema = Schema {
    version: SchemaVersion::new(1),
};

const SCHEMA_V2: Schema = Schema {
    version: SchemaVersion::new(2),
};

const TOYS: Table<&str, ToyRecord> = Table::new("toys");
const KEYED_BY_U64: Table<u64, ToyRecord> = Table::new("keyed_by_u64");
const DATABASE_HEADERS: TableDefinition<&str, &[u8]> = TableDefinition::new("__sema_headers");

struct HeaderEditor<'path> {
    path: &'path PathBuf,
}

impl<'path> HeaderEditor<'path> {
    fn new(path: &'path PathBuf) -> Self {
        Self { path }
    }

    fn overwrite(&self, bytes: &[u8]) {
        let database = redb::Database::create(self.path).unwrap();
        let transaction = database.begin_write().unwrap();
        {
            let mut table = transaction.open_table(DATABASE_HEADERS).unwrap();
            table.insert("database", bytes).unwrap();
        }
        transaction.commit().unwrap();
    }
}

#[test]
fn open_with_schema_writes_version_on_first_open() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    // re-open with same schema — should succeed
    let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_writes_database_header_on_first_open() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    let database = redb::Database::create(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    let table = transaction.open_table(DATABASE_HEADERS).unwrap();
    let guard = table.get("database").unwrap().expect("database header");
    let header = rkyv::from_bytes::<DatabaseHeader, rkyv::rancor::Error>(guard.value()).unwrap();
    assert_eq!(header, DatabaseHeader::current());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_refuses_database_header_format_mismatch() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    let mismatched = DatabaseHeader::new(
        999,
        RkyvEndian::Little,
        RkyvPointerWidth::PointerWidth32,
        true,
        true,
    );
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&mismatched).unwrap();
    HeaderEditor::new(&path).overwrite(bytes.as_slice());

    let result = Sema::open_with_schema(&path, &SCHEMA_V1);
    match result {
        Err(Error::DatabaseFormatMismatch { expected, found }) => {
            assert_eq!(expected, DatabaseHeader::current());
            assert_eq!(found, mismatched);
        }
        Err(other) => panic!("expected DatabaseFormatMismatch, got {other:?}"),
        Ok(_) => panic!("expected DatabaseFormatMismatch, got Ok(...)"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_refuses_invalid_database_header_bytes() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    HeaderEditor::new(&path).overwrite(b"");

    let result = Sema::open_with_schema(&path, &SCHEMA_V1);
    match result {
        Err(Error::DatabaseHeaderDecode { .. }) => {}
        Err(other) => panic!("expected DatabaseHeaderDecode, got {other:?}"),
        Ok(_) => panic!("expected DatabaseHeaderDecode, got Ok(...)"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn internal_tables_use_sema_namespace() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    let database = redb::Database::create(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    let table_names = transaction
        .list_tables()
        .unwrap()
        .map(|table| table.name().to_string())
        .collect::<Vec<_>>();
    assert!(table_names.contains(&"__sema_headers".to_string()));
    assert!(table_names.contains(&"__sema_meta".to_string()));
    assert!(!table_names.contains(&"__sema_records".to_string()));
    assert!(!table_names.contains(&"meta".to_string()));
    assert!(!table_names.contains(&"records".to_string()));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_with_mismatched_schema_version_hard_fails() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    let result = Sema::open_with_schema(&path, &SCHEMA_V2);
    match result {
        Err(Error::SchemaVersionMismatch { expected, found }) => {
            assert_eq!(expected, SchemaVersion::new(2));
            assert_eq!(found, SchemaVersion::new(1));
        }
        Err(other) => panic!("expected SchemaVersionMismatch, got {other:?}"),
        Ok(_) => panic!("expected SchemaVersionMismatch, got Ok(...)"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_with_schema_refuses_legacy_file_lacking_schema_version() {
    // The version-skew guard MUST refuse to retro-stamp an
    // existing file that lacks the schema-version stamp (no
    // schema_version stored). Silent acceptance was the bug
    // designer/66 §1.5 (Issue A) named.
    let path = temp_path();
    {
        let database = redb::Database::create(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        transaction.open_table(DATABASE_HEADERS).unwrap();
        transaction.commit().unwrap();
    }
    let result = Sema::open_with_schema(&path, &SCHEMA_V1);
    match result {
        Err(Error::LegacyFileLacksSchema { expected, .. }) => {
            assert_eq!(expected, SchemaVersion::new(1));
        }
        Err(other) => panic!("expected LegacyFileLacksSchema, got {other:?}"),
        Ok(_) => panic!("expected LegacyFileLacksSchema, got Ok(...)"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_tables_can_use_arbitrary_key_types() {
    // Issue L: pre-creating tables with hard-coded K=&str
    // locked out tables with other key types. After dropping
    // the pre-create step, a Table<u64, V> works alongside a
    // Table<&str, V> in the same file.
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let toy = ToyRecord {
        name: "u64-keyed".to_string(),
        value: 100,
    };
    sema.write(|txn| KEYED_BY_U64.insert(txn, 42u64, &toy))
        .unwrap();
    let read_back = sema
        .read(|txn| KEYED_BY_U64.get(txn, 42u64))
        .unwrap()
        .expect("u64-keyed value present");
    assert_eq!(read_back, toy);
    // and the &str-keyed table still works too
    let other = ToyRecord {
        name: "str-keyed".to_string(),
        value: 200,
    };
    sema.write(|txn| TOYS.insert(txn, "k", &other)).unwrap();
    assert_eq!(sema.read(|txn| TOYS.get(txn, "k")).unwrap().unwrap(), other);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_round_trips_value() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let original = ToyRecord {
        name: "first".to_string(),
        value: 42,
    };
    sema.write(|txn| TOYS.insert(txn, "k1", &original)).unwrap();
    let read_back = sema
        .read(|txn| TOYS.get(txn, "k1"))
        .unwrap()
        .expect("value present");
    assert_eq!(read_back, original);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_ensure_materializes_empty_table() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    sema.write(|txn| TOYS.ensure(txn)).unwrap();
    let rows = sema.read(|txn| TOYS.iter(txn)).unwrap();
    assert!(rows.is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_iter_returns_owned_keys_and_values_in_key_order() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let first = ToyRecord {
        name: "first".to_string(),
        value: 1,
    };
    let second = ToyRecord {
        name: "second".to_string(),
        value: 2,
    };
    let third = ToyRecord {
        name: "third".to_string(),
        value: 3,
    };
    sema.write(|txn| {
        TOYS.insert(txn, "c", &third)?;
        TOYS.insert(txn, "a", &first)?;
        TOYS.insert(txn, "b", &second)?;
        Ok(())
    })
    .unwrap();
    let rows = sema.read(|txn| TOYS.iter(txn)).unwrap();
    assert_eq!(
        rows,
        vec![
            ("a".to_string(), first),
            ("b".to_string(), second),
            ("c".to_string(), third),
        ]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_range_returns_owned_keys_and_values_in_key_order() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    for (key, value) in [("a", 1u32), ("b", 2u32), ("c", 3u32), ("d", 4u32)] {
        let toy = ToyRecord {
            name: key.to_string(),
            value,
        };
        sema.write(|txn| TOYS.insert(txn, key, &toy)).unwrap();
    }
    let rows = sema.read(|txn| TOYS.range(txn, "b".."d")).unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "b".to_string(),
                ToyRecord {
                    name: "b".to_string(),
                    value: 2
                }
            ),
            (
                "c".to_string(),
                ToyRecord {
                    name: "c".to_string(),
                    value: 3
                }
            ),
        ]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_range_visitor_can_skip_decode_and_preserve_order() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    sema.write(|txn| {
        TOYS.insert(
            txn,
            "a",
            &ToyRecord {
                name: "a".into(),
                value: 1,
            },
        )?;
        TOYS.insert(
            txn,
            "c",
            &ToyRecord {
                name: "c".into(),
                value: 3,
            },
        )?;
        Ok(())
    })
    .unwrap();
    sema.write(|txn| {
        let mut rows = txn.open_table(TableDefinition::<&str, &[u8]>::new("toys"))?;
        rows.insert("b", b"not-an-archive".as_slice())?;
        Ok(())
    })
    .unwrap();

    let mut visited = 0;
    let mut decoded = 0;
    let mut rows = Vec::new();
    sema.read(|txn| {
        TOYS.visit_range(txn, "a"..="c", |entry| {
            visited += 1;
            if entry.key() == "b" {
                return Ok(ControlFlow::Continue(()));
            }
            let row = entry.decode()?;
            decoded += 1;
            rows.push(row);
            Ok(ControlFlow::Continue(()))
        })
    })
    .unwrap();

    assert_eq!(visited, 3);
    assert_eq!(decoded, 2);
    assert_eq!(
        rows,
        vec![
            (
                "a".to_string(),
                ToyRecord {
                    name: "a".into(),
                    value: 1,
                },
            ),
            (
                "c".to_string(),
                ToyRecord {
                    name: "c".into(),
                    value: 3,
                },
            ),
        ]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_range_visitor_stops_before_later_decode_errors() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    sema.write(|txn| {
        TOYS.insert(
            txn,
            "a",
            &ToyRecord {
                name: "a".into(),
                value: 1,
            },
        )?;
        Ok(())
    })
    .unwrap();
    sema.write(|txn| {
        let mut rows = txn.open_table(TableDefinition::<&str, &[u8]>::new("toys"))?;
        rows.insert("b", b"not-an-archive".as_slice())?;
        Ok(())
    })
    .unwrap();

    let mut visited = 0;
    let mut decoded = 0;
    let mut rows = Vec::new();
    sema.read(|txn| {
        TOYS.visit_range(txn, "a"..="z", |entry| {
            visited += 1;
            rows.push(entry.decode()?);
            decoded += 1;
            Ok(ControlFlow::Break(()))
        })
    })
    .unwrap();

    assert_eq!(visited, 1);
    assert_eq!(decoded, 1);
    assert_eq!(
        rows,
        vec![(
            "a".to_string(),
            ToyRecord {
                name: "a".into(),
                value: 1,
            },
        )]
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_iter_returns_empty_when_table_is_missing() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let rows = sema.read(|txn| TOYS.iter(txn)).unwrap();
    assert!(rows.is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn invalid_rkyv_value_fails_loudly_with_table_context() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    sema.write(|txn| {
        let mut table = txn.open_table(TableDefinition::<&str, &[u8]>::new("toys"))?;
        table.insert("bad", b"".as_slice())?;
        Ok(())
    })
    .unwrap();
    let result = sema.read(|txn| TOYS.get(txn, "bad"));
    match result {
        Err(Error::RkyvDecode { table, .. }) => assert_eq!(table, "toys"),
        Err(other) => panic!("expected RkyvDecode, got {other:?}"),
        Ok(other) => panic!("expected RkyvDecode, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_get_returns_none_for_missing_key() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let result = sema.read(|txn| TOYS.get(txn, "nonexistent")).unwrap();
    assert!(result.is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn typed_table_remove_works() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let toy = ToyRecord {
        name: "doomed".to_string(),
        value: 1,
    };
    sema.write(|txn| TOYS.insert(txn, "k", &toy)).unwrap();
    let removed = sema.write(|txn| TOYS.remove(txn, "k")).unwrap();
    assert!(removed);
    let after = sema.read(|txn| TOYS.get(txn, "k")).unwrap();
    assert!(after.is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn write_closure_rolls_back_on_error() {
    let path = temp_path();
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    let toy = ToyRecord {
        name: "ghost".to_string(),
        value: 7,
    };
    let result = sema.write(|txn| -> sema::Result<()> {
        TOYS.insert(txn, "k", &toy)?;
        Err(Error::SchemaVersionMismatch {
            expected: SchemaVersion::new(1),
            found: SchemaVersion::new(2),
        })
    });
    assert!(matches!(result, Err(Error::SchemaVersionMismatch { .. })));
    // The insert should have been rolled back when the txn was dropped without commit.
    let after = sema.read(|txn| TOYS.get(txn, "k")).unwrap();
    assert!(after.is_none(), "rolled-back insert should not persist");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn schema_open_does_not_create_legacy_record_table() {
    let path = temp_path();
    {
        let _sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    }
    let database = redb::Database::create(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    let table_names = transaction
        .list_tables()
        .unwrap()
        .map(|table| table.name().to_string())
        .collect::<Vec<_>>();
    assert!(!table_names.contains(&"__sema_records".to_string()));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn schema_path_creates_parent_directories() {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "sema_kernel_subdir_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    path.push("nested");
    path.push("subdir");
    path.push("kernel.redb");
    let sema = Sema::open_with_schema(&path, &SCHEMA_V1).unwrap();
    assert_eq!(sema.path(), path);
    let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap().parent().unwrap());
}
