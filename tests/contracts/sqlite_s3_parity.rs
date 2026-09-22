//! Shared PostgreSQL/SQLite contracts for the S3 identity and control-plane facades.
//!
//! Every case runs the *identical* repository calls against a freshly provisioned
//! PostgreSQL test database and a fresh file-backed SQLite database, so assertions
//! are written once and must hold for both backends. Infrastructure setup failures
//! are reported as such; a contract mismatch names the backend that diverged.

use std::{os::unix::fs::PermissionsExt, str::FromStr, sync::Arc};

use ai_gateway::{
    application::ControlPlaneError,
    domain::{
        AdvancedBilling, AutomaticDisableTrigger, ConsoleSessionPurpose, LongContextTier, UserRole,
    },
    persistence::{
        ApiKeyCreate, ApiKeyPolicyInput, ApiKeyUpdate, ChannelBatchChanges,
        ChannelBatchUpdateInput, ChannelBatchUpdateTarget, ChannelCapabilityRecord,
        ConfigTemplateCreateInput, ConfigTemplateInput, ConsoleAuditLog, ControlPlaneApiKey,
        ControlPlaneApiKeyPolicy, ControlPlaneConfigTemplate, ControlPlaneModel, ControlPlaneProxy,
        ControlPlaneUser, ControlPlaneUserGroup, DEFAULT_ADMIN_GROUP_ID, DEFAULT_USER_GROUP_ID,
        InviteUserInput, LogicalChannelRecord, ModelInput, ModelRuleCreateInput, MutationResult,
        ProxyInput, RegistrationAttempt, RegistrationInvitationCodeInput, RepositoryError,
        RoutingGroupRecord, SelfApiKeyCreate, SelfApiKeyUpdate, SessionRotation,
        SystemAutomaticDisableSettingsInput, UserBalanceBatchChange, UserBatchChanges,
        UserBatchUpdateInput, UserBatchUpdateTarget, UserGroupInput, UserInput, UserSettingsInput,
        UserUpdateInput, sqlite::SqliteDatabase,
    },
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::FutureExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;

const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA";
const RESET_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$cmVzZXQ";
const CHOSEN_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$Y2hvc2Vu";
const REFRESH_HASH: &[u8] = b"parity-refresh-hash";
const NEXT_REFRESH_HASH: &[u8] = b"parity-next-refresh";
const INVITATION_TOKEN_HASH: &[u8] = b"parity-invitation-token";
const REGISTRATION_CODE_HASH: &[u8] = b"parity-registration-code";

/// One provisioned storage backend under test.
enum Backend {
    Postgres(super::TestDatabase),
    Sqlite {
        directory: tempfile::TempDir,
        database: Arc<SqliteDatabase>,
    },
}

impl Backend {
    /// A throwaway PostgreSQL database with the production migrations applied.
    async fn postgres() -> Self {
        Self::Postgres(super::TestDatabase::new().await)
    }

    /// A private file-backed SQLite database with the full business schema.
    async fn sqlite() -> Self {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("infrastructure: private SQLite test directory must be creatable");
        let database = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .expect("infrastructure: SQLite test database must open");
        assert_eq!(
            database
                .install_schema()
                .await
                .expect("infrastructure: SQLite schema must install"),
            6
        );
        Self::Sqlite {
            directory,
            database: Arc::new(database),
        }
    }

    fn repositories(&self) -> Repositories {
        match self {
            Self::Postgres(database) => Repositories {
                auth: AuthRepository::new(database.pool.clone()),
                control_plane: ControlPlaneRepository::new(database.pool.clone()),
            },
            Self::Sqlite { database, .. } => Repositories {
                auth: AuthRepository::from_sqlite(Arc::clone(database)),
                control_plane: ControlPlaneRepository::from_sqlite(Arc::clone(database)),
            },
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Postgres(_) => "postgres",
            Self::Sqlite { .. } => "sqlite",
        }
    }

    async fn finish(self) {
        match self {
            Self::Postgres(database) => database.cleanup().await,
            Self::Sqlite {
                directory,
                database,
            } => {
                database.close().await;
                drop(directory);
            }
        }
    }
}

/// The public facades for one backend; case functions see nothing else.
#[derive(Clone)]
struct Repositories {
    auth: AuthRepository,
    control_plane: ControlPlaneRepository,
}

/// Runs one backend-neutral case against a fresh SQLite and PostgreSQL database.
///
/// Backend construction happens before any contract work, so a missing PostgreSQL
/// server is reported as infrastructure rather than as a contract divergence. Both
/// cases always run and every divergence is reported with its backend label.
async fn run_contract<Fut>(case: fn(Repositories) -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let sqlite = Backend::sqlite().await;
    let postgres = Backend::postgres().await;
    let mut divergences = Vec::new();
    for backend in [sqlite, postgres] {
        let label = backend.label();
        let repositories = backend.repositories();
        repositories
            .control_plane
            .ensure_system_settings(system_settings())
            .await
            .unwrap_or_else(|error| {
                panic!("infrastructure: {label} system settings must initialize: {error}")
            });
        let result = std::panic::AssertUnwindSafe(case(repositories))
            .catch_unwind()
            .await;
        backend.finish().await;
        if let Err(panic) = result {
            divergences.push(format!("{label}: {}", panic_message(panic)));
        }
    }
    assert!(
        divergences.is_empty(),
        "both backends share one contract; divergences: {divergences:#?}"
    );
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "non-string panic".to_owned())
}

fn decimal(value: &str) -> Decimal {
    Decimal::from_str(value).expect("test decimal literal must parse")
}

fn normalized(value: Decimal) -> String {
    value.normalize().to_string()
}

/// Audits are JSON and the backends represent `numeric` differently: PostgreSQL
/// may emit a JSON number where SQLite emits a canonical string.
fn audit_decimal(value: &serde_json::Value) -> Decimal {
    match value {
        serde_json::Value::String(text) => decimal(text),
        other => decimal(&other.to_string()),
    }
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("test timestamp literal must parse")
        .with_timezone(&Utc)
}

fn proxy_input(name: &str) -> ProxyCreateInput {
    ProxyCreateInput {
        name: name.into(),
        proxy_url: "http://proxy.example.test:8080".into(),
        username: None,
        password: None,
        no_proxy_hosts: Vec::new(),
        enabled: true,
    }
}

async fn bootstrap_admin(repositories: &Repositories) -> Uuid {
    repositories
        .auth
        .bootstrap_admin("Root@Example.Test", "Root Admin", PASSWORD_HASH)
        .await
        .expect("bootstrap must create the first administrator")
}

#[tokio::test]
async fn catalog_and_probe_contract_matches_across_backends() {
    run_contract(catalog_and_probe_contract).await;
}

async fn catalog_and_probe_contract(repositories: Repositories) {
    let admin = bootstrap_admin(&repositories).await;
    let repository = &repositories.control_plane;
    let identity = repository.ensure_system_probe_identity().await.unwrap();
    assert_eq!(
        repository.ensure_system_probe_identity().await.unwrap(),
        identity
    );
    assert_eq!(
        repository.control_plane_lists().await.unwrap().users.len(),
        1
    );
    let input = |price: &str| ai_gateway::persistence::SyncedModelInput {
        source_model_id: "catalog-parity".into(),
        display_name: "Catalog Model".into(),
        provider_name: "Provider".into(),
        input_unit_price: decimal(price),
        cached_input_unit_price: Decimal::ZERO,
        cache_write_unit_price: Decimal::ZERO,
        output_unit_price: Decimal::ZERO,
        advanced_billing: AdvancedBilling::default(),
        source_payload: json!({"id":"catalog-parity"}),
    };
    let mut created = repository
        .prepare_catalog_models(admin, vec![input("1.000000000001")])
        .await
        .unwrap();
    compile_runtime_config(created.runtime_records().await.unwrap()).unwrap();
    let (created, _) = created.commit().await.unwrap();
    assert_eq!(created[0].action, "import");
    let id = created[0].id;
    let pending = repository
        .prepare_catalog_models(admin, vec![input("2.75")])
        .await
        .unwrap();
    drop(pending);
    assert_eq!(
        repository.load().await.unwrap().models[0].input_unit_price,
        decimal("1.000000000001")
    );
    let (updated, _) = repository
        .prepare_catalog_models(admin, vec![input("2.75")])
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(updated[0].action, "price_sync");
    assert_eq!(updated[0].id, id);
    assert_eq!(
        repository.load().await.unwrap().models[0].input_unit_price,
        decimal("2.75")
    );
    assert_eq!(
        repository.model_source_ids().await.unwrap(),
        ["catalog-parity"]
    );
}

async fn commit_mutation(
    repositories: &Repositories,
    actor: Uuid,
    mutation: ControlPlaneMutation,
) -> MutationResult {
    let (mut mutations, _) = repositories
        .control_plane
        .prepare_mutation(actor, mutation)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    mutations
        .pop()
        .expect("a single prepared mutation yields one result")
}

async fn commit_self_key(
    repositories: &Repositories,
    actor: Uuid,
    input: SelfApiKeyCreate,
) -> MutationResult {
    let (mut mutations, _) = repositories
        .control_plane
        .prepare_own_api_key_create(actor, input)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    mutations
        .pop()
        .expect("a self-service create yields one result")
}

async fn audit_for(repositories: &Repositories, action: &str, object_id: Uuid) -> ConsoleAuditLog {
    repositories
        .control_plane
        .audit_logs(100)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.action == action && entry.object_id == object_id)
        .unwrap_or_else(|| panic!("audit {action} for {object_id} must exist"))
}

async fn listed_user(repositories: &Repositories, id: Uuid) -> ControlPlaneUser {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .users
        .into_iter()
        .find(|user| user.id == id)
        .unwrap_or_else(|| panic!("user {id} must be listed"))
}

async fn increase_balance(repositories: &Repositories, actor: Uuid, user_id: Uuid, amount: &str) {
    let updated_at = listed_user(repositories, user_id).await.updated_at;
    repositories
        .control_plane
        .prepare_users_batch(
            actor,
            UserBatchUpdateInput {
                items: vec![UserBatchUpdateTarget {
                    id: user_id,
                    updated_at,
                }],
                changes: UserBatchChanges {
                    status: None,
                    balance: Some(UserBalanceBatchChange {
                        operation: "increase".into(),
                        amount: decimal(amount),
                    }),
                    user_group_id: None,
                    default_api_key_policy_id: None,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
}

struct World {
    admin: Uuid,
    user: Uuid,
    group: Uuid,
    channel: Uuid,
    access: Uuid,
    capability: Uuid,
    policy: Uuid,
}

/// Seeds one active administrator, one active user with a group policy, and the
/// ordinary group/channel the policy targets, using only public mutations.
async fn world(repositories: &Repositories) -> World {
    let admin = bootstrap_admin(repositories).await;
    let group = commit_mutation(
        repositories,
        admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Parity Group".into(),
                sharing_only: false,
                enabled: true,
            },
        },
    )
    .await
    .id;
    let access = commit_mutation(
        repositories,
        admin,
        ControlPlaneMutation::CreateUpstreamAccess(ai_gateway::persistence::UpstreamAccessInput {
            name: "Parity access".into(),
            connector_kind: ai_gateway::domain::ConnectorKind::OpenAiCompatible,
            base_url: "https://upstream.example.test".into(),
            proxy_id: None,
            enabled: true,
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
        }),
    )
    .await
    .id;
    let channel = commit_mutation(
        repositories,
        admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::LogicalChannelInput {
                group_id: group,
                access_id: access,
                credential_id: None,
                name: "Parity Channel".into(),
                enabled: true,
            },
        },
    )
    .await
    .id;
    let capability = commit_mutation(
        repositories,
        admin,
        ControlPlaneMutation::SaveChannelCapability {
            id: Uuid::new_v4(),
            expected: None,
            input: serde_json::from_value(json!({
                "channel_id": channel,
                "settings": {
                    "operation": "chat_completion",
                    "enabled": true, "available_models": ["parity-model"],
                    "request_compression": "default", "test_model": null,
                    "test_pricing_model_id": null, "auto_disable_allowed": false
                },
                "status_statistics_enabled": false, "config_template_id": null,
                "override_document": {}, "billing_multiplier": "1"
            }))
            .unwrap(),
        },
    )
    .await
    .id;
    let policy = commit_mutation(
        repositories,
        admin,
        ControlPlaneMutation::CreateApiKeyPolicy(ApiKeyPolicyInput {
            name: "Parity Policy".into(),
            allowed_group_ids: vec![group],
            allowed_channel_ids: Vec::new(),
            enabled: true,
        }),
    )
    .await
    .id;
    let user = commit_mutation(
        repositories,
        admin,
        ControlPlaneMutation::CreateUser(UserInput {
            display_name: "Parity User".into(),
            email: Some("user@example.test".into()),
            role: "user".into(),
            status: "active".into(),
            balance_amount: Decimal::ZERO,
            user_group_id: None,
            default_api_key_policy_id: Some(policy),
        }),
    )
    .await
    .id;
    World {
        admin,
        user,
        group,
        channel,
        access,
        capability,
        policy,
    }
}

#[tokio::test]
async fn auth_identity_contract_matches_across_backends() {
    run_contract(auth_identity_contract).await;
}

