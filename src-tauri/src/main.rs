// Copyright (c) Kiran Ayyagari. All rights reserved.
// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::PathBuf;
use std::process::exit;

use log::warn;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::connection::{ConnectionEntry, ConnectionStore};
use crate::console::ConsoleRegistry;
use crate::webstart::{LoadConfig, WebstartFile};

mod connection;
mod console;
mod tls;
mod webstart;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tauri::command]
async fn get_launcher_info(cs: State<'_, ConnectionStore>) -> Result<String, String> {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "launcher_version".to_string(),
        serde_json::Value::String(String::from(APP_VERSION)),
    );
    // Shown beside the password field. display() renders native separators, so
    // a Windows operator sees a Windows path rather than a unix-looking one.
    obj.insert(
        "store_path".to_string(),
        serde_json::Value::String(cs.store_path().display().to_string()),
    );
    Ok(serde_json::to_string(&obj).unwrap_or_default())
}

#[tauri::command(rename_all = "snake_case")]
async fn launch(id: String, force: bool, on_progress: Channel<serde_json::Value>, app: AppHandle, cs: State<'_, ConnectionStore>, registry: State<'_, ConsoleRegistry>) -> Result<String, String> {
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

    // Every launch re-fetches the JNLP and re-verifies the cached jars. An
    // in-memory WebstartFile cache used to short-circuit this for 120s, but
    // skipping load() also skipped the hash check, so a jar or a JNLP that
    // changed between two launches went unnoticed. The only people who relaunch
    // inside that window are the ones upgrading an engine or installing
    // extensions, which is exactly what the check exists to catch.
    let loaded = tauri::async_runtime::spawn_blocking({
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
            acknowledge_cache_mismatch: force,
        })
    }).await.map_err(|e| e.to_string())?;

    let ws = match loaded {
        Err(e) => {
            // A cache/engine collision is a distinct, recoverable outcome:
            // surface it as code 4 with details so the frontend can confirm
            // and retry with force=true, instead of a generic error.
            if let Some(cm) = e.downcast_ref::<crate::webstart::CacheMismatch>() {
                return Ok(serde_json::json!({
                    "code": 4,
                    "engine_type": cm.engine_type,
                    "version": cm.version,
                    "jars": cm.jars,
                }).to_string());
            }
            let msg = e.to_string();
            warn!("{}", msg);
            return Ok(serde_json::json!({ "code": -1, "msg": msg }).to_string());
        }
        Ok(wf) => wf,
    };
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
        .map(|s| (s.label.clone(), format!("Console - {}", ce.name)));
    // The console is a launcher-owned Tauri window, so it shows the launcher
    // icon by default. Give it the connection's icon, resolved before ws.run()
    // takes ownership of `ce`.
    let console_icon = if console_window.is_some() {
        resolve_connection_icon(&app, ce.icon_path.as_deref())
    } else {
        None
    };

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
            } else {
                // icon() consumes the builder and can fail, so keep a way to
                // make a fresh one: an icon problem must never stop the console
                // from opening.
                let base = || {
                    WebviewWindowBuilder::new(&app_handle, label.as_str(), WebviewUrl::default())
                        .title(title.clone())
                        .inner_size(760.0, 520.0)
                };
                let mut builder = base();
                // Image::from_path reads png/ico only, so a user-picked jpg or
                // gif just leaves the launcher icon in place.
                if let Some(ref p) = console_icon {
                    match tauri::image::Image::from_path(p) {
                        Ok(img) => match builder.icon(img) {
                            Ok(b) => builder = b,
                            Err(e) => {
                                warn!("could not apply console window icon: {}", e);
                                builder = base();
                            }
                        },
                        Err(e) => warn!("could not read console icon {:?}: {}", p, e),
                    }
                }
                if let Err(e) = builder.build() {
                    warn!("failed to create console window: {}", e);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    }

    let _ = cs.update_last_connected(&id);
    Ok(serde_json::json!({ "code": 0 }).to_string())
}

/// Bundled preset icons, in display order. A connection stores
/// `preset:<name>`; the files live in resources/icons/. Phosphor Icons glyphs
/// (MIT, see resources/icons/LICENSE-phosphor.txt).
const PRESET_ICONS: [&str; 12] = [
    "heartbeat", "stethoscope", "shield-check", "database", "plug", "cloud",
    "globe", "gear", "rocket", "flask", "bug", "lightning",
];

/// Resolve a `preset:<name>` icon to its bundled file. None for unknown or
/// unsafe names (the name is data from launcher-data.json, so it is not
/// trusted to form paths).
fn resolve_preset_icon(app: &AppHandle, name: &str) -> Option<PathBuf> {
    use tauri::path::BaseDirectory;
    if !PRESET_ICONS.contains(&name) {
        return None;
    }
    app.path()
        .resolve(format!("resources/icons/{}.png", name), BaseDirectory::Resource)
        .ok()
        .filter(|p| p.is_file())
}

