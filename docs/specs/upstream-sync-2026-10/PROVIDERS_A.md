# Provider 净差异与实施决策 A

本文件是[上游同步规格](../UPSTREAM_SYNC_2026_10_SPEC.md)的规范附录。

已完成端点 `beaa56133755a548ebc51b090b3816e2ae044aa6` → `221665fb2c9279f22af4684ea2d8c0c331eac1cc` 的分配净diff审阅，含测试、文档和fixture；未运行实现测试或live探测。以下“测试覆盖/测试要求”描述已阅读的上游断言和Rust实施要求，不宣称本地实现完成。

**决策优先级：主规格优先。** 本附录保留代码事实、上游行为和迁移分析；提到上游refresh/TLS开关/后台任务/默认值时不表示授权新增同名CLI能力。具体执行：不新增不安全TLS开关、不新增search refresh旗标、不启用daemon、不自动覆盖用户显式配置；新增来源默认加入策略已确认：新安装/无显式选择的升级跟随上游默认集合与已接入来源交集，默认集合内有状态来源就绪后自动加入，显式选择不覆盖；以主规格为准。详情/转存memo仅单命令实例内，部分结果终态和退出码按主规格。

## 迁移总约束（不是泛泛分类）

1. 新增来源注册到 `src/providers/mod.rs` 和来源发现/选择，保留来源名、优先级、过滤模式；每项建 `src/providers/<name>.rs`（488run 可用合法 Rust 模块名如 `run488`，外部名称仍 `488run`）及 `tests/fixtures/providers/<name>/`。现有 9 来源就地改。
2. 响应体读取改为有限读取并显式报超限；上游共用 `util.MaxUpstreamResponseBytes = 16 << 20`，**恰好 16 MiB 合法，16 MiB+1 字节失败**。不能仅检查 Content-Length，也不能 `take(limit)` 后把截断正文当正常页面。错误包含来源/阶段/状态码，但不包含 token、Cookie、代理凭据和原始敏感正文。Rust 的 `response.text()/bytes()/json()` 目前多个路径仍无限读取，HTTP 工厂只统一建客户端，不等于已防护。
3. 代理变更必须落实到搜索、详情、站点 API、登录/会话的每条客户端路径。上游 `ProxyFuncForTransport` 语义是项目代理 `PROXY → HTTPS_PROXY → HTTP_PROXY → ALL_PROXY`，并按 `NO_PROXY` 放行；Rust 应经既有 `HttpClientFactory`/CLI 配置体系实现统一选择，避免新增私有直连客户端。不要照搬 Go 对 socks5h 的归一化或 dy4k 的混用双重代理细节。
4. 重试使用可取消的 async 等待；“总尝试次数”包含首次，最后一次失败后不再等待；失败响应逐轮释放，保存真实 HTTP 状态；POST 重放必须重建请求体。各来源策略如下，不能统改成“三次指数重试”。
5. Rust 当前无搜索缓存、daemon。**不新增 Go 服务层搜索缓存、`ext.refresh` CLI 参数、管理 HTTP 页面、启动保活 goroutine 或命令结束后仍存活的后台搜索/重登。** 新来源的成功详情/转存缓存是 provider-local 的协议去重优化，可保留为单命令实例内存状态（30 分钟/256 或 512 上限），不建磁盘搜索缓存；失败不得缓存。不存在搜索缓存时刷新回退不应伪造。
6. 当前 `SearchEngine` 对来源有外层超时且 drop owned futures 取消请求（`src/search/engine.rs`）。488run/gying 的“提前交付已成功结果”应映射到 CLI 来源有效 deadline 内留回传余量并主动返回累积结果；绝不能生搬 Go 默认 4 秒服务观察窗，让 CLI 30 秒来源预算仍被截成 3.2 秒。上游 PublishBudget 实际是 `clamp(window - 800ms, 1800ms, 8s)`，默认 3.2 秒。CLI deadline 必须在来源开始时统一确定，不在不同阶段重新计时延长。
7. `CloudType` 已有 guangya/magnet/ed2k 等本批需要的全部类型；新增来源不代表新增 checker，维持现有 9 checker，未支持的校验类型沿用当前明确的 unsupported 行为。

## A. 六个上游净新增来源（逐项协议与验收）

### A1. 488run（优先级 2；新增到 Rust）

- 主站 `https://488.run`；README 提到 `b46.cn` 备用，但代码没有自动域名切换，不应在 spec 写成已有 failover。
- 首页 `GET /` 提取 `yd_frontend` Cookie；本实例缓存 **6 小时**，并发获取合并 singleflight；请求 **10 秒**、正文 **4 MiB**。获取 Cookie 失败后仍允许无 Cookie 尝试 API；API Set-Cookie 可更新缓存。非频控 401/403 首轮搜索自动强制刷新 Cookie 并重试一次；**poll 没有这次自动刷新重试**。
- `GET /api/frontend/search?q=<kw>&limit=24&theme=wechat-cinehunt`；Referer `/oeol/<escaped-kw>.htm`。接受 `data.list` 与 `data.items`，search ID 兼容 `search_id` 和 `searchId`。`blocked=true` 为合法空结果。
- incomplete 且有 search ID 时 `GET /api/frontend/search/poll?id=...&limit=24&theme=wechat-cinehunt`；默认 **420ms、最多 6 次**。poll 是增量消费队列，必须累计首批和历次增量，不能只保留最后一轮。poll code **404/1004** 视为完成；除 0/404/1004 外 API code 报错。
- HTTP 429，或 HTTP403 正文含“访问过于频繁”，或 API 错误 message 含“频繁”，进入 **20 秒冷却**；冷却中不再请求。不要访问 `/api/frontend/resource-check`。
- 合并 identity 顺序 `share_link/link/url/magnet/id/resource_id/slug/title/name`；跳过 status=invalid（大小写无关）；相同 identity 合并缺失字段、采用更长 desc/description。结果 ID 再去重、按时间降序、关键词过滤。上游 ID 是清洗后最多 **64 字符**，不能误称为哈希。
- title/name 和 description/desc 兜底；URL/磁力/ed2k 从字段及正文提取；支持 quark、baidu、uc（含 fast.uc.cn）、xunlei、aliyun、tianyi、115（含115cdn/anxia）、123 多别名、mobile、pikpak、guangya。disk_type/drive_type 可作未知 share URL 的类型提示，但本站/图片代理 URL 要拒绝。Rust 实现采用解析 host 验证，不能复制 `strings.Contains(host)` 造成伪域名误判。
- URL query `pwd/password/passcode/code` 取 **4–8 位开头字母数字 token**，正文提取码正则为 **4 位**；截去 `?pwd=623f提取码:623f` 的非 ASCII 粘连，标题去掉 `提取码：26天前` 噪声；WorkTitle=清洗标题。时间 share_time→updated_at→created_at，支持日期/日期时分秒/RFC3339，默认 CST +08；无效时间上游用 now。poster/cover 支持绝对、协议相对、站内绝对路径；标签 source_label/category/year/area 去重并排除“资源/全部”。
- 测试锁住：2 次增量 poll 后共 3 个匹配结果，丢无关标题；最新迅雷排首；百度 URL/密码/标题清洗；封面补站点、WorkTitle 非空、空 channel、488run- 前缀；stale Cookie 403 自动刷新成功。live 测试仅 `RUN_LIVE_488RUN_TEST=1`。
- **确定处置**：新增完整 CLI Provider + 进程内 Cookie 状态/冷却/增量轮询；不做启动后 2 秒 goroutine 预热，首次使用按需获取 Cookie。poll 到 CLI deadline 返回已获结果；无结果时也必须遵守硬 deadline（上游只在有结果时检查 remaining<350ms，README 所称严格预算并非代码对无结果路径的真实保证）。

