use pab_server::{ControlPlane, PasswordPolicy, PostgresStore};
use sqlx::PgPool;
use std::time::Duration;

#[sqlx::test(migrations = "./migrations")]
async fn account_limits_default_to_ten_and_only_accept_presets(pool: PgPool) {
    let store = PostgresStore::from_pool(pool.clone());
    store.initialize_settings().await.unwrap();
    store.initialize_settings().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let control = ControlPlane::new(store.clone(), PasswordPolicy::default()).unwrap();
    let user = control
        .register_account("limit-fixture", "long test password")
        .await
        .unwrap();
    let snapshot = store
        .relay_policy_snapshot(Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(snapshot.defaults.user_mbps, 10);
    assert_eq!(snapshot.user_limits[0].mbps, 10);
    for value in pab_protocol::RELAY_MBPS_OPTIONS {
        sqlx::query("UPDATE users SET relay_limit_mbps=$1 WHERE id=$2")
            .bind(value)
            .bind(user.id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
    }
    for value in [None, Some(0), Some(-1), Some(12), Some(101)] {
        assert!(
            sqlx::query("UPDATE users SET relay_limit_mbps=$1 WHERE id=$2")
                .bind(value)
                .bind(user.id.as_uuid())
                .execute(&pool)
                .await
                .is_err()
        );
    }
    store.initialize_settings().await.unwrap();
    let snapshot = store
        .relay_policy_snapshot(Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(snapshot.user_limits[0].mbps, 100);
}
