# Provider 净差异与实施决策 C

本文件是[上游同步规格](../UPSTREAM_SYNC_2026_10_SPEC.md)的规范附录。

已完成端点 `beaa56133755a548ebc51b090b3816e2ae044aa6` → `221665fb2c9279f22af4684ea2d8c0c331eac1cc` 的分配净diff审阅，含测试、文档和fixture；未运行实现测试或live探测。以下“测试覆盖/测试要求”描述已阅读的上游断言和Rust实施要求，不宣称本地实现完成。

**决策优先级：主规格优先。** 本附录保留代码事实、上游行为和迁移分析；提到上游refresh/TLS开关/后台任务/默认值时不表示授权新增同名CLI能力。具体执行：不新增不安全TLS开关、不新增search refresh旗标、不启用daemon、不自动覆盖用户显式配置；新增来源默认加入策略已确认：新安装/无显式选择的升级跟随上游默认集合与已接入来源交集，默认集合内有状态来源就绪后自动加入，显式选择不覆盖；以主规格为准。详情/转存memo仅单命令实例内，部分结果终态和退出码按主规格。

## A. 跨来源的精确约束（下文逐项引用，不替代逐项说明）

### A1. 封顶读取

实际补读了上游 `util/http_body.go`：

- `MaxUpstreamResponseBytes = 16 << 20 = 16777216` 字节；读取 limit+1，**恰等于上限成功，大于上限错误**，不是静默截断，也不能只相信 Content-Length。
- 本批手动 gzip 的 `NewCappedReader(..., MaxDecompressedBytes)` 上限为 **64 MiB = 67108864**，超限给错误。**这些调用点只加了解压绝对大小上限，没有使用压缩比限制**。同文件虽然还定义 100:1/400KiB 的 `ReadAllDecompressed`，不能把未调用的策略说成本批 Provider 的净变化。
- Rust 实施：新增/复用流式 bounded-body helper；body 消费、解码也在超时/取消范围内；不以 `.bytes()`/`.text()` 完整读完后才检查长度。gzip 明确选择“输入/输出各封顶”的管线，避免 reqwest 自动解压绕过输出上限。超限进入来源错误或原本允许的详情失败分支，不产生被截断的成功结果。
- sopanya 自有 **2MiB**、woniu HTML 自有 **8MiB**、yingso 自有 **1MiB** 另列，不可统一升到 16MiB。其中 sopanya/woniu 的原实现 `LimitReader(limit)` 有静默截断缺陷，迁移时用 limit+1 显式拒绝，保留更小限制而不复制缺陷。

### A2. 代理与重试

- 本批添加 `util.ProxyFuncForTransport()` 的含义是自建/临时 HTTP transport 也遵循上游统一代理选择，不是改变站点请求协议。Rust 必须从现有 `HttpClientFactory` / `SearchContext.client` / 同配置派生 session/client 走同一代理策略，不能 Provider 内裸 `Client::new()` 绕开 CLI 代理或另开环境配置。
- 上游 `util/retry.go` 的 `Attempts` **含首次**；最后一次失败后**不再 sleep**；失败保留最后错误链、关闭失败 response；本文“3 次”均为总请求次数，不是首次+3。
- 本批采用公共 helper 的调用均未启用 jitter。不要凭 helper 支持 ±25% jitter 就在 spec 声称这些站点新增抖动。
- Rust 用异步 sleep、可取消的 request factory，每次 POST 重建/克隆可重放字节，不能照搬 Go `req.Clone` 共享已消费 body 的问题。指定总上下文的站点不因每次重试重置整轮截止；只有明确 per-attempt 的 quarktv 要按次新建请求超时。
- 所有状态码修复都应断言最终错误含真实 HTTP 数值（如 503），不是空结果、nil 格式占位符或成功。无需仿制 Go 中文包装文本。

### A3. Rust CLI 的交付截止不等于 Go 4 秒窗口

补读了 `plugin/publish_budget.go`：Go budget 是 `clamp(AsyncResponseTimeout - 800ms, 1800ms, 8s)`，默认窗口 4s，因此默认 **3.2s**。这不是所有站点请求的 4s 网络超时。

Rust `src/search/engine.rs` 当前在来源 future 外包 `options.timeout`，全局 `all_timeout`/中断会 drop owned futures，注释明确禁止 detached search tasks。迁移 weibo/woniu 等部分交付时，复用 CLI 实际剩余截止并在外层强制超时前交出快照/进度；**不引入 Go 的 4s/3.2s 固定 API 发布窗口，不让未完成请求留在后台**。若要保证来源内部已有结果不被外层 timeout 整批吞掉，需把准确 per-source deadline/取消信号传入上下文或提供来源内部进度通道；不能仅在 Provider 临时编一个独立 4 秒计时器。

### A4. 正则/JSON 专用实现

- 函数内正则移到包级的净变化是**编译一次、匹配模式不变**；Rust 用 `LazyLock<Regex>` 等复用。以下各项明确列出模式用途，不把它冒充新增解析协议。
- `encoding/json` → `pansou/util/json` 是 Go 实现统一（sonic `UseNumber=true, EscapeHTML=true, SortMapKeys=false`），本批 sdso/xiaokupan 均主要是强类型字段，不构成 Rust 更换 JSON 依赖的要求。Rust 继续 serde_json，数值 ID 用整型，保留 RawValue/typed scalar 的含义即可。

## B. 逐 Provider 结论与确定处置

### 01 quarkres

**文件映射（已审阅）**：`plugin/quarkres/quarkres.go`。

- 唯一语义净变化：`doSearch` 的无上限 `io.ReadAll` 改 A1 **16MiB**；HTTP 必须 200，随后反序列化 `{code,message,total,data}`，API code 必须 200，这些协议没有改。
- 补读目标代码确认 GET `https://squark.cc.cd/api/search?kw={QueryEscape(keyword)}`、Referer `https://squark.cc.cd/`，data 中保留 `unique_id/title/content/datetime/links[{type,url,password}]`。
- **Rust：新增 `src/providers/quarkres.rs` 并注册**（无账号，优先级2），按目标快照原协议适配，响应走16MiB helper；不额外造缓存/重试或把“上游已校验过链接”变成跳过 CLI checker 的权利。测试恰上限/超限、HTTP/API失败、成功/空/畸形 JSON。

