use std::path::Path;

fn main() {
    emit_frontend_asset_dependencies(Path::new("../dist"));
    tauri_build::build()
}

/// Tauri embeds `frontendDist` into the Rust binary. Track the generated
/// assets themselves so a frontend-only rebuild cannot ship an older embedded
/// webview payload from Cargo's build-script cache.
fn emit_frontend_asset_dependencies(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());

    let Ok(entries) = path.read_dir() else {
        return;
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            emit_frontend_asset_dependencies(&entry_path);
        } else {
            println!("cargo:rerun-if-changed={}", entry_path.display());
        }
    }
}
