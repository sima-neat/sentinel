use std::collections::BTreeMap;
use std::io;
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
}

const TABS: [Tab; 5] = [
    Tab::Overview,
    Tab::Thermal,
    Tab::Power,
    Tab::System,
    Tab::Storage,
];

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Thermal => "Thermal",
            Tab::Power => "Power",
            Tab::System => "System",
            Tab::Storage => "Storage/Net",
        }
    }
}

pub fn run_ops(cache_path: &str, interval: Duration) -> Result<()> {
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

    let result = loop {
        if last_read.elapsed() >= cache_tick {
            if let Ok(next) = read_cache(cache_path) {
                cache = next;
                thermal_group = thermal_group.min(thermal_groups(&cache).len().saturating_sub(1));
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

fn draw(
    f: &mut Frame,
    cache: &CachePayload,
    active: Tab,
    clear_body: bool,
    selected_core: usize,
    thermal_group: usize,
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

    draw_header(f, layout[0], cache);
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
    }
    draw_footer(f, layout[3]);
}

fn draw_header(f: &mut Frame, area: Rect, cache: &CachePayload) {
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
    let line = Line::from(vec![
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
    ]);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);
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

fn draw_footer(f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    f.render_widget(
        Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(FAINT).bg(BG)),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " 1-5",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Tab  ", Style::default().fg(DIM)),
            Span::styled(
                "Tab",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Next  ", Style::default().fg(DIM)),
            Span::styled("q", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
            Span::styled(":Quit", Style::default().fg(DIM)),
        ]))
        .style(Style::default().bg(BG)),
        chunks[1],
    );
}

fn draw_overview(f: &mut Frame, area: Rect, cache: &CachePayload) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(12), Constraint::Min(0)])
        .split(area);
    let kpis = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
        ])
        .split(vertical[0]);

    draw_kpi_chart(
        f,
        kpis[0],
        "Thermal Max",
        max_group(cache, |m| m.unit == "C"),
        "C",
        40.0,
        90.0,
        thermal_max_series(cache),
        cache,
    );
    draw_kpi_chart(
        f,
        kpis[1],
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
        kpis[2],
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
        kpis[3],
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
        kpis[4],
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
        "Sentinel is reading the daemon cache. Use the Thermal tab for board temperatures, System for CPU/memory/MLA allocator use, and Storage/Net for eMMC, NVMe, and interface rates."
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
        terminal
            .draw(|frame| draw(frame, &cache, Tab::Power, true, 0, 0))
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
    }
}
