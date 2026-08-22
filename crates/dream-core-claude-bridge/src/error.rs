#[derive(Debug, thiserror::Error)]
pub enum ClaudeBridgeError {
    #[error("database error: {0}")]
    Db(#[from] dream_core_db::DbError),
}
