# PanSou Rust CLI v1 实现规格

## 1. 项目目标

将 `fish2018/pansou` 从 Go 编写的常驻 HTTP API 服务重构为 Rust 编写的本地 CLI 工具。

本次不是 Go → Rust 的逐文件翻译，而是一次产品形态重构。

最终目标：

```text
当前：

HTTP Server
  -> Gin Router
  -> Middleware/Auth
  -> Search Service
  -> Async Plugin System
  -> Cache System
  -> Providers

重构后：

CLI
  -> Search Engine
  -> Providers
  -> Merge / Filter / Rank
  -> Output

CLI
  -> Link Check Engine
  -> Cloud Checkers
  -> Output
```

Rust v1 必须是一个能够独立交付的完整版本，不允许用“后续再迁移”为理由缺少本文规定的 19 个 provider。

---

# 2. 参考实现与 source of truth

实现时以当前 Go 项目代码作为协议和解析行为的 source of truth。

分析时仓库：

```text
https://github.com/fish2018/pansou
branch: main
main tree SHA:
beaa56133755a548ebc51b090b3816e2ae044aa6
```

尤其优先参考：

```text
docs/插件修复TODO-2026-09-01.md

service/search_service.go
service/check_service.go
service/check_mobile_crypto.go

model/request.go
model/response.go
model/check.go

util/parser_util.go
util/regex_util.go
util/http_util.go

plugin/<provider>/<provider>.go
```

对于 provider 网站协议，以对应 provider 当前 Go 实现优先，不使用 README 中已经被代码替换的旧协议。

迁移顺序原则：

```text
先保证协议和解析结果一致
→ 再进行 Rust 化抽象
→ 最后删除 Go 代码
```

在全部 parity 验证完成前，不要提前删除 Go 实现。

---

# 3. Rust v1 明确包含的功能

Rust v1 必须包含：

```text
1. Telegram 频道搜索

2. 19 个 provider

   无登录态：
   - pansearch
   - yunsou
   - djgou
   - hdmoli
   - meitizy
   - yulinshufa
   - clxiong
   - jsnoteclub
   - duanjuw
   - dyyj
   - jupansou
   - cldi
   - clmao
   - cyg
   - susu

   登录态：
   - qqpd
   - weibo
   - gying
   - panlian

3. 搜索聚合

4. 并发控制

5. 超时控制

6. 结果合并和 URL 去重

7. 网盘类型识别

8. work_title / password 提取

9. 关键词过滤

10. cloud type 过滤

11. 排序

12. HTTP / HTTPS / SOCKS5 proxy

13. table / json / jsonl 输出

14. 网盘链接有效性检测

15. check 持久化缓存

16. provider 登录、退出、状态和配置

17. 多 profile / 多账号支持

18. Linux / macOS / Windows CLI 发布
```

---

# 4. Rust v1 明确不实现的功能

以下功能不要迁移：

```text
HTTP Server
Gin
REST API
/api/search
/api/check/links
/api/health

JWT
AUTH_USERS
AUTH_ENABLED
CORS
HTTP middleware

Web 管理界面
PluginWithWebHandler
RegisterWebRoutes

Nginx
Supervisor
Docker Compose Server 部署模型

服务端连接数限制
HTTP_READ_TIMEOUT
HTTP_WRITE_TIMEOUT
HTTP_IDLE_TIMEOUT
HTTP_MAX_CONNS

Go GC 优化
对象池
自定义 worker pool

当前搜索结果二级缓存
EnhancedTwoLevelCache
ShardedMemoryCache
ShardedDiskCache
DelayedBatchWriteManager
GlobalBufferManager

异步“先返回，再后台继续搜索”的 Server 模型
ASYNC_RESPONSE_TIMEOUT
ASYNC_MAX_BACKGROUND_WORKERS
ASYNC_MAX_BACKGROUND_TASKS
ASYNC_CACHE_TTL_HOURS
```

注意：

```text
搜索缓存：删除

链接检测 check cache：保留

Provider 登录/session 状态：保留
```

三者不要混淆。

---

# 5. 最终 CLI 契约

使用 `clap` 构建 CLI。

主命令：

```text
pansou search
pansou check
pansou provider
pansou config
```

## 5.1 Search

基础：

```bash
pansou search "仙逆"
```

完整参数设计：

```bash
pansou search <QUERY>

  --source <all|tg|provider>

  --provider <NAME>
  --provider <NAME>

  --channel <NAME>
  --channel <NAME>

  --cloud <TYPE>
  --cloud <TYPE>

  --include <WORD>
  --exclude <WORD>

  --jobs <N>
  --timeout <SECONDS>

  --proxy <URL>

  --format <table|json|jsonl>

  --check
  --valid-only

  --verbose
  --quiet
```

默认：

```text
source = all
format = table

jobs = 8
provider timeout = 30s

providers =
    15 个无登录 provider
    +
    已经配置且登录有效的 stateful provider

Telegram =
    config 中配置的默认 channels
```

如果 stateful provider 没有 profile：

```text
普通 pansou search:
静默跳过

显式：
pansou search xxx --provider qqpd

则返回 AUTH_REQUIRED
```

这可以避免默认搜索每次产生 4 个“未登录”警告。

---

# 6. Search 输出行为

默认 human-readable 输出以合并后的 link 为主，而不是输出完整原始 message。

例如：

