# Building Snail

Cross-platform is a hard requirement (plan.md preamble): macOS is the daily driver, Linux and
Windows stay green in CI and usable. This file states the **hard prerequisites**, especially the
ones that fail in ways that look like something else.

## All platforms

- Rust **1.97.1** (what the pin builds with). `gpui-kit =0.6.4` / `gpui-pre =0.3.5`, exact pins.
- `cargo build --workspace` pulls `rusqlite`'s **bundled** SQLite (a C compile, with FTS5 enabled).

## macOS

- **Full Xcode, not just the Command Line Tools.** GPUI's Metal shader compiler ships inside Xcode;
  Command Line Tools alone fails at build time.
- No other system packages.

## Linux

The Vulkan **loader and a working driver** are required — GPUI ignores `WGPU_BACKEND`, and its GL
path panics in the quads pipeline (zed#50996). A machine without a Vulkan driver cannot run Snail;
this is a property of the framework, not something Snail can work around.

Build dependencies (Debian/Ubuntu names; `apt-get install`):

```
libvulkan1                # the loader, NOT the Vulkan SDK
libxkbcommon-dev libxkbcommon-x11-dev
libwayland-dev libx11-dev libx11-xcb-dev libxcb1-dev
libxcb-randr0-dev libxcb-shm0-dev libxcb-xfixes0-dev libxcb-xkb-dev
libxcb-composite0-dev libxcb-damage0-dev libxcb-present-dev
libxrandr-dev libxi-dev libxcursor-dev libxext-dev
libfontconfig1-dev libfreetype6-dev
libasound2-dev libssl-dev libsqlite3-dev libzstd-dev
libdbus-1-dev            # keyring's Secret Service backend (libdbus-sys)
pkg-config cmake clang libclang-dev
```

Runtime:

- A **Vulkan driver** (Mesa `v3d`/`radv`/`anv`, or proprietary). `vulkaninfo` should show a
  non-`llvmpipe` adapter.
- `xdg-desktop-portal` and a backend (`xdg-desktop-portal-gtk` or `-wlr`) for **file dialogs**;
  attachments depend on it. Without a portal, Snail shows a real error rather than failing silently.
- A Secret Service daemon (`gnome-keyring`, KWallet) for the keychain. Without one, Snail reports a
  clear error and offers the encrypted-file fallback (E3.8).

## Windows

- MSVC toolchain with the **Spectre-mitigated libraries** (the `x86_64-pc-windows-msvc` target).
- No other system packages; `rusqlite` bundled and `keyring`'s `windows-native` backend build
  without external dependencies.
