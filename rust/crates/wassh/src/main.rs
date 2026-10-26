//! WaSSH — native entry point.
//!
//! Launches the application shell by default. `--spike` opens the Phase 0 terminal
//! harness and `--widgets` its widget gallery.

#![forbid(unsafe_code)]

mod app;
mod gallery;
mod spike;

use std::process::ExitCode;

use wassh_core::{APP_NAME, APP_VERSION, Palette, ThemeMode};
use wassh_plugins::toolbar_plugins;

fn main() -> ExitCode {
    app::mark_process_start();
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => {
            if let Err(error) = app::run() {
                eprintln!("failed to open the window: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some("--spike") => {
            if let Err(error) = spike::run_app() {
                eprintln!("failed to open the window: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("{APP_NAME} {APP_VERSION}");
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--diagnostics") => {
            print_environment();
            ExitCode::SUCCESS
        }
        Some("--perf") => {
            // `--perf [--tabs N]` measures an isolated window; see `run_perf`.
            let mut tabs = 0usize;
            let mut rest = args;
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--tabs" => match rest.next().and_then(|value| value.parse().ok()) {
                        Some(count) => tabs = count,
                        None => {
                            eprintln!("--tabs needs a number");
                            return ExitCode::FAILURE;
                        }
                    },
                    other => {
                        eprintln!("unknown argument: {other}");
                        print_help();
                        return ExitCode::FAILURE;
                    }
                }
            }
            if let Err(error) = app::run_perf(tabs) {
                eprintln!("failed to open the window: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other}");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("{APP_NAME} {APP_VERSION}");
    println!();
    println!("Usage: wassh [--version] [--diagnostics]");
    println!("       wassh --perf [--tabs N]     (cold start and memory figures)");
    println!("       wassh --spike [--widgets]   (Phase 0 terminal harness)");
    println!();
    println!("Environment:");
    println!("  WASSH_WAYLAND=1   use native Wayland even when XWayland is available");
    println!("                    (Wayland cannot restore the window position)");
}

fn print_environment() {
    println!("{APP_NAME} {APP_VERSION} (Rust)");
    match app::resident_bytes() {
        Some(bytes) => println!("memory: {:.1} MB resident", bytes as f64 / (1024.0 * 1024.0)),
        None => println!("memory: not reported on this platform"),
    }

    match wassh_store::Config::open_data_dir() {
        Ok(config) => {
            println!("data directory: {}", config.dir.display());
            let hosts = &config.hosts.get().hosts;
            println!(
                "hosts: {} in {} group(s)",
                hosts.len(),
                config.hosts.get().groups.len()
            );
            for host in hosts {
                println!(
                    "  {:<20} {:<28} {}",
                    host.display_name(),
                    host.connection.subtitle(),
                    format!("{:?}", host.connection.auth).to_lowercase()
                );
            }
            for problem in &config.problems {
                println!("configuration problem: {problem}");
            }
        }
        Err(error) => println!("data directory: unavailable ({error})"),
    }

    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let palette = Palette::for_mode(mode);
        println!(
            "theme {:<5} terminal contrast {:>5.2}:1  text {:>5.2}:1  muted {:>5.2}:1",
            mode.as_str(),
            palette.terminal_contrast(),
            palette.text_contrast(),
            palette.muted_contrast()
        );
    }

    let plugins: Vec<_> = toolbar_plugins().map(|m| m.id.as_str()).collect();
    println!("plugins: {}", plugins.join(", "));

    match wassh_ssh::x11::local_config() {
        Some(config) => println!(
            "x11: display :{}.{} ({} byte cookie)",
            config.display_number,
            config.screen,
            config.cookie_hex.len() / 2
        ),
        None => println!("x11: no local display"),
    }
}