### A2. aipan / 爱盼（优先级 2；新增到 Rust）

- `https://www.aipan.me/api/movies/search?q=<kw>`，JSON `{movies:[{id,title,rate,cover,year,genres,subtitle}]}`；去空 ID/标题、按 ID 去重，最多 **12 影片**。
- 详情 `GET /api/movies/detail/<path-escaped-id>`，Referer `/movie/<id>`，Origin 主站；每请求 **12 秒**，正文读限 **8 MiB**，详情并发 **6**，保留搜索原顺序。单条详情网络/JSON/无链接失败丢该条，不使整批失败；搜索网络/JSON 失败报错。
- 详情 `panLinks(kind,url,title,sizeLabel,own)` + `resources(kind,url,name,sizeLabel,category)`，先 panLinks 后 resources，最多 **64 链接/电影**。有效 URL 为 HTTP(S)+host 或 magnet:/ed2k:；kind alias alipan/aliyundrive/ali→aliyun、guangya_pan→guangya、189cloud→tianyi、123pan→123、139→mobile；kind 缺失按精确域名/合法子域兜底。
- 去重键 `(type,url,password)`；密码 query `pwd/password/passcode/code` 优先、label 的 `提取码/访问码/密码/取件码/password/pwd/code` 其次（文本 token **3–32 位** `[A-Za-z0-9_-]`）；HTML `&amp;` 解码、首尾引号/括号/标点清理。WorkTitle=详情 title（空则搜索标题）。保留别名、年份、上映、地区、类型、导演、主演、语言、IMDB 1 位小数、剧情、资源名和大小；详情 poster→搜索 cover，标签去重。
- 测试明确 5 类链接 quark/guangya/aliyun+密码/magnet/ed2k；元信息别名、年份、导演、资源大小；按 URL 判 baidu/uc/xunlei/tianyi/guangya/aliyun/magnet。
- **确定处置**：新增完整 JSON Provider，保持 12/6/64 和部分成功。上游仅 LimitReader(8MiB) 会静默截断：Rust 用同额度超限显式错误，不能把这是上游“已显式报超限”写入 spec。

### A3. btbtlb / BT影视（优先级 3；新增到 Rust）

- `https://www.btbtlb.com/search/<path-escaped-keyword>`，搜索 `.module-items .module-item`，标题 `.module-item-title` 或 `/detail/` 链接，ID `/detail/(数字).html`；海报 data-src→src、简介 `.video-text`、标签 caption。最多 **10 影片**。
- 影片详情并发 **4**；从 `.module-row-info a.module-row-text[href]` 分别拿 `/tdown/` 种子资源、`/pdown/` 网盘资源；**每影片最多12种子 + 4网盘，不是12总计**。全搜索按资源 URL 去重后最多 **40资源**，种子优先、网盘随后；资源页解析并发 **8**。每请求 **20 秒**，读取 **8 MiB**（上游同样只是静默 LimitReader，Rust 显式超限）。
- 影片标题 `.video-info-header > h1.page-title`；简介 `.vod_content`；资源标题 `.tinfo .page-title`，去 `.torrent` 或 `.torren` 后缀，缺失用资源条目→影片标题。每资源一个 SearchResult，WorkTitle=影片标题；保留影片海报/标签/剧情，content 添加资源名。
- 资源页从所有 a[href] 和正文识别 magnet 与常见网盘；**仅完全无已解析链接时**，读标签 Hash 并且恰好 **40 hex** 才构造小写 magnet。URL query pwd/password/passcode 或全页提取码文本兜底。更新时间→种子时间→影片 now；解析带 +0800 CST、RFC3339、标准日期/日期时间。
- 关键词由 provider 内过滤，Service/core 不再过滤（种子名常含点号）；迁移不得只声明 Provider filter 而漏实际过滤。失败影片/资源丢该项，搜索失败报错。
- 测试覆盖搜索 ID=123/相对URL/封面/简介、quark全页密码QWER、magnet、40hex Hash fallback、带CST与RFC3339时间。
- **确定处置**：新增完整三阶段 Provider，必须保留 10→(12+4)→40 两级扇出与4/8并发，避免实现成下载 `.torrent` 内容或返回站内资源页面。

### A4. buerchen / 不二资源（优先级 3；新增到 Rust）

