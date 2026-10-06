# E1 执行身份协议基础验证（阶段记录）

本阶段新增协议/状态基础，不代表 E1–E8 完成交付。尚未发布 `pab_list_execution_contexts` 或给现有 MCP 工具开放 `execution` 参数。

## 已实现

- `ExecutionSelection`：缺省 service；user/desktop_user 必须提供不透明 context_ref，不接受自报用户名、密码或“最近用户”。
- `ExecutionIdentity`：执行模式、原生账户 ID/名称、home、主组、会话及登录代次、环境来源；不含凭据和完整环境。
- 现有 `ExecutionContext.identity` 为可选观察值：旧任务 JSON 不改写为当前账户，也不猜测 root/SYSTEM。提示中未观察身份为 unobserved。
- 原生环境版本变为 native-v2，把观察到的账户/登录代次纳入摘要；账户查询失败不停止设备服务，身份保持未观察。
- `ExecutionContextRegistry` 按设备、OperatorRef、认证连接 ID 绑定引用。刷新同一身份复用引用；容量有界且不静默淘汰；注销/账户变化在解析时拒绝。
- 同 endpoint key 的多个 MCP 连接相互隔离；释放一个连接不影响另一个。接受后的身份为独立快照。

## 验证

Windows 本机、macOS ARM（603527578）、Linux 无 GUI 容器均通过：

- protocol：42 单元测试 + 3 target_context 集成测试。
- task-runtime：5 注册表单元测试 + 9 原有任务聚合集成测试。
- platform：2 原生环境测试，含实际账户读取、账户/登录代次变化导致 revision 改变。

编译检查：Windows / Mac `cargo check --locked --workspace --tests` 通过；Linux `cargo check --locked -p pab-executor -p pab-bridge --tests` 通过。
Windows Tauri 桌面 `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --tests` 通过，并同步其 lockfile 的 os-control serde 依赖。
Mac 正式宿主命令 task ID：31533756-5d17-444c-9cda-bb23a6071221，最终 succeeded，输出 E1_FOUNDATION_CHECKS_PASSED。

测试发现 serde 的内部 tagged enum 单元变体会忽略多余字段，已把 service 改为空 struct 变体，确保 `{"mode":"service","password":"..."}` 被拒绝。

## 仍需完成

上下文查询、能力协商、接受阶段与持久化指纹接入、用户 worker IPC/生命周期、现有工具执行路由和 UI。
注册表单元测试没有替代最终“同 request_id 更换身份必须冲突”的端到端测试；原生服务身份观察也没有替代真正的用户执行集成。
本阶段未打包安装、未修改产品版本、未请求或重置 macOS TCC 权限。
