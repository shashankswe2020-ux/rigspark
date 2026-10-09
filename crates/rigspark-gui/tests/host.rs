use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rigspark_gui::{Host, router};
use tower::ServiceExt;

#[test]
fn gui_uses_rigspark_branding() {
    let index = include_str!("../static/index.html");
    assert!(index.contains("<title>RigSpark</title>"));
    assert!(index.contains("/static/brand/favicon.svg"));
    assert!(index.contains("/static/brand/mark-dark.svg"));
    assert!(!index.contains("mascot-"));
    for face in ["yes", "slow", "no", "wink", "wow"] {
        for variant in ["", "-dark"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("static/brand/sparky-{face}{variant}.svg"));
            let svg = std::fs::read_to_string(&path).unwrap();
            assert!(!svg.to_ascii_lowercase().contains("<script"), "{path:?}");
        }
    }
    let config: serde_json::Value = serde_json::from_str(include_str!(
        "../../../apps/desktop/src-tauri/tauri.conf.json"
    ))
    .unwrap();
    assert_eq!(config["productName"], "RigSpark");
}

#[test]
fn embedded_fonts_carry_their_own_ofl_copyright_notices() {
    // Copyright strings from the name table of the shipped Inter 4.001 and JetBrains Mono 2.211.
    for license in [
        include_str!("../static/fonts/LICENSE-OFL.txt"),
        include_str!("../../../site/brand/fonts/LICENSE-OFL.txt"),
    ] {
        assert!(
            license.contains(
                "Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter)"
            )
        );
        assert!(license.contains(
            "Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)"
        ));
        assert!(license.contains("SIL OPEN FONT LICENSE Version 1.1"));
        assert!(
            !license.contains("Bricolage"),
            "names a font we do not ship"
        );
    }
    let notice = include_str!("../vendor/README.md");
    for font in ["Inter | 4.001", "JetBrains Mono | 2.211"] {
        assert!(notice.contains(font), "THIRD-PARTY.md missing {font}");
    }
    assert!(notice.contains("fonts.OFL.txt"));
}

#[test]
fn models_and_chat_are_the_only_generation_surfaces() {
    let index = include_str!("../static/index.html");
    let generation = include_str!("../static/generate.js");
    let chat = include_str!("../static/chat.js");

    for kind in ["text", "image", "video"] {
        assert!(index.contains(&format!("data-model-kind=\"{kind}\"")));
    }
    assert!(index.contains("id=\"active-models-summary\""));
    assert!(!index.contains("id=\"chat-generation-settings\""));
    assert!(!index.contains("id=\"generation-dir\""));
    assert!(!index.contains("id=\"generation-port\""));
    assert!(!index.contains("id=\"generation-bypass\""));
    assert!(!index.contains("data-view=\"create\""));
    assert!(!index.contains("id=\"view-create\""));
    assert!(generation.contains("rigspark.active.image"));
    assert!(generation.contains("rigspark.active.video"));
    assert!(generation.contains("activeSelections[model.kind] = model.id"));
    assert!(generation.contains("rigspark:text-model-active"));
    assert!(chat.contains("rigspark:text-model-active"));
    assert!(generation.contains("model: model.id"));
}

#[tokio::test]
async fn artifacts_are_sandboxed_and_vendor_assets_are_embedded() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    std::fs::create_dir_all(home.path().join("artifacts")).unwrap();
    std::fs::write(
        home.path().join("artifacts/chart.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
    )
    .unwrap();
    for path in [
        "/api/images/chart.svg",
        "/vendor/marked.min.js",
        "/vendor/dompurify.min.js",
        "/vendor/katex/katex.min.css",
        "/vendor/katex/katex.min.js",
        "/vendor/katex/contrib/auto-render.min.js",
        "/vendor/katex/fonts/KaTeX_Main-Regular.woff2",
        "/static/chat.js",
        "/static/calculator-runtime.js",
        "/static/calculator-template.js",
        "/static/markdown.js",
        "/static/brand/favicon.svg",
        "/static/brand/mark-dark.svg",
        "/static/brand/sparky-yes-dark.svg",
        "/static/brand/3d/sparky-studio.webp",
        "/static/fonts/inter-400.woff2",
        "/static/run-reducer.js",
        "/static/sse.js",
        "/static/telemetry.js",
        "/static/workspace.js",
        "/static/styles.css",
    ] {
        let response = router(host.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "127.0.0.1:43210")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["cache-control"], "no-store");
        if path.ends_with(".jpg") {
            assert_eq!(response.headers()["content-type"], "image/jpeg");
        }
        if path.ends_with(".css") {
            assert_eq!(response.headers()["content-type"], "text/css");
        }
        if path.ends_with(".woff2") {
            assert_eq!(response.headers()["content-type"], "font/woff2");
        }
        if path.ends_with(".webp") {
            assert_eq!(response.headers()["content-type"], "image/webp");
        }
        if path.starts_with("/static/") && path.ends_with(".svg") {
            assert_eq!(response.headers()["content-type"], "image/svg+xml");
            assert_eq!(
                response.headers()["content-security-policy"],
                "default-src 'none'; style-src 'unsafe-inline'; sandbox"
            );
        }
        if path.starts_with("/api/images/") {
            assert_eq!(
                response.headers()["content-security-policy"],
                "default-src 'none'; sandbox"
            );
        }
        assert!(
            !to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }
    for path in [
        "/api/images/%2e%2e%2fchart.svg",
        "/static/%2e%2e/package.json",
        "/static/fonts/LICENSE-OFL.txt",
        "/vendor/unknown.js",
    ] {
        let response = router(host.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "127.0.0.1:43210")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error(), "{path}");
    }
}

#[test]
fn desktop_embeds_crate_owned_assets_without_root_src() {
    let config: serde_json::Value = serde_json::from_str(include_str!(
        "../../../apps/desktop/src-tauri/tauri.conf.json"
    ))
    .unwrap();
    assert_eq!(
        config["build"]["frontendDist"],
        "../../../crates/rigspark-gui/static"
    );
    assert!(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("static/index.html")
            .is_file()
    );
    assert!(
        !std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src/gui/static")
            .exists()
    );
}

#[tokio::test]
async fn mutations_without_origin_and_cross_site_reads_are_denied() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    for (method, path, cross_site) in [
        ("POST", "/api/sessions", false),
        ("GET", "/api/status", true),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:43210");
        if cross_site {
            request = request.header("sec-fetch-site", "cross-site");
        }
        let response = router(host.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn serve_rejects_mismatched_or_non_loopback_listeners() {
    for address in ["127.0.0.1:0", "0.0.0.0:0"] {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = Host::new(
            home.path(),
            if address.starts_with("127") { 0 } else { port },
        )
        .unwrap();
        host.shutdown.cancel();
        let result = rigspark_gui::serve(listener, host).await;
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }
}

#[tokio::test]
async fn launch_boundary_rejects_rebinding_cross_origin_and_missing_tokens() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    let app = router(host.clone());
    let wrong = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .header("host", "evil.example:43210")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions")
                .header("host", "127.0.0.1:43210")
                .header("origin", "https://evil.example")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let no_token = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/workspace/tree?id=unknown")
                .header("host", "127.0.0.1:43210")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_token.status(), StatusCode::FORBIDDEN);
    let index = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "127.0.0.1:43210")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(index.status(), StatusCode::OK);
    assert!(index.headers().contains_key("content-security-policy"));
    let body = to_bytes(index.into_body(), 1024 * 1024).await.unwrap();
    assert!(
        String::from_utf8(body.to_vec())
            .unwrap()
            .contains("llmup-token")
    );
}
