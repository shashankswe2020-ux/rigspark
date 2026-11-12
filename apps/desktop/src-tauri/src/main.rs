use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_updater::UpdaterExt as _;

/// Argument the login item passes so a login launch starts in the menu bar only.
#[cfg(not(test))]
const BACKGROUND_ARG: &str = "--background";
/// Commands the launch document may invoke; each re-checks window and URL.
const DESKTOP_COMMANDS: [&str; 7] = [
    "allow-select-workspace-directory",
    "allow-desktop-status",
    "allow-set-launch-at-login",
    "allow-check-for-update",
    "allow-install-update",
    "allow-open-github-sign-in",
    "allow-open-github-repository-choice",
];
/// The only page `open_github_sign_in` opens: GitHub's device-flow code entry.
const GITHUB_DEVICE_PAGE: &str = "https://github.com/login/device";
/// The only page `open_github_repository_choice` opens: where people choose the
/// repositories the RigSpark Community GitHub App may reach.
const GITHUB_INSTALL_PAGE: &str = rigspark_runtime::source_control::INSTALL_PAGE;

struct HostState(Arc<rigspark_gui::Host>);
enum DirectoryPicker {
    #[cfg(not(test))]
    Native {
        report: bool,
        requests: std::sync::atomic::AtomicUsize,
    },
    #[cfg(test)]
    Fixture(Option<std::path::PathBuf>),
}
impl DirectoryPicker {
    async fn pick(&self) -> Option<std::path::PathBuf> {
        match self {
            #[cfg(not(test))]
            Self::Native { report, requests } => {
                let mut dialog =
                    rfd::AsyncFileDialog::new().set_title("Choose workspace directory");
                if *report {
                    let Some(directory) = std::env::var_os("LLMUP_DIALOG_SMOKE_PATH")
                        .and_then(|path| std::fs::canonicalize(path).ok())
                        .filter(|path| path.is_dir())
                    else {
                        eprintln!("R22 test directory is unavailable");
                        return None;
                    };
                    let number = requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    dialog = dialog
                        .set_directory(directory)
                        .set_title(format!("Choose workspace directory R22 {number}"));
                    println!("R22 directory picker requested");
                }
                let selected = dialog
                    .pick_folder()
                    .await
                    .map(|entry| entry.path().to_path_buf());
                if *report && let Some(path) = &selected {
                    let expected = std::env::var_os("LLMUP_DIALOG_SMOKE_PATH")
                        .and_then(|path| std::fs::canonicalize(path).ok());
                    if expected.is_none() || std::fs::canonicalize(path).ok() != expected {
                        eprintln!("R22 picker selected an unexpected directory");
                        return None;
                    }
                }
                selected
            }
            #[cfg(test)]
            Self::Fixture(path) => path.clone(),
        }
    }
}
fn desktop_capability(entry: &str) -> String {
    let root_pattern = format!("{entry}{{}}");
    serde_json::json!({"identifier":"main-desktop","windows":["main"],"local":false,"remote":{"urls":[root_pattern]},"permissions":DESKTOP_COMMANDS}).to_string()
}
fn authorized_url(url: &url::Url, origin: &str) -> bool {
    url.origin().ascii_serialization() == origin
        && url.path() == "/"
        && url.query().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}
