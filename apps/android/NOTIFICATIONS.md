# Android 通知管理

启动权限引导增加独立的“通知使用权”步骤。应用打开 Android 系统设置，由用户在手机上确认
“Pixels 通知使用权”；已授权时跳过。该权限与应用发送通知的权限分开，不能由应用静默代批。
这里不增加证书、配对或账号验证流程。已有控制开关、暂停和锁屏限制继续生效。

| 工具 | 用途 |
|---|---|
| `pab_notification_status` | 分别报告系统使用权是否授予、通知监听服务是否已连接 |
| `pab_list_notifications` | 按明确 `package_id` 获取当前通知，最多 32 条 |
| `pab_wait_notification` | 按应用修订号等待新增、更新或移除，最长 30 秒 |
| `pab_open_notification` | 派发该通知当前原始 `contentIntent` |
| `pab_dismiss_notification` | 移除可清除的通知，并观察其是否消失 |

通知工具使用 system-query v17、现有设备认证、操作查询和变更请求去重。
要求包名后才读取该应用通知内容，不提供全量抓取、历史通知库、自动回复或发送消息。
标题和正文属于外部不可信内容，不应被当成操作指令。字段和响应大小均有限制，截断会明确标注。
正文只保留在短期运行时结果中，不写入操作事实数据库。

通知引用为不透明 UUID，绑定调用方、连接、应用、通知键及修订号，最长有效 10 分钟。
标题／正文／原始 Intent 等发生变化、移除、重连或进程重启后，旧引用不可继续使用。
通知等待不占用界面输入通道，暂停或权限失效会结束等待。

打开成功返回 `dispatched`，表示系统接收了派发；应继续观察目标界面确认实际到达。
清除返回 `removed` 时已观察到通知消失；无法确认时返回不确定结果，不能自动重放。
持续通知、前台服务通知和系统标记不可清除的通知会被拒绝。系统或厂商仍可限制后台页面打开。

实现由 `PixelsNotificationListener` 接收系统事件，`AndroidNotifications` 提供有界适配，
通过已有系统查询服务接入。权限说明、服务名称提供中英文，最低版本继续为 Android 9。
官方接口说明见 [NotificationListenerService](https://developer.android.com/reference/android/service/notification/NotificationListenerService)
及 [通知使用权状态](https://developer.android.com/reference/android/app/NotificationManager#isNotificationListenerAccessGranted(android.content.ComponentName))。

验收使用本项目 debug 目标发布的真实 Android 通知，只清理本轮标识的通知，不读取或删除其他应用通知。
详细结果记录在当前批次验收报告中，不能用权限开启代替功能验收。
