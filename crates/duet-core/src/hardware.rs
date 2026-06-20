//! Runtime device enumeration. The tool never assumes specs — it probes the
//! actual machine so the ModelAdvisor budgets local models against real memory
//! and degrades honestly on modest hardware.

use std::process::Command;

#[derive(Debug, Clone)]
pub struct Hardware {
    pub os: String,
    pub arch: String,
    pub chip: String,
    pub total_ram_gb: f64,
    /// Memory realistically usable for model weights + KV cache.
    pub usable_model_gb: f64,
    pub accelerator: Accelerator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Accelerator {
    Metal,  // Apple Silicon unified memory
    Cuda,   // NVIDIA dedicated VRAM
    Cpu,    // no usable GPU
}

impl Accelerator {
    pub fn label(&self) -> &'static str {
        match self {
            Accelerator::Metal => "metal",
            Accelerator::Cuda => "cuda",
            Accelerator::Cpu => "cpu",
        }
    }
}

fn sh(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Probe the local device.
pub fn detect() -> Hardware {
    let arch = std::env::consts::ARCH.to_string();
    match std::env::consts::OS {
        "macos" => detect_macos(arch),
        "linux" => detect_linux(arch),
        other => Hardware {
            os: other.to_string(),
            arch,
            chip: "unknown".into(),
            total_ram_gb: 0.0,
            usable_model_gb: 0.0,
            accelerator: Accelerator::Cpu,
        },
    }
}

fn detect_macos(arch: String) -> Hardware {
    let total_bytes = sh("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let total_gb = total_bytes / 1024.0 / 1024.0 / 1024.0;
    let apple_silicon = arch == "aarch64" || arch == "arm64";
    let chip = sh("sysctl", &["-n", "machdep.cpu.brand_string"])
        .filter(|s| !s.is_empty())
        .or_else(|| {
            sh("system_profiler", &["SPHardwareDataType"]).and_then(|s| {
                s.lines().find(|l| l.contains("Chip") || l.contains("Processor Name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
            })
        })
        .unwrap_or_else(|| "Apple".into());
    let (accelerator, usable) = if apple_silicon {
        // Unified memory: macOS lets the GPU wire a large fraction. Budget ~70%
        // for weights, leaving headroom for OS + KV cache.
        (Accelerator::Metal, total_gb * 0.70)
    } else {
        (Accelerator::Cpu, total_gb * 0.50)
    };
    Hardware { os: "macos".into(), arch, chip, total_ram_gb: total_gb, usable_model_gb: usable, accelerator }
}

fn detect_linux(arch: String) -> Hardware {
    let total_gb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<f64>().ok())
                .map(|kb| kb / 1024.0 / 1024.0)
        })
        .unwrap_or(0.0);
    // NVIDIA VRAM via nvidia-smi
    if let Some(out) = sh("nvidia-smi", &["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]) {
        if let Some(first) = out.lines().next() {
            let mut parts = first.split(',');
            let name = parts.next().unwrap_or("GPU").trim().to_string();
            let vram_mb = parts.next().and_then(|m| m.trim().parse::<f64>().ok()).unwrap_or(0.0);
            let vram_gb = vram_mb / 1024.0;
            return Hardware {
                os: "linux".into(),
                arch,
                chip: name,
                total_ram_gb: total_gb,
                usable_model_gb: vram_gb * 0.92,
                accelerator: Accelerator::Cuda,
            };
        }
    }
    Hardware { os: "linux".into(), arch, chip: "CPU".into(), total_ram_gb: total_gb, usable_model_gb: total_gb * 0.55, accelerator: Accelerator::Cpu }
}

impl Hardware {
    pub fn summary(&self) -> String {
        format!(
            "{} / {} · {} · {:.0} GB RAM · ~{:.0} GB usable for models · accelerator: {}",
            self.os, self.arch, self.chip.trim(), self.total_ram_gb, self.usable_model_gb, self.accelerator.label()
        )
    }
}
