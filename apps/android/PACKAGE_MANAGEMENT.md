# Android 应用安装与卸载

最低 Android 9（API 28）。使用现有 `SystemQuery::Applications`、调用方身份、执行上下文、请求 ID 去重及操作记录，增加 system-query v16；旧工具仍使用原有版本门槛。桌面安装程序管理暂未实现。

## 使用流程

1. 将单个 APK 上传到 `/workspace`，或选择用户已授权目录中的 APK。当前上限 32 MiB，不支持分包集合、APKS、XAPK、APKM 或 App Bundle。
2. 用 `pab_list_execution_contexts` 获取本连接的 `desktop_user` selection。
3. 调用 `pab_inspect_app_package`，检查包名、版本、APK SHA-256、签名证书 SHA-256 及可见的已安装版本。元数据不代表可信认证；最终签名与兼容性验证由 Android 安装器执行。
4. 启动 Pixels 时会主动引导补齐安装来源权限，由 Android 设置页确认。首页的“检查应用权限”和“管理安装应用权限”也可重新进入。拒绝此权限时，安装工具返回 `install_source_permission_required`。完整启动流程见 [STARTUP_PERMISSIONS.md](STARTUP_PERMISSIONS.md)。
5. 调用 `pab_install_app`，明确传入 `path`、检查得到的 `package_id` 和 `sha256`。升级必须使用更高版本、兼容签名；不通过卸载解决升级失败。
6. 通过系统正常确认页完成或取消。`result.state=completed` 只表示本次接口已返回；包操作的实际结果见 `result.data.snapshot.result.state`。获得 `change_ref` 后，用 `pab_get_app_change` 查询后续结果。
7. 暂停、锁屏或后台策略导致确认页未显示时，恢复控制并调用 `pab_resume_app_change`。它只恢复原确认，不创建新的安装／卸载请求。系统确认凭据已失效时明确报错，不重建丢失的回调、不猜测成功。

卸载使用 `pab_uninstall_app`，同时指定包名和已查询到的 `version_code`，由系统确认。卸载会删除目标应用及其数据；取消保留应用。Pixels 的开发版、正式版及系统应用受保护。

## 状态与恢复

部分 MIUI 设备的卸载 observer 路径还出现过：确认页已经取消或应用已移除，却没有最终状态回调。因此，系统卸载处理器为 `com.miui.packageinstaller` 且 API 不高于 34 时，使用标准 `ACTION_UNINSTALL_PACKAGE` 和 `EXTRA_RETURN_RESULT`。仍由系统显示确认页；只有系统返回成功且目标包已不存在，才记为成功；系统返回取消且原版本与签名仍匹配，才记为取消。矛盾结果记为 `unconfirmed`。原任务恢复与禁止重放规则同安装兼容路径，其他处理器继续使用 PackageInstaller observer。

卸载兼容路径在受理和打开确认页前核对版本，后一次还核对签名。标准卸载 Activity 没有原子的版本锁，确认页显示期间由其他来源并发更新目标应用不在保证范围内。升级前遗留的 MIUI observer 等待记录在查询时标为 `unconfirmed` 并撤销本应用的恢复凭据；这不撤销系统操作，也不推断旧操作结果，原回调若最终到达仍可补齐事实。原确认任务已消失时同样明确标为不确定结果，不自动创建另一操作。

默认安装使用 PackageInstaller Session。这台 Android 14 真机当前的 MIUI 安装器会拒绝 API 34 及以下的 Session 确认入口；已读取设备安装器字节码确认该分支。仅当系统解析出的安装器为 `com.miui.packageinstaller` 且系统 API 不高于 34 时，改用标准 APK 安装 Activity，同样需要来源权限和系统确认，不修改系统安装器。

兼容路径先持久化请求，再启动系统界面。只有收到系统成功结果，并核对实际安装版本及当前签名证书集合一致，才报告成功；结果矛盾时报告 `unconfirmed`。恢复只把原有任务带到前台，不重新发起安装。进程中断后缺少原始结果的活动不能自动重放。暂存 APK 使用本次请求独有的只读 URI，明确终态后撤销授权并清理。