- `https://buerchen.top`，同一搜索并行 **3条SSE线路 + 每日更新**。SSE `/api/other/web_search?title=<kw>&is_type=0/2/4`，分别 quark/baidu/xunlei；**不能 is_type=-1**（站点返回暂无可用线路）。Accept `text/event-stream`；Referer `/s/<kw>.html`。
- SSE 实现支持 CRLF、多行 data 合成 JSON、注释心跳、event/id/retry 元数据、裸“线路：”状态行、`[DONE]`；`[DONE] 暂无可用线路`为错误。跳重复 URL、is_invalid!=0、错误 is_type、空 URL、URL > **8192字符**、标题不含所有分词的条目。每线路最多 **6**候选，达到6可提前正常结束（不要求DONE）。每行 scanner最大 **256KiB**，整流 **2MiB+1探测**。
- SSE JSON code非0/200、type=error、坏JSON、HTML验证页、未正常结束均报来源错误；已读候选可以和错误一起保留。Content-Type=application/json 要辨认 code42901，不能当空流成功。
- 每日更新 `GET /api2/content?keyword=<kw>`，必须服务端过滤（全目录>2MiB）；结构 `success/data.data[]` 的 title/quarkLink/baiduLink/xunleiLink。各类型最多 **2每日候选**，每日排该类型组首，然后 web补足到6；类型间轮询交错，总计最多 **18**条；解密/解析实例共享并发 **3**（两个同时搜索也不超过3）。
- 每日 POST `/api2/decrypt` JSON `{"encryptUrl": 原密文}`，取 data.url；再与SSE密文一起 POST `/api/other/save_url` JSON `{"url": encodeURIComponent(original),"title":title}`。**先浏览器encodeURIComponent，再JSON编码，不能 form/query 或双重编码**：空格=%20、+→%2B、/→%2F、=→%3D，而 `!'()*~` 保留。搜索中已有合法直链直接返回，不调用save_url；每日即使解密得到直链也仍走save_url。
- 只接受精确 `pan.quark.cn/pan.baidu.com/pan.xunlei.com` 的 `/s/[A-Za-z0-9_-]+/?`，拒绝 userinfo/端口；必须与0/2/4线路类型一致。密码 query pwd/password **4–8字母数字**→save data.code→data.password/旁文；原分享与生成分享 identity 相同且新密码空时补原密码。
- 稳定 identity：搜索密文用 `provider + token`，每日用解密的**原分享类型+path**，不能用新生成 URL；结果 ID sha256 前16字节（32hex），最终再按identity及URL+密码去重。WorkTitle=标题。
- 预算：总 **28秒**，搜索阶段 **10秒**，请求最多 **12秒**，单响应 **2MiB**；部分请求成功保留，全失败且有error才报错，合法DONE空流+每日空数组是成功空结果。
- 42901 / HTTP403 要求安全验证、HTTP429限流，均实例冷却 **1分钟**；不自动重试验证码，不调用失效标记/反馈API。成功转存缓存 **30分钟、256项**，最早过期项淘汰，失败不缓存。上游 ext.refresh 跳过缓存；CLI无需新搜索刷新接口。
- 测试/fixture锁住：3*6-2=**16结果**（一个503、一个站内广告链接排除）；原始密文 `0+/1==` 不变；每日 decrypt 不重复；合法xunlei直链不转存；refresh变URL但不变ID；调用者修改结果不污染缓存；跨搜索解析峰值<=3；fixture SSE 3资源中1 invalid，故2；daily两行各有quark/baidu所以2/2/0；所有 framing/error/超2MiB/冷却/URL编码案例均需等价Rust离线测试。
- **确定处置**：完整双链路新增，不只接“网页搜索”。保留转存副作用但严格限制18/3；进程内成功转存缓存不等于搜索缓存。无验证码绕过/自动重试。

### A5. erxiaopan / 二小盘（优先级 2；新增到 Rust）

- 基站 `https://www.2xiaopan.one`，不是已有 `erxiao` 的别名。首页搜索 `/index.php/vod/search/wd/<query-escaped-kw>.html`，下一页 `/index.php/vod/search/page/{n}/wd/<kw>.html`。`.module-search-item .video-info-header h3 a` 取干净标题，不能用带“立刻播放”的图片锚点。
- 详情 path `^/(index.php/)?vod/detail/id/(数字).html$`；分页仅 `#page a.page-next[title="下一页"]`，同源且匹配search路径、拒绝userinfo、丢fragment；“尾页”也是page-next不能跟。最多 **2页、20详情、实例共享5详情并发**，总 **25秒**，页面 **4MiB**。
- 每页 **3总尝试**，线性 **300ms、600ms**，等待可取消；HTTP400–499除429直接终止；429/5xx/网络/解析失败可重试；2xx页面超限立刻终止。README把重试归因于TTL1s轮换DNS，但代码没有“强制每次重新建连接/解析”的操作，Rust不保证或宣传强制DNS重解析，只使用可重试请求及标准连接行为。
- 搜索无 `.module-search-item` 且无 `.mac_total` 是结构错误；详情无 `#download-list` 和 h1是结构错误，不能当成功空页。成功详情才缓存 **30min/512**，返回/入缓存深拷贝（Rust owned clone）。失败或无链接不缓存。
- 下载限定 `#download-list .module-row-one`，优先 `a.copy[data-clipboard-text]`，再 `.module-row-shortcuts a[href]`；无row时才扫下载区data-clipboard-text。拒绝 `/list/share`、站内详情、未知域名、空/s/、userinfo。
- 精确域名+path：普通盘 `/s/[A-Za-z0-9_-]+/?`，支持 quark/uc/baidu/ali/xunlei/115/115cdn/anxia/pikpak/guangya/123多别名；天翼 `/t/<alnum>` 或 `/web/share?code=<alnum>`；mobile caiyun.feixin.10086.cn/yun.139.com 非根path；magnet需 **40hex btih**，ed2k需32hex hash。
- ASCII URL 截取必须剥离天翼地址后 `（访问码：9rnf）`；密码 URL pwd/password合法 **4–8位字母数字** 优先，旁文其次。天翼code是分享ID而非提取码。
- 按URL+密码去重；仅存在**多个不同有效下载标签后缀**时 WorkTitle追加集数；整部分享均第1集不能误追加。类别、年份、地区、导演、主演**最多6位**、剧情、别名、来源、海报；地区/类别单一锚文本按 `/ , ， 、` 拆分去重。
- fixture search两卡 ID13756、254；卡片标题/标签/演员/海报；detail3链接 quark/baidu/tianyi，密码空/tqwi/9rnf；全都 WorkTitle=狂飙。测试搜索翻两页得3条，详情缓存重复调用不增加、refresh翻倍；结构错误报错；伪URL与合法链接表；第1/2集分别WorkTitle；分隔标签拆分；live仅显式环境开关。
- **确定处置**：新增完整两页下载区Provider及精确链接验证、集数语义。只保留命令内成功详情缓存，不增加搜索缓存/refresh旗标；保留线性退避及4xx终止。

### A6. hjzhencai / 花卷资源（优先级 3；新增到 Rust）

