use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Clear, Dataset, Gauge, GraphType, Paragraph, Row, Table,
    Tabs, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::cache::read_cache;
use crate::model::{CachePayload, MetricDefinition, ProcessInfo};
use crate::runs::{self, SavedRun};

const BG: Color = Color::Reset;
const FG: Color = Color::White;
const DIM: Color = Color::Gray;
const FAINT: Color = Color::DarkGray;
const CYAN: Color = Color::Cyan;
const GREEN: Color = Color::Green;
const YELLOW: Color = Color::Yellow;
const RED: Color = Color::Red;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Thermal,
    Power,
    System,
    Storage,
    Compare,
}

const TABS: [Tab; 6] = [
    Tab::Overview,
    Tab::Thermal,
    Tab::Power,
    Tab::System,
    Tab::Storage,
    Tab::Compare,
];

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Thermal => "Thermal",
            Tab::Power => "Power",
            Tab::System => "System",
            Tab::Storage => "Storage/Net",
            Tab::Compare => "Compare Runs",
        }
    }
}

struct CompareState {
    runs: Vec<SavedRun>,
    selected: usize,
    visible: BTreeSet<String>,
    baseline: Option<String>,
    metric: usize,
    overlap: bool,
    message: Option<String>,
    pending_delete: Option<String>,
}

impl CompareState {
    fn load(directory: &Path) -> Self {
        let summaries = runs::list(directory).unwrap_or_default();
        let selectors: Vec<String> = summaries
            .into_iter()
            .filter(|summary| summary.ended_at.is_some())
            .map(|summary| summary.id)
            .collect();
        let runs = runs::load_many(directory, &selectors).unwrap_or_default();
        let visible = runs
            .iter()
            .take(2)
            .map(|run| run.metadata.id.clone())
            .collect();
        let baseline = runs.first().map(|run| run.metadata.id.clone());
        Self {
            runs,
            selected: 0,
            visible,
            baseline,
            metric: 0,
            overlap: true,
            message: None,
            pending_delete: None,
        }
    }

    fn refresh(&mut self, directory: &Path) {
        let next = Self::load(directory);
        self.runs = next.runs;
        self.selected = self.selected.min(self.runs.len().saturating_sub(1));
        self.visible
            .retain(|id| self.runs.iter().any(|run| &run.metadata.id == id));
        if self.visible.is_empty() {
            self.visible
                .extend(self.runs.iter().take(2).map(|run| run.metadata.id.clone()));
        }
        if self
            .baseline
            .as_ref()
            .is_none_or(|id| !self.runs.iter().any(|run| &run.metadata.id == id))
        {
            self.baseline = self.runs.first().map(|run| run.metadata.id.clone());
        }
        if self
            .pending_delete
            .as_ref()
            .is_some_and(|id| !self.runs.iter().any(|run| &run.metadata.id == id))
        {
            self.pending_delete = None;
        }
    }
}

enum InputMode {
    Normal,
    CheckpointName(String),
}

