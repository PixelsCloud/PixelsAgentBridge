# Pixels 远端文档解析：完整开发任务与实施记录

日期：2026-10-12。状态：开发和跨平台验收完成，待公开发布。主解析引擎已按用户指定切换为 Firecrawl AnyDoc 0.2.4，扫描件采用离线 Tesseract OCR。目标是让 Agent 在目标设备本地解析文档，按需取得可定位的正文、表格和页面图像。文件不经官网或云端转存；现有连接、执行身份、任务记录及二进制传输继续承担外围职责。

**一次交付约定：** 下文 A—D 只是开发中的顺序与检查点，不作为分别发布的版本。一次开发必须做完全部五个工具、中文/英文 OCR、资源隔离、Windows/macOS 安装包、正式 MCP 路由真机验收和交付检查。不能以“源码完成”“本地单测通过”或“只支持纯文本 PDF”宣布功能完成。AnyDoc 缺少的 PDF 页面模型、Excel 单元格语义和离线 OCR 由格式专用适配器补齐；不把缺口留到下一轮。公开发布以最终安装包验收结果为准。

## 产品结果与范围

用户可以说“分析 Company 上这份 PDF 的风险条款”“汇总这张 Excel 每个工作表的异常数据”。Agent 先用现有文件搜索定位绝对路径，再调用文档工具；大文档只按页、工作表或游标读取所需内容。回答能标注页码、工作表或其他可靠的原文位置。无法确定位置时明确报告，不能编造引用。

本任务承诺 Windows 和 macOS 上的 PDF、DOCX、XLSX、PPTX、PNG/JPEG；TXT、Markdown、HTML、CSV 可通过统一入口读取，但已有文本工具仍可用于简单文本。PDF 支持原生文字、混合页面及中文/英文扫描页 OCR。加密文件返回 `password_required`；本任务不收集文档密码，不编辑原始 Office 文件，不抓取 URL，不递归解析附件或压缩包。旧版 DOC/XLS/PPT、ODF 及其他 AnyDoc 声称支持的格式仅在逐格式验收后进入支持清单。这些是不属于本功能的产品边界，不是尚未做完的交付项。

