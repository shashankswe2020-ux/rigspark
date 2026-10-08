use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rigspark_gui::{Host, router};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
async fn call(host: &Arc<Host>, method: &str, path: &str, payload: Value) -> (StatusCode, Value) {
    let response = router(host.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", format!("127.0.0.1:{}", host.port))
                .header("origin", host.origin())
                .header("x-llmup-token", &host.token)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn persistence_routes_match_frontend_payloads() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    let (status, created) = call(&host, "POST", "/api/sessions", json!({"title":"Session"})).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["session"]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &host,
            "PATCH",
            &format!("/api/sessions/{id}"),
            json!({"title":"Renamed","expectedRevision":0})
        )
        .await
        .1["session"]["revision"],
        1
    );
    assert_eq!(
        call(
            &host,
            "PATCH",
            &format!("/api/sessions/{id}"),
            json!({"title":"Stale","expectedRevision":0})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &host,
            "POST",
            "/api/agents",
            json!({"name":"Builder","body":"instructions"})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let connector = call(
        &host,
        "POST",
        "/api/connectors",
        json!({"name":"Fixture","transport":"stdio","command":"never-run"}),
    )
    .await;
    assert_eq!(connector.0, StatusCode::CREATED);
    assert_eq!(connector.1["connector"]["id"], "fixture");
    let registered = call(
        &host,
        "POST",
        "/api/workspace/root",
        json!({"path":root.path()}),
    )
    .await
    .1;
    let proposal = json!({"workspaceId":registered["root"]["id"],"operations":[{"op":"create","path":"new.txt","text":"created"}]});
    assert_eq!(
        call(
            &host,
            "POST",
            "/api/workspace/edits/apply",
            proposal.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &host,
            "POST",
            "/api/workspace/edits/review",
            proposal.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&host, "POST", "/api/workspace/edits/apply", proposal)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new.txt")).unwrap(),
        "created"
    );
}
#[tokio::test]
async fn oversized_json_is_rejected_before_storage() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    assert_eq!(
        call(
            &host,
            "POST",
            "/api/sessions",
            json!({"title":"x".repeat(65536)})
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(host.sessions.list(false, 0, 50).unwrap().0.is_empty());
}

#[tokio::test]
async fn local_benchmark_report_is_not_exposed_by_gui() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    let path = home.path().join("benchmark-report.json");
    std::fs::write(&path, r#"{"schemaVersion":1,"title":"Comparison","hardware":"Test host","runtime":"Test runtime","settings":{},"models":{},"summary":{},"quality":[],"errors":[],"caveats":[],"completedAt":"2026-10-01T00:00:00Z"}"#).unwrap();
    for endpoint in ["/api/benchmarks", "/static/benchmarks.js"] {
        assert_eq!(
            call(&host, "GET", endpoint, Value::Null).await.0,
            StatusCode::NOT_FOUND,
            "{endpoint}"
        );
    }
    assert!(!include_str!("../static/index.html").contains("benchmarks"));
    assert!(!include_str!("../static/chat.js").contains("RigSparkBenchmarks"));
}

#[tokio::test]
async fn catalog_browsing_limit_is_bounded_and_default_stays_eight() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    for limit in ["0", "1001", "-1", "invalid"] {
        assert_eq!(
            call(
                &host,
                "GET",
                &format!("/api/models/recommended?limit={limit}"),
                json!({})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, default) = call(&host, "GET", "/api/models/recommended", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(default["models"].as_array().unwrap().len() <= 8);
    let (status, expanded) =
        call(&host, "GET", "/api/models/recommended?limit=100", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let default = default["models"].as_array().unwrap();
    let expanded = expanded["models"].as_array().unwrap();
    assert!(expanded.len() <= 100);
    assert!(expanded.len() >= default.len());
    assert_eq!(
        default.iter().map(|model| &model["id"]).collect::<Vec<_>>(),
        expanded
            .iter()
            .take(default.len())
            .map(|model| &model["id"])
            .collect::<Vec<_>>()
    );
    // Accepted up to the bound; ranking content is checked with fixed hardware below, because
    // each request re-detects live memory, which shifts between calls on non-unified machines.
    let (status, full) = call(
        &host,
        "GET",
        "/api/models/recommended?limit=1000",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(full["models"].as_array().unwrap().len() <= 1000);
}

#[test]
fn full_catalog_ranking_extends_the_top_results_unchanged() {
    let catalog = rigspark_core::catalog::Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let perf = rigspark_core::catalog::PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    let gib = 1024_u64.pow(3);
    let hardware: rigspark_core::sizing::Hardware = serde_json::from_value(json!({
        "arch": "arm64", "platform": "darwin", "totalRamBytes": 64 * gib,
        "freeRamBytes": 48 * gib, "freeDiskBytes": 2000 * gib,
        "gpu": [{"vendor": "apple", "vramBytes": 0}]
    }))
    .unwrap();
    let options = rigspark_core::ranking::AdviceOptions::default();
    let rank = |limit| {
        rigspark_gui::models::recommended(&catalog, &hardware, &perf, &options, limit)
            .unwrap()
            .into_iter()
            .map(|model| model["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let top = rank(100);
    let full = rank(1000);
    assert_eq!(top.len(), 100);
    assert!(full.len() > 100, "only {} models ranked", full.len());
    assert!(full.len() <= catalog.models.len());
    assert!(full.iter().any(|id| id == "bonsai:8b"));
    assert_eq!(top, full[..100]);
}

#[tokio::test]
async fn recency_window_only_narrows_recommendations_and_rejects_bad_values() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    for month in ["0", "4", "x"] {
        assert_eq!(
            call(
                &host,
                "GET",
                &format!("/api/models/recommended?month={month}"),
                json!({})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "{month}"
        );
    }
    let (status, recent) = call(
        &host,
        "GET",
        "/api/models/recommended?limit=100&month=3",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let today = rigspark_runtime::native_chat::timestamp().unwrap();
    let cutoff = rigspark_core::catalog::months_before(&today[..10], 3)
        .unwrap()
        .to_string();
    for model in recent["models"].as_array().unwrap() {
        let day = model
            .get("releaseDate")
            .or_else(|| model.get("addedAt"))
            .and_then(|day| day.as_str())
            .unwrap();
        assert!(day >= cutoff.as_str(), "{}: {day}", model["id"]);
    }
}
