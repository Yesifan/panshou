# PanSou Rust CLI 整体架构

本文说明 PanSou Rust CLI v1 的运行时结构、模块边界和扩展方式。实现以
`src/**` 为准，产品范围及兼容目标见 [RUST_CLI_V1_SPEC.md](RUST_CLI_V1_SPEC.md)。

## 1. 设计目标

PanSou 是本地运行的异步 CLI，不提供 HTTP Server。它把 Telegram 和 19 个
provider 的搜索结果统一为核心数据模型，再完成过滤、排序、网盘链接去重和可选的
有效性检测，最终输出 table、JSON 或 JSONL。

架构遵循以下约束：

- CLI 负责命令编排，业务规则下沉到可测试的库模块。
- provider 相互隔离；单个来源失败不会中止其他来源。
- 无状态 HTTP 客户端可共享，有登录态的 profile 使用独立 cookie jar。
- 搜索结果不缓存；只持久化链接检测缓存和 provider 登录状态。
- stdout 只输出结果，诊断和错误写入 stderr。

## 2. 总体结构

```text
                         +----------------------+
                         |    main / clap CLI   |
                         +----------+-----------+
                                    |
                  +-----------------+-----------------+
                  |                 |                 |
              search             check            provider/config
                  |                 |                 |
          +-------v-------+ +-------v-------+ +-------v-------+
          | SearchEngine  | |  CheckEngine  | | Config/State  |
          +-------+-------+ +-------+-------+ +---------------+
                  |                 |
       +----------+----------+      +---- cloud checkers
       |                     |
 TelegramSource       Provider trait (19 implementations)
       |                     |
       +----------+----------+
                  |
        HTTP client / Session / Proxy / Encoding
                  |
          remote public endpoints

SearchEngine -> core merge/filter/rank -> optional CheckEngine -> output
```

`src/lib.rs` 导出各业务模块，`src/main.rs` 仅解析参数、调用 CLI 编排层并将错误映射为
进程退出码。这样集成测试可以直接调用库接口，而不依赖进程内全局状态。

## 3. 模块职责

### 3.1 CLI (`src/cli`)

CLI 是应用编排边界，定义四组命令：

- `search`：解析来源、provider、频道、过滤条件、并发数、超时、代理、输出格式以及
  `--check`/`--valid-only`。
- `check`：接收参数或 stdin 中的链接，调用检测引擎。
- `provider`：列举 provider/profile，并执行登录、退出、状态查询和配置。
- `config`：显示合并后的配置或配置文件路径。

CLI 负责装配 `Config`、`AppPaths`、HTTP client、`StateStore`、provider registry、
`SearchEngine` 和 `CheckEngine`。核心库返回结构化结果，CLI 决定 stdout/stderr 以及退出码。

### 3.2 Core (`src/core`)

Core 不依赖具体站点，包含跨来源共享的领域模型和纯业务逻辑：

- `model`：`Source`、`SearchResult`、`Link`、`MergedLink` 等稳定数据结构。
- `cloud`/`link`：网盘类型识别、链接解析、标准化键、密码和 `work_title` 关联。
- `merge`：按 result key 合并搜索结果，按规范化 URL 合并链接；首次出现的位置保持稳定，
  更完整或更新的元数据可以覆盖内容。
- `filter`：include/exclude 和 cloud type 过滤。
- `rank`：按来源优先级、标题关键词和时间新鲜度进行稳定排序。
- `error`：provider、check、config、state、HTTP 和解析错误的结构化类型。

Core 是 provider、搜索和输出之间的契约层。新增来源应转换为这些模型，而不应把站点专属
响应类型传播到其他模块。

### 3.3 Search (`src/search`)

`TelegramSource` 解析 Telegram 公开频道页面；`SearchEngine` 统一调度 Telegram 频道和
provider。每个被选择的频道或 provider 都视为一个独立 source。

搜索流水线为：

```text
选择 sources
  -> 并发请求（每 source 独立 timeout）
  -> 分离成功批次与 SourceError
  -> 按选择顺序稳定合并
  -> query/include/exclude/cloud 过滤
  -> 排序
  -> 展平并按 URL 合并链接
  -> 按 cloud type 分组
```

provider 可通过 `KeywordFilterMode` 声明关键词已由远端处理，避免 core 再次错误过滤；
Telegram 和 `Core` 模式的 provider 由搜索引擎执行本地 query 过滤。

### 3.4 Providers (`src/providers`)

