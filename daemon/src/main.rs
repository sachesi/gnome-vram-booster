use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};
use zbus::{connection, interface};

fn parse_dmem_capacity(content: &str) -> Vec<(String, u64)> {
    content
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?;
            let val: u64 = parts.next()?.parse().ok()?;
            if key.starts_with("drm/") && val > 0 {
                Some((key.to_string(), val))
            } else {
                None
            }
        })
        .collect()
}

fn read_dmem_capacity() -> Option<(String, u64)> {
    let content = fs::read_to_string("/sys/fs/cgroup/dmem.capacity").ok()?;
    let entries: Vec<(String, u64)> = parse_dmem_capacity(&content);

    if let Ok(override_key) = std::env::var("DRM_KEY") {
        return entries
            .into_iter()
            .find(|(k, _)| *k == override_key)
            .or_else(|| {
                warn!("DRM_KEY={override_key} not found in dmem.capacity");
                None
            });
    }

    entries.into_iter().max_by_key(|(_, v)| *v)
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

/// What a `dmem.low` write did, so a caller can tell a scope that has no
/// `dmem.low` from one whose write did not finish in time.
#[derive(Debug, PartialEq, Eq)]
enum WriteOutcome {
    Wrote,
    Missing,
    TimedOut,
}

async fn write_cgroup_file(
    cgroup_dir: &str,
    file_name: &str,
    payload: String,
) -> std::io::Result<WriteOutcome> {
    if cgroup_dir.contains("..") {
        return Ok(WriteOutcome::Missing);
    }
    let file = format!("{cgroup_dir}/{file_name}");
    match tokio::time::timeout(std::time::Duration::from_secs(2), async move {
        if tokio::fs::metadata(&file).await.is_err() {
            return Ok::<WriteOutcome, std::io::Error>(WriteOutcome::Missing);
        }
        tokio::fs::write(&file, payload).await?;
        Ok(WriteOutcome::Wrote)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Ok(WriteOutcome::TimedOut),
    }
}

async fn write_dmem_low(
    cgroup_dir: &str,
    drm_key: &str,
    bytes: u64,
) -> std::io::Result<WriteOutcome> {
    write_cgroup_file(cgroup_dir, "dmem.low", format!("{drm_key} {bytes}\n")).await
}

async fn ensure_parent_cpu_subtree(cgroup_dir: &str) {
    let weight_file = format!("{cgroup_dir}/cpu.weight");
    if tokio::fs::metadata(&weight_file).await.is_ok() {
        return;
    }
    let mut current = std::path::PathBuf::from(cgroup_dir);
    while let Some(parent) = current.parent() {
        if parent == std::path::Path::new("/sys/fs/cgroup") || parent == std::path::Path::new("/") {
            break;
        }
        let controllers = parent.join("cgroup.controllers");
        let subtree = parent.join("cgroup.subtree_control");
        if let Ok(ctrls) = tokio::fs::read_to_string(&controllers).await
            && ctrls.split_whitespace().any(|c| c == "cpu")
            && let Ok(sub) = tokio::fs::read_to_string(&subtree).await
            && !sub.split_whitespace().any(|c| c == "cpu")
        {
            let _ = tokio::fs::write(&subtree, "+cpu\n").await;
        }
        current = parent.to_path_buf();
    }
}

async fn write_cpu_weight(cgroup_dir: &str, weight: u64) -> std::io::Result<WriteOutcome> {
    ensure_parent_cpu_subtree(cgroup_dir).await;
    write_cgroup_file(cgroup_dir, "cpu.weight", format!("{weight}\n")).await
}

/// True if the cgroup's `dmem.low` currently holds `value` for `drm_key`.
/// Used to notice a boost that something else reverted.
async fn dmem_low_is(cgroup_dir: &str, drm_key: &str, value: u64) -> bool {
    match tokio::fs::read_to_string(format!("{cgroup_dir}/dmem.low")).await {
        Ok(content) => dmem_low_has_value(&content, drm_key, value),
        Err(_) => false,
    }
}

async fn cpu_weight_is(cgroup_dir: &str, value: u64) -> bool {
    match tokio::fs::read_to_string(format!("{cgroup_dir}/cpu.weight")).await {
        Ok(content) => cpu_weight_has_value(&content, value),
        Err(_) => false,
    }
}

fn is_app_scope(cgroup_dir: &str) -> bool {
    cgroup_dir.split('/').any(|c| c == "app.slice")
}

/// True if `content` (a dmem.low file body) sets `drm_key` to exactly `value`.
fn dmem_low_has_value(content: &str, drm_key: &str, value: u64) -> bool {
    content.lines().any(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => k == drm_key && v.parse::<u64>() == Ok(value),
            _ => false,
        }
    })
}

