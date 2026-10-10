async fn bind_test_login(pool: &sqlx::PgPool, user: pab_protocol::UserId, key: pab_protocol::EndpointKey) {
    let hash = blake3::hash(key.as_bytes()).to_hex().to_string();
    sqlx::query("INSERT INTO web_sessions(token_hash,user_id) VALUES($1,$2)")
        .bind(&hash).bind(user.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO endpoint_user_contexts(endpoint_key,token_hash,revision) VALUES($1,$2,1)")
        .bind(key.as_bytes().as_slice()).bind(&hash).execute(pool).await.unwrap();
}
