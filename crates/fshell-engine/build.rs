// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=FSH_BUILD_DATETIME");

    if let Ok(override_dt) = std::env::var("FSH_BUILD_DATETIME") {
        let full = format!(
            "{} {}",
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string()),
            override_dt
        );
        println!("cargo:rustc-env=FSH_BUILD_DATETIME={override_dt}");
        println!("cargo:rustc-env=FSH_FULL_VERSION={full}");
        println!("cargo:rustc-env=FSH_BUILD_DATETIME_ISO={override_dt}");
        return;
    }

    if let Ok(sde) = std::env::var("SOURCE_DATE_EPOCH")
        && let Ok(secs) = sde.parse::<i64>()
    {
        let compact = epoch_to_compact(secs);
        let iso = epoch_to_iso(secs);
        println!("cargo:rustc-env=FSH_BUILD_DATETIME={compact}");
        println!("cargo:rustc-env=FSH_BUILD_DATETIME_ISO={iso}");
        println!("cargo:rustc-env=FSH_BUILD_TIMESTAMP={secs}");
        let full = format!(
            "{} {}",
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string()),
            compact
        );
        println!("cargo:rustc-env=FSH_FULL_VERSION={full}");
        set_git_env();
        return;
    }

    let (compact, iso, ts) = current_utc_formatted();
    println!("cargo:rustc-env=FSH_BUILD_DATETIME={compact}");
    println!("cargo:rustc-env=FSH_BUILD_DATETIME_ISO={iso}");
    println!("cargo:rustc-env=FSH_BUILD_TIMESTAMP={ts}");
    let full = format!(
        "{} {}",
        std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string()),
        compact
    );
    println!("cargo:rustc-env=FSH_FULL_VERSION={full}");
    set_git_env();
}

fn set_git_env() {
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        && output.status.success()
    {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !sha.is_empty() {
            println!("cargo:rustc-env=FSH_GIT_COMMIT={sha}");
        }
    }
    println!("cargo:rerun-if-changed=.git/HEAD");
}

fn current_utc_formatted() -> (String, String, i64) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    (epoch_to_compact(secs), epoch_to_iso(secs), secs)
}

fn epoch_to_compact(secs: i64) -> String {
    let (y, m, d, hh, mm, _) = epoch_to_ymd_hms(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}")
}

fn epoch_to_iso(secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = epoch_to_ymd_hms(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn epoch_to_ymd_hms(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400) as u32;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, hh, mm, ss)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
