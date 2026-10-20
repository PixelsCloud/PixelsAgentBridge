# macOS 登录界面 helper 的安装与升级注册

日期：2026-10-17。基线 `b8cf7c4`。属于 W4/W8 前置修复，不代表 Mac 锁屏登录通过。

## 问题及修改

安装/卸载脚本用 `loginwindow/com.pixelsagentbridge.login-helper` 查找登录界面
进程。Apple 实机上，即使从 root 后台执行，该目标也返回错误；本机 launchctl
手册给出的会话域形式是 `login/<asid>`。现有脚本因此可能漏掉需要退出的旧
LoginWindow helper；首次安装也没有对已存在的登录界面注册新 helper。

现从 `launchctl print user/0` 获取数字 audit session ID，并确认对应 login
域存在，再在该域查找、退出或注册项目自己的 helper。升级记录实际域用于原有
回滚流程；首次注册和升级恢复后的检查保持幂等。不存在活动登录域时不强行创建
会话，保留 LaunchAgent 的 LoginWindow 加载策略。没有改变既有连接认证。

实现参考本地 `D:/GoCloud/rustdesk/src/platform/macos.rs` 的 `loginwindow_asid`
处理；Apple 的会话类型约束见 [launchd.plist 文档](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchd.plist.5)。
`LimitLoadToSessionType` 的 plist 是系统约定格式，保持 plist，没有改为 TOML。

## 验证及边界

- Windows 本地启动测试 7 项通过，包含缺失/失效/非数字会话编号、升级和卸载
  目标、新安装重复注册及现有 Aqua 启动行为。
- Apple 自带 Bash 3.2.57 上同样 7 项通过；另有 4 项真实临时程序升级测试通过，
  覆盖进程退出、客户端反复启动旧程序、发布失败回滚和准备阶段失败。
- 测试只在临时目录编译一个小程序，单核；不运行安装器、不改系统服务、不锁屏、
  不改 TCC，也没有替换两个固定授权测试 App。
- 前后只读检查确认原服务仍加载、当前 UID 501 图形会话仍在、固定 App 签名及
  helper plist 哈希保持；无测试进程或编译进程残留。

实机当前是普通用户桌面，没有可访问的 root LoginWindow 域。因此注册/退出分支
使用模拟 launchctl 验证，不能冒充真实 LoginWindow bootstrap 成功。真正的锁屏、
密码登录、会话恢复及完整安装包升级仍待验证。锁屏会中断用户桌面，本轮已询问
是否允许该动作，尚未收到确认，不执行依赖该确认的测试。

结构化证据及任务 ID 见 [验收数据](remote-desktop-macos-login-launchd-2026-10-17.json)。
