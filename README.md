<p align="center"><img src="assets/icon.png" width="128" alt="AgentMesh icon"></p>

# AgentMesh

A control plane for running many Claude Code agents at once, in a native macOS app.

One window per project. Each tab is a real terminal (PTY) running its own `claude`. The sidebar lists open tabs and the project's session history.

Built in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (Zed's UI framework) with `alacritty_terminal` for the terminal emulation.

## Requirements

- macOS 12+ on Apple Silicon or Intel
- [Claude Code](https://claude.com/claude-code) (`claude`) on your `PATH`
- Rust via [rustup](https://rustup.rs) (the pinned toolchain installs itself)
- `cmake` (`brew install cmake`)
- Xcode Command Line Tools (`xcode-select --install`). Full Xcode is not needed: shaders are compiled at launch (`runtime_shaders`).

## Build from source

```sh
brew install rustup cmake
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"   # use rustup's proxies so the pin is honored

git clone https://github.com/thekarananand/AgentMesh.git
cd AgentMesh
cargo run --release
```

`rust-toolchain.toml` pins the Rust version; rustup installs it on first use. The first build downloads and compiles GPUI and takes a few minutes.

### Build and install the `.app`

```sh
scripts/bundle.sh            # builds target/release/bundle/AgentMesh.app (icon + ad-hoc signed)
scripts/bundle.sh --install  # same, then copies it to /Applications
```

The app is ad-hoc signed, not notarized. If macOS blocks a copy you downloaded, run `xattr -dr com.apple.quarantine /Applications/AgentMesh.app`.

## Files

- Logs: `~/Library/Logs/ClaudeControlPlane.log`
- Preferences: `~/Library/Application Support/ClaudeControlPlane/prefs.json`

## Troubleshooting

- **`rustc` ignores the toolchain pin:** PATH must point at rustup's proxy bins (`/opt/homebrew/opt/rustup/bin`), not a concrete toolchain dir.
- **wasmtime build fails:** install `cmake`.
- **Window opens but no text:** `gpui_platform` needs the `font-kit` feature (already set in `Cargo.toml`).
