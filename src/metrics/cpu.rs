//! CPU metrics collection from /proc/stat and /proc/loadavg.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

/// Raw CPU time values from /proc/stat
#[derive(Debug, Clone, Default)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
    pub guest: u64,
    pub guest_nice: u64,
}

impl CpuTimes {
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    pub fn active(&self) -> u64 {
        self.total() - self.idle - self.iowait
    }
}

/// Per-core CPU utilization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreUtilization {
    pub core_id: usize,
    pub utilization_percent: f64,
    pub user_percent: f64,
    pub system_percent: f64,
    pub iowait_percent: f64,
}

/// Aggregated CPU metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuMetrics {
    /// Overall CPU utilization percentage
    pub total_utilization: f64,
    /// User space CPU time percentage
    pub user_percent: f64,
    /// Kernel space CPU time percentage
    pub system_percent: f64,
    /// I/O wait percentage
    pub iowait_percent: f64,
    /// Per-core utilization
    pub per_core: Vec<CoreUtilization>,
    /// Load averages (1min, 5min, 15min)
    pub load_avg: (f64, f64, f64),
    /// Context switches per second
    pub context_switches: u64,
    /// Context switches delta (for rate calculation)
    pub context_switches_delta: Option<u64>,
    /// Interrupts per second
    pub interrupts: u64,
    /// Interrupts delta (for rate calculation)
    pub interrupts_delta: Option<u64>,
    /// Number of CPU cores
    pub core_count: usize,
}

/// CPU metrics collector with state for delta calculations
pub struct CpuCollector {
    prev_total_times: Option<CpuTimes>,
    prev_core_times: HashMap<usize, CpuTimes>,
    prev_context_switches: Option<u64>,
    prev_interrupts: Option<u64>,
}

impl CpuCollector {
    pub fn new() -> Self {
        Self {
            prev_total_times: None,
            prev_core_times: HashMap::new(),
            prev_context_switches: None,
            prev_interrupts: None,
        }
    }

    /// Collect current CPU metrics
    pub fn collect(&mut self) -> Result<CpuMetrics> {
        let stat_content = fs::read_to_string("/proc/stat")
            .context("Failed to read /proc/stat")?;

        let mut total_times = CpuTimes::default();
        let mut core_times: HashMap<usize, CpuTimes> = HashMap::new();
        let mut context_switches: u64 = 0;
        let mut interrupts: u64 = 0;

        for line in stat_content.lines() {
            if line.starts_with("cpu ") {
                total_times = parse_cpu_line(line)?;
            } else if line.starts_with("cpu") {
                // Per-core line like "cpu0", "cpu1", etc.
                let core_id: usize = line[3..].split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                core_times.insert(core_id, parse_cpu_line(line)?);
            } else if line.starts_with("ctxt ") {
                context_switches = line.split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("intr ") {
                interrupts = line.split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }

        // Calculate utilization from deltas
        let (total_util, user_pct, sys_pct, iowait_pct) = if let Some(ref prev) = self.prev_total_times {
            calculate_utilization(prev, &total_times)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        // Per-core utilization
        let mut per_core = Vec::new();
        for (core_id, times) in &core_times {
            let (util, user, sys, iowait) = if let Some(prev) = self.prev_core_times.get(core_id) {
                calculate_utilization(prev, times)
            } else {
                (0.0, 0.0, 0.0, 0.0)
            };
            per_core.push(CoreUtilization {
                core_id: *core_id,
                utilization_percent: util,
                user_percent: user,
                system_percent: sys,
                iowait_percent: iowait,
            });
        }
        per_core.sort_by_key(|c| c.core_id);

        // Context switches and interrupts deltas
        let ctx_delta = self.prev_context_switches.map(|prev| context_switches.saturating_sub(prev));
        let intr_delta = self.prev_interrupts.map(|prev| interrupts.saturating_sub(prev));

        // Load average
        let load_avg = read_load_average()?;

        // Update state for next collection
        self.prev_total_times = Some(total_times);
        self.prev_core_times = core_times;
        self.prev_context_switches = Some(context_switches);
        self.prev_interrupts = Some(interrupts);

        Ok(CpuMetrics {
            total_utilization: total_util,
            user_percent: user_pct,
            system_percent: sys_pct,
            iowait_percent: iowait_pct,
            per_core,
            load_avg,
            context_switches,
            context_switches_delta: ctx_delta,
            interrupts,
            interrupts_delta: intr_delta,
            core_count: self.prev_core_times.len(),
        })
    }
}

impl Default for CpuCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_cpu_line(line: &str) -> Result<CpuTimes> {
    let parts: Vec<u64> = line
        .split_whitespace()
        .skip(1) // Skip "cpu" or "cpuN"
        .filter_map(|s| s.parse().ok())
        .collect();

    Ok(CpuTimes {
        user: *parts.first().unwrap_or(&0),
        nice: *parts.get(1).unwrap_or(&0),
        system: *parts.get(2).unwrap_or(&0),
        idle: *parts.get(3).unwrap_or(&0),
        iowait: *parts.get(4).unwrap_or(&0),
        irq: *parts.get(5).unwrap_or(&0),
        softirq: *parts.get(6).unwrap_or(&0),
        steal: *parts.get(7).unwrap_or(&0),
        guest: *parts.get(8).unwrap_or(&0),
        guest_nice: *parts.get(9).unwrap_or(&0),
    })
}

fn calculate_utilization(prev: &CpuTimes, curr: &CpuTimes) -> (f64, f64, f64, f64) {
    let total_delta = curr.total().saturating_sub(prev.total());
    if total_delta == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    let idle_delta = (curr.idle + curr.iowait).saturating_sub(prev.idle + prev.iowait);
    let user_delta = curr.user.saturating_sub(prev.user);
    let system_delta = curr.system.saturating_sub(prev.system);
    let iowait_delta = curr.iowait.saturating_sub(prev.iowait);

    let total_util = 100.0 * (1.0 - (idle_delta as f64 / total_delta as f64));
    let user_pct = 100.0 * (user_delta as f64 / total_delta as f64);
    let sys_pct = 100.0 * (system_delta as f64 / total_delta as f64);
    let iowait_pct = 100.0 * (iowait_delta as f64 / total_delta as f64);

    (total_util, user_pct, sys_pct, iowait_pct)
}

fn read_load_average() -> Result<(f64, f64, f64)> {
    let content = fs::read_to_string("/proc/loadavg")
        .context("Failed to read /proc/loadavg")?;

    let parts: Vec<f64> = content
        .split_whitespace()
        .take(3)
        .filter_map(|s| s.parse().ok())
        .collect();

    Ok((
        *parts.first().unwrap_or(&0.0),
        *parts.get(1).unwrap_or(&0.0),
        *parts.get(2).unwrap_or(&0.0),
    ))
}
