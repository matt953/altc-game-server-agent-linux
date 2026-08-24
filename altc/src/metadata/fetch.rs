use super::{
    Metadata, art_urls_for, extension_for, gog_boxart_url, gog_product_url, parse_gog,
    parse_gog_boxart,
};
use crate::error::ApiError;
use crate::identity::Identity;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Fetches metadata and art. Kept behind a trait so the pipeline can be tested
/// without the network: a test that needs the internet to pass is a test that
/// fails for reasons unrelated to the code.
pub trait Source: Send + Sync {
    fn get_text(&self, url: &str) -> Result<String, ApiError>;
    /// Returns the bytes and the content type, so the cache can name the file
    /// after what was actually served rather than what the URL implies.
    fn get_bytes(&self, url: &str) -> Result<(Vec<u8>, String), ApiError>;
}

pub struct HttpSource {
    client: reqwest::blocking::Client,
}

impl HttpSource {
    pub fn new() -> Result<Self, ApiError> {
        let client = reqwest::blocking::Client::builder()
            // A metadata lookup must never hold up adding a game.
            .timeout(Duration::from_secs(15))
            .user_agent("altc-agent")
            .build()
            .map_err(|e| ApiError::internal(format!("http client: {e}")))?;
        Ok(Self { client })
    }
}

impl Source for HttpSource {
    fn get_text(&self, url: &str) -> Result<String, ApiError> {
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| ApiError::internal(format!("fetch {url}: {e}")))?;
        if !res.status().is_success() {
            return Err(ApiError::internal(format!(
                "{url} returned {}",
                res.status()
            )));
        }
        res.text()
            .map_err(|e| ApiError::internal(format!("read {url}: {e}")))
    }

    fn get_bytes(&self, url: &str) -> Result<(Vec<u8>, String), ApiError> {
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| ApiError::internal(format!("fetch {url}: {e}")))?;
        if !res.status().is_success() {
            return Err(ApiError::internal(format!(
                "{url} returned {}",
                res.status()
            )));
        }
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let bytes = res
            .bytes()
            .map_err(|e| ApiError::internal(format!("read {url}: {e}")))?;
        Ok((bytes.to_vec(), content_type))
    }
}

pub fn lookup(source: &dyn Source, identity: &Identity) -> Result<Metadata, ApiError> {
    match identity.store {
        "gog" => {
            let body = source.get_text(&gog_product_url(&identity.store_id))?;
            let mut meta = parse_gog(&body)?;
            // Box art lives only on v2. Failing to get it must not lose the
            // metadata we already have, so the logo stays as a fallback.
            match source
                .get_text(&gog_boxart_url(&identity.store_id))
                .ok()
                .and_then(|b| parse_gog_boxart(&b))
            {
                Some(box_art) => meta.art_urls.insert(0, box_art),
                None => tracing::warn!(
                    "metadata: no box art for gog {}; falling back to the logo",
                    identity.store_id
                ),
            }
            Ok(meta)
        }
        // Steam art needs no lookup at all; the appid is the key.
        "steam" => Ok(Metadata {
            title: identity.title.clone(),
            ..Default::default()
        }),
        other => Err(ApiError::internal(format!(
            "no metadata source for {other}"
        ))),
    }
}