pub fn run_ops(cache_path: &str, interval: Duration, runs_dir: &Path) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut active = 0usize;
    let mut cache = read_cache(cache_path)?;
    let mut last_read = Instant::now() - interval;
    let cache_tick = interval.max(Duration::from_millis(500));
    let event_tick = Duration::from_millis(50);
    let mut dirty = true;
    let mut clear_body = true;
    let mut selected_core = 0usize;
    let mut thermal_group = 0usize;
    let mut compare = CompareState::load(runs_dir);
    let mut input_mode = InputMode::Normal;
    let mut recording = runs::active(runs_dir).ok().flatten();

    let result = loop {
        if last_read.elapsed() >= cache_tick {
            if let Ok(next) = read_cache(cache_path) {
                cache = next;
                thermal_group = thermal_group.min(thermal_groups(&cache).len().saturating_sub(1));
                compare.refresh(runs_dir);
                recording = runs::active(runs_dir).ok().flatten();
                dirty = true;
            }
            last_read = Instant::now();
        }

        if dirty {
            terminal.draw(|f| {
                draw(
                    f,
                    &cache,
                    TABS[active],
                    clear_body,
                    selected_core,
                    thermal_group,
                    &compare,
                    recording.as_ref(),
                    &input_mode,
                )
            })?;
            dirty = false;
            clear_body = false;
        }

        let timeout = event_tick.min(cache_tick.saturating_sub(last_read.elapsed()));
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if let InputMode::CheckpointName(name) = &mut input_mode {
                        match key.code {
                            KeyCode::Esc => input_mode = InputMode::Normal,
                            KeyCode::Backspace => {
                                name.pop();
                            }
                            KeyCode::Enter if !name.trim().is_empty() => {
                                match runs::start(runs_dir, &cache, name.trim(), None, Vec::new()) {
                                    Ok(run) => {
                                        recording = Some(run);
                                        input_mode = InputMode::Normal;
                                    }
                                    Err(error) => compare.message = Some(error.to_string()),
                                }
                            }
                            KeyCode::Char(character)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && name.len() < 80 =>
                            {
                                name.push(character);
                            }
                            _ => {}
                        }
                        dirty = true;
                        continue;
                    }
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            break Ok::<(), anyhow::Error>(());
                        }
                        (KeyCode::Tab, _) => {
                            active = (active + 1) % TABS.len();
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::BackTab, _) => {
                            active = (active + TABS.len() - 1) % TABS.len();
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('1'), _) => {
                            active = 0;
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('2'), _) => {
                            active = 1;
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('3'), _) => {
                            active = 2;
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('4'), _) => {
                            active = 3;
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('5'), _) => {
                            active = 4;
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('6'), _) => {
                            active = 5;
                            dirty = true;
                            clear_body = true;
                        }
                        (KeyCode::Char('r'), _) => {
                            if recording.is_some() {
                                match runs::stop(runs_dir) {
                                    Ok(run) => {
                                        compare.message =
                                            Some(format!("Saved run '{}'.", run.metadata.name));
                                        recording = None;
                                        compare.refresh(runs_dir);
                                    }
                                    Err(error) => compare.message = Some(error.to_string()),
                                }
                            } else {
                                input_mode = InputMode::CheckpointName(String::new());
                            }
                            dirty = true;
                        }
                        (KeyCode::Up, _) if TABS[active] == Tab::Compare => {
                            compare.selected = compare.selected.saturating_sub(1);
                            dirty = true;
                        }
                        (KeyCode::Down, _) if TABS[active] == Tab::Compare => {
                            compare.selected =
                                (compare.selected + 1).min(compare.runs.len().saturating_sub(1));
                            dirty = true;
                        }
                        (KeyCode::Char(' '), _) if TABS[active] == Tab::Compare => {
                            if let Some(run) = compare.runs.get(compare.selected) {
                                if !compare.visible.remove(&run.metadata.id) {
                                    if compare.visible.len() < 4 {
                                        compare.visible.insert(run.metadata.id.clone());
                                    } else {
                                        compare.message =
                                            Some("At most four runs can be visible.".into());
                                    }
                                }
                            }
                            dirty = true;
                        }
                        (KeyCode::Enter, _) if TABS[active] == Tab::Compare => {
                            compare.baseline = compare
                                .runs
                                .get(compare.selected)
                                .map(|run| run.metadata.id.clone());
                            dirty = true;
                        }
                        (KeyCode::Left, _) if TABS[active] == Tab::Compare => {
                            compare.metric = (compare.metric + COMPARE_METRICS.len() - 1)
                                % COMPARE_METRICS.len();
                            dirty = true;
                        }
                        (KeyCode::Right, _) if TABS[active] == Tab::Compare => {
                            compare.metric = (compare.metric + 1) % COMPARE_METRICS.len();
                            dirty = true;
                        }
                        (KeyCode::Char('w'), _) if TABS[active] == Tab::Compare => {
                            compare.overlap = !compare.overlap;
                            dirty = true;
                        }
                        (KeyCode::Char('x'), _) if TABS[active] == Tab::Compare => {
                            let selected: Vec<SavedRun> = compare
                                .runs
                                .iter()
                                .filter(|run| compare.visible.contains(&run.metadata.id))
                                .cloned()
                                .collect();
                            let path = PathBuf::from(format!(
                                "/tmp/sentinel-comparison-{}.json",
                                chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
                            ));
                            match std::fs::write(
                                &path,
                                serde_json::to_vec_pretty(&runs::export_json(selected))
                                    .unwrap_or_default(),
                            ) {
                                Ok(()) => {
                                    compare.message = Some(format!("Exported {}", path.display()))
                                }
                                Err(error) => compare.message = Some(error.to_string()),
                            }
                            dirty = true;
                        }
                        (KeyCode::Char('d'), _) if TABS[active] == Tab::Compare => {
                            if let Some(run) = compare.runs.get(compare.selected) {
                                let id = run.metadata.id.clone();
                                let name = run.metadata.name.clone();
                                if compare.pending_delete.as_deref() == Some(id.as_str()) {
                                    match runs::delete(runs_dir, &id) {
                                        Ok(_) => {
                                            compare.message =
                                                Some(format!("Deleted run '{name}'."));
                                            compare.pending_delete = None;
                                            compare.refresh(runs_dir);
                                        }
                                        Err(error) => {
                                            compare.message = Some(error.to_string());
                                            compare.pending_delete = None;
                                        }
                                    }
                                } else {
                                    compare.pending_delete = Some(id);
                                    compare.message =
                                        Some(format!("Press d again to delete '{name}'."));
                                }
                            }
                            dirty = true;
                        }
                        (KeyCode::Up, _) if TABS[active] == Tab::System => {
                            selected_core = selected_core.saturating_sub(1);
                            dirty = true;
                        }
                        (KeyCode::Down, _) if TABS[active] == Tab::System => {
                            let max_core = cpu_core_count(&cache).saturating_sub(1);
                            selected_core = (selected_core + 1).min(max_core);
                            dirty = true;
                        }
                        (KeyCode::Left, _) if TABS[active] == Tab::Thermal => {
                            thermal_group = thermal_group.saturating_sub(1);
                            dirty = true;
                        }
                        (KeyCode::Right, _) if TABS[active] == Tab::Thermal => {
                            let max_group = thermal_groups(&cache).len().saturating_sub(1);
                            thermal_group = (thermal_group + 1).min(max_group);
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {
                    dirty = true;
                    clear_body = true;
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn draw(
    f: &mut Frame,
    cache: &CachePayload,
    active: Tab,
    clear_body: bool,
    selected_core: usize,
    thermal_group: usize,
    compare: &CompareState,
    recording: Option<&SavedRun>,
    input_mode: &InputMode,
) {
    let area = f.area();
    f.buffer_mut()
        .set_style(area, Style::default().bg(BG).fg(FG));

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(f, layout[0], cache, recording);
    draw_tabs(f, layout[1], active);
    if clear_body {
        f.render_widget(Clear, layout[2]);
    }
    match active {
        Tab::Overview => draw_overview(f, layout[2], cache),
        Tab::Thermal => draw_thermal(f, layout[2], cache, thermal_group),
        Tab::Power => draw_power(f, layout[2], cache),
        Tab::System => draw_system(f, layout[2], cache, selected_core),
        Tab::Storage => draw_storage(f, layout[2], cache),
        Tab::Compare => draw_compare(f, layout[2], compare),
    }
    draw_footer(f, layout[3], active);
    if let InputMode::CheckpointName(name) = input_mode {
        draw_checkpoint_prompt(f, area, name);
    }
}

fn draw_header(f: &mut Frame, area: Rect, cache: &CachePayload, recording: Option<&SavedRun>) {
    let sample_age = cache
        .latest
        .as_ref()
        .map(|sample| {
            chrono::Utc::now()
                .signed_duration_since(sample.timestamp)
                .num_milliseconds() as f64
                / 1000.0
        })
        .unwrap_or(f64::NAN);
    let status = if sample_age.is_finite() && sample_age < 15.0 {
        Span::styled(
            "LIVE",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "STALE",
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        )
    };
    let mut spans = vec![
        Span::styled(" ● ", Style::default().fg(GREEN)),
        Span::styled(
            "Sentinel",
            Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" v{}", cache.version), Style::default().fg(DIM)),
        Span::styled("  │  ", Style::default().fg(FAINT)),
        Span::styled("cache ", Style::default().fg(DIM)),
        status,
        Span::styled(format!("  age {:.1}s", sample_age), Style::default().fg(FG)),
        Span::styled("  samples ", Style::default().fg(DIM)),
        Span::styled(cache.samples.len().to_string(), Style::default().fg(FG)),
    ];
    if let Some(run) = recording {
        spans.push(Span::styled("  │  ", Style::default().fg(FAINT)));
        spans.push(Span::styled(
            format!("● RECORDING {}", run.metadata.name),
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(BG)),
        area,
    );
}

fn draw_tabs(f: &mut Frame, area: Rect, active: Tab) {
    let titles: Vec<Line> = TABS
        .iter()
        .enumerate()
        .map(|(idx, tab)| Line::from(format!(" {} {} ", idx + 1, tab.title())))
        .collect();
    let active_index = TABS.iter().position(|tab| *tab == active).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(active_index)
        .block(panel(""))
        .style(Style::default().fg(FG).bg(BG))
        .highlight_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD));
    f.render_widget(tabs, area);
}

