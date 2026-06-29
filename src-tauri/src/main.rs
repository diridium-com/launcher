// Copyright (c) Kiran Ayyagari. All rights reserved.
// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;

use log::{info, warn};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::connection::{ConnectionEntry, ConnectionStore};
use crate::console::ConsoleRegistry;
use crate::webstart::{LoadConfig, WebstartCache, WebstartFile};

mod connection;
mod console;
mod tls;
mod webstart;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tauri::command]
async fn get_launcher_info() -> String {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "launcher_version".to_string(),
        serde_json::Value::String(String::from(APP_VERSION)),
    );
    serde_json::to_string(&obj).unwrap_or_default()
}

#[tauri::command(rename_all = "snake_case")]
async fn launch(id: String, on_progress: Channel<serde_json::Value>, app: AppHandle, cs: State<'_, ConnectionStore>, wc: State<'_, WebstartCache>, registry: State<'_, ConsoleRegistry>) -> Result<String, String> {
    let ce = cs.get(&id)
        .ok_or_else(|| format!("connection not found: {}", id))?;

    // Fail fast on a missing java before any cert handshake or download.
    let java_home = ce.java_home.clone();
    let java_ok = tauri::async_runtime::spawn_blocking(move || webstart::check_java_available(&java_home))
        .await
        .map_err(|e| e.to_string())?;
    if let Err(e) = java_ok {
        let msg = e.to_string();
        warn!("{}", msg);
        return Ok(serde_json::json!({ "code": -1, "msg": msg }).to_string());
    }

    let cache_dir = cs.cache_dir.clone();
    let logs_dir = cs.logs_dir.clone();
    let address = ce.address.clone();
    let conn_id = ce.id.clone();
    let conn_name = ce.name.clone();
    let donotcache = ce.donotcache;
    let engine_type = ce.engine_type.clone();

    // Verify the server's TLS certificate against the connection's pin (TOFU).
    // This runs on every launch, before any download, so the cert is re-checked
    // even when the WebstartFile is cached.
    let pin = ce.pinned_cert_sha256.clone();
    let captured = tauri::async_runtime::spawn_blocking({
        let address = address.clone();
        move || crate::tls::capture_cert(&address)
    })
    .await
    .map_err(|e| e.to_string())?;
    let captured = match captured {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Could not reach {}: {}", address, e);
            warn!("{}", msg);
            return Ok(serde_json::json!({ "code": -1, "msg": msg }).to_string());
        }
    };
    match &pin {
        // First connect to this server: ask the operator to trust the cert.
        None => return Ok(serde_json::json!({ "code": 2, "cert": captured }).to_string()),
        // The cert differs from the one previously trusted.
        Some(p) if !p.eq_ignore_ascii_case(&captured.sha256) => {
            return Ok(serde_json::json!({ "code": 3, "cert": captured }).to_string())
        }
        // Matches the pin — proceed.
        Some(_) => {}
    }

    let mut ws = wc.get(&address);
    if ws.is_none() {
        let tmp = tauri::async_runtime::spawn_blocking({
            let on_progress = on_progress.clone();
            let address = address.clone();
            let cache_dir = cache_dir.clone();
            let logs_dir = logs_dir.clone();
            let pinned_cert_sha256 = pin.clone();
            move || WebstartFile::load(LoadConfig {
                base_url: &address,
                cache_dir: &cache_dir,
                donotcache,
                conn_id: &conn_id,
                conn_name: &conn_name,
                engine_type: &engine_type,
                logs_dir: &logs_dir,
                on_progress: &on_progress,
                pinned_cert_sha256,
            })
        }).await.map_err(|e| e.to_string())?;

        match tmp {
            Err(e) => {
                let msg = e.to_string();
                warn!("{}", msg);
                return Ok(serde_json::json!({ "code": -1, "msg": msg }).to_string());
            }
            Ok(wf) => {
                let wf = Arc::new(wf);
                wc.put(&address, Arc::clone(&wf));
                ws = Some(wf);
            }
        }
    }
    let ws = ws.expect("WebstartFile should be loaded at this point");
    let _ = on_progress.send(serde_json::json!({"message": "Launching administrator..."}));
    let console_sink = if ce.show_console {
        let label = console_window_label(&ce.id);
        let buf = registry.get_or_create(&label);
        let generation = console::reset_for_relaunch(&buf);
        Some(console::ConsoleSink { buf, generation, app: app.clone(), label })
    } else {
        None
    };
    // Capture what we need to open the console window AFTER the spawn succeeds,
    // so a failed launch (e.g. java not found) doesn't pop an empty console.
    let console_window = console_sink
        .as_ref()
        .map(|s| (s.label.clone(), format!("Console — {}", ce.name)));

    let r = ws.run(ce, console_sink);
    if let Err(e) = r {
        let msg = e.to_string();
        warn!("{}", msg);
        return Ok(serde_json::json!({ "code": -1, "msg": msg }).to_string());
    }

    // The process spawned — now open (or focus) the console window. Output
    // produced before the window attaches is replayed from the backlog.
    if let Some((label, title)) = console_window {
        let app_handle = app.clone();
        app.run_on_main_thread(move || {
            if let Some(w) = app_handle.get_webview_window(&label) {
                let _ = w.set_focus();
            } else if let Err(e) =
                WebviewWindowBuilder::new(&app_handle, label.as_str(), WebviewUrl::default())
                    .title(title)
                    .inner_size(760.0, 520.0)
                    .build()
            {
                warn!("failed to create console window: {}", e);
            }
        })
        .map_err(|e| e.to_string())?;
    }

    let _ = cs.update_last_connected(&id);
    Ok(serde_json::json!({ "code": 0 }).to_string())
}

