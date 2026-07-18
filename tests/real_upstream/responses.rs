use super::support::{
    SmokeFormat, SmokeSettings, smoke_nonstreaming_format, smoke_streaming_format,
};

#[tokio::test]
#[ignore = "requires an explicitly enabled real upstream key; use scripts/run-real-upstream-smoke.sh"]
async fn captures_responses_nonstreaming_usage_from_a_real_upstream() {
    let settings = SmokeSettings::from_environment();
    smoke_nonstreaming_format(&settings, SmokeFormat::Responses, &settings.responses_model).await;
}

#[tokio::test]
#[ignore = "requires an explicitly enabled real upstream key; use scripts/run-real-upstream-smoke.sh"]
async fn captures_responses_streaming_usage_from_a_real_upstream() {
    let settings = SmokeSettings::from_environment();
    smoke_streaming_format(&settings, SmokeFormat::Responses, &settings.responses_model).await;
}