### 02 quarksoo

**文件映射（已审阅）**：`plugin/quarksoo/quarksoo.go`；`plugin/quarksoo/quarksoo_retry_test.go`。

- URL `https://quarksoo.cc/search.php` 常量改可注入变量仅为本地测试；不是域名更新。
- 手写 retry → `DoWithRetry`，`p.retries+1`，默认 `MaxRetries=2` 即 **3次、间隔500ms/500ms**。网络错误、读取失败、非200均重试；每轮体读完即关闭，不再 defer 积压；16MiB上限。失败最终返回错误，不能当空结果。
- 测试真实断言：500、500、200时恰3请求；持续500恰retries+1且err非nil；成功测试耗时不超过3s（未严格断言每段500ms）；用临时缓存目录和30s异步响应窗口隔离 Go 框架，**不意味着 CLI 要加搜索缓存或30s固定窗口**。
- **Rust：新增 `quarksoo`**，保留搜索 HTML 协议与这些请求/错误语义；endpoint可依赖注入，wiremock复刻两种三次请求场景和body关闭/超限错误。

### 03 quarktv

**文件映射（已审阅）**：`plugin/quarktv/quarktv.go`；`plugin/quarktv/quarktv_retry_test.go`。

- 自建 client 加统一代理。`fetchBody` 改公共 retry，**3次总尝试，200ms、400ms指数退避**；timeout context 明确在每轮闭包内新建，按次计算（搜索15s、详情10s），不是共用整轮10/15s。
- 移除无上限自写 `ioReadAll`，改16MiB、立即关体。**还有真实行为扩展**：旧实现 HTTP200后直接返回 readErr，新实现读取错误（包括超限）也进入重试，不能仅称等价重构。删除“错误字符串含EOF就当正常结束”的自写读取行为。
- 测试：500×2后第3次返回`<html>ok</html>`；持续40MiB体必须报错。第二个测试实际写40个1MiB块，不是只设置超大Content-Length，注释有误。
- 已补读目标常量：`https://www.quarktv.com/?s=%s`；postkit `GET /wp-json/postkit/v1/pan_view?post_id=%s&visitor_id=%s`，优先级2，详情并发10。
- **Rust：新增 `quarktv`**，HTTP共享代理、3次按次超时、16MiB及读取失败重试；受CLI总deadline进一步限制。不能把每次超时误写成整轮超时。

### 04 qupanshe

**文件映射（已审阅）**：`plugin/qupanshe/qupanshe.go`。

- `getFormhash` 和 `getSearchResults` 的手动gzip reader加 **64MiB解压上限**；`postSearchRequest` 仅在 DebugLog 且非301/302的调试读取改16MiB。**净差异没有给非gzip goquery直读分支加16MiB限制**，不得夸大已全面覆盖。
- 清标题 `<[^>]*>`、回复/查看统计 `(\d+)\s*个回复\s*-\s*(\d+)\s*次查看` 预编译，模式不变。
- **Rust：新增 `qupanshe`**，按目标原有 cookie jar → formhash → POST 搜索 → 跳转结果页流程实现；会话 client继承CLI代理，gzip输出64MiB；所有网络读落实A1而不是保留非gzip漏洞。调试只做安全摘要，不能迁移原代码打印全Cookie/header。静态正则复用，无搜索缓存。

### 05 qupansou

**文件映射（已审阅）**：`plugin/qupansou/qupansou.go`。

- 真实变化只有搜索API读体16MiB；余为gofmt。原有1小时cache/Ticker并未新增，不能计为本次能力。
- 目标协议在diff已完整体现：POST `https://v.funletu.com/search` JSON `style=get,datasrc=search,query.searchtext=关键词,query.courseid=1,page.pageSize=1000,page.pageIndex=1,order={prop:sort,order:desc}`，Referer `https://pan.funletu.com/`；API status=200，跳空URL，不返回密码，解析`UpdateTime`格式`YYYY-MM-DD HH:mm:ss`。
- **Rust：新增 `qupansou`**，优先级3，保留该请求和结果映射，16MiB；**不搬 cacheTTL/后台清理**。测试大列表合法但响应超16MiB仍失败。

### 06 sdso

**文件映射（已审阅）**：`plugin/sdso/sdso.go`。

- JSON decoder换Go统一封装；标题span/HTML标签正则预编译；重试在响应非200且网络err=nil时构造真实状态码错误，解决最终`%!w(<nil>)`。
- 保留 **3次、200ms/400ms**，本次没有公共retry替换、代理新增或ReadAllLimited变化。GET `https://sdso.top/api/sd/search?name={keyword}&pageNo={n}&from={type}`、多网盘分页/AES链接解密逻辑未改，不能说是新增AES协议。
- **Rust：新增 `sdso`**，按目标多类型分页和加密URL协议实现；使用serde typed响应，静态span/HTML正则，最终保留HTTP状态码。重放/截止按A2；不为Go封装引入sonic。新provider同时采用共享有界读取，不声称这是此文件净diff新增的上限。

### 07 shandian

**文件映射（已审阅）**：`plugin/shandian/shandian.go`。

- 仅两项：自建transport遵循统一代理；3次retry非200的错误保留真实status。**200ms/400ms**不变；失败后检查req.Context取消并及时返回。
- **Rust：新增 `shandian`**（以目标现有搜索/详情HTML解析为基线），统一client、三次指数退避、非200可观测、取消立即终止；不因只涉及transport而遗漏，也不杜撰域名/解析器更新。

### 08 sopanya（完整新来源）

