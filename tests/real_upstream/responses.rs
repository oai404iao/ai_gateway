use super::support::{SmokeFormat, SmokeSettings, smoke_format};

#[tokio::test]
#[ignore = "requires an explicitly enabled real upstream key; use scripts/run-real-upstream-smoke.sh"]
async fn forwards_responses_requests_to_a_real_upstream() {
    let settings = SmokeSettings::from_environment();
    smoke_format(&settings, SmokeFormat::Responses, &settings.responses_model).await;
}
