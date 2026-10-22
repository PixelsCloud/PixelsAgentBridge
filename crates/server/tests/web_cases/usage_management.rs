use super::*;
use pab_protocol::UsageCounters;
use uuid::Uuid;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 86_400_000;

#[sqlx::test(migrations = "./migrations")]
async fn p2p_usage_is_idempotent_separate_exact_and_available_in_ranking_and_csv(pool: PgPool) {
    let f = fixture(&pool).await;
    let store = PostgresStore::from_pool(pool.clone());
    let old = pab_protocol::UsageBatch {
        id: Uuid::new_v4(),
        user_id: Some(pab_protocol::UserId::from_uuid(f.id)),
        hour_unix_ms: f.hour,
        counters: UsageCounters {
            connections: 1,
            ..Default::default()
        },
    };
    store.record_usage("client", "fixture", &old).await.unwrap();
    let path = format!("/api/web/usage?{}", suffix(&f));
    let (_, _, before) = call(&f.app, "GET", &path, &f.user, Value::Null).await;
    assert_eq!(before["p2p_reported"], false);
    let batch = pab_protocol::UsageBatch {
        id: Uuid::new_v4(),
        counters: UsageCounters {
            p2p_upload_bytes: 9007199254740993,
            p2p_download_bytes: 2048,
            p2p_reported: true,
            ..Default::default()
        },
        ..old
    };
    store
        .record_usage("client", "fixture", &batch)
        .await
        .unwrap();
    store
        .record_usage("client", "fixture", &batch)
        .await
        .unwrap();
    let (status, _, result) = call(&f.app, "GET", &path, &f.user, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result["counters_exact"]["p2p_upload_bytes"],
        "9007199254740993"
    );
    assert_eq!(result["counters_exact"]["relay_upload_bytes"], "0");
    assert_eq!(result["p2p_reported"], true);
    assert_eq!(result["points"][0]["p2p_reported"], true);
    let rank = call(
        &f.app,
        "GET",
        &format!(
            "/api/web/usage/ranking?scope=all&{}&sort=p2p_download_bytes",
            suffix(&f)
        ),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(rank.0, StatusCode::OK, "{}", rank.2);
    assert_eq!(
        rank.2["items"][0]["counters_exact"]["p2p_download_bytes"],
        "2048"
    );
    assert_eq!(rank.2["items"][0]["p2p_reported"], true);
    let (status, _, text) = csv(&f, &f.user, &suffix(&f)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("p2p_upload_bytes"));
    assert_eq!(text.matches("9007199254740993").count(), 1);
}
struct Fixture {
    app: Router,
    admin: String,
    user: String,
    id: Uuid,
    hour: i64,
}
async fn fixture(pool: &PgPool) -> Fixture {
    let (app, control) = app(pool.clone(), true).await;
    let (_, admin, _) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"usage-admin","password":"isolated usage test password"}),
    )
    .await;
    let (_, user, account) = call(
        &app,
        "POST",
        "/api/web/register",
        "",
        json!({"username":"usage-user","password":"isolated usage test password"}),
    )
    .await;
    pab_server::web::bootstrap_admin(&control, "usage-admin")
        .await
        .unwrap();
    Fixture {
        app,
        admin,
        user,
        id: account["id"].as_str().unwrap().parse().unwrap(),
        hour: time::OffsetDateTime::now_utc().unix_timestamp() / 3600 * HOUR,
    }
}
async fn seed(
    pool: &PgPool,
    table: &str,
    subject: &str,
    source: &str,
    bucket: i64,
    bytes: u64,
    incomplete: bool,
) {
    let counters = UsageCounters {
        uploaded_bytes: bytes,
        connections: 1,
        incomplete,
        ..Default::default()
    };
    sqlx::query(&format!(
        "INSERT INTO {table}(source_kind,subject,hour_unix_ms,counters) VALUES($1,$2,$3,$4)"
    ))
    .bind(source)
    .bind(subject)
    .bind(bucket)
    .bind(json!(counters))
    .execute(pool)
    .await
    .unwrap();
}
fn suffix(f: &Fixture) -> String {
    format!("from={}&until={}", f.hour, f.hour + HOUR)
}
async fn csv(
    f: &Fixture,
    cookie: &str,
    query: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let request = Request::builder()
        .uri(format!("/api/web/usage/export?{query}"))
        .header(header::HOST, "web.example")
        .header(header::ORIGIN, "https://web.example")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    (status, headers, text)
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_scope_is_shared_by_summary_ranking_and_export(pool: PgPool) {
    let f = fixture(&pool).await;
    seed(
        &pool,
        "usage_hourly",
        &f.id.to_string(),
        "client",
        f.hour,
        123,
        false,
    )
    .await;
    for endpoint in ["usage", "usage/ranking", "usage/export"] {
        for restriction in [
            "scope=all".to_owned(),
            format!("user={}", Uuid::new_v4()),
            "population=users".to_owned(),
        ] {
            assert_eq!(
                call(
                    &f.app,
                    "GET",
                    &format!("/api/web/{endpoint}?{}&{restriction}", suffix(&f)),
                    &f.user,
                    Value::Null
                )
                .await
                .0,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            call(
                &f.app,
                "GET",
                &format!("/api/web/{endpoint}?{}", suffix(&f)),
                "",
                Value::Null
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/api/web/usage/ranking?{}", suffix(&f)),
            &f.user,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, headers, text) = csv(&f, &f.user, &suffix(&f)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-usage-rows"], "1");
    assert!(text.contains("usage-user"));
    assert!(!text.contains("usage-admin"));
    assert!(!text.contains("password_hash"));
    assert!(!text.contains("token_hash"));
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM web_admin_events WHERE action='usage.exported' AND result='succeeded'").fetch_one(&pool).await.unwrap();
    assert_eq!(events, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_exact_totals_exceed_js_and_u64_without_rounding_or_saturation(pool: PgPool) {
    let f = fixture(&pool).await;
    seed(
        &pool,
        "usage_hourly",
        &f.id.to_string(),
        "client",
        f.hour,
        u64::MAX,
        false,
    )
    .await;
    seed(
        &pool,
        "usage_hourly",
        &f.id.to_string(),
        "relay",
        f.hour,
        u64::MAX,
        false,
    )
    .await;
    let path = format!("/api/web/usage?{}", suffix(&f));
    let (status, _, result) = call(&f.app, "GET", &path, &f.user, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result["counters_exact"]["uploaded_bytes"],
        "36893488147419103230"
    );
    assert_eq!(
        result["points"][0]["counters_exact"]["uploaded_bytes"],
        "36893488147419103230"
    );
    assert_eq!(result["counters"]["uploaded_bytes"], u64::MAX);
    assert_eq!(result["compatibility_saturated"], true);
    assert_eq!(result["incomplete"], false);
    let (_, headers, text) = csv(&f, &f.user, &suffix(&f)).await;
    assert_eq!(headers["x-usage-truncated"], "false");
    assert_eq!(text.matches("18446744073709551615").count(), 2);
    let rank = call(
        &f.app,
        "GET",
        &format!(
            "/api/web/usage/ranking?scope=all&{}&sort=uploaded_bytes",
            suffix(&f)
        ),
        &f.admin,
        Value::Null,
    )
    .await;
    assert_eq!(rank.0, StatusCode::OK, "{}", rank.2);
    assert_eq!(
        rank.2["items"][0]["counters_exact"]["uploaded_bytes"],
        "36893488147419103230"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_retention_auto_daily_and_lifetime_cannot_report_pruned_history_as_zero(
    pool: PgPool,
) {
    let f = fixture(&pool).await;
    let old = f.hour / DAY * DAY - 100 * DAY;
    seed(
        &pool,
        "usage_daily",
        &f.id.to_string(),
        "client",
        old,
        9007199254740993,
        false,
    )
    .await;
    seed(
        &pool,
        "usage_lifetime",
        &f.id.to_string(),
        "client",
        0,
        9007199254740993,
        false,
    )
    .await;
    for endpoint in ["usage", "usage/ranking", "usage/export"] {
        let result = call(
            &f.app,
            "GET",
            &format!(
                "/api/web/{endpoint}?from={old}&until={}&scope=all",
                old + DAY
            ),
            &f.admin,
            Value::Null,
        )
        .await;
        assert_eq!(result.0, StatusCode::GONE, "{}", result.2);
        assert_eq!(result.2["code"], "usage_range_unavailable");
    }
    for interval in ["auto", "day"] {
        let result = call(
            &f.app,
            "GET",
            &format!(
                "/api/web/usage?from={old}&until={}&interval={interval}",
                old + DAY
            ),
            &f.user,
            Value::Null,
        )
        .await;
        assert_eq!(result.0, StatusCode::OK, "{}", result.2);
        assert_eq!(result.2["interval"], "day");
        assert_eq!(
            result.2["counters_exact"]["uploaded_bytes"],
            "9007199254740993"
        );
    }
    let lifetime = call(
        &f.app,
        "GET",
        "/api/web/usage?interval=lifetime",
        &f.user,
        Value::Null,
    )
    .await
    .2;
    assert!(lifetime["from"].is_null());
    assert!(lifetime["until"].is_null());
    assert_eq!(
        lifetime["counters_exact"]["uploaded_bytes"],
        "9007199254740993"
    );
    for (from, until, interval, status) in [
        (
            f.hour / DAY * DAY - 731 * DAY,
            f.hour / DAY * DAY,
            "day",
            StatusCode::GONE,
        ),
        (f.hour, f.hour + 1, "hour", StatusCode::BAD_REQUEST),
        (f.hour, f.hour + 2 * DAY, "auto", StatusCode::BAD_REQUEST),
        (i64::MIN, i64::MAX, "auto", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            call(
                &f.app,
                "GET",
                &format!("/api/web/usage?from={from}&until={until}&interval={interval}"),
                &f.user,
                Value::Null
            )
            .await
            .0,
            status
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_ranking_has_exact_stable_pages_and_historical_population_filters(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("INSERT INTO usage_hourly(source_kind,subject,hour_unix_ms,counters) SELECT 'client','historical-'||lpad(i::text,4,'0'),$1,$2 FROM generate_series(1,205) i")
        .bind(f.hour).bind(json!(UsageCounters { uploaded_bytes:9007199254740993,..Default::default() })).execute(&pool).await.unwrap();
    seed(&pool, "usage_hourly", "guest", "relay", f.hour, 42, true).await;
    let mut subjects = std::collections::HashSet::new();
    for page in 1..=11 {
        let (status,_,value) = call(&f.app,"GET",&format!("/api/web/usage/ranking?scope=all&population=users&sort=uploaded_bytes&page={page}&page_size=20&{}",suffix(&f)),&f.admin,Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["total"], 205);
        for item in value["items"].as_array().unwrap() {
            assert!(subjects.insert(item["subject"].as_str().unwrap().to_owned()));
            assert!(item["incomplete"].as_bool().unwrap());
        }
    }
    assert_eq!(subjects.len(), 205);
    let empty = call(
        &f.app,
        "GET",
        &format!(
            "/api/web/usage/ranking?scope=all&population=users&page=100&{}",
            suffix(&f)
        ),
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(empty["total"], 205);
    assert_eq!(empty["items"], json!([]));
    let guests = call(
        &f.app,
        "GET",
        &format!(
            "/api/web/usage/ranking?scope=all&population=guests&{}",
            suffix(&f)
        ),
        &f.admin,
        Value::Null,
    )
    .await
    .2;
    assert_eq!(guests["total"], 1);
    assert_eq!(guests["items"][0]["subject"], "guest");
    for invalid in ["sort=secret", "order=drop", "page=0", "page_size=101"] {
        assert_eq!(
            call(
                &f.app,
                "GET",
                &format!("/api/web/usage/ranking?scope=all&{invalid}&{}", suffix(&f)),
                &f.admin,
                Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_csv_is_bounded_formula_safe_and_records_truncation(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("UPDATE users SET username=$2 WHERE id=$1")
        .bind(f.id)
        .bind(" =SUM(1,2)中文\"name")
        .execute(&pool)
        .await
        .unwrap();
    seed(
        &pool,
        "usage_hourly",
        &f.id.to_string(),
        "client",
        f.hour,
        9007199254740993,
        false,
    )
    .await;
    let (status, headers, text) = csv(&f, &f.admin, &format!("scope=all&{}", suffix(&f))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    assert!(text.starts_with('\u{feff}'));
    assert!(text.contains("\"' =SUM(1,2)中文\"\"name\""));
    assert!(text.contains(&f.hour.to_string()));
    assert!(text.contains("\"export_status\""));
    sqlx::query("INSERT INTO usage_hourly(source_kind,subject,hour_unix_ms,counters) SELECT 'client','row-'||lpad(i::text,5,'0'),$1,$2 FROM generate_series(1,10000) i")
        .bind(f.hour).bind(json!(UsageCounters::default())).execute(&pool).await.unwrap();
    let (_, headers, text) = csv(&f, &f.admin, &format!("scope=all&{}", suffix(&f))).await;
    assert_eq!(headers["x-usage-truncated"], "true");
    assert_eq!(headers["x-usage-rows"], "10000");
    assert_eq!(text.lines().count(), 10002);
    assert!(text.lines().last().unwrap().ends_with("\"10000\",\"true\""));
    let details: Value = sqlx::query_scalar("SELECT details FROM web_admin_events WHERE action='usage.exported' ORDER BY id DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(details["truncated"], true);
    assert_eq!(details["rows"], 10000);
    assert!(!details.to_string().contains("password"));
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_late_reports_and_unattributed_gaps_preserve_separate_sources(pool: PgPool) {
    let f = fixture(&pool).await;
    seed(
        &pool,
        "usage_hourly",
        &f.id.to_string(),
        "client",
        f.hour - HOUR,
        17,
        false,
    )
    .await;
    seed(
        &pool,
        "usage_hourly",
        &f.id.to_string(),
        "relay",
        f.hour - HOUR,
        0,
        false,
    )
    .await;
    sqlx::query("UPDATE usage_hourly SET counters=jsonb_set(counters,'{relay_download_bytes}','29') WHERE source_kind='relay'").execute(&pool).await.unwrap();
    seed(
        &pool,
        "usage_hourly",
        "guest",
        "relay",
        f.hour - HOUR,
        0,
        true,
    )
    .await;
    let path = format!("/api/web/usage?from={}&until={}", f.hour - HOUR, f.hour);
    let first = call(&f.app, "GET", &path, &f.user, Value::Null).await.2;
    assert_eq!(first["counters_exact"]["uploaded_bytes"], "17");
    assert_eq!(first["counters_exact"]["relay_download_bytes"], "29");
    assert_eq!(first["incomplete"], true);
    sqlx::query("UPDATE usage_hourly SET counters=jsonb_set(counters,'{uploaded_bytes}','18'),updated_at=clock_timestamp() WHERE source_kind='client'").execute(&pool).await.unwrap();
    let next = call(&f.app, "GET", &path, &f.user, Value::Null).await.2;
    assert_eq!(next["counters_exact"]["uploaded_bytes"], "18");
    assert_eq!(next["incomplete"], true);
    assert!(next["updated_at"].as_i64().unwrap() >= first["updated_at"].as_i64().unwrap());
}

#[sqlx::test(migrations = "./migrations")]
async fn canceled_snapshot_begin_cannot_poison_the_next_export_transaction(pool: PgPool) {
    use sqlx::postgres::{PgPoolOptions, PgSslMode};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Delay only PostgreSQL's ReadyForQuery acknowledgement after BEGIN. This
    // cancels the caller after BEGIN took effect, rather than before acquiring
    // a connection, and deterministically reproduces the live-browser failure.
    let options = pool.connect_options();
    let target = format!("{}:{}", options.get_host(), options.get_port());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let proxy = tokio::spawn(async move {
        let (client, _) = listener.accept().await.unwrap();
        let database = tokio::net::TcpStream::connect(target).await.unwrap();
        let (mut client_read, mut client_write) = client.into_split();
        let (mut db_read, mut db_write) = database.into_split();
        let upstream = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut client_read, &mut db_write).await;
        });
        let mut delay = false;
        while let Ok(tag) = db_read.read_u8().await {
            let length = db_read.read_u32().await.unwrap();
            assert!((4..=1_048_576).contains(&length));
            let mut body = vec![0; (length - 4) as usize];
            db_read.read_exact(&mut body).await.unwrap();
            if tag == b'C' && body == b"BEGIN\0" {
                delay = true;
            }
            if tag == b'Z' && delay {
                tokio::time::sleep(Duration::from_millis(100)).await;
                delay = false;
            }
            if client_write.write_u8(tag).await.is_err() {
                break;
            }
            client_write.write_u32(length).await.unwrap();
            client_write.write_all(&body).await.unwrap();
            client_write.flush().await.unwrap();
        }
        upstream.abort();
    });
    let proxy_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            options
                .as_ref()
                .clone()
                .host("127.0.0.1")
                .port(port)
                .ssl_mode(PgSslMode::Disable),
        )
        .await
        .unwrap();
    let store = PostgresStore::from_pool(proxy_pool.clone());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), store.begin_snapshot(true))
            .await
            .is_err()
    );
    let mut tx = tokio::time::timeout(Duration::from_secs(5), store.begin_snapshot(false))
        .await
        .unwrap()
        .unwrap();
    let isolation: String = sqlx::query_scalar("SHOW transaction_isolation")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let readonly: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(isolation, "repeatable read");
    assert_eq!(readonly, "off");
    sqlx::query("INSERT INTO web_admin_events(action) VALUES('snapshot_cancel_probe')")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    proxy_pool.close().await;
    tokio::time::timeout(Duration::from_secs(5), proxy)
        .await
        .unwrap()
        .unwrap();
}
