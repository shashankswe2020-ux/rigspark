use super::{TestResult, click, fresh_session, send, type_into, wait_for};
use fantoccini::Client;
use serde_json::json;

// Mirrors chat.js `messageScroller()`: the log scrolls itself once it overflows.
const SCROLLER: &str = "(() => { const log = document.querySelector('#messages'); return log.scrollHeight > log.clientHeight + 1 ? log : document.scrollingElement; })()";

const LAST_REPLY: &str =
    "[...document.querySelectorAll('.message.assistant .message-body')].at(-1)";

async fn failures(client: &Client, label: &str, checks: &str) -> TestResult {
    let script = format!(
        "const body = {LAST_REPLY}; const failed = []; const check = (name, ok) => {{ if (!ok) failed.push(name); }}; const count = (selector) => body.querySelectorAll(selector).length; {checks}; return failed;"
    );
    // Late layout work (image decode, math, wrapping) can trail the settled reply under load, so a
    // check only fails if it is still failing after a bounded settle window.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let failed = client.execute(&script, vec![]).await?;
        if failed == json!([]) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("{label} failed: {failed}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn number(client: &Client, expression: &str) -> Result<f64, Box<dyn std::error::Error>> {
    client
        .execute(&format!("return {expression};"), vec![])
        .await?
        .as_f64()
        .ok_or_else(|| format!("{expression} is not a number").into())
}

async fn formatted_reply(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    fresh_session(client).await?;
    send(client, "FORMAT_MARKDOWN").await?;
    wait_for(client, "document.querySelectorAll('.message.assistant .code-copy').length === 3 && !document.querySelector('.message.assistant.streaming')").await
}

async fn stress_reply(client: &Client, origin: &str) -> TestResult {
    client.goto(origin).await?;
    fresh_session(client).await?;
    send(client, "FORMAT_STRESS_STREAM").await?;
    wait_for(client, "(() => { const body = [...document.querySelectorAll('.message.assistant .message-body')].at(-1); return body?.textContent.includes('Response rendering stress') && !body.closest('.message').classList.contains('streaming'); })()").await
}

async fn stress_affordances(client: &Client, viewport: &str) -> TestResult {
    failures(client, &format!("{viewport} stress rendering"), r#"
        check('unicode', body.textContent.includes('café · Ελληνικά · العربية · हिन्दी · বাংলা · 日本語 · 汉字 · 한글'));
        check('emoji', body.textContent.includes('👩🏽 🚀 🏳️ ✅'));
        check('nested list', count('ul ul ul ul li') === 1);
        check('wide table', count('.markdown-table-wrap > table') === 1 && body.querySelector('.markdown-table-wrap').scrollWidth > body.querySelector('.markdown-table-wrap').clientWidth);
        check('long code', count('pre code') === 1 && body.querySelector('pre').scrollWidth > body.querySelector('pre').clientWidth);
        check('display math', count('.katex-display') === 1 && body.querySelector('.katex-display').textContent.includes('π'));
        const inlineImage = body.querySelector('img.chat-image[src^="data:image/png;base64,"]');
        const imageBox = inlineImage?.getBoundingClientRect();
        check(inlineImage ? `inline image ${imageBox.width}x${imageBox.height} complete=${inlineImage.complete}` : `inline image missing (${count('img')} img)`, inlineImage && imageBox.width >= 32 && imageBox.height >= 32);
        check('unsafe nodes', count('iframe, [onclick], #stressXss') === 0 && globalThis.__stressXss !== true);
        const rawHtml = [...body.querySelectorAll('code.raw-html')];
        check('unsafe source formatting', rawHtml.length >= 2 && rawHtml.every((node) => /<\/?(?:iframe|div)/u.test(node.textContent)));
        check('author style', [...body.querySelectorAll('[style]')].every((node) => node.closest('.katex')));
        check('malformed readable', body.textContent.includes('$$ unmatched display') && body.textContent.includes('\\(unmatched inline'));
        check('malformed formatting', count('code.unmatched-math-delimiter') === 2);
        check('actions', body.closest('.message').querySelectorAll('.msg-action').length === 2);
        check('metrics width', Math.abs(body.closest('.message').querySelector('.msg-metrics').clientWidth - body.clientWidth) < 2);
        check('page overflow', document.documentElement.scrollWidth <= innerWidth);
        // Visually hidden role labels must stay inside their message, or they escape the log's
        // clipping and stretch the page (seen as phantom page scroll on slow runners).
        check('role labels contained', [...document.querySelectorAll('.message .message-role')].every((node) => getComputedStyle(node).position !== 'absolute' || node.offsetParent === node.closest('.message')));
        const bodyRect = body.getBoundingClientRect();
        check('message body bounds', bodyRect.left >= 0 && bodyRect.right <= innerWidth + 1);
        for (const selector of ['.markdown-table-wrap', 'pre', '.katex-display', '.chat-image']) {
            const rect = body.querySelector(selector)?.getBoundingClientRect();
            check(selector + ' bounds', rect && rect.left >= 0 && rect.right <= innerWidth + 1);
        }
    "#).await
}

async fn held_log(client: &Client) -> Result<f64, Box<dyn std::error::Error>> {
    client
        .execute(
            "const style = document.createElement('style'); style.textContent = '#messages { flex: 0 0 18rem; min-height: 0; }'; document.head.append(style);",
            vec![],
        )
        .await?;
    wait_for(client, "document.querySelector('#messages').scrollHeight > document.querySelector('#messages').clientHeight").await?;
    client
        .execute(
            "document.querySelector('#messages').scrollTop = 80;",
            vec![],
        )
        .await?;
    number(client, "document.querySelector('#messages').scrollTop").await
}

fn near(label: &str, actual: f64, held: f64) -> TestResult {
    if (actual - held).abs() < 8.0 {
        Ok(())
    } else {
        Err(format!("{label}: scroll moved from {held} to {actual}").into())
    }
}

async fn affordances(client: &Client, viewport: &str) -> TestResult {
    failures(client, &format!("{viewport} affordances"), "
        check('languages', JSON.stringify([...body.querySelectorAll('.code-language')].map((node) => node.textContent.trim())) === JSON.stringify(['typescript', 'bash', 'html']));
        check('preview', count('.code-preview') === 1);
        check('copy', count('.code-copy') === 3);
        const copy = body.querySelector('.code-copy');
        const table = body.querySelector('.markdown-table-wrap');
        const metrics = body.closest('.message').querySelector('.msg-metrics');
        check('measure', body.getBoundingClientRect().width / Number.parseFloat(getComputedStyle(body).fontSize) <= 80);
        check('copy opacity', Number.parseFloat(getComputedStyle(copy).opacity) === 1);
        check('table overflow', table.scrollWidth >= table.clientWidth);
        check('metrics column', getComputedStyle(metrics).gridColumnStart === '2' && Math.abs(metrics.clientWidth - body.clientWidth) < 2);
        check('page overflow', document.documentElement.scrollWidth <= innerWidth)
    ").await
}

pub async fn desktop(client: &Client, origin: &str) -> TestResult {
    formatted_reply(client, origin).await?;
    failures(client, "GFM semantics", r#"
        const user = [...document.querySelectorAll('.message.user .message-body')].at(-1);
        const heading = (level, name) => [...body.querySelectorAll('h' + level)].some((node) => node.textContent.trim() === name && node.getClientRects().length > 0);
        check('h1', heading(1, 'Deployment result'));
        check('h2', heading(2, 'Checklist'));
        check('nested', count('ul ul li') === 2);
        check('checkboxes', count('input[type="checkbox"]') === 2);
        check('tasks', count('.contains-task-list > .task-list-item') === 2);
        check('task marker', getComputedStyle(body.querySelector('.task-list-item')).listStyleType === 'none');
        check('quote', count('blockquote p') === 2);
        check('th', count('table thead th') === 3);
        check('tr', count('table tbody tr') === 2);
        check('display math', count('.katex-display') === 4 && body.textContent.includes('LCM'));
        check('text command', body.querySelector('.katex-display .katex-html')?.textContent.startsWith('LCM'));
        check('boxed answer', [...body.querySelectorAll('.katex-display')].some((node) => node.textContent.includes('232892560') && node.querySelector('.boxpad')));
        check('inline math', count('.katex:not(.katex-display .katex)') >= 2);
        check('aligned math', [...body.querySelectorAll('.katex-display')].some((node) => node.textContent.includes('144')));
        const modelMath = [...body.querySelectorAll('.katex-display')].at(-1);
        check('model line breaks', modelMath?.querySelectorAll('.mtable .vlist-r').length >= 3);
        check('malformed math', body.textContent.includes('\\notARealCommand'));
        check('math source hidden', !body.textContent.includes('$$'));
        check('hr', count('hr') === 1);
        check('ts', body.querySelector('pre code.language-typescript')?.textContent.includes('greet'));
        check('bash', body.querySelector('pre code.language-bash')?.textContent.includes('npm run build'));
        check('html', body.querySelector('pre code.language-html')?.textContent.includes('<main'));
        check('del', body.querySelector('del')?.textContent === 'removed text');
        check('link rel', body.querySelector('a[href="https://example.com/docs?q=local"]')?.getAttribute('rel') === 'noopener noreferrer');
        check('data image', count('img[src^="data:image/png;base64,"]') === 1);
        check('local image', count('img[src="/api/images/formatting.png"]') === 1);
        for (const selector of ['img[src^="https://"]', 'img[src^="file:"]', 'img[src^="data:text/html"]', 'a[href^="javascript:"]', 'a[href^="data:"]', '#unsafe, script, svg, [onload], [onerror], [onmouseover]']) {
            check(selector, count(selector) === 0);
        }
        check('author style', [...body.querySelectorAll('[style]')].every((node) => node.closest('.katex')));
        for (const text of ['<button id="unsafe"', 'Remote image', 'File image', 'HTML data image']) {
            check(text, body.textContent.includes(text));
        }
        check('xss', globalThis.__xss !== true);
        check('user plain', user.querySelectorAll('h1, ul, pre, table').length === 0 && user.textContent.trim() === 'FORMAT_MARKDOWN')
    "#).await?;

    failures(client, "semantic structure", "
        check('h1', count('h1') === 1);
        check('h2', count('h2') === 1);
        check('lists', count('ul, ol') === 4);
        check('items', count('li') === 8);
        check('table', count('table') === 1);
        check('blockquote', count('blockquote') === 1);
        for (const name of ['Copy code', 'Preview HTML code']) {
            const button = [...body.querySelectorAll('button')].find((node) => (node.getAttribute('aria-label') || node.textContent).includes(name));
            button?.focus();
            check(name + ' focus', button && document.activeElement === button && getComputedStyle(button).outlineStyle !== 'none');
        }
    ").await?;

    let original = client
        .execute(&format!("return {LAST_REPLY}.textContent;"), vec![])
        .await?;
    client.refresh().await?;
    wait_for(
        client,
        &format!("{LAST_REPLY}?.querySelectorAll('table').length === 1"),
    )
    .await?;
    let restored = client
        .execute(&format!("const body = {LAST_REPLY}; return body.querySelectorAll('blockquote').length === 1 ? body.textContent : null;"), vec![])
        .await?;
    if restored != original
        || !original
            .as_str()
            .is_some_and(|text| text.contains("Deployment result"))
    {
        return Err("restored formatted reply differs from the original".into());
    }

    let expected = client
        .execute(&format!("return {LAST_REPLY}.innerHTML;"), vec![])
        .await?;
    fresh_session(client).await?;
    send(client, "FORMAT_MARKDOWN_STREAM").await?;
    wait_for(client, &format!("{LAST_REPLY}?.querySelectorAll('table').length === 1 && {LAST_REPLY}.querySelectorAll('.code-copy').length === 3 && !document.querySelector('.message.assistant.streaming')")).await?;
    let streamed = client
        .execute(&format!("return {LAST_REPLY}.innerHTML;"), vec![])
        .await?;
    if streamed != expected {
        return Err("streamed Markdown did not converge to the complete DOM".into());
    }

    formatted_reply(client, origin).await?;
    let held = held_log(client).await?;
    send(client, "FORMAT_MARKDOWN_SCROLL").await?;
    wait_for(client, "[...document.querySelectorAll('.message.assistant.streaming')].at(-1)?.getClientRects().length > 0").await?;
    if number(client, "document.querySelector('#messages').scrollTop").await? != held {
        return Err("message log moved when streaming started".into());
    }
    wait_for(
        client,
        "!document.querySelector('.message.assistant.streaming')",
    )
    .await?;
    near(
        "log after streaming",
        number(client, "document.querySelector('#messages').scrollTop").await?,
        held,
    )?;

    formatted_reply(client, origin).await?;
    let held = held_log(client).await?;
    send(client, "cancel this response").await?;
    click(client, ".send-btn.is-stop").await?;
    wait_for(
        client,
        "document.querySelector('.run-notice')?.textContent.includes('Stopped.')",
    )
    .await?;
    near(
        "cancellation notice",
        number(client, "document.querySelector('#messages').scrollTop").await?,
        held,
    )?;

    client.goto(origin).await?;
    fresh_session(client).await?;
    send(client, "FORMAT_MARKDOWN_INCOMPLETE").await?;
    wait_for(client, "(() => { const row = [...document.querySelectorAll('.message.assistant')].at(-1); return row?.classList.contains('streaming') && row.querySelector('pre code.language-typescript')?.textContent.includes('const') && row.querySelectorAll('.code-copy').length === 0; })()").await?;
    wait_for(client, "(() => { const row = [...document.querySelectorAll('.message.assistant')].at(-1); return row && !row.classList.contains('streaming') && row.querySelector('pre code.language-typescript')?.textContent.includes('const value = 1;') && row.querySelectorAll('.code-copy').length === 1 && row.textContent.includes('After the code.'); })()").await?;

    formatted_reply(client, origin).await?;
    click(client, ".message.assistant .code-preview").await?;
    wait_for(client, "(() => { const frame = document.querySelector('#artifact-frame'); return document.querySelector('#artifact-modal')?.getClientRects().length > 0 && frame.getAttribute('sandbox') === '' && /<main class=\"status\">Ready<\\/main>/u.test(frame.getAttribute('srcdoc') || ''); })()").await?;
    click(client, "#artifact-close").await?;
    wait_for(client, "document.querySelector('#artifact-modal')?.getClientRects().length === 0 && document.activeElement?.classList.contains('code-preview')").await?;

    client.set_window_size(1440, 900).await?;
    formatted_reply(client, origin).await?;
    affordances(client, "desktop").await?;
    stress_reply(client, origin).await?;
    stress_affordances(client, "desktop").await
}

pub async fn viewport(client: &Client, origin: &str, width: u32) -> TestResult {
    formatted_reply(client, origin).await?;
    affordances(client, &format!("{width}px")).await?;
    stress_reply(client, origin).await?;
    stress_affordances(client, &format!("{width}px")).await?;
    if width != 390 {
        return Ok(());
    }
    failures(
        client,
        "narrow tables",
        "
        check('wrapped table', count('.markdown-table-wrap > table') === 1);
        check('page overflow', document.documentElement.scrollWidth <= 390)
    ",
    )
    .await?;

    type_into(client, "#prompt", "FORMAT_MARKDOWN_SCROLL", true).await?;
    client
        .execute(&format!("const scroller = {SCROLLER}; scroller.scrollTop = scroller.scrollHeight; window.__llmupTestRun = true;"), vec![])
        .await?;
    type_into(client, "#prompt", "\u{e007}", false).await?;
    wait_for(client, "[...document.querySelectorAll('.message.assistant')].at(-1)?.classList.contains('streaming')").await?;
    wait_for(client, &format!("(() => {{ const scroller = {SCROLLER}; return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 8; }})()")).await?;
    client
        .execute(&format!("{SCROLLER}.scrollTop = 200;"), vec![])
        .await?;
    let held = number(client, &format!("{SCROLLER}.scrollTop")).await?;
    wait_for(
        client,
        "!document.querySelector('.message.assistant.streaming')",
    )
    .await?;
    near(
        "reader position",
        number(client, &format!("{SCROLLER}.scrollTop")).await?,
        held,
    )?;
    failures(
        client,
        "narrow page scroll",
        "check('page scroll', document.scrollingElement.scrollHeight <= document.scrollingElement.clientHeight + 1)",
    )
    .await
}