```text
Query: 仙逆
Sources: 15 providers + 3 TG channels
Links: 37

[quark]

1. 仙逆 年番 4K
   https://pan.quark.cn/s/xxxx
   source: provider:meitizy

2. 仙逆 全集
   https://pan.quark.cn/s/yyyy
   source: tg:xxx

[baidu]

...
```

JSON 输出使用稳定 envelope：

```json
{
  "query": "仙逆",
  "total_results": 20,
  "total_links": 37,
  "results": [],
  "links_by_type": {
    "quark": [],
    "baidu": []
  },
  "source_errors": [],
  "duration_ms": 1234
}
```

不要保留 Go API 的：

```text
code
message
data
```

包装层。

也不要继续暴露：

```text
res=merge
res=results
res=all
```

CLI 内部始终维护完整结果结构。

table 只是一个 view。

---

# 7. Search 与 Check 联动

必须同时提供：

```bash
pansou check URL
```

以及：

```bash
pansou search "仙逆" --check
```

但两者必须保持模块解耦。

逻辑：

```text
search
  ↓
MergedLink[]
  ↓
如果 CLI 指定 --check
  ↓
调用 CheckEngine
  ↓
附加 CheckState
```

不要让 `Provider` 依赖 `LinkChecker`。

`--valid-only`：

```text
隐式启用 --check

最终只输出：
state == ok
```

`locked` 不算 valid。

---

# 8. Provider 命令

统一：

```bash
pansou provider list

pansou provider profiles <NAME>

pansou provider login <NAME>
pansou provider logout <NAME>
pansou provider status <NAME>

pansou provider configure <NAME>
```

登录 provider 使用 profile。

例如：

```bash
pansou provider login qqpd --profile main

pansou provider login qqpd --profile backup
```

搜索默认聚合所有 active profiles。

这对应现有 Go 实现的多账号能力。

---

# 9. QQPD CLI 设计

QQPD 当前能力必须保留：

```text
扫码登录
多账号
频道配置
guild_id 缓存
频道去重
多账号负载均衡
Cookie 持久化
session keepalive
```

CLI：

```bash
pansou provider login qqpd --profile main
```

行为：

```text
获取二维码
↓
优先在 terminal 绘制二维码
↓
每 2 秒轮询登录状态
↓
成功后保存 Cookie
↓
显示脱敏 QQ 信息
```

如果终端无法绘制二维码：

```text
写入临时 PNG
输出文件位置
尝试用系统默认图片查看器打开
```

支持 Ctrl-C 正常取消。

频道配置：

```bash
pansou provider configure qqpd \
  --profile main \
  --channels pd97631607,languan8K115
```

也支持：

```bash
pansou provider configure qqpd \
  --profile main \
  --channel pd97631607 \
  --channel languan8K115
```

保存时：

```text
normalize channel
dedupe
resolve guild_id
persist
```

不要移植 Web 页面。

### QQPD keepalive

CLI 不存在永久后台进程，因此不能照搬：

```text
每 3 分钟后台 keepalive goroutine
```

改为 opportunistic keepalive：

```text
每次 QQPD search 前：

if now - last_keepalive >= 3min:
    执行一次 keepalive
    更新 last_keepalive
```

这样不需要 daemon。

---

# 10. Weibo CLI 设计

保留：

```text
扫码登录
多账户
目标微博用户列表
正文链接提取
评论链接提取
账号负载均衡
Cookie 持久化
```

登录：

```bash
pansou provider login weibo --profile main
```

同 QQPD：

```text
QR
→ terminal
→ 轮询
→ cookie
→ profile
```

目标用户：

```bash
pansou provider configure weibo \
  --profile main \
  --users 1234567890,2345678901
```

或者重复：

```bash
--user 1234567890
--user 2345678901
```

同时支持输入：

```text
https://weibo.com/u/1234567890
```

保存时规范化成纯数字 ID。

搜索时聚合所有 active profile 配置的目标用户，并去重。

---

# 11. Gying CLI 设计

保留：

```text
多账户
username/password login
base_url
Cookie
browser verification
remote PoW
inline PoW
legacy hash challenge
搜索
详情资源解析
```

配置：

```bash
pansou provider configure gying \
  --base-url https://www.xn--wcv59z.com
```

登录：

```bash
pansou provider login gying --profile main --username xxx
```

密码不要作为普通命令行参数。

默认通过 TTY：

```text
Password:
```

隐藏输入。

自动化场景允许：

```bash
... --password-stdin
```

不要设计：

```text
--password xxx
```

避免 shell history 泄漏。

### Gying challenge

不要引入 Playwright、Chromium、Selenium。

当前 Go 实现本身没有依赖浏览器，Rust 继续使用纯协议实现。

必须支持三条 challenge 路径：

```text
1. remote PoW

GET /res/pow

N
x
t

重复：

y = y² mod N

POST /res/pow
y=<hex>


2. inline PoW

页面：

const json={id,N,x,t}

计算：

y = y² mod N

至少等待到 3 秒

POST 当前 URL

action=verify
id=...
y=...


3. legacy hash challenge

nonce = 0..diff

sha256(str(nonce) + salt)

匹配 challenge hashes

POST:

action=verify
id=...
nonce[]=...
```

Rust 使用：

```text
num-bigint
sha2
```

直接实现。

不需要 JS engine。

需要保留：

```text
browser_verified
PHPSESSID
app_auth
```

等 Cookie 的生命周期。

登录后 warmup：

```text
GET {base_url}/mv/wkMn
```

也要保留。

---

# 12. Panlian CLI 设计

登录：

```bash
pansou provider login panlian \
  --profile main \
  --username xxx
```