- `https://www.hjzhencai.top/index.php/vod/search.html?wd=<kw>`，卡 `.module-card-item`，标题/详情 `.module-card-item-title a`，ID数字。最多 **3页、48详情、实例共享3详情并发、25秒总时限、2MiB页面**；与erxiaopan一样只跟同源“下一页”并防分页循环，但**此Provider没有自动重试循环**。
- 详情 `.module-info-heading h1`、`.module-info-introduction-content`、`.module-info-main .module-info-item`；更新字段按 `YYYY-MM-DD[ HH:MM:SS]` **CST+08**解析；海报data-original→data-src→src、跳load.gif。成功详情缓存 **30min/512**，clone防污染，无链接/验证/HTTP失败不缓存。
- 仅 `.down-wrap .down-card` 提取分享。卡内a href优先，**仅完全没有a[href]时**才扫卡文本URL。地址/下载按钮按URL+密码去重，去空问号并query正规化；旁文先去URL再取码避免把分享code误用。精确host/path及拒绝userinfo/端口，支持quark/baidu/uc/ali/xunlei/115/123多别名/guangya，天翼/t/及/web/share?code。密码pwd/password **4–8字母数字**优先，卡片文字其次；天翼code绝不作password。
- WorkTitle：同线路（down-src分组）有多个down-card时保留“第1集/第2集”；单个卡的默认第1集不追加，其他非空且不同标题的标签追加。不能照搬erxiaopan跨全部有效标签的歧义规则。
- 搜索没有`.module-heading-search`或card、详情没有`.module-info-main`或`.down-wrap`均结构错误；部分详情503/无链接丢该项；全部详情失败报错、合法空搜索成功；顺序稳定。
- fixture search IDs6522/6523/6524，detail3盘quark/baidu/tianyi，密码空/4mm3/空，更新 `2026-02-05T00:00:00+08:00`；测试下载区外广告/伪域名不收，空query去重，URL密码胜旁文，天翼无password；翻页循环截断，结果ID稳定，缓存clone，refresh，<=3并发；HTTP403/验证HTML/2MiB+1/全部503/合法空案例。
- **确定处置**：独立新增完整Provider，不能复用二小盘的DOM/集数规则硬套。缓存仅进程内，严格下载区和分享ID/密码区分必须迁移。

## B. 现有 Rust 9 来源的确定更新

| 来源 / Rust路径 | 实际净变化、边界 | 迁移处置与理由 |
|---|---|---|
| cldi / `src/providers/cldi.rs` | searchPage裸读→16MiB显式上限；retry非200保留状态（3总尝试，200/400ms）；添加时间正则与空白正则包级预编译，匹配模式不变 | 有限读取覆盖每页；已有HTTP状态可见性保留，不复制Go空错误；补等价可取消重试和失败响应释放；Rust函数内日期/清洗Regex改LazyLock，保持现代hash卡与旧模板现有解析 |
| clmao / `src/providers/clmao.rs` | searchPage与fetchModernDetail两处→16MiB；【】、[]、空白3个正则预编译；无站点协议/重试变化 | 搜索和现代详情都加有限读取；LazyLock且广告/方括号清洗不变；不宣称本区间新增modern解析或修改重试 |
| clxiong / `src/providers/clxiong.rs` | getSearchResults和fetchDetailPageInfo→16MiB；searchid数字、style url、Content中详情URL、drama/movie数字ID4正则预编译 | 两阶段有限读取，固定Regex单次编译；不改Location/searchid协议、不虚构新域名 |
| cyg / `src/providers/cyg.rs` | 搜索与下载链接响应→16MiB；retry补非200错误（3次200/400ms，但旧代码末尾已有err==nil时补状态，因此是消除重复缺口而不是从完全无状态变有）；空白正则预编译 | 两API路径都受限；现有状态语义保留，等价3次重试；不要宣称API参数或per_page默认本次改变 |
| djgou / `src/providers/djgou.rs` | 自建Transport补统一代理；搜索、BTWAF挑战第二响应→16MiB；挑战goquery解析不再丢err并且仅解析成功才替换doc；retry状态可见（3次200/400ms）；空白正则预编译 | 全部请求用ctx.client；挑战页有限读且解析失败保留原doc/有阶段错误，不能把坏challenge当无challenge；保留既有BTWAF协议；LazyLock |
| duanjuw / `src/providers/duanjuw.rs` | 私有Transport补代理；重试集中util，3总尝试、200/400ms、不在最后等待，失败非200带状态；搜索12s/详情10s为既有配置 | 搜索与详情统一factory客户端，补可重放重试及状态；不新增Go搜索/详情缓存，不把本次当搜索DOM改版 |
| dyyj / `src/providers/dyyj.rs` | 私有Transport补代理；HTML搜索和详情→16MiB；JSON decoder换项目wrapper；文本密码模式**实际修复**原双反斜杠`\\s`为真正`\s`，匹配提取码/密码/pwd+4–8字母数字；URL密码3正则仅预编译 | 当前Rust已JSON API，serde_json不需要Go库替换；沿用API而非加回旧HTML通道；API正文仍需有限读取；现有Rust文本密码已用真正\s，补空格/制表符样例确保等价，并LazyLock。不能将本项错列为纯性能迁移 |
| hdmoli / `src/providers/hdmoli.rs` | 详情页→16MiB；retry状态可见（3次200/400ms）；分类/地区/年份分隔`[，,\s]+`和夸克/百度HTML fallback正则预编译 | 详情有限读、状态错误与重试、固定Regex复用；不改变现有searchList/下载DOM范围 |
| gying / `src/providers/gying/{mod,challenge}.rs` | 详见C：零PoW人工等待、会话按需预热、必需数组对齐、部分结果/截止时间、失败与空区分；同时有服务缓存/UI/后台任务改动 | 迁移CLI适用的求解、对齐、预算、部分成功/错误语义；不迁移服务搜索缓存和daemon；现有Rust顺序详情、3s人工等待、缺type默认mv、?丢已获结果需要实改 |

## C. gying 净差异与 CLI 映射（不能笼统“认证优化”）

