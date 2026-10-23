//! End-to-end proof for M3 team-skill distribution → consumption.
//!
//! Uses real test data to show that a skill "distributed by the admin" (a
//! [`TeamSkillPayload`]) is materialized on the member's disk and then surfaces
//! through `list_available_skills` with `source = Team` — the exact bridge the
//! agent loader relies on. Also proves the standalone-safety contract: with no
//! team skills, listing is byte-for-byte the personal-mode behavior.

use std::path::Path;

use dream_core_extension::skill_service::{SkillPaths, SkillSource, list_available_skills};
use dream_core_extension::team_sync::{TeamSkillPayload, sync_team_skills};
use tempfile::TempDir;

fn make_paths(base: &Path) -> SkillPaths {
    SkillPaths {
        data_dir: base.to_path_buf(),
        user_skills_dir: base.join("skills"),
        cron_skills_dir: base.join("cron").join("skills"),
        builtin_skills_dir: base.join("builtin-skills"),
        builtin_rules_dir: base.join("builtin-rules"),
        assistant_rules_dir: base.join("assistant-rules"),
        assistant_skills_dir: base.join("assistant-skills"),
    }
}

fn payload(id: &str, name: &str, desc: &str, body: &str) -> TeamSkillPayload {
    TeamSkillPayload {
        id: id.to_string(),
        name: name.to_string(),
        description: desc.to_string(),
        content: body.to_string(),
        auto_active: false,
        category: None,
        tags: Vec::new(),
    }
}

/// Mixed model: an admin-required skill surfaces with `auto_active = true`,
/// which is exactly what the agent loader keys on to load it without opt-in.
#[tokio::test]
async fn admin_required_skill_surfaces_auto_active() {
    let tmp = TempDir::new().unwrap();
    let paths = make_paths(tmp.path());

    let mut required = payload(
        "oskill_req",
        "security-policy",
        "Mandatory security rules",
        "Always follow policy.",
    );
    required.auto_active = true;
    sync_team_skills(
        &paths.team_skills_dir(),
        &[required, payload("oskill_opt", "optional-skill", "opt", "o")],
        true,
    )
    .await
    .unwrap();

    let listed = list_available_skills(&paths).await.unwrap();
    let req = listed.iter().find(|s| s.name == "security-policy").unwrap();
    let opt = listed.iter().find(|s| s.name == "optional-skill").unwrap();
    assert!(req.auto_active, "admin-required skill must be auto-active");
    assert!(!opt.auto_active, "optional skill stays opt-in");
}

/// Admin distributes a team skill → member syncs → it appears in the skill list
/// tagged as Team, loadable by the agent.
#[tokio::test]
async fn distributed_team_skill_surfaces_in_listing() {
    let tmp = TempDir::new().unwrap();
    let paths = make_paths(tmp.path());

    // Test data: one team skill the admin "pushed", with enterprise
    // category/tag metadata attached (C2-2).
    let mut categorized = payload(
        "oskill_report",
        "weekly-report",
        "Draft the weekly report",
        "Write a concise weekly summary.",
    );
    categorized.category = Some("数据分析".into());
    categorized.tags = vec!["SQL".into()];
    let report = sync_team_skills(&paths.team_skills_dir(), &[categorized], true)
        .await
        .unwrap();
    assert_eq!(report.written, vec!["oskill_report".to_string()]);
    assert_eq!(report.kept, 1);

    let listed = list_available_skills(&paths).await.unwrap();
    let team = listed
        .iter()
        .find(|s| s.name == "weekly-report")
        .expect("distributed team skill must appear in the skill listing");
    assert_eq!(team.source, SkillSource::Team, "must be tagged as a team skill");
    assert!(!team.is_custom, "team skills are read-only, not custom");
    assert!(
        team.description.contains("weekly report"),
        "description carried from the registry: {}",
        team.description
    );
    assert_eq!(
        team.category.as_deref(),
        Some("数据分析"),
        "category must surface in the listing"
    );
    assert_eq!(team.tags, vec!["SQL".to_string()], "tags must surface in the listing");
}

/// Standalone-safety: no team skills → listing contains none, and never errors.
#[tokio::test]
async fn standalone_listing_has_no_team_skills() {
    let tmp = TempDir::new().unwrap();
    let paths = make_paths(tmp.path());

    // No sync call at all — pure personal/standalone mode.
    let listed = list_available_skills(&paths).await.unwrap();
    assert!(
        listed.iter().all(|s| s.source != SkillSource::Team),
        "standalone mode must never surface team skills"
    );
}

