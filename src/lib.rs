//! memory — the workspace's typed-database kernel.
//!
//! redb-backed; values are rkyv-archived; tables are typed
//! and version-guarded. This crate is the kernel; component-owned
//! typed table layers live inside the state-bearing consumer that
//! owns each database.
//!
//! See `ARCHITECTURE.md` for the role/boundaries; see
//! `~/primary/reports/designer/63-memory-as-workspace-database-library.md`
//! for the design.
//!
//! ## Surface
//!
//! `Memory::open_with_schema(path, &Schema)` opens a typed database
//! with a version-skew guard. `Table<K, V: Archive>` typed wrappers
//! hide rkyv encode/decode at the table boundary. `read(|txn| ...)`
//! and `write(|txn| ...)` keep redb transactions closure-scoped so
//! callers cannot leak transaction lifetimes across component
//! boundaries.

use std::marker::PhantomData;
use std::ops::RangeBounds;
use std::path::{Path, PathBuf};

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
pub use redb::{ReadTransaction, WriteTransaction};
use rkyv::api::high::HighDeserializer;
use rkyv::bytecheck::CheckBytes;
use rkyv::rancor::{self, Strategy};
use rkyv::ser::Serializer;
use rkyv::ser::allocator::ArenaHandle;
use rkyv::ser::sharing::Share;
use rkyv::util::AlignedVec;
use rkyv::validation::Validator;
use rkyv::validation::archive::ArchiveValidator;
use rkyv::validation::shared::SharedValidator;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize};
use thiserror::Error;

// ─── Error ──────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum Error {
    #[error("redb database: {0}")]
    Database(#[from] redb::DatabaseError),
    #[error("redb storage: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("redb transaction: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("redb table: {0}")]
    Table(#[from] redb::TableError),
    #[error("redb commit: {0}")]
    Commit(#[from] redb::CommitError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("rkyv: {0}")]
    Rkyv(rancor::Error),
    #[error("rkyv encode failed for table {table}: {source}")]
    RkyvEncode {
        table: &'static str,
        source: rancor::Error,
    },
    #[error("rkyv decode failed for table {table}: {source}")]
    RkyvDecode {
        table: &'static str,
        source: rancor::Error,
    },
    #[error("database header encode failed: {source}")]
    DatabaseHeaderEncode { source: rancor::Error },
    #[error("database header decode failed: {source}")]
    DatabaseHeaderDecode { source: rancor::Error },
    #[error(
        "database format mismatch — file was written with {found:?}, this build expects {expected:?}"
    )]
    DatabaseFormatMismatch {
        expected: DatabaseHeader,
        found: DatabaseHeader,
    },
    #[error(
        "schema version mismatch — file was written with v{found}, this build expects v{expected}"
    )]
    SchemaVersionMismatch {
        expected: SchemaVersion,
        found: SchemaVersion,
    },
    #[error(
        "existing memory file at {} lacks a schema version — refusing to retro-stamp v{expected}; \
         either migrate the file explicitly or open a fresh path",
        path.display()
    )]
    LegacyFileLacksSchema {
        path: PathBuf,
        expected: SchemaVersion,
    },
}

