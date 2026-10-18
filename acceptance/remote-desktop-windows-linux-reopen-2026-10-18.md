# Windows → Linux 正式观看窗口连续开关 100 次

日期：2026-10-18，基线 `dc99b27`。本轮完成 W7 的一个平台组合开关检查，
不关闭完整 LT-06.7 / LT-07.6；整体完整关闭数仍为 10/53。
逐轮数据、源文件和候选哈希见[机器记录](remote-desktop-windows-linux-reopen-2026-10-18.json)。

## 结果

同一个 Windows Desktop 进程，沿用既有认证和传输，连续创建和关闭 **100 个**
正式独立 viewer；100 个不同视频 session ID。每轮均：

1. 从主窗口调用正式打开入口，等待 WebView、Tauri bridge、`viewing` 和实际首帧。
2. 确认默认只查看，实际解码报告为 `d3d11_hardware`。
3. 将自有窗口调整到 720×600，读取原生显示区域三个背景点；与生成色块
   `[20,82,74]` 的每通道差不超过 3。先用 `WindowFromPoint` 验证点未被其他
   本机窗口覆盖；不是只检查网络收包或状态字符串。
4. 在 Linux 实际观察到本轮 `capture` worker，然后向自有 viewer 发送一次
   WM_CLOSE。等待本机窗口和 WebView 消失、原生渲染进程退出、Linux 媒体 worker
   清零后才打开下一轮。没有重放失败的打开/关闭动作。
5. 确认远端夹具的点击、按键及按住状态仍为零，保存双方资源样本。

100 次共约 **203.52 秒**，单轮约 1.77～3.89 秒（包含 SSH 观察和像素采样，
不是连接延迟测量）。没有占用冲突、旧会话复用或遗留原生媒体进程。
没有新增鉴权、传输或产品逻辑；本轮无需重新编译。

## 资源观察及限制

| 关闭后观察对象 | 首轮 | 第 100 轮 | 本轮范围 |
| --- | ---: | ---: | --- |
| Windows Desktop 句柄 | 527 | 527 | 525～530 |
| Windows Desktop 私有内存 | 17.45 MiB | 21.17 MiB | 16.86～22.91 MiB |
| Linux Executor RSS | 61,996 KiB | 64,012 KiB | 增加约 1.97 MiB |
| Linux helper RSS | 53,088 KiB | 53,088 KiB | 53,088～53,344 KiB |
| Linux helper fd | 11 | 11 | 11～14 |
| Linux supervisor fd | 15 | 15 | 始终 15 |

Supervisor、helper、Executor 在整个有效轮次中各自 PID 不变。Windows 末十轮
私有内存均值约 21.18 MiB，首十轮约 17.52 MiB；不能据此宣称长期没有泄漏。
该增长须在后续长稳中继续跟踪，必要时做堆分析，不以句柄稳定替代内存验收。
本轮没有完整采样 WebView2 进程树的内存、GPU 资源或 CPU 曲线。

Linux 为现有单屏 GNOME Wayland 会话 2，1694×1102。15 FPS 是配置值，
本轮只验证每次出图，未测连续动态 FPS；没有测试输入开关、锁屏、网络故障、
其他平台组合、30 分钟混合业务、2 小时观看或 8 小时稳定性。Windows 候选为
既有 WGC 光标协调版本，Linux 候选包含 GTK 无障碍修复；28 个 Linux native
源文件与构建快照逐字节一致。没有把本轮通过等同于所有平台最终候选通过。

## 测试驱动修正

保留两个失败记录：第一次第三个采样点被本机另一个置顶窗口覆盖；未操作那个
窗口，改为调整自有 viewer 并增加采样点归属检查。第二次完成 13 轮后，驱动在
第 14 轮过早调用尚未加载的 Tauri bridge。已增加空 bridge / 未创建页面的
只读等待，并等待旧 WebView 完全销毁；从零重新计数后的 100 轮全部通过。
没有放宽颜色门槛，没有把失败轮次混入有效 100 次。

## 恢复

远端测试使用临时 systemd 覆盖及独立 22 分钟回滚计时器，主动恢复后原安装
Executor PID 为 25566，unit、二进制和客户 TOML 哈希保持；原会话 2 仍为
active/unlocked。原安装版 Desktop 未替换或终止。临时覆盖、任务、目录、
媒体进程和输入设备已清理，回滚计时器未触发。

本机隔离 Desktop、MCP、该 profile 的 WebView2 子进程均已退出，39100 监听
已关闭；安装版未更新。忽略目录中的私有凭据 profile 保留原 ACL，未提交到 Git。
没有发布 COS 或 push。

## 可复用入口

`experiments/remote-desktop/verify_windows_linux_reopen.py`、
`viewer_reopen_cdp.mjs`、`sample_linux_media_processes.py`。
驱动需要已启动的隔离候选、仅该候选占用的 39100 调试端口、生成的全屏色块
夹具及独立回滚；不自行安装、切服务或读取固定路径的凭据。

```powershell
python experiments/remote-desktop/verify_windows_linux_reopen.py `
  --profile <private-candidate-profile> --code <device-code> `
  --remote-stage /tmp/pab-formal-linux-<id> `
  --remote-fixture /tmp/pab-linux-viewer-<id> `
  --ssh-helper <private-ssh-helper.py>
```

私有 SSH helper 提供 `connect()` 和 `run(client,password,script,timeout)`，
凭据只在既有本地私有存储读取、通过 SSH stdin 使用，不进入探针参数或结果。
必须先完成前置候选和回滚准备；不能对日常安装版的任意调试端口执行。
