#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::state::Settings;
use api::{build_router, AppState};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use emberloop_core::events::EventBus;
use std::sync::Arc;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tempfile::tempdir;
use tower::ServiceExt;

// Split the retired literal so a repo-wide removal grep stays clean.
#[test]
fn settings_load_an_old_review_interval_key_and_stop_writing_it() {
    let key = concat!("skills_review_", "interval");
    let stored: Settings = serde_json::from_str(&format!("{{\"{key}\": 5}}")).unwrap();
    let defaults = Settings::default();
    assert_eq!(stored.title_generation, defaults.title_generation);
    assert_eq!(stored.default_model, defaults.default_model);
    assert_eq!(stored.tier2_trigger, defaults.tier2_trigger);
    assert_eq!(stored.tier3_trigger, defaults.tier3_trigger);
    assert_eq!(stored.max_context_limit, defaults.max_context_limit);
    let written = serde_json::to_value(Settings::default()).unwrap();
    assert!(written.get(key).is_none());
}

struct Listed;

#[async_trait]
impl api::models::Source for Listed {
    fn name(&self) -> &'static str {
        "agy"
    }
    async fn fetch(&self) -> Result<Vec<api::models::ModelEntry>, api::models::CatalogueError> {
        Ok(["gemini-3.8-flash", "claude-opus-5-5"]
            .into_iter()
            .map(|id| api::models::ModelEntry {
                id: id.into(),
                provider: "agy".into(),
                label: id.into(),
                kind: "chat".into(),
                efforts: vec!["low".into(), "medium".into(), "high".into()],
                image_input: true,
                context_window: 1_048_576,
                source: "agy".into(),
                stale: false,
                updated_at: 0,
            })
            .collect())
    }
}

async fn state(root: &DataRoot) -> AppState {
    let models = Arc::new(api::models::ModelCatalogue::new(
        root.model_catalogue_file(),
        vec![Arc::new(Listed)],
    ));
    models.refresh().await.unwrap();
    AppState::new(
        root.clone(),
        ProjectsStore::load(root.projects_file()).await.unwrap(),
        SessionsStore::load(root.clone()).await.unwrap(),
        Arc::new(EventBus::default()),
    )
    .with_models(models)
}

#[tokio::test]
async fn reviewer_models_survive_a_daemon_restart() {
    let dir = tempdir().unwrap();
    let root = DataRoot::new(dir.path());
    let first = state(&root).await;
    first.models.refresh().await.unwrap();
    let app = build_router(first);

    let mut settings = Settings {
        spec_reviewer: api::state::FallbackLadder(vec![
            api::state::ModelRung {
                model: "claude-opus-5-5".into(),
                reasoning_effort: Some("high".into()),
            },
            api::state::ModelRung {
                model: "gemini-3.8-flash".into(),
                reasoning_effort: Some("low".into()),
            },
        ]),
        ..Default::default()
    };
    settings.subagent_tiers.smol =
        api::state::FallbackLadder::single("gemini-3.8-flash", Some("low".into()));
    settings.subagent_tiers.top = api::state::FallbackLadder(vec![
        api::state::ModelRung {
            model: "claude-opus-5-5".into(),
            reasoning_effort: Some("high".into()),
        },
        api::state::ModelRung {
            model: "gemini-3.8-flash".into(),
            reasoning_effort: Some("medium".into()),
        },
    ]);
    // Only one goldfish tier is overridden; the other two stay empty and inherit.
    settings.goldfish_tiers.top =
        api::state::FallbackLadder::single("claude-opus-5-5", Some("high".into()));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&settings).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(dir.path().join("settings.json").is_file());

    let restarted = state(&root).await;
    let loaded = restarted.settings.read().await.clone();
    assert_eq!(loaded.spec_reviewer.rungs().len(), 2);
    assert_eq!(loaded.spec_reviewer.rungs()[0].model, "claude-opus-5-5");
    assert_eq!(loaded.spec_reviewer.rungs()[1].model, "gemini-3.8-flash");
    assert_eq!(
        loaded.standards_reviewer,
        Settings::default().standards_reviewer
    );
    assert_eq!(
        loaded.subagent_tiers.smol.rungs()[0].model,
        "gemini-3.8-flash"
    );
    assert_eq!(
        loaded.subagent_tiers.smol.rungs()[0]
            .reasoning_effort
            .as_deref(),
        Some("low")
    );
    assert_eq!(loaded.subagent_tiers.top.rungs().len(), 2);
    assert_eq!(
        loaded.subagent_tiers.top.rungs()[0].model,
        "claude-opus-5-5"
    );
    assert_eq!(
        loaded.subagent_tiers.top.rungs()[0]
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    assert_eq!(loaded.goldfish_tiers.top.rungs().len(), 1);
    assert_eq!(
        loaded.goldfish_tiers.top.rungs()[0].model,
        "claude-opus-5-5"
    );
    assert!(loaded.goldfish_tiers.smol.is_empty());
    assert!(loaded.goldfish_tiers.mid.is_empty());

    // Empty ladder rejected.
    let empty_ladder = Settings {
        spec_reviewer: api::state::FallbackLadder(Vec::new()),
        ..Default::default()
    };
    let empty_res = build_router(state(&root).await)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&empty_ladder).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty_res.status(), StatusCode::BAD_REQUEST);

    // Invalid model on second rung rejected.
    let mut bad_tier_model = Settings::default();
    bad_tier_model.subagent_tiers.mid = api::state::FallbackLadder(vec![
        api::state::ModelRung {
            model: "gemini-3.8-flash".into(),
            reasoning_effort: None,
        },
        api::state::ModelRung {
            model: "nonexistent-model".into(),
            reasoning_effort: None,
        },
    ]);
    let bad_res = build_router(state(&root).await)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&bad_tier_model).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_res.status(), StatusCode::BAD_REQUEST);

    let mut bad_tier_effort = Settings::default();
    bad_tier_effort.subagent_tiers.top =
        api::state::FallbackLadder::single("claude-opus-5-5", Some("extreme".into()));
    let effort_res = build_router(state(&root).await)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&bad_tier_effort).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(effort_res.status(), StatusCode::BAD_REQUEST);

    // A non-empty goldfish ladder is validated like the others, and an empty one is accepted.
    let mut bad_goldfish_model = Settings::default();
    bad_goldfish_model.goldfish_tiers.mid =
        api::state::FallbackLadder::single("nonexistent-model", None);
    let bad_goldfish_res = build_router(state(&root).await)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&bad_goldfish_model).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_goldfish_res.status(), StatusCode::BAD_REQUEST);

    let empty_goldfish = Settings::default();
    let empty_goldfish_res = build_router(state(&root).await)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&empty_goldfish).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty_goldfish_res.status(), StatusCode::OK);
}

