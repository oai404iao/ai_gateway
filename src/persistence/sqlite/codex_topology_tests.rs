//! Canonical Codex lifecycle after transactional schema and identity conversion.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use crate::domain::{ApiOperation, ConnectorKind};
use crate::persistence::sqlite::SqliteDatabase;
use crate::persistence::{
    CodexCredentialBatchInput, CodexCredentialBatchOperation, CodexCredentialBatchTarget,
    CodexCredentialCreate, ControlPlaneMutation, ControlPlaneRepository, RepositoryError,
    RoutingGroupInput, sqlite_load,
};
use uuid::Uuid;

#[tokio::test]
async fn canonical_activation_preserves_uncertain_codex_operations() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let database = SqliteDatabase::open(&directory.path().join("pending.sqlite"))
        .await
        .unwrap();
    database
        .migrate(&super::schema::MIGRATIONS[..4])
        .await
        .unwrap();
    let group = Uuid::new_v4();
    let pool = Uuid::new_v4();
    let credentials = [Uuid::new_v4(), Uuid::new_v4()];
    let attempts = [Uuid::new_v4(), Uuid::new_v4()];
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query("INSERT INTO connector_pools(id,connector_kind) VALUES (?,'codex_oauth')")
        .bind(pool.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO channel_groups(id,name,api_format,connector_kind,connector_pool_id)
                 VALUES (?,'Pending group','open_ai_responses','codex_oauth',?)",
    )
    .bind(group.to_string())
    .bind(pool.to_string())
    .execute(&mut *transaction)
    .await
    .unwrap();
    for (index, kind) in ["refresh", "quota_reset"].into_iter().enumerate() {
        let id = credentials[index];
        sqlx::query(
            "INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind)
                     VALUES (?,?,'open_ai_responses',?,'https://codex.test','none')",
        )
        .bind(id.to_string())
        .bind(group.to_string())
        .bind(format!("Pending {index}"))
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlx::query("INSERT INTO codex_oauth_credentials
                     (channel_id,channel_group_id,connector_pool_id,label,account_id,id_token,access_token,refresh_token,last_refreshed_at)
                     VALUES (?,?,?,?,?,'test-id','test-access','test-refresh',ag_now())")
            .bind(id.to_string()).bind(group.to_string()).bind(pool.to_string())
            .bind(format!("Pending {index}")).bind(format!("account-{index}"))
            .execute(&mut *transaction).await.unwrap();
        sqlx::query("INSERT INTO _gateway_codex_operations(credential_id,attempt_id,kind,generation)
                     SELECT channel_id,?,?,refresh_generation FROM codex_oauth_credentials WHERE channel_id=?")
            .bind(attempts[index].to_string()).bind(kind).bind(id.to_string())
            .execute(&mut *transaction).await.unwrap();
    }
    let before: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT credential_id,attempt_id,kind,generation FROM _gateway_codex_operations ORDER BY credential_id"
    ).fetch_all(&mut *transaction).await.unwrap();
    transaction.commit().await.unwrap();
    database.install_schema().await.unwrap();
    let mut transaction = database.begin_write().await.unwrap();
    let after: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT credential_id,attempt_id,kind,generation FROM _gateway_codex_operations ORDER BY credential_id"
    ).fetch_all(&mut *transaction).await.unwrap();
    assert_eq!(before, after);
    for id in credentials {
        assert!(sqlx::query("UPDATE codex_oauth_credentials SET label='blocked',updated_at=ag_now() WHERE channel_id=?")
            .bind(id.to_string()).execute(&mut *transaction).await.is_err());
        assert!(
            sqlx::query(
                "UPDATE upstream_channels SET name='blocked',updated_at=ag_now() WHERE id=?"
            )
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await
            .is_err()
        );
    }
    transaction.commit().await.unwrap();
    database.close().await;
}

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
                    sharing_only: false,
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
        channel_group_id: group,
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
    assert_eq!(capabilities.len(), 4);
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
    let legacy_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN
         ('channels','channel_groups','model_rules','model_rule_routing_tiers',
          'model_rule_routing_candidates','codex_oauth_credential_channels')",
    )
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    assert_eq!(legacy_rows, 0);
    let registry: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channel_identity_registry WHERE codex_credential_id=?",
    )
    .bind(credential.to_string())
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    assert_eq!(registry, 5);
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
            group,
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
            group,
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
    assert_eq!(access.base_url, reimport.base_url);
    let revision_reimported: String =
        sqlx::query_scalar("SELECT revision FROM upstream_credentials WHERE id=?")
            .bind(credential.to_string())
            .fetch_one(&mut *reader)
            .await
            .unwrap();
    assert_ne!(revision_reimported, revision_disabled);
    drop(reader);

    // Sharing blocks deletion.
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query(
        "INSERT INTO codex_sharing_groups
         (id,credential_id,provider_account_id,provider_user_id,name,enabled,seats,
          primary_limit_amount,secondary_limit_amount,request_reservation_amount,
          user_requests_per_minute,group_requests_per_minute,
          user_max_concurrent_requests,group_max_concurrent_requests)
         VALUES (?,?,'sharing-account','sharing-user','Sharing',1,'[{}]',
                 '1','1','1',60,60,10,10)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(credential.to_string())
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let record = repository
        .codex_credential(credential)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        repository
            .prepare_codex_credential_delete(admin, credential, record.updated_at)
            .await,
        Err(RepositoryError::SharingCredentialInUse)
    ));
    let mut transaction = database.begin_write().await.unwrap();
    sqlx::query("DELETE FROM codex_sharing_groups WHERE credential_id=?")
        .bind(credential.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

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
    assert!(access_deleted.is_some());
    drop(reader);
    drop(repository);
    database.close().await;
}
