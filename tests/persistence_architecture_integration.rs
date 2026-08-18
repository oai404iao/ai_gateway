//! Durable M1 guards for the backend-neutral persistence boundary.
//!
//! These checks intentionally match Rust paths and concrete SQLx type
//! identifiers rather than ordinary words such as "PostgreSQL" or "SQLite".

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use ai_gateway::persistence::{
    AuthRepository, ControlPlaneRepository, DatabaseConnectOptions, DatabasePool, RepositoryError,
    RepositoryTransaction, RequestLogRepository, TransactionIntent,
};

const CONCRETE_SQLX_IDENTIFIERS: &[&str] = &[
    "sqlx",
    "FromRow",
    "Postgres",
    "PgArguments",
    "PgConnectOptions",
    "PgConnection",
    "PgDatabaseError",
    "PgPool",
    "PgPoolOptions",
    "PgRow",
    "PgTypeInfo",
    "PgValueRef",
    "Sqlite",
    "SqliteArguments",
    "SqliteConnectOptions",
    "SqliteConnection",
    "SqliteError",
    "SqlitePool",
    "SqlitePoolOptions",
    "SqliteRow",
    "SqliteTypeInfo",
    "SqliteValueRef",
];

const REPOSITORY_SIGNATURE_FIXTURE: &str = "tests/fixtures/persistence-repository-signatures.txt";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MethodContract {
    family: String,
    visibility: String,
    asyncness: String,
    name: String,
    signature: String,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_source(relative: impl AsRef<Path>) -> String {
    let relative = relative.as_ref();
    fs::read_to_string(manifest_dir().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", relative.display()))
}

fn rust_sources_below(relative: &str) -> Vec<PathBuf> {
    fn visit(directory: &Path, output: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, output);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                output.push(path);
            }
        }
    }

    let mut sources = Vec::new();
    visit(&manifest_dir().join(relative), &mut sources);
    sources.sort();
    sources
}

fn identifiers(source: &str) -> BTreeSet<&str> {
    source
        .split(|character: char| !(character == '_' || character.is_ascii_alphanumeric()))
        .filter(|identifier| !identifier.is_empty())
        .collect()
}

fn assert_no_concrete_sqlx_identifiers(path: &Path, source: &str) {
    let found = identifiers(source)
        .intersection(
            &CONCRETE_SQLX_IDENTIFIERS
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
        )
        .copied()
        .collect::<Vec<_>>();
    assert!(
        found.is_empty(),
        "{} directly mentions concrete SQLx identifiers: {}",
        path.display(),
        found.join(", ")
    );
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn visible_method_contracts(family: &str, source: &str) -> Vec<MethodContract> {
    let mut contracts = Vec::new();
    let mut line_offset = 0;

    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let leading_whitespace = line.len() - trimmed.len();
        let candidate_offset = line_offset + leading_whitespace;
        line_offset += line.len();

        if !(trimmed.starts_with("pub ") || trimmed.starts_with("pub(crate)")) {
            continue;
        }

        let candidate = &source[candidate_offset..];
        let body_brace = candidate
            .find('{')
            .unwrap_or_else(|| panic!("visible declaration has no opening brace: {trimmed}"));
        let signature = normalize_whitespace(&candidate[..=body_brace]);
        let (visibility, after_visibility) =
            if let Some(after_visibility) = signature.strip_prefix("pub(crate) ") {
                ("pub(crate)", after_visibility)
            } else if let Some(after_visibility) = signature.strip_prefix("pub ") {
                ("pub", after_visibility)
            } else {
                continue;
            };
        let (asyncness, after_asyncness) =
            if let Some(after_asyncness) = after_visibility.strip_prefix("async ") {
                ("async", after_asyncness)
            } else {
                ("sync", after_visibility)
            };
        let Some(after_fn) = after_asyncness.strip_prefix("fn ") else {
            continue;
        };
        let name = after_fn
            .split(|character: char| !(character == '_' || character.is_ascii_alphanumeric()))
            .next()
            .expect("method name");

        contracts.push(MethodContract {
            family: family.to_owned(),
            visibility: visibility.to_owned(),
            asyncness: asyncness.to_owned(),
            name: name.to_owned(),
            signature,
        });
    }

    contracts
}

fn fixture_method_contracts() -> Vec<MethodContract> {
    read_source(REPOSITORY_SIGNATURE_FIXTURE)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.splitn(5, '\t');
            let family = fields.next().expect("fixture family");
            let visibility = fields.next().expect("fixture visibility");
            let asyncness = fields.next().expect("fixture asyncness");
            let name = fields.next().expect("fixture method name");
            let signature = fields.next().expect("fixture full signature");
            assert!(
                fields.next().is_none(),
                "too many fixture fields for {family}::{name}"
            );
            MethodContract {
                family: family.to_owned(),
                visibility: visibility.to_owned(),
                asyncness: asyncness.to_owned(),
                name: name.to_owned(),
                signature: normalize_whitespace(signature),
            }
        })
        .collect()
}

