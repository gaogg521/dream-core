use dream_core_db::{IChannelRepository, SqliteChannelRepository};

#[tokio::test]
async fn receipts_survive_database_reopen_and_claim_concurrently_once() {
    let path = std::env::temp_dir().join(format!("onework-receipts-{}.db", dream_core_common::generate_id()));
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options.clone()).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../dream-core-db/migrations/061_channel_delivery_receipts.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let repo = SqliteChannelRepository::new(pool.clone());
    assert!(repo.claim_delivery("owner", "lark", "message-1", 1000).await.unwrap());
    pool.close().await;
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let repo = SqliteChannelRepository::new(pool.clone());
    assert!(!repo.claim_delivery("owner", "lark", "message-1", 2000).await.unwrap());
    assert!(
        repo.claim_delivery("another-owner", "lark", "message-1", 2000)
            .await
            .unwrap()
    );
    let (a, b) = tokio::join!(
        repo.claim_delivery("owner", "lark", "click-1", 2000),
        repo.claim_delivery("owner", "lark", "click-1", 2000)
    );
    assert_ne!(a.unwrap(), b.unwrap());
    assert!(repo.claim_delivery("owner", "lark", "click-2", 2000).await.unwrap());
    pool.close().await;
    std::fs::remove_file(path).unwrap();
}
