//! Connection-local scalar functions for schema validation; never perform SQL or I/O in callbacks.

use std::{
    ffi::{c_int, c_void},
    net::IpAddr,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use chrono::Utc;
use libsqlite3_sys as ffi;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::SqliteConnection;

use super::types::{parse_date, parse_timestamp, parse_uuid, timestamp};

struct Function {
    name: &'static str,
    clock: Arc<AtomicI64>,
}

pub(super) async fn register(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let mut handle = connection.lock_handle().await?;
    let clock = Arc::new(AtomicI64::new(Utc::now().timestamp_micros()));
    for (name, c_name, count) in [
        ("ag_decimal_valid", c"ag_decimal_valid", 3),
        ("ag_decimal_cmp", c"ag_decimal_cmp", 2),
        ("ag_uuid_valid", c"ag_uuid_valid", 1),
        ("ag_time_valid", c"ag_time_valid", 1),
        ("ag_date_valid", c"ag_date_valid", 1),
        ("ag_cidr_valid", c"ag_cidr_valid", 1),
        ("ag_array_valid", c"ag_array_valid", 2),
        ("ag_array_contains", c"ag_array_contains", 2),
        ("ag_array_position", c"ag_array_position", 2),
        ("ag_array_subset", c"ag_array_subset", 2),
        ("ag_json_equal", c"ag_json_equal", 2),
        ("ag_json_valid", c"ag_json_valid", 1),
        ("ag_regex", c"ag_regex", 2),
        ("ag_lower", c"ag_lower", 1),
        ("ag_md5_uuid", c"ag_md5_uuid", 1),
        ("ag_now", c"ag_now", 0),
        ("ag_set_time", c"ag_set_time", 1),
    ] {
        let data = Box::into_raw(Box::new(Function {
            name,
            clock: Arc::clone(&clock),
        }));
        let flags = ffi::SQLITE_UTF8
            | if name == "ag_set_time" {
                ffi::SQLITE_DIRECTONLY
            } else if name == "ag_now" {
                ffi::SQLITE_INNOCUOUS
            } else {
                ffi::SQLITE_INNOCUOUS | ffi::SQLITE_DETERMINISTIC
            };
        // SQLx holds the native handle lock. SQLite owns data (including on registration error)
        // and invokes destroy once, after it no longer uses the callback.
        let result = unsafe {
            ffi::sqlite3_create_function_v2(
                handle.as_raw_handle().as_ptr(),
                c_name.as_ptr(),
                count,
                flags,
                data.cast(),
                Some(invoke),
                None,
                None,
                Some(destroy),
            )
        };
        if result != ffi::SQLITE_OK {
            return Err(sqlx::Error::Protocol(
                "SQLite schema function registration failed".into(),
            ));
        }
    }
    Ok(())
}

pub(super) async fn set_transaction_time(
    connection: &mut SqliteConnection,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT ag_set_time(?)")
        .bind(Utc::now().timestamp_micros())
        .execute(connection)
        .await?;
    Ok(())
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    // This is the unique pointer transferred by register; SQLite calls its destructor once.
    drop(unsafe { Box::from_raw(data.cast::<Function>()) });
}

unsafe extern "C" fn invoke(
    context: *mut ffi::sqlite3_context,
    count: c_int,
    arguments: *mut *mut ffi::sqlite3_value,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SQLite guarantees the registered context and argument array remain valid for this call.
        let function = unsafe { &*ffi::sqlite3_user_data(context).cast::<Function>() };
        let pointers = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(arguments, count as usize) }
        };
        let values: Result<Vec<Option<String>>, ()> = pointers
            .iter()
            .map(|&value| {
                let kind = unsafe { ffi::sqlite3_value_type(value) };
                match kind {
                    ffi::SQLITE_NULL => Ok(None),
                    ffi::SQLITE_INTEGER => {
                        Ok(Some(unsafe { ffi::sqlite3_value_int64(value) }.to_string()))
                    }
                    ffi::SQLITE_TEXT => {
                        let pointer = unsafe { ffi::sqlite3_value_text(value) };
                        if pointer.is_null() {
                            return Err(());
                        }
                        let length = unsafe { ffi::sqlite3_value_bytes(value) };
                        let bytes = unsafe { std::slice::from_raw_parts(pointer, length as usize) };
                        std::str::from_utf8(bytes)
                            .map(|s| Some(s.to_owned()))
                            .map_err(|_| ())
                    }
                    _ => Err(()),
                }
            })
            .collect();
        evaluate(function, &values?)
    }));
    match result {
        Ok(Ok(Output::Null)) => unsafe { ffi::sqlite3_result_null(context) },
        Ok(Ok(Output::Number(value))) => unsafe { ffi::sqlite3_result_int64(context, value) },
        Ok(Ok(Output::Text(value))) => {
            // TRANSIENT copies the bounded UTF-8 bytes before the Rust String is dropped.
            let Ok(length) = c_int::try_from(value.len()) else {
                unsafe { ffi::sqlite3_result_error_toobig(context) };
                return;
            };
            unsafe {
                ffi::sqlite3_result_text(
                    context,
                    value.as_ptr().cast(),
                    length,
                    ffi::SQLITE_TRANSIENT(),
                )
            }
        }
        _ => unsafe {
            ffi::sqlite3_result_error(context, c"SQLite schema value rejected".as_ptr(), -1);
        },
    }
}

