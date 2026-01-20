//! Historical logging module for writing metrics to files.

use crate::display::{format_bytes_short, format_throughput};
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

/// Detailed CSV logger for writing comprehensive metrics to a CSV file
/// Includes per-core CPU, per-disk I/O, per-interface network, and full PSI breakdown
pub struct DetailedTextLogger {
    writer: BufWriter<File>,
    samples_written: u64,
    header_written: bool,
    // Track device names from first sample for consistent columns
    core_ids: Vec<usize>,
    disk_devices: Vec<String>,
    interface_names: Vec<String>,
}

impl DetailedTextLogger {
    /// Create a new detailed CSV logger writing to the specified file
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path.as_ref())
            .context("Failed to create detailed CSV file")?;

        Ok(Self {
            writer: BufWriter::new(file),
            samples_written: 0,
            header_written: false,
            core_ids: Vec::new(),
            disk_devices: Vec::new(),
            interface_names: Vec::new(),
        })
    }

    /// Write CSV header based on the first sample's structure
    fn write_header(&mut self, sample: &MetricsSample) -> Result<()> {
        // Capture device names from first sample
        self.core_ids = sample.cpu.per_core.iter().map(|c| c.core_id).collect();
        self.disk_devices = sample.disk.disks.iter().map(|d| d.device.clone()).collect();
        self.interface_names = sample.network.interfaces.iter().map(|i| i.interface.clone()).collect();

        let mut headers = vec![
            // Timestamp
            "timestamp".to_string(),
            // CPU aggregate
            "cpu_total_pct".to_string(),
            "cpu_user_pct".to_string(),
            "cpu_system_pct".to_string(),
            "cpu_iowait_pct".to_string(),
            "cpu_load_1m".to_string(),
            "cpu_load_5m".to_string(),
            "cpu_load_15m".to_string(),
            "cpu_context_switches".to_string(),
            "cpu_interrupts".to_string(),
        ];

        // Per-core CPU columns
        for core_id in &self.core_ids {
            headers.push(format!("cpu_core{}_pct", core_id));
        }

        // Memory columns
        headers.extend(vec![
            "mem_total_bytes".to_string(),
            "mem_used_bytes".to_string(),
            "mem_available_bytes".to_string(),
            "mem_used_pct".to_string(),
            "mem_buffers_bytes".to_string(),
            "mem_cached_bytes".to_string(),
            "mem_dirty_bytes".to_string(),
            "mem_writeback_bytes".to_string(),
            "mem_active_file_bytes".to_string(),
            "mem_inactive_file_bytes".to_string(),
            "mem_swap_total_bytes".to_string(),
            "mem_swap_used_bytes".to_string(),
            "mem_swap_pct".to_string(),
            "mem_major_faults".to_string(),
            "mem_minor_faults".to_string(),
            "cgroup_limit_bytes".to_string(),
            "cgroup_current_bytes".to_string(),
            "cgroup_usage_pct".to_string(),
        ]);

        // Disk aggregate columns
        headers.extend(vec![
            "disk_total_read_bytes_per_sec".to_string(),
            "disk_total_write_bytes_per_sec".to_string(),
            "disk_total_in_flight".to_string(),
        ]);

        // Per-disk columns
        for dev in &self.disk_devices {
            headers.push(format!("disk_{}_read_bytes_per_sec", dev));
            headers.push(format!("disk_{}_write_bytes_per_sec", dev));
            headers.push(format!("disk_{}_read_iops", dev));
            headers.push(format!("disk_{}_write_iops", dev));
            headers.push(format!("disk_{}_read_latency_ms", dev));
            headers.push(format!("disk_{}_write_latency_ms", dev));
            headers.push(format!("disk_{}_util_pct", dev));
            headers.push(format!("disk_{}_in_flight", dev));
        }

        // Network aggregate columns
        headers.extend(vec![
            "net_total_rx_bytes_per_sec".to_string(),
            "net_total_tx_bytes_per_sec".to_string(),
            "net_tcp_connections".to_string(),
            "net_tcp_retransmits".to_string(),
        ]);

        // Per-interface columns
        for iface in &self.interface_names {
            headers.push(format!("net_{}_rx_bytes_per_sec", iface));
            headers.push(format!("net_{}_tx_bytes_per_sec", iface));
            headers.push(format!("net_{}_rx_packets_per_sec", iface));
            headers.push(format!("net_{}_tx_packets_per_sec", iface));
            headers.push(format!("net_{}_rx_errors", iface));
            headers.push(format!("net_{}_tx_errors", iface));
        }

        // PSI columns
        headers.extend(vec![
            "psi_cpu_some_avg10".to_string(),
            "psi_cpu_some_avg60".to_string(),
            "psi_cpu_some_avg300".to_string(),
            "psi_mem_some_avg10".to_string(),
            "psi_mem_some_avg60".to_string(),
            "psi_mem_some_avg300".to_string(),
            "psi_mem_full_avg10".to_string(),
            "psi_mem_full_avg60".to_string(),
            "psi_mem_full_avg300".to_string(),
            "psi_io_some_avg10".to_string(),
            "psi_io_some_avg60".to_string(),
            "psi_io_some_avg300".to_string(),
            "psi_io_full_avg10".to_string(),
            "psi_io_full_avg60".to_string(),
            "psi_io_full_avg300".to_string(),
        ]);

        // Process columns (always included, may be empty)
        headers.extend(vec![
            "proc_pid".to_string(),
            "proc_name".to_string(),
            "proc_state".to_string(),
            "proc_cpu_pct".to_string(),
            "proc_threads".to_string(),
            "proc_fds".to_string(),
            "proc_rss_bytes".to_string(),
            "proc_vsize_bytes".to_string(),
            "proc_vm_peak_bytes".to_string(),
            "proc_rss_anon_bytes".to_string(),
            "proc_rss_file_bytes".to_string(),
            "proc_rss_shmem_bytes".to_string(),
            "proc_vm_swap_bytes".to_string(),
            "proc_io_read_bytes_per_sec".to_string(),
            "proc_io_write_bytes_per_sec".to_string(),
            "proc_io_read_bytes_total".to_string(),
            "proc_io_write_bytes_total".to_string(),
            "proc_io_rchar".to_string(),
            "proc_io_wchar".to_string(),
            "proc_io_cancelled_write_bytes".to_string(),
        ]);

        writeln!(self.writer, "{}", headers.join(","))?;
        self.header_written = true;
        Ok(())
    }

    /// Log a sample as a CSV row
    pub fn log(&mut self, sample: &MetricsSample) -> Result<()> {
        // Write header once we have a sample with populated device data
        // (first sample often has empty lists because rates need two samples)
        if !self.header_written {
            // Wait for a sample with actual device data before writing header
            if sample.disk.disks.is_empty() && sample.network.interfaces.is_empty() {
                return Ok(()); // Skip this sample, wait for populated data
            }
            self.write_header(sample)?;
        }

        let mut values: Vec<String> = Vec::new();

        // Timestamp
        values.push(sample.timestamp.format("%Y-%m-%d %H:%M:%S%.3f").to_string());

        // CPU aggregate
        values.push(format!("{:.2}", sample.cpu.total_utilization));
        values.push(format!("{:.2}", sample.cpu.user_percent));
        values.push(format!("{:.2}", sample.cpu.system_percent));
        values.push(format!("{:.2}", sample.cpu.iowait_percent));
        values.push(format!("{:.2}", sample.cpu.load_avg.0));
        values.push(format!("{:.2}", sample.cpu.load_avg.1));
        values.push(format!("{:.2}", sample.cpu.load_avg.2));
        values.push(sample.cpu.context_switches_delta.map(|v| v.to_string()).unwrap_or_default());
        values.push(sample.cpu.interrupts_delta.map(|v| v.to_string()).unwrap_or_default());

        // Per-core CPU values (match the order from header)
        for core_id in &self.core_ids {
            let util = sample.cpu.per_core
                .iter()
                .find(|c| c.core_id == *core_id)
                .map(|c| c.utilization_percent)
                .unwrap_or(0.0);
            values.push(format!("{:.2}", util));
        }

        // Memory values
        values.push(sample.memory.total.to_string());
        values.push(sample.memory.used.to_string());
        values.push(sample.memory.available.to_string());
        values.push(format!("{:.2}", sample.memory.used_percent));
        values.push(sample.memory.buffers.to_string());
        values.push(sample.memory.cached.to_string());
        values.push(sample.memory.dirty.to_string());
        values.push(sample.memory.writeback.to_string());
        values.push(sample.memory.active_file.to_string());
        values.push(sample.memory.inactive_file.to_string());
        values.push(sample.memory.swap_total.to_string());
        values.push(sample.memory.swap_used.to_string());
        values.push(format!("{:.2}", sample.memory.swap_percent));
        values.push(sample.memory.major_faults_delta.map(|v| v.to_string()).unwrap_or_default());
        values.push(sample.memory.minor_faults_delta.map(|v| v.to_string()).unwrap_or_default());
        values.push(sample.memory.cgroup_limit.map(|v| v.to_string()).unwrap_or_default());
        values.push(sample.memory.cgroup_current.map(|v| v.to_string()).unwrap_or_default());
        values.push(sample.memory.cgroup_usage_percent.map(|v| format!("{:.2}", v)).unwrap_or_default());

        // Disk aggregate
        values.push(format!("{:.2}", sample.disk.total_read_bytes_per_sec));
        values.push(format!("{:.2}", sample.disk.total_write_bytes_per_sec));
        values.push(sample.disk.total_in_flight.to_string());

        // Per-disk values (match the order from header)
        for dev in &self.disk_devices {
            if let Some(disk) = sample.disk.disks.iter().find(|d| &d.device == dev) {
                values.push(format!("{:.2}", disk.read_bytes_per_sec));
                values.push(format!("{:.2}", disk.write_bytes_per_sec));
                values.push(format!("{:.2}", disk.read_iops));
                values.push(format!("{:.2}", disk.write_iops));
                values.push(format!("{:.3}", disk.read_latency_ms));
                values.push(format!("{:.3}", disk.write_latency_ms));
                values.push(format!("{:.2}", disk.utilization_percent));
                values.push(disk.in_flight.to_string());
            } else {
                // Device not found in this sample, add empty values
                for _ in 0..8 {
                    values.push(String::new());
                }
            }
        }

        // Network aggregate
        values.push(format!("{:.2}", sample.network.total_rx_bytes_per_sec));
        values.push(format!("{:.2}", sample.network.total_tx_bytes_per_sec));
        values.push(sample.network.tcp.connections_established.to_string());
        values.push(sample.network.tcp.retransmits_delta.map(|v| v.to_string()).unwrap_or_default());

        // Per-interface values (match the order from header)
        for iface_name in &self.interface_names {
            if let Some(iface) = sample.network.interfaces.iter().find(|i| &i.interface == iface_name) {
                values.push(format!("{:.2}", iface.rx_bytes_per_sec));
                values.push(format!("{:.2}", iface.tx_bytes_per_sec));
                values.push(format!("{:.2}", iface.rx_packets_per_sec));
                values.push(format!("{:.2}", iface.tx_packets_per_sec));
                values.push(iface.rx_errors.to_string());
                values.push(iface.tx_errors.to_string());
            } else {
                // Interface not found in this sample, add empty values
                for _ in 0..6 {
                    values.push(String::new());
                }
            }
        }

        // PSI values
        if let Some(psi) = &sample.psi {
            values.push(format!("{:.2}", psi.cpu.some_avg10));
            values.push(format!("{:.2}", psi.cpu.some_avg60));
            values.push(format!("{:.2}", psi.cpu.some_avg300));
            values.push(format!("{:.2}", psi.memory.some_avg10));
            values.push(format!("{:.2}", psi.memory.some_avg60));
            values.push(format!("{:.2}", psi.memory.some_avg300));
            values.push(psi.memory.full_avg10.map(|v| format!("{:.2}", v)).unwrap_or_default());
            values.push(psi.memory.full_avg60.map(|v| format!("{:.2}", v)).unwrap_or_default());
            values.push(psi.memory.full_avg300.map(|v| format!("{:.2}", v)).unwrap_or_default());
            values.push(format!("{:.2}", psi.io.some_avg10));
            values.push(format!("{:.2}", psi.io.some_avg60));
            values.push(format!("{:.2}", psi.io.some_avg300));
            values.push(psi.io.full_avg10.map(|v| format!("{:.2}", v)).unwrap_or_default());
            values.push(psi.io.full_avg60.map(|v| format!("{:.2}", v)).unwrap_or_default());
            values.push(psi.io.full_avg300.map(|v| format!("{:.2}", v)).unwrap_or_default());
        } else {
            // No PSI data, add empty values
            for _ in 0..15 {
                values.push(String::new());
            }
        }

        // Process values
        if let Some(proc) = &sample.process {
            values.push(proc.pid.to_string());
            // Escape commas and quotes in process name
            values.push(format!("\"{}\"", proc.name.replace('"', "\"\"")));
            values.push(proc.state.to_string());
            values.push(format!("{:.2}", proc.cpu_percent));
            values.push(proc.num_threads.to_string());
            values.push(proc.num_fds.to_string());
            values.push(proc.rss_bytes.to_string());
            values.push(proc.vsize_bytes.to_string());
            values.push(proc.vm_peak.to_string());
            values.push(proc.rss_anon.to_string());
            values.push(proc.rss_file.to_string());
            values.push(proc.rss_shmem.to_string());
            values.push(proc.vm_swap.to_string());
            values.push(format!("{:.2}", proc.io_read_bytes_per_sec));
            values.push(format!("{:.2}", proc.io_write_bytes_per_sec));
            values.push(proc.io_read_bytes.to_string());
            values.push(proc.io_write_bytes.to_string());
            values.push(proc.io_rchar.to_string());
            values.push(proc.io_wchar.to_string());
            values.push(proc.io_cancelled_write_bytes.to_string());
        } else {
            // No process data, add empty values
            for _ in 0..20 {
                values.push(String::new());
            }
        }

        writeln!(self.writer, "{}", values.join(","))?;
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

    /// Get the number of samples written
    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }
}

impl Drop for DetailedTextLogger {
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