| 状态 | 含义 |
|---|---|
| `inspected` | 只完成 APK 检查，没有变更请求 |
| `submitted` | 已保存请求并提交／准备提交系统，尚无终态 |
| `waiting_user_action` | Android 要求确认；不表示已安装或已卸载 |
| `succeeded` | 收到原系统回调的成功状态 |
| `cancelled` | 收到系统中止状态，包含用户取消 |
| `failed` | 明确的暂存或系统安装器失败 |
| `unconfirmed` | 不能确定结果，保留原引用，不自动重放 |

`pab_get_operation` 查询原工具调用的历史快照；`pab_get_app_change` 查询随后到达的系统结果。每次状态观察应使用新的查询请求 ID；安装／卸载重试必须保留原请求 ID。断线、超时及应用重启不允许自动重新执行变更。

包操作记录按远程调用方隔离，使用应用私有 `no_backup/package-changes.db` 保存运行时事实。SQLite 数据库、系统 Intent 和协议 JSON 属于运行时数据，不是应用配置。配置继续使用 TOML。

一个设备同时最多接受一个明确进行中的包操作；记录容量为 512 条，达到上限时拒绝新操作，保留历史去重事实。确认页的完整 Intent 由不可变 activity PendingIntent 保存，包含卸载所需的 Binder 回调。进程内保留强引用；通知可用时，通过一条待确认通知让系统持有该凭据，明确终态后撤销通知及凭据。仅创建一个未注册到其他系统服务的 PendingIntent 不能保证跨进程存活。

数据库仅保存准确的 PendingIntent 查询字段，包含空 action、data、类型、组件、包名、分类和支持版本上的 identifier，不用字符串往返重新构造系统变更。旧 URI 格式仍可读取；凭据已失效时报告不确定结果，不重建丢失的 Binder。通知权限关闭、确认通知被撤销、系统清理或升级等情况可能使恢复不可用。状态回调使用非导出的接收器及显式、独立的可变 broadcast PendingIntent，原请求 URI 带随机校验值。终态不回退到较早的等待状态。

暂停、断线和锁屏会阻止 Pixels 新发起操作或显示确认页。已经提交给系统的请求由系统接管；暂停 Pixels 不等于撤回系统安装事务。

## 验收

仅使用 USB 真机 `e2b3b128`；模拟器按用户要求不参与。`scripts/build_android_package_fixture.py` 构建无权限的独立测试应用，包含 v1、同签名 v2 和不同签名 v3；不会通过 ADB 安装测试应用。

`scripts/test_android_packages.py` 从公网认证、强制 relay 通道调用上述工具，检查元数据与包名／哈希限制、保护目标、系统取消、正常安装、重复请求、升级保留数据、降级／异签名拒绝、卸载取消及成功，以及断线重连后的原事实查询。ADB 只用于主应用覆盖更新、设备元数据、普通首页连接和暂停设置。现有应用查询／启动／文件打开需要同版回归。

安装权限和 OEM 安全／账号确认必须通过正常系统界面完成；不使用 Root、ADB 安装权限或数据清理绕过。

## 官方依据

- [PackageInstaller](https://developer.android.com/reference/android/content/pm/PackageInstaller)：安装会话、卸载和 `STATUS_PENDING_USER_ACTION`／成功／失败回调。
- [SessionParams](https://developer.android.com/reference/android/content/pm/PackageInstaller.SessionParams)：支持的版本显式要求用户确认。
- [后台 Activity 启动](https://developer.android.com/guide/components/activities/secure-bal)：PendingIntent 创建方／发送方的启动策略；新 API 按版本检查，Android 仍执行自身限制。
- [AOSP PackageInstallerService](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/pm/PackageInstallerService.java)：卸载确认需要保留系统回调对象。
