//! GPU metrics through `nvidia-smi`.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GpuStats {
    pub name: String,
    pub temp_c: Option<u32>,
    pub fan_pct: Option<u32>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub power_min_w: Option<f64>,
    pub power_max_w: Option<f64>,
    pub core_mhz: Option<u32>,
    pub mem_mhz: Option<u32>,
    pub util_pct: Option<u32>,
    pub vram_used_mb: Option<u64>,
}

const QUERY: &str = "name,temperature.gpu,fan.speed,power.draw,power.limit,clocks.sm,clocks.mem,utilization.gpu,memory.used";

/// Read metrics of GPU 0. Returns the error text of `nvidia-smi` when the
/// driver is not reachable (missing permissions, no NVIDIA card, sandbox).
pub fn query() -> anyhow::Result<GpuStats> {
    let output = Command::new("nvidia-smi")
        .env("LC_ALL", "C")
        .args([
            "--query-gpu",
            QUERY,
            "--format=csv,noheader,nounits",
            "-i",
            "0",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("nvidia-smi не запускается: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        anyhow::bail!("{}", first_line(&msg));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or_default();
    parse_line(line)
}

/// Parse one CSV line produced with `--format=csv,noheader,nounits`.
pub fn parse_line(line: &str) -> anyhow::Result<GpuStats> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() < 9 {
        anyhow::bail!("неожиданный ответ nvidia-smi: {line:?}");
    }
    Ok(GpuStats {
        name: fields[0].to_string(),
        temp_c: parse_u32(fields[1]),
        fan_pct: parse_u32(fields[2]),
        power_w: parse_f64(fields[3]),
        power_limit_w: parse_f64(fields[4]),
        power_min_w: None,
        power_max_w: None,
        core_mhz: parse_u32(fields[5]),
        mem_mhz: parse_u32(fields[6]),
        util_pct: parse_u32(fields[7]),
        vram_used_mb: parse_u64(fields[8]),
    })
}

/// Power envelope of GPU 0 (`power.min_limit` / `power.max_limit`), best effort.
pub fn power_range() -> Option<(f64, f64)> {
    let output = Command::new("nvidia-smi")
        .env("LC_ALL", "C")
        .args([
            "--query-gpu=power.min_limit,power.max_limit",
            "--format=csv,noheader,nounits",
            "-i",
            "0",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next()?;
    let mut parts = line.split(',').map(str::trim);
    let min = parse_f64(parts.next()?)?;
    let max = parse_f64(parts.next()?)?;
    Some((min, max))
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

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("nvidia-smi вернул ошибку")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_line() {
        let stats =
            parse_line("NVIDIA GeForce RTX 3080 Ti, 67, 54, 295.34, 300.00, 1620, 810, 100, 5120")
                .unwrap();
        assert_eq!(stats.name, "NVIDIA GeForce RTX 3080 Ti");
        assert_eq!(stats.temp_c, Some(67));
        assert_eq!(stats.fan_pct, Some(54));
        assert_eq!(stats.power_w, Some(295.34));
        assert_eq!(stats.power_limit_w, Some(300.0));
        assert_eq!(stats.core_mhz, Some(1620));
        assert_eq!(stats.mem_mhz, Some(810));
        assert_eq!(stats.util_pct, Some(100));
        assert_eq!(stats.vram_used_mb, Some(5120));
    }

    #[test]
    fn tolerates_missing_values() {
        let stats = parse_line("RTX 3080 Ti, 45, [N/A], 120.00, [N/A], 1500, 810, 0, 300").unwrap();
        assert_eq!(stats.temp_c, Some(45));
        assert_eq!(stats.fan_pct, None);
        assert_eq!(stats.power_limit_w, None);
        assert_eq!(stats.vram_used_mb, Some(300));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_line("some error").is_err());
    }
}
