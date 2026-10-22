# 各平台默认自启动

默认开启，桌面端延续已有安装行为，Android 新增开机恢复。

| 平台 | 默认安装行为 | 入口 |
|---|---|---|
| Windows | 系统 Executor 服务自动启动；用户登录后打开 Desktop 窗口 | `packaging/desktop/windows/install.ps1`：Automatic 服务、SYSTEM 监督任务、CommonStartup 快捷方式 |
| macOS | 系统 LaunchDaemon 启动 Executor；Aqua 登录会话打开 Desktop 窗口 | `packaging/desktop/macos/com.pixelsagentbridge.executor.plist`、`com.pixelsagentbridge.desktop.plist` 的 `RunAtLoad = true` |
| Linux Desktop | systemd 自动启动 Executor；登录图形桌面后打开窗口 | `packaging/desktop/linux-desktop/postinst` 启用服务；系统 XDG autostart 条目默认启用 |
| Linux 无桌面版 | systemd 自动启动 Executor | `packaging/desktop/unix/install.sh`；不依赖图形登录 |
| Android | 默认开启开机恢复，首次解锁后恢复上次已开启的连接；覆盖更新后也尝试恢复 | `StartupReceiver`、`ConnectionService`；`appearance.toml` 中 `start_on_boot` 缺省为 `true` |

Android 不强制从后台弹出窗口。沿用持续前台服务和停止通知，保留 `control-intent.toml` 中的明确停止与输入暂停状态。开启自启动不会替用户首次配置服务器或开启一个已手动停止的连接。

设置页提供自启动开关、应用与自启动设置、后台耗电设置。系统拒绝启动时尽量通知用户点击恢复；打开 App 也重试恢复。厂商拦截广播、强行停止、尚未首次解锁以及屏幕采集会话授权仍受系统限制。详见 [Android 开机恢复与权限说明](apps/android/STARTUP_PERMISSIONS.md)。

升级应保留已有设备身份、设置与用户数据。
