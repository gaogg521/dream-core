//! Host-platform local OCR skill selection, and the command that runs it.
//!
//! These names are bundled with the application under `builtin-skills/`.
//! The host skill is added to the normal skill snapshot instead of placing
//! all platform variants in `auto-inject/`: a Windows session must never be
//! instructed to run a macOS or Linux command.
//!
//! # Two consumers, one lookup
//!
//! An ACP bridge session cannot call a tool on the host, so it is handed the
//! skill's *instructions* and runs the script itself through its shell. A
//! dream-engine session has `ReadImage`, which runs the command directly and
//! needs an argv instead. Both start from the same discovery, and the platform
//! difference is resolved in exactly one place ([`host_ocr_command`]) so a
//! fourth platform is a single edit.

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

/// The bundled OCR skill, resolved on disk.
pub struct ResolvedLocalOcr {
    pub name: String,
    /// Directory holding the skill, which is where `scripts/` lives.
    pub script_dir: std::path::PathBuf,
    /// The skill body, for the consumer that instructs an agent rather than
    /// running the command itself.
    pub instructions: String,
}

/// Find the host's bundled OCR skill through `skill_manager`.
///
/// `Ok(None)` means there is nothing to offer — an unsupported platform, or an
/// install whose bundled corpus does not carry it. That is not an error: every
/// caller has a working path without OCR, and failing the message instead
/// would be a regression for a feature that is an optimisation.
///
/// `Err` is reserved for a skill that is present but unusable, which is worth
/// surfacing because it means the install is damaged rather than minimal.
pub async fn resolve_host_local_ocr(
    skill_manager: &crate::capability::skill_manager::AcpSkillManager,
    user_id: &str,
    configured_skills: &[String],
) -> Result<Option<ResolvedLocalOcr>, String> {
    let Some(expected_name) = host_local_ocr_skill_name() else {
        return Ok(None);
    };
    let selected = with_host_local_ocr_skill(configured_skills);
    let discovered = skill_manager
        .discover_skills_for_user(user_id, Some(&selected), None)
        .await;
    if !discovered.iter().any(|skill| skill.name == expected_name) {
        return Ok(None);
    }

    let Some(definition) = skill_manager.get_skill(expected_name).await else {
        return Err(format!(
            "The default local OCR skill '{expected_name}' could not be read."
        ));
    };
    if definition.name != expected_name || !is_bundled_local_ocr_skill(&definition.name) {
        return Err(format!(
            "The default local OCR skill '{expected_name}' was replaced by an unexpected skill."
        ));
    }
    if definition.source != dream_core_extension::SkillSource::Builtin {
        return Err(format!(
            "The default local OCR skill '{expected_name}' is not a bundled skill."
        ));
    }
    let script_dir = definition
        .location
        .parent()
        .ok_or_else(|| format!("The default local OCR skill '{expected_name}' has no skill directory."))?
        .to_path_buf();
    let instructions = definition
        .body
        .filter(|body| !body.trim().is_empty())
        .ok_or_else(|| format!("The default local OCR skill '{expected_name}' has no instructions."))?;
    Ok(Some(ResolvedLocalOcr {
        name: definition.name,
        script_dir,
        instructions,
    }))
}

/// An argv for the host's OCR script. The image path is appended by the caller
/// as the final argument, which every bundled script expects.
pub struct LocalOcrCommand {
    pub program: String,
    pub args: Vec<String>,
    /// What actually reads the image, shown to the user in the result.
    pub label: String,
}

/// Build the command that runs `script_dir`'s OCR script on this platform.
///
/// The single place the three entry points differ. Each script is invoked by
/// its own interpreter because that is what it is: PowerShell for the
/// `Windows.Media.Ocr` binding, `swift` for the Vision framework one, and a
/// shell wrapper around `tesseract` that is executable in its own right.
pub fn host_ocr_command(script_dir: &std::path::Path) -> Option<LocalOcrCommand> {
    let scripts = script_dir.join("scripts");
    match host_local_ocr_skill_name()? {
        "local-ocr-windows" => Some(LocalOcrCommand {
            program: "powershell".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                // The bundled script is signed by nothing; a machine whose
                // policy blocks unsigned scripts would otherwise refuse it.
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-File".to_owned(),
                scripts.join("ocr.ps1").to_string_lossy().into_owned(),
                // Named parameter; the path follows it as the final argument.
                "-ImagePath".to_owned(),
            ],
            label: "Windows.Media.Ocr".to_owned(),
        }),
        "local-ocr-macos" => Some(LocalOcrCommand {
            program: "swift".to_owned(),
            args: vec![scripts.join("ocr.swift").to_string_lossy().into_owned()],
            label: "Apple Vision".to_owned(),
        }),
        "local-ocr-linux" => Some(LocalOcrCommand {
            program: scripts.join("ocr.sh").to_string_lossy().into_owned(),
            args: Vec::new(),
            label: "Tesseract".to_owned(),
        }),
        _ => None,
    }
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

    /// The command must name a script that the bundled skill actually ships,
    /// and must put the image path last — `ReadImage` appends it there.
    #[test]
    fn the_host_command_points_at_a_script_that_ships() {
        let Some(name) = host_local_ocr_skill_name() else {
            assert!(host_ocr_command(std::path::Path::new("/anywhere")).is_none());
            return;
        };
        let skill_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir")
            .join("dream-core-app/assets/builtin-skills")
            .join(name);

        let command = host_ocr_command(&skill_dir).expect("a supported host has a command");
        assert!(!command.label.trim().is_empty(), "the label names what read the image");

        // Whatever the platform, exactly one real script file is referenced —
        // either as the program itself or as an argument.
        let referenced: Vec<&String> = std::iter::once(&command.program)
            .chain(command.args.iter())
            .filter(|part| part.contains("ocr."))
            .collect();
        assert_eq!(referenced.len(), 1, "expected one script reference, got {referenced:?}");
        assert!(
            std::path::Path::new(referenced[0]).is_file(),
            "the command points at {}, which does not exist",
            referenced[0]
        );

        // A trailing named parameter is the only thing allowed to follow the
        // script, because the path is appended after it.
        if let Some(last) = command.args.last() {
            assert!(
                last.contains("ocr.") || last.starts_with('-'),
                "the last argument is {last}, which would swallow the appended image path"
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
