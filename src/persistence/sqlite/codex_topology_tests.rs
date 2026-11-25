//! Codex credential and channel lifecycle on the current business schema.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use crate::domain::{ApiOperation, ConnectorKind};
use crate::persistence::sqlite::SqliteDatabase;
use crate::persistence::{
    CodexCredentialBatchInput, CodexCredentialBatchOperation, CodexCredentialBatchTarget,
    CodexCredentialCreate, ControlPlaneMutation, ControlPlaneRepository, RoutingGroupInput,
    sqlite_load,
};
use uuid::Uuid;

#[tokio::test]
async fn canonical_create_lifecycle_and_delete_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let database = Arc::new(
        SqliteDatabase::open(&directory.path().join("codex-lifecycle.sqlite"))
            .await
            .unwrap(),
    );
    database.install_schema().await.unwrap();
    let admin = Uuid::new_v4();

    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query(
        "INSERT INTO users(id,email,display_name,role,status,password_hash)
         VALUES (?,'codex-lifecycle@test.invalid','Lifecycle','admin','active','test')",
    )
    .bind(admin.to_string())
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();

    let repository = ControlPlaneRepository::from_sqlite(Arc::clone(&database));
    let group = repository
        .prepare_mutation(
            admin,
            ControlPlaneMutation::SaveRoutingGroup {
                id: Uuid::new_v4(),
                expected: None,
                input: RoutingGroupInput {
                    name: "Codex lifecycle".into(),
                    enabled: true,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0[0]
        .id;

    let input = CodexCredentialCreate {
        label: "Lifecycle credential".into(),
        enabled: true,
        proxy_id: None,
        quota_threshold_percent: 95,
        base_url: "https://codex.test/backend-api/codex".into(),
        email: Some("member@example.test".into()),
        account_id: Some("account-lifecycle".into()),
        user_id: Some("user-lifecycle".into()),
        plan_type: Some("business".into()),
        is_fedramp: false,
        id_token: "fixture-id".into(),
        access_token: "fixture-access".into(),
        refresh_token: "fixture-refresh".into(),
        access_token_expires_at: None,
        available_models: vec!["gpt-5-codex".into()],
        quota: None,
    };
    let credential = repository
        .prepare_codex_credential_create(admin, input.clone(), None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0[0]
        .id;

    let mut reader = database.acquire_read().await.unwrap();
    assert!(
        sqlite_load(&mut reader)
            .await
            .unwrap()
            .logical_channels
            .is_empty()
    );
    drop(reader);
    let access_id = repository
        .prepare_mutation(
            admin,
            ControlPlaneMutation::CreateUpstreamAccess(
                serde_json::from_value(serde_json::json!({
                    "name":"Explicit Codex access","connector_kind":"codex",
                    "base_url":input.base_url,"enabled":true
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap()
        .0[0]
        .id;
    repository
        .prepare_mutation(
            admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: credential,
                expected: None,
                input: serde_json::from_value(serde_json::json!({
                    "name":"Explicit logical channel","group_id":group,"access_id":access_id,
                    "credential_id":credential,"enabled":true
                }))
                .unwrap(),
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    for operation in [
        "responses",
        "responses-ws",
        "web_search",
        "images_generation",
        "images_edit",
    ] {
        repository.prepare_mutation(admin, ControlPlaneMutation::SaveChannelCapability {
            id:Uuid::new_v4(),expected:None,
            input:serde_json::from_value(serde_json::json!({
                "channel_id":credential,
                "settings":{"operation":operation,"enabled":!operation.starts_with("images_"),
                    "available_models":input.available_models,"request_compression":"default",
                    "test_model":null,"test_pricing_model_id":null,"auto_disable_allowed":false},
                "status_statistics_enabled":false,"config_template_id":null,
                "override_document":{},"billing_multiplier":"1"
            })).unwrap()
        }).await.unwrap().commit().await.unwrap();
    }
    let mut reader = database.acquire_read().await.unwrap();
    let topology = sqlite_load(&mut reader).await.unwrap();
    let logical = topology
        .logical_channels
        .iter()
        .find(|channel| channel.id == credential)
        .expect("canonical logical channel");
    assert_eq!(logical.credential_id, Some(credential));
    assert!(logical.enabled);
    assert_eq!(logical.group_id, group);
    let access = topology
        .upstream_accesses
        .iter()
        .find(|access| access.id == logical.access_id)
        .expect("canonical access");
    assert_eq!(access.connector_kind, ConnectorKind::CodexOauth);
    assert_eq!(access.base_url, input.base_url);
    assert!(access.enabled);
    let capabilities = topology
        .channel_capabilities
        .iter()
        .filter(|capability| capability.channel_id == credential)
        .collect::<Vec<_>>();
    assert_eq!(capabilities.len(), 5);
    let capability = |operation| {
        capabilities
            .iter()
            .find(|capability| capability.settings.operation == operation)
            .unwrap()
    };
    assert!(capability(ApiOperation::Responses).settings.enabled);
    assert!(
        capability(ApiOperation::StandaloneWebSearch)
            .settings
            .enabled
    );
    assert!(!capability(ApiOperation::ImagesGeneration).settings.enabled);
    assert!(!capability(ApiOperation::ImagesEdit).settings.enabled);
    assert_eq!(
        capability(ApiOperation::Responses)
            .settings
            .available_models,
        input.available_models
    );
    assert!(topology.operation_rules.is_empty());
    assert!(topology.operation_candidates.is_empty());
    assert!(topology.api_key_grants.is_empty());
    assert!(topology.policy_grants.is_empty());
    let registered: Vec<String> = sqlx::query_scalar(
        "SELECT capability_id FROM channel_identity_registry
         WHERE canonical_channel_id=? AND capability_id IS NOT NULL ORDER BY capability_id",
    )
    .bind(logical.id.to_string())
    .fetch_all(&mut *reader)
    .await
    .unwrap();
    let mut expected: Vec<_> = capabilities.iter().map(|cap| cap.id.to_string()).collect();
    expected.sort();
    assert_eq!(registered, expected);
    let kind: String = sqlx::query_scalar("SELECT kind FROM upstream_credentials WHERE id=?")
        .bind(credential.to_string())
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    assert_eq!(kind, "codex_oauth");
    let revision_before: String =
        sqlx::query_scalar("SELECT revision FROM upstream_credentials WHERE id=?")
            .bind(credential.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    drop(reader);

    let record = repository
        .codex_credential(credential)
        .await
        .unwrap()
        .unwrap();
    repository
        .prepare_codex_credentials_batch(
            admin,
            CodexCredentialBatchInput {
                items: vec![CodexCredentialBatchTarget {
                    id: credential,
                    updated_at: record.updated_at,
                }],
                operation: CodexCredentialBatchOperation::Disable,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let mut reader = database.acquire_read().await.unwrap();
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM upstream_credentials WHERE id=?")
        .bind(credential.to_string())
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    assert!(!enabled);
    let revision_disabled: String =
        sqlx::query_scalar("SELECT revision FROM upstream_credentials WHERE id=?")
            .bind(credential.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    assert_ne!(revision_disabled, revision_before);
    let responses_enabled: bool = sqlx::query_scalar(
        "SELECT enabled FROM channel_capabilities WHERE channel_id=? AND operation='responses'",
    )
    .bind(credential.to_string())
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    assert!(responses_enabled);
    drop(reader);

    let record = repository
        .codex_credential(credential)
        .await
        .unwrap()
        .unwrap();
    repository
        .prepare_codex_credentials_batch(
            admin,
            CodexCredentialBatchInput {
                items: vec![CodexCredentialBatchTarget {
                    id: credential,
                    updated_at: record.updated_at,
                }],
                operation: CodexCredentialBatchOperation::Enable,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let mut reader = database.acquire_read().await.unwrap();
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM upstream_credentials WHERE id=?")
        .bind(credential.to_string())
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    assert!(enabled);
    drop(reader);

    // An administrator capability switch must survive reimport.
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query(
        "UPDATE channel_capabilities SET enabled=0,updated_at=ag_now()
         WHERE channel_id=? AND operation='responses'",
    )
    .bind(credential.to_string())
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();

    let mut reimport = input.clone();
    reimport.label = "Reimported credential".into();
    reimport.base_url = "https://new-codex.test/backend-api/codex".into();
    repository
        .prepare_codex_credential_create(admin, reimport.clone(), None)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let mut reader = database.acquire_read().await.unwrap();
    let topology = sqlite_load(&mut reader).await.unwrap();
    let responses = topology
        .channel_capabilities
        .iter()
        .find(|capability| {
            capability.channel_id == credential
                && capability.settings.operation == ApiOperation::Responses
        })
        .unwrap();
    assert!(!responses.settings.enabled);
    assert_eq!(responses.settings.available_models, input.available_models);
    let access = topology
        .upstream_accesses
        .iter()
        .find(|access| access.id == logical.access_id)
        .unwrap();
    assert_eq!(access.base_url, input.base_url);
    let revision_reimported: String =
        sqlx::query_scalar("SELECT revision FROM upstream_credentials WHERE id=?")
            .bind(credential.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    assert_ne!(revision_reimported, revision_disabled);
    drop(reader);

    // A pending SQLite Codex operation fails the delete closed.
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query(
        "INSERT INTO _gateway_codex_operations(credential_id,attempt_id,kind,generation)
         VALUES (?,?,'refresh',0)",
    )
    .bind(credential.to_string())
    .bind(Uuid::new_v4().to_string())
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let record = repository
        .codex_credential(credential)
        .await
        .unwrap()
        .unwrap();
    let error = match repository
        .prepare_codex_credential_delete(admin, credential, record.updated_at)
        .await
    {
        Ok(_) => panic!("pending operation must fence deletion"),
        Err(error) => error,
    };
    assert!(
        format!("{error:?}").contains("codex_operation_pending"),
        "unexpected pending error: {error:?}"
    );
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query("DELETE FROM _gateway_codex_operations WHERE credential_id=?")
        .bind(credential.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    let topology = repository.topology().await.unwrap();
    for capability in &topology.channel_capabilities {
        repository
            .prepare_mutation(
                admin,
                ControlPlaneMutation::DeleteChannelCapability {
                    id: capability.id,
                    expected: capability.updated_at,
                },
            )
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();
    }
    let channel = repository
        .topology()
        .await
        .unwrap()
        .logical_channels
        .into_iter()
        .find(|channel| channel.id == credential)
        .unwrap();
    repository
        .prepare_mutation(
            admin,
            ControlPlaneMutation::DeleteLogicalChannel {
                id: credential,
                expected: channel.updated_at,
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let record = repository
        .codex_credential(credential)
        .await
        .unwrap()
        .unwrap();
    repository
        .prepare_codex_credential_delete(admin, credential, record.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let mut reader = database.acquire_read().await.unwrap();
    let credential_deleted: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM upstream_credentials WHERE id=?")
            .bind(credential.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    assert!(credential_deleted.is_some());
    let channel_deleted: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM upstream_channels WHERE id=?")
            .bind(credential.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    assert!(channel_deleted.is_some());
    let active_capabilities: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channel_capabilities
         WHERE channel_id=? AND deleted_at IS NULL",
    )
    .bind(credential.to_string())
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    assert_eq!(active_capabilities, 0);
    let access_deleted: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM upstream_accesses WHERE id=?")
            .bind(logical.access_id.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    assert!(access_deleted.is_none());
    drop(reader);
    drop(repository);
    database.close().await;
}
