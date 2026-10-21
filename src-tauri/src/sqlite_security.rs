use rusqlite::{config::DbConfig, ffi, Connection};

pub(crate) const MAX_SQLITE_DATABASE_BYTES: u64 = 256 * 1024 * 1024 * 1024;
pub(crate) const MAX_SQLITE_VALUE_BYTES: i32 = 256 * 1024 * 1024;
pub(crate) const MAX_SQLITE_SQL_BYTES: i32 = 1024 * 1024;
pub(crate) const MAX_SQLITE_COLUMNS: i32 = 128;
pub(crate) const MAX_SQLITE_EXPRESSION_DEPTH: i32 = 100;
pub(crate) const MAX_SQLITE_COMPOUND_SELECTS: i32 = 32;
pub(crate) const MAX_SQLITE_VDBE_OPERATIONS: i32 = 5_000_000;
pub(crate) const MAX_SQLITE_FUNCTION_ARGUMENTS: i32 = 64;
pub(crate) const MAX_SQLITE_LIKE_PATTERN_BYTES: i32 = 64 * 1024;
pub(crate) const MAX_SQLITE_VARIABLES: i32 = 2_048;
pub(crate) const MAX_SQLITE_TRIGGER_DEPTH: i32 = 16;

pub(crate) fn configure_connection(conn: &Connection, read_only: bool) -> rusqlite::Result<()> {
    for (config, enabled) in [
        (DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, true),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER, false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false),
        (DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false),
        (DbConfig::SQLITE_DBCONFIG_WRITABLE_SCHEMA, false),
        (DbConfig::SQLITE_DBCONFIG_DQS_DDL, false),
        (DbConfig::SQLITE_DBCONFIG_DQS_DML, false),
        (DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
    ] {
        conn.set_db_config(config, enabled)?;
    }

    // rusqlite exposes sqlite3_limit behind an optional feature that this release
    // does not enable. The bundled SQLite API is still available through ffi.
    // SAFETY: the handle belongs to this live connection, each category is a
    // documented sqlite3_limit category, and sqlite3_limit does not retain it.
    unsafe {
        let handle = conn.handle();
        let mut extension_loading_enabled = 0;
        let result = ffi::sqlite3_db_config(
            handle,
            ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION,
            0_i32,
            &mut extension_loading_enabled,
        );
        if result != ffi::SQLITE_OK {
            return Err(rusqlite::Error::SqliteFailure(
                ffi::Error::new(result),
                Some("could not disable SQLite extension loading".to_string()),
            ));
        }
        for (category, limit) in [
            (ffi::SQLITE_LIMIT_LENGTH, MAX_SQLITE_VALUE_BYTES),
            (ffi::SQLITE_LIMIT_SQL_LENGTH, MAX_SQLITE_SQL_BYTES),
            (ffi::SQLITE_LIMIT_COLUMN, MAX_SQLITE_COLUMNS),
            (ffi::SQLITE_LIMIT_EXPR_DEPTH, MAX_SQLITE_EXPRESSION_DEPTH),
            (
                ffi::SQLITE_LIMIT_COMPOUND_SELECT,
                MAX_SQLITE_COMPOUND_SELECTS,
            ),
            (ffi::SQLITE_LIMIT_VDBE_OP, MAX_SQLITE_VDBE_OPERATIONS),
            (
                ffi::SQLITE_LIMIT_FUNCTION_ARG,
                MAX_SQLITE_FUNCTION_ARGUMENTS,
            ),
            (ffi::SQLITE_LIMIT_ATTACHED, 0),
            (
                ffi::SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
                MAX_SQLITE_LIKE_PATTERN_BYTES,
            ),
            (ffi::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_SQLITE_VARIABLES),
            (ffi::SQLITE_LIMIT_TRIGGER_DEPTH, MAX_SQLITE_TRIGGER_DEPTH),
            (ffi::SQLITE_LIMIT_WORKER_THREADS, 0),
        ] {
            ffi::sqlite3_limit(handle, category, limit);
        }
    }

    conn.execute_batch(
        "PRAGMA trusted_schema = OFF;
         PRAGMA foreign_keys = ON;
         PRAGMA recursive_triggers = OFF;",
    )?;
    if read_only {
        conn.execute_batch("PRAGMA query_only = ON;")?;
    } else {
        let page_size = conn.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
        let max_pages = (MAX_SQLITE_DATABASE_BYTES / page_size.max(1) as u64) as i64;
        conn.pragma_update(None, "max_page_count", max_pages)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite_limit(conn: &Connection, category: i32) -> i32 {
        // SAFETY: this is a read of a documented limit on a live test connection.
        unsafe { ffi::sqlite3_limit(conn.handle(), category, -1) }
    }

    fn extension_loading_enabled(conn: &Connection) -> bool {
        let mut enabled = 0;
        let result = unsafe {
            ffi::sqlite3_db_config(
                conn.handle(),
                ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION,
                -1_i32,
                &mut enabled,
            )
        };
        assert_eq!(result, ffi::SQLITE_OK);
        enabled != 0
    }

    #[test]
    fn defensive_connection_configuration_is_fail_closed() {
        let conn = Connection::open_in_memory().unwrap();
        configure_connection(&conn, true).unwrap();

        assert!(conn.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE).unwrap());
        assert!(!conn
            .db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA)
            .unwrap());
        assert!(!conn
            .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
            .unwrap());
        assert!(!conn
            .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW)
            .unwrap());
        assert_eq!(
            conn.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA trusted_schema", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(sqlite_limit(&conn, ffi::SQLITE_LIMIT_ATTACHED), 0);
        assert!(!extension_loading_enabled(&conn));
        assert_eq!(
            sqlite_limit(&conn, ffi::SQLITE_LIMIT_LENGTH),
            MAX_SQLITE_VALUE_BYTES
        );
        assert!(conn.execute_batch("ATTACH ':memory:' AS hostile").is_err());
        assert!(conn.execute_batch("PRAGMA writable_schema = ON").is_ok());
        assert!(!conn
            .db_config(DbConfig::SQLITE_DBCONFIG_WRITABLE_SCHEMA)
            .unwrap());
    }

    #[test]
    fn writable_connection_has_a_finite_database_page_limit() {
        let conn = Connection::open_in_memory().unwrap();
        configure_connection(&conn, false).unwrap();
        let page_size = conn
            .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
            .unwrap();
        let max_page_count = conn
            .query_row("PRAGMA max_page_count", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(
            max_page_count,
            (MAX_SQLITE_DATABASE_BYTES / page_size as u64) as i64
        );
    }

    #[test]
    fn oversized_values_are_rejected_without_poisoning_the_connection() {
        let conn = Connection::open_in_memory().unwrap();
        configure_connection(&conn, false).unwrap();
        conn.execute("CREATE TABLE bounded(value BLOB)", [])
            .unwrap();
        // Lower only this test connection's limit to exercise the same guard cheaply.
        unsafe {
            ffi::sqlite3_limit(conn.handle(), ffi::SQLITE_LIMIT_LENGTH, 128);
        }
        let oversized = vec![0u8; 256];
        assert!(conn
            .execute("INSERT INTO bounded(value) VALUES (?1)", [&oversized])
            .is_err());
        assert_eq!(
            conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
