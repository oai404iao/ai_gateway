//! Administrative connection for creating disposable PostgreSQL databases.

use reqwest::Url;
use std::{env, ffi::OsString, fs};

pub fn admin_url(override_url: Option<&str>) -> Result<Url, String> {
    let configured = override_url
        .map(OsString::from)
        .or_else(|| env::var_os("TEST_DATABASE_ADMIN_URL"));
    let url = if let Some(value) = configured {
        let value = value
            .to_str()
            .ok_or("TEST_DATABASE_ADMIN_URL must be valid UTF-8")?;
        Url::parse(value).map_err(|_| "invalid PostgreSQL admin URL")?
    } else {
        let password = fs::read_to_string("config/postgres-password").map_err(|error| {
            format!("set TEST_DATABASE_ADMIN_URL or supply config/postgres-password: {error}")
        })?;
        let password = password.trim_end_matches(['\r', '\n']);
        if password.is_empty() {
            return Err("config/postgres-password must not be empty".into());
        }
        let mut url = Url::parse("postgres://ai_gateway@127.0.0.1:5432/postgres").unwrap();
        // Url::set_password escapes delimiters but preserves existing percent escapes.
        url.set_password(Some(&password.replace('%', "%25")))
            .unwrap();
        url
    };
    if matches!(url.path().trim_matches('/'), "" | "ai_gateway") {
        return Err(
            "database admin URL must target an administrative database such as postgres".into(),
        );
    }
    Ok(url)
}
