//! Path traversal and absolute-path guards for incoming manifests.

use std::path::{Component, Path, PathBuf};

use crate::error::TransferError;

/// Ensure `rel_path` describes a location strictly inside a staging
/// directory — no `..` components, no absolute root, no
/// Windows-style drive prefix, no empty components.
///
/// Returns the joined path on success so the caller can skip
/// re-walking the components.
pub fn validate_rel_path(staging: &Path, rel_path: &Path) -> Result<PathBuf, TransferError> {
    let mut out = staging.to_path_buf();
    for component in rel_path.components() {
        match component {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {
                // `./` is a no-op; tolerate but don't do anything with it.
            }
            _ => {
                return Err(TransferError::PathTraversal {
                    rel_path: rel_path.to_path_buf(),
                });
            }
        }
    }
    // Guard against an empty manifest entry that would resolve to the
    // staging directory itself.
    if out == staging {
        return Err(TransferError::PathTraversal {
            rel_path: rel_path.to_path_buf(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staging() -> PathBuf {
        PathBuf::from("/tmp/stage")
    }

    #[test]
    fn plain_relative_path_is_accepted() {
        let got = validate_rel_path(&staging(), Path::new("foo/bar.txt")).unwrap();
        assert_eq!(got, PathBuf::from("/tmp/stage/foo/bar.txt"));
    }

    #[test]
    fn parent_dir_is_rejected() {
        assert!(matches!(
            validate_rel_path(&staging(), Path::new("foo/../escape")),
            Err(TransferError::PathTraversal { .. })
        ));
    }

    #[test]
    fn absolute_root_is_rejected() {
        assert!(matches!(
            validate_rel_path(&staging(), Path::new("/etc/passwd")),
            Err(TransferError::PathTraversal { .. })
        ));
    }

    #[test]
    fn empty_rel_path_is_rejected() {
        assert!(matches!(
            validate_rel_path(&staging(), Path::new("")),
            Err(TransferError::PathTraversal { .. })
        ));
    }

    #[test]
    fn curdir_components_are_stripped() {
        let got = validate_rel_path(&staging(), Path::new("./foo/./bar")).unwrap();
        assert_eq!(got, PathBuf::from("/tmp/stage/foo/bar"));
    }
}
