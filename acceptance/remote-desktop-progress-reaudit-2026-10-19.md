# 远程桌面进度复核：已实现、待验证与待交付

日期：2026-10-19。进度复核阶段核对代码与记录至 `c65b153`，六条关闭只依据既有结果，没有重新运行验收。随后按用户要求处理一个已确认代码缺口，结果单独记录在[软件编码容量修复](remote-desktop-software-capacity-recovery-2026-10-19.md)，代码为 `3d0b23d`；没有更新安装版或发布。

## 计数口径修正

先前的“12/53 完整关闭、41 未完整关闭”是粗粒度验收条目数，不能回答“还缺多少功能”。
其中 38 条已有部分完成结果，只有 09.3、09.6、09.7 分别标为待验收、待执行、待验收；
最终包回归又被重复附加到多个功能条目，导致已有实现和实测长期不被关闭。

本次按各条原文及用户已确认范围复核，将六条移为已关闭：
**18 已关闭 + 32 部分完成 + 3 交付待验收/执行 = 53**。
关闭功能条目不表示最终安装包已验收；09.x 仍保留。未关闭数不等于待开发功能数，不计算代码完成百分比。

## 六条关闭的依据

| 条目 | 原要求已有的证据 | 不随本条关闭的内容 |
| --- | --- | --- |
| 01.3 正确交互会话创建和退出媒体 | Windows Default/Winlogon、Mac Aqua/LoginWindow、Linux 用户/GDM 均已有实机交接；helper 退出及旧注册退休有代码和运行记录 | 01.6 全部故障组合、09.3 正式安装 |
| 03.3 稳定屏幕 ID 与统一布局 | 既有四适配器稳定 ID 枚举/切屏；90 负原点竖屏、两种窗口尺寸、100%/125% 双屏实际坐标 | 不把四适配器枚举当四屏点击；03.4 动态布局、03.5 光标、04.2 剩余输入组合 |
| 05.1 Windows 媒体跟随桌面 | 同一窗口跟随 Default/Winlogon、注销、UAC 和用户交接；代理退出后回收恢复 | 05.4 剩余故障与 09.3 安装链路 |
| 05.3 Windows 各系统会话场景 | 10-17 和 10-19 已有多次锁屏/SAS/登录，另有注销、修复后真实重启、普通用户、真实 UAC 和管理员切换 | 06.7 长稳、09.3 最终包回归；旧候选失败保留 |
| 07.1 Mac 被控端接入 | 原生 CVPixelBuffer/IOSurface 与 VideoToolbox；单屏正式窗口光标/输入、Aqua/LoginWindow、重复注销、真实重启密码登录及自动恢复 | Mac 多屏按用户要求不阻塞；06.1 新码率 helper、06.6 性能和 09.3 安装仍未关闭 |
| 07.4 Wayland 普通桌面和登录接入 | 真实用户/GDM helper UID 交接、锁屏、注销、重启后密码登录、恢复、旧进程及虚拟输入回收 | 06.1 最新预算 helper、06.6 性能、09.3 正式安装 |

代码入口：`crates/executor/src/local_ipc_media.rs`、`apps/desktop/src-tauri/src/session_helper*.rs`、
`apps/desktop/src-tauri/src/windows_session_supervisor.rs`、`crates/desktop-media/native/{windows,macos,linux}`。
`HelperSource::drop` 退休所属注册，Linux helper 监视图形会话，Mac 媒体/输入绑定 Aqua/LoginWindow；不是新增另一套会话体系。

主要运行依据：[Windows 输入与登录](remote-desktop-windows-input-login-2026-10-19.md)、
[负原点竖屏](remote-desktop-windows-portrait-matched-2026-10-19.md)、
[混合 DPI 与服务切换](remote-desktop-active-switch-mixed-dpi-2026-10-19.md)、
[历史四适配器枚举](remote-desktop-multiscreen-2026-10-17.md)、
[Mac 单屏输入与光标](remote-desktop-macos-activity-input-assistance-2026-10-18.md)、
[Mac 重复注销与重启](remote-desktop-macos-login-startup-followup-2026-10-19.md)、
[GDM helper 交接](remote-desktop-linux-gdm-product-2026-10-17.md)、
[Linux 注销与重启](remote-desktop-linux-session-boot-2026-10-19.md)。

## 已经做过，后续直接复用