/// Resolve a connection's icon selection to a file: `preset:<name>` to the
/// bundled preset, anything else as a file path, and the bundled default when
/// nothing is selected or the selection is unavailable. The single resolution
/// path used by the connection list, the picker preview, and the console
/// window, so they can never disagree. None only if even the default is gone.
fn resolve_connection_icon(app: &AppHandle, icon_path: Option<&str>) -> Option<PathBuf> {
    use tauri::path::BaseDirectory;
    if let Some(sel) = icon_path.map(str::trim).filter(|s| !s.is_empty()) {
        let resolved = match sel.strip_prefix("preset:") {
            Some(name) => resolve_preset_icon(app, name),
            None => Some(PathBuf::from(sel)).filter(|p| p.is_file()),
        };
        match resolved {
            Some(p) => return Some(p),
            None => warn!("connection icon {:?} unavailable; using the default icon", sel),
        }
    }
    app.path()
        .resolve("resources/admin-icon.png", BaseDirectory::Resource)
        .ok()
        .filter(|p| p.is_file())
}

/// Data URI of a connection's icon. Backs the connection list and the picker.
#[tauri::command(rename_all = "snake_case")]
fn get_connection_icon(icon_path: Option<String>, app: AppHandle) -> Result<String, String> {
    use base64::Engine;
    let p = resolve_connection_icon(&app, icon_path.as_deref()).ok_or("no icon available")?;
    let mime = match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    let bytes = fs::read(&p).map_err(|e| e.to_string())?;
    Ok(format!("data:{};base64,{}", mime, base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Persist an icon composed in the picker (a canvas PNG) for a connection,
/// into <data dir>/icons/<id>.png. Returns the path to store in iconPath.
#[tauri::command(rename_all = "snake_case")]
fn save_connection_icon(connection_id: String, png_base64: String, cs: State<ConnectionStore>) -> Result<String, String> {
    use base64::Engine;
    let sanitized: String = connection_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        return Err("bad connection id".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64.trim())
        .map_err(|e| e.to_string())?;
    if bytes.len() > 5 * 1024 * 1024 {
        return Err("icon too large".to_string());
    }
    if !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Err("not a png".to_string());
    }
    let dir = cs
        .cache_dir
        .parent()
        .ok_or("no data directory")?
        .join("icons");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.png", sanitized));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
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
    // This dir holds launcher-data.json (plaintext passwords) and per-connection
    // logs; restrict it to the owner on Unix. Best-effort, not fatal on failure.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&launcher_directory, fs::Permissions::from_mode(0o700));
    }

    // Migrate from legacy directories if they exist
    let legacy_ballista_dir = home_directory.join(".ballista");
    if legacy_ballista_dir.exists() {
        copy_file(legacy_ballista_dir.join("ballista-data.json"), launcher_directory.join("launcher-data.json"));
    } else {
        copy_file(home_directory.join("catapult-data.json"), launcher_directory.join("launcher-data.json"));
    }

    let connection_store = ConnectionStore::init(launcher_directory);
    if let Err(e) = connection_store {
        eprintln!("failed to initialize ConnectionStore: {}", e);
        exit(1);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_shell::init())
        .manage(connection_store.expect("ConnectionStore init was checked above"))
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
            get_connection_icon,
            save_connection_icon,
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

fn copy_file(old: PathBuf, new: PathBuf) {
    if old.exists() && !new.exists() {
        if let Err(e) = fs::copy(&old, &new) {
            warn!(
                "failed to copy the file from {:?} to {:?} : {}",
                old, new, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::copy_file;
    use std::fs;

    #[test]
    fn copy_file_copies_and_keeps_the_source() {
        let dir = std::env::temp_dir().join(format!("launcher-copy-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("ballista-data.json");
        let dst = dir.join("launcher-data.json");
        fs::write(&src, b"{\"a\":1}").unwrap();

        copy_file(src.clone(), dst.clone());

        assert_eq!(fs::read(&dst).unwrap(), b"{\"a\":1}");
        // The legacy config must survive migration (issue #20); the old
        // move-based migration deleted it.
        assert!(src.exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_file_never_overwrites_an_existing_destination() {
        let dir = std::env::temp_dir().join(format!("launcher-noclobber-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("ballista-data.json");
        let dst = dir.join("launcher-data.json");
        fs::write(&src, b"legacy").unwrap();
        fs::write(&dst, b"current").unwrap();

        copy_file(src, dst.clone());

        assert_eq!(fs::read(&dst).unwrap(), b"current");

        fs::remove_dir_all(&dir).ok();
    }
}
