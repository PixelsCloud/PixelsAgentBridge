use pab_server::{ControlPlane, PasswordPolicy, PostgresStore};
use sqlx::PgPool;
use std::time::Duration;

#[sqlx::test(migrations = "./migrations")]
async fn account_limits_default_to_five_and_only_accept_presets(pool: PgPool) {
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
    assert_eq!(snapshot.defaults.user_mbps, 5);
    assert_eq!(snapshot.user_limits[0].mbps, 5);
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

#[sqlx::test(migrations = "./migrations")]
async fn default_change_preserves_manual_limits_even_at_old_default(pool: PgPool) {
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "INSERT INTO users(id,username,username_key,password_hash,relay_limit_mbps) \
         SELECT gen_random_uuid(),'migration-'||n,'migration-'||n,'fixture', \
         CASE WHEN n=5 THEN 100 ELSE 10 END FROM generate_series(1,6) n RETURNING id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    // Successful overrides (including historical events) must survive even
    // when set to 10. Denied attempts must not preserve an untouched default.
    sqlx::query(
        "INSERT INTO web_admin_events(action,resource_id,result) VALUES \
        ('account.relay_limit_updated',$1,'succeeded'), \
        ('account.relay_limit_updated',$2,'historical_unknown'), \
        ('account.relay_limit_updated',$3,'denied')",
    )
    .bind(ids[1])
    .bind(ids[2])
    .bind(ids[3])
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO web_admin_events(action,resource_kind,resource_key,result) VALUES ('account.relay_limit_updated','account',$1,'succeeded')")
        .bind(ids[5].to_string()).execute(&pool).await.unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(include_str!("../migrations/0019_default_relay_five.sql"))
            .execute(&pool)
            .await
            .unwrap();
    }
    for (id, expected) in ids.iter().zip([5, 10, 10, 5, 100, 10]) {
        let actual: i32 = sqlx::query_scalar("SELECT relay_limit_mbps FROM users WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, expected);
    }
    let new_limit: i32 = sqlx::query_scalar("INSERT INTO users(id,username,username_key,password_hash) VALUES(gen_random_uuid(),'new-default','new-default','fixture') RETURNING relay_limit_mbps")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(new_limit, 5);
}
