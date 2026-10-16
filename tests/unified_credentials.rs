//! Independent credential ownership and reusable channel contracts.

#[cfg(all(feature = "sqlite-backend", target_os = "linux"))]
mod sqlite {
    use std::{os::unix::fs::PermissionsExt, sync::Arc};

    use ai_gateway::persistence::{
        CodexCredentialCreate, ControlPlaneMutation, DEFAULT_ADMIN_GROUP_ID, MutationResult,
        sqlite::{SqliteControlPlaneRepository, SqliteDatabase, SqliteUuid},
    };
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    const ADMIN: Uuid = Uuid::from_u128(0x6801);

    async fn repository() -> (
        tempfile::TempDir,
        Arc<SqliteDatabase>,
        SqliteControlPlaneRepository,
    ) {
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
        sqlx::query(
            "INSERT INTO users(id,email,display_name,role,status,password_hash,password_changed_at,user_group_id)
             VALUES (?,'admin@example.test','Fixture admin','admin','active','fixture-hash',ag_now(),?)",
        ).bind(SqliteUuid(ADMIN)).bind(SqliteUuid(DEFAULT_ADMIN_GROUP_ID)).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let repository = SqliteControlPlaneRepository::new(Arc::clone(&database));
        repository.ensure_system_settings(serde_json::from_value(json!({
            "api_hosts": ["https://gateway.example.test"],
            "upstream": {"connect_timeout_seconds":10,"response_header_timeout_seconds":30,"stream_idle_timeout_seconds":60},
            "passive_health": {"connection_failure_threshold":3,"cooldown_seconds":60},
            "session_affinity": {"enabled":false,"max_entries":100000,"default_ttl_seconds":3600,"rules":[]}
        })).unwrap()).await.unwrap();
        (directory, database, repository)
    }

    async fn mutate(
        repository: &SqliteControlPlaneRepository,
        input: ControlPlaneMutation,
    ) -> MutationResult {
        let mut change = repository.prepare_mutation(ADMIN, input).await.unwrap();
        change.runtime_records().await.unwrap();
        change.commit().await.unwrap().0.remove(0)
    }

    fn credential() -> CodexCredentialCreate {
        CodexCredentialCreate {
            label: "Independent account".into(),
            enabled: true,
            proxy_id: None,
            quota_threshold_percent: 95,
            base_url: "https://chatgpt.com/backend-api".into(),
            email: Some("fixture@example.test".into()),
            account_id: Some("fixture-account".into()),
            user_id: Some("fixture-user".into()),
            plan_type: Some("plus".into()),
            is_fedramp: false,
            id_token: "synthetic-id-token".into(),
            access_token: "synthetic-access-token".into(),
            refresh_token: "synthetic-refresh-token".into(),
            access_token_expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            available_models: vec!["wire-model".into()],
            quota: None,
        }
    }

    #[tokio::test]
    async fn empty_database_installs_independent_credential_schema() {
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
        let repository = SqliteControlPlaneRepository::new(Arc::clone(&database));
        assert!(repository.codex_credentials().await.unwrap().is_empty());
        assert!(
            repository
                .load_codex_credentials()
                .await
                .unwrap()
                .is_empty()
        );
        let mut connection = database.acquire_read().await.unwrap();
        let pools: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name='connector_pools'")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
        assert_eq!(pools, 0);
        let obsolete: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pragma_table_info('codex_oauth_credentials')
             WHERE name IN ('channel_group_id','connector_pool_id')",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(obsolete, 0);
    }

    #[tokio::test]
    async fn credentials_are_independent_and_reusable_across_channels() {
        let (_directory, _database, repository) = repository().await;
        let mut change = repository
            .prepare_codex_credential_create(ADMIN, credential(), None)
            .await
            .unwrap();
        change.runtime_records().await.unwrap();
        let identity = change.commit().await.unwrap().0.remove(0);
        let topology = repository.topology().await.unwrap();
        assert!(topology.logical_channels.is_empty());
        assert!(topology.upstream_accesses.is_empty());
        assert!(topology.channel_capabilities.is_empty());
        assert_eq!(repository.codex_credentials().await.unwrap().len(), 1);
        assert!(
            repository
                .codex_credential_view(identity.id)
                .await
                .unwrap()
                .unwrap()
                .channel_ids
                .is_empty()
        );
        let group = Uuid::new_v4();
        mutate(
            &repository,
            ControlPlaneMutation::SaveRoutingGroup {
                id: group,
                expected: None,
                input: serde_json::from_value(json!({"name":"Mixed group","enabled":true}))
                    .unwrap(),
            },
        )
        .await;
        let access = mutate(
            &repository,
            ControlPlaneMutation::CreateUpstreamAccess(
                serde_json::from_value(json!({
                    "name":"Codex access","connector_kind":"codex",
                    "base_url":"https://chatgpt.com/backend-api","enabled":true
                }))
                .unwrap(),
            ),
        )
        .await;
        let mut channels = Vec::new();
        for index in 0..2 {
            let channel = Uuid::new_v4();
            mutate(
                &repository,
                ControlPlaneMutation::SaveLogicalChannel {
                    id: channel,
                    expected: None,
                    input: serde_json::from_value(json!({
                        "group_id":group,"access_id":access.id,"credential_id":identity.id,
                        "name":format!("Channel {index}"),"enabled":true
                    }))
                    .unwrap(),
                },
            )
            .await;
            let capability = Uuid::new_v4();
            mutate(&repository, ControlPlaneMutation::SaveChannelCapability {
                id: capability, expected: None,
                input: serde_json::from_value(json!({
                    "channel_id":channel,
                    "settings":{"operation":"responses","enabled":true,"available_models":["wire-model"],
                        "request_compression":"default","test_model":null,"test_pricing_model_id":null,
                        "auto_disable_allowed":false},
                    "status_statistics_enabled":false,"config_template_id":null,
                    "override_document":{},"billing_multiplier":"1"
                })).unwrap(),
            }).await;
            channels.push((channel, capability));
        }
        let topology = repository.topology().await.unwrap();
        assert_eq!(topology.logical_channels.len(), 2);
        assert!(
            topology
                .logical_channels
                .iter()
                .all(|channel| channel.id != identity.id)
        );
        let view = repository
            .codex_credential_view(identity.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(view.channel_ids.len(), 2);
        assert!(
            repository
                .prepare_codex_credential_delete(ADMIN, identity.id, view.updated_at)
                .await
                .is_err()
        );
        let mut change = repository
            .prepare_codex_credential_create(ADMIN, credential(), None)
            .await
            .unwrap();
        change.runtime_records().await.unwrap();
        assert_eq!(change.commit().await.unwrap().0[0].id, identity.id);
        assert_eq!(
            repository.topology().await.unwrap().logical_channels.len(),
            2
        );
    }
}
