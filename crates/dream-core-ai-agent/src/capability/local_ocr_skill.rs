//! Host-platform local OCR skill selection for bridged text-only sessions.
//!
//! These names are bundled with the application under `builtin-skills/`.
//! The host skill is added to the normal skill snapshot instead of placing
//! all platform variants in `auto-inject/`: a Windows session must never be
//! instructed to run a macOS or Linux command.

/// Return the bundled local-OCR skill appropriate for the host platform.
///
/// `None` deliberately means an unsupported platform. Callers must retain
/// their safe image-unavailable path rather than guessing at a shell command.
pub const fn host_local_ocr_skill_name() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("local-ocr-windows")
    } else if cfg!(target_os = "macos") {
        Some("local-ocr-macos")
    } else if cfg!(target_os = "linux") {
        Some("local-ocr-linux")
    } else {
        None
    }
}

/// Add the host OCR skill to a conversation's selected skills exactly once.
///
/// The user-visible snapshot remains intact; this is the platform default
/// that makes local OCR available without an assistant-by-assistant opt-in.
pub fn with_host_local_ocr_skill(configured_skills: &[String]) -> Vec<String> {
    let mut skills = configured_skills.to_vec();
    if let Some(name) = host_local_ocr_skill_name()
        && !skills.iter().any(|skill| skill == name)
    {
        skills.push(name.to_owned());
    }
    skills
}

/// Recognize the bundled platform skill names. Keeping this exact avoids
/// treating an unrelated skill that merely mentions an image as an OCR tool.
pub fn is_bundled_local_ocr_skill(name: &str) -> bool {
    matches!(name, "local-ocr-windows" | "local-ocr-macos" | "local-ocr-linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_skill_is_added_once() {
        let input = vec!["pdf".to_owned()];
        let output = with_host_local_ocr_skill(&input);
        assert!(output.starts_with(&input));
        if let Some(name) = host_local_ocr_skill_name() {
            assert_eq!(output.iter().filter(|item| item.as_str() == name).count(), 1);
            assert_eq!(with_host_local_ocr_skill(&output), output);
        }
    }

    #[test]
    fn only_known_platform_names_are_classified_as_bundled_ocr() {
        assert!(is_bundled_local_ocr_skill("local-ocr-windows"));
        assert!(is_bundled_local_ocr_skill("local-ocr-macos"));
        assert!(is_bundled_local_ocr_skill("local-ocr-linux"));
        assert!(!is_bundled_local_ocr_skill("image-helper"));
    }

    /// The name this module returns is matched against the `name:` in the
    /// bundled skill's frontmatter, and discovery drops anything that does not
    /// match exactly. Nothing enforces that at compile time: renaming the
    /// directory, or editing the frontmatter, would leave a text-only session
    /// silently without OCR — no error, just an image the model cannot read.
    ///
    /// Checked against the shipped asset for every platform rather than only
    /// the host, so a rename is caught wherever it is made.
    #[test]
    fn every_bundled_skill_declares_the_name_this_module_looks_for() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir")
            .join("dream-core-app/assets/builtin-skills");
        assert!(
            assets.is_dir(),
            "bundled skills are not where this test expects them: {}",
            assets.display()
        );

        for name in ["local-ocr-windows", "local-ocr-macos", "local-ocr-linux"] {
            let manifest = assets.join(name).join("SKILL.md");
            let body = std::fs::read_to_string(&manifest)
                .unwrap_or_else(|error| panic!("could not read {}: {error}", manifest.display()));
            assert!(
                body.lines().any(|line| line.trim() == format!("name: {name}")),
                "{} does not declare `name: {name}`; discovery matches on that exact string",
                manifest.display()
            );
            // The hook hands the agent this directory to run the script from.
            assert!(
                assets.join(name).join("scripts").is_dir(),
                "{name} has no scripts/ directory for the agent to run"
            );
        }
    }

    /// The host must resolve to one of the names the asset check above covers,
    /// on every platform the app ships to.
    #[test]
    fn the_host_name_is_one_of_the_bundled_ones() {
        match host_local_ocr_skill_name() {
            Some(name) => assert!(is_bundled_local_ocr_skill(name)),
            // Only reachable on a platform the app does not ship; the callers
            // keep their image-unavailable path for it.
            None => assert!(!cfg!(any(
                target_os = "windows",
                target_os = "macos",
                target_os = "linux"
            ))),
        }
    }
}