async fn auth_identity_contract(repositories: Repositories) {
    let auth = &repositories.auth;

    assert!(matches!(
        auth.bootstrap_admin(" ", "Blank", PASSWORD_HASH).await,
        Err(RepositoryError::Validation)
    ));
    let admin = bootstrap_admin(&repositories).await;
    assert!(matches!(
        auth.bootstrap_admin("second@example.test", "Second Admin", PASSWORD_HASH)
            .await,
        Err(RepositoryError::Conflict)
    ));
    let bootstrap = audit_for(&repositories, "bootstrap", admin).await;
    assert_eq!(bootstrap.actor_type, "system");
    assert_eq!(
        bootstrap.after_redacted.as_ref().unwrap()["role"].as_str(),
        Some("admin")
    );

    // Operator reset matches case-insensitively and bumps the stored version.
    assert!(
        !auth
            .reset_active_admin_password("nobody@example.test", RESET_PASSWORD_HASH)
            .await
            .unwrap()
    );
    assert!(
        auth.reset_active_admin_password("ROOT@example.test", RESET_PASSWORD_HASH)
            .await
            .unwrap()
    );
    let reset = auth.password_user(admin).await.unwrap().unwrap();
    assert_eq!(reset.auth_version, 2);
    assert_eq!(reset.password_hash.as_deref(), Some(RESET_PASSWORD_HASH));

    // A live session validates, rotates once, and replay revokes it.
    let session = Uuid::from_u128(0x9001);
    let expires_at = timestamp("9999-01-02T03:04:05.123456Z");
    auth.create_session(
        session,
        admin,
        REFRESH_HASH,
        expires_at,
        Some("parity agent"),
        ConsoleSessionPurpose::Normal,
    )
    .await
    .unwrap();
    let identity = auth
        .validate_console_identity(admin, session, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.session_id, session);
    assert_eq!(identity.session_purpose, "normal");
    assert_eq!(identity.expires_at, expires_at);
    assert!(
        auth.validate_console_identity(admin, session, 1)
            .await
            .unwrap()
            .is_none()
    );
    let rotated = auth
        .rotate_session(
            session,
            REFRESH_HASH,
            NEXT_REFRESH_HASH,
            expires_at,
            Some("rotated"),
        )
        .await
        .unwrap();
    let SessionRotation::Rotated {
        user,
        refresh_expires_at,
    } = rotated
    else {
        panic!("a live session must rotate");
    };
    assert_eq!(user.id, admin);
    assert_eq!(refresh_expires_at, expires_at);
    assert!(matches!(
        auth.rotate_session(session, REFRESH_HASH, NEXT_REFRESH_HASH, expires_at, None)
            .await
            .unwrap(),
        SessionRotation::Replayed
    ));
    assert!(
        auth.validate_console_identity(admin, session, 2)
            .await
            .unwrap()
            .is_none(),
        "replay revokes the session"
    );

    // Registration codes are single-use and project the exact balance.
    let registration = RegistrationInvitationCodeInput {
        name: "Parity Seat".into(),
        max_uses: Some(1),
        expires_at: Some(timestamp("2030-01-01T00:00:00Z")),
        enabled: true,
        user_group_id: DEFAULT_USER_GROUP_ID,
        initial_balance_amount: decimal("10"),
    };
    auth.create_registration_invitation_code(admin, REGISTRATION_CODE_HASH, registration.clone())
        .await
        .unwrap();
    assert!(matches!(
        auth.create_registration_invitation_code(admin, REGISTRATION_CODE_HASH, registration)
            .await,
        Err(RepositoryError::RegistrationInvitationCodeConflict)
    ));
    assert!(matches!(
        auth.register_with_invitation_code(
            b"wrong-code",
            "new@example.test",
            "New User",
            PASSWORD_HASH
        )
        .await
        .unwrap(),
        RegistrationAttempt::InvalidCode
    ));
    let registered = auth
        .register_with_invitation_code(
            REGISTRATION_CODE_HASH,
            "new@example.test",
            "New User",
            PASSWORD_HASH,
        )
        .await
        .unwrap();
    let RegistrationAttempt::Registered(new_user) = registered else {
        panic!("a valid code must register the user");
    };
    assert_eq!(new_user.role, UserRole::User);
    assert_eq!(new_user.auth_version, 1);
    assert!(matches!(
        auth.register_with_invitation_code(
            REGISTRATION_CODE_HASH,
            "other@example.test",
            "Other",
            PASSWORD_HASH
        )
        .await
        .unwrap(),
        RegistrationAttempt::InvalidCode
    ));
    let profile = auth.profile(new_user.id).await.unwrap().unwrap();
    assert_eq!(profile.status, "active");
    assert_eq!(profile.balance_amount, decimal("10"));
    let register = audit_for(&repositories, "register", new_user.id).await;
    assert_eq!(
        audit_decimal(&register.after_redacted.as_ref().unwrap()["balance_amount"]),
        decimal("10")
    );

    // Invitations are admin-only and one-shot on acceptance.
    let invitation = Uuid::from_u128(0x9002);
    let invite = |actor_email: &str| InviteUserInput {
        email: actor_email.into(),
        display_name: "Invitee".into(),
        role: UserRole::User,
        initial_balance_amount: decimal("25.50"),
        user_group_id: None,
        default_api_key_policy_id: None,
    };
    assert!(matches!(
        auth.invite_user(
            Uuid::from_u128(0x9fff),
            invite("invitee@example.test"),
            invitation,
            INVITATION_TOKEN_HASH,
            Duration::from_secs(3_600),
        )
        .await,
        Err(RepositoryError::NotFound)
    ));
    let created = auth
        .invite_user(
            admin,
            invite("Invitee@Example.Test"),
            invitation,
            INVITATION_TOKEN_HASH,
            Duration::from_secs(3_600),
        )
        .await
        .unwrap();
    assert_eq!(created.invitation_id, invitation);
    assert!(created.expires_at > Utc::now());
    let invited = auth
        .find_login_user("invitee@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invited.id, created.user_id);
    assert_eq!(invited.status, "invited");
    assert_eq!(invited.auth_version, 1);
    let invite_audit = audit_for(&repositories, "invite", created.user_id).await;
    assert_eq!(invite_audit.actor_role.as_deref(), Some("admin"));
    assert_eq!(
        audit_decimal(&invite_audit.after_redacted.as_ref().unwrap()["balance_amount"]),
        decimal("25.50")
    );

    assert!(
        auth.accept_invitation(invitation, b"wrong-token", CHOSEN_PASSWORD_HASH)
            .await
            .unwrap()
            .is_none()
    );
    let accepted = auth
        .accept_invitation(invitation, INVITATION_TOKEN_HASH, CHOSEN_PASSWORD_HASH)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.id, created.user_id);
    assert_eq!(accepted.auth_version, 2);
    assert!(
        auth.accept_invitation(invitation, INVITATION_TOKEN_HASH, CHOSEN_PASSWORD_HASH)
            .await
            .unwrap()
            .is_none(),
        "an accepted invitation is one-shot"
    );
    let active = auth
        .find_login_user("invitee@example.test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.status, "active");
    assert_eq!(active.password_hash.as_deref(), Some(CHOSEN_PASSWORD_HASH));
}

#[tokio::test]
async fn balance_arithmetic_contract_matches_across_backends() {
    run_contract(balance_arithmetic_contract).await;
}

async fn balance_arithmetic_contract(repositories: Repositories) {
    let admin = bootstrap_admin(&repositories).await;
    let create = |email: &str, display_name: &str, balance: &str| UserInput {
        display_name: display_name.into(),
        email: Some(email.into()),
        role: "user".into(),
        status: "active".into(),
        balance_amount: decimal(balance),
        user_group_id: None,
        default_api_key_policy_id: None,
    };
    // 9007199254740993 has no exact f64 representation.
    let big = commit_mutation(
        &repositories,
        admin,
        ControlPlaneMutation::CreateUser(create(
            "big@example.test",
            "Big Balance",
            "9007199254740993",
        )),
    )
    .await
    .id;
    let small = commit_mutation(
        &repositories,
        admin,
        ControlPlaneMutation::CreateUser(create(
            "small@example.test",
            "Small Balance",
            "-0.00000001",
        )),
    )
    .await
    .id;

    assert_eq!(
        normalized(listed_user(&repositories, big).await.balance_amount),
        "9007199254740993"
    );
    assert_eq!(
        normalized(listed_user(&repositories, small).await.balance_amount),
        "-0.00000001"
    );

    // PostgreSQL rounds the stored numeric rather than the input, while SQLite
    // rounds the final checked sum; both must land on the identical exact result.
    increase_balance(&repositories, admin, big, "1").await;
    increase_balance(&repositories, admin, small, "0.000000005").await;

    assert_eq!(
        normalized(listed_user(&repositories, big).await.balance_amount),
        "9007199254740994",
        "an f64 round trip would collapse the sum to 9007199254740992"
    );
    assert_eq!(
        normalized(listed_user(&repositories, small).await.balance_amount),
        "-0.00000001",
        "a half-unit delta must round the final sum, not the input amount"
    );
}

#[tokio::test]
async fn control_plane_contract_matches_across_backends() {
    run_contract(control_plane_contract).await;
}

async fn control_plane_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    let lists = repository.control_plane_lists().await.unwrap();
    assert_eq!(lists.users.len(), 2);
    assert!(lists.users.iter().any(|user| user.id == world.admin));
    assert!(lists.users.iter().any(|user| user.id == world.user));
    let topology = repository.topology().await.unwrap();
    assert_eq!(topology.routing_groups.len(), 1);
    assert_eq!(topology.logical_channels.len(), 1);
    assert_eq!(topology.logical_channels[0].access_id, world.access);
    assert_eq!(lists.api_key_policies.len(), 1);
    assert!(lists.api_keys.is_empty());

    let channel = listed_channel(&repositories, world.channel).await;
    assert_eq!(channel.name, "Parity Channel");
    assert_eq!(
        listed_capability(&repositories, world.capability)
            .await
            .settings
            .operation,
        ai_gateway::domain::ApiOperation::ChatCompletions
    );
    assert!(
        !topology
            .logical_channels
            .iter()
            .any(|channel| channel.id == Uuid::nil())
    );
    assert!(
        repository
            .system_settings()
            .await
            .unwrap()
            .settings
            .api_hosts
            .is_empty()
    );
    assert!(repository.load().await.unwrap().proxies.is_empty());

    repository.verify_active_admin(world.admin).await.unwrap();
    assert!(matches!(
        repository.verify_active_admin(world.user).await,
        Err(RepositoryError::InvalidActor)
    ));

    // Proxy version guard: a stale ETag conflicts, the fresh one commits.
    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(proxy_input("parity")),
    )
    .await;
    let listed = repository
        .control_plane_lists()
        .await
        .unwrap()
        .proxies
        .into_iter()
        .find(|proxy| proxy.id == created.id)
        .unwrap();
    assert_eq!(listed.name, "parity");
    let rename = |expected_updated_at| ControlPlaneMutation::UpdateProxy {
        id: created.id,
        input: ProxyInput {
            name: "renamed".into(),
            proxy_url: "http://proxy.example.test:8080".into(),
            username: None,
            password: None,
            no_proxy_hosts: Vec::new(),
            enabled: true,
        },
        expected_updated_at,
    };
    let stale = repository
        .prepare_mutation(
            world.admin,
            rename(listed.updated_at - ChronoDuration::seconds(1)),
        )
        .await;
    assert!(matches!(stale.err(), Some(RepositoryError::Conflict)));
    let updated = commit_mutation(&repositories, world.admin, rename(listed.updated_at)).await;
    assert_eq!(updated.action, "update");
    assert_eq!(
        repository
            .control_plane_lists()
            .await
            .unwrap()
            .proxies
            .into_iter()
            .find(|proxy| proxy.id == created.id)
            .unwrap()
            .name,
        "renamed"
    );

    // System settings carry their own version guard and read back exactly.
    let settings = repository.system_settings().await.unwrap();
    let mut next = settings.settings.clone();
    next.api_hosts = vec!["https://parity.example.test".into()];
    let stale = repository
        .prepare_mutation(
            world.admin,
            ControlPlaneMutation::UpdateSystemSettings {
                input: next.clone(),
                expected_updated_at: settings.updated_at - ChronoDuration::seconds(1),
            },
        )
        .await;
    assert!(matches!(stale.err(), Some(RepositoryError::Conflict)));
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateSystemSettings {
            input: next,
            expected_updated_at: settings.updated_at,
        },
    )
    .await;
    assert_eq!(
        repository
            .system_settings()
            .await
            .unwrap()
            .settings
            .api_hosts,
        ["https://parity.example.test"]
    );

    // Per-user settings are a prepared change and are not audited.
    assert!(
        !repository
            .user_settings(world.user)
            .await
            .unwrap()
            .unwrap()
            .websocket_enabled
    );
    let (view, change) = repository
        .prepare_user_settings(
            world.user,
            UserSettingsInput {
                websocket_enabled: true,
            },
        )
        .await
        .unwrap();
    assert!(view.websocket_enabled);
    change.commit().await.unwrap();
    assert!(
        repository
            .user_settings(world.user)
            .await
            .unwrap()
            .unwrap()
            .websocket_enabled
    );
    assert!(matches!(
        repository
            .prepare_user_settings(
                Uuid::nil(),
                UserSettingsInput {
                    websocket_enabled: false
                }
            )
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));

    let audits = repository.audit_logs(100).await.unwrap();
    for (action, object_type) in [
        ("create", "routing_group"),
        ("create", "upstream_access"),
        ("create", "logical_channel"),
        ("create", "channel_capability"),
        ("create", "api_key_policy"),
        ("create", "user"),
        ("update", "system_settings"),
        ("update", "proxy"),
    ] {
        assert!(
            audits
                .iter()
                .any(|audit| audit.action == action && audit.object_type == object_type),
            "{action}/{object_type} must be audited"
        );
    }
    assert!(
        audits
            .iter()
            .all(|audit| matches!(audit.actor_type.as_str(), "user" | "system"))
    );
}

#[tokio::test]
async fn self_service_contract_matches_across_backends() {
    run_contract(self_service_contract).await;
}

async fn self_service_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    let options = repository.own_api_key_options(world.user).await.unwrap();
    assert_eq!(options.policy_id, Some(world.policy));
    assert!(options.policy_enabled);
    assert_eq!(options.groups.len(), 1);
    assert_eq!(options.groups[0].id, world.group);
    assert_eq!(options.channels.len(), 1);
    assert_eq!(options.channels[0].id, world.channel);

    let self_key = |name: &str, group: Uuid| SelfApiKeyCreate {
        name: name.into(),
        allowed_group_ids: vec![group],
        allowed_channel_ids: Vec::new(),
        expires_at: None,
        requests_per_minute: None,
        max_concurrent_requests: None,
        quota_limit_amount: None,
    };
    let created =
        commit_self_key(&repositories, world.user, self_key("Self key", world.group)).await;
    assert_eq!(created.action, "self_create");
    let secret = created
        .created_secret
        .clone()
        .expect("self-service create returns the secret exactly once");
    assert!(secret.starts_with("sk-"));
    let key = repository
        .own_api_key(world.user, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(key.allowed_api_formats, ["open_ai_chat_completions"]);
    assert_eq!(key.permissions, ["proxy", "models.read"]);
    assert!(
        repository
            .own_api_key(world.admin, created.id)
            .await
            .unwrap()
            .is_none(),
        "another user's key is not visible"
    );
    assert!(
        repository
            .own_api_keys(world.admin)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(repository.own_api_keys(world.user).await.unwrap().len(), 1);

    // A target outside the user's policy is rejected without writing.
    let outside = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Outside Group".into(),
                sharing_only: false,
                enabled: true,
            },
        },
    )
    .await
    .id;
    let outside_channel = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::LogicalChannelInput {
                group_id: outside,
                access_id: world.access,
                credential_id: None,
                name: "Outside channel".into(),
                enabled: true,
            },
        },
    )
    .await
    .id;
    let capability = listed_capability(&repositories, world.capability).await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveChannelCapability {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::ChannelCapabilityInput {
                channel_id: outside_channel,
                settings: capability.settings,
                status_statistics_enabled: false,
                config_template_id: None,
                override_document: json!({}),
                billing_multiplier: Decimal::ONE,
            },
        },
    )
    .await;
    let rejected = repository
        .prepare_own_api_key_create(world.user, self_key("Outside key", outside))
        .await;
    assert!(matches!(
        rejected.err(),
        Some(RepositoryError::ApiKeyTargetNotAllowed)
    ));

    // Rolling back and stale versions leave the key untouched.
    let self_update = || SelfApiKeyUpdate {
        name: "Renamed".into(),
        status: "disabled".into(),
        allowed_group_ids: vec![world.group],
        allowed_channel_ids: Vec::new(),
        expires_at: None,
        requests_per_minute: None,
        max_concurrent_requests: None,
        quota_limit_amount: None,
    };
    let before = repository
        .own_api_key(world.user, created.id)
        .await
        .unwrap()
        .unwrap();
    repository
        .prepare_own_api_key_update(world.user, created.id, self_update(), before.updated_at)
        .await
        .unwrap()
        .rollback()
        .await
        .unwrap();
    assert_eq!(
        repository
            .own_api_key(world.user, created.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Self key"
    );
    let stale = repository
        .prepare_own_api_key_update(
            world.user,
            created.id,
            self_update(),
            before.updated_at - ChronoDuration::seconds(1),
        )
        .await;
    assert!(matches!(stale.err(), Some(RepositoryError::Conflict)));
    let (mutations, _) = repository
        .prepare_own_api_key_update(world.user, created.id, self_update(), before.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(mutations[0].action, "self_update");
    assert_eq!(
        repository
            .own_api_key(world.user, created.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "disabled"
    );

    // Revoking writes one audit row; only the owner may delete.
    let (mutations, _) = repository
        .prepare_own_api_key_revoke(world.user, created.id, "requested by the user".into())
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(mutations[0].action, "self_revoke");
    let revoked = repository
        .own_api_key(world.user, created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revoked.status, "revoked");
    let revoke_audit = audit_for(&repositories, "self_revoke", created.id).await;
    assert_eq!(revoke_audit.actor_role.as_deref(), Some("user"));
    assert_eq!(
        revoke_audit.reason.as_deref(),
        Some("requested by the user")
    );

    assert!(matches!(
        repository
            .prepare_own_api_key_delete(world.admin, created.id, revoked.updated_at)
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));
    let (mutations, _) = repository
        .prepare_own_api_key_delete(world.user, created.id, revoked.updated_at)
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(mutations[0].action, "self_delete");
    assert!(
        repository
            .own_api_key(world.user, created.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .own_api_keys(world.user)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn prepared_change_contract_matches_across_backends() {
    run_contract(prepared_change_contract).await;
}

async fn prepared_change_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );

    // A committed mutation publishes a new snapshot.
    let before = runtime.snapshot();
    let created = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::CreateProxy(proxy_input("published")),
        )
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&before, &runtime.snapshot()));
    assert!(runtime.snapshot().proxy(created.id).is_some());
    assert!(
        repository
            .load()
            .await
            .unwrap()
            .proxies
            .iter()
            .any(|proxy| proxy.id == created.id)
    );

    // The repository accepts this row, but the candidate snapshot cannot compile:
    // the runtime compiler rejects a proxy URL with a query. The change must roll
    // back and the previous snapshot must stay published.
    let snapshot = runtime.snapshot();
    let failure = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::CreateProxy(ProxyCreateInput {
                name: "uncompilable".into(),
                proxy_url: "http://proxy.example.test:8080?query=1".into(),
                username: None,
                password: None,
                no_proxy_hosts: Vec::new(),
                enabled: true,
            }),
        )
        .await
        .err();
    assert!(matches!(failure, Some(ControlPlaneError::Compile(_))));
    assert!(Arc::ptr_eq(&snapshot, &runtime.snapshot()));
    assert!(
        !repository
            .load()
            .await
            .unwrap()
            .proxies
            .iter()
            .any(|proxy| proxy.name == "uncompilable")
    );

    // An overlong audit reason fails at commit on both backends; the coordinator
    // must neither publish nor leave the key revoked.
    let key = coordinator
        .create_own_api_key(
            world.user,
            SelfApiKeyCreate {
                name: "Coordinated key".into(),
                allowed_group_ids: vec![world.group],
                allowed_channel_ids: Vec::new(),
                expires_at: None,
                requests_per_minute: None,
                max_concurrent_requests: None,
                quota_limit_amount: None,
            },
        )
        .await
        .unwrap();
    let snapshot = runtime.snapshot();
    let failure = coordinator
        .revoke_own_api_key(world.user, key.id, "x".repeat(501))
        .await
        .err();
    assert!(matches!(
        failure,
        Some(ControlPlaneError::Repository(RepositoryError::Storage(_)))
    ));
    assert!(Arc::ptr_eq(&snapshot, &runtime.snapshot()));
    assert_eq!(
        repository
            .own_api_key(world.user, key.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "active"
    );
    assert!(
        !repository
            .audit_logs(100)
            .await
            .unwrap()
            .iter()
            .any(|audit| audit.action == "self_revoke" && audit.object_id == key.id)
    );

    // Dropping a prepared change rolls back its uncommitted row and audit row.
    let mut change = repository
        .prepare_mutation(
            world.admin,
            ControlPlaneMutation::CreateProxy(proxy_input("dropped")),
        )
        .await
        .unwrap();
    let dropped = change
        .runtime_records()
        .await
        .unwrap()
        .control_plane
        .proxies
        .iter()
        .find(|proxy| proxy.name == "dropped")
        .expect("the pending candidate contains the dropped proxy")
        .id;
    drop(change);
    assert!(
        !repository
            .load()
            .await
            .unwrap()
            .proxies
            .iter()
            .any(|proxy| proxy.id == dropped)
    );
    assert!(
        !repository
            .audit_logs(100)
            .await
            .unwrap()
            .iter()
            .any(|audit| audit.object_id == dropped)
    );

    // A manual reload records exactly one reload audit row and republishes.
    let snapshot = runtime.snapshot();
    let correlation_id = coordinator.manual_reload(world.admin).await.unwrap();
    assert!(!Arc::ptr_eq(&snapshot, &runtime.snapshot()));
    let reload = audit_for(&repositories, "reload", Uuid::nil()).await;
    assert_eq!(reload.object_type, "runtime_config");
    assert_eq!(
        reload.correlation_id.as_deref(),
        Some(correlation_id.to_string().as_str())
    );
}