fn draw_footer(f: &mut Frame, area: Rect, active: Tab) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    f.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(FAINT).bg(BG)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(if active == Tab::Compare {
            vec![
                Span::styled(
                    " ↑↓",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(":Select  ", Style::default().fg(DIM)),
                Span::styled(
                    "Space",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(":Toggle  ", Style::default().fg(DIM)),
                Span::styled(
                    "Enter",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(":Baseline  ", Style::default().fg(DIM)),
                Span::styled("←→", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Metric  ", Style::default().fg(DIM)),
                Span::styled("w", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Window  ", Style::default().fg(DIM)),
                Span::styled("x", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Export  ", Style::default().fg(DIM)),
                Span::styled("d", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Delete  ", Style::default().fg(DIM)),
                Span::styled("r", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Record", Style::default().fg(DIM)),
            ]
        } else {
            vec![
                Span::styled(
                    " 1-6",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(":Tab  ", Style::default().fg(DIM)),
                Span::styled(
                    "Tab",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(":Next  ", Style::default().fg(DIM)),
                Span::styled("q", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Quit  ", Style::default().fg(DIM)),
                Span::styled("r", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                Span::styled(":Record", Style::default().fg(DIM)),
            ]
        }))
        .style(Style::default().bg(BG)),
        chunks[1],
    );
}

fn draw_checkpoint_prompt(f: &mut Frame, area: Rect, name: &str) {
    let width = area.width.min(72);
    let height = 5;
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(vec![
            Line::from("Enter a checkpoint name:"),
            Line::from(Span::styled(
                format!("{}_", name),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Enter: start  Esc: cancel",
                Style::default().fg(DIM),
            )),
        ])
        .block(panel("Start recording"))
        .style(Style::default().bg(BG).fg(FG)),
        popup,
    );
}

fn draw_overview(f: &mut Frame, area: Rect, cache: &CachePayload) {
    // Keep the status and notes panels visible on standard 24-row terminals.
    // The full-size overview uses 22 rows for charts, but yields space as the
    // available body shrinks.
    let kpi_height = area.height.saturating_sub(6).min(22);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(kpi_height), Constraint::Min(0)])
        .split(area);
    let kpi_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(vertical[0]);
    let top_kpis = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(kpi_rows[0]);
    let bottom_kpis = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(kpi_rows[1]);

    draw_kpi_chart(
        f,
        top_kpis[0],
        "Thermal Max",
        max_group(cache, |m| m.unit == "C"),
        "C",
        40.0,
        90.0,
        thermal_max_series(cache),
        cache,
    );
    let power_series = series(cache, "power_current_watts");
    draw_kpi_chart(
        f,
        top_kpis[1],
        "Current Power",
        value(cache, "power_current_watts"),
        "W",
        0.0,
        max_or(power_series.clone(), 1.0) * 1.2,
        power_series,
        cache,
    );
    draw_kpi_chart(
        f,
        top_kpis[2],
        "CPU",
        value(cache, "cpu_usage_pct"),
        "%",
        0.0,
        100.0,
        series(cache, "cpu_usage_pct"),
        cache,
    );
    draw_kpi_chart(
        f,
        bottom_kpis[0],
        "Memory",
        value(cache, "linux_mem_used_pct"),
        "%",
        0.0,
        100.0,
        series(cache, "linux_mem_used_pct"),
        cache,
    );
    let mla_series = series(cache, "mla_mem_allocated_mb");
    draw_kpi_chart(
        f,
        bottom_kpis[1],
        "MLA Memory",
        value(cache, "mla_mem_allocated_mb"),
        "MB",
        0.0,
        max_or(mla_series.clone(), 1.0) * 1.2,
        mla_series,
        cache,
    );
    let network_series = sum_series(cache, &["net_rx_mbps", "net_tx_mbps"]);
    draw_kpi_chart(
        f,
        bottom_kpis[2],
        "Network",
        sum_values(cache, &["net_rx_mbps", "net_tx_mbps"]),
        "MB/s",
        0.0,
        max_or(network_series.clone(), 1.0) * 1.2,
        network_series,
        cache,
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(vertical[1]);
    draw_metric_table(
        f,
        cols[0],
        "Current status",
        cache,
        &[
            "cpu_load_1_pct",
            "linux_mem_used_mb",
            "mla_mem_allocated_mb",
            "power_current_watts",
            "disk_emmc_used_pct",
            "disk_nvme_used_pct",
            "net_rx_mbps",
            "net_tx_mbps",
        ],
    );
    draw_notes(f, cols[1], cache);
}

fn draw_thermal(f: &mut Frame, area: Rect, cache: &CachePayload, thermal_group: usize) {
    let thermal_metrics = thermal_metric_keys(cache);
    if thermal_metrics.is_empty() {
        f.render_widget(
            Paragraph::new("No thermal sensors are available in the current cache.")
                .block(panel("Thermal sensors"))
                .style(Style::default().fg(DIM).bg(BG)),
            area,
        );
        return;
    }

    let max_chart_height = area.height.min(10).max(6);
    let remaining_height = area.height.saturating_sub(max_chart_height);
    let can_show_all = remaining_height >= (thermal_metrics.len() as u16).saturating_mul(5);
    if can_show_all {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(max_chart_height), Constraint::Min(0)])
            .split(area);
        draw_group_thermal_chart(f, rows[0], cache);
        draw_thermal_sensor_charts(f, rows[1], cache, "All thermal sensors", &thermal_metrics);
        return;
    }

    let groups = thermal_groups(cache);
    let active_group = thermal_group.min(groups.len().saturating_sub(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(max_chart_height),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);
    draw_group_thermal_chart(f, rows[0], cache);
    draw_thermal_group_tabs(f, rows[1], &groups, active_group);
    let (group_name, keys) = &groups[active_group];
    draw_thermal_sensor_charts(f, rows[2], cache, &format!("{group_name} sensors"), keys);
}

fn draw_group_thermal_chart(f: &mut Frame, area: Rect, cache: &CachePayload) {
    draw_line_chart(
        f,
        area,
        "Thermal max",
        thermal_max_series(cache),
        40.0,
        90.0,
        "C",
        cache,
    );
}

fn draw_thermal_group_tabs(
    f: &mut Frame,
    area: Rect,
    groups: &[(String, Vec<String>)],
    active_group: usize,
) {
    let titles: Vec<Line> = groups
        .iter()
        .enumerate()
        .map(|(idx, (name, keys))| Line::from(format!(" {} {} ({}) ", idx + 1, name, keys.len())))
        .collect();
    let tabs = Tabs::new(titles)
        .select(active_group)
        .block(panel("Thermal sensor groups  ◀ ▶"))
        .style(Style::default().fg(FG).bg(BG))
        .highlight_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD));
    f.render_widget(tabs, area);
}

fn draw_thermal_sensor_charts(
    f: &mut Frame,
    area: Rect,
    cache: &CachePayload,
    title: &str,
    keys: &[String],
) {
    let block = panel(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if keys.is_empty() {
        f.render_widget(
            Paragraph::new("No sensors in this group.").style(Style::default().fg(DIM).bg(BG)),
            inner,
        );
        return;
    }

    let min_chart_height = 6u16;
    let max_rows = (inner.height / min_chart_height).max(1) as usize;
    if keys.len() > max_rows && inner.width >= 100 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);
        let split = keys.len().div_ceil(2);
        draw_thermal_sensor_chart_column(f, cols[0], cache, &keys[..split]);
        draw_thermal_sensor_chart_column(f, cols[1], cache, &keys[split..]);
    } else {
        draw_thermal_sensor_chart_column(f, inner, cache, keys);
    }
}

fn draw_thermal_sensor_chart_column(
    f: &mut Frame,
    area: Rect,
    cache: &CachePayload,
    keys: &[String],
) {
    if keys.is_empty() {
        return;
    }
    let constraints: Vec<Constraint> = keys
        .iter()
        .map(|_| Constraint::Ratio(1, keys.len() as u32))
        .collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    for (row, key) in rows.iter().zip(keys.iter()) {
        if let Some(metric) = metric(cache, key) {
            draw_line_chart(
                f,
                *row,
                &metric.short,
                series(cache, key),
                40.0,
                90.0,
                "C",
                cache,
            );
        }
    }
}

fn draw_power(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(12), Constraint::Min(0)])
        .split(area);
    let kpis = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(rows[0]);

    let scale_max = max_or(series(cache, "power_peak_watts"), 1.0) * 1.2;
    draw_kpi_chart(
        f,
        kpis[0],
        "Current",
        value(cache, "power_current_watts"),
        "W",
        0.0,
        scale_max,
        series(cache, "power_current_watts"),
        cache,
    );
    draw_kpi_chart(
        f,
        kpis[1],
        "Session average",
        value(cache, "power_average_watts"),
        "W",
        0.0,
        scale_max,
        series(cache, "power_average_watts"),
        cache,
    );
    draw_power_peak(f, kpis[2], value(cache, "power_peak_watts"), scale_max);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(rows[1]);
    draw_line_chart(
        f,
        columns[0],
        "Current board power",
        series(cache, "power_current_watts"),
        0.0,
        scale_max,
        "W",
        cache,
    );

    let details = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(columns[1]);
    draw_power_status(f, details[0], cache);
    draw_power_rails(f, details[1], cache);
}

fn draw_power_peak(f: &mut Frame, area: Rect, peak: Option<f64>, scale_max: f64) {
    let block = panel("Session peak");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    f.render_widget(
        Paragraph::new(format_value(peak, "W"))
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(GREEN)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
        rows[0],
    );
    let ratio = peak
        .map(|value| value / scale_max.max(0.001))
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    f.render_widget(
        Gauge::default()
            .ratio(ratio)
            .gauge_style(Style::default().fg(GREEN).bg(FAINT))
            .label(format!("{:.0}% of {:.1} W scale", ratio * 100.0, scale_max)),
        rows[1],
    );
    f.render_widget(
        Paragraph::new("Maximum valid total since daemon start")
            .alignment(Alignment::Center)
            .style(Style::default().fg(DIM).bg(BG)),
        rows[2],
    );
}

fn draw_power_status(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let Some(power) = cache.power.as_ref() else {
        f.render_widget(
            Paragraph::new(
                "Power status is unavailable. The cache may have been produced by an older Sentinel daemon.",
            )
            .block(panel("Collector status"))
            .style(Style::default().fg(DIM).bg(BG))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let state = if power.valid_samples == 0 {
        Span::styled(
            "UNAVAILABLE",
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
        )
    } else if !power.last_sample_valid || power.last_error.is_some() {
        Span::styled(
            "DEGRADED",
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "ACTIVE",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        )
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("State ", Style::default().fg(DIM)),
            state,
            Span::styled("  Profile ", Style::default().fg(DIM)),
            Span::styled(power.profile.clone(), Style::default().fg(FG)),
        ]),
        Line::from(format!(
            "100 ms-compatible sampling: {} ms  ·  duration {}",
            power.sample_interval_ms,
            format_duration(power.duration_seconds)
        )),
        Line::from(format!(
            "valid samples {}  ·  failed samples {}",
            power.valid_samples, power.failed_samples
        )),
    ];
    if let Some(error) = &power.last_error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(YELLOW),
        )));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Collector status"))
            .style(Style::default().fg(FG).bg(BG))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_power_rails(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let rails = cache
        .power
        .as_ref()
        .map(|power| power.rails.as_slice())
        .unwrap_or_default();
    let rows = rails.iter().map(|rail| {
        let color = if rail.current_watts.is_some() {
            GREEN
        } else {
            DIM
        };
        Row::new(vec![
            Cell::from(rail.label.clone()),
            Cell::from(Span::styled(
                format_value(rail.current_watts, "W"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            Cell::from(rail.samples.to_string()),
            Cell::from(rail.errors.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new(vec!["Rail", "Power", "Samples", "Errors"])
            .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
    )
    .block(panel("PMBus rails"))
    .style(Style::default().bg(BG).fg(FG));
    f.render_widget(table, area);
}

fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

fn draw_system(f: &mut Frame, area: Rect, cache: &CachePayload, selected_core: usize) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    draw_cpu_bars(f, cols[0], cache, selected_core);
    draw_system_bars(f, cols[1], cache);
}

fn draw_storage(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(13), Constraint::Min(0)])
        .split(area);
    let charts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(rows[0]);
    draw_line_chart(
        f,
        charts[0],
        "eMMC used",
        series(cache, "disk_emmc_used_pct"),
        0.0,
        100.0,
        "%",
        cache,
    );
    draw_line_chart(
        f,
        charts[1],
        "NVMe used",
        series(cache, "disk_nvme_used_pct"),
        0.0,
        100.0,
        "%",
        cache,
    );
    draw_line_chart(
        f,
        charts[2],
        "Network",
        sum_series(cache, &["net_rx_mbps", "net_tx_mbps"]),
        0.0,
        max_or(sum_series(cache, &["net_rx_mbps", "net_tx_mbps"]), 1.0),
        "MB/s",
        cache,
    );
    draw_metric_table(
        f,
        rows[1],
        "Storage and network metrics",
        cache,
        &[
            "disk_emmc_used_pct",
            "disk_emmc_used_mb",
            "disk_emmc_read_mbps",
            "disk_emmc_write_mbps",
            "disk_nvme_used_pct",
            "disk_nvme_used_mb",
            "disk_nvme_read_mbps",
            "disk_nvme_write_mbps",
            "net_rx_mbps",
            "net_tx_mbps",
        ],
    );
}
const COMPARE_METRICS: [(&str, &str, &str); 6] = [
    ("power_current_watts", "Total power", "W"),
    ("__thermal_max", "Thermal maximum", "C"),
    ("cpu_usage_pct", "CPU utilization", "%"),
    ("cpu_load_1_pct", "CPU load", "%"),
    ("linux_mem_used_mb", "RAM used", "MB"),
    ("mla_mem_allocated_mb", "MLA memory", "MB"),
];
const RUN_COLORS: [Color; 4] = [Color::Cyan, Color::Green, Color::Yellow, Color::Magenta];

fn draw_compare(f: &mut Frame, area: Rect, state: &CompareState) {
    if state.runs.is_empty() {
        f.render_widget(
            Paragraph::new(
                "No completed runs yet.\n\nPress r to start a named checkpoint, run the workload, then press r again to stop and save it.",
            )
            .block(panel("Compare Runs"))
            .style(Style::default().fg(DIM).bg(BG))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(area);
    draw_compare_run_list(f, columns[0], state);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(60),
            Constraint::Min(0),
        ])
        .split(columns[1]);
    draw_compare_metric_selector(f, right[0], state);
    draw_compare_chart(f, right[1], state);
    draw_compare_summary(f, right[2], state);
}

fn draw_compare_metric_selector(f: &mut Frame, area: Rect, state: &CompareState) {
    let titles: Vec<Line> = COMPARE_METRICS
        .iter()
        .map(|(_, label, _)| Line::from(format!(" {label} ")))
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .select(state.metric)
            .block(panel("Metric series  ◀ ▶"))
            .style(Style::default().fg(DIM).bg(BG))
            .highlight_style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        area,
    );
}

fn draw_compare_run_list(f: &mut Frame, area: Rect, state: &CompareState) {
    let rows = state.runs.iter().enumerate().map(|(index, run)| {
        let enabled = state.visible.contains(&run.metadata.id);
        let baseline = state.baseline.as_deref() == Some(run.metadata.id.as_str());
        let color = visible_run_index(state, &run.metadata.id)
            .map(|index| RUN_COLORS[index])
            .unwrap_or(DIM);
        let marker = if baseline { "B" } else { " " };
        Row::new(vec![
            Cell::from(if enabled { "●" } else { "○" }).style(Style::default().fg(color)),
            Cell::from(marker),
            Cell::from(truncate_text(&run.metadata.name, 22)),
        ])
        .style(if index == state.selected {
            Style::default().fg(FG).add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(FG)
        })
    });
    let message = state
        .message
        .as_deref()
        .unwrap_or("Space toggles visibility; Enter sets baseline.");
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(4)])
        .split(area);
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Min(8),
            ],
        )
        .header(
            Row::new(vec!["", "B", "Run"])
                .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
        )
        .block(panel("Saved runs"))
        .style(Style::default().bg(BG)),
        split[0],
    );
    f.render_widget(
        Paragraph::new(message)
            .block(panel("Status"))
            .style(Style::default().fg(DIM).bg(BG))
            .wrap(Wrap { trim: true }),
        split[1],
    );
}