password 使用 TTY 或 stdin。

需要保留当前 Go 的：

```text
预建 PHPSESSID
登录
Cookie
token 解析接口
jump 页面 fallback
123 网盘 URL normalize
结果排序
blocked pan types
```

配置：

```bash
pansou provider configure panlian \
  --blocked-cloud pikpak \
  --blocked-cloud others
```

不要移植 HTML 管理页面。

---

# 13. Provider 状态存储

使用标准用户目录，而不是当前：

```text
./cache/
```

推荐：

Linux：

```text
$XDG_CONFIG_HOME/pansou/config.toml

$XDG_STATE_HOME/pansou/providers/
$XDG_CACHE_HOME/pansou/check.redb
```

macOS / Windows 使用 `directories` crate 获取平台标准路径。

结构：

```text
providers/
├── qqpd/
│   ├── main.json
│   └── backup.json
├── weibo/
│   └── main.json
├── gying/
│   └── main.json
└── panlian/
    └── main.json
```

StateStore 必须：

```text
atomic write
temp file + rename

Unix 文件权限 0600

禁止日志打印完整 Cookie/password/token
```

---

# 14. Secret 策略

用户名可以持久化。

password 默认不能以明文持久化。

Cookie 属于敏感数据。

支持：

```text
PANSOU_STATE_KEY
```

如果设置，则使用：

```text
AES-256-GCM
```

加密：

```text
cookie
password
token
```

没有 `PANSOU_STATE_KEY` 时：

```text
Cookie 可以保存到用户私有 state 文件
Unix 0600

但 password 绝不明文保存
```

对于 Gying / Panlian：

如果用户指定：

```text
--remember-credentials
```

必须要求：

```text
PANSOU_STATE_KEY
```

否则拒绝保存密码。

Cookie 失效并且没有保存 credentials：

```text
ProviderError::AuthRequired
```

要求用户重新执行 login。

---

# 15. Rust 目录结构

使用单 workspace / 单主要 crate。

建议：

```text
Cargo.toml
Cargo.lock

src/
├── main.rs
├── lib.rs
│
├── cli/
│   ├── mod.rs
│   ├── search.rs
│   ├── check.rs
│   ├── provider.rs
│   └── config.rs
│
├── config/
│   ├── mod.rs
│   └── paths.rs
│
├── core/
│   ├── mod.rs
│   ├── model.rs
│   ├── cloud.rs
│   ├── link.rs
│   ├── merge.rs
│   ├── filter.rs
│   └── rank.rs
│
├── http/
│   ├── mod.rs
│   ├── client.rs
│   ├── proxy.rs
│   ├── session.rs
│   └── encoding.rs
│
├── search/
│   ├── mod.rs
│   ├── engine.rs
│   └── telegram.rs
│
├── providers/
│   ├── mod.rs
│   ├── registry.rs
│   │
│   ├── pansearch.rs
│   ├── yunsou.rs
│   ├── djgou.rs
│   ├── hdmoli.rs
│   ├── meitizy.rs
│   ├── yulinshufa.rs
│   ├── clxiong.rs
│   ├── jsnoteclub.rs
│   ├── duanjuw.rs
│   ├── dyyj.rs
│   ├── jupansou.rs
│   ├── cldi.rs
│   ├── clmao.rs
│   ├── cyg.rs
│   ├── susu.rs
│   │
│   ├── qqpd/
│   │   ├── mod.rs
│   │   ├── auth.rs
│   │   ├── search.rs
│   │   └── model.rs
│   │
│   ├── weibo/
│   │   ├── mod.rs
│   │   ├── auth.rs
│   │   ├── search.rs
│   │   └── model.rs
│   │
│   ├── gying/
│   │   ├── mod.rs
│   │   ├── auth.rs
│   │   ├── challenge.rs
│   │   ├── search.rs
│   │   └── model.rs
│   │
│   └── panlian/
│       ├── mod.rs
│       ├── auth.rs
│       ├── search.rs
│       └── model.rs
│
├── check/
│   ├── mod.rs
│   ├── engine.rs
│   ├── cache.rs
│   ├── normalize.rs
│   ├── baidu.rs
│   ├── aliyun.rs
│   ├── quark.rs
│   ├── tianyi.rs
│   ├── uc.rs
│   ├── mobile.rs
│   ├── one15.rs
│   ├── xunlei.rs
│   └── pan123.rs
│
├── state/
│   ├── mod.rs
│   ├── store.rs
│   └── crypto.rs
│
└── output/
    ├── mod.rs
    ├── table.rs
    ├── json.rs
    └── jsonl.rs
```

不要创建：

```text
api/
server/
middleware/
web/
```

---

# 16. Core 数据模型

不要直接翻译 Go API struct。

建议：

```rust
enum CloudType {
    Baidu,
    Aliyun,
    Quark,
    Guangya,
    Tianyi,
    Uc,
    Mobile,
    One15,
    Pikpak,
    Xunlei,
    Pan123,
    Magnet,
    Ed2k,
    Others,
}
```

必须实现：

```text
Display
FromStr
serde Serialize/Deserialize
URL detection
```

Link：

```rust
struct Link {
    cloud_type: CloudType,
    url: String,
    password: Option<String>,
    datetime: Option<DateTime<Utc>>,
    work_title: Option<String>,
}
```

SearchResult：

```rust
struct SearchResult {
    id: String,
    source: Source,
    datetime: Option<DateTime<Utc>>,
    title: String,
    content: String,
    links: Vec<Link>,
    tags: Vec<String>,
    images: Vec<String>,
}
```

