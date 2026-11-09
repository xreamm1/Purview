//! Signed, self-contained Windows game releases from the Purview repository.
use crate::{backend, launcher_update};

pub fn supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

pub fn managed(tag: &str) -> bool {
    backend::launcher_version(tag).is_some()
        && backend::version_dir(tag).join(".installed").is_file()
}

/// Custom imports and native builds keep the engine their author selected.
pub fn follows(tag: &str) -> bool {
    supported()
        && (backend::is_game_tag(tag) || crate::nightly::date_of(tag).is_some() || managed(tag))
}

pub(crate) fn releases(list: &serde_json::Value) -> Vec<backend::Release> {
    backend::parse_releases(list, Some(launcher_update::GAME_ASSET), |tag| {
        backend::launcher_version(tag).is_some()
    })
    .into_iter()
    .filter(|release| release.asset.is_some())
    .collect()
}

pub(crate) fn install_release(
    release: &backend::Release,
    progress: impl Fn(f32),
) -> Result<(), String> {
    let update = launcher_update::game(release)?;
    if update.bootstrap_tag != "bundled" {
        return Err("This Purview release does not contain its preparation files.".into());
    }
    if managed(&release.tag) {
        return Ok(());
    }
    backend::install_as(release, &release.tag, &update.sha256, None, progress)
}

pub fn try_install(progress: impl Fn(f32)) -> Result<Option<String>, String> {
    if !supported() {
        return Ok(None);
    }
    let list = backend::launcher_release_list()?;
    let Some(release) = releases(&list).into_iter().next() else {
        return Ok(None);
    };
    install_release(&release, progress)?;
    Ok(Some(release.tag))
}
