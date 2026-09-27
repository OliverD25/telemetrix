//! GPU readings from the kernel's files on Linux, for the GPUs whose driver
//! reports a load there (amdgpu; also some newer Intel drivers):
//! `/sys/class/drm/card*/device/gpu_busy_percent`, the video memory in
//! `mem_info_vram_used` and `mem_info_vram_total`, and the temperature of
//! the card's hwmon sensor. Reading a few small files costs no memory.
//! NVIDIA's own driver does not write these files; use `gpu.source = "nvml"`.

use std::path::{Path, PathBuf};

use super::GpuMetric;

struct Card {
    name: String,
    device: PathBuf,
    /// `hwmon/hwmon*/temp1_input`, in thousandths of a degree.
    temp: Option<PathBuf>,
}

pub struct Sysfs {
    cards: Vec<Card>,
}

fn number(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn text(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `card0`, `card1`, ... but not the connectors like `card0-DP-1`.
fn is_card(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn card_name(device: &Path, card: &str) -> String {
    if let Some(name) = text(&device.join("product_name")) {
        return name;
    }
    let vendor = match text(&device.join("vendor")).as_deref() {
        Some("0x1002") => "AMD GPU",
        Some("0x8086") => "Intel GPU",
        Some("0x10de") => "NVIDIA GPU",
        _ => "GPU",
    };
    format!("{vendor} ({card})")
}

impl Sysfs {
    pub fn load() -> Result<Self, String> {
        Self::load_from(Path::new("/sys/class/drm"))
    }

    /// Every card under `root` that reports a load. `Err` when none does.
    pub fn load_from(root: &Path) -> Result<Self, String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .map_err(|e| format!("cannot read {}: {e}", root.display()))?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| is_card(n))
            .collect();
        names.sort_by_key(|n| n[4..].parse::<u32>().unwrap_or(u32::MAX));
        let cards: Vec<Card> = names
            .iter()
            .map(|n| (n, root.join(n).join("device")))
            .filter(|(_, device)| device.join("gpu_busy_percent").is_file())
            .map(|(n, device)| {
                let temp = std::fs::read_dir(device.join("hwmon"))
                    .ok()
                    .and_then(|dirs| {
                        let mut dirs: Vec<PathBuf> =
                            dirs.filter_map(Result::ok).map(|e| e.path()).collect();
                        dirs.sort();
                        dirs.into_iter()
                            .map(|d| d.join("temp1_input"))
                            .find(|t| t.is_file())
                    });
                Card {
                    name: card_name(&device, n),
                    device,
                    temp,
                }
            })
            .collect();
        if cards.is_empty() {
            return Err(
                "no GPU reports its load in /sys/class/drm (NVIDIA: gpu.source = \"nvml\")".into(),
            );
        }
        Ok(Self { cards })
    }

    pub fn names(&self) -> Vec<&str> {
        self.cards.iter().map(|c| c.name.as_str()).collect()
    }

    pub fn read(&self) -> Vec<GpuMetric> {
        self.cards
            .iter()
            .map(|c| GpuMetric {
                name: c.name.clone(),
                usage_pct: number(&c.device.join("gpu_busy_percent")).map(|p| p.min(100) as f32),
                mem_used_bytes: number(&c.device.join("mem_info_vram_used")),
                mem_total_bytes: number(&c.device.join("mem_info_vram_total")),
                temp_c: c
                    .temp
                    .as_deref()
                    .and_then(number)
                    .map(|m| m as f32 / 1000.0),
                power_w: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_the_cards_that_report_a_load() {
        let root = std::env::temp_dir().join(format!("telemetrix-drm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let amd = root.join("card1").join("device");
        write(&amd.join("gpu_busy_percent"), "37\n");
        write(&amd.join("vendor"), "0x1002\n");
        write(&amd.join("mem_info_vram_used"), "536870912\n");
        write(&amd.join("mem_info_vram_total"), "8589934592\n");
        write(
            &amd.join("hwmon").join("hwmon3").join("temp1_input"),
            "51000\n",
        );
        // A card without a load file (a plain display adapter) is left out.
        write(
            &root.join("card0").join("device").join("vendor"),
            "0x8086\n",
        );
        write(&root.join("card1-DP-1").join("status"), "connected\n");
        let named = root.join("card2").join("device");
        write(&named.join("gpu_busy_percent"), "180");
        write(&named.join("product_name"), "Example Graphics 9000\n");

        let gpus = Sysfs::load_from(&root).unwrap();
        assert_eq!(gpus.names(), ["AMD GPU (card1)", "Example Graphics 9000"]);
        let r = gpus.read();
        assert_eq!(r[0].usage_pct, Some(37.0));
        assert_eq!(r[0].mem_used_bytes, Some(512 << 20));
        assert_eq!(r[0].mem_total_bytes, Some(8 << 30));
        assert_eq!(r[0].temp_c, Some(51.0));
        assert_eq!(r[1].usage_pct, Some(100.0), "capped");
        assert_eq!((r[1].mem_total_bytes, r[1].temp_c), (None, None));

        std::fs::remove_dir_all(root.join("card1")).unwrap();
        std::fs::remove_dir_all(root.join("card2")).unwrap();
        assert!(Sysfs::load_from(&root).is_err(), "no card reports a load");
        std::fs::remove_dir_all(&root).unwrap();
        assert!(Sysfs::load_from(&root).is_err(), "no folder");
    }
}
