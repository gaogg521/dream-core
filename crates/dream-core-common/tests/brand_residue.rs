//! Ratchet: keep pre-rebrand brand names out of the workspace, without eating the
//! compat layers that deliberately carry them.
//!
//! This repo was copied from an upstream project and renamed. Two bulk sweeps have
//! now collapsed a legacy value onto its current spelling — silently turning a
//! dual-write into the same name written twice and dropping the fallback. The
//! `concat!` trick in `dream_core_ai_agent::types` exists because of the first one.
//!
//! So this check is deliberately NOT "no old names anywhere". Two things are
//! allowed:
//!
//! 1. **The pinned literals in `PINNED_VALUES`.** Each is a persisted value or an
//!    on-disk name that exists on real installs — renaming one loses user data or
//!    breaks a wire contract. They are listed here, once, instead of demanding a
//!    comment beside all ~100 call sites that exercise them.
//! 2. **Any occurrence whose surrounding code says it is deliberate** — the word
//!    `legacy`, `pre-rebrand` or `pre-fork` within a few lines, which is where the
//!    explaining comment or the `LEGACY_*` binding sits.
//!
//! Everything else is flagged. If this fails, the fix is normally to rename the
//! thing. When the value is persisted or crosses a process boundary, keep a read of
//! the old name beside it and say `legacy` in that comment instead — and if it is a
//! genuinely new pinned value, add it to `PINNED_VALUES` with a reason.

use std::fs;
use std::path::{Path, PathBuf};

/// Directories whose whole point is to record what already happened, plus build output.
///
/// NOTE: migrations are matched by directory NAME at any depth (see `collect`), not
/// listed here — every `dream-domain-*` crate has its own pair, and a sweep that
/// edited eleven of them rewrote a `WHERE agent_type = '<old value>'` into a clause
/// that matches nothing (and changed checksums recorded on real installs).
const SKIPPED_PREFIXES: &[&str] = &[
    // Session notes and design docs describe the upstream project by name.
    "docs",
    // Build output, and the runtime-materialized copy of the builtin-skills corpus
    // (`data/builtin-skills` is regenerated from `crates/dream-core-app/assets`).
    "target",
    "target-test",
    "data",
    ".git",
];

/// Whole files whose subject IS the rename. Every old name in them is a record of
/// what was decided, not residue — scrubbing them destroys the reasoning.
const SKIPPED_FILES: &[&str] = &[
    "DREAM-SETUP-NOTES.md",
    // release-please regenerates this file from commit subjects, and three of
    // those subjects belong to the rename commits themselves -- they carry the
    // pre-rebrand names because naming them is what those commits were for.
    // Editing the file would falsify the history AND be undone by the bot on the
    // next release, so without this entry every release turns main red. That is
    // not hypothetical: it happened the moment 0.1.72 was cut.
    "CHANGELOG.md",
];

/// A migration test has to reproduce the PRE-migration database byte for byte:
/// the row ids, the `agent_type` values, the skill names the migration rewrites.
/// Renaming any of them makes the migration's own `WHERE` clause match nothing and
/// the test then proves the opposite of what it claims. Matched on the file name.
fn is_migration_fixture(rel: &str) -> bool {
    rel.contains("/tests/") && rel.ends_with(".rs") && rel.contains("migration")
}

const MARKERS: &[&str] = &["legacy", "pre-rebrand", "pre-fork"];
// Case variants of the bare stem, so `Aion CLI`, `aion_tools` and `AionUi` all match.
const BRAND_NEEDLES: &[&str] = &["aion", "Aion", "AION"];

