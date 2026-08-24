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

/// GOG descriptions are store HTML: `<p class="module">`, `<b>`, even
/// `<img src=...>`. Storing that raw would hand third-party markup straight to
/// whatever renders it — markup in a text field at best, an injection vector
/// in the web UI at worst. Reduced to plain text here, once, rather than
/// asking every client to sanitise it.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            // A block break should read as a break, not as glued-together words.
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    // Tags become spaces, so `<b>D&D</b>.` would read "D&D ." — close the gap
    // rather than leaving one in front of every full stop.
    let mut out = String::with_capacity(collapsed.len());
    let mut chars = collapsed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' '
            && chars
                .peek()
                .is_some_and(|n| matches!(n, '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']'))
        {
            continue;
        }
        out.push(c);
    }
    out
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
        // `lead` when GOG has one, `full` otherwise. GOG sometimes puts a
        // store banner in `lead` (BG3's is "Cross-platform multiplayer with
        // Steam is supported"), but that is a data-quality quirk on their
        // side and no length heuristic distinguishes it from a real blurb.
        description: {
            let lead = html_to_text(v["description"]["lead"].as_str().unwrap_or_default());
            if lead.is_empty() {
                html_to_text(v["description"]["full"].as_str().unwrap_or_default())
            } else {
                lead
            }
        },
        art_urls,
    })
}

pub fn gog_product_url(store_id: &str) -> String {
    format!("https://api.gog.com/products/{store_id}?expand=description")
}

/// v2 is the only GOG endpoint that exposes real box art. v1's `images.logo2x`
/// is a 200x120 landscape logo — fine in a store listing, useless as a tile.
/// Verified 2026-08-24: v2 boxArtImage is 342x482 portrait for all three games
/// on the NAS.
pub fn gog_boxart_url(store_id: &str) -> String {
    format!("https://api.gog.com/v2/games/{store_id}")
}

