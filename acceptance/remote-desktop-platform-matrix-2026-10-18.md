# 平台组合的证据与剩余验收（七个必验方向）

范围更新（2026-10-19）：用户确认暂时没有额外 Mac/Linux 桌面，明确本轮无需验证
Mac→Mac、Linux→Linux。两项保留“未验证”事实，移出本轮必验与交付阻塞范围；
其余七个方向继续按实际证据验收。Apple 按单屏处理，多屏不作为本轮阻塞。

最新核对：2026-10-19，基线 `7efc841`，另补 Xorg/X11 基本输入与 Windows 最小化释放修复；包含既有 Windows、Linux Wayland 和 Apple 登录/重启证据；原表基线为 `a4a58d8`。
随后基线 `3d83e3f` 补 Windows active/实际服务切换及 90 100%/125% 双屏基本坐标，见[最新验收](remote-desktop-active-switch-mixed-dpi-2026-10-19.md)。用户明确后续不再使用 XRServer，Windows 测试使用 90。
本轮没有关闭 LT-07.6。各记录的产物哈希分别
有效，不能混成一份已经通过全部功能的发布包。被控端登录通过不能计为反向操作端通过。

| 操作端 → 被控端 | 已有正式产品窗口证据 | 仍缺的主要证据 |
| --- | --- | --- |
| Windows → Windows | [90 双屏切换、点击与热拔恢复](remote-desktop-windows-dual-viewer-2026-10-18.md)；[统一产物负原点竖屏、缩放点击、WGC 光标和采集代理恢复](remote-desktop-windows-portrait-matched-2026-10-19.md)；[普通应用输入、注销、重启恢复、普通用户、UAC 与会话切换](remote-desktop-windows-input-login-2026-10-19.md)；[100%/125% 双屏四次点击与 active 清理](remote-desktop-active-switch-mixed-dpi-2026-10-19.md)；[普通启动首次点击](remote-desktop-windows-normal-show-2026-10-19.md) | 混合 DPI 的更多尺寸/光标组合、完整输入及特殊不激活夹具首次消息边界；最终候选反复切换、安装链路、动态基准与长稳 |
| Windows → macOS | [Apple 单屏输入、独立光标、协助占用及恢复](remote-desktop-macos-activity-input-assistance-2026-10-18.md)；[锁屏密码登录](remote-desktop-macos-lock-login-2026-10-19.md)；[注销后 LoginWindow 密码登录和画面恢复](remote-desktop-macos-logout-login-2026-10-19.md)；[重复注销、真实重启密码登录及自动恢复，未再次授权](remote-desktop-macos-login-startup-followup-2026-10-19.md) | 最终候选动态有效 FPS、完整输入与长稳；正式安装 helper 链路。重启结果来自隔离候选包，不代替安装升级。单屏不宣称多屏通过 |
| Windows → Linux | [原生窗口、输入及网络恢复](remote-desktop-product-flow-recovery-2026-10-18.md)；[普通 Xorg 桌面鼠标/文本、缩放及 Windows 最小化释放修复](remote-desktop-xorg-x11-minimize-2026-10-19.md)；[Xorg GDM 密码登录](remote-desktop-linux-system-login-2026-10-17.md)；[Wayland 注销、真实重启、密码登录及自动恢复](remote-desktop-linux-session-boot-2026-10-19.md)；[锁屏、占用、AI 完成及原业务](remote-desktop-login-occupancy-assistance-2026-10-19.md) | 反复会话切换、复杂多屏和完整输入法；30 分钟混合及长期资源验证；最终安装启动路径 |
| macOS → Windows | [Apple 正式窗口接收 90 光标、移动输入、只查看与故障隔离](remote-desktop-macos-cursor-product-2026-10-18.md) | 新候选完整键盘/鼠标、动态 FPS、多屏、Windows 系统登录与长期负载 |
| macOS → macOS | 未验证；用户于 2026-10-19 确认本轮跳过 | 不作为本轮交付阻塞项，不记为通过 |
| macOS → Linux | [正式窗口首帧、实际点击/Ctrl、关闭释放及协助闭环](remote-desktop-macos-to-linux-2026-10-17.md) | 当前候选动态 FPS、独立光标、系统登录、完整输入、异常恢复与长稳 |
| Linux → Windows | [Wayland 正式窗口、90 光标、移动与故障隔离](remote-desktop-linux-cursor-product-2026-10-18.md)；[X11 正式窗口、记事本鼠标/标记输入、最小化及关闭释放、重开](remote-desktop-xorg-x11-minimize-2026-10-19.md) | 完整键位/输入法、动态 FPS、多屏与 Windows 登录；X11 最小化恢复仍待补 |
| Linux → macOS | [Wayland 正式窗口首帧、单击/A/Shift、关闭释放及协助闭环](remote-desktop-linux-to-macos-2026-10-17.md)；[X11 正式窗口、坐标/A/Shift+A/滚轮及关闭释放](remote-desktop-xorg-x11-minimize-2026-10-19.md) | 独立光标边界、动态 FPS、Mac 登录、完整输入与长稳；X11 窗口恢复 |
| Linux → Linux | 未验证；用户于 2026-10-19 确认本轮跳过 | 不作为本轮交付阻塞项，不记为通过 |