Source：

```rust
enum Source {
    Telegram { channel: String },
    Provider { name: &'static str },
}
```

MergedLink：

```rust
struct MergedLink {
    cloud_type: CloudType,
    url: String,
    password: Option<String>,
    note: String,
    datetime: Option<DateTime<Utc>>,
    source: Source,
    images: Vec<String>,
    check: Option<CheckResult>,
}
```

---

# 17. Provider trait

不要翻译当前 `AsyncSearchPlugin`。

使用真正最小接口。

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

ProviderMeta：

```rust
struct ProviderMeta {
    name: &'static str,
    priority: i32,
    requires_auth: bool,
    keyword_filter: KeywordFilterMode,
}
```

KeywordFilterMode：

```rust
enum KeywordFilterMode {
    Core,
    Provider,
}
```

用于替代 Go：

```text
SkipServiceFilter()
```

不要再通过：

```text
UniqueID prefix
```

反查 provider 行为。

---

# 18. Provider Registry

不要使用 Go `init()` + blank imports。

显式注册：

```rust
pub fn builtin_providers(...) -> Vec<Arc<dyn Provider>>
```

Registry 必须知道：

```text
provider name
metadata
是否 ready
是否需要 profile
```

`pansou provider list` 示例：

```text
NAME         AUTH      READY
pansearch    no        yes
yunsou       no        yes
...
qqpd         qr        yes (2 profiles)
weibo        qr        no
gying        password  yes (1 profile)
panlian      password  no
```

---

# 19. SearchEngine 行为

使用 Tokio。

并发方式：

```text
FuturesUnordered
+
Semaphore
```

禁止自己重新实现 worker pool。

行为：

```text
selected TG channels
+
selected providers
        ↓
并行
        ↓
每 source 单独 timeout
        ↓
收集 successes
        ↓
记录 source errors
        ↓
merge
        ↓
filter
        ↓
rank
        ↓
merge links
```

一个 provider 失败不能让整个 search 失败。

例如：

```text
pansearch timeout
meitizy success
cyg success

→ 正常返回结果
→ stderr 显示 pansearch timeout
```

只有：

```text
所有被选择的 source 全部失败
```

才返回非 0 exit code。

---

# 20. Search merge / rank 行为

优先移植当前：

```text
service/search_service.go
```

中的业务逻辑，而不是重新发明排序算法。

至少保留：

```text
UniqueID/result key 去重

相同 result 选择信息更完整者

Link URL 去重

相同 URL：
更晚 datetime 优先

保留稳定 source 顺序

work_title 优先于 message title

没有 work_title：
使用正文里的 link-title association

关键词优先词：

合集
系列
全
完
最新
附
complete

时间新鲜度

provider priority
```

Provider 的 priority 从对应 Go 实现迁移。

不要根据本方案中的“P0/P1”自行重新赋 priority。

---

# 21. 19 个 Provider 迁移矩阵

## pansearch

Source：

```text
plugin/pansearch/pansearch.go
```

必须保留：

```text
Next.js buildId 动态获取
search
HTML 数据解析
并发请求
真实链接解析
IP 403 / proxy 情况
```

不要硬编码 buildId。

---

## yunsou

Source：

```text
plugin/yunsou/yunsou.go
```

当前目标：

```text
wpys.cc
```

协议：

```text
HTML search
pagination
```

支持当前 Go 中已有的：

```text
quark
baidu
xunlei
aliyun
uc
```

---

## djgou

Source：

```text
plugin/djgou/djgou.go
```

保留：

```text
Z-Blog 搜索模板
详情页
网盘区域
BTWAF redirect
```

---

## hdmoli

Source：

```text
plugin/hdmoli/hdmoli.go
```

当前域名以 Go 为准。

重点 regression：

```text
redirect 时不能丢失 query 参数
```

---

## meitizy

Source：

```text
plugin/meitizy/meitizy.go
```

接口：

```text
https://apis.451024.xyz/api/media/search
```

必须设置和 Go 一致的：

```text
Origin
Referer
headers
```

解析：

```text
JSON
URL 中的 password
```

这是最优先实现的 provider 之一。

---

## yulinshufa

Source：

```text
plugin/yulinshufa/yulinshufa.go
```

必须正确处理：

```text
GBK request/response
```

使用：

```text
encoding_rs
```

不要假定所有 HTTP 页面是 UTF-8。

---

## clxiong

Source：

```text
plugin/clxiong/clxiong.go
```

当前：

```text
cilixiong.org
```

保留：

```text
两步搜索
btih / magnet 构造
```

---

## jsnoteclub

Source：

```text
plugin/jsnoteclub/jsnoteclub.go
```

重点：

```text
Ghost data-key
data-key 非 hex suffix
```

相关解析必须覆盖 fixture。

---

## duanjuw

Source：

```text
plugin/duanjuw/duanjuw.go
```

当前：

```text
sm3.cc
```

保留：

```text
搜索路径
chat bubble HTML 结构
夸克/百度
提取码
```

---

## dyyj

Source：

```text
plugin/dyyj/dyyj.go
```

使用：

```text
Flarum API
```

重点：

```text
mostRelevantPost
```

直接从 API 内容提取 link。

不要重新依赖受 challenge 的详情 HTML。

---

## jupansou

Source：

```text
plugin/jupansou/jupansou.go
```

复杂 provider，必须保留：

```text
dyuzi.com
session
SSE search
/api/transfer
```

Rust 使用 SSE parser。

