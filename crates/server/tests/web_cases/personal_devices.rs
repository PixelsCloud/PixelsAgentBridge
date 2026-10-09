// Included from web_management.rs to reuse its real HTTP and isolated database fixtures.
use super::*;
use iroh_base::SecretKey;
use pab_protocol::{DeviceAccountChallenge, EndpointSignature};

pub(super) async fn account(app: &Router, name: &str) -> (String, Value, String) {
    let password = "personal-device-test";
    let (status, native) = native_call(
        app,
        "POST",
        "/api/account/register",
        "",
        json!({"username":name,"password":password}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{native}");
    let (_, cookie, _) = call(
        app,
        "POST",
        "/api/web/session",
        "",
        json!({"username":name,"password":password}),
    )
    .await;
    (
        native["access_token"].as_str().unwrap().into(),
        native["user"].clone(),
        cookie,
    )
}

async fn signed_challenge(
    app: &Router,
    token: &str,
    path: &str,
    secret: &SecretKey,
    action: &str,
    revision: Option<i64>,
) -> Value {
    let (status, body) = native_call(
        app,
        "POST",
        &format!("{path}-challenge"),
        token,
        json!({"action":action,"expected_revision":revision}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let challenge: DeviceAccountChallenge =
        serde_json::from_value(body["challenge"].clone()).unwrap();
    assert_eq!(challenge.server_origin, "https://web.example");
    json!({"challenge_id":challenge.id,"signature":EndpointSignature::from_bytes(secret.sign(&challenge.signing_message()).to_bytes())})
}

async fn fixture(pool: &PgPool) -> (Uuid, SecretKey) {
    let device = unclaimed_fixture(pool).await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM devices WHERE code=345678901")
        .fetch_one(pool)
        .await
        .unwrap();
    let secret = SecretKey::generate();
    sqlx::query("UPDATE endpoints SET endpoint_key=$1 WHERE endpoint_key=$2")
        .bind(secret.public().as_bytes().as_slice())
        .bind(device.endpoint_key.as_bytes().as_slice())
        .execute(pool)
        .await
        .unwrap();
    (id, secret)
}

use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn associations_are_proved_isolated_idempotent_and_never_change_device_identity(
    pool: PgPool,
) {
    let (app, _) = app(pool.clone(), true).await;
    let (id, secret) = fixture(&pool).await;
    let (a, _, cookie_a) = account(&app, "personal-a").await;
    let (b, _, cookie_b) = account(&app, "personal-b").await;
    let path = format!("/api/account/devices/{id}/association");
    let proof = signed_challenge(&app, &a, &path, &secret, "automatic", None).await;
    let mut invalid = proof.clone();
    invalid["signature"] = json!(EndpointSignature::from_bytes(
        SecretKey::generate().sign(b"wrong").to_bytes()
    ));
    assert_eq!(
        native_call(&app, "PUT", &path, &a, invalid).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        native_call(&app, "PUT", &path, &b, proof.clone()).await.0,
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let (s, v) = native_call(&app, "PUT", &path, &a, proof.clone()).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["revision"], 1);
    }
    assert_eq!(
        call(&app, "GET", "/api/web/devices", &cookie_a, Value::Null)
            .await
            .2["total"],
        1
    );
    assert_eq!(
        call(&app, "GET", "/api/web/devices", &cookie_b, Value::Null)
            .await
            .2["total"],
        0
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/web/devices/{id}"),
            &cookie_b,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/web/devices/{id}/association"),
            &cookie_b,
            json!({"revision":1})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let other = native_call(
        &app,
        "POST",
        &format!("{path}-challenge"),
        &b,
        json!({"action":"automatic"}),
    )
    .await;
    assert_eq!(other.1["status"], "other_account");
    assert!(other.1.get("challenge").is_none());
    let replaced = signed_challenge(&app, &b, &path, &secret, "replace", Some(1)).await;
    assert_eq!(
        native_call(&app, "PUT", &path, &b, replaced).await.1["revision"],
        2
    );
    assert_eq!(
        native_call(&app, "PUT", &path, &a, proof).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "GET", "/api/web/devices", &cookie_a, Value::Null)
            .await
            .2["total"],
        0
    );
    let identity: (i32, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT code,owner_tenant_id,registered_by_user_id FROM devices WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identity, (345678901, None, None));
}

#[sqlx::test(migrations = "./migrations")]
async fn unlink_blocks_automatic_rebinding_and_stale_proofs(pool: PgPool) {
    let (app, _) = app(pool.clone(), true).await;
    let (id, secret) = fixture(&pool).await;
    let (a, _, cookie) = account(&app, "unlink-a").await;
    let path = format!("/api/account/devices/{id}/association");
    let proof = signed_challenge(&app, &a, &path, &secret, "automatic", None).await;
    assert_eq!(
        native_call(&app, "PUT", &path, &a, proof.clone()).await.0,
        StatusCode::OK
    );
    let unlink = call(
        &app,
        "DELETE",
        &format!("/api/web/devices/{id}/association"),
        &cookie,
        json!({"revision":1}),
    )
    .await;
    assert_eq!(unlink.0, StatusCode::OK, "{}", unlink.2);
    assert_eq!(
        native_call(&app, "PUT", &path, &a, proof).await.0,
        StatusCode::CONFLICT
    );
    let automatic = native_call(
        &app,
        "POST",
        &format!("{path}-challenge"),
        &a,
        json!({"action":"automatic"}),
    )
    .await;
    assert_eq!(automatic.1["status"], "unlinked");
    assert!(automatic.1.get("challenge").is_none());
    assert_eq!(
        native_call(
            &app,
            "POST",
            &format!("{path}-challenge"),
            &a,
            json!({"action":"associate","expected_revision":1})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let proof = signed_challenge(&app, &a, &path, &secret, "associate", Some(2)).await;
    assert_eq!(
        native_call(&app, "PUT", &path, &a, proof).await.1["revision"],
        3
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn competing_associations_and_expired_challenges_cannot_overwrite(pool: PgPool) {
    let (app, _) = app(pool.clone(), true).await;
    let (id, secret) = fixture(&pool).await;
    let (a, _, _) = account(&app, "race-a").await;
    let (b, _, _) = account(&app, "race-b").await;
    let path = format!("/api/account/devices/{id}/association");
    let proof_a = signed_challenge(&app, &a, &path, &secret, "automatic", None).await;
    let proof_b = signed_challenge(&app, &b, &path, &secret, "automatic", None).await;
    let (result_a, result_b) = tokio::join!(
        native_call(&app, "PUT", &path, &a, proof_a),
        native_call(&app, "PUT", &path, &b, proof_b)
    );
    assert!(
        matches!(
            (result_a.0, result_b.0),
            (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
        ),
        "{result_a:?} {result_b:?}"
    );
    let loser = if result_a.0 == StatusCode::OK { &b } else { &a };
    let proof = signed_challenge(&app, loser, &path, &secret, "replace", Some(1)).await;
    sqlx::query("UPDATE device_account_challenges SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        native_call(&app, "PUT", &path, loser, proof).await.0,
        StatusCode::NOT_FOUND
    );
}
