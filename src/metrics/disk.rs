//! Disk I/O metrics collection from /proc/diskstats.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Per-disk I/O statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskStats {
    /// Device name (e.g., "sda", "nvme0n1")
    pub device: String,
    /// Read throughput in bytes per second
    pub read_bytes_per_sec: f64,
    /// Write throughput in bytes per second
    pub write_bytes_per_sec: f64,
    /// Read IOPS
    pub read_iops: f64,
    /// Write IOPS
    pub write_iops: f64,
    /// Average read latency in milliseconds
    pub read_latency_ms: f64,
    /// Average write latency in milliseconds
    pub write_latency_ms: f64,
    /// Disk utilization percentage
    pub utilization_percent: f64,
    /// Queue depth (average)
    pub queue_depth: f64,
    /// Total reads completed
    pub reads_completed: u64,
    /// Total writes completed
    pub writes_completed: u64,
    /// Total bytes read
    pub bytes_read: u64,
    /// Total bytes written
    pub bytes_written: u64,
}

/// Raw disk statistics from /proc/diskstats
#[derive(Debug, Clone, Default)]
struct RawDiskStats {
    reads_completed: u64,
    reads_merged: u64,
    sectors_read: u64,
    time_reading_ms: u64,
    writes_completed: u64,
    writes_merged: u64,
    sectors_written: u64,
    time_writing_ms: u64,
    ios_in_progress: u64,
    time_doing_ios_ms: u64,
    weighted_time_ms: u64,
}

/// Aggregated disk metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskMetrics {
    /// Per-disk statistics
    pub disks: Vec<DiskStats>,
    /// Total read throughput across all disks (bytes/sec)
    pub total_read_bytes_per_sec: f64,
    /// Total write throughput across all disks (bytes/sec)
    pub total_write_bytes_per_sec: f64,
    /// Spill directory information (if configured)
    pub spill_dir_info: Option<SpillDirInfo>,
}

/// Information about a spill/temp directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpillDirInfo {
    pub path: String,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub used_percent: f64,
}

/// Disk metrics collector with state for rate calculations
pub struct DiskCollector {
    prev_stats: HashMap<String, RawDiskStats>,
    prev_time_ms: u64,
    spill_dir: Option<String>,
    sector_size: u64, // Usually 512 bytes
}

impl DiskCollector {
    pub fn new() -> Self {
        Self {
            prev_stats: HashMap::new(),
            prev_time_ms: 0,
            spill_dir: None,
            sector_size: 512,
        }
    }

    /// Set the spill directory to monitor
    pub fn set_spill_dir(&mut self, path: &str) {
        self.spill_dir = Some(path.to_string());
    }

    /// Collect current disk metrics
    pub fn collect(&mut self) -> Result<DiskMetrics> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let diskstats = fs::read_to_string("/proc/diskstats")
            .context("Failed to read /proc/diskstats")?;

        let mut current_stats: HashMap<String, RawDiskStats> = HashMap::new();
        let mut disks = Vec::new();

        for line in diskstats.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 14 {
                continue;
            }

            let device = parts[2].to_string();

            // Skip partitions (e.g., sda1) - only monitor whole disks
            // Also skip loop devices and ram disks
            if device.starts_with("loop")
                || device.starts_with("ram")
                || device.starts_with("dm-")
            {
                continue;
            }

            // Check if it's a partition (ends with number for non-nvme, or has 'p' followed by number for nvme)
            let is_partition = if device.starts_with("nvme") {
                device.contains('p') && device.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
            } else {
                device.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
            };

            if is_partition {
                continue;
            }

            let stats = RawDiskStats {
                reads_completed: parts[3].parse().unwrap_or(0),
                reads_merged: parts[4].parse().unwrap_or(0),
                sectors_read: parts[5].parse().unwrap_or(0),
                time_reading_ms: parts[6].parse().unwrap_or(0),
                writes_completed: parts[7].parse().unwrap_or(0),
                writes_merged: parts[8].parse().unwrap_or(0),
                sectors_written: parts[9].parse().unwrap_or(0),
                time_writing_ms: parts[10].parse().unwrap_or(0),
                ios_in_progress: parts[11].parse().unwrap_or(0),
                time_doing_ios_ms: parts[12].parse().unwrap_or(0),
                weighted_time_ms: parts[13].parse().unwrap_or(0),
            };

            current_stats.insert(device.clone(), stats.clone());

