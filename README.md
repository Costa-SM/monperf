# monperf

A real-time Linux performance monitoring tool with a terminal UI (TUI), designed for tracking system and process metrics during data pipeline execution.

> **Note**: This tool was developed with assistance from Large Language Models (LLMs), specifically Claude. The architecture, implementation, and documentation were created through human-AI collaboration.

## Features

### Real-time TUI Dashboard
- **CPU**: Total utilization, per-core mini-bars, load average, user/sys/iowait breakdown
- **Memory**: RAM and CGroup usage with sparkline graphs, swap, page cache, page faults
- **Disk I/O**: Per-disk utilization bars, read/write throughput with sparkline history
- **Network**: RX/TX throughput, packets/sec, TCP connections, errors/drops
- **Process**: Monitor a specific process by PID or name pattern

### Sparkline Graphs
All major sections include real-time sparkline graphs showing historical trends:
- CPU utilization over time
- Memory (CGroup and RAM) percentage
- Disk read/write throughput
- Network RX/TX throughput

### Logging
- **CSV** (`.csv`): Canonical format with all detailed metrics (per-core CPU, per-disk I/O, per-interface network)
- **Human-readable text** (`.txt`): Columnar summary format for quick review

### Advanced Metrics
- **PSI (Pressure Stall Information)**: CPU, memory, and I/O pressure metrics
- **CGroup memory**: Container/cgroup memory limits and usage
- **Page cache breakdown**: Dirty pages, writeback, active/inactive file pages
- **Process I/O**: Per-process read/write bytes from `/proc/[pid]/io`
- **Disk in-flight**: Number of I/O requests currently being processed

## Installation

### From Release
Download the pre-built binary from [Releases](https://github.com/Costa-SM/monperf/releases):

```bash
# Linux x86_64 (glibc)
wget https://github.com/Costa-SM/monperf/releases/latest/download/monperf-linux-x86_64
chmod +x monperf-linux-x86_64
./monperf-linux-x86_64
```

### From Source
```bash
git clone https://github.com/Costa-SM/monperf.git
cd monperf
cargo build --release
./target/release/monperf
```

## Usage

### Basic TUI Mode
```bash
# Launch interactive TUI
./monperf

# Monitor with logging
./monperf -l metrics.csv -o observations.txt
```

### Monitor a Specific Process
```bash
# By PID
./monperf -p 12345

# By name pattern (auto-discovers process)
./monperf -n "duckprep.py"
./monperf -n "python.*my_script"
```

### Headless Mode (No TUI)
```bash
# Run for 60 seconds, collect 60 samples
./monperf --no-tui -d 60 -l metrics.csv -o observations.txt
```

### Generate Plots from Logs
```bash
# Generate SVG plots from a CSV log file
./monperf plot metrics.csv --output-dir ./plots
```

## Command Line Options

| Option | Description |
|--------|-------------|
| `-p, --pid <PID>` | Monitor a specific process by PID |
| `-n, --name <PATTERN>` | Monitor process matching name/cmdline pattern |
| `-l, --log <FILE>` | Write detailed CSV metrics to file (canonical format) |
| `-o, --text-log <FILE>` | Write human-readable summary log to file |
| `-i, --interval <SECS>` | Sampling interval in seconds (default: 1) |
| `-d, --duration <SECS>` | Run for N seconds then exit |
| `--no-tui` | Disable TUI, print to stdout |
| `--split-on-process` | Auto-split logs when monitored process starts/ends |

## TUI Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `q` | Quit |
| `p` | Toggle process panel |
| `l` | Toggle logging |
| `r` | Reset statistics |
| `s` | Split logs (creates new log segment) |
| `y` | Confirm log split |

## Log Output Format

### CSV (metrics.csv) - Canonical Format
The CSV format is the canonical log format containing all detailed metrics:
```csv
timestamp,cpu_total_pct,cpu_user_pct,cpu_system_pct,cpu_iowait_pct,cpu_load_1m,...,cpu_core0_pct,cpu_core1_pct,...,mem_total_bytes,mem_used_bytes,...,disk_total_read_bytes_per_sec,...,disk_nvme0n1_read_bytes_per_sec,...,net_total_rx_bytes_per_sec,...,net_eth0_rx_bytes_per_sec,...,psi_cpu_some_avg10,...,proc_pid,proc_name,...
2026-01-20 12:00:00.123,45.20,30.10,15.10,2.10,1.50,...,42.50,48.30,...,17179869184,8589934592,...,1048576.00,...,524288.00,...,102400.00,...,51200.00,...,0.50,...,12345,"python",...
```

**Column groups:**
- **CPU**: Total, user, system, iowait, load average, context switches, interrupts, per-core utilization
- **Memory**: Total, used, available, buffers, cached, dirty, writeback, swap, cgroup
- **Disk**: Total read/write throughput, per-disk read/write, IOPS, latency, utilization, in-flight
- **Network**: Total RX/TX, per-interface RX/TX, packets/sec, errors, link speed, utilization
- **PSI**: CPU, memory, I/O pressure (some/full averages at 10s, 60s, 300s)
- **Process**: PID, name, state, CPU%, threads, FDs, memory breakdown, I/O rates

### Text (observations.txt) - Human-Readable Summary
```
Time      CPU%  IOW%  Mem%   CG%   Cache   Dirty  RssAnon  RssFile  ...
12:00:00  45.2   2.1  50.0  48.5   4.2G    12M      2.1G    512M   ...
12:00:01  42.8   1.8  50.1  48.6   4.2G    14M      2.1G    512M   ...
```

## Requirements

- Linux (reads from `/proc` and `/sys`)
- Terminal with Unicode support (for sparkline characters)

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test
```

## Architecture

```
src/
├── main.rs          # Entry point, TUI loop, CLI parsing
├── display.rs       # TUI rendering (ratatui widgets)
├── logging.rs       # CSV and text log writers
├── alert.rs         # Alert thresholds and checking
├── process.rs       # Process discovery and metrics
├── plot.rs          # SVG plot generation
└── metrics/
    ├── mod.rs       # Metric types and collectors
    ├── cpu.rs       # CPU metrics from /proc/stat
    ├── memory.rs    # Memory metrics from /proc/meminfo
    ├── disk.rs      # Disk I/O from /proc/diskstats
    ├── network.rs   # Network metrics from /proc/net/*
    ├── process.rs   # Per-process metrics from /proc/[pid]/*
    └── psi.rs       # PSI metrics from /proc/pressure/*
```

## License

MIT
