# 发布前统一冒烟测试

本目录是发布测试的唯一入口。目标是让拿到完整源码的维护者，使用自己准备的测试环境复现检查；不需要原开发者的机器、账号、私有 CA、历史验收报告或运行目录。

**测试框架就绪不等于产品已经通过。** 用例定义没有历史“通过”状态。每个候选版本重新产生结果，缺环境、跳过、未执行和结果不确定均不能通过发布门禁。

## 目录与分发

| 路径 | 内容 | Git |
|---|---|---|
| `cases.toml` | 功能、平台、前置条件、步骤、预期、清理要求 | 提交 |
| `tool-coverage.json` | MCP 工具覆盖定义；真实 tools/list 必须完全匹配 | 提交 |
| `run.py`、`adapters.py`、`unittest_adapter.py` | 执行、适配现有测试、判定和报告 | 提交 |
| `web_fixture.py`、`requirements.txt` | 可独立创建的 HTTPS/PostgreSQL 测试环境 | 提交 |
| `live_*.py`、`mcp_client.py`、`native-lab.example.toml` | 使用正常配对与独立 MCP 会话的真机测试入口和设备模板 | 提交 |
| `fixtures/` | 浏览器合成页面、回环 HTTP 夹具和只读日志解析探针 | 提交 |
| `tests/` | 测试框架自身的回归测试 | 提交 |
| `lab.example.toml` | 无密码、无真实设备信息的配置模板 | 提交 |
| `local/` | 本机环境配置、隔离服务、测试证书和候选产物 | 忽略 |
| `runs/` | 每次运行的报告、观察记录、日志及浏览器证据 | 忽略 |
| `tmp/` | 临时开发验证数据 | 忽略 |

产品已有的单元测试、Playwright 用例和固定夹具仍保留在其所属模块，统一入口调用它们，不复制第二份测试实现。这些文件都是源码分发的一部分。旧的临时验收记录不作为任何新版本的通过证据。

远程桌面卡顿、编解码和网络耗时的专项验收见 [REMOTE_DESKTOP.md](REMOTE_DESKTOP.md)，原生合成帧检查入口为 `remote_desktop_worker.py`。

测试框架与项目代码采用仓库根目录的许可证；第三方依赖保留各自许可证。分发时应包含完整仓库源码、锁文件和固定测试夹具。当前不是一个可脱离仓库单独运行的 `smoke/` 压缩包。

## 环境准备

- Python 3.11+：运行器只用标准库；完整自检、发布检查与 HTTPS 夹具需先执行 `python -m pip install -r smoke/requirements.txt`。
- Rust 使用仓库固定工具链及 Cargo.lock；Node/npm 使用各应用的 package-lock.json，执行对应目录的 `npm ci`。
- 浏览器测试需要本机 Chrome/Edge，或安装对应 Playwright 浏览器；Web 浏览器可在 TOML 中选择。
- 真实 Web 测试需要 Docker 和已拉取的 PostgreSQL 镜像，以及已构建的 Server 和 Web 产物。镜像由使用者显式指定，不依赖私有镜像仓库。
- 原生功能在 Windows、macOS、Linux 或 Android 的实际支持平台执行。产品未承诺的平台能力不可伪造测试通过。
- Git 测试需要测试主机实际安装 `git`，不能只在源码宿主机安装。纯编译 Docker 镜像不保证包含 Git；运行 Git 用例前先检查容器内 `git --version`。
- 扫描 PDF 测试先运行 `python smoke/native_runtime.py`，将安装包使用的、经过 SHA-256 校验的 PDFium 与 OCR 数据准备到 Cargo 测试程序旁。首次运行可能下载固定版本的公共依赖；不会读取本机安装目录。
- 使用专用测试账号、数据库、目录、应用和双 Relay。原生安装/升级、切网、锁屏、权限、硬件和第三方 OAuth 需要具备相应环境。

