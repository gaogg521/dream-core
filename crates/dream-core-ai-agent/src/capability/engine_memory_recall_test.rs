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
    assert!(prefix.starts_with("[Saved Memory]\n## 代号与称呼\n"));
    assert!(prefix.ends_with("[/Saved Memory]\n\n"));
}