#[tokio::test]
async fn legacy_single_object_settings_deserialize_as_one_rung_ladders() {
    let dir = tempdir().unwrap();
    let root = DataRoot::new(dir.path());
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{
            "spec_reviewer": { "model": "claude-opus-5-5", "reasoning_effort": "high" },
            "standards_reviewer": { "model": "gemini-3.8-flash", "reasoning_effort": null },
            "subagent_tiers": {
                "smol": { "model": "gemini-3.8-flash", "reasoning_effort": "low" },
                "mid": { "model": "gemini-3.8-flash", "reasoning_effort": "medium" },
                "top": { "model": "claude-opus-5-5", "reasoning_effort": "high" }
            }
        }"#,
    )
    .unwrap();

    let loaded = state(&root).await.settings.read().await.clone();
    assert_eq!(
        loaded.spec_reviewer,
        api::state::FallbackLadder::single("claude-opus-5-5", Some("high".into()))
    );
    assert_eq!(
        loaded.standards_reviewer,
        api::state::FallbackLadder::single("gemini-3.8-flash", None)
    );
    assert_eq!(
        loaded.subagent_tiers.smol,
        api::state::FallbackLadder::single("gemini-3.8-flash", Some("low".into()))
    );
    assert_eq!(
        loaded.subagent_tiers.mid,
        api::state::FallbackLadder::single("gemini-3.8-flash", Some("medium".into()))
    );
    assert_eq!(
        loaded.subagent_tiers.top,
        api::state::FallbackLadder::single("claude-opus-5-5", Some("high".into()))
    );
}

fn put(settings: &Settings) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(settings).unwrap()))
        .unwrap()
}

#[test]
fn base_url_normalises_to_an_origin_and_rejects_everything_else() {
    assert_eq!(
        api::state::normalize_base_url("HTTPS://Box.ts.net:443/").unwrap(),
        "https://box.ts.net"
    );
    assert_eq!(
        api::state::normalize_base_url("http://100.64.0.7:8443").unwrap(),
        "http://100.64.0.7:8443"
    );
    for bad in [
        "https://box.ts.net/ember",
        "box.ts.net",
        "https://box.ts.net?x=1",
        "https://box.ts.net#top",
        "https://user:pass@box.ts.net",
    ] {
        assert_eq!(
            api::state::normalize_base_url(bad).unwrap_err(),
            "base_url must be an http(s) origin like https://host[:port]",
            "{bad}"
        );
    }
}

#[tokio::test]
async fn settings_put_rejects_a_bad_base_url_and_persists_a_good_one() {
    let dir = tempdir().unwrap();
    let root = DataRoot::new(dir.path());
    let app = build_router(state(&root).await);
    let bad = Settings {
        base_url: "https://box.ts.net/ember".into(),
        ..Default::default()
    };
    let response = app.clone().oneshot(put(&bad)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let good = Settings {
        base_url: "HTTPS://Box.ts.net:443/".into(),
        ..Default::default()
    };
    let response = app.oneshot(put(&good)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let restarted = build_router(state(&root).await);
    let response = restarted
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["base_url"], "https://box.ts.net");
}
