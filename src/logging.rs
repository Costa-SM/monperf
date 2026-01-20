//! Historical logging module for writing metrics to files.

use crate::display::{format_bytes, format_bytes_short, format_throughput, truncate_str};
use crate::metrics::{CpuMetrics, DiskMetrics, MemoryMetrics, NetworkMetrics, PsiMetrics};
use crate::process::ProcessMetrics;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

/// A single metrics sample with timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSample {
    pub timestamp: DateTime<Utc>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disk: DiskMetrics,
    pub network: NetworkMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psi: Option<PsiMetrics>,
}

/// Logger for writing metrics to JSON Lines file
pub struct MetricsLogger {
    writer: BufWriter<File>,
    samples_written: u64,
}

impl MetricsLogger {
    /// Create a new logger writing to the specified file
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path.as_ref())
            .context("Failed to create log file")?;

        Ok(Self {
            writer: BufWriter::new(file),
            samples_written: 0,
        })
    }

    /// Append a sample to the log file
    pub fn log(&mut self, sample: &MetricsSample) -> Result<()> {
        let json = serde_json::to_string(sample)?;
        writeln!(self.writer, "{}", json)?;
        self.samples_written += 1;

        // Flush every 10 samples to avoid losing data on crash
        if self.samples_written % 10 == 0 {
            self.writer.flush()?;
        }

        Ok(())
    }

    /// Flush any buffered data
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Get the number of samples written
    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }
}

impl Drop for MetricsLogger {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

/// Logger for writing human-readable text observations to a file
pub struct TextLogger {
    writer: BufWriter<File>,
    samples_written: u64,
}

impl TextLogger {
    /// Create a new text logger writing to the specified file
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path.as_ref())
            .context("Failed to create text log file")?;

        let mut logger = Self {
            writer: BufWriter::new(file),
            samples_written: 0,
        };

        // Write header - aligned with the extended output format
        writeln!(logger.writer, "# Performance Monitor Log")?;
        writeln!(logger.writer, "# Started: {}", Utc::now().format("%Y-%m-%d %H:%M:%S UTC"))?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, "# Column Definitions:")?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, "# CPU Section:")?;
        writeln!(logger.writer, "#   Time     - Sample timestamp (HH:MM:SS)")?;
        writeln!(logger.writer, "#   CPU%     - Total CPU utilization")?;
        writeln!(logger.writer, "#   IOW%     - CPU time waiting for I/O")?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, "# Memory Section:")?;
        writeln!(logger.writer, "#   Mem%     - System memory used")?;
        writeln!(logger.writer, "#   CG%      - Cgroup memory used (container limit)")?;
        writeln!(logger.writer, "#   Cache    - File-backed page cache (mmap'd parquet files live here)")?;
        writeln!(logger.writer, "#   Dirty    - Pages modified but not yet written to disk")?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, "# Process Section (if monitored with -p or -n):")?;
        writeln!(logger.writer, "#   RssAnon  - Anonymous memory (heap, stack, allocations)")?;
        writeln!(logger.writer, "#   RssFile  - File-backed memory (mmap'd files in process space)")?;
        writeln!(logger.writer, "#   ProcRd   - Actual bytes read from disk by process")?;
        writeln!(logger.writer, "#   ProcWr   - Actual bytes written to disk by process")?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, "# Disk Section:")?;
        writeln!(logger.writer, "#   InFlt    - I/O requests currently in flight")?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, "# PSI (Pressure Stall Information):")?;
        writeln!(logger.writer, "#   MemPSI   - % time tasks stalled on memory (some avg10)")?;
        writeln!(logger.writer, "#   IoPSI    - % time tasks stalled on I/O (some avg10)")?;
        writeln!(logger.writer, "#")?;
        writeln!(logger.writer, 
            "{:<8} {:>5} {:>5} {:>5} {:>5} {:>7} {:>7} {:>8} {:>8} {:>10} {:>10} {:>5} {:>5} {:>5}",
            "Time", "CPU%", "IOW%", "Mem%", "CG%", "Cache", "Dirty", "RssAnon", "RssFile",
            "ProcRd", "ProcWr", "InFlt", "MemPS", "IoPSI"
        )?;
        writeln!(logger.writer, "{}", "-".repeat(115))?;

        Ok(logger)
    }

    /// Log a sample in human-readable format
    pub fn log(&mut self, sample: &MetricsSample) -> Result<()> {
        let timestamp = sample.timestamp.format("%H:%M:%S");
        
        // Cgroup percentage
        let cgroup_str = sample.memory.cgroup_usage_percent
            .map(|p| format!("{:>5.1}", p))
            .unwrap_or_else(|| "  N/A".to_string());

        // Page cache info
        let cache_str = format_bytes_short(sample.memory.cached);
        let dirty_str = format_bytes_short(sample.memory.dirty);

        // Process memory breakdown and I/O
        let (rss_anon_str, rss_file_str, proc_rd_str, proc_wr_str) = sample.process.as_ref()
            .map(|p| (
                format_bytes_short(p.rss_anon),
                format_bytes_short(p.rss_file),
                format_throughput(p.io_read_bytes_per_sec),
                format_throughput(p.io_write_bytes_per_sec),
            ))
            .unwrap_or_else(|| (
                "     N/A".to_string(),
                "     N/A".to_string(),
                "       N/A".to_string(),
                "       N/A".to_string(),
            ));

        // Total in-flight I/O
        let in_flight = sample.disk.total_in_flight;

        // PSI metrics
        let (mem_psi, io_psi) = sample.psi.as_ref()
            .map(|p| (p.memory.some_avg10, p.io.some_avg10))
            .unwrap_or((0.0, 0.0));

        writeln!(
            self.writer,
            "{:<8} {:>5.1} {:>5.1} {:>5.1} {} {:>7} {:>7} {:>8} {:>8} {:>10} {:>10} {:>5} {:>5.1} {:>5.1}",
            timestamp,
            sample.cpu.total_utilization,
            sample.cpu.iowait_percent,
            sample.memory.used_percent,
            cgroup_str,
            cache_str,
            dirty_str,
            rss_anon_str,
            rss_file_str,
            proc_rd_str,
            proc_wr_str,
            in_flight,
            mem_psi,
            io_psi,
        )?;

        self.samples_written += 1;

        // Flush every sample for real-time logging
        self.writer.flush()?;

        Ok(())
    }

    /// Flush any buffered data
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}

