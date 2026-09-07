mod api;
mod cache;
mod daemon;
mod model;
mod power;
mod runs;
mod system;
mod thermal;
mod tui;
mod ui;
mod version;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

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
    let mut interval = None::<f64>;
    let mut once = false;
    let mut history = 240usize;
    let mut runs_dir = PathBuf::from(runs::DEFAULT_RUNS_DIR);
    let mut api_socket = PathBuf::from(api::DEFAULT_API_SOCKET);

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cache" => {
                cache = take_value(&mut args, i, "--cache")?;
                continue;
            }
            "--interval" => {
                interval = Some(take_value(&mut args, i, "--interval")?.parse()?);
                continue;
            }
            "--history" => {
                history = take_value(&mut args, i, "--history")?.parse()?;
                continue;
            }
            "--runs-dir" => {
                runs_dir = take_value(&mut args, i, "--runs-dir")?.into();
                continue;
            }
            "--api-socket" => {
                api_socket = take_value(&mut args, i, "--api-socket")?.into();
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
            "-V" | "--version" => {
                println!("simaai-sentinel {}", version::VERSION);
                return Ok(());
            }
            _ => i += 1,
        }
    }

    let command = args.first().map(String::as_str).unwrap_or("ops");
    match command {
        "daemon" => daemon::run(
            &cache,
            Duration::from_secs_f64(interval.unwrap_or(2.0).max(0.5)),
            history,
            &runs_dir,
            &api_socket,
        ),
        "table" => ui::run_table(
            &cache,
            Duration::from_secs_f64(interval.unwrap_or(0.5).max(0.2)),
            once,
        ),
        "ops" if once => ui::run_ops(
            &cache,
            Duration::from_secs_f64(interval.unwrap_or(0.5).max(0.2)),
            true,
        ),
        "ops" => tui::run_ops(
            &cache,
            Duration::from_secs_f64(interval.unwrap_or(0.5).max(0.5)),
            &runs_dir,
        ),
        "checkpoint" => checkpoint_command(&cache, &runs_dir, &args[1..]),
        "runs" => runs_command(&runs_dir, &args[1..]),
        "export" if args.len() == 1 => ui::export_json(&cache),
        "export" => export_runs_command(&runs_dir, &args[1..]),
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

fn checkpoint_command(cache_path: &str, runs_dir: &Path, args: &[String]) -> Result<()> {
    let mut name = None;
    let mut note = None;
    let mut tags = Vec::new();
    let mut stop = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--name" => name = Some(argument_value(args, &mut index, "--name")?),
            "--note" => note = Some(argument_value(args, &mut index, "--note")?),
            "--tag" => tags.push(argument_value(args, &mut index, "--tag")?),
            "--stop" => {
                stop = true;
                index += 1;
            }
            flag => bail!("unknown checkpoint option '{flag}'"),
        }
    }
    if stop {
        if name.is_some() || note.is_some() || !tags.is_empty() {
            bail!("--stop cannot be combined with --name, --note, or --tag");
        }
        let run = runs::stop(runs_dir)?;
        let summary = runs::summary(&run);
        println!(
            "Stopped '{}' ({}, {} samples, {:.3} s).",
            summary.name,
            summary.id,
            summary.samples,
            summary.duration_ms as f64 / 1000.0
        );
        return Ok(());
    }
    let name = name.context("checkpoint requires --name NAME or --stop")?;
    let cache =
        cache::read_cache(cache_path).context("read the live cache before starting checkpoint")?;
    let run = runs::start(runs_dir, &cache, &name, note, tags)?;
    println!(
        "Recording '{}' as {}. Stop with: simaai-sentinel checkpoint --stop",
        run.metadata.name, run.metadata.id
    );
    Ok(())
}