复制 `lab.example.toml` 到 `smoke/local/lab.toml` 并填写。所有路径相对仓库根目录；应用和测试环境配置用 TOML，结果报告用 JSON。

运行器串行执行套件。Cargo 适配器使用 1 个作业，运行前检查其他原生构建；嵌套 CMake 和 Rayon 也限制为 1。其他会话仍需共同遵守**每台机器总编译并发不超过 10**，发现资源冲突应等待。检查本身不安装应用、不升级手机、不修改线上服务。

Cargo 检查显式使用 `fixtures/client-defaults.toml` 的空默认配置，避免依赖维护者本机的私有地址和 CA。集成测试运行时再注入它自己创建的配置。`AUTO-13` 是单独显式启用的真机项目，会临时切换并恢复手机连接。

## 查看范围与快速检查

从仓库根目录运行：

```sh
python smoke/run.py plan --target windows --profile release
python smoke/run.py preflight --target web --profile release --lab smoke/local/lab.toml
python smoke/run.py run --target web --profile quick --arch x86_64
```

目标支持 `windows`、`macos`、`linux`、`linux-desktop`、`android`、`server`、`web`。架构必须显式选择，每个实际分发目标分别验收。`preflight` 只检查依赖/环境边界，不算功能测试。

| 范围 | 含义 |
|---|---|
| `quick` | 框架、构建/打包、前端模型或桌面交互的快速检查 |
| `release` | quick + 真实链路、所有适用功能及安装升级观察 |
| `soak` | release + 至少 8 小时运行、并发与故障测试 |

首次正式发布及连接、生命周期、资源管理的重要改动必须执行 `soak`。每次普通候选发布至少执行 `release`。快速检查不能替代发布验收。

## 独立 Web 测试环境

先按 BUILDING.md 构建候选产物；以下命令只使用已有产物，不重新分配版本。`--binary` 和 `--assets` 指向实际构建输出：

```sh
python smoke/web_fixture.py create --binary target/debug/pab-server.exe --assets apps/web/dist --postgres-image postgres:18.6
```

macOS/Linux 将二进制路径改为对应平台的 `pab-server`。镜像版本可以替换成项目支持且预先拉取的 PostgreSQL tag/digest，正式验收建议固定 digest。

命令创建带本次 UUID 标签的容器、独立数据库、测试证书和 Server 副本，输出配置路径。将该路径写入 `lab.toml` 的 `web.fixture_config`。现有真实浏览器测试统一使用 `https://127.0.0.1:38623`；端口占用时会失败，不停止占用者。数据库只绑定回环地址。

服务端的 `AUTO-14` 也使用这一所有权校验后的数据库，运行 Server/Relay 集成测试，包括真实 WSS、双 TLS Relay、强制中继和维护切换。数据库 URL 只传给测试子进程，不写进公开配置模板。

```sh
python smoke/web_fixture.py info --config smoke/local/web-RUN_ID/pab-server.toml
python smoke/web_fixture.py restart --config smoke/local/web-RUN_ID/pab-server.toml
python smoke/web_fixture.py stop --config smoke/local/web-RUN_ID/pab-server.toml
python smoke/web_fixture.py destroy --config smoke/local/web-RUN_ID/pab-server.toml
```

`stop` 停止该夹具的 Server；`destroy` 删除经 ID 和标签核对的该次测试数据库容器及卷，保留本地证据。创建中断时查看该目录的 `owner.json`，不要广泛删除容器。不得将此配置替换为生产连接串。真实 GitHub OAuth 流程需要独立的测试应用，不通过伪造 OAuth 响应宣布真实授权通过。

## 执行候选版本

```sh
python smoke/run.py run --target windows --profile release --arch x86_64 --version 1.2.3 --artifact PATH_TO_EXACT_INSTALLER --lab smoke/local/lab.toml
```

