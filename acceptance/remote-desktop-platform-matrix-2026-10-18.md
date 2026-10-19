# 平台组合的证据与剩余验收（七个必验方向）

范围更新（2026-10-19）：用户确认暂时没有额外 Mac/Linux 桌面，明确本轮无需验证
Mac→Mac、Linux→Linux。两项保留“未验证”事实，移出本轮必验与交付阻塞范围；
其余七个方向继续按实际证据验收。Apple 按单屏处理，多屏不作为本轮阻塞。

最新核对：2026-10-19，记录核对至 `211c44b`，包含 Apple 真实重启登录与自动恢复（`8e41278`）；原表基线为 `a4a58d8`。
本轮没有关闭 LT-07.6。各记录的产物哈希分别
有效，不能混成一份已经通过全部功能的发布包。被控端登录通过不能计为反向操作端通过。

| 操作端 → 被控端 | 已有正式产品窗口证据 | 仍缺的主要证据 |
| --- | --- | --- |
| Windows → Windows | [90 双屏切换、点击与热拔恢复](remote-desktop-windows-dual-viewer-2026-10-18.md)；[锁屏密码登录](remote-desktop-lock-login-2026-10-17.md)；[统一产物负原点竖屏、缩放点击、WGC 光标和采集代理恢复](remote-desktop-windows-portrait-matched-2026-10-19.md) | 混合 DPI、普通非管理员、完整输入及首次激活重复消息边界；开机/UAC/用户切换、最终候选动态基准与长稳 |
| Windows → macOS | [Apple 单屏输入、独立光标、协助占用及恢复](remote-desktop-macos-activity-input-assistance-2026-10-18.md)；[锁屏密码登录](remote-desktop-macos-lock-login-2026-10-19.md)；[注销后 LoginWindow 密码登录和画面恢复](remote-desktop-macos-logout-login-2026-10-19.md)；[重复注销、真实重启密码登录及自动恢复，未再次授权](remote-desktop-macos-login-startup-followup-2026-10-19.md) | 最终候选动态有效 FPS、完整输入与长稳；正式安装 helper 链路。重启结果来自隔离候选包，不代替安装升级。单屏不宣称多屏通过 |
| Windows → Linux | [原生窗口、输入及网络恢复](remote-desktop-product-flow-recovery-2026-10-18.md)；[Xorg GDM 密码登录](remote-desktop-linux-system-login-2026-10-17.md)；[既有 Wayland 注销登录自动恢复](remote-desktop-login-recovery-2026-10-17.md)；[新候选锁屏密码登录、双 Desktop 接手、AI 完成、60 秒混合及关闭后原业务](remote-desktop-login-occupancy-assistance-2026-10-19.md) | 新候选注销/开机与反复会话切换；普通 Xorg 完整产品、复杂多屏和输入法；30 分钟混合及长期资源验证 |
| macOS → Windows | [Apple 正式窗口接收 90 光标、移动输入、只查看与故障隔离](remote-desktop-macos-cursor-product-2026-10-18.md) | 新候选完整键盘/鼠标、动态 FPS、多屏、Windows 系统登录与长期负载 |
| macOS → macOS | 未验证；用户于 2026-10-19 确认本轮跳过 | 不作为本轮交付阻塞项，不记为通过 |
| macOS → Linux | [正式窗口首帧、实际点击/Ctrl、关闭释放及协助闭环](remote-desktop-macos-to-linux-2026-10-17.md) | 当前候选动态 FPS、独立光标、系统登录、完整输入、异常恢复与长稳 |
| Linux → Windows | [Ubuntu GNOME Wayland 正式窗口、90 光标、移动输入与故障隔离](remote-desktop-linux-cursor-product-2026-10-18.md) | 新候选完整鼠标/键盘、动态 FPS、多屏与 Windows 登录；另测 X11 操作端 |
| Linux → macOS | [Wayland 正式窗口首帧、单击/A/Shift、关闭释放及协助闭环](remote-desktop-linux-to-macos-2026-10-17.md) | 当前候选独立光标、动态 FPS、Mac 登录、完整输入与长稳；另测 X11 操作端 |
| Linux → Linux | 未验证；用户于 2026-10-19 确认本轮跳过 | 不作为本轮交付阻塞项，不记为通过 |

七个方向有部分正式窗口证据，不表示七个方向全部通过。特别是 `15 FPS` 配置值、
首帧截图、解码吞吐和实际动态呈现 FPS 分开计数。系统密码登录、管理员恢复解锁、
登录后的普通桌面三类结果也分开记录；不能用恢复脚本替代产品登录。

下一轮执行顺序与结束条件统一见[长期任务的后续安排](../REMOTE_DESKTOP_LONG_TERM_TASK.md#接下来必做的工作)。Windows UAC 先解决 90 当前未启用安全桌面提示的环境条件；Apple 已有开机登录证据直接保留，后续重点是操作端缺项、性能与最终安装链路。本次仅更新文档，未重新运行平台组合。

较早 Windows → Linux 测试曾被窗口遮挡中断，记录保留在
[当轮验收](remote-desktop-window-capture-2026-10-18.md)。最新 58 轮约 29 分钟
中断由用户确认是调整机器造成，记为环境中断，不记作 30 分钟通过，见
[中断记录](remote-desktop-mixed-window-move-2026-10-19.md)。随后 60 秒混合及
关闭画面后原 PTY/命令/传输通过，不能替代持续运行。30 分钟与 100 次开关仍待完成；
2 小时、8 小时按用户要求暂缓到晚上，8 小时验收绑定发布前最终候选。
尚未具备独立双端条件的组合保留未验收，不把同机递归捕获当成跨机器操作验收。

发布前仍须按最终候选重新核对平台能力、安装依赖、TOML/身份保留、服务与桌面
自启动、覆盖升级和 COS 下载校验。阶段性测试安装目录和系统服务恢复记录不能
替代正式安装包的升级验收。完整长期清单为 **12/53**，其余为未完整关闭；
逐项状态见[长期任务](../REMOTE_DESKTOP_LONG_TERM_TASK.md)。
