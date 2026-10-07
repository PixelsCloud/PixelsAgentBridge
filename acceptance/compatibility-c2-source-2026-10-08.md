# C2 桌面兼容源码与待验收项

Windows 原生桌面库 40 passed、5 个需要独立桌面的原生项 ignored；Windows/Mac helper IPC 22 passed。
Mac 编译发现的 AXValueGetValue 指针类型已修正，原生库与 IPC 回归退出码 0。
MCP 40 passed；浏览器 21 passed，包含默认启动和显式新实例参数、沿原请求观察且不重放。

## 实现

- macOS 应用启动增加可选 new_instance，默认维持 NSWorkspace 复用行为。选项需要
  system-query v13 和 application helper v2，旧端在派发前拒绝。Windows 明确 unsupported。
  默认 false 不序列化，保留既有请求指纹。新实例请求仍不保证应用创建窗口。
- Mac 可编辑 AXTextArea 优先选择全部文字并写 AXSelectedText；检查实际选区，失败不再
  重试 AXValue 或模拟按键。普通控件保持原路径；这不等于保存成功，需要安装后回读文件。
- Windows foreground_activation_denied 说明需在所选桌面激活窗口并重新获取引用；不自动提权。

依据：[NSWorkspace 实例选项](https://developer.apple.com/documentation/appkit/nsworkspace/openconfiguration/createsnewapplicationinstance)、
[AppKit 文本选区](https://developer.apple.com/documentation/appkit/nsaccessibilityprotocol/setaccessibilityselectedtextrange(_:))。
优先使用项目已有 objc2/accessibility 依赖，没有另写键盘模拟替代原生编辑接口。

## 安装前实测与边界

Mac 1.2.37 现有文件中文写入、正常关闭、实际文件回读一致；新建文稿 AXValue 返回匹配，
但正常关闭没有保存提示。候选改动尚未安装，不能宣称此问题已修复。
Cmd+N 会改变目标前台窗口，批量输入保守返回 unconfirmed；观察到了新窗口，所以未重放快捷键。
首次错误预期值请求在 precondition 阶段被拒绝，action_dispatched=false；不是产品修改失败。

待完成：固定签名完整包安装；新文稿保存及回读、默认复用/新实例、权限保留；Windows Release
普通用户身份/文件/PTY 重验；三平台 Release 新装/升级/隔离卸载。源码通过不代表安装验收通过。
