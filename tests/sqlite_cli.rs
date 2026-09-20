//! Production CLI lifecycle and offline backup rejection contracts on real files.

#![cfg(all(feature = "sqlite-backend", target_os = "linux"))]

use ai_gateway::runtime_config::AppConfig;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn config(root: &Path) -> PathBuf {
    fs::create_dir(root.join("database")).unwrap();
    fs::set_permissions(root.join("database"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join("spool")).unwrap();
    fs::set_permissions(root.join("spool"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut value: toml::Value = toml::from_str(include_str!("../config.example.toml")).unwrap();
    let db = value["database"].as_table_mut().unwrap();
    db.insert(
        "url".into(),
        toml::Value::String(format!(
            "sqlite://{}",
            root.join("database/gateway.sqlite").display()
        )),
    );
    db.remove("password_file");
    db.insert("max_connections".into(), toml::Value::Integer(5));
    value["request_logging"]["spool_directory"] = "./spool".into();
    let path = root.join("gateway.toml");
    fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
    path
}

fn command(args: &[&str], password: bool) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ai-gateway"));
    if let Some(index) = args.iter().position(|arg| *arg == "--config") {
        cmd.current_dir(Path::new(args[index + 1]).parent().unwrap());
    }
    let mut process = cmd
        .args(args)
        .stdin(if password {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if password {
        process
            .stdin
            .take()
            .unwrap()
            .write_all(b"fixture-password-not-a-real-secret-1234\n")
            .unwrap();
    }
    process.wait_with_output().unwrap()
}

#[test]
fn administrator_and_paired_snapshot_commands_reopen_and_reject_damage() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = directory.path();
    let config = config(root);
    let config = config.to_str().unwrap();
    let output = command(
        &[
            "bootstrap-admin",
            "--config",
            config,
            "--email",
            "admin@example.test",
            "--display-name",
            "Admin",
            "--password-stdin",
        ],
        true,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        command(
            &[
                "reset-admin-password",
                "--config",
                config,
                "--email",
                "admin@example.test",
                "--password-stdin"
            ],
            true
        )
        .status
        .success()
    );
    assert!(
        !command(
            &[
                "bootstrap-admin",
                "--config",
                config,
                "--email",
                "second@example.test",
                "--display-name",
                "Second",
                "--password-stdin"
            ],
            true
        )
        .status
        .success()
    );
    fs::write(
        root.join("spool/uncertain-fixture"),
        b"retained fixture bytes",
    )
    .unwrap();
    fs::set_permissions(
        root.join("spool/uncertain-fixture"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::write(root.join("spool/writer.lock"), b"").unwrap();
    fs::set_permissions(
        root.join("spool/writer.lock"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let snapshot = root.join("snapshot");
    let output = command(
        &[
            "backup-sqlite",
            "--config",
            config,
            "--destination",
            snapshot.to_str().unwrap(),
        ],
        false,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !command(
            &[
                "backup-sqlite",
                "--config",
                config,
                "--destination",
                snapshot.to_str().unwrap()
            ],
            false
        )
        .status
        .success()
    );
    let restored = root.join("restored");
    let args = [
        "restore-sqlite",
        "--source",
        snapshot.to_str().unwrap(),
        "--destination",
        restored.to_str().unwrap(),
    ];
    let output = command(&args, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(restored.join("spool/uncertain-fixture")).unwrap(),
        b"retained fixture bytes"
    );
    assert!(!command(&args, false).status.success());
    fs::write(snapshot.join("spool/uncertain-fixture"), b"corrupt").unwrap();
    let failed = root.join("failed");
    assert!(
        !command(
            &[
                "restore-sqlite",
                "--source",
                snapshot.to_str().unwrap(),
                "--destination",
                failed.to_str().unwrap()
            ],
            false
        )
        .status
        .success()
    );
    assert!(!failed.exists());
}

#[test]
fn sqlite_configuration_is_explicit_and_never_falls_back_to_postgres() {
    for url in [
        "sqlite::memory:",
        "sqlite:///:memory:",
        "sqlite://relative",
        "sqlite:///",
        "sqlite://localhost/a.sqlite",
        "sqlite:///a.sqlite?mode=memory",
        "sqlite:///a.sqlite#x",
        "sqlite:///a/../b.sqlite",
        "sqlite:///a/%2e%2e/b.sqlite",
        "sqlite:///a%00b.sqlite",
        "sqlite:///a\\b.sqlite",
    ] {
        let mut config: AppConfig = toml::from_str(include_str!("../config.example.toml")).unwrap();
        config.database.password_file = None;
        config.database.url = url.into();
        assert!(config.validate().is_err(), "{url}");
    }
    let mut config: AppConfig = toml::from_str(include_str!("../config.example.toml")).unwrap();
    config.database.url = "sqlite:///owned/gateway.sqlite".into();
    config.database.password_file = Some("postgres-password".into());
    assert!(config.database.sqlite_path().is_err());
    config.database.password_file = None;
    config.database.max_connections = 1;
    assert!(config.database.sqlite_path().is_err());
    config.database.max_connections = 5;
    assert_eq!(
        config.database.sqlite_path().unwrap(),
        Some(PathBuf::from("/owned/gateway.sqlite"))
    );
    assert!(config.database.connect_options().is_err());
    assert!(config.validate().is_ok());
}
