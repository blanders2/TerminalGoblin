//! TerminalGoblin: a tabbed, splittable GPU terminal for Wayland.

// Drawing helpers take positions, colors and clips explicitly.
#![allow(clippy::too_many_arguments)]

mod app;
mod config;
mod draw;
mod input;
mod layout;
mod pane;
mod renderer;
mod settings;
mod text;
mod theme;
mod ui;
mod workspace;

use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("terminalgoblin=info,wgpu=error,naga=error"),
    )
    .init();

    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("terminalgoblin {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // TERM / COLORTERM for child shells.
    alacritty_terminal::tty::setup_env();

    let event_loop = EventLoop::with_user_event().build()?;
    let mut opts = app::Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // Debug aids: render one frame offscreen to a PNG, optionally with a sample layout.
            "--screenshot" => opts.screenshot = args.next().map(Into::into),
            "--demo" => opts.demo = true,
            "--demo-menu" => opts.demo_menu = true,
            "--demo-dock" => opts.demo_dock = true,
            "--demo-settings" => opts.demo_settings = args.next(),
            _ => {}
        }
    }
    let mut app = app::App::new(event_loop.create_proxy(), opts);
    event_loop.run_app(&mut app)?;
    Ok(())
}
