//! Purview self-updates: one executable asset, with its signed digest in release notes.
//!
//! The release body contains exactly one `purview-update-v1` fenced JSON block:
//! `{"tag":"launcher@VERSION","asset":"purview.exe","sha256":"HEX","signature":"BASE64"}`.
//! Sign the usual `<sha256>  <asset>\n` checksum line using horizon-release-sign,
//! with [`signing_tag`] as the signed tag. This binds the repository, release tag,
//! file name and digest. No unsigned bootstrap or upstream key fallback is allowed.
use crate::backend::{Release, LAUNCHER_REPO};
use horizon_release_sign::PublicKey;

const OPEN: &str = "```purview-update-v1\n";
const KEYS: &str = include_str!("../update-keys.txt");

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    tag: String,
    asset: String,
    sha256: String,
    signature: String,
    game: Option<GameManifest>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GameManifest {
    asset: String,
    sha256: String,
    signature: String,
    base: String,
    bootstrap_tag: String,
}

pub const GAME_ASSET: &str = "purview-game.exe";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GameUpdate {
    pub release_tag: String,
    pub base: String,
    pub bootstrap_tag: String,
    pub sha256: String,
    pub url: String,
    pub size: u64,
}

/// The signature domain used by the publisher and the embedded verifier.
pub fn signing_tag(tag: &str) -> String {
    format!("purview-launcher:{LAUNCHER_REPO}:{tag}")
}

pub fn game_signing_tag(tag: &str, base: &str, bootstrap_tag: &str) -> String {
    format!("purview-game:{LAUNCHER_REPO}:{tag}:{base}:{bootstrap_tag}")
}

pub fn game(release: &Release) -> Result<GameUpdate, String> {
    game_with_keys(release, &horizon_release_sign::parse_keys(KEYS)?)
}

fn game_with_keys(release: &Release, keys: &[PublicKey]) -> Result<GameUpdate, String> {
    let manifest = parse_manifest(release)?;
    let game = manifest
        .game
        .ok_or("This Purview release has no signed game engine.")?;
    if game.asset != GAME_ASSET
        || !(crate::backend::is_game_tag(&game.base)
            || horizon_release_sign::is_nightly_build(&game.base))
        || !game
            .base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        || !game
            .bootstrap_tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        || game.bootstrap_tag.is_empty()
    {
        return Err("The Purview engine manifest has an invalid file or bootstrap version.".into());
    }
    let sha256 = verify_record(
        release,
        GAME_ASSET,
        &game.sha256,
        &game.signature,
        &game_signing_tag(&release.tag, &game.base, &game.bootstrap_tag),
        keys,
    )?;
    let (url, size) = release.asset.clone().ok_or("No Purview engine asset.")?;
    if size > 1 << 30 {
        return Err("The Purview engine is too large.".into());
    }
    Ok(GameUpdate {
        release_tag: release.tag.clone(),
        base: game.base,
        bootstrap_tag: game.bootstrap_tag,
        sha256,
        url,
        size,
    })
}

/// The trusted digest of this platform's executable, before any download is staged.
pub fn digest(release: &Release) -> Result<String, String> {
    let keys = horizon_release_sign::parse_keys(KEYS)?;
    digest_with_keys(release, &keys)
}

fn digest_with_keys(release: &Release, keys: &[PublicKey]) -> Result<String, String> {
    let manifest = parse_manifest(release)?;
    let asset = crate::backend::launcher_asset()
        .ok_or("Purview self-updates are not available on this platform.")?;
    if manifest.asset != asset {
        return Err("The Purview update manifest names another platform.".into());
    }
    verify_record(
        release,
        asset,
        &manifest.sha256,
        &manifest.signature,
        &signing_tag(&release.tag),
        keys,
    )
}

fn parse_manifest(release: &Release) -> Result<Manifest, String> {
    if crate::backend::launcher_version(&release.tag).is_none() {
        return Err("This is not a Purview launcher release tag.".into());
    }
    let body = release.body.replace("\r\n", "\n");
    let (_, rest) = body
        .split_once(OPEN)
        .ok_or("This Purview release has no signed update manifest.")?;
    if rest.contains(OPEN) {
        return Err("This Purview release has more than one update manifest.".into());
    }
    let (json, _) = rest
        .split_once("\n```")
        .ok_or("This Purview release has an incomplete update manifest.")?;
    let manifest: Manifest =
        serde_json::from_str(json).map_err(|e| format!("Invalid Purview update manifest: {e}"))?;
    if manifest.tag != release.tag {
        return Err("The Purview update manifest names another release.".into());
    }
    Ok(manifest)
}

