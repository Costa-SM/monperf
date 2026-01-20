//! Process-specific metrics collection from /proc/[pid]/ files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Process state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProcessState {
    Running,
    Sleeping,
    DiskSleep, // Uninterruptible sleep (waiting for I/O)
    Stopped,
    Zombie,
    Dead,
    Unknown,
}

impl std::fmt::Display for ProcessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessState::Running => write!(f, "Running"),
            ProcessState::Sleeping => write!(f, "Sleeping"),
            ProcessState::DiskSleep => write!(f, "Disk Sleep"),
            ProcessState::Stopped => write!(f, "Stopped"),
            ProcessState::Zombie => write!(f, "Zombie"),
            ProcessState::Dead => write!(f, "Dead"),
            ProcessState::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Process metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMetrics {
    /// Process ID
    pub pid: u32,
    /// Process name
    pub name: String,
    /// Process state
    pub state: ProcessState,
    /// Resident Set Size (physical memory) in bytes
    pub rss_bytes: u64,
    /// Virtual memory size in bytes
    pub vsize_bytes: u64,
    /// Peak virtual memory size in bytes
    pub vm_peak: u64,
    /// Anonymous memory (heap, stack, allocations) in bytes
    pub rss_anon: u64,
    /// File-backed memory (mmap'd files) in bytes
    pub rss_file: u64,
    /// Shared memory segments in bytes
    pub rss_shmem: u64,
    /// Swapped out memory in bytes
    pub vm_swap: u64,
    /// CPU usage percentage (requires delta calculation)
    pub cpu_percent: f64,
    /// User CPU time in ticks
    pub utime: u64,
    /// System CPU time in ticks
    pub stime: u64,
    /// Number of threads
    pub num_threads: u64,
    /// Number of open file descriptors
    pub num_fds: u64,
    /// Process command line
    pub cmdline: String,
    // I/O counters from /proc/[pid]/io
    /// Bytes read from storage (actual disk reads)
    pub io_read_bytes: u64,
    /// Bytes written to storage
    pub io_write_bytes: u64,
    /// Bytes read (includes cache hits)
    pub io_rchar: u64,
    /// Bytes written (includes buffered)
    pub io_wchar: u64,
    /// Cancelled write bytes
    pub io_cancelled_write_bytes: u64,
    /// Read bytes delta (per second)
    pub io_read_bytes_per_sec: f64,
    /// Write bytes delta (per second)
    pub io_write_bytes_per_sec: f64,
}

/// Process metrics collector with state for CPU and I/O calculation
pub struct ProcessCollector {
    pid: u32,
    prev_utime: Option<u64>,
    prev_stime: Option<u64>,
    prev_io_read_bytes: Option<u64>,
    prev_io_write_bytes: Option<u64>,
    prev_time_ms: u64,
    clock_ticks_per_sec: u64,
}

