# E2 生命周期与 E3 指定用户命令（阶段记录）

## 实现范围

- 复用 `pab-executor --user-worker` 内部模式，无独立安装程序或 Bridge Host。worker 不加载机器配置、数据库或网络凭据。
- `pab_run_command.execution` 默认 service；user 必须引用当前连接发现的上下文，提交前再次观察原生身份。不可用或身份变化明确失败，不回落到服务账户。
- 命令协议 v3；旧 service 请求保持原序列化。接受后冻结实际身份、用户 home/cwd 与环境指纹；相同请求重查原记录，同 ID 改身份返回冲突。
- 既有命令执行逻辑负责 stdin、环境、输出、超时和取消；用户 worker 通过内核核验过的本地通道返回事件与原始二进制。记录仍由服务写入本地数据库。
- Windows 挂起创建、先加入 kill-on-close Job 再恢复；Unix 使用专属进程组，退出前清理同组后代。对端丢失、缺少结果等记为 interrupted，不自动重放。
- 工作进程最终结果与退出清理确认后才写入终态。Unix 主动脱离进程组的后代不在此保证内；PTY 另需显式关闭。

## 实测结果

源码 fixture 经正式 Pixels 工具传输、编译或启动；这不是安装后当前 MCP 宿主新增参数的验收。

| 平台 | 身份与命令 | 生命周期 |
|---|---|---|
| Windows 90 SYSTEM → WTS1 Administrator | 通过 | terminate/drop/worker 自行退出均回收后代 |
| Windows 90 SYSTEM → WTS2 普通账户 | 通过 | 同上；未激活桌面 |
| macOS root → UID501 | 通过 | 同上 |
| Linux 无 GUI/logind，root → UID23001 | 通过 | 同上；容器使用 init 回收孤儿 |

命令用例核对实际 SID/UID、用户 home 默认 cwd、中文 env/stdin/stdout、原请求重查、同 ID 换身份冲突、超时失败和立即取消。

命令远端任务：Windows `16ef6529-6d4d-4fc9-b6d0-0d7a314a54bd`（两个账户）；Mac `33981c49-9808-45c6-85a0-12d94c6249e7`。Linux受控容器原生测试退出码0。
生命周期任务：Windows WTS1 `b9a4696f-b562-46c8-bf96-227edbb42122` 前三项通过，随后 WTS2 fixture ACL 失败；修正仅测试文件读取执行权限后，WTS2 `793eea4e-62cc-4a52-95f4-2d35b55b779f` 全通过；Mac `47775bb2-a7da-4572-a923-9bf14d8b2d5b` 全通过。

本地检查：task-runtime 15项通过；MCP execution 参数解析回归通过；命令 service 回归、worker 二进制帧、协议兼容测试通过；Desktop Rust tests 编译检查通过。

## 实测发现与修复

1. 三平台均复现立即取消与 Running 通知的竞态。现在接收迟到的开始事实但保留 CancelRequested，最终确认 Cancelled；回归用例覆盖，三平台重测通过。
2. macOS 只剩僵尸的组可能令 killpg 返回 EPERM。避免重复终止；自然退出时经原生进程状态确认不存在活成员才视为已清理，真实权限拒绝仍报错。
3. Windows 上传 fixture 位于 SYSTEM/Administrators 独占目录，普通账户不能再次执行它。仅给该测试文件添加目标用户 RX 后复验；不修改用户账户或机器权限。

## 尚未验收

终端、Git、文件与传输的用户执行尚待接入。父进程崩溃、双 MCP 并发、升级恢复及完整 E6 矩阵仍待完成。
未打包安装；产品版本仍为1.2.26，正式宿主仍使用原安装版本。不代表整个 E2/E3 或 E8 完成。
