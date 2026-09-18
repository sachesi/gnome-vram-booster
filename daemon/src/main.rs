use std::collections::HashMap;
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

/// What a dmem write did, so a caller can tell a cgroup that has no such
/// file from one whose write did not finish in time.
#[derive(Debug, PartialEq, Eq)]
enum WriteOutcome {
    Wrote,
    Missing,
    TimedOut,
}

async fn write_dmem_low(
    cgroup_dir: &str,
    drm_key: &str,
    bytes: u64,
) -> std::io::Result<WriteOutcome> {
    if cgroup_dir.contains("..") {
        return Ok(WriteOutcome::Missing);
    }
    let file = format!("{cgroup_dir}/dmem.low");
    let drm_key = drm_key.to_string();
    match tokio::time::timeout(std::time::Duration::from_secs(2), async move {
        if tokio::fs::metadata(&file).await.is_err() {
            return Ok::<WriteOutcome, std::io::Error>(WriteOutcome::Missing);
        }
        tokio::fs::write(&file, format!("{drm_key} {bytes}\n")).await?;
        Ok(WriteOutcome::Wrote)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Ok(WriteOutcome::TimedOut),
    }
}

/// Set the dmem file `name` of `cgroup_dir` to `value` for `drm_key`. Opened
/// with O_NONBLOCK, which stops a kernel that reclaims down to a lowered
/// dmem.max (7.3 and later) from evicting in the write: the limit takes
/// effect at once, and usage shrinks as buffers are freed. Older kernels,
/// and dmem.low, ignore it.
async fn write_dmem_value(
    cgroup_dir: &str,
    name: &str,
    drm_key: &str,
    value: &str,
) -> std::io::Result<WriteOutcome> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if cgroup_dir.contains("..") {
        return Ok(WriteOutcome::Missing);
    }
    let file = format!("{cgroup_dir}/{name}");
    let body = format!("{drm_key} {value}\n");
    let write = tokio::task::spawn_blocking(move || {
        if fs::metadata(&file).is_err() {
            return Ok(WriteOutcome::Missing);
        }
        fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&file)?
            .write_all(body.as_bytes())?;
        Ok(WriteOutcome::Wrote)
    });
    match tokio::time::timeout(std::time::Duration::from_secs(2), write).await {
        Ok(result) => result.map_err(std::io::Error::other)?,
        Err(_) => Ok(WriteOutcome::TimedOut),
    }
}

/// What `content` (a dmem.low or dmem.max file body) sets `drm_key` to: a
/// number of bytes, or `max`. None if the region is not listed.
fn dmem_entry<'a>(content: &'a str, drm_key: &str) -> Option<&'a str> {
    content.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(k), Some(v)) if k == drm_key => Some(v),
            _ => None,
        }
    })
}

/// What the dmem file `name` of `dir` holds for `drm_key`, if anything.
async fn dmem_value_of(dir: &str, name: &str, drm_key: &str) -> Option<String> {
    let content = tokio::fs::read_to_string(format!("{dir}/{name}"))
        .await
        .ok()?;
    dmem_entry(&content, drm_key).map(str::to_string)
}

/// The user service manager (`user@<uid>.service`) whose app.slice
/// `cgroup_dir` is in. None for any other app.slice: the slice settings are
/// only ever put on a user's.
fn user_manager(cgroup_dir: &str) -> Option<String> {
    let (head, _) = cgroup_dir.split_once("/app.slice/")?;
    let manager = head.rsplit('/').next()?;
    (manager.starts_with("user@") && manager.ends_with(".service")).then(|| head.to_string())
}

