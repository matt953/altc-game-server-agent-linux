use crate::error::ApiError;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Where the agent is allowed to look. Declared as a JSON array rather than a
/// separated string: paths legitimately contain both ':' and ',', and a bind
/// spec split on ':' is exactly what broke a launch on 2026-08-24.
pub fn roots() -> Vec<PathBuf> {
    let raw = std::env::var("ALTC_LIBRARY_ROOTS").unwrap_or_default();
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<String>>(&raw) {
        Ok(list) => list.into_iter().map(PathBuf::from).collect(),
        Err(e) => {
            tracing::error!("ALTC_LIBRARY_ROOTS is not a JSON array of paths: {e}");
            Vec::new()
        }
    }
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// Absent for directories: size means nothing there and stat-ing a tree
    /// would make browsing a large library slow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Resolves a requested path and proves it stays inside a configured root.
///
/// Canonicalised first, so symlinks and `..` are resolved before the check —
/// comparing the raw string would let `/games/../../etc` through.
pub fn resolve(requested: &str) -> Result<PathBuf, ApiError> {
    let roots = roots();
    if roots.is_empty() {
        return Err(ApiError::internal(
            "no library roots configured; set ALTC_LIBRARY_ROOTS and mount them into the container",
        ));
    }
    let candidate = PathBuf::from(requested);
    if !candidate.is_absolute() {
        return Err(ApiError::bad_request("path must be absolute"));
    }
    let real = candidate
        .canonicalize()
        .map_err(|_| ApiError::not_found(format!("no such path: {requested}")))?;

    for root in &roots {
        let Ok(real_root) = root.canonicalize() else {
            tracing::warn!("library root {} is not readable from the agent", root.display());
            continue;
        };
        if real == real_root || real.starts_with(&real_root) {
            return Ok(real);
        }
    }
    // Deliberately the same shape of answer as a missing path: whether
    // something exists outside the library is not the caller's business.
    Err(ApiError::not_found(format!("no such path: {requested}")))
}

pub fn browse(requested: &str) -> Result<Vec<Entry>, ApiError> {
    let dir = resolve(requested)?;
    if !dir.is_dir() {
        return Err(ApiError::bad_request("not a directory"));
    }
    let mut entries = Vec::new();
    for item in std::fs::read_dir(&dir).map_err(|e| ApiError::internal(e.to_string()))? {
        let Ok(item) = item else { continue };
        let Ok(meta) = item.metadata() else { continue };
        let name = item.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        entries.push(Entry {
            name,
            path: item.path().to_string_lossy().to_string(),
            is_dir: meta.is_dir(),
            size: (!meta.is_dir()).then(|| meta.len()),
        });
    }
    // Folders first, then alphabetical: browsing a library is a navigation
    // task, and a user is looking for a folder.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(entries)
}

/// Maps a path inside the runner container back to the host, using the game's
/// own mounts. RUN_EXE is a container path, so this is the only way to check
/// that the executable an admin picked actually exists.
pub fn host_path_for(container_path: &str, mounts: &[String]) -> Option<PathBuf> {
    let mut best: Option<(usize, PathBuf)> = None;
    for m in mounts {
        let parts: Vec<&str> = m.split(':').collect();
        if parts.len() != 3 {
            continue;
        }
        let (source, destination) = (parts[0], parts[1]);
        let Some(rest) = container_path.strip_prefix(destination) else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with('/') {
            continue; // /games/doom must not match /games/doomsday
        }
        // Longest destination wins, so nested mounts resolve correctly.
        if best.as_ref().is_none_or(|(len, _)| destination.len() > *len) {
            best = Some((
                destination.len(),
                PathBuf::from(source).join(rest.trim_start_matches('/')),
            ));
        }
    }
    best.map(|(_, p)| p)
}

pub fn exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_container_path_back_to_the_host() {
        let mounts = vec!["/mnt/HDDs/Media/Games/doom:/games/doom:rw".to_string()];
        assert_eq!(
            host_path_for("/games/doom/DOOM.exe", &mounts).unwrap(),
            PathBuf::from("/mnt/HDDs/Media/Games/doom/DOOM.exe")
        );
    }

    #[test]
    fn does_not_match_a_sibling_with_a_shared_prefix() {
        let mounts = vec!["/host/doom:/games/doom:rw".to_string()];
        assert!(host_path_for("/games/doomsday/x.exe", &mounts).is_none());
    }

    #[test]
    fn the_deepest_mount_wins() {
        let mounts = vec![
            "/host/all:/games:rw".to_string(),
            "/host/doom:/games/doom:rw".to_string(),
        ];
        assert_eq!(
            host_path_for("/games/doom/DOOM.exe", &mounts).unwrap(),
            PathBuf::from("/host/doom/DOOM.exe")
        );
    }

    #[test]
    fn an_unmapped_path_has_no_host_equivalent() {
        let mounts = vec!["/host/doom:/games/doom:rw".to_string()];
        assert!(host_path_for("/elsewhere/x.exe", &mounts).is_none());
    }

    #[test]
    fn browsing_needs_roots_configured() {
        // Not set in the test environment: must refuse rather than expose /.
        unsafe { std::env::remove_var("ALTC_LIBRARY_ROOTS") };
        let err = resolve("/etc").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
}
