use pab_protocol::RelayLimitDefaults;
use pab_server::PostgresStore;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn fresh_settings_are_singleton_and_reinitialization_preserves_values(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    store
        .initialize_settings(RelayLimitDefaults {
            user_mbps: 24,
            guest_mbps: 1,
        })
        .await
        .unwrap();
    store
        .initialize_settings(RelayLimitDefaults {
            user_mbps: 5,
            guest_mbps: 1,
        })
        .await
        .unwrap();
    let settings: Vec<(bool, i32, i32, i64)> = sqlx::query_as(
        "SELECT singleton, default_user_mbps, default_guest_mbps, policy_revision FROM server_settings",
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(settings, vec![(true, 24, 1, 1)]);
    assert!(
        sqlx::query(
            "INSERT INTO server_settings (default_user_mbps, default_guest_mbps) VALUES (1, 1)"
        )
        .execute(&pool)
        .await
        .is_err()
    );
}
