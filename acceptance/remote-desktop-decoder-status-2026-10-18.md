# 实际解码器状态与定向延迟/限速验收

日期：2026-10-18。产品基线 `f02ff08`；本轮对应 W1/W3、03.1、06.4～06.6 的阶段改动，不关闭完整交付条目。

## 实现

窗口首帧确认后，从已有原生事件读取当前代次实际使用的解码器，加入本地 `ViewerStatus.decoder`。界面显示“硬件解码”或“软件解码”；连接、切屏、重连、结束时清除旧值。没有报告、未知类型或原生错误时不猜测实际路径，不拿请求参数、操作系统或 GPU 型号代替实际报告。

Windows 复用 D3D11/OpenH264 原生报告，Mac 复用首帧后的 VideoToolbox/OpenH264 报告，Linux 使用已有 OpenH264 报告。只读出当前代次状态，不增加网络协议字段、配置文件或认证流程，也不更改媒体 ACK 和输入语义。初始硬解失败后原生直接选软件，也会正确显示软件；运行中换代次由既有恢复流程处理。

复核 Windows 原生路径：在协商输入/输出类型前设置 DXGI manager，输出必须为同设备、NV12、带 `D3D11_BIND_DECODER` 的 GPU buffer，否则拒绝；不会在该路径内清空 manager 后静默软件解码。该判断依据实际原生输出与 [Microsoft 的 D3D11 协商/软件后备约定](https://learn.microsoft.com/en-us/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation)，并非仅看硬解请求参数；本轮没有采集 GPU 引擎利用率或 ETW。

可机读的脱敏汇总与原始记录哈希见 [验收数据](remote-desktop-decoder-status-2026-10-18.json)。

## 构建和自动检查

- Windows 媒体单测 **23 项**、Linux **25 项**、Apple **24 项**通过。新增检查覆盖三种原生路径、报告前未知、后续冲突报告拒绝、未报告类型的新代次不继承旧状态。
- 界面 **7 项** Playwright 检查通过，包含三种报告、未知值、恢复清除和 72px 工具栏尺寸；TypeScript/Vite 生产资源构建通过。
- 完整 Desktop：Windows 3 分 41 秒、Linux 2 分 09 秒、Apple 4 分 29 秒构建通过。每个 Cargo 构建 4 jobs、嵌套 native 1；Windows 与 Linux 完成并行阶段后才让 Apple 接替 Linux，编译并发峰值不超过 8。前端编译也限制到 4 个工作线程。
- Apple 此轮只构建/测试，不改已授权 App bundle、安装版、TCC 或登录会话。构建不等于 Mac 正式窗口验收。

| 完整 Desktop | SHA-256 |
| --- | --- |
| Windows | `129550c8a89a0375e251274682edc7ca3a445acedf251c3e34b01cf5de5e95a9` |
| Linux | `7a419ff9f5944d8be772f05562c377fb4649dd96f4faee0243773372ec32cbf7` |
| macOS | `6d41521a1432e78da4f7d1f22f8e2b5d345dba7d51bd48d4b6a52714ef15d6db` |

Linux Executor 未改，仍为 `3be28536c340748ec0f125d2f743da895e2e31efa2edfc221bf87635ffb1ec5c`。

## 正式窗口实际硬解与软件后备

Windows 隔离 Desktop 操作 Ubuntu 22.04 / GNOME 42.9 / VMware Wayland，源单屏 1314×704，自有动态色块夹具。正常首帧明确报告 `d3d11_hardware`，界面文字为 `Hardware decoding`。通过拥有的窗口按住 Ctrl 后，只终止该窗口自己的渲染子进程；同一个父窗口约 **2.157 秒**内自动恢复为新 session，报告 `openh264_software`，界面变成 `Software decoding`。恢复中旧类型清除，恢复后保持仅查看，远端 Ctrl 已释放。

两次生成区域截图的实际像素变化证明画面继续更新，没有只依赖 Ready 状态；不把此比较计为 FPS 或画质检查。正常 WM_CLOSE 后媒体进程退出，临时服务及七个本地配置/凭据/密钥文件清理，原服务与配置保留。驱动 **exit 0**。

原始记录：`.build/remote-desktop/formal-linux-login-53f4e7f8d7cf/product-decoder-status-result.json`，本地 profile `assistance-product-3c8de5f66665`。它补上了上一轮仅根据启动参数判断硬解优先策略的证据缺口；不追溯把上一轮没有采集的实际解码状态改成已通过。

## 150ms 单向延迟与 128kbit/s 限速

扩展 `experiments/remote-desktop/linux_owned_udp_fault.py --netem`。只匹配拥有的 Windows Desktop 的 IPv4 UDP 地址/端口；用 clsact/flower/mirred 重定向到本轮临时 IFB 的 netem 队列，保留 ens33 原根 fq_codel。队列上限 64 包，运行 20 秒，独立 45 秒回滚保护。原理和边界参考 iproute2 的 [mirred](https://github.com/iproute2/iproute2/blob/main/man/man8/tc-mirred.8)、[netem](https://github.com/iproute2/iproute2/blob/main/man/man8/tc-netem.8) 以及 Linux [IFB](https://github.com/torvalds/linux/blob/master/drivers/net/ifb.c) 实现；有定时粒度和整形误差，设置值不作为实际 RTT 测量。

最终一轮：

- 规则生效约 **20.094 秒**，匹配 **381 包**。内核队列报告 delay=0.15 秒、rate=16000 字节/秒、limit=64；第一至最后采样的已发送字节差/同机单调时间差为 **117.60kbit/s**。该流量包含 QUIC 开销，不是净视频码率。
- 20 个队列样本中最多 **12 包 / 10,852 字节**积压，drops=0。临时 IFB、过滤器和规则文件清理，原根 qdisc 前后相同。
- 30 个窗口/资源样本均为 viewing、同一 session、`d3d11_hardware`。受限期间和撤销后，生成区域像素变化通过；Ctrl 释放送达，最后无按住的键或鼠标。
- 只采集 Desktop 主进程的 RSS：**71,872,512～72,720,384 字节**。不含 WebView/原生子进程、服务、显存，时长很短，不能据此认定无泄漏或资源平台期。
- 正常关闭、原服务恢复、用户会话保持解锁、远端目录和本地私有文件清理均通过，最终驱动 **exit 0**。

原始记录：`.build/remote-desktop/formal-linux-login-f04cd427d9dd/product-netem-result.json`，本地 profile `assistance-product-0fc5a972f633`；汇总 `.build/remote-desktop/netem-decoder-summary.json`。本轮没有统计实际呈现 FPS，不能用仍显示 15 FPS 的目标选择框宣称限速下仍达 15 FPS；也没有验证自适应码率/编码尺寸。

## 保留的测试失败及恢复

1. `formal-linux-login-d94fb0b0c8d0`：驱动用 Windows 默认 GBK 读取 Node 的 Unicode 界面文本失败。改为显式 UTF-8；产品未修改，完整后续轮次通过。
2. `formal-linux-login-48047d40db0c`：新窗口尚未出现在 CDP 列表，首个只读状态检查失败；清理脚本格式串还有未转义的空 JSON。已改为有界只读轮询并修正转义，未重放打开窗口操作；确认原服务恢复后清理。
3. `formal-linux-login-8b0cd86c1a8a`：20 秒整形已执行，撤销过滤器并恢复根队列后，删除 IFB 的 alias 校验失败。实际创建命令未保留 alias，已通过 sysfs 和设备类型确认；没有放宽删除检查。人工核对唯一临时接口的 ifindex=5、ifb 类型与 netem 后删除；回滚 timer 此时已卸载，stop 返回 5。原服务和目录最终清理通过。原始队列证据保存在 `netem-cleanup-diagnosis.json`，这轮不记为完整通过。
4. 工具现分开创建接口与设置 alias，记录 ifindex，删除前同时检查接口编号、类型及 sysfs alias；最终 `f04cd427d9dd` 轮次自动恢复并退出成功。

## 继续工作

实际解码状态的 Mac/Linux 操作端窗口验收、实际显示 FPS/降档状态、双向 RTT、随机丢包、强制 Relay/路径切换、同连接混合业务与长时间资源测试仍待完成。此次三平台构建不替代三平台窗口实测，单一 20 秒网络场景不关闭 06.5/06.6 或 W3。
