mod dream_engine;
mod dream_ui;
mod claude;
mod cli_helpers;
mod codebuddy;
mod codex;
mod gemini;
mod opencode;
mod qwen;

pub use dream_engine::DreamEngineAdapter;
pub use dream_ui::DreamUiAdapter;
pub use claude::ClaudeAdapter;
pub use codebuddy::CodeBuddyAdapter;
pub use codex::CodexAdapter;
pub use gemini::GeminiAdapter;
pub use opencode::OpencodeAdapter;
pub use qwen::QwenAdapter;
