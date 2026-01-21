//! Performance Monitor - A system monitoring CLI for identifying bottlenecks.
//!
//! Monitors CPU, memory, disk I/O, network, and process-specific metrics
//! with real-time TUI display, historical logging, and alerting.

mod alert;
mod display;
mod logging;
mod metrics;
mod plot;
mod process;

use alert::{AlertChecker, AlertThresholds};
use anyhow::Result;
use chrono::Utc;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use display::{format_bytes, format_throughput, CpuHistory, DiskHistory, MemoryHistory, NetworkHistory};
use logging::{CsvLogger, MetricsSample, SummaryAccumulator, TextLogger};
use metrics::{CpuMetrics, DiskMetrics, MemoryMetrics, NetworkMetrics};
use process::{ProcessCollector, ProcessMetrics};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    prelude::CrosstermBackend,
    Terminal,
};
use std::io;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::time::Duration;

/// Performance monitoring CLI for identifying system bottlenecks
#[derive(Parser, Debug)]
#[command(name = "monperf")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Process ID to monitor (optional)
    #[arg(short, long)]
    pid: Option<u32>,

    /// Process name to monitor (finds PID automatically)
    #[arg(short = 'n', long)]
    process_name: Option<String>,

    /// Sampling interval in seconds
    #[arg(short = 'i', long, default_value = "1")]
    interval: f64,

    /// Log all metrics to CSV file (canonical detailed format)
    #[arg(short, long)]
    log: Option<PathBuf>,

    /// Log metrics to file (human-readable summary format)
    #[arg(short = 'o', long)]
    text_log: Option<PathBuf>,

    /// Spill directory to monitor for size
    #[arg(short, long)]
    spill_dir: Option<PathBuf>,

    /// Run for specified duration (seconds), then exit with summary
    #[arg(short, long)]
    duration: Option<u64>,

    /// Disable TUI and output metrics to stdout
    #[arg(long)]
    no_tui: bool,

    /// Generate summary report at end
    #[arg(long)]
    summary: bool,

    /// CPU warning threshold (%)
    #[arg(long, default_value = "80")]
    cpu_warn: f64,

    /// CPU critical threshold (%)
    #[arg(long, default_value = "95")]
    cpu_crit: f64,

    /// Memory warning threshold (%)
    #[arg(long, default_value = "80")]
    mem_warn: f64,

    /// Memory critical threshold (%)
    #[arg(long, default_value = "95")]
    mem_crit: f64,

    /// Cgroup memory warning threshold (%)
    #[arg(long, default_value = "85")]
    cgroup_warn: f64,

    /// Cgroup memory critical threshold (%)
    #[arg(long, default_value = "95")]
    cgroup_crit: f64,

    /// Generate plots from a CSV log file (use with --plot-output)
    #[arg(long)]
    plot: Option<PathBuf>,

    /// Output directory for generated plots (default: ./plots)
    #[arg(long, default_value = "plots")]
    plot_output: PathBuf,

    /// Automatically split logs when monitored process starts or ends
    #[arg(long)]
    split_on_process: bool,

    /// UDP port to listen for control messages (split logs on message, rename if filename provided)
    #[arg(long)]
    control_port: Option<u16>,
}

/// Application state
struct App {
    cpu_collector: metrics::cpu::CpuCollector,
    mem_collector: metrics::memory::MemoryCollector,
    disk_collector: metrics::disk::DiskCollector,
    net_collector: metrics::network::NetworkCollector,
    psi_collector: metrics::psi::PsiCollector,
    proc_collector: Option<ProcessCollector>,

    cpu_metrics: Option<CpuMetrics>,
    mem_metrics: Option<MemoryMetrics>,
    disk_metrics: Option<DiskMetrics>,
    net_metrics: Option<NetworkMetrics>,
    psi_metrics: Option<metrics::PsiMetrics>,
    proc_metrics: Option<ProcessMetrics>,

    alert_checker: AlertChecker,
    alerts: Vec<alert::Alert>,

    csv_logger: Option<CsvLogger>,
    text_logger: Option<TextLogger>,
    accumulator: SummaryAccumulator,

    uptime_secs: u64,
    samples_collected: u64,
    show_process: bool,
    logging_enabled: bool,

    // Process discovery settings
    process_name_pattern: Option<String>,
    process_rescan_interval: u64,  // Rescan every N samples
    current_monitored_pid: Option<u32>,

