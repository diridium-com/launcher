// Copyright (c) Kiran Ayyagari. All rights reserved.
// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

use anyhow::Error;
use home::env::Env;
use home::env::OS_ENV;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Create a file readable/writable only by the owner (0600 on Unix). The
/// connection store holds plaintext passwords, so it must not be written at the
/// default umask (typically world-readable 0644). On non-Unix it falls back to
/// the platform default; the file lives under the user's home directory.
fn create_private_file(path: &std::path::Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // `.mode()` only applies when the file is newly created. Set it
        // explicitly so a pre-existing tmp (e.g. left world-readable by a crash
        // in an older build that used File::create) is tightened to owner-only
        // before we write the plaintext-password JSON into it.
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        File::create(path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionEntry {
    pub address: String,
    #[serde(rename = "heapSize")]
    pub heap_size: String,
    pub id: String,
    #[serde(rename = "javaHome")]
    pub java_home: String,
    #[serde(rename = "javaArgs")]
    pub java_args: Option<String>,
    pub name: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "get_default_group")]
    pub group: String,
    #[serde(default = "get_default_notes")]
    pub notes: String,
    #[serde(default = "get_default_donotcache")]
    pub donotcache: bool,
    #[serde(default, rename = "lastConnected")]
    pub last_connected: Option<i64>,
    #[serde(default, rename = "showConsole")]
    pub show_console: bool,
    #[serde(default = "get_default_engine_type", rename = "engineType")]
    pub engine_type: String,
    /// Trusted server leaf-cert SHA-256 (hex). None = not yet trusted; the first
    /// launch prompts the operator (TOFU). Not a secret, so it lives in the JSON.
    #[serde(default, rename = "pinnedCertSha256")]
    pub pinned_cert_sha256: Option<String>,
    /// Path to this connection's custom icon, shown in the launcher's own
    /// surfaces (the connection list and its console window). None/empty means
    /// the bundled default. A missing file falls back to the default rather
    /// than failing anything.
    #[serde(default, rename = "iconPath")]
    pub icon_path: Option<String>,
    /// The Phosphor glyph and badge colour `icon_path` was composed from, kept
    /// so the picker can restore the selection and recolour it on a later
    /// visit. Purely picker state: the icon that gets used always comes from
    /// `icon_path`, and both are None for a hand-picked image file.
    #[serde(default, rename = "iconGlyph")]
    pub icon_glyph: Option<String>,
    #[serde(default, rename = "iconColor")]
    pub icon_color: Option<String>,
}

pub struct ConnectionStore {
    con_cache: Mutex<HashMap<String, Arc<ConnectionEntry>>>,
    con_location: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl Default for ConnectionEntry {
    fn default() -> Self {
        ConnectionEntry {
            address: String::new(),
            heap_size: String::from("512m"),
            id: Uuid::new_v4().to_string(),
            java_home: find_java_home(),
            java_args: Some(String::new()),
            name: String::new(),
            username: None,
            password: None,
            group: get_default_group(),
            notes: get_default_notes(),
            donotcache: get_default_donotcache(),
            last_connected: None,
            show_console: false,
            engine_type: get_default_engine_type(),
            pinned_cert_sha256: None,
            icon_path: None,
            icon_glyph: None,
            icon_color: None,
        }
    }
}

impl ConnectionStore {
    /// The file connections are persisted to. Surfaced in the UI next to the
    /// password field: the operator is told the password is stored unencrypted,
    /// and that is only actionable if they can find the file. Read from the
    /// store's own field rather than recomputed so the two cannot drift.
    pub fn store_path(&self) -> &Path {
        &self.con_location
    }