impl ProcessCollector {
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            prev_utime: None,
            prev_stime: None,
            prev_io_read_bytes: None,
            prev_io_write_bytes: None,
            prev_time_ms: 0,
            clock_ticks_per_sec: unsafe { libc::sysconf(libc::_SC_CLK_TCK) as u64 },
        }
    }

    /// Check if the process exists
    pub fn exists(&self) -> bool {
        Path::new(&format!("/proc/{}", self.pid)).exists()
    }

    /// Collect current process metrics
    pub fn collect(&mut self) -> Result<ProcessMetrics> {
        let proc_path = format!("/proc/{}", self.pid);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Read /proc/[pid]/stat
        let stat_content = fs::read_to_string(format!("{}/stat", proc_path))
            .context("Failed to read process stat")?;

        // Parse stat - format: pid (comm) state fields...
        // The comm field can contain spaces, so we need to find the last ')' first
        let comm_end = stat_content.rfind(')').context("Invalid stat format")?;
        let comm_start = stat_content.find('(').context("Invalid stat format")?;

        let name = stat_content[comm_start + 1..comm_end].to_string();
        let fields: Vec<&str> = stat_content[comm_end + 2..].split_whitespace().collect();

        let state = match fields.first().map(|s| s.chars().next()) {
            Some(Some('R')) => ProcessState::Running,
            Some(Some('S')) => ProcessState::Sleeping,
            Some(Some('D')) => ProcessState::DiskSleep,
            Some(Some('T')) => ProcessState::Stopped,
            Some(Some('Z')) => ProcessState::Zombie,
            Some(Some('X')) => ProcessState::Dead,
            _ => ProcessState::Unknown,
        };

        // Fields are 0-indexed after state
        // utime = field 11 (14th overall), stime = field 12 (15th overall)
        // num_threads = field 17 (20th overall), vsize = field 20 (23rd overall)
        // rss = field 21 (24th overall) - in pages
        let utime: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
        let stime: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
        let num_threads: u64 = fields.get(17).and_then(|s| s.parse().ok()).unwrap_or(0);
        let vsize_bytes: u64 = fields.get(20).and_then(|s| s.parse().ok()).unwrap_or(0);
        let rss_pages: u64 = fields.get(21).and_then(|s| s.parse().ok()).unwrap_or(0);

        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 };
        let rss_bytes = rss_pages * page_size;

        // Calculate CPU percentage
        let cpu_percent = if let (Some(prev_utime), Some(prev_stime)) = (self.prev_utime, self.prev_stime) {
            let time_delta_ms = now_ms.saturating_sub(self.prev_time_ms);
            if time_delta_ms > 0 {
                let cpu_delta = (utime + stime).saturating_sub(prev_utime + prev_stime);
                let cpu_seconds = cpu_delta as f64 / self.clock_ticks_per_sec as f64;
                let elapsed_seconds = time_delta_ms as f64 / 1000.0;
                (cpu_seconds / elapsed_seconds) * 100.0
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Count file descriptors
        let num_fds = fs::read_dir(format!("{}/fd", proc_path))
            .map(|entries| entries.count() as u64)
            .unwrap_or(0);

        // Read command line
        let cmdline = fs::read_to_string(format!("{}/cmdline", proc_path))
            .unwrap_or_default()
            .replace('\0', " ")
            .trim()
            .to_string();

        // Read /proc/[pid]/status for memory breakdown
        let (vm_peak, rss_anon, rss_file, rss_shmem, vm_swap) = 
            read_process_status(&proc_path);

        // Read /proc/[pid]/io for I/O counters
        let (io_read_bytes, io_write_bytes, io_rchar, io_wchar, io_cancelled_write_bytes) = 
            read_process_io(&proc_path);

        // Calculate I/O rates
        let time_delta_secs = now_ms.saturating_sub(self.prev_time_ms) as f64 / 1000.0;
        let io_read_bytes_per_sec = if time_delta_secs > 0.0 {
            if let Some(prev) = self.prev_io_read_bytes {
                io_read_bytes.saturating_sub(prev) as f64 / time_delta_secs
            } else {
                0.0
            }
        } else {
            0.0
        };
        let io_write_bytes_per_sec = if time_delta_secs > 0.0 {
            if let Some(prev) = self.prev_io_write_bytes {
                io_write_bytes.saturating_sub(prev) as f64 / time_delta_secs
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Update state
        self.prev_utime = Some(utime);
        self.prev_stime = Some(stime);
        self.prev_io_read_bytes = Some(io_read_bytes);
        self.prev_io_write_bytes = Some(io_write_bytes);
        self.prev_time_ms = now_ms;

        Ok(ProcessMetrics {
            pid: self.pid,
            name,
            state,
            rss_bytes,
            vsize_bytes,
            vm_peak,
            rss_anon,
            rss_file,
            rss_shmem,
            vm_swap,
            cpu_percent,
            utime,
            stime,
            num_threads,
            num_fds,
            cmdline,
            io_read_bytes,
            io_write_bytes,
            io_rchar,
            io_wchar,
            io_cancelled_write_bytes,
            io_read_bytes_per_sec,
            io_write_bytes_per_sec,
        })
    }
}

/// Read memory breakdown from /proc/[pid]/status
fn read_process_status(proc_path: &str) -> (u64, u64, u64, u64, u64) {
    let status = fs::read_to_string(format!("{}/status", proc_path)).unwrap_or_default();
    
    let mut vm_peak: u64 = 0;
    let mut rss_anon: u64 = 0;
    let mut rss_file: u64 = 0;
    let mut rss_shmem: u64 = 0;
    let mut vm_swap: u64 = 0;
    
    for line in status.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        // Values in /proc/[pid]/status are in kB
        let value: u64 = parts[1].parse().unwrap_or(0) * 1024;
        
        match parts[0] {
            "VmPeak:" => vm_peak = value,
            "RssAnon:" => rss_anon = value,
            "RssFile:" => rss_file = value,
            "RssShmem:" => rss_shmem = value,
            "VmSwap:" => vm_swap = value,
            _ => {}
        }
    }
    
    (vm_peak, rss_anon, rss_file, rss_shmem, vm_swap)
}

/// Read I/O counters from /proc/[pid]/io
fn read_process_io(proc_path: &str) -> (u64, u64, u64, u64, u64) {
    let io = fs::read_to_string(format!("{}/io", proc_path)).unwrap_or_default();
    
    let mut read_bytes: u64 = 0;
    let mut write_bytes: u64 = 0;
    let mut rchar: u64 = 0;
    let mut wchar: u64 = 0;
    let mut cancelled_write_bytes: u64 = 0;
    
    for line in io.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let value: u64 = parts[1].parse().unwrap_or(0);
        
        match parts[0] {
            "read_bytes:" => read_bytes = value,
            "write_bytes:" => write_bytes = value,
            "rchar:" => rchar = value,
            "wchar:" => wchar = value,
            "cancelled_write_bytes:" => cancelled_write_bytes = value,
            _ => {}
        }
    }
    
    (read_bytes, write_bytes, rchar, wchar, cancelled_write_bytes)
}