    // Log rotation settings
    csv_log_base: Option<PathBuf>,
    text_log_base: Option<PathBuf>,
    log_segment: u32,
    pending_log_split: bool,  // Confirmation state for log split
    status_message: Option<(String, std::time::Instant)>,  // Temporary status message
    tui_mode: bool,  // Whether running in TUI mode (suppress eprintln)

    // Auto-split on process state change
    split_on_process: bool,
    prev_process_running: bool,

    // History for sparkline graphs
    cpu_history: CpuHistory,
    memory_history: MemoryHistory,
    disk_history: DiskHistory,
    network_history: NetworkHistory,

    // Control socket for external log split commands
    control_socket: Option<UdpSocket>,
}

impl App {
    fn new(args: &Args) -> Result<Self> {
        // Determine process to monitor
        let (proc_collector, current_pid, pattern) = if let Some(pid) = args.pid {
            // Explicit PID - no pattern matching needed
            (Some(ProcessCollector::new(pid)), Some(pid), None)
        } else if let Some(ref name) = args.process_name {
            // Pattern matching - will be rescanned periodically
            if let Some(pid) = process::find_process_by_name(name) {
                eprintln!("Found process '{}' with PID {}", name, pid);
                (Some(ProcessCollector::new(pid)), Some(pid), Some(name.clone()))
            } else {
                eprintln!("Process '{}' not found yet, will keep searching...", name);
                (None, None, Some(name.clone()))
            }
        } else {
            (None, None, None)
        };

        // Setup disk collector with spill dir
        let mut disk_collector = metrics::disk::DiskCollector::new();
        if let Some(ref spill_dir) = args.spill_dir {
            disk_collector.set_spill_dir(&spill_dir.to_string_lossy());
        }

        // Setup CSV logger (canonical detailed format)
        let csv_logger = if let Some(ref log_path) = args.log {
            Some(CsvLogger::new(log_path)?)
        } else {
            None
        };

        // Setup text logger (human-readable summary)
        let text_logger = if let Some(ref log_path) = args.text_log {
            Some(TextLogger::new(log_path)?)
        } else {
            None
        };

        // Setup alert thresholds
        let thresholds = AlertThresholds {
            cpu_warn: args.cpu_warn,
            cpu_crit: args.cpu_crit,
            memory_warn: args.mem_warn,
            memory_crit: args.mem_crit,
            cgroup_warn: args.cgroup_warn,
            cgroup_crit: args.cgroup_crit,
            ..Default::default()
        };

        // Determine initial process running state
        let initial_process_running = proc_collector.is_some();

        // Setup control socket if port specified
        let control_socket = if let Some(port) = args.control_port {
            match UdpSocket::bind(format!("127.0.0.1:{}", port)) {
                Ok(socket) => {
                    // Set non-blocking so we don't block the main loop
                    socket.set_nonblocking(true)?;
                    eprintln!("Control socket listening on UDP port {}", port);
                    Some(socket)
                }
                Err(e) => {
                    eprintln!("Warning: Failed to bind control socket on port {}: {}", port, e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            cpu_collector: metrics::cpu::CpuCollector::new(),
            mem_collector: metrics::memory::MemoryCollector::new(),
            disk_collector,
            net_collector: metrics::network::NetworkCollector::new(),
            psi_collector: metrics::psi::PsiCollector::new(),
            proc_collector,
            cpu_metrics: None,
            mem_metrics: None,
            disk_metrics: None,
            net_metrics: None,
            psi_metrics: None,
            proc_metrics: None,
            alert_checker: AlertChecker::new(thresholds),
            alerts: Vec::new(),
            csv_logger,
            text_logger,
            accumulator: SummaryAccumulator::new(),
            uptime_secs: 0,
            samples_collected: 0,
            show_process: true,
            logging_enabled: true,
            process_name_pattern: pattern,
            process_rescan_interval: 10, // Rescan for process every 10 samples
            current_monitored_pid: current_pid,
            csv_log_base: args.log.clone(),
            text_log_base: args.text_log.clone(),
            log_segment: 0,
            pending_log_split: false,
            status_message: None,
            tui_mode: false,  // Set by run_tui
            split_on_process: args.split_on_process,
            prev_process_running: initial_process_running,
            cpu_history: CpuHistory::default(),
            memory_history: MemoryHistory::default(),
            disk_history: DiskHistory::default(),
            network_history: NetworkHistory::default(),
            control_socket,
        })
    }

    /// Rescan for matching process if using pattern matching
    fn refresh_process_collector(&mut self) {
        // Only rescan if we have a pattern (not explicit PID)
        let pattern = match &self.process_name_pattern {
            Some(p) => p.clone(),
            None => return,
        };

        // Check if current process still exists
        let current_exists = self.proc_collector
            .as_ref()
            .map(|p| p.exists())
            .unwrap_or(false);

        if current_exists {
            // Current process still running, no need to rescan
            return;
        }

        // Try to find a new matching process
        if let Some(pid) = process::find_process_by_name(&pattern) {
            // Found a (potentially new) process
            if self.current_monitored_pid != Some(pid) {
                let msg = format!("Found process '{}' with PID {}", pattern, pid);
                if self.tui_mode {
                    self.status_message = Some((msg, std::time::Instant::now()));
                } else {
                    eprintln!("{}", msg);
                }
                self.proc_collector = Some(ProcessCollector::new(pid));
                self.current_monitored_pid = Some(pid);
            }
        } else if self.current_monitored_pid.is_some() {
            // Process disappeared
            let msg = format!("Process '{}' ended, searching...", pattern);
            if self.tui_mode {
                self.status_message = Some((msg, std::time::Instant::now()));
            } else {
                eprintln!("Process '{}' (PID {:?}) ended, searching for new instance...", 
                         pattern, self.current_monitored_pid);
            }
            self.proc_collector = None;
            self.proc_metrics = None;
            self.current_monitored_pid = None;
        }
    }

    fn collect_metrics(&mut self) -> Result<()> {
        // Periodically rescan for matching process (every N samples)
        if self.process_name_pattern.is_some() 
            && (self.samples_collected == 0 
                || self.samples_collected % self.process_rescan_interval == 0
                || self.proc_collector.is_none()) 
        {
            self.refresh_process_collector();
        }

        self.cpu_metrics = Some(self.cpu_collector.collect()?);
        self.mem_metrics = Some(self.mem_collector.collect()?);
        self.disk_metrics = Some(self.disk_collector.collect()?);
        self.net_metrics = Some(self.net_collector.collect()?);
        self.psi_metrics = self.psi_collector.collect().ok();

        // Update history for sparklines
        if let Some(ref cpu) = self.cpu_metrics {
            self.cpu_history.push(cpu.total_utilization);
        }
        if let Some(ref mem) = self.mem_metrics {
            self.memory_history.push(mem.used_percent, mem.cgroup_usage_percent);
        }
        if let Some(ref disk) = self.disk_metrics {
            self.disk_history.push(disk.total_read_bytes_per_sec, disk.total_write_bytes_per_sec);
        }
        if let Some(ref net) = self.net_metrics {
            self.network_history.push(net.total_rx_bytes_per_sec, net.total_tx_bytes_per_sec);
        }

        if let Some(ref mut proc) = self.proc_collector {
            if proc.exists() {
                self.proc_metrics = proc.collect().ok();
            } else {
                // Process ended, trigger rescan on next sample
                self.proc_metrics = None;
                self.proc_collector = None;
                self.current_monitored_pid = None;
            }
        }

        // Check for process state change and auto-split logs if enabled
        let current_process_running = self.proc_collector.is_some() && self.proc_metrics.is_some();
        if self.split_on_process && self.samples_collected > 0 {
            if current_process_running != self.prev_process_running {
                // Process state changed - split logs
                let event = if current_process_running {
                    "process started"
                } else {
                    "process ended"
                };
                
                // Only split if logging is configured
                if self.csv_log_base.is_some() || self.text_log_base.is_some() {
                    if let Err(e) = self.rotate_logs() {
                        let msg = format!("Auto-split failed on {}: {}", event, e);
                        if self.tui_mode {
                            self.set_status(&msg);
                        } else {
                            eprintln!("{}", msg);
                        }
                    } else {
                        let msg = format!("Logs split on {} → segment {}", event, self.log_segment);
                        if self.tui_mode {
                            self.set_status(&msg);
                        } else {
                            eprintln!("{}", msg);
                        }
                    }
                }
            }
        }
        self.prev_process_running = current_process_running;

        self.samples_collected += 1;

        // Check alerts
        if let (Some(cpu), Some(mem), Some(disk), Some(net)) = (
            &self.cpu_metrics,
            &self.mem_metrics,
            &self.disk_metrics,
            &self.net_metrics,
        ) {
            let new_alerts = self
                .alert_checker
                .check(cpu, mem, disk, net, self.proc_metrics.as_ref());

            for alert in new_alerts {
                self.alerts.push(alert);
            }

            // Keep only last 20 alerts
            if self.alerts.len() > 20 {
                self.alerts.drain(0..self.alerts.len() - 20);
            }

            // Log and accumulate
            let sample = MetricsSample {
                timestamp: Utc::now(),
                cpu: cpu.clone(),
                memory: mem.clone(),
                disk: disk.clone(),
                network: net.clone(),
                process: self.proc_metrics.clone(),
                psi: self.psi_metrics.clone(),
            };

            if self.logging_enabled {
                if let Some(ref mut csv_logger) = self.csv_logger {
                    if let Err(e) = csv_logger.log(&sample) {
                        if self.tui_mode {
                            self.set_status(&format!("CSV log error: {}", e));
                        } else {
                            eprintln!("CSV log error: {}", e);
                        }
                    }
                }
                if let Some(ref mut text_logger) = self.text_logger {
                    if let Err(e) = text_logger.log(&sample) {
                        if self.tui_mode {
                            self.set_status(&format!("Text log error: {}", e));
                        } else {
                            eprintln!("Text log error: {}", e);
                        }
                    }
                }
            }

            self.accumulator.add_sample(sample);
        }

        Ok(())
    }

    /// Rotate log files to start a new segment
    fn rotate_logs(&mut self) -> Result<()> {
        self.log_segment += 1;
        let segment = self.log_segment;
        
        // Rotate CSV log (canonical format)
        if let Some(ref base_path) = self.csv_log_base {
            let new_path = Self::segment_path(base_path, segment);
            self.csv_logger = Some(CsvLogger::new(&new_path)?);
            if !self.tui_mode {
                eprintln!("Started new CSV log: {}", new_path.display());
            }
        }
        
        // Rotate text log (human-readable summary)
        if let Some(ref base_path) = self.text_log_base {
            let new_path = Self::segment_path(base_path, segment);
            self.text_logger = Some(TextLogger::new(&new_path)?);
            if !self.tui_mode {
                eprintln!("Started new text log: {}", new_path.display());
            }
        }
        
        // Reset accumulator for new segment
        self.accumulator.clear();
        
        Ok(())
    }

    /// Generate a segmented path from base path
    fn segment_path(base: &PathBuf, segment: u32) -> PathBuf {
        let stem = base.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("log");
        let ext = base.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        
        let new_name = if ext.is_empty() {
            format!("{}_{}", stem, segment)
        } else {
            format!("{}_{}.{}", stem, segment, ext)
        };
        
        base.with_file_name(new_name)
    }

    /// Set a temporary status message
    fn set_status(&mut self, msg: &str) {
        self.status_message = Some((msg.to_string(), std::time::Instant::now()));
    }

    /// Get current status message if not expired (3 seconds)
    fn get_status(&self) -> Option<&str> {
        self.status_message.as_ref().and_then(|(msg, time)| {
            if time.elapsed().as_secs() < 3 {
                Some(msg.as_str())
            } else {
                None
            }
        })
    }

    /// Get current log file name for display
    fn current_log_name(&self) -> Option<String> {
        // Prefer CSV log name (canonical), then text log
        let base = self.csv_log_base.as_ref()
            .or(self.text_log_base.as_ref())?;
        
        let path = if self.log_segment == 0 {
            base.clone()
        } else {
            Self::segment_path(base, self.log_segment)
        };
        
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    }

    /// Check for control messages on the UDP socket
    /// Returns Some(filename) if a split was requested with a rename, None otherwise
    fn check_control_messages(&mut self) -> Option<String> {
        let socket = self.control_socket.as_ref()?;
        
        let mut buf = [0u8; 1024];
        match socket.recv_from(&mut buf) {
            Ok((len, addr)) => {
                let msg = String::from_utf8_lossy(&buf[..len]).trim().to_string();
                
                if !self.tui_mode {
                    eprintln!("Control message from {}: '{}'", addr, msg);
                }
                
                // Message can be:
                // - Empty or "split" -> split logs, no rename
                // - Filename -> split logs and rename current segment to this name
                if msg.is_empty() || msg.eq_ignore_ascii_case("split") {
                    Some(String::new())  // Empty string signals split without rename
                } else {
                    Some(msg)  // Filename to rename to
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No message available (non-blocking)
                None
            }
            Err(e) => {
                if !self.tui_mode {
                    eprintln!("Control socket error: {}", e);
                }
                None
            }
        }
    }

    /// Rename the current log segment to a custom name
    fn rename_current_segment(&mut self, new_name: &str) -> Result<()> {
        // Get current paths
        let csv_current = self.csv_log_base.as_ref().map(|base| {
            if self.log_segment == 0 {
                base.clone()
            } else {
                Self::segment_path(base, self.log_segment)
            }
        });
        
        let text_current = self.text_log_base.as_ref().map(|base| {
            if self.log_segment == 0 {
                base.clone()
            } else {
                Self::segment_path(base, self.log_segment)
            }
        });
        
        // Close current loggers first
        self.csv_logger = None;
        self.text_logger = None;
        
        // Rename CSV log
        if let Some(current) = csv_current {
            if current.exists() {
                let dir = current.parent().unwrap_or_else(|| std::path::Path::new("."));
                let new_path = dir.join(format!("{}.csv", new_name));
                if let Err(e) = std::fs::rename(&current, &new_path) {
                    if !self.tui_mode {
                        eprintln!("Failed to rename CSV log to {}: {}", new_path.display(), e);
                    }
                } else if !self.tui_mode {
                    eprintln!("Renamed CSV log to: {}", new_path.display());
                }
            }
        }
        
        // Rename text log
        if let Some(current) = text_current {
            if current.exists() {
                let dir = current.parent().unwrap_or_else(|| std::path::Path::new("."));
                let new_path = dir.join(format!("{}.txt", new_name));
                if let Err(e) = std::fs::rename(&current, &new_path) {
                    if !self.tui_mode {
                        eprintln!("Failed to rename text log to {}: {}", new_path.display(), e);
                    }
                } else if !self.tui_mode {
                    eprintln!("Renamed text log to: {}", new_path.display());
                }
            }
        }
        
        Ok(())
    }

    fn print_metrics(&self) {
        if let (Some(cpu), Some(mem), Some(disk), Some(net)) = (
            &self.cpu_metrics,
            &self.mem_metrics,
            &self.disk_metrics,
            &self.net_metrics,
        ) {
            println!("\n--- Sample {} ---", self.samples_collected);
            println!(
                "CPU: {:.1}% (user:{:.1}% sys:{:.1}% iowait:{:.1}%) Load: {:.2} {:.2} {:.2}",
                cpu.total_utilization,
                cpu.user_percent,
                cpu.system_percent,
                cpu.iowait_percent,
                cpu.load_avg.0,
                cpu.load_avg.1,
                cpu.load_avg.2
            );
            println!(
                "Memory: {} / {} ({:.1}%) Swap: {} / {}",
                format_bytes(mem.used),
                format_bytes(mem.total),
                mem.used_percent,
                format_bytes(mem.swap_used),
                format_bytes(mem.swap_total)
            );
            if let Some(pct) = mem.cgroup_usage_percent {
                println!(
                    "Cgroup: {} / {} ({:.1}%)",
                    format_bytes(mem.cgroup_current.unwrap_or(0)),
                    format_bytes(mem.cgroup_limit.unwrap_or(0)),
                    pct
                );
            }
            println!(
                "Disk: R {} W {}",
                format_throughput(disk.total_read_bytes_per_sec),
                format_throughput(disk.total_write_bytes_per_sec)
            );
            println!(
                "Network: RX {} TX {}",
                format_throughput(net.total_rx_bytes_per_sec),
                format_throughput(net.total_tx_bytes_per_sec)
            );

            if let Some(proc) = &self.proc_metrics {
                println!(
                    "Process [{}]: CPU:{:.1}% RSS:{} Threads:{} FDs:{}",
                    proc.name,
                    proc.cpu_percent,
                    format_bytes(proc.rss_bytes),
                    proc.num_threads,
                    proc.num_fds
                );
            }

            // Print any new alerts
            for alert in self.alerts.iter().rev().take(3) {
                let prefix = match alert.severity {
                    alert::Severity::Warning => "⚠️  WARNING",
                    alert::Severity::Critical => "🚨 CRITICAL",
                };
                println!("{}: {}", prefix, alert.message);
            }
        }
    }

    fn print_summary(&self) {
        if let Some(summary) = self.accumulator.generate_summary() {
            println!("\n{}", "=".repeat(60));
            println!("                    PERFORMANCE SUMMARY");
            println!("{}", "=".repeat(60));
            println!("Duration: {:.1}s  Samples: {}", summary.duration_secs, summary.samples_count);
            println!();
            println!("CPU:");
            println!(
                "  Utilization: avg {:.1}%, max {:.1}%",
                summary.cpu_avg_utilization, summary.cpu_max_utilization
            );
            println!(
                "  IOWait: avg {:.1}%, max {:.1}%",
                summary.cpu_avg_iowait, summary.cpu_max_iowait
            );
            println!();
            println!("Memory:");
            println!(
                "  Usage: avg {:.1}%, max {:.1}% ({})",
                summary.memory_avg_used_percent,
                summary.memory_max_used_percent,
                format_bytes(summary.memory_max_used_bytes)
            );
            if let Some(cgroup_max) = summary.cgroup_max_usage_percent {
                println!("  Cgroup max: {:.1}%", cgroup_max);
            }
            if summary.swap_max_used > 0 {
                println!("  Swap max: {}", format_bytes(summary.swap_max_used));
            }
            println!();
            println!("Disk I/O:");
            println!(
                "  Max read throughput: {}",
                format_throughput(summary.disk_max_read_throughput)
            );
            println!(
                "  Max write throughput: {}",
                format_throughput(summary.disk_max_write_throughput)
            );
            println!("  Max utilization: {:.1}%", summary.disk_max_utilization);
            println!();
            println!("Network:");
            println!("  Total RX: {}", format_bytes(summary.network_total_rx_bytes));
            println!("  Total TX: {}", format_bytes(summary.network_total_tx_bytes));
            println!(
                "  Max RX throughput: {}",
                format_throughput(summary.network_max_rx_throughput)
            );
            println!(
                "  Max TX throughput: {}",
                format_throughput(summary.network_max_tx_throughput)
            );

            if let Some(proc_cpu) = summary.process_max_cpu {
                println!();
                println!("Process:");
                println!("  Max CPU: {:.1}%", proc_cpu);
                if let Some(rss) = summary.process_max_rss {
                    println!("  Max RSS: {}", format_bytes(rss));
                }
                if let Some(fds) = summary.process_max_fds {
                    println!("  Max FDs: {}", fds);
                }
            }

            if !summary.bottleneck_indicators.is_empty() {
                println!();
                println!("Bottleneck Analysis:");
                for indicator in &summary.bottleneck_indicators {
                    println!("  • {}", indicator);
                }
            }
            println!("{}", "=".repeat(60));
        }
    }
}

fn run_tui(mut app: App, interval: Duration, duration: Option<Duration>) -> Result<App> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Enable TUI mode to suppress eprintln messages
    app.tui_mode = true;

    let start_time = std::time::Instant::now();
    let tick_rate = interval;
    let mut last_tick = std::time::Instant::now();

    // Initial collection to populate metrics
    app.collect_metrics()?;

    loop {
        // Check duration limit
        if let Some(dur) = duration {
            if start_time.elapsed() >= dur {
                break;
            }
        }

        // Draw UI
        terminal.draw(|f| {
            // First split off the fixed-height bottom sections
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(10),      // Main area (CPU + Memory + Disk + Network)
                    Constraint::Length(5),    // Bottom row (Process only) - compact
                    Constraint::Length(1),    // Help bar
                ])
                .split(f.area());
            
            // Split the main area into top and middle rows (each gets half)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(50),  // Top row (CPU + Memory)
                    Constraint::Percentage(50),  // Middle row (Disk + Network)
                ])
                .split(main_chunks[0]);

            // Top row: CPU and Memory
            let top_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[0]);