1. **PoW提交人工最短3秒→默认0**：新增 `powMinSolveTime time.Duration` 默认零，可回调3s；实际计算及模幂协议没变。Rust `solve_challenge` 当前仍 `solve_inline_pow(..., Duration::from_secs(3))`，确定改为默认零；保留算法及单测可传minimum，避免硬写sleep3s。
2. User新增 last_warmup_at；预热是挑战感知访问详情页，刷新 `vrg_sc/vrg_go`，成功记录时间；TTL **10分钟**。磁盘加载后必须清零，因为旧进程时间戳不能证明新scraper准备完毕。默认账号恢复后触发启动预热；保活从裸首页GET改为同一挑战感知详情预热。Rust映射为**运行时会话状态**，进程恢复时cold，必要时按需预热，登录已触发预热则记warm；不持久化可跨进程信任的warm时间，不建定时保活daemon。
3. SearchWithResult缓存：非refresh命中直接返回；refresh失败或无账号但旧缓存非空回退旧数据；真正成功空结果返回空且不覆盖旧缓存，只有新非空结果写cache。**Rust无搜索缓存，确定不新增该能力**；但要借其意图保留本次请求已获得的有效结果。无账号Rust既有AuthRequired保持，不照搬Go“打印警告然后成功空结果”，因为CLI已有明确认证错误模型。
4. executeSearchTasks从只返回results改为(results,error)，记录成功scraper搜索调用次数：至少一账号抓取成功则合并去重并返回，即便其它账号失败；全部调用失败才返回firstErr。**不是所有详情HTTP失败都能报错**：fetchAllDetails的普通非403详情失败仍可能返回空/nil；spec不要夸大为全面修复。CLI确定增加账号成功计数：一个账号合法空+另一个失败仍合法空，所有账号失败才source_error。当前Rust只看all空和last错误，会把该情形错误化。
5. fetchAllDetails新增deadline：发请求前过期则跳过；最多 **50**详情并发（既有上限），不再无条件wg.Wait；到点锁内快照先回，已启动Go goroutine后台结束；403通过errChan和已获snapshot一起返回。结果非空+403上层先交付，后台重登；无结果+剩余<**4秒reloginBudget**则立即错误+后台修复；足够预算才同步重登一次再搜。CLI确定保留已获结果，详情使用受控并发并在内部deadline收工，取消未完成future，不留detached任务。只有无结果且预算足够、且用户已保存凭据，才在当前命令内重登一次并重试；无凭据AuthRequired，不后台凭空登录。
6. Go预算默认 `clamp(AsyncResponseTimeout-800ms,1.8s,8s)`，且现代码把searchWithScraper开始时间用于fetch deadline，外层retry另起deadline，和按需预热可能共同拉长；预热分支条件“remaining>预算一半”在刚生成deadline时几乎恒成立，不能按注释宣称预热总在后台。Rust确定来源开始一次deadline，所有预热/搜索/重登/详情共用，按CLI实际timeout，不引入Go默认4秒截断。
7. `hasAlignedIndex(index)` 同时检查 index>=0 且 `< len(l.title), len(l.d), len(l.i)`；fetch循环和buildResult均用。缺 `d` 不能默认“mv”。Rust当前.get避免panic但会默认mv发错误详情请求，确定改为跳不齐条目。可选year/info等仍缺省，不能把全部数组强制同长。
8. 日志新增>3秒耗时/403/预热/无账号提示；管理HTML CSS增加overflow避免长串溢出，管理测试搜索可从Cookie重建缺失scraper。这些HTTP管理页面/CSS不迁移到CLI；诊断信息转stderr/tracing且脱敏，不能抄诊断live中打印剥离后的Cookie。
9. 测试必须移植的离线约束：Title2/D1/I2 index0合法、1/2/-1非法；D/I缺失非法；buildResult缺D不panic且返回空；有账号成功空 vs 全失败；部分详情成功后403仍交付；timeout保留部分且取消余项；恢复会话不因持久warm时间跳过必要预热；人工PoW等待0。
10. `gying_refresh_test.go` 三用例分别锁缓存回退、非刷新命中、无cache无账号空，属于Go缓存语义，不是Rust新增缓存要求。`gying_live_test.go` **净差异只有3行注释**，不能声称本区间实际修改了测试skip条件。新增`gying_diag_live_test.go` 386行全读，涵盖登录/响应落盘、重启恢复、缺browser_verified、失效cookie、阶段计时、PoW零等待；均环境开关，不进常规CI网络依赖。

## D. 其余 21 个上游既有变更来源（Rust目前没有，同样确定新增）

以下每项都需完整的来源适配器，之后落实所列净增变化，不能只加名字或标记unsupported。对仅预编译的来源，不凭空宣称站点协议变更；对应现有目标上游实现作为协议移植参照。

