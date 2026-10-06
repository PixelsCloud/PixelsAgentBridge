# 指定用户执行：E0 原生原型验收

日期：2026-10-06。基线：862eb90。原始结果：`execution-e0-2026-10-06.json`。

本记录只证明原生账户切换、文件创建和现有 PTY 库可组合使用。
正式 MCP 的 execution 参数、上下文发现、任务指纹、文件/传输/Git 接入、应用入口尚未交付。
没有更新已安装软件，没有递增产品版本，也没有重置 macOS 授权。

## 实现与复用

- `pab-os-control::execution` 封装原生账户查询、准备目标用户和启动/回收工作进程。
- Windows 使用已有 windows-sys：WTSQueryUserToken、GetTokenInformation、CreateEnvironmentBlock、CreateProcessAsUserW。
  SID + session ID + AuthenticationId 再校验；使用会话原始 token，不自动取 linked elevated token。
- macOS/Linux 使用已有 libc：getpwuid_r、getgrouplist；在独立子进程的 pre_exec 内设置补充组、GID、UID，之后才 chdir。
  不在多线程服务进程中调用 setuid/setgid，不用 sudo/runas 实现产品身份切换。
- 所有参数按 argv 传递；Windows 按 CRT 规则转义，包含中文、空格、引号及末尾反斜线的实测通过。
- 子进程以原生用户信息建立干净环境，不继承服务环境；既有 portable-pty 0.9 原样用于用户工作进程内部。
- 工作进程为内部执行隔离机制，不接管 MCP 网络连接，不是 Bridge Host，也不是新的安装产品。

依据：[CreateProcessAsUserW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw)、
[portable-pty 0.9](https://docs.rs/portable-pty/0.9.0/portable_pty/)。

## 实测

| 环境 | 服务 → 用户 | 结果 |
|---|---|---|
| Windows 90 / 211399447 | SYSTEM → Administrator，session 1 | 成功：原生 SID、HOME、cwd、ConPTY whoami 与目标匹配；父进程仍是 SYSTEM |
| Windows 90 / 211399447 | SYSTEM → 已存在的另一个登录账户，session 2（断开状态） | 成功：独立 SID、用户目录、ConPTY 输出匹配；没有激活或修改该桌面会话 |
| macOS ARM / 603527578 | root → huayang，UID 501 / GID 20 | 成功：文件属主为 501:20；PTY 的 uid/gid/补充组均来自目标账户；父进程仍为 root |
| Linux Debian 容器，无 GUI、systemd、logind | root → 独立测试账户 UID 23001 | 成功：文件属主 23001:23001；额外组 23002 出现在 PTY id 输出中；父进程仍为 root |

Mac 和 Windows 操作均通过本会话正式 `pixels.pab_*` 工具进行；运行的是隔离验收二进制，不等同于新增 MCP API 的最终验收。
Linux 测试账户仅在 `docker run --rm` 容器中创建，容器结束后移除，不更改 WSL/宿主用户。
本机 WSL Ubuntu 20.04 使用 systemd，可作为后续服务安装测试环境；E0 没有把该环境记为安装验收通过。

附加检查：不存在的账户/会话拒绝、相对 worker 路径拒绝、缺失可执行文件拒绝、工作进程终止并重复终止、服务测试环境变量不泄漏。
macOS/Linux 另验证了 root 可访问而目标用户不可进入的 cwd：在降权后被内核拒绝，没有先以 root 进入目录再伪装身份。

Windows 文件 ACL 独立检查：Administrator 创建文件的 OwnerSid 为 S-1-5-32-544（Administrators），这是该管理员 token 的默认属主行为。
不能据此误判为 SYSTEM 代写，也不能承诺所有 Windows 管理员文件的 OwnerSid 必须等于个人用户 SID。

## 发现并修复的既有问题

Linux 首轮 PTY 命令已成功退出，但 `TerminalSession::close()` 再 kill 已回收进程，返回 ESRCH。
现检查进程退出状态，已退出视为关闭成功；kill 与退出竞争时再次核对进程状态。
在现有原生 PTY 测试中加入自然退出后的两次 close，保持真实进程验证而非实现镜像测试。

- Windows：os-control 4/4；terminal 7/7。
- Linux：os-control 4/4；terminal 5/5。
- Mac：os-control 10 passed / 1 原有 launchd 变更测试 ignored；terminal 5/5。
- `git diff --check` 通过。

## 后续接入必须完成的边界

1. 上下文引用绑定设备/调用方，接受请求时冻结实际身份，身份纳入去重与本地记录。
2. worker 使用 Executor 自身的内部模式；增加受控本地 IPC、握手身份核验、退出/取消/父进程丢失协议和有界资源管理。
   原型进程句柄仅保证直接工作进程终止，不保证所有后代；不能据此宣称完整任务取消已完成。
3. Windows 目前选取已有登录会话；未登录账户无 token 时失败。profile 生命周期、访问受限 cwd 的 worker 侧核验仍需正式集成。
4. Unix 环境目前为原生账户信息加干净系统 PATH。用户 shell 启动文件、SSH agent/凭据与 macOS GUI bootstrap 需分别实现/验证，不默默继承 root 环境。
5. 文件/上传下载必须把打开、临时文件创建、发布都移入目标权限上下文；主服务仍保留去重、路径互斥、进度和持久历史职责。
6. 现有工具未接入身份前，不发布切换身份能力声明或忽略 execution 参数后按服务身份执行。
7. Windows/macOS 应用入口原型、Linux systemd 服务安装、两轮正式 MCP 集成验收仍待完成。