可以使用：

```text
eventsource-stream
```

或实现严格测试过的最小 SSE parser。

临时 search session 不等于用户登录 profile。

不要错误放入 StateStore。

---

## cldi

Source：

```text
plugin/cldi/cldi.go
```

当前搜索 HTML。

从：

```text
/hash/<btih>.html
```

直接构造：

```text
magnet:?xt=urn:btih:
```

---

## clmao

Source：

```text
plugin/clmao/clmao.go
```

保留：

```text
Base64 页面 payload
详情页
magnet
pagination
```

---

## cyg

Source：

```text
plugin/cyg/cyg.go
```

当前：

```text
WordPress API
www.acgndog.com
```

优先 JSON API。

不要没有必要地用 HTML parser。

---

## susu

Source：

```text
plugin/susu/susu.go
```

保留：

```text
?rest_route=
form API
动态按钮层级
JWT
password
link extraction
```

如果某个按钮被 Cloudflare challenge：

```text
丢弃该无效 link
继续返回其他有效结果
```

不要因此让整个 provider 失败。

---

# 22. HTTP 基础设施

统一使用：

```text
reqwest
rustls
```

不要使用 OpenSSL 作为默认 TLS backend。

原因：

```text
减少系统依赖
方便 musl / cross compile
```

HttpClientFactory 支持：

```text
HTTP proxy
HTTPS proxy
SOCKS5 proxy

timeout
redirect policy
cookie jar
custom headers
user-agent
```

重要：

不同 provider session 必须使用不同 CookieJar。

禁止全局共享所有 provider cookie。

模型：

```text
Stateless provider:
shared HTTP client

Stateful provider/profile:
dedicated Session
```

---

# 23. Provider URL 可测试性

不要把所有 endpoint 直接散落在函数中。

每个 provider 推荐：

```rust
struct Endpoints {
    base_url: Url,
}
```

生产构造函数使用真实 URL。

测试构造函数允许：

```text
WireMock server
```

覆盖 endpoint。

这样 provider protocol tests 不依赖互联网。

---

# 24. Link parser

需要从：

```text
util/parser_util.go
util/regex_util.go
```

迁移网盘识别能力。

至少支持：

```text
baidu
aliyun
quark
guangya
tianyi
uc
mobile
115
pikpak
xunlei
123
magnet
ed2k
others
```

应该提供：

```rust
fn extract_links(text: &str) -> Vec<Link>
```

和：

```rust
fn detect_cloud_type(url: &str) -> CloudType
```

以及：

```text
password extraction
URL normalization
work title association
```

这些应该集中在 core，不要每个 provider 重复实现。

如果 provider 协议本身已经返回明确的 URL/type，可以直接构造 Link。

---

# 25. Check 子系统

Rust v1 必须完整迁移现有支持的 9 种检测器：

```text
aliyun
quark
uc
baidu
tianyi
123
xunlei
115
mobile
```

来源：

```text
service/check_service.go
service/check_mobile_crypto.go
```

必须保留状态：

```rust
enum CheckState {
    Ok,
    Bad,
    Locked,
    Unsupported,
    Uncertain,
}
```

语义：

```text
ok          链接有效
bad         链接已经失效
locked      需要提取码 / password 错误
unsupported 当前 checker 不支持
uncertain   网络/协议错误，无法确认
```

---

# 26. LinkChecker trait

```rust
#[async_trait]
trait LinkChecker: Send + Sync {
    fn cloud_type(&self) -> CloudType;

    async fn check(
        &self,
        ctx: &CheckContext,
        link: &Link,
    ) -> Result<CheckResult, CheckError>;
}
```

CheckEngine：

```text
normalize
↓
cache lookup
↓
concurrent check
↓
cache save
↓
ordered output
```

---

# 27. Check CLI

单链接：

```bash
pansou check https://pan.quark.cn/s/xxx
```

多个：

```bash
pansou check URL1 URL2 URL3
```

stdin：

```bash
cat links.txt | pansou check --stdin
```

参数：

```text
--type <cloud>
--password <pwd>     # 仅单 URL 时允许

--jobs <N>
--timeout <SECONDS>

--proxy <URL>

--refresh
--no-cache

--format <table|json|jsonl>

--fail-invalid
```

默认自动检测 cloud type。

---

# 28. Check cache

搜索结果 cache 删除。

但是 check cache 必须保留。

原因：

```text
网盘 check 有服务端风控
重复验证没有价值
CLI 每次进程都是一次性
因此必须有 disk cache
```

建议使用纯 Rust：

```text
redb
```

路径：

```text
cache/check.redb
```

key：

```text
cloud type
+
normalized URL
+
proxy scope
```

不要把完整 proxy URL 直接作为可读 cache key。

使用 hash。

TTL：

```text
严格迁移当前 Go CheckService 的各状态 TTL 策略。

不要在迁移过程中重新设计 TTL。
```

`--refresh` 绕过 cache 并覆盖。

`--no-cache`：

```text
不读
不写
```

---

# 29. Check 并发

当前 Go `CheckWithProxy` 顺序执行。

Rust CLI 可以直接改善为并发，但要限制并发。

默认：

```text
check jobs = 8
```

对同一 normalized URL 的重复输入：

```text
先 dedupe network request
然后恢复原输入顺序输出
```

不需要迁移 Go 的长期 `inflight map`。

CLI 进程生命周期很短。

---

# 30. Config

默认：

```text
config.toml
```

示例：

