# Linux 媒体服务读取 Executor 配置

日期：2026-10-18。范围：独立 `--media-supervisor` 启动路径。

## 问题与修正

Linux systemd 媒体服务直接进入 supervisor，没有执行普通 Desktop 的配置加载。
Executor 已读取客户 TOML 的 `[executor].data_dir`，supervisor 却仍使用默认
机器数据目录生成本地访问凭据。自定义目录时，两者可能无法完成 helper 身份验证。

supervisor 现在线程、日志和运行目录初始化之前，复用现有 `client::startup`
并选择 `Role::Executor`；默认读取安装位置 TOML，也支持显式 `--config`。
相对路径按配置文件目录解析，客户文件不重写。Operator 的数据目录与偏好不参与。
缺失的显式配置或格式错误立即退出，不创建媒体 runtime 或继续使用默认凭据。

## 验证

使用一次性 Linux 容器运行完整 Desktop 二进制，不连接真实显示会话、服务账号
或用户桌面。测试入口为
[`verify_linux_media_config.py`](../experiments/remote-desktop/verify_linux_media_config.py)。

- 旧候选 `1432cced…`：显式 TOML 指定 Executor 数据目录，实际日志仍落到默认
  `/var/lib/pixels-agent-bridge`，确认旧入口忽略配置。
- 新候选 `e0b2eb95…`：显式路径、安装路径两种启动均使用 Executor 的相对/绝对
  数据目录；Operator 目录未创建，配置内容保持。
- 新候选：显式配置不存在、TOML 格式错误均退出码 1，runtime 未创建。
- 11 项 service-config 回归通过，完整 Linux Desktop release 构建通过
  （Cargo 3 作业，原生 1 作业）。

完整结果和产物哈希见 [JSON](remote-desktop-linux-media-config-2026-10-18.json)。
这是启动配置回归，不代替真实安装升级、自定义目录 helper 接入或全部登录验收。

## 其他发布检查发现

`linux-desktop/postinst`、`unix/install.sh` 和 `client::configure` 仍含硬编码
服务默认地址。它们不满足“公开默认配置为空、可选本地私有编译配置”的完整要求，
须在 LT-09.4 统一处理并验证客户覆盖优先级；本次没有只删除脚本参数而保留
底层默认值，也没有改动主工作区正在修改的配置代码。
