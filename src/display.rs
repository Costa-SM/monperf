//! Terminal UI display using ratatui.

use crate::alert::Alert;
use crate::metrics::{CpuMetrics, DiskMetrics, MemoryMetrics, NetworkMetrics};
use crate::process::ProcessMetrics;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Sparkline},
    Frame,
};

/// Get the last N elements from a slice to fit the graph width
/// The sparkline uses 1 char per data point, so we use area.width - 2 (for borders)
fn slice_for_width<'a>(data: &'a [u64], area: Rect) -> &'a [u64] {
    let graph_width = area.width.saturating_sub(2) as usize;
    if data.len() <= graph_width {
        data
    } else {
        &data[data.len() - graph_width..]
    }
}

/// Format bytes to human readable string
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format bytes per second
pub fn format_throughput(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    if bytes_per_sec >= GB {
        format!("{:.2} GB/s", bytes_per_sec / GB)
    } else if bytes_per_sec >= MB {
        format!("{:.2} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.2} KB/s", bytes_per_sec / KB)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// Format bytes to shorter human readable string (no space, for compact output)
pub fn format_bytes_short(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1}T", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

/// Truncate a string to max length, adding ".." if truncated
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 2 {
        s[..max_len].to_string()
    } else {
        format!("{}..", &s[..max_len - 2])
    }
}

/// Get color based on percentage value
fn percentage_color(value: f64, warn_threshold: f64, crit_threshold: f64) -> Color {
    if value >= crit_threshold {
        Color::Red
    } else if value >= warn_threshold {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Format a percentage with color based on value
fn percentage_style(value: f64, warn_threshold: f64, crit_threshold: f64) -> Style {
    Style::default()
        .fg(percentage_color(value, warn_threshold, crit_threshold))
        .add_modifier(if value >= crit_threshold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        })
}

/// Render CPU metrics widget with per-core overview
pub fn render_cpu(f: &mut Frame, area: Rect, cpu: &CpuMetrics, history: Option<&CpuHistory>) {
    let block = Block::default()
        .title(" CPU ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Calculate how many lines we need for per-core display
    let cores_per_row = 8; // Show 8 cores per row
    let core_rows = (cpu.core_count + cores_per_row - 1) / cores_per_row;
    let core_display_height = core_rows.max(1) as u16;

    // Split into: overall gauge, per-core display, sparkline, details
    // Layout: details at top, per-core bars, sparkline at bottom
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                   // Overall CPU (compact)
            Constraint::Length(core_display_height), // Per-core bars
            Constraint::Length(2),                   // Details (load, user/sys/iowait)
            Constraint::Min(4),                      // Sparkline graph at bottom
        ])
        .split(inner);

    // Overall CPU - compact single line with mini progress bar
    let cpu_pct = cpu.total_utilization.clamp(0.0, 100.0);
    // Fixed text: "Total: " (7) + "XXX.X%" (6) + " [" (2) + "]" (1) = 16 chars
    let bar_width = (chunks[0].width as usize).saturating_sub(16).min(30);
    let filled = ((cpu_pct / 100.0) * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);
    
    let bar_color = percentage_color(cpu_pct, 70.0, 90.0);
    let overall_line = Line::from(vec![
        Span::raw("Total: "),
        Span::styled(
            format!("{:>5.1}%", cpu_pct),
            Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ["),
        Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::raw("]"),
    ]);
    f.render_widget(Paragraph::new(overall_line), chunks[0]);

    // Per-core compact visualization
    let mut core_lines: Vec<Line> = Vec::new();
    
    for row in 0..core_rows {
        let start_core = row * cores_per_row;
        let end_core = (start_core + cores_per_row).min(cpu.core_count);
        
        let mut spans: Vec<Span> = Vec::new();
        
        for core_idx in start_core..end_core {
            if let Some(core) = cpu.per_core.get(core_idx) {
                let pct = core.utilization_percent.clamp(0.0, 100.0);
                let color = percentage_color(pct, 70.0, 90.0);
                
                // Create a mini bar for each core: [##  ] format
                let mini_bar_width = 4;
                let mini_filled = ((pct / 100.0) * mini_bar_width as f64).round() as usize;
                let mini_empty = mini_bar_width - mini_filled;
                
                spans.push(Span::styled(
                    format!("{:>2}:", core.core_id),
                    Style::default().fg(Color::DarkGray),
                ));
                spans.push(Span::styled(
                    "█".repeat(mini_filled),
                    Style::default().fg(color),
                ));
                spans.push(Span::styled(
                    "░".repeat(mini_empty),
                    Style::default().fg(Color::DarkGray),
                ));
                spans.push(Span::raw(" "));
            }
        }
        
        core_lines.push(Line::from(spans));
    }
    
    f.render_widget(Paragraph::new(core_lines), chunks[1]);

    // CPU details
    let iowait_style = if cpu.iowait_percent > 30.0 {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if cpu.iowait_percent > 10.0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };

    // Color load average based on core count
    let load_color = if cpu.load_avg.0 > cpu.core_count as f64 {
        Color::Red
    } else if cpu.load_avg.0 > cpu.core_count as f64 * 0.7 {
        Color::Yellow
    } else {
        Color::Green
    };

    let details = vec![
        Line::from(vec![
            Span::raw("Load: "),
            Span::styled(
                format!("{:.2} {:.2} {:.2}", cpu.load_avg.0, cpu.load_avg.1, cpu.load_avg.2),
                Style::default().fg(load_color),
            ),
            Span::raw("  User: "),
            Span::styled(format!("{:.1}%", cpu.user_percent), Style::default().fg(Color::Cyan)),
            Span::raw("  Sys: "),
            Span::styled(format!("{:.1}%", cpu.system_percent), Style::default().fg(Color::Magenta)),
            Span::raw("  IOW: "),
            Span::styled(format!("{:.1}%", cpu.iowait_percent), iowait_style),
        ]),
        Line::from(vec![
            Span::raw("Ctx/s: "),
            Span::styled(
                format!("{}", cpu.context_switches_delta.unwrap_or(0)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw("  Intr/s: "),
            Span::styled(
                format!("{}", cpu.interrupts_delta.unwrap_or(0)),
                Style::default().fg(Color::Gray),
            ),
        ]),
    ];

    f.render_widget(Paragraph::new(details), chunks[2]);

    // CPU history sparkline at bottom (sized to graph width)
    if let Some(hist) = history {
        if !hist.utilization.is_empty() {
            let data = slice_for_width(&hist.utilization, chunks[3]);
            let max_val = data.iter().max().copied().unwrap_or(100).max(100);
            let cpu_sparkline = Sparkline::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(format!(" CPU % (max {}%) ", max_val)))
                .data(data)
                .max(max_val)
                .style(Style::default().fg(Color::Cyan));
            f.render_widget(cpu_sparkline, chunks[3]);
        }
    }
}

/// Helper to render a labeled progress bar with readable text
fn render_progress_bar(
    label: &str,
    value: &str,
    percent: f64,
    width: usize,
    warn: f64,
    crit: f64,
) -> Line<'static> {
    let bar_width = width.saturating_sub(label.len() + value.len() + 5);
    let pct = percent.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);
    let color = percentage_color(pct, warn, crit);

    Line::from(vec![
        Span::raw(label.to_string()),
        Span::raw(" ["),
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::raw("] "),
        Span::styled(value.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ])
}

/// Render memory metrics widget
pub fn render_memory(f: &mut Frame, area: Rect, mem: &MemoryMetrics, history: Option<&MemoryHistory>) {
    let block = Block::default()
        .title(" Memory ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Layout: text at top, sparkline fills remaining space at bottom
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // Bars + details (fixed)
            Constraint::Min(6),    // Sparkline graph at bottom (fills remaining)
        ])
        .split(inner);
    
    // Split text area
    let text_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // System memory bar
            Constraint::Length(1), // Cgroup memory bar
            Constraint::Length(2), // Details
        ])
        .split(main_chunks[0]);

    let bar_width = text_chunks[0].width as usize;

    // System memory bar
    let mem_label = format!(
        "{} / {} ({:.1}%)",
        format_bytes(mem.used),
        format_bytes(mem.total),
        mem.used_percent
    );
    let mem_bar = render_progress_bar("RAM:", &mem_label, mem.used_percent, bar_width, 70.0, 90.0);
    f.render_widget(Paragraph::new(mem_bar), text_chunks[0]);

    // Cgroup memory bar (if available)
    if let (Some(limit), Some(current), Some(percent)) =
        (mem.cgroup_limit, mem.cgroup_current, mem.cgroup_usage_percent)
    {
        let cgroup_label = format!(
            "{} / {} ({:.1}%)",
            format_bytes(current),
            format_bytes(limit),
            percent
        );
        let cgroup_bar = render_progress_bar("Cgroup:", &cgroup_label, percent, bar_width, 80.0, 95.0);
        f.render_widget(Paragraph::new(cgroup_bar), text_chunks[1]);
    } else {
        let no_cgroup = Line::from(vec![
            Span::raw("Cgroup: "),
            Span::styled("N/A", Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(no_cgroup), text_chunks[1]);
    }

    // Memory details
    let swap_color = if mem.swap_used > 0 {
        Color::Yellow
    } else {
        Color::White
    };

    let details = vec![
        Line::from(vec![
            Span::raw("Avail: "),
            Span::styled(format_bytes(mem.available), Style::default().fg(Color::Green)),
            Span::raw(" Buf: "),
            Span::styled(format_bytes(mem.buffers), Style::default().fg(Color::Gray)),
            Span::raw(" Cache: "),
            Span::styled(format_bytes(mem.cached), Style::default().fg(Color::Gray)),
        ]),
        Line::from(vec![
            Span::raw("Swap: "),
            Span::styled(
                format!("{}/{}", format_bytes(mem.swap_used), format_bytes(mem.swap_total)),
                Style::default().fg(swap_color),
            ),
            Span::raw(" PgFlt: "),
            Span::styled(
                format!("Maj:{} Min:{}", 
                    mem.major_faults_delta.unwrap_or(0),
                    mem.minor_faults_delta.unwrap_or(0)
                ),
                Style::default().fg(Color::Gray),
            ),
        ]),
    ];
    f.render_widget(Paragraph::new(details), text_chunks[2]);

    // Memory history sparkline at bottom (fills remaining space)
    // Memory history sparkline (sized to graph width)
    if let Some(hist) = history {
        if !hist.used_percent.is_empty() {
            // Determine if we should show cgroup or system memory
            let has_cgroup = hist.cgroup_percent.iter().any(|&v| v > 0);
            let (raw_data, color) = if has_cgroup {
                (&hist.cgroup_percent[..], Color::Red)
            } else {
                (&hist.used_percent[..], Color::Magenta)
            };
            let data = slice_for_width(raw_data, main_chunks[1]);
            let max_val = data.iter().max().copied().unwrap_or(100);
            let title = if has_cgroup {
                format!(" Cgroup % (max {}%) ", max_val)
            } else {
                format!(" RAM % (max {}%) ", max_val)
            };
            
            let sparkline = Sparkline::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(title))
                .data(data)
                .max(100)  // Memory is always 0-100%
                .style(Style::default().fg(color));
            f.render_widget(sparkline, main_chunks[1]);
        }
    }
}

/// Render disk metrics widget
pub fn render_disk(f: &mut Frame, area: Rect, disk: &DiskMetrics, history: Option<&DiskHistory>) {
    let block = Block::default()
        .title(" Disk I/O ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Calculate how many rows we need for disk display (3 disks per row with R/W values)
    let disks_per_row = 3;
    let disk_rows = if disk.disks.is_empty() { 
        0 
    } else { 
        (disk.disks.len() + disks_per_row - 1) / disks_per_row 
    };
    let disk_display_height = disk_rows.max(1) as u16;

    // Layout: text at top, sparklines fill remaining space at bottom
    let text_height = 1 + disk_display_height;
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(text_height),  // Total + per-disk bars
            Constraint::Min(6),               // Sparklines area (fills remaining)
        ])
        .split(inner);
    
    // Split text area
    let text_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                   // Total throughput
            Constraint::Length(disk_display_height), // Per-disk utilization bars
        ])
        .split(main_chunks[0]);
    
    // Split sparklines area evenly
    let graph_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(1, 2),  // Read sparkline
            Constraint::Ratio(1, 2),  // Write sparkline
        ])
        .split(main_chunks[1]);

    // Total throughput line with colored R/W values
    let mut total_spans = vec![
        Span::raw("Total: "),
        Span::styled(format!("R {}", format_throughput(disk.total_read_bytes_per_sec)), Style::default().fg(Color::Cyan)),
        Span::raw(" | "),
        Span::styled(format!("W {}", format_throughput(disk.total_write_bytes_per_sec)), Style::default().fg(Color::Yellow)),
    ];
    if let Some(ref spill) = disk.spill_dir_info {
        total_spans.push(Span::raw(format!("  Spill: {}", format_bytes(spill.used_bytes))));
    }
    f.render_widget(Paragraph::new(Line::from(total_spans)), text_chunks[0]);

    // Per-disk utilization bars with R/W throughput values
    let mut disk_lines: Vec<Line> = Vec::new();
    
    for row in 0..disk_rows {
        let start_idx = row * disks_per_row;
        let end_idx = (start_idx + disks_per_row).min(disk.disks.len());
        
        let mut spans: Vec<Span> = Vec::new();
        
        for disk_idx in start_idx..end_idx {
            if let Some(d) = disk.disks.get(disk_idx) {
                let pct = d.utilization_percent.clamp(0.0, 100.0);
                let color = percentage_color(pct, 50.0, 80.0);
                
                // Shorten device name (nvme0n1 -> n0, sda -> sda)
                let short_name = if d.device.starts_with("nvme") {
                    // nvme0n1 -> n0, nvme5n1 -> n5
                    let num = d.device.chars()
                        .filter(|c| c.is_ascii_digit())
                        .take(1)
                        .collect::<String>();
                    format!("n{}", num)
                } else {
                    d.device.chars().take(3).collect()
                };
                
                // Create a mini bar for each disk: name:[####  ] R/W format
                let mini_bar_width: usize = 4;
                let mini_filled = ((pct / 100.0) * mini_bar_width as f64).round() as usize;
                let mini_empty = mini_bar_width.saturating_sub(mini_filled);
                
                // Format short R/W values with colors matching graphs
                let read_short = format_throughput_short(d.read_bytes_per_sec);
                let write_short = format_throughput_short(d.write_bytes_per_sec);
                
                spans.push(Span::styled(
                    format!("{:>2}:", short_name),
                    Style::default().fg(Color::DarkGray),
                ));
                spans.push(Span::styled(
                    "█".repeat(mini_filled),
                    Style::default().fg(color),
                ));
                spans.push(Span::styled(
                    "░".repeat(mini_empty),
                    Style::default().fg(Color::DarkGray),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(read_short, Style::default().fg(Color::Cyan)));
                spans.push(Span::raw("/"));
                spans.push(Span::styled(write_short, Style::default().fg(Color::Yellow)));
                spans.push(Span::raw(" "));
            }
        }
        
        disk_lines.push(Line::from(spans));
    }
    
    if disk_lines.is_empty() {
        disk_lines.push(Line::from(Span::styled("No disks detected", Style::default().fg(Color::DarkGray))));
    }
    
    f.render_widget(Paragraph::new(disk_lines), text_chunks[1]);

    // Sparklines for disk history at bottom (sized to graph width)
    if let Some(hist) = history {
        if !hist.read_history.is_empty() {
            // Read sparkline (cyan)
            let read_data = slice_for_width(&hist.read_history, graph_chunks[0]);
            let read_max = read_data.iter().max().copied().unwrap_or(1).max(1);
            let read_title = format!(" Read max:{} ", format_throughput(read_max as f64 * 1024.0));
            let read_sparkline = Sparkline::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(read_title))
                .data(read_data)
                .max(read_max)
                .style(Style::default().fg(Color::Cyan));
            f.render_widget(read_sparkline, graph_chunks[0]);

            // Write sparkline (yellow)
            let write_data = slice_for_width(&hist.write_history, graph_chunks[1]);
            let write_max = write_data.iter().max().copied().unwrap_or(1).max(1);
            let write_title = format!(" Write max:{} ", format_throughput(write_max as f64 * 1024.0));
            let write_sparkline = Sparkline::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(write_title))
                .data(write_data)
                .max(write_max)
                .style(Style::default().fg(Color::Yellow));
            f.render_widget(write_sparkline, graph_chunks[1]);
        }
    }
}

