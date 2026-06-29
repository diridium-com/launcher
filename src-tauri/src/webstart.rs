// Copyright (c) Kiran Ayyagari. All rights reserved.
// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Error;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use log::{info, warn};
use reqwest::blocking::Client;
use reqwest::Url;
use roxmltree::Node;
use rustc_hash::FxHashMap;
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;

use crate::connection::ConnectionEntry;

/// How long a cached WebstartFile remains valid before re-fetching (seconds)
const WEBSTART_CACHE_TTL_SECS: u64 = 120;

/// Windows: CREATE_NO_WINDOW flag to suppress console window
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Configuration for loading a WebstartFile, replacing a long parameter list.
pub struct LoadConfig<'a> {
    pub base_url: &'a str,
    pub cache_dir: &'a PathBuf,
    pub donotcache: bool,
    pub conn_id: &'a str,
    pub conn_name: &'a str,
    pub engine_type: &'a str,
    pub logs_dir: &'a PathBuf,
    pub on_progress: &'a Channel<serde_json::Value>,
    /// The connection's trusted leaf-cert SHA-256 (hex). Required here: the
    /// launch command verifies/captures the pin before calling load().
    pub pinned_cert_sha256: Option<String>,
}

#[derive(Debug)]
pub struct WebstartFile {
    main_class: String,
    args: Vec<String>,
    j2ses: Option<Vec<J2se>>,
    jar_dir: PathBuf,
    logs_dir: PathBuf,
    conn_id: String,
    loaded_at: SystemTime,
}

/// from jnlp -> resources -> j2se
#[derive(Debug)]
struct J2se {
    java_vm_args: Option<String>,
    version: String,
}

pub struct WebstartCache {
    cache: Mutex<FxHashMap<String, Arc<WebstartFile>>>,
}

impl WebstartCache {
    pub fn init() -> Self {
        let cache = Mutex::new(FxHashMap::default());
        WebstartCache { cache }
    }

    pub fn get(&self, url: &str) -> Option<Arc<WebstartFile>> {
        let cache = self.cache.lock().expect("webstart cache lock poisoned");
        let wf = cache.get(url);
        if let Some(wf) = wf {
            let now = SystemTime::now();
            let elapsed = now
                .duration_since(wf.loaded_at)
                .expect("failed to calculate the duration");
            if elapsed.as_secs() < WEBSTART_CACHE_TTL_SECS {
                return Some(Arc::clone(wf));
            }
        }
        None
    }

    pub fn put(&self, url: &str, wf: Arc<WebstartFile>) {
        let mut cache = self.cache.lock().expect("webstart cache lock poisoned");
        cache.insert(url.to_string(), wf);
    }
}