```toml
[network]
proxy = ""
timeout_secs = 30

[search]
jobs = 8
channels = ["tgsearchers3"]

providers = [
  "pansearch",
  "yunsou",
  "djgou",
  "hdmoli",
  "meitizy",
  "yulinshufa",
  "clxiong",
  "jsnoteclub",
  "duanjuw",
  "dyyj",
  "jupansou",
  "cldi",
  "clmao",
  "cyg",
  "susu"
]

[check]
enabled_cache = true
jobs = 8

[providers.gying]
base_url = "https://www.xn--wcv59z.com"

[providers.panlian]
blocked_clouds = []
```

优先级：

```text
CLI
>
PANSOU_* environment
>
config.toml
>
defaults
```

兼容旧变量作为 fallback：

```text
PROXY
HTTP_PROXY
HTTPS_PROXY
CHANNELS
ENABLED_PLUGINS
```

但 README 主要文档使用新的：

```text
PANSOU_PROXY
PANSOU_CHANNELS
PANSOU_PROVIDERS
```

---

# 31. 错误模型

库层不要到处使用：

```rust
anyhow::Error
```

定义：

```text
ProviderError
CheckError
ConfigError
StateError
HttpError
ParseError
```

可以使用 `thiserror`。

CLI/main 层可以使用 `anyhow` 汇总。

ProviderError 至少区分：

```text
Timeout
Network
Parse
AuthRequired
RateLimited
Blocked
Protocol
Unavailable
```

这样 JSON 输出的：

```text
source_errors
```

是机器可读的。

---

# 32. Exit code

建议：

```text
0
命令成功

2
CLI 参数或配置错误

3
Search 所有 selected sources 全部失败

4
显式选择的 provider 需要登录

5
全局网络 / 初始化错误

10
check --fail-invalid 且发现 bad / locked
```

部分 provider 搜索失败但仍有其他来源成功：

```text
exit 0
```

错误信息写 stderr。

数据写 stdout。

这样方便：

```bash
pansou search xxx --json | jq ...
```

---

# 33. Logging

使用：

```text
tracing
tracing-subscriber
```

规则：

```text
正常 table/json:
stdout = 数据

日志:
stderr
```

`--quiet`：

```text
只输出数据
```

`--verbose`：

```text
provider timing
request retry
challenge
cache hit
```

日志中必须 redact：

```text
password
Cookie
Authorization
token
qrsig
```

---

# 34. Rust dependencies

不要人工锁死本文中的具体 patch version。

Coding agent 使用当前兼容 stable Rust 的最新稳定版本，并提交 `Cargo.lock`。

建议：

```text
CLI
clap

Async
tokio
futures
async-trait

HTTP
reqwest
reqwest-cookie-store / cookie_store
url

Serialization
serde
serde_json
toml

HTML
scraper

Regex
regex

Encoding
encoding_rs

Time
chrono

Errors
thiserror
anyhow

Logging
tracing
tracing-subscriber

Crypto
sha2
aes-gcm
base64
rand

Gying PoW
num-bigint

Check XML/compression
quick-xml
flate2

SSE
eventsource-stream

State paths
directories

Check cache
redb

QR login
qrcode

Password input
rpassword

Human output
comfy-table

Testing
wiremock
tempfile
```

只有真正需要时再增加依赖。

禁止为了一个 provider 引入：

```text
Chromium
Playwright
Selenium
Electron
Node.js runtime
```

---

# 35. Tests 目录

```text
tests/
├── fixtures/
│   ├── telegram/
│   │
│   ├── providers/
│   │   ├── pansearch/
│   │   ├── yunsou/
│   │   ├── ...
│   │   └── panlian/
│   │
│   └── check/
│
├── provider_parsing.rs
├── merge.rs
├── ranking.rs
├── link_parser.rs
├── check_normalize.rs
└── cli.rs
```

每个 provider 必须有 fixture。

不能只有 live 网络测试。

---

# 36. Provider fixture 要求

每个无登录 provider 至少：

```text
1 个成功 search response

1 个空结果

1 个 malformed / changed response
```

复杂 provider：

jupansou：

```text
SSE snapshot
SSE multiple snapshots
done event
transfer response
```

Gying：

```text
normal search page
remote PoW
inline PoW
legacy hash challenge
login shell
nologin shell
detail JSON
```

QQPD：

```text
QR response
waiting
login success
channel page
search response
```

Weibo：

```text
QR
login
正文
评论
target user normalization
```

Panlian：

```text
login
search
token endpoint
jump fallback
123 normalize
```

---

# 37. Live tests

Live tests 不进入默认 CI。

使用：

```text
#[ignore]
```

或者环境变量：

```text
PANSOU_LIVE_TESTS=1
```

例如：

```bash
PANSOU_LIVE_TESTS=1 \
cargo test provider_pansearch_live -- --ignored
```

原因：

```text
provider 站点随时变化
Cloudflare
地域
IP
限流
```

CI 不能因为第三方网站临时不可用而失败。

---

# 38. Parity 策略

Go 是迁移期间的 oracle。

对于同一个 fixture：

```text
Go parser
Rust parser
```

最终 normalize 后比较：

```text
title
links
cloud type
password
work_title
```

对于能够 live 测试的 provider，可以临时执行：

```text
Go provider
Rust provider
```

搜索同一 query，并比较：

```text
URL set
```

不要要求顺序 100% 一致。

但核心 link 集合必须接近。

迁移完成之后，可以删除 parity 开发辅助代码。

---

# 39. Definition of Done：单个 Provider

