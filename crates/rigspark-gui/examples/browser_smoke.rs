use fantoccini::{Client, ClientBuilder, Locator};
use serde_json::json;
use std::{error::Error, path::PathBuf, time::Duration};

#[path = "support/client_units.rs"]
mod client_units;
#[path = "support/formatting.rs"]
mod formatting;
#[path = "support/models.rs"]
mod models;

type TestResult = Result<(), Box<dyn Error>>;

// New chat is ignored until finalize(); its final announcement follows clearing the active run.
const RUN_SETTLED: &str = "!window.__llmupTestRun || !['', 'Sending message.', 'Stopping…'].includes(document.querySelector('#a11y-status')?.textContent ?? '')";
const MARK_RUN: &str = "window.__llmupTestRun = true;";

async fn wait_for(client: &Client, expression: &str) -> TestResult {
    wait_for_within(client, expression, Duration::from_secs(15)).await
}

async fn wait_for_within(client: &Client, expression: &str, limit: Duration) -> TestResult {
    let script = format!("return Boolean({expression});");
    tokio::time::timeout(limit, async {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if client.execute(&script, vec![]).await?.as_bool() == Some(true) {
                return Ok::<(), fantoccini::error::CmdError>(());
            }
        }
    })
    .await
    .map_err(|_| format!("timed out waiting for {expression}"))??;
    Ok(())
}

async fn click(client: &Client, css: &str) -> TestResult {
    let selector = json!(css);
    wait_for(
        client,
        &format!("(() => {{ const node = document.querySelector({selector}); return node && node.getClientRects().length > 0 && !node.disabled; }})()"),
    )
    .await
    .map_err(|error| format!("click {css}: {error}"))?;
    client
        .find(Locator::Css(css))
        .await?
        .click()
        .await
        .map_err(|error| format!("click {css}: {error}"))?;
    Ok(())
}

async fn new_session(client: &Client) -> TestResult {
    wait_for(
        client,
        "document.querySelector('#session-new')?.getClientRects().length > 0",
    )
    .await?;
    wait_for(client, RUN_SETTLED).await?;
    // Initial activation of the server's session races New chat, so let it render first.
    let active = client
        .execute_async(
            "const done = arguments[arguments.length - 1]; fetch('/api/sessions').then((r) => r.json()).then((d) => { const s = d.sessions.find((x) => x.id === d.activeSessionId); done(s ? [s.id, s.messageCount] : null); }, () => done(false));",
            vec![],
        )
        .await?;
    if let Some([id, count]) = active.as_array().map(Vec::as_slice) {
        wait_for(
            client,
            &format!("document.querySelector('.rail-session-item.active')?.dataset.sessionId === {id} && document.querySelectorAll('.message').length === {count}"),
        )
        .await?;
    } else if active == json!(false) {
        return Err("session list unavailable".into());
    }
    let previous = client
        .execute(
            "return document.querySelector('.rail-session-item.active')?.dataset.sessionId || '';",
            vec![],
        )
        .await?;
    client
        .execute(
            "const original = window.fetch; window.__llmupCreated = null; window.__llmupRequested = false; window.fetch = async (...args) => { const create = String(args[0]).endsWith('/api/sessions') && args[1]?.method === 'POST'; if (create) window.__llmupRequested = true; const response = await original(...args); if (create) { const data = await response.clone().json().catch(() => null); window.__llmupCreated = [response.status, data?.session?.id ?? null]; window.fetch = original; } return response; };",
            vec![],
        )
        .await?;
    // `newChat` ignores clicks until the previous run releases `activeRun`, which can trail the
    // settled status text; retrying is safe because nothing is sent until the POST is observed.
    let mut requested = false;
    for _ in 0..20 {
        click(client, "#session-new").await?;
        if wait_for_within(
            client,
            "window.__llmupRequested",
            Duration::from_millis(500),
        )
        .await
        .is_ok()
        {
            requested = true;
            break;
        }
    }
    if !requested {
        return Err(format!("New chat never sent a request; previous session {previous}").into());
    }
    wait_for(
        client,
        "window.__llmupCreated && window.__llmupCreated[0] === 201 && /^[a-f0-9-]{36}$/.test(window.__llmupCreated[1] ?? '') && document.querySelector('.rail-session-item.active')?.dataset.sessionId === window.__llmupCreated[1] && document.querySelectorAll('.message').length === 0",
    )
    .await
    .map_err(|error| format!("{error}; previous session {previous}").into())
}

