# TerminalGoblin

A tabbed, splittable, GPU-rendered terminal for Linux/Wayland (with an experimental Windows build), written in Rust.

## Commands
- Build: `cargo build` (release: `cargo build --release`)
- Run: `cargo run --release`
- Install for this user: `./install.sh` (release build to `~/.local/bin`, plus the desktop file and icon)
- Test (all): `cargo test`
- Test (one module): `cargo test <module>::`, e.g. `cargo test config::`
- Lint/format: `cargo fmt` and `cargo clippy --all-targets`
- Windows type-check from Linux: `cargo check --target x86_64-pc-windows-msvc`, and again with `--release`, which also compiles the release-only console code in `main.rs`
- See the UI without a display session: `cargo run -- --screenshot <out.png> --demo` renders one frame offscreen to a PNG. `--demo-menu`, `--demo-dock` and `--demo-settings PAGE` show other states.

## Layout
| Path | Purpose | Committed |
|---|---|---|
| `src/main.rs` | Command-line flags, the `@` client entry point, event loop startup | Yes |
| `src/app.rs` | Application state and window/keyboard/mouse event handling | Yes |
| `src/workspace.rs`, `src/layout.rs` | Tabs, tab groups, panes, and the split tree inside a tab | Yes |
| `src/pane.rs` | One terminal: `alacritty_terminal` `Term` plus its PTY | Yes |
| `src/input.rs` | Keys and mouse to terminal byte sequences (incl. Kitty keyboard protocol) | Yes |
| `src/renderer.rs`, `src/shader.wgsl` | wgpu setup and the instanced quad renderer | Yes |
| `src/draw.rs`, `src/ui.rs` | Building each frame; tab bar/menu geometry and hit testing | Yes |
| `src/text.rs` | Fonts, shaping, glyph atlas; box-drawing characters drawn geometrically | Yes |
| `src/settings.rs` | The settings window | Yes |
| `src/config.rs`, `src/theme.rs` | Config file, shell profiles, keybindings; color schemes | Yes |
| `src/ipc.rs`, `src/remote.rs` | Control socket and `terminalgoblin @ ...` client; handling its requests | Yes |
| `src/clipboard.rs` | Clipboard and primary selection, one implementation per OS | Yes |
| `src/desktop.rs` | Keeps the installed `.desktop` file's actions in step with saved locations | Yes |
| `assets/` | Desktop file and icon used by `install.sh` | Yes |
| `docs/` | README screenshot | Yes |
| `.scratch/` | Throwaway scripts, repros, probes | **No** (gitignored) |
| `.local/` | Generated output, caches, logs | **No** (gitignored) |
| `target/` | Cargo build output | **No** (gitignored) |

It's a single binary crate with no modules or plugins. New code goes in `src/`; don't create files in the repo root unless asked.

## Where tests go
| Test covers | Location |
|---|---|
| Code in one file | A `#[cfg(test)] mod tests` at the bottom of that file |
| Throwaway check / repro | `.scratch/` (not a real test) |

- There's no `tests/` folder: it's a binary crate, so tests in `tests/` couldn't import its code.
- Promote a `.scratch/` script to a real test only if it catches a real bug or I ask for it.

## Rules for files you create
- Throwaway scripts, debug prints, one-off repros → `.scratch/` only.
- Generated files → `.local/`. Don't read `.local/` or `target/` unless the task requires it.

## Secrets
- The project has no secrets. Never read, print or commit `.env*` files.
- If you see something that looks like a credential in tracked code, stop and tell me.

## Before you finish
1. Delete everything you created in `.scratch/`, or tell me why it should stay.
2. Run `git status` and list any new untracked files outside `src/`.
3. Run `cargo test` and `cargo clippy --all-targets` and report the result. If you touched `#[cfg(windows)]` code or dependencies, also run the Windows type-check above.

## Gotchas
- **Platforms:** Linux is the default. Windows-only code goes behind `#[cfg(windows)]` and its Linux counterpart behind `#[cfg(not(windows))]`, not `cfg(unix)`. Platform-specific dependencies go in the `[target.'cfg(...)'.dependencies]` tables in `Cargo.toml`.
- **Windows is never run here:** it's only type-checked, which needs `rustup target add x86_64-pc-windows-msvc`. Say so when reporting Windows changes.
- **Wayland only:** on Linux, winit is built without X11, and the clipboard needs a Wayland display (`Clipboard::new` returns `None` without one).
- **Config file:** the settings window saves through `config::save_to`, which edits the user's file with `toml_edit` so their comments survive. Never rewrite it with `toml::to_string`. A new option needs a `Config` field, its `Default`, its line in `DEFAULT_CONFIG`, and a line in `save_to`.
- **The control socket is a public interface:** tools like PatchGoblin use `terminalgoblin @ ...` and its JSON to open named, colored tabs and find them again. Keep request and reply fields backward compatible.
- **Default font:** Noto Sans Mono matches cosmic-term, so Claude Code renders the same as on COSMIC. Don't change the Linux defaults without asking.
- **Debug builds:** dependencies are built with `opt-level = 2` even in debug, so the first build is slow but the app runs smoothly.
