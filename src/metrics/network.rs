//! Network I/O metrics collection from /proc/net/dev and related files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

/// Per-interface network statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceStats {
    /// Interface name (e.g., "eth0", "ens5")
    pub interface: String,
    /// Receive throughput in bytes per second
    pub rx_bytes_per_sec: f64,
    /// Transmit throughput in bytes per second
    pub tx_bytes_per_sec: f64,
    /// Receive packets per second
    pub rx_packets_per_sec: f64,
    /// Transmit packets per second
    pub tx_packets_per_sec: f64,
    /// Receive errors
    pub rx_errors: u64,
    /// Transmit errors
    pub tx_errors: u64,
    /// Receive drops
    pub rx_drops: u64,
    /// Transmit drops
    pub tx_drops: u64,
    /// Total bytes received
    pub rx_bytes_total: u64,
    /// Total bytes transmitted
    pub tx_bytes_total: u64,
}

/// Raw interface statistics
#[derive(Debug, Clone, Default)]
struct RawInterfaceStats {
    rx_bytes: u64,
    rx_packets: u64,
    rx_errors: u64,
    rx_drops: u64,
    tx_bytes: u64,
    tx_packets: u64,
    tx_errors: u64,
    tx_drops: u64,
}

/// TCP statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpStats {
    /// Number of established connections
    pub connections_established: u64,
    /// TCP retransmits
    pub retransmits: u64,
    /// TCP retransmits delta (for rate calculation)
    pub retransmits_delta: Option<u64>,
    /// Connections to HTTPS (port 443)
    pub https_connections: u64,
}

/// Aggregated network metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMetrics {
    /// Per-interface statistics
    pub interfaces: Vec<InterfaceStats>,
    /// Total receive throughput (bytes/sec)
    pub total_rx_bytes_per_sec: f64,
    /// Total transmit throughput (bytes/sec)
    pub total_tx_bytes_per_sec: f64,
    /// TCP statistics
    pub tcp: TcpStats,
}

/// Network metrics collector with state for rate calculations
pub struct NetworkCollector {
    prev_stats: HashMap<String, RawInterfaceStats>,
    prev_time_ms: u64,
    prev_retransmits: Option<u64>,
}

impl NetworkCollector {
    pub fn new() -> Self {
        Self {
            prev_stats: HashMap::new(),
            prev_time_ms: 0,
            prev_retransmits: None,
        }
    }

    /// Collect current network metrics
    pub fn collect(&mut self) -> Result<NetworkMetrics> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let netdev = fs::read_to_string("/proc/net/dev")
            .context("Failed to read /proc/net/dev")?;

        let mut current_stats: HashMap<String, RawInterfaceStats> = HashMap::new();
        let mut interfaces = Vec::new();

        for line in netdev.lines().skip(2) {
            // Skip header lines
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 17 {
                continue;
            }

            let interface = parts[0].trim_end_matches(':').to_string();

            // Skip loopback
            if interface == "lo" {
                continue;
            }

            let stats = RawInterfaceStats {
                rx_bytes: parts[1].parse().unwrap_or(0),
                rx_packets: parts[2].parse().unwrap_or(0),
                rx_errors: parts[3].parse().unwrap_or(0),
                rx_drops: parts[4].parse().unwrap_or(0),
                tx_bytes: parts[9].parse().unwrap_or(0),
                tx_packets: parts[10].parse().unwrap_or(0),
                tx_errors: parts[11].parse().unwrap_or(0),
                tx_drops: parts[12].parse().unwrap_or(0),
            };

            current_stats.insert(interface.clone(), stats.clone());

            // Calculate rates if we have previous data
            if let Some(prev) = self.prev_stats.get(&interface) {
                let time_delta_ms = now_ms.saturating_sub(self.prev_time_ms);
                if time_delta_ms > 0 {
                    let time_delta_sec = time_delta_ms as f64 / 1000.0;

                    let rx_bytes_delta = stats.rx_bytes.saturating_sub(prev.rx_bytes);
                    let tx_bytes_delta = stats.tx_bytes.saturating_sub(prev.tx_bytes);
                    let rx_packets_delta = stats.rx_packets.saturating_sub(prev.rx_packets);
                    let tx_packets_delta = stats.tx_packets.saturating_sub(prev.tx_packets);

                    interfaces.push(InterfaceStats {
                        interface: interface.clone(),
                        rx_bytes_per_sec: rx_bytes_delta as f64 / time_delta_sec,
                        tx_bytes_per_sec: tx_bytes_delta as f64 / time_delta_sec,
                        rx_packets_per_sec: rx_packets_delta as f64 / time_delta_sec,
                        tx_packets_per_sec: tx_packets_delta as f64 / time_delta_sec,
                        rx_errors: stats.rx_errors,
                        tx_errors: stats.tx_errors,
                        rx_drops: stats.rx_drops,
                        tx_drops: stats.tx_drops,
                        rx_bytes_total: stats.rx_bytes,
                        tx_bytes_total: stats.tx_bytes,
                    });
                }
            }
        }

        // Calculate totals
        let total_rx: f64 = interfaces.iter().map(|i| i.rx_bytes_per_sec).sum();
        let total_tx: f64 = interfaces.iter().map(|i| i.tx_bytes_per_sec).sum();

        // Get TCP stats
        let tcp = self.collect_tcp_stats()?;

        // Update state
        self.prev_stats = current_stats;
        self.prev_time_ms = now_ms;

        Ok(NetworkMetrics {
            interfaces,
            total_rx_bytes_per_sec: total_rx,
            total_tx_bytes_per_sec: total_tx,
            tcp,
        })
    }

    fn collect_tcp_stats(&mut self) -> Result<TcpStats> {
        // Count established TCP connections
        let tcp_content = fs::read_to_string("/proc/net/tcp").unwrap_or_default();
        let tcp6_content = fs::read_to_string("/proc/net/tcp6").unwrap_or_default();

        let mut established: u64 = 0;
        let mut https_connections: u64 = 0;

        for line in tcp_content.lines().chain(tcp6_content.lines()).skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                continue;
            }

            // State is in hex, 01 = ESTABLISHED
            if let Some(state) = parts.get(3) {
                if *state == "01" {
                    established += 1;

                    // Check if remote port is 443 (HTTPS)
                    // Format: local_address:port remote_address:port
                    if let Some(remote) = parts.get(2) {
                        if let Some(port_hex) = remote.split(':').last() {
                            if let Ok(port) = u16::from_str_radix(port_hex, 16) {
                                if port == 443 {
                                    https_connections += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Get retransmits from /proc/net/snmp
        let mut retransmits: u64 = 0;
        if let Ok(snmp) = fs::read_to_string("/proc/net/snmp") {
            let lines: Vec<&str> = snmp.lines().collect();
            for i in 0..lines.len() {
                if lines[i].starts_with("Tcp:") && i + 1 < lines.len() && lines[i + 1].starts_with("Tcp:") {
                    let values: Vec<&str> = lines[i + 1].split_whitespace().collect();
                    // RetransSegs is typically at index 12
                    if values.len() > 12 {
                        retransmits = values[12].parse().unwrap_or(0);
                    }
                    break;
                }
            }
        }

        let retransmits_delta = self.prev_retransmits.map(|prev| retransmits.saturating_sub(prev));
        self.prev_retransmits = Some(retransmits);

        Ok(TcpStats {
            connections_established: established,
            retransmits,
            retransmits_delta,
            https_connections,
        })
    }
}

impl Default for NetworkCollector {
    fn default() -> Self {
        Self::new()
    }
}
