//! `telemetrix update [--check] [--dry-run] [--force]`.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::commands::screensaver;
use crate::update::{self, CheckError, Source, Version};

pub fn run(check: bool, dry_run: bool, force: bool) -> ExitCode {
    let own = Version::current();
    let exe = match std::env::current_exe() {
        Ok(p) => PathBuf::from(screensaver::strip_verbatim(&p.display().to_string())),
        Err(e) => {
            eprintln!("update: cannot find this program's file: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("this program: telemetrix {own} ({})", exe.display());
    if exe
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case(screensaver::WATCH_EXE))
    {
        eprintln!("update: this is the screensaver watcher's copy; run the installed telemetrix");
        return ExitCode::FAILURE;
    }
    let Some(platform) = update::platform() else {
        eprintln!(
            "update: releases have no build for {} {}; build from source instead",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        return ExitCode::FAILURE;
    };
    let source = Source::github();
    let release = match update::latest(&source) {
        Ok(r) => r,
        Err(CheckError::RateLimited(m)) => {
            println!("{m}. Nothing was changed; try again later.");
            return ExitCode::FAILURE;
        }
        Err(CheckError::Failed(m)) => {
            eprintln!("update: {m}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "latest release: {} (github.com/OliverD25/telemetrix)",
        release.tag
    );
    let newer = release.version > own;
    if !newer && !force {
        if release.version == own {
            println!("up to date");
        } else {
            println!(
                "this build is newer than the latest release; nothing to do \
                 (--force installs the release anyway)"
            );
        }
        return ExitCode::SUCCESS;
    }
    if check {
        if newer {
            println!(
                "update available: {own} -> {}. Run `telemetrix update` to install it.",
                release.version
            );
        } else {
            println!("--force: the release would replace this build");
        }
        return ExitCode::SUCCESS;
    }
    let name = match update::pick_asset(&release, platform) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("update: {e}");
            return ExitCode::FAILURE;
        }
    };
    let watcher = screensaver::task_installed(None);
    if dry_run {
        println!("--dry-run: nothing is changed. update would:");
        println!(
            "1. download {} from release {}",
            update::SUMS_NAME,
            release.tag
        );
        println!("2. download {name} next to this program and check its SHA-256 against it");
        if cfg!(windows) {
            println!(
                "3. rename {} to {} (deleted at the next start) and move the download in its place",
                exe.display(),
                update::old_path(&exe).display()
            );
        } else {
            println!("3. move the download over {} in one rename", exe.display());
        }
        if watcher {
            println!(
                "4. run the new program with `screensaver update`, so the watcher copy follows"
            );
        }
        return ExitCode::SUCCESS;
    }
    println!("downloading {name}");
    let d = match update::download(&source, &release, &name, &exe) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("update: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{} bytes, SHA-256 {} matches {}",
        d.bytes,
        update::to_hex(&d.sha256),
        update::SUMS_NAME
    );
    if let Err(e) = update::replace_exe(&exe, &d.path) {
        let _ = std::fs::remove_file(&d.path);
        eprintln!(
            "update: cannot replace {}: {e}; nothing was changed",
            exe.display()
        );
        return ExitCode::FAILURE;
    }
    update::discard_ready(&exe);
    println!("installed telemetrix {} as {}", d.version, exe.display());
    if cfg!(windows) {
        println!(
            "the old program is {} until the next start",
            update::old_path(&exe).display()
        );
    }
    if watcher {
        println!("updating the screensaver watcher:");
        if let Err(e) = screensaver::refresh_watcher(&exe, true) {
            eprintln!("update: {e}; run telemetrix screensaver update by hand");
            return ExitCode::FAILURE;
        }
    }
    println!("done. An open dashboard shows `update ready · u restart`.");
    ExitCode::SUCCESS
}
