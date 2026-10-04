use pab_protocol::RelayLimitDefaults;
use pab_server::PostgresStore;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn fresh_settings_are_singleton_and_reinitialization_preserves_values(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    store
        .initialize_settings(RelayLimitDefaults {
            team_mbps: 80,
            member_mbps: 16,
            personal_mbps: 24,
        })
        .await
        .unwrap();
    store
        .initialize_settings(RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        })
        .await
        .unwrap();
    let settings: Vec<(bool, i32, i32, i32, i64)> = sqlx::query_as(
        "SELECT singleton, default_team_mbps, default_member_mbps, default_personal_mbps, policy_revision FROM server_settings",
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(settings, vec![(true, 80, 16, 24, 1)]);
    assert!(sqlx::query("INSERT INTO server_settings (default_team_mbps, default_member_mbps, default_personal_mbps) VALUES (1, 1, 1)").execute(&pool).await.is_err());
}