命令输出 `smoke/runs/<run-id>/report.json`。记录提交号、完整源码指纹、用例定义指纹、产物大小和 SHA-256。未完成的人工项目保持 `not_run`；自动项目缺环境为 `blocked`。`--case AUTO-01 --case AUTO-02` 可用于局部诊断，但报告仍保留全部必测项，不会因为选得少而通过门禁。

```sh
python smoke/run.py report --report smoke/runs/RUN_ID/report.json
python smoke/run.py resume --report smoke/runs/RUN_ID/report.json --case AUTO-10 --lab smoke/local/lab.toml
```

恢复只执行显式选择的自动用例，不自动重放业务请求。修改源码/用例或换包后创建新运行。中断的子进程和远程操作必须核查原 ID、清理后再开展新运行；不确定结果不能当成操作失败重发。

## 真机观察与逐工具覆盖

已配对的专用桌面测试设备可通过 `live_native.py` 执行一组真实 MCP 检查：严格 Relay 路径、服务及用户身份、Unicode 命令和文件哈希、旧哈希写入拒绝、复制移动、搜索归档以及用户终端。配置使用 TOML 的 `[[device]]` 数组，每项填写 `name`、`code` 和 `os`（`windows` 或 `macos`）。真实设备配置放在 `local/`，不提交账号或设备信息。

```sh
python smoke/live_native.py --lab smoke/local/native-lab.toml --mcp PATH_TO_MCP --mode relay --output smoke/runs/UNIQUE_NATIVE_RUN
```

此入口使用应用已保存的配对信息，可通过 `--config` 指定正常 MCP 配置；创建独立 MCP 会话和 UUID 临时目录，退出时清理自己的目录与终端。报告中的工具目录匹配不等于全部工具已执行，也不自动满足发布门禁。真实图形交互、安装升级和 Relay 运维仍按对应功能用例验收。

`live_soak.py` 使用相同设备 TOML，要求明确指定独立测试账号的 MCP 配置，并至少运行 28800 秒。每分钟执行真实只读操作，采集 Executor 内存、线程和句柄（macOS 为 lsof 打开的文件项数），每两小时显式断开并重新连接本测试会话。原始采样持续追加到 samples.jsonl，progress.json 可查看运行状态；失败采样保留并阻止通过。它只覆盖这组原生连接及资源观察，不能替代 Web/Android 的长稳、休眠恢复或明确负载下的压力测试。

```sh
python smoke/live_soak.py --lab smoke/local/native-lab.toml --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --seconds 28800 --output smoke/runs/UNIQUE_NATIVE_SOAK
```

长稳运行期间可在另一个终端启动只读复核器；它等待原运行结束，检查 PID 连续性、采样缺口、实际观察跨度及测试会话清理。重启后资源计数下降不能掩盖崩溃或泄漏；未满八小时的单次复核返回 2，不表示通过。

```sh
python smoke/soak_audit.py --run smoke/runs/UNIQUE_NATIVE_SOAK --watch
```

最终 `result.json` 和 `audit.json` 均通过才完成这组原生长稳观察；仍需 SOAK-01 的其他适用部分。复核器只读已有运行目录，不控制设备，也不会缩短原测试。

`native-lab.example.toml` 提供无真实账号和设备信息的模板。以下入口必须显式指定已正常登录、已配对的测试操作端 TOML；每次使用新的输出目录。它们只补充对应功能的真实证据，不能单独将整项发布用例或全部平台标为通过。

