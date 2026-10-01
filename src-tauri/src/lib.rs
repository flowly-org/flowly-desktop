//! Flowly tray time tracker: a tray icon with the running timer, today's tasks and a small window.

mod api;

use api::{ApiError, Client};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;

#[derive(Default, Clone)]
struct Snapshot {
    timer: Option<Value>,
    tasks: Vec<Value>,
    workspaces: Vec<Value>,
}

struct AppState {
    client: Arc<Client>,
    snapshot: Mutex<Snapshot>,
    tray: Mutex<Option<TrayIcon>>,
}

fn err(e: ApiError) -> String {
    e.to_string()
}

fn elapsed(entry: &Value) -> i64 {
    let started = entry["started_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    started.map(|s| (chrono::Utc::now() - s.with_timezone(&chrono::Utc)).num_seconds().max(0)).unwrap_or(0)
}

fn hm(secs: i64) -> String {
    format!("{}:{:02}", secs / 3600, (secs % 3600) / 60)
}

fn title_of(entry: &Value) -> String {
    entry["task_title"].as_str().or(entry["description"].as_str()).filter(|s| !s.is_empty()).unwrap_or("Untitled work").to_string()
}

/// Refresh timer + today's tasks from the API, then rebuild the tray.
async fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let client = state.client.clone();
    if !client.signed_in() {
        *state.snapshot.lock().unwrap() = Snapshot::default();
        rebuild_tray(app);
        let _ = app.emit("flowly://state", ());
        return;
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let timer = client.get("/api/my/timer/").await;
    let tasks = client.get(&format!("/api/my/tasks/?day={today}&include_overdue=true&assignee=me")).await;
    let ctx = client.get("/api/my/context/").await;
    if matches!(timer, Err(ApiError::SignedOut)) {
        client.sign_out();
    }
    {
        let mut snap = state.snapshot.lock().unwrap();
        if let Ok(t) = timer {
            snap.timer = t.get("entry").cloned().filter(|e| !e.is_null());
        }
        if let (Ok(Value::Array(list)), Ok(Value::Array(ws))) = (tasks, ctx) {
            // Done statuses per workspace, to leave finished tasks out of the menu.
            let done: std::collections::HashSet<i64> = ws
                .iter()
                .flat_map(|w| w["tag_groups"].as_array().cloned().unwrap_or_default())
                .filter(|g| g["kind"] == "status")
                .flat_map(|g| g["tags"].as_array().cloned().unwrap_or_default())
                .filter(|t| t["is_done"].as_bool().unwrap_or(false))
                .filter_map(|t| t["id"].as_i64())
                .collect();
            let tracking: std::collections::HashSet<i64> =
                ws.iter().filter(|w| w["time_tracking"].as_bool().unwrap_or(true)).filter_map(|w| w["id"].as_i64()).collect();
            snap.tasks = list
                .into_iter()
                .filter(|t| !done.contains(&t["status"].as_i64().unwrap_or(-1)))
                .filter(|t| tracking.contains(&t["workspace"].as_i64().unwrap_or(-1)))
                .take(15)
                .collect();
            snap.workspaces = ws
                .iter()
                .filter(|w| w["time_tracking"].as_bool().unwrap_or(true))
                .map(|w| json!({"id": w["id"], "name": w["name"], "color": w["color"]}))
                .collect();
        }
    }
    rebuild_tray(app);
    let _ = app.emit("flowly://state", ());
}

fn rebuild_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let snap = state.snapshot.lock().unwrap().clone();
    let signed_in = state.client.signed_in();
    let build = || -> tauri::Result<Menu<tauri::Wry>> {
        let menu = Menu::new(app)?;
        if !signed_in {
            menu.append(&MenuItem::with_id(app, "signin", "Sign in to Flowly…", true, None::<&str>)?)?;
        } else {
            match &snap.timer {
                Some(t) => {
                    let label = format!("● {} · {}", hm(elapsed(t)), title_of(t));
                    menu.append(&MenuItem::with_id(app, "status", label, false, None::<&str>)?)?;
                    menu.append(&MenuItem::with_id(app, "stop", "Stop timer", true, None::<&str>)?)?;
                }
                None => menu.append(&MenuItem::with_id(app, "status", "No timer running", false, None::<&str>)?)?,
            }
            let tasks = Submenu::with_id(app, "tasks", "Start on today's task", !snap.tasks.is_empty())?;
            for t in &snap.tasks {
                let id = format!("task:{}:{}", t["workspace"], t["id"]);
                tasks.append(&MenuItem::with_id(app, id, t["title"].as_str().unwrap_or("Task"), true, None::<&str>)?)?;
            }
            menu.append(&tasks)?;
        }
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        menu.append(&MenuItem::with_id(app, "show", "Show window", true, None::<&str>)?)?;
        menu.append(&MenuItem::with_id(app, "open", "Open Flowly", true, None::<&str>)?)?;
        if signed_in {
            menu.append(&MenuItem::with_id(app, "signout", "Sign out", true, None::<&str>)?)?;
        }
        menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;
        Ok(menu)
    };
    let guard = state.tray.lock().unwrap();
    if let Some(tray) = guard.as_ref() {
        if let Ok(menu) = build() {
            let _ = tray.set_menu(Some(menu));
        }
        let text = snap.timer.as_ref().map(|t| hm(elapsed(t)));
        #[cfg(target_os = "macos")]
        let _ = tray.set_title(text.as_deref());
        let tip = match (&snap.timer, &text) {
            (Some(t), Some(time)) => format!("Flowly · {time} on {}", title_of(t)),
            _ => "Flowly".to_string(),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

async fn start_timer(app: &AppHandle, workspace: i64, task: Option<i64>, description: String) -> Result<(), String> {
    let client = app.state::<AppState>().client.clone();
    client
        .post("/api/my/timer/", json!({"action": "start", "workspace": workspace, "task": task, "description": description}))
        .await
        .map_err(err)?;
    refresh(app).await;
    Ok(())
}

// --- commands used by the window ----------------------------------------------

#[tauri::command]
fn get_state(state: State<AppState>) -> Value {
    let snap = state.snapshot.lock().unwrap().clone();
    json!({
        "signed_in": state.client.signed_in(),
        "email": state.client.email(),
        "timer": snap.timer,
        "tasks": snap.tasks,
        "workspaces": snap.workspaces,
    })
}

#[tauri::command]
async fn sign_in(app: AppHandle) -> Result<(), String> {
    let client = app.state::<AppState>().client.clone();
    let opener = app.clone();
    client
        .sign_in(move |url| {
            let _ = opener.opener().open_url(url, None::<&str>);
        })
        .await
        .map_err(err)?;
    refresh(&app).await;
    show_window(&app);
    Ok(())
}

#[tauri::command]
async fn sign_out(app: AppHandle) -> Result<(), String> {
    app.state::<AppState>().client.sign_out();
    refresh(&app).await;
    Ok(())
}

#[tauri::command]
async fn start(app: AppHandle, workspace: i64, task: Option<i64>, description: Option<String>) -> Result<(), String> {
    start_timer(&app, workspace, task, description.unwrap_or_default()).await
}

#[tauri::command]
async fn stop(app: AppHandle) -> Result<(), String> {
    let client = app.state::<AppState>().client.clone();
    client.post("/api/my/timer/", json!({"action": "stop"})).await.map_err(err)?;
    refresh(&app).await;
    Ok(())
}

#[tauri::command]
async fn reload(app: AppHandle) -> Result<(), String> {
    refresh(&app).await;
    Ok(())
}

#[tauri::command]
fn open_flowly(app: AppHandle, path: Option<String>) {
    let _ = app.opener().open_url(format!("{}{}", api::DEFAULT_APP, path.unwrap_or_else(|| "/day".into())), None::<&str>);
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show_window(app)))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_state, sign_in, sign_out, start, stop, reload, open_flowly])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config = app.path().app_config_dir()?;
            app.manage(AppState { client: Arc::new(Client::new(config)), snapshot: Mutex::new(Snapshot::default()), tray: Mutex::new(None) });

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            let tray = TrayIconBuilder::with_id("flowly")
                .icon(icon)
                .icon_as_template(true)
                .tooltip("Flowly")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    let id = event.id().as_ref().to_string();
                    let app = app.clone();
                    match id.as_str() {
                        "quit" => app.exit(0),
                        "show" => show_window(&app),
                        "open" => {
                            let _ = app.opener().open_url(format!("{}/day", api::DEFAULT_APP), None::<&str>);
                        }
                        "signin" => {
                            tauri::async_runtime::spawn(async move {
                                if let Err(e) = sign_in(app.clone()).await {
                                    let _ = app.emit("flowly://error", e);
                                }
                            });
                        }
                        "signout" => {
                            tauri::async_runtime::spawn(async move {
                                let _ = sign_out(app).await;
                            });
                        }
                        "stop" => {
                            tauri::async_runtime::spawn(async move {
                                let _ = stop(app).await;
                            });
                        }
                        other if other.starts_with("task:") => {
                            let parts: Vec<&str> = other.split(':').collect();
                            if let (Some(ws), Some(task)) = (parts.get(1).and_then(|s| s.parse().ok()), parts.get(2).and_then(|s| s.parse().ok())) {
                                tauri::async_runtime::spawn(async move {
                                    if let Err(e) = start_timer(&app, ws, Some(task), String::new()).await {
                                        let _ = app.emit("flowly://error", e);
                                    }
                                });
                            }
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_window(tray.app_handle());
                    }
                })
                .build(app)?;
            *app.state::<AppState>().tray.lock().unwrap() = Some(tray);

            // Poll the API every minute; refresh the tray clock every 20 s.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut tick: u32 = 0;
                loop {
                    if tick % 3 == 0 {
                        refresh(&handle).await;
                    } else {
                        rebuild_tray(&handle);
                    }
                    tick = tick.wrapping_add(1);
                    tokio::time::sleep(Duration::from_secs(20)).await;
                }
            });
            if !app.state::<AppState>().client.signed_in() {
                show_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window just hides it; the app lives in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Flowly");
}