七个方向有部分正式窗口证据，不表示七个方向全部通过。特别是 `15 FPS` 配置值、
首帧截图、解码吞吐和实际动态呈现 FPS 分开计数。系统密码登录、管理员恢复解锁、
登录后的普通桌面三类结果也分开记录；不能用恢复脚本替代产品登录。

下一轮执行顺序与结束条件统一见[长期任务的后续安排](../REMOTE_DESKTOP_LONG_TERM_TASK.md#接下来必做的工作)。Windows 离线自动重连及最小化释放已有修复，Linux Wayland 登录和普通 Xorg/X11 基本输入已有证据。Apple 已有被控端开机登录证据直接保留，后续重点是操作端缺项、性能与最终安装链路。各方向测试产物分别记录，不合并为同一最终发布包。

本轮已补 Windows→Linux 普通 Xorg、Linux X11→Windows/Mac 基本输入及 Windows 最小化释放修复，随后补 Windows active/服务设置切换及 90 混合 DPI 基本坐标。原服务与专属环境已恢复清理。[客户端私有编译 TOML 与公开空配置](remote-desktop-client-defaults-2026-10-19.md) 已实现并完成 Windows 真实隔离启动；[编码码率反馈](remote-desktop-adaptive-bitrate-2026-10-19.md) 已接入，并补通[正式 helper 转发、90 实际限速及切屏延续](remote-desktop-helper-budget-weak-network-2026-10-19.md)。90 实际显示 14.85→4.99→14.70 FPS，限速时命令/PTY 通过；Linux 新原生预算循环 90/90 解码、13.36 FPS。Mac/Linux 新窗口、完整新 session 重连首帧及 RTT 仍待验收，不替代七方向最终候选。接着补上述验证、Apple 操作端、完整键位/输入法、X11 窗口恢复及 Mac/Linux active/通知，固定候选后做正式安装与稳定性。历史结果只在对应代码与条件未受影响时复用；Linux→Linux 仍按用户要求跳过。

较早 Windows → Linux 测试曾被窗口遮挡中断，记录保留在
[当轮验收](remote-desktop-window-capture-2026-10-18.md)。最新 58 轮约 29 分钟
中断由用户确认是调整机器造成，记为环境中断，不记作 30 分钟通过，见
[中断记录](remote-desktop-mixed-window-move-2026-10-19.md)。随后 60 秒混合及
关闭画面后原 PTY/命令/传输通过，不能替代持续运行。30 分钟与 100 次开关仍待完成；
2 小时、8 小时按用户要求暂缓到晚上，8 小时验收绑定发布前最终候选。
尚未具备独立双端条件的组合保留未验收，不把同机递归捕获当成跨机器操作验收。

发布前仍须按最终候选重新核对平台能力、安装依赖、TOML/身份保留、服务与桌面
自启动、覆盖升级和 COS 下载校验。阶段性测试安装目录和系统服务恢复记录不能
替代正式安装包的升级验收。本次重新核对既有证据后，长期清单为
**18/53 已关闭、32 部分完成、3 交付待验收/执行**，见[进度复核](remote-desktop-progress-reaudit-2026-10-19.md)。逐项状态见[长期任务](../REMOTE_DESKTOP_LONG_TERM_TASK.md)。