#[tauri::command(rename_all = "snake_case")]
fn set_pin(connection_id: String, sha256: String, cs: State<ConnectionStore>) -> Result<(), String> {
    // Canonicalize so every stored pin is byte-identical to what tls::to_hex
    // produces and the launch-time compare always agrees.
    let pin = sha256.trim().to_ascii_lowercase();
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid certificate fingerprint".to_string());
    }
    cs.update_pin(&connection_id, Some(pin)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_default_connectionentry(_cs: State<ConnectionStore>) -> Result<serde_json::Value, String> {
    let connection_entry = ConnectionEntry::default();
    Ok(serde_json::json!(connection_entry))
}

#[tauri::command]
fn get_all_groups(cs: State<ConnectionStore>) -> Result<serde_json::Value, String> {
    let groups = cs.get_all_groups().map_err(|e| e.to_string())?;
    Ok(serde_json::json!(groups))
}

#[tauri::command]
fn get_all_engine_types(cs: State<ConnectionStore>) -> Result<serde_json::Value, String> {
    let engine_types = cs.get_all_engine_types().map_err(|e| e.to_string())?;
    Ok(serde_json::json!(engine_types))
}

#[tauri::command]
fn load_connections(cs: State<ConnectionStore>) -> String {
    cs.to_json_array_string()
}

#[tauri::command]
fn load_single_connection(cs: State<ConnectionStore>, connection_id: String) -> Result<serde_json::Value, String> {
    let connection_entry = cs.get(connection_id.as_str())
        .ok_or_else(|| format!("connection not found: {}", connection_id))?;
    Ok(serde_json::json!(connection_entry))
}

#[tauri::command]
fn save(ce: &str, cs: State<ConnectionStore>) -> Result<String, String> {
    let ce: ConnectionEntry = serde_json::from_str(ce)
        .map_err(|e| format!("failed to deserialize ConnectionEntry: {}", e))?;
    cs.save(ce).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete(id: &str, cs: State<ConnectionStore>) -> Result<String, String> {
    cs.delete(id).map_err(|e| e.to_string())?;
    Ok(String::from("success"))
}

#[tauri::command(rename_all = "snake_case")]
fn import(file_path: &str, overwrite: bool, cs: State<ConnectionStore>) -> Result<String, String> {
    cs.import(file_path, overwrite).map_err(|e| e.to_string())
}

fn main() {
    let env_fix = fix_path_env::fix_vars(&["JAVA_HOME", "PATH"]);
    if let Err(_e) = env_fix {
        eprintln!("failed to read JAVA_HOME and PATH environment variables");
    }

    let home_directory = home::home_dir().expect("unable to find the path to home directory");
    let launcher_directory = home_directory.join(".launcher");
    if let Err(e) = fs::create_dir(&launcher_directory) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            eprintln!("failed to create .launcher directory: {}", e);
            exit(1);
        }
    }

    // Migrate from legacy directories if they exist
    let legacy_ballista_dir = home_directory.join(".ballista");
    if legacy_ballista_dir.exists() {
        move_file(legacy_ballista_dir.join("ballista-data.json"), launcher_directory.join("launcher-data.json"));
    } else {
        move_file(home_directory.join("catapult-data.json"), launcher_directory.join("launcher-data.json"));
    }

    let connection_store = ConnectionStore::init(launcher_directory);
    if let Err(e) = connection_store {
        eprintln!("failed to initialize ConnectionStore: {}", e);
        exit(1);
    }

    let webcache = WebstartCache::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_shell::init())
        .manage(connection_store.expect("ConnectionStore init was checked above"))
        .manage(webcache)
        .manage(ConsoleRegistry::default())
        .invoke_handler(tauri::generate_handler![
            launch,
            import,
            delete,
            save,
            get_default_connectionentry,
            get_all_groups,
            get_all_engine_types,
            load_connections,
            load_single_connection,
            get_launcher_info,
            set_pin,
            console::console_subscribe,
            console::console_save
        ])
        .on_window_event(|window, event| {
            // When a console window closes, drop its buffer so a later relaunch
            // starts clean instead of replaying a dead session.
            if let tauri::WindowEvent::Destroyed = event {
                let label = window.label().to_string();
                if label.starts_with("console-") {
                    if let Some(reg) = window.app_handle().try_state::<ConsoleRegistry>() {
                        reg.remove(&label);
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Build a valid, unique Tauri window label for a connection's console.
/// Tauri labels allow only `[a-zA-Z0-9-/:_]`; sanitize anything else. The
/// `console-` prefix is what the frontend (app.vue) and the console capability
/// glob (`console-*`) match on.
fn console_window_label(conn_id: &str) -> String {
    let sanitized: String = conn_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("console-{}", sanitized)
}

fn move_file(old: PathBuf, new: PathBuf) {
    if old.exists() && !new.exists() {
        if let Err(e) = fs::rename(&old, &new) {
            info!(
                "failed to move the file from {:?} to {:?} : {}",
                old, new, e
            );
        }
    }
}
