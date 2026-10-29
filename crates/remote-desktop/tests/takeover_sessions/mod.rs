use super::*;

#[tokio::test]
async fn takeover_requires_account_exact_confirmation_and_releases_input(){
    let backend=Arc::new(Backend::default());
    let global=DesktopHost::new(Some(backend.clone()));
    let first=Harness::with_account(&global,pair().await,Some(UserId::new())).await;
    let second=Harness::with_account(&global,pair().await,Some(UserId::new())).await;
    let guest=Harness::new(&global).await;
    let mut old=RemoteDesktopSession::open(first.client.clone(),None,30,WAIT).await.unwrap();
    old.next_frame(WAIT).await.unwrap();old.set_input_enabled(true).await.unwrap();
    old.input_control().send(RemoteDesktopInput::Key{virtual_key:0x11,down:true}).await.unwrap();
    let token=RemoteDesktopSession::occupancy(&second.client,WAIT).await.unwrap().unwrap();
    assert_eq!(token,old.opened().config.session_id);
    // Looking at/cancelling a prompt is read-only; the original viewer still works.
    old.next_frame(WAIT).await.unwrap();assert_eq!(backend.input_held.load(Ordering::SeqCst),1);
    assert!(RemoteDesktopSession::takeover(&guest.client,token,WAIT).await.is_err());
    assert!(RemoteDesktopSession::takeover(&second.client,RequestId::new(),WAIT).await.is_err());
    assert_eq!(backend.input_held.load(Ordering::SeqCst),1);
    RemoteDesktopSession::takeover(&second.client,token,WAIT).await.unwrap();
    assert_eq!(backend.input_active.load(Ordering::SeqCst),0);
    assert_eq!(backend.input_held.load(Ordering::SeqCst),0);
    assert!(old.was_taken_over().await);
    assert!(old.input_control().send(RemoteDesktopInput::Key{virtual_key:65,down:true}).await.is_err());
    let mut next=RemoteDesktopSession::open(second.client.clone(),None,30,WAIT).await.unwrap();
    next.next_frame(WAIT).await.unwrap();
    // A stale prompt must not evict the replacement owner.
    assert!(RemoteDesktopSession::takeover(&first.client,token,WAIT).await.is_err());
    next.next_frame(WAIT).await.unwrap();
    assert_eq!(backend.maximum.load(Ordering::SeqCst),1);
    next.close().await.unwrap();
    first.legacy().await;second.legacy().await;
}
