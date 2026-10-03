use super::{WebError, WebState, session::viewer, support::same_origin};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;

pub(crate) async fn subscribe(
    State(state): State<WebState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, WebError> {
    if !same_origin(&headers) {
        return Err(WebError::new(StatusCode::FORBIDDEN, "origin_rejected"));
    }
    viewer(&state, &headers).await?;
    let permit = state
        .subscriptions
        .clone()
        .try_acquire_owned()
        .map_err(|_| WebError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited"))?;
    let mut changes = state.control.web_changes();
    Ok(upgrade.max_message_size(1024).max_frame_size(1024).on_upgrade(move|socket|async move{
        let _permit=permit;
        let(mut sender,mut receiver)=socket.split();
        let mut interval=tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seq=0u64;
        loop{
            // Periodic invalidation also catches CLI/external database changes.
            // Messages contain no resource IDs, names, counts or task contents.
            tokio::select!{
                _=interval.tick()=>{},
                changed=changes.changed()=>{if changed.is_err(){break;}},
                message=receiver.next()=>{
                    match message{Some(Ok(Message::Ping(bytes)))=>{if !matches!(tokio::time::timeout(Duration::from_secs(5),sender.send(Message::Pong(bytes))).await,Ok(Ok(()))){break;}},Some(Ok(Message::Pong(_)))=>{},_=>break}
                    continue;
                }
            }
            if viewer(&state,&headers).await.is_err(){let _=tokio::time::timeout(Duration::from_secs(5),sender.send(Message::Close(None))).await;break;}
            seq=seq.wrapping_add(1);
            let message=Message::Text(json!({"type":"refresh","sequence":seq}).to_string().into());
            if !matches!(tokio::time::timeout(Duration::from_secs(5),sender.send(message)).await,Ok(Ok(()))){break;}
        }
    }))
}
