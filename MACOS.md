# macOS 完整适配与验收

## 当前执行状态（2026-10-07）

执行用户/应用管理的源码增量正在验收，尚未替换安装版 1.2.26。
Desktop 已接入命令、终端、目录、传输的执行用户选择及应用管理面板；实际身份进入本地任务历史，
未知传输结果沿原操作核对。Mac ARM 的 Bridge 69 项、MCP 38 项测试和 Desktop 编译检查已通过。
见[执行规划](EXECUTION_CONTEXT_ROADMAP.md)及[UI 源码报告](acceptance/execution-e5-ui-2026-10-07.md)。
应用真实生命周期、完整包升级和正式 AI 宿主验收仍待完成；没有重置当前 TCC 授权。

Finder窗口查询问题已修复并实机验收：AXWindows混入的AXScrollArea不再导致整个
查询失败，仍严格要求目标AXWindow唯一且符合进程/标题/几何信息。Mac已安装1.2.26，
原签名和授权保留，Finder/Terminal查询及21项正式控件回归通过；ARM/Intel完整包已生成。
见[修复报告](acceptance/finder-window-fix-2026-10-06.md)。下方1.2.25记录为修复前交付。

最新控件自动化已实现：`pab_ui_query/get/action/wait`，使用 AX 开源绑定和 helper 所属的
隔离 worker。Mac ARM/Intel 1.2.25 完整包已生成；ARM 已安装，固定签名、设备身份和
已有权限保留，安装后的控件闭环两轮及原生27项基础回归通过。Intel仅构建/签名检查。
Windows90和本机均已安装1.2.24。重载后的正式AI宿主四个新工具已在两端各完成两轮验收，
实际列表选择、文本、勾选与点击计数均已核对。Mac Terminal只读查询通过；Finder遇到
非AXWindow根对象返回interactive_window_unavailable，保留为兼容性限制。
详见[控件自动化报告](acceptance/ui-u0-2026-10-06.md)。

以下1.2.22记录为上一轮交付。

回归驱动和指定显示器输入已开发，最终 Mac ARM/Intel 1.2.22 debug PKG 已构建。
Mac ARM 的升级、实际截图/点击、重启后登录和权限保留已通过。临时安装作业曾重复执行，
已移除并记录一次性作业约束。Windows 90及本机已升级1.2.21，基础回归已通过。
验收发现 MCP 停止观察后查询可能一直返回旧任务状态，已修复并在两端通过回归。
最终 ARM 1.2.22 已安装，60工具初始化、27项基础用例和3项SDK回归通过；Intel无实机。
详细证据、安装包哈希和待验项目见 [本轮报告](acceptance/2026-10-06.md)。

## 下一阶段计划（2026-10-06）

上一轮已完成，新会话的60工具加载、Windows/Mac各27项基础回归、任务查询与过期屏幕
拒绝均已通过。下一轮为[桌面控件查询与操作长任务](UI_AUTOMATION_ROADMAP.md)，
实现、平台后端、MCP、回归、安装和重载后的正式宿主验收均已完成；U0–U6保留实测范围及平台限制。

## 上一轮计划存档（基于1.2.15验收结果，现已完成）

完整长任务的执行顺序、异常矩阵和交付标准已整理至
[ACCEPTANCE_ROADMAP.md](ACCEPTANCE_ROADMAP.md)，执行时以该文档为准。

本节保留当时计划；下方按日期保留的记录只代表当时状态。锁屏、注销后登录、重启登录、
熄屏恢复、升级保留权限和异常退出松键均已有实机证据，不再列为未实现功能。

1. **优先：可重复执行的端到端回归。** 整理通过正式MCP工具运行的验收驱动和结果记录，
   使用自建窗口、临时目录、测试子进程；每项保存请求ID、实际结果和失败原因。
   普通套件覆盖连接、命令、文件传输、终端、系统查询、截图、窗口、中文输入及保存。
   锁屏、注销、重启和升级单独作为有副作用的套件，按本次会话授权运行，不能混入默认测试。
   凭据仅在运行时输入，不写入脚本或报告；失败查询原操作，禁止自动重放未确认的输入。
   通过标准：能独立重跑并区分通过、失败、环境缺失，输出可核对的文件内容／校验和／窗口状态，
   结束时无残留按键、测试进程或电源断言。
2. **随后：多屏选择与坐标一致性。** 先审查截图的monitor_id、像素到桌面坐标映射与
   旧版鼠标归一化坐标的范围，定义截图选屏后如何准确点击同一屏幕；复用xcap／Enigo。
   覆盖负原点、不同缩放、主屏切换、拔除目标屏幕和镜像屏。目标消失明确失败，不默默换屏。
   单元测试覆盖几何与身份变化；物理多屏实测需实际硬件，不能用模拟数据宣称全部通过。
3. **发布验证：跨平台安装与完整包。** 在Windows本机及winserver复核共享依赖、截图和输入；
   Mac ARM验证完整包版本／hash／签名及升级后的权限。Intel先做编译和打包，有Intel机器后
   再做原生运行验收。增加包内容及版本一致性自动检查；升级／卸载测试只用于约定的测试设备，
   失败保留日志，不自动清除设备身份或用户数据。产品构建继续统一递增版本。
4. **工具增强：先列能力缺口，再选择下一项。** 对现有工具用真实任务梳理仍需拼接原始命令的
   场景，优先比较结构化桌面控件查询／动作、文件查找／内容搜索的收益和现有实现重复度。
   开发前查成熟开源库，明确Windows／macOS支持范围、对象失效、取消／超时和结果验证，
   再形成具体协议与测试方案。本阶段尚未决定新增工具名或承诺操作所有应用。

上述第1–3项已交付，第4项现由控件长任务承接。FileVault预启动、物理多屏、Intel和旧macOS兼容性保持独立环境
限制记录，不把它们阻塞为所有后续开发的前置条件。继续维持任务记录仅存本地、Server/Web
不接收任务记录、各MCP独立连接及Desktop状态上报的现有架构。

## 重启与登录前控制验收（2026-10-06，1.2.15）

使用已安装的1.2.15完整包，通过正式 Pixels MCP 完成真实重启、登录前控制和登录后验收。
本轮无需修改程序、重新编译或增加版本号；未重新申请权限。

- 重启前确认 Executor LaunchDaemon、LoginWindow和Aqua LaunchAgent的RunAtLoad／KeepAlive配置。
  当前测试设备FileVault关闭。使用一次性launchd任务执行正常系统重启；该任务没有安装到
  开机目录，重启后确认不存在，避免再次重启。
- 系统启动标识从 `3326E3E7-6AEC-4A4C-A024-DD4138D0A0B2` 变为
  `CF9B18CE-B56B-4C8E-9644-E1EC4F043E28`，确认经历完整系统启动。
  未登录时 `/dev/console` 为root/0，设备自行上线；Executor PID296、LoginWindow
  helper PID286均由launchd自动启动，runs=1、无退出记录，没有手动bootstrap或启动GUI。
- 默认截图返回真实登录界面。登录前执行显示器熄屏，只读Quartz确认active=[]、asleep=true，
  随后默认截图自行唤醒并返回正常首帧。使用正式MCP鼠标／按键工具填写已授权登录凭据，
  截图确认密码输入生效，确认后进入huayang桌面。凭据未写入文档或测试文件。
- 登录后 `/dev/console` 为huayang/501；进程列表确认root LoginWindow helper退出，
  用户GUI PID520和Aqua helper PID1065运行。新的窗口查询helper为
  `6052ca12-44c9-49f4-99ab-a12621414944`。
- 用户所有、可写的独立TextEdit测试文件中输入 `RebootPassed-重启验收通过` 并Command-S；
  读取文件核对实际内容后正常关闭测试窗口。批次操作
  `81c3807b-154c-4211-9eb3-0c4ba5686493` 三步完成。
- 验收后左右Command／Shift／Option／Control均松开，输入恢复账本为零，没有新增
  pab-desktop崩溃报告。Mac保留在已登录桌面，原有用户应用未被用于输入测试。

已覆盖：完整PKG升级保留权限、锁屏／注销后登录、重启自动上线、登录前熄屏唤醒和输入、
登录后桌面接管。剩余环境验收：物理多屏／镜像屏、Intel实机、多用户快速切换。
FileVault预启动解锁不在此次结果内，不把系统已启动后的LoginWindow控制等同于磁盘解锁。

## 熄屏后的显示器恢复（2026-10-06，1.2.14–1.2.15）

### 原因与实现

- 1.2.13 实机复现：默认截图、指定 monitor 2 截图和显示器查询都失败。
  只读 Quartz 诊断显示 active=[]、online=[2]、display 2 asleep=true、active=false。
  因此本次已证实的原因是屏幕休眠使活动显示器列表为空；不是 TCC 未授权。
  用系统 caffeinate 短暂唤醒后，旧版截图立即恢复。
- 继续使用 xcap 0.8.2 枚举／截图。活动列表为空时，通过现有 objc2 生态的
  `objc2-io-kit`／`objc2-core-foundation` 调用公开的 IOPMAssertionDeclareUserActivity。
  最多观察2秒，退出时释放电源断言，不修改系统休眠设置，不发送虚拟按键唤醒。
  截图、显示器查询、旧版鼠标移动共享恢复逻辑；键盘释放路径不等待唤醒。
- 鼠标的显示器准备在 helper 工作线程执行，实际输入仍留在 AppKit 主线程。
  1.2.14 实测 active 恢复后的首帧可能仍在亮屏过渡而全黑；1.2.15 仅在唤醒后要求
  500ms连续非空观察，再执行操作。已亮屏请求不增加等待；这不是对受保护画面可截图的保证。
