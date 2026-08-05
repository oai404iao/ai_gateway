use super::support::{SmokeSettings, smoke_standalone_web_search};

#[tokio::test]
#[ignore = "requires explicitly enabled real standalone web-search upstream settings; use scripts/run-real-upstream-smoke.sh"]
async fn forwards_standalone_web_search_to_a_real_upstream() {
    let settings = SmokeSettings::from_environment();
    smoke_standalone_web_search(&settings).await;
}