// API-created session plus reload avoids racing initial session loading; layouts may hide the rail.
async fn fresh_session(client: &Client) -> TestResult {
    wait_for(client, "document.querySelector('#prompt')").await?;
    wait_for(client, RUN_SETTLED).await?;
    let id = client
        .execute_async(
            "const done = arguments[arguments.length - 1]; fetch('/api/sessions', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: '{}'}).then((r) => r.ok ? r.json() : null).then((d) => done(d?.session?.id ?? null), () => done(null));",
            vec![],
        )
        .await?;
    let id = id.as_str().ok_or("session create failed")?.to_owned();
    client.refresh().await?;
    let result = wait_for(
        client,
        &format!(
            "document.querySelector('.rail-session-item.active')?.dataset.sessionId === {} && document.querySelectorAll('.message').length === 0",
            json!(id)
        ),
    )
    .await;
    if let Err(error) = result {
        let state = client.execute_async("const done = arguments[arguments.length - 1]; fetch('/api/sessions').then((r) => r.json()).then((d) => done({server: d.activeSessionId, top: d.sessions.slice(0, 3).map((s) => s.id), rail: document.querySelector('.rail-session-item.active')?.dataset.sessionId ?? null, messages: document.querySelectorAll('.message').length}), () => done(null));", vec![]).await?;
        return Err(format!("{error}; created {id}; session state {state}").into());
    }
    Ok(())
}

async fn send(client: &Client, message: &str) -> TestResult {
    let typed = async {
        let prompt = client.find(Locator::Css("#prompt")).await?;
        prompt.clear().await?;
        prompt.send_keys(message).await?;
        client.execute(MARK_RUN, vec![]).await?;
        prompt.send_keys("\u{e007}").await
    };
    typed
        .await
        .map_err(|error| format!("send {message:?}: {error}"))?;
    Ok(())
}

async fn journeys(client: &Client, origin: &str, artifacts: &std::path::Path) -> TestResult {
    client.set_window_size(1440, 1000).await?;
    client.goto(origin).await?;
    client
        .wait()
        .at_most(Duration::from_secs(15))
        .for_element(Locator::Css("#prompt"))
        .await?;
    new_session(client).await?;
    send(client, "checkpoint five").await?;
    wait_for(client, "document.querySelector('.message.assistant')?.textContent.includes('Native reply: checkpoint five') && document.querySelector('#a11y-status')?.textContent === 'Response ready.'").await?;
    wait_for(client, "document.title === 'RigSpark' && (() => { const image = document.querySelector('.message.assistant .message-avatar'); return image && /^\\/static\\/brand\\/sparky-yes(-dark)?\\.svg$/.test(image.getAttribute('src')) && image.complete && image.naturalWidth > 0; })() && !document.querySelector('.message.user .message-avatar')").await?;
    client.refresh().await?;
    wait_for(client, "document.querySelector('.message.assistant')?.textContent.includes('Native reply: checkpoint five') && document.querySelectorAll('.message.user').length === 1").await?;

    new_session(client).await?;
    send(client, "cancel this response").await?;
    wait_for(client, "document.querySelector('.message.assistant')?.textContent.includes('Pending fixture response')").await?;
    click(client, ".send-btn.is-stop").await?;
    wait_for(
        client,
        "document.querySelector('.run-notice')?.textContent.includes('Stopped.')",
    )
    .await?;
    client.refresh().await?;
    wait_for(client, "document.querySelector('.rail-session-item.active') && document.querySelectorAll('.message').length === 0").await?;

    click(client, "#context-add").await?;
    wait_for(
        client,
        "document.querySelector('#context-picker')?.getClientRects().length > 0",
    )
    .await?;
    client.active_element().await?.send_keys("\u{e00c}").await?;
    wait_for(client, "document.activeElement?.id === 'context-add' && document.querySelector('#context-picker')?.getClientRects().length === 0").await?;
    wait_for(client, "document.documentElement.scrollWidth <= innerWidth").await?;
    wait_for(client, "document.querySelector('#metrics-state')?.dataset.state === 'live' && document.querySelectorAll('.metric-chart').length === 7").await?;
    std::fs::write(
        artifacts.join("native-desktop.png"),
        client.screenshot().await?,
    )?;
    Ok(())
}

