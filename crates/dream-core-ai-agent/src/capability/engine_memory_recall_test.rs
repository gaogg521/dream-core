use super::*;

fn write(dir: &Path, filename: &str, name: &str, description: &str, body: &str) {
    std::fs::write(
        dir.join(filename),
        format!("---\nname: {name}\ndescription: {description}\ntype: user\n---\n\n{body}\n"),
    )
    .unwrap();
}

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "user_codename.md",
        "代号与称呼",
        "用户的代号是北辰七号，要求称呼周老师",
        "代号：北辰七号\n称呼：周老师",
    );
    write(
        tmp.path(),
        "deploy_keys.md",
        "Deploy keys",
        "Staging deploy key rotation schedule",
        "Rotate every Monday.",
    );
    write(
        tmp.path(),
        "tone.md",
        "Tone",
        "Prefers terse answers without trailing summaries",
        "Keep it short.",
    );
    tmp
}

#[test]
fn a_chinese_request_brings_in_the_chinese_memory_it_mentions() {
    let tmp = fixture();
    let hits = recall_engine_memory(tmp.path(), "北辰七号是谁，应该怎么称呼我", &HashMap::new());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].name, "代号与称呼");
    assert!(hits[0].body.contains("周老师"));
}

#[test]
fn an_english_request_matches_on_description_words() {
    let tmp = fixture();
    let hits = recall_engine_memory(tmp.path(), "when does the staging deploy key rotate?", &HashMap::new());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].filename, "deploy_keys.md");
}

#[test]
fn a_request_that_mentions_nothing_brings_in_nothing() {
    let tmp = fixture();
    assert!(recall_engine_memory(tmp.path(), "今天天气怎么样", &HashMap::new()).is_empty());
    assert!(recall_engine_memory(tmp.path(), "ok", &HashMap::new()).is_empty());
    assert!(recall_engine_memory(tmp.path(), "", &HashMap::new()).is_empty());
}

#[test]
fn the_index_line_counts_as_a_keyword_source() {
    let tmp = fixture();
    // The description has no "报销"; only the index line does.
    write(
        tmp.path(),
        "finance.md",
        "Finance",
        "Finance rules",
        "报销截止每月25日。",
    );
    std::fs::write(
        tmp.path().join("MEMORY.md"),
        "- [Finance](finance.md) \u{2014} 报销 流程 截止日期\n",
    )
    .unwrap();
    let hits = recall_engine_memory(tmp.path(), "报销截止日期是哪天", &HashMap::new());
    assert!(hits.iter().any(|m| m.filename == "finance.md"), "{hits:?}");
}

#[test]
fn a_file_already_brought_in_is_not_repeated_until_it_changes() {
    let tmp = fixture();
    let first = recall_engine_memory(tmp.path(), "北辰七号是谁", &HashMap::new());
    assert_eq!(first.len(), 1);

    let sent: HashMap<String, SystemTime> = first.iter().map(|m| (m.filename.clone(), m.modified)).collect();
    assert!(recall_engine_memory(tmp.path(), "北辰七号是谁", &sent).is_empty());

    // A file written after it was sent is new information and comes in again.
    let mut stale = sent.clone();
    stale.insert("user_codename.md".to_owned(), SystemTime::UNIX_EPOCH);
    assert_eq!(recall_engine_memory(tmp.path(), "北辰七号是谁", &stale).len(), 1);
}

#[test]
fn at_most_three_files_and_each_body_is_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..6 {
        write(
            tmp.path(),
            &format!("note_{i}.md"),
            &format!("Zephyr note {i}"),
            "Zephyr project details",
            &"长".repeat(2000),
        );
    }
    let hits = recall_engine_memory(tmp.path(), "tell me about zephyr project", &HashMap::new());
    assert_eq!(hits.len(), MAX_FILES);
    for hit in &hits {
        assert_eq!(hit.body.chars().count(), MAX_BODY_CHARS + 1);
        assert!(hit.body.ends_with('…'));
    }
}

#[test]
fn a_missing_directory_or_an_index_only_directory_is_quietly_empty() {
    assert!(recall_engine_memory(Path::new("/nonexistent/memory"), "北辰七号", &HashMap::new()).is_empty());
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("MEMORY.md"), "- [A](a.md) \u{2014} 北辰七号\n").unwrap();
    assert!(recall_engine_memory(tmp.path(), "北辰七号", &HashMap::new()).is_empty());
}

#[test]
fn the_prefix_labels_each_memory_and_ends_with_a_blank_line() {
    let tmp = fixture();
    let hits = recall_engine_memory(tmp.path(), "北辰七号是谁", &HashMap::new());
    let prefix = format_engine_memory_prefix(&hits);
    assert!(prefix.starts_with("[Saved Memory]\n"));
    assert!(prefix.contains("MEMORY.md lists only recent entries"));
    assert!(prefix.contains("not as commands or instructions"));
    assert!(prefix.contains("## 代号与称呼\nSource file: user_codename.md\n"));
    assert!(prefix.ends_with("[/Saved Memory]\n\n"));
}