impl From<rancor::Error> for Error {
    fn from(value: rancor::Error) -> Self {
        Self::Rkyv(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// ─── Database header — rkyv format guard ───────────────────

/// Persisted database header naming the rkyv format choices
/// this build expects.
#[derive(Archive, Serialize, RkyvDeserialize, Debug, Clone, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub struct DatabaseHeader {
    format_version: u32,
    endian: RkyvEndian,
    pointer_width: RkyvPointerWidth,
    unaligned: bool,
    bytecheck: bool,
}

impl DatabaseHeader {
    pub const fn current() -> Self {
        Self {
            format_version: 1,
            endian: RkyvEndian::Little,
            pointer_width: RkyvPointerWidth::PointerWidth32,
            unaligned: true,
            bytecheck: true,
        }
    }

    pub const fn new(
        format_version: u32,
        endian: RkyvEndian,
        pointer_width: RkyvPointerWidth,
        unaligned: bool,
        bytecheck: bool,
    ) -> Self {
        Self {
            format_version,
            endian,
            pointer_width,
            unaligned,
            bytecheck,
        }
    }
}

/// Endianness pinned into Memory's rkyv feature set.
#[derive(Archive, Serialize, RkyvDeserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub enum RkyvEndian {
    Little,
}

/// Pointer width pinned into Memory's rkyv feature set.
#[derive(Archive, Serialize, RkyvDeserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[rkyv(derive(Debug))]
pub enum RkyvPointerWidth {
    PointerWidth32,
}

// ─── Schema — kernel-mode open contract ─────────────────────

/// A consumer's schema declaration: just the schema version
/// today. Pass to [`Memory::open_with_schema`] at open time;
/// the kernel writes the version on first open and refuses
/// to open a file whose stored version doesn't match.
///
/// **Tables aren't declared here.** Per redb's model, a
/// table is uniquely identified by `(name, key_type,
/// value_type)`. The full type information lives on the
/// consumer's typed [`Table`] constants, not on a list of
/// names. Tables get created lazily on first use through
/// `Table::get` / `Table::insert`.
///
/// Schemas are static so they can be declared at module top:
///
/// ```ignore
/// const SCHEMA: Schema = Schema { version: SchemaVersion::new(1) };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Schema {
    pub version: SchemaVersion,
}

/// Schema version. Bump on any layout change (added field,
/// added table, removed column). The kernel hard-fails on
/// mismatch — schema upgrades are coordinated, not silent.
///
/// Construct via [`SchemaVersion::new`]; read out via
/// [`SchemaVersion::value`]. The wrapped field is private
/// so callers can't construct invalid versions or compare
/// raw u32s by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// ─── Table — typed wrapper around redb's TableDefinition ───

/// A typed table: keys of type `K`, values of type `V` (which
/// must be rkyv-archivable). The wrapper hides the encode/decode
/// at the table boundary so consumers see typed Rust values
/// in and out.
///
/// Declare at module top:
///
/// ```ignore
/// const MESSAGES: Table<&str, Message> = Table::new("messages");
/// ```
///
/// Use through the closure-scoped txn helpers:
///
/// ```ignore
/// let message = memory.read(|txn| MESSAGES.get(&txn, "m-abc"))?;
/// memory.write(|txn| MESSAGES.insert(&txn, "m-xyz", &new_message))?;
/// ```
pub struct Table<K, V>
where
    K: redb::Key + 'static,
{
    name: &'static str,
    _key: PhantomData<K>,
    _value: PhantomData<V>,
}

impl<K, V> Table<K, V>
where
    K: redb::Key + 'static,
{
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _key: PhantomData,
            _value: PhantomData,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    fn definition(&self) -> TableDefinition<'_, K, &'static [u8]> {
        TableDefinition::new(self.name)
    }

    /// Materialize this table in the database without writing
    /// a row. Consumer typed layers call this from their own
    /// schema open path when they want table existence checked
    /// eagerly instead of waiting for first insert.
    pub fn ensure(&self, txn: &WriteTransaction) -> Result<()> {
        let _table = txn.open_table(self.definition())?;
        Ok(())
    }
}

/// A redb key whose value can be owned outside the transaction
/// that yielded it.
///
/// redb keys are often borrowed at read time (`&str`, `&[u8]`).
/// Memory's table iteration methods eagerly collect rows and close
/// the read transaction before returning, so those borrowed keys
/// need an owned shape.
pub trait OwnedTableKey: redb::Key + 'static {
    type Owned;

    fn owned_key(value: Self::SelfType<'_>) -> Self::Owned;
}

macro_rules! impl_copy_owned_table_key {
    ($($key:ty),* $(,)?) => {
        $(
            impl OwnedTableKey for $key {
                type Owned = $key;

                fn owned_key(value: Self::SelfType<'_>) -> Self::Owned {
                    value
                }
            }
        )*
    };
}

impl_copy_owned_table_key!(
    (),
    bool,
    char,
    u8,
    u16,
    u32,
    u64,
    u128,
    i8,
    i16,
    i32,
    i64,
    i128,
);

impl OwnedTableKey for &'static str {
    type Owned = String;

    fn owned_key(value: Self::SelfType<'_>) -> Self::Owned {
        value.to_string()
    }
}

impl OwnedTableKey for String {
    type Owned = String;

