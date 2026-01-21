//! Plot generation from CSV log files.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use plotters::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Simplified sample structure for plotting (parsed from CSV)
#[derive(Debug, Clone, Default)]
pub struct PlotSample {
    pub timestamp: DateTime<Utc>,
    pub cpu_total: f64,
    pub cpu_user: f64,
    pub cpu_system: f64,
    pub cpu_iowait: f64,
    pub mem_used_pct: f64,
    pub cgroup_usage_pct: Option<f64>,
    pub disk_read_bytes_per_sec: f64,
    pub disk_write_bytes_per_sec: f64,
    pub net_rx_bytes_per_sec: f64,
    pub net_tx_bytes_per_sec: f64,
    pub proc_cpu_pct: Option<f64>,
    pub proc_rss_bytes: Option<u64>,
}

/// Detailed sample structure with per-core, per-disk, per-interface data
#[derive(Debug, Clone, Default)]
pub struct DetailedPlotSample {
    pub timestamp: DateTime<Utc>,
    // CPU
    pub cpu_total: f64,
    pub cpu_user: f64,
    pub cpu_system: f64,
    pub cpu_iowait: f64,
    pub cpu_load_1m: f64,
    pub cpu_load_5m: f64,
    pub cpu_load_15m: f64,
    pub per_core_pct: Vec<f64>,
    // Memory
    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub mem_available_bytes: u64,
    pub mem_used_pct: f64,
    pub mem_buffers_bytes: u64,
    pub mem_cached_bytes: u64,
    pub mem_dirty_bytes: u64,
    pub mem_writeback_bytes: u64,
    pub mem_swap_total_bytes: u64,
    pub mem_swap_used_bytes: u64,
    pub cgroup_limit_bytes: Option<u64>,
    pub cgroup_current_bytes: Option<u64>,
    pub cgroup_usage_pct: Option<f64>,
    // Disk (per-device)
    pub disk_devices: Vec<String>,
    pub disk_read_bytes_per_sec: Vec<f64>,
    pub disk_write_bytes_per_sec: Vec<f64>,
    pub disk_util_pct: Vec<f64>,
    pub disk_total_read: f64,
    pub disk_total_write: f64,
    // Network (per-interface)
    pub net_interfaces: Vec<String>,
    pub net_rx_bytes_per_sec: Vec<f64>,
    pub net_tx_bytes_per_sec: Vec<f64>,
    pub net_total_rx: f64,
    pub net_total_tx: f64,
    // PSI
    pub psi_cpu_some_avg10: f64,
    pub psi_mem_some_avg10: f64,
    pub psi_mem_full_avg10: Option<f64>,
    pub psi_io_some_avg10: f64,
    pub psi_io_full_avg10: Option<f64>,
    // Process
    pub proc_cpu_pct: Option<f64>,
    pub proc_rss_bytes: Option<u64>,
    pub proc_io_read_bytes_per_sec: Option<f64>,
    pub proc_io_write_bytes_per_sec: Option<f64>,
}

/// Load basic samples from a CSV log file (for simple plots)
pub fn load_samples<P: AsRef<Path>>(path: P) -> Result<Vec<PlotSample>> {
    let detailed = load_detailed_samples(path)?;
    Ok(detailed.into_iter().map(|d| PlotSample {
        timestamp: d.timestamp,
        cpu_total: d.cpu_total,
        cpu_user: d.cpu_user,
        cpu_system: d.cpu_system,
        cpu_iowait: d.cpu_iowait,
        mem_used_pct: d.mem_used_pct,
        cgroup_usage_pct: d.cgroup_usage_pct,
        disk_read_bytes_per_sec: d.disk_total_read,
        disk_write_bytes_per_sec: d.disk_total_write,
        net_rx_bytes_per_sec: d.net_total_rx,
        net_tx_bytes_per_sec: d.net_total_tx,
        proc_cpu_pct: d.proc_cpu_pct,
        proc_rss_bytes: d.proc_rss_bytes,
    }).collect())
}