fn runs_command(runs_dir: &Path, args: &[String]) -> Result<()> {
    let command = args.first().map(String::as_str).unwrap_or("list");
    match command {
        "list" if args.len() <= 1 => {
            let active_id = runs::active_metadata(runs_dir)?.map(|run| run.metadata.id);
            println!(
                "{:<28}  {:<20}  {:>8}  {:>8}  State",
                "ID", "Name", "Duration", "Samples"
            );
            for run in runs::list(runs_dir)? {
                let state = if active_id.as_deref() == Some(run.id.as_str()) {
                    "recording"
                } else {
                    "complete"
                };
                println!(
                    "{:<28}  {:<20}  {:>7.1}s  {:>8}  {}",
                    run.id,
                    truncate(&run.name, 20),
                    run.duration_ms as f64 / 1000.0,
                    run.samples,
                    state
                );
            }
            Ok(())
        }
        "show" if args.len() == 2 => {
            let run = runs::load(runs_dir, &args[1])?;
            let summary = runs::summary(&run);
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "metadata": run.metadata,
                    "duration_ms": summary.duration_ms,
                    "samples": summary.samples,
                    "energy_joules": summary.energy_joules,
                    "metric_count": run.metrics.len(),
                }))?
            );
            Ok(())
        }
        "delete" if args.len() == 2 => {
            let run = runs::delete(runs_dir, &args[1])?;
            println!("Deleted '{}' ({}).", run.metadata.name, run.metadata.id);
            Ok(())
        }
        "clear" if args == ["clear", "--force"] => {
            let count = runs::clear_completed(runs_dir)?;
            println!(
                "Cleared {count} completed run(s). Active recording, if any, was preserved."
            );
            Ok(())
        }
        "clear" => bail!(
            "clearing all completed runs is destructive; confirm with: simaai-sentinel runs clear --force"
        ),
        "list" | "show" | "delete" => bail!("usage: simaai-sentinel runs {command} [RUN]"),
        other => bail!("unknown runs command '{other}'"),
    }
}

fn export_runs_command(runs_dir: &Path, args: &[String]) -> Result<()> {
    let mut selectors = Vec::new();
    let mut format = None;
    let mut output = None::<PathBuf>;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => format = Some(argument_value(args, &mut index, "--format")?),
            "--output" => output = Some(argument_value(args, &mut index, "--output")?.into()),
            value if value.starts_with('-') => bail!("unknown export option '{value}'"),
            value => {
                selectors.push(value.to_string());
                index += 1;
            }
        }
    }
    if selectors.is_empty() {
        bail!("export requires at least one RUN name or ID");
    }
    let format = format.context("export requires --format csv|json")?;
    let output = output.context("export requires --output PATH")?;
    if output.is_dir() {
        bail!("export output is a directory: {}", output.display());
    }
    let selected_runs = runs::load_many(runs_dir, &selectors)?;
    let data = match format.as_str() {
        "csv" => runs::export_csv(&selected_runs).into_bytes(),
        "json" => serde_json::to_vec_pretty(&runs::export_json(selected_runs.clone()))?,
        other => bail!("unsupported export format '{other}'; use csv or json"),
    };
    write_output_atomic(&output, &data)?;
    if format == "csv" {
        let metadata_path = output.with_extension(format!(
            "{}.metadata.json",
            output
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("csv")
        ));
        write_output_atomic(
            &metadata_path,
            &serde_json::to_vec_pretty(&runs::export_json(selected_runs))?,
        )?;
        println!("Wrote run metadata to {}.", metadata_path.display());
    }
    println!(
        "Exported {} run(s) to {}.",
        selectors.len(),
        output.display()
    );
    Ok(())
}

fn write_output_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        bail!(
            "export output directory does not exist: {}",
            parent.display()
        );
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
    ));
    fs::write(&temporary, data).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn argument_value(args: &[String], index: &mut usize, flag: &str) -> Result<String> {
    let value = args
        .get(*index + 1)
        .with_context(|| format!("{flag} requires a value"))?
        .clone();
    *index += 2;
    Ok(value)
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.into()
    } else {
        value
            .chars()
            .take(width.saturating_sub(1))
            .collect::<String>()
            + "…"
    }
}

fn print_help() {
    println!(
        "simaai-sentinel [--version] [--cache PATH] [--runs-dir PATH] [--api-socket PATH] [--interval SEC] [--once] [command]\n\n\
Commands:\n  ops        Continuous terminal operations view (default)\n  table      Continuous color-coded table view\n  checkpoint Start or stop a persistent named run capture\n  runs       List, show, or delete captured runs\n  export     Export runs as CSV/JSON; without arguments print live cache JSON\n  sensors    Explain collected metrics and thresholds\n  status     Show daemon/cache status\n  daemon     Run the background collector daemon\n\n\
Daemon options:\n  --history N    Number of samples to keep in cache\n\n\
Examples:\n  simaai-sentinel checkpoint --name baseline --note \"before optimization\"\n  simaai-sentinel checkpoint --stop\n  simaai-sentinel runs list\n  simaai-sentinel export baseline optimized --format csv --output comparison.csv\n"
    );
}