// ---------------------------------------------------------------------------
// Shared lifecycle contracts for the remaining ordinary ControlPlaneMutation
// variants. Each case runs the identical facade calls on both backends.
// ---------------------------------------------------------------------------

/// The Console `ETag` round trip for one stored version: GET emits
/// `"<rfc3339 micros>Z"` and `If-Match` parses that same string back, so a read
/// model's `updated_at` must survive this format byte-for-byte on both
/// backends. Returning the parsed value keeps the mutation guards honest.
fn expected_etag(updated_at: DateTime<Utc>) -> DateTime<Utc> {
    let etag = format!(
        "\"{}\"",
        updated_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    );
    let body = etag
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .expect("Console ETags are quoted");
    assert!(body.ends_with('Z'), "Console ETags use the Z suffix");
    DateTime::parse_from_rfc3339(body)
        .expect("Console ETags carry RFC 3339 timestamps")
        .with_timezone(&Utc)
}

fn user_create_input(display_name: &str) -> UserInput {
    UserInput {
        display_name: display_name.into(),
        email: None,
        role: "user".into(),
        status: "active".into(),
        balance_amount: Decimal::ZERO,
        user_group_id: None,
        default_api_key_policy_id: None,
    }
}

fn user_group_input(name: &str) -> UserGroupInput {
    UserGroupInput {
        name: name.into(),
        description: Some(format!("{name} description")),
        default_api_key_policy_id: None,
        visible_codex_quota_group_ids: Vec::new(),
        filter_fast_mode: false,
    }
}

fn advanced_billing() -> AdvancedBilling {
    serde_json::from_value(json!({
        "long_context_tiers": [{
            "input_tokens_threshold": 128000,
            "input_unit_price": "0.3",
            "cached_input_unit_price": "0.15",
            "cache_write_unit_price": "0.6",
            "output_unit_price": "0.9"
        }],
        "request_multipliers": [{
            "json_pointer": "/reasoning/effort",
            "value": "high",
            "multiplier": "2"
        }]
    }))
    .expect("advanced billing fixture must deserialize")
}

fn model_input(source_model_id: &str, display_name: &str) -> ModelInput {
    ModelInput {
        source_model_id: source_model_id.into(),
        display_name: display_name.into(),
        provider_name: Some("Parity Provider".into()),
        enabled: true,
        price_unit_tokens: 1_000_000,
        input_unit_price: decimal("0.15"),
        cached_input_unit_price: decimal("0.075"),
        cache_write_unit_price: decimal("0.3"),
        output_unit_price: decimal("0.6"),
        price_effective_at: timestamp("2026-01-01T00:00:00Z"),
        advanced_billing: Some(advanced_billing()),
        source_payload: Some(json!({"id": source_model_id})),
    }
}

fn admin_api_key_create(user: Uuid, group: Uuid, name: &str) -> ApiKeyCreate {
    ApiKeyCreate {
        user_id: user,
        name: name.into(),
        allowed_api_formats: vec!["open_ai_chat_completions".into()],
        permissions: vec!["proxy".into(), "models.read".into()],
        allowed_group_ids: vec![group],
        allowed_channel_ids: Vec::new(),
        expires_at: None,
        requests_per_minute: Some(60),
        max_concurrent_requests: Some(4),
        quota_limit_amount: Some(decimal("100")),
    }
}

async fn listed_user_group(repositories: &Repositories, id: Uuid) -> ControlPlaneUserGroup {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .user_groups
        .into_iter()
        .find(|group| group.id == id)
        .unwrap_or_else(|| panic!("user group {id} must be listed"))
}

async fn listed_model(repositories: &Repositories, id: Uuid) -> ControlPlaneModel {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .models
        .into_iter()
        .find(|model| model.id == id)
        .unwrap_or_else(|| panic!("model {id} must be listed"))
}

async fn listed_channel(repositories: &Repositories, id: Uuid) -> LogicalChannelRecord {
    repositories
        .control_plane
        .topology()
        .await
        .unwrap()
        .logical_channels
        .into_iter()
        .find(|channel| channel.id == id && channel.deleted_at.is_none())
        .unwrap_or_else(|| panic!("channel {id} must be listed"))
}

fn logical_input(channel: &LogicalChannelRecord) -> ai_gateway::persistence::LogicalChannelInput {
    ai_gateway::persistence::LogicalChannelInput {
        group_id: channel.group_id,
        access_id: channel.access_id,
        credential_id: channel.credential_id,
        name: channel.name.clone(),
        enabled: channel.enabled,
    }
}

fn capability_input(
    capability: &ai_gateway::persistence::ChannelCapabilityRecord,
) -> ai_gateway::persistence::ChannelCapabilityInput {
    ai_gateway::persistence::ChannelCapabilityInput {
        channel_id: capability.channel_id,
        settings: capability.settings.clone(),
        status_statistics_enabled: capability.status_statistics_enabled,
        config_template_id: capability.config_template_id,
        override_document: capability.override_document.clone(),
        billing_multiplier: capability.billing_multiplier,
    }
}

async fn listed_access(
    repositories: &Repositories,
    id: Uuid,
) -> ai_gateway::persistence::UpstreamAccessRecord {
    repositories
        .control_plane
        .topology()
        .await
        .unwrap()
        .upstream_accesses
        .into_iter()
        .find(|access| access.id == id && access.deleted_at.is_none())
        .unwrap()
}

fn access_input(
    access: &ai_gateway::persistence::UpstreamAccessRecord,
) -> ai_gateway::persistence::UpstreamAccessInput {
    ai_gateway::persistence::UpstreamAccessInput {
        name: access.name.clone(),
        connector_kind: access.connector_kind,
        base_url: access.base_url.clone(),
        proxy_id: access.proxy_id,
        connect_timeout_ms: access.connect_timeout_ms,
        response_header_timeout_ms: access.response_header_timeout_ms,
        stream_idle_timeout_ms: access.stream_idle_timeout_ms,
        enabled: access.enabled,
    }
}

async fn listed_capability(repositories: &Repositories, id: Uuid) -> ChannelCapabilityRecord {
    repositories
        .control_plane
        .topology()
        .await
        .unwrap()
        .channel_capabilities
        .into_iter()
        .find(|capability| capability.id == id && capability.deleted_at.is_none())
        .unwrap_or_else(|| panic!("capability {id} must be listed"))
}

struct OperationGraph {
    rule: ai_gateway::persistence::OperationRuleRecord,
    tiers: Vec<ai_gateway::persistence::OperationTierRecord>,
    candidates: Vec<ai_gateway::persistence::OperationCandidateRecord>,
}

async fn operation_graph(repositories: &Repositories, profile: Uuid) -> OperationGraph {
    let topology = repositories.control_plane.topology().await.unwrap();
    let rule = topology
        .operation_rules
        .into_iter()
        .find(|rule| {
            rule.model_routing_profile_id == profile
                && rule.operation == ai_gateway::domain::ApiOperation::ChatCompletions
        })
        .unwrap();
    let tiers = topology
        .operation_tiers
        .into_iter()
        .filter(|tier| tier.rule_id == rule.id)
        .collect::<Vec<_>>();
    let candidates = topology
        .operation_candidates
        .into_iter()
        .filter(|candidate| tiers.iter().any(|tier| tier.id == candidate.tier_id))
        .collect();
    OperationGraph {
        rule,
        tiers,
        candidates,
    }
}

async fn listed_key(repositories: &Repositories, id: Uuid) -> ControlPlaneApiKey {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .api_keys
        .into_iter()
        .find(|key| key.id == id)
        .unwrap_or_else(|| panic!("api key {id} must be listed"))
}

async fn listed_policy(repositories: &Repositories, id: Uuid) -> ControlPlaneApiKeyPolicy {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .api_key_policies
        .into_iter()
        .find(|policy| policy.id == id)
        .unwrap_or_else(|| panic!("api key policy {id} must be listed"))
}

async fn listed_proxy(repositories: &Repositories, id: Uuid) -> ControlPlaneProxy {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .proxies
        .into_iter()
        .find(|proxy| proxy.id == id)
        .unwrap_or_else(|| panic!("proxy {id} must be listed"))
}

async fn listed_template(repositories: &Repositories, id: Uuid) -> ControlPlaneConfigTemplate {
    repositories
        .control_plane
        .control_plane_lists()
        .await
        .unwrap()
        .config_templates
        .into_iter()
        .find(|template| template.id == id)
        .unwrap_or_else(|| panic!("config template {id} must be listed"))
}

async fn listed_group(repositories: &Repositories, id: Uuid) -> RoutingGroupRecord {
    repositories
        .control_plane
        .topology()
        .await
        .unwrap()
        .routing_groups
        .into_iter()
        .find(|group| group.id == id && group.deleted_at.is_none())
        .unwrap_or_else(|| panic!("channel group {id} must be listed"))
}

async fn listed_model_rule(
    repositories: &Repositories,
    id: Uuid,
) -> ai_gateway::persistence::upstream_topology::profiles::RoutingProfileView {
    repositories
        .control_plane
        .routing_profiles()
        .await
        .unwrap()
        .into_iter()
        .find(|rule| rule.id == id)
        .unwrap_or_else(|| panic!("model rule {id} must be listed"))
}

#[tokio::test]
async fn user_group_lifecycle_contract_matches_across_backends() {
    run_contract(user_group_lifecycle_contract).await;
}

async fn user_group_lifecycle_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    // Quota visibility names the canonical group owning a Codex connector pool.
    let codex_group = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Parity Codex".into(),
                sharing_only: false,
                enabled: true,
            },
        },
    )
    .await
    .id;
    repository
        .prepare_codex_credential_create(
            world.admin,
            business_codex_credential(
                codex_group,
                "Parity credential",
                "parity@example.test",
                "parity-member",
            ),
            None,
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();

    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateUserGroup(UserGroupInput {
            name: "Parity Team".into(),
            description: Some("Parity team".into()),
            default_api_key_policy_id: Some(world.policy),
            visible_codex_quota_group_ids: vec![codex_group],
            filter_fast_mode: true,
        }),
    )
    .await;
    assert_eq!(created.object_type, "user_group");
    assert_eq!(created.action, "create");
    assert!(created.before_redacted.as_object().unwrap().is_empty());
    let group = listed_user_group(&repositories, created.id).await;
    assert_eq!(group.name, "Parity Team");
    assert_eq!(group.description.as_deref(), Some("Parity team"));
    assert_eq!(group.default_api_key_policy_id, Some(world.policy));
    assert_eq!(group.visible_codex_quota_group_ids, vec![codex_group]);
    assert!(group.filter_fast_mode);
    assert_eq!(group.system_role, None);
    assert_eq!(group.member_count, 0);
    assert_eq!(
        expected_etag(group.updated_at),
        group.updated_at,
        "both backends store a version the Console ETag format can echo unchanged"
    );

    // Membership and the effective policy follow the group.
    let member = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateUser(UserInput {
            display_name: "Team Member".into(),
            email: Some("team-member@example.test".into()),
            role: "user".into(),
            status: "active".into(),
            balance_amount: Decimal::ZERO,
            user_group_id: Some(created.id),
            default_api_key_policy_id: None,
        }),
    )
    .await
    .id;
    assert_eq!(
        listed_user_group(&repositories, created.id)
            .await
            .member_count,
        1
    );
    let listed_member = listed_user(&repositories, member).await;
    assert_eq!(listed_member.user_group_id, created.id);
    assert_eq!(
        listed_member.effective_api_key_policy_id,
        Some(world.policy)
    );

    // A non-Codex target is rejected without writing; a stale version conflicts.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::UpdateUserGroup {
                    id: created.id,
                    input: UserGroupInput {
                        visible_codex_quota_group_ids: vec![world.group],
                        ..user_group_input("Parity Team")
                    },
                    expected_updated_at: expected_etag(group.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::UpdateUserGroup {
                    id: created.id,
                    input: user_group_input("Parity Team"),
                    expected_updated_at: expected_etag(group.updated_at)
                        - ChronoDuration::seconds(1),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    assert_eq!(
        listed_user_group(&repositories, created.id).await.name,
        "Parity Team"
    );

    // A committed update replaces every field, including clearing visibility.
    let update = |expected_updated_at| ControlPlaneMutation::UpdateUserGroup {
        id: created.id,
        input: UserGroupInput {
            name: "Parity Team Renamed".into(),
            description: None,
            default_api_key_policy_id: Some(world.policy),
            visible_codex_quota_group_ids: Vec::new(),
            filter_fast_mode: false,
        },
        expected_updated_at,
    };
    let updated = commit_mutation(
        &repositories,
        world.admin,
        update(expected_etag(group.updated_at)),
    )
    .await;
    assert_eq!(updated.action, "update");
    assert_eq!(
        updated.after_redacted["default_api_key_policy_id"],
        json!(world.policy)
    );
    let group = listed_user_group(&repositories, created.id).await;
    assert_eq!(group.name, "Parity Team Renamed");
    assert_eq!(group.description, None);
    assert!(group.visible_codex_quota_group_ids.is_empty());
    assert!(!group.filter_fast_mode);
    assert_eq!(group.member_count, 1);

    // A non-administrator cannot manage user groups.
    assert!(matches!(
        repository
            .prepare_mutation(member, update(expected_etag(group.updated_at)))
            .await
            .err(),
        Some(RepositoryError::InvalidActor)
    ));

    // Deletion reassigns members and disables group-bound registration codes.
    let code = repositories
        .auth
        .create_registration_invitation_code(
            world.admin,
            b"parity-team-registration",
            RegistrationInvitationCodeInput {
                name: "Team Code".into(),
                max_uses: Some(1),
                expires_at: Some(timestamp("2030-01-01T00:00:00Z")),
                enabled: true,
                user_group_id: created.id,
                initial_balance_amount: decimal("3"),
            },
        )
        .await
        .unwrap();
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteUserGroup {
            id: created.id,
            deleted_by: world.admin,
            expected_updated_at: expected_etag(group.updated_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert_eq!(
        deleted.reason.as_deref(),
        Some("1 users reassigned; 1 registration invitation codes disabled")
    );
    assert_eq!(
        listed_user(&repositories, member).await.user_group_id,
        DEFAULT_USER_GROUP_ID
    );
    assert!(
        repository
            .control_plane_lists()
            .await
            .unwrap()
            .user_groups
            .iter()
            .all(|group| group.id != created.id)
    );
    let code = repositories
        .auth
        .registration_invitation_code(code.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!code.enabled);
}

#[tokio::test]
async fn user_lifecycle_contract_matches_across_backends() {
    run_contract(user_lifecycle_contract).await;
}

async fn user_lifecycle_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateUser(UserInput {
            display_name: "Lifecycle User".into(),
            email: Some("Lifecycle@Example.Test".into()),
            role: "user".into(),
            status: "active".into(),
            balance_amount: decimal("5.25"),
            user_group_id: None,
            default_api_key_policy_id: Some(world.policy),
        }),
    )
    .await;
    let user = created.id;
    assert_eq!(created.object_type, "user");
    assert_eq!(created.action, "create");
    let listed = listed_user(&repositories, user).await;
    assert_eq!(listed.display_name, "Lifecycle User");
    assert_eq!(listed.email.as_deref(), Some("Lifecycle@Example.Test"));
    assert_eq!(listed.role, "user");
    assert_eq!(listed.status, "active");
    assert_eq!(listed.balance_amount, decimal("5.25"));
    assert_eq!(listed.user_group_id, DEFAULT_USER_GROUP_ID);
    assert_eq!(listed.default_api_key_policy_id, Some(world.policy));
    assert_eq!(listed.effective_api_key_policy_id, Some(world.policy));
    assert!(!listed.websocket_enabled);
    assert!(!listed.can_reissue_invitation);

    // A live session is revoked when identity fields change.
    let session = Uuid::from_u128(0x9201);
    repositories
        .auth
        .create_session(
            session,
            user,
            b"lifecycle-refresh",
            timestamp("9999-01-02T03:04:05.123456Z"),
            Some("lifecycle"),
            ConsoleSessionPurpose::Normal,
        )
        .await
        .unwrap();
    assert!(
        repositories
            .auth
            .validate_console_identity(user, session, 1)
            .await
            .unwrap()
            .is_some()
    );

    let update = |expected_updated_at| ControlPlaneMutation::UpdateUser {
        id: user,
        input: UserUpdateInput {
            display_name: Some("Lifecycle Admin".into()),
            email: Some(Some("lifecycle-admin@example.test".into())),
            role: Some("admin".into()),
            status: Some("suspended".into()),
            balance_amount: Some(decimal("7.5")),
            user_group_id: None,
            default_api_key_policy_id: Some(Some(world.policy)),
            websocket_enabled: Some(true),
        },
        expected_updated_at,
    };
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                update(expected_etag(listed.updated_at) - ChronoDuration::seconds(1)),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let updated = commit_mutation(
        &repositories,
        world.admin,
        update(expected_etag(listed.updated_at)),
    )
    .await;
    assert_eq!(updated.action, "update");
    assert_eq!(
        updated.before_redacted["default_api_key_policy_id"],
        json!(world.policy)
    );
    let listed = listed_user(&repositories, user).await;
    assert_eq!(listed.display_name, "Lifecycle Admin");
    assert_eq!(
        listed.email.as_deref(),
        Some("lifecycle-admin@example.test")
    );
    assert_eq!(listed.role, "admin");
    assert_eq!(listed.status, "suspended");
    assert_eq!(listed.balance_amount, decimal("7.5"));
    assert_eq!(listed.user_group_id, DEFAULT_ADMIN_GROUP_ID);
    assert!(listed.websocket_enabled);
    assert!(
        repositories
            .auth
            .validate_console_identity(user, session, 2)
            .await
            .unwrap()
            .is_none(),
        "identity changes revoke live sessions"
    );
    assert_eq!(
        repositories
            .auth
            .password_user(user)
            .await
            .unwrap()
            .unwrap()
            .auth_version,
        2
    );

    // A cosmetic update preserves the authorization epoch.
    let cosmetic = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateUser {
            id: user,
            input: UserUpdateInput {
                display_name: Some("Lifecycle Admin II".into()),
                email: None,
                role: None,
                status: None,
                balance_amount: None,
                user_group_id: None,
                default_api_key_policy_id: None,
                websocket_enabled: None,
            },
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(cosmetic.action, "update");
    assert_eq!(
        repositories
            .auth
            .password_user(user)
            .await
            .unwrap()
            .unwrap()
            .auth_version,
        2
    );

    // An administrator cannot delete their own account.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteUser {
                    id: user,
                    deleted_by: user,
                    expected_updated_at: expected_etag(listed.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::CannotDeleteSelf)
    ));

    let key = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateApiKey(admin_api_key_create(
            user,
            world.group,
            "Lifecycle key",
        )),
    )
    .await
    .id;

    let listed = listed_user(&repositories, user).await;
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteUser {
            id: user,
            deleted_by: world.admin,
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert_eq!(
        deleted.reason.as_deref(),
        Some("user anonymized and API keys deleted")
    );
    assert!(
        repository
            .control_plane_lists()
            .await
            .unwrap()
            .users
            .iter()
            .all(|user| user.id != deleted.id)
    );
    assert!(repository.own_api_keys(user).await.unwrap().is_empty());
    assert!(repository.own_api_key(user, key).await.unwrap().is_none());
    assert!(
        repository
            .audit_logs(100)
            .await
            .unwrap()
            .iter()
            .any(|audit| {
                audit.action == "delete" && audit.object_type == "user" && audit.object_id == user
            })
    );
}

