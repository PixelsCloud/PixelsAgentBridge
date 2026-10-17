# Linux → Mac 正式窗口、输入释放与 AI 协助

日期：2026-10-17；工作树基线 `8e76f10`。复用已构建的候选产物，本轮未编译产品。
产物 SHA-256、原始探测结果、任务 ID 和清理记录见
[机器记录](remote-desktop-linux-to-macos-2026-10-17.json)。

## 实际链路

Linux 操作端为 Ubuntu 22.04.5 GNOME Wayland，会话 23、UID 1000、1280×800。
独立 Desktop/MCP 使用 0700 私有 profile 和 TOML；仅测试资料接收已有 Apple
设备凭据，安装版配置和设备列表未修改。Mac 被控端为 macOS 27.0.1 arm64，
1920×1080、1×，候选 Executor 与已授权固定路径/签名的 Aqua helper 配对。

Mac 临时服务继续使用独立设备身份/认证副本，先启动 8 分钟恢复作业，候选最多
运行 7 分钟。未覆盖安装目录或原数据库。Linux 原后台服务全程保持运行。

Linux MCP 发起真实协助请求，主窗口显示提示。通过指定 PID 的 AT-SPI 按钮
打开正式独立窗口。VMware VIX 独立截图确认原生显示区域确实呈现 Mac 桌面和
随后创建的 AppKit 测试窗口，默认只查看；VIX 仅作观察，不作为产品采集通道。
只向自建窗口发送输入，输入经 Linux 原生 surface、产品网络、Mac helper/worker。

## 结果

| 检查 | 观察 |
| --- | --- |
| 单击 | AppKit downs/ups 从 0/0 变为 1/1 |
| 坐标 | Linux 实际桌面 (251,558) 对应测试 view (100,100)，实际正好 (100,100) |
| A 键 | keys_down/keys_up 各 1，Mac keyCode=0 |
| 停止操作后再次单击 | downs/ups 保持 1/1 |
| 明确完成协助 | 原 Linux MCP 返回 completed/user_completed，terminal=true、verify_result_before_resuming=true |
| AI 复核 | 通过原 Mac 命令连接读取新鲜计数，结果一致 |
| 按住 Shift | shift_down=1、shift_up=0、shifted=true |
| 关闭正式查看窗口 | shift_up=1、shifted=false |

目标输入点由源画面 1920×1080、Linux 实际原生区域 (70,146,1210,654) 和保持
比例的黑边共同计算。Wayland AT-SPI 给出的区域位置在此环境中是相对窗口的；
使用独立 VM 截图核对实际桌面原点，没有直接将其当作屏幕坐标。

每次 Mac 计数探测要求 PID=73740、数据新鲜度小于 2 秒。关闭窗口使用指定
Linux 查看窗口的原生 Close 按钮；先在 Mac 看到 Shift 释放，再通知 Linux 的
有界输入驱动释放本机 Shift。采集到的不是 fixture 退出后的旧数据。
AI 复核由本机代理的原命令连接执行，Linux 请求进程负责 wait/完成通知。

## 启动与权限观察

此前隔离 Executor 放在 root 私有目录会阻止 native user 重新执行。本轮沿用
已调整的布局：可执行文件位于 0711 外层、模式 0755；身份资料仍位于 0700
private 目录。候选服务的 user context 执行 `/usr/bin/id -u` 返回 501，退出 0，
补齐该进程创建路径的实机验证；不将单条命令外推为文件权限或全部命令回归。

画面中可见一个“查找本地网络设备”的系统提示，没有操作该提示，也没有证明
它由哪个运行实例触发。测试窗口在其前方正常获得焦点并接收输入。此轮未观察
到屏幕录制/辅助功能的重新申请，不把本地网络提示解释成屏幕录制授权丢失。

首次 root 准备脚本误经 user 包装器调用，在首行身份断言处退出，未创建资源；
随后改用 service context。64 位 Python 无法加载本机 32 位 VIX DLL，在建立
VIX 连接前失败；改用已有 32 位观察包装器成功截图。两项均为测试驱动问题，
没有改产品逻辑或把不确定操作重复提交。

## 恢复与边界

Mac 原服务恢复 PID 74172；安装程序、原启动脚本、服务配置、设备密钥哈希和
设备认证记录保持一致。候选/fixture 进程为零，3 个临时 launchd 作业、临时服务
目录及 fixture 目录已删除。固定已授权测试包保留，不改签名或路径。
Linux Desktop/MCP 和输入 worker 正常退出，私有 profile（含账号/凭据副本）
已删除，原服务 active、会话仍为 23；临时传输文件已从两端删除。

两份独立 VM 截图只保存在本地 Git 忽略的 `.build` 目录，未提交图像：
首帧 SHA-256 `4ddc33ab2c31b8ac4cae0deaa17f04de6b9edc0eebd34f2aa2608f47dac7263a`，
fixture 画面 `f5d5a4c792b79634af8c2c170de8c1bbcda55f569a94e0a8dff7d89485752171`。

本轮只覆盖普通单屏首帧、单击/A/Shift、只查看、关闭释放与协助闭环。
15 FPS 仍是设置值，不能据此声称动态帧率已验收；多屏、系统登录、完整键盘/
输入法、弱网、持续运行及最终安装发布仍未完成，W4/W5/W6/W7 不整体关闭。