fn verify_record(
    release: &Release,
    asset: &str,
    hash: &str,
    signature: &str,
    signed_for: &str,
    keys: &[PublicKey],
) -> Result<String, String> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("The Purview update manifest has an invalid SHA-256.".into());
    }
    let expected_url = format!(
        "https://github.com/{LAUNCHER_REPO}/releases/download/{}/{asset}",
        release.tag,
    );
    if !release
        .asset
        .as_ref()
        .is_some_and(|(url, _)| url == &expected_url)
    {
        return Err("The Purview update asset is not from its release repository.".into());
    }
    let sums = horizon_release_sign::sums_file(&[(asset.into(), hash.into())]);
    let listed = horizon_release_sign::verify(signed_for, sums.as_bytes(), signature, keys)?;
    listed
        .into_iter()
        .find(|(name, _)| name == asset)
        .map(|(_, hash)| hash)
        .ok_or_else(|| "The Purview update manifest does not name its executable.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_release_sign::{sha256_hex, sums_file, SigningKey};

    fn release(key: &SigningKey, tag: &str) -> Release {
        let asset = crate::backend::launcher_asset().unwrap_or("unsupported");
        let hash = sha256_hex(b"new executable");
        let sums = sums_file(&[(asset.into(), hash.clone())]);
        let signature = key.sign(&signing_tag(tag), sums.as_bytes());
        let manifest =
            serde_json::json!({"tag":tag,"asset":asset,"sha256":hash,"signature":signature});
        Release {
            tag: tag.into(),
            name: "Purview".into(),
            body: format!("Notes\n{OPEN}{manifest}\n```\n"),
            date: String::new(),
            time: String::new(),
            asset: Some((
                format!("https://github.com/{LAUNCHER_REPO}/releases/download/{tag}/{asset}"),
                14,
            )),
            prerelease: false,
            nightly: None,
        }
    }

    #[test]
    fn the_fork_trust_root_is_present_and_separate_from_game_keys() {
        let keys = horizon_release_sign::parse_keys(KEYS);
        assert!(keys.is_ok());
        let keys = keys.unwrap_or_default();
        assert!(!keys.is_empty());
        let upstream = horizon_release_sign::trusted_keys();
        assert!(keys.iter().all(|key| !upstream.contains(key)));
    }

    #[test]
    fn engine_signature_binds_the_exact_bootstrap_and_cannot_reuse_launcher_signatures(
    ) -> Result<(), String> {
        let (key, _) = SigningKey::generate()?;
        let tag = "launcher@0.3.0";
        let base = "nightly-2026-11-05-38609b7";
        let bootstrap = "nightly-2026-11-05";
        let hash = sha256_hex(b"engine");
        let sums = sums_file(&[(GAME_ASSET.into(), hash.clone())]);
        let signature = key.sign(&game_signing_tag(tag, base, bootstrap), sums.as_bytes());
        let body = |base: &str, bootstrap: &str, signature: &str| {
            let manifest = serde_json::json!({"tag":tag,"asset":"purview.exe",
                "sha256":sha256_hex(b"launcher"),"signature":"unused launcher signature",
                "game":{"asset":GAME_ASSET,"sha256":hash,"signature":signature,
                    "base":base,"bootstrap_tag":bootstrap}});
            format!("{OPEN}{manifest}\n```\n")
        };
        let mut r = release(&key, tag);
        r.asset = Some((
            format!("https://github.com/{LAUNCHER_REPO}/releases/download/{tag}/{GAME_ASSET}"),
            6,
        ));
        r.body = body(base, bootstrap, &signature);
        let verified = game_with_keys(&r, &[key.public()])?;
        assert_eq!(
            (
                verified.base.as_str(),
                verified.bootstrap_tag.as_str(),
                verified.sha256.as_str()
            ),
            (base, bootstrap, hash.as_str())
        );
        r.body = body("nightly-2026-11-05-66f3981", bootstrap, &signature);
        assert!(game_with_keys(&r, &[key.public()]).is_err());
        r.body = body(base, "nightly", &signature);
        assert!(game_with_keys(&r, &[key.public()]).is_err());
        let launcher_signature = key.sign(&signing_tag(tag), sums.as_bytes());
        r.body = body(base, bootstrap, &launcher_signature);
        assert!(game_with_keys(&r, &[key.public()]).is_err());
        r.body = body(base, bootstrap, "");
        assert!(game_with_keys(&r, &[key.public()]).is_err());
        Ok(())
    }

    #[test]
    fn update_metadata_binds_repository_tag_file_hash_and_signer() -> Result<(), String> {
        if crate::backend::launcher_asset().is_none() {
            return Ok(());
        }
        let (key, _) = SigningKey::generate()?;
        let (other, _) = SigningKey::generate()?;
        let r = release(&key, "launcher@0.2.1");
        assert_eq!(
            digest_with_keys(&r, &[key.public()]),
            Ok(sha256_hex(b"new executable"))
        );
        assert!(digest_with_keys(&r, &[other.public()]).is_err());
        assert!(
            digest(&r).is_err(),
            "a publisher's random key is not trusted"
        );
        let mut changed = r.clone();
        changed.tag = "launcher@0.2.2".into();
        assert!(digest_with_keys(&changed, &[key.public()]).is_err());
        changed = r.clone();
        changed.body = changed
            .body
            .replace(&sha256_hex(b"new executable"), &sha256_hex(b"tampered"));
        assert!(digest_with_keys(&changed, &[key.public()]).is_err());
        changed = r.clone();
        changed.body = changed.body.replace(
            crate::backend::launcher_asset().unwrap_or("unsupported"),
            "unexpected.exe",
        );
        assert!(digest_with_keys(&changed, &[key.public()]).is_err());
        changed = r.clone();
        changed.asset = Some(("https://example.test/purview.exe".into(), 14));
        assert!(digest_with_keys(&changed, &[key.public()]).is_err());
        changed = r.clone();
        changed.body.push_str(&r.body);
        assert!(digest_with_keys(&changed, &[key.public()]).is_err());
        changed.body = "Unsigned notes".into();
        assert!(digest_with_keys(&changed, &[key.public()]).is_err());
        let mut crlf = r;
        crlf.body = crlf.body.replace('\n', "\r\n");
        assert!(digest_with_keys(&crlf, &[key.public()]).is_ok());
        Ok(())
    }
}
