# Launcher

A desktop app that starts the Open Integration Engine administrator for each of your engines.

Originally forked from [Ballista](https://github.com/kayyagari/ballista) by [Kiran Ayyagari](https://github.com/kayyagari). Thanks to Kiran for the original project.

> **Consider the OIE web administrator.** Open Integration Engine now has a browser-based
> administrator, with nothing to install on each workstation:
> https://openintegrationengine.org/web-administrator/
>
> Launcher suits people who manage many engines from one machine or need a native app. For
> others the web administrator is simpler, so try it before you commit to a desktop launcher.

## Requirements

- A JavaFX-enabled JDK. The administrator is a JavaFX application, so a plain JRE will not run it.
  Launcher neither installs nor downloads Java. Set a `Java Home` per connection, or let Launcher use
  `JAVA_HOME` or the `java` on your `PATH`.
- Windows: Windows 10 or later, or Windows Server 2016 or later, plus the Microsoft Edge WebView2
  Runtime. Windows 11 includes WebView2, and most Windows 10 machines have it by way of Microsoft Edge,
  but Windows Server images frequently do not.
- macOS: nothing else is needed. The system WebView is part of the OS.
- Linux: the `.deb` and `.rpm` need `libwebkit2gtk-4.1-0` and `libgtk-3-0` from your distribution.
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

Use the Standalone Installer. The Bootstrapper on the same page downloads the runtime when it runs,
so it fails the same way.

Nothing else in the installation needs internet access: WebView2 is the only thing the installer
downloads, and Launcher has no auto-updater.

### The MSIX package

From 2.4.1 on, each release also has an MSIX package per architecture, for example
`Launcher_2.4.1_x64.msix`. It installs for the current user without administrator rights. Double-click
it to open App Installer, or in PowerShell:

```
Add-AppxPackage .\Launcher_2.4.1_x64.msix
```

The package can't install WebView2, so install it first where it is missing (see above). Windows
Server 2022 has neither WebView2 nor App Installer out of the box: there the package installs with
`Add-AppxPackage`, but Launcher doesn't open until WebView2 is installed. The package declares Windows
10 version 1809 (build 17763) as its minimum, which is also Windows Server 2019. On anything older, use
the `.exe` or the `.msi`.

To install it for every user, run this in an administrator PowerShell. The account that runs it gets
Launcher at once, and each new user gets it at their first sign-in.

```
Add-AppxProvisionedPackage -Online -PackagePath .\Launcher_2.4.1_x64.msix -SkipLicense
```

Where sideloading is turned off by policy, Windows refuses the package with "Sideloading is blocked by
policy. Contact your administrator to allow you to install apps from anywhere." Under AppLocker, the
package is governed by packaged app rules, for installing and for starting it: allow the publisher
Diridium Technologies, Inc. Executable rules don't apply to the `Launcher.exe` inside the package.

Launcher from the package uses the same `.launcher` folder in your home folder as the other installers,
for connections, the downloaded administrator files and the logs. Its own settings, such as the theme,
go into the package's private storage, which Windows deletes when the package is removed. On a machine
where the `.exe` or `.msi` has already run, the package uses the settings that earlier install created
instead.

If you install the package beside the `.exe` or `.msi`, you get a second copy with its own Start menu
entry, and the two copies share those settings. Uninstall the one you don't want. A newer package installs over an
older one and keeps the settings.

### Checking a download

Every release file is built by GitHub Actions from the release's tag in this repository, and its
SHA-256 is listed on the release page.

From 2.4.1 on, the Windows files (`.exe`, `.msi` and `.msix`), the `Launcher.exe` they install and the
`.exe` installer's `uninstall.exe` are signed by Diridium Technologies, Inc. To check one, open the
file's Properties and its Digital Signatures tab, or in PowerShell:

```
Get-AuthenticodeSignature .\Launcher_2.4.1_x64-setup.exe
```

The status should be `Valid` and the signer `Diridium Technologies, Inc.`. The helper DLLs that the
`.exe` installer and its `uninstall.exe` unpack into a temporary folder while they run are not signed.
AppLocker or Windows Defender Application Control (WDAC) rules can allow Launcher by publisher. Where DLL rules are on, allow the helper DLLs
by hash or path. The macOS and Linux files are not signed; check them against the SHA-256 on the
release page.

## How to use

1. Download the installer for your operating system from the releases page.
2. Create a new connection, or import connections from another Launcher's export or from the
   `data/connections.json` file in the Mirth Connect Administrator Launcher's install folder.
3. Launch a connection by double-clicking the desired server, or select it and click the play button.
4. Edit a connection by clicking the pencil icon on a server row.
5. Adjust the `Java Home` field's value if necessary. It must point at a JavaFX-enabled JDK (see Requirements).

## Features

- Dark theme UI with keyboard zoom support (Cmd/Ctrl +/-/0)
- Real-time server connectivity status
- Sort by group, name, last connected, or status
- Java console output viewer
- Per-connection TLS certificate pinning (trust on first use), for the self-signed certificates these servers usually carry
- Cross-platform: macOS, Windows, Linux

## Compiling

Follow the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/) for your platform.

CI's build steps are in [`.github/workflows/build-launcher.yml`](.github/workflows/build-launcher.yml).

### Quick start

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

## Design notes

[docs/design-notes.md](docs/design-notes.md) explains why parts of Launcher work the way they do.

## License

This project is licensed under the [Mozilla Public License 2.0](LICENSE).
