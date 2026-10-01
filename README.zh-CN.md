# Pixels Agent Bridge

**通过 AI Agent 远程操控你的设备。**

[English](README.md) · 简体中文

Pixels Agent Bridge 通过 Model Context Protocol（MCP），将本机 AI Agent
连接到远程计算机。Agent 可以选择设备、确认操作系统、执行原生命令、传输文件、
使用交互终端，以及调用受支持的桌面能力。桌面应用提供设备管理、任务记录，
并实时展示正在操作设备的 MCP 进程及其状态。

项目处于持续开发阶段。Windows、Linux 的远程命令与文件流程已经完成真机验证；
各平台支持范围与验证状态见下文。

## 目录

- [主要能力](#主要能力)
- [工作流程与组件](#工作流程)
- [快速开始](#快速开始)
- [MCP 工具](#mcp-工具)
- [连接管理与活动展示](#连接管理与活动展示)
- [常见问题](#常见问题)
- [平台与实现状态](#平台与实现状态)
- [开发与构建](#开发与构建)
- [服务端部署](#服务端部署)
- [仓库结构](#仓库结构)

## 主要能力

- **Agent 操作设备**：通过统一的 `pab_*` MCP 工具选择目标并执行操作，已实现 Codex 接入。
- **执行原生命令**：指定程序和参数数组，查询任务状态，读取标准输出和错误输出。
  返回结果携带经过验证的目标操作系统。
- **传输文件**：用绝对路径上传、下载二进制文件，校验完整性，并显式决定是否覆盖。
- **交互终端**：打开远程终端、发送输入、读取输出、调整尺寸和关闭会话。
- **桌面能力**：在受支持的桌面会话中列出窗口、向 Agent 返回有界 JPEG 预览或保存 PNG 原图，
  发送鼠标、键盘或 Windows 安全注意序列事件。
- **设备管理**：保存历史设备、重命名、复制信息，分别显示在线状态和连接状态；
  已连接卡片展示当前使用 P2P 还是 Relay。
- **任务记录**：按设备筛选、分页查看，也可在单个设备面板中查看该设备的历史任务。
- **MCP 活动**：在设置中查看进程、客户端身份、设备连接、工具调用、任务和传输摘要。
- **自部署**：使用 Docker Compose 部署控制服务、PostgreSQL 和 Relay。

桌面支持亮色、暗色主题，以及简体中文、繁体中文和英文。
首次启动根据系统语言选择，用户手动切换后保存选择。

## 工作流程

![Pixels Agent Bridge 工作流程动画](diagram/workflow/agent-workflow.svg)

[打开 SVG](diagram/workflow/agent-workflow.svg) ·
[查看静态 PNG](diagram/workflow/agent-workflow@2x.png)

在浏览器中打开 SVG 可以观看动画，不支持动画的图片预览仍可以显示完整架构。

1. **发起请求**：AI 客户端通过本机 `pab-mcp` 调用 Pixels 工具，当前 MCP 传输使用 stdio。
2. **寻址与认证**：Bridge 确认设备并认证访问；控制服务提供设备身份、在线状态、
   授权后的网络地址和 Relay 策略。
3. **建立连接**：iroh 使用 P2P 或 Relay 回退建立设备连接，网络变化时路径可以切换。
   动画依次展示两种路径，实际传输根据连接情况选择。
4. **执行操作**：目标 Executor 在设备的原生环境中处理命令、文件、终端和桌面请求。
5. **返回结果**：Agent 查询任务状态、进度和输出；任务记录与 MCP 上报让 Desktop 展示执行情况。

**控制服务负责协调接入，操作数据通过设备连接传输，使用 P2P 或 Relay。**
每个 MCP 进程有自己的 Bridge Runtime 和设备连接；Desktop 接收状态并读取任务记录，
不会作为所有 Agent 共用的执行代理。

### 组件职责

| 组件 | 职责 |
|---|---|
| `pab-desktop` | Tauri 界面、设备管理、远程工具、任务记录和 MCP 活动展示 |
| `pab-mcp` | 向 AI Agent 提供设备工具的本机 stdio MCP 服务 |
| `pab-bridge` | 操作端 Runtime、远程操作和本地持久化共享 Rust 库，也提供开发用 CLI |
| `pab-executor` | 目标设备后台服务，认证连接方并执行操作 |
| `pab-server` | 账号、设备、端点、权限和 Relay 策略的 TLS 控制服务 |
| `pab-relay-server` | 按授权转发数据，执行配置的带宽限制 |
| PostgreSQL | 中央账号、设备、成员关系及授权数据 |
| SQLite | 本机设备身份、凭据、任务和操作记录 |

Windows 安装还包含桌面会话辅助进程，提供需要交互桌面或安全桌面的能力，
职责与可见的主界面窗口不同。

## 快速开始

### 1. 安装完整客户端

在操作端和目标计算机安装完整桌面包。Windows 包包含 Desktop、Executor、MCP
和安装脚本，EXE 安装程序启动后台服务并创建快捷方式。

安装需要部署 UUID、WSS 控制地址和 HTTPS Relay 地址，需要互相通信的计算机
应使用同一部署。自部署先完成[服务端部署](#服务端部署)。
安装包可按下文的[构建说明](#构建-windows-安装包)生成。

Windows 无人值守访问由机器级服务承担，被操作端无需一直打开桌面主窗口。

### 2. 保存设备连接

1. 在目标计算机的“我的设备”页面查看九位设备码和密码。
2. 在操作端填写对方设备码及密码。
3. 成功连接一次，将设备和凭据保存到本机用户的 Bridge 数据库。
4. 选择已保存的设备，查看基本信息、任务记录或远程工具；双击设备 item 建立连接。

界面将设备码分组显示。MCP 参数和复制的 ID 使用不带空格的九位数字，例如 `123456789`。

### 3. 启用 Codex

先为当前系统用户安装 Codex CLI，并确保 `PATH` 中可以找到它。
在“设置 → AI Agent”中启用 Codex。应用验证同包 MCP 程序，为当前系统用户注册
`pixels`，并配置工具直接执行，不逐项弹出审批提示。启用后重新启动 Codex 加载配置。

也可手动写入 Codex 配置；Windows 示例，程序路径按实际安装位置调整：

```toml
[mcp_servers.pixels]
command = "C:\\Program Files\\PixelsAgentBridge\\pab-mcp.exe"
default_tools_approval_mode = "approve"
```

`approve` 表示预先批准工具调用；`auto` 仍可能根据工具声明触发审批。
远端设备认证和操作系统权限仍然有效。

Linux、macOS 手动注册安装目录中的 `run-mcp.sh`，使其加载部署配置。
其他 MCP 客户端可以接入 stdio 入口，其自动配置及兼容性尚未完成与 Codex 同等程度的验证。

### 4. 让 Agent 操作设备

示例提示：

> 连接设备 123456789，先确认操作系统，再查看系统盘的剩余空间。
> 使用 Pixels 工具，并根据目标操作系统选择命令。

> 将 C:\build\app.zip 上传到设备 123456789 的 C:\Users\Operator\Desktop\app.zip，
> 覆盖现有文件，上传后校验 SHA-256。

将设备码和路径替换为自己的实际信息。

## MCP 工具

当前源码提供 **48 个工具**。宿主可能显示命名空间，例如 `pixels.pab_connect`。

| 工具 | 用途 |
|---|---|
| `pab_list_devices` | 列出本机保存的设备，控制服务离线时也可读取 |
| `pab_connect` | 认证目标设备，返回经过验证的操作系统和执行环境 |
| `pab_run_command` | 按程序和参数数组启动原生程序 |
| `pab_get_task` | 查询状态、进度、完成信息和输出范围 |
| `pab_read_output` | 按偏移读取保存的 stdout 或 stderr |
| `pab_list_directory` | 分页列出目录内容 |
| `pab_list_windows` | 获取包含窗口引用、位置、尺寸、PID、显示器和状态的有限快照 |
| `pab_list_monitors` | 列出显示器 ID、原点、尺寸、主屏、缩放和旋转 |
| `pab_focus_window` | 聚焦指定引用窗口，遵守系统前台限制 |
| `pab_window_control` | 最小化、最大化、还原或请求正常关闭窗口 |
| `pab_type_text` | 向明确指定的前台窗口输入 Unicode 文字 |
| `pab_capture_screenshot` | 返回有界 JPEG/PNG 预览图片，或保存原图，附带尺寸、格式和哈希 |
| `pab_desktop_input` | 发送受支持的鼠标、键盘或安全注意序列事件 |
| `pab_open_terminal` | 打开交互终端 |
| `pab_terminal_input` | 发送终端输入 |
| `pab_terminal_read` | 读取终端输出 |
| `pab_terminal_resize` | 调整终端尺寸 |
| `pab_terminal_close` | 关闭终端会话 |
| `pab_upload_file` | 异步上传二进制文件，校验后可显式覆盖已有普通文件 |
| `pab_download_file` | 异步下载二进制文件，校验后可显式覆盖已有普通文件 |
| `pab_get_operation` | 按原请求 ID 读取持久化的传输、命令、文件操作或系统查询结果 |
| `pab_list_operations` | 列出本 MCP 的传输、命令、文件操作和系统查询，支持设备/状态过滤及游标 |
| `pab_cancel_operation` | 请求取消操作，查询原请求以确认结果 |
| `pab_disconnect` | 断开本 MCP 与设备的连接，与 Desktop 和其他 MCP 相互独立 |
| `pab_file_stat` | 查看目标文件、目录和链接的元数据 |
| `pab_file_read` | 按行或字节范围读取文本，返回 SHA-256 版本 |
| `pab_file_write` | 暂存后发布文本，显式覆盖，可检查原文件版本 |
| `pab_file_patch` | 核对原版本后进行精确替换 |
| `pab_file_search` | 按名称或文本搜索，支持 glob 和明确的结果上限 |
| `pab_file_hash` | 后台分块计算 SHA-256，支持进度查询及取消 |
| `pab_mkdir` | 创建目录，显式控制父目录和已存在行为，报告部分结果 |
| `pab_file_copy` | 目标机内复制文件或显式递归目录 |
| `pab_file_move` | 复制并核对目标后移除源，支持跨文件系统 |
| `pab_file_delete` | 删除有界、显式递归的计划项，报告实际删除结果 |
| `pab_archive_create` | 按明确源路径生成 ZIP，校验后发布 |
| `pab_archive_extract` | 解压 ZIP，预检路径、类型、冲突和限额，逐文件校验 |
| `pab_system_info` | 查询主机、OS、CPU、内存/Swap 和 Executor 身份，可选 NVIDIA 显卡信息 |
| `pab_list_disks` | 查询磁盘/挂载点、文件系统、容量和可用空间 |
| `pab_list_processes` | 一次采集有界进程列表，支持 PID/名称/用户过滤，不实时分页 |
| `pab_get_process` | 查询指定 PID 当前可获得的身份和资源信息 |
| `pab_list_network_interfaces` | 查询网卡地址、MAC、MTU、状态和累计收发字节 |
| `pab_list_network_connections` | 查询 TCP/UDP、监听端口和可见 PID，支持地址/端口/状态过滤 |
| `pab_resolve_dns` | 通过目标机器配置的 DNS 服务器查询记录 |
| `pab_list_sessions` | 通过 Windows WTS 或 Linux logind 查询 OS 登录会话 |
| `pab_terminate_process` | 核对原生进程身份后请求退出，可显式允许超时强制终止 |
| `pab_list_services` | 按名称和状态查询 Windows SCM / Linux systemd 服务 |
| `pab_get_service` | 查询指定服务的状态、启动方式和运行信息 |
| `pab_service_control` | 异步启动、停止、重启服务或修改启用/禁用配置 |

先调用 `pab_connect` 并保留目标环境。大部分设备工具需要 `device_code`；
终端后续操作使用打开终端时返回的 `session_id`。
密码从本机 Bridge 数据库读取，不作为工具参数传递。

进程终止和服务管理要求 system-query 能力版本 3。先用 `pab_get_process` 获取 `termination_identity`，再传入 `pab_terminate_process` 的 `identity`；不能用秒级启动时间代替。Windows 使用原生创建时间并在操作期间持有同一个进程句柄。Linux 持有原进程的 pidfd，身份租约有效 10 分钟、最多 256 个；过期或 Executor 重启后需要重新查询，不回退到按 PID 发信号。

终止默认 `force=false`、等待 5000 ms。Windows 对顶层窗口发送 WM_CLOSE；无窗口进程明确返回不支持正常退出，显式 `force=true` 才直接强制终止。Linux 先发 SIGTERM，超时且 `force=true` 才发 SIGKILL；强制后的退出确认另有最多 5 秒等待。只操作一个进程，不终止进程树，不允许终止 Executor 自身或系统 init。

服务查询使用 Windows SCM 或 Linux systemd 系统总线，Linux 要求完整 `.service` 名称。列表按名称字面子串、状态精确过滤，不实时分页；列表是摘要，详细配置使用 `pab_get_service`。Windows 列表不含内核驱动，Linux 会包含未加载的已安装服务（`not_loaded`）。启用/禁用只改启动配置，Windows 启用设为自动启动；Linux 修改持久化 unit 链接，不强制解除屏蔽。启动、停止、重启等待实际状态，Linux 还核对原 job 完成信号。Windows 不额外停止依赖服务；systemd 仍可能执行 unit 定义的依赖事务。不能停止或重启 Executor 自己所在的服务。

这两个控制工具是异步操作：约 250 ms 后仍未完成就返回 `running`，使用原 `request_id` 和 `pab_get_operation` 查询结果。相同 ID 永远不重放，断开或调用方取消不会撤销已提交动作，服务超时也不回滚。结果保留阶段、是否提交过修改、Linux job 路径、观测状态和错误；未确认不等于未执行。运行中目前只报告总状态，细分阶段在最终结果中返回。相同资源的并发控制返回 `resource_busy`；不增加审批流程，OS 权限错误直接返回。服务控制超时默认 30000 ms，范围 100–60000 ms；原生阻塞 SCM 调用不能硬中断。Windows 专用窗口、临时服务及隔离 QUIC 验证已通过，Linux 原生后端已交叉编译检查，Linux 真机及安装后验收仍待完成。

系统查询使用 `sysinfo`，要求目标支持 system-query 能力版本 1。每次返回带采集起止时间的一次结果，不持续监控，也不是 OS 原子快照。列表默认 100 项，最多 1000 项，同时受 32 KiB 实际序列化预算限制；`truncated=true` 时使用过滤缩小范围，不做实时分页。可选字段不可读取时返回 null，不采集进程命令参数和环境变量。

`sample_cpu` 在系统信息中默认开启，在进程查询中默认关闭。开启后按库的最小间隔采样两次，`cpu_sample_ms` 表示区间；`cpu_usage_basis_points` 中 10000 等于 100%，进程跨核 CPU 可超过 100%。容量单位为字节；频率是第一个逻辑 CPU 的 MHz，不是所有核心的平均值。网卡返回库提供的累计计数，不是瞬时速度。

`include_gpu=true` 时通过动态加载的 `nvml-wrapper` 查询 NVIDIA。GPU 分区单独报告状态与字段错误；NVML 不可用不能解释为没有显卡。AMD/Intel 后端尚未实现。系统查询等待完成，暂不支持取消；阻塞 OS/驱动调用没有硬中断期限，后台准入有界，采集器忙时返回 `executor_busy`。每个 MCP 最多 16 个未解决的系统查询。相同 `request_id` 通过 `pab_get_operation` 读取原采样，省略 ID 才产生新采样；中断或结果未确认时不自动重跑。Desktop 任务记录显示查询类型和返回项数。

这组工具已通过 Windows 本地测试，包含真实进程生命周期、结果持久化和隔离 QUIC。安装后的宿主、Windows/Linux 双机和完整 NVIDIA 硬件验收仍待完成；现有安装包不含本批改动。

网络连接、DNS 和 OS 会话查询要求 system-query 能力版本 2，C1 工具继续兼容版本 1。复用原采样持久化、输出预算、任务记录和原 ID 查询规则。

连接表使用 `netstat2`，支持 TCP/UDP、IPv4/IPv6、精确 IP/端口/PID 和 TCP 状态过滤，不做实时分页。UDP 没有可观察的远端及状态；TCP 监听条目的远端为 null，空 PID 列表表示未观察到归属。扫描受到 100000 项和软性 5 秒期限约束，截断明确报告；这不能硬中断库的阻塞枚举调用。

DNS 使用 `hickory-resolver`，每次读取目标系统 DNS 配置，不回退公共 DNS、不使用本地解析缓存。默认 A，另支持 AAAA/CNAME/MX/NS/PTR/SOA/SRV/TXT；PTR 可输入 IP 或反向域名，域名使用 ASCII/IDNA。返回名称、类型、TTL 和 DNS 展示文本，长值有 `value_truncated` 标记。查询超时默认 5000 ms，范围 100–10000；读取系统配置属于另行完成的原生 I/O。这是 DNS 查询，不等同于系统原生解析器：不读取 hosts，不处理 mDNS 或 Windows NRPT/VPN 分流策略，上游 DNS 仍可能有缓存。

会话查询列出 OS 会话，不是账户、MCP 会话或 PAB 终端。Windows WTS 可以包含尚无登录用户的服务/监听会话。Linux 通过系统总线访问 logind，采集期限为 5 秒；logind 缺失、访问失败或不支持的平台明确失败，可选字段失败写入每项 `errors`。用户名和状态过滤均为精确匹配。这三个工具已通过 Windows 本地测试、本地 DNS 替身和隔离 QUIC；Linux 采集库交叉编译检查通过。Linux/macOS 运行时和安装后的宿主验收仍待完成。

文本工具要求目标 Executor 也升级。支持 UTF-8、UTF-16 和 BOM 检测，不静默替换
无法解码的字节。文件上限 4 MiB，单次最多返回 16 KiB UTF-8 文本，写入及补丁
输入上限 128 KiB。继续读取时使用 `next_offset`，把返回的 `metadata.sha256`
作为 `expected_hash`。补丁的每项包含 `find`、`replace` 和 `expected_matches`
（默认 1），全部针对原文匹配，不允许重叠。写入与修改返回 `operation_ref`，
重复提交沿用 `request_id`；结果未确认时用 `pab_get_operation` 查询原请求。
这一组有界文本操作等待完成返回，暂不支持取消。文件正文使用设备二进制通道，
不写入任务记录。版本检查与 PAB 路径锁不等于针对外部编辑器的操作系统级原子 CAS。

搜索、Hash 和创建目录要求目标 Executor 的文件能力版本为 2，原有文本工具仍兼容版本 1。搜索使用字面子串，不是正则表达式；默认按名称、区分大小写，glob 使用 `/` 分隔的相对路径，例如 `**/*.rs`。会包含隐藏文件，不应用 gitignore。每次最多返回 100 项；内容匹配返回行号、最多 160 字符的行首预览和该文件的 SHA-256。扫描受到 4096 项、64 MiB 读取计费预算、5 秒和输出字节上限约束；通过 `truncated`、`stop_reason`、跳过数和有限警告说明不完整结果，不提供变化中目录的实时分页。

`pab_file_hash` 在远端确认接收后返回 `operation_ref`，后台按 256 KiB 分块计算，不全量载入大文件。使用 `pab_get_operation` 查询 `progress.completed_bytes`、`progress.total_bytes` 和最终 `metadata.sha256`；使用 `pab_cancel_operation` 请求停止。只有 `cancelled` 才确认已停止。Executor 同时最多 4 个 Hash 作业，每个最多 30 分钟，单次读取超时为 30 秒。相同 `request_id` 返回原操作，文件后续变化也不会触发重算。观察到大小、修改时间或可用身份变化时明确失败，不宣称提供外部并发写入下的原子快照。MCP 会周期刷新活动记录；离线查询可能返回最后保存的状态，应结合进度时间判断新鲜度。

`pab_mkdir` 默认 `parents=false`、`exist_ok=false`，通过 `created_paths` 报告实际创建目录及失败前的部分结果，不自动回滚。结果不明时查询原 ID，不自动重放。每个 MCP 最多保留 32 个未解决的活动文件操作，达到上限应先核对已有结果。上述工具不主动跟随符号链接或 Windows reparse 点；路径复核及 PAB 自身锁不能消除外部程序的全部并发竞态。

复制、移动、删除和 ZIP 要求目标 Executor 的文件能力版本至少为 3。远端接收后返回 `operation_ref`，随后后台执行；通过既有操作工具查询或取消。源和目标是精确路径，不自动附加文件名。默认 `recursive=false`、`overwrite=false`；目录复制/移动显式覆盖时可合并目录，保留目标中的无关条目。移动在同盘和跨盘都采用复制、校验、删源的顺序，需要额外 I/O 和暂存空间。

后台使用 64 KiB 缓冲区，Executor 同时最多 4 个批量作业，协作式期限为 30 分钟。默认 4096 项、1 GiB 文件数据、64 层；`max_bytes` 最大可设 8 GiB。解压的项目限额包含隐含父目录和目标根目录；ZIP 输入和暂存最多为 `max_bytes + 2 MiB`，中央目录元数据最多 2 MiB。只解压未加密的 Stored/Deflate ZIP，路径必须是 UTF-8 可移植名称；拒绝链接、越界、大小写重名和文件/目录冲突，逐文件通过 CRC、长度和暂存 hash 后才发布。`max_ratio` 默认 200，范围 1–1000；高压缩率的正常压缩包也可能因限额被拒绝。

`mutation` 返回阶段、计划/处理项数、写入/删除数、`partial`、`source_removed` 和有限逐项结果（64 项或 8 KiB）。取消在检查点停止，阻塞 OS I/O 可能延迟停止；只有 `cancelled` 确认 worker 已退出。失败或取消保留已生效项，包括后续 ZIP 条目 CRC 损坏前已解出的文件，不自动回滚。删除只移除计划项，拒绝盘符/根目录。结果未确认时查询原 ID，不重新执行。PAB 路径锁覆盖祖先和子路径，但不提供对外部程序的原子目录操作；ZIP 不保留 ACL、属主和扩展元数据。

新文件工具已完成 Windows 本地自动化验证，包括隔离 QUIC 与 MCP stdio。安装后的宿主调用、物理跨盘和 Windows/Linux 双机验收仍待完成；旧安装包不含这些新增工具。

### 窗口引用、控制与 Unicode 输入

显示器与窗口枚举复用 [xcap](https://github.com/nashaofu/xcap)，文字输入复用
[Enigo](https://github.com/enigo-rs/enigo)，外部窗口操作使用 Windows 官方绑定或
[x11rb](https://github.com/psychon/x11rb) 的标准 EWMH 消息。这些工具要求 Executor
系统能力 v4 和新版活动桌面 helper。Windows 与 Linux/X11 适配已实现；Linux 图形环境
实机验收待补。Wayland 明确返回不支持。macOS 可以通过 xcap 枚举，暂不支持窗口控制和
文字输入，也尚未在 Mac 上验证。

先用 `pab_list_windows` 获取 `window_ref`，再对该引用执行操作。输入前先调用
`pab_focus_window`。窗口列表最多 64 个，显示器最多 32 个，响应最多 32 KiB，超限明确
标记截断。枚举遵循 xcap 的筛选规则（Windows 排除 helper 自身进程及隐藏窗口等），
采样中消失或无法读取属性的条目跳过。坐标采用 xcap 原生坐标，不能直接当作缩略截图像素坐标。引用仅属于当前
helper 连接；重连、切换桌面、窗口销毁或身份标记丢失后失效。执行前校验 PID 与窗口
身份标记，但外部应用仍可在校验与操作之间改变窗口或焦点，这不是原子窗口锁。

文字限制为 1–4096 个 UTF-8 字节，不允许 NUL，不替换剪贴板。输入 API 接受不代表应用
最终文本正确；焦点变化或应用行为可能造成部分输入。关闭只发送正常关闭请求，不结束
进程；保存提示框或应用拒绝关闭时，会返回失败并标明副作用尚未确认。

保留 `request_id` / `operation_ref`，同一个请求不会重复执行。慢操作先返回 `running`，
断线后通过 `pab_get_operation` 获取原结果。已派发操作丢失 helper 后标记未确认，Executor
重启后也不重放已接受的修改操作。这些操作不能取消或撤销。持久化记录仅保存输入文字的
BLAKE3 摘要，不保存原文；摘要不是加密，不能防止对可预测文字的猜测。目标应用和 AI
宿主仍可能保留输入文字。

### 截图预览与原图

截图复用 [xcap](https://github.com/nashaofu/xcap) 采集画面，使用
[image](https://github.com/image-rs/image) 缩放、编码 JPEG/PNG 和有界解码校验，不自行实现截图引擎。

`pab_capture_screenshot` 默认 `mode="preview"`：JPEG，尺寸不超过 1600×1000，
编码后不超过 512 KiB。质量从 75 开始，必要时降至 45，再缩小尺寸满足预算。
返回 MCP 图片内容、已保存的文件和元数据；Base64 只出现在图片块中，不重复写入文本、
结构化结果、SQLite 或活动上报。

```json
{"device_code":"214601537"}
```

`mode="original"` 默认 PNG，保留采集尺寸，编码预算 8 MiB。显式指定 JPEG 原图时，
默认质量为 85。支持 `format`、仅 JPEG 的 `quality`、`max_bytes`、仅预览模式的
`max_width/max_height`、`monitor_id` 和相对所选屏幕的 `region`。区域越界直接失败，
不偷偷裁剪。采集最多 16 Mi 像素，单边最多 16384 像素。每次调用采集新画面；
同帧高清区域和窗口截图尚未实现。

```json
{"device_code":"214601537","mode":"original","destination":"C:\\Temp\\screen.png","include_image":false}
```

`destination` 可省略。指定时必须为绝对路径，扩展名与图片格式一致；不覆盖已有文件。
编码图片超过 512 KiB 时只保存文件，明确返回 `image_omitted_reason`，不输出过大的内联图片。
`include_image=false` 可显式只要文件。元数据包含采集时间、屏幕标识、全局区域原点、
源尺寸和编码尺寸、实际 JPEG 质量、字节数和 SHA-256。传输沿用二进制帧，校验实际编码、
长度、尺寸和哈希。

新选项要求升级目标 Executor 与桌面助手。旧端明确返回升级提示，不降级成其他屏幕或忽略区域。
旧客户端的主屏 PNG 请求继续保留。当前已验证 Windows 编译、合成图片、隔离助手/QUIC
及 MCP 响应；安装后宿主图片显示、原生 4K/多屏、macOS/Linux 图形机仍待验收。
本阶段产品接口明确不支持 Wayland。

### 示例：执行命令与读取结果

以下 JSON 是工具参数，不是终端命令。

调用 `pab_connect`：

```json
{ "device_code": "123456789" }
```

确认目标为 Windows 后，调用 `pab_run_command`：

```json
{
  "device_code": "123456789",
  "program": "powershell.exe",
  "args": ["-NoProfile", "-NonInteractive", "-Command", "Get-PSDrive -PSProvider FileSystem"]
}
```

Linux 目标可以使用 `program: "df"` 和 `args: ["-h"]`。
Bridge 不会隐式添加 Shell，使用 Shell 语法时需要明确指定解释器。

将命令结果中的 `task.task_ref.task_id` 和 `device_code` 传给 `pab_get_task`，
查询至任务完成。再调用 `pab_read_output` 读取标准输出：

```json
{
  "device_code": "123456789",
  "task_id": "TASK_ID_FROM_THE_COMMAND_RESULT",
  "stream": "stdout",
  "offset": 0
}
```

### 示例：异步传输

上传下载立即返回 `operation_ref`，后台继续连接和传输。可选 `wait_ms` 最多等待
5000 毫秒，它不会限制传输时长。两端路径均须使用各自系统的绝对路径，最多 4096 个字符。

```json
{
  "device_code": "123456789",
  "source": "C:\\work\\artifact.zip",
  "destination": "C:\\incoming\\artifact.zip",
  "overwrite": true,
  "request_id": "c746c0d6-f349-4e4d-92fa-9e3fb25abcf4"
}
```

将返回的引用传给 `pab_get_operation`，取消时使用相同参数调用 `pab_cancel_operation`：

```json
{ "device_code": "123456789", "operation_id": "c746c0d6-f349-4e4d-92fa-9e3fb25abcf4" }
```

相同 `request_id` 和参数会返回原记录，完成后或重启后重复提交也不会再次执行。
省略该参数会生成新 ID。`cancel_requested` 表示取消意图，`cancelled` 才表示已确认停止；
已经发布的文件仍为完成。`unconfirmed` 表示正在核对原请求，不能改用新 ID 重做。
每个 MCP 同时最多接受 8 个结果尚未确认的传输。传输须结束或核对出结果后再断开；
断开后的设备需要显式调用 `pab_connect` 重新连接。已退出会话的记录可查询，但其他会话不能取消。
缓存的系统环境会标记为 `remembered_device`。

重新构建安装并重启 AI 客户端后才能加载新工具。旧 MCP 安装包保留原工具集和行为；
使用文本工具还需要升级目标机器上的 Executor。

## 连接管理与活动展示

Desktop 和每个 MCP 进程分别维护设备连接。在 Desktop 中断开设备，
不会自动断开 Agent 的 MCP 连接；多个 Agent 也分别拥有自己的 Runtime。

Desktop 主进程在 `0.0.0.0:26035` 提供 Axum 状态服务：

| 入口 | 用途 |
|---|---|
| `GET /health` | 上报服务健康检查 |
| `GET /api/mcp` | 查询当前 MCP 进程快照 |
| `WS /ws/mcp` | MCP 注册、心跳和快照上报 |

MCP 通常连接 `ws://127.0.0.1:26035/ws/mcp`，上报客户端身份、进程与会话、
设备码及名称、连接阶段、P2P／Relay 路径，以及任务和传输摘要。
上报不包含密码、密钥、原始命令参数或命令输出；任务详情持久化在 SQLite 中。

该监听服务当前没有认证，用于状态汇总，不是 HTTP MCP 工具执行入口。
Desktop 重开后 MCP 会重新上报；状态服务不可用不会阻止 MCP 操作设备。

## 常见问题

**在线与已连接有什么区别？**

在线表示设备当前出现在控制服务中；已连接表示当前操作端 Runtime 已建立设备连接并完成认证。
设备在线时，Desktop 仍然可以处于未连接状态。

**Desktop 显示未连接，为什么 Agent 仍然可以操作？**

`pab_connect` 使用保存的凭据，为该 MCP 进程建立或复用连接。
Desktop 的连接状态属于自己的 Runtime，可以在“设置 → AI Agent → MCP 连接”查看 Agent 的活动。

**多个 Agent 可以同时操作同一台机器吗？**

它们分别使用自己的连接、任务 ID 和终端会话 ID，但操作仍发生在同一台计算机上。
同时修改同一个文件、向同一个桌面发送输入，可能互相影响；使用不同文件路径，并协调共享资源的操作。

**MCP 程序意外退出后怎么办？**

宿主支持重启 MCP 连接时使用其重启功能，否则重新打开 Agent 会话。
单独启动另一个 `pab-mcp` 进程，无法接回已经断开的 stdio 流，宿主自动恢复仍在研究中。

## 平台与实现状态

| 范围 | 当前状态 |
|---|---|
| Windows 桌面、命令、文件和终端 | 已实现，核心远程流程完成真机验证 |
| Windows 截图和输入 | 已验证登录后桌面，也验证了未登录截图和安全注意序列 |
| Linux 命令和文件 | 已完成真实远程 Linux 设备验证 |
| Linux 图形桌面 | 已有实现组件，仍需图形真机验证；输入需要 X11 |
| macOS | 已有安装模板及平台代码，尚未完成完整真机验证 |
| Codex | 已验证一键注册和真实工具调用 |
| 其他 MCP 宿主 | 可手动接入 stdio，仍需逐宿主验证 |

当前不包含连续远程视频流、共享 Bridge Host 或 HTTP MCP 执行入口。
stdio 进程意外退出后的恢复依赖宿主，自动恢复仍在研究中。

## 开发与构建

### 环境要求

- Rust **1.95.0**，由 `rust-toolchain.toml` 选择。
- Node.js **20.x 的 20.19+，或 22.12+**，对应当前 Vite 要求，以及 npm。
- Python 3，用于打包和仓库脚本。
- 平台对应的 Tauri 依赖：Windows 需要 MSVC 构建工具和 WebView2，
  Linux 依赖示例见 `packaging/desktop/Dockerfile.linux`。
- Docker 和 Compose，用于服务端部署或 Linux 构建。
- 生成 Windows EXE 安装包时，将 NSIS **3.12** 放在 `tools/nsis`。

### 启动开发窗口

```sh
git clone git@github.com:PixelsCloud/PixelsAgentBridge.git
cd PixelsAgentBridge/apps/desktop
npm ci
npm run dev:desktop
```

原生 Tauri 窗口支持前端热更新。开发命令关闭 Rust 自动监听，修改后端后需要重启。
在普通浏览器预览页面不会获得 Tauri 原生设备接口。

### 构建 Windows 安装包

在仓库根目录编译核心 Debug 程序：

```powershell
cargo build --locked -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp
```

在 `apps/desktop` 编译 Desktop：

```powershell
npm ci
.\node_modules\.bin\tauri.cmd build --debug --no-bundle
```

返回仓库根目录生成完整归档和安装包：

```powershell
python packaging/desktop/build.py --platform windows --profile debug
python packaging/desktop/build_nsis.py --profile debug --deployment-id "YOUR_DEPLOYMENT_UUID" --control-url "wss://control.example.com/control" --relay-url "https://relay.example.com"
```

执行前替换部署信息。输出位于 `.build/packages/`，包含
`pixels-agent-bridge-windows-x86_64-debug-setup.exe` 和校验清单。
安装、升级测试使用完整包；Release 打包需要对应程序和明确的 profile。

### 检查命令

桌面 Rust 项目独立于核心 Cargo 工作空间：

```sh
python scripts/verify_iroh_vendor.py
cargo test --locked --workspace
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
cargo clippy --locked --workspace --all-targets -- -D warnings
```

在 `apps/desktop` 执行 `npm run build` 检查前端。PostgreSQL 集成测试需要通过
`DATABASE_URL` 配置可丢弃的测试数据库；依赖特定环境或标记 ignored 的测试需额外准备。
这些是检查入口，不表示所有平台或可选测试均已通过。

## 服务端部署

Compose 包含 PostgreSQL 17、控制后端和 Relay。启动前：

1. 将 `packaging/docker/example.env` 复制为 `packaging/docker/private.env`。
2. 设置部署 UUID，并在该部署生命周期内保持不变。
3. 配置数据库密码、域名、证书路径和端口映射。
4. 准备与域名匹配的后端、Relay TLS 证书，以及需要的 CA 文件。
5. 创建环境配置指定的私有 Relay 控制密钥文件。

在仓库根目录运行：

```sh
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml build
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml ps
```

后端和 Relay HTTPS 端口默认监听回环地址，便于接入反向代理；Relay QUIC 发布 UDP 7842。
PostgreSQL 位于私有网络，命名卷在普通容器替换后保留数据。
`PAB_RELAY_IMAGE` 可独立指定 Relay 镜像。

证书文件名、密钥、持久化和配置细节见 [Docker 部署说明](packaging/docker/README.md)。

## 仓库结构

| 路径 | 内容 |
|---|---|
| `apps/desktop` | React、TypeScript、Ant Design 和 Tauri 应用 |
| `crates/bridge` | 操作端 Runtime、持久化、CLI 和 MCP 工具 |
| `crates/executor` | 目标服务及远程能力 |
| `crates/server` | 中央控制服务和 PostgreSQL 数据模型 |
| `crates/relay` | Relay 及策略同步 |
| `crates/agent-core`、`protocol`、`transport`、`task-runtime` | 身份、协议、网络和任务基础模块 |
| `crates/platform`、`terminal`、`windows-sas` | 原生平台、终端和 Windows 安全注意序列 |
| `packaging` | 桌面安装包及 Docker 部署 |
| `patches/iroh-relay`、`vendor/iroh-relay` | 固定版本 Relay 补丁、校验资料和源码 |
| `diagram/workflow` | 对外工作流程 SVG 和 PNG |

更多说明：[开发文档](DEVELOPMENT.md)、[桌面应用](apps/desktop/README.md)、
[桌面打包](packaging/desktop/README.md)。

反馈或贡献时，请说明平台、操作来自 Desktop 还是 MCP、复现步骤，并提供移除凭据后的日志。
[提交 Issue](https://github.com/PixelsCloud/PixelsAgentBridge/issues)。