/// Format throughput as a fixed-width short string (4 chars, e.g., "  0 ", "12K ", " 1M ")
fn format_throughput_short(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        "   0".to_string()
    } else if bytes_per_sec < 1024.0 {
        format!("{:>3}B", bytes_per_sec as u64)
    } else if bytes_per_sec < 1024.0 * 1024.0 {
        format!("{:>3}K", (bytes_per_sec / 1024.0) as u64)
    } else if bytes_per_sec < 1024.0 * 1024.0 * 1024.0 {
        format!("{:>3}M", (bytes_per_sec / (1024.0 * 1024.0)) as u64)
    } else {
        format!("{:>3}G", (bytes_per_sec / (1024.0 * 1024.0 * 1024.0)) as u64)
    }
}

/// CPU history for sparkline display
pub struct CpuHistory {
    pub utilization: Vec<u64>,  // CPU % history (0-100)
    pub max_samples: usize,
}

impl CpuHistory {
    pub fn new(max_samples: usize) -> Self {
        Self {
            utilization: Vec::with_capacity(max_samples),
            max_samples,
        }
    }

    pub fn push(&mut self, cpu_percent: f64) {
        if self.utilization.len() >= self.max_samples {
            self.utilization.remove(0);
        }
        self.utilization.push(cpu_percent as u64);
    }
}

