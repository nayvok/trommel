//! GPU metrics through `nvidia-smi` (NVIDIA only in v1).

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GpuStats {
    pub index: u32,
    /// Stable NVIDIA UUID (`GPU-…`); used for all OC operations.
    pub uuid: String,
    pub name: String,
    pub temp_c: Option<u32>,
    pub fan_pct: Option<u32>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub power_default_w: Option<f64>,
    pub power_min_w: Option<f64>,
    pub power_max_w: Option<f64>,
    pub core_mhz: Option<u32>,
    pub mem_mhz: Option<u32>,
    pub util_pct: Option<u32>,
    pub vram_used_mb: Option<u64>,
    /// PCI bus id from `nvidia-smi` (`00000000:01:00.0`). Together with the
    /// miners' numeric bus id it maps per-card miner data to a physical card.
    #[serde(default)]
    pub pci_bus_id: Option<String>,
}

const QUERY: &str = "index,uuid,name,temperature.gpu,fan.speed,power.draw,power.limit,power.default_limit,power.min_limit,power.max_limit,clocks.sm,clocks.mem,utilization.gpu,memory.used,pci.bus_id";

/// Read metrics of every GPU. Returns the `nvidia-smi` error text when the
/// driver is not reachable (missing permissions, no NVIDIA card, sandbox).
pub fn query_all() -> anyhow::Result<Vec<GpuStats>> {
    let output = Command::new("nvidia-smi")
        .env("LC_ALL", "C")
        .args(["--query-gpu", QUERY, "--format=csv,noheader,nounits"])
        .output()
        .map_err(|e| anyhow::anyhow!("nvidia-smi is not runnable: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        anyhow::bail!("{}", first_line(&msg));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        gpus.push(parse_line(line)?);
    }
    if gpus.is_empty() {
        anyhow::bail!("nvidia-smi returned no GPUs");
    }
    Ok(gpus)
}

/// First GPU, kept for compatibility with older call sites.
pub fn query() -> anyhow::Result<GpuStats> {
    query_all()?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no NVIDIA GPU found"))
}

/// Parse one CSV line produced with `--format=csv,noheader,nounits`.
pub fn parse_line(line: &str) -> anyhow::Result<GpuStats> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() < 14 {
        anyhow::bail!("unexpected nvidia-smi response: {line:?}");
    }
    Ok(GpuStats {
        index: parse_u32(fields[0]).unwrap_or(0),
        uuid: fields[1].to_string(),
        name: fields[2].to_string(),
        temp_c: parse_u32(fields[3]),
        fan_pct: parse_u32(fields[4]),
        power_w: parse_f64(fields[5]),
        power_limit_w: parse_f64(fields[6]),
        power_default_w: parse_f64(fields[7]),
        power_min_w: parse_f64(fields[8]),
        power_max_w: parse_f64(fields[9]),
        core_mhz: parse_u32(fields[10]),
        mem_mhz: parse_u32(fields[11]),
        util_pct: parse_u32(fields[12]),
        vram_used_mb: parse_u64(fields[13]),
        pci_bus_id: fields.get(14).map(|field| field.trim().to_string()),
    })
}

/// Power envelope of the first GPU, best effort.
pub fn power_range() -> Option<(f64, f64)> {
    let gpu = query().ok()?;
    Some((gpu.power_min_w?, gpu.power_max_w?))
}

fn parse_u32(field: &str) -> Option<u32> {
    parse_f64(field).map(|v| v.round().clamp(0.0, u32::MAX as f64) as u32)
}

fn parse_u64(field: &str) -> Option<u64> {
    parse_f64(field).map(|v| v.round().max(0.0) as u64)
}

fn parse_f64(field: &str) -> Option<f64> {
    let field = field.trim();
    if field.is_empty()
        || field.eq_ignore_ascii_case("n/a")
        || field.starts_with('[')
        || field.contains("Not Supported")
    {
        return None;
    }
    field.parse::<f64>().ok()
}

/// Decimal PCI bus number of an `nvidia-smi` `pci.bus_id`
/// (`00000000:01:00.0` → 1). Miners report this number for their devices.
pub fn pci_bus_number(bus_id: &str) -> Option<u32> {
    let parts: Vec<&str> = bus_id.trim().split(':').collect();
    let bus = match parts.as_slice() {
        [_, bus, _, ..] => *bus,
        [bus, _] => *bus,
        [bus] => *bus,
        _ => return None,
    };
    u32::from_str_radix(bus, 16).ok()
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("nvidia-smi returned an error")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_line() {
        let stats = parse_line(
            "0, GPU-12345678-1234-1234-1234-123456789abc, NVIDIA GeForce RTX 3080 Ti, 67, 54, 295.34, 300.00, 350.00, 100.00, 400.00, 1620, 810, 100, 5120",
        )
        .unwrap();
        assert_eq!(stats.index, 0);
        assert_eq!(stats.uuid, "GPU-12345678-1234-1234-1234-123456789abc");
        assert_eq!(stats.name, "NVIDIA GeForce RTX 3080 Ti");
        assert_eq!(stats.temp_c, Some(67));
        assert_eq!(stats.fan_pct, Some(54));
        assert_eq!(stats.power_w, Some(295.34));
        assert_eq!(stats.power_limit_w, Some(300.0));
        assert_eq!(stats.power_default_w, Some(350.0));
        assert_eq!(stats.power_min_w, Some(100.0));
        assert_eq!(stats.power_max_w, Some(400.0));
        assert_eq!(stats.core_mhz, Some(1620));
        assert_eq!(stats.mem_mhz, Some(810));
        assert_eq!(stats.util_pct, Some(100));
        assert_eq!(stats.vram_used_mb, Some(5120));
        assert_eq!(stats.pci_bus_id, None);
    }

    #[test]
    fn parses_the_pci_bus_id_and_converts_it_to_decimal() {
        let stats = parse_line(
            "0, GPU-12345678-1234-1234-1234-123456789abc, NVIDIA GeForce RTX 3080 Ti, 67, 54, 295.34, 300.00, 350.00, 100.00, 400.00, 1620, 810, 100, 5120, 00000000:01:00.0",
        )
        .unwrap();
        assert_eq!(stats.pci_bus_id.as_deref(), Some("00000000:01:00.0"));
        assert_eq!(pci_bus_number("00000000:01:00.0"), Some(1));
        assert_eq!(pci_bus_number("00000000:0a:00.0"), Some(10));
        assert_eq!(pci_bus_number("01:00.0"), Some(1));
        assert_eq!(pci_bus_number("not-a-bus"), None);
    }

    #[test]
    fn tolerates_missing_values() {
        let stats = parse_line(
            "0, GPU-abc, RTX 3080 Ti, 45, [N/A], 120.00, [N/A], [N/A], [N/A], [N/A], 1500, 810, 0, 300",
        )
        .unwrap();
        assert_eq!(stats.temp_c, Some(45));
        assert_eq!(stats.fan_pct, None);
        assert_eq!(stats.power_limit_w, None);
        assert_eq!(stats.power_default_w, None);
        assert_eq!(stats.vram_used_mb, Some(300));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_line("some error").is_err());
    }
}
