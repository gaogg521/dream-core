//! Tell the agent about the folders attached to a conversation's project.
//!
//! An agent is started in the project's workspace folder and knows nothing
//! else about the project. Folders the user adds from the Explorer ("Add
//! folder to project") are listed there next to the workspace, so to the user
//! they are as much a part of the conversation — yet the agent was never told
//! they exist, and answered "what is in this project" from the workspace alone.
//!
//! The note travels with each new turn rather than once per session: folders
//! can be added or removed between turns, a resumed or recreated session has no
//! memory of a note sent to an earlier process, and the cost is a few lines. It
//! is prepended to what the agent receives, never to the persisted message, so
//! the transcript still shows what the user typed.

use dream_core_project::AttachedFolder;

/// Prefix `content` with the note for `folders`, or return it unchanged.
///
/// A slash command is left alone: agents only recognise one at the very start
/// of the prompt, so a prefix would turn `/compact` into ordinary text.
pub(crate) fn with_attached_folders_note(content: String, folders: &[AttachedFolder]) -> String {
    if folders.is_empty() || content.trim_start().starts_with('/') {
        return content;
    }
    format!("{}{content}", attached_folders_note(folders))
}

fn attached_folders_note(folders: &[AttachedFolder]) -> String {
    let mut note = String::from(
        "[Project folders]\n\
         Besides the working directory, the user's project includes these folders. \
         They are part of the same project: when the user asks about the project, its \
         directories or its files, include them, using the absolute paths below.\n",
    );
    for folder in folders {
        note.push_str(&format!("- {}: {}\n", folder.name, folder.path.display()));
    }
    note.push_str("[/Project folders]\n\n");
    note
}

#[cfg(test)]
#[path = "project_folders_test.rs"]
mod project_folders_test;