| 来源 / 拟Rust路径 | 本次实际净差异及数值 | 确定处置 |
|---|---|---|
| 5266ys / `src/providers/ys5266.rs`（外部5266ys） | magnetHash函数内 `(?i)btih:([a-z0-9]+)` 提为包级，返回小写hash；无协议变化 | 新增来源，hash正则LazyLock一次编译，保留大小写不敏感及原hash字符范围；不能因只有优化就漏来源 |
| ahhhhfs / `src/providers/ahhhhfs.rs` | 自建优化Transport接统一代理；3总尝试200/400ms非200保存真实status，避免%!w(nil)；其它大量为空白格式 | 新增搜索/详情适配，所有请求统一代理及准确错误；不把既有1h详情cache/30min清理误称新增，不移植定时清理goroutine |
| aikanzy / `src/providers/aikanzy.rs` | 统一代理；4个固定HTML/密码正则预编译；util重试 **maxRetries+1=4次**、200/400/800ms、非200状态可见 | 新增 `/search?word=...&molds=article`+详情来源；4总尝试，不能写3；测试2失败第3成功耗时>=600ms、404错误含状态非%!w(nil) |
| alupan / `src/providers/alupan.rs` | 统一代理；util重试搜索**3**次、详情**2**次，200ms*2^n；非200带status；retry测试3次第三成功>=600ms，2次502错误带502 | 新增来源；区分搜索3/详情2、正确重放、逐轮释放，保持12s搜索/10s详情原超时设置 |
| ash / `src/providers/ash.rs` | retry非200错误保status；原策略 **2总尝试、100ms等待**，共享15s请求context取消则停；原有正文2MiB只是LimitReader，不是本次新增 | 新增 allsharehub 来源，2次短退避且可取消；保留错误原因，Rust读取按2MiB显式超限（不将静默截断复制过来） |
| bixin / `src/providers/bixin.rs` | fetchPage裸读→16MiB；循环→util，默认retries=2故**3总尝试**，固定**500ms**，逐轮立即close，网络/读取/非200均重试；最终JSON解析不在重试闭包 | 新增 `www.bixbiy.com/api/discussions`（PageSize50、搜索并行offset0/50为目标原实现），保持固定500ms而不是指数；JSON格式错不要擅自当可重试网络错 |
| daishudj / `src/providers/daishudj.rs` | 私有Transport补统一代理；util重试3总尝试200/400ms并保非200status；搜索10s/详情8s为既有值 | 新增袋鼠短剧完整适配并统一客户端；不引入服务cache cleaner |
| diduan / `src/providers/diduan.rs` | 详情正文→16MiB；`post-(\d+)` 提为包级；没有其它功能变化 | 新增来源，详情有限读取、文章ID Regex复用；不宣称新加base64协议或云盘类型 |
| discourse / `src/providers/discourse.rs` | searchImpl分页及GetTopicDetail→16MiB；HTML标签与空白正则预编译；其余主要格式 | 新增通用论坛来源；分页及topic详情同时有限读取；已有成功分页的部分结果保留，不能只约束首个搜索请求 |
| duoduo / `src/providers/duoduo.rs` | 统一代理；3次200/400ms保非200status；其余为空白 | 新增完整多多来源，搜索/详情统一代理与错误；不虚构新HTML解析 |
| dy4k / `src/providers/dy4k.rs` | createProxyTransport默认加统一代理；搜索HTML及失败debug响应读取→16MiB；原自带HTTP/SOCKS选择仍覆盖/叠加 | 新增完整来源，用统一客户端；**不照抄随机本地代理或SOCKS Dial+Proxy双层隧道**。失败body诊断也有限读/脱敏，不落原文 |
| dyyjpro / `src/providers/dyyjpro.rs` | 统一代理；fetchBody→util重试3次200/400ms，每次独立timeout；读取→16MiB；**读取错误现在进入闭包后会重试，旧实现200读失败直接返回** | 新增 `/ ?cat=&s=` WordPress来源，保持搜索15s/详情10s且per-attempt，不把整个3次共用一次10s；包含读取失败的重试是实际行为增量 |
| erxiao / `src/providers/erxiao.rs` | 仅createOptimizedHTTPClient补ProxyFuncForTransport | 新增原erxiao完整来源；落实搜索/详情代理，不与新二小盘erxiaopan合并；无额外协议变更 |
| feikuai / `src/providers/feikuai.rs` | 私有Transport补代理；API正文→16MiB，读失败触发原searchWeb fallback；四位文本密码和日期Regex预编译 | 新增完整API→网页fallback适配，超限也应走同类可见fallback；固定Regex单次编译，不能在API读取失败时误吞成空 |
| gaoqing888 / `src/providers/gaoqing888.rs` | 统一代理；fetchBody→util 3次200/400ms、每次独立timeout、正文16MiB；和dyyjpro同样新增读取失败重试 | 新增 `https://www.gaoqing888.com/search?kw=`+数字/detail磁力来源；搜索15s/详情10s；测试第3次成功、600ms退避、持续503错误、每次100ms超时总<1.5s锁住 |
| haisou / `src/providers/haisou.rs` | fetchSearchPage及fetchShareLink→16MiB；3次200/400ms保status；highlight span和通用HTML正则预编译 | 新增完整分页→hsid解析分享来源，两处都有限读；不得把hsid当最终网盘链接 |
| hdr4k / `src/providers/hdr4k.rs` | cleanHTML `<[^>]*>` 与 `\s+` 仅提为包级，其余空白格式 | 新增原来源能力，固定清洗Regex复用；无新协议，不能因净差异纯性能而移出迁移范围 |
| huban / `src/providers/huban.rs` | 仅自建Transport补统一代理 | 新增原虎斑来源，搜索/详情使用同一配置代理；无额外重试/解析变化 |
| hunhepan / `src/providers/hunhepan.rs` | searchAPI每页响应裸读→16MiB，保原read error传errChan | 新增完整混合盘来源，每页都受限且错误不能变空结果；未改变原API协议/分页参数 |
| ikantv / `src/providers/ikantv.rs` | doSearch正文→16MiB；retry补status（3次200/400ms）；原末尾已有lastErr==nil fallback，因此不是首次可见status | 新增完整爱看来源，有限读+精确错误，保持既有接口返回结构，勿夸大为API更新 |
| javdb / `src/providers/javdb.rs` | 搜索与详情→16MiB；普通3次200/400ms重试保status；专门429循环→util；清洗空白Regex预编译 | 新增完整磁力来源，必须保独立429语义（见下），不套通用非200重试 |

### javdb 不能丢的限流细则

- `doRequestWithRateLimitRetry` 非429 **包括500** 直接返回response，无err、只1次、不设置rateLimited；由调用方再判断status。网络错误可以走其重试预算，不能把注释“只429”误读成网络错误完全不重试。
- `MaxRetryOnRateLimit=0` 为默认，意味着 **1次总尝试**，遇429立即错误“429限流，不重试”，rateLimitCount+1、rateLimited=1，响应立即释放。
- 若未来显式启用N，则最多N+1次；等待为 **4–8秒（含端点）均匀随机整数秒**，不是指数倍增；最后失败不等待，最终错误标明仍限流。
- 新测试精确锁住非429的500原样返回/只一次/不置限流；默认0下429只一次、计数1、标记1、错误包含429。Rust常规测试使用注入时钟/随机避免真实长睡眠。

## E. 容易误写入 spec 的已确认边界

- aikanzy=4总尝试；alupan搜索3/详情2；ash=2总尝试且100ms；bixin=3且固定500ms；erxiaopan=3且线性300/600ms；javdb429默认仅1次。不能统一说“重试3次”。
- btbtlb是每片12磁力资源+4网盘资源、总40，不是每片12总数；aipan是12电影×每片最多64链接、详情并发6。
- 488run limit24是每次API参数，不是结果总上限；多轮合并可超过24；最多6轮，失效status丢弃，无关标题过滤。
- buerchen的每日数据搜索不是直接全目录下载；每天每类2占该类6配额、总18；密文decode后仍需encodeURIComponent再JSON传save_url；不能把密文直接当URL或删除+ / =字符。
- erxiaopan与hjzhencai的DOM、分页上限、并发、正文大小、WorkTitle集数规则各不相同；erxiao又是第三个独立来源。
- dyyj文本密码正则从字面反斜杠s变为真正空白匹配，确有行为修复。dyyjpro/gaoqing888的body read error从直接失败变为可重试，也不是纯重构。
- cyg/ikantv原来已有底部HTTP状态fallback，新增分支不能夸大成此前HTTP状态完全丢失。
- gying_live_test净变化只有注释；README对488run严格预算、erxiaopan每次强制新连接DNS的描述不能覆盖代码事实。
- 所有新增成功cache都是详情/转存局部cache；不能用它推导Rust需要新增搜索缓存或daemon。所有来源新增与9checker保持独立。

