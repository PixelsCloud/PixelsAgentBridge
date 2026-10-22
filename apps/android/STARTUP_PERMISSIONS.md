# 启动时准备权限

## 开机恢复连接

开机启动默认开启。系统发送 `BOOT_COMPLETED`（开机后首次解锁）或 `MY_PACKAGE_REPLACED`（覆盖升级）时，恢复上次用户开启的连接；打开 App 也会尝试恢复。使用已有 `specialUse` 前台连接服务和持续通知，不自动启动屏幕采集，不弹出 Activity。Android 12+ 的这两个广播属于后台启动前台服务的例外；Android 15+ 禁止开机启动的媒体投屏等服务类型不用于此入口。

开关保存在现有 `no_backup/appearance.toml` 的 `start_on_boot` 布尔字段，缺省为 `true`，旧配置无需手动迁移。连接意愿、暂停状态继续使用 `control-intent.toml`：明确停止的连接不会被开机、更新或打开页面重新开启，已暂停的输入保持暂停。配置不可读时保持原文件，不恢复连接。

设置页提供自启动开关、系统应用设置和后台耗电设置入口。厂商的自启动与后台运行限制需要用户在系统界面开启；强行停止后系统可能不再发送广播。系统拒绝后台启动时，在通知权限允许的情况下提示用户点击打开 App 恢复。安装后至少需打开一次 App 并配置、开启连接；尚未首次解锁时不读取身份和凭据，不承诺解锁前上线。屏幕采集仍遵循已有会话授权。

参考：[后台启动前台服务的例外](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start)、[Android 15 开机服务类型限制](https://developer.android.com/about/versions/15/behavior-changes-15#fgs-boot-completed)、[Direct Boot 与首次解锁](https://developer.android.com/privacy-and-security/direct-boot)。

## 首次权限准备

应用进入首页即检查当前设备所需权限，自动展示缺少权限的准备流程，不需要先连接远程端或等工具报错。已满足的项目跳过；用户返回系统页面后重新检查并继续下一项，同一 Activity 会话内不反复请求已尝试的项目。首页“检查应用权限”可重新开始，用户也可以选择“稍后设置”。

| 项目 | 授权方式与范围 |
|---|---|
| 通知 | Android 13+ 使用运行时请求；已关闭或永久拒绝通知时也提供系统通知设置入口。旧版本检查通知开关。授权本身不启动连接 |
| 通知使用权 | 独立于发送通知权限；打开系统通知使用权设置，由用户开启“Pixels 通知使用权”。返回后重新核对授权，监听服务绑定状态由通知状态工具分别报告，详见[通知管理](NOTIFICATIONS.md) |
| 无障碍 | 显示用途后打开系统无障碍设置，检查实际启用的 Pixels 服务。是否已启用与服务是否正常绑定分别展示 |
| 安装来源 | 打开本应用的“安装未知应用”页面，返回后检查 `canRequestPackageInstalls`。不自动授予权限；每次包操作的系统确认继续保留 |
| 文件目录 | 打开系统目录选择器申请持久读写授权；也可以明确选择“仅使用工作目录”。只读目录维持其实际授权范围，不申请全盘存储权限 |
| 截图 | Android 9／10 使用系统屏幕捕获授权；Android 11+ 使用已配置的无障碍截图实现，无需另开捕获会话 |

网络和前台服务等普通声明权限由 Android 安装时处理。不会请求项目未使用的相机、联系人、定位等权限，也不会新增证书或账号流程。

这台 MIUI 真机在安装独立 APK 时还要求小米账号验证，在 Pixels 首次打开测试应用时显示“允许打开其他应用”。这些是厂商在具体操作时追加的系统确认，不能通过普通 Android 权限接口在启动时统一授予。账号验证由用户在手机上亲自完成；不收集账号密码，也不关闭或绕过系统确认。

“仅使用工作目录”的选择以 `no_backup/permission-setup.toml` 保存，单独使用原子文件写入，不修改连接设置、凭据、设备身份或暂停状态。Activity 实例状态保存当前步骤和已尝试项目；系统页面未返回时不会再次启动同一请求。进程或页面重建后重新读取实际系统授权及本地选择。

代码分为纯 Kotlin 的 `PermissionSetupSequence`、Activity 的 `StartupPermissions`、Compose 提示界面和独立 TOML 选择存储。三个核心测试验证已授权跳过、拒绝后不循环，以及状态恢复／重试。

USB 真机验收脚本为 `scripts/test_android_startup_permissions.py`，通过普通本地界面检查启动提示、目录选择器取消、工作区选择、重新检查／推迟、停止连接时不被授权流程重新连接，以及核验当前应用进程后注入重启故障并确认选择保留。已有工作目录选择时使用 `--prepared`。这台 MIUI 真机在 SIGKILL 后会把无障碍服务标为崩溃；测试通过已有 `android_accessibility.py` 在正常系统界面恢复绑定，不能记作系统自动恢复。没有用 shell 权限命令授予或撤销权限，也不会清空应用数据。完整 APK 包管理随后经公网认证 relay 单独验收。

系统特殊权限需要用户在系统设置页确认，见 [Android 特殊权限流程](https://developer.android.com/training/permissions/requesting-special)。屏幕捕获授权属于会话，不能永久预先获取，见 [MediaProjection 会话授权](https://developer.android.com/media/grow/media-projection)。Android 9 目前按用户要求不使用模拟器实测，最低 API 28 由构建与 Lint 校验。
