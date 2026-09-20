//! Canonical SQLite TEXT representations for domain identifiers and UTC dates.

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use sqlx::{
    Decode, Encode, Sqlite, Type,
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef},
};
use uuid::Uuid;

pub(super) fn timestamp(value: DateTime<Utc>) -> String {
    // Match SQLx PostgreSQL's microseconds-since-2000 encoding, including pre-epoch truncation.
    let epoch = DateTime::from_timestamp(946_684_800, 0).expect("valid PostgreSQL epoch");
    let micros = (value.naive_utc() - epoch.naive_utc())
        .num_microseconds()
        .expect("chrono timestamp fits microseconds");
    epoch
        .checked_add_signed(chrono::Duration::microseconds(micros))
        .unwrap_or(value)
        .to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub(super) fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, BoxDynError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| "invalid SQLite timestamp")?
        .with_timezone(&Utc);
    if value.len() != 27 || timestamp(parsed) != value {
        return Err("noncanonical SQLite timestamp".into());
    }
    Ok(parsed)
}

macro_rules! text_type {
    ($name:ident, $inner:ty, $format:expr, $parse:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(pub $inner);

        impl Type<Sqlite> for $name {
            fn type_info() -> SqliteTypeInfo {
                <String as Type<Sqlite>>::type_info()
            }
            fn compatible(ty: &SqliteTypeInfo) -> bool {
                <String as Type<Sqlite>>::compatible(ty)
            }
        }
        impl<'q> Encode<'q, Sqlite> for $name {
            fn encode_by_ref(
                &self,
                arguments: &mut SqliteArgumentsBuffer,
            ) -> Result<IsNull, BoxDynError> {
                let text = ($format)(self.0);
                let _: $inner = ($parse)(&text)?;
                <String as Encode<'q, Sqlite>>::encode(text, arguments)
            }
        }
        impl<'r> Decode<'r, Sqlite> for $name {
            fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
                let text = <&str as Decode<'r, Sqlite>>::decode(value)?;
                ($parse)(text).map(Self)
            }
        }
    };
}

pub(super) fn parse_uuid(text: &str) -> Result<Uuid, BoxDynError> {
    let value = Uuid::parse_str(text).map_err(|_| "invalid SQLite UUID")?;
    if value.to_string() != text {
        return Err("noncanonical SQLite UUID".into());
    }
    Ok(value)
}

pub(super) fn parse_date(text: &str) -> Result<NaiveDate, BoxDynError> {
    let value = NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| "invalid SQLite date")?;
    if text.len() != 10 || value.to_string() != text {
        return Err("noncanonical SQLite date".into());
    }
    Ok(value)
}

text_type!(
    SqliteUuid,
    Uuid,
    |value: Uuid| value.to_string(),
    parse_uuid
);
text_type!(SqliteTimestamp, DateTime<Utc>, timestamp, parse_timestamp);
text_type!(
    SqliteDate,
    NaiveDate,
    |value: NaiveDate| value.to_string(),
    parse_date
);