    fn owned_key(value: Self::SelfType<'_>) -> Self::Owned {
        value
    }
}

impl OwnedTableKey for &'static [u8] {
    type Owned = Vec<u8>;

    fn owned_key(value: Self::SelfType<'_>) -> Self::Owned {
        value.to_vec()
    }
}

impl<const LENGTH: usize> OwnedTableKey for &'static [u8; LENGTH] {
    type Owned = [u8; LENGTH];

    fn owned_key(value: Self::SelfType<'_>) -> Self::Owned {
        *value
    }
}

impl<K, V> Table<K, V>
where
    K: redb::Key + 'static,
    V: Archive
        + for<'a> Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, rancor::Error>>,
    V::Archived: rkyv::Deserialize<V, HighDeserializer<rancor::Error>>
        + for<'b> CheckBytes<
            Strategy<Validator<ArchiveValidator<'b>, SharedValidator>, rancor::Error>,
        >,
{
    /// Read the typed value at `key`, if present. Returns
    /// `Ok(None)` if the table doesn't exist yet (it gets
    /// created lazily on first write).
    pub fn get<'txn>(
        &self,
        txn: &'txn ReadTransaction,
        key: impl std::borrow::Borrow<K::SelfType<'txn>>,
    ) -> Result<Option<V>> {
        let table = match txn.open_table(self.definition()) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(other) => return Err(other.into()),
        };
        let Some(guard) = table.get(key)? else {
            return Ok(None);
        };
        Ok(Some(self.decode_value(guard.value())?))
    }

    /// Insert `value` at `key`, overwriting any existing value.
    pub fn insert<'txn>(
        &self,
        txn: &'txn WriteTransaction,
        key: impl std::borrow::Borrow<K::SelfType<'txn>>,
        value: &V,
    ) -> Result<()> {
        let bytes = rkyv::to_bytes::<rancor::Error>(value).map_err(|source| Error::RkyvEncode {
            table: self.name,
            source,
        })?;
        let mut table = txn.open_table(self.definition())?;
        table.insert(key, bytes.as_slice())?;
        Ok(())
    }

    /// Remove the entry at `key`. Returns whether anything
    /// was removed (false if the table doesn't exist or the
    /// key isn't present).
    pub fn remove<'txn>(
        &self,
        txn: &'txn WriteTransaction,
        key: impl std::borrow::Borrow<K::SelfType<'txn>>,
    ) -> Result<bool> {
        let mut table = match txn.open_table(self.definition()) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(other) => return Err(other.into()),
        };
        Ok(table.remove(key)?.is_some())
    }

    /// Snapshot every typed row in key order. The result owns
    /// both keys and values, so the redb transaction can close
    /// before callers use the rows.
    pub fn iter(&self, txn: &ReadTransaction) -> Result<Vec<(K::Owned, V)>>
    where
        K: OwnedTableKey,
    {
        let table = match txn.open_table(self.definition()) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(other) => return Err(other.into()),
        };
        let mut rows = Vec::new();
        for entry in table.iter()? {
            let (key_guard, bytes_guard) = entry?;
            rows.push((
                K::owned_key(key_guard.value()),
                self.decode_value(bytes_guard.value())?,
            ));
        }
        Ok(rows)
    }

    /// Snapshot typed rows whose keys fall inside `range`.
    /// Order is redb key order.
    pub fn range<'range, KeyRange>(
        &self,
        txn: &ReadTransaction,
        range: impl RangeBounds<KeyRange> + 'range,
    ) -> Result<Vec<(K::Owned, V)>>
    where
        K: OwnedTableKey,
        KeyRange: std::borrow::Borrow<K::SelfType<'range>> + 'range,
    {
        let table = match txn.open_table(self.definition()) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(other) => return Err(other.into()),
        };
        let mut rows = Vec::new();
        for entry in table.range(range)? {
            let (key_guard, bytes_guard) = entry?;
            rows.push((
                K::owned_key(key_guard.value()),
                self.decode_value(bytes_guard.value())?,
            ));
        }
        Ok(rows)
    }

    fn decode_value(&self, bytes: &[u8]) -> Result<V> {
        rkyv::from_bytes::<V, rancor::Error>(bytes).map_err(|source| Error::RkyvDecode {
            table: self.name,
            source,
        })
    }
}

