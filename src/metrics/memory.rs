//! Memory metrics collection from /proc/meminfo and cgroup files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

/// Memory metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    /// Total system RAM in bytes
    pub total: u64,
    /// Used memory in bytes (excluding buffers/cache)
    pub used: u64,
    /// Available memory in bytes
    pub available: u64,
    /// Buffer memory in bytes
    pub buffers: u64,
    /// Cached memory in bytes (file-backed page cache)
    pub cached: u64,
    /// Dirty pages (modified but not yet written to disk) in bytes
    pub dirty: u64,
    /// Pages being written back to disk in bytes
    pub writeback: u64,
    /// Active file-backed pages in bytes
    pub active_file: u64,
    /// Inactive file-backed pages in bytes
    pub inactive_file: u64,
    /// Swap total in bytes
    pub swap_total: u64,
    /// Swap used in bytes
    pub swap_used: u64,
    /// Cgroup memory limit (if in cgroup)
    pub cgroup_limit: Option<u64>,
    /// Cgroup memory current usage
    pub cgroup_current: Option<u64>,
    /// Cgroup memory usage percentage
    pub cgroup_usage_percent: Option<f64>,
    /// Major page faults
    pub major_page_faults: u64,
    /// Minor page faults
    pub minor_page_faults: u64,
    /// Major page faults delta (for rate calculation)
    pub major_faults_delta: Option<u64>,
    /// Minor page faults delta (for rate calculation)
    pub minor_faults_delta: Option<u64>,
    /// Used memory percentage
    pub used_percent: f64,
    /// Swap used percentage
    pub swap_percent: f64,
}

/// Memory metrics collector with state for delta calculations
pub struct MemoryCollector {
    prev_major_faults: Option<u64>,
    prev_minor_faults: Option<u64>,
}

impl MemoryCollector {
    pub fn new() -> Self {
        Self {
            prev_major_faults: None,
            prev_minor_faults: None,
        }
    }

    /// Collect current memory metrics
    pub fn collect(&mut self) -> Result<MemoryMetrics> {
        let meminfo = fs::read_to_string("/proc/meminfo")
            .context("Failed to read /proc/meminfo")?;

        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let mut available: u64 = 0;
        let mut buffers: u64 = 0;
        let mut cached: u64 = 0;
        let mut dirty: u64 = 0;
        let mut writeback: u64 = 0;
        let mut active_file: u64 = 0;
        let mut inactive_file: u64 = 0;
        let mut swap_total: u64 = 0;
        let mut swap_free: u64 = 0;

        for line in meminfo.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }

            let value: u64 = parts[1].parse().unwrap_or(0) * 1024; // Convert from KB to bytes

            match parts[0] {
                "MemTotal:" => total = value,
                "MemFree:" => free = value,
                "MemAvailable:" => available = value,
                "Buffers:" => buffers = value,
                "Cached:" => cached = value,
                "Dirty:" => dirty = value,
                "Writeback:" => writeback = value,
                "Active(file):" => active_file = value,
                "Inactive(file):" => inactive_file = value,
                "SwapTotal:" => swap_total = value,
                "SwapFree:" => swap_free = value,
                _ => {}
            }
        }

        let used = total.saturating_sub(free + buffers + cached);
        let swap_used = swap_total.saturating_sub(swap_free);

        // Cgroup v2 memory limits
        let (cgroup_limit, cgroup_current) = read_cgroup_memory();
        let cgroup_usage_percent = match (cgroup_limit, cgroup_current) {
            (Some(limit), Some(current)) if limit > 0 => {
                Some(100.0 * current as f64 / limit as f64)
            }
            _ => None,
        };

        // Page faults from /proc/vmstat
        let (major_faults, minor_faults) = read_page_faults();

        let major_delta = self.prev_major_faults.map(|prev| major_faults.saturating_sub(prev));
        let minor_delta = self.prev_minor_faults.map(|prev| minor_faults.saturating_sub(prev));

        self.prev_major_faults = Some(major_faults);
        self.prev_minor_faults = Some(minor_faults);

        let used_percent = if total > 0 {
            100.0 * used as f64 / total as f64
        } else {
            0.0
        };

        let swap_percent = if swap_total > 0 {
            100.0 * swap_used as f64 / swap_total as f64
        } else {
            0.0
        };

        Ok(MemoryMetrics {
            total,
            used,
            available,
            buffers,
            cached,
            dirty,
            writeback,
            active_file,
            inactive_file,
            swap_total,
            swap_used,
            cgroup_limit,
            cgroup_current,
            cgroup_usage_percent,
            major_page_faults: major_faults,
            minor_page_faults: minor_faults,
            major_faults_delta: major_delta,
            minor_faults_delta: minor_delta,
            used_percent,
            swap_percent,
        })
    }
}

impl Default for MemoryCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn read_cgroup_memory() -> (Option<u64>, Option<u64>) {
    // Try cgroup v2 first
    let limit = fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|s| {
            let trimmed = s.trim();
            if trimmed == "max" {
                None // No limit set
            } else {
                trimmed.parse().ok()
            }
        });

    let current = fs::read_to_string("/sys/fs/cgroup/memory.current")
        .ok()
        .and_then(|s| s.trim().parse().ok());

    // If v2 not available, try v1
    if limit.is_none() && current.is_none() {
        let limit_v1 = fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes")
            .ok()
            .and_then(|s| {
                let val: u64 = s.trim().parse().ok()?;
                // Very large values indicate no limit
                if val > 1_000_000_000_000_000 {
                    None
                } else {
                    Some(val)
                }
            });

        let current_v1 = fs::read_to_string("/sys/fs/cgroup/memory/memory.usage_in_bytes")
            .ok()
            .and_then(|s| s.trim().parse().ok());

        return (limit_v1, current_v1);
    }

    (limit, current)
}

fn read_page_faults() -> (u64, u64) {
    let vmstat = fs::read_to_string("/proc/vmstat").unwrap_or_default();
    let mut major: u64 = 0;
    let mut minor: u64 = 0;

    for line in vmstat.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        match parts[0] {
            "pgmajfault" => major = parts[1].parse().unwrap_or(0),
            "pgfault" => minor = parts[1].parse().unwrap_or(0),
            _ => {}
        }
    }

    (major, minor)
}

/// Check for OOM kills from dmesg (requires root or dmesg access)
pub fn check_oom_kills() -> u64 {
    // Try to read from kernel ring buffer
    if let Ok(output) = std::process::Command::new("dmesg")
        .args(["--level", "err,warn"])
        .output()
    {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            return stdout
                .lines()
                .filter(|line| line.contains("Out of memory") || line.contains("oom-kill"))
                .count() as u64;
        }
    }
    0
}
