use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    emit_build_info();
    build_frontend();
    tauri_build::build();
}

fn emit_build_info() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&manifest_dir);

    print_git_rerun_paths(repo_root);
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    println!(
        "cargo:rustc-env=DOU_VOICE_COMMIT_HASH={}",
        git_output(repo_root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=DOU_VOICE_COMMIT_SHORT_HASH={}",
        git_output(repo_root, &["rev-parse", "--short=12", "HEAD"])
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=DOU_VOICE_GIT_DIRTY={}",
        git_dirty(repo_root)
    );
    println!(
        "cargo:rustc-env=DOU_VOICE_BUILD_UNIX_MS={}",
        build_unix_ms()
    );
    println!(
        "cargo:rustc-env=DOU_VOICE_BUILD_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=DOU_VOICE_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string())
    );
}

fn build_frontend() {
    println!("cargo:rerun-if-env-changed=DOU_VOICE_SKIP_FRONTEND_BUILD");
    if std::env::var_os("DOU_VOICE_SKIP_FRONTEND_BUILD").is_some() {
        println!(
            "cargo:warning=Skipping frontend build because DOU_VOICE_SKIP_FRONTEND_BUILD is set"
        );
        return;
    }

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let desktop_dir = manifest_dir
        .parent()
        .expect("src-tauri should have a parent desktop directory");

    for path in [
        "package.json",
        "bun.lock",
        "build-web.js",
        "main-src",
        "overlay-src",
        "web/index.html",
        "web/overlay.html",
        "web/scripts/tauri-api.js",
        "web/styles",
    ] {
        print_rerun_if_changed(&desktop_dir.join(path));
    }

    let bun = if cfg!(windows) { "bun.exe" } else { "bun" };
    if !desktop_dir.join("node_modules").exists() {
        run_command(desktop_dir, bun, &["install"]);
    }
    run_command(desktop_dir, bun, &["run", "build:web"]);
}

fn run_command(workdir: &Path, program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run `{}` in {}: {error}. Install Bun or set DOU_VOICE_SKIP_FRONTEND_BUILD=1 to skip frontend bundling.",
                command_line(program, args),
                workdir.display()
            )
        });
    if !status.success() {
        panic!(
            "frontend command failed: `{}` in {} exited with {status}",
            command_line(program, args),
            workdir.display()
        );
    }
}

fn print_git_rerun_paths(repo_root: &Path) {
    let git_dir = repo_root.join(".git");
    print_rerun_if_changed(&git_dir.join("HEAD"));
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) {
        if let Some(reference) = head.strip_prefix("ref: ") {
            print_rerun_if_changed(&git_dir.join(reference.trim()));
        }
    }
}

fn git_output(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_dirty(repo_root: &Path) -> bool {
    Command::new("git")
        .args(["diff", "--quiet", "--ignore-submodules", "--"])
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .status()
        .map(|status| !status.success())
        .unwrap_or(false)
}

fn build_unix_ms() -> String {
    if let Ok(epoch_seconds) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(seconds) = epoch_seconds.parse::<u128>() {
            return (seconds * 1_000).to_string();
        }
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn print_rerun_if_changed(path: &Path) {
    if path.is_dir() {
        println!("cargo:rerun-if-changed={}", path.display());
        for entry in std::fs::read_dir(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
            .flatten()
        {
            print_rerun_if_changed(&entry.path());
        }
        return;
    }
    println!("cargo:rerun-if-changed={}", path.display());
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}