所有搜索来源实现统一的异步 `Provider` trait：

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn meta(&self) -> ProviderMeta;
    async fn search(
        &self,
        ctx: &SearchContext,
        query: &str,
    ) -> Result<Vec<SearchResult>, ProviderError>;
}
```

`ProviderMeta` 描述名称、优先级、认证类型和关键词过滤职责。registry 显式注册 15 个
无登录 provider；CLI 再从 `StateStore` 加载已就绪的 QQPD、Weibo、Gying 和 Panlian
profile。显式注册避免仅增加文件却遗漏运行时接线。

provider 内部只负责站点协议、请求头、分页和页面/API 解析，并返回核心模型。站点地址以
endpoint 结构或构造参数注入，便于 fixture/mock server 测试。状态型 provider 的认证、
profile 数据和搜索实现分开组织。

### 3.5 Check (`src/check`)

`CheckEngine` 根据 `CheckCloudType` 选择 `LinkChecker`，目前各 checker 独立实现对应网盘
协议。输入先被规范化，并按“网盘类型 + URL + 密码摘要 + 代理作用域摘要”去重；检测结束后
恢复原始输入顺序和 URL。

检测状态统一为：

- `ok`：链接可用。
- `bad`：链接明确失效。
- `locked`：需要提取码或提取码错误。
- `unsupported`：当前类型无 checker。
- `uncertain`：网络、超时或协议信息不足，无法确定。

Redb 仅作为 check cache 使用。不同状态有不同 TTL；`--refresh` 跳过读取但更新缓存，
`--no-cache` 同时禁止读取和写入。搜索不会进入这套缓存。

### 3.6 HTTP (`src/http`)

`HttpClientFactory` 集中构建 reqwest/rustls 客户端，统一处理：

- 请求超时和有限重定向策略；
- HTTP、HTTPS、SOCKS5/SOCKS5H 代理校验；
- User-Agent 和 provider 所需默认请求头；
- 无状态共享 client 与 profile 隔离的 `Session`；
- UTF-8 及 provider 明确指定的 GBK 等响应编码。

`Session` 封装专属 cookie jar，不在 `Debug` 中暴露 cookie。provider 不应自行建立绕过
这些策略的全局客户端。

### 3.7 State (`src/state`)

`StateStore` 将登录态按 `providers/<provider>/<profile>.json` 隔离，校验 provider/profile
名称以阻止路径穿越，并使用私有目录、私有文件及原子写入。Cookie、token、authorization
等敏感字段在设置 `PANSOU_STATE_KEY` 后使用 AES-256-GCM 加密；密码不允许在无密钥时
持久化。密钥支持 32 字节原文、64 位十六进制或编码后为 32 字节的 Base64。

状态只用于登录会话和账号配置，不承担搜索缓存。每个 profile 的 HTTP session/cookie jar
独立，避免账号之间串用认证信息。

### 3.8 Config (`src/config`)

`Config` 读取平台标准用户目录中的 TOML，并应用以下优先级：

```text
CLI 参数 > PANSOU_* 环境变量 > 兼容环境变量 > config.toml > 默认值
```

配置覆盖网络代理和超时、搜索并发/频道/provider、检测缓存/并发，以及少量 provider
非敏感设置。敏感登录态不写入 `config.toml`，而由 `StateStore` 管理。

`AppPaths` 通过平台目录计算：

```text
<config>/pansou/config.toml
<state>/pansou/providers/<provider>/<profile>.json
<cache>/pansou/check.redb
```

### 3.9 Output (`src/output`)

输出层只消费结构化 `SearchOutcome` 或 `CheckResult`：

- table 面向终端阅读；
- JSON 提供稳定的整体 envelope；
- JSONL 每行一个合并链接或检测结果，便于流式管道处理。

机器可读输出不混入日志。搜索的 `source_errors` 保留来源、错误类型和消息，以便调用方处理
部分失败。

## 4. 并发与超时模型

搜索使用 Tokio、`FuturesUnordered` 和 `Semaphore`。`--jobs` 限制同时运行的 source 数，
每个 source 外层再使用 `tokio::time::timeout`，因此慢站点不会无限占用任务。异步完成顺序
不会改变结果确定性：批次和错误在合并前按 source 的选择序号排序。

检测使用异步 stream 的 `buffer_unordered(jobs)` 限制请求并发。相同检测键一次只发出一个
请求，再把结果复制回相应输入位置。

单个 source 失败属于正常的部分失败：其他来源仍返回且进程退出 0；所有选择的 source
都失败时退出 3。架构中没有自定义线程池、后台搜索任务或搜索缓存。

## 5. 错误模型与退出码

库层使用 `thiserror` 定义领域错误；provider 错误区分 timeout、network、parse、
auth required、rate limited、blocked、protocol 和 unavailable。CLI 边界使用 `anyhow`
附加上下文并统一呈现。

退出码契约：

| 退出码 | 含义 |
| ---: | --- |
| 0 | 成功，或搜索仅部分来源失败 |
| 2 | CLI 参数、配置或可解析的输入错误 |
| 3 | 所有选定搜索来源失败 |
| 4 | 显式选择的 provider 需要认证 |
| 5 | 全局初始化或网络设施错误 |
| 10 | `check --fail-invalid` 检测到 `bad`/`locked` |

provider 失败会进入 `SearchOutcome.source_errors`；check 的暂时性错误通常归一为
`uncertain`，避免把网络故障误判成链接失效。

## 6. 安全边界

- 使用 rustls 的正常证书校验，不提供关闭 TLS 校验的开关。
- 代理只能由配置、环境变量或 CLI 显式启用；代理 URL 在诊断与 cache key 中均去除或摘要
  凭证。
- Cookie、token、Authorization 和 password 不写日志，`Debug` 实现主动隐藏密钥与
  cookie jar。
- 密码通过隐藏输入或 stdin 获取，不要求作为命令行参数进入 shell history。
- provider/profile 名称限制为安全字符，状态文件原子写入且 Unix 权限收紧。
- check cache key 对密码和代理 URL 做 SHA-256 摘要，缓存内容不保存代理凭证。
- Gying challenge 仅执行纯计算，不启动隐藏浏览器。
- 远端 HTML/API 都是不可信输入；解析错误留在对应 source 内，不扩大为全局失败。

## 7. 扩展点

### 新增搜索 provider

1. 在 `src/providers` 实现 `Provider`，把响应转换为 `SearchResult`/`Link`。
2. 为 endpoint 提供可注入构造方式，并添加成功、空结果、协议错误等 fixture 测试。
3. 无状态 provider 加入 `builtin_stateless_providers()`；状态型 provider 同时接入
   `StateStore` 的 ready/profile 判断和 `provider` 子命令。
4. 明确 `ProviderMeta` 的优先级、认证方式与关键词过滤模式。

### 新增网盘类型或 checker

1. 扩展 core 的 `CloudType`、域名识别和 URL 规范化规则。
2. 在 `src/check` 实现 `LinkChecker` 并加入 `builtin_checkers()`。
3. 使用脱敏且包含协议差异的 cache key，补充状态、超时、密码及 fixture 测试。

### 新增输出格式或配置项

输出格式应只修改 `src/output` 和 clap 枚举，不能把展示逻辑放入引擎。配置项需同时定义
默认值、文件字段、环境覆盖、CLI 覆盖和校验，并遵守敏感数据与普通配置的边界。

## 8. 目录结构

```text
src/
├── main.rs                 # 进程入口、退出码
├── lib.rs                  # 库模块导出
├── cli/mod.rs              # clap 契约与应用编排
├── core/                   # 领域模型、链接解析、合并、过滤、排序、错误
├── search/
│   ├── engine.rs           # source 调度和聚合流水线
│   └── telegram.rs         # Telegram 公开频道来源
├── providers/
│   ├── mod.rs              # Provider trait、metadata、registry
│   ├── <stateless>.rs      # 无状态 provider
│   ├── qqpd/               # QR 登录、频道搜索
│   ├── weibo/              # QR 登录、用户搜索
│   ├── gying/              # 密码登录、challenge、搜索
│   └── panlian/            # 密码登录、搜索
├── check/                  # 检测引擎、Redb cache、网盘 checker、协议/加密
├── http/                   # client、session、proxy、文本编码
├── state/                  # profile 存储和 AES-256-GCM
├── config/                 # TOML、环境覆盖、平台路径
└── output/                 # table / JSON / JSONL

tests/                      # 跨模块与 CLI 集成测试
fixtures/providers/         # provider 协议与解析 fixture
docs/                       # 规格、架构和用户文档
refer/pansou/               # 迁移期间的 Go 协议参考，不参与 Rust 构建
```

## 9. 测试边界

纯函数（链接解析、合并、过滤、排序、加解密）使用单元测试；provider 通过 fixture 或本地
mock server 验证请求协议与解析；CLI 集成测试验证参数契约、输出结构和退出码。默认 CI
不依赖真实账号或第三方站点，live tests 必须显式标记为 ignored，避免外部波动和凭证进入
自动化环境。