/// Find a process by name or command-line pattern (returns best match)
/// Matches against both /proc/PID/comm and /proc/PID/cmdline
/// Excludes perf-monitor processes to avoid matching ourselves
pub fn find_process_by_name(pattern: &str) -> Option<u32> {
    let proc_dir = Path::new("/proc");
    let pattern_lower = pattern.to_lowercase();
    let my_pid = std::process::id();
    
    // Collect all matching PIDs with their cmdlines and a priority score
    // Higher score = better match
    let mut matches: Vec<(u32, String, i32)> = Vec::new();
    
    if let Ok(entries) = fs::read_dir(proc_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if let Ok(pid) = filename.parse::<u32>() {
                    // Skip our own process
                    if pid == my_pid {
                        continue;
                    }
                    
                    let cmdline_path = path.join("cmdline");
                    let cmdline = fs::read_to_string(&cmdline_path).unwrap_or_default();
                    let cmdline_clean = cmdline.replace('\0', " ");
                    let cmdline_lower = cmdline_clean.to_lowercase();
                    
                    // Skip perf-monitor processes (including other instances)
                    if cmdline_lower.contains("perf-monitor") {
                        continue;
                    }
                    
                    // Skip shell processes (bash, zsh, sh) unless pattern explicitly matches
                    let comm_path = path.join("comm");
                    let comm = fs::read_to_string(&comm_path).unwrap_or_default();
                    let comm_trimmed = comm.trim().to_lowercase();
                    
                    if (comm_trimmed == "bash" || comm_trimmed == "zsh" || comm_trimmed == "sh") 
                        && !pattern_lower.contains("bash") 
                        && !pattern_lower.contains("zsh")
                        && !pattern_lower.contains("sh") {
                        continue;
                    }
                    
                    // Check for matches and assign priority
                    let mut score = 0;
                    
                    // Exact comm match is highest priority
                    if comm_trimmed == pattern_lower {
                        return Some(pid); // Return immediately for exact match
                    }
                    
                    // Check cmdline for pattern
                    if !cmdline_lower.contains(&pattern_lower) {
                        continue;
                    }
                    
                    // Get the first argument (the executable/script)
                    let first_arg = cmdline_clean.split_whitespace().next().unwrap_or("");
                    let first_arg_lower = first_arg.to_lowercase();
                    
                    // Highest priority: pattern is in the first argument (executable name)
                    if first_arg_lower.contains(&pattern_lower) {
                        score += 100;
                    }
                    
                    // High priority: pattern matches a .py file and this is a python process
                    if pattern_lower.ends_with(".py") && 
                       (comm_trimmed == "python" || comm_trimmed.starts_with("python")) {
                        score += 50;
                    }
                    
                    // Medium priority: not a wrapper script
                    if !first_arg_lower.contains("bash") && !first_arg_lower.contains("/sh") {
                        score += 10;
                    }
                    
                    matches.push((pid, cmdline_clean, score));
                }
            }
        }
    }
    
    // Return the match with highest score, or highest PID as tiebreaker (most recent)
    matches.into_iter()
        .max_by_key(|(pid, _, score)| (*score, *pid))
        .map(|(pid, _, _)| pid)
}

/// List all processes matching a name pattern
pub fn find_processes_by_pattern(pattern: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    let proc_dir = Path::new("/proc");
    let pattern_lower = pattern.to_lowercase();

    if let Ok(entries) = fs::read_dir(proc_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if let Ok(pid) = filename.parse::<u32>() {
                    // Check comm file
                    let comm_path = path.join("comm");
                    if let Ok(comm) = fs::read_to_string(&comm_path) {
                        if comm.trim().to_lowercase().contains(&pattern_lower) {
                            pids.push(pid);
                            continue;
                        }
                    }
                    // Check cmdline
                    let cmdline_path = path.join("cmdline");
                    if let Ok(cmdline) = fs::read_to_string(&cmdline_path) {
                        if cmdline.to_lowercase().contains(&pattern_lower) {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
    }
    pids
}
