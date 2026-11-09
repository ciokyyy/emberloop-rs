#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::state::Settings;

#[test]
fn skills_review_interval_defaults_to_five_and_stored_settings_without_it_load() {
    assert_eq!(Settings::default().skills_review_interval, 5);
    let stored: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(stored.skills_review_interval, 5);
}
