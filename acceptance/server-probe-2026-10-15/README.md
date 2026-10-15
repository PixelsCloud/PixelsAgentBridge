# 服务器地址验证按钮

日期：2026-10-15。未更新本机安装应用，未部署或修改服务器。

每个输入框右侧提供独立验证按钮。未填写地址或尚未加载配置时禁用；验证中显示加载状态，输入变动立即清除旧结果，过期异步响应不会覆盖新地址的结果。验证操作不调用保存或重启。

原生实现位于 `crates/agent-core/src/server_probe.rs`，Desktop 通过 `validate_operator_server` 调用：

- 控制：HTTPS 健康接口必须返回 204，WSS 发送无凭据的 `ListTrafficScopes`，校验与请求 ID 匹配的未登录协议响应。
- Relay：有界读取 `/healthz` JSON 并检查状态，再验证 `/relay` 握手协商 `iroh-relay-v2` 或 `iroh-relay-v1`。不注册临时身份，不声称已经验证用户转发权限。
- 校验证书，支持当前配置的自定义 CA；禁止重定向，HTTP 请求最长 8 秒、整个验证最长 10 秒。错误只返回固定类别，不把响应原文、地址或凭据作为错误输出。

验证结果：

- Rust 地址校验单测通过，Desktop `cargo check -j 10` 通过，TypeScript/Vite 构建通过。
- `python scripts/test_server_probe.py`：6 项真实 HTTPS/WSS 原生测试通过，覆盖正确响应、请求 ID 不匹配、错误 Relay 协议、普通网页、过大响应、重定向、服务故障、不可信证书、超时。证书仅存在于临时测试目录。
- Playwright 既有设置/历史 15 项通过；新增三种语言独立验证、验证不保存、旧结果失效共 4 项通过。空状态使用隐藏结果容器断言。
- 对默认控制服务和 Relay 执行只读原生验证均通过，当次耗时分别为 231 ms 和 155 ms；耗时是完整检查耗时，不是网络 RTT。
- `settings-verified.png` 为浏览器 fixture 布局截图，展示测试地址及模拟 42 ms 结果，不是线上延迟记录。

编译未并发运行，最多 10 个构建 jobs。未执行 macOS 原生打包或正式安装包验收。
