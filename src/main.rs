//! TerminalGoblin: a tabbed, splittable GPU terminal for Wayland (and Windows).

// Drawing helpers take positions, colors and clips explicitly.
#![allow(clippy::too_many_arguments)]
// A GUI program on Windows, so starting it doesn't open a console window. Debug builds keep the
// console for their log.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod config;
mod desktop;
mod draw;
mod input;
mod ipc;
mod layout;
mod pane;
mod remote;
mod renderer;
mod settings;
mod text;
mod theme;
mod ui;
mod workspace;

use winit::event_loop::EventLoop;

const HELP: &str = "\
Usage: terminalgoblin [OPTIONS] [-e COMMAND ARGS...]

Open a window. With any of the options below, the tab opens in the TerminalGoblin
that's already running (if there is one) instead of a new window.

  --cwd DIR             Start in DIR
  --title TITLE         Tab title (kept even if the shell sets its own)
  --color #RRGGBB       Tab color
  --env KEY=VALUE       Extra environment variable (repeatable)
  --group NAME          Put the tab in this tab group
  --profile NAME        Shell profile to use
  --location NAME       Open the saved location NAME
  -e, -- COMMAND...     Run COMMAND instead of the shell
  --new-instance        Always open a new window
  @ COMMAND ...         Control a running TerminalGoblin (see `terminalgoblin @ help`)
  -V, --version
";

fn main() -> anyhow::Result<()> {
    // Windows GUI programs aren't attached to the console they're started from: attach, so
    // `terminalgoblin @ ...`, --help and errors print there. Does nothing without one.
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        AttachConsole(ATTACH_PARENT_PROCESS);
    }

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("terminalgoblin=info,wgpu=error,naga=error"),
    )
    .init();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("@") {
        std::process::exit(ipc::client_main(&argv[1..]));
    }

    let (mut launch, rest) = ipc::parse_launch_flags(&argv).map_err(anyhow::Error::msg)?;
    let mut opts = app::Options::default();
    let mut new_instance = false;
    let mut args = rest.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("terminalgoblin {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(());
            }
            "--new-instance" => new_instance = true,
            "--launch-json" => {
                let json = args.next().unwrap_or_default();
                launch = serde_json::from_str(&json)?;
            }
            // Debug aids: render one frame offscreen to a PNG, optionally with a sample layout.
            "--screenshot" => opts.screenshot = args.next().map(Into::into),
            "--demo" => opts.demo = true,
            "--demo-menu" => opts.demo_menu = true,
            "--demo-dock" => opts.demo_dock = true,
            "--demo-settings" => opts.demo_settings = args.next(),
            other => log::warn!("ignoring unknown option {other}"),
        }
    }

    if !launch.is_empty() {
        // Like Windows Terminal: a launch with options becomes a tab in the running window.
        if !new_instance
            && let Some(reply) = ipc::forward_launch(&launch)
            && reply.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
        {
            return Ok(());
        }
        opts.launch = Some(launch);
    }

    // TERM / COLORTERM for child shells.
    alacritty_terminal::tty::setup_env();

    // Let go of that console now, so closing it doesn't close the window too.
    #[cfg(all(windows, not(debug_assertions)))]
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }

    let event_loop = EventLoop::with_user_event().build()?;
    let mut app = app::App::new(event_loop.create_proxy(), opts);
    event_loop.run_app(&mut app)?;
    Ok(())
}
