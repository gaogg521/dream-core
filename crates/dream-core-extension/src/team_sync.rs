//! Team skill sync — materialize team-distributed skills onto the member's
//! disk so they survive server outages (offline-first) and are picked up by
//! the normal skill loader.
//!
//! Distribution model (see `docs/guides/enterprise-product-master-plan.zh-CN.md`,
//! mechanism M3): the enterprise admin defines team skills in the server-side
//! `one-devops` registry. A member (client mode) fetches the ones visible to
//! them and calls [`sync_team_skills`], which writes each as a real `SKILL.md`
//! under `{data_dir}/team-skills/{id}/` plus a `.team-origin` marker.
//!
//! Reconciliation deletes only skills the server no longer serves, and only
//! when the fetch was **authoritative** (server reachable). A transient
//! network failure passes `authoritative = false`, so cached team skills are
//! kept and stay usable offline — exactly the "server down doesn't affect use,
//! admin delete does" contract.

use std::collections::HashSet;
use std::path::Path;

use tracing::warn;

use crate::constants::SKILL_MANIFEST_FILE;
use crate::error::ExtensionError;

/// Marker file dropped in every materialized team-skill directory so
/// reconciliation only ever touches dirs this sync owns (never a user skill
/// that happens to share the folder). Holds the source registry id.
const TEAM_ORIGIN_MARKER: &str = ".team-origin";

/// Marker for admin-required (auto-active) team skills: member agents load
/// them without a per-assistant opt-in (mixed distribution model).
pub(crate) const TEAM_AUTO_MARKER: &str = ".team-auto";

/// One team skill as fetched from the server registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamSkillPayload {
    /// Stable server registry id; used as the on-disk directory name.
    pub id: String,
    pub name: String,
    pub description: String,
    /// SKILL.md body, or a full SKILL.md (with its own frontmatter).
    pub content: String,
    /// Admin marked this skill auto-active for member agents.
    pub auto_active: bool,
    /// Enterprise category name (C2-2), written into the frontmatter so the
    /// grouping survives offline. `None` → frontmatter carries no category.
    pub category: Option<String>,
    /// Enterprise tag names (C2-2), same treatment as `category`.
    pub tags: Vec<String>,
}

/// Outcome of a sync pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TeamSyncReport {
    /// Directory ids written or refreshed this pass.
    pub written: Vec<String>,
    /// Directory ids removed by reconciliation (server no longer serves them).
    pub removed: Vec<String>,
    /// Number of team skills present after the pass.
    pub kept: usize,
}

/// Reduce an arbitrary registry id to a safe single-segment directory name.
/// Registry ids are already `oskill_<uuid>`-shaped, but we defend against
/// path traversal regardless.
fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Insert `category`/`tags` (C2-2) into the `mapping` when present. Returns
/// the number of keys added.
fn inject_category_tags(mapping: &mut serde_yaml::Mapping, payload: &TeamSkillPayload) -> usize {
    let mut added = 0;
    if let Some(category) = payload.category.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        mapping.insert(
            serde_yaml::Value::String("category".into()),
            serde_yaml::Value::String(category.into()),
        );
        added += 1;
    }
    if !payload.tags.is_empty() {
        let tags: Vec<serde_yaml::Value> = payload
            .tags
            .iter()
            .map(|t| serde_yaml::Value::String(t.trim().to_owned()))
            .filter(|t| !matches!(t, serde_yaml::Value::String(s) if s.is_empty()))
            .collect();
        if !tags.is_empty() {
            mapping.insert(
                serde_yaml::Value::String("tags".into()),
                serde_yaml::Value::Sequence(tags),
            );
            added += 1;
        }
    }
    added
}