/// True if `content` (a cpu.weight file body) sets weight to exactly `value`.
fn cpu_weight_has_value(content: &str, value: u64) -> bool {
    content.trim().parse::<u64>().ok() == Some(value)
}

/// Best-effort startup cleanup: clear dmem.low and cpu.weight values left behind
/// by a crashed or SIGKILLed daemon. Only clears app.slice scopes whose values
/// match our boost values; unrelated values are left untouched.
fn cleanup_stale_boosts(
    drm_key: &str,
    boost_bytes: u64,
    cpu_boost_weight: Option<u64>,
) -> (usize, usize) {
    fn walk(
        dir: &std::path::Path,
        drm_key: &str,
        boost_bytes: u64,
        cpu_boost_weight: Option<u64>,
        cleared_vram: &mut usize,
        cleared_cpu: &mut usize,
    ) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                walk(
                    &path,
                    drm_key,
                    boost_bytes,
                    cpu_boost_weight,
                    cleared_vram,
                    cleared_cpu,
                );
            } else if is_app_scope(&path.to_string_lossy()) {
                if entry.file_name() == "dmem.low" {
                    if let Ok(content) = fs::read_to_string(&path)
                        && dmem_low_has_value(&content, drm_key, boost_bytes)
                    {
                        match fs::write(&path, format!("{drm_key} 0\n")) {
                            Ok(()) => *cleared_vram += 1,
                            Err(e) => {
                                warn!("startup cleanup: failed to clear {}: {e}", path.display())
                            }
                        }
                    }
                } else if entry.file_name() == "cpu.weight"
                    && let Some(w) = cpu_boost_weight
                    && let Ok(content) = fs::read_to_string(&path)
                    && cpu_weight_has_value(&content, w)
                {
                    match fs::write(&path, "100\n") {
                        Ok(()) => *cleared_cpu += 1,
                        Err(e) => warn!("startup cleanup: failed to reset {}: {e}", path.display()),
                    }
                }
            }
        }
    }
    let mut cleared_vram = 0;
    let mut cleared_cpu = 0;
    walk(
        std::path::Path::new("/sys/fs/cgroup/user.slice"),
        drm_key,
        boost_bytes,
        cpu_boost_weight,
        &mut cleared_vram,
        &mut cleared_cpu,
    );
    (cleared_vram, cleared_cpu)
}

