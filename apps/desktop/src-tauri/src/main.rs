use std::sync::Arc;
use tauri::Manager;

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
fn picker_capability(entry: &str) -> String {
    let root_pattern = format!("{entry}{{}}");
    serde_json::json!({"identifier":"main-picker","windows":["main"],"local":false,"remote":{"urls":[root_pattern]},"permissions":["allow-select-workspace-directory"]}).to_string()
}
fn authorized_url(url: &url::Url, origin: &str) -> bool {
    url.origin().ascii_serialization() == origin
        && url.path() == "/"
        && url.query().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}
#[tauri::command]
async fn select_workspace_directory<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, HostState>,
    picker: tauri::State<'_, DirectoryPicker>,
) -> Result<Option<String>, String> {
    if window.label() != "main"
        || !authorized_url(
            &window.url().map_err(|_| "window unavailable")?,
            &state.0.origin(),
        )
    {
        return Err("directory picker is unavailable to this document".into());
    }
    Ok(picker
        .pick()
        .await
        .map(|path| path.to_string_lossy().into_owned()))
}
#[cfg(not(test))]
fn main() {
    let dialog_smoke = std::env::args().any(|argument| argument == "--dialog-smoke-test");
    let smoke = dialog_smoke || std::env::args().any(|argument| argument == "--smoke-test");
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
        .invoke_handler(tauri::generate_handler![select_workspace_directory])
        .setup(move |app| {
            app.add_capability(picker_capability(&entry))?;
            let script = format!("if(window===window.top && location.origin==={} && location.pathname==='/'){{Object.defineProperty(window,'llmupDesktop',{{value:Object.freeze({{selectWorkspaceDirectory:()=>window.__TAURI_INTERNALS__.invoke('select_workspace_directory')}}),writable:false,configurable:false}});}}",serde_json::to_string(&origin)?);
            let allowed = origin.clone();
            let navigation_app=app.handle().clone();
            let smoke_app=app.handle().clone();
            if smoke { tauri::async_runtime::spawn(async move { tokio::time::sleep(std::time::Duration::from_secs(if dialog_smoke { 60 } else { 20 })).await; smoke_app.exit(1); }); }
            tauri::WebviewWindowBuilder::new(app,"main",tauri::WebviewUrl::External(entry.parse()?))
                .title("RigSpark").inner_size(1280.0,840.0).min_inner_size(760.0,540.0)
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
                        let _=window.eval(if dialog_smoke { include_str!("dialog-smoke.js") } else { "location.href='/__native_smoke/'+(document.title==='RigSpark' && document.querySelector('main') && document.querySelector('textarea') && typeof window.llmupDesktop?.selectWorkspaceDirectory==='function' && typeof window.__TAURI_INTERNALS__?.invoke==='function' ? 'pass':'fail')" });
                    }
                })
                .on_new_window(|_,_|tauri::webview::NewWindowResponse::Deny)
                .on_download(|_,_|false)
                .build()?;
            let closing = launch_host.clone();
            app.get_webview_window("main").expect("main window").on_window_event(move |event| { if matches!(event,tauri::WindowEvent::Destroyed) { closing.shutdown.cancel(); } });
            Ok(())
        })
        .build(tauri::generate_context!()).expect("Tauri application");
    app.run(move |_, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            host.shutdown.cancel();
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
            .invoke_handler(tauri::generate_handler![select_workspace_directory])
            .build(tauri::generate_context!())
            .unwrap();
        app.add_capability(picker_capability(&entry)).unwrap();
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