| 入口 | 实际检查 | 前置条件 |
|---|---|---|
| `live_git.py` | 状态、定向提交、日志、diff、checkout、本地 bare 远端 fetch/push/pull、无效引用不损坏未提交内容 | 目标已安装 Git；仅创建临时仓库 |
| `live_documents.py` | PDF/Word/Excel/PPT、中文文本、OCR、搜索、PNG 预览哈希及加密/损坏文件拒绝 | 候选客户端包含文档 worker、原生依赖与 OCR 模型 |
| `live_system.py` | OS 登录会话、目标 DNS、原生日志分页与分块哈希、进程/监听端口过滤、错误身份拒绝及精确终止 | 专用测试主机；默认 macOS 读取最近一分钟 unified log，Windows 读取最近一小时 System 日志 |
| `live_services.py` | 专用服务的启用/禁用、启动/重复启动、重启 PID、停止和再次启动 | Windows 需 SCM 创建权限与专用夹具；macOS 需 root 创建临时 launchd plist |
| `live_browser.py` | 表单、查找/提取、按键、弹窗、拖放、上传下载、截图、坐标、PDF、诊断和标签页历史 | Windows/macOS；填入精确 `user`，需可用 desktop_user 与已打包浏览器运行时；macOS 使用系统 Python 3 提供回环页面 |
| `live_containers.py` | 唯一标签/ID 查询、启动、日志、重启时间、停止并清理 | 测试设备与所选本地 Docker context 指向同一个 daemon；已有含 `/bin/sh`、`sleep` 且不声明 volume 的镜像 |

Git、文档和系统入口使用相同参数形式：

```sh
python smoke/live_git.py --lab smoke/local/native-lab.toml --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --output smoke/runs/UNIQUE_GIT_RUN
python smoke/live_documents.py --lab smoke/local/native-lab.toml --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --output smoke/runs/UNIQUE_DOCUMENT_RUN
python smoke/live_system.py --lab smoke/local/native-lab.toml --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --output smoke/runs/UNIQUE_SYSTEM_RUN
```

文档入口复用源码中 `acceptance/fixtures/document/` 的固定样本，不依赖样本生成器的 Python 包。它验证内容和预览，不替代长任务取消、权限边界或候选安装包的完整性检查。系统入口中的 `log_source` 可在设备 TOML 内显式选择；扫描页为空时继续原游标，不将空页视为没有日志，也不把平台标记的 unverified 改成完整。

```sh
cargo build --locked -j 4 -p pab-os-control --example service_fixture
python smoke/live_services.py --lab smoke/local/native-lab.toml --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --windows-fixture target/debug/examples/service_fixture.exe --output smoke/runs/UNIQUE_SERVICE_RUN
python smoke/live_browser.py --lab smoke/local/browser-lab.toml --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --output smoke/runs/UNIQUE_BROWSER_RUN
python smoke/live_containers.py --device-code TEST_DEVICE_CODE --docker-context TEST_CONTEXT --image EXISTING_FIXTURE_IMAGE --config smoke/local/test-operator.toml --mcp PATH_TO_MCP --output smoke/runs/UNIQUE_CONTAINER_RUN
```

macOS-only 服务测试不传 `--windows-fixture`。服务入口不会控制已有服务，不会自动提权；启动配置的 enable/disable 与当前运行状态分别读回验证。容器入口不拉镜像、不映射端口、不挂宿主目录，不修改已有容器。

浏览器入口仅访问自己的合成页面与 Pixels profile，在目标回环地址提供最多十分钟的页面，结束时关闭本次浏览器、HTTP 任务和临时产物。`fill_form` 的 combobox 值使用**可见选项文字**，`select_option` 使用选项值。完整 BROWSER-01 还需人工正常确认接管、验证暂停期间访问被拒绝以及确认后同一会话可继续；脚本不绕过该确认，不输入真实登录凭据。

定位某份真实 Pixels 日志的多行解析问题时，可以使用只读探针，不替换正在运行的 Executor：

```sh
cargo run --locked -j 4 -p pab-os-system-log --example smoke-log-probe -- LOG_ROOT START_UNIX_MS END_UNIX_MS
```

它读取指定目录的 `executor.log`，只输出计数和原始内容哈希。真实日志副本保存在 `runs/`，不可提交。多行消息保留在前一条带时间戳的记录中；文件开头无时间戳仍明确报告无法验证，不编造时间。

