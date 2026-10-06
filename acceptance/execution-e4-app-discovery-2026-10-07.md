# E4 应用发现后端与双平台原型

日期：2026-10-07。基线 cad3d78，源码版本 1.2.26。此阶段不更换安装包，不增加已安装 MCP 工具声明。

## 实现与来源

- 复用 windows 0.62.2 的 `SHGetKnownFolderItem(FOLDERID_AppsFolder)` / `IShellItem::BindToHandler(BHID_EnumItems)`：保留系统 Shell 应用标识，适用于商店应用与注册的桌面应用。目录不递归扫描磁盘。
- macOS 复用已锁定依赖 objc2-app-kit 0.3.2、plist：安装目录为 `/Applications`、`/System/Applications`、当前用户 `Applications`，目录遍历深度和总条目有界，不进入应用包继续扫描。Info.plist 仅接受有大小限制的普通文件，防止 FIFO 阻塞。
- 运行快照：Windows 当前会话可见顶层窗口所对应的去重进程；macOS NSWorkspace runningApplications。不是全部系统进程的替代接口，Windows 不把无窗口后台进程称为完整应用清单。
- 运行实例含 PID 与复用已有平台 API 的进程启动身份；Windows 返回实际 WTS 会话，但本阶段尚未查询进程账户 SID，`account_id: null` 明确表示未知。Mac 核对 proc_bsdinfo UID 并核对前后进程身份。
- 快照附带实际执行账户和图形会话。Windows 拒绝 session 0 与系统服务账户，Mac 复用 SessionGetInfo/console 校验，拒绝 root 和非活动图形会话；未将仅切换 UID 当成进入桌面会话。
- 名称、应用 ID、路径可搜索；最多 200 个结果，序列化后不超过现有 32 KiB 响应预算（为外层预留空间），截断不截短 ID/路径。进程列表不提供翻页；应缩小搜索范围重新观察。
- Linux 无界面产品直接返回 `unsupported_platform`，不尝试连接显示服务器。

官方接口依据：[Microsoft IShellItem](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ishellitem)、[Apple NSWorkspace](https://developer.apple.com/documentation/AppKit/NSWorkspace)。接口签名另外核对本机 Cargo 锁定版本的生成绑定源码。

## 已验证

| 环境 | 证据 |
|---|---|
| Windows 本机 chess / WTS 2 | apps_probe installed 返回受响应大小约束的 190 条并标记 truncated；running 当时返回 29 个当前会话进程。Notepad 搜索返回 Notepad/Notepad++/Notepad2e，ID 保持完整；无新窗口启动或用户文件写入 |
| Mac 603527578 / UID 501 | 通过原生 Pixels MCP 执行源代码探针，在 `launchctl asuser 501` + 原生用户身份下发现 71 个安装条目、69 个运行实例，图形 session 100002；root 调用被拒绝 |
| Mac 搜索回归 | TextEdit 返回 com.apple.TextEdit 和完整 app 路径；Finder 返回中文“访达”、com.apple.finder、PID 601、macos_audit 进程身份与真实 UID/session |
| 三平台边界 | Windows/Mac/Linux 的搜索顺序、条目限制及大量 JSON 转义字符响应预算测试通过；协议拒绝无效限制、过长 UTF-8 搜索和未知分页字段；Linux 无图形环境明确拒绝调用 |
| 编译 | Windows workspace --tests check、Windows 与 Mac apps_probe 构建通过；Linux desktop-control 相关测试构建通过 |

Mac 全量探针任务：`9f8eff16-b1a1-4a81-aaa1-f7ef4ec8dde1`；最终源码测试与精确搜索：`4dac9aea-50e4-474b-9dcf-8d646049b225`，均 exit 0。Linux 测试在 --rm Debian bookworm 无 GUI 容器完成，容器已退出删除。

本轮只创建源码构建产物，没有启动/关闭任何用户应用、改动权限设置或用户文档。

## 尚未完成

这是 E4 发现后端原型，不是 E4 整体交付。下一步完成 launch/open 原生接口、指定桌面用户上下文与 helper 路由、请求去重/结果观察、正式 MCP 三工具和最小 UI。公开工具需要能力协商后才能声明可用，不能直接调用当前 SYSTEM/root helper 冒充用户。

仍需 Windows 90 用户会话验收、后台/多窗口应用归因验证、Windows 进程账户观察、Mac/Windows 启动与复用/中文文件打开/未保存退出流程，以及全部 E7/E8 安装与正式宿主验收。应用目录来源有限，不承诺穷举便携应用或自定义安装目录。
