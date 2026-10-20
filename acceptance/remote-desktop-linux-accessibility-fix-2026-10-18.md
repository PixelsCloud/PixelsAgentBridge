# Linux 已销毁按钮的无障碍状态兼容修复

日期：2026-10-18。分支基线 `a8862a3`。接续
[故障定位记录](remote-desktop-linux-accessibility-2026-10-18.md)。

## 修改及适用范围

Linux Desktop 在 GUI 主线程、创建 GTK 窗口前检测
`GtkButtonAccessible::ref_state_set` 对已无 widget 的对象是否返回空值。
受影响的运行库使用进程内兼容回调：正常按钮仍调用原函数；已销毁按钮返回包含
`ATK_STATE_DEFUNCT` 的状态集合，供 AT-SPI 安全查询。运行库已有非空状态时跳过。
保留一个类引用至进程退出，避免保存的回调所属类被卸载；重复初始化不重复替换。

这是单一公开虚方法的兼容处理，范围仅限该按钮实现。没有修改系统 GTK、禁用
无障碍功能、屏蔽日志或改变输入逻辑，也没有将所有 GTK 子类视为已修复。
新入口的 Rust API 明确要求 GUI 主线程及创建控件前调用；媒体 helper 不调用它。

此前考虑调整标题栏初始化顺序。实际 Desktop GDB 跟踪捕获五次按钮创建、三次
销毁，销毁路径经过 `gtk_header_bar_set_decoration_layout`；但独立对照中，无论
先后设置 resizable，后续标题栏更新仍留下 widget/state 都为空的旧按钮对象。
因此没有修改 Tao 或升级依赖来规避此问题。

## 验证

Ubuntu 22.04 / GTK 3.24.33 / ATK 2.36.0 / AT-SPI bridge 2.38.0，原 Wayland 会话。
探针使用 `G_DEBUG=fatal-criticals` 与仅查询自有 PID 的无障碍遍历器。

| 检查 | 结果 |
| --- | --- |
| 未启用兼容处理的 retired-button 负向对照 | 4 次查询期间复现原 ATK critical / SIGTRAP |
| 同一二进制 guard-retired-button | 13 次查询，无该告警，自然退出 |
| guard-header-late 标题栏更新 | 12 次查询；三个已销毁按钮均返回非空状态；自然退出 |
| guard-host 原生 Presenter 嵌入 | 12 次查询，无该告警，自然退出 |
| 正常按钮 enabled/disabled/enabled | 状态集合与原回调逐项相同，原动作接口保留 |
| 销毁按钮、释放最后引用 | DEFUNCT 状态、动作拒绝执行、对象完成析构均通过 |
| 重复初始化 | 结果一致，通过 |
| 完整 Linux Release Desktop / 媒体单测 | 构建成功；25 项通过，0 失败 |

状态契约测试第一次错误地把“相同集合 XOR 返回 NULL”传入空集合检查，触发测试
自己的 critical；修正测试后通过。原失败记录保留，不能算作产品缺陷或通过。
初始化顺序对照中另有一次查询对象已消失，保留为查询失败。

新 Desktop SHA-256：
`1727fc537ff5d721057e17bf7f4f21681efad8378e973b8694c086576e88dd6f`。
完整构建 Cargo 并发 4；另一个独立 C++ 检查最多使用 2 CPU，总量未超过 10。

实际 Desktop 首轮创建五个、销毁三个标题栏按钮，63 次查询尝试未出现 ATK
critical。其中六次查询碰到已经退出的短暂子进程，记录为失败，不能称为 63 次全部
成功。该轮没有成功创建 AI 协助请求，因此仅证明启动窗口路径；诊断 25 秒后主动
结束进程，不能计作自然退出或窗口关闭验收。临时配置已清理，会话保持。

第二轮使用既有 Apple 凭据的隔离副本，通过原认证创建协助请求。结束 Desktop
前检查到 `status=waiting`、`delivery=displayed`、`window_label=null`，确认提示
已显示，未打开远程 viewer 或发送输入。60 次查询尝试中，四次遇到已退出的短暂
子进程；其余查询成功，未出现 ATK critical。标题栏仍执行五次创建、三次销毁。
请求随后取消，隔离进程结束，远端临时副本删除，原用户会话保持。

该轮准备阶段曾因 SQLite WAL 拒绝直接复制，后续快照又因测试脚本未关闭连接而
无法立即删除。两次均未启动待测 Desktop，均清理了远端暂存目录；脚本改用一致
快照并显式关闭连接后才完成本轮。新增本地快照已删除，已有私有资料和安装版保留。

## 可复现入口及后续边界

[原生状态契约检查](../crates/desktop-media/tests/native_linux_accessibility.cpp)
在 Linux 图形会话执行：

```sh
c++ -std=c++17 -Wall -Wextra -Werror \
  crates/desktop-media/tests/native_linux_accessibility.cpp \
  $(pkg-config --cflags --libs gtk+-3.0) -o /tmp/pab-atk-state-check
G_DEBUG=fatal-criticals /tmp/pab-atk-state-check check
```

[诊断探针](../experiments/remote-desktop/linux_accessibility_probe.cpp) 支持
`guard-retired-button`、`guard-header-late`、`guard-host` 对照；编译与遍历命令见
前一份定位记录。源码、测试二进制和私有原始日志摘要见
[结构化记录](remote-desktop-linux-accessibility-fix-2026-10-18.json)。

本修复不代表 VMware 图形驱动诊断、完整 Linux 登录、多屏或长期稳定性已验收。
总清单仍为 10 条完整关闭、43 条待收口。
