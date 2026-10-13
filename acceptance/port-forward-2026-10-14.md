# TCP 端口转发验收记录（2026-10-14）

已实现 `pab_port_forward_open`、`pab_port_forward_status`、`pab_port_forward_close`。MCP 在本机 `127.0.0.1` 监听，复用已认证设备连接，以独立 QUIC 双向流把原始 TCP 字节送到远端 Executor 可访问的目标。每条连接独立半关闭，双向缓冲各为 16 KiB；状态包含路径、连接数、字节数、错误和错误时间。单 MCP 最多 8 个监听、64 条活动连接，Executor 每会话 64 条、全进程 256 条。

## 安装与版本

| 平台 | 验收安装形态 | 校验 |
| --- | --- | --- |
| Windows x86_64（90，设备码 428358431） | V1.2.97 静默覆盖安装；安装器退出码 0，Executor 服务 Running；安装脚本逐项核对包、Executor、MCP 哈希 | 安装包 SHA-256 `98196bcbc686a9ffb2c7e8aacd450eb9432ea180e75870960ea6325869f0a6bb`；Executor `436aaa7791ed3a3527903322b9303550a237b5a64abecaf5f1d24c4f5fc0d257`；MCP `b7acfe123b7f78bb0f13f642ed324325137a4e63ed92600af9be55651c6cbcdf` |
| macOS aarch64（设备码 419438177） | 同源码编译的 Executor、MCP 安装到现有服务目录；ad-hoc 签名通过 `codesign --verify`；Executor 持续运行 | Executor `1374c11c848ee82079aa59181cdba5a43aeb571b3362e1e0b5b15f7b91ed6a4c`；MCP `9646c9809e6a5c26ce042786420500908b96c9a5bffb9fa0f4868f6d2b5614fc`；已安装 MCP 列出 110 个工具，包含新增 3 项 |
| Linux x86_64 | V1.2.97 正式 tar.gz 包安装进一次性 systemd 容器；Executor active；包内 MCP 列出新增 3 项工具 | 包 SHA-256 `272d4374fc43f85eeea688224d14ec2bcadc452f4d0667a95f9227434a42c212` |

上述包仅用于验收，未作为官网公开更新发布。macOS 验收安装的是组件，没有制作新的发行版 `.pkg`；项目目前不使用 Apple 分发签名。

## 端到端矩阵

每格都通过同一份 [验收脚本](port_forward_live.py)：远端服务只监听 `127.0.0.1:38673/38674`；真实本机 `curl` 访问远端 HTTP；65 次二进制 echo 共 267,555 字节双向精确一致；另有 2,000,003 字节随机数据的双向慢读。64 条连接同时占用时第 65 条被拒，原有连接与命令工具继续工作；8 个监听占满时第 9 个被拒。还验证了端口占用、远端拒绝连接后重试、远端解析 `localhost`、另一个 MCP 会话不可查询映射、显式关闭与重复关闭、端口重绑，以及设备断开后映射失败且重连不复活。Relay 用调试版 MCP 的现有 `PAB_TEST_RELAY_ONLY` 开关强制，正式 Executor 未改网络设置。

| 远端 | 直连 | Relay | echo 请求/响应 SHA-256（直连 / Relay） | 2 MB 请求/响应 SHA-256（直连 / Relay） |
| --- | --- | --- | --- | --- |
| Windows 90 | 通过 | 通过 | `41bcd04aae2fa21ed225af1c7812a34a7b40f020f9761a9165d831d27003ab40` / `e178fc480242b0044bb1d9424c3e4f4e6d75967a7c2e524288370bea2d254ea4` | `40880f6a2433a007c91d6039b632ad874ca190d639df7a354ebe525d3424de1c` / `c176d3a0088b2c598c46eeaf3e16b67d028c2ffe7736ef32b3b792ebabc239c0` |
| macOS | 通过 | 通过 | `d0cd593a28231886f6ff8b4eeff915b293772ba3fbac51cb49e404448c275109` / `bf0ae074c36ee5658cdf5d1333f4661558a8f07ef9c5ab2d4393fabf96726faf` | `9ccd2a9ae913fa439d89b48c433081e9c68a8d80a53c474447c16692df06e761` / `0dee2ae42a878aa4575f1631d6056899153d9bf249aaaa5e0c0eccdd5da17ca5` |
| Linux 容器 | 通过 | 通过 | `a9a6e9691b52e793841a3a1563a277709111fc3efb5e1f9bb59546fc99bf393c` / `3af0db76425137a41b78811fc6d8f22ad2560d67b1f330f845dc1a2c8d275350` | `72140f317af85298f3cb0d9905e66f9b15017cd62b4a15afa238cc307f52dfe0` / `c3dc0b5a23a48ecd98f430ad0c49f1fd3f790087f35fe81a0845188210fed359` |

同目录六份 `port-forward-{windows,macos,linux}-{direct,relay}` 原始结果由 [复核脚本](verify_port_forward_reports.py)逐项检查，全部通过。额外检查：Company 旧 Executor（559038160）明确返回 `unsupported`，不留下监听；90 解析 `pixels-acceptance.invalid` 返回 `DnsFailed`，单连接失败后监听仍可用。对应记录为 `port-forward-old-executor.jsonl`、`port-forward-dns-failure.jsonl`。

## Rust 验证与边界

- Protocol：60 个单元、3 个集成测试通过；Executor：原 177 个单元测试通过、8 个忽略，另新增 100 ms 加速空闲超时测试通过；Bridge：83 个库测试、43 个 MCP 二进制测试通过。编译最多 `-j18`。
- Mac 首次 Relay 慢读曾截断。定位到验收安装的临时 `launchctl submit` 作业每约 10 秒重启 Executor；移除作业后完整重测通过，Executor 后续持续运行。Linux 首轮断开测试发现临时设备未被 `pab_disconnect` 实际断开，已用已认证连接的设备身份修复并重测通过。
- 30 分钟空闲没有按真实时间等待，以 100 ms 加速测试验证超时路径；未人为制造一条正在传输的连接从直连切换到 Relay。状态管理只在已认证连接真正关闭时终止监听，不依据路径名称关闭。