fn draw_compare_chart(f: &mut Frame, area: Rect, state: &CompareState) {
    let (key, label, unit) = COMPARE_METRICS[state.metric];
    let selected: Vec<&SavedRun> = state
        .runs
        .iter()
        .filter(|run| state.visible.contains(&run.metadata.id))
        .take(4)
        .collect();
    if selected.is_empty() {
        f.render_widget(
            Paragraph::new("Select at least one run with Space.")
                .block(panel(format!("{label} · no runs selected")))
                .style(Style::default().fg(DIM).bg(BG)),
            area,
        );
        return;
    }
    let overlap_seconds = selected
        .iter()
        .filter_map(|run| {
            run.samples
                .last()
                .map(|sample| elapsed_seconds(run, sample))
        })
        .reduce(f64::min);
    let limit = state.overlap.then_some(overlap_seconds).flatten();
    let points: Vec<Vec<(f64, f64)>> = selected
        .iter()
        .map(|run| compare_series(run, key, limit))
        .collect();
    let datasets: Vec<Dataset> = selected
        .iter()
        .zip(points.iter())
        .enumerate()
        .map(|(index, (run, points))| {
            Dataset::default()
                .name(format!("{} {}", run_symbol(index), run.metadata.name))
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(RUN_COLORS[index]))
                .data(points)
        })
        .collect();
    let x_max = points
        .iter()
        .filter_map(|points| points.last().map(|point| point.0))
        .fold(1.0f64, f64::max);
    let values = points
        .iter()
        .flat_map(|points| points.iter().map(|point| point.1));
    let (mut y_min, mut y_max) = values.fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
    );
    if !y_min.is_finite() || !y_max.is_finite() {
        y_min = 0.0;
        y_max = 1.0;
    } else {
        let padding = ((y_max - y_min) * 0.1).max(0.5);
        y_min = (y_min - padding).max(0.0);
        y_max += padding;
    }
    let window = if state.overlap {
        "common overlap"
    } else {
        "full duration"
    };
    let chart = Chart::new(datasets)
        .block(panel(format!("{label} · {window} · elapsed time")))
        .x_axis(
            Axis::default()
                .title("seconds")
                .style(Style::default().fg(DIM))
                .bounds([0.0, x_max])
                .labels(vec![Span::raw("0"), Span::raw(format!("{x_max:.1}"))]),
        )
        .y_axis(
            Axis::default()
                .title(unit)
                .style(Style::default().fg(DIM))
                .bounds([y_min, y_max])
                .labels(vec![
                    Span::raw(format!("{y_min:.1}")),
                    Span::raw(format!("{y_max:.1}")),
                ]),
        )
        .legend_position(Some(ratatui::widgets::LegendPosition::TopLeft))
        .style(Style::default().bg(BG));
    f.render_widget(chart, area);
}

