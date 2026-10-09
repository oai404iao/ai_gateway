//! Plugin storage contracts shared by isolated PostgreSQL and SQLite fixtures.

use ai_gateway::persistence::{
    ControlPlaneMutation, ControlPlaneRepository, PluginArtifactInput, PluginJobCompletion,
    PluginSaveInput, PluginSettingsInput, RepositoryError,
};
use serde_json::json;
use uuid::Uuid;

const ADMIN: Uuid = Uuid::from_u128(0xa101);
const USER: Uuid = Uuid::from_u128(0xa102);

async fn contract(repository: ControlPlaneRepository) {
    repository
        .ensure_system_settings(super::bootstrap_system_settings())
        .await
        .unwrap();
    let artifact = PluginArtifactInput {
        plugin_id: "fixture-provider".into(),
        digest: "a".repeat(64),
        version: "1.0.0".into(),
        manifest: json!({"id":"fixture-provider","version":"1.0.0","operations":["responses"],"commands":["attempt.body"]}),
    };
    assert!(matches!(
        repository
            .prepare_mutation(
                USER,
                ControlPlaneMutation::RegisterPluginArtifact(artifact.clone())
            )
            .await,
        Err(RepositoryError::InvalidActor)
    ));
    repository
        .register_discovered_plugin(artifact.clone())
        .await
        .unwrap();
    repository
        .register_discovered_plugin(artifact.clone())
        .await
        .unwrap();
    let rows = repository.plugin_records().await.unwrap();
    let state = rows
        .states
        .iter()
        .find(|s| s.plugin_id == artifact.plugin_id)
        .unwrap();
    assert_eq!(state.revision, 1);
    assert!(!state.enabled);
    let settings = PluginSettingsInput {
        schema_version: 1,
        values: json!({"mode":"private-value-not-for-audit"}),
    };
    let input = PluginSaveInput {
        enabled: true,
        artifact_digest: Some(artifact.digest.clone()),
        settings: Some(settings.clone()),
    };
    let mut change = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::SavePlugin {
                plugin_id: artifact.plugin_id.clone(),
                input: input.clone(),
                expected_revision: 1,
            },
        )
        .await
        .unwrap();
    let pending = change.runtime_records().await.unwrap();
    assert_eq!(
        pending
            .plugin_records
            .settings
            .iter()
            .find(|s| s.plugin_id == artifact.plugin_id)
            .unwrap()
            .values,
        settings.values
    );
    change.rollback().await.unwrap();
    assert!(
        repository
            .plugin_settings(&artifact.plugin_id)
            .await
            .unwrap()
            .is_none()
    );
    let (mutations, _) = repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::SavePlugin {
                plugin_id: artifact.plugin_id.clone(),
                input: input.clone(),
                expected_revision: 1,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(
        !mutations[0]
            .after_redacted
            .to_string()
            .contains("private-value-not-for-audit")
    );
    assert!(matches!(
        repository
            .prepare_mutation(
                ADMIN,
                ControlPlaneMutation::SavePlugin {
                    plugin_id: artifact.plugin_id.clone(),
                    input,
                    expected_revision: 1,
                }
            )
            .await,
        Err(RepositoryError::Conflict)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                ADMIN,
                ControlPlaneMutation::DeletePluginArtifact {
                    plugin_id: artifact.plugin_id.clone(),
                    digest: artifact.digest.clone(),
                    expected_revision: 2,
                }
            )
            .await,
        Err(RepositoryError::Conflict)
    ));
    repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::SavePlugin {
                plugin_id: artifact.plugin_id.clone(),
                input: PluginSaveInput {
                    enabled: false,
                    artifact_digest: None,
                    settings: None,
                },
                expected_revision: 2,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::DeletePluginArtifact {
                plugin_id: artifact.plugin_id.clone(),
                digest: artifact.digest.clone(),
                expected_revision: 3,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    repository
        .register_discovered_plugin(artifact.clone())
        .await
        .unwrap();
    assert!(
        !repository
            .plugin_artifacts()
            .await
            .unwrap()
            .iter()
            .any(|a| a.plugin_id == artifact.plugin_id)
    );
    assert_eq!(
        repository
            .plugin_settings(&artifact.plugin_id)
            .await
            .unwrap()
            .unwrap()
            .values,
        settings.values
    );
    repository
        .prepare_mutation(
            ADMIN,
            ControlPlaneMutation::RegisterPluginArtifact(artifact.clone()),
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(
        repository
            .plugin_artifacts()
            .await
            .unwrap()
            .iter()
            .any(|a| a.plugin_id == artifact.plugin_id)
    );

    let job = Uuid::new_v4();
    assert!(matches!(
        repository
            .create_plugin_install_job(USER, job, "install")
            .await,
        Err(RepositoryError::InvalidActor)
    ));
    assert_eq!(
        repository
            .create_plugin_install_job(ADMIN, job, "install")
            .await
            .unwrap()
            .status,
        "queued"
    );
    assert_eq!(
        repository
            .claim_plugin_install_job(ADMIN, job)
            .await
            .unwrap()
            .status,
        "running"
    );
    assert!(matches!(
        repository.claim_plugin_install_job(ADMIN, job).await,
        Err(RepositoryError::Conflict)
    ));
    let complete = PluginJobCompletion {
        plugin_id: Some(artifact.plugin_id.clone()),
        artifact_digest: Some(artifact.digest),
        error_code: None,
    };
    assert_eq!(
        repository
            .finish_plugin_install_job(ADMIN, job, complete.clone())
            .await
            .unwrap()
            .status,
        "succeeded"
    );
    assert!(matches!(
        repository
            .finish_plugin_install_job(ADMIN, job, complete)
            .await,
        Err(RepositoryError::Conflict)
    ));
    let queued = Uuid::new_v4();
    let running = Uuid::new_v4();
    repository
        .create_plugin_install_job(ADMIN, queued, "discover")
        .await
        .unwrap();
    repository
        .create_plugin_install_job(ADMIN, running, "install")
        .await
        .unwrap();
    repository
        .claim_plugin_install_job(ADMIN, running)
        .await
        .unwrap();
    assert_eq!(repository.interrupt_plugin_install_jobs().await.unwrap(), 2);
    assert_eq!(repository.interrupt_plugin_install_jobs().await.unwrap(), 0);
    for id in [queued, running] {
        let job = repository.plugin_install_job(id).await.unwrap().unwrap();
        assert_eq!(job.status, "failed");
        assert_eq!(job.error_code.as_deref(), Some("interrupted"));
    }
    let abandoned = Uuid::new_v4();
    repository
        .create_plugin_install_job(ADMIN, abandoned, "install")
        .await
        .unwrap();
    assert_eq!(
        repository
            .fail_plugin_install_job(abandoned, "actor_revoked")
            .await
            .unwrap()
            .status,
        "failed"
    );
    assert!(matches!(
        repository
            .fail_plugin_install_job(job, "actor_revoked")
            .await,
        Err(RepositoryError::Conflict)
    ));
    let failed_install = Uuid::new_v4();
    repository
        .create_plugin_install_job(ADMIN, failed_install, "install")
        .await
        .unwrap();
    repository
        .fail_plugin_install_job(failed_install, "plugin_install_failed")
        .await
        .unwrap();
    assert_eq!(
        repository
            .plugin_install_job(failed_install)
            .await
            .unwrap()
            .unwrap()
            .error_code
            .as_deref(),
        Some("plugin_install_failed")
    );
}

#[tokio::test]
async fn postgres_plugin_storage_contract() {
    let database = super::TestDatabase::new().await;
    for (id, role) in [(ADMIN, "admin"), (USER, "user")] {
        sqlx::query(
            "INSERT INTO users(id,email,display_name,role,status) VALUES ($1,$2,$4,$3,'active')",
        )
        .bind(id)
        .bind(format!("{id}@example.test"))
        .bind(role)
        .bind(format!("Plugin {role}"))
        .execute(&database.pool)
        .await
        .unwrap();
    }
    contract(ControlPlaneRepository::new(database.pool.clone())).await;
    database.cleanup().await;
}

#[cfg(feature = "sqlite-backend")]
#[tokio::test]
async fn sqlite_plugin_storage_and_migration_contract() {
    use ai_gateway::persistence::sqlite::{SqliteDatabase, SqliteUuid};
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let database = Arc::new(
        SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .unwrap(),
    );
    database.install_schema().await.unwrap();
    let mut tx = database.begin_write().await.unwrap();
    for (id, role) in [(ADMIN, "admin"), (USER, "user")] {
        sqlx::query(
            "INSERT INTO users(id,email,display_name,role,status) VALUES (?1,?2,?4,?3,'active')",
        )
        .bind(SqliteUuid(id))
        .bind(format!("{id}@example.test"))
        .bind(role)
        .bind(format!("Plugin {role}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let repository = ControlPlaneRepository::from_sqlite(Arc::clone(&database));
    contract(repository.clone()).await;
    let legacy = json!({
        "workspace_path":"/custom/workspace","git_remote_url":"https://github.com/example/custom",
        "originator":"custom","client_version":"9.8.7","user_agent":"custom/9.8.7"
    });
    let mut tx = database.begin_write().await.unwrap();
    sqlx::raw_sql("DROP TABLE plugin_install_jobs; DROP TABLE plugin_settings; DROP TABLE plugin_states; DROP TABLE plugin_artifacts; DELETE FROM _gateway_sqlite_migrations WHERE version=11;")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE system_settings SET value=json_set(value,'$.codex',json(?1)),updated_at=ag_now() WHERE setting_key='forwarding_policy'")
        .bind(legacy.to_string()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    database.install_schema().await.unwrap();
    let settings = repository.plugin_settings("codex").await.unwrap().unwrap();
    assert_eq!(settings.values, legacy);
    assert_eq!(settings.schema_version, 1);
    assert_eq!(settings.revision, 1);
    repository
        .ensure_system_settings(super::bootstrap_system_settings())
        .await
        .unwrap();
    assert_eq!(
        repository
            .plugin_settings("codex")
            .await
            .unwrap()
            .unwrap()
            .values,
        legacy
    );
    assert!(
        serde_json::to_value(repository.system_settings().await.unwrap())
            .unwrap()
            .get("codex")
            .is_none()
    );
    database.close().await;
}
