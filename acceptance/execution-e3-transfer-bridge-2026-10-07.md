# E3 Bridge/MCP 用户传输与显式续传

## 本次改动

- `pab_upload_file` / `pab_download_file` 增加严格的 `execution` 和 `resume_from`。默认 service；user 只能携带当前连接查询得到的 context_ref，不接受用户名、密码或自称身份。
- Bridge 先查询 transfer v2 能力。旧端可继续默认 service 传输，明确 user/续传不支持时先拒绝，不回退服务账户。
- 新版 Executor 在二进制内容之前返回 `TransferAccepted`，包含已经原子落库的真实执行上下文；Bridge 核对请求、调用者、方向、路径与身份模式。显式续传还核对原完整上下文。
- 本地异步队列持久保存请求选择、原任务引用和实际上下文。同 ID 改 execution/resume_from 会冲突，重复请求返回历史状态。状态查询与操作列表包含 execution_context。
- 用户身份一经观察不能被后来响应替换或清空；MCP 主动查询和后台恢复使用相同检查。尚未收到身份时保留未观察状态。
- 显式续传只接受已确认 failed/cancelled 的原记录，并要求相同设备、调用方、路径和覆盖设置。其他仍活跃的 MCP 的记录不能被接管。Executor 继续约束原生身份、上传内容、下载哈希与偏移。
- 下载续传沿用原完整哈希，转移原 staging 文件到新请求 ID；后续再次中断可以继续引用新的请求。只有完整哈希正确且通过现有取消/发布门控才发布本地文件。
- 已接受请求不会因新连接自动重新传文件；重复 ID 的远端回复要求查询原状态。接受前的明确拒绝可直接记录失败，不误报为发布结果未确认。
- Executor 重启后 running 上传转为 interrupted 且未发布；Bridge 可确认这类失败，随后显式续传。committing 仍需原身份核对，不能直接判失败。

## 验证

Windows 本地：Bridge transfer 18 项、MCP catalog 9 项、Executor transfer 12 项通过；workspace tests 编译检查通过。默认忽略的原生权限 fixture 在目标服务账户下另外运行。

新增 Bridge 网络用例使用真实 QUIC、受控远端协议响应：service/user 上传、用户下载从 staging 偏移继续、校验期望哈希、身份不符时不传二进制、旧端在打开传输流之前拒绝、明确接受前错误不进入未确认。

新增队列用例覆盖：身份缺失/改变拒绝、同 ID 改选择冲突、原任务运行中不能续传、已中断上传转失败、改变目标路径拒绝、数据库重开及新连接引用、续传返回其他用户身份拒绝、列表显示原身份。

目录校验首轮发现传输 execution 没有进入已有类型校验分支，新增测试失败；补入后 9 项全部通过。没有为了通过测试放宽 schema。

复核默认 service 发现初版传输校验错误地要求 identity 为空，但平台实际会观察 Service 身份。已按 mode 区分 Service/User 并保留未知旧记录；新增真实本机 service 上传及本地发布核对回归，Bridge 模拟端也改用有实际 Service 身份的响应。

| 平台 | 当前证据 |
|---|---|
| macOS root → UID501 | catalog 9 项通过于 `f63a27c3-d960-49ed-9c96-763e8fc72818`；最终 Service 修正后 `3379c15c-3754-4a9b-90e0-dafbcb5cd10c` 的 Bridge 18 项、Executor 12 项及原生用户 QUIC 集成全部通过，退出 0；fixture 目录剩余 0 |
| Windows90 SYSTEM → WTS1 / WTS2 | 最终 `cd610d49-e554-48c8-9e7c-235e45b0b60c`：12 项回归及两轮原生用户 QUIC 集成通过；测试目录与工作进程均剩余 0 |
| Linux 无桌面容器 | 最终 Bridge 18 项、Executor 12 项及 UID23001 原生 QUIC 集成通过；MCP catalog 9 项通过；Docker 命令退出 0，容器自动删除 |

Windows 本次原生 fixture 产物 SHA-256：

- Executor `4c17af442b0b1349ab643a4cbc7d2697b0ab5d53529814387febc76539d85ba6`
- lib tests `d2510965333caf780f506ea614c6876ddf4fb8fdc81e7bf46d1e5ca5a4283a20`

## 证据范围与后续

Bridge 网络测试使用受控远端，Executor 原生 fixture 通过 QUIC 启动真实用户进程；两者分别验证客户端协议处理与原生权限行为，尚未把这一阶段称作“安装后的 MCP 端到端验收”。当前会话仍使用已安装的旧 MCP 工具 schema，不能在这里直接传新增参数。

E6 仍需覆盖运行中取消/重启、长哈希、资源及路径替换等完整矩阵；E5 仍需最小 UI 的身份展示。接下来推进用户环境/凭据、应用入口、UI、完整测试与 E7/E8 打包安装及正式宿主验收。版本保持 1.2.26，未重启或替换正式服务。

后续复核事项：`task_service/filesystem.rs` 的文本发布恢复仍以“存在 identity”判断是否走 user-worker，而 Service 身份也可能存在；应增加 mode 分流和实际 service 回归，不能把仅用户路径通过当作默认 service 恢复已验证。Git 恢复已按 request.execution 分流。