    pub fn init(data_dir_path: PathBuf) -> Result<Self, Error> {
        let con_location = data_dir_path.join("launcher-data.json");

        let mut cache = HashMap::new();
        // An empty or missing file is a normal first run. A non-empty file that
        // won't parse is preserved (renamed aside) before we start empty, so a
        // corrupt config never silently wipes the user's saved connections.
        match fs::read_to_string(&con_location) {
            Ok(contents) if contents.trim().is_empty() => {}
            Ok(contents) => {
                match serde_json::from_str::<HashMap<String, ConnectionEntry>>(&contents) {
                    Ok(data) => {
                        for (id, ce) in data {
                            cache.insert(id, Arc::new(ce));
                        }
                    }
                    Err(e) => {
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let backup = con_location
                            .with_file_name(format!("launcher-data.corrupt-{}.json", ts));
                        warn!(
                            "could not parse {:?}: {}; backing it up to {:?} and starting empty",
                            con_location, e, backup
                        );
                        if let Err(re) = fs::rename(&con_location, &backup) {
                            warn!("failed to back up unparseable connection store: {}", re);
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::new(e)),
        }

        // The file holds plaintext passwords but may carry looser permissions:
        // copied/renamed from a legacy ballista or catapult config with the
        // source's mode bits, or written before the 0600 hardening. Only the
        // save path goes through create_private_file, so tighten it here on
        // every start.
        #[cfg(unix)]
        if con_location.exists() {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) =
                fs::set_permissions(&con_location, fs::Permissions::from_mode(0o600))
            {
                warn!("could not restrict permissions on {:?}: {}", con_location, e);
            }
        }

        let cache_dir = data_dir_path.join("cache");
        if !cache_dir.exists() {
            fs::create_dir(&cache_dir)?;
        }

        let logs_dir = data_dir_path.join("logs");
        if !logs_dir.exists() {
            fs::create_dir(&logs_dir)?;
        }

        Ok(ConnectionStore {
            con_location,
            con_cache: Mutex::new(cache),
            cache_dir,
            logs_dir,
        })
    }

    pub fn to_json_array_string(&self) -> String {
        let cache = self.con_cache.lock().expect("connection cache lock poisoned");
        let entries: Vec<&Arc<ConnectionEntry>> = cache.values().collect();
        serde_json::to_string(&entries).unwrap_or_else(|_| String::from("[]"))
    }

    pub fn get(&self, id: &str) -> Option<Arc<ConnectionEntry>> {
        let cs = self.con_cache.lock().expect("connection cache lock poisoned");
        cs.get(id).map(Arc::clone)
    }

    pub fn save(&self, mut ce: ConnectionEntry) -> Result<String, Error> {
        if ce.id.is_empty() {
            ce.id = uuid::Uuid::new_v4().to_string();
        }

        let mut jh = ce.java_home.trim().to_string();
        if jh.is_empty() {
            jh = find_java_home();
        }
        ce.java_home = jh;

        if let Some(ref username) = ce.username {
            let username = username.trim();
            if username.is_empty() {
                ce.username = None;
            }
        }

        if let Some(ref password) = ce.password {
            let password = password.trim();
            if password.is_empty() {
                ce.password = None;
            }
        }

        let data = serde_json::to_string(&ce)?;
        self.con_cache
            .lock()
            .expect("connection cache lock poisoned")
            .insert(ce.id.clone(), Arc::new(ce));
        self.write_connections_to_disk()?;
        Ok(data)
    }

    /// Removes the connection, then the per-connection files that nothing will
    /// reference again: its isolated (do-not-cache) jar directory, its launch
    /// log, and its saved icon. All are keyed by the connection id, so once the
    /// entry is gone there is no way to reach them from the UI and they sit on
    /// disk forever. An isolated cache dir runs to ~90MB.
    ///
    /// The shared `<engine-type>/<version>` cache is deliberately left alone:
    /// other connections use it.
    ///
    /// If an administrator launched from this connection is still running with
    /// do-not-cache on, removing its jar directory pulls classes out from under
    /// it. That is the same hazard the launch path already creates, since it
    /// wipes and repopulates this directory on every do-not-cache launch.
    ///
    /// Cleanup failures are logged rather than returned: the connection is
    /// already gone by then, and failing the command would misreport that.
    pub fn delete(&self, id: &str) -> Result<(), Error> {
        self.con_cache.lock().expect("connection cache lock poisoned").remove(id);
        self.write_connections_to_disk()?;

        let isolated = self
            .cache_dir
            .join("_isolated")
            .join(crate::webstart::sanitize_for_path(id));
        if isolated.exists() {
            if let Err(e) = fs::remove_dir_all(&isolated) {
                warn!("could not remove {:?}: {}", isolated, e);
            }
        }

        // The launch path names the log with the RAW id, not the sanitized one.
        let log = self.logs_dir.join(format!("{}.log", id));
        if log.exists() {
            if let Err(e) = fs::remove_file(&log) {
                warn!("could not remove {:?}: {}", log, e);
            }
        }

        // save_connection_icon writes <data dir>/icons/<id>.png, sanitizing the
        // id the same way. Only a composed icon lives there; a hand-picked
        // image file stays wherever the operator keeps it.
        if let Some(data_dir) = self.cache_dir.parent() {
            let icon = data_dir.join("icons").join(format!(
                "{}.png",
                id.chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                    .collect::<String>()
            ));
            if icon.exists() {
                if let Err(e) = fs::remove_file(&icon) {
                    warn!("could not remove {:?}: {}", icon, e);
                }
            }
        }
        Ok(())
    }

    pub fn import(&self, file_path: &str, overwrite: bool) -> Result<String, Error> {
        let f = File::open(file_path)?;
        let data: Vec<ConnectionEntry> = serde_json::from_reader(f)?;

        let mut cache = self.con_cache.lock().expect("connection cache lock poisoned");
        let duplicates: Vec<String> = data
            .iter()
            .filter(|ce| cache.contains_key(&ce.id))
            .map(|ce| ce.name.clone())
            .collect();

        if !duplicates.is_empty() && !overwrite {
            drop(cache);
            let result = serde_json::json!({
                "status": "duplicates",
                "names": duplicates,
                "total": data.len(),
            });
            return Ok(result.to_string());
        }

        let java_home = find_java_home();
        let count = data.len();
        for mut ce in data {
            ce.java_home = java_home.clone();
            cache.insert(ce.id.clone(), Arc::new(ce));
        }
        drop(cache);

        self.write_connections_to_disk()?;
        let result = serde_json::json!({
            "status": "ok",
            "total": count,
        });
        Ok(result.to_string())
    }

    fn write_connections_to_disk(&self) -> Result<(), Error> {
        let val = {
            let c = self.con_cache.lock().expect("connection cache lock poisoned");
            serde_json::to_string_pretty(&*c)?
        };
        // Write to a sibling temp file, fsync, then atomically rename over the
        // target so a crash mid-write can never leave a truncated (data-losing)
        // launcher-data.json. The lock is released before this blocking I/O.
        let tmp = self.con_location.with_file_name("launcher-data.json.tmp");
        {
            let mut f = create_private_file(&tmp).map_err(|e| {
                warn!("unable to open file for writing: {}", e);
                Error::new(e)
            })?;
            f.write_all(val.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.con_location)?;
        Ok(())
    }

    pub fn update_last_connected(&self, id: &str) -> Result<(), Error> {
        let mut cache = self.con_cache.lock().expect("connection cache lock poisoned");
        if let Some(entry) = cache.get(id) {
            let mut updated = (**entry).clone();
            updated.last_connected = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock is before UNIX epoch")
                    .as_millis() as i64,
            );
            cache.insert(id.to_string(), Arc::new(updated));
        }
        drop(cache);
        self.write_connections_to_disk()?;
        Ok(())
    }

    /// Set (or clear) a connection's pinned certificate fingerprint.
    pub fn update_pin(&self, id: &str, sha256: Option<String>) -> Result<(), Error> {
        let mut cache = self.con_cache.lock().expect("connection cache lock poisoned");
        if let Some(entry) = cache.get(id) {
            let mut updated = (**entry).clone();
            updated.pinned_cert_sha256 = sha256;
            cache.insert(id.to_string(), Arc::new(updated));
        }
        drop(cache);
        self.write_connections_to_disk()?;
        Ok(())
    }

    pub fn get_all_groups(&self) -> Result<HashSet<String>, Error> {
        let connections = self.con_cache
            .lock()
            .expect("connection cache lock poisoned");

        let mut groups: HashSet<String> = HashSet::new();
        groups.insert(get_default_group());
        groups.extend(connections.values().map(|ce| ce.group.clone()));
        Ok(groups)
    }

    pub fn get_all_engine_types(&self) -> Result<HashSet<String>, Error> {
        let connections = self.con_cache
            .lock()
            .expect("connection cache lock poisoned");

        let mut engine_types: HashSet<String> = HashSet::new();
        engine_types.insert(get_default_engine_type());
        engine_types.extend(connections.values().map(|ce| ce.engine_type.clone()));
        Ok(engine_types)
    }
}

/// Default `java_home` for a new connection. Use JAVA_HOME if set, otherwise
/// leave it empty so the launch falls back to `java` on PATH. We deliberately do
/// not guess a JDK per platform: the old guesses were wrong (macOS pinned Java
/// 8, Windows picked up the System32 stub) and the standard mechanisms are more
/// reliable. The user can still set java_home per connection in the editor.
pub fn find_java_home() -> String {
    match OS_ENV.var_os("JAVA_HOME").and_then(|jh| jh.to_str().map(String::from)) {
        Some(jh) => {
            info!("JAVA_HOME is set to {}", jh);
            jh
        }
        None => String::new(),
    }
}

fn get_default_group() -> String {
    String::from("Default")
}

fn get_default_notes() -> String {
    String::new()
}

fn get_default_donotcache() -> bool {
    false
}

fn get_default_engine_type() -> String {
    String::from("Open Integration Engine")
}

#[cfg(all(test, unix))]
mod tests {
    use super::{create_private_file, ConnectionStore};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// delete() has to remove the per-connection files, because once the entry
    /// is gone nothing can reach them: an isolated cache dir runs to ~90MB and
    /// would sit there forever. Equally it must NOT touch the shared
    /// <engine-type>/<version> cache, which other connections launch from.
    #[test]
    fn delete_removes_per_connection_files_but_not_the_shared_cache() {
        let data_dir = std::env::temp_dir().join(format!("launcher-del-{}", std::process::id()));
        fs::remove_dir_all(&data_dir).ok();
        fs::create_dir_all(&data_dir).expect("create data dir");
        let store = ConnectionStore::init(data_dir.clone()).expect("init store");

        let id = "1234abcd-0000-1111-2222-333344445555";
        let isolated = store.cache_dir.join("_isolated").join(id);
        fs::create_dir_all(&isolated).expect("isolated dir");
        fs::write(isolated.join("some.jar"), b"jar").expect("jar");

        fs::create_dir_all(&store.logs_dir).expect("logs dir");
        let log = store.logs_dir.join(format!("{}.log", id));
        fs::write(&log, b"log").expect("log");

        let icons = data_dir.join("icons");
        fs::create_dir_all(&icons).expect("icons dir");
        let icon = icons.join(format!("{}.png", id));
        fs::write(&icon, b"png").expect("icon");

        // Shared cache for an engine type + version: must survive.
        let shared = store.cache_dir.join("open-integration-engine").join("4_6_0").join("core");
        fs::create_dir_all(&shared).expect("shared dir");
        let shared_jar = shared.join("mirth-client.jar");
        fs::write(&shared_jar, b"jar").expect("shared jar");

        store.delete(id).expect("delete");

        assert!(!isolated.exists(), "isolated cache dir should be removed");
        assert!(!log.exists(), "launch log should be removed");
        assert!(!icon.exists(), "saved icon should be removed");
        assert!(shared_jar.is_file(), "shared cache must NOT be touched");

        fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn create_private_file_enforces_owner_only_even_if_preexisting() {
        let dir = std::env::temp_dir().join(format!("launcher-perm-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launcher-data.json.tmp");

        // Simulate a stale, world-readable tmp left by an older build's File::create.
        fs::write(&path, b"stale").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let _f = create_private_file(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "secrets file must be owner-only (0600)");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_tightens_permissions_of_existing_store() {
        let dir = std::env::temp_dir().join(format!("launcher-init-perm-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("launcher-data.json");

        // Simulate a store migrated from a legacy config with umask-default mode.
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        super::ConnectionStore::init(dir.clone()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "connection store must be owner-only (0600) after init");

        fs::remove_dir_all(&dir).ok();
    }
}
