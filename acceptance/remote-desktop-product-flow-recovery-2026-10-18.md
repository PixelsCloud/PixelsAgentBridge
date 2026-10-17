# 正式窗口停滞、静止、最小化和定向 UDP 断流

日期：2026-10-18。产品基线 `974a8a4`，对应 W1/W3/W5、03.6、04.4、06.2/06.5 的阶段验收。完整条目仍未关闭。

## 环境和边界

Windows 隔离 Desktop，经正常账号/设备认证打开 Ubuntu 22.04、GNOME 42.9、VMware Wayland 单屏 1314×704 的正式独立窗口。Linux 仅暂时使用 runtime service override 和隔离 helper；未修改本机安装版或远端安装文件。自有 GTK 夹具显示动态色块，只统计 Ctrl 和鼠标左键的状态，不采集输入正文。测试未点击远端其他应用、锁屏或修改显示模式。

完整 Windows Desktop 构建通过，4 jobs、嵌套 native 1 job，无其他编译并行，耗时 5 分 40 秒。产物 SHA-256：

- Windows Desktop：`35237c633488175fa55f18e094b9230725378de5d6428f37bce38e1817dc5b21`
- Linux Desktop：`6fe68abc63db46c69180c9eab20917172f286743dbff886d1e667f5869dcda7a`
- Linux Executor：`3be28536c340748ec0f125d2f743da895e2e31efa2edfc221bf87635ffb1ec5c`

这是正式窗口行为验收，不是 FPS、颜色精度、九种组合或长期资源曲线验收。画面恢复除了 Ready 状态，还比较固定生成区域的两次截图，均有 233,639 个像素的通道变化超过 5；截图仅本地保存。没有把这种像素变化换算为显示帧数。

## 渲染停滞及窗口行为

只暂停拥有的隔离 Desktop 的原生渲染子进程，校验父 PID、可执行路径和 render 参数；设独立定时恢复保护，不暂停整个 Desktop、系统或安装版。

1. 短暂停顿约 0.8 秒后恢复进程，保持原视频 session，动态画面继续变化。
2. 通过自有原生窗口输入通道在夹具按住 Ctrl，再暂停渲染进程。约 **6.246 秒**后，同一个父窗口自动打开新 session，原渲染进程已退出，回到只查看。夹具 Ctrl 计数从 down=1/up=0 变为 down=1/up=1，无按住状态、没有输入重放。
3. 这一轮从 `stream-interactive` 转到 `stream-interactive-software`，属于渲染器故障后的软件后备，**不能据此证明网络故障保持硬解或服务端积压超时是唯一恢复原因**。前者只代表硬解优先策略，也可能在原生初始化时自动后备；不能只根据参数判断实际使用的解码器。
4. 重新启用操作，Ctrl 按下/释放再次实际送达；随后关闭操作并恢复仅查看。
5. 在恢复的软件解码路径暂停生成动画，观察约 **9.08 秒**，夹具 ticks 1430→1430，选定静态区域像素一致；视频 session 不变。恢复动画后画面继续更新。
6. 在该软件解码路径原生最小化 **8 秒**，仍保持同一 session；恢复后动态画面正常。未将最小化期间的消费计为显示 FPS。
7. 正常 WM_CLOSE 后，本地该窗口媒体进程数归零；再次由设备入口打开产生新 session，默认只查看且实际画面变化，再次正常关闭。

完整产品断言记录：`.build/remote-desktop/formal-linux-login-2d02ec9604db/product-backlog-result.json`；本地 profile 为 `assistance-product-10fecf5e5cb8`。最终驱动退出码为 1，原因是测试复制配置时打开的 SQLite 句柄未关闭，阻止删除临时数据库，不是上述产品断言失败。驱动退出后已检查无测试进程并删除五个余下私有文件，见该 profile 的 `final-cleanup.json`。这轮不能标为整个测试脚本 exit 0。

## 实际网络断流

使用可复用的 `experiments/remote-desktop/linux_owned_udp_fault.py`，先确认当前 Linux 服务确为本轮暂存 Executor，再核对本地隔离 Desktop 的 IPv4 UDP 端口。只匹配 `192.168.159.1:63259` 的出站 UDP；未改根 `fq_codel`，未阻断 SSH、控制服务或其他端点。过滤器的字段和动作参考 iproute2 的 [flower](https://github.com/iproute2/iproute2/blob/main/man/man8/tc-flower.8) 和 [gact](https://github.com/iproute2/iproute2/blob/main/man/man8/tc-gact.8) 文档。设置独立 20 秒回滚 timer，正常路径七秒后删除自身规则。

- 实际过滤时长 **7.004 秒**。tc action 统计为 **44 个匹配包、31 次 drops**；分别保留原始计数，不混称 44 次丢包。
- 过滤仍生效时的 SSH 探针确认 Ctrl 已释放（down=1/up=1），夹具仍在更新，没有鼠标按住状态。
- 另一 MCP 端点经原有连接执行 `/usr/bin/true` 成功。这证明其他端点仍可工作；不等同于同一受损 QUIC 连接上的命令/文件混合压力测试。
- 恢复后，同一父窗口自动进入新 session，保持只查看；未点击重试，原生渲染仍为 `stream-interactive`，没有切换强制软件策略。该参数只证明硬解优先，未采集本轮原生实际解码器的状态，不能声明硬解实测。动态区域像素比较通过，没有 Ctrl 重放。
- 本地观察约 **10.99 秒**完成恢复确认，计时包括规则启动、并行命令和状态轮询，不能减去七秒后声称为精确恢复延迟。
- 正常关闭后无本地媒体子进程；临时规则、文件、服务、输入设备和七个复制的本地配置/凭据/密钥文件均清理，原服务恢复 active，原配置及 unit/程序哈希保持，用户会话仍解锁。根 qdisc 前后 JSON 一致。

本轮驱动 **exit 0**。原始记录：`.build/remote-desktop/formal-linux-login-b0b02807837c/product-backlog-result.json`；本地 profile 为 `assistance-product-bb967d555bc4`。当前实现同时包含输入租约/心跳与视频积压回收，单次产品结果不能把 Ctrl 释放唯一归因于某一个定时器。服务端积压的隔离因果另见上一轮真实 QUIC 测试。

## 测试驱动修正和保留失败

- 第一次驱动误将 arm 的两行 JSON 当成一个对象，测试未进入窗口；恢复时又遇到 watch 正在退出的清理检查。确认原服务恢复、watch inactive 后才清理。记录 `formal-linux-login-ee489f56e764`。
- 下一轮已经完成部分产品检查，但关闭时进程枚举恰好遇到子进程退出。驱动改为容忍 `NoSuchProcess`，没有修改产品。记录 `formal-linux-login-be119e679b87`。
- SQLite 复制用的 Python connection 上下文只结束事务，没有关闭句柄，导致临时文件清理失败。驱动现显式关闭自己的复制句柄，并用独立 finally 保证远端恢复不会被本地清理异常跳过；最终 UDP 轮次已完整清理并 exit 0。
- 初版统计字段把 action `packets` 命名为 dropped_packets；已改为 matched_packets 与 reported_drops，并保留原 tc 统计及修正说明。

## 未覆盖

持续限速、高 RTT、随机丢包、反向断流、强制 Relay/路径切换、同连接混合终端与文件传输、长时间资源曲线以及其他平台窗口尚未通过本轮。真实网络结果没有证明持续降码率或分辨率功能完成；03.6、04.4、06.1～06.6 与完整 W3 均保留未关闭。