pub fn parse_gog_boxart(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v["_links"]["boxArtImage"]["href"]
        .as_str()
        .map(String::from)
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

/// umu needs a Steam appid for protonfixes, so every Proton game already
/// carries one as `GAMEID=umu-<appid>`. That is a free, exact Steam id: no
/// title matching, no key. It unlocks Steam's 600x900 box art (larger than
/// GOG's 342x482) and ProtonDB, which is keyed by appid and nothing else.
pub fn steam_appid_from_runner(runner_json: &str) -> Option<String> {
    let runner: serde_json::Value = serde_json::from_str(runner_json).ok()?;
    let env = runner["env"].as_array()?;
    for e in env {
        let val = e.as_str()?;
        if let Some(id) = val.strip_prefix("GAMEID=umu-") {
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

pub fn protondb_url(appid: &str) -> String {
    format!("https://www.protondb.com/api/v1/reports/summaries/{appid}.json")
}

/// The tier is the whole point of the badge; the rest of the payload is noise
/// for a library tile.
pub fn parse_protondb(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v["tier"].as_str().map(String::from)
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

#[cfg(test)]
mod appid_tests {
    use super::*;

    #[test]
    fn finds_the_steam_appid_umu_already_carries() {
        // Exactly as it appears on the NAS today.
        let runner =
            r#"{"type":"docker","env":["LANG=en_US.UTF-8","GAMEID=umu-201810","STORE=gog"]}"#;
        assert_eq!(steam_appid_from_runner(runner).as_deref(), Some("201810"));
    }

    #[test]
    fn a_game_without_a_gameid_has_no_appid() {
        let runner = r#"{"type":"docker","env":["RUN_EXE=/games/x/x.exe"]}"#;
        assert!(steam_appid_from_runner(runner).is_none());
    }

    #[test]
    fn a_non_numeric_gameid_is_not_an_appid() {
        // umu also accepts named ids like GAMEID=umu-default.
        let runner = r#"{"env":["GAMEID=umu-default"]}"#;
        assert!(steam_appid_from_runner(runner).is_none());
    }

    #[test]
    fn reads_the_protondb_tier_and_ignores_the_rest() {
        let body = r#"{"bestReportedTier":"platinum","confidence":"strong","score":0.89,
                       "tier":"gold","total":257,"trendingTier":"platinum"}"#;
        // The current tier, not the best anyone ever reported.
        assert_eq!(parse_protondb(body).as_deref(), Some("gold"));
    }

    #[test]
    fn a_game_with_no_protondb_entry_is_not_an_error() {
        assert!(parse_protondb("{}").is_none());
    }
}

#[cfg(test)]
mod html_tests {
    use super::*;

    #[test]
    fn strips_the_markup_gog_actually_sends() {
        let html = r#"<p class="module">Cross-platform <b>multiplayer</b> is supported.</p>"#;
        assert_eq!(
            html_to_text(html),
            "Cross-platform multiplayer is supported."
        );
    }

    #[test]
    fn an_image_tag_leaves_nothing_behind() {
        // GOG embeds these; a client must never be handed a live img tag.
        let html = r#"<b>Update!</b><br><img src="https://x/y.jpg" onerror="alert(1)">Text"#;
        let text = html_to_text(html);
        assert!(!text.contains('<'), "got: {text}");
        assert!(!text.contains("onerror"), "got: {text}");
        assert!(text.contains("Update!") && text.contains("Text"));
    }

    #[test]
    fn a_tag_before_punctuation_does_not_leave_a_gap() {
        assert_eq!(html_to_text("set in <b>D&amp;D</b>."), "set in D&D.");
        assert_eq!(html_to_text("<i>one</i>, <i>two</i>!"), "one, two!");
    }

    #[test]
    fn entities_are_decoded_and_whitespace_collapsed() {
        assert_eq!(html_to_text("A &amp; B\n\n  C"), "A & B C");
    }

    #[test]
    fn an_empty_lead_falls_back_to_the_full_description() {
        let body = r#"{"description":{"lead":"","full":"<p>The actual blurb.</p>"}}"#;
        assert_eq!(parse_gog(body).unwrap().description, "The actual blurb.");
    }

    #[test]
    fn a_short_lead_is_still_the_lead() {
        let body = r#"{"description":{"lead":"Wolfenstein returns.","full":"<p>Long.</p>"}}"#;
        assert_eq!(parse_gog(body).unwrap().description, "Wolfenstein returns.");
    }
}

/// How a Steam appid was arrived at. Worth recording: everything else in the
/// chain is exact (a GOG id read from disk, an appid umu already needed), but
/// a title lookup is a guess, and a guess should be visible as one.
pub const APPID_FROM_ADMIN: &str = "admin";
pub const APPID_FROM_GAMEID: &str = "gameid";
pub const APPID_FROM_TITLE: &str = "title-match";

pub fn steam_search_url(title: &str) -> String {
    // Steam's community search; keyless. Encode enough that titles with
    // spaces, colons and apostrophes survive.
    let encoded: String = title
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|b| format!("%{b:02X}"))
                .collect(),
        })
        .collect();
    format!("https://steamcommunity.com/actions/SearchApps/{encoded}")
}

/// Only an exact title match counts. Taking the top hit blindly picks the
/// wrong game as soon as a title has sequels or remasters — "Wolfenstein: The
/// New Order" returns six results, and being first is not the same as being
/// right.
pub fn parse_steam_search(body: &str, wanted_title: &str) -> Option<String> {
    let results: serde_json::Value = serde_json::from_str(body).ok()?;
    let wanted = wanted_title.trim().to_lowercase();
    for entry in results.as_array()? {
        let name = entry["name"].as_str()?.trim().to_lowercase();
        if name == wanted {
            return entry["appid"].as_str().map(String::from);
        }
    }
    None
}

pub fn steam_appdetails_url(appid: &str) -> String {
    format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=english")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SteamDetails {
    pub tagline: String,
    pub release_date: String,
    pub developer: String,
    pub genres: String,
}

/// Steam prints dates for humans ("3 Aug, 2023"). GOG gives ISO. A field that
/// is sometimes one and sometimes the other is worse than either, so Steam's
/// is normalised on the way in.
pub fn iso_date(display: &str) -> String {
    let cleaned = display.replace(',', "");
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    let month = |m: &str| -> Option<&'static str> {
        Some(match &m.to_lowercase()[..3.min(m.len())] {
            "jan" => "01",
            "feb" => "02",
            "mar" => "03",
            "apr" => "04",
            "may" => "05",
            "jun" => "06",
            "jul" => "07",
            "aug" => "08",
            "sep" => "09",
            "oct" => "10",
            "nov" => "11",
            "dec" => "12",
            _ => return None,
        })
    };
    match parts.as_slice() {
        // "3 Aug 2023"
        [d, m, y] if d.parse::<u32>().is_ok() => match month(m) {
            Some(mm) => format!("{y}-{mm}-{:02}", d.parse::<u32>().unwrap_or(1)),
            None => display.to_string(),
        },
        // "Aug 3 2023"
        [m, d, y] if d.parse::<u32>().is_ok() => match month(m) {
            Some(mm) => format!("{y}-{mm}-{:02}", d.parse::<u32>().unwrap_or(1)),
            None => display.to_string(),
        },
        _ => display.to_string(),
    }
}

