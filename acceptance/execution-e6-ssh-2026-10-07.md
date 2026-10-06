# E6：指定用户的真实 SSH agent 认证

源码版本 1.2.26，基线 2c11f5c。本次未打包、安装或更换正式 MCP 宿主。

## 验证方法

复用 `native_user_git_acceptance` 的真实用户 worker、Git 工具、身份记录、提交及推送核对，
以临时 OpenSSH 服务替换仅本机文件协议的 remote。新增脚本
`scripts/execution_ssh_acceptance.py` 管理 loopback 服务、专用密钥和独立用户 agent。

- root 启动测试宿主，产品代码按选择切到普通 UID，Git 及 SSH 均在该用户 worker 内执行。
- 密钥由目标 UID 生成，加入该 UID 的独立 agent 后删除磁盘私钥；客户端只有公钥文件，证明实际使用 agent 签名。
- `core.sshCommand` 只写入专用测试仓库，显式指定测试 agent 和 known_hosts；不改现有 `~/.ssh`、系统 sshd、登录 agent 或真实仓库。
- 先直接核验测试 SSH 服务返回的 UID，再通过产品 Git push/fetch 验证认证、提交 OID 和实际执行身份。
- 替换为错误主机公钥、不存在的 agent socket，均要求有界失败；恢复后 fetch 成功。
- 原用户的丢失 push 回执核对仍经过真实 SSH 连接，不重放 push。

## 结果

| 目标 | 结果 |
|---|---|
| macOS 27 ARM，root → huayang / UID 501 | agent push/fetch、错误主机密钥拒绝、无 agent 拒绝、恢复 fetch 全部通过 |
| Debian bookworm 无 GUI/logind，root → pabworker / UID 23001 | 同样全部通过 |
| Windows90，SYSTEM → WTS1 Administrator / WTS2 普通用户 | 新测试程序的默认本地 bare 工作流均回归通过；本项不是 Windows SSH 验收 |

Mac 成功任务 `2f2fe2ce-9247-442b-a726-f9cea93d2f2d`，退出码 0。
Linux 使用 `pab-linux-execution-ssh:bookworm`（基于已有编译镜像增加 openssh-server/client），
测试容器退出码 0，并由 `--rm` 清理；未开启图形环境或修改宿主 SSH 服务。
Windows90 回归任务 `3dfe003c-2f5a-491c-9db8-87bdb606bee2`，两个用户各 1 项通过，退出码 0。
测试程序 SHA-256 `c9c848a9d24eedaeb32e74124ffa5dff52af54c5db433bfd4fb848f499ed0c3e`；
user worker SHA-256 `4c17af442b0b1349ab643a4cbc7d2697b0ab5d53529814387febc76539d85ba6`。

首轮失败属于测试环境：OpenSSH StrictModes 拒绝公共 `/tmp`（Mac 为 `/private/tmp`）
祖先目录。通过客户端/服务端日志确认后，将独立测试目录放到目标用户 home；保留 StrictModes，
没有降低认证要求，也没有修改产品身份切换代码。

脚本成功、失败均结束自己启动的进程组并删除专用目录。Mac 核对任务
`b808ce16-eaae-48fe-bd41-a8b5d9026370` 显示 Git/SSH 测试目录均为 0；
按可执行文件路径再次核对（`f8c285e3-2aba-4329-a55f-230cd7cba643`）确认源码 worker 进程为 0。
首次按命令行文本计数命中了诊断命令自身，未将该数字误作遗留 worker。
Windows 清理任务 `4f1c0ff2-39a3-4c48-b369-9ba8d8ae094f` 核对专用目录路径、文件哈希及进程后删除测试程序；
确认测试目录已删除、用户 Git 工作区残留 0、测试进程 0。

## 复现

在 Unix 测试机安装 OpenSSH server/client，准备普通测试账户，先构建当前源码的 Executor
及 `cargo test -p pab-executor --lib --no-run` 对应测试程序，再以 root 执行：

```sh
python3 scripts/execution_ssh_acceptance.py \
  --uid <测试账户UID> \
  --worker <当前源码pab-executor绝对路径> \
  --test-binary <当前源码Executor测试程序绝对路径>
```

## 边界

这是原生用户 worker + 真实 SSH 的源码集成验收，不是安装后正式 AI 宿主新参数验收。
agent 通过临时仓库显式配置；不宣称 Linux 可自动发现其他用户 agent，也不宣称完成 Mac Keychain 验收。
Windows 的实际 SSH 凭据、慢 credential helper、钥匙串、双 MCP 并发和完整生命周期矩阵仍待完成。
本轮发现 Linux 既有包脚本仍强制携带 Desktop，需要在 E7 增加明确的无界面交付入口后安装验收。
