use super::*;

#[sqlx::test(migrations = "./migrations")]
async fn overview_account_totals_are_aggregated_and_administrator_only(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let (_, admin, _) = call(&app, "POST", "/api/web/register", "", json!({"username":"overview-admin","password":"long isolated password"})).await;
    let (_, reader, _) = call(&app, "POST", "/api/web/register", "", json!({"username":"overview-reader","password":"long isolated password"})).await;
    call(&app, "POST", "/api/web/register", "", json!({"username":"overview-disabled","password":"long isolated password"})).await;
    sqlx::query("UPDATE users SET status='disabled' WHERE username='overview-disabled'").execute(&pool).await.unwrap();
    pab_server::web::bootstrap_admin(&control, "overview-admin").await.unwrap();
    let response = call(&app, "GET", "/api/web/overview?scope=all", &admin, Value::Null).await;
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(response.2["accounts"], 3);
    assert_eq!(response.2["account_summary"], json!({"enabled":2,"disabled":1,"administrators":1}));
    let mine = call(&app, "GET", "/api/web/overview?scope=mine", &reader, Value::Null).await;
    assert!(mine.2.get("accounts").is_none());
    assert!(mine.2.get("account_summary").is_none());
    assert_eq!(call(&app, "GET", "/api/web/overview?scope=all", &reader, Value::Null).await.0, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn overview_device_queries_preserve_scope_and_support_owner_search(pool: PgPool) {
    let (app, control) = app(pool.clone(), true).await;
    let device = unclaimed_fixture(&pool).await;
    let (_, admin, _) = call(&app, "POST", "/api/web/register", "", json!({"username":"dashboard-admin","password":"long isolated password"})).await;
    let (_, owner, owner_data) = call(&app, "POST", "/api/web/register", "", json!({"username":"dashboard-owner","password":"long isolated password"})).await;
    let (_, stranger, _) = call(&app, "POST", "/api/web/register", "", json!({"username":"dashboard-stranger","password":"long isolated password"})).await;
    let device_id = match device.principal { pab_protocol::EndpointProofPrincipal::Device { device_id } => device_id.as_uuid(), _ => unreachable!() };
    sqlx::query("INSERT INTO device_accounts(device_id,user_id,revision) VALUES($1,$2,1)")
        .bind(device_id).bind(uuid::Uuid::parse_str(owner_data["id"].as_str().unwrap()).unwrap()).execute(&pool).await.unwrap();
    pab_server::web::bootstrap_admin(&control, "dashboard-admin").await.unwrap();
    let query = "/api/web/devices?sort=reported&attention=true&owner_search=true&q=dashboard-owner&page_size=4";
    for (cookie, scope, expected) in [(&admin, "&scope=all", 1), (&owner, "", 1), (&stranger, "", 0)] {
        let response = call(&app, "GET", &format!("{query}{scope}"), cookie, Value::Null).await;
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(response.2["total"], expected);
    }
    let legacy = call(&app, "GET", "/api/web/devices?q=dashboard-owner", &admin, Value::Null).await;
    assert_eq!(legacy.2["total"], 0);
    assert_eq!(call(&app, "GET", "/api/web/devices?sort=invalid", &admin, Value::Null).await.0, StatusCode::BAD_REQUEST);
}
