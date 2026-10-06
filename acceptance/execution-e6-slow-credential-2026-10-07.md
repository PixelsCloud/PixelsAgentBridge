# E6 慢 Git 凭据程序的取消与超时

日期：2026-10-07。本轮为源码修复和原生用户集成测试，未递增版本或重新打包。

## 发现与修复

本机 Windows 10 专业版的 SYSTEM 测试进程选择 chess/WTS 2，通过已安装 1.2.27 Executor
启动 user-worker。原代码在收到 Git 最终响应后先等待 worker 自行退出，最多 5 秒，随后才回收 Job。
实际测试在取消后观察到凭据程序继续执行延迟写入，测试失败，exit 101。

Windows 上继承管道的读取可能阻止 worker 的运行时及时退出。这段宽限期会让后代进程继续工作。
修复为收到最终响应（或 IPC 错误/超时）后立即调用现有 UserProcess.terminate，
回收独立 Windows Job / Unix 进程组，并在清理完成前持有父端仓库锁。
没有改变 unconfirmed 语义，没有将取消解释为已回滚。

## 新增测试

`task_service::git_user_tests::native_user_git_slow_credential_cleanup`：

- 使用生成的用户 home 子目录及独立仓库；只修改该仓库的 credential.helper。
- 本地临时 HTTP 服务返回 401，触发真实 Git credential helper；清空继承的 helper 列表，不读取真实凭据。
- helper 先写启动标记，等待 4 秒后尝试写第二个文件。只有观察到启动标记后才主动取消。
- 分别测试主动取消和 2 秒操作期限。结果须为 unconfirmed 且实际账户正确。
- 等待超过 helper 的延迟写入时间，要求无第二个文件；Unix 还核对标记文件的 UID。
- 相同 request ID 保持原结果且不再次调用 helper；后续 status 成功，证明仓库锁已释放。

## 结果

| 平台 | 修复后证据 |
|---|---|
| 本机 Windows，chess/WTS 2 | 新增测试通过，14.03 秒；原正常用户 Git 工作流通过，4.28 秒；12 项 Git 回归通过 |
| Linux 无 GUI，UID 23001 | 新增测试及原正常用户 Git 工作流共 2 项通过，13.14 秒；12 项 Git 回归通过 |
| Linux 无 GUI，UID 23002 | 新增测试通过，主动取消及期限场景均通过 |
| macOS | 尚未运行该修复的原生回归，须恢复正式 MCP 连接后验证 |

Windows/Linux 测试使用**修改后的父端测试二进制 + 已安装 1.2.27 worker**。
修改在父端协调逻辑；已安装服务本身未被替换，因此不能据此宣称安装包已包含修复。
Windows 编译检查及两个平台测试编译通过。

Windows 专用 SYSTEM 计划任务已移除，生成的仓库已清理，剩余 Executor 只有原安装服务。
Linux 两用户生成的仓库和临时测试程序已清理，原服务及 E8 验收数据卷保留。
本机 `.build/slow-credential-windows-before-fix*` 保留失败证据，
`.build/slow-credential-windows*`、`.build/git-user-regression-windows*` 保留通过证据，均不提交原始日志。

## 仍待完成

这不是当前 AI 宿主 `pab_git_*` 的正式端到端验收，也未覆盖 service 默认身份、
主动脱离进程组的凭据程序、Mac Keychain、真实 Windows SSH 凭据及完整并发矩阵。
Mac 回归后须重新生成并部署受影响安装包，原 E7 包的哈希仅代表此前版本。
