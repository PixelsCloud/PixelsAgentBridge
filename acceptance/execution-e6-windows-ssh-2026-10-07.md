# E6 Windows 指定用户的真实 SSH agent 认证

日期：2026-10-07；源码基线 `9fba018`，未打包或替换正式服务。
环境：Windows 10 x86_64，SYSTEM 测试宿主选择 chess / WTS 2，使用实际 user-worker 与 Git 工具执行。

## 测试方法

新增 `scripts/execution_ssh_windows_acceptance.py`，使用已安装 Git for Windows 的 OpenSSH 客户端和独立 ssh-agent。
测试密钥由当前 Windows 用户生成，加入独立 agent 后删除私钥文件。客户端配置仅指定该公钥与该 agent，
不修改 Windows ssh-agent 服务、用户登录 agent、真实 `~/.ssh` 或用户全局 Git 配置。

OpenSSH 服务运行在自动删除的 Linux 测试容器，只发布到 Windows 的 127.0.0.1 随机端口。
只挂载一个本次创建的 Git 夹具目录，没有挂载用户 home。容器使用独立普通账户 `pabssh`，关闭密码登录、
转发和 TTY，启用严格主机公钥及服务端 StrictModes 检查。容器内的 Git safe.directory 仅指向该夹具仓库。

复用既有 `native_user_git_acceptance`：测试宿主用 SYSTEM 临时计划任务运行，但 Git、SSH 与文件创建通过
产品选择的 Windows 用户 worker 执行。测试增加可选的专用 workspace/remote_url，校验 workspace 是目标用户
home 下、名称具有指定前缀、无链接且为空的直接子目录；Unix 默认夹具保持原流程。

## 结果

| 场景 | 结果 |
|---|---|
| 实际 SSH 服务普通账户探针 | 返回 pabssh |
| 指定 Windows 用户经 agent push/fetch | 通过，提交 OID 与远端 bare 仓库一致 |
| 主机公钥不匹配 | 明确拒绝，包含 Host key verification failed，在 20 秒界限内 |
| agent 不存在且磁盘私钥已删除 | 明确拒绝，包含 Permission denied，在 20 秒界限内 |
| 恢复原 agent/主机公钥 | fetch 成功 |
| 丢失 push 回执的原用户只读核对 | 通过，不重放 push |
| 实际用户身份、测试文件所有者 | 与选择的 token 一致，不是 SYSTEM |
| 清理 | 生成目录、容器、agent、SYSTEM 计划任务全部清理 |

首次测试本体已输出全部通过，但 PowerShell 启动器的结果断言失败，未记为整个测试通过。
改用持有原始 Process 对象的退出码采集后重新运行，测试进程与脚本均退出 0，输出：

```text
SSH_ACCEPTANCE agent_push=pass agent_fetch=pass wrong_host=pass no_agent=pass restored=pass
status=pass, session=2, private_key_on_disk_during_test=false
fixture_removed=true, agent_stopped=true
```

测试程序 SHA-256：`9df3902ad2cfb8908fe3db839fe19634b348d892c7a16308f2ccd89cf937a63b`。
测试 worker SHA-256：`8047904e659e5b58b137f694bb59dea120afe156e380991d0c99cc0da4c8c02c`。
结束后另查 Docker 和 Task Scheduler，均无本夹具前缀的遗留资源。

共享测试入口修改后，Mac root → UID 501 和 Linux root → UID 23001 的原生默认 Git 流程均通过。
Mac 原生任务 `52b84003-5690-4b2c-ad07-51078379b184`，退出码 0；Linux 临时容器退出 0 并删除。

## 复现与边界

在与目标 WTS 用户相同的 Windows 账户下，以提升权限的 PowerShell 运行：

```powershell
python scripts/execution_ssh_windows_acceptance.py --session 2 `
  --worker target/debug/pab-executor.exe `
  --test-binary target/debug/deps/pab_executor-<构建产生的哈希>.exe
```

这是 Git for Windows OpenSSH + 指定用户 worker 的真实认证测试，不涵盖 Windows 系统 ssh-agent 服务、
硬件密钥、所有第三方 agent 或 Windows90 的独立 SSH 环境。它不是新安装包和正式 MCP 工具的完整 E8 验收。
Mac Keychain、其余生命周期/并发矩阵和最终安装验收仍待完成。