#[tokio::test]
async fn model_routing_lifecycle_contract_matches_across_backends() {
    run_contract(model_routing_lifecycle_contract).await;
}

async fn model_routing_lifecycle_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    // A non-administrator cannot create models.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.user,
                ControlPlaneMutation::CreateModel(model_input("unauthorized", "Unauthorized")),
            )
            .await
            .err(),
        Some(RepositoryError::InvalidActor)
    ));

    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("parity-model", "Parity Model")),
    )
    .await;
    let model = created.id;
    assert_eq!(created.object_type, "model");
    assert_eq!(created.action, "create");
    let listed = listed_model(&repositories, model).await;
    assert_eq!(listed.source_model_id, "parity-model");
    assert_eq!(listed.display_name, "Parity Model");
    assert_eq!(listed.provider_name.as_deref(), Some("Parity Provider"));
    assert!(listed.enabled);
    assert_eq!(listed.price_unit_tokens, 1_000_000);
    assert_eq!(listed.input_unit_price, decimal("0.15"));
    assert_eq!(listed.cached_input_unit_price, decimal("0.075"));
    assert_eq!(listed.cache_write_unit_price, decimal("0.3"));
    assert_eq!(listed.output_unit_price, decimal("0.6"));
    assert_eq!(
        listed.advanced_billing,
        serde_json::to_value(advanced_billing()).unwrap()
    );
    assert!(listed.last_synced_at.is_none());
    assert_eq!(
        repository.model_source_ids().await.unwrap(),
        ["parity-model"]
    );

    // An update without advanced billing or a source payload preserves them.
    let update = |expected_updated_at| ControlPlaneMutation::UpdateModel {
        id: model,
        input: ModelInput {
            source_model_id: "parity-model".into(),
            display_name: "Parity Model v2".into(),
            provider_name: None,
            enabled: true,
            price_unit_tokens: 2_000_000,
            input_unit_price: decimal("0.2"),
            cached_input_unit_price: decimal("0.1"),
            cache_write_unit_price: decimal("0.4"),
            output_unit_price: decimal("0.8"),
            price_effective_at: timestamp("2026-02-01T00:00:00Z"),
            advanced_billing: None,
            source_payload: None,
        },
        expected_updated_at,
    };
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                update(expected_etag(listed.updated_at) - ChronoDuration::seconds(1)),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let updated = commit_mutation(
        &repositories,
        world.admin,
        update(expected_etag(listed.updated_at)),
    )
    .await;
    assert_eq!(updated.action, "update");
    assert_eq!(updated.after_redacted["display_name"], "Parity Model v2");
    let listed = listed_model(&repositories, model).await;
    assert_eq!(listed.display_name, "Parity Model v2");
    assert_eq!(listed.provider_name, None);
    assert_eq!(listed.price_unit_tokens, 2_000_000);
    assert_eq!(listed.input_unit_price, decimal("0.2"));
    assert_eq!(listed.output_unit_price, decimal("0.8"));
    assert_eq!(
        listed.advanced_billing,
        serde_json::to_value(advanced_billing()).unwrap(),
        "omitting advanced billing preserves the stored policy"
    );

    // A non-object source payload is rejected without writing.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::UpdateModel {
                    id: model,
                    input: ModelInput {
                        source_payload: Some(json!(["not", "an", "object"])),
                        ..model_input("parity-model", "Parity Model v2")
                    },
                    expected_updated_at: expected_etag(listed.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));

    // Priced models own one profile, with at most one child per operation.
    let profile = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id: model }),
    )
    .await;
    assert_eq!(profile.object_type, "model_rule");
    assert_eq!(profile.action, "create");
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id: model }),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));

    let protocol = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveOperationRule {
            id: Uuid::new_v4(),
            expected_updated_at: None,
            input: ai_gateway::persistence::OperationRuleInput {
                model_routing_profile_id: profile.id,
                operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                enabled: false,
                routing_tiers: vec![],
            },
        },
    )
    .await;
    assert_eq!(protocol.object_type, "model_operation_rule");
    assert_eq!(protocol.action, "create");
    let rule = listed_model_rule(&repositories, profile.id).await;
    assert_eq!(rule.model_id, model);
    assert_eq!(rule.client_model, "parity-model");
    assert_eq!(rule.model_display_name, "Parity Model v2");
    let graph = operation_graph(&repositories, profile.id).await;
    assert!(!graph.rule.enabled);
    assert!(graph.tiers.is_empty());
    assert!(graph.candidates.is_empty());
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::SaveOperationRule {
                    id: Uuid::new_v4(),
                    expected_updated_at: None,
                    input: ai_gateway::persistence::OperationRuleInput {
                        model_routing_profile_id: profile.id,
                        operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                        enabled: false,
                        routing_tiers: vec![],
                    },
                },
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));

    // A ready route commits whole, and every guard is checked first.
    let update_rule = |expected_updated_at| ControlPlaneMutation::SaveOperationRule {
        id: protocol.id,
        input: ai_gateway::persistence::OperationRuleInput {
            model_routing_profile_id: profile.id,
            operation: ai_gateway::domain::ApiOperation::ChatCompletions,
            routing_tiers: vec![ai_gateway::persistence::OperationTierInput {
                priority: 0,
                selection_strategy: "weighted_round_robin".into(),
                candidates: vec![ai_gateway::persistence::OperationCandidateInput {
                    capability_id: world.capability,
                    upstream_model: "parity-model".into(),
                    weight: 3,
                }],
            }],
            enabled: true,
        },
        expected_updated_at: Some(expected_updated_at),
    };
    let protocol_updated_at = operation_graph(&repositories, profile.id)
        .await
        .rule
        .updated_at;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                update_rule(expected_etag(protocol_updated_at) - ChronoDuration::seconds(1)),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::SaveOperationRule {
                    id: protocol.id,
                    input: ai_gateway::persistence::OperationRuleInput {
                        model_routing_profile_id: profile.id,
                        operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                        routing_tiers: vec![ai_gateway::persistence::OperationTierInput {
                            priority: 0,
                            selection_strategy: "weighted_random".into(),
                            candidates: vec![ai_gateway::persistence::OperationCandidateInput {
                                capability_id: world.capability,
                                upstream_model: "unknown-model".into(),
                                weight: 1,
                            }],
                        }],
                        enabled: true,
                    },
                    expected_updated_at: Some(expected_etag(protocol_updated_at)),
                },
            )
            .await
            .err(),
        Some(RepositoryError::RoutingDependencyInvalid)
    ));
    let updated = commit_mutation(
        &repositories,
        world.admin,
        update_rule(expected_etag(protocol_updated_at)),
    )
    .await;
    assert_eq!(updated.action, "update");
    let graph = operation_graph(&repositories, profile.id).await;
    assert!(graph.rule.enabled);
    assert_eq!(graph.tiers.len(), 1);
    assert_eq!(graph.tiers[0].priority, 0);
    assert_eq!(graph.tiers[0].strategy, "weighted_round_robin");
    assert_eq!(graph.candidates.len(), 1);
    assert_eq!(graph.candidates[0].capability_id, world.capability);
    assert_eq!(graph.candidates[0].upstream_model, "parity-model");
    assert_eq!(graph.candidates[0].weight, 3);
    assert!(
        listed_capability(&repositories, world.capability)
            .await
            .settings
            .enabled
    );
    compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap();

    // A disabled model cannot carry a routing profile.
    let disabled_model = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(ModelInput {
            enabled: false,
            ..model_input("parity-model-disabled", "Disabled Model")
        }),
    )
    .await
    .id;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::CreateRule(ModelRuleCreateInput {
                    model_id: disabled_model,
                }),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));

    // Deleting the priced model disables its protocol rules and removes it from
    // every read projection.
    let listed = listed_model(&repositories, model).await;
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteModel {
            id: model,
            deleted_by: world.admin,
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert_eq!(
        deleted.reason.as_deref(),
        Some("1 operation rules disabled; 0 scheduled test references cleared")
    );
    assert!(
        repository
            .control_plane_lists()
            .await
            .unwrap()
            .models
            .iter()
            .all(|model| model.id != deleted.id)
    );
    assert!(
        repository
            .model_source_ids()
            .await
            .unwrap()
            .iter()
            .all(|id| id != "parity-model")
    );
}

#[tokio::test]
async fn admin_api_key_and_policy_contract_matches_across_backends() {
    run_contract(admin_api_key_and_policy_contract).await;
}