async fn mobile_layout(client: &Client, origin: &str, artifacts: &std::path::Path) -> TestResult {
    client.goto(origin).await?;
    wait_for(client, "innerWidth === 390 && innerHeight === 844 && document.documentElement.scrollWidth <= innerWidth").await?;
    let prompt = client
        .wait()
        .at_most(Duration::from_secs(15))
        .for_element(Locator::Css("#prompt"))
        .await?;
    prompt.click().await?;
    wait_for(
        client,
        "document.querySelector('button[aria-label=\"Send\"]')?.getClientRects().length > 0",
    )
    .await?;
    std::fs::write(
        artifacts.join("native-mobile.png"),
        client.screenshot().await?,
    )?;
    Ok(())
}

async fn chat_lifecycle(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    new_session(client).await?;
    send(client, "hello world").await?;
    wait_for(client, "[...document.querySelectorAll('.message.assistant .message-body')].at(-1)?.textContent.includes('Native reply: hello world')").await?;

    new_session(client).await?;
    send(client, "first message").await?;
    wait_for(client, "[...document.querySelectorAll('.message.assistant')].at(-1)?.textContent.includes('Native reply: first message') && !document.querySelector('.send-btn.is-stop')").await?;
    send(client, "second message").await?;
    wait_for(client, "[...document.querySelectorAll('.message.assistant')].at(-1)?.textContent.includes('Native reply: second message') && document.querySelectorAll('.message.user').length === 2 && document.querySelectorAll('.message.assistant').length === 2").await?;

    new_session(client).await?;
    client
        .execute(
            "const prompt = document.querySelector('#prompt'); prompt.value = arguments[0]; prompt.dispatchEvent(new Event('input', {bubbles: true})); window.__llmupTestRun = true;",
            vec![json!("Formatting\n\n# Result\n\n- first\n- second\n\n```ts\nconst value = 1;\n```")],
        )
        .await?;
    client
        .find(Locator::Css("#prompt"))
        .await?
        .send_keys("\u{e009}\u{e007}\u{e000}")
        .await?;
    let structured = "(() => { const body = [...document.querySelectorAll('.message.assistant .message-body')].at(-1); const heading = body && [...body.querySelectorAll('h1,h2,h3,h4,h5,h6')].find((node) => node.textContent.trim() === 'Result'); return heading && heading.getClientRects().length > 0 && body.querySelectorAll('li').length === 2 && body.querySelector('pre code')?.textContent.includes('const value = 1;'); })()";
    wait_for(client, structured).await?;
    client.refresh().await?;
    wait_for(client, structured).await?;

    new_session(client).await?;
    send(client, "cancel this response").await?;
    click(client, ".send-btn.is-stop").await?;
    wait_for(client, "document.querySelector('.run-notice')?.textContent.includes('Stopped.') && document.querySelector('.retry-btn')?.getClientRects().length > 0").await
}

async fn set_update(client: &Client, kind: &str) -> TestResult {
    let status = client
        .execute_async(
            "const done = arguments[arguments.length - 1]; fetch('/__fixture/update', {method: 'POST', body: arguments[0]}).then((r) => done(r.status), () => done(0));",
            vec![json!(kind)],
        )
        .await?;
    if status != json!(204) {
        return Err(format!("fixture update control returned {status}").into());
    }
    Ok(())
}

