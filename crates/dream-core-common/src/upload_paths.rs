//! The managed upload directory, and the legacy one it replaced.
//!
//! A chat attachment does not travel as a device path. The client uploads the
//! bytes, the backend stages them under a directory it owns, and the send
//! boundary later re-resolves the `Upload` reference by checking the path is
//! still inside that directory (the D2 invariant in `dream-core-project`'s
//! `resolve_chat_file_ref`). "Which directory" is therefore a contract between
//! the write side and every read side, and they have to agree on it literally.
//!
//! They stopped agreeing during the 1ONE rebrand: uploads kept landing in
//! `<tmp>/aionui` while the conversation send boundary had already moved on to
//! `<tmp>/dream`, so an attachment that uploaded successfully was rejected as
//! outside the managed root the moment it was sent. Centralising the value here
//! is what keeps the two halves from drifting apart again.
//!
//! # Why reads accept two roots
//!
//! Renaming the directory would orphan every file already staged under the old
//! name — an attachment a user picked before the upgrade, a reference image a
//! media job still points at. So the rename is one-directional: new files are
//! written to [`upload_root`], and [`upload_roots`] — what validation and reads
//! go through — accepts the legacy directory as well. Nothing on disk moves, and
//! the legacy root simply stops accumulating new files and ages out with the
//! OS's own temp sweep.

use std::path::{Path, PathBuf};

/// Directory name new uploads are staged under, inside the OS temp directory.
pub const UPLOAD_ROOT_DIR: &str = "dream";

/// Pre-rebrand directory name. Read-only: still accepted, never written to.
pub const LEGACY_UPLOAD_ROOT_DIR: &str = "aionui";

/// Where a newly uploaded file is written.
pub fn upload_root() -> PathBuf {
    std::env::temp_dir().join(UPLOAD_ROOT_DIR)
}

/// Every directory an `Upload` reference is allowed to resolve inside.
///
/// Current root first: order is not load-bearing for containment checks, but a
/// caller that reports "the managed upload directory" in an error should name
/// the one it would write to.
pub fn upload_roots() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    vec![tmp.join(UPLOAD_ROOT_DIR), tmp.join(LEGACY_UPLOAD_ROOT_DIR)]
}

/// Is `candidate` inside any managed upload root?
///
/// Takes the roots as an argument rather than calling [`upload_roots`] so tests
/// can point it at a scratch directory instead of the real temp directory.
pub fn within_any_root(roots: &[PathBuf], candidate: &Path, within: impl Fn(&Path, &Path) -> bool) -> bool {
    roots.iter().any(|root| within(root, candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this module exists to prevent: the write side and the send
    /// boundary naming different directories, which turns every attachment into
    /// an "outside the managed root" rejection at send time.
    #[test]
    fn new_uploads_are_written_under_the_current_root() {
        assert_eq!(upload_root(), std::env::temp_dir().join("dream"));
    }

    /// Files staged before the rename must stay resolvable — a user who picked
    /// an attachment yesterday should not find it rejected today.
    #[test]
    fn reads_still_accept_the_pre_rebrand_root() {
        let roots = upload_roots();
        assert!(roots.contains(&std::env::temp_dir().join(LEGACY_UPLOAD_ROOT_DIR)));
        assert!(roots.contains(&upload_root()));
    }

    /// The root that would be written to is the one an error message should
    /// name, so it has to come first.
    #[test]
    fn the_current_root_is_listed_first() {
        assert_eq!(upload_roots().first(), Some(&upload_root()));
    }

    #[test]
    fn a_path_outside_every_root_is_rejected() {
        let roots = vec![PathBuf::from("/managed/dream"), PathBuf::from("/managed/aionui")];
        let within = |root: &Path, candidate: &Path| candidate.starts_with(root);

        assert!(within_any_root(&roots, Path::new("/managed/dream/a.png"), within));
        assert!(within_any_root(&roots, Path::new("/managed/aionui/b.png"), within));
        assert!(!within_any_root(&roots, Path::new("/elsewhere/c.png"), within));
    }

    #[test]
    fn no_roots_means_nothing_resolves() {
        let within = |root: &Path, candidate: &Path| candidate.starts_with(root);
        assert!(!within_any_root(&[], Path::new("/managed/dream/a.png"), within));
    }
}