## F. 验收要求与已审阅范围说明

- 本次是完整净diff核查，不是执行性验证；没有运行Go/Rust测试或对外live探测。上述测试约束来自已逐行审阅的断言和fixtures，未宣称运行通过。
- Rust迁移每来源至少success/empty/malformed离线fixtures；新增来源HTTP测试覆盖真实路径/参数/头、上限+1、解析失败与合法空区别、部分失败、取消与请求数；测试fixture不可含真实cookie/token。
- 现有Rust已具正确HTTP错误或密码正则时，迁移以回归锁定与性能/读取防护为主，不为“同步上游”回退现有更安全行为。
- 以下映射逐文件确认审阅，涵盖所有63文件，不保留待审阅或待核查状态。机器可读副本：`/tmp/pansou-review-providers-1-reviewed.json`。

## 63 文件已审阅映射

| # | 文件 | 审阅证据 / 确定处置 |
|---|---|---|
| 1 | `plugin/488run/488run.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A1: Cookie/增量poll/清洗/冷却；新增完整Provider，CLI deadline，不启动预热goroutine；新增Rust来源，详见A1 |
| 2 | `plugin/488run/488run_test.go` | 已审阅；全部231行：多轮poll累计3匹配、排序、百度清洗、封面、WorkTitle、401/403刷新；live环境开关；新增Rust来源，详见A1 |
| 3 | `plugin/488run/README.md` | 已审阅；全部34行：API/Cookie/增量队列/备用域名声明与实际代码区别；新增Rust来源，详见A1 |
| 4 | `plugin/5266ys/5266ys.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: hash Regex预编译；新增来源并LazyLock；新增Rust来源，详见D |
| 5 | `plugin/ahhhhfs/ahhhhfs.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 统一代理/非200状态错误；新增来源；新增Rust来源，详见D |
| 6 | `plugin/aikanzy/aikanzy.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 统一代理/4次200-400-800ms/固定Regex；新增来源；新增Rust来源，详见D |
| 7 | `plugin/aikanzy/aikanzy_retry_test.go` | 已审阅；全部69行：两失败第三成功>=600ms、404真状态、无nil包装；新增Rust来源，详见D |
| 8 | `plugin/aipan/aipan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A2: 搜索12/详情并发6/每片64链接/12s/8MiB；新增来源；新增Rust来源，详见A2 |
| 9 | `plugin/aipan/aipan_test.go` | 已审阅；全部62行：5类链接含guangya/磁力/ed2k、密码、元数据、按URL识别；新增Rust来源，详见A2 |
| 10 | `plugin/alupan/alupan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/搜索3详情2/200ms指数/状态错误；新增来源；新增Rust来源，详见D |
| 11 | `plugin/alupan/alupan_retry_test.go` | 已审阅；全部67行：第三次成功>=600ms、2次502错误真状态；新增Rust来源，详见D |
| 12 | `plugin/ash/ash.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 非200状态/2次100ms；新增来源；新增Rust来源，详见D |
| 13 | `plugin/bixin/bixin.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 16MiB/3次固定500ms/逐轮释放；新增来源；新增Rust来源，详见D |
| 14 | `plugin/btbtlb/btbtlb.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A3: 三阶段10影片/12种子+4盘/总40/并发4-8；新增来源；新增Rust来源，详见A3 |
| 15 | `plugin/btbtlb/btbtlb_test.go` | 已审阅；全部75行：搜索卡/相对图片/全页提取码/magnet/40hex fallback/CST与RFC3339；新增Rust来源，详见A3 |
| 16 | `plugin/buerchen/README.md` | 已审阅；全部35行：双链路/配额/编码/预算/缓存/挑战，按CLI局部状态映射；新增Rust来源，详见A4 |
| 17 | `plugin/buerchen/buerchen.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A4: SSE 0-2-4+每日解密转存/18候选/3并发/编码/冷却；新增来源；新增Rust来源，详见A4 |
| 18 | `plugin/buerchen/buerchen_test.go` | 已审阅；全部252行：18-2结果、daily/direct、编码/ID稳定/cache clone/失败不缓存/共享3并发；live开关；新增Rust来源，详见A4 |
| 19 | `plugin/buerchen/source.go` | 已审阅；全部178行：host+path验证/4-8位密码/12s2MiB请求/1min cooldown/30min256缓存；新增Rust来源，详见A4 |
| 20 | `plugin/buerchen/stream.go` | 已审阅；全部139行：SSE framing/错误识别/6条/8192 URL/256KiB行/2MiB流；新增Rust来源，详见A4 |
| 21 | `plugin/buerchen/stream_test.go` | 已审阅；全部141行：fixture 2与2/2/0；所有CRLF/多行/DONE/错误/超限/冷却/encodeURIComponent；新增Rust来源，详见A4 |
| 22 | `plugin/buerchen/testdata/daily.json` | 已审阅；全部22行：success.data.data两行，quark/baidu有值，xunlei空；新增Rust来源，详见A4 |
| 23 | `plugin/buerchen/testdata/search.sse` | 已审阅；全部14行：裸线路标记、2有效+1invalid、DONE；新增Rust来源，详见A4 |
| 24 | `plugin/cldi/cldi.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 16MiB/状态错误/Regex复用；就地更新；更新现有Rust来源，详见B |
| 25 | `plugin/clmao/clmao.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 搜索+modern详情16MiB/Regex复用；就地更新；更新现有Rust来源，详见B |
| 26 | `plugin/clxiong/clxiong.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 搜索+详情16MiB/4 Regex复用；就地更新；更新现有Rust来源，详见B |
| 27 | `plugin/cyg/cyg.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 两API16MiB/状态错误/Regex复用；就地更新；更新现有Rust来源，详见B |
| 28 | `plugin/daishudj/daishudj.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/3次200-400ms/状态错误；新增来源；新增Rust来源，详见D |
| 29 | `plugin/diduan/diduan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 详情16MiB/文章IDRegex；新增来源；新增Rust来源，详见D |
| 30 | `plugin/discourse/discourse.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 分页+Topic详情16MiB/清洗Regex；新增来源；新增Rust来源，详见D |
| 31 | `plugin/djgou/djgou.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 代理/搜索+挑战16MiB/解析错误/状态/Regex；就地更新；更新现有Rust来源，详见B |
| 32 | `plugin/duanjuw/duanjuw.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 代理/3次200-400ms/状态错误；就地更新；更新现有Rust来源，详见B |
| 33 | `plugin/duoduo/duoduo.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/状态错误；新增来源；新增Rust来源，详见D |
| 34 | `plugin/dy4k/dy4k.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/搜索+debug读取16MiB；新增来源且不抄双代理；新增Rust来源，详见D |
| 35 | `plugin/dyyj/dyyj.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 代理/HTML16MiB/JSON库/真正空白密码正则；就地更新；更新现有Rust来源，详见B |
| 36 | `plugin/dyyjpro/dyyjpro.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/16MiB/per-attempt timeout/读取失败重试；新增来源；新增Rust来源，详见D |
| 37 | `plugin/erxiao/erxiao.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 仅私有Transport统一代理；独立新增，不合并erxiaopan；新增Rust来源，详见D |
| 38 | `plugin/erxiaopan/README.md` | 已审阅；全部41行：两页/下载区/验证码/标签/cache/DNS声明；不承诺代码未强制的新建连接；新增Rust来源，详见A5 |
| 39 | `plugin/erxiaopan/erxiaopan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A5: 2页20详情5并发/25s4MiB/3次线性300-600ms/下载区及集数；新增来源；新增Rust来源，详见A5 |
| 40 | `plugin/erxiaopan/erxiaopan_test.go` | 已审阅；全部323行：捕获页面/3盘密码/翻页cache/结构错误/URL合法性/集数区分/标签拆分/live开关；新增Rust来源，详见A5 |
| 41 | `plugin/erxiaopan/testdata/detail.html` | 已审阅；全部128行：狂飙下载区三盘+内链噪声、inline天翼密码、7演员截6、封面；新增Rust来源，详见A5 |
| 42 | `plugin/erxiaopan/testdata/search.html` | 已审阅；全部104行：两卡13756/254、干净h3、元数据、下一页与尾页同class；新增Rust来源，详见A5 |
| 43 | `plugin/feikuai/feikuai.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/API16MiB及fallback/Regex；新增来源；新增Rust来源，详见D |
| 44 | `plugin/gaoqing888/gaoqing888.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/16MiB/3次200-400ms/per-attempt timeout；新增来源；新增Rust来源，详见D |
| 45 | `plugin/gaoqing888/gaoqing888_retry_test.go` | 已审阅；全部73行：第3次成功/600ms/503失败/3*100ms+600ms<1.5s；新增Rust来源，详见D |
| 46 | `plugin/gying/gying.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B+C: 零PoW等待/会话预热/三数组对齐/预算部分成功/错误区分；不迁移服务cache和daemon；更新现有Rust来源，详见B+C |
| 47 | `plugin/gying/gying_diag_live_test.go` | 已审阅；全部386行：登录/响应诊断落盘/恢复/缺验证cookie/失效会话/各阶段计时/PoW零等待；仅opt-in且Rust不能抄Cookie泄漏日志；更新现有Rust来源，详见B+C |
| 48 | `plugin/gying/gying_index_guard_test.go` | 已审阅；全部66行：title/d/i三数组、负index、缺字段、buildResult不panic；更新现有Rust来源，详见B+C |
| 49 | `plugin/gying/gying_live_test.go` | 已审阅；净差异仅3行注释；不宣称实际修改skip条件；更新现有Rust来源，详见B+C |
| 50 | `plugin/gying/gying_refresh_test.go` | 已审阅；全部64行：refresh旧结果fallback、正常cache命中、无cache无账号成功空；Go服务缓存不引入Rust；更新现有Rust来源，详见B+C |
| 51 | `plugin/haisou/haisou.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 搜索+hsid读取16MiB/状态/Regex；新增来源；新增Rust来源，详见D |
| 52 | `plugin/hdmoli/hdmoli.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 详情16MiB/状态/Regex；就地更新；更新现有Rust来源，详见B |
| 53 | `plugin/hdr4k/hdr4k.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 仅清洗Regex预编译；新增完整来源；新增Rust来源，详见D |
| 54 | `plugin/hjzhencai/README.md` | 已审阅；全部35行：查询/翻页/限额/下载区/WorkTitle/cache/合法分享；新增Rust来源，详见A6 |
| 55 | `plugin/hjzhencai/hjzhencai.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A6: 3页48详情3并发/25s2MiB/下载分组WorkTitle；新增来源；新增Rust来源，详见A6 |
| 56 | `plugin/hjzhencai/hjzhencai_test.go` | 已审阅；全部297行：3卡3盘密码/CST时间/下载区范围/分组WorkTitle/伪URL/分页循环/clone/部分失败/3并发/403超限合法空；新增Rust来源，详见A6 |
| 57 | `plugin/hjzhencai/testdata/detail.html` | 已审阅；全部91行：module-info元数据/2026-02-05更新/三盘每组单第1集/重复按钮/天翼code；新增Rust来源，详见A6 |
| 58 | `plugin/hjzhencai/testdata/search.html` | 已审阅；全部93行：三卡6522-6524/干净标题/海报占位/演员内容；新增Rust来源，详见A6 |
| 59 | `plugin/huban/huban.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 仅私有Transport统一代理；新增完整来源；新增Rust来源，详见D |
| 60 | `plugin/hunhepan/hunhepan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 每页API16MiB；新增完整来源；新增Rust来源，详见D |
| 61 | `plugin/ikantv/ikantv.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 搜索16MiB/状态可见性；新增来源；新增Rust来源，详见D |
| 62 | `plugin/javdb/javdb.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 16MiB/普通与429独立重试/默认429仅一次/Regex；新增来源；新增Rust来源，详见D |
| 63 | `plugin/javdb/javdb_ratelimit_test.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 16MiB/普通与429独立重试/默认429仅一次/Regex；新增来源；新增Rust来源，详见D |