async fn accessibility(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    wait_for(client, "document.querySelector('#prompt')?.tagName === 'TEXTAREA' && document.querySelector('#prompt').getAttribute('aria-label') === 'Message the local model' && document.querySelector('#prompt').getClientRects().length > 0 && document.querySelector('button[aria-label=\"Send\"]')?.getClientRects().length > 0 && document.querySelector('#messages')?.getAttribute('role') === 'log'").await?;

    new_session(client).await?;
    client
        .execute(
            "const status = document.querySelector('#a11y-status'); const seen = []; window.__announcements = seen; new MutationObserver(() => { if (status.textContent) seen.push(status.textContent); }).observe(status, {childList: true, characterData: true, subtree: true});",
            vec![],
        )
        .await?;
    send(client, "announce each streamed word").await?;
    wait_for(client, "document.querySelector('.message.assistant')?.textContent.includes('Native reply: announce each streamed word') && window.__announcements.at(-1) === 'Response ready.'").await?;
    let announcements = client
        .execute("return window.__announcements;", vec![])
        .await?;
    if announcements != json!(["Sending message.", "Response ready."]) {
        return Err(format!("status region repeated streamed content: {announcements}").into());
    }

    set_update(client, "available").await?;
    client.refresh().await?;
    wait_for(client, "(() => { const link = document.querySelector('#update-link'); return link && !link.hidden && link.getClientRects().length > 0 && link.textContent === 'Update to 0.12.0' && link.getAttribute('href') === 'https://github.com/shashankswe2020-ux/rigspark/releases' && link.target === '_blank' && link.rel === 'noopener noreferrer'; })()").await?;
    set_update(client, "unknown").await?;
    client.refresh().await?;
    wait_for(client, "document.querySelector('#prompt') && document.querySelector('#update-link')?.getClientRects().length === 0").await
}

async fn narrow_chat(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    wait_for(client, "innerWidth === 320 && document.querySelector('button[aria-label=\"Send\"]')?.getClientRects().length > 0").await?;
    send(client, "narrow").await?;
    wait_for(client, "[...document.querySelectorAll('.message.assistant')].at(-1)?.textContent.includes('Native reply: narrow')").await
}

async fn browser_errors(
    webdriver: &url::Url,
    client: &Client,
) -> Result<Vec<String>, Box<dyn Error>> {
    let session = client
        .session_id()
        .await?
        .ok_or("WebDriver session has no id")?;
    let body = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()?
        .post(webdriver.join(&format!("session/{session}/se/log"))?)
        .header("content-type", "application/json")
        .body(r#"{"type":"browser"}"#)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    Ok(severe_messages(&value["value"]).ok_or("unexpected browser log response")?)
}

// Matches the legacy harness: a missing favicon and user-initiated Stop aborts are expected.
fn severe_messages(entries: &serde_json::Value) -> Option<Vec<String>> {
    Some(
        entries
            .as_array()?
            .iter()
            .filter(|entry| entry["level"] == "SEVERE")
            .filter_map(|entry| entry["message"].as_str())
            .filter(|message| !message.contains("favicon") && !message.contains("ERR_ABORTED"))
            .filter(|message| {
                !(message.contains("http://127.0.0.1:48231/api/catalog/update")
                    && message.contains("502 (Bad Gateway)"))
            })
            .map(str::to_owned)
            .collect(),
    )
}

fn loopback_url(raw: &str) -> Result<url::Url, Box<dyn Error>> {
    let url = url::Url::parse(raw)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("test endpoints must be plain HTTP loopback roots".into());
    }
    Ok(url)
}

