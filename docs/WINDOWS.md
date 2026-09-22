# Windows

Snail supports current Windows 11 on x86-64 with the MSVC toolchain and a physical DirectX
adapter. The v1 deliverable is a portable ZIP: no installer, administrator rights, service,
auto-start entry, file association, or auto-updater. ARM64 and RDP/VDI are not release gates.

## Reference machine

Baseline started 2026-09-21 at repository revision
`255b513be184e94455646c0355533273cbfcd1de`.

| Item | Recorded value |
| --- | --- |
| Windows | Windows 11 Pro, version 10.0.26200, build 26200 |
| CPU | AMD Ryzen 9 7900X, 12 cores / 24 logical processors |
| RAM | 31.1 GiB |
| Physical GPU | AMD Radeon RX 9070 XT, driver 32.0.31041.1004 |
| Integrated GPU | AMD Radeon Graphics, driver 32.0.21045.5002 |
| Other display adapter | SudoMaker Virtual Display Adapter 1.10.9.289 |
| Displays reported by Windows | 2; physical topology and per-monitor scales still to verify |
| Current primary mode | 2560 x 1440 |
| Visual Studio | Community 2022 17.14.41, MSVC x64/x86 tools detected |
| Rust target | `x86_64-pc-windows-msvc` |
| Project Rust | 1.97.1 (`8bab26f4f`, 2026-07-14) |

### Before-state

- The checkout initially used the machine's default Rust 1.98.1. Reusing its partial `target`
  output with 1.97.1 made proc-macro DLLs fail metadata loading. Removing generated `target`
  output and rebuilding wholly with the pinned toolchain fixed that environmental failure.
- The first clean 1.97.1 workspace build reached `snail` and failed because the non-macOS app-menu
  rows registered a click handler without first assigning a GPUI element ID. This is the first
  source-level Windows port defect fixed by E19.
- The Visual Studio installer does not currently report a Spectre library component. Both debug
  and release MSVC builds link successfully without it, so it is not a Snail prerequisite.
- `cargo build --workspace --locked` passes with Rust 1.97.1. The Windows workspace test run passes
  329 tests; the one live iCloud probe remains intentionally ignored.
- The debug executable opened a visible, responsive top-level window with an empty disposable
  profile. Its working set settled near 81–92 MiB during the initial screen. GPUI logs
  `0x887A002D` when the optional DXGI debug interface is absent, disables DirectX debugging, and
  continues rendering; Windows Graphics Tools is therefore not a runtime prerequisite.
- The x64 release build and portable packaging pass. The release executable embeds Snail's icon and
  version metadata, uses the Windows GUI subsystem, and imports only Windows system/API-set DLLs.
  The console wrapper successfully ran the store benchmark against an empty disposable profile.
  The package checksum verifies.
- One release build attempt was blocked by Windows Application Control with error 4551 while
  loading a generated build-script executable. An immediate retry succeeded without a source or
  security-policy change; record it again if it recurs.
- Release GUI validation, the 200k-message fixture, screenshots, multi-DPI checks, and populated
  benchmark measurements remain to be recorded.

## Build from source

Install Visual Studio 2022 with **Desktop development with C++**, including the current MSVC x64/x86
build tools and a Windows 11 SDK. Then, from PowerShell:

```powershell
rustup toolchain install 1.97.1-x86_64-pc-windows-msvc --profile minimal
cargo build --workspace --locked
cargo test --workspace --locked
```

The repository's `rust-toolchain.toml` selects 1.97.1 automatically. Snail bundles SQLite and does
not require a Vulkan SDK or a third-party credential library on Windows.

For a personal Gmail build, put the Google Cloud **Desktop app** client JSON at the gitignored
`.secrets\google-oauth.json` path before building. The build script reads `installed.client_id` and
`installed.client_secret`; `SNAIL_GOOGLE_CLIENT_ID` and `SNAIL_GOOGLE_CLIENT_SECRET` are the CI
alternative. Snail binds a random loopback port and uses
`http://127.0.0.1:<port>/oauth2callback`, which Google Desktop clients accept without a fixed
redirect URI. Never commit either source.

## Data and logs

By default, durable data is under `%APPDATA%\dev.snail.app`; cache and logs are under
`%LOCALAPPDATA%\dev.snail.app`. `SNAIL_CONFIG_DIR`, `SNAIL_CACHE_DIR`, and `SNAIL_LOG_DIR` override
those roots for disposable test profiles. Snail must never write beside the executable.

## Known support boundaries

- A physical DirectX adapter supported by GPUI is required. Software rendering is not enabled.
- RDP/VDI and Windows on ARM are not v1 promises.
- Portable-build notification clicks are supported while Snail is running or minimized. Activation
  after the process has exited requires installed COM registration and is not promised.

## Acceptance record

The current portable artifact is `dist\snail-0.1.0-windows-x86_64.zip`, accompanied by a SHA-256
file. It contains only `snail.exe`, `snail-cli.exe`, and `README-WINDOWS.md`.

Status: **in progress**. E19.13 is not complete and this document is not yet a release sign-off.