impl WebstartFile {
    pub fn load(config: LoadConfig) -> Result<WebstartFile, Error> {
        let base_url = normalize_url(config.base_url)?;
        let webstart = format!("{}/webstart.jnlp", base_url);
        // The connection id can come from an imported file, so sanitize it before
        // it ever touches the filesystem (cache dirs, log path). main.rs already
        // sanitizes the same id for window labels.
        let safe_conn_id = sanitize_for_path(config.conn_id);
        let _ = config.on_progress.send(serde_json::json!({"message": "Fetching server configuration..."}));
        // Download over a pinned-TLS client. The launch command guarantees the
        // pin is present and matches the live cert before we get here.
        let pin = config
            .pinned_cert_sha256
            .as_deref()
            .ok_or_else(|| Error::msg("internal error: launch reached download with no pinned certificate"))?;
        let client = crate::tls::pinned_client(pin)?;

        let r = client.get(&webstart).send()?;
        let data = r.text()?;
        let doc = roxmltree::Document::parse(&data)?;

        let root = doc.root();
        let main_class_node = get_node(&root, "application-desc").ok_or(Error::msg(
            "Got something from MC that was not an application-desc node in a JNLP XML",
        ))?;
        let main_class = main_class_node
            .attribute("main-class")
            .ok_or(Error::msg("missing main-class attribute"))?
            .to_string();
        let args = get_client_args(&main_class_node);

        let resources_node = get_node(&root, "resources");

        let mut jnlp_version = "default".to_string();
        if let Some(jnlp_node) = get_node(&root, "jnlp") {
            if let Some(v) = jnlp_node.attribute("version") {
                jnlp_version = v.replace(['/', '\\', '.'], "_");
            }
        }

        // Build jar_dir based on donotcache flag and engine type
        let jar_dir = if config.donotcache {
            let dir = config.cache_dir.join("_isolated").join(&safe_conn_id);
            if dir.exists() {
                info!("removing isolated cache directory {:?}", dir);
                std::fs::remove_dir_all(&dir)?;
            }
            dir
        } else {
            let vendor = sanitize_for_path(config.engine_type);
            info!("using engine type for cache: {} (sanitized: {})", config.engine_type, vendor);
            config.cache_dir.join(&vendor).join(&jnlp_version)
        };

        if !jar_dir.exists() {
            info!("creating directory {:?}", jar_dir);
            std::fs::create_dir_all(&jar_dir)?;
        }

        // Create core/ and extensions/ subdirectories
        let core_dir = jar_dir.join("core");
        if !core_dir.exists() {
            std::fs::create_dir_all(&core_dir)?;
        }

        let mut j2ses = None;
        if let Some(resources_node) = resources_node {
            j2ses = get_j2ses(&resources_node);
            download_jars(&resources_node, &client, &jar_dir, &base_url, config.on_progress)?;
        }

        // Migration: clean up old per-connection cache directory
        if !config.donotcache {
            let sanitized_name = config.conn_name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '-' })
                .collect::<String>();
            let id_prefix = &safe_conn_id[..safe_conn_id.len().min(8)];
            let old_cache_folder = format!("{}_{}", sanitized_name, id_prefix);
            let old_jar_dir = config.cache_dir.join(old_cache_folder);
            if old_jar_dir.exists() {
                info!("migrating: removing old cache directory {:?}", old_jar_dir);
                let _ = std::fs::remove_dir_all(&old_jar_dir);
            }
        }

        let ws = WebstartFile {
            main_class,
            jar_dir,
            logs_dir: config.logs_dir.clone(),
            conn_id: safe_conn_id,
            args,
            loaded_at: SystemTime::now(),
            j2ses,
        };

        Ok(ws)
    }

    pub fn run(
        &self,
        ce: Arc<ConnectionEntry>,
        console: Option<crate::console::ConsoleSink>,
    ) -> Result<(), Error> {
        let mut mirth_jars = Vec::new();
        let mut other_jars = Vec::new();

        // Collect JARs from core/ and extensions/*/
        let mut dirs_to_scan = vec![self.jar_dir.join("core")];
        let ext_dir = self.jar_dir.join("extensions");
        if ext_dir.exists() {
            for entry in ext_dir.read_dir()? {
                let entry = entry?;
                if entry.metadata()?.is_dir() {
                    dirs_to_scan.push(entry.path());
                }
            }
        }

        for dir in &dirs_to_scan {
            if !dir.exists() {
                continue;
            }
            for e in dir.read_dir()? {
                let e = e?;
                if e.metadata()?.is_dir() {
                    continue;
                }
                let file_path = e.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jar") {
                    continue;
                }
                let file_name = match file_path.file_name().and_then(|f| f.to_str()) {
                    Some(name) => name.to_string(),
                    None => continue,
                };
                let file_path_str = match file_path.to_str() {
                    Some(p) => p.to_string(),
                    None => continue,
                };

                // MirthConnect's own jars contain some overridden classes
                // of the dependent libraries and hence must be loaded first
                // https://forums.mirthproject.io/forum/mirth-connect/support/15524-using-com-mirth-connect-client-core-client
                if file_name.starts_with("mirth") {
                    mirth_jars.push(file_path_str);
                } else {
                    other_jars.push(file_path_str);
                }
            }
        }

        mirth_jars.sort();
        other_jars.sort();
        let classpath_separator = if cfg!(windows) { ";" } else { ":" };
        mirth_jars.extend(other_jars);
        let classpath = mirth_jars.join(classpath_separator);

        let java_home = ce.java_home.trim();
        let mut cmd = if java_home.is_empty() {
            Command::new("java")
        } else {
            Command::new(PathBuf::from(java_home).join("bin").join("java"))
        };

        info!("using java from: {:?}", cmd.get_program().to_str());

        if let Some(ref vm_args) = self.j2ses {
            for va in vm_args {
                if va.version.contains("1.9") {
                    if let Some(java_vm_args) = &va.java_vm_args {
                        let filtered = sanitize_vm_args(java_vm_args);
                        if !filtered.is_empty() {
                            info!("setting JDK_JAVA_OPTIONS for version {}", va.version);
                            cmd.env("JDK_JAVA_OPTIONS", &filtered);
                        }
                    }
                }
            }
        }

        let heap = ce.heap_size.trim();
        if !heap.is_empty() {
            cmd.arg(format!("-Xmx{}", heap));
        }

        if let Some(args) = ce.java_args.as_deref() {
            let sanitized = sanitize_vm_args(args);
            if !sanitized.is_empty() {
                cmd.args(sanitized.split_whitespace());
            }
        }

        cmd.arg("-cp")
            .arg(classpath)
            .arg(&self.main_class)
            .args(&self.args);

        if let Some(ref username) = ce.username {
            cmd.arg(username);
            if let Some(ref password) = ce.password {
                cmd.arg(password);
            }
        }

        if let Some(console) = console {
            // Capture BOTH stdout and stderr. Swing/AWT exceptions from the
            // administrator land on stderr, so capturing only stdout (as the
            // old Java console did) silently dropped them.
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            info!("launching administrator with console (main class {})", self.main_class);
            let mut child = cmd.spawn()?;

            let out_reader = child
                .stdout
                .take()
                .map(|out| spawn_console_reader(out, "out", Arc::clone(&console.buf)));
            let err_reader = child
                .stderr
                .take()
                .map(|err| spawn_console_reader(err, "err", Arc::clone(&console.buf)));

            // Reap the process, then wait for the readers to drain the final
            // output before posting the exit notice so it appears last. Reaping
            // also avoids the zombie the fire-and-forget path used to leak.
            let buf = console.buf;
            let generation = console.generation;
            let app = console.app;
            let label = console.label;
            std::thread::spawn(move || {
                let exit = child.wait();
                if let Some(h) = out_reader {
                    let _ = h.join();
                }
                if let Some(h) = err_reader {
                    let _ = h.join();
                }
                let (status, clean) = match exit {
                    Ok(s) => (format!("process exited ({})", s), s.success()),
                    Err(e) => (format!("failed to wait on process: {}", e), false),
                };
                // Close the console only on a clean exit of the current process.
                // On an abend (non-zero), leave it open so the error/stack trace
                // stays readable.
                if crate::console::mark_exited(&buf, generation, status) && clean {
                    crate::console::close_window(&app, &label);
                }
            });
        } else {
            let log_path = self.logs_dir.join(format!("{}.log", self.conn_id));
            let log_file = File::create(&log_path);
            match log_file {
                Ok(log_file) => {
                    let stderr_log = log_file.try_clone().unwrap_or_else(|_| File::create(&log_path).expect("failed to create log file"));
                    cmd.stdout(Stdio::from(log_file));
                    cmd.stderr(Stdio::from(stderr_log));
                }
                Err(_) => {
                    cmd.stdout(Stdio::inherit());
                    cmd.stderr(Stdio::inherit());
                }
            }
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            info!("launching administrator (main class {})", self.main_class);
            cmd.spawn()?;
        }

        Ok(())
    }
}

