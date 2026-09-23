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

use anyhow::Error;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use log::{info, warn};
use reqwest::blocking::Client;
use reqwest::Url;
use roxmltree::Node;
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;

use crate::connection::ConnectionEntry;

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
    /// When false, a cache dir that already holds *core* jars whose contents
    /// differ from what this server's JNLP declares (a foreign-engine collision
    /// under the same engine-type + version) aborts with [`CacheMismatch`] so
    /// the operator can confirm. When true, the operator has acknowledged it and
    /// the differing jars are overwritten. Differing extension jars never abort;
    /// they just re-download.
    pub acknowledge_cache_mismatch: bool,
}

/// Returned by `load` when the cache directory for this engine-type + version
/// already contains *core* jars whose contents differ from what the server's
/// JNLP declares. A given engine version always ships the identical core jar
/// set, so a core difference under the same version means a *different* engine's
/// jars are in this shared directory, which usually means two connections share
/// an engine type but point at different engines. Extension jars are excluded:
/// they are installed and upgraded independently of the engine version, so a
/// changed extension jar is an upgrade, not a collision. Carried up as an
/// `anyhow::Error` and downcast by the launch command into a distinct frontend
/// code.
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

impl WebstartFile {
    pub fn load(config: LoadConfig) -> Result<WebstartFile, Error> {
        // The address may be the JNLP itself, in which case base_url becomes the
        // directory it lives in. See split_jnlp_url.
        let (webstart, base_url) = split_jnlp_url(config.base_url)?;
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
            let ctx = JarCollectCtx {
                client: &client,
                cache_root: &jar_dir,
                on_progress: config.on_progress,
            };
            classpath_jars = download_jars(
                &ctx,
                &resources_node,
                &base_url,
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
    ) -> Result<(), Error> {
        let classpath_separator = if cfg!(windows) { ";" } else { ":" };
        let classpath = self.classpath(classpath_separator);

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
            .args(build_client_args(
                &self.args,
                ce.username.as_deref(),
                ce.password.as_deref(),
            ));

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
pub(crate) fn sanitize_for_path(s: &str) -> String {
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
    /// A core (engine) jar rather than an extension jar. Only core jars can
    /// mark a cache dir as holding a foreign engine's files; see
    /// [`classify_cached_jar`].
    is_core: bool,
}

// Internal download helper. `engine_type` and `version` are only labels for the
// CacheMismatch it may return; everything else a collection pass needs is in
// `ctx`.
fn download_jars(
    ctx: &JarCollectCtx,
    resources_node: &Node,
    base_url: &str,
    acknowledge_cache_mismatch: bool,
    engine_type: &str,
    version: &str,
) -> Result<Vec<PathBuf>, Error> {
    let mut tasks = Vec::new();
    let core_dir = ctx.cache_root.join("core");
    collect_jar_tasks(ctx, resources_node, &core_dir, base_url, &mut tasks, true)?;

    // Classpath order follows the JNLP jar declaration order: JarTasks are
    // collected in document order (core first, then each extension's jars).
    // This must NOT be re-sorted; Mirth relies on overlay jars preceding their
    // stock counterparts. Includes cache-hit jars, not just freshly downloaded.
    let classpath_jars: Vec<PathBuf> = tasks.iter().map(|t| t.file_path.clone()).collect();

    let _ = ctx.on_progress.send(serde_json::json!({
        "message": format!("Checking {} cached files...", tasks.len()),
    }));

    // Single hash pass over the cached jars. classify_cached_jar reads each file
    // at most once and decides BOTH whether it needs (re)downloading and whether
    // it is evidence of a foreign engine (present, has a declared hash, and the
    // on-disk content does not match).
    //
    // Only CORE jars count as that evidence. A given engine version always ships
    // the identical *core* jar set, so a core mismatch really does mean two
    // connections share an engine type + version while pointing at different
    // engines (same cache dir), which is worth stopping for.
    //
    // Extension jars are not evidence and never abort a launch. They are
    // installed and upgraded independently of the engine version, so an
    // extension whose jar name carries no version (tlsmanager-client.jar) simply
    // changes content in place when it is upgraded. Treating that as a foreign
    // engine aborted the launch and sent operators off to delete the cache dir
    // by hand, when the right answer was always just to download the new bytes.
    // A differing extension jar still sets needs_download, so it refreshes.
    let mut to_download = Vec::new();
    let mut foreign = Vec::new();
    let total_tasks = tasks.len();
    for (i, task) in tasks.iter().enumerate() {
        // Hashing every cached jar reads hundreds of MB, so report as we go
        // rather than leaving the status bar on one message for the whole pass.
        // The last file gets a tick too, so the bar does not sit on a stale
        // count through the tail of the pass.
        if (i % 25 == 0 && i > 0) || i + 1 == total_tasks {
            let _ = ctx.on_progress.send(serde_json::json!({
                "message": format!("Checking cached files ({}/{})...", i + 1, total_tasks),
            }));
        }
        let (needs_download, is_foreign) =
            classify_cached_jar(&task.file_path, task.hash.as_deref(), task.is_core);
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
        // Sent BEFORE the request, naming the file. copy_to() below blocks for
        // the whole transfer, so reporting only on completion left the status
        // bar silent for the entire download of each jar, which reads as a
        // stall on a slow link.
        let name = task
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let _ = ctx.on_progress.send(serde_json::json!({
            "message": format!("Downloading {} ({}/{})...", name, i + 1, total),
        }));
        let mut resp = ctx.client.get(&task.url).send()?;
        // Download to a temp file then rename, so a truncated download never
        // leaves a usable (partial) jar to be put on the classpath next launch.
        // The classpath scan only picks `.jar`, so an orphaned `.part` is ignored.
        //
        // The temp name carries a unique suffix because two launches can be in
        // their download phase at once: different connections sharing an engine
        // type + version resolve to the same cache dir, and after a cache wipe
        // or an engine upgrade they both fetch the same missing jars. A fixed
        // `.part` meant both truncated and wrote the same file, then both
        // renamed, leaving a corrupt jar on the classpath of whichever launch
        // lost. The next launch's hash check repairs it, which is precisely what
        // makes it expensive to debug.
        let mut tmp = task.file_path.clone().into_os_string();
        tmp.push(format!(
            ".part.{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let tmp = PathBuf::from(tmp);
        {
            let mut f = File::create(&tmp)?;
            resp.copy_to(&mut f)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &task.file_path)?;
        if i + 1 == total {
            let _ = ctx.on_progress.send(serde_json::json!({
                "message": format!("Downloaded {} file(s)", total),
            }));
        }
    }

    Ok(classpath_jars)
}

/// The parts of a collection pass that do not change as it recurses into
/// `<extension>` elements. Same reason [`LoadConfig`] exists: the per-level
/// arguments are the interesting ones, and threading the invariants through
/// every call obscures that.
struct JarCollectCtx<'a> {
    client: &'a Client,
    /// Top-level cache dir, for creating extension subdirectories.
    cache_root: &'a Path,
    on_progress: &'a Channel<serde_json::Value>,
}

/// Collect JAR download tasks from a JNLP resources node.
/// `jar_output_dir` is where JAR files for this level are stored.
/// `is_core` marks the jars collected at this level as engine jars; the
/// recursion into an `<extension>` passes false.
fn collect_jar_tasks(
    ctx: &JarCollectCtx,
    resources_node: &Node,
    jar_output_dir: &Path,
    base_url: &str,
    tasks: &mut Vec<JarTask>,
    is_core: bool,
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
            tasks.push(JarTask { url, file_path, hash, is_core });
        } else if extension {
            let ext_name = get_file_name_from_path(href);
            if !is_safe_basename(ext_name) {
                warn!("skipping extension with unsafe href: {}", href);
                continue;
            }
            let ext_dir_name = ext_name.strip_suffix(".jnlp").unwrap_or(ext_name);
            let ext_dir = ctx.cache_root.join("extensions").join(ext_dir_name);
            if !ext_dir.exists() {
                std::fs::create_dir_all(&ext_dir)?;
            }

            let _ = ctx.on_progress.send(serde_json::json!({
                "message": format!("Fetching extension {}...", ext_dir_name),
            }));
            let r = ctx.client.get(url).send()?;
            let data = r.text()?;

            let doc = roxmltree::Document::parse(&data)?;
            let root = doc.root();
            // Resolve the extension's own jars against the directory it was
            // actually fetched from. Hardcoding "webstart/extensions" happened to
            // match every standard engine, so it never fired, but it silently
            // pointed elsewhere the moment a server nested extensions deeper.
            let ext_dir_href = get_dir_from_path(href);
            let ext_base_url = if ext_dir_href.is_empty() {
                base_url.to_string()
            } else {
                format!("{}/{}", base_url, ext_dir_href)
            };
            if let Some(resources_node) = get_node(&root, "resources") {
                collect_jar_tasks(ctx, &resources_node, &ext_dir, &ext_base_url, tasks, false)?;
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

/// The directory part of an href, or "" when it names a file with no directory.
/// Splits on both separators for the same reason `get_file_name_from_path` does.
fn get_dir_from_path(p: &str) -> &str {
    match p.rfind(['/', '\\']) {
        Some(i) => &p[..i],
        None => "",
    }
}

/// Split a connection address into the JNLP URL to fetch and the base URL that
/// relative hrefs inside it resolve against.
///
/// The engine's own landing page tells operators to point their launcher at
/// `https://host:port/webstart.jnlp`, so an address ending in a `.jnlp` file is
/// the JNLP itself and must be used as given rather than treated as a base to
/// append to. Appending produced `.../webstart.jnlp/webstart.jnlp`, which the
/// server answered with a component descriptor, so the launch died on the
/// confusing "not an application-desc node" error (#21).
///
/// Honouring it means the base for hrefs becomes the directory the JNLP was
/// found in, not the address: jars and extensions are relative to the file's
/// location. Anything not ending in `.jnlp` is a base and gets the default file
/// name appended, so a context path like `/mirth` keeps working unchanged.
///
/// Using the address as given also means a server that serves its JNLP from a
/// non-default path is supported, which is why this honours the address instead
/// of stripping the file name off and re-appending the default.
fn split_jnlp_url(address: &str) -> Result<(String, String), Error> {
    let normalized = normalize_url(address)?;
    // Detect on the parsed path, not the whole string: a host that happens to
    // end in ".jnlp" has no path and must not be mistaken for a file name.
    let parsed = Url::parse(&normalized)?;
    let last_segment = parsed.path().rsplit('/').next().unwrap_or("");
    let is_jnlp_file = last_segment.len() > 5
        && last_segment[last_segment.len() - 5..].eq_ignore_ascii_case(".jnlp");

    if is_jnlp_file {
        // normalize_url leaves no trailing slash, so cutting at the last '/'
        // yields the directory holding the JNLP. The detection above guarantees
        // there is a path segment, so this '/' is always past the authority.
        let cut = normalized
            .rfind('/')
            .ok_or_else(|| Error::msg("internal error: jnlp address has no path separator"))?;
        let base = normalized[..cut].to_string();
        Ok((normalized, base))
    } else {
        Ok((format!("{}/webstart.jnlp", normalized), normalized))
    }
}

/// A basename is safe to join under the cache only if it has no path separators
/// and is not a traversal component.
fn is_safe_basename(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// The administrator's own argument list: every JNLP `<argument>` in document
/// order, then the optional username and password.
///
/// Forwarding the JNLP arguments untouched is the launcher's entire role in
/// client-side certificate pinning. The server puts `-trust <thumbprint>` in
/// the JNLP and we pass the baton rather than reinterpreting it
/// (OpenIntegrationEngine/engine#280, launcher #18). The client's parser
/// consumes flags wherever they appear and advances its positional index only
/// on non-flag tokens, so username and password bind to positions 2 and 3 only
/// as long as they stay last and JNLP order is preserved. Reordering here, or
/// inserting an argument of our own, breaks client pinning silently: the launch
/// still succeeds and the trust value lands in the wrong slot.
///
/// A password without a username is dropped, since it would otherwise bind to
/// the username position.
fn build_client_args(
    jnlp_args: &[String],
    username: Option<&str>,
    password: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = jnlp_args.to_vec();
    if let Some(username) = username {
        args.push(username.to_string());
        if let Some(password) = password {
            args.push(password.to_string());
        }
    }
    args
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
/// - present, hash differs: needs download, and foreign only when `is_core`.
///   A given engine version always ships the identical core jar set, so a core
///   jar with different content belongs to a different engine. Extensions are
///   installed and upgraded independently of the engine version, so a changed
///   extension jar means an extension was upgraded, not that the cache is
///   foreign. It re-downloads without aborting the launch.
/// - present but unreadable: treated as unchanged (matches prior behavior).
fn classify_cached_jar(
    jar_file_path: &Path,
    hash_in_jnlp: Option<&str>,
    is_core: bool,
) -> (bool, bool) {
    if !jar_file_path.exists() {
        return (true, false);
    }
    match hash_in_jnlp {
        None => (false, false),
        Some(declared) => match sha256_of_file(jar_file_path) {
            None => (false, false),
            Some(on_disk) => {
                let differs = on_disk.as_str() != declared;
                (differs, differs && is_core)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_client_args, classify_cached_jar, collect_jar_tasks, get_dir_from_path,
        get_file_name_from_path,
        get_node, is_safe_basename, normalize_url, sanitize_for_path, sha256_of_file,
        split_jnlp_url, Channel, WebstartFile,
    };
    use anyhow::Error;
    use std::path::PathBuf;

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
        assert_eq!(classify_cached_jar(&jar, Some(&real), true), (false, false));
        // CORE, present + differing declared hash -> download AND foreign
        assert_eq!(classify_cached_jar(&jar, Some("not-the-hash"), true), (true, true));
        // EXTENSION, present + differing declared hash -> download, NEVER foreign.
        // An extension upgraded in place under a stable jar name (e.g.
        // tlsmanager-client.jar) must refresh, not abort the launch.
        assert_eq!(classify_cached_jar(&jar, Some("not-the-hash"), false), (true, false));
        // present + no declared hash -> keep, not foreign
        assert_eq!(classify_cached_jar(&jar, None, true), (false, false));
        // missing file -> download, not foreign
        assert_eq!(classify_cached_jar(&dir.join("missing.jar"), Some("x"), true), (true, false));
        assert_eq!(classify_cached_jar(&dir.join("missing.jar"), Some("x"), false), (true, false));

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

    /// Minimal one-shot HTTP responder on an ephemeral port. Enough for
    /// `collect_jar_tasks`, which fetches exactly one thing over the network:
    /// the extension's JNLP. The jars themselves are downloaded later, by
    /// `download_jars`, so nothing else is ever requested here.
    fn serve_once(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request head, otherwise the client can see a reset
                // instead of the response.
                let peek = stream.try_clone().expect("clone stream");
                let mut reader = BufReader::new(peek);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    line.clear();
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://{}", addr), handle)
    }

    /// The extension-upgrade fix lives in the `is_core` TAG, not in
    /// `classify_cached_jar`. If the recursion into `<extension>` ever passes
    /// `true` again, every other test in this file still passes and the bug is
    /// back: an extension upgraded under a stable jar name would once more be
    /// read as a foreign engine and abort the launch. This pins the tagging.
    #[test]
    fn collect_jar_tasks_tags_extension_jars_as_non_core() {
        const EXT_JNLP: &str = concat!(
            "<?xml version=\"1.0\"?>\n",
            "<jnlp><resources>\n",
            "  <jar href=\"libs/foo/foo-client-1.0.0.jar\" sha256=\"ZXh0\"/>\n",
            "</resources></jnlp>"
        );
        const CORE_JNLP: &str = concat!(
            "<?xml version=\"1.0\"?>\n",
            "<jnlp><resources>\n",
            "  <jar href=\"webstart/client-lib/mirth-client.jar\" sha256=\"Y29yZQ==\"/>\n",
            "  <extension href=\"webstart/extensions/foo.jnlp\"/>\n",
            "</resources></jnlp>"
        );

        let (base_url, server) = serve_once(EXT_JNLP);
        let doc = roxmltree::Document::parse(CORE_JNLP).expect("parse core jnlp");
        let root = doc.root();
        let resources = get_node(&root, "resources").expect("resources node");

        let cache_root = std::env::temp_dir().join(format!("launcher-cjt-{}", std::process::id()));
        let core_dir = cache_root.join("core");
        std::fs::create_dir_all(&core_dir).expect("create core dir");

        // With no timeout a mock that dies before responding hangs the test
        // run instead of failing it. The thread serve_once spawns needs no
        // cleanup: it is detached and dies with the process.
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("build test client");
        let on_progress: Channel<serde_json::Value> = Channel::new(|_| Ok(()));
        let mut tasks = Vec::new();
        let ctx = super::JarCollectCtx {
            client: &client,
            cache_root: &cache_root,
            on_progress: &on_progress,
        };
        collect_jar_tasks(&ctx, &resources, &core_dir, &base_url, &mut tasks, true)
            .expect("collect_jar_tasks");
        server.join().ok();

        // Document order: core jars first, then each extension's jars.
        assert_eq!(tasks.len(), 2, "one core jar + one extension jar");

        assert!(tasks[0].is_core, "core jar must be tagged is_core");
        assert!(
            tasks[0].file_path.ends_with("core/mirth-client.jar"),
            "core jar path was {:?}",
            tasks[0].file_path
        );

        assert!(
            !tasks[1].is_core,
            "extension jar must NOT be tagged is_core, or an extension upgrade \
             aborts the launch again"
        );
        assert!(
            tasks[1].file_path.ends_with("extensions/foo/foo-client-1.0.0.jar"),
            "extension jar path was {:?}",
            tasks[1].file_path
        );

        // And the tag feeds through to the classification. The file has to
        // exist with the WRONG bytes for this to mean anything: on a missing
        // file classify_cached_jar returns (true, false) before it ever looks
        // at is_core, so the assertion would pass either way.
        std::fs::write(&tasks[1].file_path, b"not the declared content").expect("write jar");
        assert_eq!(
            classify_cached_jar(&tasks[1].file_path, tasks[1].hash.as_deref(), tasks[1].is_core),
            (true, false),
            "a changed extension jar re-downloads without being called foreign"
        );

        std::fs::remove_dir_all(&cache_root).ok();
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

    /// The engine's landing page hands operators a URL ending in webstart.jnlp,
    /// so that form must be used as given rather than appended to (#21). A base
    /// address, with or without a context path, must keep behaving as before.
    ///
    /// This pins the function, NOT the call site: reverting load() to append
    /// unconditionally leaves this test green. What stops that regression is
    /// structural rather than a test, so keep it that way: the only
    /// "{}/webstart.jnlp" append in this file lives inside split_jnlp_url, so
    /// there is no second path to the JNLP url to drift out of step.
    #[test]
    fn split_jnlp_url_honours_an_address_that_is_already_a_jnlp() -> Result<(), Error> {
        let candidates = [
            // (address, expected jnlp url, expected href base)
            ("https://h:8443", "https://h:8443/webstart.jnlp", "https://h:8443"),
            ("https://h:8443/", "https://h:8443/webstart.jnlp", "https://h:8443"),
            (
                "https://h:8443/webstart.jnlp",
                "https://h:8443/webstart.jnlp",
                "https://h:8443",
            ),
            // Trailing slash after the file name still names the file.
            (
                "https://h:8443/webstart.jnlp/",
                "https://h:8443/webstart.jnlp",
                "https://h:8443",
            ),
            // Case is not meaningful in the extension.
            (
                "https://h:8443/WEBSTART.JNLP",
                "https://h:8443/WEBSTART.JNLP",
                "https://h:8443",
            ),
            // A context path is a base, not a file.
            (
                "https://h:8443/mirth",
                "https://h:8443/mirth/webstart.jnlp",
                "https://h:8443/mirth",
            ),
            // A non-default JNLP path is honoured, and its directory is the base.
            (
                "https://h:8443/foo/custom.jnlp",
                "https://h:8443/foo/custom.jnlp",
                "https://h:8443/foo",
            ),
            // A host ending in .jnlp has no path and must not look like a file.
            (
                "https://webstart.jnlp",
                "https://webstart.jnlp/webstart.jnlp",
                "https://webstart.jnlp",
            ),
        ];

        for (address, expected_jnlp, expected_base) in candidates {
            let (jnlp, base) = split_jnlp_url(address)?;
            assert_eq!(expected_jnlp, &jnlp, "jnlp url for {}", address);
            assert_eq!(expected_base, &base, "href base for {}", address);
        }
        Ok(())
    }

    /// An extension's jars resolve against the directory the extension JNLP was
    /// fetched from. This nests one level deeper than the standard layout, which
    /// is the only arrangement that tells the two apart: the old hardcoded
    /// "webstart/extensions" matched the standard layout exactly, so nothing
    /// caught it.
    #[test]
    fn extension_jars_resolve_against_the_extension_href_directory() {
        const EXT_JNLP: &str = concat!(
            "<?xml version=\"1.0\"?>\n",
            "<jnlp><resources>\n",
            "  <jar href=\"libs/foo/foo-client-1.0.0.jar\" sha256=\"ZXh0\"/>\n",
            "</resources></jnlp>"
        );
        const CORE_JNLP: &str = concat!(
            "<?xml version=\"1.0\"?>\n",
            "<jnlp><resources>\n",
            "  <extension href=\"webstart/extensions/vendor/foo.jnlp\"/>\n",
            "</resources></jnlp>"
        );

        let (base_url, server) = serve_once(EXT_JNLP);
        let doc = roxmltree::Document::parse(CORE_JNLP).expect("parse core jnlp");
        let root = doc.root();
        let resources = get_node(&root, "resources").expect("resources node");

        let cache_root = std::env::temp_dir().join(format!("launcher-extbase-{}", std::process::id()));
        let core_dir = cache_root.join("core");
        std::fs::create_dir_all(&core_dir).expect("create core dir");

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("build test client");
        let on_progress: Channel<serde_json::Value> = Channel::new(|_| Ok(()));
        let mut tasks = Vec::new();
        let ctx = super::JarCollectCtx {
            client: &client,
            cache_root: &cache_root,
            on_progress: &on_progress,
        };
        collect_jar_tasks(&ctx, &resources, &core_dir, &base_url, &mut tasks, true)
            .expect("collect_jar_tasks");
        server.join().ok();

        assert_eq!(tasks.len(), 1, "the extension contributes its one jar");
        assert_eq!(
            format!("{}/webstart/extensions/vendor/libs/foo/foo-client-1.0.0.jar", base_url),
            tasks[0].url,
            "extension jar must resolve under the extension's own directory"
        );

        std::fs::remove_dir_all(&cache_root).ok();
    }

    /// Client-side cert pinning depends on this argv contract and nothing in the
    /// launcher would fail if it broke: the administrator still starts, it just
    /// parses `-trust` into the wrong slot. The realistic shape below comes from
    /// OpenIntegrationEngine/engine#280 via launcher #18.
    #[test]
    fn build_client_args_forwards_jnlp_order_then_appends_credentials() {
        let jnlp: Vec<String> = ["server", "version", "-ssl", "p", "c", "-trust", "THUMB"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let args = build_client_args(&jnlp, Some("admin"), Some("secret"));
        assert_eq!(
            vec!["server", "version", "-ssl", "p", "c", "-trust", "THUMB", "admin", "secret"],
            args,
            "JNLP arguments must keep document order, credentials go last"
        );

        // Deliberately NOT asserted here: how the client parses that argv. The
        // flags' arities are the client's business, and #18's whole point is
        // that the launcher passes the baton rather than reinterpreting it.
        // Modelling the parser here would pin our guess about it, and would
        // break this test whenever the client changed for reasons that have
        // nothing to do with the launcher.

        // No credentials configured: the JNLP arguments are forwarded alone.
        assert_eq!(jnlp, build_client_args(&jnlp, None, None));

        // A password with no username would bind to the username position.
        assert_eq!(jnlp, build_client_args(&jnlp, None, Some("secret")));

        // Username alone is valid; the administrator prompts for the password.
        let mut expected = jnlp.clone();
        expected.push("admin".to_string());
        assert_eq!(expected, build_client_args(&jnlp, Some("admin"), None));
    }

    #[test]
    fn get_dir_from_path_returns_the_directory_or_empty() {
        assert_eq!("webstart/extensions", get_dir_from_path("webstart/extensions/foo.jnlp"));
        assert_eq!(
            "webstart/extensions/vendor",
            get_dir_from_path("webstart/extensions/vendor/foo.jnlp")
        );
        assert_eq!("", get_dir_from_path("foo.jnlp"));
        // Both separators, matching get_file_name_from_path.
        assert_eq!("a\\b", get_dir_from_path("a\\b\\foo.jnlp"));
    }
}