fn authorize<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    state: &HostState,
) -> Result<(), String> {
    if window.label() != "main"
        || !authorized_url(
            &window.url().map_err(|_| "window unavailable")?,
            &state.0.origin(),
        )
    {
        return Err("desktop features are unavailable to this document".into());
    }
    Ok(())
}
#[tauri::command]
async fn select_workspace_directory<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
    picker: tauri::State<'_, DirectoryPicker>,
) -> Result<Option<String>, String> {
    authorize(&window, &state)?;
    Ok(picker
        .pick()
        .await
        .map(|path| path.to_string_lossy().into_owned()))
}
/// Opens GitHub's device sign-in page in the default browser. The window blocks other
/// origins and new windows, and the page takes no argument, so nothing else can open.
#[tauri::command]
async fn open_github_sign_in<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
) -> Result<(), String> {
    authorize(&window, &state)?;
    open_in_browser(GITHUB_DEVICE_PAGE)
}
/// Opens the page where people pick the repositories the GitHub App may reach. Like
/// sign-in, it takes no argument, so it opens only that one fixed page.
#[tauri::command]
async fn open_github_repository_choice<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
) -> Result<(), String> {
    authorize(&window, &state)?;
    open_in_browser(GITHUB_INSTALL_PAGE)
}
fn open_in_browser(page: &'static str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(page)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|_| "could not open the browser".to_string())
}
#[tauri::command]
async fn desktop_status<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
) -> Result<serde_json::Value, String> {
    authorize(&window, &state)?;
    let app = window.app_handle();
    Ok(serde_json::json!({
        "version": app.package_info().version.to_string(),
        "launchAtLogin": app.autolaunch().is_enabled().unwrap_or(false),
    }))
}
#[tauri::command]
async fn set_launch_at_login<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
    enabled: bool,
) -> Result<bool, String> {
    authorize(&window, &state)?;
    let launcher = window.app_handle().autolaunch();
    if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    }
    .map_err(
        |_| "the login item could not be changed; check System Settings > General > Login Items",
    )?;
    launcher
        .is_enabled()
        .map_err(|_| "the login item state is unavailable".into())
}
#[tauri::command]
async fn check_for_update<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
) -> Result<Option<serde_json::Value>, String> {
    authorize(&window, &state)?;
    let update = window
        .app_handle()
        .updater()
        .map_err(|_| "updates are unavailable in this build")?
        .check()
        .await
        .map_err(|_| "could not check for updates")?;
    Ok(update.map(|update| {
        serde_json::json!({
            "version": rigspark_core::reports::strip_control(&update.version),
            "notes": update.body.as_deref().map(rigspark_core::reports::strip_control),
        })
    }))
}
#[tauri::command]
async fn install_update<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
) -> Result<(), String> {
    authorize(&window, &state)?;
    let app = window.app_handle().clone();
    let update = app
        .updater()
        .map_err(|_| "updates are unavailable in this build")?
        .check()
        .await
        .map_err(|_| "could not check for updates")?
        .ok_or("RigSpark is up to date")?;
    // The updater verifies the release signature against the bundled public key.
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|_| "the update could not be verified or installed")?;
    app.restart()
}
#[cfg(not(test))]
fn show_main<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
#[cfg(not(test))]
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    let open = MenuItem::with_id(app, "open", "Open RigSpark", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", "Check for Updates…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit RigSpark", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &update,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let mut tray = tauri::tray::TrayIconBuilder::with_id("main")
        .tooltip("RigSpark")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "update" => {
                show_main(app);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.eval("window.dispatchEvent(new Event('rigspark:check-update'))");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });
    // One hood, black on clear: macOS tints it for the menu bar. RigSpark Crew shows three,
    // so the two apps can be told apart there.
    match tauri::image::Image::from_bytes(TRAY_ICON) {
        Ok(icon) => tray = tray.icon(icon).icon_as_template(true),
        Err(_) => {
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
        }
    }
    tray.build(app)?;
    Ok(())
}
/// The menu bar icon (a template image, 22 pt at 2x).
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-template.png");

