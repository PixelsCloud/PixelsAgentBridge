use super::*;
pub(super) async fn publish_occupied(window:&WebviewWindow,owner:&Owner){
    let session=async {
        let device=owner.runtime.resolve_device_code(owner.code).await?;
        owner.runtime.desktop_occupancy(device).await
    }.await.ok().flatten();
    publish(window,owner,"occupied",None,None);
    let mut status=owner.status.borrow().clone();
    status.takeover_session=session;
    owner.status.send_replace(status.clone());
    let _=window.emit_to(window.label(),"desktop-viewer-status",status);
}
pub(super) async fn confirm(window:&WebviewWindow,state:&RemoteDesktopState,english:bool)->Result<bool,String>{
    let owner=state.owner(window).await?;
    let Ok(_prompt)=owner.close_request.try_lock() else{return Ok(false)};
    let expected={let status=owner.status.borrow();if status.phase!="occupied"{return Ok(false)}status.takeover_session.ok_or("Takeover requires an updated remote app")?};
    let (title,body,confirm,cancel)=if english {
        ("Take over remote desktop?","This device is being controlled by another session. Taking over will disconnect that session and release its held keys and mouse buttons.","Take over","Cancel")
    }else{
        ("是否强制接管？","这台设备正在被其他会话控制。强制接管将断开原控制端，释放其键盘和鼠标状态，然后建立你的连接。","强制接管","取消")
    };
    if !crate::confirmation::ask(window.app_handle(),Some(window),title,body,confirm,cancel).await{return Ok(false)}
    // Revalidate both the local account/window and the exact remote session.
    let current=state.owner(window).await?;
    if !Arc::ptr_eq(&owner,&current) || *owner.stop.borrow() || owner.status.borrow().takeover_session!=Some(expected){return Ok(false)}
    let device=owner.runtime.resolve_device_code(owner.code).await.map_err(|e|e.to_string())?;
    owner.runtime.take_over_desktop(device,expected).await.map_err(|_|if english{"The remote session changed or could not be released. Reconnect and confirm again."}else{"远端会话已变化或暂时无法释放，请重新连接后再次确认。"}.to_owned())?;
    owner.commands.try_send(Command::Retry).map_err(|_|"Viewer is closing".to_owned())?;
    Ok(true)
}