/// Admin deletes a team skill on the server → an authoritative resync removes it
/// from the member's listing; unrelated skills stay.
#[tokio::test]
async fn admin_delete_propagates_to_listing() {
    let tmp = TempDir::new().unwrap();
    let paths = make_paths(tmp.path());
    let dir = paths.team_skills_dir();

    sync_team_skills(
        &dir,
        &[
            payload("oskill_a", "alpha", "a", "body a"),
            payload("oskill_b", "beta", "b", "body b"),
        ],
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        list_available_skills(&paths)
            .await
            .unwrap()
            .iter()
            .filter(|s| s.source == SkillSource::Team)
            .count(),
        2
    );

    // Server now only serves alpha (admin deleted beta) — authoritative resync.
    sync_team_skills(&dir, &[payload("oskill_a", "alpha", "a", "body a")], true)
        .await
        .unwrap();

    let listed = list_available_skills(&paths).await.unwrap();
    let team_names: Vec<&str> = listed
        .iter()
        .filter(|s| s.source == SkillSource::Team)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(team_names, vec!["alpha"], "beta must be reconciled away, alpha kept");
}

/// Distribution must reach the agent resolver, not only the settings list.
#[tokio::test]
async fn distributed_skills_resolve_by_name_and_stop_resolving_after_revocation() {
    use dream_core_db::{SqliteSkillRepository, init_database_memory};
    use dream_core_extension::skill_service::{materialize_skills_for_agent, materialize_skills_for_agent_with_repo};
    let tmp = TempDir::new().unwrap();
    let paths = make_paths(tmp.path());
    sync_team_skills(
        &paths.team_skills_dir(),
        &[
            payload("oskill_brief", "meeting-brief", "Brief", "Create meeting-brief.md"),
            payload("oskill_csv", "action-csv", "CSV", "Create action-items.csv"),
        ],
        true,
    )
    .await
    .unwrap();
    let names = vec!["meeting-brief".into(), "action-csv".into()];
    let disk = materialize_skills_for_agent(&paths, "conv-team", &names).await.unwrap();
    assert_eq!(disk.len(), 2);
    let db = init_database_memory().await.unwrap();
    let repo = SqliteSkillRepository::new(db.pool().clone());
    let resolved = materialize_skills_for_agent_with_repo(&paths, &repo, "conv-team", &names)
        .await
        .unwrap();
    assert_eq!(resolved, disk);
    assert_eq!(resolved[0].source_path, paths.team_skills_dir().join("oskill_csv"));
    assert!(
        std::fs::read_to_string(resolved[1].source_path.join("SKILL.md"))
            .unwrap()
            .contains("Create meeting-brief.md")
    );
    sync_team_skills(&paths.team_skills_dir(), &[], true).await.unwrap();
    assert!(
        materialize_skills_for_agent_with_repo(&paths, &repo, "conv-revoked", &names)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn five_hundred_distributed_skills_resolve_and_refresh_after_revocation() {
    use dream_core_extension::skill_service::materialize_skills_for_agent;
    let tmp = TempDir::new().unwrap();
    let paths = make_paths(tmp.path());
    let payloads: Vec<_> = (0..500)
        .map(|i| {
            payload(
                &format!("oskill_{i:03}"),
                &format!("team-{i:03}"),
                "Team",
                "Company guidance",
            )
        })
        .collect();
    sync_team_skills(&paths.team_skills_dir(), &payloads, true)
        .await
        .unwrap();
    let names: Vec<_> = payloads.iter().map(|p| p.name.clone()).collect();
    let started = std::time::Instant::now();
    let resolved = materialize_skills_for_agent(&paths, "conv-bulk", &names).await.unwrap();
    eprintln!("500 distributed skill resolution: {:?}", started.elapsed());
    assert_eq!(resolved.len(), 500);
    for (item, expected) in resolved.iter().zip(&payloads) {
        assert_eq!(item.name, expected.name);
        assert_eq!(item.source_path, paths.team_skills_dir().join(&expected.id));
    }
    sync_team_skills(&paths.team_skills_dir(), &payloads[..250], true)
        .await
        .unwrap();
    let refreshed = materialize_skills_for_agent(&paths, "conv-bulk-next", &names)
        .await
        .unwrap();
    assert_eq!(refreshed, resolved[..250]);
}