#[cfg(not(test))]
fn main() {
    let dialog_smoke = std::env::args().any(|argument| argument == "--dialog-smoke-test");
    let smoke = dialog_smoke || std::env::args().any(|argument| argument == "--smoke-test");
    let background = !smoke && std::env::args().any(|argument| argument == BACKGROUND_ARG);
    rigspark_runtime::process_control::enable_package_manager_fallback();
    if let Some(dir) = bundled_runtime_dir() {
        let _ = rigspark_runtime::process_control::register_bundled_runtime_dir(dir);
    }
    let runtime = tokio::runtime::Runtime::new().expect("native async runtime");
    // A failure here used to panic before any window existed, so the app bounced in the
    // Dock and vanished. Now it is logged and shown, and a left-behind lock can be cleared.
    let (listener, host) = loop {
        match start_host(&runtime) {
            Ok(started) => break started,
            Err(failure) => {
                log_startup_failure(&failure);
                if smoke || background || !offer_recovery(&failure) {
                    std::process::exit(1);
                }
            }
        }
    };
    host.desktop
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let serving = runtime.spawn(rigspark_gui::serve(listener, host.clone()));
    let origin = host.origin();
    let entry = format!("{origin}/");
    let launch_host = host.clone();
    let app = tauri::Builder::default()
        .manage(HostState(host.clone()))
        .manage(DirectoryPicker::Native { report: dialog_smoke, requests: Default::default() })
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            select_workspace_directory,
            desktop_status,
            set_launch_at_login,
            check_for_update,
            install_update,
            open_github_sign_in,
            open_github_repository_choice
        ])
        .setup(move |app| {
            if !smoke
                && let Ok(dir) = app.path().app_config_dir()
                && first_run(&dir)
            {
                // Ready to go after install: start in the menu bar at login until the user opts out.
                let _ = app.autolaunch().enable();
            }
            app.add_capability(desktop_capability(&entry))?;
            let script = format!("if(window===window.top && location.origin==={} && location.pathname==='/'){{const invoke=(command,args)=>window.__TAURI_INTERNALS__.invoke(command,args);Object.defineProperty(window,'llmupDesktop',{{value:Object.freeze({{selectWorkspaceDirectory:()=>invoke('select_workspace_directory'),status:()=>invoke('desktop_status'),setLaunchAtLogin:(enabled)=>invoke('set_launch_at_login',{{enabled:enabled===true}}),checkForUpdate:()=>invoke('check_for_update'),installUpdate:()=>invoke('install_update'),openGitHubSignIn:()=>invoke('open_github_sign_in'),openGitHubRepositoryChoice:()=>invoke('open_github_repository_choice')}}),writable:false,configurable:false}});}}",serde_json::to_string(&origin)?);
            let allowed = origin.clone();
            let navigation_app=app.handle().clone();
            let smoke_app=app.handle().clone();
            if smoke { tauri::async_runtime::spawn(async move { tokio::time::sleep(std::time::Duration::from_secs(if dialog_smoke { 60 } else { 20 })).await; smoke_app.exit(1); }); }
            tauri::WebviewWindowBuilder::new(app,"main",tauri::WebviewUrl::External(entry.parse()?))
                .title("RigSpark").inner_size(1280.0,840.0).min_inner_size(760.0,540.0)
                .visible(!background)
                // Matches the workspace's dark window colour so there is no white flash before paint.
                .background_color(tauri::window::Color(28,28,30,255))
                .initialization_script(script)
                .on_navigation(move |url| {
                    if smoke && url.origin().ascii_serialization()==allowed && url.path().starts_with("/__native_smoke/") {
                        let passed=url.path()=="/__native_smoke/pass";
                        println!("Native WebView frontend/bridge smoke: {}",if passed{"passed"}else{"failed"});
                        navigation_app.exit(if passed{0}else{1});
                        return false;
                    }
                    authorized_url(url,&allowed)
                })
                .on_page_load(move |window,payload| {
                    if smoke && matches!(payload.event(),tauri::webview::PageLoadEvent::Finished) {
                        let _=window.eval(if dialog_smoke { include_str!("dialog-smoke.js") } else { "location.href='/__native_smoke/'+(document.title==='RigSpark' && document.querySelector('main') && document.querySelector('textarea') && typeof window.llmupDesktop?.selectWorkspaceDirectory==='function' && ['status','setLaunchAtLogin','checkForUpdate','installUpdate','openGitHubSignIn','openGitHubRepositoryChoice'].every((name)=>typeof window.llmupDesktop?.[name]==='function') && !document.querySelector('#desktop-settings')?.hidden && typeof window.__TAURI_INTERNALS__?.invoke==='function' ? 'pass':'fail')" });
                    }
                })
                .on_new_window(|_,_|tauri::webview::NewWindowResponse::Deny)
                .on_download(|_,_|false)
                .build()?;
            let closing = launch_host.clone();
            let window = app.get_webview_window("main").expect("main window");
            let hidden = window.clone();
            // Closing the window keeps serving from the menu bar; Quit stops everything.
            window.on_window_event(move |event| match event {
                tauri::WindowEvent::CloseRequested { api, .. } if !smoke => {
                    api.prevent_close();
                    let _ = hidden.hide();
                }
                tauri::WindowEvent::Destroyed => closing.shutdown.cancel(),
                _ => {}
            });
            if !smoke {
                build_tray(app)?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|cause| {
            let failure = StartupFailure {
                stage: "open the RigSpark window",
                detail: cause.to_string(),
                lock: None,
            };
            log_startup_failure(&failure);
            if !smoke && !background {
                offer_recovery(&failure);
            }
            std::process::exit(1)
        });
    app.run(move |handle, event| match event {
        tauri::RunEvent::Exit => host.shutdown.cancel(),
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => show_main(handle),
        _ => {
            let _ = handle;
        }
    });
    let _ = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), serving).await
    });
}

