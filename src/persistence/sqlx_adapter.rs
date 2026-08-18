//! Private SQLx-to-repository failure classification.

use std::io;

use sqlx::error::{DatabaseError, ErrorKind};

use super::{RepositoryError, error::RepositoryErrorSource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureKind {
    TransactionConflict,
    Constraint,
    Busy,
    Timeout,
    Corrupt,
    StorageUnavailable,
    Migration,
    DatabaseFailure,
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        let kind = classify_sqlx_error(&error);
        let source = RepositoryErrorSource::new(error);
        match kind {
            FailureKind::TransactionConflict => Self::TransactionConflict(source),
            FailureKind::Constraint => Self::Constraint(source),
            FailureKind::Busy => Self::Busy(source),
            FailureKind::Timeout => Self::Timeout(source),
            FailureKind::Corrupt => Self::Corrupt(source),
            FailureKind::StorageUnavailable => Self::StorageUnavailable(source),
            FailureKind::Migration => Self::Migration(source),
            FailureKind::DatabaseFailure => Self::DatabaseFailure(source),
        }
    }
}

fn classify_sqlx_error(error: &sqlx::Error) -> FailureKind {
    match error {
        sqlx::Error::Database(error) => classify_database_error(error.as_ref()),
        sqlx::Error::Io(error) if error.kind() == io::ErrorKind::TimedOut => FailureKind::Timeout,
        sqlx::Error::Io(error) if error.kind() == io::ErrorKind::WouldBlock => FailureKind::Busy,
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => FailureKind::StorageUnavailable,
        sqlx::Error::PoolTimedOut => FailureKind::Timeout,
        sqlx::Error::Protocol(_) | sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => {
            FailureKind::Corrupt
        }
        sqlx::Error::Migrate(_) => FailureKind::Migration,
        _ => FailureKind::DatabaseFailure,
    }
}

fn classify_database_error(error: &(dyn DatabaseError + 'static)) -> FailureKind {
    if matches!(
        error.kind(),
        ErrorKind::UniqueViolation
            | ErrorKind::ForeignKeyViolation
            | ErrorKind::NotNullViolation
            | ErrorKind::CheckViolation
    ) {
        return FailureKind::Constraint;
    }

    if let Some(error) = error.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
        return classify_postgres_sqlstate(error.code()).unwrap_or(FailureKind::DatabaseFailure);
    }

    #[cfg(feature = "sqlite-backend")]
    if let Some(error) = error.try_downcast_ref::<sqlx::sqlite::SqliteError>() {
        return error
            .code()
            .and_then(|code| code.parse::<i32>().ok())
            .and_then(classify_sqlite_result_code)
            .unwrap_or(FailureKind::DatabaseFailure);
    }

    FailureKind::DatabaseFailure
}

fn classify_postgres_sqlstate(code: &str) -> Option<FailureKind> {
    match code {
        "40001" | "40P01" => Some(FailureKind::TransactionConflict),
        "55P03" => Some(FailureKind::Busy),
        "57014" => Some(FailureKind::Timeout),
        "XX001" | "XX002" => Some(FailureKind::Corrupt),
        "57P01" | "57P02" | "57P03" | "57P04" => Some(FailureKind::StorageUnavailable),
        // Preserve the Console's established validation mapping while moving
        // SQLSTATE inspection behind the persistence boundary.
        "22001" | "22007" | "22P02" | "23502" | "23503" | "23505" | "23514" => {
            Some(FailureKind::Constraint)
        }
        _ if code.starts_with("08") || code.starts_with("53") || code.starts_with("58") => {
            Some(FailureKind::StorageUnavailable)
        }
        _ => None,
    }
}