/// Literals that must keep their spelling, with the reason they are pinned.
/// A hit is allowed when the needle sits inside one of these.
const PINNED_VALUES: &[(&str, &str)] = &[
    (
        "\"aionrs\"",
        "persisted agent_type / conversation-type wire value; see `AgentType`'s serde alias",
    ),
    (".aionrs/sessions", "session directory written by pre-rebrand builds"),
    (".aionrs.log", "backend log suffix on existing installs"),
    (".aioncore.log", "the other backend log suffix on existing installs"),
    ("\"aionrs-sessions\"", "value of LEGACY_AGENT_SESSIONS_DIR"),
    (
        "\"aionpro\"",
        "serde value of IdentityMode::DreamPro, persisted in external IdP rows",
    ),
    ("'aionpro'", "same value, written literally in SQL fixtures and seeds"),
    (
        "aion-extension.json",
        "value of LEGACY_EXTENSION_MANIFEST_FILE; extensions on disk still use it",
    ),
    (
        "aion_compact",
        "pre-rename engine tracing target, still emitted by pinned engine binaries",
    ),
    (
        "aion_tools",
        "pre-rename engine tracing target, still emitted by pinned engine binaries",
    ),
    (
        "aion_skills",
        "pre-rename engine tracing target, still emitted by pinned engine binaries",
    ),
    (
        "aion_memory",
        "pre-rename engine tracing target, still emitted by pinned engine binaries",
    ),
    (
        "AionHub",
        "third-party hub the extension bundle is downloaded from at build time",
    ),
    (
        "Aion CLI",
        "display name seeded by migration 001; migration-state fixtures must use it",
    ),
    (
        "Aion cli",
        "same seeded display name, lower-cased in a couple of fixtures",
    ),
    ("Aion cron", "cron row name in the same pre-migration fixture"),
    (
        "aion_cli",
        "identifier form of that seeded name, in migration names and their tests",
    ),
    (
        "aion-cli",
        "pre-migration channel `backend` input alias, accepted beside the legacy agent-type value",
    ),
    (
        "aion.svg",
        "icon path seeded by migration 001; migration 057 is what repoints it",
    ),
    (
        "aionhub",
        "third-party hub the extension bundle is downloaded from at build time",
    ),
];

/// How many lines either side of a hit are searched for a deliberateness marker.
/// Both directions: the explaining comment comes first, the `LEGACY_*` binding after.
const WINDOW: usize = 12;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/dream-core-common.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn is_scannable(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "toml" | "sql" | "json" | "ps1" | "sh" | "md" | "js" | "mjs" | "cjs" | "ts" | "py" | "yml" | "yaml"
        )
    )
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let skipped = SKIPPED_PREFIXES
            .iter()
            .any(|p| rel == *p || rel.starts_with(&format!("{p}/")))
            // Design notes live under `docs/` at every level, not only the repo root,
            // and every crate carries its own `migrations/` + `migrations_mysql/`.
            || rel
                .split('/')
                .any(|seg| matches!(seg, "docs" | "migrations" | "migrations_mysql"))
            || SKIPPED_FILES.contains(&rel.as_str())
            || is_migration_fixture(&rel);
        if skipped {
            continue;
        }
        if path.is_dir() {
            collect(&path, root, out);
        } else if is_scannable(&path) {
            out.push(path);
        }
    }
}

fn marked_deliberate(lines: &[&str], index: usize) -> bool {
    let start = index.saturating_sub(WINDOW);
    let end = (index + WINDOW + 1).min(lines.len());
    let window = lines[start..end].join("\n").to_lowercase();
    MARKERS.iter().any(|m| window.contains(m))
}

/// Blank out every pinned literal so only unpinned residue is left to match.
fn without_pinned(line: &str) -> String {
    let mut out = line.to_owned();
    for (lit, _) in PINNED_VALUES {
        out = out.replace(lit, " ");
    }
    out
}

#[test]
fn no_unmarked_pre_rebrand_brand_names() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    assert!(files.len() > 100, "expected to scan the workspace, saw {}", files.len());

    let mut offenders = Vec::new();
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else { continue };
        if !BRAND_NEEDLES.iter().any(|n| text.contains(n)) {
            continue;
        }
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let residue = without_pinned(line);
            if !BRAND_NEEDLES.iter().any(|n| residue.contains(n)) {
                continue;
            }
            if marked_deliberate(&lines, i) {
                continue;
            }
            // char-wise, not byte-wise: these files are full of CJK.
            let excerpt: String = line.trim().chars().take(110).collect();
            offenders.push(format!("{rel}:{}  {excerpt}", i + 1));
        }
    }

    assert!(
        offenders.is_empty(),
        "Pre-rebrand brand names with no deliberateness marker ({} lines).\n\
         Rename them. If the value is persisted or crosses a process boundary, keep a read of\n\
         the old name beside it and say \"legacy\" in that comment — do not just add a skip.\n\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// The pinned list is the only blanket allowance, so it must stay small and explained.
#[test]
fn every_pinned_value_carries_a_reason() {
    for (lit, why) in PINNED_VALUES {
        assert!(!lit.is_empty(), "pinned literal must not be empty");
        assert!(
            why.len() > 20,
            "pinned literal {lit:?} needs a real reason, got {why:?}"
        );
        assert!(
            BRAND_NEEDLES.iter().any(|n| lit.contains(n)),
            "pinned literal {lit:?} does not contain a brand needle — it would allow nothing"
        );
    }
}