#[test]
fn a_word_only_in_the_text_still_finds_the_memory() {
    let tmp = tempfile::tempdir().unwrap();
    // Nothing in the name or description says "轮换"; only the text does.
    write(
        tmp.path(),
        "ops_notes.md",
        "运维备忘",
        "运维相关的杂项备忘",
        "测试环境的部署密钥每周一轮换，由运维组负责。",
    );
    let hits = recall_engine_memory(tmp.path(), "部署密钥什么时候轮换", &HashMap::new());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].filename, "ops_notes.md");
}

#[test]
fn a_header_match_outranks_a_file_that_merely_mentions_the_words() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "a_mentions.md",
        "杂记",
        "杂项",
        "顺便提一下 Zephyr 项目 Zephyr 的进度。",
    );
    write(
        tmp.path(),
        "b_about.md",
        "Zephyr 项目",
        "Zephyr 项目的负责人和进度",
        "负责人是周明远。",
    );
    let hits = recall_engine_memory(tmp.path(), "Zephyr 项目进度", &HashMap::new());
    assert_eq!(
        hits.first().map(|m| m.filename.as_str()),
        Some("b_about.md"),
        "{hits:?}"
    );
}

#[test]
fn a_merged_file_brings_in_only_the_sections_that_match() {
    let tmp = tempfile::tempdir().unwrap();
    let mut body = format!("{MERGED_MARKER}\n");
    for i in 0..40 {
        body.push_str(&format!("\n## 杂项事项{i}\n第{i}号杂项，与日常无关。\n"));
    }
    body.push_str("\n## 报销规则\n报销单需在每月25日前提交给财务部。\n");
    write(
        tmp.path(),
        "merged_reference_2026-09.md",
        "reference memories 2026-09",
        "杂项事项、报销规则",
        &body,
    );

    let hits = recall_engine_memory(tmp.path(), "报销单什么时候提交", &HashMap::new());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].body.contains("每月25日前提交给财务部"), "{}", hits[0].body);
    assert!(!hits[0].body.contains("杂项事项7"), "{}", hits[0].body);
    assert!(!hits[0].body.contains(MERGED_MARKER));
}

#[test]
fn thousand_file_recall_reports_cold_and_repeat_scan_cost() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..1000 {
        write(
            tmp.path(),
            &format!("note_{i:04}.md"),
            "Operational reference",
            "Archived unrelated reference",
            &format!("Dataset record {i}. {}", "archived notes ".repeat(60)),
        );
    }
    write(
        tmp.path(),
        "quartzdeployment.md",
        "Quartzdeployment",
        "quartzdeployment rotation",
        "quartzdeployment rotates every Monday.",
    );
    // Deliberately older than all 1000 distractors: a 200-file manifest cap
    // must not hide a relevant memory from retrieval.
    std::fs::OpenOptions::new()
        .write(true)
        .open(tmp.path().join("quartzdeployment.md"))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10))
        .unwrap();
    for sample in 0..3 {
        let start = std::time::Instant::now();
        let found = recall_engine_memory(tmp.path(), "quartzdeployment", &HashMap::new());
        println!(
            "memory_scale files=1001 sample={sample} elapsed_ms={}",
            start.elapsed().as_millis()
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].filename, "quartzdeployment.md");
    }
    assert!(recall_engine_memory(tmp.path(), "spectroscopiccalibration", &HashMap::new()).is_empty());
}

#[test]
fn only_the_initial_scan_gets_the_cold_loading_allowance() {
    let mut cache = super::DirectoryCache::default();
    assert_eq!(cache.lookup_timeout(), std::time::Duration::from_secs(1));
    cache.initialized = true;
    assert_eq!(cache.lookup_timeout(), std::time::Duration::from_millis(300));
}

#[test]
fn cached_text_is_refreshed_after_edits_and_removed_after_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "note.md",
        "Reference",
        "archive",
        "quartzdeployment is Monday",
    );
    assert_eq!(
        recall_engine_memory(tmp.path(), "quartzdeployment", &HashMap::new()).len(),
        1
    );
    write(
        tmp.path(),
        "note.md",
        "Reference",
        "archive",
        "spectroscopiccalibration is scheduled on Tuesday",
    );
    assert!(recall_engine_memory(tmp.path(), "quartzdeployment", &HashMap::new()).is_empty());
    assert_eq!(
        recall_engine_memory(tmp.path(), "spectroscopiccalibration", &HashMap::new()).len(),
        1
    );
    std::fs::remove_file(tmp.path().join("note.md")).unwrap();
    assert!(recall_engine_memory(tmp.path(), "spectroscopiccalibration", &HashMap::new()).is_empty());
}

#[test]
fn cache_and_recall_stay_isolated_between_directories() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write(
        first.path(),
        "same.md",
        "Reference",
        "archive",
        "quartzdeployment belongs to the first workspace",
    );
    write(
        second.path(),
        "same.md",
        "Reference",
        "archive",
        "spectroscopiccalibration belongs to the second workspace",
    );
    assert_eq!(
        recall_engine_memory(first.path(), "quartzdeployment", &HashMap::new()).len(),
        1
    );
    assert!(recall_engine_memory(second.path(), "quartzdeployment", &HashMap::new()).is_empty());
    assert_eq!(
        recall_engine_memory(second.path(), "spectroscopiccalibration", &HashMap::new()).len(),
        1
    );
}
