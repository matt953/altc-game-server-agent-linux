pub mod fetch;

use crate::error::ApiError;
use crate::identity::Identity;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// What the pipeline found out about a game. Every field comes from a store or
/// metadata source; nothing here is typed by an admin, which is what makes a
/// populated grid proof that the pipeline actually ran.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Metadata {
    pub title: String,
    pub slug: String,
    pub release_date: String,
    pub description: String,
    /// Remote art, best first. Downloaded and cached locally.
    pub art_urls: Vec<String>,
}

/// GOG image URLs are protocol-relative and ALREADY carry their own suffix,
/// e.g. `//images-1.gog-statics.com/<hash>_glx_logo_2x.jpg`. Appending an
/// extension 404s — verified against the live API 2026-08-24.
fn absolute(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

pub fn parse_gog(body: &str) -> Result<Metadata, ApiError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ApiError::internal(format!("gog json: {e}")))?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let images = &v["images"];
    // logo2x before logo: a tile is displayed large and the 2x asset is the
    // only one with enough resolution not to look soft.
    let art_urls = ["logo2x", "logo", "background", "icon"]
        .iter()
        .filter_map(|k| images[k].as_str())
        .map(absolute)
        .collect();
    Ok(Metadata {
        title: s("title"),
        slug: s("slug"),
        release_date: s("release_date"),
        description: v["description"]["lead"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        art_urls,
    })
}

pub fn gog_product_url(store_id: &str) -> String {
    format!("https://api.gog.com/products/{store_id}?expand=description")
}

/// Steam publishes portrait box art with no key and no lookup, which is the
/// right shape for a tile where GOG's landscape logo is not.
pub fn steam_art_urls(appid: &str) -> Vec<String> {
    [
        "library_600x900_2x.jpg",
        "library_600x900.jpg",
        "header.jpg",
    ]
    .iter()
    .map(|f| format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{f}"))
    .collect()
}

pub fn art_urls_for(identity: &Identity, meta: &Metadata) -> Vec<String> {
    match identity.store {
        "steam" => steam_art_urls(&identity.store_id),
        _ => meta.art_urls.clone(),
    }
}

/// Where a game's cached art lives. Keyed by app id, not title, so renaming a
/// game does not orphan its art.
pub fn art_path(cache_dir: &Path, app_id: &str, ext: &str) -> PathBuf {
    cache_dir.join(format!("{app_id}.{ext}"))
}

pub fn extension_for(content_type: &str) -> &'static str {
    match content_type {
        t if t.contains("png") => "png",
        t if t.contains("webp") => "webp",
        _ => "jpg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from the real api.gog.com response for Wolfenstein, 2026-08-24.
    const GOG_BODY: &str = r#"{
        "id": 1943729964,
        "title": "Wolfenstein: The New Order",
        "slug": "wolfenstein_the_new_order",
        "release_date": "2014-05-19T00:00:00+0300",
        "description": {"lead": "Wolfenstein returns.", "full": "..."},
        "images": {
            "background": "//images-1.gog-statics.com/aaa_bg.jpg",
            "logo": "//images-3.gog-statics.com/bbb_glx_logo.jpg",
            "logo2x": "//images-1.gog-statics.com/bbb_glx_logo_2x.jpg",
            "icon": "//images-2.gog-statics.com/ccc_icon.png"
        }
    }"#;

    #[test]
    fn reads_gogs_own_answer() {
        let m = parse_gog(GOG_BODY).unwrap();
        assert_eq!(m.title, "Wolfenstein: The New Order");
        assert_eq!(m.slug, "wolfenstein_the_new_order");
        assert_eq!(m.release_date, "2014-05-19T00:00:00+0300");
        assert_eq!(m.description, "Wolfenstein returns.");
    }

    #[test]
    fn art_urls_become_absolute_without_gaining_an_extension() {
        let m = parse_gog(GOG_BODY).unwrap();
        // The suffix is already in the URL; appending .png/.jpg 404s.
        assert_eq!(
            m.art_urls[0],
            "https://images-1.gog-statics.com/bbb_glx_logo_2x.jpg"
        );
        assert!(m.art_urls.iter().all(|u| u.starts_with("https://")));
        assert!(!m.art_urls.iter().any(|u| u.ends_with(".jpg.jpg")));
    }

    #[test]
    fn the_high_resolution_logo_is_preferred() {
        let m = parse_gog(GOG_BODY).unwrap();
        assert!(m.art_urls[0].contains("_2x"), "got {:?}", m.art_urls);
    }

    #[test]
    fn steam_art_is_portrait_first() {
        let urls = steam_art_urls("220");
        assert!(urls[0].contains("library_600x900"));
        assert!(urls.iter().all(|u| u.contains("/steam/apps/220/")));
    }

    #[test]
    fn a_steam_game_uses_steam_art_not_the_stores_metadata_images() {
        let id = Identity {
            store: "steam",
            store_id: "220".into(),
            title: "Half-Life 2".into(),
        };
        let urls = art_urls_for(&id, &Metadata::default());
        assert!(urls[0].contains("library_600x900"));
    }

    #[test]
    fn art_is_keyed_by_id_so_a_rename_does_not_orphan_it() {
        let p = art_path(Path::new("/cache"), "578802895", "jpg");
        assert_eq!(p, PathBuf::from("/cache/578802895.jpg"));
    }

    #[test]
    fn content_type_picks_the_extension() {
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/jpeg"), "jpg");
        assert_eq!(extension_for("image/webp"), "webp");
    }
}
