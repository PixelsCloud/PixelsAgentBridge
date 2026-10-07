# Mac 安装版工作流与界面验收

日期：2026-10-07。设备 603527578，macOS ARM，huayang UID 501。正式 MCP 1.2.32，
目标完整安装包 1.2.37。修复与版本已提交 `1a65c9e` 并 SSH push。
操作引用见 [安装回执索引](execution-e8-installed-final-2026-10-07.json)。

## 修复与验证

- TextEdit 普通 AXTextArea 没有 AXEnabled，但 AXValue 可写。仅对该普通文本角色
  推导可用状态；显式 disabled、受保护输入及提供者错误不改成可用。
- 系统保存面板含其他进程的 AX 控件。实际进程/窗口仍须有效；不同 PID 的控件必须
  在原窗口实时 AXChildren 树内重新找到，遍历限制 400ms/2000 节点/64 深度。
  不把任意同进程/异进程窗口当成目标后代。
- 保存面板 AXSplitter 能返回属性和子节点，但 action_names 返回 provider_failed。
  现在保留 actions 字段错误并继续遍历，不推断 AXPress。47 项 desktop-control 测试通过。
- 1.2.34/35 解决了部分问题，实测仍失败；1.2.36 加入定位信息；1.2.37 去除临时诊断并
  完成真实查询/编辑/保存。各次构建均自动占用版本，不回退版本，也没有把中间包记成最终通过。

两轮 TextEdit 中文内容均经 UI 设置、保存后由文件工具和独立 UID/内容检查验证。
第一轮文件名框会把路径斜杠变成冒号，实际文件在原保存目录 `.build`；检测到预期路径
不存在后查明原文件，没有重复保存来掩盖结果。第二轮直接关闭后的首次保存为空，
菜单位置选择也未改变实际目录；工具的 provider 接受不能证明业务保存成功。
重新打开该测试文件、写入并显式 Command-S 后，实际文件精确匹配。正式工作流要求
写入后显式保存并回读验证，不把 AX matched 当成文件落盘完成。

两份最终文件与旧应用打开夹具均属 UID 501，均正常关闭后用原用户文件工具删除。
旧 element_ref 被拒绝；真实禁用菜单项返回 control_not_enabled、action_dispatched=false。
没有强杀 TextEdit 或丢弃文稿，没有重置 TCC。

## 实际安装界面

通过已安装 app 的真实控件/鼠标键盘切换简体、繁体、英文与亮暗主题，逐一检查六种
设置页面。截屏在 `.build/finish-macos-ui/`；读取 WebKit 本机存储仅限 pab.language、
pab.theme 两个字段，确认用户选择保存，最终恢复简体亮色。屏幕录制和辅助功能均可用。

在 Mac Desktop 新建仅用于测试的本机连接，实际选择 huayang 执行 `/usr/bin/id`。
任务 succeeded，输出 UID 501，任务详情显示执行身份 huayang；记录没有注入模拟数据。
截图 `actual-user-history.jpg`。随后经界面删除测试设备，数据库只读检查
remembered_devices 和 device_credentials 都为 0；保留两条测试活动记录作为历史事实。
关闭主界面后系统 session-helper 仍存活，原生窗口查询继续工作。

原生 AX 对受 React 控制的输入 set_value 返回 mismatched 时，没有声称成功；改用
公开输入工具发送实际键盘事件。NSWorkspace 启动本产品时可复用已有无窗口 helper，
所以 reused_instance 不代表主窗口可见；验收使用系统 open -n 启动独立 UI 并观察窗口。

## 交付与清理

完整 ARM PKG 安装退出 0、所有文件与清单一致、设备身份保留，三个签名产物通过
固定证书核验；没有更新系统信任或重置 TCC。ARM/Intel 包均已通过原生下载工具回到本机。

| 产物 | SHA-256 |
|---|---|
| macos-aarch64-debug-setup.pkg 1.2.37 | e031037186397ee0fb65d1f23eeb429ec93573459b2caa5856f81f041090c7b4 |
| macos-x86_64-debug-setup.pkg 1.2.37 | c1ee2069bce343e043d4293027c203aa290bce5957235df37e8922427f5b90b8 |

Intel 只有构建/签名/哈希检查，没有 Intel 实机证据。当前免费固定签名方案不等于
Developer ID 或公证。一次性安装 LaunchDaemon 已卸载；自建 caffeinate 已停止，
AX 定位探针已删除；三份测试文档和测试窗口已清理。私有升级数据库备份保留在目标机私有目录。
