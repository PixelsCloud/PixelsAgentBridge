# 编码与桌面兼容 Release 交付记录

本文保留首次交付时的事实；重载后新增参数已验收，Mac 包已更新到 1.2.43，
当前结果和产物哈希见 [后续复验记录](compatibility-session-resume-2026-10-08.md)。

本轮计划：[COMPATIBILITY_RELEASE_PLAN.md](../COMPATIBILITY_RELEASE_PLAN.md)。
Windows/Linux Release 1.2.41；macOS 双架构 Release 1.2.42。
Mac 的 1.2.40 是首次候选构建，随后补入输出末段刷新修复，按规则再次编译为 1.2.42。
源码版本和每个平台安装版本分别记录，不把所有设备描述成同一版本。

## 源码验证

- Windows 与 Mac：共享输出解码 4 项、MCP 40 项通过。
- Linux：共享输出解码 4 项、MCP 40 项、协议 51 项通过。
- Windows 原生桌面库 40 passed、5 个专用原生测试 ignored；Windows/Mac helper IPC 各 22 passed。
- 浏览器全套 21 项通过；随后新增完成通知与读取竞态测试，与编码切换测试共 2 项通过；TypeScript 检查通过。
- 构建版本 15 项、Linux 包 1 项通过；跨平台包测试 5 passed、2 个 Mac 原生项 skipped。
  旧跨平台测试错误地要求 Linux 含 Desktop，已修正为无界面组件名单。
- Mac 固定签名原生测试 2 passed：跨构建保持指定证书约束，错误证书和篡改文件拒绝。

## 已安装远端

| 设备 | 版本 | 结果 |
|---|---|---|
| 90 / 211399447 | 1.2.41 Release | 安装退出 0；三个二进制哈希一致；服务运行；设备身份及抽查任务记录、事件、输出保持 |
| Mac / 603527578 | 1.2.42 ARM Release | 安装退出 0；完整 App/程序哈希一致；固定证书验证；设备身份和任务记录保持；无重新授权操作 |
| Linux / 565893930 | 1.2.41 Release | 安装退出 0；两个程序哈希一致；设备身份、访问凭据、记录/事件/输出保持；新心跳已认证 |
| 本机 Windows | 1.2.41 Release | 安装退出 0；三个程序哈希一致；服务运行且新心跳已认证；设备身份、已保存设备及凭据、抽查任务和事件保持 |

Windows 普通账户 SID 尾号 1099、Medium 完整性：升级前后命令和 PTY 身份一致；自有目录中文文件
内容及所有者保持；程序目录写入 access_denied，不回退 SYSTEM。该账户当前没有可用交互桌面，
普通用户应用交互、UAC 和多活动 WTS 没有冒充通过。

Linux 普通用户 pabuser1 升级后实际 uid/gid 23001、home /home/pabuser1，UTF-8 中文/emoji 输出正确。
独立临时容器直接使用最终 Release tar，手动模式和真实 systemd 模式的新装、重装升级、服务启停、
卸载、数据保留、忽略 TERM 后终止均通过；安装目录外同名进程未被停止。临时容器已删除。
Windows/macOS 未做破坏现有系统服务的“隔离目录卸载”，没有声称其全新系统安装/卸载已完成。

## Mac 文稿保存的实际边界

1. 升级前后均能实际截图和查询 AXTextArea，不重置 TCC、不重新授权。
   服务启动的非 Aqua 诊断子进程曾返回权限 false，不能替代实际桌面 helper 的权限观测。
2. 新建纯文本：AX 写入中文/emoji，截图及 AX 值一致；明确 Cmd+S、填写文件名、原生保存后，
   文件为 30 bytes，SHA-256 a726d0db7c50ba15507badee1dc7a3acf61b5d3ed89ebaadbea48407ac2c8ab7；
   再正常关闭，文件内容保持。
3. 已有含 emoji 文稿：替换全部内容、Cmd+S、回读为 34 bytes，SHA-256
   d5d876c02d86fad5e741a777e66d424d35d7577b9b91b866edaa8f2fb7070cd0；正常关闭通过。
4. **尚未解决：未保存文稿直接关闭。** 纯文本关闭后弹出保存提示，但在提示里保存落盘为空；
   富文本 AX 值虽匹配，直接关闭未出现保存提示。不能据此宣称文稿保存完全修复，根因尚未确定。
   当前可验证流程是明确保存并回读文件后再关闭，不能将 AX 值匹配当作应用文档持久化成功。
5. 保存目录从“位置”控件选取，文件名仅填 basename，没有把绝对路径塞进文件名。
   Cmd+N/Cmd+S 出现新窗口/Sheet 时可能在后置焦点检查返回 unconfirmed，均观察原操作，未重放。

仅本轮测试窗口/文件、PTY、caffeinate 和一次性安装作业被清理，安装私有备份保留。
Mac Intel 包已交叉编译、签名并校验，当前没有 Intel 机器，未声称原生运行通过。

## 产物（本仓库 .build/packages）

| 文件 | SHA-256 |
|---|---|
| pixels-agent-bridge-windows-x86_64-release-setup.exe | 7f96d2a0b0aabad90fc14e8326cc0f39918430a246038d9ad1cfb9fa533a448b |
| pixels-agent-bridge-linux-x86_64-release.tar.gz | 01edb320b5d4af77295b51236e31b634f0da727c3c4614c0d89220e01dc0c7bc |
| pixels-agent-bridge-macos-aarch64-release-setup.pkg | 5793b106ba46ecb93590ce4c690d3d3b716b3686011cd3e636a3d7c645f14b8d |
| pixels-agent-bridge-macos-x86_64-release-setup.pkg | a486995433a6a6d4847096ee345b812b1c060a30bab6d328106e4f92f692ed0f |

## 当前未完成项

- 本机安装已完成，私有数据库备份及安装回执位于 `.build/compat-local-install-1.2.41`；
  未将数据库、凭据或其摘要提交到仓库。
- 当前宿主加载的旧 MCP schema 没有 encoding/include_base64/new_instance。安装更新后需重载
  MCP 会话，再用原生 pixels 工具验收新增参数，包括 CP936 原命令重读、原始字节、实例复用/新建、
  Windows 新实例选项拒绝。未通过独立脚本伪装此宿主验收。
- 上述 Mac 未保存文稿关闭问题，以及缺少普通用户交互桌面、Intel、多屏实机的测试边界保留。

工具作业编号见 [早期操作索引](compatibility-operation-index-2026-10-08.json)和
[Release 操作索引](compatibility-release-operation-index-2026-10-08.json)。running/complete 为各次观察时的值，
同一操作的后续查询以原 ID 关联，不将早期 running 当作最终状态。索引仅含编号及状态，
不包含密码、键盘输入、用户私有任务输出或数据库内容。
