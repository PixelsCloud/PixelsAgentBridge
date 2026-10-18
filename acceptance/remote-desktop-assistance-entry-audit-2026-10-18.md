# AI 协助入口交付核对

日期：2026-10-18。核对源码基线 `6bf8815`。本轮关闭 LT-08.2，完整关闭数为
11/53；其他 42 项仍须各自完成。这里核对已有实机证据，没有重新运行 GUI 验收。

## LT-08.2 的逐项依据

| 要求 | 当前实现 | 已有实机证据 |
| --- | --- | --- |
| 通知发起任务所在操作端的 Desktop | `Store::current_identity` 使用当前用户的 operator 目录、账号 ID 和 revision；Desktop 观察本地 pending 请求，新请求调用 `tray::show_window` 显示提示所在主窗口 | [Windows 原始结果](remote-desktop-assistance-2026-10-17.json) 的 `product` 记录真实提示显示、无 Desktop 状态及请求方退出；不是远端机器弹窗 |
| 用户进入后打开或聚焦同一独立窗口 | 提示按钮显式调用 `desktop_assistance_open`，传入目标设备/屏幕；`open_window` 按当前账号、runtime 和设备复用窗口并聚焦 | 同份 Windows 结果的 `remote_workflow.same_window_reused=true`；[新前端原生包](remote-desktop-macos-activity-input-assistance-2026-10-18.json) 的 `assistance.second_request_same_window_and_video_session=true` |
| 默认只查看，操作必须显式开启 | 新窗口的 ViewerStatus 默认禁用输入；协助复用已有可操作窗口时发送关闭输入命令；提示不自动调用输入开启 | Windows 记录 `reuse_reset_view_only=true`、`explicit_ui_control=true`；[Mac 操作端](remote-desktop-macos-cursor-product-2026-10-18.md) 和 [Linux 操作端](remote-desktop-linux-cursor-product-2026-10-18.md) 均记录提示打开、显式启用、关闭重开后只查看 |
| 接入正式原生窗口 | `desktop_assistance_open` 调用产品 `open_window`，不是浏览器模拟画面；实际 viewing 后才绑定 active | Windows `actual_native_first_frame=true`；Linux [原始结果](remote-desktop-linux-cursor-product-2026-10-18.json) 有 waiting→active→waiting→active 历史、不同窗口及重新启用后的远端实际坐标；Mac 有正式窗口截图哈希及源端位置观察 |

核对入口为 `apps/desktop/src/DesktopAssistanceNotice.tsx`、
`apps/desktop/src-tauri/src/desktop_assistance.rs`、
`apps/desktop/src-tauri/src/operator/remote_desktop.rs`、
`apps/desktop/src-tauri/src/tray.rs` 和 `crates/bridge/src/desktop_assistance.rs`。
各实机记录绑定各自构建哈希，不能把本次核对称作当前候选的三平台重新验收。

## 不随本项关闭的范围

LT-08.3～08.6 仍保留：完整的占用/离线/投递失败、真实账号和服务切换、active
观看中的进程重启，以及跨平台完成/复核等边界。尤其 `desktop_assistance_displayed`
目前接受主 WebView 的动画完成确认，未核对原生窗口是否在确认时已被最小化或隐藏；
需要定向验证这个时序，不能用正常弹出成功证明所有投递状态准确。

本轮正在运行的新 Linux 混合测试独立记录，不以本项关闭代替 W7 长稳、W8 安装
升级或 COS 发布，也不将历史未通过轮次改记为通过。