impl Default for CpuHistory {
    fn default() -> Self {
        Self::new(500)  // Large buffer, display will use graph width
    }
}

/// Memory history for sparkline display
pub struct MemoryHistory {
    pub used_percent: Vec<u64>,    // System memory % history
    pub cgroup_percent: Vec<u64>,  // Cgroup memory % history (if available)
    pub max_samples: usize,
}

impl MemoryHistory {
    pub fn new(max_samples: usize) -> Self {
        Self {
            used_percent: Vec::with_capacity(max_samples),
            cgroup_percent: Vec::with_capacity(max_samples),
            max_samples,
        }
    }

    pub fn push(&mut self, used_pct: f64, cgroup_pct: Option<f64>) {
        if self.used_percent.len() >= self.max_samples {
            self.used_percent.remove(0);
            self.cgroup_percent.remove(0);
        }
        self.used_percent.push(used_pct as u64);
        self.cgroup_percent.push(cgroup_pct.unwrap_or(0.0) as u64);
    }
}

impl Default for MemoryHistory {
    fn default() -> Self {
        Self::new(500)  // Large buffer, display will use graph width
    }
}

/// Disk history for sparkline display
pub struct DiskHistory {
    pub read_history: Vec<u64>,   // Read KB/s history
    pub write_history: Vec<u64>,  // Write KB/s history
    pub max_samples: usize,
}