async fn admin_api_key_and_policy_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateApiKey(admin_api_key_create(
            world.user,
            world.group,
            "Admin key",
        )),
    )
    .await;
    assert_eq!(created.object_type, "api_key");
    assert_eq!(created.action, "create");
    let secret = created
        .created_secret
        .clone()
        .expect("an administrative create returns the secret exactly once");
    assert!(secret.starts_with("sk-"));
    assert!(created.after_redacted["allowed_group_ids"].is_array());
    let listed = listed_key(&repositories, created.id).await;
    assert_eq!(listed.user_id, world.user);
    assert_eq!(listed.user_status, "active");
    assert_eq!(listed.name, "Admin key");
    assert_eq!(listed.secret, secret);
    assert_eq!(listed.status, "active");
    assert_eq!(listed.allowed_api_formats, ["open_ai_chat_completions"]);
    assert_eq!(listed.permissions, ["proxy", "models.read"]);
    assert_eq!(listed.allowed_group_ids, vec![world.group]);
    assert!(listed.allowed_channel_ids.is_empty());
    assert_eq!(listed.requests_per_minute, Some(60));
    assert_eq!(listed.max_concurrent_requests, Some(4));
    assert_eq!(listed.quota_limit_amount, Some(decimal("100")));
    assert_eq!(listed.quota_used_amount, Decimal::ZERO);

    // A non-user owner is rejected without writing.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::CreateApiKey(ApiKeyCreate {
                    user_id: world.group,
                    ..admin_api_key_create(world.user, world.group, "Orphan key")
                }),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));

    let update = |expected_updated_at| ControlPlaneMutation::UpdateApiKey {
        id: created.id,
        input: ApiKeyUpdate {
            name: "Admin key renamed".into(),
            status: "disabled".into(),
            allowed_api_formats: vec![
                "open_ai_chat_completions".into(),
                "open_ai_responses".into(),
            ],
            permissions: vec!["proxy".into()],
            allowed_group_ids: vec![world.group],
            allowed_channel_ids: vec![world.channel],
            expires_at: Some(timestamp("2030-01-01T00:00:00Z")),
            requests_per_minute: Some(120),
            max_concurrent_requests: None,
            quota_limit_amount: None,
        },
        expected_updated_at,
    };
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                update(expected_etag(listed.updated_at) - ChronoDuration::seconds(1)),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let updated = commit_mutation(
        &repositories,
        world.admin,
        update(expected_etag(listed.updated_at)),
    )
    .await;
    assert_eq!(updated.action, "update");
    let listed = listed_key(&repositories, created.id).await;
    assert_eq!(listed.name, "Admin key renamed");
    assert_eq!(listed.status, "disabled");
    assert_eq!(
        listed.allowed_api_formats,
        ["open_ai_chat_completions", "open_ai_responses"]
    );
    assert_eq!(listed.permissions, ["proxy"]);
    assert_eq!(listed.allowed_channel_ids, vec![world.channel]);
    assert_eq!(listed.expires_at, Some(timestamp("2030-01-01T00:00:00Z")));
    assert_eq!(listed.requests_per_minute, Some(120));
    assert_eq!(listed.max_concurrent_requests, None);
    assert_eq!(listed.quota_limit_amount, None);

    // Revocation is one-way and audited with its reason.
    let revoked = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::RevokeApiKey {
            id: created.id,
            reason: "compromised".into(),
        },
    )
    .await;
    assert_eq!(revoked.action, "revoke");
    assert_eq!(revoked.reason.as_deref(), Some("compromised"));
    assert_eq!(
        listed_key(&repositories, created.id).await.status,
        "revoked"
    );
    let revoked_at = listed_key(&repositories, created.id).await.updated_at;
    assert!(matches!(
        repository
            .prepare_mutation(world.admin, update(expected_etag(revoked_at)))
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));

    // Deletion erases the secret and removes the key from every read.
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteApiKey {
            id: created.id,
            deleted_by: world.admin,
            expected_updated_at: expected_etag(revoked_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert_eq!(
        deleted.reason.as_deref(),
        Some("API key deleted and secret erased")
    );
    assert!(
        repository
            .control_plane_lists()
            .await
            .unwrap()
            .api_keys
            .iter()
            .all(|key| key.id != deleted.id)
    );
    assert!(
        repository
            .own_api_keys(world.user)
            .await
            .unwrap()
            .is_empty()
    );

    // A policy update replaces its targets and can be disabled.
    let update_policy = |expected_updated_at| ControlPlaneMutation::UpdateApiKeyPolicy {
        id: world.policy,
        input: ApiKeyPolicyInput {
            name: "Parity Policy v2".into(),
            allowed_group_ids: vec![world.group],
            allowed_channel_ids: vec![world.channel],
            enabled: false,
        },
        expected_updated_at,
    };
    let policy = listed_policy(&repositories, world.policy).await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                update_policy(expected_etag(policy.updated_at) - ChronoDuration::seconds(1)),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let updated = commit_mutation(
        &repositories,
        world.admin,
        update_policy(expected_etag(policy.updated_at)),
    )
    .await;
    assert_eq!(updated.action, "update");
    assert_eq!(
        updated.before_redacted["allowed_channel_ids"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let policy = listed_policy(&repositories, world.policy).await;
    assert_eq!(policy.name, "Parity Policy v2");
    assert_eq!(policy.allowed_group_ids, vec![world.group]);
    assert_eq!(policy.allowed_channel_ids, vec![world.channel]);
    assert!(!policy.enabled);

    // A disabled policy cannot be attached to a user or a user group.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::CreateUserGroup(UserGroupInput {
                    default_api_key_policy_id: Some(world.policy),
                    ..user_group_input("Disabled policy group")
                }),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::CreateUser(UserInput {
                    default_api_key_policy_id: Some(world.policy),
                    ..user_create_input("Disabled Policy User")
                }),
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
}

#[tokio::test]
async fn channel_lifecycle_contract_matches_across_backends() {
    run_contract(channel_lifecycle_contract).await;
}

#[tokio::test]
async fn reusable_upstream_identity_contract_matches_across_backends() {
    run_contract(reusable_upstream_identity_contract).await;
}

async fn reusable_upstream_identity_contract(repositories: Repositories) {
    use ai_gateway::{domain::UpstreamAuth, persistence::UpstreamCredentialInput};
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let mut input = UpstreamCredentialInput {
        name: "shared identity".into(),
        kind: "bearer".into(),
        header_name: None,
        secret: Some("identity-first-secret".into()),
        enabled: true,
        allowed_base_urls: vec!["https://upstream.example.test".into()],
    };
    let created = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::CreateUpstreamCredential(input.clone()),
        )
        .await
        .unwrap();
    let id = created.id;
    assert!(
        !created
            .after_redacted
            .to_string()
            .contains("identity-first-secret")
    );
    let channel = listed_channel(&repositories, world.channel).await;
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: world.channel,
                input: ai_gateway::persistence::LogicalChannelInput {
                    credential_id: Some(id),
                    name: "bound identity".into(),
                    ..logical_input(&channel)
                },
                expected: Some(channel.updated_at),
            },
        )
        .await
        .unwrap();
    let second = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: Uuid::new_v4(),
                expected: None,
                input: ai_gateway::persistence::LogicalChannelInput {
                    group_id: world.group,
                    access_id: world.access,
                    credential_id: Some(id),
                    name: "second identity reference".into(),
                    enabled: false,
                },
            },
        )
        .await
        .unwrap()
        .id;
    let capability = listed_capability(&repositories, world.capability).await;
    let second_capability = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveChannelCapability {
                id: Uuid::new_v4(),
                expected: None,
                input: ai_gateway::persistence::ChannelCapabilityInput {
                    channel_id: second,
                    settings: capability.settings,
                    status_statistics_enabled: false,
                    config_template_id: None,
                    override_document: json!({}),
                    billing_multiplier: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap()
        .id;
    let bound = repository
        .upstream_credential_detail(id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.credential.channel_ids.len(), 2);
    let original_revision = repository
        .load()
        .await
        .unwrap()
        .channels
        .into_iter()
        .find(|channel| channel.id == world.capability)
        .unwrap()
        .credential
        .unwrap()
        .revision;
    let old_version = bound.credential.updated_at;
    input.secret = Some("identity-rotated-secret".into());
    let rotated = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::UpdateUpstreamCredential {
                id,
                input: input.clone(),
                expected_updated_at: old_version,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(runtime.snapshot().channel(world.capability).unwrap().upstream_auth(),
        UpstreamAuth::Bearer(secret) if secret.as_ref()=="identity-rotated-secret")
    );
    let bound_records = repository
        .load()
        .await
        .unwrap()
        .channels
        .into_iter()
        .filter(|record| [world.capability, second_capability].contains(&record.id))
        .collect::<Vec<_>>();
    assert_eq!(bound_records.len(), 2);
    for record in bound_records {
        assert_eq!(
            record.upstream_api_key.as_deref(),
            Some("identity-rotated-secret")
        );
        assert_ne!(record.credential.unwrap().revision, original_revision);
    }
    assert!(
        coordinator
            .mutate(
                world.admin,
                ControlPlaneMutation::UpdateUpstreamCredential {
                    id,
                    input: input.clone(),
                    expected_updated_at: old_version,
                }
            )
            .await
            .is_err()
    );
    let before = runtime.snapshot();
    let mut invalid_scope = input.clone();
    invalid_scope.allowed_base_urls = vec!["https://unrelated.example.test".into()];
    assert!(
        coordinator
            .mutate(
                world.admin,
                ControlPlaneMutation::UpdateUpstreamCredential {
                    id,
                    input: invalid_scope,
                    expected_updated_at: rotated.updated_at,
                }
            )
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(&before, &runtime.snapshot()));
    let mut invalid_kind = input.clone();
    invalid_kind.kind = "header".into();
    invalid_kind.header_name = Some("x-api-key".into());
    assert!(
        coordinator
            .mutate(
                world.admin,
                ControlPlaneMutation::UpdateUpstreamCredential {
                    id,
                    input: invalid_kind,
                    expected_updated_at: rotated.updated_at,
                }
            )
            .await
            .is_err()
    );
    input.secret = None;
    input.enabled = false;
    let disabled = coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::UpdateUpstreamCredential {
                id,
                input: input.clone(),
                expected_updated_at: rotated.updated_at,
            },
        )
        .await
        .unwrap();
    assert!(runtime.snapshot().channel(world.capability).is_none());
    assert!(
        coordinator
            .mutate(
                world.admin,
                ControlPlaneMutation::DeleteUpstreamCredential {
                    id,
                    expected_updated_at: disabled.updated_at,
                }
            )
            .await
            .is_err()
    );
    input.enabled = true;
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::UpdateUpstreamCredential {
                id,
                input,
                expected_updated_at: disabled.updated_at,
            },
        )
        .await
        .unwrap();
    assert!(runtime.snapshot().channel(world.capability).is_some());
    let original_binding = repository
        .load()
        .await
        .unwrap()
        .channels
        .into_iter()
        .find(|channel| channel.id == world.capability)
        .unwrap()
        .credential_binding_revision;
    let channel = listed_channel(&repositories, world.channel).await;
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: world.channel,
                input: ai_gateway::persistence::LogicalChannelInput {
                    name: "unbound identity".into(),
                    credential_id: None,
                    ..logical_input(&channel)
                },
                expected: Some(channel.updated_at),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .snapshot()
            .channel(world.capability)
            .unwrap()
            .upstream_auth(),
        UpstreamAuth::None
    ));
    let channel = listed_channel(&repositories, world.channel).await;
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: world.channel,
                input: ai_gateway::persistence::LogicalChannelInput {
                    credential_id: Some(id),
                    name: "restored identity".into(),
                    ..logical_input(&channel)
                },
                expected: Some(channel.updated_at),
            },
        )
        .await
        .unwrap();
    let rebound = repository
        .load()
        .await
        .unwrap()
        .channels
        .into_iter()
        .find(|channel| channel.id == world.capability)
        .unwrap()
        .credential_binding_revision;
    assert_ne!(original_binding, rebound);
    let channel = listed_channel(&repositories, world.channel).await;
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: world.channel,
                input: ai_gateway::persistence::LogicalChannelInput {
                    name: "unbound again".into(),
                    credential_id: None,
                    ..logical_input(&channel)
                },
                expected: Some(channel.updated_at),
            },
        )
        .await
        .unwrap();
    let remaining = repository
        .upstream_credential_detail(id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remaining.credential.channel_ids, [second]);
    assert_eq!(remaining.secret.as_deref(), Some("identity-rotated-secret"));
    assert!(
        coordinator
            .mutate(
                world.admin,
                ControlPlaneMutation::DeleteUpstreamCredential {
                    id,
                    expected_updated_at: remaining.credential.updated_at,
                }
            )
            .await
            .is_err()
    );
    let channel = listed_channel(&repositories, second).await;
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::SaveLogicalChannel {
                id: second,
                input: ai_gateway::persistence::LogicalChannelInput {
                    enabled: false,
                    credential_id: None,
                    name: "unbound second".into(),
                    ..logical_input(&channel)
                },
                expected: Some(channel.updated_at),
            },
        )
        .await
        .unwrap();
    coordinator
        .mutate(
            world.admin,
            ControlPlaneMutation::DeleteUpstreamCredential {
                id,
                expected_updated_at: remaining.credential.updated_at,
            },
        )
        .await
        .unwrap();
    assert!(
        repository
            .upstream_credential_detail(id)
            .await
            .unwrap()
            .is_none()
    );
    let serialized =
        serde_json::to_string(&repository.upstream_credentials().await.unwrap()).unwrap();
    assert!(!serialized.contains("identity-rotated-secret"));
    assert!(
        !serde_json::to_string(&repository.audit_logs(100).await.unwrap())
            .unwrap()
            .contains("identity-rotated-secret")
    );
}

