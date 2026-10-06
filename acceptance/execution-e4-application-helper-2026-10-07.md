# E4 Windows 后台应用 helper 与通道隔离

源码版本 1.2.26。本轮不分配产品版本、不替换已安装软件；安装和正式宿主验收仍在 E7/E8。

## 实现与事实更正

- 现有 Windows Default helper 已以登录用户运行，对 split-token 管理员使用 linked elevated token 来控制高权限窗口；Winlogon helper 才是 SYSTEM。此前 routing 报告将两者都称为 SYSTEM 不准确，已更正。
- supervisor 为每个 WTSActive 且存在用户的非零会话维护独立的 `--application-helper --desktop=Default`。应用 helper 使用原始 WTS token 和用户环境，复核登录代次，不获取 linked elevated token。旧窗口控制路径保留。
- 注册增加 `applications_only`；必须完成内核 PID/账户核验。窗口、截图、新版截图、键鼠、DesktopQuery 路由及 UI 连接释放广播均跳过应用专用 helper。
- Windows 主 UI/普通 session helper 不再声明应用能力；Mac 沿用原有用户 helper 和签名身份。Linux 不启动应用 helper。
- 应用调用前后复核 WTS 活动状态、Default 桌面和实际身份。断开、注销、用户变化后不重定向到别的会话。
- 原生应用调用放在专用 helper 的阻塞线程，20 秒未完成就退出该 helper。supervisor 后续补充新 helper；原请求保持未确认，不重放。只结束 helper 本身，无 Job 树终止，因此不会因 helper 回收主动杀掉已启动的应用。

## 已验证

- Windows `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --tests` 通过。
- Windows Executor 本地 IPC 回归 22/22；新增测试在应用 helper 后注册的情况下，原窗口通道仍能收到请求和释放广播；移除窗口 helper 后，五类旧请求均拒绝，应用通道没有收到误发消息。
- Mac 以 UID 501 运行同一 IPC 回归 22/22；Desktop 编译检查通过。正式 `pixels.pab_run_command` 任务 `4251e604-edbe-46ff-912b-4bed1a97442e`，退出码 0。WTS 活动复核增量同步后的编译任务 `45062183-e1bf-4c0b-a499-38b2078ebcf2`，退出码 0。
- Debian 无图形容器 IPC 回归 18/18，退出码 0；不新增 Linux GUI 产品能力。
- Windows 90 原生只读探针使用当前 supervisor 源码，原始 WTS/app token 的登录代次和 elevation type 一致，错误登录代次拒绝，用户 LOCALAPPDATA 正确。任务 `e594a088-23bb-4425-a190-a3e69dc23419`，退出码 0；session 1 / Administrator 的 elevation type 为 Default（1），本次不能替代 split-token 场景。探针按 SHA-256 核对后已从 90 清理。
- Windows 本机同一只读探针以 SYSTEM 临时计划任务执行，session 2 / chess，同样为 Default（1）；原始/app/window token 类型一致，登录代次和用户目录核对通过，退出码 0。PowerShell 启动器未取得有效输出，改用 cmd 直接启动探针后通过；临时计划任务和系统临时日志已删除。本机 MCP 未保存自连接密码，因此此项是本地原生测试，不是正式宿主自连接验收。两台机器都未覆盖 split-token 管理员，仍列待测。

## 尚待验收

- 安装后主 UI 关闭、多活动 RDP 会话、锁屏/注销、supervisor/helper 重启的真实流程。
- 可控阻塞原生调用下的进程回收、原请求未确认和已启动应用存活，仍需原生故障注入实测；不能以源码超时分支代替此项验收。
- 普通用户及 split-token 管理员启动应用后的实际进程 token，与现有高权限窗口控制回归。
- E5 UI、其余 E6 矩阵和 E7/E8 交付仍未完成。