enum Output {
    Null,
    Number(i64),
    Text(String),
}
fn boolean(value: bool) -> Output {
    Output::Number(i64::from(value))
}
fn decimal(text: &str) -> Result<Decimal, ()> {
    let value = Decimal::from_str_exact(text).map_err(|_| ())?;
    if value.normalize().to_string() != text {
        return Err(());
    }
    Ok(value)
}
fn json(text: &str) -> Result<Value, ()> {
    serde_json::from_str(text).map_err(|_| ())
}
fn array(text: &str) -> Result<Vec<Value>, ()> {
    match json(text)? {
        Value::Array(values) => Ok(values),
        _ => Err(()),
    }
}

fn evaluate(function: &Function, args: &[Option<String>]) -> Result<Output, ()> {
    let arg = |n: usize| args.get(n).and_then(Option::as_deref).ok_or(());
    if function.name == "ag_now" {
        let now = chrono::DateTime::from_timestamp_micros(function.clock.load(Ordering::Relaxed))
            .ok_or(())?;
        return Ok(Output::Text(timestamp(now)));
    }
    if function.name == "ag_set_time" {
        function
            .clock
            .store(arg(0)?.parse().map_err(|_| ())?, Ordering::Relaxed);
        return Ok(Output::Number(1));
    }
    if args.first().is_some_and(Option::is_none) {
        return Ok(Output::Null);
    }
    Ok(match function.name {
        "ag_uuid_valid" => boolean(parse_uuid(arg(0)?).is_ok()),
        "ag_time_valid" => boolean(parse_timestamp(arg(0)?).is_ok()),
        "ag_date_valid" => boolean(parse_date(arg(0)?).is_ok()),
        "ag_cidr_valid" => boolean(valid_cidr(arg(0)?)),
        "ag_decimal_valid" => {
            let precision: u32 = arg(1)?.parse().map_err(|_| ())?;
            let scale: u32 = arg(2)?.parse().map_err(|_| ())?;
            let valid = decimal(arg(0)?)
                .is_ok_and(|value| super::decimal::fits_precision(value, precision, scale));
            boolean(valid)
        }
        "ag_decimal_cmp" => {
            if args[1].is_none() {
                return Ok(Output::Null);
            }
            Output::Number(match decimal(arg(0)?)?.cmp(&decimal(arg(1)?)?) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
        }
        "ag_array_valid" => {
            let kind = arg(1)?;
            boolean(array(arg(0)?).is_ok_and(|values| {
                values.iter().all(|value| {
                    value.is_null()
                        || value.as_str().is_some_and(|s| match kind {
                            "uuid" => parse_uuid(s).is_ok(),
                            "api_format" => matches!(
                                s,
                                "open_ai_chat_completions" | "open_ai_responses" | "open_ai_images"
                            ),
                            "text" => !s.contains('\0'),
                            _ => false,
                        })
                })
            }))
        }
        "ag_array_contains" | "ag_array_position" => {
            let values = array(arg(0)?)?;
            let needle = args[1]
                .as_ref()
                .map_or(Value::Null, |v| Value::String(v.clone()));
            let position = values.iter().position(|v| *v == needle);
            if function.name == "ag_array_position" {
                position.map_or(Output::Null, |p| Output::Number(p as i64 + 1))
            } else if values.is_empty() {
                boolean(false)
            } else if needle.is_null() || (position.is_none() && values.contains(&Value::Null)) {
                Output::Null
            } else {
                boolean(position.is_some())
            }
        }
        "ag_array_subset" => {
            let right = array(arg(1)?)?;
            boolean(
                array(arg(0)?)?
                    .iter()
                    .all(|v| !v.is_null() && right.contains(v)),
            )
        }
        "ag_json_equal" => {
            if args[1].is_none() {
                return Ok(Output::Null);
            }
            boolean(json(arg(0)?)? == json(arg(1)?)?)
        }
        "ag_json_valid" => boolean(serde_json::from_str::<ValidJson>(arg(0)?).is_ok()),
        "ag_regex" => boolean(
            regex::RegexBuilder::new(arg(0)?)
                .case_insensitive(true)
                .build()
                .map_err(|_| ())?
                .is_match(arg(1)?),
        ),
        // The PG baseline uses simple per-scalar lowercase, not final-sigma or dotted-I expansion.
        "ag_lower" => Output::Text(
            arg(0)?
                .chars()
                .map(|c| c.to_lowercase().next().unwrap_or(c))
                .collect(),
        ),
        "ag_md5_uuid" => {
            let bytes = <md5::Md5 as md5::Digest>::digest(arg(0)?.as_bytes());
            Output::Text(uuid::Uuid::from_slice(&bytes).map_err(|_| ())?.to_string())
        }
        _ => return Err(()),
    })
}

struct ValidJson;

impl<'de> serde::Deserialize<'de> for ValidJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Self)
    }
}

impl<'de> serde::de::Visitor<'de> for ValidJson {
    type Value = Self;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without NUL strings")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self, E> {
        if value.contains('\0') {
            Err(E::custom("JSON string contains NUL"))
        } else {
            Ok(self)
        }
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self, A::Error> {
        while seq.next_element::<Self>()?.is_some() {}
        Ok(self)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self, A::Error> {
        // Validate every occurrence before duplicate keys can discard an earlier value.
        while map.next_entry::<Self, Self>()?.is_some() {}
        Ok(self)
    }
}

fn valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u32>() else {
        return false;
    };
    match address {
        IpAddr::V4(ip) if prefix <= 32 => u32::from(ip).checked_shl(prefix).unwrap_or(0) == 0,
        IpAddr::V6(ip) if prefix <= 128 => u128::from(ip).checked_shl(prefix).unwrap_or(0) == 0,
        _ => false,
    }
}