/// Load detailed samples from a CSV log file (for detailed plots)
pub fn load_detailed_samples<P: AsRef<Path>>(path: P) -> Result<Vec<DetailedPlotSample>> {
    let file = File::open(path.as_ref())
        .with_context(|| format!("Failed to open log file: {}", path.as_ref().display()))?;
    
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    
    // Read header line
    let header_line = lines.next()
        .ok_or_else(|| anyhow::anyhow!("Empty CSV file"))?
        .context("Failed to read header")?;
    
    // Parse header into column indices
    let headers: Vec<String> = header_line.split(',').map(|s| s.to_string()).collect();
    let col_idx: HashMap<String, usize> = headers.iter()
        .enumerate()
        .map(|(i, h)| (h.clone(), i))
        .collect();
    
    // Find all CPU core columns
    let mut core_ids: Vec<usize> = Vec::new();
    for header in &headers {
        if header.starts_with("cpu_core") && header.ends_with("_pct") {
            if let Some(id_str) = header.strip_prefix("cpu_core").and_then(|s| s.strip_suffix("_pct")) {
                if let Ok(id) = id_str.parse::<usize>() {
                    core_ids.push(id);
                }
            }
        }
    }
    core_ids.sort();
    
    // Find all disk device columns
    let mut disk_devices: Vec<String> = Vec::new();
    for header in &headers {
        if header.starts_with("disk_") && header.ends_with("_read_bytes_per_sec") && header != "disk_total_read_bytes_per_sec" {
            if let Some(dev) = header.strip_prefix("disk_").and_then(|s| s.strip_suffix("_read_bytes_per_sec")) {
                disk_devices.push(dev.to_string());
            }
        }
    }
    
    // Find all network interface columns
    let mut net_interfaces: Vec<String> = Vec::new();
    for header in &headers {
        if header.starts_with("net_") && header.ends_with("_rx_bytes_per_sec") && header != "net_total_rx_bytes_per_sec" {
            if let Some(iface) = header.strip_prefix("net_").and_then(|s| s.strip_suffix("_rx_bytes_per_sec")) {
                net_interfaces.push(iface.to_string());
            }
        }
    }
    
    let mut samples = Vec::new();
    
    for (line_num, line) in lines.enumerate() {
        let line = line.with_context(|| format!("Failed to read line {}", line_num + 2))?;
        if line.trim().is_empty() {
            continue;
        }
        
        let fields: Vec<&str> = line.split(',').collect();
        
        // Helper to parse field
        let parse_f64 = |name: &str| -> f64 {
            col_idx.get(name)
                .and_then(|&i| fields.get(i))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0)
        };
        
        let parse_opt_f64 = |name: &str| -> Option<f64> {
            col_idx.get(name)
                .and_then(|&i| fields.get(i))
                .and_then(|s| {
                    let s = s.trim();
                    if s.is_empty() { None } else { s.parse().ok() }
                })
        };
        
        let parse_u64 = |name: &str| -> u64 {
            col_idx.get(name)
                .and_then(|&i| fields.get(i))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        };
        
        let parse_opt_u64 = |name: &str| -> Option<u64> {
            col_idx.get(name)
                .and_then(|&i| fields.get(i))
                .and_then(|s| {
                    let s = s.trim();
                    if s.is_empty() { None } else { s.parse().ok() }
                })
        };
        
        // Parse timestamp
        let timestamp_str = col_idx.get("timestamp")
            .and_then(|&i| fields.get(i))
            .map(|s| *s)
            .unwrap_or("");
        
        let timestamp = DateTime::parse_from_str(
            &format!("{} +0000", timestamp_str),
            "%Y-%m-%d %H:%M:%S%.3f %z"
        )
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
        
        // Parse per-core CPU
        let per_core_pct: Vec<f64> = core_ids.iter()
            .map(|id| parse_f64(&format!("cpu_core{}_pct", id)))
            .collect();
        
        // Parse per-disk data
        let disk_read: Vec<f64> = disk_devices.iter()
            .map(|dev| parse_f64(&format!("disk_{}_read_bytes_per_sec", dev)))
            .collect();
        let disk_write: Vec<f64> = disk_devices.iter()
            .map(|dev| parse_f64(&format!("disk_{}_write_bytes_per_sec", dev)))
            .collect();
        let disk_util: Vec<f64> = disk_devices.iter()
            .map(|dev| parse_f64(&format!("disk_{}_util_pct", dev)))
            .collect();
        
        // Parse per-interface network data
        let net_rx: Vec<f64> = net_interfaces.iter()
            .map(|iface| parse_f64(&format!("net_{}_rx_bytes_per_sec", iface)))
            .collect();
        let net_tx: Vec<f64> = net_interfaces.iter()
            .map(|iface| parse_f64(&format!("net_{}_tx_bytes_per_sec", iface)))
            .collect();
        
        let sample = DetailedPlotSample {
            timestamp,
            // CPU
            cpu_total: parse_f64("cpu_total_pct"),
            cpu_user: parse_f64("cpu_user_pct"),
            cpu_system: parse_f64("cpu_system_pct"),
            cpu_iowait: parse_f64("cpu_iowait_pct"),
            cpu_load_1m: parse_f64("cpu_load_1m"),
            cpu_load_5m: parse_f64("cpu_load_5m"),
            cpu_load_15m: parse_f64("cpu_load_15m"),
            per_core_pct,
            // Memory
            mem_total_bytes: parse_u64("mem_total_bytes"),
            mem_used_bytes: parse_u64("mem_used_bytes"),
            mem_available_bytes: parse_u64("mem_available_bytes"),
            mem_used_pct: parse_f64("mem_used_pct"),
            mem_buffers_bytes: parse_u64("mem_buffers_bytes"),
            mem_cached_bytes: parse_u64("mem_cached_bytes"),
            mem_dirty_bytes: parse_u64("mem_dirty_bytes"),
            mem_writeback_bytes: parse_u64("mem_writeback_bytes"),
            mem_swap_total_bytes: parse_u64("mem_swap_total_bytes"),
            mem_swap_used_bytes: parse_u64("mem_swap_used_bytes"),
            cgroup_limit_bytes: parse_opt_u64("cgroup_limit_bytes"),
            cgroup_current_bytes: parse_opt_u64("cgroup_current_bytes"),
            cgroup_usage_pct: parse_opt_f64("cgroup_usage_pct"),
            // Disk
            disk_devices: disk_devices.clone(),
            disk_read_bytes_per_sec: disk_read,
            disk_write_bytes_per_sec: disk_write,
            disk_util_pct: disk_util,
            disk_total_read: parse_f64("disk_total_read_bytes_per_sec"),
            disk_total_write: parse_f64("disk_total_write_bytes_per_sec"),
            // Network
            net_interfaces: net_interfaces.clone(),
            net_rx_bytes_per_sec: net_rx,
            net_tx_bytes_per_sec: net_tx,
            net_total_rx: parse_f64("net_total_rx_bytes_per_sec"),
            net_total_tx: parse_f64("net_total_tx_bytes_per_sec"),
            // PSI
            psi_cpu_some_avg10: parse_f64("psi_cpu_some_avg10"),
            psi_mem_some_avg10: parse_f64("psi_mem_some_avg10"),
            psi_mem_full_avg10: parse_opt_f64("psi_mem_full_avg10"),
            psi_io_some_avg10: parse_f64("psi_io_some_avg10"),
            psi_io_full_avg10: parse_opt_f64("psi_io_full_avg10"),
            // Process
            proc_cpu_pct: parse_opt_f64("proc_cpu_pct"),
            proc_rss_bytes: parse_opt_u64("proc_rss_bytes"),
            proc_io_read_bytes_per_sec: parse_opt_f64("proc_io_read_bytes_per_sec"),
            proc_io_write_bytes_per_sec: parse_opt_f64("proc_io_write_bytes_per_sec"),
        };
        
        samples.push(sample);
    }
    
    if samples.is_empty() {
        return Err(anyhow::anyhow!("No samples found in CSV file"));
    }
    
    Ok(samples)
}

