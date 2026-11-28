//! Additive model onboarding must be all-or-nothing on both storage engines.

use ai_gateway::persistence::PageOptions;

use super::*;
use ai_gateway::{
    models_dev::ModelsDevModel,
    persistence::{ModelOnboardingInput, OnboardingModel, OnboardingOperation},
};

#[tokio::test]
async fn model_onboarding_contract_matches_across_backends() {
    run_contract(onboarding_contract).await;
}

fn op(version: Option<DateTime<Utc>>, candidates: Vec<Value>) -> Vec<OnboardingOperation> {
    serde_json::from_value(json!([{"operation":"chat_completion", "rule_updated_at":version, "candidates":candidates}])).unwrap()
}

async fn onboarding_contract(repositories: Repositories) {
    let w = world(&repositories).await;
    let repo = &repositories.control_plane;
    let start = repo.topology().await.unwrap();
    let cap = listed_capability(&repositories, w.capability).await;
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repo.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repo.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let candidate = |name: &str, version, priority, register| {
        json!({
            "capability_id": w.capability, "updated_at":version, "upstream_model":name,
            "priority":priority, "register_model":register
        })
    };
    let manual = || OnboardingModel::Create(Box::new(model_input("onboarded", "New Model")));
    let request = |model, operations| ModelOnboardingInput { model, operations };
    let before = runtime.snapshot();
    let audit_len = repo
        .audit_logs(PageOptions {
            limit: Some(100),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len();
    let register = candidate("new-wire", cap.updated_at, 1, true);
    let missing = candidate("missing-wire", cap.updated_at, 3, false);
    assert!(matches!(
        coordinator
            .onboard_model(
                w.admin,
                request(manual(), op(None, vec![register.clone(), missing]))
            )
            .await,
        Err(ControlPlaneError::Repository(
            RepositoryError::RoutingDependencyInvalid
        ))
    ));
    assert!(Arc::ptr_eq(&before, &runtime.snapshot()));
    assert!(repo.model_source_versions().await.unwrap().is_empty());
    assert!(repo.routing_profiles().await.unwrap().is_empty());
    assert_eq!(
        repo.audit_logs(PageOptions {
            limit: Some(100),
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len(),
        audit_len
    );
    assert_eq!(
        listed_capability(&repositories, w.capability)
            .await
            .settings
            .available_models,
        ["parity-model"]
    );

    let model = coordinator
        .onboard_model(w.admin, request(manual(), op(None, vec![register.clone()])))
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&before, &runtime.snapshot()));
    assert!(
        runtime
            .snapshot()
            .operation_rule(ApiOperation::ChatCompletions, "onboarded")
            .is_some()
    );
    let audits = repo
        .audit_logs(PageOptions {
            limit: Some(100),
            ..Default::default()
        })
        .await
        .unwrap()
        .items;
    assert_eq!(audits.len(), audit_len + 4);
    let correlation = &audits
        .iter()
        .find(|a| a.object_id == model)
        .unwrap()
        .correlation_id;
    assert!(correlation.is_some());
    assert_eq!(
        audits
            .iter()
            .filter(|a| &a.correlation_id == correlation)
            .count(),
        4
    );
    assert!(matches!(
        repo.prepare_model_onboarding(w.admin, request(manual(), op(None, vec![register])))
            .await,
        Err(RepositoryError::Conflict)
    ));

    let priced = listed_model(&repositories, model).await;
    let existing = |updated_at| OnboardingModel::Existing {
        id: model,
        updated_at,
    };
    let profile = repo.routing_profiles().await.unwrap()[0].id;
    let graph = operation_graph(&repositories, profile).await;
    let topology = repo.topology().await.unwrap();
    let mut edited =
        ai_gateway::persistence::upstream_topology::rules::rule_input(&topology, &graph.rule);
    edited.enabled = false;
    edited.routing_tiers[0].selection_strategy = "weighted_round_robin".into();
    edited.routing_tiers[0].candidates[0].weight = 7;
    let mut untouched = edited.routing_tiers[0].clone();
    untouched.priority = 2;
    edited.routing_tiers.push(untouched);
    let rule = commit_mutation(
        &repositories,
        w.admin,
        ControlPlaneMutation::SaveOperationRule {
            id: graph.rule.id,
            input: edited,
            expected_updated_at: Some(graph.rule.updated_at),
        },
    )
    .await;
    let prior_tiers = operation_graph(&repositories, profile).await.tiers;
    let cap_version = listed_capability(&repositories, w.capability)
        .await
        .updated_at;
    let old = candidate("new-wire", cap_version, 1, false);
    let same_tier = candidate("parity-model", cap_version, 1, false);
    let new_tier = candidate("new-wire", cap_version, 3, false);
    for (version, target) in [
        (None, old.clone()),
        (Some(graph.rule.updated_at), old.clone()),
        (
            Some(rule.updated_at),
            candidate("new-wire", cap.updated_at, 1, false),
        ),
    ] {
        assert!(matches!(
            repo.prepare_model_onboarding(
                w.admin,
                request(existing(priced.updated_at), op(version, vec![target]))
            )
            .await,
            Err(RepositoryError::Conflict)
        ));
    }
    let append = |model_version| {
        request(
            existing(model_version),
            op(
                Some(rule.updated_at),
                vec![old.clone(), same_tier.clone(), new_tier.clone()],
            ),
        )
    };
    let mut reprice = model_input("onboarded", "New Model");
    reprice.output_unit_price = decimal("3");
    let repriced = commit_mutation(
        &repositories,
        w.admin,
        ControlPlaneMutation::UpdateModel {
            id: model,
            input: reprice,
            expected_updated_at: priced.updated_at,
        },
    )
    .await;
    assert!(matches!(
        repo.prepare_model_onboarding(w.admin, append(priced.updated_at))
            .await,
        Err(RepositoryError::Conflict)
    ));
    assert_eq!(
        coordinator
            .onboard_model(w.admin, append(repriced.updated_at))
            .await
            .unwrap(),
        model
    );
    let topology = repo.topology().await.unwrap();
    let graph = operation_graph(&repositories, profile).await;
    let saved =
        ai_gateway::persistence::upstream_topology::rules::rule_input(&topology, &graph.rule);
    assert!(!saved.enabled);
    assert_eq!(saved.routing_tiers.len(), 3);
    for prior in prior_tiers {
        let current = graph
            .tiers
            .iter()
            .find(|tier| tier.priority == prior.priority)
            .unwrap();
        assert_eq!(
            current.id, prior.id,
            "append preserves the historical tier identity"
        );
    }
    let tier = &saved.routing_tiers[0];
    assert_eq!(
        (
            tier.priority,
            tier.selection_strategy.as_str(),
            tier.candidates.len()
        ),
        (1, "weighted_round_robin", 2)
    );
    assert_eq!(
        tier.candidates
            .iter()
            .find(|c| c.upstream_model == "new-wire")
            .unwrap()
            .weight,
        7
    );
    let tier = &saved.routing_tiers[2];
    assert_eq!(
        (
            tier.priority,
            tier.selection_strategy.as_str(),
            tier.candidates[0].weight
        ),
        (3, "weighted_random", 1)
    );
    assert_eq!(
        listed_model(&repositories, model).await.updated_at,
        repriced.updated_at
    );
    assert_eq!(json!(topology.policy_grants), json!(start.policy_grants));

    let catalog = ModelsDevModel {
        model_id: "catalog-price".into(),
        provider_id: "provider".into(),
        provider_name: "Provider".into(),
        display_name: "Catalog price".into(),
        input_unit_price: decimal("3.125"),
        cached_input_unit_price: Decimal::ZERO,
        cache_write_unit_price: Decimal::ZERO,
        output_unit_price: decimal("4.5"),
        advanced_billing: AdvancedBilling::default(),
        source_payload: json!({"source": "catalog-price"}),
    };
    let import = || {
        request(
            OnboardingModel::Catalog(catalog.clone()),
            op(None, vec![old.clone()]),
        )
    };
    let imported = coordinator.onboard_model(w.admin, import()).await.unwrap();
    let catalog_price = listed_model(&repositories, imported).await;
    assert_eq!(catalog_price.input_unit_price, decimal("3.125"));
    assert!(catalog_price.last_synced_at.is_some());
    assert!(matches!(
        repo.prepare_model_onboarding(w.admin, import()).await,
        Err(RepositoryError::Conflict)
    ));
}
