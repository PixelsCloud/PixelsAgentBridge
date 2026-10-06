# 用户环境首批实现与文件恢复分流

日期：2026-10-07。基线：1cfddf3；源码版本 1.2.26。本阶段未打包、未替换已安装宿主。

## 已修复

- 文件写入/补丁发布恢复：原生 Service 身份走服务账号核验，User 身份仍使用冻结的原账户；账户不可用时保持 unconfirmed，不借用服务权限，也不重新执行写入。
- 先增强原有 publication_intent_recovers_after_restart_without_reexecution 测试，复现 `Some(Service)` 被错误保留为 unconfirmed；修复后 legacy/Service/User 不可用三分支均通过。
- Unix 用户工作进程 PATH 包含该账户 `.local/bin`、`.cargo/bin` 与系统目录；macOS 增加 Homebrew 双架构路径，并读取有大小/类型/所有者限制的 `/etc/paths`、`/etc/paths.d`。不执行 shell 初始化脚本，不复制服务 PATH。
- macOS 仅以 `launchctl asuser UID launchctl getenv SSH_AUTH_SOCK` 查询选中用户的 agent 地址；2 秒查询期限、8 KiB 输出上限，复用工作进程组回收。仅接受目标 UID 拥有的原生 socket。整个用户工作进程仍直接启动，内核 PID 校验不变。
- Linux 无图形环境不猜测其他账号的 agent；仅原生服务 UID 与选中 UID 相同且 socket 所属一致时保留其 SSH_AUTH_SOCK。
- 指定用户命令拒绝 HOME/USER/LOGNAME/USERPROFILE/APPDATA 等身份环境覆盖，大小写一致处理；允许 PATH 等调用方配置。旧 Service 模式保持兼容。

## 系统依据与实际行为

在目标 Mac 阅读系统自带 `man launchctl`：asuser 选择 Mach bootstrap/审计上下文，但不改变 UID/GID，也不采用用户环境；getenv 返回调用上下文的 launchd 环境值。因此仍由已有 setgroups/setgid/setuid 启动器承担真实身份切换。

实测仅 `sudo -u huayang launchctl getenv SSH_AUTH_SOCK` 返回空；通过 asuser 查询得到 UID 501 拥有的 socket。没有使用整个 launchd 环境，也没有读取/复制私钥。

## 验证结果

| 环境 | 已通过 |
|---|---|
| 本机 Windows | 文件系统相关 16 项；新身份环境校验；workspace --tests check |
| Mac 603527578 / ARM64 | 环境边界 3 项（路径、socket 归属、查询超时/超量回收）；文件发布恢复三分支；身份参数校验 |
| Debian bookworm 无 GUI 容器 | 同样的环境边界 3 项；文件发布恢复三分支 |
| Mac root → huayang UID 501 | 原生启动探针：HOME/cwd/文件所有者/PTY/按 PATH 查找 id 正确，Homebrew 与用户目录存在于 PATH，选中用户 agent 可见；服务哨兵未泄漏 |
| Linux root → pabworker UID 23001 | 原生启动探针通过，用户 PATH 正确，服务 agent 未继承；没有 DISPLAY/图形会话依赖 |

Mac 测试任务：`a42423ee-909a-4a75-bc17-58485568740e`；原生启动探针：`cff5bcf6-1886-4648-ba5e-b370fafa381f`，均 exit 0。Mac 本轮探针目录已删除；Linux 两个 --rm 容器已退出并删除，探针目录随容器清理。

## 仍待完成

这不是全部用户环境/凭据验收：还需真实用户自装程序查找、Git SSH/known_hosts/凭据 helper/Keychain 边界、认证拒绝与超时、Windows 用户环境实测扩展。这里只证明正确 agent 地址被传入，并未宣称已通过真实 SSH 认证。Linux root 切换其他账号时自定义 SSH agent 尚无发现入口。

非交互命令不会读取用户 shell 初始化脚本；现有 Mac PTY 仍是 zsh -l -i，Linux PTY 是 sh -i。终端启动模式的结构化回传与明确文档仍待补齐。

应用发现/启动/打开文件、最小 UI、完整异常矩阵、三平台完整安装及正式 AI 宿主验收继续按 EXECUTION_CONTEXT_ROADMAP.md 推进，不将源码测试视为最终交付。
