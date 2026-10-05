use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use rigspark_cli::tui_generate::{GenerateView, ModelRow, drive, handle_key, model_rows, render};
use rigspark_core::{
    generation::{GenerationCatalog, GenerationKind, fit},
    sizing::Hardware,
};
use rigspark_runtime::generation::{
    Events, GenerationError, GenerationFuture, GenerationOutcome, Generator, NativeOptions,
    NativeOutcome,
};
use std::{path::PathBuf, sync::Mutex};
use tokio_util::sync::CancellationToken;

fn hardware(ram_gib: f64) -> Hardware {
    serde_json::from_value(serde_json::json!({"arch":"arm64","platform":"darwin",
        "totalRamBytes":ram_gib*1073741824.0,"freeRamBytes":ram_gib*1073741824.0,
        "freeDiskBytes":5e11,"gpu":[{"vendor":"apple","vramBytes":0}]}))
    .unwrap()
}
fn rows(ram_gib: f64) -> Vec<ModelRow> {
    model_rows(&GenerationCatalog::bundled().unwrap(), &hardware(ram_gib))
}
fn key(code: KeyCode) -> std::io::Result<Event> {
    Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}
fn typed(text: &str) -> Vec<std::io::Result<Event>> {
    text.chars().map(|ch| key(KeyCode::Char(ch))).collect()
}
fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Records the request, streams two progress lines, and reports a saved file.
#[derive(Default)]
struct Instant {
    seen: Mutex<Vec<NativeOptions>>,
}
impl Generator for Instant {
    fn generate(
        &self,
        options: NativeOptions,
        events: Events,
        _: CancellationToken,
    ) -> GenerationFuture<'_> {
        Box::pin(async move {
            events("Weights wan_2.1_vae.safetensors: 100% of 0.2 GiB".into());
            events("Queued ComfyUI prompt abc-123; generating video".into());
            let model = GenerationCatalog::bundled()
                .unwrap()
                .resolve(&options.model)
                .unwrap()
                .clone();
            let outcome = NativeOutcome {
                result: GenerationOutcome {
                    model: model.id.clone(),
                    kind: model.kind,
                    path: options.output_dir.join("rigspark-video-abc-123.webp"),
                    bytes: 10,
                    prompt_id: "abc-123".into(),
                    seed: options.seed.unwrap_or(1),
                    weights: Vec::new(),
                },
                fit: fit(&model, &hardware(36.0)),
            };
            self.seen.lock().unwrap().push(options);
            Ok(outcome)
        })
    }
}

/// Never finishes on its own; returns `Cancelled` once rigspark cancels it.
#[derive(Default)]
struct UntilCancelled {
    cancelled: Mutex<bool>,
}
impl Generator for UntilCancelled {
    fn generate(
        &self,
        _: NativeOptions,
        events: Events,
        cancel: CancellationToken,
    ) -> GenerationFuture<'_> {
        Box::pin(async move {
            events("Queued ComfyUI prompt slow-1; generating image".into());
            cancel.cancelled().await;
            *self.cancelled.lock().unwrap() = true;
            Err(GenerationError::Cancelled)
        })
    }
}

#[tokio::test]
async fn form_submits_selected_model_prompt_seed_and_streams_progress() {
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    let mut view = GenerateView::new(rows(36.0), PathBuf::from("/tmp/out"));
    view.select("video");
    view.directory = "/Users/me/ComfyUI".into();
    let mut script = typed("a fox in snow");
    script.push(key(KeyCode::Tab)); // directory
    script.push(key(KeyCode::Tab)); // seed
    script.extend(typed("42x"));
    script.push(key(KeyCode::Enter));
    script.push(key(KeyCode::Esc));
    let generator = Instant::default();
    let code = drive(
        &mut terminal,
        &mut futures_util::stream::iter(script),
        &generator,
        &mut view,
        false,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    let seen = generator.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].model, "wan2.1-t2v:1.3b");
    assert_eq!(seen[0].prompt, "a fox in snow");
    assert_eq!(
        seen[0].seed,
        Some(42),
        "non-digits are ignored in the seed field"
    );
    assert_eq!(seen[0].comfyui_dir, PathBuf::from("/Users/me/ComfyUI"));
    let saved = PathBuf::from("/tmp/out").join("rigspark-video-abc-123.webp");
    assert_eq!(view.saved, vec![saved.clone()]);
    // Native separators: Windows renders the joined component with `\`.
    assert!(
        view.status_text()
            .starts_with(&format!("Saved video: {}", saved.display()))
    );
    let log: Vec<_> = view.log_lines().cloned().collect();
    assert!(
        log.iter()
            .any(|line| line.contains("Queued ComfyUI prompt abc-123"))
    );
    let text = screen(&terminal);
    assert!(text.contains("rigspark / generate"), "{text}");
    assert!(text.contains("Saved video"), "{text}");
}