/// Generate all plots from samples (using detailed data)
pub fn generate_plots<P: AsRef<Path>>(samples: &[PlotSample], output_dir: P) -> Result<Vec<String>> {
    // Convert simple samples back to load detailed data
    // This is a bit wasteful but maintains API compatibility
    let output_dir = output_dir.as_ref();
    std::fs::create_dir_all(output_dir)?;
    
    let mut generated = Vec::new();
    
    // Generate CPU plot
    let cpu_path = output_dir.join("cpu.svg");
    plot_cpu(samples, &cpu_path)?;
    generated.push(cpu_path.display().to_string());
    
    // Generate Memory plot
    let mem_path = output_dir.join("memory.svg");
    plot_memory(samples, &mem_path)?;
    generated.push(mem_path.display().to_string());
    
    // Generate Disk I/O plot
    let disk_path = output_dir.join("disk_io.svg");
    plot_disk_io(samples, &disk_path)?;
    generated.push(disk_path.display().to_string());
    
    // Generate Network I/O plot
    let net_path = output_dir.join("network_io.svg");
    plot_network_io(samples, &net_path)?;
    generated.push(net_path.display().to_string());
    
    // Generate Process plot if data exists
    if samples.iter().any(|s| s.proc_cpu_pct.is_some()) {
        let proc_path = output_dir.join("process.svg");
        plot_process(samples, &proc_path)?;
        generated.push(proc_path.display().to_string());
    }
    
    // Generate combined overview
    let overview_path = output_dir.join("overview.svg");
    plot_overview(samples, &overview_path)?;
    generated.push(overview_path.display().to_string());
    
    Ok(generated)
}

/// Generate all plots including detailed views from CSV file path
pub fn generate_all_plots<P: AsRef<Path>, Q: AsRef<Path>>(csv_path: P, output_dir: Q) -> Result<Vec<String>> {
    let output_dir = output_dir.as_ref();
    std::fs::create_dir_all(output_dir)?;
    
    let detailed_samples = load_detailed_samples(&csv_path)?;
    let simple_samples = load_samples(&csv_path)?;
    
    let mut generated = Vec::new();
    
    // Basic plots
    let cpu_path = output_dir.join("cpu.svg");
    plot_cpu(&simple_samples, &cpu_path)?;
    generated.push(cpu_path.display().to_string());
    
    let mem_path = output_dir.join("memory.svg");
    plot_memory(&simple_samples, &mem_path)?;
    generated.push(mem_path.display().to_string());
    
    let disk_path = output_dir.join("disk_io.svg");
    plot_disk_io(&simple_samples, &disk_path)?;
    generated.push(disk_path.display().to_string());
    
    let net_path = output_dir.join("network_io.svg");
    plot_network_io(&simple_samples, &net_path)?;
    generated.push(net_path.display().to_string());
    
    // Detailed plots
    if !detailed_samples.is_empty() && !detailed_samples[0].per_core_pct.is_empty() {
        let cpu_cores_path = output_dir.join("cpu_cores.svg");
        plot_cpu_cores(&detailed_samples, &cpu_cores_path)?;
        generated.push(cpu_cores_path.display().to_string());
    }
    
    let mem_detail_path = output_dir.join("memory_detailed.svg");
    plot_memory_detailed(&detailed_samples, &mem_detail_path)?;
    generated.push(mem_detail_path.display().to_string());
    
    if !detailed_samples.is_empty() && !detailed_samples[0].disk_devices.is_empty() {
        let disk_detail_path = output_dir.join("disk_io_detailed.svg");
        plot_disk_io_detailed(&detailed_samples, &disk_detail_path)?;
        generated.push(disk_detail_path.display().to_string());
    }
    
    if !detailed_samples.is_empty() && !detailed_samples[0].net_interfaces.is_empty() {
        let net_detail_path = output_dir.join("network_io_detailed.svg");
        plot_network_io_detailed(&detailed_samples, &net_detail_path)?;
        generated.push(net_detail_path.display().to_string());
    }
    
    // PSI plot
    let psi_path = output_dir.join("psi.svg");
    plot_psi(&detailed_samples, &psi_path)?;
    generated.push(psi_path.display().to_string());
    
    // Load average plot
    let load_path = output_dir.join("load_average.svg");
    plot_load_average(&detailed_samples, &load_path)?;
    generated.push(load_path.display().to_string());
    
    // Process plot if data exists
    if simple_samples.iter().any(|s| s.proc_cpu_pct.is_some()) {
        let proc_path = output_dir.join("process.svg");
        plot_process(&simple_samples, &proc_path)?;
        generated.push(proc_path.display().to_string());
        
        let proc_io_path = output_dir.join("process_io.svg");
        plot_process_io(&detailed_samples, &proc_io_path)?;
        generated.push(proc_io_path.display().to_string());
    }
    
    // Combined overview
    let overview_path = output_dir.join("overview.svg");
    plot_overview(&simple_samples, &overview_path)?;
    generated.push(overview_path.display().to_string());
    
    Ok(generated)
}

/// Convert timestamp to seconds from start
fn to_elapsed_secs(samples: &[PlotSample]) -> Vec<f64> {
    if samples.is_empty() {
        return vec![];
    }
    let start = samples[0].timestamp;
    samples.iter()
        .map(|s| (s.timestamp - start).num_milliseconds() as f64 / 1000.0)
        .collect()
}