async fn channel_lifecycle_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;
    let credential = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateUpstreamCredential(
            ai_gateway::persistence::UpstreamCredentialInput {
                name: "Reusable channel identity".into(),
                kind: "bearer".into(),
                header_name: None,
                secret: Some("upstream-secret".into()),
                allowed_base_urls: vec![
                    "https://upstream-v2.example.test".into(),
                    "https://upstream.example.test".into(),
                ],
                enabled: true,
            },
        ),
    )
    .await
    .id;

    // Supporting resources for the update.
    let proxy = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(proxy_input("channel-proxy")),
    )
    .await
    .id;
    let template = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateConfigTemplate(ConfigTemplateCreateInput {
            name: "Channel template".into(),
            description: Some("Channel template".into()),
            document: json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-template": "on"}}
            }),
            enabled: true,
        }),
    )
    .await
    .id;

    let access = listed_access(&repositories, world.access).await;
    let input = ai_gateway::persistence::UpstreamAccessInput {
        base_url: "https://upstream-v2.example.test".into(),
        proxy_id: Some(proxy),
        connect_timeout_ms: Some(1_500),
        response_header_timeout_ms: Some(45_000),
        stream_idle_timeout_ms: Some(60_000),
        ..access_input(&access)
    };
    let update = ControlPlaneMutation::UpdateUpstreamAccess {
        id: world.access,
        input: input.clone(),
        expected_updated_at: expected_etag(access.updated_at) - ChronoDuration::seconds(1),
    };
    assert!(matches!(
        repository.prepare_mutation(world.admin, update).await.err(),
        Some(RepositoryError::Conflict)
    ));
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateUpstreamAccess {
            id: world.access,
            input,
            expected_updated_at: expected_etag(access.updated_at),
        },
    )
    .await;
    let channel = listed_channel(&repositories, world.channel).await;
    let updated = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: world.channel,
            expected: Some(channel.updated_at),
            input: ai_gateway::persistence::LogicalChannelInput {
                name: "Parity Channel v2".into(),
                credential_id: Some(credential),
                ..logical_input(&channel)
            },
        },
    )
    .await;
    let capability = listed_capability(&repositories, world.capability).await;
    let mut settings = capability.settings.clone();
    settings.auto_disable_allowed = true;
    settings.available_models = vec!["parity-model".into(), "parity-model-v2".into()];
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveChannelCapability {
            id: world.capability,
            expected: Some(capability.updated_at),
            input: ai_gateway::persistence::ChannelCapabilityInput {
                channel_id: world.channel,
                settings,
                status_statistics_enabled: true,
                billing_multiplier: decimal("1.5"),
                config_template_id: Some(template),
                override_document: json!({
                        "version": 1,
                        "api_format": "open_ai_chat_completions",
                        "request_headers": {"set": {"x-channel": "on"}}
                }),
            },
        },
    )
    .await;
    assert_eq!(updated.action, "update");
    assert!(updated.after_redacted.get("upstream_api_key").is_none());
    assert_eq!(updated.after_redacted["credential_id"], json!(credential));
    let detail = listed_channel(&repositories, world.channel).await;
    let access = listed_access(&repositories, world.access).await;
    let capability = listed_capability(&repositories, world.capability).await;
    assert_eq!(detail.name, "Parity Channel v2");
    assert_eq!(access.base_url, "https://upstream-v2.example.test");
    assert!(detail.enabled);
    assert!(capability.settings.auto_disable_allowed);
    assert_eq!(capability.billing_multiplier, decimal("1.5"));
    assert_eq!(access.proxy_id, Some(proxy));
    assert_eq!(capability.config_template_id, Some(template));
    assert_eq!(
        capability.override_document["request_headers"]["set"]["x-channel"],
        "on"
    );
    assert_eq!(access.connect_timeout_ms, Some(1_500));
    assert_eq!(access.response_header_timeout_ms, Some(45_000));
    assert_eq!(access.stream_idle_timeout_ms, Some(60_000));
    assert_eq!(detail.credential_id, Some(credential));
    assert_eq!(
        repository
            .upstream_credential_detail(credential)
            .await
            .unwrap()
            .unwrap()
            .secret
            .as_deref(),
        Some("upstream-secret")
    );
    assert_eq!(
        capability.settings.available_models,
        ["parity-model", "parity-model-v2"]
    );

    // A channel edit preserves the referenced identity without writing its secret.
    let preserve = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: world.channel,
            input: ai_gateway::persistence::LogicalChannelInput {
                name: "Parity Channel v3".into(),
                credential_id: Some(credential),
                ..logical_input(&detail)
            },
            expected: Some(expected_etag(detail.updated_at)),
        },
    )
    .await;
    assert_eq!(preserve.action, "update");
    let detail = listed_channel(&repositories, world.channel).await;
    assert_eq!(detail.credential_id, Some(credential));
    assert_eq!(
        serde_json::to_value(listed_capability(&repositories, world.capability).await).unwrap(),
        serde_json::to_value(&capability).unwrap()
    );
    assert_eq!(
        serde_json::to_value(listed_access(&repositories, world.access).await).unwrap(),
        serde_json::to_value(&access).unwrap()
    );
    let cleared = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: world.channel,
            input: ai_gateway::persistence::LogicalChannelInput {
                credential_id: None,
                ..logical_input(&detail)
            },
            expected: Some(expected_etag(detail.updated_at)),
        },
    )
    .await;
    assert_eq!(cleared.action, "update");
    let detail = listed_channel(&repositories, world.channel).await;
    assert_eq!(detail.credential_id, None);
    assert_eq!(
        repository
            .upstream_credential_detail(credential)
            .await
            .unwrap()
            .unwrap()
            .secret
            .as_deref(),
        Some("upstream-secret")
    );

    // A batch update maps each target version and is all-or-nothing.
    let listed = listed_capability(&repositories, world.capability).await;
    let batch = |expected_updated_at| ChannelBatchUpdateInput {
        items: vec![ChannelBatchUpdateTarget {
            id: world.capability,
            updated_at: expected_updated_at,
        }],
        changes: ChannelBatchChanges {
            enabled: Some(false),
            auto_disable_allowed: Some(true),
            billing_multiplier: Some(decimal("2")),
        },
    };
    assert!(matches!(
        repository
            .prepare_channels_batch(
                world.admin,
                batch(expected_etag(listed.updated_at) - ChronoDuration::seconds(1)),
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    let (mut mutations, _) = repository
        .prepare_channels_batch(world.admin, batch(expected_etag(listed.updated_at)))
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(mutations.pop().unwrap().action, "batch_update");
    let listed = listed_capability(&repositories, world.capability).await;
    assert!(!listed.settings.enabled);
    let capability = listed_capability(&repositories, world.capability).await;
    assert!(capability.settings.auto_disable_allowed);
    assert_eq!(capability.billing_multiplier, decimal("2"));
    let (mut mutations, _) = repository
        .prepare_channels_batch(
            world.admin,
            ChannelBatchUpdateInput {
                items: vec![ChannelBatchUpdateTarget {
                    id: world.capability,
                    updated_at: expected_etag(listed.updated_at),
                }],
                changes: ChannelBatchChanges {
                    enabled: Some(true),
                    auto_disable_allowed: Some(true),
                    billing_multiplier: None,
                },
            },
        )
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(mutations.pop().unwrap().action, "batch_update");

    // Automatic disable follows persisted settings; manual recovery is explicit.
    let settings = repository.system_settings().await.unwrap();
    let mut input = settings.settings.clone();
    input.automatic_disable = SystemAutomaticDisableSettingsInput {
        enabled: true,
        error_status_codes: vec![429],
        error_message_keywords: Vec::new(),
    };
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateSystemSettings {
            input,
            expected_updated_at: settings.updated_at,
        },
    )
    .await;
    assert!(
        repository
            .prepare_channel_disable(world.capability, &AutomaticDisableTrigger::HttpStatus(503))
            .await
            .unwrap()
            .is_none(),
        "a non-matching trigger prepares nothing"
    );
    let mut change = repository
        .prepare_channel_disable(world.capability, &AutomaticDisableTrigger::HttpStatus(429))
        .await
        .unwrap()
        .expect("a matching trigger prepares an automatic disable");
    assert!(
        change
            .runtime_records()
            .await
            .unwrap()
            .control_plane
            .channels[0]
            .auto_disabled
    );
    let (mut mutations, correlation_id) = change.commit().await.unwrap();
    assert_eq!(mutations.pop().unwrap().action, "auto_disable");
    assert_ne!(correlation_id, Uuid::nil());
    let listed = listed_capability(&repositories, world.capability).await;
    assert!(listed.auto_disabled);
    assert!(
        listed
            .auto_disable_reason
            .as_deref()
            .unwrap()
            .contains("429")
    );
    assert!(
        repository
            .prepare_channel_disable(world.capability, &AutomaticDisableTrigger::HttpStatus(429))
            .await
            .unwrap()
            .is_none(),
        "a repeated trigger is idempotent"
    );

    let recovered = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::RecoverChannel {
            id: world.capability,
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(recovered.action, "manual_recover");
    assert!(recovered.reason.as_deref().unwrap().contains("manual"));
    let listed = listed_capability(&repositories, world.capability).await;
    assert!(!listed.auto_disabled);
    assert_eq!(listed.auto_disable_reason, None);
    let listed = listed_channel(&repositories, world.channel).await;

    // Live capabilities must be retired before their logical channel.
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteLogicalChannel {
                    id: world.channel,
                    expected: expected_etag(listed.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::RoutingDependencyInvalid)
    ));
    assert!(
        listed_channel(&repositories, world.channel)
            .await
            .deleted_at
            .is_none()
    );
    let capability = listed_capability(&repositories, world.capability).await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteChannelCapability {
            id: world.capability,
            expected: capability.updated_at,
        },
    )
    .await;
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteLogicalChannel {
            id: world.channel,
            expected: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert!(
        repository
            .topology()
            .await
            .unwrap()
            .logical_channels
            .iter()
            .all(|channel| channel.id != world.channel || channel.deleted_at.is_some())
    );
}

#[tokio::test]
async fn template_proxy_and_group_deletion_contract_matches_across_backends() {
    run_contract(template_proxy_and_group_deletion_contract).await;
}

async fn template_proxy_and_group_deletion_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    // Templates create, preserve on omission, and replace a present document.
    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateConfigTemplate(ConfigTemplateCreateInput {
            name: "Parity Template".into(),
            description: Some("Parity template".into()),
            document: json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-template": "on"}}
            }),
            enabled: true,
        }),
    )
    .await;
    assert_eq!(created.object_type, "config_template");
    assert_eq!(created.action, "create");
    let listed = listed_template(&repositories, created.id).await;
    assert_eq!(listed.name, "Parity Template");
    assert_eq!(listed.description.as_deref(), Some("Parity template"));
    assert_eq!(
        listed.api_format.as_deref(),
        Some("open_ai_chat_completions")
    );
    assert!(listed.enabled);
    assert_eq!(
        repository
            .control_plane_config_template_detail(created.id)
            .await
            .unwrap()
            .unwrap()
            .document["request_headers"]["set"]["x-template"],
        "on"
    );

    let update =
        |document: Option<Value>, expected_updated_at| ControlPlaneMutation::UpdateConfigTemplate {
            id: created.id,
            input: ConfigTemplateInput {
                name: "Parity Template v2".into(),
                description: None,
                document,
                enabled: false,
            },
            expected_updated_at,
        };
    let preserved = commit_mutation(
        &repositories,
        world.admin,
        update(None, expected_etag(listed.updated_at)),
    )
    .await;
    assert_eq!(preserved.action, "update");
    let detail = repository
        .control_plane_config_template_detail(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.name, "Parity Template v2");
    assert_eq!(detail.description, None);
    assert!(!detail.enabled);
    assert_eq!(
        detail.document["request_headers"]["set"]["x-template"],
        "on"
    );
    let replaced = commit_mutation(
        &repositories,
        world.admin,
        update(
            Some(json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-template": "off"}}
            })),
            expected_etag(detail.updated_at),
        ),
    )
    .await;
    assert_eq!(replaced.action, "update");
    assert_eq!(
        repository
            .control_plane_config_template_detail(created.id)
            .await
            .unwrap()
            .unwrap()
            .document["request_headers"]["set"]["x-template"],
        "off"
    );

    // Shared accesses retain proxy ownership until explicitly detached.
    let proxy = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(proxy_input("detach-me")),
    )
    .await
    .id;
    let attached_proxy = listed_proxy(&repositories, proxy).await;
    assert_eq!(attached_proxy.name, "detach-me");
    assert!(!attached_proxy.credential_configured);
    let access = listed_access(&repositories, world.access).await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateUpstreamAccess {
            id: world.access,
            input: ai_gateway::persistence::UpstreamAccessInput {
                proxy_id: Some(proxy),
                ..access_input(&access)
            },
            expected_updated_at: expected_etag(access.updated_at),
        },
    )
    .await;
    let in_use_at = attached_proxy.updated_at;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteProxy {
                    id: proxy,
                    expected_updated_at: expected_etag(in_use_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::ProxyInUse)
    ));
    let access = listed_access(&repositories, world.access).await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateUpstreamAccess {
            id: world.access,
            input: ai_gateway::persistence::UpstreamAccessInput {
                proxy_id: None,
                ..access_input(&access)
            },
            expected_updated_at: expected_etag(access.updated_at),
        },
    )
    .await;
    let detached_at = listed_proxy(&repositories, proxy).await.updated_at;
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteProxy {
            id: proxy,
            expected_updated_at: expected_etag(detached_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert!(
        repository
            .control_plane_lists()
            .await
            .unwrap()
            .proxies
            .is_empty()
    );
    assert!(repository.load().await.unwrap().proxies.is_empty());

    // Deletion cannot cascade across routes, capabilities, or fixed grants.
    let model = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("parity-model", "Parity Model")),
    )
    .await
    .id;
    let profile = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id: model }),
    )
    .await
    .id;
    let protocol = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveOperationRule {
            id: Uuid::new_v4(),
            expected_updated_at: None,
            input: ai_gateway::persistence::OperationRuleInput {
                model_routing_profile_id: profile,
                operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                routing_tiers: vec![ai_gateway::persistence::OperationTierInput {
                    priority: 0,
                    selection_strategy: "weighted_random".into(),
                    candidates: vec![ai_gateway::persistence::OperationCandidateInput {
                        capability_id: world.capability,
                        upstream_model: "parity-model".into(),
                        weight: 1,
                    }],
                }],
                enabled: true,
            },
        },
    )
    .await;
    let bound_key = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateApiKey(admin_api_key_create(
            world.user,
            world.group,
            "Group-bound key",
        )),
    )
    .await
    .id;

    let group = listed_group(&repositories, world.group).await;
    let capability = listed_capability(&repositories, world.capability).await;
    let channel = listed_channel(&repositories, world.channel).await;
    let before = repository.topology().await.unwrap();
    let observable = observable_state(&repositories).await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteRoutingGroup {
                    id: world.group,
                    expected: group.updated_at,
                }
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    for mutation in [
        ControlPlaneMutation::DeleteLogicalChannel {
            id: world.channel,
            expected: channel.updated_at,
        },
        ControlPlaneMutation::DeleteChannelCapability {
            id: world.capability,
            expected: capability.updated_at,
        },
    ] {
        assert!(matches!(
            repository
                .prepare_mutation(world.admin, mutation)
                .await
                .err(),
            Some(RepositoryError::RoutingDependencyInvalid)
        ));
    }
    assert_eq!(observable_state(&repositories).await, observable);
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveOperationRule {
            id: protocol.id,
            expected_updated_at: Some(protocol.updated_at),
            input: ai_gateway::persistence::OperationRuleInput {
                model_routing_profile_id: profile,
                operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                enabled: false,
                routing_tiers: vec![],
            },
        },
    )
    .await;
    for mutation in [
        ControlPlaneMutation::DeleteChannelCapability {
            id: world.capability,
            expected: capability.updated_at,
        },
        ControlPlaneMutation::DeleteLogicalChannel {
            id: world.channel,
            expected: channel.updated_at,
        },
        ControlPlaneMutation::DeleteRoutingGroup {
            id: world.group,
            expected: group.updated_at,
        },
    ] {
        assert_eq!(
            commit_mutation(&repositories, world.admin, mutation)
                .await
                .action,
            "delete"
        );
    }

    let lists = repository.control_plane_lists().await.unwrap();
    let topology = repository.topology().await.unwrap();
    assert_eq!(
        serde_json::to_value(&topology.api_key_grants).unwrap(),
        serde_json::to_value(&before.api_key_grants).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&topology.policy_grants).unwrap(),
        serde_json::to_value(&before.policy_grants).unwrap()
    );
    assert!(
        topology
            .upstream_accesses
            .iter()
            .any(|access| access.id == world.access && access.deleted_at.is_none())
    );
    assert!(
        topology
            .routing_groups
            .iter()
            .all(|group| group.id != world.group || group.deleted_at.is_some())
    );
    assert!(
        topology
            .logical_channels
            .iter()
            .all(|channel| channel.deleted_at.is_some())
    );
    assert!(repository.load().await.unwrap().groups.is_empty());
    let graph = operation_graph(&repositories, profile).await;
    assert!(!graph.rule.enabled);
    assert!(graph.tiers.is_empty());
    assert!(graph.candidates.is_empty());
    let key = lists
        .api_keys
        .iter()
        .find(|key| key.id == bound_key)
        .expect("the key retains its explicit authorization origin");
    assert_eq!(key.allowed_group_ids, vec![world.group]);
    let policy = lists
        .api_key_policies
        .iter()
        .find(|policy| policy.id == world.policy)
        .expect("the policy retains its explicit authorization origin");
    assert_eq!(policy.allowed_group_ids, vec![world.group]);
}

/// A comparable projection of everything an ordinary control-plane change can
/// mutate: the Console lists, the singleton settings, and the audit row count.
async fn observable_state(repositories: &Repositories) -> Value {
    let topology = repositories.control_plane.topology().await.unwrap();
    json!({
        "topology": {
            "groups": topology.routing_groups,
            "accesses": topology.upstream_accesses,
            "channels": topology.logical_channels,
            "capabilities": topology.channel_capabilities,
            "rules": topology.operation_rules,
            "tiers": topology.operation_tiers,
            "candidates": topology.operation_candidates,
            "key_grants": topology.api_key_grants,
            "policy_grants": topology.policy_grants,
        },
        "lists": serde_json::to_value(
            repositories
                .control_plane
                .control_plane_lists()
                .await
                .unwrap()
        )
        .unwrap(),
        "settings": serde_json::to_value(
            repositories
                .control_plane
                .system_settings()
                .await
                .unwrap()
        )
        .unwrap(),
        "audits": repositories
            .control_plane
            .audit_logs(100)
            .await
            .unwrap()
            .len(),
    })
}

/// Every ordinary mutation must reject an unauthorized actor and a stale
/// version before writing anything. The case builds one resource per family,
/// then replays each mutation with a non-administrator and with an outdated
/// version, asserting after each attempt that the observable store is identical.
#[tokio::test]
async fn authorization_and_etag_rollback_contract_matches_across_backends() {
    run_contract(authorization_and_etag_rollback_contract).await;
}