#[cfg(feature = "sqlite-backend")]
fn classify_sqlite_result_code(code: i32) -> Option<FailureKind> {
    match code {
        // SQLITE_BUSY_SNAPSHOT means a read transaction cannot be promoted
        // because another connection has committed since its snapshot.
        517 => return Some(FailureKind::TransactionConflict),
        // SQLITE_BUSY_TIMEOUT reports that the configured busy timeout
        // elapsed while waiting for a lock.
        773 => return Some(FailureKind::Timeout),
        // SQLITE_IOERR_CORRUPTFS identifies filesystem-level corruption.
        8458 => return Some(FailureKind::Corrupt),
        _ => {}
    }

    let primary_code = code & 0xff;
    match primary_code {
        // SQLITE_BUSY, SQLITE_LOCKED, and SQLITE_PROTOCOL (a locking protocol
        // failure) all mean another user of the database currently prevents
        // progress.
        5 | 6 | 15 => Some(FailureKind::Busy),
        // SQLITE_INTERRUPT is SQLite's query-cancellation result.
        9 => Some(FailureKind::Timeout),
        // SQLITE_NOMEM, READONLY, IOERR, FULL, and CANTOPEN are resource or
        // storage-access failures.
        7 | 8 | 10 | 13 | 14 => Some(FailureKind::StorageUnavailable),
        // SQLITE_CORRUPT and SQLITE_NOTADB identify invalid storage.
        11 | 26 => Some(FailureKind::Corrupt),
        // SQLITE_TOOBIG, CONSTRAINT, MISMATCH, and RANGE reject supplied or
        // encoded SQL data.
        18 | 19 | 20 | 25 => Some(FailureKind::Constraint),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error as _, io};

    use super::{FailureKind, classify_postgres_sqlstate, classify_sqlx_error};
    use crate::persistence::RepositoryError;

    #[test]
    fn classifies_synthetic_sqlx_failures_without_backend_details() {
        assert_eq!(
            classify_sqlx_error(&sqlx::Error::PoolTimedOut),
            FailureKind::Timeout
        );
        assert_eq!(
            classify_sqlx_error(&sqlx::Error::Io(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection reset",
            ))),
            FailureKind::StorageUnavailable
        );
        assert_eq!(
            classify_sqlx_error(&sqlx::Error::Protocol("invalid frame".to_owned())),
            FailureKind::Corrupt
        );
        assert_eq!(
            classify_sqlx_error(&sqlx::Error::ColumnNotFound("missing".to_owned())),
            FailureKind::DatabaseFailure
        );
        assert_eq!(
            classify_sqlx_error(&sqlx::Error::Migrate(Box::new(
                sqlx::migrate::MigrateError::VersionMissing(7),
            ))),
            FailureKind::Migration
        );
    }

    #[test]
    fn postgres_sqlstates_cover_conflict_constraint_busy_timeout_corrupt_and_unavailable() {
        assert_eq!(
            classify_postgres_sqlstate("40001"),
            Some(FailureKind::TransactionConflict)
        );
        assert_eq!(
            classify_postgres_sqlstate("40P01"),
            Some(FailureKind::TransactionConflict)
        );
        assert_eq!(
            classify_postgres_sqlstate("22P02"),
            Some(FailureKind::Constraint)
        );
        assert_eq!(
            classify_postgres_sqlstate("23505"),
            Some(FailureKind::Constraint)
        );
        assert_eq!(classify_postgres_sqlstate("22003"), None);
        assert_eq!(classify_postgres_sqlstate("55P03"), Some(FailureKind::Busy));
        assert_eq!(
            classify_postgres_sqlstate("57014"),
            Some(FailureKind::Timeout)
        );
        assert_eq!(
            classify_postgres_sqlstate("XX001"),
            Some(FailureKind::Corrupt)
        );
        assert_eq!(
            classify_postgres_sqlstate("08006"),
            Some(FailureKind::StorageUnavailable)
        );
        assert_eq!(
            classify_postgres_sqlstate("53300"),
            Some(FailureKind::StorageUnavailable)
        );
        assert_eq!(classify_postgres_sqlstate("42601"), None);
    }

    #[cfg(feature = "sqlite-backend")]
    #[test]
    fn sqlite_extended_codes_take_precedence_over_primary_code_fallbacks() {
        use super::classify_sqlite_result_code;

        assert_eq!(
            classify_sqlite_result_code(517),
            Some(FailureKind::TransactionConflict)
        );
        assert_eq!(classify_sqlite_result_code(773), Some(FailureKind::Timeout));
        assert_eq!(
            classify_sqlite_result_code(8458),
            Some(FailureKind::Corrupt)
        );
        assert_eq!(classify_sqlite_result_code(5), Some(FailureKind::Busy));
        assert_eq!(
            classify_sqlite_result_code(19),
            Some(FailureKind::Constraint)
        );
        assert_eq!(classify_sqlite_result_code(26), Some(FailureKind::Corrupt));
        assert_eq!(classify_sqlite_result_code(1), None);
    }

    #[test]
    fn repository_error_source_is_terminal_and_opaque() {
        let error = RepositoryError::from(sqlx::Error::Protocol("invalid frame".to_owned()));
        assert!(matches!(error, RepositoryError::Corrupt(_)));
        let source = error.source().expect("repository error source");
        assert_eq!(source.to_string(), "opaque persistence backend failure");
        assert_eq!(format!("{source:?}"), "RepositoryErrorSource { .. }");
        assert!(source.source().is_none());
        assert!(!format!("{error:?}").contains("invalid frame"));
    }
}