/// Convert timestamp to seconds from start (for detailed samples)
fn to_elapsed_secs_detailed(samples: &[DetailedPlotSample]) -> Vec<f64> {
    if samples.is_empty() {
        return vec![];
    }
    let start = samples[0].timestamp;
    samples.iter()
        .map(|s| (s.timestamp - start).num_milliseconds() as f64 / 1000.0)
        .collect()
}

/// Plot CPU metrics
fn plot_cpu<P: AsRef<Path>>(samples: &[PlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let total: Vec<f64> = samples.iter().map(|s| s.cpu_total).collect();
    let user: Vec<f64> = samples.iter().map(|s| s.cpu_user).collect();
    let system: Vec<f64> = samples.iter().map(|s| s.cpu_system).collect();
    let iowait: Vec<f64> = samples.iter().map(|s| s.cpu_iowait).collect();
    
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let mut chart = ChartBuilder::on(&root)
        .caption("CPU Utilization", ("sans-serif", 30))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(0f64..max_time, 0f64..100f64)?;
    
    chart.configure_mesh()
        .x_desc("Time (seconds)")
        .y_desc("CPU %")
        .draw()?;
    
    // Total CPU
    chart.draw_series(LineSeries::new(
        times.iter().zip(total.iter()).map(|(x, y)| (*x, *y)),
        &BLUE,
    ))?.label("Total").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    
    // User CPU
    chart.draw_series(LineSeries::new(
        times.iter().zip(user.iter()).map(|(x, y)| (*x, *y)),
        &GREEN,
    ))?.label("User").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN));
    
    // System CPU
    chart.draw_series(LineSeries::new(
        times.iter().zip(system.iter()).map(|(x, y)| (*x, *y)),
        &RED,
    ))?.label("System").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    
    // IO Wait
    chart.draw_series(LineSeries::new(
        times.iter().zip(iowait.iter()).map(|(x, y)| (*x, *y)),
        &MAGENTA,
    ))?.label("IOWait").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], MAGENTA));
    
    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    
    root.present()?;
    Ok(())
}