#[tokio::test]
async fn escape_cancels_a_running_generation_then_exits() {
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut view = GenerateView::new(rows(36.0), PathBuf::from("/tmp"));
    view.directory = "/Users/me/ComfyUI".into();
    let mut script = typed("lighthouse");
    script.push(key(KeyCode::Enter));
    script.push(key(KeyCode::Esc)); // cancel while running
    script.push(key(KeyCode::Esc)); // exit form
    let generator = UntilCancelled::default();
    let code = drive(
        &mut terminal,
        &mut futures_util::stream::iter(script),
        &generator,
        &mut view,
        false,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    assert!(*generator.cancelled.lock().unwrap());
    assert_eq!(view.status_text(), "Cancelled");
    assert!(view.saved.is_empty());
}

#[tokio::test]
async fn invalid_forms_never_reach_the_generator() {
    let generator = Instant::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    // Missing prompt, then missing directory.
    let mut view = GenerateView::new(rows(36.0), PathBuf::from("/tmp"));
    let mut script = vec![key(KeyCode::Enter)];
    script.extend(typed("x"));
    script.push(key(KeyCode::Enter));
    script.push(key(KeyCode::Esc));
    drive(
        &mut terminal,
        &mut futures_util::stream::iter(script),
        &generator,
        &mut view,
        false,
    )
    .await
    .unwrap();
    assert!(screen(&terminal).contains("enter your ComfyUI directory"));
    // A model that does not fit is refused unless bypass was requested.
    let mut view = GenerateView::new(rows(16.0), PathBuf::from("/tmp"));
    view.select("image");
    view.directory = "/c".into();
    view.prompt = "p".into();
    assert!(view.options().unwrap_err().contains("--bypass"));
    view.bypass = true;
    assert_eq!(view.options().unwrap().model, "flux1-schnell:fp8");
    assert!(generator.seen.lock().unwrap().is_empty());
}

#[test]
fn keys_navigate_models_and_ctrl_c_interrupts_when_idle() {
    let mut view = GenerateView::new(rows(36.0), PathBuf::from("/tmp"));
    assert_eq!(view.rows[view.selected].kind, GenerationKind::Image.name());
    handle_key(
        &mut view,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
    );
    handle_key(&mut view, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(view.rows[view.selected].id, "flux1-schnell:fp16");
    handle_key(&mut view, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(view.rows[view.selected].id, "wan2.1-t2v:1.3b");
    handle_key(
        &mut view,
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
    );
    assert!(
        view.prompt.is_empty(),
        "typing on the model list is ignored"
    );
    assert_eq!(
        handle_key(
            &mut view,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        rigspark_cli::tui_generate::Action::Exit(130)
    );
}

#[test]
fn render_shows_models_fit_and_unknown_speed_at_small_sizes() {
    for (width, height) in [(120, 32), (80, 24), (60, 20)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut view = GenerateView::new(rows(16.0), PathBuf::from("/tmp"));
        view.prompt = "a lighthouse\u{1b}[31m".into();
        terminal.draw(|frame| render(frame, &view, true)).unwrap();
        let text = screen(&terminal);
        assert!(
            text.contains("flux1-schnell:fp8"),
            "{width}x{height}\n{text}"
        );
        assert!(text.contains("wan2.1-t2v:1.3b"), "{text}");
        assert!(text.contains("Prompt"), "{text}");
        assert!(!text.contains('\u{1b}'));
        if width >= 80 {
            assert!(text.contains("speed unknown"), "{text}");
            assert!(text.contains("no") && text.contains("yes"), "{text}");
        }
    }
}
