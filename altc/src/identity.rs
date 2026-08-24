use serde::Serialize;
use std::path::Path;

/// What a game install says about itself.
///
/// Every store leaves a manifest in the install folder, so browsing to a
/// folder identifies the game outright — no typing a title, no fuzzy
/// matching, and no API key just to obtain an id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Identity {
    pub store: &'static str,
    pub store_id: String,
    /// The store's own title. GOG's is canonical, punctuation included, which
    /// is where titles like "Wolfenstein: The New Order" come from.
    pub title: String,
}

pub fn detect(folder: &Path) -> Option<Identity> {
    gog(folder).or_else(|| steam(folder))
}

/// GOG writes goggame-<id>.info, a JSON file carrying gameId and name.
fn gog(folder: &Path) -> Option<Identity> {
    for entry in std::fs::read_dir(folder).ok()? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("goggame-") || !name.ends_with(".info") {
            continue;
        }
        let body = std::fs::read_to_string(entry.path()).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
        // rootGameId is the base game for DLC-carrying installs; prefer it so
        // a DLC folder resolves to the product a user recognises.
        let id = parsed["rootGameId"]
            .as_str()
            .or_else(|| parsed["gameId"].as_str())?
            .to_string();
        let title = parsed["name"].as_str().unwrap_or_default().to_string();
        if id.is_empty() {
            continue;
        }
        return Some(Identity {
            store: "gog",
            store_id: id,
            title,
        });
    }
    None
}

/// Steam writes appmanifest_<appid>.acf in steamapps/, a key-value format.
/// The install itself sits in steamapps/common/<name>, so accept either the
/// game folder or the steamapps folder above it.
fn steam(folder: &Path) -> Option<Identity> {
    let candidates = [
        folder.to_path_buf(),
        folder.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
        folder
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_default(),
    ];
    let wanted = folder.file_name()?.to_string_lossy().to_string();

    for dir in candidates.iter().filter(|d| d.is_dir()) {
        for entry in std::fs::read_dir(dir).ok()? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                continue;
            }
            let body = std::fs::read_to_string(entry.path()).ok()?;
            let appid = acf_value(&body, "appid")?;
            let installdir = acf_value(&body, "installdir").unwrap_or_default();
            // Several manifests can share a steamapps folder; take the one
            // whose installdir matches the folder we were asked about.
            if dir != folder && installdir != wanted {
                continue;
            }
            return Some(Identity {
                store: "steam",
                store_id: appid,
                title: acf_value(&body, "name").unwrap_or(installdir),
            });
        }
    }
    None
}

/// ACF is Valve's nested key-value text: `"key"  "value"` per line. Only flat
/// top-level lookups are needed here, so this reads lines rather than pulling
/// in a parser for a format we touch in one place.
fn acf_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    for line in body.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(&needle) else {
            continue;
        };
        let rest = rest.trim();
        if !rest.starts_with('"') {
            continue;
        }
        let value: String = rest[1..].chars().take_while(|c| *c != '"').collect();
        return Some(value);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("altc-id-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_a_gog_install() {
        let d = tmp("gog");
        // Shape taken from the real file on the NAS, 2026-08-24.
        std::fs::write(
            d.join("goggame-1943729964.info"),
            r#"{"buildId":"52","clientId":"49","gameId":"1943729964",
                "name":"Wolfenstein: The New Order","rootGameId":"1943729964","version":1}"#,
        )
        .unwrap();
        let id = detect(&d).unwrap();
        assert_eq!(id.store, "gog");
        assert_eq!(id.store_id, "1943729964");
        // The colon is the store's own punctuation, not a mistake to strip.
        assert_eq!(id.title, "Wolfenstein: The New Order");
    }

    #[test]
    fn prefers_the_base_game_over_a_dlc_id() {
        let d = tmp("gogdlc");
        std::fs::write(
            d.join("goggame-2000.info"),
            r#"{"gameId":"2000","rootGameId":"1000","name":"Some DLC"}"#,
        )
        .unwrap();
        assert_eq!(detect(&d).unwrap().store_id, "1000");
    }

    #[test]
    fn reads_a_steam_install_from_the_game_folder() {
        let root = tmp("steam");
        let steamapps = root.join("steamapps");
        let game = steamapps.join("common").join("Portal");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            steamapps.join("appmanifest_400.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"400\"\n\t\"name\"\t\t\"Portal\"\n\t\"installdir\"\t\t\"Portal\"\n}\n",
        )
        .unwrap();
        let id = detect(&game).unwrap();
        assert_eq!(id.store, "steam");
        assert_eq!(id.store_id, "400");
        assert_eq!(id.title, "Portal");
    }

    #[test]
    fn does_not_claim_a_sibling_games_manifest() {
        let root = tmp("steamsibling");
        let steamapps = root.join("steamapps");
        let game = steamapps.join("common").join("HalfLife");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            steamapps.join("appmanifest_400.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"400\"\n\t\"name\"\t\t\"Portal\"\n\t\"installdir\"\t\t\"Portal\"\n}\n",
        )
        .unwrap();
        assert!(detect(&game).is_none(), "must not label HalfLife as Portal");
    }

    #[test]
    fn an_unrecognised_folder_is_not_guessed_at() {
        let d = tmp("plain");
        std::fs::write(d.join("game.exe"), b"x").unwrap();
        assert!(detect(&d).is_none());
    }
}