impl DiskHistory {
    pub fn new(max_samples: usize) -> Self {
        Self {
            read_history: Vec::with_capacity(max_samples),
            write_history: Vec::with_capacity(max_samples),
            max_samples,
        }
    }

    pub fn push(&mut self, read_bytes_per_sec: f64, write_bytes_per_sec: f64) {
        let read_kb = (read_bytes_per_sec / 1024.0).max(0.0) as u64;
        let write_kb = (write_bytes_per_sec / 1024.0).max(0.0) as u64;
        
        if self.read_history.len() >= self.max_samples {
            self.read_history.remove(0);
            self.write_history.remove(0);
        }
        self.read_history.push(read_kb);
        self.write_history.push(write_kb);
    }
}

impl Default for DiskHistory {
    fn default() -> Self {
        Self::new(500)  // Large buffer, display will use graph width
    }
}

/// Network history for sparkline display
pub struct NetworkHistory {
    pub rx_history: Vec<u64>,  // RX KB/s history
    pub tx_history: Vec<u64>,  // TX KB/s history
    pub max_samples: usize,
}

impl NetworkHistory {
    pub fn new(max_samples: usize) -> Self {
        Self {
            rx_history: Vec::with_capacity(max_samples),
            tx_history: Vec::with_capacity(max_samples),
            max_samples,
        }
    }

