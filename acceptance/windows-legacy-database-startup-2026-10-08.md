# Windows 旧数据库导致安装时服务启动失败

用户现场为正确的带引号 Executor 路径、LocalSystem、自动启动，服务退出码 1。
实际启动日志明确为 SQLite 1299：`NOT NULL constraint failed: device_access.deployment_id`。
卸载保留机器数据库，因此重新安装仍读取旧 schema；不能归因于服务注册残留或网络。
较早的 InvalidCredentials/relay 拒绝日志属于旧进程，不能代替本次启动失败原因。

按用户明确要求，开发阶段不兼容旧数据库：发现旧格式直接删除旧库内容并重建，不备份、不迁移。
检测条件为 executor.sqlite3 的 device_access 表包含已废弃的 deployment_id 列。
处理位于跨平台 Executor 的数据库打开路径，Windows、macOS、Linux 无界面版共用。
删除范围是该库内全部用户表、视图及其数据（关联索引、触发器随表删除），包括旧任务记录。
保留库外的 device-endpoint.key 与 local-access.key；启动流程重新生成临时设备密码。
当前格式的数据库不清理；损坏、锁定、访问失败不冒充旧格式，不触发数据删除。

实现使用 SQLite 写事务原子删除旧 schema，不直接 unlink 正在被其他连接访问的数据库文件。
取得写锁后再次检测格式，防止并发打开时重复清理新数据。失败时事务回滚；随后由原有初始化
流程建立现行表及凭据。这里没有旧字段迁移或 deployment ID 的运行时兼容逻辑。

源码已补充：

- 在 device_access 读写之前自动清理上述旧格式数据库，并输出清理日志。
- 安装器启动服务失败时，展示最近的 Executor 启动错误及日志位置，避免只看到 Start-Service 泛化错误。
- 当前密码的重启保留、主动轮换行为不变。

验证项目：旧表/关联记录/视图清理、新表写入、身份密钥保留、任务库重建、现行数据保留、
并发打开不重复清理、损坏数据库不误删、旧库启动后新密码可验证、再次启动保持密码。
本地通过 `cargo test --locked -p pab-executor --lib device_access::tests`（3 项）和
`cargo test --locked -p pab-executor --lib bootstrap::tests`（5 项）；安装脚本 PowerShell
语法解析及 `git diff --check` 通过。Mac/Linux 使用共用逻辑，本次未在这两个平台重新打包或实测。
Windows 安装失败现场无法远程连接，最终安装恢复需在该机器上验证。

## 安装包验证

Windows Release 1.2.45 已重新构建。ZIP CRC、三个可执行文件与构建清单的 SHA-256、
安装器 FileVersion/ProductVersion 均通过。使用与包内哈希一致的 Executor，在隔离目录中
通过 `show-access` 触发旧库自动清理（清理后因尚无凭据，按预期返回未初始化），随后写入
现行格式并验证能正常读取；身份密钥内容保持不变。未触碰本机已安装服务或数据库。

- 安装包：`.build/packages/pixels-agent-bridge-windows-x86_64-release-setup.exe`
- 大小：22,813,340 字节
- SHA-256：`71c6f2be6fdd5d31159be4e2269a4196ed44ab8b4c46738c80b18ee81a82caec`
