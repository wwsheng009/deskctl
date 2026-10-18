use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crate is inside workspace/crates")
        .to_path_buf();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));
    let target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        workspace_root.join(target_dir)
    };

    let runtime_source = workspace_root.join("third_party/onnxruntime/win-x64");
    println!("cargo:rerun-if-changed={}", runtime_source.display());

    if cfg!(windows) {
        copy_runtime(
            &runtime_source,
            &target_dir.join(&profile),
            &["onnxruntime.dll", "onnxruntime_providers_shared.dll"],
        );
        copy_runtime(
            &runtime_source,
            &target_dir.join(&profile).join("deps"),
            &["onnxruntime.dll", "onnxruntime_providers_shared.dll"],
        );
    }

    let models_source = workspace_root.join("models");
    println!("cargo:rerun-if-changed={}", models_source.display());
    if models_source.is_dir() {
        copy_models(&models_source, &target_dir.join(&profile).join("models"));
    }
}

fn copy_runtime(source: &Path, destination: &Path, names: &[&str]) {
    if !source.is_dir() {
        println!(
            "cargo:warning=ONNX Runtime assets not found at {}; run scripts/fetch-runtime.ps1",
            source.display()
        );
        return;
    }
    if let Err(error) = std::fs::create_dir_all(destination) {
        println!(
            "cargo:warning=failed to create runtime destination {}: {error}",
            destination.display()
        );
        return;
    }
    for name in names {
        let from = source.join(name);
        if !from.is_file() {
            println!("cargo:warning=missing runtime asset {}", from.display());
            continue;
        }
        let to = destination.join(name);
        if same_file(&from, &to) {
            continue;
        }
        if let Err(error) = std::fs::copy(&from, &to) {
            println!(
                "cargo:warning=failed to copy {} to {}: {error} (stop running deskctl and rebuild if the DLL is locked)",
                from.display(),
                to.display()
            );
        }
    }
}

fn copy_models(source: &Path, destination: &Path) {
    if let Err(error) = std::fs::create_dir_all(destination) {
        println!(
            "cargo:warning=failed to create model destination {}: {error}",
            destination.display()
        );
        return;
    }
    let Ok(entries) = std::fs::read_dir(source) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let to = destination.join(name);
        if same_file(&path, &to) {
            continue;
        }
        if let Err(error) = std::fs::copy(&path, &to) {
            println!(
                "cargo:warning=failed to copy model {} to {}: {error}",
                path.display(),
                to.display()
            );
        }
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