    pub fn push(&mut self, rx_bytes_per_sec: f64, tx_bytes_per_sec: f64) {
        let rx_kb = (rx_bytes_per_sec / 1024.0).max(0.0) as u64;
        let tx_kb = (tx_bytes_per_sec / 1024.0).max(0.0) as u64;
        
        if self.rx_history.len() >= self.max_samples {
            self.rx_history.remove(0);
            self.tx_history.remove(0);
        }
        self.rx_history.push(rx_kb);
        self.tx_history.push(tx_kb);
    }
}

impl Default for NetworkHistory {
    fn default() -> Self {
        Self::new(500)  // Large buffer, display will use graph width
    }
}

/// Render network metrics widget with sparkline graphs
pub fn render_network(f: &mut Frame, area: Rect, net: &NetworkMetrics, history: Option<&NetworkHistory>) {
    let block = Block::default()
        .title(" Network I/O ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Layout: text at top, sparklines fill remaining space at bottom
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Text info (fixed height)
            Constraint::Min(6),     // Sparklines area (fills remaining)
        ])
        .split(inner);
    
    // Split sparklines into RX (top) and TX (bottom)
    let graph_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(main_chunks[1]);

    // Text information with colored RX/TX values, vertically aligned
    // Use 6-char width for all values to align columns
    let rx_str = format_throughput_short(net.total_rx_bytes_per_sec);
    let tx_str = format_throughput_short(net.total_tx_bytes_per_sec);
    
    let mut lines = vec![
        Line::from(vec![
            Span::raw("Total: RX "),
            Span::styled(format!("{:>6}", rx_str), Style::default().fg(Color::Cyan)),
            Span::raw(" TX "),
            Span::styled(format!("{:>6}", tx_str), Style::default().fg(Color::Green)),
        ]),
    ];

    // Show first interface details if available
    if let Some(iface) = net.interfaces.first() {
        let rx_pkt = format!("{:>6.0}", iface.rx_packets_per_sec);
        let tx_pkt = format!("{:>6.0}", iface.tx_packets_per_sec);
        // Format: "iface: RX xxxxxx TX xxxxxx" - align "RX" with Total line
        let iface_short: String = iface.interface.chars().take(5).collect();
        lines.push(Line::from(vec![
            Span::raw(format!("{:>5}: RX ", iface_short)),
            Span::styled(rx_pkt, Style::default().fg(Color::Cyan)),
            Span::raw(" TX "),
            Span::styled(tx_pkt, Style::default().fg(Color::Green)),
            Span::raw(" pkt/s"),
        ]));
        lines.push(Line::from(vec![
            Span::raw(format!("TCP: {} Retx: {}", 
                net.tcp.connections_established,
                net.tcp.retransmits_delta.unwrap_or(0)
            )),
            if iface.rx_errors > 0 || iface.tx_errors > 0 || iface.rx_drops > 0 || iface.tx_drops > 0 {
                Span::styled(
                    format!("  Err: {}/{} Drop: {}/{}",
                        iface.rx_errors, iface.tx_errors, iface.rx_drops, iface.tx_drops
                    ),
                    Style::default().fg(Color::Red),
                )
            } else {
                Span::raw("")
            },
        ]));
    }

    f.render_widget(Paragraph::new(lines), main_chunks[0]);

    // Sparklines for network history (sized to graph width)
    if let Some(hist) = history {
        // RX bytes sparkline (cyan)
        if !hist.rx_history.is_empty() {
            let rx_data = slice_for_width(&hist.rx_history, graph_chunks[0]);
            let rx_max = rx_data.iter().max().copied().unwrap_or(1).max(1);
            let rx_title = format!(" RX ▼ max:{} ", format_throughput(rx_max as f64 * 1024.0));
            let rx_sparkline = Sparkline::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(rx_title))
                .data(rx_data)
                .max(rx_max)
                .style(Style::default().fg(Color::Cyan));
            f.render_widget(rx_sparkline, graph_chunks[0]);
        }

        // TX bytes sparkline (green)
        if !hist.tx_history.is_empty() {
            let tx_data = slice_for_width(&hist.tx_history, graph_chunks[1]);
            let tx_max = tx_data.iter().max().copied().unwrap_or(1).max(1);
            let tx_title = format!(" TX ▲ max:{} ", format_throughput(tx_max as f64 * 1024.0));
            let tx_sparkline = Sparkline::default()
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(tx_title))
                .data(tx_data)
                .max(tx_max)
                .style(Style::default().fg(Color::Green));
            f.render_widget(tx_sparkline, graph_chunks[1]);
        }
    }
}