**文件映射（已审阅）**：`plugin/sopanya/sopanya.go`（新增543行全读）；`plugin/sopanya/sopanya_test.go`（新增76行全读）。

- 无登录、优先级3，base `https://sopanya.com`。trim空关键词直接空结果。整次context **28s**；client timeout至少28s（Rust不能扩大用户明确设置的总截止）。GET `/api/other/web_search?title={encoded}&is_type=-1`，Accept SSE，Origin/Referer都用base。
- 按行读SSE：只收`data:`；空payload跳过、`[DONE]`停止、坏JSON忽略；event必须title和url非空且`type==""`（`progress`等事件排除）。stream **2MiB**，Scanner单行最大**256KiB**、初始buffer4096。最多**24候选**、每个`is_type`最多**8**、按event.URL token去重；**6 workers**解析，同时继续读stream；最后按候选序号稳定还原顺序。
- event.URL若直接含受支持的分享URL，第一条放URL，其余放Content，不必调用save接口。
- 否则 POST JSON `/api/other/save_url`，payload是 `{url: QueryEscape(event.URL),title:event.Title,stoken:"",description:event.Description}`。注意**opaque token先URL编码后再放JSON**。响应HTTP任意2xx才继续；`code`字段实际没有用作成功判断，不能擅自宣称只接受code200。
- 无data或data.URL无可识别分享时，message含`封禁/登录状态异常/登录异常/链接受限/无权限/forbidden/unauthorized`直接丢候选；其他情况每次等**300ms**后 POST同一payload到 `/api/other/save_status`，最多**8次**；等待`data.ready && resource!=nil`并且resource.URL中有分享链接。save/status body也各**2MiB**。按resource `URL/Content/VODContent`三处采集并去重。
- 分享识别：HTTP(S)的quark、aliyun/alipan、baidu、uc、xunlei、189、115、123pan、mypikpak，对应Rust公共cloud类型（天翼`Tianyi`等），不增checker。解HTML实体及`\/`，剥尾中英文标点；不保留陌生网页为分享。
- 密码URL query key大小写不敏感，`pwd/password/code/passcode`；再按`提取码/密码/访问码/访问密码/password/pwd/code`文本模式取3–16位字母数字。每条URL密码优先，其次data.Code/data.URL/data.Content/data.Description/event.Description。**实码缺口：裸data.Code="abcd"并不会被`extractPassword`接受，它只接受query/带标签文本；迁移应直接接受结构化code，不复制这个无测试覆盖的缺陷。**
- 时间优先update_time后create_time；正数数字 >1e12 按毫秒/1000，否则秒；字符串支持RFC3339、`YYYY-MM-DD HH:mm:ss`、`YYYY-MM-DD`。没有源时间时Go结果时间用now（链接时间仍零）。URL+密码作为去重key；整条结果只有所有链接都已见才丢，部分重复时原Go仍保留该结果全部links。
- UniqueID=`sopanya-`+SHA256(首URL+NUL+密码)前8字节hex（无links才用title）；title优先data再event，content优先data.Description再event.Description再data.Content；最后做关键词过滤。
- 测试覆盖：支持域名/陌生域名拒绝；progress忽略、opaque-token经save_url解到百度`pwd=xeqy`，ID前缀；中文尾标点两链接及pwd。**没有**覆盖status轮询、上限、永久失败、空/畸形流和裸code；迁移必须补wiremock断言这些边界。
- **Rust确定处置：新增 `sopanya` 并注册**，实现上述SSE+解析管线和24/8/6/8×300ms等限制，保持排序/去重；2MiB用显式超限错误。超时/取消无结果应报告timeout，而不是照搬Go“ctx已超时且0结果返回nil”的漏洞；不引入后台job或缓存。

### 09 sousou

**文件映射（已审阅）**：`plugin/sousou/sousou.go`。

- `searchByType`每页API读体16MiB；Panso日期`YYYY-MM-DD[ HH:mm:ss]`、密码`提取码|密码|pwd`+4位字母数字正则预编译，其余没变。
- **Rust：新增 `sousou`**，保留目标的按diskType分页及现有panso HTML路径，不把预编译称为新fallback；每页16MiB，静态日期/密码Regex，无搜索cache。fixture断言4位密码和已有日期格式不变。

### 10 susu（Rust已有）

**文件映射（已审阅）**：`plugin/susu/susu.go`。

- **只有**从详情href提取postID的 `/([0-9]+)\.html` 等价正则移到包级。没有API、JWT、链接/password或cache策略变化。
- 当前Rust `src/providers/susu.rs::posts` 每次创建id Regex；**明确修改为LazyLock复用**，保留`item-...`优先、href兜底。复用现有fixtures并补href兜底ID回归即可；无需重写协议/新增缓存。

### 11 thepiratebay

**文件映射（已审阅）**：`plugin/thepiratebay/thepiratebay.go`。

- 大量空白/gofmt，实际仅统一代理、非200重试末错误含status。**3次、200ms/400ms**。
- 搜索端点`https://thpibay.xyz/search/{pathescaped}/1/99/0`及其他页、MaxPages=30、并发上限200、40hex magnet检查、title_en扩展和标题点号换空格均为原有，不是净新增。24小时pageCache也是原有。
- **Rust：新增 `thepiratebay`**，按目标原协议解析magnet/种子metadata、保留30页上限并遵守CLI jobs/站点并发；共享代理和明确HTTP失败；不照抄200全局高并发或pageCache/清理goroutine。不增加magnet checker网络操作。

### 12 u3c3

**文件映射（已审阅）**：`plugin/u3c3/u3c3.go`。

- 获取search2参数页面、搜索结果页面两处都加16MiB；提取`var nmefafej = "..."`、去HTML和折叠空白三正则改编译一次，接受模式不变。
- **Rust：新增 `u3c3`**，按目标“先发现动态search2再搜索”流程；两处独立封顶、Regex复用；不能只限制结果页遗漏token/bootstrap页。成功/缺search2/超限页fixtures均需有。

