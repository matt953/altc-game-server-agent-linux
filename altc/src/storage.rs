use crate::error::ApiError;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Where the agent is allowed to look.
///
/// Read once at startup and carried in state, not fetched from the
/// environment on every call: a global read is untestable in parallel and
/// hides a boot-time misconfiguration until someone happens to browse.
#[derive(Clone, Debug, Default)]
pub struct Library {
    roots: Vec<PathBuf>,
}

impl Library {
    /// Declared as a JSON array rather than a separated string: paths
    /// legitimately contain both ':' and ',', and splitting a bind spec on
    /// ':' is exactly what broke a launch on 2026-08-24.
    pub fn from_env() -> Self {
        let raw = std::env::var("ALTC_LIBRARY_ROOTS").unwrap_or_default();
        if raw.trim().is_empty() {
            tracing::warn!(
                "ALTC_LIBRARY_ROOTS is not set: the agent cannot see the games it manages, \
                 so browsing and add-a-game validation are unavailable"
            );
            return Self::default();
        }
        match serde_json::from_str::<Vec<String>>(&raw) {
            Ok(list) => {
                let me = Self {
                    roots: list.into_iter().map(PathBuf::from).collect(),
                };
                for r in &me.roots {
                    if !r.is_dir() {
                        tracing::error!(
                            "library root {} is declared but not readable; is it mounted into the container?",
                            r.display()
                        );
                    }
                }
                me
            }
            Err(e) => {
                tracing::error!("ALTC_LIBRARY_ROOTS is not a JSON array of paths: {e}");
                Self::default()
            }
        }
    }

    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn is_configured(&self) -> bool {
        !self.roots.is_empty()
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
impl Library {
    pub fn resolve(&self, requested: &str) -> Result<PathBuf, ApiError> {
        let roots = &self.roots;
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

        for root in roots {
            let Ok(real_root) = root.canonicalize() else {
                tracing::warn!(
                    "library root {} is not readable from the agent",
                    root.display()
                );
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

    /// Whether a path would fall inside a root, WITHOUT requiring it to exist.
    ///
    /// resolve() canonicalises, which fails outright for a missing path — so it
    /// can never answer "this is in the library but is not there", which is
    /// exactly what an admin needs to be told when they pick the wrong folder.
    /// Walks up to the nearest existing ancestor and checks that instead.
    pub fn is_within(&self, path: &Path) -> bool {
        let mut probe = path;
        loop {
            if probe.exists() {
                let Ok(real) = probe.canonicalize() else {
                    return false;
                };
                return self.roots.iter().any(|root| {
                    root.canonicalize()
                        .map(|r| real == r || real.starts_with(&r))
                        .unwrap_or(false)
                });
            }
            match probe.parent() {
                Some(parent) => probe = parent,
                None => return false,
            }
        }
    }

    pub fn browse(&self, requested: &str) -> Result<Vec<Entry>, ApiError> {
        let dir = self.resolve(requested)?;
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
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }
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
        if best
            .as_ref()
            .is_none_or(|(len, _)| destination.len() > *len)
        {
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
    fn an_unconfigured_library_refuses_rather_than_exposing_the_filesystem() {
        let err = Library::default().resolve("/etc").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn traversal_cannot_escape_a_root() {
        let tmp = std::env::temp_dir().join(format!("altc-roots-{}", std::process::id()));
        let inside = tmp.join("games");
        std::fs::create_dir_all(&inside).unwrap();
        let lib = Library::new(vec![inside.clone()]);
        assert!(lib.resolve(inside.to_str().unwrap()).is_ok());
        // Resolved before the check, so this lands outside the root.
        let escape = inside.join("../../etc");
        assert!(lib.resolve(escape.to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