/// Why the desktop could not start, in words a person can act on.
#[derive(Debug)]
struct StartupFailure {
    stage: &'static str,
    detail: String,
    /// Set when another process holds, or a crashed one left, the state lock.
    lock: Option<std::path::PathBuf>,
}
impl StartupFailure {
    #[cfg(not(test))]
    fn new(stage: &'static str, cause: impl std::fmt::Display) -> Self {
        Self {
            stage,
            detail: cause.to_string(),
            lock: None,
        }
    }
    fn message(&self) -> String {
        let mut text = format!("RigSpark could not {}.\n\n{}", self.stage, self.detail);
        if let Some(lock) = &self.lock {
            text.push_str(&format!(
                "\n\nThe state lock {} is held by another RigSpark process, or was left behind \
                 when one quit unexpectedly. If `rigspark` or `llmup` is running in a terminal, \
                 let it finish or quit it, then try again. Otherwise you can clear the lock.",
                lock.display()
            ));
        }
        text.push_str(&format!(
            "\n\nDetails are saved in {}.",
            startup_log_path().display()
        ));
        text
    }
}
#[cfg(not(test))]
type Started = (tokio::net::TcpListener, std::sync::Arc<rigspark_gui::Host>);
#[cfg(not(test))]
fn start_host(runtime: &tokio::runtime::Runtime) -> Result<Started, StartupFailure> {
    let config = rigspark_runtime::state::Config::load()
        .map_err(|cause| StartupFailure::new("find its data folder", cause))?;
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind((
            std::net::Ipv4Addr::LOCALHOST,
            0,
        )))
        .map_err(|cause| StartupFailure::new("open a local port on 127.0.0.1", cause))?;
    let port = listener
        .local_addr()
        .map_err(|cause| StartupFailure::new("open a local port on 127.0.0.1", cause))?
        .port();
    let host = rigspark_gui::Host::new(&config.home, port)
        .map_err(|cause| host_failure(cause.as_ref(), &config))?;
    Ok((listener, host))
}
/// Names the data folder in the message, and marks a lock failure so it can be cleared.
fn host_failure(
    cause: &(dyn std::error::Error + 'static),
    config: &rigspark_runtime::state::Config,
) -> StartupFailure {
    let locked = cause
        .downcast_ref::<rigspark_runtime::state::StateError>()
        .is_some_and(|error| error.kind == "locked");
    StartupFailure {
        stage: "open its data folder",
        detail: format!("{} ({})", cause, config.home.display()),
        lock: locked.then(|| config.lock.clone()),
    }
}
/// `~/Library/Logs/RigSpark/startup.log` on macOS (Console.app shows it), else the temp dir.
fn startup_log_path() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join("Library/Logs/RigSpark/startup.log");
    }
    std::env::temp_dir().join("rigspark-startup.log")
}
#[cfg(not(test))]
fn log_startup_failure(failure: &StartupFailure) {
    eprintln!("RigSpark startup failed: {failure:?}");
    let path = startup_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let line = format!(
        "{stamp} v{} could not {}: {}{}\n",
        env!("CARGO_PKG_VERSION"),
        failure.stage,
        failure.detail,
        failure
            .lock
            .as_ref()
            .map(|lock| format!(" [lock {}]", lock.display()))
            .unwrap_or_default()
    );
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}
/// Removes the state lock and its mutation guard so the next start can take them.
/// Only ever run after the person chose to, as the lock's rules require.
fn clear_state_lock(lock: &std::path::Path) -> std::io::Result<()> {
    let mut guard = lock.as_os_str().to_owned();
    guard.push(".guard");
    for result in [
        std::fs::remove_file(lock),
        std::fs::remove_dir(std::path::PathBuf::from(guard)),
    ] {
        match result {
            Err(cause) if cause.kind() != std::io::ErrorKind::NotFound => return Err(cause),
            _ => {}
        }
    }
    Ok(())
}
/// Shows the failure. Returns true when the person cleared the lock and wants a retry.
#[cfg(not(test))]
fn offer_recovery(failure: &StartupFailure) -> bool {
    const CLEAR: &str = "Clear Lock and Retry";
    const QUIT: &str = "Quit";
    let dialog = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("RigSpark could not start")
        .set_description(failure.message());
    let Some(lock) = &failure.lock else {
        dialog.set_buttons(rfd::MessageButtons::Ok).show();
        return false;
    };
    let choice = dialog
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            CLEAR.into(),
            QUIT.into(),
        ))
        .show();
    let clear = matches!(&choice, rfd::MessageDialogResult::Custom(label) if label == CLEAR)
        || matches!(choice, rfd::MessageDialogResult::Ok);
    if !clear {
        return false;
    }
    match clear_state_lock(lock) {
        Ok(()) => true,
        Err(cause) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("RigSpark could not clear the lock")
                .set_description(format!("{}: {cause}", lock.display()))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            false
        }
    }
}