#[allow(clippy::too_many_lines, clippy::type_complexity)]
async fn authorization_and_etag_rollback_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    let user_group = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateUserGroup(user_group_input("Rollback Team")),
    )
    .await
    .id;
    let model = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("rollback-model", "Rollback Model")),
    )
    .await
    .id;
    let profile = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id: model }),
    )
    .await
    .id;
    let protocol = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveOperationRule {
            id: Uuid::new_v4(),
            expected_updated_at: None,
            input: ai_gateway::persistence::OperationRuleInput {
                model_routing_profile_id: profile,
                operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                enabled: false,
                routing_tiers: vec![],
            },
        },
    )
    .await
    .id;
    let key = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateApiKey(admin_api_key_create(
            world.user,
            world.group,
            "Rollback key",
        )),
    )
    .await
    .id;
    let proxy = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(proxy_input("rollback-proxy")),
    )
    .await
    .id;
    let template = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateConfigTemplate(ConfigTemplateCreateInput {
            name: "Rollback template".into(),
            description: None,
            document: json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-rollback": "on"}}
            }),
            enabled: true,
        }),
    )
    .await
    .id;

    let user_view = listed_user(&repositories, world.user).await;
    let group_view = listed_group(&repositories, world.group).await;
    let channel_view = listed_channel(&repositories, world.channel).await;
    let capability_view = listed_capability(&repositories, world.capability).await;
    let access_view = listed_access(&repositories, world.access).await;
    let settings_view = repository.system_settings().await.unwrap();
    let stale = group_view.updated_at - ChronoDuration::seconds(1);

    // Creations carry no version guard but still require an administrator.
    let creations: Vec<(&str, Box<dyn Fn() -> ControlPlaneMutation>)> = vec![
        (
            "CreateUser",
            Box::new(|| ControlPlaneMutation::CreateUser(user_create_input("Rollback User"))),
        ),
        (
            "CreateUserGroup",
            Box::new(|| ControlPlaneMutation::CreateUserGroup(user_group_input("Rollback Team 2"))),
        ),
        (
            "CreateModel",
            Box::new(|| {
                ControlPlaneMutation::CreateModel(model_input(
                    "rollback-model-2",
                    "Rollback Model 2",
                ))
            }),
        ),
        (
            "CreateApiKey",
            Box::new(|| {
                ControlPlaneMutation::CreateApiKey(admin_api_key_create(
                    world.user,
                    world.group,
                    "Rollback key 2",
                ))
            }),
        ),
        (
            "CreateRoutingGroup",
            Box::new(|| ControlPlaneMutation::SaveRoutingGroup {
                id: Uuid::new_v4(),
                expected: None,
                input: ai_gateway::persistence::RoutingGroupInput {
                    name: "Rollback Group 2".into(),
                    sharing_only: false,
                    enabled: true,
                },
            }),
        ),
        (
            "CreateLogicalChannel",
            Box::new(|| ControlPlaneMutation::SaveLogicalChannel {
                id: Uuid::new_v4(),
                expected: None,
                input: ai_gateway::persistence::LogicalChannelInput {
                    group_id: world.group,
                    access_id: world.access,
                    name: "Rollback Channel 2".into(),
                    enabled: true,
                    credential_id: None,
                },
            }),
        ),
        (
            "CreateUpstreamAccess",
            Box::new(|| ControlPlaneMutation::CreateUpstreamAccess(access_input(&access_view))),
        ),
        (
            "CreateCapability",
            Box::new(|| ControlPlaneMutation::SaveChannelCapability {
                id: Uuid::new_v4(),
                expected: None,
                input: capability_input(&capability_view),
            }),
        ),
        (
            "CreateOperationRule",
            Box::new(|| ControlPlaneMutation::SaveOperationRule {
                id: Uuid::new_v4(),
                expected_updated_at: None,
                input: ai_gateway::persistence::OperationRuleInput {
                    model_routing_profile_id: profile,
                    operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                    enabled: false,
                    routing_tiers: vec![],
                },
            }),
        ),
        (
            "CreateProxy",
            Box::new(|| ControlPlaneMutation::CreateProxy(proxy_input("rollback-proxy-2"))),
        ),
        (
            "CreateConfigTemplate",
            Box::new(|| {
                ControlPlaneMutation::CreateConfigTemplate(ConfigTemplateCreateInput {
                    name: "Rollback template 2".into(),
                    description: None,
                    document: json!({"version": 1, "request_headers": {"set": {"x": "y"}}}),
                    enabled: true,
                })
            }),
        ),
        (
            "CreateRule",
            Box::new(|| ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id: model })),
        ),
        (
            "RevokeApiKey",
            Box::new(|| ControlPlaneMutation::RevokeApiKey {
                id: key,
                reason: "unauthorized attempt".into(),
            }),
        ),
    ];
    for (label, build) in &creations {
        let before = observable_state(&repositories).await;
        assert!(
            matches!(
                repository.prepare_mutation(world.user, build()).await.err(),
                Some(RepositoryError::InvalidActor)
            ),
            "{label} must reject a non-administrator"
        );
        assert_eq!(
            observable_state(&repositories).await,
            before,
            "{label} must not write for an unauthorized actor"
        );
    }

    // Versioned mutations require both an administrator and the current version.
    let versioned: Vec<(&str, Box<dyn Fn(DateTime<Utc>) -> ControlPlaneMutation>)> = vec![
        (
            "UpdateUserGroup",
            Box::new(move |version| ControlPlaneMutation::UpdateUserGroup {
                id: user_group,
                input: user_group_input("Rollback Team Renamed"),
                expected_updated_at: version,
            }),
        ),
        (
            "DeleteUserGroup",
            Box::new(move |version| ControlPlaneMutation::DeleteUserGroup {
                id: user_group,
                deleted_by: world.admin,
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateUser",
            Box::new(move |version| ControlPlaneMutation::UpdateUser {
                id: world.user,
                input: UserUpdateInput {
                    display_name: Some("Rollback Renamed".into()),
                    email: None,
                    role: None,
                    status: None,
                    balance_amount: None,
                    user_group_id: None,
                    default_api_key_policy_id: None,
                    websocket_enabled: None,
                },
                expected_updated_at: version,
            }),
        ),
        (
            "DeleteUser",
            Box::new(move |version| ControlPlaneMutation::DeleteUser {
                id: world.user,
                deleted_by: world.admin,
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateModel",
            Box::new(move |version| ControlPlaneMutation::UpdateModel {
                id: model,
                input: model_input("rollback-model", "Rollback Model Renamed"),
                expected_updated_at: version,
            }),
        ),
        (
            "DeleteModel",
            Box::new(move |version| ControlPlaneMutation::DeleteModel {
                id: model,
                deleted_by: world.admin,
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateApiKeyPolicy",
            Box::new(move |version| ControlPlaneMutation::UpdateApiKeyPolicy {
                id: world.policy,
                input: ApiKeyPolicyInput {
                    name: "Rollback Policy Renamed".into(),
                    allowed_group_ids: vec![world.group],
                    allowed_channel_ids: Vec::new(),
                    enabled: true,
                },
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateApiKey",
            Box::new(move |version| ControlPlaneMutation::UpdateApiKey {
                id: key,
                input: ApiKeyUpdate {
                    name: "Rollback key renamed".into(),
                    status: "disabled".into(),
                    allowed_api_formats: vec!["open_ai_chat_completions".into()],
                    permissions: vec!["proxy".into()],
                    allowed_group_ids: vec![world.group],
                    allowed_channel_ids: Vec::new(),
                    expires_at: None,
                    requests_per_minute: None,
                    max_concurrent_requests: None,
                    quota_limit_amount: None,
                },
                expected_updated_at: version,
            }),
        ),
        (
            "DeleteApiKey",
            Box::new(move |version| ControlPlaneMutation::DeleteApiKey {
                id: key,
                deleted_by: world.admin,
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateRoutingGroup",
            Box::new(move |version| ControlPlaneMutation::SaveRoutingGroup {
                id: world.group,
                input: ai_gateway::persistence::RoutingGroupInput {
                    name: "Rollback Group Renamed".into(),
                    sharing_only: false,
                    enabled: true,
                },
                expected: Some(version),
            }),
        ),
        (
            "DeleteRoutingGroup",
            Box::new(move |version| ControlPlaneMutation::DeleteRoutingGroup {
                id: world.group,
                expected: version,
            }),
        ),
        (
            "UpdateLogicalChannel",
            Box::new(move |version| ControlPlaneMutation::SaveLogicalChannel {
                id: world.channel,
                input: logical_input(&channel_view),
                expected: Some(version),
            }),
        ),
        (
            "DeleteLogicalChannel",
            Box::new(move |version| ControlPlaneMutation::DeleteLogicalChannel {
                id: world.channel,
                expected: version,
            }),
        ),
        (
            "RecoverChannel",
            Box::new(move |version| ControlPlaneMutation::RecoverChannel {
                id: world.capability,
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateOperationRule",
            Box::new(move |version| ControlPlaneMutation::SaveOperationRule {
                id: protocol,
                input: ai_gateway::persistence::OperationRuleInput {
                    model_routing_profile_id: profile,
                    operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                    routing_tiers: vec![ai_gateway::persistence::OperationTierInput {
                        priority: 0,
                        selection_strategy: "weighted_random".into(),
                        candidates: vec![ai_gateway::persistence::OperationCandidateInput {
                            capability_id: world.capability,
                            upstream_model: "parity-model".into(),
                            weight: 1,
                        }],
                    }],
                    enabled: true,
                },
                expected_updated_at: Some(version),
            }),
        ),
        (
            "UpdateUpstreamAccess",
            Box::new(|version| ControlPlaneMutation::UpdateUpstreamAccess {
                id: world.access,
                input: access_input(&access_view),
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateCapability",
            Box::new(|version| ControlPlaneMutation::SaveChannelCapability {
                id: world.capability,
                input: capability_input(&capability_view),
                expected: Some(version),
            }),
        ),
        (
            "DeleteCapability",
            Box::new(|version| ControlPlaneMutation::DeleteChannelCapability {
                id: world.capability,
                expected: version,
            }),
        ),
        (
            "UpdateProxy",
            Box::new(move |version| ControlPlaneMutation::UpdateProxy {
                id: proxy,
                input: ProxyInput {
                    name: "rollback-proxy-renamed".into(),
                    proxy_url: "http://proxy.example.test:8080".into(),
                    username: None,
                    password: None,
                    no_proxy_hosts: Vec::new(),
                    enabled: true,
                },
                expected_updated_at: version,
            }),
        ),
        (
            "DeleteProxy",
            Box::new(move |version| ControlPlaneMutation::DeleteProxy {
                id: proxy,
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateConfigTemplate",
            Box::new(move |version| ControlPlaneMutation::UpdateConfigTemplate {
                id: template,
                input: ConfigTemplateInput {
                    name: "Rollback template renamed".into(),
                    description: None,
                    document: None,
                    enabled: true,
                },
                expected_updated_at: version,
            }),
        ),
        (
            "UpdateSystemSettings",
            Box::new({
                let input = settings_view.settings.clone();
                move |version| ControlPlaneMutation::UpdateSystemSettings {
                    input: input.clone(),
                    expected_updated_at: version,
                }
            }),
        ),
    ];
    for (label, build) in &versioned {
        let before = observable_state(&repositories).await;
        assert!(
            matches!(
                repository
                    .prepare_mutation(world.user, build(stale))
                    .await
                    .err(),
                Some(RepositoryError::InvalidActor)
            ),
            "{label} must reject a non-administrator"
        );
        assert_eq!(
            observable_state(&repositories).await,
            before,
            "{label} must not write for an unauthorized actor"
        );
        assert!(
            matches!(
                repository
                    .prepare_mutation(world.admin, build(stale))
                    .await
                    .err(),
                Some(RepositoryError::Conflict)
            ),
            "{label} must reject a stale version"
        );
        assert_eq!(
            observable_state(&repositories).await,
            before,
            "{label} must roll back a stale version without writing"
        );
    }

    // The batch admission paths enforce the same actor and version boundaries.
    let before = observable_state(&repositories).await;
    assert!(matches!(
        repository
            .prepare_channels_batch(
                world.user,
                ChannelBatchUpdateInput {
                    items: vec![ChannelBatchUpdateTarget {
                        id: world.capability,
                        updated_at: expected_etag(
                            listed_capability(&repositories, world.capability)
                                .await
                                .updated_at
                        ),
                    }],
                    changes: ChannelBatchChanges {
                        enabled: Some(false),
                        auto_disable_allowed: None,
                        billing_multiplier: None,
                    },
                },
            )
            .await
            .err(),
        Some(RepositoryError::InvalidActor)
    ));
    assert!(matches!(
        repository
            .prepare_users_batch(
                world.user,
                UserBatchUpdateInput {
                    items: vec![UserBatchUpdateTarget {
                        id: world.user,
                        updated_at: expected_etag(user_view.updated_at),
                    }],
                    changes: UserBatchChanges {
                        status: Some("suspended".into()),
                        balance: None,
                        user_group_id: None,
                        default_api_key_policy_id: None,
                    },
                },
            )
            .await
            .err(),
        Some(RepositoryError::InvalidActor)
    ));
    assert!(matches!(
        repository
            .prepare_channels_batch(
                world.admin,
                ChannelBatchUpdateInput {
                    items: vec![ChannelBatchUpdateTarget {
                        id: world.capability,
                        updated_at: stale,
                    }],
                    changes: ChannelBatchChanges {
                        enabled: Some(false),
                        auto_disable_allowed: None,
                        billing_multiplier: None,
                    },
                },
            )
            .await
            .err(),
        Some(RepositoryError::Conflict)
    ));
    assert_eq!(observable_state(&repositories).await, before);

    // Group deletion never cascades across newly added logical channels.
    let extra_channel = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::LogicalChannelInput {
                group_id: world.group,
                access_id: world.access,
                name: "Rollback Extra Channel".into(),
                enabled: true,
                credential_id: None,
            },
        },
    )
    .await;
    let refreshed_group = listed_group(&repositories, world.group).await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteRoutingGroup {
                    id: world.group,
                    expected: expected_etag(refreshed_group.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(
        listed_channel(&repositories, extra_channel.id)
            .await
            .deleted_at
            .is_none(),
        "the refused deletion left the new channel intact"
    );

    // Input validation fails before any write on both backends.
    let before = observable_state(&repositories).await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::UpdateModel {
                    id: model,
                    input: ModelInput {
                        advanced_billing: Some(AdvancedBilling {
                            long_context_tiers: vec![LongContextTier {
                                input_tokens_threshold: 0,
                                input_unit_price: Decimal::ZERO,
                                cached_input_unit_price: Decimal::ZERO,
                                cache_write_unit_price: Decimal::ZERO,
                                output_unit_price: None,
                            }],
                            ..AdvancedBilling::default()
                        }),
                        ..model_input("rollback-model", "Rollback Model")
                    },
                    expected_updated_at: expected_etag(
                        listed_model(&repositories, model).await.updated_at
                    ),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::RevokeApiKey {
                    id: key,
                    reason: "   ".into(),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::UpdateUser {
                    id: world.user,
                    input: UserUpdateInput {
                        role: Some("owner".into()),
                        display_name: None,
                        email: None,
                        status: None,
                        balance_amount: None,
                        user_group_id: None,
                        default_api_key_policy_id: None,
                        websocket_enabled: None,
                    },
                    expected_updated_at: expected_etag(user_view.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert_eq!(
        observable_state(&repositories).await,
        before,
        "invalid input must not write on either backend"
    );

    // The originally seeded resources are untouched by every rolled-back attempt.
    assert_eq!(
        serde_json::to_value(listed_user(&repositories, world.user).await).unwrap(),
        serde_json::to_value(user_view).unwrap()
    );
    repository.verify_active_admin(world.admin).await.unwrap();
}

#[tokio::test]
async fn control_plane_projection_contract_matches_across_backends() {
    run_contract(control_plane_projection_contract).await;
}

async fn control_plane_projection_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    // Group metadata does not rewrite capability configuration.
    let before = listed_group(&repositories, world.group).await;
    let capability_before = listed_capability(&repositories, world.capability).await;
    let updated = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: world.group,
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Group v2".into(),
                sharing_only: before.sharing_only,
                enabled: false,
            },
            expected: Some(expected_etag(before.updated_at)),
        },
    )
    .await;
    assert_eq!(updated.action, "update");
    let after = listed_group(&repositories, world.group).await;
    assert_eq!(after.name, "Group v2");
    assert!(!after.enabled);
    assert_eq!(
        serde_json::to_value(listed_capability(&repositories, world.capability).await).unwrap(),
        serde_json::to_value(capability_before).unwrap()
    );
    assert!(updated.after_redacted.get("request_compression").is_none());
    // restore
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: world.group,
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Parity Group".into(),
                sharing_only: after.sharing_only,
                enabled: true,
            },
            expected: Some(expected_etag(after.updated_at)),
        },
    )
    .await;

    // B. advanced_billing read-back equality.
    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("probe-model", "Probe Model")),
    )
    .await;
    let listed = listed_model(&repositories, created.id).await;
    assert_eq!(
        serde_json::to_value(&listed.advanced_billing).unwrap(),
        serde_json::to_value(advanced_billing()).unwrap()
    );

    // C. system settings full roundtrip.
    let settings = repository.system_settings().await.unwrap();
    let mut next = settings.settings.clone();
    next.api_hosts = vec![
        "https://a.example.test".into(),
        "https://b.example.test".into(),
    ];
    next.session_affinity.max_entries = 4242;
    let changed = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateSystemSettings {
            input: next.clone(),
            expected_updated_at: settings.updated_at,
        },
    )
    .await;
    assert_eq!(changed.action, "update");
    let read_back = repository.system_settings().await.unwrap();
    assert_eq!(
        serde_json::to_value(&read_back.settings).unwrap(),
        serde_json::to_value(&next).unwrap()
    );
    assert_eq!(read_back.settings.api_hosts.len(), 2);

    // D. audit ordering: newest first.
    let audits = repository.audit_logs(100).await.unwrap();
    let mut sorted = audits.clone();
    sorted.sort_by(|left, right| {
        right
            .occurred_at
            .cmp(&left.occurred_at)
            .then(right.id.cmp(&left.id))
    });
    assert_eq!(
        audits.iter().map(|a| a.id).collect::<Vec<_>>(),
        sorted.iter().map(|a| a.id).collect::<Vec<_>>()
    );

    // E. control-plane lists ordering with multiple resources.
    for (id, name) in [
        ("probe-model-b", "Probe Model B"),
        ("probe-model-c", "Probe Model C"),
    ] {
        commit_mutation(
            &repositories,
            world.admin,
            ControlPlaneMutation::CreateModel(model_input(id, name)),
        )
        .await;
    }
    let lists = repository.control_plane_lists().await.unwrap();
    let model_ids = lists
        .models
        .iter()
        .map(|model| model.id)
        .collect::<Vec<_>>();
    let mut sorted_ids = model_ids.clone();
    sorted_ids.sort_unstable();
    assert_eq!(model_ids, sorted_ids, "models are ordered by id");
    let group_ids = repository
        .topology()
        .await
        .unwrap()
        .routing_groups
        .iter()
        .map(|group| group.id)
        .collect::<Vec<_>>();
    let mut sorted_groups = group_ids.clone();
    sorted_groups.sort_unstable();
    assert_eq!(group_ids, sorted_groups, "channel groups are ordered by id");
    let user_group_names = lists
        .user_groups
        .iter()
        .map(|group| group.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        user_group_names.first().map(String::as_str),
        Some("Default Administrators")
    );
    assert_eq!(
        user_group_names.get(1).map(String::as_str),
        Some("Default Users")
    );
}

#[tokio::test]
async fn missing_and_soft_deleted_resource_contract_matches_across_backends() {
    run_contract(missing_and_soft_deleted_resource_contract).await;
}

async fn missing_and_soft_deleted_resource_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    // 1. Missing resources must be NotFound (not Conflict) and never write.
    let missing = Uuid::from_u128(0x7e57);
    let before = observable_state(&repositories).await;
    let channel_view = listed_channel(&repositories, world.channel).await;
    let capability_view = listed_capability(&repositories, world.capability).await;
    let access_view = listed_access(&repositories, world.access).await;
    for (label, mutation) in [
        (
            "UpdateUser",
            ControlPlaneMutation::UpdateUser {
                id: missing,
                input: UserUpdateInput {
                    display_name: Some("x".into()),
                    email: None,
                    role: None,
                    status: None,
                    balance_amount: None,
                    user_group_id: None,
                    default_api_key_policy_id: None,
                    websocket_enabled: None,
                },
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "DeleteUser",
            ControlPlaneMutation::DeleteUser {
                id: missing,
                deleted_by: world.admin,
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateUserGroup",
            ControlPlaneMutation::UpdateUserGroup {
                id: missing,
                input: user_group_input("x"),
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "DeleteUserGroup",
            ControlPlaneMutation::DeleteUserGroup {
                id: missing,
                deleted_by: world.admin,
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateModel",
            ControlPlaneMutation::UpdateModel {
                id: missing,
                input: model_input("x", "x"),
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "DeleteModel",
            ControlPlaneMutation::DeleteModel {
                id: missing,
                deleted_by: world.admin,
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateApiKey",
            ControlPlaneMutation::UpdateApiKey {
                id: missing,
                input: ApiKeyUpdate {
                    name: "x".into(),
                    status: "active".into(),
                    allowed_api_formats: vec!["open_ai_chat_completions".into()],
                    permissions: vec!["proxy".into()],
                    allowed_group_ids: vec![world.group],
                    allowed_channel_ids: Vec::new(),
                    expires_at: None,
                    requests_per_minute: None,
                    max_concurrent_requests: None,
                    quota_limit_amount: None,
                },
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "DeleteApiKey",
            ControlPlaneMutation::DeleteApiKey {
                id: missing,
                deleted_by: world.admin,
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateApiKeyPolicy",
            ControlPlaneMutation::UpdateApiKeyPolicy {
                id: missing,
                input: ApiKeyPolicyInput {
                    name: "x".into(),
                    allowed_group_ids: vec![world.group],
                    allowed_channel_ids: Vec::new(),
                    enabled: true,
                },
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateRoutingGroup",
            ControlPlaneMutation::SaveRoutingGroup {
                id: missing,
                input: ai_gateway::persistence::RoutingGroupInput {
                    name: "x".into(),
                    sharing_only: false,
                    enabled: true,
                },
                expected: Some(Utc::now()),
            },
        ),
        (
            "DeleteRoutingGroup",
            ControlPlaneMutation::DeleteRoutingGroup {
                id: missing,
                expected: Utc::now(),
            },
        ),
        (
            "UpdateLogicalChannel",
            ControlPlaneMutation::SaveLogicalChannel {
                id: missing,
                input: logical_input(&channel_view),
                expected: Some(Utc::now()),
            },
        ),
        (
            "DeleteLogicalChannel",
            ControlPlaneMutation::DeleteLogicalChannel {
                id: missing,
                expected: Utc::now(),
            },
        ),
        (
            "RecoverChannel",
            ControlPlaneMutation::RecoverChannel {
                id: missing,
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateOperationRule",
            ControlPlaneMutation::SaveOperationRule {
                id: missing,
                input: ai_gateway::persistence::OperationRuleInput {
                    model_routing_profile_id: missing,
                    operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                    routing_tiers: Vec::new(),
                    enabled: false,
                },
                expected_updated_at: Some(Utc::now()),
            },
        ),
        (
            "UpdateProxy",
            ControlPlaneMutation::UpdateProxy {
                id: missing,
                input: ProxyInput {
                    name: "x".into(),
                    proxy_url: "http://proxy.example.test:8080".into(),
                    username: None,
                    password: None,
                    no_proxy_hosts: Vec::new(),
                    enabled: true,
                },
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "DeleteProxy",
            ControlPlaneMutation::DeleteProxy {
                id: missing,
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateConfigTemplate",
            ControlPlaneMutation::UpdateConfigTemplate {
                id: missing,
                input: ConfigTemplateInput {
                    name: "x".into(),
                    description: None,
                    document: None,
                    enabled: true,
                },
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "CreateOperationRule",
            ControlPlaneMutation::SaveOperationRule {
                id: Uuid::new_v4(),
                expected_updated_at: None,
                input: ai_gateway::persistence::OperationRuleInput {
                    model_routing_profile_id: missing,
                    operation: ai_gateway::domain::ApiOperation::ChatCompletions,
                    enabled: false,
                    routing_tiers: vec![],
                },
            },
        ),
        (
            "CreateRule",
            ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id: missing }),
        ),
        (
            "UpdateUpstreamAccess",
            ControlPlaneMutation::UpdateUpstreamAccess {
                id: missing,
                input: access_input(&access_view),
                expected_updated_at: Utc::now(),
            },
        ),
        (
            "UpdateCapability",
            ControlPlaneMutation::SaveChannelCapability {
                id: missing,
                input: capability_input(&capability_view),
                expected: Some(Utc::now()),
            },
        ),
        (
            "DeleteCapability",
            ControlPlaneMutation::DeleteChannelCapability {
                id: missing,
                expected: Utc::now(),
            },
        ),
    ] {
        let error = repository
            .prepare_mutation(world.admin, mutation)
            .await
            .err();
        if label == "CreateOperationRule" {
            assert!(
                matches!(error, Some(RepositoryError::Validation)),
                "a missing profile is an invalid operation-rule reference, got {error:?}"
            );
        } else {
            assert!(
                matches!(error, Some(RepositoryError::NotFound)),
                "{label} on a missing row must be NotFound, got {error:?}"
            );
        }
        assert_eq!(
            observable_state(&repositories).await,
            before,
            "{label} must not write"
        );
    }

    // 2. Soft-deleted resources are NotFound for later writes.
    let model = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("probe-deleted", "Probe Deleted")),
    )
    .await
    .id;
    let listed = listed_model(&repositories, model).await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteModel {
            id: model,
            deleted_by: world.admin,
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteModel {
                    id: model,
                    deleted_by: world.admin,
                    expected_updated_at: expected_etag(listed.updated_at),
                },
            )
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));
    // The source_model_id becomes reusable after soft deletion.
    let recreated = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("probe-deleted", "Probe Deleted Again")),
    )
    .await;
    assert_ne!(recreated.id, model);

    // A tombstoned capability cannot be recovered, deleted, or re-created.
    let channel = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::LogicalChannelInput {
                group_id: world.group,
                access_id: world.access,
                name: "Probe Channel".into(),
                enabled: true,
                credential_id: None,
            },
        },
    )
    .await
    .id;
    let capability = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveChannelCapability {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::ChannelCapabilityInput {
                channel_id: channel,
                ..capability_input(&capability_view)
            },
        },
    )
    .await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteChannelCapability {
            id: capability.id,
            expected: capability.updated_at,
        },
    )
    .await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::RecoverChannel {
                    id: capability.id,
                    expected_updated_at: Utc::now(),
                },
            )
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));
    for mutation in [
        ControlPlaneMutation::DeleteChannelCapability {
            id: capability.id,
            expected: capability.updated_at,
        },
        ControlPlaneMutation::SaveChannelCapability {
            id: capability.id,
            expected: Some(capability.updated_at),
            input: ai_gateway::persistence::ChannelCapabilityInput {
                channel_id: channel,
                ..capability_input(&capability_view)
            },
        },
    ] {
        assert!(matches!(
            repository
                .prepare_mutation(world.admin, mutation)
                .await
                .err(),
            Some(RepositoryError::NotFound)
        ));
    }
    let listed = listed_channel(&repositories, channel).await;
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteLogicalChannel {
            id: channel,
            expected: listed.updated_at,
        },
    )
    .await;
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteLogicalChannel {
                    id: channel,
                    expected: listed.updated_at,
                }
            )
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));

    // 4. Proxy credential clearing round trips, then a delete is NotFound.
    let proxy = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(ProxyCreateInput {
            name: "probe-cred".into(),
            proxy_url: "http://user:secret@proxy.example.test:8080".into(),
            username: Some("user".into()),
            password: Some("secret".into()),
            no_proxy_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            enabled: true,
        }),
    )
    .await
    .id;
    let record = repository
        .load()
        .await
        .unwrap()
        .proxies
        .into_iter()
        .find(|record| record.id == proxy)
        .unwrap();
    assert_eq!(record.username.as_deref(), Some("user"));
    assert_eq!(record.password.as_deref(), Some("secret"));
    assert_eq!(record.no_proxy_hosts, vec!["localhost", "127.0.0.1"]);
    let listed = listed_proxy(&repositories, proxy).await;
    assert!(listed.credential_configured);
    let cleared = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateProxy {
            id: proxy,
            input: ProxyInput {
                name: "probe-cred".into(),
                proxy_url: "http://proxy.example.test:8080".into(),
                username: Some(None),
                password: Some(None),
                no_proxy_hosts: Vec::new(),
                enabled: false,
            },
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(cleared.action, "update");
    assert_eq!(cleared.after_redacted["credential_configured"], false);
    let record = repository
        .load()
        .await
        .unwrap()
        .proxies
        .into_iter()
        .find(|record| record.id == proxy)
        .unwrap();
    assert_eq!(record.username, None);
    assert_eq!(record.password, None);
    assert!(record.no_proxy_hosts.is_empty());
    assert!(!record.enabled);
    let listed = listed_proxy(&repositories, proxy).await;
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteProxy {
            id: proxy,
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(deleted.action, "delete");
    assert!(matches!(
        repository
            .prepare_mutation(
                world.admin,
                ControlPlaneMutation::DeleteProxy {
                    id: proxy,
                    expected_updated_at: Utc::now(),
                },
            )
            .await
            .err(),
        Some(RepositoryError::NotFound)
    ));

    // 5. Model deletion clears channel scheduled-test references and reports it.
    let priced = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("probe-priced", "Probe Priced")),
    )
    .await
    .id;
    let probe_channel = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::LogicalChannelInput {
                group_id: world.group,
                access_id: world.access,
                credential_id: None,
                name: "Probe Auto Channel".into(),
                enabled: true,
            },
        },
    )
    .await;
    let auto_capability = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveChannelCapability {
            id: Uuid::new_v4(), expected: None,
            input: serde_json::from_value(json!({
                "channel_id": probe_channel.id,
                "settings": {
                    "operation": "chat_completion", "enabled": true,
                    "available_models": ["probe-priced"], "request_compression": "default",
                    "test_model": "probe-priced", "test_pricing_model_id": priced, "auto_disable_allowed": false
                },
                "billing_multiplier": "1", "status_statistics_enabled": false,
                "config_template_id": null, "override_document": {}
            })).unwrap(),
        },
    )
    .await;
    let listed = listed_capability(&repositories, auto_capability.id).await;
    assert_eq!(listed.settings.test_model.as_deref(), Some("probe-priced"));
    assert_eq!(listed.settings.test_pricing_model_id, Some(priced));
    let priced_view = listed_model(&repositories, priced).await;
    let deleted = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::DeleteModel {
            id: priced,
            deleted_by: world.admin,
            expected_updated_at: expected_etag(priced_view.updated_at),
        },
    )
    .await;
    assert_eq!(
        deleted.reason.as_deref(),
        Some("0 operation rules disabled; 1 scheduled test references cleared")
    );
    let listed = listed_capability(&repositories, auto_capability.id).await;
    assert_eq!(listed.settings.test_model, None);
    assert_eq!(listed.settings.test_pricing_model_id, None);

    // 6. Batch guard rails: empty, duplicate, stale are rejected identically.
    assert!(matches!(
        repository
            .prepare_channels_batch(
                world.admin,
                ChannelBatchUpdateInput {
                    items: Vec::new(),
                    changes: ChannelBatchChanges {
                        enabled: Some(true),
                        auto_disable_allowed: None,
                        billing_multiplier: None,
                    },
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
    assert!(matches!(
        repository
            .prepare_channels_batch(
                world.admin,
                ChannelBatchUpdateInput {
                    items: Vec::new(),
                    changes: ChannelBatchChanges {
                        enabled: None,
                        auto_disable_allowed: None,
                        billing_multiplier: None,
                    },
                },
            )
            .await
            .err(),
        Some(RepositoryError::Validation)
    ));
}

#[tokio::test]
async fn capability_validation_contract_matches_across_backends() {
    run_contract(capability_validation_contract).await;
}

async fn capability_validation_contract(repositories: Repositories) {
    let world = world(&repositories).await;
    let repository = &repositories.control_plane;

    let before = repository.topology().await.unwrap();
    let group = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Probe group".into(),
                sharing_only: false,
                enabled: true,
            },
        },
    )
    .await;
    assert_eq!(
        repository
            .topology()
            .await
            .unwrap()
            .channel_capabilities
            .len(),
        before.channel_capabilities.len()
    );
    let channel = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveLogicalChannel {
            id: Uuid::new_v4(),
            expected: None,
            input: ai_gateway::persistence::LogicalChannelInput {
                group_id: group.id,
                access_id: world.access,
                credential_id: None,
                name: "Probe channel".into(),
                enabled: true,
            },
        },
    )
    .await;
    let response_input = json!({
        "channel_id": channel.id,
        "settings": {
            "operation": "responses", "enabled": true,
            "available_models": ["probe"], "request_compression": "zstd",
            "test_model": null, "test_pricing_model_id": null, "auto_disable_allowed": false
        },
        "status_statistics_enabled": true, "config_template_id": null,
        "override_document": {}, "billing_multiplier": "1"
    });
    let response = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveChannelCapability {
            id: Uuid::new_v4(),
            expected: None,
            input: serde_json::from_value(response_input.clone()).unwrap(),
        },
    )
    .await;
    let priced = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateModel(model_input("probe-priced", "Probe Priced")),
    )
    .await
    .id;
    let mut chat = response_input;
    chat["settings"]["operation"] = json!("chat_completion");
    chat["settings"]["request_compression"] = json!("default");
    for fault in ["compression", "probe_catalogue", "negative_multiplier"] {
        let mut invalid = chat.clone();
        if fault == "compression" {
            invalid["settings"]["request_compression"] = json!("zstd");
        }
        if fault == "probe_catalogue" {
            invalid["settings"]["test_model"] = json!("probe-priced");
            invalid["settings"]["test_pricing_model_id"] = json!(priced);
        }
        if fault == "negative_multiplier" {
            invalid["billing_multiplier"] = json!("-1");
        }
        assert!(
            matches!(
                repository
                    .prepare_mutation(
                        world.admin,
                        ControlPlaneMutation::SaveChannelCapability {
                            id: Uuid::new_v4(),
                            expected: None,
                            input: serde_json::from_value(invalid).unwrap(),
                        }
                    )
                    .await
                    .err(),
                Some(RepositoryError::Validation)
            ),
            "{fault}"
        );
    }
    commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::SaveRoutingGroup {
            id: group.id,
            expected: Some(group.updated_at),
            input: ai_gateway::persistence::RoutingGroupInput {
                name: "Renamed probe group".into(),
                enabled: false,
                sharing_only: false,
            },
        },
    )
    .await;
    let capability = listed_capability(&repositories, response.id).await;
    assert_eq!(
        capability.settings.request_compression,
        ai_gateway::domain::RequestCompression::Zstd
    );
    assert!(capability.status_statistics_enabled);
    let after = repository.topology().await.unwrap();
    assert_eq!(
        after.channel_capabilities.len(),
        before.channel_capabilities.len() + 1
    );
    assert_eq!(
        serde_json::to_value(after.policy_grants).unwrap(),
        serde_json::to_value(before.policy_grants).unwrap()
    );
}