/// Split a document that opens with `---` into (frontmatter yaml text, body
/// after the closing fence line). Same fence semantics as
/// [`crate::skill_service::extract_frontmatter_text`]; `None` when there is
/// no closing fence.
fn split_skill_document(content: &str) -> Option<(&str, &str)> {
    let after_open = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let mut pos = 0;
    for line in after_open.lines() {
        let raw = &after_open[pos..];
        let line_len = line.len();
        let line_with_ending_len = if raw[line_len..].starts_with("\r\n") {
            line_len + 2
        } else if raw[line_len..].starts_with('\n') {
            line_len + 1
        } else {
            line_len
        };
        if line == "---" {
            let yaml = &after_open[..pos];
            let yaml = yaml
                .strip_suffix("\r\n")
                .or_else(|| yaml.strip_suffix('\n'))
                .unwrap_or(yaml);
            return Some((yaml, &after_open[pos + line_with_ending_len..]));
        }
        pos += line_with_ending_len;
    }
    None
}

/// Build a well-formed `SKILL.md`. If the registry content already carries
/// YAML frontmatter (`---` at the very start) we trust it as a complete file;
/// otherwise we synthesize frontmatter from the name/description so the skill
/// loader's frontmatter parser can pick up name + description.
///
/// Enterprise category/tag metadata (C2-2) is merged in on every pass so an
/// admin rename shows up at the next sync: a synthesized frontmatter gets the
/// keys added directly, a pre-authored one is parsed and re-serialized with
/// them inserted. A pre-authored frontmatter that cannot be parsed is left
/// untouched (the skill still loads; it just shows uncategorized).
fn build_skill_md(payload: &TeamSkillPayload) -> String {
    if payload.content.trim_start().starts_with("---") {
        if payload.category.is_none() && payload.tags.is_empty() {
            return payload.content.clone();
        }
        let Some((frontmatter, body)) = split_skill_document(&payload.content) else {
            return payload.content.clone();
        };
        let Ok(serde_yaml::Value::Mapping(mut mapping)) = serde_yaml::from_str::<serde_yaml::Value>(frontmatter) else {
            warn!(skill_id = %payload.id, "team skill frontmatter is not a YAML mapping; leaving it untouched");
            return payload.content.clone();
        };
        if inject_category_tags(&mut mapping, payload) == 0 {
            return payload.content.clone();
        }
        // `serde_yaml::to_string` ends with a newline, so `---\n{merged}---`
        // re-fences cleanly and `{body}` keeps its original leading newline.
        let merged =
            serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping)).unwrap_or_else(|_| format!("{frontmatter}\n"));
        return format!("---\n{merged}---\n{body}");
    }
    let name = payload.name.trim();
    let description = payload.description.trim().replace(['\r', '\n'], " ");
    let mut mapping = serde_yaml::Mapping::new();
    mapping.insert(
        serde_yaml::Value::String("name".into()),
        serde_yaml::Value::String(name.into()),
    );
    mapping.insert(
        serde_yaml::Value::String("description".into()),
        serde_yaml::Value::String(description.into()),
    );
    inject_category_tags(&mut mapping, payload);
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping)).unwrap_or_default();
    format!("---\n{yaml}---\n\n{}", payload.content)
}

