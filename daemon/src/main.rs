use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};
use zbus::{connection, interface};

/// Parse dmem.capacity → first DRM device key + VRAM bytes.
/// Format per line: `drm/0000:2d:00.0/vram 8573157376`
fn read_dmem_capacity() -> Option<(String, u64)> {
    let content = fs::read_to_string("/sys/fs/cgroup/dmem.capacity").ok()?;
    for line in content.lines() {
        let mut parts = line.splitn(2, ' ');
        let key = parts.next()?.trim();
        let val: u64 = parts.next()?.trim().parse().ok()?;
        if key.starts_with("drm/") && val > 0 {
            return Some((key.to_string(), val));
        }
    }
    None
}

/// Read /proc/{pid}/cgroup → cgroup v2 path under /sys/fs/cgroup.
fn cgroup_path_for_pid(pid: u32) -> Option<String> {
    let text = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    for line in text.lines() {
        if let Some(rel) = line.strip_prefix("0::") {
            return Some(format!("/sys/fs/cgroup{}", rel.trim()));
        }
    }
    None
}

/// Write dmem.low to cgroup dir. Returns Ok(false) if file doesn't exist.
fn write_dmem_low(cgroup_dir: &str, drm_key: &str, bytes: u64) -> std::io::Result<bool> {
    let file = format!("{cgroup_dir}/dmem.low");
    if !Path::new(&file).exists() {
        return Ok(false);
    }
    fs::write(&file, format!("{drm_key} {bytes}\n"))?;
    Ok(true)
}

/// Only boost app-level scopes/services under app.slice.
/// Skips session-level units (user@1000.service root, session.slice services).
fn is_app_scope(cgroup_dir: &str) -> bool {
    cgroup_dir.contains("/app.slice/")
}

/// Friendly display name: last path component of cgroup dir.
fn unit_label(cgroup_dir: &str) -> &str {
    cgroup_dir.rsplit('/').next().unwrap_or(cgroup_dir)
}

struct Inner {
    prev_cgroup: Option<String>,
    current_unit: String,
    drm_key: String,
    vram_total: u64,
}

impl Inner {
    async fn handle_focus(&mut self, pid: u32) {
        let cgroup = match cgroup_path_for_pid(pid) {
            Some(p) => p,
            None => {
                warn!("Cannot read cgroup for pid={pid}");
                return;
            }
        };

        if !is_app_scope(&cgroup) {
            info!("pid={pid} skip (not app.slice): {}", unit_label(&cgroup));
            return;
        }

        let label = unit_label(&cgroup).to_string();
        info!("focus pid={pid} → {label}");

        // Revert previous
        if let Some(ref prev) = self.prev_cgroup.clone() {
            if *prev != cgroup {
                match write_dmem_low(prev, &self.drm_key, 0) {
                    Ok(true) => info!("dmem.low=0 ← {}", unit_label(prev)),
                    Ok(false) => warn!("dmem.low missing (scope gone?): {}", unit_label(prev)),
                    Err(e) => warn!("revert failed for {}: {e}", unit_label(prev)),
                }
            }
        }

        // Boost
        let total = self.vram_total;
        match write_dmem_low(&cgroup, &self.drm_key, total) {
            Ok(true) => info!("dmem.low={total} → {label}"),
            Ok(false) => warn!("dmem.low missing — is dmemcg-booster running? ({label})"),
            Err(e) => warn!("boost failed for {label}: {e}"),
        }

        self.prev_cgroup = Some(cgroup);
        self.current_unit = label;
    }
}

struct VramBoosterService {
    inner: Arc<Mutex<Inner>>,
}

#[interface(name = "org.gnome.VramBooster")]
impl VramBoosterService {
    async fn focus_changed(&self, pid: u32) {
        self.inner.lock().await.handle_focus(pid).await;
    }

    #[zbus(property)]
    async fn current_unit(&self) -> String {
        self.inner.lock().await.current_unit.clone()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let (drm_key, vram_total) = match read_dmem_capacity() {
        Some(v) => v,
        None => {
            tracing::error!("No dmem capacity in /sys/fs/cgroup/dmem.capacity. Is dmemcg-booster running?");
            std::process::exit(1);
        }
    };
    info!("GPU: {drm_key}, VRAM: {vram_total} bytes ({} MiB)", vram_total / 1024 / 1024);

    let inner = Arc::new(Mutex::new(Inner {
        prev_cgroup: None,
        current_unit: String::new(),
        drm_key,
        vram_total,
    }));

    let _conn = connection::Builder::system()?
        .name("org.gnome.VramBooster")?
        .serve_at("/org/gnome/VramBooster", VramBoosterService { inner })?
        .build()
        .await?;

    info!("gnome-vram-booster ready on system bus (org.gnome.VramBooster)");
    std::future::pending::<()>().await;
    Ok(())
}
