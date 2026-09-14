use rusqlite::backup::Backup;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sha256: &'static str,
    pub sql: &'static str,
}

pub(crate) const MIGRATIONS: [Migration; 4] = [
    Migration {
        version: 1,
        name: "0001_initial.sql",
        sha256: "46c01480194590be5896f89ec5f2c14a600ffc6bd345bd1492caaaa66c13e565",
        sql: include_str!("../../../../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "0002_event_payloads_and_episodes.sql",
        sha256: "70b8be628d5cf2de76fe322fc2c73f87172553ba2ddf4115396c5701d421c43b",
        sql: include_str!("../../../../migrations/0002_event_payloads_and_episodes.sql"),
    },
    Migration {
        version: 3,
        name: "0003_runtime_hardening.sql",
        sha256: "5546cf4d58301dfcf8419bcbbd1433dced7919f00cef725b7b6239eb7071d408",
        sql: include_str!("../../../../migrations/0003_runtime_hardening.sql"),
    },
    Migration {
        version: 4,
        name: "0004_preference_claims.sql",
        sha256: "91540d6687f606fbd59feb6e86b5f2a7e403d48fb362b3e2dc93d722f498156d",
        sql: include_str!("../../../../migrations/0004_preference_claims.sql"),
    },
];

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid embedded migration catalog: {0}")]
    InvalidCatalog(String),
    #[error(
        "embedded migration checksum mismatch for {name}: expected {expected}, calculated {calculated}"
    )]
    EmbeddedChecksum {
        name: &'static str,
        expected: &'static str,
        calculated: String,
    },
    #[error("database migration history is invalid: {0}")]
    InvalidHistory(String),
    #[error(
        "applied migration checksum mismatch for version {version}: expected {expected}, stored {stored}"
    )]
    AppliedChecksum {
        version: i64,
        expected: &'static str,
        stored: String,
    },
    #[error("database schema {found} is newer than native schema {supported}")]
    NewerSchema { found: i64, supported: i64 },
    #[error("database integrity verification failed: {0}")]
    Integrity(String),
    #[error("migration changed the immutable event identity/hash set")]
    EventFingerprintChanged,
    #[error(
        "migration failed: {cause}; restore_succeeded={restore_succeeded}; recovery backup={backup_path}"
    )]
    MigrationFailed {
        cause: String,
        restore_succeeded: bool,
        backup_path: PathBuf,
    },
    #[error("migration succeeded but controlled backup cleanup failed at {path}: {cause}")]
    BackupCleanup { path: PathBuf, cause: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EventFingerprint {
    count: usize,
    digest: String,
}

pub(crate) fn open_database(path: &Path) -> Result<Connection, MigrationError> {
    open_database_with_catalog(path, &MIGRATIONS)
}

pub(crate) fn open_database_with_catalog(
    path: &Path,
    catalog: &[Migration],
) -> Result<Connection, MigrationError> {
    validate_catalog(catalog)?;
    let target_version = catalog
        .last()
        .map(|migration| migration.version)
        .unwrap_or(0);

    let mut connection = Connection::open(path)?;
    configure_connection(&connection)?;
    let current_version = schema_version(&connection)?;
    if current_version > target_version {
        return Err(MigrationError::NewerSchema {
            found: current_version,
            supported: target_version,
        });
    }
    validate_history(&connection, catalog, current_version)?;
    validate_applied_manifest(&connection, catalog, current_version, false)?;

    let pending = catalog
        .iter()
        .filter(|migration| migration.version > current_version)
        .copied()
        .collect::<Vec<_>>();
    if pending.is_empty() {
        validate_database(&connection)?;
        validate_applied_manifest(&connection, catalog, current_version, true)?;
        return Ok(connection);
    }

    let before = event_fingerprint(&connection)?;
    let backup_path = if current_version > 0 {
        Some(create_online_backup(
            &connection,
            path,
            current_version,
            target_version,
        )?)
    } else {
        None
    };

    let migration_result = (|| -> Result<(), MigrationError> {
        for migration in &pending {
            apply_migration(&mut connection, migration, catalog)?;
            let applied = schema_version(&connection)?;
            if applied != migration.version {
                return Err(MigrationError::InvalidHistory(format!(
                    "{} did not advance schema to version {} (found {applied})",
                    migration.name, migration.version
                )));
            }
        }
        validate_history(&connection, catalog, target_version)?;
        validate_applied_manifest(&connection, catalog, target_version, true)?;
        validate_database(&connection)?;
        let after = event_fingerprint(&connection)?;
        if before != after {
            return Err(MigrationError::EventFingerprintChanged);
        }
        Ok(())
    })();

    if let Err(error) = migration_result {
        if let Some(backup_path) = backup_path {
            drop(connection);
            let restore_succeeded = restore_backup(path, &backup_path).is_ok();
            return Err(MigrationError::MigrationFailed {
                cause: error.to_string(),
                restore_succeeded,
                backup_path,
            });
        }
        return Err(error);
    }

    if let Some(backup_path) = backup_path
        && let Err(error) = secure_remove(&backup_path)
    {
        return Err(MigrationError::BackupCleanup {
            path: backup_path,
            cause: error.to_string(),
        });
    }

    Ok(connection)
}

fn apply_migration(
    connection: &mut Connection,
    migration: &Migration,
    catalog: &[Migration],
) -> Result<(), MigrationError> {
    let body = migration_body(migration)?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(&body)?;
    let applied = schema_version(&transaction)?;
    if applied != migration.version {
        return Err(MigrationError::InvalidHistory(format!(
            "{} did not advance schema to version {} (found {applied})",
            migration.name, migration.version
        )));
    }
    record_manifest_rows(&transaction, catalog, applied)?;
    transaction.commit()?;
    Ok(())
}

fn migration_body(migration: &Migration) -> Result<String, MigrationError> {
    let mut begin_count = 0_usize;
    let mut commit_count = 0_usize;
    let mut body = String::with_capacity(migration.sql.len());
    for line in migration.sql.lines() {
        match line.trim().to_ascii_uppercase().as_str() {
            "BEGIN IMMEDIATE;" => begin_count += 1,
            "COMMIT;" => commit_count += 1,
            _ => {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    if begin_count != 1 || commit_count != 1 {
        return Err(MigrationError::InvalidCatalog(format!(
            "{} must contain exactly one BEGIN IMMEDIATE and one COMMIT",
            migration.name
        )));
    }
    Ok(body)
}

fn configure_connection(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "secure_delete", "ON")?;
    Ok(())
}

fn validate_catalog(catalog: &[Migration]) -> Result<(), MigrationError> {
    if catalog.is_empty() {
        return Err(MigrationError::InvalidCatalog(
            "catalog must contain at least one migration".into(),
        ));
    }
    for (index, migration) in catalog.iter().enumerate() {
        let expected_version = i64::try_from(index + 1).unwrap_or(i64::MAX);
        if migration.version != expected_version {
            return Err(MigrationError::InvalidCatalog(format!(
                "expected version {expected_version}, found {}",
                migration.version
            )));
        }
        if !migration
            .name
            .starts_with(&format!("{:04}_", migration.version))
        {
            return Err(MigrationError::InvalidCatalog(format!(
                "migration {} does not match version {}",
                migration.name, migration.version
            )));
        }
        let calculated = sha256(migration.sql.as_bytes());
        if calculated != migration.sha256 {
            return Err(MigrationError::EmbeddedChecksum {
                name: migration.name,
                expected: migration.sha256,
                calculated,
            });
        }
    }
    Ok(())
}

fn schema_version(connection: &Connection) -> Result<i64, MigrationError> {
    if !table_exists(connection, "schema_migrations")? {
        return Ok(0);
    }
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

fn validate_history(
    connection: &Connection,
    catalog: &[Migration],
    current_version: i64,
) -> Result<(), MigrationError> {
    if current_version == 0 {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations WHERE version <= ?1 ORDER BY version")?;
    let versions = statement
        .query_map([current_version], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected = (1..=current_version).collect::<Vec<_>>();
    if versions != expected {
        return Err(MigrationError::InvalidHistory(format!(
            "expected consecutive versions {expected:?}, found {versions:?}"
        )));
    }
    if usize::try_from(current_version)
        .ok()
        .is_none_or(|version| version > catalog.len())
    {
        return Err(MigrationError::NewerSchema {
            found: current_version,
            supported: i64::try_from(catalog.len()).unwrap_or(i64::MAX),
        });
    }
    Ok(())
}

fn validate_applied_manifest(
    connection: &Connection,
    catalog: &[Migration],
    current_version: i64,
    require_complete: bool,
) -> Result<(), MigrationError> {
    if !table_exists(connection, "migration_manifest")? {
        if require_complete && current_version >= 3 {
            return Err(MigrationError::InvalidHistory(
                "schema 3+ is missing migration_manifest".into(),
            ));
        }
        return Ok(());
    }

    let mut statement = connection
        .prepare("SELECT version, name, sha256 FROM migration_manifest ORDER BY version")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if require_complete && rows.len() != usize::try_from(current_version).unwrap_or(usize::MAX) {
        return Err(MigrationError::InvalidHistory(format!(
            "manifest has {} rows for schema {current_version}",
            rows.len()
        )));
    }
    for (version, stored_name, stored_hash) in rows {
        let migration = catalog
            .iter()
            .find(|migration| migration.version == version)
            .ok_or_else(|| {
                MigrationError::InvalidHistory(format!(
                    "manifest contains unknown version {version}"
                ))
            })?;
        if stored_name != migration.name {
            return Err(MigrationError::InvalidHistory(format!(
                "version {version} is named {stored_name}, expected {}",
                migration.name
            )));
        }
        if stored_hash != migration.sha256 {
            return Err(MigrationError::AppliedChecksum {
                version,
                expected: migration.sha256,
                stored: stored_hash,
            });
        }
    }
    Ok(())
}

fn record_manifest_rows(
    connection: &Connection,
    catalog: &[Migration],
    current_version: i64,
) -> Result<(), MigrationError> {
    if !table_exists(connection, "migration_manifest")? {
        return Ok(());
    }
    for migration in catalog
        .iter()
        .filter(|migration| migration.version <= current_version)
    {
        let inserted = connection.execute(
            "INSERT OR IGNORE INTO migration_manifest(version, name, sha256, applied_at_ms)
             SELECT version, ?2, ?3, applied_at_ms
             FROM schema_migrations WHERE version = ?1",
            params![migration.version, migration.name, migration.sha256],
        )?;
        if inserted > 1 {
            return Err(MigrationError::InvalidHistory(format!(
                "manifest insert affected {inserted} rows for version {}",
                migration.version
            )));
        }
    }
    Ok(())
}

fn validate_database(connection: &Connection) -> Result<(), MigrationError> {
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(MigrationError::Integrity(integrity));
    }
    let foreign_key_errors: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if foreign_key_errors != 0 {
        return Err(MigrationError::Integrity(format!(
            "foreign_key_check returned {foreign_key_errors} rows"
        )));
    }
    Ok(())
}

fn event_fingerprint(connection: &Connection) -> Result<EventFingerprint, MigrationError> {
    if !table_exists(connection, "raw_events")? {
        return Ok(EventFingerprint {
            count: 0,
            digest: sha256(&[]),
        });
    }
    let mut statement =
        connection.prepare("SELECT event_id, content_hash FROM raw_events ORDER BY event_id")?;
    let mut rows = statement.query([])?;
    let mut hasher = Sha256::new();
    let mut count = 0_usize;
    while let Some(row) = rows.next()? {
        let event_id: String = row.get(0)?;
        let content_hash: String = row.get(1)?;
        for value in [&event_id, &content_hash] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        count += 1;
    }
    Ok(EventFingerprint {
        count,
        digest: hex::encode(hasher.finalize()),
    })
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn create_online_backup(
    source: &Connection,
    database_path: &Path,
    from_version: i64,
    to_version: i64,
) -> Result<PathBuf, MigrationError> {
    let backup_path = backup_path(database_path, from_version, to_version)?;
    create_restricted_file(&backup_path)?;
    let backup_result = (|| -> Result<(), MigrationError> {
        let mut destination = Connection::open(&backup_path)?;
        destination.busy_timeout(Duration::from_secs(5))?;
        {
            let backup = Backup::new(source, &mut destination)?;
            backup.run_to_completion(64, Duration::from_millis(10), None)?;
        }
        validate_database(&destination)?;
        drop(destination);
        sync_file(&backup_path)?;
        Ok(())
    })();
    if let Err(error) = backup_result {
        let _ = secure_remove(&backup_path);
        return Err(error);
    }
    Ok(backup_path)
}

fn backup_path(
    database_path: &Path,
    from_version: i64,
    to_version: i64,
) -> Result<PathBuf, MigrationError> {
    let file_name = database_path
        .file_name()
        .ok_or_else(|| MigrationError::InvalidHistory("database path has no file name".into()))?;
    let mut backup_name = OsString::from(file_name);
    backup_name.push(format!(
        ".migration-v{from_version}-to-v{to_version}-{}-{}.backup.sqlite3",
        std::process::id(),
        now_nanos()
    ));
    Ok(database_path.with_file_name(backup_name))
}

fn create_restricted_file(path: &Path) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path)?;
    file.sync_all()
}

fn restore_backup(database_path: &Path, backup_path: &Path) -> Result<(), std::io::Error> {
    let mut temporary_name = OsString::from(
        database_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("mnemokernel.sqlite3")),
    );
    temporary_name.push(format!(
        ".restore-{}-{}.tmp",
        std::process::id(),
        now_nanos()
    ));
    let temporary_path = database_path.with_file_name(temporary_name);
    fs::copy(backup_path, &temporary_path)?;
    sync_file(&temporary_path)?;

    for sidecar in database_sidecars(database_path) {
        remove_if_exists(&sidecar)?;
    }
    remove_if_exists(database_path)?;
    if let Err(error) = fs::rename(&temporary_path, database_path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    Ok(())
}

fn database_sidecars(database_path: &Path) -> [PathBuf; 3] {
    let base = database_path.as_os_str().to_string_lossy();
    [
        PathBuf::from(format!("{base}-wal")),
        PathBuf::from(format!("{base}-shm")),
        PathBuf::from(format!("{base}-journal")),
    ]
}

fn remove_if_exists(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync_file(path: &Path) -> Result<(), std::io::Error> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

fn secure_remove(path: &Path) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    let zeros = [0_u8; 64 * 1024];
    let mut remaining = length;
    while remaining > 0 {
        let count = usize::try_from(remaining.min(zeros.len() as u64)).unwrap_or(zeros.len());
        file.write_all(&zeros[..count])?;
        remaining -= count as u64;
    }
    file.set_len(0)?;
    file.sync_all()?;
    drop(file);
    fs::remove_file(path)
}

fn sha256(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const BROKEN_MIGRATION_SQL: &str = "BEGIN IMMEDIATE;\n\
INSERT INTO schema_migrations(version, applied_at_ms) VALUES (4, 4);\n\
COMMIT;\n\
THIS IS NOT SQL;\n";
    const BROKEN_MIGRATION_SHA256: &str =
        "fe0f972a0a57606b65470c4e55eeac0804fa4a94320b6433191679711cf1eeb0";

    fn temporary_database(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mnemokernel-migration-{label}-{}-{}.sqlite3",
            std::process::id(),
            now_nanos()
        ))
    }

    fn backup_candidates(path: &Path) -> Vec<PathBuf> {
        let Some(parent) = path.parent() else {
            return Vec::new();
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return Vec::new();
        };
        let prefix = format!("{file_name}.migration-");
        fs::read_dir(parent)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|candidate| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(&prefix) && name.ends_with(".backup.sqlite3")
                    })
            })
            .collect()
    }

    fn cleanup_database(path: &Path) {
        for backup in backup_candidates(path) {
            let _ = secure_remove(&backup);
        }
        for sidecar in database_sidecars(path) {
            let _ = fs::remove_file(sidecar);
        }
        let _ = fs::remove_file(path);
    }

    fn manifest(connection: &Connection) -> Vec<(i64, String, String)> {
        let mut statement = connection
            .prepare("SELECT version, name, sha256 FROM migration_manifest ORDER BY version")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn seed_legacy_database(path: &Path, version: i64) {
        assert!((1..=2).contains(&version));
        let connection = Connection::open(path).unwrap();
        connection.execute_batch(MIGRATIONS[0].sql).unwrap();
        connection
            .execute(
                "INSERT INTO scopes VALUES (?1, 'test', 'bot', 'private', 'user', 'default', 1)",
                ["scope"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO raw_events VALUES (
                    'event', 'scope', 'test:1', 'user', 'Alice', 0, 1, 1,
                    'legacy secret marker', 'content-hash', NULL, '[]', '{}', 2
                 )",
                [],
            )
            .unwrap();
        if version == 2 {
            connection.execute_batch(MIGRATIONS[1].sql).unwrap();
        }
    }

    #[test]
    fn fresh_database_records_embedded_manifest_and_reopens_idempotently() {
        let path = temporary_database("fresh");
        let connection = open_database(&path).unwrap();
        let expected = MIGRATIONS
            .iter()
            .map(|migration| {
                (
                    migration.version,
                    migration.name.to_owned(),
                    migration.sha256.to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(manifest(&connection), expected);
        drop(connection);

        let reopened = open_database(&path).unwrap();
        assert_eq!(schema_version(&reopened).unwrap(), 4);
        assert_eq!(manifest(&reopened), expected);
        assert!(backup_candidates(&path).is_empty());
        drop(reopened);
        cleanup_database(&path);
    }

    #[test]
    fn v1_and_v2_upgrade_preserve_event_identity_and_remove_success_backup() {
        for version in [1, 2] {
            let path = temporary_database(&format!("upgrade-v{version}"));
            seed_legacy_database(&path, version);

            let connection = open_database(&path).unwrap();
            assert_eq!(schema_version(&connection).unwrap(), 4);
            assert_eq!(manifest(&connection).len(), 4);
            let envelope: (String, String) = connection
                .query_row(
                    "SELECT event_id, content_hash FROM raw_events WHERE event_id = 'event'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(envelope, ("event".into(), "content-hash".into()));
            let payload: (String, String) = connection
                .query_row(
                    "SELECT payload_state, content FROM raw_event_payloads WHERE event_id = 'event'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(payload, ("active".into(), "legacy secret marker".into()));
            assert!(backup_candidates(&path).is_empty());
            drop(connection);
            cleanup_database(&path);
        }
    }

    #[test]
    fn online_backup_includes_committed_wal_content() {
        let path = temporary_database("wal");
        let connection = open_database(&path).unwrap();
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE wal_marker(value TEXT NOT NULL);\n\
                 INSERT INTO wal_marker VALUES ('committed but not checkpointed');",
            )
            .unwrap();
        assert!(database_sidecars(&path)[0].exists());

        let backup_path = create_online_backup(&connection, &path, 3, 4).unwrap();
        let backup = Connection::open(&backup_path).unwrap();
        let value: String = backup
            .query_row("SELECT value FROM wal_marker", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "committed but not checkpointed");
        drop(backup);
        secure_remove(&backup_path).unwrap();
        drop(connection);
        cleanup_database(&path);
    }

    #[test]
    fn embedded_checksum_mismatch_fails_before_database_creation() {
        let path = temporary_database("embedded-tamper");
        let catalog = [Migration {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..MIGRATIONS[0]
        }];
        assert!(matches!(
            open_database_with_catalog(&path, &catalog),
            Err(MigrationError::EmbeddedChecksum { .. })
        ));
        assert!(!path.exists());
    }

    #[test]
    fn applied_manifest_tampering_fails_closed() {
        let path = temporary_database("manifest-tamper");
        drop(open_database(&path).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE migration_manifest SET sha256 = ?1 WHERE version = 1",
                ["0000000000000000000000000000000000000000000000000000000000000000"],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            open_database(&path),
            Err(MigrationError::AppliedChecksum { version: 1, .. })
        ));
        cleanup_database(&path);
    }

    #[test]
    fn failed_migration_restores_old_database_and_retains_recovery_backup() {
        let path = temporary_database("rollback");
        let connection = open_database(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE rollback_marker(value TEXT NOT NULL);\n\
                 INSERT INTO rollback_marker VALUES ('must survive');",
            )
            .unwrap();
        drop(connection);

        let mut catalog = MIGRATIONS.to_vec();
        catalog.push(Migration {
            version: 5,
            name: "0005_broken.sql",
            sha256: BROKEN_MIGRATION_SHA256,
            sql: BROKEN_MIGRATION_SQL,
        });
        let error = open_database_with_catalog(&path, &catalog).unwrap_err();
        let backup_path = match error {
            MigrationError::MigrationFailed {
                restore_succeeded: true,
                backup_path,
                ..
            } => backup_path,
            other => panic!("unexpected migration error: {other}"),
        };
        assert!(backup_path.exists());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let restored = open_database(&path).unwrap();
        assert_eq!(schema_version(&restored).unwrap(), 4);
        let marker: String = restored
            .query_row("SELECT value FROM rollback_marker", [], |row| row.get(0))
            .unwrap();
        assert_eq!(marker, "must survive");
        drop(restored);

        secure_remove(&backup_path).unwrap();
        cleanup_database(&path);
    }
}
