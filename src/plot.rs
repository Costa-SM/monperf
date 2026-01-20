//! Plot generation from JSON log files.

use crate::logging::MetricsSample;
use anyhow::{Context, Result};
use plotters::prelude::*;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Load samples from a JSON Lines log file
pub fn load_samples<P: AsRef<Path>>(path: P) -> Result<Vec<MetricsSample>> {
    let file = File::open(path.as_ref())
        .with_context(|| format!("Failed to open log file: {}", path.as_ref().display()))?;
    
    let reader = BufReader::new(file);
    let mut samples = Vec::new();
    
    for (line_num, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("Failed to read line {}", line_num + 1))?;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<MetricsSample>(&line) {
            Ok(sample) => samples.push(sample),
            Err(e) => {
                // Skip non-JSON lines (like text log format)
                if line_num == 0 {
                    return Err(anyhow::anyhow!(
                        "Log file doesn't appear to be JSON format. Use -l to create JSON logs.\nError: {}", e
                    ));
                }
            }
        }
    }
    
    if samples.is_empty() {
        return Err(anyhow::anyhow!("No samples found in log file"));
    }
    
    Ok(samples)
}

/// Generate all plots from samples
pub fn generate_plots<P: AsRef<Path>>(samples: &[MetricsSample], output_dir: P) -> Result<Vec<String>> {
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
    if samples.iter().any(|s| s.process.is_some()) {
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

/// Convert timestamp to seconds from start
fn to_elapsed_secs(samples: &[MetricsSample]) -> Vec<f64> {
    if samples.is_empty() {
        return vec![];
    }
    let start = samples[0].timestamp;
    samples.iter()
        .map(|s| (s.timestamp - start).num_milliseconds() as f64 / 1000.0)
        .collect()
}

/// Plot CPU metrics
fn plot_cpu<P: AsRef<Path>>(samples: &[MetricsSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let total: Vec<f64> = samples.iter().map(|s| s.cpu.total_utilization).collect();
    let user: Vec<f64> = samples.iter().map(|s| s.cpu.user_percent).collect();
    let system: Vec<f64> = samples.iter().map(|s| s.cpu.system_percent).collect();
    let iowait: Vec<f64> = samples.iter().map(|s| s.cpu.iowait_percent).collect();
    
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
fn plot_memory<P: AsRef<Path>>(samples: &[MetricsSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let used_pct: Vec<f64> = samples.iter().map(|s| s.memory.used_percent).collect();
    let cgroup_pct: Vec<f64> = samples.iter()
        .map(|s| s.memory.cgroup_usage_percent.unwrap_or(0.0))
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
fn plot_disk_io<P: AsRef<Path>>(samples: &[MetricsSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let read_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.disk.total_read_bytes_per_sec))
        .collect();
    let write_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.disk.total_write_bytes_per_sec))
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
fn plot_network_io<P: AsRef<Path>>(samples: &[MetricsSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let rx_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.network.total_rx_bytes_per_sec))
        .collect();
    let tx_mb: Vec<f64> = samples.iter()
        .map(|s| to_mb_per_sec(s.network.total_tx_bytes_per_sec))
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
fn plot_process<P: AsRef<Path>>(samples: &[MetricsSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let cpu: Vec<f64> = samples.iter()
        .map(|s| s.process.as_ref().map(|p| p.cpu_percent).unwrap_or(0.0))
        .collect();
    let rss_gb: Vec<f64> = samples.iter()
        .map(|s| s.process.as_ref().map(|p| p.rss_bytes as f64 / (1024.0 * 1024.0 * 1024.0)).unwrap_or(0.0))
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
fn plot_overview<P: AsRef<Path>>(samples: &[MetricsSample], path: P) -> Result<()> {
    let times = to_elapsed_secs(samples);
    let max_time = times.last().copied().unwrap_or(1.0);
    
    let root = SVGBackend::new(path.as_ref(), (1600, 900)).into_drawing_area();
    root.fill(&WHITE)?;
    
    let areas = root.split_evenly((2, 2));
    
    // CPU (top-left)
    {
        let total: Vec<f64> = samples.iter().map(|s| s.cpu.total_utilization).collect();
        let iowait: Vec<f64> = samples.iter().map(|s| s.cpu.iowait_percent).collect();
        
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
        let used_pct: Vec<f64> = samples.iter().map(|s| s.memory.used_percent).collect();
        let cgroup_pct: Vec<f64> = samples.iter()
            .map(|s| s.memory.cgroup_usage_percent.unwrap_or(0.0))
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
            .map(|s| to_mb_per_sec(s.disk.total_read_bytes_per_sec))
            .collect();
        let write_mb: Vec<f64> = samples.iter()
            .map(|s| to_mb_per_sec(s.disk.total_write_bytes_per_sec))
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
            .map(|s| to_mb_per_sec(s.network.total_rx_bytes_per_sec))
            .collect();
        let tx_mb: Vec<f64> = samples.iter()
            .map(|s| to_mb_per_sec(s.network.total_tx_bytes_per_sec))
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