/// True if the cgroup's `dmem.low` currently holds `value` for `drm_key`.
/// Used to notice a boost that something else reverted.
async fn dmem_low_is(cgroup_dir: &str, drm_key: &str, value: u64) -> bool {
    match tokio::fs::read_to_string(format!("{cgroup_dir}/dmem.low")).await {
        Ok(content) => dmem_low_has_value(&content, drm_key, value),
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

/// Best-effort startup cleanup: clear dmem.low values left behind by a crashed
/// or SIGKILLed daemon. Only clears app.slice scopes whose value for the selected
/// drm_key equals our boost value; unrelated values are left untouched.
fn cleanup_stale_boosts(drm_key: &str, boost_bytes: u64) -> usize {
    fn walk(dir: &std::path::Path, drm_key: &str, boost_bytes: u64, cleared: &mut usize) {
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
                walk(&path, drm_key, boost_bytes, cleared);
            } else if entry.file_name() == "dmem.low" && is_app_scope(&path.to_string_lossy()) {
                let content = match fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if dmem_low_has_value(&content, drm_key, boost_bytes) {
                    match fs::write(&path, format!("{drm_key} 0\n")) {
                        Ok(()) => *cleared += 1,
                        Err(e) => warn!("startup cleanup: failed to clear {}: {e}", path.display()),
                    }
                }
            }
        }
    }
    let mut cleared = 0;
    walk(
        std::path::Path::new("/sys/fs/cgroup/user.slice"),
        drm_key,
        boost_bytes,
        &mut cleared,
    );
    cleared
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

/// Off unless asked for: from Linux 7.3 a charge that fails on the ceiling
/// sends the focused app's new buffers to system memory rather than evicting
/// background apps, see docs/usage.md.
fn read_reserve_mib() -> u64 {
    match std::env::var("VRAM_RESERVE_MIB") {
        Ok(v) => v.parse().unwrap_or_else(|_| {
            warn!("VRAM_RESERVE_MIB invalid, app.slice ceiling off");
            0
        }),
        Err(_) => 0,
    }
}

/// The ceiling on app.slice: VRAM less the reserve. None when the reserve is
/// 0, which turns the ceiling off, or when nothing would be left.
fn ceiling_for(vram_total: u64, reserve_mib: u64) -> Option<u64> {
    if reserve_mib == 0 {
        return None;
    }
    reserve_mib
        .checked_mul(1024 * 1024)
        .and_then(|r| vram_total.checked_sub(r))
        .filter(|c| *c > 0)
}

/// Whether session.slice gets a dmem.low of the whole VRAM; on unless
/// VRAM_PROTECT_SESSION=0.
fn read_protect_session() -> bool {
    match std::env::var("VRAM_PROTECT_SESSION").as_deref() {
        Ok("0") => false,
        Ok("1") | Err(_) => true,
        Ok(_) => {
            warn!("VRAM_PROTECT_SESSION invalid, protecting session.slice");
            true
        }
    }
}

/// Where a slice setting on one user's slice stands, as last seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SliceState {
    /// The slice has no such file for the GPU, or the write did not finish.
    Unknown,
    /// The slice holds this daemon's value.
    Held,
    /// Written, but the old value stayed: a kernel before 7.3 refuses a
    /// dmem.max below what the slice already uses, without an error.
    Refused,
    /// The write failed.
    Failed,
    /// The slice has a value of someone else's, which is left alone.
    Foreign,
}

/// A dmem value the daemon keeps on a slice of every user whose app it
/// boosts: the protection of session.slice, or the ceiling on app.slice. It
/// is written only where the file holds its unset value, left alone where
/// someone else set it, and put back at exit while it is still the daemon's.
struct SliceSetting {
    /// What it is, for the log.
    what: &'static str,
    /// The slice, below `user@<uid>.service`.
    slice: &'static str,
    file: &'static str,
    /// What the file holds when nobody set it.
    unset: &'static str,
    value: u64,
}

impl SliceSetting {
    fn session_protection(value: u64) -> Self {
        Self {
            what: "the protection of session.slice",
            slice: "session.slice",
            file: "dmem.low",
            unset: "0",
            value,
        }
    }