/// Plot Memory metrics
fn plot_memory<P: AsRef<Path>>(samples: &[PlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let used_pct: Vec<f64> = samples.iter().map(|s| s.mem_used_pct).collect();
    let cgroup_pct: Vec<f64> = samples.iter()
        .map(|s| s.cgroup_usage_pct.unwrap_or(0.0))
        .collect();
    
    let max_time = times.last().copied().unwrap_or(1.0);
    let has_cgroup = cgroup_pct.iter().any(|&v| v > 0.0);
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let mut chart = ChartBuilder::on(&root)
        .caption("Memory Utilization", ("sans-serif", 30))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(0f64..max_time, 0f64..100f64)?;
    
    chart.configure_mesh()
        .x_desc("Time (seconds)")
        .y_desc("Memory %")
        .draw()?;
    
    // System Memory
    chart.draw_series(LineSeries::new(
        times.iter().zip(used_pct.iter()).map(|(x, y)| (*x, *y)),
        &BLUE,
    ))?.label("System Memory").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    
    // Cgroup Memory
    if has_cgroup {
        chart.draw_series(LineSeries::new(
            times.iter().zip(cgroup_pct.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Cgroup Memory").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    }
    
    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    
    root.present()?;
    Ok(())
}

/// Convert bytes/sec to MB/sec
fn to_mb_per_sec(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

/// Plot Disk I/O metrics
fn plot_disk_io<P: AsRef<Path>>(samples: &[PlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let read_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.disk_read_bytes_per_sec))
        .collect();
    let write_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.disk_write_bytes_per_sec))
        .collect();
    
    let max_time = times.last().copied().unwrap_or(1.0);
    let max_throughput = read_mb.iter().chain(write_mb.iter())
        .cloned()
        .fold(0.0_f64, f64::max)
        .max(1.0) * 1.1;
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let mut chart = ChartBuilder::on(&root)
        .caption("Disk I/O Throughput", ("sans-serif", 30))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(0f64..max_time, 0f64..max_throughput)?;
    
    chart.configure_mesh()
        .x_desc("Time (seconds)")
        .y_desc("Throughput (MB/s)")
        .draw()?;
    
    // Read throughput
    chart.draw_series(LineSeries::new(
        times.iter().zip(read_mb.iter()).map(|(x, y)| (*x, *y)),
        &BLUE,
    ))?.label("Read").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    
    // Write throughput
    chart.draw_series(LineSeries::new(
        times.iter().zip(write_mb.iter()).map(|(x, y)| (*x, *y)),
        &RED,
    ))?.label("Write").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    
    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    
    root.present()?;
    Ok(())
}

/// Plot Network I/O metrics
fn plot_network_io<P: AsRef<Path>>(samples: &[PlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let rx_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.net_rx_bytes_per_sec))
        .collect();
    let tx_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.net_tx_bytes_per_sec))
        .collect();
    
    let max_time = times.last().copied().unwrap_or(1.0);
    let max_throughput = rx_mb.iter().chain(tx_mb.iter())
        .cloned()
        .fold(0.0_f64, f64::max)
        .max(1.0) * 1.1;
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let mut chart = ChartBuilder::on(&root)
        .caption("Network I/O Throughput", ("sans-serif", 30))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(0f64..max_time, 0f64..max_throughput)?;
    
    chart.configure_mesh()
        .x_desc("Time (seconds)")
        .y_desc("Throughput (MB/s)")
        .draw()?;
    
    // RX throughput
    chart.draw_series(LineSeries::new(
        times.iter().zip(rx_mb.iter()).map(|(x, y)| (*x, *y)),
        &BLUE,
    ))?.label("RX (Download)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    
    // TX throughput
    chart.draw_series(LineSeries::new(
        times.iter().zip(tx_mb.iter()).map(|(x, y)| (*x, *y)),
        &GREEN,
    ))?.label("TX (Upload)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN));
    
    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    
    root.present()?;
    Ok(())
}

/// Plot Process metrics
fn plot_process<P: AsRef<Path>>(samples: &[PlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let cpu: Vec<f64> = samples.iter()
        .map(|s| s.proc_cpu_pct.unwrap_or(0.0))
        .collect();
    let rss_gb: Vec<f64> = samples.iter()
        .map(|s| s.proc_rss_bytes.map(|b| b as f64 / (1024.0 * 1024.0 * 1024.0)).unwrap_or(0.0))
        .collect();
    
    let max_time = times.last().copied().unwrap_or(1.0);
    let max_cpu = cpu.iter().cloned().fold(0.0_f64, f64::max).max(100.0) * 1.1;
    let max_rss = rss_gb.iter().cloned().fold(0.0_f64, f64::max).max(0.1) * 1.1;
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let (upper, lower) = root.split_vertically(300);
    
    // CPU chart
    {
        let mut chart = ChartBuilder::on(&upper)
            .caption("Process CPU", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(30)
            .y_label_area_size(50)
            .build_cartesian_2d(0f64..max_time, 0f64..max_cpu)?;
        
        chart.configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("CPU %")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(cpu.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?;
    }
    
    // RSS chart
    {
        let mut chart = ChartBuilder::on(&lower)
            .caption("Process Memory (RSS)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(30)
            .y_label_area_size(50)
            .build_cartesian_2d(0f64..max_time, 0f64..max_rss)?;
        
        chart.configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("RSS (GB)")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(rss_gb.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?;
    }
    
    root.present()?;
    Ok(())
}

/// Generate overview plot with all metrics
fn plot_overview<P: AsRef<Path>>(samples: &[PlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let root = SVGBackend::new(path.as_ref(), (1600, 900)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let areas = root.split_evenly((2, 2));
    
    // CPU (top-left)
    {
        let total: Vec<f64> = samples.iter().map(|s| s.cpu_total).collect();
        let iowait: Vec<f64> = samples.iter().map(|s| s.cpu_iowait).collect();
        
        let mut chart = ChartBuilder::on(&areas[0])
            .caption("CPU Utilization", ("sans-serif", 20))
            .margin(5)
            .x_label_area_size(30)
            .y_label_area_size(40)
            .build_cartesian_2d(0f64..max_time, 0f64..100f64)?;
        
        chart.configure_mesh().draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(total.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("Total");
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(iowait.iter()).map(|(x, y)| (*x, *y)),
            &MAGENTA,
        ))?.label("IOWait");
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Memory (top-right)
    {
        let used_pct: Vec<f64> = samples.iter().map(|s| s.mem_used_pct).collect();
        let cgroup_pct: Vec<f64> = samples.iter()
            .map(|s| s.cgroup_usage_pct.unwrap_or(0.0))
            .collect();
        
        let mut chart = ChartBuilder::on(&areas[1])
            .caption("Memory Utilization", ("sans-serif", 20))
            .margin(5)
            .x_label_area_size(30)
            .y_label_area_size(40)
            .build_cartesian_2d(0f64..max_time, 0f64..100f64)?;
        
        chart.configure_mesh().draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(used_pct.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("System");
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(cgroup_pct.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Cgroup");
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Disk I/O (bottom-left)
    {
        let read_mb: Vec<f64> = samples.iter()
            .map(|s| to_mb_per_sec(s.disk_read_bytes_per_sec))
            .collect();
        let write_mb: Vec<f64> = samples.iter()
            .map(|s| to_mb_per_sec(s.disk_write_bytes_per_sec))
            .collect();
        
        let max_y = read_mb.iter().chain(write_mb.iter())
            .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
        
        let mut chart = ChartBuilder::on(&areas[2])
            .caption("Disk I/O (MB/s)", ("sans-serif", 20))
            .margin(5)
            .x_label_area_size(30)
            .y_label_area_size(50)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh().draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(read_mb.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("Read");
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(write_mb.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Write");
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Network I/O (bottom-right)
    {
        let rx_mb: Vec<f64> = samples.iter()
            .map(|s| to_mb_per_sec(s.net_rx_bytes_per_sec))
            .collect();
        let tx_mb: Vec<f64> = samples.iter()
            .map(|s| to_mb_per_sec(s.net_tx_bytes_per_sec))
            .collect();
        
        let max_y = rx_mb.iter().chain(tx_mb.iter())
            .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
        
        let mut chart = ChartBuilder::on(&areas[3])
            .caption("Network I/O (MB/s)", ("sans-serif", 20))
            .margin(5)
            .x_label_area_size(30)
            .y_label_area_size(50)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh().draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(rx_mb.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("RX");
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(tx_mb.iter()).map(|(x, y)| (*x, *y)),
            &GREEN,
        ))?.label("TX");
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    root.present()?;
    Ok(())
}

// =============================================================================
// DETAILED PLOTS
// =============================================================================

/// Generate a color palette for multiple series
fn get_color_palette(n: usize) -> Vec<RGBColor> {
    let base_colors = vec![
        RGBColor(31, 119, 180),   // Blue
        RGBColor(255, 127, 14),   // Orange
        RGBColor(44, 160, 44),    // Green
        RGBColor(214, 39, 40),    // Red
        RGBColor(148, 103, 189),  // Purple
        RGBColor(140, 86, 75),    // Brown
        RGBColor(227, 119, 194),  // Pink
        RGBColor(127, 127, 127),  // Gray
        RGBColor(188, 189, 34),   // Yellow-green
        RGBColor(23, 190, 207),   // Cyan
    ];
    
    let mut colors = Vec::new();
    for i in 0..n {
        colors.push(base_colors[i % base_colors.len()]);
    }
    colors
}

/// Plot all CPU cores in a single file with heatmap-style visualization
fn plot_cpu_cores<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    if samples.is_empty() || samples[0].per_core_pct.is_empty() {
        return Ok(());
    }
    
    let times = to_elapsed_secs_detailed(samples);
    let num_cores = samples[0].per_core_pct.len();
    let max_time = times.last().copied().unwrap_or(1.0);
    
    // Calculate height based on number of cores (minimum 20 pixels per core)
    let chart_height = (num_cores * 25).max(400).min(2000) as u32;
    let root = SVGBackend::new(path.as_ref(), (1600, chart_height + 200)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let (upper, lower) = root.split_vertically(chart_height);
    
    // Upper area: Heatmap of all cores
    {
        let mut chart = ChartBuilder::on(&upper)
            .caption(format!("CPU Core Utilization ({} cores)", num_cores), ("sans-serif", 30))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0..num_cores)?;
        
        chart.configure_mesh()
            .x_desc("Time (seconds)")
            .y_desc("Core ID")
            .y_label_formatter(&|y| format!("Core {}", y))
            .draw()?;
        
        // Draw each core as colored rectangles based on utilization
        let time_step = if times.len() > 1 { 
            (times[1] - times[0]).max(0.1) 
        } else { 
            1.0 
        };
        
        for (t_idx, time) in times.iter().enumerate() {
            if t_idx >= samples.len() { break; }
            let sample = &samples[t_idx];
            
            for (core_id, &util) in sample.per_core_pct.iter().enumerate() {
                // Color based on utilization (green -> yellow -> red)
                let color = if util < 50.0 {
                    RGBColor(
                        (util * 5.1) as u8,
                        200,
                        50,
                    )
                } else {
                    RGBColor(
                        255,
                        (255.0 - (util - 50.0) * 5.1).max(0.0) as u8,
                        50,
                    )
                };
                
                chart.draw_series(std::iter::once(Rectangle::new(
                    [(*time, core_id), (*time + time_step, core_id + 1)],
                    color.filled(),
                )))?;
            }
        }
    }
    
    // Lower area: Legend/color scale
    {
        let mut chart = ChartBuilder::on(&lower)
            .caption("Utilization Scale", ("sans-serif", 20))
            .margin(10)
            .x_label_area_size(30)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..100f64, 0..1)?;
        
        chart.configure_mesh()
            .x_desc("CPU %")
            .disable_y_mesh()
            .disable_y_axis()
            .draw()?;
        
        // Draw color scale
        for pct in 0..100 {
            let color = if pct < 50 {
                RGBColor(
                    (pct as f64 * 5.1) as u8,
                    200,
                    50,
                )
            } else {
                RGBColor(
                    255,
                    (255.0 - (pct as f64 - 50.0) * 5.1).max(0.0) as u8,
                    50,
                )
            };
            
            chart.draw_series(std::iter::once(Rectangle::new(
                [(pct as f64, 0), ((pct + 1) as f64, 1)],
                color.filled(),
            )))?;
        }
    }
    
    root.present()?;
    Ok(())
}

/// Plot detailed memory breakdown
fn plot_memory_detailed<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs_detailed(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let root = SVGBackend::new(path.as_ref(), (1600, 1200)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let areas = root.split_evenly((2, 2));
    
    // Memory Usage (bytes)
    {
        let used_gb: Vec<f64> = samples.iter().map(|s| s.mem_used_bytes as f64 / 1e9).collect();
        let cached_gb: Vec<f64> = samples.iter().map(|s| s.mem_cached_bytes as f64 / 1e9).collect();
        let buffers_gb: Vec<f64> = samples.iter().map(|s| s.mem_buffers_bytes as f64 / 1e9).collect();
        let available_gb: Vec<f64> = samples.iter().map(|s| s.mem_available_bytes as f64 / 1e9).collect();
        
        let max_y = samples.first().map(|s| s.mem_total_bytes as f64 / 1e9).unwrap_or(100.0);
        
        let mut chart = ChartBuilder::on(&areas[0])
            .caption("Memory Usage (GB)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("GB")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(used_gb.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("Used").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(cached_gb.iter()).map(|(x, y)| (*x, *y)),
            &GREEN,
        ))?.label("Cached").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN));
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(buffers_gb.iter()).map(|(x, y)| (*x, *y)),
            &MAGENTA,
        ))?.label("Buffers").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], MAGENTA));
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(available_gb.iter()).map(|(x, y)| (*x, *y)),
            &CYAN,
        ))?.label("Available").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], CYAN));
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Dirty/Writeback
    {
        let dirty_mb: Vec<f64> = samples.iter().map(|s| s.mem_dirty_bytes as f64 / 1e6).collect();
        let writeback_mb: Vec<f64> = samples.iter().map(|s| s.mem_writeback_bytes as f64 / 1e6).collect();
        
        let max_y = dirty_mb.iter().chain(writeback_mb.iter())
            .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
        
        let mut chart = ChartBuilder::on(&areas[1])
            .caption("Dirty/Writeback Pages (MB)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("MB")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(dirty_mb.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Dirty").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(writeback_mb.iter()).map(|(x, y)| (*x, *y)),
            &MAGENTA,
        ))?.label("Writeback").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], MAGENTA));
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Swap Usage
    {
        let swap_used_gb: Vec<f64> = samples.iter().map(|s| s.mem_swap_used_bytes as f64 / 1e9).collect();
        let swap_total_gb = samples.first().map(|s| s.mem_swap_total_bytes as f64 / 1e9).unwrap_or(1.0);
        let max_y = swap_total_gb.max(0.1);
        
        let mut chart = ChartBuilder::on(&areas[2])
            .caption("Swap Usage (GB)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("GB")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(swap_used_gb.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Swap Used").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // CGroup Usage
    {
        let has_cgroup = samples.iter().any(|s| s.cgroup_limit_bytes.is_some());
        
        if has_cgroup {
            let cgroup_pct: Vec<f64> = samples.iter()
                .map(|s| s.cgroup_usage_pct.unwrap_or(0.0))
                .collect();
            let ram_pct: Vec<f64> = samples.iter().map(|s| s.mem_used_pct).collect();
            
            let mut chart = ChartBuilder::on(&areas[3])
                .caption("CGroup vs RAM Usage (%)", ("sans-serif", 25))
                .margin(10)
                .x_label_area_size(40)
                .y_label_area_size(60)
                .build_cartesian_2d(0f64..max_time, 0f64..100f64)?;
            
            chart.configure_mesh()
                .x_desc("Time (s)")
                .y_desc("%")
                .draw()?;
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(cgroup_pct.iter()).map(|(x, y)| (*x, *y)),
                &RED,
            ))?.label("CGroup").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(ram_pct.iter()).map(|(x, y)| (*x, *y)),
                &BLUE,
            ))?.label("RAM").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
            
            chart.configure_series_labels()
                .background_style(WHITE.mix(0.8))
                .position(SeriesLabelPosition::UpperRight)
                .draw()?;
        }
    }
    
    root.present()?;
    Ok(())
}

