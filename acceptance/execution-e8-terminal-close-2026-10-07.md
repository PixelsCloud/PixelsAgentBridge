# E8 原生终端关闭问题及回归

日期：2026-10-07。修复基线 `44d95b1`；测试构建沿用源码版本 1.2.28，不是新安装包。

## 原生宿主发现

Linux 已安装 1.2.27、设备 `565893930`、用户 `pabuser1`（UID/GID 23001）。
本会话直接使用 `pixels.pab_terminal_open/input/read/resize/close`，没有通过另起 stdio 代理代替工具。
会话 `d7a86737-f8f9-4a3f-ba5d-a2a588db8bcc` 的真实 UID、home/cwd、中文文件和 resize 正常，
但 close 返回 `NotFound`，随后 read 同样返回 `NotFound`。
原生命令任务 `6679994b-b09a-4c27-ad0f-fe7391060d7c` 只读查询 Executor 数据库，确认原会话实际已是 `closed`。
没有重新打开终端掩盖此结果，也没有改写历史记录。

根因：Executor 在 close 回执前删除会话，而 BridgeRuntime 在收到 close 回执后仍需读取末尾输出并归档。
指定用户的工作进程也在 close 后退出，原有输出仅保留在该进程中。另一个问题是 Desktop 的定时读取可能抢先读到 EOF，
导致关闭操作再次查询已经释放的会话；原本固定 20 次读取也只能归档最多 640 KiB 的剩余内容。

## 修复

- close 停止终端后保留有界输出，读取 EOF 才删除会话；未读取的缓存 30 秒清理，沿用每会话 1 MiB 缓冲及总会话配额。
- 用户工作进程以已有二进制帧传回关闭时的末尾输出，确认工作进程退出后缓存到父进程；不保留后台工作进程等待客户端读取。
- close 与 read 串行执行；已确认关闭的会话在连接断开时不降级成 `interrupted`。
- Bridge 持有会话锁完成关闭和归档，已在等待的读取收到 EOF；读取有输出时不额外 sleep，以 15 秒窗口替代 20 次读取上限。
- 关闭已确认但归档失败时，记录失败并释放本地活动会话；不能把失败留下为永久运行或伪装为完整归档。

## 源码验收

实际回环 QUIC 测试覆盖默认终端 close → 分帧 read → EOF/释放、另一连接拒绝读取关闭后的缓存、
关闭后断开连接不回退状态。指定用户测试也改为走此 QUIC 流程，不再只直接调用 Session::close。

BridgeRuntime 通过真实 QUIC 测试对端验证 700,003 字节尾部的完整文件归档、与 Desktop 轮询等价的已等待读取，
以及远端返回 NotFound 时的失败记录和本地活动项释放。它是独立协议测试对端，不冒充实际 Executor。

| 环境 | Executor 默认终端/错误记录/断线状态 | 真实指定用户 PTY + QUIC 关闭 | BridgeRuntime 大尾部/并发/失败归档 |
|---|---|---|---|
| Windows 10 x86_64 | 4 项通过 | SYSTEM 测试进程选择 chess/WTS 2，通过 | 通过 |
| macOS 27 ARM64 | 4 项通过 | root 测试进程选择 huayang/UID 501，通过 | 通过 |
| Debian 12 x86_64 无界面 | 4 项通过 | root 测试进程分别选择 UID 23001/23002，通过 | 通过 |

Windows 用户测试日志 `.build/terminal-close-windows.log`，退出码 0；临时 SYSTEM 计划任务已移除。
Mac 原生任务 `00ce2a4a-79e1-4326-9c63-d5b1ef4ba7d2` 执行真实用户及默认 PTY，退出码 0；
`ae068bec-7a67-422f-bbfd-ba82ce1aebcb` 执行 Bridge 回归及 Executor 测试；
断线状态补测任务 `210e1426-e57d-4e28-aec9-d6ad8d5213f8`。
Linux 使用保留的 headless 验收容器及临时测试二进制，不替换正在运行的已安装 Executor。

本证据不包括 30 秒缓存过期的计时实测，也不等于最终安装宿主验收。需要把本修复与此前 Git 清理修复一起打包、安装后，
再通过正式工具验证；原生两轮端到端任务尚未完成。
