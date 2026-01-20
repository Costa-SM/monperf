//! Alerting module for threshold-based notifications.

use crate::metrics::{CpuMetrics, DiskMetrics, MemoryMetrics, NetworkMetrics};
use crate::process::ProcessMetrics;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Alert severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Warning,
    Critical,
}

/// An alert triggered by a threshold breach
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub timestamp: DateTime<Utc>,
    pub severity: Severity,
    pub category: String,
    pub message: String,
}

/// Alert threshold configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertThresholds {
    /// CPU utilization warning threshold (%)
    pub cpu_warn: f64,
    /// CPU utilization critical threshold (%)
    pub cpu_crit: f64,

    /// Memory usage warning threshold (%)
    pub memory_warn: f64,
    /// Memory usage critical threshold (%)
    pub memory_crit: f64,

    /// Cgroup memory usage warning threshold (%)
    pub cgroup_warn: f64,
    /// Cgroup memory usage critical threshold (%)
    pub cgroup_crit: f64,

    /// Disk utilization warning threshold (%)
    pub disk_util_warn: f64,
    /// Disk utilization critical threshold (%)
    pub disk_util_crit: f64,

    /// Disk queue depth warning threshold
    pub disk_queue_warn: f64,
    /// Disk queue depth critical threshold
    pub disk_queue_crit: f64,

    /// IO wait warning threshold (%)
    pub iowait_warn: f64,
    /// IO wait critical threshold (%)
    pub iowait_crit: f64,

    /// Process RSS warning threshold (bytes)
    pub process_rss_warn: Option<u64>,
    /// Process RSS critical threshold (bytes)
    pub process_rss_crit: Option<u64>,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            cpu_warn: 80.0,
            cpu_crit: 95.0,
            memory_warn: 80.0,
            memory_crit: 95.0,
            cgroup_warn: 85.0,
            cgroup_crit: 95.0,
            disk_util_warn: 70.0,
            disk_util_crit: 90.0,
            disk_queue_warn: 5.0,
            disk_queue_crit: 20.0,
            iowait_warn: 30.0,
            iowait_crit: 60.0,
            process_rss_warn: None,
            process_rss_crit: None,
        }
    }
}

/// Alert checker that maintains state to avoid duplicate alerts
pub struct AlertChecker {
    thresholds: AlertThresholds,
    active_alerts: Vec<String>, // Track active alert keys to avoid duplicates
    cooldown_secs: i64,
    last_alert_time: std::collections::HashMap<String, DateTime<Utc>>,
}

impl AlertChecker {
    pub fn new(thresholds: AlertThresholds) -> Self {
        Self {
            thresholds,
            active_alerts: Vec::new(),
            cooldown_secs: 10, // Don't repeat same alert for 10 seconds
            last_alert_time: std::collections::HashMap::new(),
        }
    }

    /// Check metrics and return any new alerts
    pub fn check(
        &mut self,
        cpu: &CpuMetrics,
        memory: &MemoryMetrics,
        disk: &DiskMetrics,
        _network: &NetworkMetrics,
        process: Option<&ProcessMetrics>,
    ) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let now = Utc::now();

        // CPU alerts
        if cpu.total_utilization >= self.thresholds.cpu_crit {
            self.maybe_alert(
                &mut alerts,
                now,
                "cpu_crit",
                Severity::Critical,
                "CPU",
                format!("CPU critical: {:.1}%", cpu.total_utilization),
            );
        } else if cpu.total_utilization >= self.thresholds.cpu_warn {
            self.maybe_alert(
                &mut alerts,
                now,
                "cpu_warn",
                Severity::Warning,
                "CPU",
                format!("CPU warning: {:.1}%", cpu.total_utilization),
            );
        }

        // IO Wait alerts
        if cpu.iowait_percent >= self.thresholds.iowait_crit {
            self.maybe_alert(
                &mut alerts,
                now,
                "iowait_crit",
                Severity::Critical,
                "CPU",
                format!("IOWait critical: {:.1}%", cpu.iowait_percent),
            );
        } else if cpu.iowait_percent >= self.thresholds.iowait_warn {
            self.maybe_alert(
                &mut alerts,
                now,
                "iowait_warn",
                Severity::Warning,
                "CPU",
                format!("IOWait warning: {:.1}%", cpu.iowait_percent),
            );
        }

        // Memory alerts
        if memory.used_percent >= self.thresholds.memory_crit {
            self.maybe_alert(
                &mut alerts,
                now,
                "memory_crit",
                Severity::Critical,
                "Memory",
                format!("Memory critical: {:.1}%", memory.used_percent),
            );
        } else if memory.used_percent >= self.thresholds.memory_warn {
            self.maybe_alert(
                &mut alerts,
                now,
                "memory_warn",
                Severity::Warning,
                "Memory",
                format!("Memory warning: {:.1}%", memory.used_percent),
            );
        }