| 能力 | 已完成的范围 | 剩余限制 |
| --- | --- | --- |
| 独立窗口与既有连接 | 正式窗口、可靠传输、原账号/设备授权、Direct 与强制 Relay | 各平台设备菜单重复聚焦、全屏/恢复组合尚未收齐 |
| 多屏和坐标 | 真实切屏、负原点、竖屏、双窗口尺寸、100%/125% 双屏基本点击 | 动态布局及混合 DPI 更多光标/窗口组合 |
| 基本输入 | 三平台已有真实鼠标/键盘输入；90 记事本单/双击、拖拽、滚轮、Ctrl+A；Linux X11→90/Mac 基本操作 | 完整键位/输入法、拖出释放等剩余组合 |
| 三平台被控端系统登录 | Windows、Mac、Wayland 均有锁屏、注销、真实重启密码登录及恢复；Xorg GDM 登录也已有记录 | Mac/Linux 作为操作端时的目标登录缺项、最终包回归 |
| 单查看者 | 两个真实 Desktop：第二端 Busy、第一端退出后接手 | 不新增设备数量或使用次数限制 |
| AI 人工协助 | 请求、定向提示、打开/复用窗口、默认只查看、显式完成到 MCP；Windows active 取消/崩溃/过期 | Mac/Linux 隐藏/最小化通知和 active 异常、离线/删除设备、另一实际账号 |
| 服务配置 | 私有编译 TOML、公开默认空值、客户覆盖优先；真实设置页切换后清理旧身份/观看/协助，重启不恢复 | 客户实际新服务和三平台正式安装；回环地址测试不等于新服务登录成功 |
| 自适应与性能 | helper 预算转发；90 实际限速降档与恢复 14.70 FPS；Linux 最新 helper 正式窗口 1306×768 实际 14.696 FPS、受限新视频 session 约 2.119 秒出首帧；Mac 修复在线显示器后最新 helper 显示锁屏 | Mac 动态 FPS、最新 helper 非零预算变化及解除限速动态恢复、RTT/路径缺项；Linux 当前结果不是 1080p 通过 |
| 旧业务 | 视频并行及关闭后 PTY/命令/4 MiB 传输；限速时命令/PTY；终端末行遮挡已修复 | 最终包受影响业务回归与安装升级 |

补充依据：[Xorg/X11 与最小化](remote-desktop-xorg-x11-minimize-2026-10-19.md)、
[登录、占用、AI 与原业务](remote-desktop-login-occupancy-assistance-2026-10-19.md)、
[私有默认配置](remote-desktop-client-defaults-2026-10-19.md)、
[helper 与实际限速](remote-desktop-helper-budget-weak-network-2026-10-19.md)。

## 真正剩余的四类工作

1. **问题定位与必要修复**：高熵软件编码 error 3 已在 3d0b23d 修复；本轮 Mac 15 项、Linux 16 项软件回归通过，各忽略 1 项既有手动基准。Mac 新包另定位并修复锁屏在线显示器误判，原生回归及正式窗口锁屏显示通过。X11 最小化恢复聚焦仍未验证；Windows 特殊不激活夹具保留，普通窗口与记事本已通过。只对确认的产品问题改代码。
2. **现有实现的缺项验收**：Linux 受限新视频 session 首帧和 Mac/Linux 最新 helper 正式窗口已补，不再整体列为待开发，见[新窗口与首帧](remote-desktop-helper-window-first-frame-2026-10-19.md)。仍缺 Mac 动态 >12 FPS、最新 helper 非零预算变化、解除限速动态恢复、RTT/路径变化；另补原清单剩余输入/窗口/光标、操作端登录、AI 生命周期及实际另一账号/新服务。不另行强制跨新 session 持久化旧预算。
3. **最终候选与交付验证**：主线集成、固定三平台有版本和哈希的安装包，核对依赖/签名/压缩；正式新装/升级/卸载、自启动、TOML/身份保留、受影响旧功能回归，30 分钟混合与 100 次开关。2 小时和 8 小时仍按用户要求留到晚上。
4. **发布**：必需验收通过后 push、COS、下载校验、自动安装和回滚验证。当前尚未发布本轮新包。

以上四类是执行批次，不是四个尚未开发的模块，也不是已穷尽所有测试用例。
同一构建和条件下已通过、且未受改动影响的结果直接复用；最终包仅重做必要集成与受影响范围，避免重复测试代替推进交付。
Apple 单屏、两种同平台双端组合跳过、XRServer 停用和晚间长测安排保持不变。

剩余 35 个编号已逐个分配至长期任务的[唯一执行清单](../REMOTE_DESKTOP_LONG_TERM_TASK.md#去重后的执行清单)，覆盖窗口/输入、媒体/网络、生命周期/协助、操作端平台、稳定性和安装发布；分组没有增加功能要求。后续只更新该清单和对应原条目的证据，避免多个计划反复列出同一个缺口。