/// Verify the java binary the connection will use is runnable, before doing any
/// network work. Resolves the same binary as `run()` (the connection's Java Home
/// if set, otherwise `java` on PATH) and runs a cheap `java -version`.
pub fn check_java_available(java_home: &str) -> Result<(), Error> {
    let java_home = java_home.trim();
    let java_bin = if java_home.is_empty() {
        PathBuf::from("java")
    } else {
        PathBuf::from(java_home).join("bin").join("java")
    };

    let mut cmd = Command::new(&java_bin);
    cmd.arg("-version");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    match cmd.output() {
        Ok(_) => Ok(()),
        Err(_) => {
            let location = if java_home.is_empty() {
                "on PATH".to_string()
            } else {
                format!("at {}", java_bin.display())
            };
            Err(Error::msg(format!(
                "Java (with JavaFX) not found {}. Set Java Home to a JavaFX-enabled JDK, or put one on PATH.",
                location
            )))
        }
    }
}

/// Read a child stream line by line and push each line into the console buffer.
/// Runs on its own thread; exits at EOF or on read error. Returns the join
/// handle so the reaper can wait for the final output before posting exit.
fn spawn_console_reader<R: Read + Send + 'static>(
    reader: R,
    stream: &'static str,
    buf: Arc<Mutex<crate::console::ConsoleBuf>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut r = BufReader::new(reader);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            // read_until tolerates non-UTF-8 bytes (e.g. platform-encoded output
            // on Windows); decode lossily so a single bad byte can't kill the
            // reader and silently truncate the rest of the console.
            match r.read_until(b'\n', &mut bytes) {
                Ok(0) => break,
                Ok(_) => {
                    while matches!(bytes.last(), Some(b'\n') | Some(b'\r')) {
                        bytes.pop();
                    }
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    crate::console::push_line(&buf, stream, text);
                }
                Err(_) => break,
            }
        }
    })
}

