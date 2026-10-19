# 九种平台组合的证据与剩余验收

范围更新（2026-10-19）：用户确认暂时没有额外 Mac/Linux 桌面，明确本轮无需验证
Mac→Mac、Linux→Linux。两项保留“未验证”事实，移出本轮必验与交付阻塞范围；
其余七个方向继续按实际证据验收。下方 2026-10-18 的记录为历史基线，不表示全部通过。

核对日期：2026-10-18；源码基线 `a4a58d8`。本表整理既有实机记录，不是九种
组合的新候选重测，也不关闭 LT-07.6。各记录的产物哈希分别有效，不能混成一份
已经通过全部功能的发布包。Apple 按用户确认的单屏条件验收。

| 操作端 → 被控端 | 已有正式产品窗口证据 | 仍缺的主要证据 |
| --- | --- | --- |
| Windows → Windows | [90 双屏切换、点击与热拔恢复](remote-desktop-windows-dual-viewer-2026-10-18.md)；[锁屏密码登录](remote-desktop-lock-login-2026-10-17.md) | 当前候选的旋转/混合 DPI/负原点完整组合、WGC 光标后备、完整键盘与长期负载；首次线程激活的点击重复边界仍有未关闭记录 |
| Windows → macOS | [Apple 单屏输入、独立光标、协助占用及恢复](remote-desktop-macos-activity-input-assistance-2026-10-18.md) | 当前候选动态有效 FPS、系统锁屏/登录、完整键盘与长稳；单屏不宣称多屏通过 |
| Windows → Linux | [原生窗口、输入及网络恢复](remote-desktop-product-flow-recovery-2026-10-18.md)；[Xorg GDM 密码登录](remote-desktop-linux-system-login-2026-10-17.md)；[Wayland 锁屏密码解锁](remote-desktop-linux-wayland-window-login-2026-10-17.md)；[光标与唤醒恢复](remote-desktop-linux-wake-recovery-2026-10-18.md) | 当前候选混合长稳；较新修复后的完整密码登录/注销回归；普通 Xorg 完整产品验证、复杂多屏与输入法 |
| macOS → Windows | [Apple 正式窗口接收 90 光标、移动输入、只查看与故障隔离](remote-desktop-macos-cursor-product-2026-10-18.md) | 新候选完整键盘/鼠标、动态 FPS、多屏、Windows 系统登录与长期负载 |
| macOS → macOS | 未验证；用户于 2026-10-19 确认本轮跳过 | 不作为本轮交付阻塞项，不记为通过 |
| macOS → Linux | [正式窗口首帧、实际点击/Ctrl、关闭释放及协助闭环](remote-desktop-macos-to-linux-2026-10-17.md) | 当前候选动态 FPS、独立光标、系统登录、完整输入、异常恢复与长稳 |
| Linux → Windows | [Ubuntu GNOME Wayland 正式窗口、90 光标、移动输入与故障隔离](remote-desktop-linux-cursor-product-2026-10-18.md) | 新候选完整鼠标/键盘、动态 FPS、多屏与 Windows 登录；另测 X11 操作端 |
| Linux → macOS | [Wayland 正式窗口首帧、单击/A/Shift、关闭释放及协助闭环](remote-desktop-linux-to-macos-2026-10-17.md) | 当前候选独立光标、动态 FPS、Mac 登录、完整输入与长稳；另测 X11 操作端 |
| Linux → Linux | 未验证；用户于 2026-10-19 确认本轮跳过 | 不作为本轮交付阻塞项，不记为通过 |

七个方向有部分正式窗口证据，不表示七个方向全部通过。特别是 `15 FPS` 配置值、
首帧截图、解码吞吐和实际动态呈现 FPS 分开计数。系统密码登录、管理员恢复解锁、
登录后的普通桌面三类结果也分开记录；不能用恢复脚本替代产品登录。

本轮 Windows → Linux 混合测试被本机窗口遮挡中断，采样修正后短测通过，见
[独立记录](remote-desktop-window-capture-2026-10-18.md)；没有将短测计为长稳。
该路径稳定后补 2 小时及现有业务回归，再补齐矩阵；8 小时验收绑定发布前最终候选。
尚未具备独立双端条件的组合保留未验收，不把同机递归捕获当成跨机器操作验收。

发布前仍须按最终候选重新核对平台能力、安装依赖、TOML/身份保留、服务与桌面
自启动、覆盖升级和 COS 下载校验。阶段性测试安装目录和系统服务恢复记录不能
替代正式安装包的升级验收。完整长期清单保持 11/53。
