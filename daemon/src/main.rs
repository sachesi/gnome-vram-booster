use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};
use zbus::{connection, interface};

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

fn cgroup_path_for_pid(pid: u32) -> Option<String> {
    let text = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    for line in text.lines() {
        if let Some(rel) = line.strip_prefix("0::") {
            return Some(format!("/sys/fs/cgroup{}", rel.trim()));
        }
    }
    None
}

async fn write_dmem_low(cgroup_dir: &str, drm_key: &str, bytes: u64) -> std::io::Result<bool> {
    let file = format!("{cgroup_dir}/dmem.low");
    if tokio::fs::metadata(&file).await.is_err() {
        return Ok(false);
    }
    tokio::fs::write(&file, format!("{drm_key} {bytes}\n")).await?;
    Ok(true)
}

fn is_app_scope(cgroup_dir: &str) -> bool {
    cgroup_dir.contains("/app.slice/")
}

fn unit_label(cgroup_dir: &str) -> &str {
    cgroup_dir.rsplit('/').next().unwrap_or(cgroup_dir)
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn pid_comm(pid: u32) -> String {
    read_trimmed(&format!("/proc/{pid}/comm")).unwrap_or_default()
}

fn find_app_scope_for_pid(pid: u32, max_depth: usize) -> Option<String> {
    fn check(pid: u32, depth: usize, max_depth: usize) -> Option<String> {
        if let Some(cg) = cgroup_path_for_pid(pid) {
            if is_app_scope(&cg) {
                return Some(cg);
            }
        }
        if depth >= max_depth {
            return None;
        }
        let task_dir = format!("/proc/{pid}/task");
        let task_entries = fs::read_dir(&task_dir).ok()?;
        for entry in task_entries.flatten() {
            let tid: u32 = match entry.file_name().to_string_lossy().parse() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let children_str = match read_trimmed(&format!("/proc/{tid}/children")) {
                Some(s) => s,
                None => continue,
            };
            for child_str in children_str.split_whitespace() {
                let child: u32 = match child_str.parse() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if let Some(cg) = check(child, depth + 1, max_depth) {
                    return Some(cg);
                }
            }
        }
        None
    }
    check(pid, 0, max_depth)
}

fn read_boost_ratio() -> f64 {
    match std::env::var("VRAM_BOOST_RATIO") {
        Ok(v) => match v.parse::<f64>() {
            Ok(r) if (0.0..=1.0).contains(&r) => r,
            _ => {
                warn!("VRAM_BOOST_RATIO invalid, using 0.90");
                0.90
            }
        },
        Err(_) => 0.90,
    }
}

struct Inner {
    prev_cgroup: Option<String>,
    current_unit: String,
    drm_key: String,
    vram_total: u64,
    boost_ratio: f64,
}

impl Inner {
    fn boost_bytes(&self) -> u64 {
        (self.vram_total as f64 * self.boost_ratio) as u64
    }

    async fn reset_previous(&mut self) {
        if let Some(ref prev) = self.prev_cgroup {
            match write_dmem_low(prev, &self.drm_key, 0).await {
                Ok(true) => info!("dmem.low=0 \u{2190} {}", unit_label(prev)),
                Ok(false) => info!("dmem.low missing (scope gone?): {}", unit_label(prev)),
                Err(e) => warn!("revert failed for {}: {e}", unit_label(prev)),
            }
        }
        self.prev_cgroup = None;
        self.current_unit.clear();
    }

    async fn handle_focus(&mut self, cgroup: Option<String>, pid: u32) -> bool {
        let cgroup = match cgroup {
            Some(p) => p,
            None => {
                info!("pid={pid} skip (no app.slice in cgroup tree)");
                return false;
            }
        };

        if self.prev_cgroup.as_deref() == Some(cgroup.as_str()) {
            return true;
        }

        let label = unit_label(&cgroup).to_string();
        let boost = self.boost_bytes();
        let comm = tokio::task::spawn_blocking(move || pid_comm(pid))
            .await
            .unwrap_or_default();
        info!("focus pid={pid} ({comm}) \u{2192} dmem.low={boost} \u{2192} {label}");

        self.reset_previous().await;

        match write_dmem_low(&cgroup, &self.drm_key, boost).await {
            Ok(true) => info!("dmem.low={boost} \u{2192} {label}"),
            Ok(false) => warn!("dmem.low missing \u{2014} is dmemcg-booster running? ({label})"),
            Err(e) => warn!("boost failed for {label}: {e}"),
        }

        self.prev_cgroup = Some(cgroup);
        self.current_unit = label;
        true
    }
}

struct VramBoosterService {
    inner: Arc<Mutex<Inner>>,
}

#[interface(name = "org.gnome.VramBooster")]
impl VramBoosterService {
    async fn focus_changed(&self, pid: u32) -> bool {
        let cgroup = tokio::task::spawn_blocking(move || find_app_scope_for_pid(pid, 3))
            .await
            .unwrap_or_default();
        self.inner.lock().await.handle_focus(cgroup, pid).await
    }

    #[zbus(property)]
    async fn current_unit(&self) -> String {
        self.inner.lock().await.current_unit.clone()
    }

    #[zbus(property)]
    async fn drm_key(&self) -> String {
        self.inner.lock().await.drm_key.clone()
    }

    #[zbus(property)]
    async fn vram_total(&self) -> u64 {
        self.inner.lock().await.vram_total
    }

    #[zbus(property)]
    async fn boost_ratio(&self) -> f64 {
        self.inner.lock().await.boost_ratio
    }

    #[zbus(property)]
    async fn boosted_bytes(&self) -> u64 {
        self.inner.lock().await.boost_bytes()
    }

    #[zbus(property)]
    async fn prev_cgroup(&self) -> String {
        self.inner
            .lock()
            .await
            .prev_cgroup
            .clone()
            .unwrap_or_default()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let boost_ratio = read_boost_ratio();
    info!("boost_ratio={boost_ratio}");

    let (drm_key, vram_total) = match read_dmem_capacity() {
        Some(v) => v,
        None => {
            tracing::error!(
                "No dmem capacity in /sys/fs/cgroup/dmem.capacity. Is dmemcg-booster running?"
            );
            std::process::exit(1);
        }
    };
    let boost_bytes = (vram_total as f64 * boost_ratio) as u64;
    info!(
        "GPU: {drm_key}, VRAM: {vram_total} bytes ({} MiB), boost: {boost_bytes} bytes",
        vram_total / 1024 / 1024
    );

    let inner = Arc::new(Mutex::new(Inner {
        prev_cgroup: None,
        current_unit: String::new(),
        drm_key,
        vram_total,
        boost_ratio,
    }));

    let _conn = connection::Builder::system()?
        .name("org.gnome.VramBooster")?
        .serve_at(
            "/org/gnome/VramBooster",
            VramBoosterService {
                inner: inner.clone(),
            },
        )?
        .build()
        .await?;

    info!("gnome-vram-booster ready on system bus (org.gnome.VramBooster)");

    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = sigterm.recv() => info!("received SIGTERM"),
        _ = sigint.recv() => info!("received SIGINT"),
    }

    let mut guard = inner.lock().await;
    guard.reset_previous().await;
    info!("cleanup done, exiting");
    Ok(())
}