### 13 wanou

**文件映射（已审阅）**：`plugin/wanou/wanou.go`。

- 实际变化：优化client遵循代理；API读体16MiB；其余空白/排版。原有重试**2次、100ms间隔**没改，不应写成3次500ms。
- GET `https://woog.nxog.eu.org/api.php/provide/vod?ac=detail&wd=...`；code=1成功，`vod_down_from/vod_down_url`按`$$$`对应，密码query提取、支持多盘/magnet/ed2k为既有协议。
- **Rust：新增 `wanou`**，按目标VOD JSON映射、16MiB、2次100ms、共享代理；磁力与ed2k保留为链接而非新增checker；无daemon/performance stats管理端。

### 14 weibo（Rust已有）

**文件映射（已审阅）**：`plugin/weibo/weibo.go`。

- `executeTasks`新增按PublishBudget计算deadline、过期未开始任务计skipped并不发请求、到点停止等待并在锁内复制已有结果返回。原`wg.Wait()`必须等全批用户3页+评论完成，导致Go发布窗口丢整批。Go未完成任务仍后台结束，**此部分不迁**。
- 这里的快照按**整个用户任务完成**后追加，不是每条微博/每页即时交付；无已完成用户时仍可能空快照。循环中semaphore容量沿用`MaxConcurrentWeibo=30`，并不是文件头`MaxConcurrentUsers=10`真的控制了这段；不要据名字误写净变化。Rust当前`buffered(10).collect()`也是等全批，需要改有界增量收集才能迁到实际收益。
- `searchUserWeibo`客户端超时 **30s→15s**；comments client仍30s、外链页15s，不能声称微博全部HTTP都降15s。
- **6处**ReadAll改16MiB：搜索微博、hotflow评论、评论/正文跳转外链页、二维码状态JSONP、二维码info、二维码图片（info+image是两个独立调用点）。不得遗漏登录链路也必须封顶。
- 5个预编译正则：sinaurl中u参数、去HTML、空白、api_key、qrid；协议模式不变。PC/mobile cookie refresh、Web管理、QR retcode判断等其余大段是格式化，并没有新增。
- **Rust明确改** `src/providers/weibo/search.rs`、必要时`auth.rs`和HTTP helper：搜索request按15s（且不越CLI剩余timeout）、所有上述读取有界、regex单次编译；把当前等全批改成截止前部分收集并取消剩余owned futures，精确deadline由engine传递；保留当前profile/session和source_error契约，不迁Gin、hash管理网址、debug里打印Cookie/qrid/altURL或detached后台任务。
- 测试必须两用户一快一慢，deadline返回快者且慢者取消；无结果全失败仍source_error，15s只影响搜索；QR/图片超限安全失败。原diff没有新增对应weibo单测，不可声称已受上游测试约束。

### 15 woniu（完整新来源）

**文件映射（已审阅）**：`plugin/woniu/README.md`；`plugin/woniu/woniu.go`（新增1410行全读）；`plugin/woniu/woniu_live_test.go`（127行）；`plugin/woniu/woniu_test.go`（763行）。四文件包括全部内嵌fixtures/断言均已审阅。

**协议/数值**：

