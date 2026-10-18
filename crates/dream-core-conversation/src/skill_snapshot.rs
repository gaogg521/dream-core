//! Pure helpers that compute `conversation.extra.skills` values. No I/O here
//! — callers (e.g. `ConversationService::create`) fetch the auto-inject name
//! set up-front and pass it in. This keeps unit tests deterministic and keeps
//! `dream-conversation` from taking a hard dep on `dream-extension` beyond
//! the `SkillResolver` trait.

use serde_json::Value;

/// Compute the initial `skills` snapshot for a brand-new conversation.
///
/// Formula: `(auto_inject − exclude_auto_inject) ∪ preset_enabled`,
/// sorted ascending, deduplicated.
pub fn compute_initial_skills(
    auto_inject: &[String],
    preset_enabled: &[String],
    exclude_auto_inject: &[String],
) -> Vec<String> {
    let excluded: std::collections::HashSet<&String> = exclude_auto_inject.iter().collect();
    let mut out: std::collections::BTreeSet<String> =
        auto_inject.iter().filter(|n| !excluded.contains(n)).cloned().collect();
    for name in preset_enabled {
        out.insert(name.clone());
    }
    out.into_iter().collect()
}

/// Mutate `extra` in place to add a `skills` array derived from legacy
/// fields if absent. Returns `true` when a mutation happened (caller
/// persists the row). Strips the legacy fields whether or not `skills`
/// was already present, so a single pass cleans up partial rows too.
///
/// Legacy formula: `(auto_inject_now − extra.exclude_builtin_skills) ∪
/// extra.enabled_skills`.
pub fn backfill_skills_if_missing(extra: &mut Value, auto_inject_now: &[String]) -> bool {
    let Some(obj) = extra.as_object_mut() else {
        return false;
    };

    let legacy_enabled = take_string_array(obj, "enabled_skills");
    let legacy_excluded = take_string_array(obj, "exclude_builtin_skills");
    let legacy_loaded = obj.remove("loaded_skills");
    let had_legacy = !legacy_enabled.is_empty() || !legacy_excluded.is_empty() || legacy_loaded.is_some();

    let needs_compute = !obj.contains_key("skills");
    if needs_compute {
        let computed = compute_initial_skills(auto_inject_now, &legacy_enabled, &legacy_excluded);
        obj.insert(
            "skills".to_owned(),
            Value::Array(computed.into_iter().map(Value::String).collect()),
        );
    }

    needs_compute || had_legacy
}

/// Whether [`backfill_skills_if_missing`] would read `auto_inject_now` for this
/// `extra`. The auto-inject set costs a walk of the skill catalog on disk, so
/// callers fetch it only when a row still lacks a `skills` snapshot.
pub fn skills_backfill_needs_catalog(extra: &Value) -> bool {
    extra.as_object().is_some_and(|obj| !obj.contains_key("skills"))
}

fn take_string_array(obj: &mut serde_json::Map<String, Value>, key: &str) -> Vec<String> {
    obj.remove(key)
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compute_initial_union_dedup_sort() {
        let skills = compute_initial_skills(
            &["cron".into(), "todo-tracker".into()],
            &["pdf".into(), "cron".into()],
            &[],
        );
        assert_eq!(skills, vec!["cron", "pdf", "todo-tracker"]);
    }

    #[test]
    fn compute_initial_applies_exclude() {
        let skills = compute_initial_skills(&["cron".into(), "todo-tracker".into()], &[], &["cron".into()]);
        assert_eq!(skills, vec!["todo-tracker"]);
    }

    #[test]
    fn compute_initial_exclude_does_not_affect_preset_opt_in() {
        // User excluded cron from auto-inject, but the preset still added it
        // explicitly — preset wins.
        let skills = compute_initial_skills(&["cron".into()], &["cron".into()], &["cron".into()]);
        assert_eq!(skills, vec!["cron"]);
    }

    #[test]
    fn backfill_writes_skills_and_strips_legacy() {
        let mut extra = json!({
            "workspace": "/tmp/foo",
            "enabled_skills": ["pdf"],
            "exclude_builtin_skills": ["cron"],
            "loaded_skills": [{"name": "cron", "description": "old cache"}],
        });
        let mutated = backfill_skills_if_missing(&mut extra, &["cron".into(), "todo-tracker".into()]);
        assert!(mutated);
        assert_eq!(extra["skills"], json!(["pdf", "todo-tracker"]));
        assert!(extra.get("enabled_skills").is_none());
        assert!(extra.get("exclude_builtin_skills").is_none());
        assert!(extra.get("loaded_skills").is_none());
    }

    #[test]
    fn backfill_noop_when_skills_present_and_no_legacy() {
        let mut extra = json!({
            "skills": ["cron"],
            "workspace": "/tmp/foo",
        });
        let mutated = backfill_skills_if_missing(&mut extra, &["cron".into()]);
        assert!(!mutated);
        assert_eq!(extra["skills"], json!(["cron"]));
    }

    #[test]
    fn backfill_strips_legacy_even_when_skills_already_present() {
        let mut extra = json!({
            "skills": ["cron"],
            "loaded_skills": [{"name": "cron", "description": "stale"}],
        });
        let mutated = backfill_skills_if_missing(&mut extra, &[]);
        assert!(mutated);
        assert!(extra.get("loaded_skills").is_none());
    }

    #[test]
    fn backfill_ignores_non_object_extra() {
        let mut extra = json!(null);
        let mutated = backfill_skills_if_missing(&mut extra, &["cron".into()]);
        assert!(!mutated);
    }

    #[test]
    fn catalog_needed_only_when_skills_missing() {
        assert!(skills_backfill_needs_catalog(&json!({ "enabled_skills": ["pdf"] })));
        assert!(skills_backfill_needs_catalog(&json!({})));
        assert!(!skills_backfill_needs_catalog(&json!({ "skills": [] })));
        assert!(!skills_backfill_needs_catalog(&json!(null)));
    }

    /// Skipping the catalog must not change the outcome for a row that already
    /// has `skills`: the empty set passed instead is never read.
    #[test]
    fn rows_with_skills_backfill_the_same_without_the_catalog() {
        let row = json!({ "skills": ["cron"], "enabled_skills": ["pdf"] });
        let mut with_catalog = row.clone();
        let mut without = row;
        assert_eq!(
            backfill_skills_if_missing(&mut with_catalog, &["auto".into()]),
            backfill_skills_if_missing(&mut without, &[])
        );
        assert_eq!(with_catalog, without);
    }
}
