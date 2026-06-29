# Launcher

Tauri v2 desktop app for launching and managing integration engine administrator instances. Rust backend + Nuxt 4/Vue 3 frontend. Originally forked from [kayyagari/ballista](https://github.com/kayyagari/ballista).

## Build

```bash
npm install          # frontend dependencies
npm run tauri build  # full build (frontend + Rust + .app/.dmg bundle)
```

Rust-only check/test (from repo root):
```bash
cargo check
cargo test
```

## Project Structure

- `src-tauri/src/` — Rust backend (Tauri commands, TLS cert pinning, webstart/JNLP handling)
  - `main.rs` — Tauri command handlers and app setup
  - `connection.rs` — ConnectionStore, connection persistence, per-connection cert pin storage
  - `webstart.rs` — JNLP parsing, jar downloading, Java process launching
  - `tls.rs` — per-connection TLS certificate pinning (trust-on-first-use) and the pinned reqwest client
  - `console.rs` — native console subsystem (streams the admin process stdout/stderr to a Tauri window)
- `app/` — Nuxt 4 frontend (pages, components, composables, types)

## Conventions

- Tauri commands use `rename_all = "snake_case"` — JS side must use snake_case parameter names
- Self-signed certs are expected; the trust boundary is **per-connection TLS certificate pinning** (trust-on-first-use). First connect prompts the operator to trust the server's leaf-cert SHA-256; later connects reject a changed cert. There is no JAR signature verification (the old `verify.rs` was removed). `native-tls` is kept only for the http plugin's connectivity probe; the launch path uses a pinned rustls client.
- The admin console is a native Tauri webview window fed by Rust over a Channel (no bundled Java console jar)
- The administrator is a JavaFX app, so the Java used to launch it (the connection's Java Home, or `java` on PATH) must be a JavaFX-enabled JDK. `launch` fails fast with a clear message if Java can't be found; the launcher does not auto-detect a JDK (uses `JAVA_HOME`/PATH only).
- Rust error handling: prefer `?` operator and `ok_or_else` over `.unwrap()` — return errors to frontend, don't panic
- Mutex locks: use `.expect("descriptive message")` since poisoning is unrecoverable
- Frontend uses Tailwind CSS v4 with `@theme` design tokens in `app/assets/css/main.css`
- Icons: Phosphor Icons via `ph:` prefix

## Remotes

- `origin` — `pacmano1/launcher` (this project's own repo; separated from upstream `kayyagari/ballista`)