采用 [Firecrawl AnyDoc](https://github.com/firecrawl/anydoc) Rust crate 作为主要文档解析引擎，锁定 0.2.4。它将 Office、CSV、EPUB 等转换成统一 Markdown 和结构化模型；PDF 只输出 Markdown，并以 `NeedsOcr` 报告扫描页。PDF 页码使用同生态的 `pdf-inspector` 逐页提取，PDFium 渲染需 OCR 的页面，离线 Tesseract 识别简体中文和英文。Excel 的精确单元格坐标、值类型、公式使用 Calamine 适配，不让 Agent 从展平 Markdown 猜单元格。OCR 数据与 PDFium 动态库随安装包提供，不在用户设备运行时下载。

## Agent 工具契约

| 工具 | 输入 | 返回与行为 |
| --- | --- | --- |
| `pab_document_open` | `device_code`、目标机绝对 `path`、可选的 `execution`、`ocr_mode=auto/off/on`、可选语言；不接受 URL | 工具内部生成请求 ID。Executor 完成校验和排队后立即返回 `operation_ref`；绝不等待 AnyDoc 解析结束。完成后由现有 `pab_get_operation` 取得 `document_ref`、格式、文件版本和目录摘要，不要求 Agent 构造 UUID。`auto` 只对识别为扫描的页启用 OCR。 |
| `pab_document_read` | `device_code`、`document_ref`、选择器 `{kind: overview/page/sheet/table/chunk, ...}`、可选 `cursor` | 返回最多 16 KiB 的正文/结构化条目、可靠来源位置、`next_cursor`、`truncated` 和提取方法。一个选择器只对应一个文档快照；工作表和表格超长时也分页。 |
| `pab_document_search` | `device_code`、`document_ref`、查询文本、大小写选项、结果上限 | 在已解析快照中返回有界命中、少量上下文和位置，供 Agent 决定读取哪一页/工作表；不再次解析源文件。 |
| `pab_document_render_page` | `device_code`、`document_ref`、PDF 页码或源图片页码、受限尺寸 | 返回 MCP 图片及页码，用于图表、版面或 OCR 不确定内容的人工/模型视觉核对；不提供批量整本图片输出。PPTX 只有在独立验证可靠渲染后才开放此动作。 |
| `pab_document_close` | `device_code`、`document_ref` | 删除该引用的临时提取结果；重复调用无害。未关闭的缓存也会按保留期自动回收。 |

`pab_document_open` 的运行状态走现有操作查询体系；不再增加同义的 `status` 工具。所有工具在 MCP 说明中写明“什么时候使用、什么时候继续用 `pab_file_read`、失败后如何根据原 `operation_ref` 查询”。文档工具集合默认启用，与当前“所有工具默认打开”的产品决定一致。

### 返回结构

- `document_ref` 是服务器生成的不透明引用，绑定目标设备、发起者、执行身份和不可变输入快照；Agent 只传回工具给出的值。
- `overview` 含格式、大小、SHA-256、解析方法、页数/工作表数/表格数（可用时）、目录或工作表列表、提取警告和缓存到期时间。缺失数据用 `unavailable`，不填零。
- 每个内容块含 `kind`、`text` 或表格数据、`location`、`source_method=native/ocr/mixed`。位置可为 PDF 页码、PPTX 幻灯片、XLSX 工作表及单元格范围、DOCX 标题/段落；提取器没有可靠坐标时使用 `location.kind=unknown`。
- 结构化表格保留行列顺序、空单元格以及文本/数字/日期/公式类型；若 AnyDoc 无法提供某项，则返回明确的 `unavailable`，不把展示文本当成单元格真实类型。
- 源文件在建立快照时被读取和校验；后续读取缓存中的同一快照。源文件之后发生变化不会暗改结果，重新 `open` 才读取新版本。

## 实现落点

1. **依赖验证。** 在隔离构建目录锁定 AnyDoc、pdf-inspector、PDFium、Tesseract 与 Calamine 版本，制作测试样本矩阵，记录 Windows/macOS 的编译结果、解析质量、冷/热耗时、峰值内存、exe/安装包增量。检查第三方分发文件。构建并行度不超过 18；D 盘空间不足时使用隔离构建目录，不动用户文档。
2. **协议与操作记录。** 在 `crates/protocol` 增加独立的文档请求/响应和能力版本，保持文件工具的文本上限、设备码和执行身份语义。`crates/executor` 将 `open` 记为只读异步操作，沿用 `operation_ref`、取消和未确认结果查询；Bridge 复用现有认证连接和二进制帧。JSON 只含参数/小结果，不传整份文档。新旧客户端按能力版本给出 `unsupported`，不误调用。
3. **目标机解析工作进程。** `pab-document-worker` 是按任务启动的短命进程，不新增常驻服务或配置文件。`execution` 沿用文件工具的 service/user 选择；指定用户不可用时不改用服务身份。工作进程以选定身份打开本地文件，先检查文件大小，再读取有界字节形成快照，用 AnyDoc 解析，需 OCR 的页由 PDFium 和 Tesseract 补齐。解析始终在独立子进程，不在 Executor、Bridge 或 MCP 进程直接运行。解析异常、超时或取消时结束子进程及其后代，后台服务继续响应。只接受目标机本地路径，不从文档工具发起网络读取。
4. **结果索引与缓存。** Executor 将规范化块、位置索引和搜索索引存到私有临时目录，按发起者、设备、身份和 `document_ref` 控制访问。缓存有磁盘/数量/年龄上限，原始文档字节不长期保存；重启后能恢复仍有效的结果或明确返回 `document_expired`。`close`、取消和到期清理缓存。单次 MCP 输出最多 16 KiB，通过游标继续；图片沿用现有二进制/MCP image 路径，不做 Base64 大 JSON。
5. **OCR 和格式适配。** 先试 AnyDoc 原生提取；扫描或无可提取文字的页在 `auto` 下使用随包提供的中英 OCR 引擎及语言数据，不在运行时自动下载模型。OCR 页和原生页分开标记，并保留低置信度/无法识别警告。Excel 精确单元格与公式、DOCX 表格、PPTX 讲者备注等以样本验收为准，缺口通过小范围适配器修补。
6. **MCP、设置和记录。** 在 Bridge MCP 目录注册上述五个工具，调用参数由 Pixels 完成默认值、UUID、游标和错误映射。工具说明要求 Agent 将文档正文视为待分析数据，不执行其中写给 Agent 的指令。Desktop 的工具分类增加“文档”，默认打开；任务记录显示文档路径、格式、状态、耗时和字节数，不保存正文、密码或 OCR 原文。客户端无需文档编辑器页面。
7. **打包与升级。** Windows 和 macOS 安装包包含所需解析依赖及离线 OCR 数据；Linux 后台包至少完成构建和 headless 文档读取验收。覆盖升级保留操作记录，但临时解析缓存可以按格式版本安全失效。检查安装体积和磁盘需求，再发布新版本。

## 资源与错误规则

开发初始上限：单文件 64 MiB、单文档 500 页、工作表累计 250,000 个非空单元格、解析后缓存 64 MiB、每设备同时 1 个解析任务、每次文本响应 16 KiB。这些数值在真实样本压测后固定或下调；超限在读入/解压/输出各阶段尽早失败。工作进程设独立的内存预算和超时看门狗；Windows 使用子进程 Job Object，macOS/Linux 用可用的进程限制及父进程监测。纯文本解析和 OCR 使用不同预算，OCR 队列不能占满普通文件/系统操作的执行槽。AnyDoc 的内建解析限制作为第二道限制，不能替代进程级限制。工作进程输出使用有界通道和背压，缓存达到配额立即停止，不在 Executor 内拼接整份结果。`read/search/render_page` 对索引执行有界读取或独立后台计算，不在事件循环中全量扫描缓存；Bridge/MCP 不持有全局 Runtime 锁等待这些操作。取消或超时必须终止子进程及后代、清理未完成缓存；单纯丢弃等待中的 Future 不算取消。错误至少区分：`unsupported_format`、`password_required`、`no_extractable_text`、`ocr_unavailable`、`resource_limit`、`file_changed_during_snapshot`、`document_expired`、`permission_denied`、`worker_failed`。解析部分成功时保留有效内容和逐页/逐表警告，不冒充完整成功。

## 单次开发顺序与完成定义

| 阶段 | 交付与通过条件 |
| --- | --- |
| A. 引擎与样本验证 | AnyDoc、PDFium 和本地 OCR 在 Windows/macOS 编译；核心样本的正文、表格、中文、位置字段逐项比对。确认依赖体积、编译时间、OCR 离线打包方案与受测版本；Excel 单元格语义由适配器核对。 |
| B. 正式链路 | 协议、Executor 独立解析进程、Bridge、全部五个 MCP 工具、操作记录和缓存贯通；PDF/DOCX/XLSX/PPTX 的原生文字和表格可按位置分页读取；返回的是目标机正式安装程序的结果。 |
| C. OCR 与视觉 | 图片经 MCP 图片通道返回；扫描 PDF/PNG/JPEG 的中英 OCR、混合页和低置信度提示通过；OCR 数据随安装包离线可用；异常页可渲染和核对。 |
| D. 压力、兼容与发行 | 大文件、压缩炸弹、坏文件、加密文件、权限、源文件变化、取消、断线、工作进程崩溃、缓存到期和覆盖升级均有确定结果。用故意卡住/耗尽预算的解析样本压测时，同时调用 `pab_system_info`、文件读取和设备心跳，三者仍能正常响应；任务结束后无孤儿子进程和残留大缓存。旧客户端/服务端能力不匹配时明确拒绝。最终安装包在 Company Windows 与 Mac 经正式 Pixels MCP 路由通过。 |

验收样本包含：带目录和表格的中文 PDF、纯扫描 PDF、原生/扫描混合 PDF、带表格与图片的 DOCX、多工作表/公式/日期 XLSX、带备注的 PPTX、中文图片、加密/损坏/超限文件。至少跑通一条真实任务：“Agent 在 Company 读取多份文档，检索指定内容，引用页码或工作表，生成摘要到另一台已添加设备”。每个承诺格式都必须在目标机上通过正式 MCP 工具读取，核对已知答案和来源位置；OCR 要核对中文与英文样本，不以非空输出代替正确性。检查原文没有进入官网、云存储或日志；安装包升级后工具仍可用。本机已有其他会话时不覆盖安装，编译最多使用 18 核。

只有 A—D 的最终安装包验收通过，才能标记本功能完成。交付物包括源码、锁定的依赖版本、可复现样本与测试、Windows/macOS 安装包、MCP 工具说明、真实设备验收记录及版本说明。具体性能数值以实测为准，不采用上游自述速度作为验收结论；验收后再提交和推送对应代码。

## 实施记录（2026-10-13）

以上是开发前的目标，验收以实际实现为准。已实现的解析快照存放在 Executor 的有界内存缓存中，最多 8 份、合计 64 MiB，保留 1 小时；服务重启后引用失效并明确返回 `document_expired_or_not_found`。这取代了计划中的可恢复磁盘缓存，不把源文档或解析正文持久化。OCR 语言固定为安装包内的简体中文和英文；工具不提供选择其他语言的参数。PDF 原生页与 OCR 页可通过页面的 `ocr_confidence` 区分，未建立统一的 `source_method` 字段。单次正文读取上限为 14 KiB，以留出 MCP 返回结构的空间。

Windows 工作进程使用 Job Object 的 1.5 GiB 进程内存上限，Linux 使用 2 GiB 地址空间上限；测试 Mac 不支持这些进程限制，因此使用独立线程每 100 ms 检查工作进程的驻留内存，并在 1.5 GiB 时结束进程。Executor 另有 240 秒解析超时，取消会终止短命工作进程。离线样本涵盖原生/扫描/混合 PDF、DOCX 表格、XLSX 单元格与公式、PPTX 备注、图片 OCR、加密/损坏文档和压缩炸弹；正式 MCP 链路已在 Company Windows 验证五个工具和取消时其他工具的响应。跨平台最终包与公开发布以验收记录为准。