- 默认显示器采用 xcap 保留的 CGGetActiveDisplayList 首项，避免混用稍后查询的主屏标记。
  明确指定 ID 时不回退到别的屏幕；无设备、无效尺寸／缩放、会话已变更会失败。
  截图后重新校验 ID、原点、尺寸及缩放，防止返回与图像不一致的坐标映射。

依据：[Apple 活动显示器顺序](https://developer.apple.com/documentation/coregraphics/cggetactivedisplaylist(_:_:_:))、
[Apple 用户活动唤醒 API](https://developer.apple.com/documentation/iokit/1557127-iopmassertiondeclareuseractivity)。
最初怀疑的主屏标记瞬态未单独复现，不将其写成此次实机故障的确定根因。

### 测试范围

- 新增6项纯逻辑测试：默认／指定选择、重新枚举、指定ID丢失不回退、无效几何／无屏幕、
  唤醒一次且不重放输入、唤醒失败／会话变化／等待上限。Windows desktop-control
  22项通过、4项显式图形测试忽略；Mac 26项通过，Tauri编译检查通过。
- 1.2.14 完整PKG升级成功，原有签名与权限保留。屏幕休眠时，鼠标移动已实际唤醒
  并到达(624,395)，与归一化输入(16000,24000)及2560×1080屏幕一致。
- 1.2.15 完整PKG再次升级成功，codesign完整校验通过，无需重新授权。
  分别使用 pmset displaysleepnow 进入真实熄屏，先用只读Quartz查询确认 active=[]、
  asleep=true，再通过正式MCP工具执行：默认截图首帧正常；显示器查询恢复monitor 2；
  鼠标移动到(937,263)，与输入(24000,16000)一致。查询操作ID
  `c284b749-6afd-43f0-97db-a2ba27931e9e`。三条路径不依赖外部caffeinate唤醒。
- 锁屏后再次熄屏，默认截图自行唤醒并返回登录画面，使用用户已授权凭据解锁成功，
  截图确认恢复桌面。无效monitor ID 4294967295明确失败，没有返回另一块屏幕。
- 验收后没有Bridge电源断言残留，系统显示器休眠设置仍为180；所有修饰键松开、
  held账本为零，没有新增pab-desktop崩溃报告，安装用临时launchd job已移除。

最终Mac ARM64 debug PKG位于 `.build/packages/pixels-agent-bridge-macos-aarch64-debug-setup.pkg`，
版本1.2.15，71,984,587字节，SHA-256
`38896a6c0b2994f374fe73101f052a181508544ac44d04e24238b55aab195b73`。
完整TGZ SHA-256 `b692042781ec5a81df319c836cb69bb7579f6eadcbaa7a7ea3f763ef1f92099b`。
继续使用固定免费签名证书；PKG未做Apple签名／公证。本地与Mac源码版本同步1.2.15／计数16，
本轮未打包或安装Windows。Mac日志为 `.build/display-wake-{tests,check,package}.log`
及 `.build/install-display-wake.log`。

本轮不会将物理多屏插拔、镜像屏、合盖无显示器或 FileVault 预启动算作已验收。
此轮显示器修复验证仅锁屏未重启；后续重启验收结果见本文顶部。

## 自身窗口崩溃与锁屏／注销验收（2026-10-06，1.2.10–1.2.13）

### 已修复和验证

- 1.2.9 GUI 在聚焦自己的窗口时崩溃。系统报告
  `pab-desktop-2026-10-06-005038.ips` 明确记录 `Must only be used from the main thread`，
  调用链为 `AXUIElementPerformAction → accessibilityPerformRaise → makeKeyAndOrderFront`。
  当时批量操作停在 focus，锁屏快捷键尚未执行。
- 将 AX 窗口标记、验证、聚焦查询、控制和释放调度到主线程；注册表锁在调度后获取。
  状态轮询的等待仍在工作线程，避免阻塞 AppKit 的最小化、恢复动画。
- 1.2.10 完整 PKG 安装后，暂时停止 Aqua helper，让 GUI 单独处理请求。
  聚焦自身、最小化、恢复均观察到目标状态，GUI PID11831 保持存活，没有新的崩溃报告。
  操作 ID 分别为 `e23d4545-d1e6-4094-8ddd-979a7d052338`、
  `6308fd92-cf7b-4676-b0ab-436d890b34a9`、`69158070-0d33-4cc3-9fa5-dfc8f7dab3fb`。
- 恢复 Aqua helper 后，旧引用在任何动作执行前被拒绝。重新枚举窗口后，
  Control-Command-Q 实际锁屏；JPEG 截图确认密码界面。通过正式 MCP 输入工具输入
  用户提供的登录凭据后成功解锁，截图确认返回桌面。凭据未写入测试文件或文档。
  锁屏操作 ID 为 `361df588-716d-4a67-bb8f-1eccd808c6f2`。
- 用户确认保存工作后，Shift-Command-Q 打开正常退出登录对话框，确认后完成注销。
  焦点转入系统对话框导致快捷键批次末尾验证为 unconfirmed，未重放；依据截图确认下一步。
  `/dev/console` 变为 root，LoginWindow LaunchAgent 自动启动，能够截取登录界面。

### 登录前输入问题与修复

- 1.2.10 在 LoginWindow 首次输入失败，Executor 原始记录为
  `no connection could be established: (failed creating event source)`；不是辅助功能未授权。
  失败请求 `27bac914-cc08-491f-9b0b-0ef6763434b1` 尚未发送按键。
- Enigo 0.5.0 默认使用 private event-state table。只创建／释放事件源、不发送事件的
  诊断显示该环境 private 创建失败，combined 和 HID 创建成功。1.2.11 仅对活动的
  LoginWindow 使用 Enigo 的 combined-session 设置，Aqua 保持 private 设置。
  依据：[Apple CGEventSourceStateID](https://developer.apple.com/documentation/coregraphics/cgeventsourcestateid)、
  [Enigo](https://github.com/enigo-rs/enigo)。
- Executor 对事件源不可用、显示器不可用、桌面已切换返回固定的具体原因；保留任意
  helper 诊断不外泄的规则，新增测试验证已知原因和带私有路径的变体。
- Mac desktop-control 20项、Executor desktop 相关6项测试通过；Windows desktop-control
  16项通过、4项图形环境测试忽略。引入 Enigo 补丁后 Mac Tauri cargo check 通过。
  1.2.10–1.2.13 均完成正式签名构建。
  因当前已注销，使用校验 SHA-256 后的完整 TGZ 和随包 install.sh，以已确认的 huayang
  账号更新应用、Executor、MCP及服务，再启动 LoginWindow helper；没有仅替换单个二进制。
  GUI PKG 安装器仍要求已登录普通用户，此次没有宣称完成登录界面的 PKG 安装验收。
- 1.2.11 消除了事件源错误，但接口接受并不意味着登录框收到输入。用户确认系统屏幕
  访问提示后仍无效果；TCC 日志明确显示实际 LoginWindow helper 的 ScreenCapture
  和 PostEvent/Accessibility 已允许，不能继续归因于用户未授权。
- 1.2.12 为 Enigo 0.5.0 增加仅 LoginWindow 使用的 Session event tap 设置，保留其
  按键映射、事件生成与释放逻辑。源码和 MIT 许可置于 `vendor/enigo`，两个 Cargo
  workspace 均使用可复现的本地 patch；普通 Aqua 桌面保持上游 HID 行为。
  参考 [RustDesk 输入实现](https://github.com/rustdesk/rustdesk/blob/master/src/server/input_service.rs)。
  此项单独安装后仍未观察到输入生效，不能将其宣称为单独解决原因。
- 1.2.13 为 macOS 的 pab-desktop 可执行文件增加预登录图形程序声明
  `__CGPreLoginApp,__cgpreloginapp`，`otool -l` 确认产物包含该 section。
  参考 [RustDesk 构建参数](https://github.com/rustdesk/rustdesk/blob/master/.cargo/config.toml)
  和 [Chromium Remote Desktop](https://chromium.googlesource.com/chromium/src/+/refs/tags/113.0.5649.2/remoting/host/remoting_me2me_host.cc)。
  完整安装且签名校验通过后，同样的鼠标及按键操作首次在登录框实际出现密码圆点。
  此次没有重置 TCC、关闭 SIP 或授予私有 entitlement。

### 1.2.13 注销后登录验收通过

- 通过正式 `pixels.pab_desktop_input` 输入已授权的登录凭据并确认，截图观察到用户桌面。
  `/dev/console` 从 root 恢复为 huayang/501；LoginWindow helper 退出，用户 GUI
  PID15831 和 Aqua helper PID16299 运行。凭据未写入源文件或测试文档。
- 新窗口枚举返回新 helper `da4e529b-2a22-4b32-b5c8-a2d707715170`。
  旧引用请求 `9670e04b-40ec-4370-8cdc-cde99a3f2d0e` 在动作执行前被拒绝。
- 用户所有、可写的 TextEdit 验收文件中输入 `LoginRecovered-中文` 并 Command-S；
  文件读取实际确认内容已保存。批次 `23e36dab-a741-4447-a9b8-f09122e0f024`
  三步完成，之后正常关闭测试窗口，没有修改用户其他文档。
- HID/combined 状态中的左右 Command、Shift、Option、Control 均已释放，全部输入
  恢复账本 held 槽为零；本轮没有新增 pab-desktop 崩溃报告。

### 待继续与产物

1. 锁屏时曾分别出现 `primary monitor unavailable` 和 `selected monitor is unavailable`；
   明确指定重新枚举得到的 monitor 2 可以截图。默认显示器选择的切换边界仍需进一步定位。
2. 本轮未测试重启、FileVault 预启动登录、多用户快速切换；当前设备 FileVault 关闭。

最终 Mac ARM64 debug PKG：`.build/packages/pixels-agent-bridge-macos-aarch64-debug-setup.pkg`，
版本1.2.13，71,990,447字节，SHA-256
`625ffb1f1eb6356556501d75be0cb56c0e9fd2597489e148329ef1db2c6f2cdc`。
完整 TGZ SHA-256 `5845981e68355ac66722fd340526eba1cb33fd4ec926053a3201b720797e774f`。
继续沿用固定自签名证书；PKG 未做 Apple 签名／公证。Windows 源码同步版本1.2.13／计数14，
仅运行测试，未构建或安装 Windows 产品。日志位于 Mac `.build/ax-main-*`、
`.build/login-source-*`、`.build/login-tap-*`、`.build/prelogin-marker-*` 和对应
`install-*.log`；四个临时安装 launchd job 均已移除。

## 输入进程异常退出恢复（2026-10-06，1.2.9）

已将 1.2.9 完整 PKG 覆盖安装到设备603527578，沿用固定自签名证书，未重置 TCC。
Windows 与 Mac 源码版本同步到1.2.9/计数10；此次未构建或安装 Windows 程序。
先前已验证的权限、签名及侧栏提示改动提交为 `997fdca`。

### 实现

- 保留 Enigo 0.5.0 发送输入，增加待释放位图：256个原生键码和3个鼠标按钮，共259字节。
  按下前先写入并同步，释放成功后才清除；不保存输入文字、不重放输入。
- 使用 Rust 标准库文件锁。持有按键的进程保持独占锁，其他进程不会将其正常输入
  当成遗留状态；异常退出时 OS 释放锁，仍在运行或由 launchd 重启的进程接手。
  新输入遇到其他进程持锁会明确失败，避免叠加按住状态。
- GUI 和 standalone helper 每2秒独立检查，恢复不依赖 Executor IPC 是否连通。
  仅在辅助功能可用且当前控制台会话匹配时发送释放；失败记录保留供后续重试。
- 恢复目录按 UID、系统启动 UUID、登录安全会话隔离，目录0700、文件0600；拒绝
  符号链接、硬链接和不正确归属。文件损坏或写入失败时阻止新的按键按下。
- 文字、快捷键、鼠标按钮及旧版逐事件输入均经过恢复封装。Enigo macOS 文字输入
  使用的原生键码0和可能使用的Tab纳入清理，文字内容不写入恢复文件。

实现依据：[Rust 文件锁](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)、
[Enigo](https://github.com/enigo-rs/enigo)；具体输入行为同时核对了本地锁定的0.5.0源码。

### 验证与实机结果

- 恢复逻辑新增9项测试，覆盖真正杀死测试子进程后的锁释放、活进程不被接管、
  失败按下/释放、重试、损坏记录、文件写入失败以及已释放内容不重复恢复。
  测试子进程只模拟记录，不发送系统键盘事件。
- Windows desktop-control：16通过、4项交互式图形测试按默认规则忽略；Mac：20通过。
  Mac桌面 cargo check、完整签名打包及前端类型检查通过。
- 1.2.8升级前、1.2.9升级后在同一个 huayang 所有的 TextEdit 测试文件输入中英文、
  Command-S保存，读取文件核对内容；两版均截图2560×1080 JPEG成功。升级期间
  没有重新申请权限，屏幕录制和辅助功能跨版本保留均得到本次实机验证。
- 实机故障注入：通过正式 MCP 逐事件接口按下右Shift，确认恢复记录及系统Shift
  状态后，只对持有记录的 Bridge 进程发送SIGKILL。GUI PID11101被终止后，helper
  约1.64秒清空记录并恢复Shift状态；helper PID11093被终止后，launchd重启路径
  约0.13秒完成相同检查。后者不是承诺的最大恢复时间。
- 第一次故障观察脚本仅检查右侧Shift码60，未触发杀进程；macOS对该事件的按住
  查询反映在Shift码56。随后先用短按/松开确认行为，再同时检查两侧完成上述测试。
- 两次故障后重新枚举窗口、输入 `CrashRecovered-输入正常`、Command-S保存成功；
  CombinedSession/HIDSystem左右Command、Shift、Option、Control全部未按下，
  待释放文件为空。测试文件保存并正常关闭，GUI已重新打开，临时安装job已移除。

产物：Mac `.build/packages/pixels-agent-bridge-macos-aarch64-debug-setup.pkg`，
71,952,553字节，SHA-256 `d44d2953a0aa26251779805d070e5e0acb582e80568795a12f7e64cbd0968d5c`。
日志：`.build/input-recovery-{mac-tests,desktop-check,package}.log`、
`.build/install-input-recovery-1.2.9.log`。
故障验收操作ID：GUI `5093e575-ba37-4044-b907-5201ac19c6eb`，
helper `8b4ee083-3efe-47cb-ae9e-16ae91d38d59`。

### 仍需明确的边界

桌面变更工具当前明确不支持中途取消；超时或断线不能当作已取消，必须查询原操作，
不自动重放。普通错误清理与异常退出恢复不等于撤销已输入内容。
本方案针对进程退出，不会强行接管仍持锁但主线程卡死的进程；权限被撤销、所有
恢复进程被禁用时也不能保证即时释放。锁屏/注销后的登录控制与会话切换仍未验收，
本轮没有注销或重启用户机器。

## 权限提示位置调整（2026-10-06）

移除主内容区的权限横幅。权限不可用时，仅在左侧“本机服务运行中”上方显示
“权限待开启”（繁体“權限待開啟”、英文“Permissions needed”）；悬停显示缺失
权限的用途，点击切换到设置的应用偏好页，保留详细状态、申请授权、系统设置和
重启入口。设置子页由 App 管理，已经停在其他设置子页时也能正确跳转。
权限全部可用或非 macOS 时不显示此行。查询仍定时刷新，不因轮询重复申请。
启动申请异常只记日志，不再把权限错误作为主页 toast 展示。

验证：前端 TypeScript 检查通过；浏览器模拟验证了侧栏位置、点击回调、权限生效
后隐藏及撤销后恢复、三语言、亮暗色、设置操作及轮询不申请权限。此次未编译
原生程序、未打包安装；Mac 实机仍使用 1.2.8 的旧界面。

继续检查输入恢复代码：现有保护包含断开 helper IPC 后松键、10 秒空闲松键、
SIGTERM 退出前松键。进程强制结束、崩溃或主线程卡死后的恢复仍待实现和实测，
不视为上述正常退出路径已经覆盖；本次未对真实 Mac 注入故障或遗留按键。

## 固定自签名与权限迁移（2026-10-05 晚）

按用户不付费的要求，使用构建机生成并持久保存的自签名代码证书；不使用 Apple
付费证书，不公证，不安装系统信任根，也不关闭 Gatekeeper。实现与备份要求见
[BUILDING.md](BUILDING.md)。私钥只在 huayang 的独立构建钥匙串中，不上传仓库。
本次证书 SHA-1：`BD9EAA0BA8136249F6FA2D8AB7154512E73AB275`（公开指纹）。

统一构建签名 App、Executor 和 MCP，绑定各自固定 ID 与该证书；构建前检查材料，
签名后记录产物哈希，PKG 再校验三者身份。未发现材料时不会静默退回临时签名。
1.2.7 已通过完整 PKG 安装到603527578，不是局部替换；系统 Installer 成功，
App 版本、安装后的 designated requirement 与 HTTP 26035 健康检查均核验通过。
1.2.8 已使用同一证书完整构建并覆盖安装，源码版本计数9同步 Windows。
实机比较1.2.7/1.2.8 designated requirement 完全一致；覆盖升级没有重置 TCC，
升级后2560×1080 JPEG 截图成功，屏幕录制授权保留。用户随后确认已完成重新授权，
1.2.8 正式 helper 的辅助功能也生效，窗口引用、焦点和文字输入均可用。

权限界面使用“当前不可用”描述系统 API 的观测结果，避免把签名不匹配、重启待生效
误判为用户未授权；新增手动重启 App/helper 入口。设置入口改用主线程 NSWorkspace。
helper 在收到 SIGTERM 时停止请求循环并尝试松键，然后退出，供 launchd 重启。
重启入口不清理授权；正常安装升级也不自动重置授权。

迁移中发现系统仍保留旧 `82b6315d…` 临时签名的授权记录，用户打开开关后仍然
返回签名不匹配。已告知用户并用官方 tccutil 针对 `vip.rgaa.pab.desktop` 单独重置
ScreenCapture/Accessibility，随后重新启动正式 App 申请；未直接修改 TCC 数据库。
这次重置是旧签名迁移的故障处理，不放进每次安装或启动流程。
重置后正式 helper 成功枚举14个窗口并截图2560×1080 JPEG，签名不匹配日志消失；
当时辅助功能仍返回不可用；随后已恢复。退出 GUI、仅保留 standalone helper 的
实测中，可写 TextEdit 文件成功输入 `PAB-中文-1.2.8`，窗口截图确认实际内容；
窗口绑定批次 focus + Command-S 完成，直接读取保存文件确认文字一致。
前后 CombinedSession/HIDSystem 的左右 Command/Shift/Option/Control 均为 false。
旧测试文件此前由 root 创建，TextEdit 弹出无写入权限对话框导致焦点变化，操作
明确返回未确认而未重放；已修正该测试文件归属，新的夹具由 huayang 所有。
辅助功能授权的准确完成时刻与升级重叠，没有足够证据单独断言其跨版本保持已经
完整验收；屏幕录制保持已实测。登录前控制、异常崩溃松键仍不标记完成。

验证：Mac 两项签名测试通过（两份不同程序的要求一致、错误证书/篡改拒绝）；
Mac/Windows 14项版本测试通过（含缺失签名不消耗版本）；Mac/Windows Desktop
cargo check 和前端类型检查通过；1.2.7 的六项 PKG 测试含实际构建/展开通过。
浏览器模拟验证了权限持续提示、查询不反复申请、设置目标、重启失败显示、授权/
撤销刷新、三语言及非 Mac 隐藏。NSWorkspace 设置入口仍需正式 App 实机点击验收。

产物均位于 Mac `.build/packages/pixels-agent-bridge-macos-aarch64-debug-setup.pkg`，
新构建覆盖同名文件：1.2.7 SHA-256 为
`dfc98a7e214a079efc2160faf7bb5dbb4205226ab87cbddb5c65b331cac08d56`；
当前1.2.8为 `cf7dea6da7e7ed6f25a09651a42e24918a3fd1833b39fc7b3ac67ff30e1a2cee`。
日志为 Mac `.build/self-signing-tests.log`、`self-signed-{build,upgrade-build,pkg-tests,version-tests}.log`，
以及 `install-self-signed-1.2.7.log`。

## 输入与登录会话修复进展（2026-10-05 晚，1.2.6）

已在设备 `603527578` 编译并完整安装 ARM Debug `1.2.6`，版本计数7；Windows
源码同步该版本记录，本轮没有构建 Windows 安装包。以下为当前状态，后文记录保留
各次验收时的版本和边界，不能将历史通过项视为本轮输入修复已经通过。

### 已观察到的问题与改动

- 重启后只读查询未发现 Command/Shift/Option/Control 残留；本次卡死前缺少直接
  输入故障证据，不能认定与此前 Command 残留是同一个原因。
- 在独立 TextEdit 测试文件上，旧 standalone helper 的 focus 返回已观察到焦点，
  随后的纯文本输入报 AX `-25204`，截图未见文字插入。停止该 helper、让已运行的
  GUI 接管后，同一测试窗口实际插入了 `PAB-GUI-中文`。这证明两种宿主的行为存在
  差异；`-25204` 本身不能直接解释为用户未授予辅助功能。
- standalone helper 增加 AppKit 主线程事件循环，使用 Prohibited 激活策略避免
  抢占前台；IPC 保留在异步工作线程。Enigo 的创建、输入和释放统一调度到主线程。
- legacy 输入记录尝试按下的键和鼠标按钮，即使原生调用部分失败也进入释放清单；
  helper 周期检查连续10秒未收到输入的按住状态并尝试释放，覆盖调用端消失但
  helper 仍存活的情况。此机制不能保证 SIGKILL、进程崩溃或事件循环卡死后的清理。
- 添加独立 LoginWindow LaunchAgent，与现有 Aqua helper 分开；安装、升级和卸载
  脚本包含其生命周期。会话检查同时使用控制台 UID 与 Security Session 的图形/
  活动控制台属性，防止普通 root 后台进程被误判为可输入的登录窗口。
- 后台输入不主动申请 TCC 权限；前台应用仍通过系统接口申请并持续展示缺失权限。
  文字输入只有真正准备发送事件时才标记 action_started，避免预检失败误报已发送。

### 验证与产物

| 检查 | 结果 |
|---|---|
| Mac desktop-control 单元测试 | 11项通过 |
| Mac、Windows Desktop cargo check --locked | 通过 |
| Windows desktop-control | 7项通过，4项图形环境测试默认忽略 |
| Mac PKG 测试 | 6项通过，包含实际构建和展开临时安装包 |
| 三平台归档测试、Shell 语法、plist 与 diff 检查 | 通过 |
| 实机升级 | Installer 成功；Aqua helper running，LoginWindow plist 校验通过，HTTP 26035 /health 正常 |
| 新版 helper 的真实输入与登录前控制 | 尚未验收，不能标记完成 |

Mac 安装包：`.build/packages/pixels-agent-bridge-macos-aarch64-debug-setup.pkg`。
SHA-256：`17586d6a94e77f3c2cb4f55f1b20c5326c3467e2e416d5067e3a8169da8a4e1c`。
日志：`.build/input-loop-build.log`、`.build/install-input-fix-1.2.6.log`、
`.build/input-fix-real-pkg-tests.log`。安装包仍为 ad-hoc 应用、未签名 PKG，未公证。

升级后正式 helper 返回 `screen_recording_permission_required`。22:14 的系统 TCC
日志进一步确认具体原因：`Failed to match existing code requirement for subject
vip.rgaa.pab.desktop and service kTCCServiceScreenCapture`。已保存授权的 cdhash 为
`82b6315d530a99a5dac246d77eb80ccd17a796bf`，当前应用为
`687164300992d034d5d37823b163ffe9d9e19ab2`。这是 ad-hoc 构建更新后的签名要求
不匹配，不能据此指责用户未开启权限。单独重启 helper 后错误仍在，停用 helper
改由 GUI 处理也失败；诊断结束已恢复 Aqua helper。系统日志还发现通过 `/usr/bin/open`
打开设置时触发 AppleEvents entitlement 缺失，设置入口需单独修正并实测。

后续应给构建配置稳定的代码签名身份：开发使用 Apple Development，分发使用
Developer ID Application，并验证跨版本升级仍能匹配授权。当前机器没有可用签名
证书，不能把临时签名包描述为已解决授权持久化。依据见
[Apple DTS 对 ad-hoc 构建身份变化的确认](https://developer.apple.com/forums/thread/819406)。
只读查询系统 TCC 数据库被拒绝后没有重试绕过；未修改 TCC 数据库、清空权限或绕过
系统授权。下一轮先在独立测试窗口验证纯文本实际效果，再验证快捷键/释放和本机
键盘可用性；最后在用户结束当前工作后安排登录窗口、锁屏和会话切换验收。
测试文件 `.build/PAB-input-recovery-20261005.txt` 的 TextEdit 窗口仍保留，待授权后
仅关闭该测试窗口，不终止用户其他文档。未进行新的注销、重启或密码输入。

## 本次提交范围（2026-10-05）

提交已验证的权限申请与持续提示、安装文件权限修复、截图元数据回归测试、
macOS legacy 输入接口路由及对应隔离测试，保留1.2.5/计数6的已使用版本记录。
此前主线程调度实验涉及的输入引擎、批次和指针查询改动暂留工作区，未纳入此提交。
下文对已安装1.2.3 GUI 和1.2.5 Executor 的实测包含这些实验改动，不表示本提交
已经解决输入可靠性；本提交不构成一个新的完整安装包。

## 重启后非键盘功能复验（2026-10-05 08:38–08:44）

使用原生 `pixels.pab_*` 工具连接603527578。Executor 已安装1.2.5，Desktop/helper
仍为1.2.3；本轮未编译、覆盖安装或改变授权。用户自行登录后，CombinedSession
和 HIDSystem 两个事件源的左右 Command/Shift/Option/Control 全部为 false，
flags 为 `0x20000000`；验收结束再次查询仍一致。仅查询按键状态，未发送键鼠输入。

| 范围 | 本轮结果 |
|---|---|
| 系统、进程、磁盘、网络接口、TCP 监听、DNS、登录会话 | 通过；接口数量上限触发明确的 truncated/entry_limit |
| launchd | Executor 服务查询通过；没有启停现有系统服务 |
| 文件 | 中文路径读写、补丁、搜索、目录分页、复制、移动、删除通过 |
| 文件冲突 | 旧 expected_hash 返回 version_conflict；未声明覆盖返回 already_exists；重读内容未被破坏 |
| 二进制传输 | Windows→Mac→Windows 2 MiB 随机文件，三端 SHA-256 一致 |
| ZIP | 原生异步打包、解包终态 completed；解包后的二进制哈希与原文件相同 |
| 命令 | 中文 stdout/stderr 正确；非零退出保留 exit_code=7；200ms 超时和主动取消均达到对应终态 |
| PTY | 新终端中文输出、调整尺寸至110×35、stty 核对和关闭通过 |
| 进程控制 | 专用测试进程 get_process 返回身份，按 macOS audit token 发 SIGTERM 后确认退出；未操作用户进程 |
| Git | 独立临时仓库 status/diff/log 通过；普通用户源码仓库访问失败，见下方边界 |
| 图形 | 显示器枚举、1920×1080全屏 JPEG（180463字节）、673×439窗口 JPEG（15867字节）通过，均未缩放 |
| 窗口控制 | 独立 TextEdit 测试窗口 focus/minimize/restore/close 均观察到目标状态，最终枚举确认窗口消失 |
| Desktop | 从已安装 App 启动主界面，HTTP 26035 /health 返回 status=ok |

二进制 SHA-256：`5825e64043fabef20ff08c584fe9df3dda4f6d25ff02bfa476d0306c6d1c0fbd`。
证据：Windows `.build/mac-postreboot-acceptance-20261005.json`（63项调用/观察记录，
不是63个独立测试用例）。临时目录 `.build/acceptance-20261005-postreboot` 已通过
原生删除工具清理，确认 completed；PTY 和测试窗口均已关闭。

边界与剩余项：

- Executor 以 root 运行，`pab_git_status` 访问 huayang 的仓库时触发 Git
  `dubious ownership`。未添加全局 safe.directory，也未修改用户仓库归属；
  后续需设计明确的用户执行身份及其 Git 凭据使用方式，不能将临时 root 仓库
  测试成功当作普通用户工作区已支持。
- Docker socket 不存在，返回 docker_unavailable，未测容器操作；Apple GPU
  在当前 NVML 后端返回 unsupported，未将空列表算作 GPU 查询通过。
- 文字、快捷键、鼠标点击、输入批次及登录前输入本轮没有验收，之前的事件投递
  和修饰键残留问题仍需修复。未重测 Git 远端推送、服务启停、Intel 实机。

## 登录前控制开发任务（2026-10-05，部分实现、待实机验收）

用户已明确要求实现 Mac 开机登录界面的截图和输入密码进入桌面；当前先完成上述
其他功能验收。本节是完整开发要求；当前进展见文首，**不能视为已验收的能力**。不在文档、日志或
测试夹具中保存用户提供的登录密码。

依据：Apple 的 [PreLoginAgents 示例](https://developer.apple.com/library/archive/samplecode/PreLoginAgents/Introduction/Intro.html)
和 [DTS 关于登录前 agent 的说明](https://developer.apple.com/forums/thread/775067)
支持使用独立的 LoginWindow 会话；RustDesk 的
[agent.plist](https://github.com/rustdesk/rustdesk/blob/master/src/platform/privileges_scripts/agent.plist)
同时配置 LoginWindow/Aqua，可参考其生命周期设计。示例/配置本身不能证明本项目
在当前 macOS 上已具备截图、输入和授权能力，必须实测。

开发顺序与完成标准：

1. 先修复输入事件生命周期和修饰键清理，验证正常结束、取消、失去连接、helper
   切换与异常退出，不影响本机键盘。修复前不把现有输入代码直接搬到登录窗口。
2. 为安装包加入 LoginWindow helper 生命周期，同时保留 Aqua helper。实现主线程
   事件循环、会话类型/身份识别、运行日志目录和安装/卸载回收。不能仅依据
   `/dev/console` 的 UID 判断登录前、锁屏、用户切换等场景。
3. Executor 按活动会话选择 helper；登录前后切换时结束旧操作、失效旧 window_ref、
   释放按键并重新注册。切换中的操作返回明确的未确认/会话变化状态，不自动重放
   密码、按键或提交操作。后台文件/命令连接应持续可用。
4. 沿用 xcap/Enigo 等现有库，验证 LoginWindow 的屏幕录制和输入权限；缺少权限
   返回明确状态并由正常系统授权流程处理。密码仅用于用户主动发起的当前输入，
   不写入命令参数、任务记录、文件、剪贴板或自动登录配置。
5. 区分启动后的 macOS 登录窗口、已登录用户锁屏、快速用户切换，以及 FileVault
   启动前解锁；后者不属于运行中的 Bridge 登录窗口控制能力，不承诺覆盖。

测试计划：会话识别/路由/按键释放的单元测试；LoginWindow/Aqua plist 与安装升级/
卸载回归；真实注销、登录、锁屏解锁、重启后的 JPEG 截图和输入；权限拒绝/撤销、
输入中断、helper 崩溃重启、错误密码不自动重试、多用户切换；每轮前后读取修饰键
状态并确认本机键盘可用。需要注销/重启的实机验收应在用户结束当前工作后进行。

交付条件：既能从登录窗口进入桌面，又不会导致本机按键残留；会话切换、错误状态、
敏感输入不留存均通过测试，才可将该功能标为完成。

2026-10-04 构建更新：macOS 已接入统一版本递增。`bash packaging/desktop/build-macos.sh debug all` 在 Apple Silicon 实机完整生成 ARM/Intel 两套 `1.2.1` Debug tar.gz 和 PKG，同批只递增一次。每种架构的5项PKG测试（含实际重新打包和展开）全部通过，版本计数保持2；13项版本测试、三平台归档回归也通过。安装包仍为未签名、未公证的开发产物，本轮未执行安装或 Intel 实机运行。详情见 [BUILDING.md](BUILDING.md)。

## 实机安装版验收补充（2026-10-04）

通过 Pixels MCP 操作设备 `603527578`（Apple M4、macOS 27.0.1、用户 huayang）。
验收起点确实为已安装的 `0.1.0`，不是源码启动环境。Executor 来自
`/Library/Application Support/PixelsAgentBridge`，Desktop/helper 来自 `/Applications`。
本轮在该机器编译并升级到 ARM Debug `1.2.2`，设备码未变，升级前任务仍可查询。

| 验收范围 | 实测结果 |
|---|---|
| 系统、磁盘、网络接口、连接、DNS、登录会话 | 通过；Desktop 监听 `0.0.0.0:26035`，健康检查正常 |
| launchd 与安装版进程 | Executor、用户 Desktop/helper 正常；本轮只读查询服务，未重测通用服务启停 |
| 文件、目录 | 中文路径读写、目录分页、哈希、内容搜索、补丁、复制移动、ZIP 打包解包通过 |
| 二进制传输 | Windows→Mac→Windows 的 2 MiB 文件与 ZIP 解包结果 SHA-256 均为 `91d3beb88a9b2f778a6c44a1c53b63d3c79931845a9aef84b3fb414610bd1938` |
| 冲突与异常 | 旧哈希补丁、未声明覆盖被拒绝；原内容未变；命令非零退出、超时、取消终态正确 |
| 命令、PTY、进程 | 中文 stdout/stderr、终端中文输入/读取/调整大小/关闭、带身份的测试进程 SIGTERM 通过 |
| Git | 独立临时仓库 commit/status/log/diff 通过；同仓库并发查询明确返回 busy，串行重查正常 |
| MCP 与 Desktop 上报 | 安装版 MCP initialize 从 `0.1.0` 更新为 `1.2.2`；实际 stdio 子进程上线登记、退出移除均通过 |
| 显示器 | 重启恢复后的新版 helper 正确枚举 1920×1080 显示器 |
| 截图、窗口与输入 | 尚未通过：系统权限仍未授予。截图现在返回明确的 Screen Recording 权限错误；等待用户确认系统授权后再做测试窗口的真实操作 |
| GPU、Docker | Apple GPU 当前返回 unsupported；Docker socket 不存在，未将其算作通过 |

验收发现并修复：

1. 应用原先只有权限预检/打开设置，没有原生申请。现在前台应用启动时调用
   `CGRequestScreenCaptureAccess` 和带 prompt 的 `AXIsProcessTrustedWithOptions`。
   同一进程自动申请去重；设置页可主动申请、打开设置，并在回到应用/定时检查时刷新。
   权限预检和后台查询不反复弹框；最终授权由 macOS 用户确认，当前尚未确认弹框及授权后的完整链路。
2. 截图权限失败原先被统一映射为 Unsupported。现在已知权限错误返回 AccessDenied
   与稳定原因，不透传任意 helper 诊断内容。
3. 构建继承限制性权限，App 可执行文件为 0700、Info.plist 为 0600；安装后归 root
   导致普通用户启动失败。归档已规范目录/可执行文件为 0755、普通资源为 0644，
   安装器另加权限规范化。实机已修正安装路径权限并恢复 Desktop/helper；重打包后的
   PKG 也通过展开权限检查。重打包不消耗版本，仍为 `1.2.2`、计数3。

验证：Mac 的权限错误回归1项、桌面控制9项、Desktop cargo check、前端类型检查通过；
Windows 同项权限错误回归和 Desktop cargo check/类型检查通过；三平台归档回归通过，
Mac 实际 PKG 构建/展开在内的5项测试全部通过。没有 Intel 运行、Docker、签名公证或卸载验收。
日志在 Mac 仓库 `.build/mac-permissions-{check,build,repackage,pkg-tests}.log`，
本地工具结果摘要在 `.build/mac-acceptance-results.json`。安装包仍为原文件名
`pixels-agent-bridge-macos-aarch64-debug-setup.pkg`，最终 SHA-256 为
`795bbf36835a2030444b55832b872a75c318eccc0e4d0cba226ea0a33d735924`。

后续步骤：用户确认系统授权 → 重启 helper/App 使授权生效 → 截图/窗口/文字/快捷键/
点击和批量输入实测 → 关闭独立测试窗口。未授权前不向用户现有窗口发送输入。

## 授权后的补验与持续提醒（2026-10-04）

用户已在系统“设备控制和数据访问”中允许 Pixels Agent Bridge 与 pab-executor。
通过正式 helper 的窗口枚举和窗口 JPEG 截图已成功（系统设置窗口及独立 TextEdit
测试窗口），测试窗口最小化、恢复通过。经 Executor/sudo 启动的独立 `--macos-check`
进程仍报告未授权，不能据此否定正在运行的 GUI/helper 已获得的权限；界面提示使用
前台应用自身的权限检测。

- 新增全页面常驻权限提示，未授权时持续显示且不可手动关闭；每2秒、窗口聚焦和
  页面重新可见时检查，授权消失、撤销重新出现，仅显示缺失项。提供申请授权与
  打开对应设置入口。检查不反复调用原生申请接口，不影响仍可用的文件/命令功能。
- 浏览器模拟验收通过：拒绝保留、无关闭入口、不循环申请、部分授权、设置目标、
  完全授权消失、撤销重现、非 macOS 隐藏，以及亮暗主题和三种语言。Windows/Mac
  前端类型检查通过。该 UI 改动已同步两端源码，本次未重新编译打包，已安装版仍为1.2.2。
- 窗口 focus 实测未确认：系统接受操作但2秒内未观察到测试 TextEdit 窗口获得焦点；
  批量输入在第0步停止，后续文字/快捷键均 skipped，测试文件未变。键鼠输入不能算通过。
- 全屏截图尚未通过当前 Windows MCP：远端已生成1920×1080 JPEG，但本机 MCP
  拒绝包含 monitor `desktop_rect` 的响应。窗口截图成功；需核对/更新 Windows 安装版
  MCP 的截图元数据兼容后复测，不能把窗口截图成功当作全屏截图通过。

浏览器脚本与截图位于本机 `.build/mac-permission-ui-test.cjs`、
`.build/mac-permission-{light,dark}.png`。后续优先处理焦点切换与客户端截图兼容，
再验收中文输入、快捷键、点击和批量操作。

## 再次授权后的复测（2026-10-04 20:57）

- 新建独立 TextEdit 测试文件，窗口枚举与 focus 返回 `foreground_observed`，窗口
  JPEG 截图成功（673×439、未缩放）。不能再将这些操作失败归因于用户没有授权。
- 批量操作 focus → Cmd+A → 中文输入 → Cmd+S 在快捷键阶段中断，返回
  `unconfirmed`，没有重放。读取测试文件确认仍为 `PAB RETEST ONLY`。
- 安装版1.2.2 Desktop 主程序崩溃，后台 Executor/helper 仍在。日志已记录
  `The application has the permission to simulate input`；崩溃报告
  `pab-desktop-2026-10-04-205757.ips` 的故障线程为 `tokio-rt-worker`，堆栈为
  `batch_action → Enigo::key → get_layoutdependent_keycode → keycode_to_string →
  TSMGetInputSourceProperty → dispatch_assert_queue → SIGTRAP`。这是输入调用的
  线程问题，不能用再次索要用户权限解决；需修复 GUI 与独立 helper 的输入线程调度。
- 全屏截图远端已生成1920×1080 JPEG，本机现有 `C:\Program Files\PixelsAgentBridge\pab-mcp.exe`
  仍拒绝其带 monitor `desktop_rect` 的响应。当前源码已经允许该元数据；应更新并
  重新加载 Windows MCP 后复测。窗口截图正常，不能据此宣称全屏链路通过。
- 本轮未发送后续键鼠输入，测试窗口关闭后清理测试文件，并重新打开 Desktop。
  崩溃日志保留，复测摘要保存在 Windows `.build/mac-permission-retest.json`。

## 输入线程修复与授权身份排查（2026-10-04 21:49）

- Mac 安装版更新为 `1.2.3`：使用 `dispatch2 0.3.1` 将 Enigo 的创建、
  键鼠/文字调用及释放调度到主线程。覆盖批量输入、单独文字输入、浏览器输入和
  鼠标命中检查。批次等待与调度仍在原请求线程；原有窗口身份/焦点检查、部分失败
  停止与按键释放逻辑保留。GUI 主事件循环处理队列，独立 helper 在 main 的
  `block_on` 内直接处理请求，不向没有事件循环的后台线程派发主队列等待。
- Windows `1.2.4` 安装包已生成，包含当前支持 monitor `desktop_rect` 的 MCP。
  新增协议回归覆盖原分辨率/Retina 逻辑矩形、多显示器负坐标、错误显示器和矩形
  原点不一致；未通过运行中的旧版 MCP 验证全屏链路，需更新并重启客户端后复测。
- Mac desktop-control 9项、protocol 30项测试通过；desktop-control 严格 Clippy
  通过。Windows desktop-control 7项通过（4项交互测试未执行），protocol 30项
  通过。版本13项、归档回归、实际 Mac PKG 构建/展开5项通过。
- Mac PKG 安装成功，已安装 App 显示 `1.2.3`，Executor/helper/App 恢复、
  HTTP health 正常；ARM PKG SHA-256 为
  `09a69d0f4a0f76b04102dd0a30459603f1335b41efd8a3d0d5ee562ec5ee14b5`。
- 升级后用户报告“已经授权，仍显示未授权”。重启 App/helper 仍返回权限错误。
  系统 tccd 日志明确记录 ScreenCapture 和 Accessibility 两项
  `Failed to match existing code requirement`：旧授权 cdhash `881a3806…`，
  当前安装版 cdhash `82b6315d…`。这是临时签名更新造成的身份不匹配，界面并非
  单纯没有刷新。`security find-identity -v -p codesigning` 返回0个可用身份。
- 已仅对 `vip.rgaa.pab.desktop` 用系统 `tccutil reset` 重置这两项失效记录，
  重启并触发新版的原生申请，等待用户在系统中确认。未修改 TCC 数据库，未
  自动授予权限。真实快捷键/中文输入尚未完成，不将编译通过当作输入验收通过。
- 固定签名是后续更新保留授权的前提。开发构建可使用 Apple Development，分发
  使用 Developer ID Application；Tauri 支持通过 `APPLE_SIGNING_IDENTITY`
  或 `bundle.macOS.signingIdentity` 配置。证书须由产品方配置，不能用仅固定
  bundle ID 或伪造“已授权”代替。无需把重置所有用户权限加入每次安装流程。

参考：[Apple 对临时签名导致授权失效的说明](https://developer.apple.com/forums/thread/819406)、
[Tauri macOS 签名配置](https://v2.tauri.app/zh-cn/distribute/sign/macos/)。
本轮日志：Mac `.build/mac-input-fix-{test,build,pkg-test,clippy}.log`、
`.build/input-fix-install.log`；Windows `.build/mac-input-fix-windows-build.log`。

22:00 收尾：Windows `1.2.4` 原名安装包已静默安装，退出码0，三个已安装
EXE 的 SHA-256 与构建记录一致。安装后核验脚本在 PowerShell 7 下找不到
`Get-ScheduledTask`；通过 CIM 补验确认 Executor Running、supervisor 状态4
（Running），Default/Winlogon helper 与 Desktop 主程序均启动。不是安装失败。
证据在 `.build/mac-input-fix-install-local/verified.json`。安装结束旧 MCP 进程，
当前 Codex 会话尚未重建 stdio；需要重启会话载入新版，再验收 Mac 全屏截图与
输入。此前已同步两端源码版本为1.2.4/计数5，Mac 安装产物保持1.2.3；本收尾
段落写于 MCP 退出后，尚未同步 Mac 文档。Mac 新授权仍待用户系统确认。

## 新会话复测（2026-10-05 07:50–07:56）

原生 `pixels.pab_*` 工具重新连通603527578，本机新版 MCP 进程正常。
Mac 安装版仍为1.2.3；没有重编译、覆盖安装或再次重置授权。

- 屏幕录制/辅助功能预检已通过。开始时出现 `Display 1 Shield`、无可控制窗口，
  默认截图日志为 `selected monitor is unavailable`。`caffeinate -u -t 60` 唤醒
  显示器后恢复，不能归因为权限再次失效；没有证据确认当时是密码锁屏。
- 全屏 JPEG 1920×1080通过：指定显示器与默认显示器均成功，MCP 返回原分辨率
  图片与文件，正确接受 monitor `desktop_rect`，未再出现旧客户端元数据拒绝。
  指定显示器截图240330字节，默认截图211696字节。独立 TextEdit 窗口截图
  673×439成功。窗口枚举、focus、最终 close 均通过实际状态校验。
- 快捷键已不崩溃：focus → Cmd+A → 中文文字 → Cmd+S 四步均返回 API 已接受，
  Desktop PID保持、HTTP health正常；但磁盘文件仍为 `PAB FIXTURE ONLY`，
  截图文字也未变化。因此**输入内容验收失败**，不能用 completed 状态代替效果。
- 单独 `pab_type_text`、新启动 GUI 下的点击/文字批次仍无可见输入。批次中添加
  等待未解决。日志出现 `UCKeyTranslate failed with status: -25340`，它是线索，
  尚不足以解释 Unicode text 和点击同时无效，未直接认定其为所有输入问题的根因。
- 系统 TCC 对当前 GUI 的 `kTCCServicePostEvent` 明确返回 `authValue=2`、
  `authReason=4`（应用与 WindowServer 两次检查），未见本轮新的签名不匹配。
  不要求用户重复授权，不在未查清前修改授权状态。
- 关闭 GUI 后重启独立 helper：focus成功，点击/文字在 Accessibility 校验阶段
  返回 -25204；后续批次步骤 skipped。另发现 `open -a` 可能只激活同一 bundle
  的 helper，`open -n -a` 才重新启动 GUI。已恢复 GUI 和 HTTP 服务。
- legacy key 工具返回 Unsupported，不能宣称旧输入接口通过。
- 测试仅操作 `.build/pab-input-acceptance-20261005.txt` 的独立窗口；关闭窗口
  后删除临时文件。工具结果保存在 Windows `.build/mac-input-retest-20261005.json`。

剩余问题：输入 API 接受但实际事件未到达目标应用、独立 helper 的 Accessibility
调用失败、睡眠显示器错误被对外统一显示为 Unsupported。下一步以独立测试窗口
定位事件源生命周期、GUI session/事件投递与键盘布局转换，不把重新授权当作修复。

### 本机键盘无法输入的恢复（2026-10-05）

用户报告 Mac 本机键盘无法输入后，停止所有输入验收。只读查询发现左 Command
（keycode 55）在 CombinedSession/HIDSystem 两个事件源中都保持按下，flags 为
`0x20100108`。停止 GUI/helper 后仍未清除。此前 Command 快捷键崩溃可能留下
按键状态，但当前证据不足以确认唯一来源。

- 修复 Executor legacy DesktopInput 在 macOS 被 Windows-only 条件提前拒绝的
  路由错误；SecureAttention 仍仅支持 Windows。Mac 隔离测试
  `macos_key_release_reaches_helper_but_secure_attention_is_rejected` 显式运行通过，
  不连接已安装 helper、不发送真实输入。
- 按版本规则预留1.2.5（build_count 6），仅构建并更新 Mac Executor，GUI/helper
  保持已授权的1.2.3；没有生成1.2.5完整安装包。两端源码版本同步。
- 原生工具的 Command key-up 返回 applied，但系统仍报告按下，不能当作恢复。
  Python 诊断进程无 PostEvent 权限，未发送事件；System Events 拒绝 osascript
  松键（1002）。未修改 TCC 数据库或重置应用授权。
- 用户明确允许重启且确认无需保留未保存内容。系统启动时间已变为
  2026-10-05 08:20:39，原生 MCP 已重连、后台命令正常；控制台尚无用户登录。
  重启后的用户会话按键状态和本机输入仍需验证；不继续快捷键/文字输入测试。

## 目标与边界

覆盖现有 60 个 MCP 工具、Desktop、Executor、后台运行和安装卸载。
Windows/Linux 既有原生后端和安装脚本保持不变。macOS 新增代码通过
目标平台条件编译隔离；共用文件只增加必要的平台分派、分发入口和 Retina
截图元数据校验兼容，不改变原有 Windows/Linux 截图负载。

用户授权后可实现的能力纳入实现；系统保护禁止、应用不提供原生接口或无法
安全保持目标身份的操作明确失败，不伪造成功，不关闭 SIP 或绕过 TCC。
缺少硬件、服务器、签名证书或用户授权导致的未验证项单独记录，不能算已验收。

## 实施计划

1. 平台隔离：撤出此前共用 Unix 脚本中的未提交 macOS 改动，独立实现安装包。
2. 原生构建：检查 workspace、Tauri、前端；Apple Silicon 原生构建，Intel 记录检查范围。
3. 安装运行：原生 app、launchd、用户 helper、本地访问凭据、Finder 配置、升级卸载。
4. 基础功能：设备、命令、文件、终端、任务查询、取消、日志与持久化。
5. 系统功能：系统/磁盘/网络/DNS/进程；macOS 登录会话、进程身份、launchd 管理。
6. 桌面功能：权限检测、屏幕和窗口截图、显示器、窗口控制、Unicode、键鼠、批次。
7. 集成：Git、Docker、本地 PATH、凭据与 socket、MCP 注册和桌面状态提示。
8. 验收交付：测试、构建、打包、跨平台差异审查、使用说明和受限清单。

## 验收原则

- 原生编译和前端构建通过；新后端有针对身份、解析、权限失败的测试。
- 复用仓库测试验证文件、终端、Git、Docker、QUIC 和 stdio；需外部服务的测试
  与可隔离执行的测试分开报告。
- 破坏性测试仅操作临时目录、测试子进程和临时服务。
- 不修改真实用户的系统服务或 Git 仓库来验证控制功能。
- 桌面测试不随意向当前用户窗口发送输入；使用明确的测试窗口或权限预检。
- 每个范围标注已验证、实现但待环境验证、系统限制三种状态之一。
- 收尾检查 Windows/Linux 后端及安装脚本无修改；共享逻辑的必要改动单独说明。

## 实现与验证结果（2026-10-03）

代码适配已完成；未具备权限或外部环境的验收不计为通过。此次在 Apple Silicon
macOS 上执行自动化测试，Intel 做编译检查和 Release 交叉打包，不代表 Intel 实机验收。

| 范围 | 实现 | 已验证与限制 |
|---|---|---|
| 原生构建 | 独立 macOS `.app`、ARM/Intel 条件编译 | 两种架构均通过核心、桌面端编译，并生成 Release tar 包 |
| 安装、升级、卸载 | 独立脚本、系统 LaunchDaemon、Aqua LaunchAgent、本地凭据 | Shell/plist 校验和三平台打包回归通过；未实际安装正式服务 |
| 设备、命令、任务、日志 | 复用跨平台 Executor/Bridge | ARM 自动化、隔离 QUIC 和 stdio 通过；真实部署连接待验收 |
| 文件、目录、归档 | 复用既有工具，识别系统 `/tmp`、`/var`、`/etc` 别名 | 134 项 Executor 测试通过；自建符号链接仍拒绝 |
| 交互终端 | 原生 PTY + `/bin/zsh -l -i` | PTY 输出、中文、分段读取等自动化通过 |
| 系统、磁盘、网络、DNS | 复用跨平台采集 | 平台采集与隔离网络测试通过 |
| 登录会话 | macOS utmpx、UID、控制台会话 | 原生只读查询通过；记录反映 utmpx 登录，不保证覆盖全部 GUI 进程 |
| 进程终止 | audit token 固定身份、内核身份校验发信号、kqueue 观察退出 | 错误身份不杀进程、只终止自建测试子进程通过 |
| 服务管理 | launchd 查询、启停、重启、启用、禁用 | 原生查询和临时 LaunchAgent 完整生命周期通过，已清理 |
| 显示器、截图 | xcap、Retina 逻辑坐标元数据、截图像素区域裁剪 | 显示器枚举、元数据映射测试通过；真实截图因权限跳过 |
| 窗口、Unicode、键鼠和输入批次 | 公共 Accessibility API + Enigo、保留窗口身份、输入状态清理 | 原生编译、权限预检、键码/几何测试通过；实际桌面操作因权限跳过 |
| Git | 复用原生 Git 和任务持久化 | 临时仓库、本地远端和隔离协议测试通过；真实 SSH/凭据待环境验证 |
| Docker | 复用 Bollard/本地 Unix socket | HTTP 替身和协议测试通过；当前无可用 Docker Engine，真实容器跳过 |
| 桌面设置、MCP | macOS 权限面板、Homebrew PATH、安装版 MCP 定位、Finder 配置 | 前端构建、桌面 Rust 测试和真实 MCP 子进程重连测试通过 |
| 签名、公证 | `.app` ad-hoc 签名 | 无 Developer ID/公证凭据；不是已公证的公开分发包 |

### 平台隔离与兼容性

- `packaging/desktop/unix/`、`packaging/desktop/windows/` 以及 Windows/Linux 原生
  后端未改动。新增打包测试同时验证 Windows ZIP、Linux tar 和 macOS app tar。
- Windows/Linux 下权限面板不显示；新增 Rust 后端、终端选择、系统路径别名处理和
  Homebrew 查找均通过平台条件隔离。没有在本机运行 Windows/Linux 程序，不能据此
  声称已完成两平台运行回归。
- Retina 截图复用已有可选 `desktop_rect` 字段表示逻辑桌面范围，允许显示器截图
  携带它。Windows/Linux 原有请求、响应、字段含义不变；**连接 Mac 时，请同时更新
  MCP、Executor 和 Desktop/helper**，旧版可能拒绝该元数据。
- 截图 `region` 仍以原始图像像素计，桌面点击坐标以逻辑点计；使用返回的
  `preview_to_desktop` 映射，不把 Retina 图片像素直接用作点击坐标。负原点、多屏
  映射已有纯数据测试，物理多屏未验收。
- 保留现有 60 个 MCP 工具接口，没有添加仅 Mac 可见的工具或改变服务名校验规则。

## 构建与使用

### 双击安装 `.pkg`（2026-10-04 更新）

已新增 `packaging/desktop/build_macos_pkg.py`，将校验过的 tar 包封装为原生 Installer
安装器；默认地址与 Windows NSIS 完全一致：

- Control：`wss://pab.rgaa.vip/control`
- Relay：`https://pab-relay.rgaa.vip`

已合并新版协议（`695aa1c`），部署 UUID 已完全移除。macOS 安装、PKG 打包参数、
预置配置和测试均同步删除该字段，不再需要用户提供 UUID。旧版 tar 包不能混用。

构建新版两种架构的 tar 包后，在仓库根目录执行（此打包器需要 Python 3.12+）：

```sh
python3.13 packaging/desktop/build_macos_pkg.py --arch aarch64
python3.13 packaging/desktop/build_macos_pkg.py --arch x86_64
```

生成 `pixels-agent-bridge-macos-{aarch64|x86_64}-release-setup.pkg` 和各自的 SHA-256
清单。双击、输入管理员密码即可安装，不需要在目标 Mac 执行终端命令或填写部署参数。
安装器会检查原生架构、当前登录用户及系统卷，复用现有安装逻辑，配置后台服务和
该用户的访问凭据。屏幕录制/辅助功能依然必须手动授权。请先退出旧 Desktop/MCP。

安装器使用 scripts-only 组件，receipt 不包含完整已安装文件清单，仍使用
`uninstall.sh` 卸载；安装失败无自动回滚。`--sign` 支持 Developer ID Installer
证书；当前没有该证书，不会把未签名、未公证的包描述为已公证发行版。

验证覆盖参数、Windows 地址一致性、Shell 转义、归档路径和校验和，并构建/展开临时
`.pkg` 检查内容和不含部署 UUID 的配置；测试包不执行安装，结束后清理。
ARM 和 Intel 分别执行上述测试，均为 5 项通过；解包后再次核验全部二进制架构和
app 签名完整性。Windows/Linux 安装脚本与 Windows NSIS 打包器均未改动。

本次新版源码打包回归：`pab-bridge`、`pab-executor`、`pab-protocol` 共 256 项
测试通过，2 项默认忽略；Desktop 使用新构建的 ARM Release MCP，8 项全部通过。
日志为 `.build/macos-package-regression-current.log` 和
`.build/macos-desktop-package-tests-current.log`。下文 2026-10-03 的全仓库记录保留作
适配历史，本次没有重新执行需要 PostgreSQL 或桌面权限的环境验收。

### 源码构建与 tar 安装

需要 Xcode Command Line Tools、仓库固定版本的 Rust、Node/npm、Python 3.12+。
本次开发环境补齐了 Homebrew rustup、Node、Python 3.13 和
Rust Intel target；不由安装包自动安装这些开发工具。

在 macOS 仓库根目录执行：

```sh
npm --prefix apps/desktop ci
bash packaging/desktop/build-macos.sh debug all
# 默认只编译当前架构的开发包：bash packaging/desktop/build-macos.sh
# 正式双架构：bash packaging/desktop/build-macos.sh release all
# Intel 交叉构建：bash packaging/desktop/build-macos.sh release x86_64
# Apple Silicon：bash packaging/desktop/build-macos.sh release aarch64
```

脚本调用统一构建入口，在编译前递增一次版本，构建 Executor、MCP、前端和原生 app，
再打包 tar.gz 和 PKG 到 `.build/packages/`。`all` 的 ARM/Intel 共用该版本，打包不再次递增。
直接入口为 `python3.13 scripts/build.py macos --macos-arch all --package`；使用便捷脚本
可以自动选择 Python 3.12+ 和 Homebrew 工具路径。以仓库所属用户执行，不以 root 编译。
ARM 包名为 `pixels-agent-bridge-macos-aarch64-release.tar.gz`；指定 `x86_64` 可在
Mac 上交叉构建 Intel 包（需安装相应 Rust target）。相邻 `SHA256.json` 记录
Release 校验和，Debug 使用独立清单。构建产物使用目标三元组子目录以隔离架构。
配置的最低系统版本是 macOS 12，但旧系统运行兼容性尚未实测。

解压完整包，以实际桌面用户执行（替换两个服务器地址，不要直接以 root 登录安装）：

```sh
sudo bash install.sh wss://CONTROL_ENDPOINT https://RELAY_ENDPOINT
```

安装前退出旧 Desktop 及使用 MCP 的客户端。升级使用新完整包重新执行同一命令；
脚本停止 PAB helper/Executor 后替换文件，保留数据。不提供安装失败的事务回滚，
升级前可备份下列数据目录。不要混合新旧二进制。

| 内容 | 安装路径 |
|---|---|
| GUI + 用户 helper | `/Applications/Pixels Agent Bridge.app` |
| MCP、Executor、配置、脚本 | `/Library/Application Support/PixelsAgentBridge` |
| 机器身份、任务、日志 | `/Library/Application Support/PixelsAgentBridgeData` |
| 当前用户 Bridge、设置、本地凭据 | `~/Library/Application Support/PixelsAgentBridge` |
| 机器后台服务 | `/Library/LaunchDaemons/com.pixelsagentbridge.executor.plist` |
| 登录后的桌面 helper | `/Library/LaunchAgents/com.pixelsagentbridge.session-helper.plist` |

Finder 可直接打开 app；无需从终端注入部署变量。已有用户设置优先于机器级配置。
MCP stdio 命令为 `/Library/Application Support/PixelsAgentBridge/run-mcp.sh`，将整个
路径作为客户端配置中的一个 command 字符串。设置页中的 AI Agent 集成会查找
Homebrew Codex CLI；未安装 Codex CLI 时可手动注册这个 stdio 入口。

卸载（先退出 Desktop/MCP 客户端）：

```sh
sudo bash '/Library/Application Support/PixelsAgentBridge/uninstall.sh'
```

卸载只移除 app、PAB 程序和两个 launchd 作业，保留机器/用户数据，便于恢复安装。
其他用户不自动获得本地敏感信息访问；需要另行以管理员身份签发其 `local-access.key`。

### 权限和运行账户

- 屏幕录制和辅助功能需用户在系统设置中授予 **Pixels Agent Bridge**。设置页可
  查看状态、打开对应面板；未授权明确报错，不弹出循环请求、不绕过 TCC/SIP。
  屏幕权限变更后重启 app，并重新登录以重启后台 helper。
- 可无交互检查：`'/Applications/Pixels Agent Bridge.app/Contents/MacOS/pab-desktop' --macos-check`。
  本次结果是 ARM、活动控制台、1 个显示器，屏幕录制/辅助功能均 `false`，本地访问
  凭据未配置。按用户允许跳过权限受限项的要求，没有发送真实键鼠事件或申请授权。
- 图形操作只针对当前活动控制台用户；不支持登录前桌面、FileVault 解锁、绕过锁屏
  或 Windows 安全注意序列。受保护窗口、无法唯一匹配 AX 的窗口、应用不支持的
  操作会失败。最大化是调整窗口几何而非进入全屏 Space；应用限制几何时返回未确认。
- Executor 是机器级 root 服务，桌面捕获/输入由用户 helper 完成。Git/SSH 凭据、
  工作目录权限、Docker socket 属于 **Executor 的运行身份**，不能假设继承桌面
  用户的 SSH agent、Docker CLI context 或用户 PATH。
- Docker 需已有本地 Engine，支持本地 `DOCKER_HOST=unix:///绝对路径`；必要时由
  管理员在机器 `settings.env` 配置，并重启 Executor。不要放宽 socket 权限来绕过
  访问控制。本次没有安装 Docker、修改个人 Git 凭据或实际部署 PAB 系统服务。

### macOS 服务管理约定

使用 `system:标签`、`gui:UID:标签`、`user:UID:标签`，例如
`gui:501:com.example.worker`。列表列出 Executor 所在 bootstrap 域；显式名称可查询
其他域，仍受系统权限约束。启停需在标准 LaunchAgents/LaunchDaemons 目录找到同名
且 Label 一致的 plist。禁止控制 `com.apple.*` 和 `com.pixelsagentbridge.*`。

Stop 使用 bootout 卸载当前作业，Start 用 bootstrap/kickstart；Enable/Disable
只改变 launchd 禁用标志，不承诺立即启动/终止进程。Restart 用 kickstart -k 并观察
新 PID。launchd 可能按默认约 10 秒节流，建议服务控制超时设置 20 秒或更多；请求
被接受但状态未观察到时返回 `unconfirmed`，不能当成未发生或自动重试。

## 验收记录与复现

以下命令在仓库根目录执行；日志保存在本机 `.build/macos-*.log`（不纳入 Git）。

```sh
cargo test --workspace --exclude pab-server --lib --bins --tests --offline
cargo test --workspace --exclude pab-server --doc --offline
cargo test -p pab-server --lib --offline
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --offline
cargo test -p pab-os-control temporary_launch_agent_lifecycle -- --ignored
PAB_MCP_SMOKE_EXE="$PWD/target/debug/pab-mcp" cargo test \
  --manifest-path apps/desktop/src-tauri/Cargo.toml \
  real_mcp_stdio_processes_register_before_tools_and_clean_up_on_exit -- --ignored
python3 packaging/desktop/test_packaging.py
cargo clippy -p pab-os-control -p pab-os-sessions -p pab-desktop-control --all-targets -- -D warnings
cargo check --workspace --all-targets --target x86_64-apple-darwin --locked
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --target x86_64-apple-darwin --locked
```

- 非 Server workspace：29 组测试，338 通过、4 默认忽略；doctest 单独重跑通过。
- Server 纯单元：6 通过；Desktop 最终使用 Release MCP 并启用 ignored 测试，
  8 项全部通过。临时 launchd 服务测试另外显式运行通过（约 10.9 秒）。
- 全仓库 Clippy（不加 `-D warnings`）通过；原有 `too_many_arguments` 等警告
  导致全仓库严格模式不通过。新增三个原生后端的严格 Clippy 通过。
- 完整 Server 数据库集成测试需 PostgreSQL `DATABASE_URL`，当前未配置；没有把
  缺少数据库导致的失败视为 macOS 回归，也未改动数据库服务。
- `scripts/verify_iroh_vendor.py` 仍失败：仓库既有 `vendor/iroh-relay/src/server.rs`
  和 `src/server/testing.rs` 与记录清单不一致。已核对相关文件工作树与 Git HEAD
  一致，本次未改 vendor/补丁清单，避免把无关跨平台修复混入本任务。

### 最新交付（2026-10-04，无部署 UUID）

产物位于 `.build/packages/`：

| 平台 | 双击安装包 | 大小 |
|---|---|---|
| Apple Silicon | `pixels-agent-bridge-macos-aarch64-release-setup.pkg` | 28,672,232 字节 |
| Intel | `pixels-agent-bridge-macos-x86_64-release-setup.pkg` | 30,873,068 字节 |

- ARM PKG SHA-256：`1fbe26fab2e2d6c3ec734e052f6d8515c9c4246b6a3e275c512979909af4ce2a`。
- Intel PKG SHA-256：`d14f9d6c3871e5051fb9b3fc14354b0edbfef5362b8eb664ade1cadec596113d`。
- 分别与 `SHA256-macos-aarch64-setup-release.json`、
  `SHA256-macos-x86_64-setup-release.json` 核对一致；清单包含实际内置服务器地址。
- 两个同名架构的 Release `.tar.gz` 也已重新生成，校验和见 `SHA256.json`。
  2026-10-03 的旧包已被新版替换，不能再使用旧哈希验收。
- 两个架构的 app、Executor、MCP 均核验为对应 Mach-O；解包后 app 的
  `codesign --verify --deep --strict` 通过。此处是 ad-hoc app 完整性校验，
  **不是** Developer ID 签名或公证；`pkgutil` 确认两个 PKG 均未签名。
- 每个架构均通过 5 项 PKG 测试（含实际构建/解包）；三平台打包回归、Shell 语法、
  plist、`git diff --check` 通过。系统 Installer 能读取 ARM 包的安装选项。
- 新版 ARM app 的只读诊断成功；没有实际安装/启动后台服务，没有 Intel 实机运行验收。
- 安装包未公证，其他 Mac 的 Gatekeeper/企业安全策略仍可能阻止启动；正式公开
  分发应提供 Developer ID 签名和公证，不要关闭系统安全保护来解决。

完成范围是仓库代码、原生安装包和现有环境可执行的验收。未做真实部署安装、
真实 Docker Engine、实际图形操作、Intel 实机或旧版 macOS 验收；原因和边界见上文。
