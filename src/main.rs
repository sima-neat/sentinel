mod cache;
mod daemon;
mod model;
mod ring;
mod system;
mod thermal;
mod tui;
mod ui;

use std::env;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::cache::default_cache_path;

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut cache = default_cache_path().to_string();
    let mut interval = 0.5f64;
    let mut once = false;
    let mut history = 240usize;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cache" => {
                cache = take_value(&mut args, i, "--cache")?;
                continue;
            }
            "--interval" => {
                interval = take_value(&mut args, i, "--interval")?.parse()?;
                continue;
            }
            "--history" => {
                history = take_value(&mut args, i, "--history")?.parse()?;
                continue;
            }
            "--once" => {
                args.remove(i);
                once = true;
                continue;
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ => i += 1,
        }
    }

    let command = args.first().map(String::as_str).unwrap_or("ops");
    match command {
        "daemon" => daemon::run(&cache, Duration::from_secs_f64(interval.max(0.2)), history),
        "table" => ui::run_table(&cache, Duration::from_secs_f64(interval.max(0.2)), once),
        "ops" if once => ui::run_ops(&cache, Duration::from_secs_f64(interval.max(0.2)), true),
        "ops" => tui::run_ops(&cache, Duration::from_secs_f64(interval.max(0.5))),
        "sensors" | "metrics" => ui::print_sensors(&cache),
        "status" => ui::print_status(&cache),
        other => bail!("unknown command '{other}'"),
    }
}

fn take_value(args: &mut Vec<String>, index: usize, flag: &str) -> Result<String> {
    if index + 1 >= args.len() {
        bail!("{flag} requires a value");
    }
    let value = args.remove(index + 1);
    args.remove(index);
    Ok(value)
}

fn print_help() {
    println!(
        "simaai-sentinel [--cache PATH] [--interval SEC] [--once] [command]\n\n\
Commands:\n  ops        Continuous terminal operations view (default)\n  table      Continuous color-coded table view\n  sensors    Explain collected metrics and thresholds\n  status     Show daemon/cache status\n  daemon     Run the background collector daemon\n\n\
Daemon options:\n  --history N    Number of samples to keep in cache\n"
    );
}
