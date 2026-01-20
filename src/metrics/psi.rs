//! Pressure Stall Information (PSI) metrics collection from /proc/pressure/.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

/// PSI metrics for a single resource (CPU, memory, or I/O)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PsiResourceMetrics {
    /// Percentage of time in last 10s at least one task was stalled
    pub some_avg10: f64,
    /// Percentage of time in last 60s at least one task was stalled
    pub some_avg60: f64,
    /// Percentage of time in last 300s at least one task was stalled
    pub some_avg300: f64,
    /// Total microseconds stalled (some)
    pub some_total: u64,
    /// Percentage of time in last 10s ALL tasks were stalled (not available for CPU)
    pub full_avg10: Option<f64>,
    /// Percentage of time in last 60s ALL tasks were stalled
    pub full_avg60: Option<f64>,
    /// Percentage of time in last 300s ALL tasks were stalled
    pub full_avg300: Option<f64>,
    /// Total microseconds stalled (full)
    pub full_total: Option<u64>,
}

/// Complete PSI metrics for all resources
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PsiMetrics {
    /// CPU pressure metrics
    pub cpu: PsiResourceMetrics,
    /// Memory pressure metrics
    pub memory: PsiResourceMetrics,
    /// I/O pressure metrics
    pub io: PsiResourceMetrics,
}

/// PSI metrics collector
pub struct PsiCollector;

impl PsiCollector {
    pub fn new() -> Self {
        Self
    }

    /// Collect current PSI metrics
    pub fn collect(&mut self) -> Result<PsiMetrics> {
        Ok(PsiMetrics {
            cpu: read_psi_file("/proc/pressure/cpu", false),
            memory: read_psi_file("/proc/pressure/memory", true),
            io: read_psi_file("/proc/pressure/io", true),
        })
    }
}

impl Default for PsiCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Read and parse a PSI file
/// `has_full` indicates if the resource has "full" metrics (memory and I/O do, CPU doesn't)
fn read_psi_file(path: &str, has_full: bool) -> PsiResourceMetrics {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return PsiResourceMetrics::default(),
    };

    let mut metrics = PsiResourceMetrics::default();

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let is_full = parts[0] == "full";
        let is_some = parts[0] == "some";

        if !is_full && !is_some {
            continue;
        }

        // Parse avg10, avg60, avg300, total from the line
        // Format: some avg10=X.XX avg60=X.XX avg300=X.XX total=XXXXX
        for part in &parts[1..] {
            if let Some((key, value)) = part.split_once('=') {
                match (key, is_full) {
                    ("avg10", false) => metrics.some_avg10 = value.parse().unwrap_or(0.0),
                    ("avg60", false) => metrics.some_avg60 = value.parse().unwrap_or(0.0),
                    ("avg300", false) => metrics.some_avg300 = value.parse().unwrap_or(0.0),
                    ("total", false) => metrics.some_total = value.parse().unwrap_or(0),
                    ("avg10", true) if has_full => metrics.full_avg10 = Some(value.parse().unwrap_or(0.0)),
                    ("avg60", true) if has_full => metrics.full_avg60 = Some(value.parse().unwrap_or(0.0)),
                    ("avg300", true) if has_full => metrics.full_avg300 = Some(value.parse().unwrap_or(0.0)),
                    ("total", true) if has_full => metrics.full_total = Some(value.parse().unwrap_or(0)),
                    _ => {}
                }
            }
        }
    }

    metrics
}
