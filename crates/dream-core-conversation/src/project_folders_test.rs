use std::path::PathBuf;

use dream_core_project::AttachedFolder;

use super::with_attached_folders_note;

fn folder(name: &str, path: &str) -> AttachedFolder {
    AttachedFolder {
        name: name.to_owned(),
        path: PathBuf::from(path),
    }
}

#[test]
fn every_attached_folder_is_named_with_its_absolute_path_before_the_message() {
    let folders = [folder("测试123", "/home/u/测试123"), folder("docs", "/srv/docs")];

    let sent = with_attached_folders_note("看看这个目录有什么?".to_owned(), &folders);

    assert!(sent.starts_with("[Project folders]\n"), "{sent}");
    assert!(sent.contains("- 测试123: /home/u/测试123\n"), "{sent}");
    assert!(sent.contains("- docs: /srv/docs\n"), "{sent}");
    assert!(sent.ends_with("[/Project folders]\n\n看看这个目录有什么?"), "{sent}");
}

#[test]
fn a_project_without_attached_folders_sends_the_message_untouched() {
    assert_eq!(with_attached_folders_note("hello".to_owned(), &[]), "hello");
}

#[test]
fn a_slash_command_is_not_prefixed() {
    let folders = [folder("docs", "/srv/docs")];

    assert_eq!(
        with_attached_folders_note("  /compact".to_owned(), &folders),
        "  /compact"
    );
}
