use std::collections::HashMap;
use zbus::Connection;
use zvariant::OwnedValue;

/// Unit names and cgroup paths come from other processes, any user's, so
/// they can carry control characters that would rewrite the terminal. Strip
/// them.
fn printable(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control()).collect()
}

fn format_val(v: &OwnedValue) -> String {
    if let Ok(s) = v.downcast_ref::<String>() {
        if s.is_empty() {
            "(none)".into()
        } else {
            printable(&s)
        }
    } else if let Ok(n) = v.downcast_ref::<u64>() {
        n.to_string()
    } else if let Ok(n) = v.downcast_ref::<f64>() {
        format!("{:.2}", n)
    } else {
        format!("{v:?}")
    }
}

fn human_bytes(b: u64) -> String {
    let mb = b / (1024 * 1024);
    let gb = mb as f64 / 1024.0;
    if gb >= 1.0 {
        format!("{b} ({mb} MiB, {gb:.2} GiB)")
    } else {
        format!("{b} ({mb} MiB)")
    }
}

fn get_u64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<u64> {
    props.get(key).and_then(|v| v.downcast_ref::<u64>().ok())
}

fn get_f64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<f64> {
    props.get(key).and_then(|v| v.downcast_ref::<f64>().ok())
}

#[tokio::main]
async fn main() {
    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: cannot connect to system bus: {e}");
            std::process::exit(1);
        }
    };

    let result: Result<HashMap<String, OwnedValue>, _> = conn
        .call_method(
            Some("org.gnome.VramBooster"),
            "/org/gnome/VramBooster",
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &("org.gnome.VramBooster",),
        )
        .await
        .and_then(|r| r.body().deserialize());

    let props = match result {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot query daemon: {e}");
            eprintln!("Is gnome-vram-booster running?");
            std::process::exit(1);
        }
    };

    let total = get_u64(&props, "VramTotal").unwrap_or(0);
    let boosted = get_u64(&props, "BoostedBytes").unwrap_or(0);
    let boost_ratio = get_f64(&props, "BoostRatio").unwrap_or(0.0);
    let ceiling = get_u64(&props, "AppSliceCeiling").unwrap_or(0);
    let session_low = get_u64(&props, "SessionSliceLow").unwrap_or(0);

    println!("=== GNOME VRAM Booster Status ===");
    println!("Daemon:           running");
    println!(
        "DRM key:          {}",
        props.get("DrmKey").map_or("?".into(), format_val)
    );
    println!("VRAM total:       {}", human_bytes(total));
    println!("Boost ratio:      {:.0}%", boost_ratio * 100.0);
    println!(
        "Boosted bytes:    {} ({}% of total)",
        human_bytes(boosted),
        if total > 0 {
            (boosted as f64 / total as f64 * 100.0) as u64
        } else {
            0
        }
    );
    if session_low == 0 {
        println!("Session low:      off");
    } else {
        println!("Session low:      {}", human_bytes(session_low));
    }
    if ceiling == 0 {
        println!("App ceiling:      off");
    } else {
        println!(
            "App ceiling:      {} ({} MiB reserved)",
            human_bytes(ceiling),
            total.saturating_sub(ceiling) / 1024 / 1024
        );
    }
    println!(
        "Current unit:     {}",
        props.get("CurrentUnit").map_or("(none)".into(), format_val)
    );
    println!(
        "Boosted cgroup:   {}",
        props.get("PrevCgroup").map_or("(none)".into(), format_val)
    );
}