/// Render process metrics widget
pub fn render_process(f: &mut Frame, area: Rect, proc: Option<&ProcessMetrics>) {
    let block = Block::default()
        .title(" Process ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(p) = proc {
        let state_color = match p.state {
            crate::process::ProcessState::Running => Color::Green,
            crate::process::ProcessState::DiskSleep => Color::Yellow,
            crate::process::ProcessState::Zombie => Color::Red,
            _ => Color::White,
        };

        let lines = vec![
            Line::from(format!("PID: {}  Name: {}", p.pid, p.name)),
            Line::from(vec![
                Span::raw("State: "),
                Span::styled(format!("{}", p.state), Style::default().fg(state_color)),
            ]),
            Line::from(format!(
                "CPU: {:.1}%  Threads: {}  FDs: {}",
                p.cpu_percent, p.num_threads, p.num_fds
            )),
            Line::from(format!(
                "RSS: {}  VSZ: {}",
                format_bytes(p.rss_bytes),
                format_bytes(p.vsize_bytes)
            )),
            Line::from(""),
            Line::from(format!(
                "Cmd: {}",
                if p.cmdline.len() > 60 {
                    format!("{}...", &p.cmdline[..57])
                } else {
                    p.cmdline.clone()
                }
            )),
        ];
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        let text = Paragraph::new("No process being monitored");
        f.render_widget(text, inner);
    }
}

