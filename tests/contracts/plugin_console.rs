//! Native plugin administration must remain generic, versioned and reauthorized.

use super::*;
use ai_gateway::connector_plugins::{DirectoryPluginCatalog, Plugin};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

fn response_etag(response: &axum::response::Response) -> String {
    response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned()
}

async fn grant(app: &App) -> String {
    let response = request(
        app,
        "POST",
        "/console/v1/plugins/reauth",
        json!({"password":TEST_PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn wait_job(app: &App, id: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response = request(
                app,
                "GET",
                &format!("/console/v1/plugins/jobs/{id}"),
                json!({}),
                &[],
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let value = body_json(response).await;
            if matches!(value["status"].as_str(), Some("succeeded" | "failed")) {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("plugin job finishes")
}

fn package(directory: &Path) -> (Vec<u8>, String) {
    package_version(directory, "1.0.0")
}

fn package_version(directory: &Path, version: &str) -> (Vec<u8>, String) {
    let path = directory.join(format!("fixture-{version}.so"));
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-DFIXTURE_SETTINGS"])
        .arg(format!("-DFIXTURE_VERSION=\"{version}\""))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/connector-sdk/tests/fixture.c"))
        .arg("-o")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let library = std::fs::read(&path).unwrap();
    let digest = hex_digest(&library);
    let plugin = Plugin::load(&path, &digest, "fixture").unwrap();
    let manifest = serde_json::to_vec(plugin.manifest()).unwrap();
    let build = serde_json::to_vec(&json!({
        "schema_version":1,"connector_abi":ai_gateway_connector_sdk::ABI_VERSION,
        "connector_version":plugin.manifest().version,
        "target":format!("{}-unknown-linux-gnu",std::env::consts::ARCH),
        "library":"connector.so","library_sha256":digest,
    }))
    .unwrap();
    let mut files = vec![
        ("connector.so", library),
        ("manifest.json", manifest),
        ("build-info.json", build),
        ("LICENSE", b"Fixture license".to_vec()),
        ("THIRD_PARTY_NOTICES.md", b"Fixture notices".to_vec()),
        ("LICENSES/fixture/LICENSE", b"Fixture license".to_vec()),
    ];
    let sums = files
        .iter()
        .map(|(name, data)| format!("{}  {name}\n", hex_digest(data)))
        .collect::<String>();
    files.push(("SHA256SUMS", sums.into_bytes()));
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    for (name, data) in files {
        let mut header = tar::Header::new_ustar();
        header.set_size(data.len() as u64);
        header.set_mode(0o444);
        header.set_cksum();
        archive
            .append_data(&mut header, format!("package/{name}"), data.as_slice())
            .unwrap();
    }
    (archive.into_inner().unwrap().finish().unwrap(), digest)
}

#[tokio::test]
async fn plugin_console_hot_upgrade_pins_old_code_and_preserves_deleted_inventory() {
    let database = TestDatabase::new().await;
    let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let catalog = Arc::new(DirectoryPluginCatalog::open(directory.path().join("plugins")).unwrap());
    let app = app_with_options(database.pool.clone(), None, Some(catalog)).await;
    let path = "/console/v1/plugins/fixture";
    let mut digests = Vec::new();
    let mut last_archive = Vec::new();
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let (archive, digest) = package_version(directory.path(), version);
        let response = upload(&app, &grant(&app).await, archive.clone()).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let job = body_json(response).await;
        assert_eq!(
            wait_job(&app, job["id"].as_str().unwrap()).await["status"],
            "succeeded"
        );
        digests.push(digest);
        last_archive = archive;
    }
    let detail = request(&app, "GET", path, json!({}), &[]).await;
    let mut etag = response_etag(&detail);
    for index in [0, 1, 0] {
        let token = grant(&app).await;
        let response = request(
            &app,
            "PUT",
            &format!("{path}/state"),
            json!({"enabled":true,"artifact_digest":digests[index]}),
            &[("if-match", &etag), ("x-plugin-authorization", &token)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = app.runtime.snapshot();
        let pinned = snapshot.plugins().get("fixture").unwrap();
        assert_eq!(
            pinned.manifest().version,
            if index == 0 { "1.0.0" } else { "2.0.0" }
        );
        let response = request(&app, "GET", &format!("{path}/settings"), json!({}), &[]).await;
        let previous_etag = etag;
        etag = response_etag(&response);
        assert_ne!(etag, previous_etag);
        assert_eq!(
            body_json(response).await["values"],
            json!({"mode":"default"})
        );
        let token = grant(&app).await;
        assert_eq!(
            request(
                &app,
                "PUT",
                &format!("{path}/settings"),
                json!({"schema_version":1,"values":{"mode":"alternate"}}),
                &[
                    ("if-match", &previous_etag),
                    ("x-plugin-authorization", &token)
                ]
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        // A published replacement cannot mutate a caller's already pinned instance.
        let token = grant(&app).await;
        assert_eq!(
            request(
                &app,
                "PUT",
                &format!("{path}/settings"),
                json!({"schema_version":1,"values":{"mode":"alternate"}}),
                &[("if-match", &etag), ("x-plugin-authorization", &token)]
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            pinned.call("echo", &json!({}), &[]).unwrap().metadata["settings"]["mode"],
            "default"
        );
        let response = request(&app, "GET", &format!("{path}/settings"), json!({}), &[]).await;
        etag = response_etag(&response);
        let token = grant(&app).await;
        assert_eq!(
            request(
                &app,
                "PUT",
                &format!("{path}/settings"),
                json!({"schema_version":1,"values":{"mode":"default"}}),
                &[("if-match", &etag), ("x-plugin-authorization", &token)]
            )
            .await
            .status(),
            StatusCode::OK
        );
        let response = request(&app, "GET", path, json!({}), &[]).await;
        etag = response_etag(&response);
    }
    let token = grant(&app).await;
    let deleted = request(
        &app,
        "DELETE",
        &format!("{path}/artifacts/{}", digests[2]),
        json!({}),
        &[("if-match", &etag), ("x-plugin-authorization", &token)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let token = grant(&app).await;
    let response = request(
        &app,
        "POST",
        "/console/v1/plugins/discover",
        json!({}),
        &[("x-plugin-authorization", &token)],
    )
    .await;
    let job = body_json(response).await;
    assert_eq!(
        wait_job(&app, job["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let detail = body_json(request(&app, "GET", path, json!({}), &[]).await).await;
    assert_eq!(detail["artifacts"].as_array().unwrap().len(), 2);
    let response = upload(&app, &grant(&app).await, last_archive).await;
    let job = body_json(response).await;
    assert_eq!(
        wait_job(&app, job["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let detail = body_json(request(&app, "GET", path, json!({}), &[]).await).await;
    assert_eq!(detail["artifacts"].as_array().unwrap().len(), 3);
    database.cleanup().await;
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn upload(app: &App, token: &str, bytes: Vec<u8>) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/v1/plugins/install")
                .header("authorization", format!("Bearer {}", app.access_token))
                .header("x-plugin-authorization", token)
                .header("content-type", "application/octet-stream")
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn plugin_console_reauthorization_is_admin_only_single_use_and_session_bound() {
    let database = TestDatabase::new().await;
    let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let catalog = Arc::new(DirectoryPluginCatalog::open(directory.path().join("plugins")).unwrap());
    let app = app_with_options(database.pool.clone(), None, Some(catalog)).await;
    let wrong = request(
        &app,
        "POST",
        "/console/v1/plugins/reauth",
        json!({"password":"wrong-password-long-enough"}),
        &[],
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        request(&app, "POST", "/console/v1/plugins/discover", json!({}), &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let token = grant(&app).await;
    let second = app
        .auth
        .login_with_user_agent(
            format!("spec-user-{}@example.test", app.user_id),
            TEST_PASSWORD.into(),
            Some("Second session".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        request_with_token(
            &app,
            &second.access_token,
            "POST",
            "/console/v1/plugins/discover",
            json!({}),
            &[("x-plugin-authorization", &token)]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/console/v1/plugins/discover",
            json!({}),
            &[("x-plugin-authorization", &token)]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let token = grant(&app).await;
    let discovered = request(
        &app,
        "POST",
        "/console/v1/plugins/discover",
        json!({}),
        &[("x-plugin-authorization", &token)],
    )
    .await;
    assert_eq!(discovered.status(), StatusCode::ACCEPTED);
    let job = body_json(discovered).await;
    assert_eq!(
        wait_job(&app, job["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/console/v1/plugins/discover",
            json!({}),
            &[("x-plugin-authorization", &token)]
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );

    let user = Uuid::new_v4();
    let email = format!("{user}@example.test");
    let password = hash_console_password(TEST_PASSWORD.into()).await.unwrap();
    sqlx::query("INSERT INTO users(id,email,display_name,role,status,password_hash) VALUES ($1,$2,$2,'user','active',$3)")
        .bind(user).bind(&email).bind(password).execute(&database.pool).await.unwrap();
    let session = app
        .auth
        .login_with_user_agent(email, TEST_PASSWORD.into(), None)
        .await
        .unwrap();
    for (method, path, body) in [
        ("GET", "/console/v1/plugins", json!({})),
        (
            "POST",
            "/console/v1/plugins/reauth",
            json!({"password":TEST_PASSWORD}),
        ),
        ("POST", "/console/v1/plugins/discover", json!({})),
        ("POST", "/console/v1/plugins/install", json!({})),
    ] {
        assert_eq!(
            request_with_token(&app, &session.access_token, method, path, body, &[])
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    database.cleanup().await;
}

#[tokio::test]
async fn plugin_upload_is_removed_when_actor_is_revoked_before_job_claim() {
    let database = TestDatabase::new().await;
    let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let catalog = Arc::new(DirectoryPluginCatalog::open(directory.path().join("plugins")).unwrap());
    let app = app_with_options(database.pool.clone(), None, Some(Arc::clone(&catalog))).await;
    sqlx::raw_sql(
        "CREATE FUNCTION revoke_plugin_installer() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN UPDATE users SET status='suspended' WHERE id=NEW.actor_user_id; RETURN NEW; END $$;
         CREATE TRIGGER revoke_plugin_installer AFTER INSERT ON plugin_install_jobs
         FOR EACH ROW EXECUTE FUNCTION revoke_plugin_installer();",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let response = upload(&app, &grant(&app).await, b"not a real package".to_vec()).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let id = body_json(response).await["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status: String =
                sqlx::query_scalar("SELECT status FROM plugin_install_jobs WHERE id=$1")
                    .bind(id)
                    .fetch_one(&database.pool)
                    .await
                    .unwrap();
            if status == "failed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !catalog
            .root()
            .join("staging")
            .join(format!("{id}.tar.gz"))
            .exists()
    );
    assert!(app.runtime.snapshot().plugins().get("fixture").is_none());
    database.cleanup().await;
}

#[tokio::test]
async fn plugin_console_install_activate_settings_cas_and_disable_missing_artifact() {
    let database = TestDatabase::new().await;
    let directory = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let catalog = Arc::new(DirectoryPluginCatalog::open(directory.path().join("plugins")).unwrap());
    let app = app_with_options(database.pool.clone(), None, Some(Arc::clone(&catalog))).await;
    let (archive, digest) = package(directory.path());
    let token = grant(&app).await;
    let response = upload(&app, &token, archive.clone()).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let job = body_json(response).await;
    assert_eq!(
        wait_job(&app, job["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_eq!(
        upload(&app, &token, archive).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let detail = request(&app, "GET", "/console/v1/plugins/fixture", json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let value = body_json(detail).await;
    assert_eq!(value["status"], "disabled");
    assert!(!catalog.is_loaded(&digest));
    let token = grant(&app).await;
    let activated = request(
        &app,
        "PUT",
        "/console/v1/plugins/fixture/state",
        json!({"enabled":true,"artifact_digest":digest}),
        &[("if-match", &etag), ("x-plugin-authorization", &token)],
    )
    .await;
    assert_eq!(activated.status(), StatusCode::OK);
    assert!(app.runtime.snapshot().plugins().get("fixture").is_some());
    let response = request(
        &app,
        "GET",
        "/console/v1/plugins/fixture/settings",
        json!({}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let value = body_json(response).await;
    assert_eq!(value["values"], json!({"mode":"default"}));
    assert_eq!(value["descriptor"]["fields"][0]["key"], "mode");
    assert!(value.get("codex").is_none());
    let token = grant(&app).await;
    let response = request(
        &app,
        "PUT",
        "/console/v1/plugins/fixture/settings",
        json!({"schema_version":1,"values":{"mode":"alternate"}}),
        &[("if-match", &etag), ("x-plugin-authorization", &token)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = grant(&app).await;
    assert_eq!(
        request(
            &app,
            "PUT",
            "/console/v1/plugins/fixture/settings",
            json!({"schema_version":1,"values":{"mode":"default"}}),
            &[("if-match", &etag), ("x-plugin-authorization", &token)]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let response = request(
        &app,
        "GET",
        "/console/v1/plugins/fixture/settings",
        json!({}),
        &[],
    )
    .await;
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        body_json(response).await["values"],
        json!({"mode":"alternate"})
    );
    let token = grant(&app).await;
    assert_eq!(
        request(
            &app,
            "PUT",
            "/console/v1/plugins/fixture/settings",
            json!({"schema_version":1,"values":{"mode":"alternate","unknown":true}}),
            &[("if-match", &etag), ("x-plugin-authorization", &token)]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let artifact = catalog.resolve("fixture", &digest).unwrap();
    std::fs::remove_file(artifact.path).unwrap();
    let token = grant(&app).await;
    let disabled = request(
        &app,
        "PUT",
        "/console/v1/plugins/fixture/state",
        json!({"enabled":false,"artifact_digest":digest}),
        &[("if-match", &etag), ("x-plugin-authorization", &token)],
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    assert!(app.runtime.snapshot().plugins().get("fixture").is_none());
    let token = grant(&app).await;
    let bad = upload(&app, &token, b"not a package".to_vec()).await;
    assert_eq!(bad.status(), StatusCode::ACCEPTED);
    let job = body_json(bad).await;
    assert_eq!(
        wait_job(&app, job["id"].as_str().unwrap()).await["status"],
        "failed"
    );
    database.cleanup().await;
}
