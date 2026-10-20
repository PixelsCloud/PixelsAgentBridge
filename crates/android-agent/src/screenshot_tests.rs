use super::*;
use serde_json::{Value, json};

async fn sdk_job(system: &SystemService) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let work = system.sdk.poll().await;
            if !work.is_null() {
                return work;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn window_scoped_screenshot_uses_existing_mailboxes_and_shared_wire_verification() {
    let temp = tempfile::tempdir().unwrap();
    let system = SystemService::open(
        &temp.path().join("system.sqlite"),
        [9; 32],
        DeviceRef {
            tenant_id: TenantId::new(),
            device_id: DeviceId::new(),
        },
    )
    .await
    .unwrap();
    let broker = PlatformBroker::open(&temp.path().join("mobile.sqlite"), [8; 32])
        .await
        .unwrap();
    let options = ScreenshotOptions {
        window_ref: Some(RequestId::new().to_string()),
        ..Default::default()
    };
    let expected = options.clone();
    let id = RequestId::new();
    let service = system.clone();
    let platform = broker.clone();
    let task = tokio::spawn(async move {
        capture(
            &platform,
            &service,
            "owner",
            "session",
            Arc::new(Mutex::new(ReferenceScope::default())),
            id,
            options,
        )
        .await
    });
    let work = sdk_job(&system).await;
    assert_eq!(work["command"]["connection_scope"], "session");
    assert_eq!(
        work["command"]["query"]["query"]["window_ref"],
        json!(expected.window_ref)
    );
    system
        .sdk
        .complete(
            serde_json::from_value(work["ticket"].clone()).unwrap(),
            json!({"window_key":"fixture:1","width":64,"height":64}),
        )
        .await;
    let work = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let work = broker.poll().await.unwrap();
            if !work.is_null() {
                return work;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(work["command"]["action"], "screenshot");
    let frame = pab_screenshot::encode(
        pab_screenshot::image::DynamicImage::new_rgb8(64, 64),
        &ScreenshotOptions::default(),
        None,
        (0, 0),
    )
    .unwrap();
    broker
        .complete(
            serde_json::from_value(work["ticket"].clone()).unwrap(),
            json!({"width":64,"height":64}),
            frame.bytes,
        )
        .await
        .unwrap();
    let work = sdk_job(&system).await;
    system
        .sdk
        .complete(
            serde_json::from_value(work["ticket"].clone()).unwrap(),
            json!({"window_key":"fixture:1","width":64,"height":64}),
        )
        .await;
    let (meta, bytes) = task.await.unwrap().unwrap();
    assert_eq!(meta.request_id, id);
    assert_eq!(
        meta.capture.as_ref().unwrap().window_ref,
        expected.window_ref
    );
    pab_screenshot::verify(&bytes, meta.capture.as_ref().unwrap(), &expected).unwrap();
    assert_eq!(meta.sha256, format!("{:x}", Sha256::digest(bytes)));
}