            if let Some(ref cpu) = app.cpu_metrics {
                display::render_cpu(f, top_chunks[0], cpu, Some(&app.cpu_history));
            }
            if let Some(ref mem) = app.mem_metrics {
                display::render_memory(f, top_chunks[1], mem, Some(&app.memory_history));
            }

            // Middle row: Disk and Network
            let mid_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);

            if let Some(ref disk) = app.disk_metrics {
                display::render_disk(f, mid_chunks[0], disk, Some(&app.disk_history));
            }
            if let Some(ref net) = app.net_metrics {
                display::render_network(f, mid_chunks[1], net, Some(&app.network_history));
            }

            // Bottom row: Process info only (no alerts)
            if app.show_process {
                display::render_process(f, main_chunks[1], app.proc_metrics.as_ref());
            } else {
                display::render_system_info(f, main_chunks[1], app.uptime_secs);
            }

            // Help bar with status and current log name
            let log_name = app.current_log_name();
            display::render_help_bar(f, main_chunks[2], app.pending_log_split, app.get_status(), log_name.as_deref());
        })?;

        // Handle input
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.pending_log_split {
                        // Confirmation mode for log split
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                app.pending_log_split = false;
                                if let Err(e) = app.rotate_logs() {
                                    app.set_status(&format!("Log split failed: {}", e));
                                } else {
                                    app.set_status(&format!("Logs split → segment {}", app.log_segment));
                                }
                            }
                            _ => {
                                app.pending_log_split = false;
                                app.set_status("Log split cancelled");
                            }
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Char('p') => app.show_process = !app.show_process,
                            KeyCode::Char('l') => app.logging_enabled = !app.logging_enabled,
                            KeyCode::Char('r') => {
                                app.alerts.clear();
                                app.accumulator.clear();
                            }
                            KeyCode::Char('s') => {
                                // Check if logging is configured
                                if app.csv_log_base.is_some() || app.text_log_base.is_some() {
                                    app.pending_log_split = true;
                                } else {
                                    app.set_status("No log files configured (-l, -o, or --detailed-log)");
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Check for control messages (log split requests)
        if let Some(rename_to) = app.check_control_messages() {
            // First rename the current segment if a name was provided
            if !rename_to.is_empty() {
                let _ = app.rename_current_segment(&rename_to);
            }
            // Then rotate to a new segment
            if app.csv_log_base.is_some() || app.text_log_base.is_some() {
                if let Err(e) = app.rotate_logs() {
                    app.set_status(&format!("Control split failed: {}", e));
                } else {
                    app.set_status("Log split via control port");
                }
            }
        }

        // Collect metrics on tick
        if last_tick.elapsed() >= tick_rate {
            app.collect_metrics()?;
            app.uptime_secs += interval.as_secs();
            last_tick = std::time::Instant::now();
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(app)
}

fn run_no_tui(mut app: App, interval: Duration, duration: Option<Duration>) -> Result<App> {
    let start_time = std::time::Instant::now();

    loop {
        // Check duration limit
        if let Some(dur) = duration {
            if start_time.elapsed() >= dur {
                break;
            }
        }

        app.collect_metrics()?;
        app.print_metrics();

        // Check for control messages (log split requests)
        if let Some(rename_to) = app.check_control_messages() {
            // First rename the current segment if a name was provided
            if !rename_to.is_empty() {
                let _ = app.rename_current_segment(&rename_to);
            }
            // Then rotate to a new segment
            if app.csv_log_base.is_some() || app.text_log_base.is_some() {
                if let Err(e) = app.rotate_logs() {
                    eprintln!("Control split failed: {}", e);
                } else {
                    eprintln!("Log split via control port");
                }
            }
        }

        std::thread::sleep(interval);
    }

    Ok(app)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Plot mode: generate plots from existing log file
    if let Some(ref log_path) = args.plot {
        eprintln!("Loading samples from: {}", log_path.display());
        eprintln!("Generating plots in: {}", args.plot_output.display());
        let generated = plot::generate_all_plots(log_path, &args.plot_output)?;
        
        eprintln!("\nGenerated {} plots:", generated.len());
        for path in generated {
            eprintln!("  • {}", path);
        }
        return Ok(());
    }

    // Normal monitoring mode
    let interval = Duration::from_secs_f64(args.interval);
    let duration = args.duration.map(Duration::from_secs);
    let summary = args.summary || args.duration.is_some();

    let app = App::new(&args)?;

    let result = if args.no_tui {
        run_no_tui(app, interval, duration)
    } else {
        run_tui(app, interval, duration)
    };

    // Handle cleanup and summary
    match result {
        Ok(app) => {
            if summary {
                app.print_summary();
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            return Err(e);
        }
    }

    // Log file messages
    if let Some(ref log_path) = args.log {
        eprintln!("CSV metrics logged to: {}", log_path.display());
    }
    if let Some(ref log_path) = args.text_log {
        eprintln!("Text summary logged to: {}", log_path.display());
    }

    Ok(())
}