#[tokio::main]
async fn main() -> TestResult {
    let mut args = std::env::args().skip(1);
    let webdriver = loopback_url(&args.next().ok_or("expected WebDriver URL")?)?;
    let origin = loopback_url(&args.next().ok_or("expected fixture URL")?)?;
    let browser =
        PathBuf::from(args.next().ok_or("expected browser executable")?).canonicalize()?;
    let artifacts = PathBuf::from(args.next().ok_or("expected artifact directory")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    std::fs::create_dir_all(&artifacts)?;
    let mut capabilities = json!({
        "browserName": "chrome",
        "goog:chromeOptions": {
            "binary": browser,
            "args": ["--headless=new", "--disable-background-networking", "--no-first-run", "--no-default-browser-check"]
        },
        "goog:loggingPrefs": {"browser": "ALL"},
        "timeouts": {"pageLoad": 15000, "script": 15000}
    });
    for viewport in [None, Some((390, 844)), Some((320, 720)), Some((768, 1024))] {
        if let Some((width, height)) = viewport {
            capabilities["goog:chromeOptions"]["mobileEmulation"] = json!({
                "deviceMetrics": {"width": width, "height": height, "pixelRatio": 1}
            });
        }
        let client = tokio::time::timeout(
            Duration::from_secs(30),
            ClientBuilder::new(hyper_util::client::legacy::connect::HttpConnector::new())
                .capabilities(capabilities.as_object().unwrap().clone())
                .connect(webdriver.as_str()),
        )
        .await??;
        let result = tokio::time::timeout(Duration::from_secs(180), async {
            match viewport {
                None => {
                    client_units::run(&client).await?;
                    journeys(&client, origin.as_str(), &artifacts).await?;
                    chat_lifecycle(&client, origin.as_str()).await?;
                    accessibility(&client, origin.as_str()).await?;
                    formatting::desktop(&client, origin.as_str()).await?;
                    models::details(&client, origin.as_str(), &artifacts).await?;
                    models::bonsai(&client, origin.as_str(), 1440, &artifacts).await?;
                    models::tools(&client, origin.as_str()).await?;
                    models::workspace(&client, origin.as_str()).await?;
                    client.set_window_size(1280, 900).await?;
                    models::installed(&client, origin.as_str(), 1280, &artifacts).await?;
                }
                Some((390, _)) => {
                    mobile_layout(&client, origin.as_str(), &artifacts).await?;
                    formatting::viewport(&client, origin.as_str(), 390).await?;
                    models::narrow_details(&client, origin.as_str(), &artifacts).await?;
                    models::bonsai(&client, origin.as_str(), 390, &artifacts).await?;
                    models::installed(&client, origin.as_str(), 390, &artifacts).await?;
                }
                Some((320, _)) => {
                    narrow_chat(&client, origin.as_str()).await?;
                    formatting::viewport(&client, origin.as_str(), 320).await?;
                    models::bonsai(&client, origin.as_str(), 320, &artifacts).await?;
                }
                Some((width, _)) => {
                    formatting::viewport(&client, origin.as_str(), width).await?;
                    models::bonsai(&client, origin.as_str(), width, &artifacts).await?;
                }
            }
            let errors = browser_errors(&webdriver, &client).await?;
            if errors.is_empty() {
                Ok::<(), Box<dyn Error>>(())
            } else {
                Err(format!("unexpected console errors:\n{}", errors.join("\n")).into())
            }
        })
        .await;
        if !matches!(&result, Ok(Ok(())))
            && let Ok(Ok(screenshot)) =
                tokio::time::timeout(Duration::from_secs(5), client.screenshot()).await
        {
            let _ = std::fs::write(artifacts.join("failure.png"), screenshot);
        }
        let closed = tokio::time::timeout(Duration::from_secs(10), client.close()).await;
        result??;
        closed??;
    }
    println!(
        "PASS: native browser client modules, persistence, cancellation, keyboard, accessibility, update and layout journeys"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_runner_rejects_remote_or_credentialed_endpoints() {
        assert!(loopback_url("http://127.0.0.1:9515").is_ok());
        for url in [
            "https://127.0.0.1:9515",
            "http://localhost:9515",
            "http://example.com",
            "http://user@127.0.0.1",
            "http://127.0.0.1/path",
            "http://127.0.0.1/?query",
            "http://127.0.0.1/#fragment",
        ] {
            assert!(loopback_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn console_gate_keeps_severe_entries_except_expected_noise() {
        let entries = json!([
            {"level": "SEVERE", "message": "Uncaught TypeError: x is undefined"},
            {"level": "SEVERE", "message": "GET /favicon.ico 404"},
            {"level": "SEVERE", "message": "net::ERR_ABORTED /api/chat"},
            {"level": "WARNING", "message": "deprecated"},
            {"level": "INFO", "message": "ready"}
        ]);
        assert_eq!(
            severe_messages(&entries).unwrap(),
            vec!["Uncaught TypeError: x is undefined".to_owned()]
        );
        assert!(severe_messages(&json!({"value": []})).is_none());
    }
}