/// Render alerts widget
pub fn render_alerts(f: &mut Frame, area: Rect, alerts: &[Alert]) {
    let block = Block::default()
        .title(" Alerts ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if alerts.is_empty() {
        let text = Paragraph::new(Span::styled(
            "No active alerts",
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(text, inner);
        return;
    }

    let items: Vec<ListItem> = alerts
        .iter()
        .take(5) // Show only last 5 alerts
        .map(|alert| {
            let style = match alert.severity {
                crate::alert::Severity::Warning => Style::default().fg(Color::Yellow),
                crate::alert::Severity::Critical => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            };
            ListItem::new(Span::styled(&alert.message, style))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render system info widget
pub fn render_system_info(f: &mut Frame, area: Rect, uptime_secs: u64) {
    let block = Block::default()
        .title(" System ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Gray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let hours = uptime_secs / 3600;
    let mins = (uptime_secs % 3600) / 60;
    let secs = uptime_secs % 60;

    let uptime_str = if hours > 24 {
        let days = hours / 24;
        format!("{}d {}h {}m", days, hours % 24, mins)
    } else {
        format!("{}h {}m {}s", hours, mins, secs)
    };

    let lines = vec![
        Line::from(format!("Uptime: {}", uptime_str)),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// Render help bar at the bottom
pub fn render_help_bar(f: &mut Frame, area: Rect, pending_split: bool, status: Option<&str>, current_log: Option<&str>) {
    let (text, style) = if pending_split {
        (
            " Split logs? Press Y to confirm, any other key to cancel ".to_string(),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )
    } else if let Some(msg) = status {
        (
            format!(" {} ", msg),
            Style::default().fg(Color::White).bg(Color::Blue),
        )
    } else {
        let log_info = current_log
            .map(|name| format!(" ({})", name))
            .unwrap_or_default();
        (
            format!(" q: Quit | p: Toggle process | l: Toggle logging | r: Reset | s: Split logs{} ", log_info),
            Style::default().fg(Color::Black).bg(Color::Gray),
        )
    };
    
    let paragraph = Paragraph::new(text).style(style);
    f.render_widget(paragraph, area);
}