// ─── Memory — the database handle ─────────────────────────────

const META: TableDefinition<&str, u64> = TableDefinition::new("__memory_meta");
const DATABASE_HEADERS: TableDefinition<&str, &[u8]> = TableDefinition::new("__memory_headers");
const DATABASE_HEADER_KEY: &str = "database";
const SCHEMA_VERSION_KEY: &str = "schema_version";

pub struct Memory {
    database: Database,
    path: PathBuf,
}

impl Memory {
    /// Open or create a memory database at `path` with a
    /// declared schema. The kernel writes the schema version
    /// on first open and hard-fails on mismatch.
    pub fn open_with_schema(path: &Path, schema: &Schema) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Distinguish "fresh file" (first ever open) from
        // "existing file" (already has bytes on disk). The
        // version-skew guard treats them differently: fresh
        // files get the schema stamped; existing files
        // without a stamp are legacy and refused.
        let is_fresh_file = !path.exists();
        let database = Database::create(path)?;
        let transaction = database.begin_write()?;
        {
            let mut meta = transaction.open_table(META)?;
            Self::ensure_database_header(&transaction)?;
            Self::ensure_schema_version(&mut meta, schema.version, is_fresh_file, path)?;
            // Tables are NOT pre-created here. Per redb's
            // model, tables are typed (K, V); a list of
            // names would prematurely commit them to one
            // K type. Tables get created lazily on first
            // `Table::get` / `Table::insert` with the
            // consumer's actual K and V.
        }
        transaction.commit()?;
        Ok(Memory {
            database,
            path: path.to_path_buf(),
        })
    }

    fn ensure_database_header(transaction: &WriteTransaction) -> Result<()> {
        let mut headers = transaction.open_table(DATABASE_HEADERS)?;
        let expected = DatabaseHeader::current();
        let Some(stored) = headers.get(DATABASE_HEADER_KEY)? else {
            let bytes = rkyv::to_bytes::<rancor::Error>(&expected)
                .map_err(|source| Error::DatabaseHeaderEncode { source })?;
            headers.insert(DATABASE_HEADER_KEY, bytes.as_slice())?;
            return Ok(());
        };
        let found = rkyv::from_bytes::<DatabaseHeader, rancor::Error>(stored.value())
            .map_err(|source| Error::DatabaseHeaderDecode { source })?;
        if found != expected {
            return Err(Error::DatabaseFormatMismatch { expected, found });
        }
        Ok(())
    }

    fn ensure_schema_version(
        meta: &mut redb::Table<'_, &str, u64>,
        expected: SchemaVersion,
        is_fresh_file: bool,
        path: &Path,
    ) -> Result<()> {
        let stored = meta.get(SCHEMA_VERSION_KEY)?.map(|guard| guard.value());
        match (stored, is_fresh_file) {
            (Some(value), _) => {
                let found = SchemaVersion::new(value as u32);
                if found != expected {
                    return Err(Error::SchemaVersionMismatch { expected, found });
                }
            }
            (None, true) => {
                meta.insert(SCHEMA_VERSION_KEY, expected.value() as u64)?;
            }
            (None, false) => {
                return Err(Error::LegacyFileLacksSchema {
                    path: path.to_path_buf(),
                    expected,
                });
            }
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a closure in a write transaction; commit on `Ok`,
    /// roll back (drop) on `Err`. The kernel-mode happy path:
    ///
    /// ```ignore
    /// memory.write(|txn| {
    ///     MESSAGES.insert(&txn, "m-abc", &message)?;
    ///     LOCKS.insert(&txn, "designer", &lock)?;
    ///     Ok(())
    /// })?;
    /// ```
    pub fn write<R>(&self, body: impl FnOnce(&WriteTransaction) -> Result<R>) -> Result<R> {
        let txn = self.database.begin_write()?;
        let result = body(&txn)?;
        txn.commit()?;
        Ok(result)
    }

    /// Run a closure in a read transaction; the txn drops at
    /// end of scope.
    pub fn read<R>(&self, body: impl FnOnce(&ReadTransaction) -> Result<R>) -> Result<R> {
        let txn = self.database.begin_read()?;
        body(&txn)
    }
}
