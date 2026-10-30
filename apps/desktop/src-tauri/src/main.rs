use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_updater::UpdaterExt as _;

/// Argument the login item passes so a login launch starts in the menu bar only.
#[cfg(not(test))]
const BACKGROUND_ARG: &str = "--background";
/// Commands the launch document may invoke; each re-checks window and URL.
const DESKTOP_COMMANDS: [&str; 5] = [
    "allow-select-workspace-directory",
    "allow-desktop-status",
    "allow-set-launch-at-login",
    "allow-check-for-update",
    "allow-install-update",
];

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
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
#[cfg(not(test))]
fn main() {
    let dialog_smoke = std::env::args().any(|argument| argument == "--dialog-smoke-test");
    let smoke = dialog_smoke || std::env::args().any(|argument| argument == "--smoke-test");
    let background = !smoke && std::env::args().any(|argument| argument == BACKGROUND_ARG);
    rigspark_runtime::process_control::enable_package_manager_fallback();
    let runtime = tokio::runtime::Runtime::new().expect("native async runtime");
    let config = rigspark_runtime::state::Config::load().expect("native configuration");
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind((
            std::net::Ipv4Addr::LOCALHOST,
            0,
        )))
        .expect("loopback listener");
    let host = rigspark_gui::Host::new(
        &config.home,
        listener.local_addr().expect("listener address").port(),
    )
    .expect("native GUI host");
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
            install_update
        ])
        .setup(move |app| {
            app.add_capability(desktop_capability(&entry))?;
            let script = format!("if(window===window.top && location.origin==={} && location.pathname==='/'){{const invoke=(command,args)=>window.__TAURI_INTERNALS__.invoke(command,args);Object.defineProperty(window,'llmupDesktop',{{value:Object.freeze({{selectWorkspaceDirectory:()=>invoke('select_workspace_directory'),status:()=>invoke('desktop_status'),setLaunchAtLogin:(enabled)=>invoke('set_launch_at_login',{{enabled:enabled===true}}),checkForUpdate:()=>invoke('check_for_update'),installUpdate:()=>invoke('install_update')}}),writable:false,configurable:false}});}}",serde_json::to_string(&origin)?);
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
                        let _=window.eval(if dialog_smoke { include_str!("dialog-smoke.js") } else { "location.href='/__native_smoke/'+(document.title==='RigSpark' && document.querySelector('main') && document.querySelector('textarea') && typeof window.llmupDesktop?.selectWorkspaceDirectory==='function' && ['status','setLaunchAtLogin','checkForUpdate','installUpdate'].every((name)=>typeof window.llmupDesktop?.[name]==='function') && !document.querySelector('#desktop-settings')?.hidden && typeof window.__TAURI_INTERNALS__?.invoke==='function' ? 'pass':'fail')" });
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
        .build(tauri::generate_context!()).expect("Tauri application");
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
                install_update
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
}