/// Ensure +cpu is written to cgroup.subtree_control in user and app slices so child scopes expose cpu.weight.
fn ensure_cpu_subtree_control() {
    fn check_and_enable(dir: &std::path::Path) {
        let controllers = dir.join("cgroup.controllers");
        let subtree = dir.join("cgroup.subtree_control");
        if let Ok(ctrls) = fs::read_to_string(&controllers)
            && ctrls.split_whitespace().any(|c| c == "cpu")
        {
            let _ = fs::write(&subtree, "+cpu\n");
        }
    }

    fn walk(dir: &std::path::Path) {
        check_and_enable(dir);
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type()
                && ft.is_dir()
            {
                let path = entry.path();
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.ends_with(".slice") || s.starts_with("user@") {
                    walk(&path);
                }
            }
        }
    }

    walk(std::path::Path::new("/sys/fs/cgroup/user.slice"));
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
        if let Some(cg) = cgroup_path_for_pid(pid)
            && is_app_scope(&cg)
        {
            return Some(cg);
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
            let children_str = match read_trimmed(&format!("/proc/{pid}/task/{tid}/children")) {
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

fn parse_boost_ratio(raw: &str) -> Option<f64> {
    match raw.parse::<f64>() {
        Ok(r) if (0.0..=1.0).contains(&r) => Some(r),
        _ => None,
    }
}

fn read_boost_ratio() -> f64 {
    match std::env::var("VRAM_BOOST_RATIO") {
        Ok(v) => parse_boost_ratio(&v).unwrap_or_else(|| {
            warn!("VRAM_BOOST_RATIO invalid, using 0.90");
            0.90
        }),
        Err(_) => 0.90,
    }
}

fn parse_cpu_boost_weight(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty()
        || s == "0"
        || s.eq_ignore_ascii_case("false")
        || s.eq_ignore_ascii_case("off")
        || s.eq_ignore_ascii_case("disabled")
    {
        return None;
    }
    match s.parse::<u64>() {
        Ok(w) if (1..=10000).contains(&w) => Some(w),
        _ => None,
    }
}

fn read_cpu_boost_weight() -> Option<u64> {
    match std::env::var("CPU_BOOST_WEIGHT") {
        Ok(v) => parse_cpu_boost_weight(&v).or_else(|| {
            warn!("CPU_BOOST_WEIGHT invalid, disabling CPU boost");
            None
        }),
        Err(_) => Some(10000),
    }
}

struct Inner {
    prev_cgroup: Option<String>,
    current_unit: String,
    drm_key: String,
    vram_total: u64,
    boost_ratio: f64,
    cpu_boost_weight: Option<u64>,
}

impl Inner {
    fn boost_bytes(&self) -> u64 {
        (self.vram_total as f64 * self.boost_ratio) as u64
    }

    async fn reset_previous(&mut self) {
        if let Some(ref prev) = self.prev_cgroup {
            let cpu_boost = self.cpu_boost_weight.is_some();
            let (dmem_res, cpu_res) = tokio::join!(write_dmem_low(prev, &self.drm_key, 0), async {
                if cpu_boost {
                    Some(write_cpu_weight(prev, 100).await)
                } else {
                    None
                }
            });
            match dmem_res {
                Ok(WriteOutcome::Wrote) => info!("dmem.low=0 \u{2190} {}", unit_label(prev)),
                Ok(WriteOutcome::Missing) => {
                    info!("dmem.low missing (scope gone?): {}", unit_label(prev));
                }
                Ok(WriteOutcome::TimedOut) => warn!(
                    "Reverting dmem.low to 0 for {} did not finish in 2 s",
                    unit_label(prev)
                ),
                Err(e) => warn!(
                    "Failed to revert dmem.low to 0 for {}: {e}",
                    unit_label(prev)
                ),
            }
            if let Some(res) = cpu_res {
                match res {
                    Ok(WriteOutcome::Wrote) => {
                        info!("cpu.weight=100 \u{2190} {}", unit_label(prev))
                    }
                    Ok(WriteOutcome::Missing) => {}
                    Ok(WriteOutcome::TimedOut) => warn!(
                        "Reverting cpu.weight to 100 for {} did not finish in 2 s",
                        unit_label(prev)
                    ),
                    Err(e) => warn!(
                        "Failed to revert cpu.weight to 100 for {}: {e}",
                        unit_label(prev)
                    ),
                }
            }
        }
        self.prev_cgroup = None;
        self.current_unit.clear();
    }

    async fn handle_focus(&mut self, cgroup: Option<String>, pid: u32) -> bool {
        let cgroup = match cgroup {
            Some(p) => p,
            None => {
                info!("pid={pid} skip (no app.slice in cgroup tree); clearing previous boost");
                self.reset_previous().await;
                return false;
            }
        };

        let label = unit_label(&cgroup).to_string();
        let boost = self.boost_bytes();

        // Same cgroup as last time. Trusting in-memory state would hide a boost
        // that something else reverted, so the file decides.
        if self.prev_cgroup.as_deref() == Some(cgroup.as_str()) {
            let (dmem_ok, cpu_ok) =
                tokio::join!(dmem_low_is(&cgroup, &self.drm_key, boost), async {
                    match self.cpu_boost_weight {
                        Some(w) => cpu_weight_is(&cgroup, w).await,
                        None => true,
                    }
                });
            if dmem_ok && cpu_ok {
                return true;
            }
            info!("the boost on {label} was reverted from outside, applying it again");
        }
        let comm = tokio::task::spawn_blocking(move || pid_comm(pid))
            .await
            .unwrap_or_default();
        let cpu_info = self
            .cpu_boost_weight
            .map_or(String::new(), |w| format!(" cpu.weight={w}"));
        info!("focus pid={pid} ({comm}) \u{2192} dmem.low={boost}{cpu_info} \u{2192} {label}");

        if self.prev_cgroup.as_deref() != Some(cgroup.as_str()) {
            self.reset_previous().await;
        }

        let (dmem_res, cpu_res) =
            tokio::join!(write_dmem_low(&cgroup, &self.drm_key, boost), async {
                if let Some(w) = self.cpu_boost_weight {
                    Some((w, write_cpu_weight(&cgroup, w).await))
                } else {
                    None
                }
            });

        if let Some((w, res)) = cpu_res {
            match res {
                Ok(WriteOutcome::Wrote) => info!("cpu.weight={w} \u{2192} {label}"),
                Ok(WriteOutcome::Missing) => {}
                Ok(WriteOutcome::TimedOut) => {
                    warn!("Failed to boost {label}: the write to cpu.weight did not finish in 2 s");
                }
                Err(e) => warn!("Failed to write cpu.weight boost for {label}: {e}"),
            }
        }

        match dmem_res {
            Ok(WriteOutcome::Wrote) => {
                info!("dmem.low={boost} \u{2192} {label}");
                self.prev_cgroup = Some(cgroup);
                self.current_unit = label;
                true
            }
            Ok(WriteOutcome::Missing) => {
                warn!("Failed to boost {label}: it has no dmem.low. Is dmemcg-booster running?");
                false
            }
            Ok(WriteOutcome::TimedOut) => {
                warn!("Failed to boost {label}: the write to dmem.low did not finish in 2 s");
                false
            }
            Err(e) => {
                warn!("Failed to write dmem.low boost for {label}: {e}");
                false
            }
        }
    }
}

struct VramBoosterService {
    inner: Arc<Mutex<Inner>>,
}

#[interface(name = "org.gnome.VramBooster")]
impl VramBoosterService {
    async fn focus_changed(&self, pid: u32) -> bool {
        let cgroup = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            tokio::task::spawn_blocking(move || find_app_scope_for_pid(pid, 3)),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten();
        self.inner.lock().await.handle_focus(cgroup, pid).await
    }

    async fn clear_focus(&self) -> bool {
        self.inner.lock().await.reset_previous().await;
        true
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
    async fn cpu_boost_weight(&self) -> u64 {
        self.inner.lock().await.cpu_boost_weight.unwrap_or(0)
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

    let cpu_boost_weight = read_cpu_boost_weight();
    if let Some(w) = cpu_boost_weight {
        info!("cpu_boost_weight={w}");
        ensure_cpu_subtree_control();
    } else {
        info!("cpu_boost disabled");
    }

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

    let cleanup_key = drm_key.clone();
    let inner = Arc::new(Mutex::new(Inner {
        prev_cgroup: None,
        current_unit: String::new(),
        drm_key,
        vram_total,
        boost_ratio,
        cpu_boost_weight,
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

    // After the bus name, never before: a second instance has to fail claiming
    // it while the running one still owns the boost it applied.
    let (cleared_vram, cleared_cpu) =
        cleanup_stale_boosts(&cleanup_key, boost_bytes, cpu_boost_weight);
    info!("startup cleanup: cleared {cleared_vram} stale dmem.low boost value(s)");
    if cpu_boost_weight.is_some() {
        info!("startup cleanup: cleared {cleared_cpu} stale cpu.weight boost value(s)");
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dmem_capacity_picks_drm_entries() {
        let content = "drm/0000:2d:00.0/vram 8573157376\ndrm/0000:2d:00.0/gtt 0\nsystem 12345\n";
        let entries = parse_dmem_capacity(content);
        assert_eq!(
            entries,
            vec![("drm/0000:2d:00.0/vram".to_string(), 8573157376)]
        );
    }

    #[test]
    fn parse_dmem_capacity_ignores_malformed() {
        assert!(parse_dmem_capacity("").is_empty());
        assert!(parse_dmem_capacity("drm/x/vram notanumber\njunk\n").is_empty());
    }

    #[test]
    fn parse_boost_ratio_bounds() {
        assert_eq!(parse_boost_ratio("0.85"), Some(0.85));
        assert_eq!(parse_boost_ratio("0"), Some(0.0));
        assert_eq!(parse_boost_ratio("1"), Some(1.0));
        assert_eq!(parse_boost_ratio("1.5"), None);
        assert_eq!(parse_boost_ratio("-0.1"), None);
        assert_eq!(parse_boost_ratio("abc"), None);
    }

    #[test]
    fn parse_cpu_boost_weight_bounds() {
        assert_eq!(parse_cpu_boost_weight("10000"), Some(10000));
        assert_eq!(parse_cpu_boost_weight("100"), Some(100));
        assert_eq!(parse_cpu_boost_weight("1"), Some(1));
        assert_eq!(parse_cpu_boost_weight("0"), None);
        assert_eq!(parse_cpu_boost_weight("false"), None);
        assert_eq!(parse_cpu_boost_weight("off"), None);
        assert_eq!(parse_cpu_boost_weight("disabled"), None);
        assert_eq!(parse_cpu_boost_weight("10001"), None);
        assert_eq!(parse_cpu_boost_weight("abc"), None);
    }

    #[test]
    fn is_app_scope_exact_component_only() {
        assert!(is_app_scope(
            "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/app-foo.scope"
        ));
        assert!(!is_app_scope("/sys/fs/cgroup/user.slice/session.slice"));
        // substring that is not an exact path component must not match
        assert!(!is_app_scope("/sys/fs/cgroup/my-app.slice-x/foo"));
    }

    #[test]
    fn dmem_low_has_value_matches_exact_key_and_value() {
        let body = "drm/0000:2d:00.0/vram 7715841638\n";
        assert!(dmem_low_has_value(
            body,
            "drm/0000:2d:00.0/vram",
            7715841638
        ));
        assert!(!dmem_low_has_value(body, "drm/0000:2d:00.0/vram", 0));
        assert!(!dmem_low_has_value(body, "drm/other/vram", 7715841638));
        assert!(!dmem_low_has_value("", "drm/0000:2d:00.0/vram", 7715841638));
    }

    #[test]
    fn cpu_weight_has_value_matches_exact_value() {
        assert!(cpu_weight_has_value("10000\n", 10000));
        assert!(cpu_weight_has_value("100", 100));
        assert!(!cpu_weight_has_value("100", 10000));
        assert!(!cpu_weight_has_value("", 100));
    }
}
