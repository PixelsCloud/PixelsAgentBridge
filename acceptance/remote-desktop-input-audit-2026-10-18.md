# 输入验证与剩余工作报告

日期：2026-10-18。产品源码基线 `55eec5c`，本轮只增加测试和核对交付记录，
不改变产品认证、传输或输入行为，不更新本机安装版。

## 当前交付状态

53 条交付要求中，10 条完整关闭、43 条待收口。本轮关闭 04.1。
未关闭项包含已经实现但尚未完成全条件验收的工作，数量不是代码完成率。

| 剩余工作 | 已有结果 | 尚需完成 |
| --- | --- | --- |
| 光标、多屏和输入 | 三平台原生显示和基本输入；XRServer 真实四屏切换；Linux 锁屏光标恢复 | Mac 光标源正式窗口实测、X11 系统隐藏光标、90 首次激活重复消息、混合 DPI/旋转/热插拔、各平台键位与输入法 |
| 系统登录 | Windows 锁屏登录；Linux 注销登录后自动恢复；Linux 唤醒时序修复 | Mac 系统登录；Windows 开机未登录、UAC、用户切换等边界 |
| 媒体和网络 | 硬软编解码及故障后备；Linux 单屏短测约 14.4 FPS；定向断流和短时限速 | 各平台 1080p 动态画面 >12 FPS 的完整基准、硬件设备故障、持续弱网、Direct/Relay 变化、资源曲线 |
| AI 人工协助 | request/status/wait/cancel 和真实请求到用户操作的基本闭环 | 占用、离线、重复、过期、取消、账号变化等完整边界 |
| 整体验收 | 若干真实平台方向和故障轮次通过 | 九种平台组合、30 分钟混合业务、2 小时观看、100 次开关、候选版 8 小时稳定性；Linux GTK/ATK 告警定位 |
| 集成发布 | 开发分支持续提交，三平台已有候选构建 | 主线集成、安装升级/卸载/自启动、客户 TOML 保留、旧业务回归、COS 发布与下载更新 |

Mac 按用户确认的单屏条件验收。本次经现有连接只读检查 Apple，控制台 UID
501、`CGSSessionScreenIsLocked=true`，原服务仍加载，测试进程和编译进程为空。
没有解锁、注销、改变 TCC 或替换测试 App；普通桌面的源端复测仍待可用会话。

## 04.1 关闭依据

| 要求 | 当前实现和已有证据 |
| --- | --- |
| 默认只查看及状态反馈 | `RemoteDesktopWindow.tsx` 显示启用/停止操作和状态；原生窗口状态初始及每次恢复均为禁用输入；前端既有测试覆盖首帧前不可操作、切屏复位及显式状态 |
| 显式开启与关闭 | [Mac 正式窗口](remote-desktop-macos-cursor-product-2026-10-18.md)、[Linux 正式窗口](remote-desktop-linux-cursor-product-2026-10-18.md) 验证启用/停止操作及重开默认只查看；[Windows→Linux 正式窗口](remote-desktop-linux-wake-recovery-2026-10-18.md) 验证恢复后只查看及实际 Ctrl 释放 |
| 会话与代次绑定 | `apps/desktop/src-tauri/src/operator/remote_desktop.rs` 的 `InputGeneration` 同时绑定 session 和 generation，接收命令时比对当前身份；恢复不重放旧命令 |
| 沿用设备授权及 helper | Bridge 的 `runtime/remote_desktop.rs` 和 `connection/remote_desktop.rs` 复用已认证连接；`local_ipc_media_input.rs` 以活动采集配置选取 helper，并复用 helper 身份验证及独立输入通道 |

这里关闭的是开关、反馈和路由要求，不关闭 04.2 的鼠标行为、04.3 的键盘布局、
04.4 的全部释放边界或整个 W2。已有正式窗口证据来自所链接的各自构建，
本轮没有重新运行三平台 GUI 验收。

## 新增原始消息检查

入口：`crates/remote-desktop/tests/input_wire/mod.rs`。复用真实本地 Iroh/QUIC
连接，绕过正常客户端的递增序号生成器发送消息。输入 sink 为计数测试实现，
视频为确定性测试载荷；不声称测试了系统键鼠注入、密码认证或 H.264。

1. 正常启用并按下 Ctrl 后，分别发送序号 0、1、2、4、u64 最大值；预期下一
   序号为 3。控制流结束、输入与采集回收、持有状态释放，sink 只执行一次输入。
   同连接旧业务探针仍可响应，新会话默认不能发送输入，重新启用也不重放旧键。
2. 当前代次启用后，发送旧代次 0 和未来代次 2 的输入，收到 `StaleGeneration`，
   sink 执行次数仍为 0；下一条合法输入执行一次。关闭控制流后释放输入与采集。
3. Open 使用零、旧版本和未来版本均返回 Unsupported，不能附着控制流或占用
   查看者；随后合法会话可打开，保持仅查看，关闭后原连接仍可响应。

“旧业务探针”只验证同连接另一流的 GetPresence 响应，不替代终端、命令、文件
传输的混合业务验收。sink 计数只证明提交次数，不证明远端应用完成动作。
04.5 保持未完整关闭，90 系统首次激活的重复消息也仍保留。

执行 `cargo test --locked -p pab-remote-desktop --lib --test reliable_sessions -j4`：
9 项库单测、34 项可靠会话测试通过，0 失败。构建 25.42 秒，会话测试 14.30 秒。
Cargo 4 jobs，原生嵌套构建单任务；未同时启动其他编译。

原始日志：`.build/remote-desktop/desktop-input-wire-tests.log`，SHA-256：
`ab62227661ada7937203fc7b35735012348ea3127a46faca6d1da3b934a920b5`。
本轮没有执行新的三平台打包、安装或 COS 发布。
