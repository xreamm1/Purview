//! A signed fork engine installed beside its signed upstream bootstrap, never over it.
use crate::{backend, build_import, launcher_update};
use std::fs;

const RECORD: &str = "purview-engine.json";

pub fn supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

pub fn managed(tag: &str) -> bool {
    build_import::is_imported(tag) && backend::version_dir(tag).join(RECORD).is_file()
}

/// Custom imports and native builds keep the engine their author selected.
pub fn follows(tag: &str) -> bool {
    supported()
        && (backend::is_game_tag(tag) || crate::nightly::date_of(tag).is_some() || managed(tag))
}

pub fn install(progress: impl Fn(f32)) -> Result<String, String> {
    if !supported() {
        return Err("Purview engine downloads are available on Windows x64.".into());
    }
    let list = crate::github::json(&format!(
        "https://api.github.com/repos/{}/releases?per_page=100",
        backend::LAUNCHER_REPO,
    ))?;
    let releases = backend::parse_releases(&list, Some(launcher_update::GAME_ASSET), |tag| {
        backend::launcher_version(tag).is_some()
    });
    let release = releases
        .iter()
        .find(|r| r.asset.is_some())
        .ok_or("No Purview release provides a game engine for Windows x64.")?;
    let update = launcher_update::game(release)?;
    if !backend::installed().contains(&update.base) {
        let json = crate::github::json(&format!(
            "https://api.github.com/repos/{}/releases/tags/{}",
            backend::GAME_REPO,
            update.bootstrap_tag,
        ))?;
        let available = backend::game_releases(&serde_json::json!([json]));
        let base = available.iter().find(|r| r.tag == update.base)
            .ok_or("The pinned bootstrap no longer matches this Purview engine. Install a newer Purview release.")?;
        backend::install(base, |p| progress(p * 0.7))?;
    }
    let cache = backend::data_dir()
        .join("purview-engine")
        .join(&update.sha256);
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let program = cache.join(launcher_update::GAME_ASSET);
    let valid = backend::file_sha256(&program).is_ok_and(|hash| hash == update.sha256);
    if !valid {
        backend::download(&update.url, update.size, &update.sha256, &program, &|p| {
            progress(0.7 + p * 0.25)
        })?;
    }
    let tag = build_import::import(&program, Some(&update.base))?;
    // An existing import with the same short id must still hold these exact signed bytes.
    let imported = backend::version_dir(&tag).join(backend::CLIENT);
    horizon_release_sign::check(
        &update.release_tag,
        &update.sha256,
        &backend::file_sha256(&imported)?,
    )?;
    fs::write(
        backend::version_dir(&tag).join(RECORD),
        serde_json::to_vec_pretty(&update).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    progress(1.0);
    Ok(tag)
}