    fn ceiling(value: u64) -> Self {
        Self {
            what: "the ceiling on app.slice",
            slice: "app.slice",
            file: "dmem.max",
            unset: "max",
            value,
        }
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

struct Inner {
    prev_cgroup: Option<String>,
    current_unit: String,
    drm_key: String,
    vram_total: u64,
    boost_ratio: f64,
    /// What the daemon keeps on each user's session.slice and app.slice.
    slices: Vec<SliceSetting>,
    /// Each slice seen so far, by its directory and the setting's index in
    /// `slices`, with the setting's state there.
    slice_states: HashMap<(String, usize), SliceState>,
}

impl Inner {
    fn boost_bytes(&self) -> u64 {
        (self.vram_total as f64 * self.boost_ratio) as u64
    }

    /// Put the slice settings on the slices of `manager` where they are not
    /// there. Checked at every focus change rather than once: the slices are
    /// made anew when a user manager restarts, and a refused value is worth
    /// trying again.
    async fn ensure_slices(&mut self, manager: &str) {
        for (i, setting) in self.slices.iter().enumerate() {
            let dir = format!("{manager}/{}", setting.slice);
            let ours = setting.value.to_string();
            let before = self
                .slice_states
                .get(&(dir.clone(), i))
                .copied()
                .unwrap_or(SliceState::Unknown);
            let current = dmem_value_of(&dir, setting.file, &self.drm_key).await;
            let state = match current.as_deref() {
                None => SliceState::Unknown,
                Some(v) if v == ours => SliceState::Held,
                Some(v) if v == setting.unset => {
                    match write_dmem_value(&dir, setting.file, &self.drm_key, &ours).await {
                        Ok(WriteOutcome::Wrote) => {
                            let now = dmem_value_of(&dir, setting.file, &self.drm_key).await;
                            if now.as_deref() == Some(ours.as_str()) {
                                SliceState::Held
                            } else {
                                SliceState::Refused
                            }
                        }
                        Ok(_) => SliceState::Unknown,
                        Err(e) => {
                            if before != SliceState::Failed {
                                warn!("cannot set {} in {dir}: {e}", setting.what);
                            }
                            SliceState::Failed
                        }
                    }
                }
                Some(v) => {
                    if before != SliceState::Foreign {
                        warn!(
                            "{dir}/{} is {v} for {}, not this daemon's; leaving it alone",
                            setting.file, self.drm_key
                        );
                    }
                    SliceState::Foreign
                }
            };
            if state != before {
                match state {
                    SliceState::Held => info!(
                        "set {} in {dir}: {}={}",
                        setting.what, setting.file, setting.value
                    ),
                    SliceState::Refused => info!(
                        "the kernel kept {dir}/{} as it was, since the slice uses more than {} bytes; trying again at the next focus change",
                        setting.file, setting.value
                    ),
                    _ => {}
                }
            }
            self.slice_states.insert((dir, i), state);
        }
    }

    /// Put the unset value back wherever a slice still holds the daemon's.
    async fn restore_slices(&mut self) {
        for (dir, i) in std::mem::take(&mut self.slice_states).into_keys() {
            let setting = &self.slices[i];
            let current = dmem_value_of(&dir, setting.file, &self.drm_key).await;
            if current != Some(setting.value.to_string()) {
                continue;
            }
            match write_dmem_value(&dir, setting.file, &self.drm_key, setting.unset).await {
                Ok(WriteOutcome::Wrote) => info!("took off {} in {dir}", setting.what),
                Ok(WriteOutcome::Missing) => {}
                Ok(WriteOutcome::TimedOut) => {
                    warn!("taking off {} in {dir} did not finish in 2 s", setting.what)
                }
                Err(e) => warn!("cannot take off {} in {dir}: {e}", setting.what),
            }
        }
    }

    /// The value the daemon keeps in `file` of a slice; 0 when it keeps none.
    fn slice_value(&self, file: &str) -> u64 {
        self.slices
            .iter()
            .find(|s| s.file == file)
            .map_or(0, |s| s.value)
    }

    async fn reset_previous(&mut self) {
        if let Some(ref prev) = self.prev_cgroup {
            match write_dmem_low(prev, &self.drm_key, 0).await {
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

        if let Some(manager) = user_manager(&cgroup) {
            self.ensure_slices(&manager).await;
        }

        let label = unit_label(&cgroup).to_string();
        let boost = self.boost_bytes();

        // Same cgroup as last time. Trusting in-memory state would hide a boost
        // that something else reverted, so the file decides.
        if self.prev_cgroup.as_deref() == Some(cgroup.as_str()) {
            if dmem_low_is(&cgroup, &self.drm_key, boost).await {
                return true;
            }
            info!("the boost on {label} was reverted from outside, applying it again");
        }
        let comm = tokio::task::spawn_blocking(move || pid_comm(pid))
            .await
            .unwrap_or_default();
        info!("focus pid={pid} ({comm}) \u{2192} dmem.low={boost} \u{2192} {label}");

        if self.prev_cgroup.as_deref() != Some(cgroup.as_str()) {
            self.reset_previous().await;
        }

        match write_dmem_low(&cgroup, &self.drm_key, boost).await {
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

    /// dmem.max this daemon puts on each user's app.slice; 0 when off.
    #[zbus(property)]
    async fn app_slice_ceiling(&self) -> u64 {
        self.inner.lock().await.slice_value("dmem.max")
    }

    /// dmem.low the daemon puts on each user's session.slice; 0 when none.
    #[zbus(property)]
    async fn session_slice_low(&self) -> u64 {
        self.inner.lock().await.slice_value("dmem.low")
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
    let reserve_mib = read_reserve_mib();
    let ceiling = ceiling_for(vram_total, reserve_mib);
    if ceiling.is_none() && reserve_mib > 0 {
        warn!("VRAM_RESERVE_MIB={reserve_mib} leaves nothing of the VRAM; app.slice ceiling off");
    }
    let protect_session = read_protect_session();
    let mut slices = Vec::new();
    if protect_session {
        slices.push(SliceSetting::session_protection(vram_total));
    }
    if let Some(c) = ceiling {
        slices.push(SliceSetting::ceiling(c));
    }
    info!(
        "GPU: {drm_key}, VRAM: {vram_total} bytes ({} MiB), boost: {boost_bytes} bytes, session.slice protected: {protect_session}, app.slice ceiling: {}",
        vram_total / 1024 / 1024,
        match ceiling {
            Some(c) => format!("{c} bytes ({reserve_mib} MiB reserved)"),
            None => "off".to_string(),
        }
    );

    let cleanup_key = drm_key.clone();
    let inner = Arc::new(Mutex::new(Inner {
        prev_cgroup: None,
        current_unit: String::new(),
        drm_key,
        vram_total,
        boost_ratio,
        slices,
        slice_states: HashMap::new(),
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
    let cleared = cleanup_stale_boosts(&cleanup_key, boost_bytes);
    info!("startup cleanup: cleared {cleared} stale dmem.low boost value(s)");

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
    guard.restore_slices().await;
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
    fn is_app_scope_exact_component_only() {
        assert!(is_app_scope(
            "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/app-foo.scope"
        ));
        assert!(!is_app_scope("/sys/fs/cgroup/user.slice/session.slice"));
        // substring that is not an exact path component must not match
        assert!(!is_app_scope("/sys/fs/cgroup/my-app.slice-x/foo"));
    }

    #[test]
    fn user_manager_only_for_a_users_app_slice() {
        let manager = "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service";
        assert_eq!(
            user_manager(&format!("{manager}/app.slice/app-foo.scope")),
            Some(manager.to_string())
        );
        assert_eq!(
            user_manager(&format!("{manager}/app.slice/app-x.slice/app-foo.scope")),
            Some(manager.to_string())
        );
        assert_eq!(user_manager("/sys/fs/cgroup/app.slice/foo.service"), None);
        assert_eq!(
            user_manager("/sys/fs/cgroup/user.slice/user-1000.slice/session-2.scope"),
            None
        );
    }

    #[test]
    fn ceiling_for_leaves_the_reserve() {
        let mib = 1024 * 1024;
        assert_eq!(ceiling_for(8192 * mib, 256), Some(7936 * mib));
        assert_eq!(ceiling_for(8192 * mib, 0), None);
        assert_eq!(ceiling_for(256 * mib, 256), None);
        assert_eq!(ceiling_for(8192 * mib, u64::MAX), None);
    }

    /// The slice files are ordinary files here: a setting goes where the file
    /// is unset, stays off one someone else set, and is put back at exit
    /// only while it is the daemon's.
    #[tokio::test]
    async fn a_slice_setting_goes_only_where_nobody_else_set_one() {
        let manager = std::env::temp_dir().join(format!("gvb-slices-{}", std::process::id()));
        let _ = fs::remove_dir_all(&manager);
        let key = "drm/0000:2d:00.0/vram";
        let mut inner = Inner {
            prev_cgroup: None,
            current_unit: String::new(),
            drm_key: key.to_string(),
            vram_total: 1000,
            boost_ratio: 0.9,
            slices: vec![
                SliceSetting::session_protection(1000),
                SliceSetting::ceiling(900),
            ],
            slice_states: HashMap::new(),
        };
        let files = [
            (manager.join("session.slice/dmem.low"), "0", "1000"),
            (manager.join("app.slice/dmem.max"), "max", "900"),
        ];
        for (file, _, _) in &files {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
        }
        let manager = manager.to_string_lossy().into_owned();

        // no such files yet: nothing is written
        inner.ensure_slices(&manager).await;
        assert!(files.iter().all(|(file, _, _)| !file.exists()));

        for (file, unset, _) in &files {
            fs::write(file, format!("{key} {unset}\n")).unwrap();
        }
        inner.ensure_slices(&manager).await;
        for (file, _, ours) in &files {
            assert_eq!(fs::read_to_string(file).unwrap(), format!("{key} {ours}\n"));
        }
        assert!(inner.slice_states.values().all(|s| *s == SliceState::Held));

        inner.restore_slices().await;
        for (file, unset, _) in &files {
            assert_eq!(
                fs::read_to_string(file).unwrap(),
                format!("{key} {unset}\n")
            );
        }

        for (file, _, _) in &files {
            fs::write(file, format!("{key} 500\n")).unwrap();
        }
        inner.ensure_slices(&manager).await;
        assert!(
            inner
                .slice_states
                .values()
                .all(|s| *s == SliceState::Foreign)
        );
        inner.restore_slices().await;
        for (file, _, _) in &files {
            assert_eq!(fs::read_to_string(file).unwrap(), format!("{key} 500\n"));
        }
        let _ = fs::remove_dir_all(&manager);
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
}
