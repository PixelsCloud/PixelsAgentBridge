# Pixels Agent Bridge 使用指南与自我部署

> 本文与[官网文档](https://agent.rgaa.vip/docs/)同步维护。

## 目录

- [开始之前](#overview)
- [安装 Pixels](#install)
- [程序配置统一使用 TOML](#application-config)
- [Linux 安装](#linux-install)
- [登录账号](#account)
- [添加设备](#devices)
- [接入 AI Agent](#agents)
- [完成第一个任务](#first-task)
- [独立文件传输](#file-transfer)
- [远程桌面与图片剪贴板](#remote-desktop)
- [远程终端与 Android 权限](#remote-terminal)
- [下载与升级](#client-updates)
- [部署您自己的服务](#self-hosting)
- [准备配置与证书](#deploy-config)
- [构建、启动与初始化](#deploy-start)
- [让两端连接您的服务](#deploy-clients)
- [Relay 多节点与滚动升级](#relay-cluster)
- [备份、升级与维护](#maintenance)
- [常见问题](#questions)

<a id="overview"></a>

## 开始之前

> **注意：** 我们提供免费 Relay 服务，如果想获得更快的速度，请您自行部署 Relay 服务。

Pixels Agent Bridge 由操作端、目标设备和 AI Agent 组成。操作端安装 Desktop 并登录账号；目标设备运行 Executor。AI Agent 通过本机 MCP 使用保存的设备连接。

> 账号用于识别发起控制的人；设备码和设备密码用于验证目标设备。两者都需要，登录不能代替设备密码。

<a id="install"></a>

## 安装 Pixels

在操作端和目标设备安装适合系统的版本。Windows 和 macOS 安装包包含桌面应用、后台设备服务与 MCP。Linux 可选 Ubuntu 桌面版或无界面服务器版。两端需要连接到同一个 Pixels 控制服务。

1. 从官网下载对应平台的安装包。
2. 完成安装并启动 Pixels Desktop；目标设备上的后台服务保持运行。
3. macOS 首次使用桌面截图或键鼠功能时，按系统提示授予屏幕录制与辅助功能权限。

[前往下载](https://github.com/PixelsCloud/PixelsAgentBridge/releases/latest)

<a id="application-config"></a>

## 程序配置统一使用 TOML

Pixels 自定义的程序配置统一使用 TOML。安装器在首次安装时创建配置，升级保留已有配置及注释。Server、Relay 和官网服务也使用 TOML；第三方 Agent 客户端及系统要求的配置格式保持不变。

- Windows: `C:\Program Files\PixelsAgentBridge\config.toml`
- macOS: `/Library/Application Support/PixelsAgentBridge/config.toml`
- Linux: `/etc/pixels-agent-bridge/config.toml`

control_url = ""
relay_urls = []

[executor]
# device_name = "my-device"
# log_level = "info"
# data_dir = "/var/lib/pixels-agent-bridge"

修改安装配置后重启后台服务，并重新启动 Desktop/MCP。Desktop 设置页保存的服务器地址为用户数据目录下的 operator-server.toml，MCP 工具开关为 mcp-tools.toml。用户服务器设置覆盖安装默认值；显式 --config PATH 使用指定 TOML，并跳过用户服务器覆盖。安装参数只用于首次安装，已有配置请直接编辑 TOML。

安装配置空值使用发行包中编译的默认服务，设置输入框不回显该默认值。开源构建未提供默认服务时等待配置，客户可填写完整的控制服务和 Relay 地址覆盖默认值。本地打包的默认值从不上 Git 的 .env/client-defaults.toml 读取。

旧 settings.json/settings.env 由安装器迁移；旧 operator-server.json 和 mcp-tools.json 在首次读取时迁移。已有 TOML 优先，格式错误会报错，不回退到旧配置。设备身份、登录凭据、数据库和发布清单属于运行数据，保留原格式。

自部署的中继可按需增加 Windows 或 Linux 节点；服务端分发节点目录，客户端自动选择及故障切换，账号总带宽额度保持不变。配置和升级步骤见下方自我部署说明。

<a id="linux-install"></a>

## Linux 安装

### Ubuntu 桌面版

Ubuntu 22.04 x86_64 图形桌面可在官网下载 DEB，包含桌面应用、后台设备服务和 MCP。在终端进入安装包所在目录，执行下方命令（将文件名替换为实际下载的文件）。

sudo apt install ./pixels-agent-bridge-desktop-linux-x86_64-VERSION.deb

从应用菜单打开 Pixels Agent Bridge，登录并添加设备即可使用。升级下载新版 DEB 并再次运行安装命令，账号、设备身份和配置会保留；重装相同版本时加 --reinstall。安装会重启服务并中断连接。桌面版不使用下方无界面服务器的安装脚本升级。Wayland 下的本机截屏和键鼠能力取决于桌面环境。

### 服务器版一行安装

Linux x86_64 服务器可用下面一行命令安装后台设备服务和 MCP。支持 Ubuntu 20.04 及以上、Debian 11 及以上（glibc 2.31+），需要 root 或 sudo 权限；使用发行包中编译的默认服务，没有默认值时等待配置。脚本会下载正式版本并验证签名、校验和及运行库，再安装服务。

curl -fsSL https://agent.rgaa.vip/install.sh | bash

安装器按需补齐 Python 签名验证组件，无需升级系统的 OpenSSL 或 glibc。安装成功后，运行下面的命令查看设备码和密码，再从 Windows、macOS 或 Linux Desktop 添加这台设备。

sudo /opt/pixels-agent-bridge/run-executor.sh show-access

以后再次运行安装命令即可升级，保留设备身份、数据和原有服务地址。升级会重启服务并中断正在运行的连接。

后续修改配置并重启服务：

sudo nano /etc/pixels-agent-bridge/config.toml
sudo systemctl restart pixels-agent-bridge-executor.service

### 自部署与无 systemd 环境

curl -fsSL https://agent.rgaa.vip/install.sh | bash -s -- --control-url wss://bridge.example.com/control --relay-url https://relay.example.com

没有 systemd 的容器可在 bash -s -- 后添加 --no-service，然后由自己的进程管理器启动 /opt/pixels-agent-bridge/run-executor.sh，并持久化 /etc/pixels-agent-bridge 和 /var/lib/pixels-agent-bridge。上述无界面服务器包不包含桌面应用；Git 等工作负载程序按需另装。

<a id="account"></a>

## 登录账号

在发起控制的电脑上打开 Desktop，点击左侧顶部的账号入口。可以注册 Pixels 账号，也可以使用 GitHub 登录。登录状态会保留，直到您主动退出。

目标设备可以只运行后台服务，无需登录账号。已运行的 MCP 会同步 Desktop 的账号变化。

<a id="devices"></a>

## 添加设备

1. 在目标设备的 Desktop 中查看九位设备码和设备密码。
2. 在操作端打开「设备列表」，在「添加设备」中输入这两项并连接。
3. 验证成功后设备会出现在「已添加的设备」中，之后 Agent 可以使用保存的凭据连接。

> 每台设备都有自己的设备码和密码。添加设备是保存连接凭据；远程设备不会因为被连接就自动变成您的本机。

<a id="agents"></a>

## 接入 AI Agent

Desktop 启动时自动检测并配置已安装的 Codex、Claude Code、Kimi Code、DeepSeek Harness 和 OpenCode，保留已有配置和主动关闭的接入。新建 Agent 会话或重启客户端以加载 MCP；「设置 → AI Agent」可查看状态、修复、启用或关闭接入。

- Codex
- Claude Code
- Kimi Code
- DeepSeek Harness
- OpenCode

在 Desktop 的「MCP 连接」页可查看运行中的 Agent 会话。每个 MCP 独立连接；无需先在 Desktop 中手动连接目标设备。

<a id="first-task"></a>

## 完成第一个任务

在 Agent 中输入下面的示例，确认它能找到已添加的设备，连接并返回系统信息。您也可以直接指定九位设备码。

> 用 Pixels 连接我已添加的设备，检查系统和磁盘空间，并总结结果。

连接成功后，可继续让 Agent 执行命令、传输文件或操作桌面。不同平台与权限下可用功能可能不同。

<a id="file-transfer"></a>

## 独立文件传输

从设备菜单打开「文件传输」，无需先连接远程桌面。左侧是本机，右侧是目标设备；多个设备使用不同标签页。

1. 分别打开源目录和目标目录。使用路径面包屑导航，点击编辑图标输入路径，或在路径右侧下拉框选择磁盘和常用目录。
2. 选中本机文件或文件夹后点击「发送」，写入右侧当前目录；选中远端项目后点击「接收」，写入左侧当前目录。
3. 下方任务列表分别显示进度、速度、用时、项目数和状态。按任务状态暂停、继续或取消；打开「详情」可按状态筛选并翻页查看项目。
4. 同名冲突默认询问，可选择「替换」「跳过」或「保留两者」。勾选「对本次传输后续所有冲突应用此选择」后，仅当前批次沿用该选择；其他传输仍会询问。
5. 「清理已完成」清理已确认完成的传输记录，可包含跳过的项目；失败和未完成的任务保留，已传输文件不会删除。

目录工具栏提供搜索、隐藏文件、刷新和新建文件夹等操作。远端设备名后显示实际 P2P／Relay 路径及可用的延迟信息。

本机使用当前 Desktop 用户；远端计算机使用可用的已登录桌面用户。没有可用用户时请按提示登录并解锁，程序不会静默改用后台服务身份。Android 则使用应用自身身份和已授权目录。

<a id="remote-desktop"></a>

## 远程桌面与图片剪贴板

从设备菜单打开「远程桌面」，默认只查看。确认目标后点击「启用操作」发送键盘和鼠标输入；需要输入时先聚焦远程画面。

- 可切换显示器、全屏和 15／30／60 FPS 上限，默认 30 FPS。实际帧率取决于画面变化、设备和网络条件，上限不是固定帧率保证。
- 鼠标使用操作端本机指针，远端同步指针样式和点击热点。连接状态可查看 P2P／Relay；统计面板用于检查媒体和输入状态。
- macOS 屏幕录制与辅助功能需要正常系统授权。锁屏、登录画面和安全桌面的可用性取决于平台、辅助服务和权限。

### 复制文字、截图和图片

启用操作并开启剪贴板同步后，在当前操作的查看窗口同步文字和图片。复制截图或图片后，在远端应用的可粘贴区域粘贴；也可从远端反向复制图片。图片使用 PNG 无损压缩，保留透明度。

单张 PNG 上限 32 MiB，解码像素数据上限 64 MiB。超限、不支持或同步失败会显示提示；文件和富文本不在此同步范围内，传文件请用文件传输窗口。Android 暂不支持此桌面剪贴板同步。

### 接管已占用的会话

同一目标同时只允许一个远程桌面查看者。已有会话时会询问是否强制接管；明确确认后断开原查看者，取消则保留原会话。被控端未支持接管时，请关闭原会话或升级。接管不会绕过设备认证、系统登录或权限。

<a id="remote-terminal"></a>

## 远程终端与 Android 权限

在设备菜单打开「远程终端」，使用目标系统提供的 Shell。Windows 在支持的环境下另提供 System 终端；结束终端会结束该 Shell 及其运行的程序，不等于断开其他终端或设备连接。

Android 可从电脑 Desktop 打开文件传输和终端窗口，使用应用自身 UID，不是 Root、ADB shell 或 Windows Console 用户。文件访问限应用工作区和系统选择器已授权目录；只读权限和授权撤销仍然生效。

Android 终端使用 /workspace 等实际可访问的工作目录；系统选择器授权的 /shared-… 目录通过文件工具访问，不能直接当作终端的 POSIX 工作目录。未提供的功能会显示暂无此功能或相应权限提示。

<a id="client-updates"></a>

## 下载与升级

客户端、管理服务（Server + Web）和 Relay 在同一个 GitHub Release 下载，各组件保留实际版本号。请选择正确的平台和架构；有界面客户端、Linux 无界面服务器包和服务端分别列出。

[打开统一发布页](https://github.com/PixelsCloud/PixelsAgentBridge/releases/latest)

Windows、macOS 两种架构、Linux 桌面和 Linux 无界面包提供对应的签名稳定更新渠道。Linux 官网一行安装仅用于无界面服务器包；桌面版使用 DEB。Android 使用相同签名的新 APK 覆盖升级，保留应用数据、设备身份和已有授权，不卸载或清除数据。

<a id="self-hosting"></a>

## 部署您自己的服务

使用自己的 Server、Relay 和数据库，让操作端与目标设备连接到您管理的服务。这里采用仓库现有的 Docker Compose 部署方式，配置与源码保持一致。

### Server + Web

账号、设备管理和控制连接。浏览器管理台随 Server 提供。

### Relay

设备无法直连时中继转发数据，支持 HTTPS 与 QUIC。

### PostgreSQL

持久保存账号、设备和管理数据。任务与文件仍留在本地。

管理服务（包含 Web）与 Relay 的 Windows ZIP、Linux tar.gz 和客户端安装包统一放在同一个发布页面，各组件保留实际版本号。下载服务端安装包与部署运行服务是两个步骤，请按目标环境的部署指南安装并保留原有配置和数据。

[下载客户端、管理服务与 Relay](https://github.com/PixelsCloud/PixelsAgentBridge/releases/latest)

也可在原生 Windows 上直接运行 pab-server.exe 和 pab-relay-server.exe，不需要 WSL；PostgreSQL 可放在其他机器。原生服务安装、版本替换及回退见专项指南。

[原生服务与 Relay 集群运维指南 ↗](../docs/RELAY_CLUSTER_OPERATIONS.md)

### 准备环境

- 一台可运行 Docker 与 Docker Compose v2 的服务器，以及 Git、Python 3.11+。以下命令以 Linux shell 为例；Windows 可在 Docker Desktop 的 Linux 容器环境与 WSL 中执行。
- 准备两个域名，例如 bridge.example.com 和 relay.example.com，解析到您的服务器，并准备匹配域名的 TLS 证书与信任链。
- 在公网开放 HTTPS 与 Relay 的 UDP 端口；数据库和容器上游 HTTPS 端口保持在本机或内部网络。

| 端口 | 用途 | 访问范围 |
| --- | --- | --- |
| `443 / TCP` | Server 管理台、控制连接与 Relay HTTPS | 公网，按域名分别代理 |
| `7842 / UDP` | Relay QUIC | 公网，直达 Relay |
| `31400 / TCP` | Server HTTPS | 127.0.0.1 |
| `31443 / TCP` | Relay HTTPS | 127.0.0.1 |
| `5432 / TCP` | PostgreSQL | Compose 内部网络 |

[查看仓库 Docker 部署说明 ↗](../packaging/docker/README.md)

<a id="deploy-config"></a>

## 准备配置与证书

从仓库根目录准备部署文件。private.env 负责镜像、挂载和端口；Server 与 Relay 各自读取一份 TOML。

**Shell · setup**

```bash
git clone https://github.com/PixelsCloud/PixelsAgentBridge.git
cd PixelsAgentBridge
cp packaging/docker/example.env packaging/docker/private.env
mkdir -p packaging/docker/config packaging/docker/certs
cp packaging/docker/pab-server.example.toml packaging/docker/config/pab-server.toml
cp packaging/docker/pab-relay-server.example.toml packaging/docker/config/pab-relay-server.toml
```

### 编排配置

编辑复制得到的 private.env，设置数据库密码，并让内部 DNS 别名与证书域名一致。稍后构建完成后，再填写实际输出的镜像标签。

**private.env · fields to edit**

```dotenv
PAB_DB_PASSWORD=YOUR_DB_PASSWORD
PAB_BACKEND_DNS_ALIAS=bridge.example.com
PAB_RELAY_DNS_ALIAS=relay.example.com
```

### Server 与 Relay

将以下域名、数据库密码和共享密钥替换为您自己的值。共享密钥至少 32 个可打印 ASCII 字符，两份 TOML 中必须相同。数据库密码与 private.env 一致；URL 中的保留字符需要百分号编码。

**Shell · generate shared secret**

```bash
python -c "import secrets; print(secrets.token_hex(32))"
```

<details>
<summary>pab-server.toml</summary>

**config/pab-server.toml**

```toml
listen = "0.0.0.0:8443"

[database]
url = "postgres://pab:YOUR_DB_PASSWORD@postgres:5432/pab"

[tls]
cert = "/run/pab-certs/backend-cert.pem"
key = "/run/pab-certs/backend-key.pem"

[web]
origin = "https://bridge.example.com"
registration_enabled = true
assets = "/usr/local/bin/web"

[relay]
control_secret = "YOUR_SHARED_SECRET"

[log]
directory = "/var/log/pab"
level = "info"
```

</details>

<details>
<summary>pab-relay-server.toml</summary>

**config/pab-relay-server.toml**

```toml
node_id = "primary"
usage_dir = "/var/lib/pab/usage"
https_bind = "0.0.0.0:31443"
quic_bind = "0.0.0.0:7842"

[tls]
cert = "/run/pab-certs/relay-cert.pem"
key = "/run/pab-certs/relay-key.pem"

[control]
url = "wss://bridge.example.com:8443/relay-control"
ca_cert = "/run/pab-certs/backend-ca.pem"
secret = "YOUR_SHARED_SECRET"

[log]
directory = "/var/log/pab"
level = "info"
```

</details>

### 证书与 HTTPS 入口

将下面的证书文件放入 packaging/docker/certs。证书名称需要匹配 public HTTPS 域名和 Compose DNS 别名；ca.pem 保存用于验证对应服务证书的信任链。配置及私钥仅授予服务所需的读取权限。

| 服务 | 文件 |
| --- | --- |
| Server | `backend-cert.pem`<br>`backend-key.pem`<br>`backend-ca.pem` |
| Relay | `relay-cert.pem`<br>`relay-key.pem`<br>`relay-ca.pem` |

分别把两个域名代理到本机 31400 和 31443 端口，保留 HTTPS 上游验证和 WebSocket Upgrade。UDP 7842 直接放行，不经过 HTTP 代理。修改配置后先运行 nginx -t，再重载 Nginx。

<details>
<summary>展开 Nginx 配置参考</summary>

示例用于与 Compose 同机的 Nginx。替换证书路径及域名；私有 CA 需要同时被代理、Relay 和客户端信任。

**Nginx · Server + Relay**

```nginx
# Include in the Nginx http {} context.
map $http_upgrade $pab_connection_upgrade {
    default upgrade;
    '' close;
}

server {
    listen 443 ssl;
    server_name bridge.example.com;
    ssl_certificate /etc/nginx/certs/bridge-fullchain.pem;
    ssl_certificate_key /etc/nginx/certs/bridge-key.pem;

    location / {
        proxy_pass https://127.0.0.1:31400;
        proxy_http_version 1.1;
        proxy_set_header Host $http_host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $pab_connection_upgrade;
        proxy_ssl_server_name on;
        proxy_ssl_name bridge.example.com;
        proxy_ssl_verify on;
        proxy_ssl_trusted_certificate /etc/nginx/certs/backend-ca.pem;
        proxy_read_timeout 3600s;
        proxy_buffering off;
        proxy_cache off;
    }
}

server {
    listen 443 ssl;
    server_name relay.example.com;
    ssl_certificate /etc/nginx/certs/relay-fullchain.pem;
    ssl_certificate_key /etc/nginx/certs/relay-key.pem;

    location / {
        proxy_pass https://127.0.0.1:31443;
        proxy_http_version 1.1;
        proxy_set_header Host $http_host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $pab_connection_upgrade;
        proxy_ssl_server_name on;
        proxy_ssl_name relay.example.com;
        proxy_ssl_verify on;
        proxy_ssl_trusted_certificate /etc/nginx/certs/relay-ca.pem;
        proxy_read_timeout 3600s;
        proxy_buffering off;
        proxy_cache off;
    }
}
```

</details>

- [Server TOML ↗](../packaging/docker/pab-server.example.toml)
- [Relay TOML ↗](../packaging/docker/pab-relay-server.example.toml)
- [可选：GitHub 登录 ↗](../docs/GITHUB_LOGIN_SETUP.md)

<a id="deploy-start"></a>

## 构建、启动与初始化

使用统一构建入口生成包含 Server、Relay 与 Web 管理台的 Release 镜像。构建在 Docker 内完成，不需要在运行服务器上保留 Node。

**Shell · build image**

```bash
python scripts/build.py docker --profile release
```

把构建输出的镜像标签填写到 private.env 的 PAB_SERVER_IMAGE 中，然后启动。默认 Relay 使用同一镜像；Compose 会先启动 PostgreSQL，再由 init 完成数据库初始化，随后启动 backend 与 relay。

**Shell · start & inspect**

```bash
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d --no-build
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml ps
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml logs --tail=100 backend relay
```

> init 是一次性初始化容器，成功执行后退出属于正常状态。backend、relay 与 postgres 应持续运行并通过健康检查。

### 创建管理员

在 https://bridge.example.com 注册您的账号，再在服务器上执行下面的命令，将 YOUR_USERNAME 替换为已注册的用户名。注册不会自动授予管理员权限。

**Shell · administrator**

```bash
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml exec backend pab-server web-admin YOUR_USERNAME --config /etc/pab/pab-server.toml
```

### 验证入口

**Shell · HTTPS health checks**

```bash
curl --fail https://bridge.example.com/health
curl --fail https://relay.example.com/healthz
```

两个健康检查应成功，浏览器应能打开管理台并登录。账号带宽在 Web 管理台中设置；任务、命令与文件不会存入服务端数据库。

[构建与版本说明 ↗](../BUILDING.md)

<a id="deploy-clients"></a>

## 让两端连接您的服务

操作端与目标设备必须使用同一套服务。控制服务地址使用 wss://，Relay 地址使用 https://。

**Client endpoints**

```text
Control: wss://bridge.example.com/control
Relay:   https://relay.example.com
```

客户填写的地址优先于发行包的内置默认值。后台服务需在安装目录的 config.toml 中配置完整地址对并重启服务；公开构建没有内置地址时保持待配置。操作端的用户设置只覆盖 Desktop/MCP，不改变后台服务。

### 操作端 Desktop / MCP

打开「设置 → 网络连接」，填写操作端服务器地址，保存后重启应用。然后登录这套服务的账号，并重启 Agent 或新建会话，让 MCP 加载新的连接设置。

### 目标设备 Executor

后台设备服务使用安装时的服务器配置。请使用面向这套服务的安装包，或按平台安装脚本配置控制与 Relay 地址。仅修改 Desktop 操作端设置不会改变后台服务。

目标设备在线后，在操作端保存设备码与密码，接入您的 Agent，再运行前面的第一个任务。健康检查只能确认服务入口；实际设备连接和任务返回才完成使用验证。

- [返回第一个任务 ↑](#first-task)
- [平台安装配置说明 ↗](../packaging/desktop/README.md)

<a id="relay-cluster"></a>

## Relay 多节点与滚动升级

需要更多中继容量时，在 Server TOML 的 relay.cluster 节点清单中为每台 Relay 配置独立 HTTPS 地址、节点 ID、密钥及容量。Windows 与 Linux 节点可混用，账号的上传和下载额度由全部节点共享。中央 Server 仍保持单活。

升级脚本先暂存并校验版本，逐节点维护；默认等待 30 秒后继续替换，不被长连接阻塞。新版本必须通过双向转发校验才恢复服务；失败时恢复原版本并停止后续升级。既有账号、设备、配置与数据保留。首次由单节点切换到集群模式前，需按运维指南停止旧 Relay 并等待原策略到期。

[配置、自动升级和回滚步骤 ↗](../docs/RELAY_CLUSTER_OPERATIONS.md)

<a id="maintenance"></a>

## 备份、升级与维护

升级前备份 PostgreSQL、private.env、两份 TOML、证书及 Relay 用量队列。以下示例数据库名为 pab；如您修改了 PAB_DB_NAME，请同步调整命令。

**Shell · database backup**

```bash
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml exec -T postgres pg_dump -U pab -d pab -Fc > pab-backup.dump
```

- Server 与 Relay 使用兼容版本一起更新，再更新客户端。修改镜像标签后使用 up -d --no-build 重建，保留数据卷。
- 修改 TOML 后重建受影响的容器；共享密钥变更需要同时更新 Server 和 Relay。
- 普通升级不清空数据库、不重新登记设备，也不执行 down -v。回退涉及数据库结构时，应先按升级前备份在独立数据库中验证恢复。

**Shell · reload Server configuration**

```bash
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d --no-deps --force-recreate backend
```

[完整部署与升级说明 ↗](../docs/WEB_DEPLOYMENT.md)

<a id="questions"></a>

## 常见问题

### Agent 看不到 Pixels 工具？

确认已在「设置 → AI Agent」启用对应客户端，再重启客户端或新建会话。检查 Desktop 的「MCP 连接」页是否出现该客户端。

### 连接设备失败？

确认操作端已登录、设备码与密码正确、目标设备服务正在运行，且两端使用相同的控制服务。

### 任务记录保存在哪里？

任务、命令和操作记录保存在本地；服务端保存账号、设备管理信息和汇总用量。

### 自我部署后，设备没有上线？

确认后台 Executor 使用的是您自己的控制服务和 Relay 地址，而不只是修改了 Desktop 操作端设置。检查域名解析、证书信任以及目标设备的服务日志。

### Relay 未通过健康检查？

查看 relay 容器日志，核对两份 TOML 的共享密钥、control.url、Compose DNS 别名与证书名称。再检查 HTTPS 和 UDP 7842 的网络连通性。

[仍有问题？在 GitHub 提交 Issue](https://github.com/PixelsCloud/PixelsAgentBridge/issues)