pub fn parse_steam_details(body: &str, appid: &str) -> Option<SteamDetails> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let entry = &v[appid];
    if entry["success"] != serde_json::Value::Bool(true) {
        return None;
    }
    let d = &entry["data"];
    let join = |key: &str, field: &str| -> String {
        d[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x[field].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    Some(SteamDetails {
        tagline: html_to_text(d["short_description"].as_str().unwrap_or_default()),
        release_date: iso_date(d["release_date"]["date"].as_str().unwrap_or_default()),
        developer: d["developers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        genres: join("genres", "description"),
    })
}

#[cfg(test)]
mod steam_tests {
    use super::*;

    // The real shape from steamcommunity.com/actions/SearchApps, 2026-08-24.
    const SEARCH: &str = r#"[
        {"appid":"201810","name":"Wolfenstein: The New Order"},
        {"appid":"201790","name":"Wolfenstein: The Old Blood"}
    ]"#;

    #[test]
    fn an_exact_title_resolves_to_its_appid() {
        assert_eq!(
            parse_steam_search(SEARCH, "Wolfenstein: The New Order").as_deref(),
            Some("201810")
        );
    }

    #[test]
    fn matching_ignores_case_and_surrounding_space() {
        assert_eq!(
            parse_steam_search(SEARCH, "  wolfenstein: the new order ").as_deref(),
            Some("201810")
        );
    }

    #[test]
    fn a_near_miss_is_not_accepted_just_because_it_is_first() {
        // The whole point: "first result" and "right result" are different.
        assert!(parse_steam_search(SEARCH, "Wolfenstein").is_none());
        assert!(parse_steam_search(SEARCH, "Wolfenstein: The New Colossus").is_none());
    }

    #[test]
    fn no_results_is_not_a_match() {
        assert!(parse_steam_search("[]", "Anything").is_none());
    }

    #[test]
    fn a_title_with_punctuation_survives_the_url() {
        let url = steam_search_url("Baldur's Gate 3");
        assert!(!url.contains(' '), "got {url}");
        assert!(url.contains("Baldur") && url.contains("%27"), "got {url}");
    }

    #[test]
    fn steam_dates_are_normalised_to_iso() {
        assert_eq!(iso_date("3 Aug, 2023"), "2023-08-03");
        assert_eq!(iso_date("19 May, 2014"), "2014-05-19");
        assert_eq!(iso_date("Aug 3, 2023"), "2023-08-03");
        // Anything unrecognised is passed through rather than mangled.
        assert_eq!(iso_date("Coming soon"), "Coming soon");
    }

    #[test]
    fn reads_the_fields_gog_does_not_have() {
        let body = r#"{"1086940":{"success":true,"data":{
            "short_description":"A story-rich, party-based RPG set in <b>D&amp;D</b>.",
            "release_date":{"coming_soon":false,"date":"3 Aug, 2023"},
            "developers":["Larian Studios"],
            "genres":[{"description":"Adventure"},{"description":"RPG"}]
        }}}"#;
        let d = parse_steam_details(body, "1086940").unwrap();
        assert_eq!(d.tagline, "A story-rich, party-based RPG set in D&D.");
        assert_eq!(d.release_date, "2023-08-03");
        assert_eq!(d.developer, "Larian Studios");
        assert_eq!(d.genres, "Adventure, RPG");
    }

    #[test]
    fn an_unsuccessful_lookup_is_not_a_blank_record() {
        assert!(parse_steam_details(r#"{"1":{"success":false}}"#, "1").is_none());
    }
}