/// The Console proxy projections must strip an embedded `user:password@`
/// credential component without otherwise altering the URL. Both the list and
/// the audit trail consume this projection, so a backend that substitutes an
/// escape byte silently corrupts the operator-visible URL and the audit record.
///
/// PostgreSQL currently fails this: `postgres_control_plane.rs` passes the
/// escape-string literal `E'\1'` to `regexp_replace`, which PostgreSQL decodes
/// as the single byte 0x01 (an invalid octal escape for a backreference),
/// replacing the scheme instead of capturing it. The fix is to double the
/// backslash in the Rust source (`E'\\1'`) or drop the escape string in favor of
/// the standard-conforming `'\1'` in both projections (list at the
/// `ControlPlaneProxy` query and the `proxy_audit` query).
#[tokio::test]
async fn proxy_credential_stripping_contract_matches_across_backends() {
    run_contract(proxy_credential_stripping_contract).await;
}

async fn proxy_credential_stripping_contract(repositories: Repositories) {
    let world = world(&repositories).await;

    let created = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(ProxyCreateInput {
            name: "credentialed".into(),
            proxy_url: "http://user:secret@proxy.example.test:8080".into(),
            username: Some("user".into()),
            password: Some("secret".into()),
            no_proxy_hosts: vec!["localhost".into()],
            enabled: true,
        }),
    )
    .await;
    let listed = listed_proxy(&repositories, created.id).await;
    assert!(listed.credential_configured);
    assert_eq!(
        listed.proxy_url, "http://proxy.example.test:8080",
        "the Console list projection strips the credential component"
    );
    let create_audit = audit_for(&repositories, "create", created.id).await;
    assert_eq!(
        create_audit.after_redacted.as_ref().unwrap()["proxy_url"],
        json!("http://proxy.example.test:8080"),
        "the audit projection strips the credential component"
    );
    assert_eq!(listed.no_proxy_hosts, vec!["localhost"]);
    assert!(listed.enabled);

    // A query string and fragment are also stripped from the exposed URL.
    let query = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::CreateProxy(ProxyCreateInput {
            name: "query".into(),
            proxy_url: "socks5h://proxy.example.test:1080?token=1#frag".into(),
            username: None,
            password: None,
            no_proxy_hosts: Vec::new(),
            enabled: true,
        }),
    )
    .await;
    assert_eq!(
        listed_proxy(&repositories, query.id).await.proxy_url,
        "socks5h://proxy.example.test:1080"
    );

    // An update that keeps the embedded credential re-projects identically.
    let listed = listed_proxy(&repositories, created.id).await;
    let updated = commit_mutation(
        &repositories,
        world.admin,
        ControlPlaneMutation::UpdateProxy {
            id: created.id,
            input: ProxyInput {
                name: "credentialed".into(),
                proxy_url: "http://user:secret@proxy.example.test:9090".into(),
                username: None,
                password: None,
                no_proxy_hosts: vec!["localhost".into()],
                enabled: true,
            },
            expected_updated_at: expected_etag(listed.updated_at),
        },
    )
    .await;
    assert_eq!(updated.action, "update");
    let updated_audit = audit_for(&repositories, "update", created.id).await;
    assert_eq!(
        updated_audit.after_redacted.as_ref().unwrap()["proxy_url"],
        json!("http://proxy.example.test:9090")
    );
    assert_eq!(
        listed_proxy(&repositories, created.id).await.proxy_url,
        "http://proxy.example.test:9090"
    );
}
