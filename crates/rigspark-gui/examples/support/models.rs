use super::{
    TestResult, choose, click, click_within, fresh_session, press_focused, send, type_into,
    wait_for,
};
use fantoccini::Client;
use serde_json::{Value, json};
use std::time::Duration;

const VISIBLE: &str = "((node) => Boolean(node && node.getClientRects().length > 0))";

async fn control(client: &Client, path: &str, body: &str) -> TestResult {
    let status = client
        .execute_async(
            "const done = arguments[arguments.length - 1]; fetch(arguments[0], {method: 'POST', body: arguments[1]}).then((r) => done(r.status), () => done(0));",
            vec![json!(path), json!(body)],
        )
        .await?;
    if status != json!(204) {
        return Err(format!("fixture control {path} returned {status}").into());
    }
    Ok(())
}

async fn fetch_json(client: &Client, path: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(client
        .execute_async(
            "const done = arguments[arguments.length - 1]; fetch(arguments[0]).then((r) => r.json()).then(done, () => done(null));",
            vec![json!(path)],
        )
        .await?)
}

async fn accept_confirm(client: &Client) -> Result<String, Box<dyn std::error::Error>> {
    let text = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(text) = client.get_alert_text().await {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for the confirmation dialog")?;
    client.accept_alert().await?;
    Ok(text)
}

async fn wait_for_start(client: &Client, expected: &Value, exact: bool) -> TestResult {
    let fields = expected
        .as_object()
        .ok_or("expected start must be an object")?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let starts = fetch_json(client, "/__fixture/model-starts").await?;
            if starts.as_array().is_some_and(|starts| {
                starts.iter().any(|start| {
                    (!exact || start.as_object().map(serde_json::Map::len) == Some(fields.len()))
                        && fields
                            .iter()
                            .all(|(key, value)| start.get(key) == Some(value))
                })
            }) {
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| format!("model start {expected} was never requested"))?
}

async fn open_models(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    click(client, ".rail-item[data-view=\"models\"]").await?;
    wait_for(
        client,
        &format!("{VISIBLE}(document.querySelector('.model-card-item'))"),
    )
    .await
}

// Fit controls live in a popover; open it before interacting with them.
async fn open_fit_settings(client: &Client) -> TestResult {
    if client
        .execute("return document.querySelector('#fit-pop').hidden;", vec![])
        .await?
        == json!(true)
    {
        click(client, "#fit-settings").await?;
    }
    wait_for(
        client,
        &format!("{VISIBLE}(document.querySelector('#models-fit-only'))"),
    )
    .await
}

async fn open_first_detail(client: &Client) -> TestResult {
    click(
        client,
        ".model-card-item button[aria-label^=\"View performance details for\"]",
    )
    .await?;
    wait_for(client, &format!("{VISIBLE}(document.querySelector('#model-detail-title')) && {VISIBLE}(document.querySelector('#model-detail-back'))")).await
}

pub async fn details(client: &Client, origin: &str, artifacts: &std::path::Path) -> TestResult {
    open_models(client, origin).await?;
    wait_for(
        client,
        "document.querySelector('#catalog-status')?.textContent.includes('bundled')",
    )
    .await?;
    click(client, "#update-catalog").await?;
    wait_for(client, "document.querySelector('#catalog-status')?.textContent.includes('revision 1') && !document.querySelector('#update-catalog').disabled").await?;
    wait_for(client, "document.querySelector('#catalog-update-result')?.textContent.includes('Catalog updated to revision 1') && !document.querySelector('#catalog-update-result').hidden").await?;
    click(client, "#update-catalog").await?;
    wait_for(client, "document.querySelector('#catalog-update-result')?.textContent.includes('Already using the latest published catalog') && !document.querySelector('#catalog-update-result').hidden && !document.querySelector('#update-catalog').disabled").await?;
    click(client, "#update-catalog").await?;
    wait_for(client, "document.querySelector('#catalog-update-error')?.textContent.includes('offline') && !document.querySelector('#update-catalog').disabled && document.querySelector('.model-card-item')").await?;
    wait_for(
        client,
        "document.querySelector('#catalog-update-result')?.hidden === true",
    )
    .await?;
    std::fs::write(
        artifacts.join("catalog-update-desktop.png"),
        client.screenshot().await?,
    )?;
    let name = client
        .execute(
            "return document.querySelector('.model-card-item .model-card-title').textContent;",
            vec![],
        )
        .await?;
    open_first_detail(client).await?;
    let failed = client
        .execute(
            &format!("const visible = {VISIBLE}; const failed = []; const check = (label, ok) => {{ if (!ok) failed.push(label); }};
                const heading = (name) => [...document.querySelectorAll('#model-detail h1, #model-detail h2, #model-detail h3, #model-detail h4')].some((node) => node.textContent.trim() === name && visible(node));
                check('detail', visible(document.querySelector('#model-detail')));
                check('catalog hidden', !visible(document.querySelector('#model-catalog-panel')));
                check('title', document.querySelector('#model-detail-title').textContent === arguments[0]);
                for (const name of ['Recommendation score', 'Performance & fit', 'Model profile', 'Quantization options', 'Catalog evidence']) check(name, heading(name));
                check('score rows', document.querySelectorAll('.model-score-row').length === 5);
                check('quant row', visible(document.querySelector('.model-quant-table tbody tr')));
                check('evidence note', [...document.querySelectorAll('#model-detail *')].some((node) => node.children.length === 0 && node.textContent.includes('no benchmark result is implied') && visible(node)));
                return failed;"),
            vec![name],
        )
        .await?;
    if failed != json!([]) {
        return Err(format!("model detail failed: {failed}").into());
    }
    std::fs::write(
        artifacts.join("model-detail-desktop.png"),
        client.screenshot().await?,
    )?;
    click(client, "#model-detail-back").await?;
    wait_for(client, &format!("{VISIBLE}(document.querySelector('#model-catalog-panel')) && !{VISIBLE}(document.querySelector('#model-detail'))")).await?;

    control(client, "/__fixture/recommended-context", "65536").await?;
    click(client, "#refresh-models").await?;
    wait_for(client, "document.querySelector('.model-card-item .model-card-meta')?.textContent.includes('65,536 context tokens')").await?;
    open_first_detail(client).await?;
    let start = client
        .execute(
            "const button = [...document.querySelectorAll('#model-detail button')].find((node) => node.textContent.trim() === 'Start model'); if (button) button.dataset.fixtureStart = 'true'; return Boolean(button);",
            vec![],
        )
        .await?;
    if start != json!(true) {
        return Err("model detail has no Start model button".into());
    }
    click(client, "#model-detail button[data-fixture-start]").await?;
    accept_confirm(client).await?;
    wait_for_start(client, &json!({"context": 65536}), false).await?;
    control(client, "/__fixture/recommended-context", "").await
}

pub async fn bonsai(
    client: &Client,
    origin: &str,
    width: u32,
    artifacts: &std::path::Path,
) -> TestResult {
    open_models(client, origin).await?;
    let advice = fetch_json(client, "/api/models/recommended?limit=1000&context=mid").await?;
    let model = advice["models"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["id"] == "bonsai:8b"))
        .ok_or("Bonsai missing from expanded API response")?;
    if model["throughput"]["known"] != false
        || model["contextFitKnown"] != false
        || model["diskBytes"].as_f64() != Some(1_158_654_496.0)
        || model["backends"] != json!(["llamacpp", "lmstudio"])
    {
        return Err(format!("incorrect Bonsai API evidence: {model}").into());
    }
    std::fs::write(
        artifacts.join(format!("bonsai-api-{width}.json")),
        serde_json::to_vec_pretty(model)?,
    )?;
    let selector = ".model-card-item button[aria-label=\"View performance details for bonsai:8b\"]";
    click(client, selector).await?;
    wait_for(
        client,
        "document.querySelector('#model-detail-title')?.textContent === 'bonsai:8b'",
    )
    .await?;
    let facts = client
        .execute(
            "return document.querySelector('#model-detail').innerText;",
            vec![],
        )
        .await?;
    let text = facts.as_str().ok_or("missing detail text")?;
    for expected in [
        "8.19B",
        "apache-2.0",
        "Q1_0",
        "65,536",
        "llamacpp",
        "SHA-256 cataloged",
        "Bonsai-8B-Q1_0.gguf",
        "Speed unknown",
    ] {
        if !text.contains(expected) {
            return Err(format!("Bonsai detail missing {expected}: {text}").into());
        }
    }
    wait_for(client, "document.documentElement.scrollWidth <= innerWidth && [...document.querySelectorAll('#model-detail .model-backend-select option')].every(option => !['ollama', 'mlx'].includes(option.value)) && [...document.querySelectorAll('.model-detail-metric')].find(node => node.textContent.includes('Decode speed'))?.textContent.includes('unknown')").await?;
    choose(
        client,
        "#model-detail .model-backend-select option[value='llamacpp']",
    )
    .await?;
    let overflow = client.execute("const parent = document.querySelector('#model-detail').getBoundingClientRect(); return [...document.querySelectorAll('.model-detail-header, .model-detail-actions > *')].filter(node => {const rect = node.getBoundingClientRect(); return rect.right > parent.right + 1 || rect.left < parent.left - 1;}).map(node => ({tag:node.tagName, width:node.getBoundingClientRect().width, parent:parent.width}));", vec![]).await?;
    if overflow != json!([]) {
        return Err(format!("Bonsai action overflow at {width}: {overflow}").into());
    }
    std::fs::write(
        artifacts.join(format!("bonsai-detail-{width}.png")),
        client.screenshot().await?,
    )?;
    std::fs::write(artifacts.join(format!("bonsai-detail-{width}.txt")), text)?;
    let before = fetch_json(client, "/__fixture/model-starts").await?;
    click(client, "#model-detail .model-detail-actions button").await?;
    let prompt = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(text) = client.get_alert_text().await {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| format!("Bonsai confirmation timeout at {width}"))?;
    if !prompt.contains("bonsai:8b") || prompt.contains("runtime model tag") {
        return Err(format!("wrong confirmation: {prompt}").into());
    }
    client.dismiss_alert().await?;
    if fetch_json(client, "/__fixture/model-starts").await? != before {
        return Err("cancelled Bonsai confirmation sent a start request".into());
    }
    if width == 1440 {
        click(client, "#model-detail-back").await?;
        open_fit_settings(client).await?;
        choose(client, "#context-window option[value='65536']").await?;
        click(client, "#refresh-models").await?;
        wait_for(client, "[...document.querySelectorAll('.model-card-item')].find(node => node.querySelector('.model-card-title')?.textContent === 'bonsai:8b')?.textContent.includes('65,536 context tokens')").await?;
        click(client, selector).await?;
        choose(
            client,
            "#model-detail .model-backend-select option[value='llamacpp']",
        )
        .await?;
        click(client, "#model-detail .model-detail-actions button").await?;
        accept_confirm(client).await?;
        wait_for_start(
            client,
            &json!({"model":"bonsai:8b", "backend":"llamacpp", "context":65536}),
            true,
        )
        .await?;
        open_models(client, origin).await?;
        open_fit_settings(client).await?;
        choose(client, "#context-window option[value='mid']").await?;
        click(client, "#refresh-models").await?;
        click(client, selector).await?;
    }
    click(client, "#model-detail-back").await?;
    wait_for(
        client,
        &format!("{VISIBLE}(document.querySelector('#model-catalog-panel'))"),
    )
    .await?;
    Ok(())
}

pub async fn narrow_details(
    client: &Client,
    origin: &str,
    artifacts: &std::path::Path,
) -> TestResult {
    open_models(client, origin).await?;
    wait_for(client, "document.querySelector('#catalog-status')?.textContent.includes('revision 1') && document.documentElement.scrollWidth <= innerWidth").await?;
    std::fs::write(
        artifacts.join("catalog-update-mobile.png"),
        client.screenshot().await?,
    )?;
    open_first_detail(client).await?;
    wait_for(client, &format!("[...document.querySelectorAll('#model-detail h3')].some((node) => node.textContent.trim() === 'Quantization options' && {VISIBLE}(node)) && getComputedStyle(document.querySelector('.model-detail-metrics')).gridTemplateColumns.length > 0 && document.documentElement.scrollWidth <= innerWidth")).await?;
    std::fs::write(
        artifacts.join("model-detail-mobile.png"),
        client.screenshot().await?,
    )?;
    Ok(())
}

pub async fn installed(
    client: &Client,
    origin: &str,
    width: u32,
    artifacts: &std::path::Path,
) -> TestResult {
    open_models(client, origin).await?;
    client
        .execute(
            "const set = (id, value) => { const node = document.querySelector(id); node.value = value; node.dispatchEvent(new Event(node.tagName === 'SELECT' ? 'change' : 'input', {bubbles: true})); };
             set('#model-source', 'installed'); set('#context-window', 'custom'); set('#context-tokens', '65536'); set('#installed-port', '11435');",
            vec![],
        )
        .await?;
    click(client, "#refresh-models").await?;
    wait_for(client, &format!("[...document.querySelectorAll('#recommended-list .model-card-title')].some((node) => node.textContent === 'gemma4:e4b-it-qat' && {VISIBLE}(node)) && document.querySelector('#recommended-list').textContent.includes('Context fit unknown')")).await?;
    open_fit_settings(client).await?;
    click(client, "#models-fit-only").await?;
    wait_for(
        client,
        "document.querySelector('#recommended-list').textContent.includes('No installed models match')",
    )
    .await?;
    click(client, "#models-fit-only").await?;
    click(client, "#model-bypass").await?;
    press_focused(client, "\u{e00c}").await?;
    wait_for(client, "document.querySelector('#fit-pop').hidden && document.activeElement?.id === 'fit-settings'").await?;
    wait_for(client, "(() => { const start = [...document.querySelectorAll('#recommended-list button')].find((node) => node.textContent === 'Start'); return start && !start.disabled && document.documentElement.scrollWidth <= innerWidth; })()").await?;
    std::fs::write(
        artifacts.join(format!("installed-{width}.png")),
        client.screenshot().await?,
    )?;
    click(client, "#recommended-list .model-card-actions button").await?;
    let prompt = accept_confirm(client).await?;
    if !prompt.contains("65536") || !prompt.contains("integrity") {
        return Err(format!(
            "installed start confirmation is missing context or integrity: {prompt}"
        )
        .into());
    }
    wait_for_start(
        client,
        &json!({"model": "gemma4:e4b-it-qat", "backend": "ollama", "context": 65536, "bypass": true, "installed": true, "port": 11435}),
        true,
    )
    .await
}

pub async fn tools(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    control(client, "/__fixture/tools", "attach").await?;
    let result = async {
        for approve in [true, false] {
            fresh_session(client).await?;
            send(client, "use the TOOL please").await?;
            let decision = if approve { "Approve" } else { "Deny" };
            wait_for(client, &format!("document.querySelector('.tool-card')?.textContent.includes('demo_tool') && [...document.querySelectorAll('.tool-card button')].some((node) => node.textContent === {})", json!(decision))).await?;
            client
                .execute(
                    "[...document.querySelectorAll('.tool-card button')].find((node) => node.textContent === arguments[0]).dataset.fixtureDecision = 'true';",
                    vec![json!(decision)],
                )
                .await?;
            click(client, ".tool-card button[data-fixture-decision]").await?;
            if approve {
                wait_for(client, "document.querySelector('.tool-card')?.textContent.includes('Used demo_tool') && [...document.querySelectorAll('.message.assistant')].at(-1)?.textContent.includes('Tool finished. Done.')").await?;
            } else {
                wait_for(client, "(() => { const card = document.querySelector('.tool-card'); return card?.textContent.includes('denied') && !card.textContent.includes('Used demo_tool'); })()").await?;
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    wait_for(client, super::RUN_SETTLED).await?;
    control(client, "/__fixture/tools", "detach").await?;
    result
}

pub async fn workspace(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    let path = fetch_json(client, "/__fixture/workspace").await?;
    let path = path.as_str().ok_or("fixture workspace path unavailable")?;
    fresh_session(client).await?;
    // The paperclip only appears once workspace context is available (the bar is layout-only).
    wait_for(
        client,
        &format!(
            "!document.querySelector('#context-bar').hidden && {VISIBLE}(document.querySelector('#context-add'))"
        ),
    )
    .await?;
    click(client, "#context-add").await?;
    wait_for(
        client,
        &format!("{VISIBLE}(document.querySelector('#context-root-path'))"),
    )
    .await?;
    type_into(client, "#context-root-path", path, true).await?;
    click(client, "#context-root-add").await?;
    wait_for(
        client,
        &format!("{VISIBLE}(document.querySelector('#context-search'))"),
    )
    .await?;
    type_into(client, "#context-search", "app", false).await?;
    // Results re-render while the search settles, replacing a tagged node; tag and click again.
    let mut picked: TestResult = Err("context result never clickable".into());
    for _ in 0..5 {
        wait_for(client, &format!("(() => {{ const result = [...document.querySelectorAll('.context-result')].find((node) => node.textContent.includes('src/app.ts')); if (result && {VISIBLE}(result)) result.dataset.fixtureResult = 'true'; return Boolean(result?.dataset.fixtureResult); }})()")).await?;
        picked = click_within(
            client,
            ".context-result[data-fixture-result]",
            Duration::from_secs(3),
        )
        .await;
        if picked.is_ok() {
            break;
        }
    }
    picked?;
    wait_for(client, &format!("[...document.querySelectorAll('.context-chip')].some((node) => node.textContent.includes('app.ts') && {VISIBLE}(node))")).await?;
    send(client, "review this file").await?;
    wait_for(
        client,
        "document.querySelector('.context-ledger')?.textContent.includes('1 of 1')",
    )
    .await
}