fn draw_compare_summary(f: &mut Frame, area: Rect, state: &CompareState) {
    let (key, label, unit) = COMPARE_METRICS[state.metric];
    let selected: Vec<&SavedRun> = state
        .runs
        .iter()
        .filter(|run| state.visible.contains(&run.metadata.id))
        .take(4)
        .collect();
    let overlap = selected
        .iter()
        .filter_map(|run| {
            run.samples
                .last()
                .map(|sample| elapsed_seconds(run, sample))
        })
        .reduce(f64::min);
    let limit = state.overlap.then_some(overlap).flatten();
    let baseline_mean = selected
        .iter()
        .find(|run| state.baseline.as_deref() == Some(run.metadata.id.as_str()))
        .and_then(|run| compare_stats(run, key, limit).mean);
    let rows = selected.iter().enumerate().map(|(index, run)| {
        let stats = compare_stats(run, key, limit);
        let delta = match (stats.mean, baseline_mean) {
            (Some(value), Some(baseline)) if baseline.abs() > f64::EPSILON => {
                format!("{:+.1}%", (value - baseline) / baseline * 100.0)
            }
            _ => "-".into(),
        };
        let energy = if key == "power_current_watts" {
            runs::integrate_energy_until(run, key, limit)
                .map(|value| format!("{value:.1} J"))
                .unwrap_or_else(|| "-".into())
        } else {
            "-".into()
        };
        Row::new(vec![
            Cell::from(format!(
                "{} {}",
                run_symbol(index),
                truncate_text(&run.metadata.name, 16)
            ))
            .style(Style::default().fg(RUN_COLORS[index])),
            Cell::from(stats.count.to_string()),
            Cell::from(format_optional(stats.minimum, unit)),
            Cell::from(format_optional(stats.mean, unit)),
            Cell::from(format_optional(stats.median, unit)),
            Cell::from(format_optional(stats.p95, unit)),
            Cell::from(format_optional(stats.maximum, unit)),
            Cell::from(delta),
            Cell::from(energy),
        ])
    });
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(17),
                Constraint::Length(7),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(9),
                Constraint::Length(10),
            ],
        )
        .header(
            Row::new(vec![
                "Run", "Samples", "Min", "Mean", "Median", "P95", "Max", "Δ base", "Energy",
            ])
            .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
        )
        .block(panel(format!("{label} summary")))
        .style(Style::default().bg(BG).fg(FG)),
        area,
    );
}

fn compare_series(run: &SavedRun, key: &str, limit: Option<f64>) -> Vec<(f64, f64)> {
    run.samples
        .iter()
        .filter_map(|sample| {
            let elapsed = elapsed_seconds(run, sample);
            if limit.is_some_and(|limit| elapsed > limit) {
                return None;
            }
            compare_value(run, sample, key).map(|value| (elapsed, value))
        })
        .collect()
}

fn compare_value(run: &SavedRun, sample: &crate::model::Sample, key: &str) -> Option<f64> {
    if key == "__thermal_max" {
        run.metrics
            .iter()
            .filter(|metric| metric.unit == "C")
            .filter_map(|metric| sample.values.get(&metric.key).copied().flatten())
            .filter(|value| value.is_finite())
            .reduce(f64::max)
    } else {
        sample
            .values
            .get(key)
            .copied()
            .flatten()
            .filter(|value| value.is_finite())
    }
}

fn compare_stats(run: &SavedRun, key: &str, limit: Option<f64>) -> runs::MetricStatistics {
    let mut values: Vec<f64> = compare_series(run, key, limit)
        .into_iter()
        .map(|point| point.1)
        .collect();
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return runs::MetricStatistics {
            count: 0,
            minimum: None,
            maximum: None,
            mean: None,
            median: None,
            p95: None,
        };
    }
    let percentile = |fraction: f64| {
        let index = ((values.len() - 1) as f64 * fraction).round() as usize;
        values[index]
    };
    let mean = Some(values.iter().sum::<f64>() / values.len() as f64);
    runs::MetricStatistics {
        count: values.len(),
        minimum: values.first().copied(),
        maximum: values.last().copied(),
        mean,
        median: Some(percentile(0.5)),
        p95: Some(percentile(0.95)),
    }
}

fn elapsed_seconds(run: &SavedRun, sample: &crate::model::Sample) -> f64 {
    (sample.timestamp - run.metadata.started_at)
        .num_microseconds()
        .unwrap_or(0)
        .max(0) as f64
        / 1_000_000.0
}

fn visible_run_index(state: &CompareState, id: &str) -> Option<usize> {
    state
        .runs
        .iter()
        .filter(|run| state.visible.contains(&run.metadata.id))
        .take(4)
        .position(|run| run.metadata.id == id)
}

fn run_symbol(index: usize) -> &'static str {
    ["⠿", "•", "■", "▮"].get(index).copied().unwrap_or("•")
}

fn format_optional(value: Option<f64>, unit: &str) -> String {
    value
        .map(|value| format!("{value:.2} {unit}"))
        .unwrap_or_else(|| "-".into())
}