/// Sanitize a string for use as a filesystem path component.
/// Lowercase, replace dots with underscores, other non-alphanumeric with hyphens,
/// then trim leading/trailing separators.
fn sanitize_for_path(s: &str) -> String {
    let sanitized: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c
            } else if c == '.' {
                '_'
            } else {
                '-'
            }
        })
        .collect();
    sanitized
        .trim_matches(|c: char| c == '-' || c == '_')
        .to_string()
}

struct JarTask {
    url: String,
    file_path: PathBuf,
    hash: Option<String>,
}

fn download_jars(
    resources_node: &Node,
    client: &Client,
    dir_path: &Path,
    base_url: &str,
    on_progress: &Channel<serde_json::Value>,
) -> Result<(), Error> {
    let mut tasks = Vec::new();
    let core_dir = dir_path.join("core");
    collect_jar_tasks(resources_node, client, &core_dir, base_url, dir_path, &mut tasks, on_progress)?;

    let _ = on_progress.send(serde_json::json!({
        "message": format!("Checking {} cached files...", tasks.len()),
    }));
    let mut to_download = Vec::new();
    for task in &tasks {
        if has_file_changed(&task.file_path, task.hash.as_deref())? {
            to_download.push(task);
        }
    }

    if to_download.is_empty() {
        return Ok(());
    }

    let total = to_download.len();
    for (i, task) in to_download.iter().enumerate() {
        let mut resp = client.get(&task.url).send()?;
        // Download to a temp file then rename, so a truncated download never
        // leaves a usable (partial) jar to be put on the classpath next launch.
        // The classpath scan only picks `.jar`, so an orphaned `.part` is ignored.
        let mut tmp = task.file_path.clone().into_os_string();
        tmp.push(".part");
        let tmp = PathBuf::from(tmp);
        {
            let mut f = File::create(&tmp)?;
            resp.copy_to(&mut f)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &task.file_path)?;
        let _ = on_progress.send(serde_json::json!({
            "message": format!("Downloaded ({}/{})", i + 1, total),
        }));
    }

    Ok(())
}

/// Collect JAR download tasks from a JNLP resources node.
/// `jar_output_dir` is where JAR files for this level are stored.
/// `cache_root` is the top-level cache dir (for creating extension subdirectories).
fn collect_jar_tasks(
    resources_node: &Node,
    client: &Client,
    jar_output_dir: &Path,
    base_url: &str,
    cache_root: &Path,
    tasks: &mut Vec<JarTask>,
    on_progress: &Channel<serde_json::Value>,
) -> Result<(), Error> {
    for n in resources_node.children() {
        let jar = n.has_tag_name("jar");
        let extension = n.has_tag_name("extension");

        if !jar && !extension {
            continue;
        }

        let href = match n.attribute("href") {
            Some(h) => h,
            None => continue,
        };
        let url = format!("{}/{}", base_url, href);

        if jar {
            let file_name = get_file_name_from_path(href);
            if !is_safe_basename(file_name) {
                warn!("skipping jar with unsafe href: {}", href);
                continue;
            }
            let file_path = jar_output_dir.join(file_name);
            let hash = n.attribute("sha256").map(|s| s.to_string());
            tasks.push(JarTask { url, file_path, hash });
        } else if extension {
            let ext_name = get_file_name_from_path(href);
            if !is_safe_basename(ext_name) {
                warn!("skipping extension with unsafe href: {}", href);
                continue;
            }
            let ext_dir_name = ext_name.strip_suffix(".jnlp").unwrap_or(ext_name);
            let ext_dir = cache_root.join("extensions").join(ext_dir_name);
            if !ext_dir.exists() {
                std::fs::create_dir_all(&ext_dir)?;
            }

            let _ = on_progress.send(serde_json::json!({
                "message": format!("Fetching extension {}...", ext_dir_name),
            }));
            let r = client.get(url).send()?;
            let data = r.text()?;

            let doc = roxmltree::Document::parse(&data)?;
            let root = doc.root();
            let ext_base_url = format!("{}/webstart/extensions", base_url);
            if let Some(resources_node) = get_node(&root, "resources") {
                collect_jar_tasks(&resources_node, client, &ext_dir, &ext_base_url, cache_root, tasks, on_progress)?;
            }
        }
    }
    Ok(())
}