/// Plot per-disk I/O breakdown
fn plot_disk_io_detailed<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    if samples.is_empty() || samples[0].disk_devices.is_empty() {
        return Ok(());
    }
    
    let times = to_elapsed_secs_detailed(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    let devices = &samples[0].disk_devices;
    let num_devices = devices.len();
    let colors = get_color_palette(num_devices);
    
    // Calculate height based on number of devices
    let plot_height = 400_u32;
    let total_height = plot_height * 3 + 100;
    
    let root = SVGBackend::new(path.as_ref(), (1600, total_height)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let areas = root.split_evenly((3, 1));
    
    // Read throughput per device
    {
        let max_y = samples.iter()
            .flat_map(|s| s.disk_read_bytes_per_sec.iter())
            .cloned()
            .fold(0.0_f64, f64::max) / 1e6 * 1.1;
        let max_y = max_y.max(1.0);
        
        let mut chart = ChartBuilder::on(&areas[0])
            .caption("Disk Read Throughput (MB/s)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(80)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("MB/s")
            .draw()?;
        
        for (dev_idx, device) in devices.iter().enumerate() {
            let data: Vec<f64> = samples.iter()
                .map(|s| s.disk_read_bytes_per_sec.get(dev_idx).copied().unwrap_or(0.0) / 1e6)
                .collect();
            let color = colors[dev_idx];
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(data.iter()).map(|(x, y)| (*x, *y)),
                &color,
            ))?.label(device.clone()).legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
        }
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Write throughput per device
    {
        let max_y = samples.iter()
            .flat_map(|s| s.disk_write_bytes_per_sec.iter())
            .cloned()
            .fold(0.0_f64, f64::max) / 1e6 * 1.1;
        let max_y = max_y.max(1.0);
        
        let mut chart = ChartBuilder::on(&areas[1])
            .caption("Disk Write Throughput (MB/s)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(80)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("MB/s")
            .draw()?;
        
        for (dev_idx, device) in devices.iter().enumerate() {
            let data: Vec<f64> = samples.iter()
                .map(|s| s.disk_write_bytes_per_sec.get(dev_idx).copied().unwrap_or(0.0) / 1e6)
                .collect();
            let color = colors[dev_idx];
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(data.iter()).map(|(x, y)| (*x, *y)),
                &color,
            ))?.label(device.clone()).legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
        }
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Utilization per device
    {
        let mut chart = ChartBuilder::on(&areas[2])
            .caption("Disk Utilization (%)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(80)
            .build_cartesian_2d(0f64..max_time, 0f64..100f64)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("%")
            .draw()?;
        
        for (dev_idx, device) in devices.iter().enumerate() {
            let data: Vec<f64> = samples.iter()
                .map(|s| s.disk_util_pct.get(dev_idx).copied().unwrap_or(0.0))
                .collect();
            let color = colors[dev_idx];
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(data.iter()).map(|(x, y)| (*x, *y)),
                &color,
            ))?.label(device.clone()).legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
        }
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    root.present()?;
    Ok(())
}

/// Plot per-interface network I/O
fn plot_network_io_detailed<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    if samples.is_empty() || samples[0].net_interfaces.is_empty() {
        return Ok(());
    }
    
    let times = to_elapsed_secs_detailed(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    let interfaces = &samples[0].net_interfaces;
    let num_interfaces = interfaces.len();
    let colors = get_color_palette(num_interfaces);
    
    let root = SVGBackend::new(path.as_ref(), (1600, 800)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let (upper, lower) = root.split_vertically(400);
    
    // RX throughput per interface
    {
        let max_y = samples.iter()
            .flat_map(|s| s.net_rx_bytes_per_sec.iter())
            .cloned()
            .fold(0.0_f64, f64::max) / 1e6 * 1.1;
        let max_y = max_y.max(1.0);
        
        let mut chart = ChartBuilder::on(&upper)
            .caption("Network RX Throughput (MB/s)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(80)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("MB/s")
            .draw()?;
        
        for (idx, iface) in interfaces.iter().enumerate() {
            let data: Vec<f64> = samples.iter()
                .map(|s| s.net_rx_bytes_per_sec.get(idx).copied().unwrap_or(0.0) / 1e6)
                .collect();
            let color = colors[idx];
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(data.iter()).map(|(x, y)| (*x, *y)),
                &color,
            ))?.label(iface.clone()).legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
        }
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // TX throughput per interface
    {
        let max_y = samples.iter()
            .flat_map(|s| s.net_tx_bytes_per_sec.iter())
            .cloned()
            .fold(0.0_f64, f64::max) / 1e6 * 1.1;
        let max_y = max_y.max(1.0);
        
        let mut chart = ChartBuilder::on(&lower)
            .caption("Network TX Throughput (MB/s)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(80)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("MB/s")
            .draw()?;
        
        for (idx, iface) in interfaces.iter().enumerate() {
            let data: Vec<f64> = samples.iter()
                .map(|s| s.net_tx_bytes_per_sec.get(idx).copied().unwrap_or(0.0) / 1e6)
                .collect();
            let color = colors[idx];
            
            chart.draw_series(LineSeries::new(
                times.iter().zip(data.iter()).map(|(x, y)| (*x, *y)),
                &color,
            ))?.label(iface.clone()).legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], color));
        }
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    root.present()?;
    Ok(())
}

/// Plot PSI (Pressure Stall Information) metrics
fn plot_psi<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs_detailed(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let root = SVGBackend::new(path.as_ref(), (1600, 900)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let areas = root.split_evenly((3, 1));
    
    // CPU Pressure
    {
        let cpu_some: Vec<f64> = samples.iter().map(|s| s.psi_cpu_some_avg10).collect();
        
        let max_y = cpu_some.iter().cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
        
        let mut chart = ChartBuilder::on(&areas[0])
            .caption("CPU Pressure (avg10)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("% stalled")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(cpu_some.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("Some").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // Memory Pressure
    {
        let mem_some: Vec<f64> = samples.iter().map(|s| s.psi_mem_some_avg10).collect();
        let mem_full: Vec<f64> = samples.iter()
            .map(|s| s.psi_mem_full_avg10.unwrap_or(0.0))
            .collect();
        
        let max_y = mem_some.iter().chain(mem_full.iter())
            .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
        
        let mut chart = ChartBuilder::on(&areas[1])
            .caption("Memory Pressure (avg10)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("% stalled")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(mem_some.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("Some").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(mem_full.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Full").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    // I/O Pressure
    {
        let io_some: Vec<f64> = samples.iter().map(|s| s.psi_io_some_avg10).collect();
        let io_full: Vec<f64> = samples.iter()
            .map(|s| s.psi_io_full_avg10.unwrap_or(0.0))
            .collect();
        
        let max_y = io_some.iter().chain(io_full.iter())
            .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
        
        let mut chart = ChartBuilder::on(&areas[2])
            .caption("I/O Pressure (avg10)", ("sans-serif", 25))
            .margin(10)
            .x_label_area_size(40)
            .y_label_area_size(60)
            .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
        
        chart.configure_mesh()
            .x_desc("Time (s)")
            .y_desc("% stalled")
            .draw()?;
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(io_some.iter()).map(|(x, y)| (*x, *y)),
            &BLUE,
        ))?.label("Some").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
        
        chart.draw_series(LineSeries::new(
            times.iter().zip(io_full.iter()).map(|(x, y)| (*x, *y)),
            &RED,
        ))?.label("Full").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
        
        chart.configure_series_labels()
            .background_style(WHITE.mix(0.8))
            .position(SeriesLabelPosition::UpperRight)
            .draw()?;
    }
    
    root.present()?;
    Ok(())
}

/// Plot load average
fn plot_load_average<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs_detailed(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let load_1m: Vec<f64> = samples.iter().map(|s| s.cpu_load_1m).collect();
    let load_5m: Vec<f64> = samples.iter().map(|s| s.cpu_load_5m).collect();
    let load_15m: Vec<f64> = samples.iter().map(|s| s.cpu_load_15m).collect();
    
    let max_y = load_1m.iter().chain(load_5m.iter()).chain(load_15m.iter())
        .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let mut chart = ChartBuilder::on(&root)
        .caption("Load Average", ("sans-serif", 30))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
    
    chart.configure_mesh()
        .x_desc("Time (seconds)")
        .y_desc("Load")
        .draw()?;
    
    chart.draw_series(LineSeries::new(
        times.iter().zip(load_1m.iter()).map(|(x, y)| (*x, *y)),
        &BLUE,
    ))?.label("1 min").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    
    chart.draw_series(LineSeries::new(
        times.iter().zip(load_5m.iter()).map(|(x, y)| (*x, *y)),
        &GREEN,
    ))?.label("5 min").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN));
    
    chart.draw_series(LineSeries::new(
        times.iter().zip(load_15m.iter()).map(|(x, y)| (*x, *y)),
        &RED,
    ))?.label("15 min").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    
    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    
    root.present()?;
    Ok(())
}

/// Plot process I/O metrics
fn plot_process_io<P: AsRef<Path>>(samples: &[DetailedPlotSample], path: P) -> Result<()> {
    let times = to_elapsed_secs_detailed(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let read_mb: Vec<f64> = samples.iter()
        .map(|s| s.proc_io_read_bytes_per_sec.unwrap_or(0.0) / 1e6)
        .collect();
    let write_mb: Vec<f64> = samples.iter()
        .map(|s| s.proc_io_write_bytes_per_sec.unwrap_or(0.0) / 1e6)
        .collect();
    
    let max_y = read_mb.iter().chain(write_mb.iter())
        .cloned().fold(0.0_f64, f64::max).max(1.0) * 1.1;
    
    let root = SVGBackend::new(path.as_ref(), (1200, 600)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let mut chart = ChartBuilder::on(&root)
        .caption("Process I/O Throughput", ("sans-serif", 30))
        .margin(10)
        .x_label_area_size(40)
        .y_label_area_size(60)
        .build_cartesian_2d(0f64..max_time, 0f64..max_y)?;
    
    chart.configure_mesh()
        .x_desc("Time (seconds)")
        .y_desc("MB/s")
        .draw()?;
    
    chart.draw_series(LineSeries::new(
        times.iter().zip(read_mb.iter()).map(|(x, y)| (*x, *y)),
        &BLUE,
    ))?.label("Read").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    
    chart.draw_series(LineSeries::new(
        times.iter().zip(write_mb.iter()).map(|(x, y)| (*x, *y)),
        &RED,
    ))?.label("Write").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    
    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    
    root.present()?;
    Ok(())
}
