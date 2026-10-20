# 动态布局既有证据复核

日期：2026-10-20；复核检查点 `8881a9c`。这是原始结果核对，没有启动新测试、注销虚拟机或变更配置。[结构化引用](remote-desktop-wayland-layout-evidence-reaudit-2026-10-20.json)。

上一批把 Wayland 动态布局整体列为缺项不准确。10 月 17 日已完成以下真实测试，不能因为验收记录较早而重新归为未做：

| 条件 | 已有原始证据 | 范围 |
| --- | --- | --- |
| 采集中的分辨率变化 | [采集记录](remote-desktop-linux-capture-2026-10-17.json)的 `lifecycle`：`layout_change_retires_old_capture_generation`、恢复原模式、`new_capture_after_restoration` 均通过 | 生产 GNOME/PipeWire worker；测试脚本哈希与当前文件一致 |
| 按住 Ctrl/按钮时变更布局 | [输入记录](remote-desktop-linux-input-2026-10-17.json)原生和内嵌 Desktop 两次 `idle_layout_change_releases_state_and_reopens` 通过 | GTK 自有窗口，真实切到 1920×1080，旧 worker 退出，恢复后输入不带旧修饰键/按钮 |
| supervised uinput 后备 | [后备记录](remote-desktop-linux-uinput-login-2026-10-17.md)同最终产物 11 项输入包含布局变化 | 与 GNOME 客户端虚拟设备路径分别记账 |

源码核对仍保留 MonitorsChanged 订阅、启动布局核对、空闲输入有效性检查，以及采集编码结束后的代次检查；本批 Mac 修改不触及它们。复核没有把这些原生结果改写为新做的 Wayland 正式窗口模式切换。

合并上述后端证据、[Windows 动态 DPI/分辨率及多屏交接](remote-desktop-dynamic-dpi-layout-2026-10-20.md)、[本批 Mac 产品与 X11 原生布局](remote-desktop-macos-linux-layout-boundary-2026-10-20.md)，以及既有共享 session/generation 旧输入/旧帧隔离和首帧门控结果，**关闭原条目 03.4**。使用新 session 恢复时 generation 可从 1 开始，身份以二元标识判断。

03.4 的关闭不代表所有平台/硬件的排列组合或最终候选包已经验收；操作端尚缺组合继续归 07.x，安装和固定候选回归归 09.x，不在本条重复挂账。03.5 光标能力缺口仍保留。当前为 **25 已关闭 + 25 部分完成 + 3 交付待验收/执行 = 53**。

下一步直接推进 Mac 实际非零预算及恢复；不重做上述 Wayland 条件。