            // Calculate rates if we have previous data
            if let Some(prev) = self.prev_stats.get(&device) {
                let time_delta_ms = now_ms.saturating_sub(self.prev_time_ms);
                if time_delta_ms > 0 {
                    let time_delta_sec = time_delta_ms as f64 / 1000.0;

                    let reads_delta = stats.reads_completed.saturating_sub(prev.reads_completed);
                    let writes_delta = stats.writes_completed.saturating_sub(prev.writes_completed);
                    let sectors_read_delta = stats.sectors_read.saturating_sub(prev.sectors_read);
                    let sectors_written_delta = stats.sectors_written.saturating_sub(prev.sectors_written);
                    let time_reading_delta = stats.time_reading_ms.saturating_sub(prev.time_reading_ms);
                    let time_writing_delta = stats.time_writing_ms.saturating_sub(prev.time_writing_ms);
                    let time_ios_delta = stats.time_doing_ios_ms.saturating_sub(prev.time_doing_ios_ms);

                    let read_bytes_per_sec = (sectors_read_delta * self.sector_size) as f64 / time_delta_sec;
                    let write_bytes_per_sec = (sectors_written_delta * self.sector_size) as f64 / time_delta_sec;
                    let read_iops = reads_delta as f64 / time_delta_sec;
                    let write_iops = writes_delta as f64 / time_delta_sec;

                    let read_latency_ms = if reads_delta > 0 {
                        time_reading_delta as f64 / reads_delta as f64
                    } else {
                        0.0
                    };

                    let write_latency_ms = if writes_delta > 0 {
                        time_writing_delta as f64 / writes_delta as f64
                    } else {
                        0.0
                    };

                    // Utilization: time_doing_ios / elapsed_time * 100
                    let utilization_percent = (time_ios_delta as f64 / time_delta_ms as f64) * 100.0;
                    let utilization_percent = utilization_percent.min(100.0);

                    // Queue depth from weighted time
                    let queue_depth = stats.weighted_time_ms as f64 / time_delta_ms as f64;

                    disks.push(DiskStats {
                        device: device.clone(),
                        read_bytes_per_sec,
                        write_bytes_per_sec,
                        read_iops,
                        write_iops,
                        read_latency_ms,
                        write_latency_ms,
                        utilization_percent,
                        queue_depth,
                        reads_completed: stats.reads_completed,
                        writes_completed: stats.writes_completed,
                        bytes_read: stats.sectors_read * self.sector_size,
                        bytes_written: stats.sectors_written * self.sector_size,
                    });
                }
            }
        }

        // Calculate totals
        let total_read = disks.iter().map(|d| d.read_bytes_per_sec).sum();
        let total_write = disks.iter().map(|d| d.write_bytes_per_sec).sum();

        // Get spill directory info
        let spill_dir_info = self.spill_dir.as_ref().and_then(|path| get_dir_info(path));

        // Update state
        self.prev_stats = current_stats;
        self.prev_time_ms = now_ms;

        Ok(DiskMetrics {
            disks,
            total_read_bytes_per_sec: total_read,
            total_write_bytes_per_sec: total_write,
            spill_dir_info,
        })
    }
}

impl Default for DiskCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn get_dir_info(path: &str) -> Option<SpillDirInfo> {
    let path = Path::new(path);
    if !path.exists() {
        return None;
    }

    // Use statvfs to get filesystem info
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::mem::MaybeUninit;

        let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let mut statvfs = MaybeUninit::<libc::statvfs>::uninit();

        let result = unsafe { libc::statvfs(c_path.as_ptr(), statvfs.as_mut_ptr()) };

        if result == 0 {
            let statvfs = unsafe { statvfs.assume_init() };
            let block_size = statvfs.f_frsize as u64;
            let total_blocks = statvfs.f_blocks as u64;
            let available_blocks = statvfs.f_bavail as u64;
            let free_blocks = statvfs.f_bfree as u64;

            let total_bytes = total_blocks * block_size;
            let available_bytes = available_blocks * block_size;
            let used_bytes = total_bytes - (free_blocks * block_size);
            let used_percent = if total_bytes > 0 {
                100.0 * used_bytes as f64 / total_bytes as f64
            } else {
                0.0
            };

            return Some(SpillDirInfo {
                path: path.to_string_lossy().to_string(),
                used_bytes,
                available_bytes,
                total_bytes,
                used_percent,
            });
        }
    }

    None
}
