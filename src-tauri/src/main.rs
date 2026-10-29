// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod client_config;
mod remote;
mod tokens;

use askbar_core::{config_path, db_path, Config, ConversationSummary, StoredMessage, Ui};
use backend::{Backend, Local, Settings, StreamEvent};
use remote::Remote;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{ipc::Channel, AppHandle, Emitter, LogicalSize, Manager, Monitor, PhysicalPosition, State, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tokio::sync::oneshot;

/// Ignore focus-loss events this soon after showing; some window managers
/// send a spurious one while the window is being mapped.
const BLUR_GRACE: Duration = Duration::from_millis(500);

/// Hide-on-blur waits this long and is cancelled by any show/hide/toggle or
/// focus regain in between. Without it, X11 breaks the hotkey: activating the
/// shortcut grab sends the focused window a focus-out, the blur handler hides
/// the window, and then the toggle sees "hidden" and shows it again.
const BLUR_DEBOUNCE: Duration = Duration::from_millis(200);

struct Placement {
    width: f64,
    height: f64,
    top_percent: f64,
}

struct AppState {
    /// Swapped when the user pairs with a server.
    backend: RwLock<Arc<dyn Backend>>,
    cancel: Mutex<Option<oneshot::Sender<()>>>,
    shown_at: Mutex<Instant>,
    placement: Mutex<Placement>,
    hide_on_blur: AtomicBool,
    visible: AtomicBool,
    /// Bumped on every show/hide/toggle/focus change; lets a pending blur-hide
    /// detect that something else happened first.
    epoch: AtomicU64,
    hotkey: Mutex<Option<Shortcut>>,
    notice: Mutex<Option<String>>,
}

impl AppState {
    fn backend(&self) -> Arc<dyn Backend> {
        self.backend.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------- window

fn pick_monitor(w: &WebviewWindow) -> Option<Monitor> {
    if let (Ok(cur), Ok(monitors)) = (w.cursor_position(), w.available_monitors()) {
        for m in monitors {
            let (p, s) = (m.position(), m.size());
            let inside = cur.x >= p.x as f64
                && cur.x < p.x as f64 + s.width as f64
                && cur.y >= p.y as f64
                && cur.y < p.y as f64 + s.height as f64;
            if inside {
                return Some(m);
            }
        }
    }
    w.current_monitor().ok().flatten().or_else(|| w.primary_monitor().ok().flatten())
}

/// Size the window and centre it horizontally on the monitor the cursor is on.
fn apply_layout(w: &WebviewWindow, p: &Placement) {
    let Some(m) = pick_monitor(w) else {
        let _ = w.set_size(LogicalSize::new(p.width, p.height));
        let _ = w.center();
        return;
    };
    let scale = m.scale_factor();
    let (mw, mh) = (m.size().width as f64, m.size().height as f64);
    let width = p.width.min(mw / scale);
    let height = p.height.min(mh / scale * 0.92);
    let _ = w.set_size(LogicalSize::new(width, height));
    let x = m.position().x as f64 + (mw - width * scale) / 2.0;
    let y = m.position().y as f64 + mh * p.top_percent;
    let _ = w.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

fn show(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    let state = app.state::<AppState>();
    state.epoch.fetch_add(1, Ordering::SeqCst);
    *lock(&state.shown_at) = Instant::now();
    apply_layout(&w, &lock(&state.placement));
    let _ = w.show();
    let _ = w.set_focus();
    state.visible.store(true, Ordering::Relaxed);
    let _ = app.emit("askbar:shown", ());
}

fn hide(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.epoch.fetch_add(1, Ordering::SeqCst);
    state.visible.store(false, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

fn toggle(app: &AppHandle) {
    // We track visibility ourselves: the toolkit's `is_visible()` can be stale
    // when queried from the hotkey thread, and focus reporting is unreliable on
    // some window managers. The window is always-on-top, so "visible" is enough.
    let state = app.state::<AppState>();
    // Cancels any blur-hide that the shortcut grab itself just triggered.
    state.epoch.fetch_add(1, Ordering::SeqCst);
    if state.visible.load(Ordering::Relaxed) {
        hide(app);
    } else {
        show(app);
    }
}

// ------------------------------------------------------------- config

fn register_hotkey(app: &AppHandle, spec: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let gs = app.global_shortcut();
    if let Some(old) = lock(&state.hotkey).take() {
        let _ = gs.unregister(old);
    }
    let shortcut: Shortcut = spec.parse().map_err(|_| {
        format!(
            "The hotkey \"{spec}\" in config.yaml is not valid. Use modifiers plus one key, such as \"Ctrl+Shift+Space\", \"Alt+Space\" or \"Super+A\"."
        )
    })?;
    gs.register(shortcut).map_err(|e| {
        let mut msg = format!(
            "Could not register the hotkey \"{spec}\": {e}. Another app or the system may already use it; change `hotkey` in config.yaml."
        );
        if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            msg.push_str(" On Wayland, bind a desktop shortcut to the `askbar` command instead; running it again toggles the window.");
        }
        msg
    })?;
    *lock(&state.hotkey) = Some(shortcut);
    Ok(())
}

/// Re-read config.yaml and apply what can change at runtime.
fn apply_config(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let cfg = match tauri::async_runtime::block_on(state.backend().settings()) {
        Ok(c) => c,
        Err(e) => return Some(e),
    };
    state.hide_on_blur.store(cfg.ui.hide_on_blur, Ordering::Relaxed);
    lock(&state.placement).top_percent = cfg.ui.top_percent;
    register_hotkey(app, &cfg.hotkey).err()
}

// ----------------------------------------------------------- commands

#[derive(Serialize)]
struct Bootstrap {
    ui: Ui,
    hotkey: String,
    provider: String,
    model: String,
    notice: Option<String>,
}

#[tauri::command]
async fn bootstrap(state: State<'_, AppState>) -> Result<Bootstrap, String> {
    let (cfg, mut notice) = match state.backend().settings().await {
        Ok(c) => (c, None),
        Err(e) => (Settings::builtin(), Some(e)),
    };
    if let Some(n) = lock(&state.notice).take() {
        notice = Some(n);
    }
    Ok(Bootstrap { ui: cfg.ui, hotkey: cfg.hotkey, provider: cfg.provider, model: cfg.model, notice })
}

/// `null` when the backend is fine; otherwise what is wrong and (for a missing provider) the alternatives.
#[tauri::command]
async fn server_status(state: State<'_, AppState>) -> Result<Option<backend::ClientStatus>, String> {
    Ok(state.backend().status().await)
}

/// Exchange a pairing string for a device token, remember the server, and switch to it.
#[tauri::command]
async fn pair(app: AppHandle, state: State<'_, AppState>, pairing: String) -> Result<(), String> {
    let path = client_config::path();
    let keychain = tokens::Keychain::new(&path);
    remote::pair(&pairing, &path, &keychain).await?;
    let remote = Remote::new(client_config::ClientConfig::load(&path), path, Box::new(keychain));
    *state.backend.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(remote);
    // Pick up the new hotkey and look and feel.
    let _ = apply_config(&app);
    Ok(())
}

#[tauri::command]
async fn set_active_provider(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state.backend().set_active_provider(name).await
}

#[tauri::command]
async fn list_conversations(state: State<'_, AppState>) -> Result<Vec<ConversationSummary>, String> {
    state.backend().list_conversations().await
}

#[tauri::command]
async fn load_conversation(state: State<'_, AppState>, id: i64) -> Result<Vec<StoredMessage>, String> {
    state.backend().load_conversation(id).await
}

#[tauri::command]
async fn delete_conversation(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.backend().delete_conversation(id).await
}

#[tauri::command]
async fn ask(
    state: State<'_, AppState>,
    conversation_id: Option<i64>,
    text: String,
    on_event: Channel<StreamEvent>,
) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    let (tx, rx) = oneshot::channel();
    *lock(&state.cancel) = Some(tx);
    state
        .backend()
        .ask(conversation_id, text, rx, &|e| {
            let _ = on_event.send(e);
        })
        .await;
    lock(&state.cancel).take();
    Ok(())
}

#[tauri::command]
fn cancel_ask(state: State<'_, AppState>) {
    if let Some(tx) = lock(&state.cancel).take() {
        let _ = tx.send(());
    }
}

#[tauri::command]
fn resize_window(app: AppHandle, state: State<'_, AppState>, width: f64, height: f64) {
    let Some(w) = app.get_webview_window("main") else { return };
    let mut p = lock(&state.placement);
    p.width = width.clamp(240.0, 4000.0);
    p.height = height.clamp(60.0, 4000.0);
    apply_layout(&w, &p);
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    hide(&app);
}

#[tauri::command]
fn open_config() -> Result<(), String> {
    let path = config_path();
    opener::open(&path)
        .or_else(|_| opener::reveal(&path))
        .map_err(|e| format!("Could not open {}: {e}", path.display()))
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only http(s) links can be opened.".into());
    }
    opener::open_browser(&url).map_err(|e| e.to_string())
}

// --------------------------------------------------------------- main

fn main() {
    tauri::Builder::default()
        // Must be first. A second launch (e.g. a desktop shortcut running
        // `askbar`) toggles the running instance, which is how Wayland users
        // get a hotkey.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| toggle(app)))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle(app);
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(true) => {
                window.app_handle().state::<AppState>().epoch.fetch_add(1, Ordering::SeqCst);
            }
            WindowEvent::Focused(false) => {
                let app = window.app_handle().clone();
                let state = app.state::<AppState>();
                let recent = lock(&state.shown_at).elapsed() < BLUR_GRACE;
                if !state.hide_on_blur.load(Ordering::Relaxed) || recent {
                    return;
                }
                let epoch = state.epoch.load(Ordering::SeqCst);
                std::thread::spawn(move || {
                    std::thread::sleep(BLUR_DEBOUNCE);
                    let state = app.state::<AppState>();
                    if state.epoch.load(Ordering::SeqCst) == epoch && state.visible.load(Ordering::Relaxed) {
                        hide(&app);
                    }
                });
            }
            _ => {}
        })
        .setup(|app| {
            // A client.yaml means "use the server"; without one the app works on this machine alone.
            let client_yaml = client_config::path();
            let (backend, created, config_error): (Arc<dyn Backend>, bool, Option<String>) = if client_yaml.exists() {
                let tokens = Box::new(tokens::Keychain::new(&client_yaml));
                (Arc::new(Remote::new(client_config::ClientConfig::load(&client_yaml), client_yaml.clone(), tokens)), false, None)
            } else {
                let (created, err) = match Config::load_or_create(&config_path()) {
                    Ok((_, created)) => (created, None),
                    Err(e) => (false, Some(e.to_string())),
                };
                (Arc::new(Local::open(&db_path())?), created, err)
            };
            let (cfg, settings_error) = match tauri::async_runtime::block_on(backend.settings()) {
                Ok(s) => (s, None),
                Err(e) => (Settings::builtin(), Some(e)),
            };
            let config_error = config_error.or(settings_error);

            app.manage(AppState {
                backend: RwLock::new(backend),
                cancel: Mutex::new(None),
                shown_at: Mutex::new(Instant::now()),
                placement: Mutex::new(Placement { width: 768.0, height: 140.0, top_percent: cfg.ui.top_percent }),
                hide_on_blur: AtomicBool::new(cfg.ui.hide_on_blur),
                visible: AtomicBool::new(false),
                epoch: AtomicU64::new(0),
                hotkey: Mutex::new(None),
                notice: Mutex::new(None),
            });

            #[cfg(target_os = "macos")]
            if cfg.ui.macos_hide_dock_icon {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let toggle_item = MenuItem::with_id(app, "toggle", "Show / hide", true, None::<&str>)?;
            let config_item = MenuItem::with_id(app, "config", "Open config file", true, None::<&str>)?;
            let reload_item = MenuItem::with_id(app, "reload", "Reload config", true, None::<&str>)?;
            let pair_item = MenuItem::with_id(app, "pair", "Pair with server…", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Askbar", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_item, &config_item, &reload_item, &pair_item, &quit_item])?;

            let mut tray = TrayIconBuilder::with_id("askbar")
                .tooltip("Askbar")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "toggle" => toggle(app),
                    "config" => {
                        let _ = open_config();
                    }
                    "reload" => {
                        let state = app.state::<AppState>();
                        *lock(&state.notice) = apply_config(app).or(Some("Config reloaded.".into()));
                        show(app);
                    }
                    "pair" => {
                        show(app);
                        let _ = app.emit("askbar:pair", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        toggle(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // Hotkey problems and first-run guidance are shown in the window,
            // since there is no console to read.
            let handle = app.handle().clone();
            let hotkey_error = register_hotkey(&handle, &cfg.hotkey).err();
            let notice = config_error.or(hotkey_error).or_else(|| {
                created.then(|| {
                    format!(
                        "Welcome. Add your API key in {} (tray icon → Open config file), then ask away.",
                        config_path().display()
                    )
                })
            });
            if notice.is_some() {
                *lock(&handle.state::<AppState>().notice) = notice;
                show(&handle);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            server_status,
            set_active_provider,
            pair,
            list_conversations,
            load_conversation,
            delete_conversation,
            ask,
            cancel_ask,
            resize_window,
            hide_window,
            open_config,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Askbar");
}