impl Drop for TextLogger {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

/// Summary statistics calculated from metrics history
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSummary {
    pub duration_secs: f64,
    pub samples_count: u64,

    // CPU summary
    pub cpu_avg_utilization: f64,
    pub cpu_max_utilization: f64,
    pub cpu_avg_iowait: f64,
    pub cpu_max_iowait: f64,

    // Memory summary
    pub memory_avg_used_percent: f64,
    pub memory_max_used_percent: f64,
    pub memory_max_used_bytes: u64,
    pub cgroup_max_usage_percent: Option<f64>,
    pub swap_max_used: u64,

    // Disk summary
    pub disk_max_read_throughput: f64,
    pub disk_max_write_throughput: f64,
    pub disk_max_utilization: f64,

    // Network summary
    pub network_total_rx_bytes: u64,
    pub network_total_tx_bytes: u64,
    pub network_max_rx_throughput: f64,
    pub network_max_tx_throughput: f64,

    // Process summary (if monitored)
    pub process_max_cpu: Option<f64>,
    pub process_max_rss: Option<u64>,
    pub process_max_fds: Option<u64>,

    // Bottleneck analysis
    pub bottleneck_indicators: Vec<String>,
}

/// Accumulator for building summary statistics
pub struct SummaryAccumulator {
    samples: Vec<MetricsSample>,
    start_time: Option<DateTime<Utc>>,
}

impl SummaryAccumulator {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            start_time: None,
        }
    }

    /// Add a sample to the accumulator
    pub fn add_sample(&mut self, sample: MetricsSample) {
        if self.start_time.is_none() {
            self.start_time = Some(sample.timestamp);
        }
        self.samples.push(sample);
    }

    /// Generate summary from accumulated samples
    pub fn generate_summary(&self) -> Option<MetricsSummary> {
        if self.samples.is_empty() {
            return None;
        }

        let first = self.samples.first()?;
        let last = self.samples.last()?;
        let duration_secs = (last.timestamp - first.timestamp).num_milliseconds() as f64 / 1000.0;

        // CPU stats
        let cpu_utils: Vec<f64> = self.samples.iter().map(|s| s.cpu.total_utilization).collect();
        let cpu_iowaits: Vec<f64> = self.samples.iter().map(|s| s.cpu.iowait_percent).collect();

        // Memory stats
        let mem_used_pcts: Vec<f64> = self.samples.iter().map(|s| s.memory.used_percent).collect();
        let mem_used_bytes: Vec<u64> = self.samples.iter().map(|s| s.memory.used).collect();
        let cgroup_usages: Vec<f64> = self.samples.iter()
            .filter_map(|s| s.memory.cgroup_usage_percent)
            .collect();
        let swap_used: Vec<u64> = self.samples.iter().map(|s| s.memory.swap_used).collect();

        // Disk stats
        let disk_reads: Vec<f64> = self.samples.iter().map(|s| s.disk.total_read_bytes_per_sec).collect();
        let disk_writes: Vec<f64> = self.samples.iter().map(|s| s.disk.total_write_bytes_per_sec).collect();
        let disk_utils: Vec<f64> = self.samples.iter()
            .flat_map(|s| s.disk.disks.iter().map(|d| d.utilization_percent))
            .collect();

        // Network stats
        let net_rx: Vec<f64> = self.samples.iter().map(|s| s.network.total_rx_bytes_per_sec).collect();
        let net_tx: Vec<f64> = self.samples.iter().map(|s| s.network.total_tx_bytes_per_sec).collect();

        // Process stats
        let proc_cpus: Vec<f64> = self.samples.iter()
            .filter_map(|s| s.process.as_ref().map(|p| p.cpu_percent))
            .collect();
        let proc_rss: Vec<u64> = self.samples.iter()
            .filter_map(|s| s.process.as_ref().map(|p| p.rss_bytes))
            .collect();
        let proc_fds: Vec<u64> = self.samples.iter()
            .filter_map(|s| s.process.as_ref().map(|p| p.num_fds))
            .collect();

        // Calculate network totals from interface totals in last sample
        let network_total_rx = last.network.interfaces.iter().map(|i| i.rx_bytes_total).sum();
        let network_total_tx = last.network.interfaces.iter().map(|i| i.tx_bytes_total).sum();

        // Bottleneck analysis
        let mut bottlenecks = Vec::new();
        let avg_cpu = avg(&cpu_utils);
        let max_cpu = max_f64(&cpu_utils);
        let max_iowait = max_f64(&cpu_iowaits);
        let max_disk_util = max_f64(&disk_utils);
        let max_cgroup = max_f64(&cgroup_usages);

        if avg_cpu > 90.0 {
            bottlenecks.push("CPU-bound: High average CPU utilization (>90%)".to_string());
        }
        if max_iowait > 50.0 {
            bottlenecks.push("I/O-bound: High CPU iowait observed (>50%)".to_string());
        }
        if max_cgroup > 90.0 {
            bottlenecks.push("Memory-bound: Cgroup memory near limit (>90%)".to_string());
        }
        if *swap_used.iter().max().unwrap_or(&0) > 0 {
            bottlenecks.push("Memory pressure: Swap usage detected".to_string());
        }
        if max_disk_util > 80.0 {
            bottlenecks.push("Disk I/O-bound: High disk utilization (>80%)".to_string());
        }

        Some(MetricsSummary {
            duration_secs,
            samples_count: self.samples.len() as u64,
            cpu_avg_utilization: avg_cpu,
            cpu_max_utilization: max_cpu,
            cpu_avg_iowait: avg(&cpu_iowaits),
            cpu_max_iowait: max_iowait,
            memory_avg_used_percent: avg(&mem_used_pcts),
            memory_max_used_percent: max_f64(&mem_used_pcts),
            memory_max_used_bytes: *mem_used_bytes.iter().max().unwrap_or(&0),
            cgroup_max_usage_percent: if cgroup_usages.is_empty() { None } else { Some(max_f64(&cgroup_usages)) },
            swap_max_used: *swap_used.iter().max().unwrap_or(&0),
            disk_max_read_throughput: max_f64(&disk_reads),
            disk_max_write_throughput: max_f64(&disk_writes),
            disk_max_utilization: max_disk_util,
            network_total_rx_bytes: network_total_rx,
            network_total_tx_bytes: network_total_tx,
            network_max_rx_throughput: max_f64(&net_rx),
            network_max_tx_throughput: max_f64(&net_tx),
            process_max_cpu: if proc_cpus.is_empty() { None } else { Some(max_f64(&proc_cpus)) },
            process_max_rss: proc_rss.iter().max().copied(),
            process_max_fds: proc_fds.iter().max().copied(),
            bottleneck_indicators: bottlenecks,
        })
    }

    /// Clear accumulated samples
    pub fn clear(&mut self) {
        self.samples.clear();
        self.start_time = None;
    }
}

impl Default for SummaryAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

fn avg(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn max_f64(values: &[f64]) -> f64 {
    values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
}
