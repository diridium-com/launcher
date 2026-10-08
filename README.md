# Launcher

A lean and simple launcher for Open Integration Engine administration.

Originally forked from [Ballista](https://github.com/kayyagari/ballista) by [Kiran Ayyagari](https://github.com/kayyagari). Thank you Kiran for the original project and foundation this builds upon.

> ### Consider the OIE web administrator
>
> Open Integration Engine now has a **browser-based administrator**, with nothing to install on
> each workstation.
>
> ### **https://openintegrationengine.org/web-administrator/**
>
> Worth evaluating before you commit to a desktop launcher. Launcher remains useful if you manage
> many engines from one machine, or need a native app, but for a lot of people the web
> administrator is the simpler answer.

## Requirements

- **A JavaFX-enabled JDK.** The administrator is a JavaFX application, so a plain JRE will not run it.
  Launcher neither installs nor downloads Java. Set a `Java Home` per connection, or let Launcher use
  `JAVA_HOME` or the `java` on your `PATH`.
- **Windows:** Windows 10 or later, or Windows Server 2016 or later, plus the Microsoft Edge WebView2
  Runtime. Windows 11 includes WebView2, and most Windows 10 machines have it by way of Microsoft Edge,
  but Windows Server images frequently do not.
- **macOS:** nothing additional. The system WebView is part of the OS.
- **Linux:** the `.deb` and `.rpm` need `libwebkit2gtk-4.1-0` and `libgtk-3-0` from your distribution.
  The AppImage carries its own, so it is the better choice on a machine without repository access.

### Installing on Windows without internet access

If WebView2 is already installed, the installer leaves it alone and downloads nothing. If it is
missing, the installer downloads it from Microsoft, which needs working internet access.

On a machine with no internet access that download fails and the installation does not finish. The
`.exe` reports `Failed to install WebView2! The app can't run without it. Try restarting the
installer.` and the `.msi` reports a generic Windows Installer error. Restarting does not help, because
nothing on the machine can reach Microsoft.

Install WebView2 first:

1. On a machine that does have internet access, download the **Evergreen Standalone Installer** for the
   target machine's architecture from https://developer.microsoft.com/microsoft-edge/webview2/
2. Copy it to the target machine and run it.
3. Run the Launcher installer again.

Take the Standalone Installer, not the Bootstrapper on the same page. The Bootstrapper downloads the
runtime when it runs, so it fails the same way.

WebView2 is the only thing the installer downloads, and Launcher has no auto-updater.

## How To Use

1. Go to releases and download a suitable installer for your OS platform
2. Create a new connection or import existing connections from `<MCAL-root>/data/connections.json`
3. Launch a connection by double-clicking the desired server, or select it and click the play button
4. Edit a connection by clicking the pencil icon on a server row
5. Adjust the `Java Home` field's value if necessary. The administrator needs a **JavaFX-enabled JDK**; a plain JRE will not run it.

## Features

- Dark theme UI with keyboard zoom support (Cmd/Ctrl +/-/0)
- Real-time server connectivity status
- Sort by group, name, last connected, or status
- Java console output viewer
- Per-connection TLS certificate pinning (trust on first use), for the self-signed certificates these servers usually carry
- Cross-platform: macOS, Windows, Linux

## Compiling

Follow the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/) for your platform.

A good reference for build steps is [`.github/workflows/build-launcher.yml`](.github/workflows/build-launcher.yml).

### Quick Start

```bash
npm install
npm run tauri build
```

### Windows

Follow the openssl instructions at https://docs.rs/crate/openssl/0.9.24 using PowerShell:

```powershell
$env:OPENSSL_DIR='C:\Program Files\OpenSSL-Win64\'
$env:OPENSSL_INCLUDE_DIR='C:\Program Files\OpenSSL-Win64\include'
$env:OPENSSL_LIB_DIR='C:\Program Files\OpenSSL-Win64\lib'
$env:OPENSSL_NO_VENDOR=1
```

## License

This project is licensed under the [Mozilla Public License 2.0](LICENSE).