- 优先级2、需要Password auth；默认 `https://wn4k.com`，可自定义base_url，登录/搜索/详情统一基于当前base。用户名密码POST `/user/login.html`，form `user_name/user_pwd`，Content-Type form+UTF-8，`X-Requested-With: XMLHttpRequest`；HTTP200且JSON `code=1`成功，失败暴露msg的安全文本；独立jar收Cookie，成功但无Cookie仍失败。不持久化密码。
- 普通request20s、login30s，HTML8MiB、登录读体16MiB；GET `/user/index.html`校验会话，最终URL或302 Location到`/user/login`即失效；GET `/user/logout.html`通知退出，站点退出失败也删除本地会话。verify/logout最多drain64KiB。
- GET `/vodsearch/-------------/?wd={encoded}`，**只首页**，最多24影片，不尝试虚构分页。`a.video-card`，href `/voddetail/(\d+)/?`，ID去重，title属性优先`.video-title`，poster/episode/meta映射；详情并发**6**、最多每结果**200链接**。
- 详情GET卡片URL，带所选账号Cookie。`.pan-lock-box`或同时含“登录后可见”和“******”视为锁定。无可用账号**明确auth错误**；有搜索影片但所有详情无真实链接**明确协议/登录态错误**，不吞成空结果。无影片才正常空结果；一些详情失败仍保留其他结果。
- 标题优先`h1.premium-title`，content组合Note/Meta/`#detailDescText`（兜底`.detail-desc-text`），简介最多300 Unicode字符加...、忽略“暂无简介”；`.premium-tags-top .p-tag`剔除★评分；`.premium-meta-grid .meta-item`的时间支持`YYYY-MM-DD HH:mm`/含秒/纯日期；返回搜索poster优先、详情poster兜底。
- 每`.pan-link-item`依次尝试`data-pan-item`、首`a.pan-link-btn[href]`、首`[data-copy]`、所有`.pan-link-meta`文本；**每记录取首个合法链接**，全结果按URL去重；拒绝空/******，解HTML实体，空白/引号/尖括号截尾、去尾中英标点；query密码优先`pwd/password/passcode/code`，文本fallback regex3–12位字母数字。URL保留真实query。
- 盘型含115域名`115cdn.com/115.com/anxia.com`、百度、夸克、UC、阿里/alipan、光鸭guangyapan、迅雷、189、123（123684/123685/123912/123pan.com/123pan.cn/123592）、移动caiyun.139.com、pikpak、weiyun、lanzou、magnet、ed2k，陌生HTTP链接为others。**Go按字符串contains识别域名且HTTP只查前缀，Rust应使用URL hostname边界；不要复制`evil/?u=115.com`误判和非法URL接受。**公共模型按主规格明确新增Weiyun/Lanzou，外部值分别weiyun/lanzou，不映射为Others；域名识别、过滤、JSON及Unsupported验收同步扩展。无需因有15类输出就加15个checker。
- Link.WorkTitle必须带影片名：trim后二者空时回退、note等于/已包含movie则不重复，否则`movie + " " + note`。ID和MessageID均`woniu-{id}`，结果Channel空；结果按Title排序并最终关键词过滤。这是保住“庆余年”+仅规格备注不被合并后关键词过滤清空的关键，需适配当前Rust link title/work-title公共字段而非把备注覆盖作品名。

**账号/配置与CLI迁移**：

- Go账号hash是SHA256(username+WONIU_HASH_SALT)，默认salt=`pansou_woniu_secret_2026`；`{CACHE_PATH}/woniu_users/{hash}.json` 0600（Cookie而非密码），配置`woniu_config.json`0644；账号active且Cookie非空可选，最近LastAccessAt优先。Go实际getActiveUsers不按ExpireAt过滤（注释说约30天但不能靠文档声称会自动拒绝过期），Cookie无expires时估30天；Rust按已有StateStore/profile expiry约束实现更可靠。
- base trim、无scheme补https、去尾slash；修改base_url会令所有会话expired需重登（即使传同值原Go也失效）。CLI需明确支持来源base_url配置并在真实站点变化时失效绑定的session，禁止旧站Cookie随意带到新站。
- Go有status/login/logout/get_config/update_config/test_search动作、hash路由；**CLI转换成现有auth/profile/config子命令**，密码只TTY或`--password-stdin`；不迁Gin、Web页、账号hash权限机制或管理API。“Web管理界面”README与实码不一致：GET hash只返回JSON，没有HTML UI。
- Go后台启动后等2min、每小时验证、距上次检查<30min跳过；Rust无daemon，**确定不迁周期goroutine**，以auth status/登录后/搜索前必要时按需校验更新状态。会话持久化保留，绝不是引入搜索缓存。
- **重要实现/文档不一致**：`fetchDetails`只在主循环发goroutine前检查deadline，sem获取后不复检，最后无界`wg.Wait()`；24个task在快循环里可全部入队，且in-flight每次20s。README“到点先交已有结果”及live test预算断言**不是代码保证**。Rust实施应6工人、领取任务/请求前复检准确CLI deadline、截止返回已有并取消剩余；不能照搬该循环后声称实现了预算交付。
- 另一缺陷：normalize先截去空白尾部再从raw找“提取码”，因此“URL 提取码abcd”（无query）会丢文本密码，测试只覆盖query已有password=1234。迁移先保存原串做密码上下文，再裁剪URL。
- 手动loadDocument遇3xx最多补一次GET并复制Cookie；Rust用有界重定向且遵循同源session作用域，不向跨域Location发送原站Cookie。

**测试实质**：

- 本地测试覆盖：搜索卡片去重/非法/无标题/相对图；115/百度/夸克密码与WorkTitle；四来源同URL不重复；href兜底UC；锁定class与文本占位符拒绝；所有盘型和others/非法；尾文本裁剪；content/tags/time；带Cookie整链路、ID/Channel/images；无账号明确报错、全部详情失败明确报错；登录form/Cookie/code/msg；302失效；0600落盘重载；active筛选；base规范化及变更使账号失效/拒空；hash稳定64hex；管理status/login/config/logout/unknown、未登录测试搜索拒绝、用户名入口302；“庆余年”WorkTitle两回归。
- live test仅显式WONIU_LIVE且凭据存在才跑：真实登录verify+完整搜索、非空链接无占位符、空Channel、woniu前缀、搜索不超PublishBudget；另测游客`/voddetail/7128/`锁定。**本次没有运行live测试，不把这些当线上现状已证实**。
- **Rust：新增 `src/providers/woniu/{mod,auth,search}.rs`、注册Password auth及现有状态/CLI路径**，以上本地测试转换wiremock/fixtures并新增截止/超限/纯文本密码/跨域cookie回归；所有实现都在CLI命令生命周期内。

### 16 wuji

**文件映射（已审阅）**：`plugin/wuji/wuji.go`。

- 搜索HTML和详情HTML各从裸ReadAll改16MiB；广告`【...】`/开头数字+广告、方括号、空白四正则改静态。其余gofmt。
- `https://xcili.net/search?q=%s&page=%d`、固定最多5页、并发10、详情`input#input-magnet[value]`原有。原3次retry只对网络err，**1s/2s线性等待**；HTTP非200在外层报错、并不在helper中重试，本次未改。
- **Rust：新增 `wuji`**，两处封顶/静态regex，保留有效magnet才返回、不把detail占位符输给CLI；不迁1小时magnetCache，retry不能被统一成3次200ms+所有status。

### 17 xb6v

**文件映射（已审阅）**：`plugin/xb6v/xb6v.go`。

- 手动gzip reader输出封64MiB；本次不含非gzip读取统一封顶。预编译六正则：JS location.href、带searchid的href/url、result/?searchid=数字、内容名含数字、空白折叠、`/{id}.html`。
- **Rust：新增 `xb6v`**，保留现有搜索POST/Location或JS-searchid结果跳转和详情磁力解析，gzip64MiB且其余读受共享上限，静态regex；不声称本次新增JS/meta fallback或新的站点协议。

### 18 xdpan

**文件映射（已审阅）**：`plugin/xdpan/xdpan.go`。

- 只有5个正则提到包级：`时间: YYYY-M-D`、`格式:`、script `window.open` 百度URL、`[window.]location.href` 百度URL、content `详情: URL`。**window.open/location两种跳转在基准已经存在，本次不是新增兼容路径**。
- **Rust：纳入新增 `xdpan`**，按目标原协议实现后使用单次编译等价Regex（用户同意迁全部CLI适用来源，不因变更仅性能就默认排除）；测试两种script跳转/时间/格式现有行为不变。无新增缓存/重试/端点更改需求。

### 19 xdyh

**文件映射（已审阅）**：`plugin/xdyh/xdyh.go`。

- 自建client统一代理；搜索读体16MiB；非200三次重试正确保留status；间隔**500ms、1000ms**，不是常见200/400。其余格式化。
- 原POST `https://ys.66ds.de/search` JSON `{keyword,sites:null,max_workers:10,save_to_file:false,split_links:true}`，timeout15s，status="success"，data DriveLinks/Password转换未改。
- **Rust：新增 `xdyh`**，保持原JSON/密码/结果映射、15s和3次500/1000ms且POST body可重放、16MiB、共享代理；不迁20分钟清空的searchCache。

### 20 xiaoji

**文件映射（已审阅）**：`plugin/xiaoji/xiaoji.go`。

- 唯二真实变化：优化transport统一代理、非200重试末错误不丢status；**3次、200ms/400ms**。大量其余为gofmt。
- 原`https://www.xiaojitv.com/?s=...` → `.resource-compact-link a`详情 → `/go.html?url={base64}`解真实链接/直接http/magnet/ed2k，没有本次新解码协议。搜索10s/详情8s保持。
- **Rust：新增 `xiaoji`**，共享代理、明确status和按原退避；重现原详情/direct/base64协议，不迁detailCache/30min清理。不要把格式化里的base64重试说成新功能。

### 21 xiaokupan（Go专属实现替换）

**文件映射（已审阅）**：`plugin/xiaokupan/xiaokupan.go`。

- 本文件全部语义差异是Go标准JSON换统一utiljson：serovalNode.Scalar RawMessage alias、buildSearchPayload Marshal、response Unmarshal、reference ID/int和字符串scalar decode。**seroval引用深度16、结构type=4引用/type=1字符串、server-function动态发现、请求payload完全未改**。
- **Rust确定处置：不为这一Go依赖替换单独新增Provider/更换serde_json**；这不是因当前来源只有19而排除，而是没有新的CLI适用协议/行为。若其他分组有xiaokupan完整来源接入要求，使用现有serde_json强类型/RawValue等价解析并保留深度16即可；本批没有新的实现功能要加、没有“待核查”事项。

### 22 xiaoyu

**文件映射（已审阅）**：`plugin/xiaoyu/xiaoyu.go`。

- 自建client统一代理；公共retry替代复制循环，**maxRetries=2所以3次总尝试、线性200ms/400ms**，HTTP200才成功、失败关体。虽然三次时数值和指数一样，但策略是线性，若扩次数下一段应600ms而非800ms。
- 原GET `https://xykmovie.com/s/{page}/{keyword}`，15s、最多10页、页并发5、自有2MiB读限制没有在本次改。
- **Rust：新增 `xiaoyu`**，统一代理、可取消线性retry、保留2MiB不升16MiB；复刻多页解析，不新增缓存。

### 23 xiaozhang

**文件映射（已审阅）**：`plugin/xiaozhang/xiaozhang.go`。

- `doRequest`每次新建的临时transport（用来切换followRedirect并DisableCompression）加统一代理，不能只给主client配代理。
- 搜索/详情两个gzip路径各封64MiB；`/subject/{id}.html`、`提取码|密码`字母数字密码两regex静态。非gzip直读本次未封顶；其余格式化。
- **Rust：新增 `xiaozhang`**（`https://xzys.fun/search.html?keyword=...`），no-follow取得Location、再follow到真实详情的client都继承CLI代理，解压输出上限+共享有界读，Regex复用；不迁30min详情缓存及每项sleep清理任务。

### 24 xinjuc

**文件映射（已审阅）**：`plugin/xinjuc/xinjuc.go`。

- 优化client统一代理、三次retry非200保留status（**200/400ms**）；4个简介清洗regex静态：空白、百度下载说明、百度URL、“提取码4位”。其余gofmt；没有ReadAllLimited/gzip变化。
- 原`https://www.xinjuc.com/?s=...` → `div.article-content`，百度`/s/`后至少10位、query密码优先文本、搜索10s/详情8s/并发15等未改。
- **Rust：新增 `xinjuc`**，共享代理/真实错误/静态regex，按目标百度特定解析保留query密码；同时用共享bounded read但不宣称是此文件已有净变化；不搬detailCache。

### 25 xuexizhinan（纯格式化）

**文件映射（已审阅）**：`plugin/xuexizhinan/xuexizhinan.go`。

- 本文件120行diff**全部尾空白/gofmt/末尾换行**；URL、10s、并发8、24h缓存、解析/匹配/错误均无净语义变化。
- **Rust确定不因这个文件新增来源或改变协议**。不能把上下文里`xuexizhinan.com/?post_type=book&s=%s`、标题全部关键词匹配、magnet/quark、cache等原有代码当新需求。已完整核查，无待办。

### 26 xys

**文件映射（已审阅）**：`plugin/xys/xys.go`。

- 搜索API响应16MiB；3次retry末错误含HTTP status（**200/400ms**）；4个regex静态：`const DToken`、HTML、空白、带秒datetime。原token流程/URL未改。
- Base `https://www.yunso.net`，TokenPath `/index/user/s`，SearchPath `/api/validate/searchX2`，MaxResults50为既有协议，和另一个Provider `yunso`不能合并误认。
- **Rust：新增 `xys`**，token bootstrap和SearchX2按目标协议、API body16MiB/真实状态码/静态regex，统一client；不把预编译说成新token提取机制。

### 27 yingso

**文件映射（已审阅）**：`plugin/yingso/yingso.go`；`plugin/yingso/yingso_test.go`。

- 实质站点修复：GET配置按 **`/test1`+UTF16循环XOR密钥`672134612` → `/test`+`12345678`**顺序尝试。不只404回退：网络/HTTP错误、envelope非200/空data、XOR后坏JSON、配置必要字段/区间不合法也回退。
- 必须URLVersion/UserID非空且 **0 <= start < end <= 24**；每候选必须用其配对key，不能先用新key失败后仅切URL。
- 真实链接POST按 **`/{url_version}/getKey670I23762183` → `/{url_version}/getKey`**，请求/JSON/envelope非200或空白data均回退；每轮清空response，避免旧响应遗留；最后错误含item ID。只需payload `{id,userId}`，不要加入站点非必要id2/x1。
- 补读目标确认完整新增Rust所需协议：base `https://ysapi.yingso.fun`，Origin/Referer `https://yingso.fun`；整轮25s、body1MiB、并发解析8、pageSize30/pageNum1/root0/cat="all"。POST `/{version}/search`和getKey都发`{no,info}`：no随机24位`0-9a-z`，info为payload JSON按`no[start:end]`的**UTF-16 code-unit XOR**，不是UTF-8 bytes XOR。root1阿里、2夸克、3迅雷、4百度、5UC，未知root拒绝；完整URL不补prefix、裸shareKey补对应/s/。URL+密码去重并保留原search顺序。
- 新测试：主链路用新端点新key，含夸克、百度pwd1234和未知root跳过；新增仅提供旧端点/旧key的服务器，结果必须是`https://pan.quark.cn/s/legacy-share`。新端点坏配置/空data回退、两端全失败须补测试。
- **Rust：新增 `yingso`**，两组顺序回退、UTF16/24位区间边界和1MiB/25s/8并发均迁；不要仅改一个URL或只支持新站弃旧，亦不扩大为轮询所有path组合/加缓存。加非BMP字符XOR测试防Rust字符模型错位。

### 28 yiove

**文件映射（已审阅）**：`plugin/yiove/yiove.go`。

- 自建client加代理；公共retry替代复制循环并修复非200错误丢失。默认maxRequestRetries3、**200ms/400ms指数**，失败关体、末轮不等。
- `https://bbs.yiove.com/search-%s-1.htm`、搜索/详情各12s、最多12帖子/6链接、6详情worker等目标原限制未在本次改变。
- **Rust：新增 `yiove`**，共享代理/3次状态码可观测retry，沿用原论坛HTML协议和上述限制；不迁Go缓存/专用服务filter开关，按现有Rust KeywordFilterMode表达。

### 29 ypfxw

**文件映射（已审阅）**：`plugin/ypfxw/ypfxw.go`。

- 自建client加代理；公共retry+非200真实status；搜索总**3次（200/400ms）**、详情总**2次（200ms）**，指数基数200ms，不能一刀切都3次。
- 原搜索12s/详情10s、最大并发12，详情`/post/{id}.html`和链接密码解析未改。
- **Rust：新增 `ypfxw`**，明确区分搜索/详情attempts与timeout，共享代理；不搬1小时detailCache/30min清理。

### 30 yuhuage

**文件映射（已审阅）**：`plugin/yuhuage/yuhuage.go`。

- 搜索和详情裸ReadAll都改16MiB；详情retry提成公共helper：**MaxRetryCount2→3次总尝试、1s/2s线性**；网络、非200、读失败重试，最终**返回nil链接不向上报err**保留原容错。合法200但没链接不重试。
- 搜索原3次**200/400ms指数**不改次数/策略，但修复非200最终状态码丢失。ID/hash、HTML、空白三regex静态；其余格式化。
- 原`https://www.iyuhuage.fun/search/{query}-1-time.html`，详情`a.download[href^=magnet:]`与thunder(others)；已有429的60s rateLimited flag不属新功能。特别注意搜索retry仅返回200，原外层`if resp.StatusCode==429`很多时候不可达，不能声称本次修好了429。
- **Rust：新增 `yuhuage`**，两级不同重试和16MiB/静态Regex迁；详情失败允许局部缺链，搜索全失败必须source_error含status。不要引入常驻60s后台清标志/详情cache，仅命令内需要时做rate-limit状态。

### 31 yulinshufa（Rust已有）

**文件映射（已审阅）**：`plugin/yulinshufa/yulinshufa.go`。

- 自建transport代理；公共retry替代同义循环，3次**200/400ms指数**，非200错误数值仍在，失败关体。OnRetry日志第attempt+2次不属于用户输出契约。不含读取/解析变化。
- 目标原常量request20s、detail30s、requestMaxRetries3；GBK请求与解码都原有。
- 当前Rust `src/providers/yulinshufa.rs::search` 列表/详情均直接send一次（client已从ctx取代理正确），**需要新增共享retry覆盖列表和详情**，3次200/400ms、可取消、最终status明确，保持现有GBK和部分详情失败行为；代理已覆盖应写“验证保持”，不造自定义client或改baseURL。测试可先两次503再成功、详情失败保留其他结果及全失败error。

### 32 yunso

**文件映射（已审阅）**：`plugin/yunso/yunso.go`；`plugin/yunso/yunso_test.go`。

- API页body16MiB；解密返回URL统一`normalizeYunsoURL`，原只trim。
- 三条分支均要做同一规范化：明文HTTP(S)、base64解出明文URL、base64+循环XOR旧密文。trim后按URL解析，仅当**ForceQuery=true且RawQuery为空**去掉空`?`，保留fragment，不是简单`TrimRight('?')`。
- 新测试十例完整覆盖：`URL?`、前后空白、`URL?#files`去?；`?pwd=abcd&sort=name`、`?note=what?`、`#files?`、路径`%3F`原样；padding base64、无padding base64、legacy XOR均清空query。`parseItems`不再额外trim URL，消重、分类、密码都用解密后同一规范URL。
- 内嵌HTML三卡两夸克URL（一个尾?且pa=abcd）+百度`pwd=efgh&sort=name`，解析先3项，dedup/convert后2结果；夸克密码abcd保留，百度真实query和efgh保留。
- **Rust：新增 `yunso`**，保留目标原API/HTML卡片/新旧解密三路，规范化必须在URL分类/password/dedup之前；16MiB、所有以上参数/编码/片段fixture迁，不能与`xys`或`yunsou`合并。

### 33 yunsou（Rust已有）

**文件映射（已审阅）**：`plugin/yunsou/yunsou.go`。

- 自建client代理；retry公共化，**3次、200/400ms指数**，状态码失败本来就能保留，本次不是新修此bug；失败关体、末轮不等。
- `https://wpys.cc/s/{keyword}.html`、最多10页/100结果、30s、copyText链接/日期等未变。
- 当前Rust `src/providers/yunsou.rs::search` 用ctx.client因此代理已满足；网络单次send且后页失败break，**加3次retry围住每页send**，保持每页成功后现有解析/页数100条边界及后页失败容错，不把取消变成普通break。wiremock断言前页503×2成功、耗尽status、后页重试用尽保留前页。

### 34 zhizhen

**文件映射（已审阅）**：`plugin/zhizhen/json结构分析.md`；`plugin/zhizhen/zhizhen.go`。

- sourceURLs从`http://www.miqk.cc`,`https://mihdr.top`,`https://www.mihdr.top`收敛为**唯一 `http://www.mihdr.top`**。必须www、必须HTTP；目标说明旧名无解析、HTTPS Cloudflare握手失败，**这些是目标文档报告，不是本次重新联机验证**。
- search **8s→25s**、detail **6s→25s**；不能只把HTTP client设25而context仍8/6。自建client代理、3次**200/400ms**retry最终status修复、context取消后及时返回。
- 文档同步搜索`/index.php/vod/search/wd/{keyword}.html`、详情`/index.php/vod/detail/id/{id}.html`；`.module-search-item`、`#download-list .module-row-one`、`data-clipboard-text`解析**没有改**。文档实测耗时搜索1.9–17.8s、详情6.8–14.2s只说明25s理由，不能视为SLA。
- **Rust：新增 `zhizhen`**，默认明文HTTP新host不强制升级https，搜索/详情各25s上限受CLI总deadline控制，共享代理/正确status/原解析器；配置与文档显式告知HTTP。测试请求scheme+host+path、两个timeout配置、无旧域名回退、HTML原结构解析，不引入缓存。

### 35 zlxapp

**文件映射（已审阅）**：`plugin/zlxapp/zlxapp.go`。

- 公共retry替换，maxRetries2即**3次、线性200/400ms**，WaitFunc用timer+select保持**等待可被req.Context取消且立即返回取消错误**；每轮发请求前先检查ctx。最后不等、失败关体、HTTP200成功。
- **必须以实码为准**：本文件残留注释称“取消后仍走完剩余次数”，但目标`util/retry.go`的WaitFunc错误直接return，故该注释已失真；spec不得据注释声明已知额外重试差异。
- 原base `https://www.zlxapp.top`、20s、自有2MiB、`const listItemsData = `解析没改。
- **Rust：新增 `zlxapp`**，保留目标原页面内JSON协议，3次线性异步等待、drop/取消立即终止且不再发请求，自有2MiB不扩大。测试睡眠期cancel后无下次hits；不加daemon。

### 36 zxzj

**文件映射（已审阅）**：`plugin/zxzj/zxzj.go`。

- 自建client代理；`fetchSinglePanLink`播放器body16MiB；3次**200/400ms指数**非200错误保留status；四regex静态：`/(voddetail|detail)/{id}.html`、`var player_aaaa={...}`、`pwd=4位`、更新时间有秒/纯日期。
- `https://www.zxzjys.com`、30s主client、播放页JS结构/URL/password解码已有，不是新解析协议。
- **Rust：新增 `zxzj`**，保留原搜索→详情→播放页share解析、共享代理、播放器16MiB和3次可观测retry；Regex复用，成功/空/畸形player JSON+超限fixture，不加搜索缓存。

## C. 建议主代理写入 spec 的验收清单

1. **明确来源扩展，不用19来源上限过滤本批。** 本批新增30个：quarkres、quarksoo、quarktv、qupanshe、qupansou、sdso、shandian、sopanya、sousou、thepiratebay、u3c3、wanou、woniu、wuji、xb6v、xdpan、xdyh、xiaoji、xiaoyu、xiaozhang、xinjuc、xys、yingso、yiove、ypfxw、yuhuage、yunso、zhizhen、zlxapp、zxzj。4个已有来源修改/验证：susu、weibo、yulinshufa、yunsou。xiaokupan的Go封装替换和xuexizhinan纯格式化明确记为无CLI净行为变更，不是待迁移来源。
2. 每来源保持成功/空/畸形fixtures；安全读取测试覆盖 limit-1、limit、limit+1、无/虚假Content-Length、分块传输、gzip输出边界；原Go未给所有HTML封顶的漏洞不能照搬。
3. 重试按本文每来源的次数/线性或指数/间隔和status-vs-read错误范围实现，不把30处统一成3×500ms；POST body每轮可重放，取消包含sleep/body读取，末轮不sleep。代理测试覆盖search/detail/auth/临时no-redirect client。
4. woniu本地fixture与账号错误状态、profile0600、站点变更会话失效、工作标题过滤回归；不用线上凭据做默认CI，不复制README预算保证而漏实码缺陷。
5. sopanya SSE进度/坏行/结束/每类8总24/6并发/轮询8×300ms/永久失败/直接URL/裸code/2MiB和256KiB行上限；截止取消，不detach。
6. yingso新新、旧旧、配置坏数据后回退、getKey空data后回退、所有失败、UTF16非BMP和start/end边界；yunso十种规范URL与三卡去重/password fixture；zhizhen明文HTTP唯一host与两种25s配置。
7. 截止交付在现有Rust最终table/JSON和来源摘要上表达，不能给CLI添加Go后台daemon/搜索缓存，也不能把“Provider内部部分结果成功”伪装成来源完整完成而不反映partial状态。准确实现需要SearchContext真实来源deadline/取消传播，不仅局部超时。