/// Filter JNLP java-vm-args to block flags that could execute arbitrary code.
fn sanitize_vm_args(args: &str) -> String {
    let dangerous_prefixes: &[&str] = &[
        "-javaagent:",
        "-agentpath:",
        "-agentlib:",
        "-xbootclasspath",
        "-xx:onoutofmemoryerror",
        "-xx:onerror",
    ];

    args.split_whitespace()
        .filter(|arg| {
            let lower = arg.to_lowercase();
            let blocked = dangerous_prefixes.iter().any(|p| lower.starts_with(p));
            if blocked {
                info!("sanitize_vm_args: dropping dangerous flag: {}", arg);
            }
            !blocked
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn get_file_name_from_path(p: &str) -> &str {
    // Split on both separators: a server-supplied href could use '\' to escape
    // the cache directory on Windows.
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// A basename is safe to join under the cache only if it has no path separators
/// and is not a traversal component.
fn is_safe_basename(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

fn get_client_args(root: &Node) -> Vec<String> {
    root.descendants()
        .filter(|n| n.has_tag_name("argument"))
        .filter_map(|n| n.text().map(|t| t.to_string()))
        .collect()
}

fn get_j2ses(resources: &Node) -> Option<Vec<J2se>> {
    let j2ses: Vec<J2se> = resources
        .descendants()
        .filter(|n| n.has_tag_name("j2se"))
        .filter_map(|n| {
            let java_vm_args = n.attribute("java-vm-args")?;
            let version = n.attribute("version")?;
            Some(J2se {
                java_vm_args: Some(java_vm_args.to_string()),
                version: version.to_string(),
            })
        })
        .collect();

    if j2ses.is_empty() { None } else { Some(j2ses) }
}

fn get_node<'a>(root: &'a Node, tag_name: &str) -> Option<Node<'a, 'a>> {
    root.descendants().find(|n| n.has_tag_name(tag_name))
}

pub(crate) fn normalize_url(u: &str) -> Result<String, Error> {
    let parsed_url = Url::parse(u)?;
    let mut reconstructed_url = String::with_capacity(u.len());
    reconstructed_url.push_str(parsed_url.scheme());
    reconstructed_url.push_str("://");
    let host = parsed_url.host_str().map_or("", |h| h);
    reconstructed_url.push_str(host);
    if let Some(port) = parsed_url.port() {
        reconstructed_url.push_str(&format!(":{}", port));
    }
    reconstructed_url.push('/');
    for pp in parsed_url.path().split_terminator("/") {
        if !pp.is_empty() {
            reconstructed_url.push_str(pp);
            reconstructed_url.push('/');
        }
    }
    reconstructed_url.pop(); // remove trailing /
    Ok(reconstructed_url)
}

fn sha256_of_file(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(BASE64.encode(hasher.finalize()))
}

fn has_file_changed(jar_file_path: &Path, hash_in_jnlp: Option<&str>) -> Result<bool, Error> {
    if !jar_file_path.exists() {
        return Ok(true);
    }
    if let Some(hash_in_jnlp) = hash_in_jnlp {
        if let Some(current_hash) = sha256_of_file(jar_file_path) {
            return Ok(hash_in_jnlp != current_hash.as_str());
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{get_file_name_from_path, is_safe_basename, normalize_url, sanitize_for_path};
    use anyhow::Error;

    #[test]
    fn sanitize_for_path_strips_traversal() {
        assert_eq!(sanitize_for_path("../../etc"), "etc");
        assert_eq!(sanitize_for_path("..\\..\\x"), "x");
        assert_eq!(sanitize_for_path("Open Integration Engine"), "open-integration-engine");
        assert_eq!(sanitize_for_path("a.b.c"), "a_b_c");
        let s = sanitize_for_path("foo/../bar");
        assert!(!s.contains('/') && !s.contains('\\'));
    }

    #[test]
    fn basename_splits_both_separators() {
        assert_eq!(get_file_name_from_path("a/b/c.jar"), "c.jar");
        assert_eq!(get_file_name_from_path("a\\b\\c.jar"), "c.jar");
        assert_eq!(get_file_name_from_path("plain.jar"), "plain.jar");
    }

    #[test]
    fn is_safe_basename_rejects_traversal() {
        assert!(is_safe_basename("core.jar"));
        assert!(!is_safe_basename(""));
        assert!(!is_safe_basename("."));
        assert!(!is_safe_basename(".."));
        assert!(!is_safe_basename("a/b"));
        assert!(!is_safe_basename("a\\b"));
    }

    #[test]
    pub fn test_normalize_url() -> Result<(), Error> {
        let candidates = [
            ("https://localhost:8443", "https://localhost:8443"),
            ("https://localhost:8443/", "https://localhost:8443"),
            ("https://localhost:8443//", "https://localhost:8443"),
            (
                "https://localhost:8443//a///bv",
                "https://localhost:8443/a/bv",
            ),
        ];

        for (src, expected) in candidates {
            let reconstructed_url = normalize_url(src)?;
            assert_eq!(expected, &reconstructed_url);
        }
        Ok(())
    }
}