任何 provider 只有满足以下全部条件才算迁移完成：

```text
1. Rust 实现存在

2. registry 注册

3. provider list 能看到

4. fixture parser test 存在

5. malformed response 不 panic

6. timeout 生效

7. proxy 走统一 HTTP 层

8. link 使用统一 CloudType

9. password / work_title 正确

10. ProviderError 正确分类

11. cargo fmt 通过

12. clippy 通过

13. unit tests 通过

14. 能做 live probe 时完成一次 probe
```

不能出现：

```text
TODO: later
unimplemented!()
todo!()
panic!("not supported yet")
```

用于 19 个已承诺 provider 的主路径。

---

# 40. 实现阶段

## Phase 0：建立 Rust 项目

创建：

```text
Cargo.toml
src/main.rs
src/lib.rs
```

设置：

```text
edition = 2024
```

先建立：

```text
cli
config
core
http
output
```

Go 代码暂时保留。

---

## Phase 1：Core

完成：

```text
CloudType
Link
SearchResult
MergedLink
Source

link parser
password parser
merge
filter
rank
```

从：

```text
model/*
util/parser_util.go
util/regex_util.go
service/search_service.go
```

迁移。

先完成 unit tests。

---

## Phase 2：HTTP + Telegram

实现：

```text
HttpClientFactory
proxy
timeout
encoding
Telegram provider
```

Telegram：

```text
https://t.me/s/{channel}?q={query}
```

迁移：

```text
ParseSearchResults
```

确保：

```bash
pansou search xxx --source tg
```

已经可以独立工作。

---

## Phase 3：第一批简单 Provider

按顺序：

```text
meitizy
cyg
yunsou
hdmoli
cldi
clxiong
duanjuw
dyyj
```

完成之后验证：

```text
JSON API
HTML
GBK 之外普通页面
magnet
Flarum
```

等基础能力。

---

## Phase 4：剩余无登录 Provider

按顺序：

```text
pansearch
yulinshufa
jsnoteclub
djgou
clmao
susu
jupansou
```

最后做：

```text
jupansou
```

因为涉及 SSE/session/transfer。

到此必须达到：

```text
15/15 stateless providers
```

---

## Phase 5：Check

迁移：

```text
check normalize
check cache
9 checkers
```

先实现：

```text
pansou check
```

再在 CLI 层接：

```text
pansou search --check
```

---

## Phase 6：StateStore

实现：

```text
ProviderProfile
StateStore
AES state encryption
atomic writes
profile commands
```

先不接 provider。

完成：

```bash
pansou provider profiles xxx
```

基础设施。

---

## Phase 7：账号密码 Provider

顺序：

```text
panlian
gying
```

先 Panlian。

最后 Gying，因为有 PoW challenge。

完成：

```text
17/19
```

---

## Phase 8：扫码 Provider

顺序：

```text
weibo
qqpd
```

实现：

```text
terminal QR
polling
Cookie persistence
configure
multi-profile
```

QQPD 再实现 opportunistic keepalive。

完成：

```text
19/19
```

---

## Phase 9：CLI hardening

完成：

```text
table/json/jsonl

stderr/stdout isolation

exit code

--quiet
--verbose

search --check
search --valid-only

config show
config path
```

---

## Phase 10：删除 Go Server

只有在：

```text
19 provider 完成
+
Telegram 完成
+
9 checker 完成
+
CI 通过
```

之后才能删除 Go 生产代码。

最终删除：

```text
api/
service/
plugin/
model/
util/
config/

main.go
go.mod
go.sum

Dockerfile
docker-compose.yml
```

如果 fixture 或迁移文档需要引用旧数据，可以迁到：

```text
tests/fixtures/
docs/migration/
```

不要最终保留两个生产实现。

Git history 已经保存 Go 版本。

---

# 41. README 重写

最终 README 应从：

```text
PanSou 网盘搜索 API
```

改成：

```text
PanSou CLI
```

首屏示例：

```bash
pansou search "仙逆"

pansou search "仙逆" --cloud quark

pansou search "仙逆" \
  --provider meitizy \
  --provider pansearch

pansou check \
  https://pan.quark.cn/s/xxxx
```

登录：

```bash
pansou provider login qqpd --profile main

pansou provider configure qqpd \
  --profile main \
  --channels xxx,yyy
```

README 必须明确：

```text
19 providers
9 link checkers
supported cloud types
proxy
config paths
state security
```

删除 Server/Docker/Nginx/JWT 文档。

---

# 42. CI

GitHub Actions 至少执行：

```bash
cargo fmt --check

cargo clippy \
  --all-targets \
  --all-features \
  -- \
  -D warnings

cargo test --all-features
```

Release build：

```text
Linux x86_64
Linux arm64

macOS x86_64
macOS arm64

Windows x86_64
```

优先 rustls，避免平台 OpenSSL 问题。

Linux 尽量提供：

```text
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
```

如果某个纯 Rust 依赖导致 musl 问题再评估，不要提前引入系统库。

---

# 43. Release artifact

GitHub Release：

```text
pansou-linux-x86_64.tar.gz
pansou-linux-aarch64.tar.gz

pansou-macos-x86_64.tar.gz
pansou-macos-aarch64.tar.gz

pansou-windows-x86_64.zip

SHA256SUMS
```

二进制：

```text
pansou
```

Windows：

```text
pansou.exe
```

---

# 44. 性能目标

不需要追求当前 Server 的长期 QPS。

CLI 优先优化：

```text
startup time
search latency
parallel provider execution
low idle complexity
```

