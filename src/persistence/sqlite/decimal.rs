//! Exact decimal TEXT transport; column precision and business limits belong to the schema/repository.

use rust_decimal::Decimal;
use sqlx::{
    Decode, Encode, Sqlite, Type, ValueRef,
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
};

/// Bind only to TEXT columns; SQLite NUMERIC affinity/arithmetic can silently lose precision.
/// Canonical strings are not numerically sortable and must not be used with SQL SUM/CAST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqliteDecimal(pub Decimal);

impl Type<Sqlite> for SqliteDecimal {
    fn type_info() -> SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> Encode<'q, Sqlite> for SqliteDecimal {
    fn encode_by_ref(
        &self,
        arguments: &mut Vec<SqliteArgumentValue<'q>>,
    ) -> Result<IsNull, BoxDynError> {
        <String as Encode<'q, Sqlite>>::encode(self.0.normalize().to_string(), arguments)
    }
}

impl<'r> Decode<'r, Sqlite> for SqliteDecimal {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        if !Self::compatible(&value.type_info()) {
            return Err("SQLite decimal must be stored as TEXT".into());
        }
        let text = <&str as Decode<'r, Sqlite>>::decode(value)?;
        let decimal =
            Decimal::from_str_exact(text).map_err(|_| "Invalid SQLite decimal representation")?;
        if decimal.normalize().to_string() != text {
            return Err("Noncanonical SQLite decimal representation".into());
        }
        Ok(Self(decimal))
    }
}
