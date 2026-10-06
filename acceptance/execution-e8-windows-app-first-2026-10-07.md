# E8 Windows 原生工具首轮应用与文件验证

日期：2026-10-07。90：`211399447`，Server 2022，已安装 1.2.27。
当前正式 Pixels MCP 已恢复并加载 68 个工具；以下通过原生 `pixels.pab_*` 完成。

## 工作流证据

1. 锁屏时账户查询提供 Administrator/user，但没有 desktop_user。窗口列表确认 Windows 登录界面。
   使用原生 secure_attention、已授权的本地保存凭据输入及登录按钮解锁，没有修改登录策略。
   报告及仓库不包含密码。
2. 解锁后账户查询 `40eea493-ae43-4e0a-aee2-d1b43bb12efd` 提供 Administrator、WTS 1 的 desktop_user。
   应用发现 `70ed59d3-8b72-45e4-aac0-8b2de17c10ce` 返回系统记事本、Notepad++、Notepad--。
3. 使用 user 上下文创建中文路径文件，`pab_open_file` 通过发现的 OS ID 打开系统记事本。
   操作 `e389ed07-e875-4554-bea6-7e1dce55eac5` 返回账户 SID、会话 1、实际进程 PID 39320；
   后续窗口观察确认该进程打开了正确文件。
4. UI 控件查询取得记事本编辑器，`pab_ui_action set_value` 写入两行中文内容并返回 matched。
   操作 `e4e717b3-66d2-45e9-a093-0e2741d56482`。
5. 正常关闭请求 `64b3db5b-7823-41c8-814e-f79c4bfabcaf` 触发保存提示，原窗口仍在。
   原操作报告未观察到关闭，未强制杀进程、未重放关闭；查询确认提示对应专用测试文件。
   点击“保存”，操作 `caaf584f-0b8c-47cf-b39d-06d0dcb6699b`。
6. user 文件读取 `dd318723-ab79-474a-8819-4686b31bfcf6` 与输入内容逐字一致，UTF-8/CRLF、66 字节，
   SHA-256 `0560764e72dccba30a2a9b76bd34c3b8538687297f5f38ff2ae8ba48057d0768`。
   原生用户命令任务 `1b90d13f-550c-4611-881f-4b4f9dd32dea` 核对 SID、文件 owner 与 token 默认 owner
   一致（内置管理员为 Administrators 组），并确认测试编辑器进程已退出。
7. 文件删除 `8408dcea-bb04-4eaa-aad5-91c2a65f6fe1` 以原用户完成，删除 1 项，partial=false。
   没有关闭其他应用。

## 实际边界与剩余

对记事本的首次 focus 被 Windows 前台策略拒绝，明确返回错误；未用绕过策略的方法取得焦点。
控件定向编辑和保存仍完成，因此本轮不宣称 focus 成功。

这是应用/文件工作流首轮，不是 E8 全部完成：尚需第二轮（新建/另存为等）、普通账户/多会话，
命令/终端/Git/传输两轮，以及 Mac/Linux 对应工作流和包含最新修复的最终安装包。
Mac 仍锁屏，已有测试文档的关闭清理需要桌面可操作。