        // Cgroup memory alerts
        if let Some(cgroup_pct) = memory.cgroup_usage_percent {
            if cgroup_pct >= self.thresholds.cgroup_crit {
                self.maybe_alert(
                    &mut alerts,
                    now,
                    "cgroup_crit",
                    Severity::Critical,
                    "Memory",
                    format!("Cgroup memory critical: {:.1}%", cgroup_pct),
                );
            } else if cgroup_pct >= self.thresholds.cgroup_warn {
                self.maybe_alert(
                    &mut alerts,
                    now,
                    "cgroup_warn",
                    Severity::Warning,
                    "Memory",
                    format!("Cgroup memory warning: {:.1}%", cgroup_pct),
                );
            }
        }

        // Swap usage alert
        if memory.swap_used > 0 {
            self.maybe_alert(
                &mut alerts,
                now,
                "swap",
                Severity::Warning,
                "Memory",
                format!(
                    "Swap in use: {:.1}% ({} bytes)",
                    memory.swap_percent, memory.swap_used
                ),
            );
        }

        // Disk alerts
        for d in &disk.disks {
            if d.utilization_percent >= self.thresholds.disk_util_crit {
                self.maybe_alert(
                    &mut alerts,
                    now,
                    &format!("disk_{}_crit", d.device),
                    Severity::Critical,
                    "Disk",
                    format!("Disk {} critical: {:.1}%", d.device, d.utilization_percent),
                );
            } else if d.utilization_percent >= self.thresholds.disk_util_warn {
                self.maybe_alert(
                    &mut alerts,
                    now,
                    &format!("disk_{}_warn", d.device),
                    Severity::Warning,
                    "Disk",
                    format!("Disk {} warning: {:.1}%", d.device, d.utilization_percent),
                );
            }

            if d.queue_depth >= self.thresholds.disk_queue_crit {
                self.maybe_alert(
                    &mut alerts,
                    now,
                    &format!("disk_{}_queue_crit", d.device),
                    Severity::Critical,
                    "Disk",
                    format!("Disk {} queue critical: {:.1}", d.device, d.queue_depth),
                );
            } else if d.queue_depth >= self.thresholds.disk_queue_warn {
                self.maybe_alert(
                    &mut alerts,
                    now,
                    &format!("disk_{}_queue_warn", d.device),
                    Severity::Warning,
                    "Disk",
                    format!("Disk {} queue warning: {:.1}", d.device, d.queue_depth),
                );
            }
        }

        // Process alerts
        if let Some(proc) = process {
            if let Some(rss_crit) = self.thresholds.process_rss_crit {
                if proc.rss_bytes >= rss_crit {
                    self.maybe_alert(
                        &mut alerts,
                        now,
                        "process_rss_crit",
                        Severity::Critical,
                        "Process",
                        format!(
                            "Process {} RSS critical: {} bytes",
                            proc.name, proc.rss_bytes
                        ),
                    );
                }
            }
            if let Some(rss_warn) = self.thresholds.process_rss_warn {
                if proc.rss_bytes >= rss_warn
                    && self
                        .thresholds
                        .process_rss_crit
                        .map_or(true, |c| proc.rss_bytes < c)
                {
                    self.maybe_alert(
                        &mut alerts,
                        now,
                        "process_rss_warn",
                        Severity::Warning,
                        "Process",
                        format!(
                            "Process {} RSS warning: {} bytes",
                            proc.name, proc.rss_bytes
                        ),
                    );
                }
            }
        }

        alerts
    }

    fn maybe_alert(
        &mut self,
        alerts: &mut Vec<Alert>,
        now: DateTime<Utc>,
        key: &str,
        severity: Severity,
        category: &str,
        message: String,
    ) {
        // Check cooldown
        if let Some(last_time) = self.last_alert_time.get(key) {
            let elapsed = (now - *last_time).num_seconds();
            if elapsed < self.cooldown_secs {
                return;
            }
        }

        self.last_alert_time.insert(key.to_string(), now);

        alerts.push(Alert {
            timestamp: now,
            severity,
            category: category.to_string(),
            message,
        });
    }

    /// Get current thresholds
    pub fn thresholds(&self) -> &AlertThresholds {
        &self.thresholds
    }

    /// Update thresholds
    pub fn set_thresholds(&mut self, thresholds: AlertThresholds) {
        self.thresholds = thresholds;
    }
}
