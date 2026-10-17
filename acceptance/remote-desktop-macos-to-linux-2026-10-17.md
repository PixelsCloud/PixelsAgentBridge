# Mac → Linux 正式窗口与 AI 协助

日期：2026-10-17；工作树基线 `3be229f`。本轮复用已构建的三份候选产物，具体
SHA-256、原始探测任务编号和截图摘要见
[机器记录](remote-desktop-macos-to-linux-2026-10-17.json)。没有新编译或替换安装版。

## 环境与路径

操作端为 Apple macOS 27.0.1 arm64，单屏 1920×1080、1×；固定已授权测试 App，
正式独立窗口 1100×760，位置 (410,80)。Mac MCP 与 Desktop 使用相同隔离 TOML
和账号，通过已有认证、可靠传输连接 Linux VM。没有请求新的 macOS 权限。

目标为 Ubuntu 22.04.5 GNOME Wayland，活动会话 23、UID 1000、1280×800，
测试前后均 unlocked。Linux 临时 Executor 由 systemd drop-in 指向隔离产物，
保留原配置，先安排独立恢复计时器，再由有界管理服务启动；7 分钟限时恢复，
另有 8 分钟回滚。独立 root 媒体 supervisor 核实会话后降权启动用户 helper，
与生产实现相同。没有更换会话、重启或修改屏幕布局。

Linux 自建 GTK 全屏测试窗口最多运行 300 秒，只统计左键和 Ctrl 的按下/抬起，
不记录正文。数据每 100 ms 原子更新；每次远端探测都要求数据新鲜度小于 2 秒。
输入先进入 Mac 产品窗口的原生显示区域，再经过产品网络通道抵达 Linux。
没有直接调用 Linux 输入工具代替产品链路。

## 实际结果

| 步骤 | Linux / Mac 观察 |
| --- | --- |
| AI 发起协助 | Mac 主窗口显示原因、目标和打开按钮，状态 waiting/displayed |
| 明确打开远程桌面 | 原生首帧为 Linux 测试窗口，默认仅查看，状态 active |
| 启用操作后中心单击 | clicks=1、releases=1，位置 (640.1133,400.1172) |
| Ctrl 按下/抬起 | down=1、up=1、held=false |
| 停止操作后再单击画面 | clicks/release 均保持 1，没有发送远端点击 |
| 点击完成协助 | 原 Mac MCP 请求方观察到 completed/user_completed |
| AI 重新核对结果 | 原命令通道读取 Linux 新鲜状态，计数与释放状态一致 |
| 再启用、聚焦画面并按住 Ctrl | clicks=2、releases=2、Ctrl down=2/up=1、held=true |
| 关闭独立窗口 | Ctrl up 变为 2、held=false；左键无残留 |

中心目标 (639.5,399.5)，实际每轴误差均小于 1 像素。关闭操作通过该窗口红色
关闭按钮对应的 AX 元素 invoke 执行；在释放 Mac 本机 Ctrl 之前，先从 Linux
独立命令通道确认远端已收到释放。这里证明的是窗口关闭后的释放路径，不能
替代网络异常、强杀进程或整机休眠的释放验收。

AI 复核由本机代理通过已有 Linux 命令连接执行；Mac 请求进程负责观察状态。
不声称 Mac MCP 自行执行了后续复核命令。15 FPS 是本次配置值，画面为静态测试
窗口，不能用本次首帧和操作验收代替动态有效帧率测量。

## 启动观察与清理

首次 Mac 测试驱动将连接等待 5 秒后返回 pending 当成失败，尚未创建协助请求
或启动 GUI。确认该驱动任务失败退出后，改为同一 MCP 连接内有限轮询，随后
完成连接。它是测试驱动的观察错误，不是 Linux 连接能力的失败结论。
首个启用操作点击仅激活 Mac 窗口，截图确认仍处于仅查看，随后明确点击启用
才操作画面，没有以 API 返回成功代替模式状态检查。

既有 Linux 设备凭据只临时复制到 Mac 私有测试 profile。传输文件在导入后从
两端删除；结束后删除本轮新增的 Linux 凭据行，固定测试包和原 90 测试资料保留。
安装版账号、设备列表和配置未修改，凭据未出现在记录中。

Mac GUI/MCP 任务正常退出，候选进程为零。Linux 媒体 supervisor 与测试窗口
停止，原 Executor 恢复 active，drop-in 和恢复计时器清理，临时 profile、
媒体运行目录和本轮传入的 Executor 删除。安装版二进制、TOML 和 systemd unit
哈希与测试前完全一致。清理首次进程扫描把自身命令源文本误认作 fixture，
在删除前拒绝；改为精确 argv 路径匹配后确认无实际遗留进程，再完成清理。

本轮补齐 Mac → Linux 的普通单屏首帧、基本输入、只查看、关闭释放和协助闭环。
macOS/Linux 系统登录、真实多屏、完整输入、其他矩阵方向、弱网、长期稳定性和
最终候选包仍待验收；W4/W5/W6/W7 不整体关闭，未发布 COS。

测试窗口源文件：[linux_input_fixture.py](../experiments/remote-desktop/linux_input_fixture.py)。
此工具针对当前 UID 1000 的 GNOME Wayland 测试环境，会覆盖该测试机器主屏，
不适合作为普通后台任务运行。