每个功能用例的完整步骤和预期在 `cases.toml`。生成本次专属观察模板：

```sh
python smoke/run.py observation-template --report smoke/runs/RUN_ID/report.json --case RELAY-01
```

填写操作者、环境、每一步实际观察及清理结果，再记录：

```sh
python smoke/run.py record --report smoke/runs/RUN_ID/report.json --observation PATH_TO_COMPLETED_OBSERVATION.json
```

观察需对应同一 run_id 和产物哈希，每一步都要有业务后置条件。例如维护必须观察到节点停止接收新连接、备用节点承接新连接以及恢复后真正可连接；不能只写“按钮可点”或“HTTP 200”。人工证据单独标记，运行器检查完整性，不冒充独立见证人。

`MCP-01` 要求按 `tool-coverage.json` 逐一验证已注册工具，保存每项实际调用和目标端结果的脱敏附件。测试目录中存在某工具的名字不证明该工具已经执行成功。平台不支持的工具需记录协议/平台依据，在其他支持平台验证；缺硬件或权限是阻塞，不是不适用。

Android 使用指定测试设备，覆盖安装必须 `adb -s SERIAL install -r`，保留数据、设备身份和签名。不要卸载、清应用数据或换签名来让升级测试通过。首次安装测试另用干净的专用设备/模拟器。

`AUTO-13` 使用 `lab.toml` 中显式启用的 `[android]` 配置。先构建 `pab-mcp` 和 `remote-fixture`，并核对手机已安装 APK 的哈希；入口不隐式安装。它暂时断开普通连接，通过 ADB reverse 验证实际 TLS Relay、配对、输入、截图及请求去重，最后恢复原连接意图。它不是公网穿透测试，不能替代公网、权限变更或切网项目。单独诊断可运行：

```sh
python smoke/android.py --serial SERIAL --apk PATH_TO_APK --mcp PATH_TO_MCP --fixture PATH_TO_REMOTE_FIXTURE --output smoke/runs/UNIQUE_ANDROID_RUN
```

普通网络下的 Android 命令、PTY 和端口转发还可运行以下入口。提供已登录的专用操作端 TOML；测试不内置服务地址、不注册账号，也不安装应用。配对密码仅经标准输入传递，结束后恢复手机原连接及暂停状态。端口转发会检查真实 Relay 路径、二进制哈希、计数、关闭和暂停行为。

```sh
python smoke/android_network.py --serial SERIAL --operator-config smoke/local/operator.toml --apk PATH_TO_APK --mcp PATH_TO_MCP --output smoke/runs/UNIQUE_ANDROID_NETWORK_RUN
```

操作端账号必须在测试服务器上仍然存在且登录有效；本地 `account status` 缓存不代表服务端会话有效。清理测试账号后不能继续复用旧会话。专用账号的登录配置只存于 `smoke/local/`，通过 `pab-mcp account login USERNAME --password-stdin` 正常登录，密码不放入命令参数或报告。公网脚本按当前 Android 的“断开连接”交互验证停止、拒绝新请求和重新连接，结束后恢复测试前的连接意愿；不会绕过权限或调用已取消的暂停按钮。

浏览器测试页面位于 `smoke/fixtures/browser.html`。在专用测试设备上用 `python -m http.server 38624 --bind 127.0.0.1 --directory smoke/fixtures` 提供该页面，再通过浏览器工具访问 `/browser.html`。页面包含输入、选择、弹窗、拖放、上传下载和表格；逐项读取页面结果验证操作，最后关闭本次浏览器会话和 HTTP 服务。登录接管的恢复必须由目标用户正常确认，不能改写确认标记来绕过。

## 发布门禁

