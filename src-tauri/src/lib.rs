pub mod adapters;
pub mod core;
pub mod seams;
#[path = "tauri/mod.rs"]
pub mod tauri_adapter;

pub fn run() {
    use std::sync::Arc;

    use ::tauri::Manager;

    use crate::adapters::fixture_catalog::FixtureCatalogStore;
    use crate::adapters::sqlite::SqliteCatalogStore;
    use crate::core::catalog::CatalogService;
    use crate::tauri_adapter::catalog_api::CatalogApi;
    use crate::tauri_adapter::commands::{inspect_skill, list_agents, list_skills};

    let catalog = CatalogApi::new(CatalogService::new(Arc::new(
        FixtureCatalogStore::library_desk(),
    )));

    ::tauri::Builder::default()
        .manage(catalog)
        .setup(|app| {
            let library_root = app
                .path()
                .home_dir()
                .map_err(|error| error.to_string())?
                .join("Library/Application Support/skill-man");
            let store = SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3"))
                .map_err(|error| error.to_string())?;
            app.manage(store);
            Ok(())
        })
        .invoke_handler(::tauri::generate_handler![
            list_skills,
            inspect_skill,
            list_agents
        ])
        .run(::tauri::generate_context!())
        .expect("Skill Man runtime failed");
}