#[test]
fn production_callers_do_not_depend_on_concrete_sqlx_backends() {
    let mut sources = [
        "src/application",
        "src/http",
        "src/mcp",
        "src/workers",
        "src/runtime_config",
    ]
    .into_iter()
    .flat_map(rust_sources_below)
    .collect::<Vec<_>>();
    sources.extend([
        manifest_dir().join("src/main.rs"),
        manifest_dir().join("src/lib.rs"),
    ]);
    sources.sort();

    for path in sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_no_concrete_sqlx_identifiers(&path, &source);
    }
}

#[test]
fn neutral_contract_modules_do_not_depend_on_sqlx_shapes() {
    for relative in [
        "src/persistence/auth.rs",
        "src/persistence/codex.rs",
        "src/persistence/control_plane.rs",
        "src/persistence/request_log.rs",
        "src/persistence/records.rs",
        "src/persistence/error.rs",
    ] {
        assert_no_concrete_sqlx_identifiers(Path::new(relative), &read_source(relative));
    }
}

#[test]
fn persistence_root_keeps_backend_modules_private_and_avoids_glob_exports() {
    let source = read_source("src/persistence.rs");
    for line in source.lines() {
        let compact = line.split_ascii_whitespace().collect::<String>();
        assert!(
            compact != "pubmodpostgres;" && compact != "pubmodsqlite;",
            "src/persistence.rs must keep concrete backend modules private: {line}"
        );
        assert!(
            compact != "pubusepostgres::*;" && compact != "pubuseself::postgres::*;",
            "src/persistence.rs must not glob-export PostgreSQL implementation details: {line}"
        );
    }
}

#[test]
fn postgres_modules_only_publish_the_temporary_repository_implementations() {
    let allowed = [
        "AuthRepository",
        "ControlPlaneRepository",
        "RequestLogRepository",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    let mut found = BTreeSet::new();

    for path in rust_sources_below("src/persistence/postgres") {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        for line in source.lines().map(str::trim_start) {
            let Some(declaration) = line
                .strip_prefix("pub struct ")
                .or_else(|| line.strip_prefix("pub enum "))
            else {
                continue;
            };
            let name = declaration
                .split(|character: char| !(character == '_' || character.is_ascii_alphanumeric()))
                .next()
                .expect("public type name");
            assert!(
                allowed.contains(name),
                "{} defines public backend contract type {name}",
                path.display()
            );
            found.insert(name.to_owned());
        }
    }

    assert_eq!(found, allowed);
}

#[test]
fn established_persistence_names_remain_publicly_importable() {
    fn assert_public_type<T>() {}

    assert_public_type::<DatabaseConnectOptions>();
    assert_public_type::<DatabasePool>();
    assert_public_type::<RepositoryTransaction<'static>>();
    assert_public_type::<AuthRepository>();
    assert_public_type::<ControlPlaneRepository>();
    assert_public_type::<RequestLogRepository>();
    assert_public_type::<RepositoryError>();
    assert_public_type::<TransactionIntent>();
}

#[test]
fn m1_repository_operation_ledger_remains_exactly_104_methods() {
    let auth = read_source("src/persistence/postgres/auth.rs");
    let mut actual = visible_method_contracts("auth", &auth)
        .into_iter()
        .filter(|method| method.name != "new")
        .collect::<Vec<_>>();

    let postgres = read_source("src/persistence/postgres/mod.rs");
    let control_plane_impl = postgres
        .find("impl ControlPlaneRepository {")
        .expect("ControlPlaneRepository implementation");
    let (request_log, control_plane) = postgres.split_at(control_plane_impl);
    actual.extend(
        visible_method_contracts("request_log", request_log)
            .into_iter()
            .filter(|method| method.name != "new"),
    );
    actual.extend(
        visible_method_contracts("control_plane", control_plane)
            .into_iter()
            .filter(|method| method.name != "new" && method.name != "proxy_record"),
    );

    let codex = read_source("src/persistence/postgres/codex.rs");
    actual.extend(visible_method_contracts("codex", &codex));

    let expected = fixture_method_contracts();
    assert_eq!(actual.len(), 104, "actual repository operation count");
    assert_eq!(expected.len(), 104, "fixture repository operation count");

    let actual_set = actual.iter().cloned().collect::<BTreeSet<_>>();
    let expected_set = expected.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        actual_set.len(),
        actual.len(),
        "duplicate repository operation signature"
    );
    assert_eq!(
        expected_set.len(),
        expected.len(),
        "duplicate fixture repository operation signature"
    );

    assert_eq!(actual_set, expected_set);
}
