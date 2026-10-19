# Apple 重复注销登录与启动域修复

日期：2026-10-19。延续[首次注销登录](remote-desktop-macos-logout-login-2026-10-19.md)，Apple M4 / macOS 27.0.1、单屏 1920×1080。

## 已验证

同一个已授权测试 App 再次通过正式独立观看窗口注销，控制台从 UID 501 转为 UID 0。实际画面显示 LoginWindow 密码输入框，观察期间没有再次弹出屏幕访问授权提示。经画面核对后只提交一次密码，控制台恢复 UID 501，窗口自动恢复用户桌面。新视频 session 为 `f7fa386f-22a7-4645-aaee-9cf94dae1cfc`，generation 1，D3D11 硬解；操作权限复位为只查看。本次没有测量持续 FPS。

隔离 profile：`.build/remote-desktop/mac-login-product-5d2bb0c9ef4b`。本地私有截图包括 `owned-surface-1792376021315166200.png`（无授权提示的登录画面）、`owned-surface-1792376100878057300.png`（输入前核对）、`owned-surface-1792376154631696500.png`（恢复桌面）。测试结束后观看窗口、隔离 Desktop/MCP 退出，原服务 PID 91694 恢复，测试 stage 删除，安装文件和设备密钥哈希不变。恢复检查任务 `24524e2d-bff2-4ce1-8feb-b431740668f4`，清理任务 `19408e03-9920-40e9-a651-0086adaf89e8`。

## 安装脚本修复

真实 LoginWindow 的 audit session ID 为 102243，而 `user/0` 的为 100004。旧 `pab_loginwindow_domain()` 仅从 `user/0` 推导，因此在实际登录界面返回不存在。共享的安装/卸载生命周期脚本现优先验证并使用 `loginwindow` 目标；仅在该目标不可用且控制台 UID 为 0 时尝试原数字域后备。

只读实机对照：旧脚本探测任务 `3c5e0bee-a52e-4a13-8186-321417153b3b` 返回空域/失败；修复后任务 `3ee5662b-3e16-419a-a788-e4b1aba412d6` 返回 `loginwindow`；用户登录后任务 `0c57369b-9b12-493a-8312-ccf2b25b7e7f` 正确返回无活动 LoginWindow。

10 项启动测试通过，覆盖别名优先、数字后备条件、新安装/升级/卸载及重复执行。Apple 原生 Bash 3.2.57 上另有 4 项升级测试通过。包结构检查 5 项通过、4 项因平台或完整包条件跳过。这些是脚本检查，不能替代最终安装包升级验收。

## 第一次开机测试：部署失败，随后恢复

已执行一次真实重启，新 boot 时间为 2026-10-19 10:21:08，控制台 UID 为 0，设备服务重新连接。但 macOS 清空了 `/private/tmp`，测试 App 原来只保存在该目录，导致测试 helper 无法启动。本次失败属于测试部署缺陷，不能据此认定产品开机登录通过或失败。

未在此轮提交密码；已停止隔离任务、恢复原服务 PID 906，并删除测试启动项和 stage。隔离 profile 为 `.build/remote-desktop/mac-login-product-7b31ba1e908c`。

随后从固定来源重建同一签名身份的测试 App，耗时 4 分 44 秒，完整包保存在 `~/Library/Application Support/PixelsRemoteDesktopAcceptance`。构建后签名二进制 SHA-256 为 `d4b10da0bb6465f365f9b34dbe27c4b521b015ba5375bb3129b1dbda7fa40f8f`。在已经启动的 LoginWindow 手动接入候选服务后，画面与一次密码登录可用，但测试父目录 0700 阻止用户会话 helper 启动；修正为 0711 并仅重启测试 helper 后才恢复。该轮不记自动恢复通过。持久路径中的重建包在 LoginWindow 和 Aqua 各触发一次屏幕访问提示，按用户已有授权点击允许。恢复原服务 PID 3959 并清理 stage 后，才进入下一轮。

## 第二次开机测试：候选程序自动登录链路通过

在同一持久路径、同一签名及二进制下再次真实重启，boot 时间为 2026-10-19 10:44:24（1792377864）。FileVault 关闭，未配置自动登录；实际控制台 UID 0。测试 LoginWindow LaunchAgent 自动运行（PID 286），没有通过手动 kickstart 或重新注册补启动。正式独立窗口直接显示密码框，本轮未弹权限确认。

显式启用操作后，通过观看窗口只提交一次系统密码。控制台恢复 UID 501，Aqua helper 自动运行（PID 1674），观看窗口自动恢复真实用户桌面；没有手动启动 helper、重新打开窗口或补发密码。新 session `7cd02f91-3770-4d2a-96eb-29e129f9dd85`，generation 1，`d3d11_hardware`，`input_enabled=false`。Aqua 也未再次提示授权。

隔离 profile 为 `.build/remote-desktop/mac-login-product-6808bf1b7371`。截图 `owned-surface-1792377923553461200.png` 证明开机密码界面，`owned-surface-1792377975371619900.png` 证明恢复桌面。结束后原服务 PID 2042 恢复，安装文件哈希不变，独立 Desktop/MCP 退出，测试启动项和私有 stage 已删除；持久化构建缓存及 App 留作后续复用。

此结果证明候选程序在真实开机系统登录场景可用，**不替代最终安装包的新装/升级验收**。测试使用隔离 Executor 和临时 launchd 注册，自动回滚原服务；未覆盖 FileVault 预启动、其他 Mac 版本、多屏或持续 FPS。详细产物、截图哈希及状态见[机器记录](remote-desktop-macos-login-startup-followup-2026-10-19.json)。

构建约束按用户最新明确要求执行：**每台机器分别最多 10 核/作业**，计入同机其他会话和嵌套编译。Apple 本轮 Cargo 为 4 作业，嵌套原生编译为 1；本机未追加编译。2 小时和 8 小时测试继续留到晚上；本记录不关闭整个 LT-07 或最终安装验收。
