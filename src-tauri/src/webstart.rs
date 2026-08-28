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
    /// When false, a cache dir that already holds jars whose contents differ
    /// from what this server's JNLP declares (a foreign-engine collision under
    /// the same engine-type + version) aborts with [`CacheMismatch`] so the
    /// operator can confirm. When true, the operator has acknowledged it and the
    /// differing jars are overwritten.
    pub acknowledge_cache_mismatch: bool,
}

/// Returned by `load` when the cache directory for this engine-type + version
/// already contains jars whose contents differ from what the server's JNLP
/// declares. Because a given engine version always ships the identical jar set,
/// a content difference under the same version means a *different* engine's jars
/// are in this shared directory, which usually means two connections share an
/// engine type but point at different engines. Carried up as an `anyhow::Error`
/// and downcast by the launch command into a distinct frontend code.
#[derive(Debug)]
pub struct CacheMismatch {
    pub engine_type: String,
    pub version: String,
    pub jars: Vec<String>,
}

impl std::fmt::Display for CacheMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cache for {} {} holds {} file(s) that differ from this server",
            self.engine_type,
            self.version,
            self.jars.len()
        )
    }
}

impl std::error::Error for CacheMismatch {}

#[derive(Debug)]
pub struct WebstartFile {
    main_class: String,
    args: Vec<String>,
    j2ses: Option<Vec<J2se>>,
    logs_dir: PathBuf,
    conn_id: String,
    loaded_at: SystemTime,
    /// Classpath jars in JNLP-declared order. Order is significant: Mirth ships
    /// patched overlay jars (e.g. `rhino-mc-modifications.jar`) whose classes must
    /// shadow their stock counterparts, and the JNLP lists each overlay before its
    /// stock jar. Preserving this order is what makes the overlays win.
    classpath_jars: Vec<PathBuf>,
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
        let mut jnlp_version_raw = "default".to_string();
        if let Some(jnlp_node) = get_node(&root, "jnlp") {
            if let Some(v) = jnlp_node.attribute("version") {
                jnlp_version = v.replace(['/', '\\', '.'], "_");
                jnlp_version_raw = v.to_string();
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
        let mut classpath_jars = Vec::new();
        if let Some(resources_node) = resources_node {
            j2ses = get_j2ses(&resources_node);
            classpath_jars = download_jars(
                &resources_node,
                &client,
                &jar_dir,
                &base_url,
                config.on_progress,
                config.acknowledge_cache_mismatch,
                config.engine_type,
                &jnlp_version_raw,
            )?;
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
            logs_dir: config.logs_dir.clone(),
            conn_id: safe_conn_id,
            args,
            loaded_at: SystemTime::now(),
            j2ses,
            classpath_jars,
        };

        Ok(ws)
    }

    /// Build the classpath string, preserving JNLP-declared jar order.
    ///
    /// Order is significant: Mirth ships patched overlay jars
    /// (rhino/fife/jedit/jersey/staxon/zip4j `-mc-modifications.jar`,
    /// `xpp3-...-modified.jar`) whose classes must shadow their stock
    /// counterparts, and the JNLP lists each overlay before its stock jar. A
    /// previous directory scan + alphabetical sort dropped that order (e.g.
    /// `rhino-1.7.15.1.jar` sorted before `rhino-mc-modifications.jar`), loading
    /// the stock class first and causing IllegalAccessError at runtime. This
    /// MUST NOT sort.
    fn classpath(&self, separator: &str) -> String {
        self.classpath_jars
            .iter()
            .filter_map(|p| p.to_str())
            .collect::<Vec<_>>()
            .join(separator)
    }

    pub fn run(
        &self,
        ce: Arc<ConnectionEntry>,
        console: Option<crate::console::ConsoleSink>,
        icon_bootstrap: Option<(PathBuf, PathBuf)>,
    ) -> Result<(), Error> {
        let classpath_separator = if cfg!(windows) { ";" } else { ":" };
        let mut classpath = self.classpath(classpath_separator);

        // Dock/taskbar icon for the admin process: prepend the bootstrap jar to
        // the classpath and start it instead of the admin's main class; it sets
        // the icon (java.awt.Taskbar, or per-window stamping where unsupported)
        // and then reflectively invokes the real main. Both paths must exist;
        // an icon problem must never stop a launch, so anything missing means
        // we launch the admin directly as before.
        let mut main_class = self.main_class.as_str();
        let mut icon_props: Vec<String> = Vec::new();
        if let Some((ref jar, ref icon)) = icon_bootstrap {
            if jar.is_file() && icon.is_file() {
                classpath = format!("{}{}{}", jar.display(), classpath_separator, classpath);
                icon_props.push(format!("-Dlauncher.icon={}", icon.display()));
                icon_props.push(format!("-Dlauncher.main={}", self.main_class));
                main_class = "IconBootstrap";
            } else {
                warn!("icon bootstrap resources missing ({:?}, {:?}); launching without custom icon", jar, icon);
            }
        }

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

        cmd.args(&icon_props);
        cmd.arg("-cp")
            .arg(classpath)
            .arg(main_class)
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

// Internal download helper; the extra args are the collision-check context
// (labels + the ack flag). Grouping them into a struct for one private fn would
// be more indirection than it's worth.
#[allow(clippy::too_many_arguments)]
fn download_jars(
    resources_node: &Node,
    client: &Client,
    dir_path: &Path,
    base_url: &str,
    on_progress: &Channel<serde_json::Value>,
    acknowledge_cache_mismatch: bool,
    engine_type: &str,
    version: &str,
) -> Result<Vec<PathBuf>, Error> {
    let mut tasks = Vec::new();
    let core_dir = dir_path.join("core");
    collect_jar_tasks(resources_node, client, &core_dir, base_url, dir_path, &mut tasks, on_progress)?;

    // Classpath order follows the JNLP jar declaration order: JarTasks are
    // collected in document order (core first, then each extension's jars).
    // This must NOT be re-sorted; Mirth relies on overlay jars preceding their
    // stock counterparts. Includes cache-hit jars, not just freshly downloaded.
    let classpath_jars: Vec<PathBuf> = tasks.iter().map(|t| t.file_path.clone()).collect();

    let _ = on_progress.send(serde_json::json!({
        "message": format!("Checking {} cached files...", tasks.len()),
    }));

    // Single hash pass over the cached jars. classify_cached_jar reads each file
    // at most once and decides BOTH whether it needs (re)downloading and whether
    // it is a foreign-engine jar (present, has a declared hash, and the on-disk
    // content does not match). Because a given engine version always ships the
    // identical jar set, a content mismatch under the same version can only be a
    // different engine's jar, i.e. two connections sharing an engine type but
    // pointing at different engines (same cache dir). If any are found and the
    // operator has not acknowledged it, abort with CacheMismatch before any
    // download.
    let mut to_download = Vec::new();
    let mut foreign = Vec::new();
    for task in &tasks {
        let (needs_download, is_foreign) =
            classify_cached_jar(&task.file_path, task.hash.as_deref());
        if needs_download {
            to_download.push(task);
        }
        if is_foreign {
            if let Some(name) = task.file_path.file_name().and_then(|n| n.to_str()) {
                foreign.push(name.to_string());
            }
        }
    }

    if !acknowledge_cache_mismatch && !foreign.is_empty() {
        foreign.sort();
        return Err(CacheMismatch {
            engine_type: engine_type.to_string(),
            version: version.to_string(),
            jars: foreign,
        }
        .into());
    }

    if to_download.is_empty() {
        return Ok(classpath_jars);
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

    Ok(classpath_jars)
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

/// Classify one cached jar in a single hash read: `(needs_download, is_foreign)`.
///
/// - missing file: needs download, not foreign.
/// - present, no JNLP hash to compare: keep (not downloaded, not foreign).
/// - present, hash matches: keep.
/// - present, hash differs: needs download AND foreign. A same-named jar with
///   different content is a different engine's jar, since a given engine version
///   always ships the identical jar set.
/// - present but unreadable: treated as unchanged (matches prior behavior).
fn classify_cached_jar(jar_file_path: &Path, hash_in_jnlp: Option<&str>) -> (bool, bool) {
    if !jar_file_path.exists() {
        return (true, false);
    }
    match hash_in_jnlp {
        None => (false, false),
        Some(declared) => match sha256_of_file(jar_file_path) {
            None => (false, false),
            Some(on_disk) => {
                let differs = on_disk.as_str() != declared;
                (differs, differs)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_cached_jar, get_file_name_from_path, is_safe_basename, normalize_url,
        sanitize_for_path, sha256_of_file, WebstartFile,
    };
    use anyhow::Error;
    use std::path::PathBuf;
    use std::time::SystemTime;

    #[test]
    fn classify_cached_jar_detects_foreign_and_missing() {
        use std::fs;
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("launcher-cj-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let jar = dir.join("a.jar");
        fs::File::create(&jar).unwrap().write_all(b"hello world").unwrap();
        let real = sha256_of_file(&jar).unwrap();

        // present + matching declared hash -> keep, not foreign
        assert_eq!(classify_cached_jar(&jar, Some(&real)), (false, false));
        // present + differing declared hash -> download AND foreign
        assert_eq!(classify_cached_jar(&jar, Some("not-the-hash")), (true, true));
        // present + no declared hash -> keep, not foreign
        assert_eq!(classify_cached_jar(&jar, None), (false, false));
        // missing file -> download, not foreign
        assert_eq!(classify_cached_jar(&dir.join("missing.jar"), Some("x")), (true, false));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn classpath_preserves_jnlp_order_and_does_not_sort() {
        // The JNLP lists each patched overlay BEFORE its stock jar. The old
        // directory-scan + alphabetical sort put rhino-1.7.15.1.jar ahead of
        // rhino-mc-modifications.jar, loading the stock (package-private)
        // NativeDate and causing IllegalAccessError. Order must be preserved.
        let ws = WebstartFile {
            main_class: "com.example.Main".to_string(),
            args: vec![],
            j2ses: None,
            logs_dir: PathBuf::from("/tmp/logs"),
            conn_id: "test".to_string(),
            loaded_at: SystemTime::UNIX_EPOCH,
            classpath_jars: vec![
                PathBuf::from("/c/core/rhino-mc-modifications.jar"),
                PathBuf::from("/c/core/rhino-1.7.15.1.jar"),
                PathBuf::from("/c/core/mirth-client.jar"),
            ],
        };
        let cp = ws.classpath(":");
        assert_eq!(
            cp,
            "/c/core/rhino-mc-modifications.jar:/c/core/rhino-1.7.15.1.jar:/c/core/mirth-client.jar"
        );
        assert!(
            cp.find("rhino-mc-modifications").unwrap() < cp.find("rhino-1.7.15.1").unwrap(),
            "patched overlay must precede its stock jar"
        );
    }

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