fn truncate_text(value: &str, width: usize) -> String {
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

fn draw_kpi_chart(
    f: &mut Frame,
    area: Rect,
    title: &str,
    current: Option<f64>,
    unit: &str,
    min: f64,
    max: f64,
    samples: Vec<f64>,
    cache: &CachePayload,
) {
    let max = max.max(min + 0.001);
    let current_text = format_value(current, unit);
    let scale_text = format!("scale {}-{}", compact_number(min), compact_number(max));
    let block = panel(format!("{title}  {current_text}  ·  {scale_text} {unit}"));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let color = match unit {
        "C" => level_color(current, 70.0, 85.0),
        "%" => level_color(current, 80.0, 95.0),
        _ => GREEN,
    };
    let points = chart_points(&samples);
    let x_max = points.last().map(|point| point.0).unwrap_or(1.0).max(1.0);
    let chart = Chart::new(vec![Dataset::default()
        .name("trend")
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(color))
        .data(&points)])
    .x_axis(
        Axis::default()
            .style(Style::default().fg(DIM))
            .bounds([0.0, x_max])
            .labels(time_axis_labels(cache)),
    )
    .y_axis(
        Axis::default()
            .style(Style::default().fg(DIM))
            .bounds([min, max])
            .labels(vec![
                Span::raw(compact_number(min)),
                Span::raw(compact_number(max)),
            ]),
    )
    .style(Style::default().bg(BG));
    f.render_widget(chart, inner);
}

fn chart_points(samples: &[f64]) -> Vec<(f64, f64)> {
    let points: Vec<(f64, f64)> = samples
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .enumerate()
        .map(|(idx, value)| (idx as f64, value))
        .collect();
    if points.is_empty() {
        vec![(0.0, 0.0)]
    } else {
        points
    }
}

fn time_axis_labels(cache: &CachePayload) -> Vec<Span<'static>> {
    let first = cache.samples.first().map(|sample| sample.timestamp);
    let latest = cache
        .latest
        .as_ref()
        .map(|sample| sample.timestamp)
        .or_else(|| cache.samples.last().map(|sample| sample.timestamp));
    let left = first
        .zip(latest)
        .map(|(first, latest)| format_age_label(latest.signed_duration_since(first)))
        .unwrap_or_else(|| "0s ago".to_string());
    vec![Span::raw(left), Span::raw("now")]
}

fn format_age_label(duration: chrono::Duration) -> String {
    let seconds = duration.num_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 60 * 60 {
        format!("{}m ago", seconds / 60)
    } else {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours}h ago")
        } else {
            format!("{hours}h{minutes}m ago")
        }
    }
}

