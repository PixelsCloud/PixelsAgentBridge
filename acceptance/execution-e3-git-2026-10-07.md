# E3 指定用户 Git 阶段记录

## 实现

- 八个现有 Git 工具增加 execution 选择；缺省 service 保持原行为，user 要求 capability v11。参数进入原请求指纹，拒绝伪造用户名、密码和 desktop_user。
- 接受前按当前连接引用复核原生身份，原子保存实际 execution_context。查重先于新上下文解析，重连后仍返回原记录，不改写执行者。
- 所有 Git 子进程、路径检查及仓库文件访问都在目标用户 worker 内执行；复用原生 Git 的配置、hooks、凭据机制和既有执行实现，不拼接 sudo/runas。
- Executor 继续拥有 SQLite 和仓库互斥锁。worker 发现实际 git directory 后向父进程申请锁，进度必须持久化并收到确认才继续写操作；service 与不同用户的 worker 共享同一锁域。
- 保留原取消、超时及未确认语义；通道丢失不重放。锁保持到工作进程清理之后。
- 未确认 push 的只读核对使用原记录冻结的用户身份，不依赖新连接的引用，更不回落到 service；身份当前不可用则保留未确认。

## 证据

| 环境 | 指定用户 Git 原生集成 |
|---|---|
| Windows90 SYSTEM → WTS1 Administrator | 通过 |
| Windows90 SYSTEM → WTS2普通账户 | 通过 |
| macOS root → UID501 | 通过 |
| Linux无GUI/logind root → UID23001 | 通过 |

独立用户目录内完成 Git init、用户创建文件、选择文件 commit、log、推送到临时 bare 仓库。
核对提交数量/作者、工作树与远端OID、Unix文件UID及Windows目标token默认所有者。
测试持有服务仓库锁时用户worker返回busy；同请求不重复提交、同ID换用户冲突、重连后查询原提交、按原用户核对丢失的push回执。

Windows任务 `f223c119-5d63-4b37-abfe-c45ed3c48b48`；Mac任务 `4e83aac2-63ea-4ac4-8c85-7e4222f607e7`；Linux受控容器测试退出0。
Mac首次测试发现父进程误拒绝“尚在解析仓库”的无副作用进度，导致未确认。现在允许该初始阶段，后续进度仍必须已获仓库锁；修正后三平台通过。

回归：原有12项Git测试通过（含真实QUIC、冲突、选择提交、fetch取消/超时、bare push/fetch/pull、丢回执核对）；MCP Git参数2项、协议Git契约、无有效用户引用不接受service任务通过。
Workspace tests编译、Desktop Rust tests编译通过。

清理核对：Windows两用户目录无 `pab-git-acceptance-*` 残留，也无测试worker进程（任务 `fa61a424-c3ab-4b0e-b3d4-5fd4c9e4e74f`）；Mac残留目录数0（任务 `6ec861d3-4919-4ad3-a30c-0ad3a9c13fd0`）。Linux容器已退出并自动删除。

## 未完成范围

这是源码集成测试，正式 Pixels 工具负责远端传输/编译/启动/观察，尚非安装后新增 execution 参数的宿主验收。
真实SSH agent、macOS钥匙串、慢凭据helper、用户PATH来源和完整双MCP/进程崩溃矩阵仍需E6；临时bare仓库不能证明远端凭据认证已完成。
文件、传输、应用入口、最小UI和最终打包仍在长任务范围内。未增加产品版本、未安装新的MCP。