目标：

```text
正常 CLI startup:
用户不可感知的明显延迟

provider 并发：
受 --jobs 控制

无无意义 background task

命令结束：
所有资源自然释放
```

不要为几十毫秒优化引入：

```text
object pool
custom allocator
复杂 cache
```

---

# 45. 安全原则

禁止：

```text
TLS InsecureSkipVerify 等价物
全局关闭证书验证

日志输出 Cookie
日志输出 password
日志输出 Authorization

将 username/password 放 URL

在 shell history 中要求用户传 password

为了 Cloudflare 启动隐藏浏览器
```

Proxy 必须由用户明确配置。

Gying 的 challenge 只能按当前 Go 的纯计算逻辑实现。

---

# 46. Coding Agent 工作规则

Coding agent 在实现过程中遵守：

```text
1. 不扩大 v1 provider 范围。

2. 不实现 Server compatibility layer。

3. 不为了兼容旧 REST API 启动 localhost HTTP Server。

4. “plugin” 在 Rust 中统一叫 provider。

5. 不迁移 search cache。

6. check cache 必须保留。

7. 不修改 provider 网站协议，除非当前 Go 协议已经失效。

8. 若发现网站协议变化：
   - 先确认当前 Go 是否同样失效。
   - 对 provider 做最小协议修复。
   - 添加 fixture。
   - 再继续迁移。

9. 不允许因为某个 provider 难做而从 v1 删除。

10. 不允许 silent failure。
    Provider 失败必须进入 source_errors 或 stderr。

11. 不允许 provider parse panic。

12. 不允许把网络 live test 作为默认 CI gate。

13. 每完成一个 provider：
    立即添加 tests。

14. 不要一次写完 19 个 provider 后才测试。

15. Go 实现直到最终 parity 完成后才删除。
```

---

# 47. 推荐 commit 粒度

建议：

```text
feat: bootstrap rust cli

feat: add core search models
feat: port link parser
feat: port merge and ranking
feat: add telegram search

feat(provider): add meitizy
feat(provider): add cyg
feat(provider): add yunsou
...

feat(check): add check engine
feat(check): add quark checker
...

feat(state): add provider profile store

feat(provider): add panlian auth
feat(provider): add gying auth and challenge
feat(provider): add weibo qr auth
feat(provider): add qqpd qr auth

feat: add search link validation

ci: add cross-platform build

docs: rewrite readme for rust cli

chore: remove legacy go server
```

避免一个巨大 commit。

---

# 48. Rust v1 最终验收标准

只有同时满足下面条件才能发布 v1：

```text
[ ] cargo build --release 成功

[ ] cargo fmt --check 成功

[ ] cargo clippy -D warnings 成功

[ ] cargo test 成功


Search:

[ ] Telegram

[ ] pansearch
[ ] yunsou
[ ] djgou
[ ] hdmoli
[ ] meitizy
[ ] yulinshufa
[ ] clxiong
[ ] jsnoteclub
[ ] duanjuw
[ ] dyyj
[ ] jupansou
[ ] cldi
[ ] clmao
[ ] cyg
[ ] susu

[ ] qqpd
[ ] weibo
[ ] gying
[ ] panlian


Check:

[ ] aliyun
[ ] quark
[ ] uc
[ ] baidu
[ ] tianyi
[ ] 123
[ ] xunlei
[ ] 115
[ ] mobile


CLI:

[ ] search table

[ ] search json

[ ] search jsonl

[ ] provider selection

[ ] TG channel selection

[ ] cloud filter

[ ] include/exclude

[ ] proxy

[ ] concurrency

[ ] timeout

[ ] search --check

[ ] search --valid-only

[ ] check

[ ] check cache

[ ] provider login

[ ] provider logout

[ ] provider status

[ ] provider configure

[ ] multi profile


Stateful:

[ ] QQPD QR login

[ ] QQPD channel configuration

[ ] QQPD guild_id cache

[ ] QQPD opportunistic keepalive

[ ] Weibo QR login

[ ] Weibo target-user configuration

[ ] Gying password login

[ ] Gying configurable base URL

[ ] Gying 3 challenge modes

[ ] Panlian login

[ ] Panlian token/jump flow


Quality:

[ ] 所有 provider 有 fixture

[ ] 所有 parser malformed input 不 panic

[ ] 无 plaintext password persistence

[ ] logs redact secrets

[ ] live tests 默认 ignored

[ ] GitHub Actions 跨平台通过

[ ] README 已完全切换到 CLI

[ ] Go Server 代码已从最终生产 tree 删除
```

---

# 49. 最终架构原则

如果实现过程中发生设计分歧，以这几个原则做决定：

```text
CLI 生命周期优先于 Server 生命周期。

Provider 是数据源，不是 Web plugin。

Search 是：
query -> results

Check 是：
link -> state

State 是：
provider authentication/session

三者分层。

并发使用 Tokio。

HTTP 使用统一 reqwest 层。

Provider 协议保持局部。

业务 parser 尽量复用 core。

外部站点失败必须 fail-soft。

登录 provider 没登录时不能拖垮普通搜索。

先正确，再抽象。

先 parity，再优化。
```

v1 的核心交付定义是：

```text
一个 Rust 二进制

能够：

搜索 Telegram
+
搜索 19 个指定 provider
+
管理 4 个 provider 的登录状态
+
合并、过滤、排序资源
+
检测 9 种网盘分享链接是否有效
+
以适合 shell 的方式输出结果

并且不依赖常驻服务器。
```
