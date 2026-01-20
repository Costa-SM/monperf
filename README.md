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
- **JSON Lines** (`.jsonl`): Machine-readable format with all metrics for post-analysis
- **Human-readable text** (`.txt`): Columnar format for quick review

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
./monperf -l metrics.json -o observations.txt
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
./monperf --no-tui -d 60 -l metrics.json -o observations.txt
```

### Generate Plots from Logs
```bash
# Generate SVG plots from a JSON log file
./monperf plot metrics.json --output-dir ./plots
```

## Command Line Options

| Option | Description |
|--------|-------------|
| `-p, --pid <PID>` | Monitor a specific process by PID |
| `-n, --name <PATTERN>` | Monitor process matching name/cmdline pattern |
| `-l, --log <FILE>` | Write JSON metrics to file |
| `-o, --observations <FILE>` | Write human-readable log to file |
| `-i, --interval <MS>` | Sampling interval in milliseconds (default: 1000) |
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

### JSON (metrics.json)
Each line is a complete JSON object with all metrics:
```json
{
  "timestamp": "2026-01-20T12:00:00Z",
  "cpu": { "total_utilization": 45.2, "user_percent": 30.1, ... },
  "memory": { "used": 8589934592, "total": 17179869184, ... },
  "disk": { "total_read_bytes_per_sec": 1048576, ... },
  "network": { "total_rx_bytes_per_sec": 102400, ... },
  "process": { "pid": 12345, "rss_bytes": 104857600, ... },
  "psi": { "cpu": { "some_avg10": 0.5 }, "memory": { ... }, "io": { ... } }
}
```

### Text (observations.txt)
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
├── logging.rs       # JSON and text log writers
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