/// Materialize `payloads` under `team_skills_dir` and reconcile removals.
///
/// `authoritative` MUST be `true` only when the payload set is the complete,
/// current server view (server reachable). When `false`, no deletion happens
/// so offline / partial-failure never wipes the local cache.
pub async fn sync_team_skills(
    team_skills_dir: &Path,
    payloads: &[TeamSkillPayload],
    authoritative: bool,
) -> Result<TeamSyncReport, ExtensionError> {
    tokio::fs::create_dir_all(team_skills_dir).await?;

    let mut report = TeamSyncReport::default();
    let mut wanted: HashSet<String> = HashSet::new();

    for payload in payloads {
        let dir_id = sanitize_id(&payload.id);
        if dir_id.is_empty() {
            continue;
        }
        let skill_dir = team_skills_dir.join(&dir_id);
        tokio::fs::create_dir_all(&skill_dir).await?;
        tokio::fs::write(skill_dir.join(SKILL_MANIFEST_FILE), build_skill_md(payload)).await?;
        tokio::fs::write(skill_dir.join(TEAM_ORIGIN_MARKER), payload.id.as_bytes()).await?;
        let auto_marker = skill_dir.join(TEAM_AUTO_MARKER);
        if payload.auto_active {
            tokio::fs::write(&auto_marker, b"1").await?;
        } else if let Err(e) = tokio::fs::remove_file(&auto_marker).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(ExtensionError::Io(e));
        }
        wanted.insert(dir_id.clone());
        report.written.push(dir_id);
    }

    if authoritative {
        let mut entries = match tokio::fs::read_dir(team_skills_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(e) => return Err(ExtensionError::Io(e)),
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Only reconcile dirs this sync owns.
            if !path.join(TEAM_ORIGIN_MARKER).exists() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if !wanted.contains(&dir_name) {
                tokio::fs::remove_dir_all(&path).await?;
                report.removed.push(dir_name);
            }
        }
    }

    report.kept = wanted.len();
    report.written.sort();
    report.removed.sort();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(id: &str, name: &str, content: &str) -> TeamSkillPayload {
        TeamSkillPayload {
            id: id.to_string(),
            name: name.to_string(),
            description: format!("{name} description"),
            content: content.to_string(),
            auto_active: false,
            category: None,
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn auto_marker_written_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        let mut p = payload("oskill_auto", "auto-skill", "body");
        p.auto_active = true;
        sync_team_skills(&dir, &[p.clone()], true).await.unwrap();
        assert!(dir.join("oskill_auto").join(TEAM_AUTO_MARKER).exists());

        // Admin flips it back to opt-in → marker cleared on resync.
        p.auto_active = false;
        sync_team_skills(&dir, &[p], true).await.unwrap();
        assert!(!dir.join("oskill_auto").join(TEAM_AUTO_MARKER).exists());
    }

    #[tokio::test]
    async fn writes_skill_md_with_synthesized_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        let report = sync_team_skills(&dir, &[payload("oskill_a", "alpha", "do alpha")], true)
            .await
            .unwrap();
        assert_eq!(report.written, vec!["oskill_a".to_string()]);
        assert_eq!(report.kept, 1);

        let md = std::fs::read_to_string(dir.join("oskill_a").join(SKILL_MANIFEST_FILE)).unwrap();
        assert!(md.starts_with("---\nname: alpha\n"));
        assert!(md.contains("do alpha"));
        assert!(dir.join("oskill_a").join(TEAM_ORIGIN_MARKER).exists());
    }

    #[tokio::test]
    async fn preserves_full_skill_md_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        let full = "---\nname: beta\ndescription: pre-authored\n---\nbody";
        sync_team_skills(&dir, &[payload("oskill_b", "ignored", full)], true)
            .await
            .unwrap();
        let md = std::fs::read_to_string(dir.join("oskill_b").join(SKILL_MANIFEST_FILE)).unwrap();
        assert_eq!(md, full);
    }

    #[tokio::test]
    async fn category_and_tags_land_in_synthesized_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        let mut p = payload("oskill_cat", "alpha", "do alpha");
        p.category = Some("数据分析".into());
        p.tags = vec!["SQL".into(), "报表".into()];
        sync_team_skills(&dir, &[p], true).await.unwrap();

        let md = std::fs::read_to_string(dir.join("oskill_cat").join(SKILL_MANIFEST_FILE)).unwrap();
        assert!(md.starts_with("---\n"), "frontmatter must open the file: {md}");
        let (name, description, category, tags) = crate::skill_service::test_parse_frontmatter(&md).unwrap();
        assert_eq!(name, "alpha");
        assert_eq!(description, "alpha description");
        assert_eq!(category.as_deref(), Some("数据分析"));
        assert_eq!(tags, vec!["SQL".to_string(), "报表".to_string()]);
    }

    #[tokio::test]
    async fn category_and_tags_merge_into_preauthored_frontmatter_and_survive_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        let full = "---\nname: beta\ndescription: pre-authored\n---\nbody with --- hr\n\n---\nmore";
        let mut p = payload("oskill_b", "ignored", full);
        p.category = Some("old-cat".into());
        p.tags = vec!["t1".into()];
        sync_team_skills(&dir, &[p.clone()], true).await.unwrap();

        let md = std::fs::read_to_string(dir.join("oskill_b").join(SKILL_MANIFEST_FILE)).unwrap();
        let (name, description, category, tags) = crate::skill_service::test_parse_frontmatter(&md).unwrap();
        assert_eq!(name, "beta");
        assert_eq!(description, "pre-authored");
        assert_eq!(category.as_deref(), Some("old-cat"));
        assert_eq!(tags, vec!["t1".to_string()]);
        assert!(md.contains("body with --- hr"), "body must survive the merge: {md}");

        // Admin renames the category → the next sync rewrites the frontmatter.
        p.category = Some("new-cat".into());
        p.tags = vec!["t2".into()];
        sync_team_skills(&dir, &[p], true).await.unwrap();
        let md = std::fs::read_to_string(dir.join("oskill_b").join(SKILL_MANIFEST_FILE)).unwrap();
        let (_, _, category, tags) = crate::skill_service::test_parse_frontmatter(&md).unwrap();
        assert_eq!(category.as_deref(), Some("new-cat"));
        assert_eq!(tags, vec!["t2".to_string()]);
        assert!(
            !md.contains("old-cat") && !md.contains("t1"),
            "stale metadata must be gone: {md}"
        );
    }

    #[tokio::test]
    async fn removed_category_drops_frontmatter_key_on_resync() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        let mut p = payload("oskill_c", "gamma", "body");
        p.category = Some("数据分析".into());
        p.tags = vec!["SQL".into()];
        sync_team_skills(&dir, &[p.clone()], true).await.unwrap();
        p.category = None;
        p.tags = Vec::new();
        sync_team_skills(&dir, &[p], true).await.unwrap();

        let md = std::fs::read_to_string(dir.join("oskill_c").join(SKILL_MANIFEST_FILE)).unwrap();
        let (_, _, category, tags) = crate::skill_service::test_parse_frontmatter(&md).unwrap();
        assert_eq!(category, None);
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn unparseable_preauthored_frontmatter_is_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        // `[name` is not valid YAML (unclosed flow sequence).
        let full = "---\nname: [beta\n---\nbody";
        let mut p = payload("oskill_d", "ignored", full);
        p.category = Some("数据分析".into());
        sync_team_skills(&dir, &[p], true).await.unwrap();
        let md = std::fs::read_to_string(dir.join("oskill_d").join(SKILL_MANIFEST_FILE)).unwrap();
        assert_eq!(md, full);
    }

    #[tokio::test]
    async fn authoritative_sync_removes_server_deleted_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        sync_team_skills(
            &dir,
            &[payload("oskill_a", "alpha", "a"), payload("oskill_b", "beta", "b")],
            true,
        )
        .await
        .unwrap();

        // Server now only serves alpha -> beta must be reconciled away.
        let report = sync_team_skills(&dir, &[payload("oskill_a", "alpha", "a")], true)
            .await
            .unwrap();
        assert_eq!(report.removed, vec!["oskill_b".to_string()]);
        assert!(dir.join("oskill_a").exists());
        assert!(!dir.join("oskill_b").exists());
    }

    #[tokio::test]
    async fn non_authoritative_sync_keeps_cache_offline() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        sync_team_skills(
            &dir,
            &[payload("oskill_a", "alpha", "a"), payload("oskill_b", "beta", "b")],
            true,
        )
        .await
        .unwrap();

        // Offline: empty payload but NOT authoritative -> nothing removed.
        let report = sync_team_skills(&dir, &[], false).await.unwrap();
        assert!(report.removed.is_empty());
        assert!(dir.join("oskill_a").exists());
        assert!(dir.join("oskill_b").exists());
    }

    #[tokio::test]
    async fn reconcile_never_touches_foreign_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("team-skills");
        tokio::fs::create_dir_all(dir.join("hand-made")).await.unwrap();
        tokio::fs::write(dir.join("hand-made").join(SKILL_MANIFEST_FILE), "x")
            .await
            .unwrap();

        // Authoritative empty sync: the foreign dir has no marker, so it stays.
        let report = sync_team_skills(&dir, &[], true).await.unwrap();
        assert!(report.removed.is_empty());
        assert!(dir.join("hand-made").exists());
    }

    #[test]
    fn sanitize_id_blocks_traversal() {
        assert_eq!(sanitize_id("oskill_abc-123"), "oskill_abc-123");
        assert_eq!(sanitize_id("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_id("a/b\\c"), "abc");
    }
}
