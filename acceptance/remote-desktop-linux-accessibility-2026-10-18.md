# Linux 无障碍告警定位

日期：2026-10-18。工作分支基线 `e5b9a2b`。本轮定位发布清单中的 GTK/ATK
critical 告警，未修改产品认证、输入、原生画面嵌入或安装版。

## 已确认的故障机制

正式 Desktop 在 AT-SPI 缓存遍历期间，对一个仍被引用的 `GtkButtonAccessible`
读取状态。GDB 确认该对象对应的 widget 已为 NULL，返回的状态集合也是 NULL。
AT-SPI bridge 随后调用 `atk_state_set_contains_state`，触发原日志中的断言。

正式产品的四轮隔离复现均抓到同一路径；最后一轮同时确认对象类型、角色 42
和空 widget。协助请求仍为 waiting、window_label 为 null，没有启用远端输入。
这证明该告警可以在进入远程画面前触发，不能据此修改视频重挂载或鼠标语义。

独立 GTK/WebKit 程序中的 retired-button 模式也复现相同调用栈：创建自有按钮、
保留无障碍对象引用、两秒后销毁按钮；外部客户端继续查询此进程的无障碍树。
该模式未构造 Presenter，没有远程连接、采集或输入。第一次同模式仅观测到
空状态、没有告警；随后使用已纳入仓库的只读遍历器触发告警。保留两轮结果，
不声称故障每次必现。

GTK 3.24.33 的按钮无障碍实现，在关联 widget 已销毁时返回 NULL；父类通用
widget 实现则可返回 DEFUNCT 状态集合。AT-SPI 的缓存序列化路径使用返回值
进行状态检查与释放。源码与实测一致，但尚未定位正式窗口中具体哪次按钮销毁
使其仍留在缓存，也尚未验证产品规避方案。参见
[GTK 按钮实现](https://github.com/GNOME/gtk/blob/3.24.33/gtk/a11y/gtkbuttonaccessible.c)、
[GTK 通用 widget 实现](https://github.com/GNOME/gtk/blob/3.24.33/gtk/a11y/gtkwidgetaccessible.c)、
[AT-SPI 缓存路径参考](https://github.com/GNOME/at-spi2-core/blob/AT_SPI2_CORE_2_46_0/atk-adaptor/adaptors/cache-adaptor.c)。
最后一个源码链接为后续版本的相同路径参考；实机包版本及反汇编确认见下文，
不能把该链接版本说成实机版本。

## 对照与边界

| 对照 | 实际结果 |
| --- | --- |
| 普通 GTK/WebKit，Wayland / XWayland | 状态树查询均正常，进程正常退出 |
| 加入正式 Presenter，Wayland / XWayland | 状态树查询均正常，进程正常退出 |
| 额外读取控件位置，Wayland 两种窗口 | 未复现 ATK 告警 |
| 每 500ms 挂载/移除 Presenter | 8 次创建；一次查询对象在遍历期间消失，记录为查询错误；未出现 ATK critical，进程正常退出 |
| 隔离正式 Desktop 空白账号页 | 查询期间未出现该告警；25 秒后诊断主动发送 SIGTERM，不能算程序自然退出 |
| 现有账号副本及 AI 协助提示 | 四次复现 AT-SPI 缓存中的空状态集合；最后两次确认 GtkButtonAccessible |
| 自有按钮销毁并保留无障碍引用 | 确认 widget/state 均为空；使用仓库遍历器的一轮复现相同告警 |

诊断设置 `G_DEBUG=fatal-criticals`，使告警触发 SIGTRAP 以保存调用栈。
因此诊断轮次的进程中止不等于正常环境下必然崩溃；原产品轮次在打印告警后
窗口仍可工作。挂载对照仍出现此前记录的 VMware `svga_surface_destroy`
驱动诊断，此问题与 ATK 告警分别记录，没有将其忽略为全部通过。

环境：Ubuntu 22.04，GTK `3.24.33-1ubuntu2.2`、ATK `2.36.0-3build1`、
AT-SPI bridge `2.38.0-3`、WebKitGTK `2.50.4-0ubuntu0.22.04.1`。
正式 Desktop SHA-256：
`6294fb4bfd985a16248eb5ed2da28b1d6d4d2c93a065ca7c9072feb95164754d`。
逐轮日志哈希、调用栈摘要和二进制身份见[结构化记录](remote-desktop-linux-accessibility-2026-10-18.json)。

## 复现工具

- [linux_accessibility_probe.cpp](../experiments/remote-desktop/linux_accessibility_probe.cpp)：
  `baseline`、`host`、`cycle`、`retired-button` 四种模式，每次窗口最多运行八秒
  （加载页面另有十五秒上限）。不采屏，不发送键鼠。
- [linux_accessibility_walk.py](../experiments/remote-desktop/linux_accessibility_walk.py)：
  显式指定 PID 和可执行文件，确认属于当前用户后只读取该进程的无障碍树。
  不读取控件名称、文本、密码或值，不调用控件动作。

在 Linux 的图形会话中，使用 GTK3/WebKitGTK4.1 开发依赖编译诊断程序。
诊断编译一次一个编译器进程，本轮容器 CPU 上限 2，无并发编译。

```sh
c++ -std=c++17 -O1 -g -Wall -Wextra -Werror \
  experiments/remote-desktop/linux_accessibility_probe.cpp \
  $(pkg-config --cflags --libs gtk+-3.0 epoxy webkit2gtk-4.1) \
  -o /tmp/pab-linux-accessibility-probe
G_DEBUG=fatal-criticals gdb --args /tmp/pab-linux-accessibility-probe retired-button
# 在同一图形会话的另一个终端中，使用探针 ready 输出的 PID：
python3 experiments/remote-desktop/linux_accessibility_walk.py PID \
  --exe /tmp/pab-linux-accessibility-probe --extents
```

四轮正式产品测试均取消了协助请求、结束隔离进程、删除临时配置与凭据副本。
未替换安装服务、未改变权限或执行注销。后续环境检查发现 Docker 未运行、
SSH 不可达；Docker 已恢复，VMware Guest Tools 确认虚拟机运行、IP 未变、SSH
服务监听正常。继续使用 Guest Tools 完成诊断，不调整主机防火墙或网络路由。

恢复 Docker 后，最终诊断源码单任务重新编译，并通过 Guest Tools 在原虚拟机
复测：baseline、host 各完成 12 次树查询并正常退出；retired-button 再次触发
相同 ATK critical。暂存目录删除、会话保持。最终诊断二进制 SHA-256：
`5a6f486794fc242f8b27ffdfb6700b11a4371f1400b88e8c6f8ade6da872f22e`。
先前 Docker/SSH 失败保留为环境失败，未当作测试通过。

本项状态：**故障机制已定位，产品处置仍待完成**。不关闭整个 Linux、窗口
生命周期或发布阶段；清单仍为 10 项完整关闭、43 项待收口。

后续进展：产品兼容修复及正式包启动/协助提示回归已完成，见
[后续修复验收](remote-desktop-linux-accessibility-fix-2026-10-18.md)。上文保留原定位阶段结论。