`AUTO-08` 默认执行 Executor 普通测试，并把未执行的原生测试保留为阻塞。准备好原生测试主机后，在 `lab.toml` 显式启用 `[native_executor]`：设置专用 worker、Windows 登录会话 ID 或 Unix UID、进程夹具；准备 Docker 镜像及相应桌面/钥匙串环境。入口会先枚举所有原生测试，再逐个使用独立进程执行，每项都必须实际产生一个测试结果。不会因普通单元测试通过就漏掉原生项目。已有测试二进制也可在目标主机通过 `python smoke/executor.py --binary PATH --native --output smoke/runs/UNIQUE_NATIVE_RUN` 执行，所需 `PAB_*_TEST_*` 环境变量由测试主机设置。

Windows 原生验收由准备好的 LocalSystem 测试服务运行。先用 `query user` 确认本次交互会话 ID，再填写 `native_executor.user`，不要固定假设为 `1`。入口将普通桌面测试放到该用户会话中运行，切换用户等原生验收保留服务权限；不同身份的结果统一计数，任一失败仍阻止通过。直接使用预构建测试程序时添加 `--unit-user SESSION_ID`。用户进程通过系统用户令牌启动，不传递服务端凭据或完整服务环境。

```sh
python smoke/run.py gate --report smoke/runs/RUN_ID/report.json --artifact PATH_TO_EXACT_INSTALLER
```

全部适用项目通过、证据完整且不超过 72 小时、没有未清理资源、源码与产物未变化才返回 0。失败、零测试、跳过、预期失败、Playwright flaky、手动把自动用例标为通过、缺失/重复用例都会阻断。报告是本地审计依据，不是抵御恶意篡改者的签名证明。

`scripts/release.py` 已接入门禁。先准备候选，再测试同一个包，最后恢复发布：

```sh
python scripts/release.py windows --prepare-only --notes-zh "更新说明" --notes-en "Release notes"
# 使用输出中的实际版本和安装包运行 release/soak；完成全部适用项目。
python scripts/release.py windows --resume --smoke-report smoke/runs/RUN_ID/report.json --notes-zh "更新说明" --notes-en "Release notes"
```

上传和 finalize 前必须提供匹配报告；不会在测试后重新构建替换候选包。`build-version.json` 和
`product-versions.toml` 是版本分配记录，不进入源码指纹，报告直接绑定发布版本及产物哈希；
其他源码应先提交。`--allow-dirty` 只可用于构建演练，不能绕过发布门禁。已发布阶段的恢复只核对公开更新接口，不补发上传。

## 敏感数据与清理

自动日志会对常见凭据格式脱敏，但这不能覆盖任意业务正文。使用合成数据，浏览器 trace、截图、数据库和原始工具输出均按私有运行证据保存；不要提交 `local/`、`runs/`、`tmp/`。分享报告前再次检查内容。项目 `.dockerignore` 也排除这些目录。

## 框架自检与扩展

```sh
python -m unittest discover -s smoke/tests -v
```

新增能力时同时更新功能用例、必要的工具覆盖映射和自动适配器。自动适配器必须使用参数数组执行，不接受任意 shell 命令；输出需有可解析的非空测试计数。现有测试都可直接单独运行进行诊断，发布结论统一由这里汇总。

管理平台与 Relay 拆分的构建编排、归档边界及构建记录校验包含在框架自检
`tests/test_service_products.py` 中。它验证真实 ZIP/tar 内容、归档串包和产物变更拒绝，
并检查 Docker 阶段依赖；使用合成二进制，不代表真实程序或镜像已经完成发布验收。
正式版本仍须运行服务端、Web、双 Relay 与客户端的适用集成项目。

独立产品使用 `--target management` 或 `--target relay`；Relay 范围不选择 Web 测试。
两者均包含服务集成与 `INT-01` 最新版整套联调，另有 Relay 独立部署升级用例。
开发阶段管理平台、Relay 和客户端统一使用最新候选产物，记录实际版本与哈希，
验证登录、P2P/Relay 传输、策略、维护恢复、统计与重启后的数据保留。
该用例使用标准逐步观察模板，不要求新旧版本组合或额外矩阵材料。
