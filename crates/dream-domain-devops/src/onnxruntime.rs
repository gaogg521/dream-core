//! Locating and loading the ONNX Runtime shared library.
//!
//! The local embedding model runs on ONNX Runtime. It is **not linked into**
//! `dreamcore`: the prebuilt static libraries that `ort` can download are built
//! with a newer toolchain than the release builds use (they need glibc 2.38 and
//! a recent libstdc++ on Linux, the dynamic CRT on Windows) and do not exist for
//! `x86_64-apple-darwin` at all, so linking them broke the release build on four
//! of five platforms. Instead the official Microsoft shared library travels
//! next to the binary (release archives, the desktop installer, the Docker image)
//! and is loaded here, on first use.
//!
//! A missing library therefore only disables *local* embedding, with a message
//! that says what to do. It never stops the program from starting.
//!
//! Search order:
//! 1. `ORT_DYLIB_PATH` (explicit override; also how a developer points a cargo
//!    build at a downloaded copy);
//! 2. next to the running executable, then in an `onnxruntime/` folder there;
//! 3. the same two places in up to three parent directories — a cargo build
//!    runs from `target/debug` (or `target/debug/deps` for tests) and
//!    `scripts/fetch-onnxruntime.*` puts the library in `target/onnxruntime`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// File name of the shared library on this platform, as shipped (the release
/// archives normalise the versioned names the Microsoft packages use).
pub const fn library_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}

/// Whether Microsoft publishes an ONNX Runtime build for this platform. Intel
/// Macs are not covered by any release of the version `ort` needs.
pub const fn platform_supported() -> bool {
    !(cfg!(target_os = "macos") && cfg!(target_arch = "x86_64"))
}

/// Where to look, in order, for a given executable directory.
pub fn candidate_paths(exe_dir: &Path) -> Vec<PathBuf> {
    let name = library_file_name();
    let mut out = Vec::new();
    for dir in exe_dir.ancestors().take(4) {
        out.push(dir.join(name));
        out.push(dir.join("onnxruntime").join(name));
    }
    out
}

/// The first existing candidate.
pub fn find_library(exe_dir: &Path) -> Option<PathBuf> {
    candidate_paths(exe_dir).into_iter().find(|path| path.is_file())
}

fn not_found_message(exe_dir: Option<&Path>) -> String {
    if !platform_supported() {
        return "local embedding is not available on Intel Macs (ONNX Runtime does not ship a build for this \
                platform); configure a remote embedding endpoint for the knowledge base instead"
            .to_owned();
    }
    let looked = exe_dir
        .map(|dir| format!("{} (and its parent folders)", dir.display()))
        .unwrap_or_else(|| "the application folder".to_owned());
    format!(
        "ONNX Runtime library `{}` was not found next to dreamcore (looked in {looked}); local embedding is \
         unavailable. Reinstall the app, set ORT_DYLIB_PATH to the library, or configure a remote embedding \
         endpoint for the knowledge base",
        library_file_name()
    )
}

/// Load the library once and remember the outcome. `Ok` carries the path that
/// was loaded; `Err` is a message fit to show to the person.
///
/// Must run before any `ort` API is touched: without it `ort` looks the library
/// up by bare name and *panics* when it is missing.
pub fn ensure_loaded() -> Result<&'static Path, String> {
    static LOADED: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            let explicit = std::env::var_os("ORT_DYLIB_PATH").filter(|value| !value.is_empty());
            let path = match explicit {
                Some(value) => PathBuf::from(value),
                None => {
                    let exe_dir = std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.parent().map(Path::to_path_buf));
                    match exe_dir.as_deref().and_then(find_library) {
                        Some(found) => found,
                        None => return Err(not_found_message(exe_dir.as_deref())),
                    }
                }
            };
            // Absolute, so ort does not re-resolve it relative to the executable.
            let path = std::path::absolute(&path).unwrap_or(path);
            match ort::init_from(&path) {
                Ok(_) => {
                    tracing::info!(path = %path.display(), "ONNX Runtime library loaded");
                    Ok(path)
                }
                Err(e) => Err(format!(
                    "ONNX Runtime library could not be loaded: {e}. Local embedding is unavailable; reinstall the \
                     app or configure a remote embedding endpoint for the knowledge base"
                )),
            }
        })
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn library_is_found_next_to_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe_dir = dir.path().join("bundled-dreamcore").join("win32-x64");
        touch(&exe_dir.join(library_file_name()));
        assert_eq!(find_library(&exe_dir), Some(exe_dir.join(library_file_name())));
    }

    #[test]
    fn a_cargo_build_finds_the_copy_the_fetch_script_leaves_in_target() {
        let dir = tempfile::tempdir().unwrap();
        // tests run from target/debug/deps; the script writes target/onnxruntime/.
        let exe_dir = dir.path().join("target").join("debug").join("deps");
        let lib = dir.path().join("target").join("onnxruntime").join(library_file_name());
        touch(&lib);
        std::fs::create_dir_all(&exe_dir).unwrap();
        assert_eq!(find_library(&exe_dir), Some(lib));
    }

    #[test]
    fn the_closest_copy_wins() {
        let dir = tempfile::tempdir().unwrap();
        let exe_dir = dir.path().join("app").join("bin");
        let near = exe_dir.join(library_file_name());
        touch(&near);
        touch(&dir.path().join("app").join(library_file_name()));
        assert_eq!(find_library(&exe_dir), Some(near));
    }

    #[test]
    fn nothing_found_when_the_library_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_library(&dir.path().join("a").join("b")), None);
    }

    #[test]
    fn the_not_found_message_says_what_to_do() {
        let message = not_found_message(Some(Path::new("/opt/app")));
        assert!(message.contains("local embedding"), "{message}");
        if platform_supported() {
            assert!(message.contains(library_file_name()), "{message}");
            assert!(message.contains("ORT_DYLIB_PATH"), "{message}");
        } else {
            assert!(message.contains("Intel Macs"), "{message}");
        }
    }
}
