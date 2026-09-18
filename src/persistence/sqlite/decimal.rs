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

/// Column-aware transport restores PostgreSQL's scale for API serialization without rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqliteNumeric<const PRECISION: u32, const SCALE: u32>(pub Decimal);

pub type SqliteAmount = SqliteNumeric<24, 8>;
pub type SqliteUnitPrice = SqliteNumeric<24, 12>;
pub type SqliteSharingAmount = SqliteNumeric<20, 8>;
pub type SqliteTokenRate = SqliteNumeric<14, 4>;

pub(super) fn fits_precision(value: Decimal, precision: u32, scale: u32) -> bool {
    let value = value.normalize();
    let text = value.abs().to_string();
    let integer = text.split('.').next().unwrap_or("");
    precision > 0
        && precision <= 28
        && scale <= precision
        && value.scale() <= scale
        && (integer == "0" || integer.len() <= (precision - scale) as usize)
}

impl<const P: u32, const S: u32> SqliteNumeric<P, S> {
    pub fn new(mut value: Decimal) -> Result<Self, BoxDynError> {
        if !fits_precision(value, P, S) {
            return Err("SQLite numeric column precision exceeded".into());
        }
        // SQLx decodes PostgreSQL's zero with scale zero regardless of the column typmod.
        if value.is_zero() {
            value = Decimal::ZERO;
        } else {
            value.rescale(S);
        }
        Ok(Self(value))
    }
}

impl<const P: u32, const S: u32> Type<Sqlite> for SqliteNumeric<P, S> {
    fn type_info() -> SqliteTypeInfo {
        SqliteDecimal::type_info()
    }
    fn compatible(ty: &SqliteTypeInfo) -> bool {
        SqliteDecimal::compatible(ty)
    }
}

impl<'q, const P: u32, const S: u32> Encode<'q, Sqlite> for SqliteNumeric<P, S> {
    fn encode_by_ref(
        &self,
        arguments: &mut Vec<SqliteArgumentValue<'q>>,
    ) -> Result<IsNull, BoxDynError> {
        Self::new(self.0)?;
        SqliteDecimal(self.0).encode_by_ref(arguments)
    }
}

impl<'r, const P: u32, const S: u32> Decode<'r, Sqlite> for SqliteNumeric<P, S> {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Self::new(SqliteDecimal::decode(value)?.0)
    }
}

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
