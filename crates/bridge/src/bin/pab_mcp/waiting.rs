use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{future::Future, time::Duration};

pub(super) fn wait_ms(args: &Value) -> u64 {
    args["wait_ms"].as_u64().unwrap_or(0).min(30000)
}

pub(super) fn completed(v: &Value) -> bool {
    if v["complete"].as_bool() == Some(true) {
        return true;
    }
    let state = v
        .pointer("/result/state")
        .or_else(|| v.pointer("/operation/state"))
        .and_then(Value::as_str);
    matches!(
        state,
        Some("completed" | "failed" | "cancelled" | "interrupted")
    )
}

pub(super) async fn wait_value<F, Fut>(args: &Value, mut read: F) -> Result<Value, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms(args));
    let mut value = read().await?;
    let revision = |v: &Value| format!("{:x}", Sha256::digest(serde_json::to_vec(v).unwrap()));
    let baseline = args["after_revision"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| revision(&value));
    loop {
        let current = revision(&value);
        let changed = current != baseline;
        let until_complete = args["wait_until"].as_str() == Some("complete");
        if completed(&value)
            || (!until_complete && changed)
            || tokio::time::Instant::now() >= deadline
        {
            value["revision"] = json!(current);
            value["changed"] = json!(changed);
            value["wait_expired"] =
                json!(!completed(&value) && (!changed || until_complete) && wait_ms(args) > 0);
            return Ok(value);
        }
        tokio::time::sleep_until(
            deadline.min(tokio::time::Instant::now() + Duration::from_millis(100)),
        )
        .await;
        match tokio::time::timeout_at(deadline, read()).await {
            Ok(result) => value = result?,
            Err(_) => { /* Return the last observed facts, never a fabricated failure. */ }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn change_completion_and_timeout_preserve_original_facts() {
        let first = wait_value(&json!({}), || async {
            Ok(json!({"complete":false,"progress":1}))
        })
        .await
        .unwrap();
        let changed = wait_value(
            &json!({"wait_ms":1000,"after_revision":first["revision"]}),
            || async { Ok(json!({"complete":false,"progress":2})) },
        )
        .await
        .unwrap();
        assert_eq!(changed["changed"], true);
        assert_eq!(changed["wait_expired"], false);
        let timeout = wait_value(&json!({"wait_ms":5,"wait_until":"complete"}), || async {
            Ok(json!({"complete":false}))
        })
        .await
        .unwrap();
        assert_eq!(timeout["wait_expired"], true);
        assert_eq!(timeout["complete"], false);
        let done = wait_value(&json!({"wait_ms":1000,"wait_until":"complete"}), || async {
            Ok(json!({"complete":true}))
        })
        .await
        .unwrap();
        assert_eq!(done["wait_expired"], false);
    }
}
