use super::support::{SmokeSettings, smoke_images_edit, smoke_images_generation};

#[tokio::test]
#[ignore = "requires explicitly enabled real Images upstream settings; use scripts/run-real-upstream-smoke.sh"]
async fn captures_images_generation_usage_from_a_real_upstream() {
    let settings = SmokeSettings::from_environment();
    smoke_images_generation(&settings).await;
}

#[tokio::test]
#[ignore = "requires explicitly enabled real Images upstream settings; use scripts/run-real-upstream-smoke.sh"]
async fn captures_images_edit_usage_from_a_real_upstream() {
    let settings = SmokeSettings::from_environment();
    smoke_images_edit(&settings).await;
}