fn compact_number(value: f64) -> String {
    if value.abs() >= 100.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

fn draw_line_chart(
    f: &mut Frame,
    area: Rect,
    title: &str,
    samples: Vec<f64>,
    min: f64,
    max: f64,
    unit: &str,
    cache: &CachePayload,
) {
    let latest = samples.iter().rev().copied().find(|v| v.is_finite());
    let label = format!(
        "{}  {}  ·  scale {}-{} {}",
        title,
        format_value(latest, unit),
        compact_number(min),
        compact_number(max),
        unit
    );
    draw_multi_line_chart(
        f,
        area,
        &label,
        &[(title, samples, GREEN)],
        min,
        max,
        unit,
        cache,
    );
}

fn draw_multi_line_chart(
    f: &mut Frame,
    area: Rect,
    title: &str,
    series_list: &[(&str, Vec<f64>, Color)],
    min: f64,
    max: f64,
    unit: &str,
    cache: &CachePayload,
) {
    let max = max.max(min + 0.001);
    let block = panel(format!(
        "{}  ·  scale {}-{} {}",
        title,
        compact_number(min),
        compact_number(max),
        unit
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let x_max = series_list
        .iter()
        .filter_map(|(_, samples, _)| samples.len().checked_sub(1))
        .max()
        .unwrap_or(1) as f64;
    let points: Vec<Vec<(f64, f64)>> = series_list
        .iter()
        .map(|(_, samples, _)| chart_points(samples))
        .collect();
    let datasets: Vec<Dataset> = series_list
        .iter()
        .zip(points.iter())
        .map(|((name, _samples, color), points)| {
            Dataset::default()
                .name(*name)
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(*color))
                .data(points)
        })
        .collect();
    let chart = Chart::new(datasets)
        .x_axis(
            Axis::default()
                .style(Style::default().fg(DIM))
                .bounds([0.0, x_max.max(1.0)])
                .labels(time_axis_labels(cache)),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(DIM))
                .bounds([min, max])
                .labels(vec![
                    Span::raw(compact_number(min)),
                    Span::raw(compact_number(max)),
                ]),
        )
        .legend_position(Some(ratatui::widgets::LegendPosition::TopLeft))
        .style(Style::default().bg(BG));
    f.render_widget(chart, inner);
}

fn draw_metric_table(f: &mut Frame, area: Rect, title: &str, cache: &CachePayload, keys: &[&str]) {
    let owned: Vec<String> = keys.iter().map(|key| key.to_string()).collect();
    draw_metric_table_owned(f, area, title, cache, &owned);
}

fn draw_metric_table_owned(
    f: &mut Frame,
    area: Rect,
    title: &str,
    cache: &CachePayload,
    keys: &[String],
) {
    let rows = keys.iter().filter_map(|key| {
        let metric = metric(cache, key)?;
        let current = value(cache, key);
        let color = level_color(
            current,
            metric.warn.unwrap_or(f64::NAN),
            metric.critical.unwrap_or(f64::NAN),
        );
        Some(Row::new(vec![
            Cell::from(metric.short.clone()),
            Cell::from(metric.group.clone()),
            Cell::from(Span::styled(
                format_value(current, &metric.unit),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            Cell::from(status_text(current, metric)),
            Cell::from(short_trend(series(cache, key))),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(vec!["Metric", "Group", "Value", "Status", "Trend"])
            .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
    )
    .block(panel(title))
    .style(Style::default().bg(BG).fg(FG));
    f.render_widget(table, area);
}

fn draw_notes(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let text = if cache.errors.is_empty() {
        "Sentinel is reading the daemon cache. Use Thermal for board temperatures, Power for board power use, System for CPU/memory/MLA allocator use, and Storage/Net for eMMC, NVMe, and interface rates."
            .to_string()
    } else {
        format!("Daemon reported errors:\n{}", cache.errors.join("\n"))
    };
    f.render_widget(
        Paragraph::new(text)
            .block(panel("Notes"))
            .style(Style::default().fg(FG).bg(BG))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_cpu_bars(f: &mut Frame, area: Rect, cache: &CachePayload, selected_core: usize) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);
    draw_core_bars(f, rows[0], cache, selected_core);
    draw_process_panel(f, rows[1], cache, selected_core);
}

fn draw_core_bars(f: &mut Frame, area: Rect, cache: &CachePayload, selected_core: usize) {
    let block = panel("Per-core CPU");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();
    for (idx, metric) in cache
        .metrics
        .iter()
        .filter(|metric| metric.key.starts_with("cpu_core_"))
        .enumerate()
    {
        let current = value(cache, &metric.key).unwrap_or(0.0);
        lines.push(core_bar_line(
            &metric.short,
            current,
            format!("{current:.0}%"),
            inner.width,
            idx == selected_core,
        ));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Waiting for per-core CPU samples...",
            Style::default().fg(DIM),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(BG).fg(FG)),
        inner,
    );
}

fn draw_process_panel(f: &mut Frame, area: Rect, cache: &CachePayload, selected_core: usize) {
    let title = format!("Processes last seen on c{selected_core}");
    let rows = focused_processes(cache, selected_core)
        .into_iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|proc_| {
            Row::new(vec![
                Cell::from(proc_.pid.to_string()),
                Cell::from(
                    proc_
                        .cpu_core
                        .map(|core| format!("c{core}"))
                        .unwrap_or_else(|| "-".into()),
                ),
                Cell::from(format!("{:.1}", proc_.cpu_pct)),
                Cell::from(format!("{:.0}", proc_.rss_mb)),
                Cell::from(proc_.name.clone()),
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(vec!["PID", "CPU", "%CPU", "RSS MB", "Name"])
            .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
    )
    .block(panel(title))
    .style(Style::default().bg(BG).fg(FG));
    f.render_widget(table, area);
}

fn draw_system_bars(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(area);
    draw_line_chart(
        f,
        rows[0],
        "CPU usage",
        series(cache, "cpu_usage_pct"),
        0.0,
        100.0,
        "%",
        cache,
    );
    draw_line_chart(
        f,
        rows[1],
        "CPU load 1m",
        series(cache, "cpu_load_1_pct"),
        0.0,
        100.0,
        "%",
        cache,
    );
    draw_line_chart(
        f,
        rows[2],
        "Linux memory",
        series(cache, "linux_mem_used_pct"),
        0.0,
        100.0,
        "%",
        cache,
    );
    let mla_series = series(cache, "mla_mem_allocated_mb");
    draw_line_chart(
        f,
        rows[3],
        "MLA memory",
        mla_series.clone(),
        0.0,
        max_or(mla_series, 1.0) * 1.2,
        "MB",
        cache,
    );
    let lower_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(7), Constraint::Length(7)])
        .split(rows[4]);
    draw_metric_table(
        f,
        lower_rows[0],
        "System details",
        cache,
        &[
            "cpu_usage_pct",
            "cpu_load_1_pct",
            "linux_mem_used_pct",
            "linux_mem_used_mb",
            "mla_mem_allocated_mb",
        ],
    );
    draw_cpu_help(f, lower_rows[1]);
}

fn draw_cpu_help(f: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(vec![
            Span::styled(
                "CPU usage",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" is immediate busy time from /proc/stat over the latest sample window."),
        ]),
        Line::from(vec![
            Span::styled(
                "CPU load",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" is 1-minute runnable/waiting work normalized by logical CPU cores."),
        ]),
        Line::from(vec![Span::raw(
            "High load with low usage often points to queued or I/O-blocked work.",
        )]),
    ];
    f.render_widget(
        Paragraph::new(text)
            .block(panel("CPU interpretation"))
            .style(Style::default().fg(FG).bg(BG))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn core_bar_line(
    label: &str,
    pct: f64,
    value_text: String,
    width: u16,
    selected: bool,
) -> Line<'static> {
    generic_bar_line(label, pct, value_text, width, selected)
}

fn generic_bar_line(
    label: &str,
    pct: f64,
    value_text: String,
    width: u16,
    selected: bool,
) -> Line<'static> {
    let label_width = 8usize;
    let value_width = 10usize;
    let bar_width = width
        .saturating_sub(label_width as u16)
        .saturating_sub(value_width as u16)
        .saturating_sub(3)
        .max(6) as usize;
    let pct = pct.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
    let color = if pct >= 90.0 {
        RED
    } else if pct >= 70.0 {
        YELLOW
    } else {
        GREEN
    };
    let mut bar = String::with_capacity(bar_width);
    for idx in 0..bar_width {
        bar.push(if idx < filled { '▮' } else { ' ' });
    }
    Line::from(vec![
        Span::styled(format!("{label:<label_width$} "), label_style(selected)),
        Span::styled(bar, Style::default().fg(color)),
        Span::styled(
            format!(" {value_text:>value_width$}"),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn label_style(selected: bool) -> Style {
    if selected {
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM).add_modifier(Modifier::BOLD)
    }
}

fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(FAINT))
        .title(Span::styled(
            format!(" {} ", title.into()),
            Style::default().fg(DIM),
        ))
        .style(Style::default().bg(BG))
}

fn metric<'a>(cache: &'a CachePayload, key: &str) -> Option<&'a MetricDefinition> {
    cache.metrics.iter().find(|metric| metric.key == key)
}

fn cpu_core_count(cache: &CachePayload) -> usize {
    cache
        .metrics
        .iter()
        .filter(|metric| metric.key.starts_with("cpu_core_"))
        .count()
}

fn focused_processes(cache: &CachePayload, selected_core: usize) -> Vec<&ProcessInfo> {
    let mut processes: Vec<&ProcessInfo> = cache
        .processes
        .iter()
        .filter(|proc_| proc_.cpu_core == Some(selected_core))
        .collect();
    if processes.is_empty() {
        processes = cache.processes.iter().collect();
    }
    processes.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.rss_mb
                    .partial_cmp(&a.rss_mb)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    processes
}

fn value(cache: &CachePayload, key: &str) -> Option<f64> {
    cache
        .latest
        .as_ref()
        .and_then(|sample| sample.values.get(key).copied().flatten())
}

fn series(cache: &CachePayload, key: &str) -> Vec<f64> {
    cache
        .samples
        .iter()
        .filter_map(|sample| sample.values.get(key).copied().flatten())
        .collect()
}

fn thermal_metric_keys(cache: &CachePayload) -> Vec<String> {
    cache
        .metrics
        .iter()
        .filter(|metric| metric.unit == "C")
        .map(|metric| metric.key.clone())
        .collect()
}

fn thermal_groups(cache: &CachePayload) -> Vec<(String, Vec<String>)> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for metric in cache.metrics.iter().filter(|metric| metric.unit == "C") {
        grouped
            .entry(metric.group.clone())
            .or_default()
            .push(metric.key.clone());
    }
    let order = ["MLA", "APU", "CVU", "TOP", "Board"];
    let mut out = Vec::new();
    for group in order {
        if let Some(keys) = grouped.remove(group) {
            out.push((group.to_string(), keys));
        }
    }
    out.extend(grouped);
    out
}

fn sum_values(cache: &CachePayload, keys: &[&str]) -> Option<f64> {
    let mut total = 0.0;
    let mut found = false;
    for key in keys {
        if let Some(value) = value(cache, key) {
            total += value;
            found = true;
        }
    }
    found.then_some(total)
}

fn sum_series(cache: &CachePayload, keys: &[&str]) -> Vec<f64> {
    cache
        .samples
        .iter()
        .map(|sample| {
            keys.iter()
                .filter_map(|key| sample.values.get(*key).copied().flatten())
                .sum::<f64>()
        })
        .collect()
}

fn max_group(cache: &CachePayload, predicate: impl Fn(&MetricDefinition) -> bool) -> Option<f64> {
    cache
        .metrics
        .iter()
        .filter(|metric| predicate(metric))
        .filter_map(|metric| value(cache, &metric.key))
        .reduce(f64::max)
}

fn thermal_max_series(cache: &CachePayload) -> Vec<f64> {
    cache
        .samples
        .iter()
        .map(|sample| {
            cache
                .metrics
                .iter()
                .filter(|metric| metric.unit == "C")
                .filter_map(|metric| sample.values.get(&metric.key).copied().flatten())
                .reduce(f64::max)
                .unwrap_or(f64::NAN)
        })
        .filter(|value| value.is_finite())
        .collect()
}

fn level_color(value: Option<f64>, warn: f64, critical: f64) -> Color {
    let Some(value) = value else {
        return DIM;
    };
    if critical.is_finite() && value >= critical {
        RED
    } else if warn.is_finite() && value >= warn {
        YELLOW
    } else {
        GREEN
    }
}

fn status_text(value: Option<f64>, metric: &MetricDefinition) -> Span<'static> {
    let Some(value) = value else {
        return Span::styled("unknown", Style::default().fg(DIM));
    };
    if metric.critical.is_some_and(|critical| value >= critical) {
        Span::styled(
            "critical",
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
        )
    } else if metric.warn.is_some_and(|warn| value >= warn) {
        Span::styled("warning", Style::default().fg(YELLOW))
    } else {
        Span::styled("normal", Style::default().fg(GREEN))
    }
}

fn format_value(value: Option<f64>, unit: &str) -> String {
    let Some(value) = value else {
        return "-".into();
    };
    match unit {
        "%" => format!("{value:.1}%"),
        "C" => format!("{value:.1} C"),
        "MB" => format!("{value:.1} MB"),
        "MB/s" => format!("{value:.2} MB/s"),
        "load" => format!("{value:.2}"),
        _ => format!("{value:.2} {unit}"),
    }
}

fn normalize_series(samples: Vec<f64>, min: f64, max: f64) -> Vec<f64> {
    let max = max.max(min + 0.001);
    samples
        .into_iter()
        .filter(|value| value.is_finite())
        .map(|value| ((value - min) / (max - min) * 100.0).clamp(0.0, 100.0))
        .collect()
}

fn spark_data(samples: &[f64], width: usize) -> Vec<u64> {
    let slice = if samples.len() > width {
        &samples[samples.len() - width..]
    } else {
        samples
    };
    slice.iter().map(|v| v.round() as u64).collect()
}

fn short_trend(samples: Vec<f64>) -> String {
    let normalized = normalize_series(samples, 0.0, 100.0);
    let data = spark_data(&normalized, 18);
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    data.iter()
        .map(|value| {
            let idx = ((*value as f64 / 100.0) * 7.0).round() as usize;
            BARS[idx.min(7)]
        })
        .collect()
}

fn max_or(samples: Vec<f64>, fallback: f64) -> f64 {
    samples
        .into_iter()
        .filter(|value| value.is_finite())
        .fold(fallback, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ratatui::backend::TestBackend;

    use crate::model::{PowerRailStatus, PowerStatus, Sample};
    use crate::runs::{RunMetadata, RUN_SCHEMA};

    fn empty_compare() -> CompareState {
        CompareState {
            runs: Vec::new(),
            selected: 0,
            visible: BTreeSet::new(),
            baseline: None,
            metric: 0,
            overlap: true,
            message: None,
            pending_delete: None,
        }
    }

    #[test]
    fn power_tab_renders_totals_status_and_rails() {
        let values = BTreeMap::from([
            ("power_current_watts".into(), Some(8.0)),
            ("power_average_watts".into(), Some(7.5)),
            ("power_peak_watts".into(), Some(9.0)),
        ]);
        let sample = Sample {
            timestamp: Utc::now(),
            values,
        };
        let metrics = [
            ("power_current_watts", "Current"),
            ("power_average_watts", "Average"),
            ("power_peak_watts", "Peak"),
        ]
        .into_iter()
        .map(|(key, label)| {
            MetricDefinition::new(key, label, label, "Power", "W", label, None, None)
        })
        .collect();
        let cache = CachePayload {
            schema: 1,
            version: "0.1.0".into(),
            updated_at: Utc::now(),
            metrics,
            latest: Some(sample.clone()),
            samples: vec![sample],
            processes: Vec::new(),
            power: Some(PowerStatus {
                profile: "modalix_som".into(),
                sample_interval_ms: 100,
                duration_seconds: 12.0,
                valid_samples: 120,
                failed_samples: 0,
                last_sample_valid: true,
                last_error: None,
                rails: vec![PowerRailStatus {
                    key: "power_rail_mla_watts".into(),
                    label: "MLA 0.68V".into(),
                    current_watts: Some(1.25),
                    samples: 120,
                    errors: 0,
                }],
            }),
            errors: Vec::new(),
        };

        let backend = TestBackend::new(140, 42);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let compare = empty_compare();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &cache,
                    Tab::Power,
                    true,
                    0,
                    0,
                    &compare,
                    None,
                    &InputMode::Normal,
                )
            })
            .expect("draw power tab");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("Session average"));
        assert!(rendered.contains("Session peak"));
        assert!(rendered.contains("Maximum valid total"));
        assert!(rendered.contains("modalix_som"));
        assert!(rendered.contains("MLA 0.68V"));

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &cache,
                    Tab::Overview,
                    true,
                    0,
                    0,
                    &compare,
                    None,
                    &InputMode::Normal,
                )
            })
            .expect("draw overview tab");
        let overview: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(overview.contains("Current Power"));
    }

    #[test]
    fn overview_preserves_status_panels_on_standard_height_terminal() {
        let cache = CachePayload {
            schema: 1,
            version: "0.1.0".into(),
            updated_at: Utc::now(),
            metrics: Vec::new(),
            latest: None,
            samples: Vec::new(),
            processes: Vec::new(),
            power: None,
            errors: Vec::new(),
        };
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let compare = empty_compare();

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &cache,
                    Tab::Overview,
                    true,
                    0,
                    0,
                    &compare,
                    None,
                    &InputMode::Normal,
                )
            })
            .expect("draw overview tab");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("Current Power"));
        assert!(rendered.contains("Current status"));
        assert!(rendered.contains("Notes"));
    }

    #[test]
    fn compare_tab_renders_aligned_runs_and_summary() {
        let start = Utc::now();
        let metric = MetricDefinition::new(
            "power_current_watts",
            "Current power",
            "Power",
            "Power",
            "W",
            "Board power",
            None,
            None,
        );
        let make_run = |id: &str, name: &str, values: [f64; 3]| SavedRun {
            schema: RUN_SCHEMA,
            metadata: RunMetadata {
                id: id.into(),
                name: name.into(),
                note: None,
                tags: Vec::new(),
                started_at: start,
                ended_at: Some(start + chrono::Duration::seconds(4)),
                sample_interval_ms: Some(2_000),
                sentinel_version: "0.1.0".into(),
                system: BTreeMap::new(),
            },
            metrics: vec![metric.clone()],
            samples: values
                .into_iter()
                .enumerate()
                .map(|(index, value)| Sample {
                    timestamp: start + chrono::Duration::seconds(index as i64 * 2),
                    values: BTreeMap::from([("power_current_watts".into(), Some(value))]),
                })
                .collect(),
        };
        let baseline = make_run("base", "baseline", [2.0, 3.0, 4.0]);
        let optimized = make_run("opt", "optimized", [1.0, 2.0, 3.0]);
        let compare = CompareState {
            runs: vec![baseline, optimized],
            selected: 0,
            visible: BTreeSet::from(["base".into(), "opt".into()]),
            baseline: Some("base".into()),
            metric: 0,
            overlap: true,
            message: None,
            pending_delete: None,
        };
        let backend = TestBackend::new(150, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_compare(frame, frame.area(), &compare))
            .expect("draw compare tab");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("baseline"));
        assert!(rendered.contains("optimized"));
        assert!(rendered.contains("common overlap"));
        assert!(rendered.contains("Δ base"));
        assert!(rendered.contains("J"));
        assert!(rendered.contains("Thermal maximum"));
        assert!(rendered.contains("CPU load"));
        assert!(rendered.contains("RAM used"));
        assert!(rendered.contains("MLA memory"));
    }
}