/// Downloads the first art URL that works and caches it. Returns the path.
///
/// Tries in order rather than failing on the first miss: stores rotate assets,
/// and a missing 2x logo should fall back to the plain one, not leave a tile
/// blank.
pub fn cache_art(
    source: &dyn Source,
    cache_dir: &Path,
    app_id: &str,
    identity: &Identity,
    meta: &Metadata,
) -> Result<PathBuf, ApiError> {
    let urls = art_urls_for(identity, meta);
    if urls.is_empty() {
        return Err(ApiError::not_found("no art available for this game"));
    }
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| ApiError::internal(format!("art cache dir: {e}")))?;

    let mut last = String::new();
    for url in &urls {
        match source.get_bytes(url) {
            Ok((bytes, content_type)) => {
                let path = super::art_path(cache_dir, app_id, extension_for(&content_type));
                std::fs::write(&path, &bytes)
                    .map_err(|e| ApiError::internal(format!("write art: {e}")))?;
                tracing::info!("metadata: cached art for {app_id} from {url}");
                return Ok(path);
            }
            Err(e) => {
                tracing::debug!("art source {url} unusable: {}", e.1);
                last = e.1;
            }
        }
    }
    Err(ApiError::not_found(format!(
        "no art source worked (last error: {last})"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fake {
        text: HashMap<String, String>,
        bytes: HashMap<String, (Vec<u8>, String)>,
    }

    impl Source for Fake {
        fn get_text(&self, url: &str) -> Result<String, ApiError> {
            self.text
                .get(url)
                .cloned()
                .ok_or_else(|| ApiError::internal(format!("{url} returned 404")))
        }
        fn get_bytes(&self, url: &str) -> Result<(Vec<u8>, String), ApiError> {
            self.bytes
                .get(url)
                .cloned()
                .ok_or_else(|| ApiError::internal(format!("{url} returned 404")))
        }
    }

    fn gog_identity() -> Identity {
        Identity {
            store: "gog",
            store_id: "1943729964".into(),
            title: "Wolfenstein: The New Order".into(),
        }
    }

    #[test]
    fn prefers_real_box_art_over_the_store_logo() {
        // v1 gives a 200x120 landscape logo; v2 gives 342x482 portrait art.
        let fake = Fake {
            text: HashMap::from([
                (
                    gog_product_url("1943729964"),
                    r#"{"title":"W","images":{"logo2x":"//x/logo_2x.jpg"}}"#.to_string(),
                ),
                (
                    gog_boxart_url("1943729964"),
                    r#"{"_links":{"boxArtImage":{"href":"https://x/box.jpg"}}}"#.to_string(),
                ),
            ]),
            bytes: HashMap::new(),
        };
        let meta = lookup(&fake, &gog_identity()).unwrap();
        assert_eq!(meta.art_urls[0], "https://x/box.jpg");
        // The logo is kept behind it, not discarded.
        assert!(meta.art_urls.iter().any(|u| u.contains("logo")));
    }

    #[test]
    fn missing_box_art_keeps_the_metadata_and_the_logo() {
        let fake = Fake {
            text: HashMap::from([(
                gog_product_url("1943729964"),
                r#"{"title":"W","images":{"logo2x":"//x/logo_2x.jpg"}}"#.to_string(),
            )]),
            bytes: HashMap::new(),
        };
        let meta = lookup(&fake, &gog_identity()).unwrap();
        assert_eq!(meta.title, "W");
        assert_eq!(meta.art_urls[0], "https://x/logo_2x.jpg");
    }

    #[test]
    fn looks_a_gog_game_up_by_the_id_in_its_install_folder() {
        let fake = Fake {
            text: HashMap::from([(
                gog_product_url("1943729964"),
                r#"{"title":"Wolfenstein: The New Order","slug":"w","images":{"logo2x":"//x/a_2x.jpg"}}"#
                    .to_string(),
            )]),
            bytes: HashMap::new(),
        };
        let meta = lookup(&fake, &gog_identity()).unwrap();
        assert_eq!(meta.title, "Wolfenstein: The New Order");
    }

    #[test]
    fn falls_back_when_the_best_art_is_missing() {
        let meta = Metadata {
            art_urls: vec!["https://x/missing.jpg".into(), "https://x/works.jpg".into()],
            ..Default::default()
        };
        let fake = Fake {
            text: HashMap::new(),
            bytes: HashMap::from([(
                "https://x/works.jpg".to_string(),
                (b"JPEGDATA".to_vec(), "image/jpeg".to_string()),
            )]),
        };
        let dir = std::env::temp_dir().join(format!("altc-art-{}", std::process::id()));
        let path = cache_art(&fake, &dir, "42", &gog_identity(), &meta).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"JPEGDATA");
        assert!(path.ends_with("42.jpg"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_extension_follows_what_was_served_not_the_url() {
        let meta = Metadata {
            // URL says .jpg, server says png: the file must be named png.
            art_urls: vec!["https://x/lying.jpg".into()],
            ..Default::default()
        };
        let fake = Fake {
            text: HashMap::new(),
            bytes: HashMap::from([(
                "https://x/lying.jpg".to_string(),
                (b"PNGDATA".to_vec(), "image/png".to_string()),
            )]),
        };
        let dir = std::env::temp_dir().join(format!("altc-art2-{}", std::process::id()));
        let path = cache_art(&fake, &dir, "42", &gog_identity(), &meta).unwrap();
        assert!(path.ends_with("42.png"), "got {path:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_working_art_is_an_error_not_an_empty_file() {
        let meta = Metadata {
            art_urls: vec!["https://x/gone.jpg".into()],
            ..Default::default()
        };
        let fake = Fake {
            text: HashMap::new(),
            bytes: HashMap::new(),
        };
        let dir = std::env::temp_dir().join(format!("altc-art3-{}", std::process::id()));
        assert!(cache_art(&fake, &dir, "42", &gog_identity(), &meta).is_err());
        assert!(!dir.join("42.jpg").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