#[cfg(not(test))]
/// The bundled `llama-server` sits beside the app executable (`Contents/MacOS` on macOS,
/// the install directory on Windows).
fn bundled_runtime_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe()
        .ok()?
        .canonicalize()
        .ok()?
        .parent()?
        .to_path_buf();
    let name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    dir.join(name).is_file().then_some(dir)
}
/// True exactly once per install: the first launch records a marker in `dir`.
fn first_run(dir: &std::path::Path) -> bool {
    std::fs::create_dir_all(dir).is_ok()
        && std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("first-run-complete"))
            .is_ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn mock_app(
        home: &std::path::Path,
        picker: DirectoryPicker,
    ) -> (tauri::App<tauri::test::MockRuntime>, String) {
        let host = rigspark_gui::Host::new(home, 43210).unwrap();
        let entry = format!("{}/", host.origin());
        let app = tauri::test::mock_builder()
            .manage(HostState(host))
            .manage(picker)
            .invoke_handler(tauri::generate_handler![
                select_workspace_directory,
                desktop_status,
                set_launch_at_login,
                check_for_update,
                install_update,
                open_github_sign_in,
                open_github_repository_choice
            ])
            .build(tauri::generate_context!())
            .unwrap();
        app.add_capability(desktop_capability(&entry)).unwrap();
        (app, entry)
    }
    #[test]
    fn picker_ipc_is_scoped_to_the_launch_document_and_window() {
        let home = tempfile::tempdir().unwrap();
        let (app, entry) = mock_app(
            home.path(),
            DirectoryPicker::Fixture(Some(home.path().to_path_buf())),
        );
        let main = tauri::WebviewWindowBuilder::new(
            &app,
            "main",
            tauri::WebviewUrl::External(entry.parse().unwrap()),
        )
        .build()
        .unwrap();
        let artifact = tauri::WebviewWindowBuilder::new(
            &app,
            "artifact",
            tauri::WebviewUrl::External(entry.parse().unwrap()),
        )
        .build()
        .unwrap();
        for (label, page_url, allowed) in [
            ("main", entry.as_str(), true),
            (
                "main",
                "http://127.0.0.1:43210/api/images/preview.svg",
                false,
            ),
            ("main", "http://127.0.0.1:43211/", false),
            ("main", "https://example.com/", false),
            ("artifact", entry.as_str(), false),
        ] {
            let webview = if label == "main" { &main } else { &artifact };
            let result = tauri::test::get_ipc_response(
                webview,
                tauri::webview::InvokeRequest {
                    cmd: "select_workspace_directory".into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: page_url.parse().unwrap(),
                    body: tauri::ipc::InvokeBody::default(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
            );
            if allowed {
                assert_eq!(
                    result.unwrap().deserialize::<Option<String>>().unwrap(),
                    Some(home.path().to_string_lossy().into_owned())
                );
            } else {
                assert!(
                    result.is_err(),
                    "privileged IPC must reject {label} at {page_url}"
                );
            }
        }
    }
    #[test]
    fn github_sign_in_opens_only_from_the_launch_document() {
        // The allowed case would start the system browser, so only refusals run here.
        let home = tempfile::tempdir().unwrap();
        let (app, entry) = mock_app(home.path(), DirectoryPicker::Fixture(None));
        let main = tauri::WebviewWindowBuilder::new(
            &app,
            "main",
            tauri::WebviewUrl::External(entry.parse().unwrap()),
        )
        .build()
        .unwrap();
        let artifact = tauri::WebviewWindowBuilder::new(
            &app,
            "artifact",
            tauri::WebviewUrl::External(entry.parse().unwrap()),
        )
        .build()
        .unwrap();
        for (webview, page_url, command) in ["open_github_sign_in", "open_github_repository_choice"]
            .into_iter()
            .flat_map(|command| {
                [
                    (&main, "http://127.0.0.1:43210/api/images/preview.svg"),
                    (&main, "http://127.0.0.1:43211/"),
                    (&main, "https://example.com/"),
                    (&artifact, entry.as_str()),
                ]
                .map(|(webview, page_url)| (webview, page_url, command))
            })
        {
            let result = tauri::test::get_ipc_response(
                webview,
                tauri::webview::InvokeRequest {
                    cmd: command.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: page_url.parse().unwrap(),
                    body: tauri::ipc::InvokeBody::default(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
            );
            assert!(result.is_err(), "{command} must reject {page_url}");
        }
        assert_eq!(GITHUB_DEVICE_PAGE, "https://github.com/login/device");
        assert_eq!(
            GITHUB_INSTALL_PAGE,
            "https://github.com/apps/rigspark-community/installations/new"
        );
    }
    #[test]
    fn only_launch_root_can_navigate_or_invoke_picker() {
        let origin = "http://127.0.0.1:43210";
        assert!(authorized_url(
            &format!("{origin}/#chat").parse().unwrap(),
            origin
        ));
        for url in [
            "http://127.0.0.1:43210/api/images/image.svg",
            "http://127.0.0.1:43211/",
            "https://example.com/",
            "http://localhost:43210/",
            "http://127.0.0.1:43210/?next=evil",
        ] {
            assert!(!authorized_url(&url.parse().unwrap(), origin));
        }
    }
    #[test]
    fn cancelled_picker_grants_no_directory() {
        let home = tempfile::tempdir().unwrap();
        let (app, entry) = mock_app(home.path(), DirectoryPicker::Fixture(None));
        let main = tauri::WebviewWindowBuilder::new(
            &app,
            "main",
            tauri::WebviewUrl::External(entry.parse().unwrap()),
        )
        .build()
        .unwrap();
        let result = tauri::test::get_ipc_response(
            &main,
            tauri::webview::InvokeRequest {
                cmd: "select_workspace_directory".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: entry.parse().unwrap(),
                body: tauri::ipc::InvokeBody::default(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.into(),
            },
        );
        assert_eq!(
            result.unwrap().deserialize::<Option<String>>().unwrap(),
            None
        );
    }
    #[test]
    fn a_held_state_lock_is_reported_as_clearable_and_other_failures_are_not() {
        let home = tempfile::tempdir().unwrap();
        let config = rigspark_runtime::state::Config::from_home(home.path()).unwrap();
        let store = rigspark_runtime::state::StateStore::new(config.clone());
        // Our own live PID: the lock cannot be taken over, exactly like a wedged start.
        std::fs::write(&config.lock, format!("{}\n", std::process::id())).unwrap();
        let cause = rigspark_gui::Host::new(home.path(), 43210)
            .err()
            .expect("a held lock stops the host");
        let failure = host_failure(cause.as_ref(), &config);
        assert_eq!(failure.lock.as_deref(), Some(config.lock.as_path()));
        let message = failure.message();
        assert!(message.contains(&config.lock.display().to_string()));
        assert!(message.contains(&startup_log_path().display().to_string()));

        clear_state_lock(&config.lock).unwrap();
        // Clearing twice, or with no guard directory, is not an error.
        clear_state_lock(&config.lock).unwrap();
        store
            .lock(std::time::Duration::from_millis(100))
            .unwrap()
            .release()
            .unwrap();

        let other = std::io::Error::other("disk full");
        assert!(host_failure(&other, &config).lock.is_none());
    }
    #[test]
    fn clearing_the_lock_removes_an_abandoned_guard_but_not_a_busy_one() {
        let home = tempfile::tempdir().unwrap();
        let lock = home.path().join("lock");
        let guard = home.path().join("lock.guard");
        std::fs::write(&lock, "12345\n").unwrap();
        std::fs::create_dir(&guard).unwrap();
        clear_state_lock(&lock).unwrap();
        assert!(!lock.exists() && !guard.exists());
        // A guard with contents is not ours to remove; report it instead.
        std::fs::create_dir(&guard).unwrap();
        std::fs::write(guard.join("other"), "").unwrap();
        assert!(clear_state_lock(&lock).is_err());
    }
    #[test]
    fn first_run_is_true_once_and_survives_restarts() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("org.rigspark.desktop");
        assert!(first_run(&dir));
        assert!(!first_run(&dir));
        assert!(!first_run(&dir));
    }
    #[test]
    fn bundle_signing_is_ad_hoc_so_downloads_are_not_reported_damaged() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["bundle"]["macOS"]["signingIdentity"], "-");
    }
    #[test]
    fn desktop_product_name_is_filesystem_safe() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let name = config["productName"].as_str().unwrap();
        assert_eq!(name, "RigSpark");
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._ -".contains(&byte))
        );
    }

    #[test]
    fn tray_icon_is_a_one_hood_template() {
        let tray = tauri::image::Image::from_bytes(super::TRAY_ICON).unwrap();
        assert_eq!((tray.width(), tray.height()), (44, 44));
        // black ink on clear only, so macOS can tint it
        assert!(
            tray.rgba()
                .chunks(4)
                .all(|p| p[3] == 0 || (p[0] < 40 && p[1] < 40 && p[2] < 40))
        );
    }
}
