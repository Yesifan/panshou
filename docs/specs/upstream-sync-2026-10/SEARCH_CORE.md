# 搜索、核心解析与检测净差异

本文件是[上游同步规格](../UPSTREAM_SYNC_2026_10_SPEC.md)的规范附录。

已完成端点 `beaa56133755a548ebc51b090b3816e2ae044aa6` → `221665fb2c9279f22af4684ea2d8c0c331eac1cc` 的分配净diff审阅，含测试、文档和fixture；未运行实现测试或live探测。以下“测试覆盖/测试要求”描述已阅读的上游断言和Rust实施要求，不宣称本地实现完成。

**决策优先级：主规格优先。** 本附录保留代码事实、上游行为和迁移分析；提到上游refresh/TLS开关/后台任务/默认值时不表示授权新增同名CLI能力。具体执行：不新增不安全TLS开关、不新增search refresh旗标、不启用daemon、不自动覆盖用户显式配置；新增来源默认加入策略已确认：新安装/无显式选择的升级跟随上游默认集合与已接入来源交集，默认集合内有状态来源就绪后自动加入，显式选择不覆盖；以主规格为准。详情/转存memo仅单命令实例内，部分结果终态和退出码按主规格。

## 1. 可直接写入 spec 的确定结论

1. **115 checker 必迁移明确的判定修复**：不能再让 list/count/snap/title 抢在失效状态和 forbid_reason 前面判有效；顶层非零 share_state 优先，顶层 0 才回落嵌套状态。有效仅限无有效禁用原因且 state∈{0,1}，并满足 active=1 或有可识别内容。当前 Rust `src/check/one15.rs` 仍有这次修复针对的旧错误，还缺 share_title 证据与顶层零值回落。
2. **9 个 checker 共用 HTTP 安全边界必迁移**：响应上限 16 MiB；gzip/zlib 解压上限 `min(64 MiB, max(400 KiB, 100×compressed_bytes))`，压缩长度未知只使用 64 MiB；超过必须报错而非把截断数据当有效 JSON。适用 Aliyun、Baidu、Quark、UC、Tianyi、123、Xunlei、115、Mobile，不是新增 checker。
3. **TG 提取必迁移**：消息 wrap 同级 inline keyboard、无密码百度链接保留、UC share ID 中合法 `123` 不截断、真正 DOM `<br>` 换行投影、跳过日期/元信息的标题选择、按链接定位取码及明确的解析可信度诊断。
4. **TG 正文关键词命中应保留链接**：不是只匹配标题。当前 Rust `apply_query_filter` 已有等价实现及测试，标记“已覆盖、补回归”，不是再做一次行为修改。
5. **编排迁移适用原则**：失败、零结果、已启动但取消、未启动必须分开；任务取消传播到底层 I/O；请求上下文和关键词不可写到共享 Provider 单例；有界出口与诊断迁入 CLI。Go 的客户端 conc 被提高到任务数、自适应跨请求状态、4 秒异步窗口推导、后台继续搜和主搜索缓存合并，不得机械替换 Rust 的 `--jobs`、`--timeout`、`--all-timeout` 与退出即取消契约。
6. **明确不引入搜索缓存或 daemon**：主缓存 TTL/按键锁/异步 backfill/二级缓存 shadowing、后台 TG 健康探测、`/api/health` 20 轮 liveness 面板属于服务端机制；它们不是遗漏，而是本轮 CLI 非适用项。适用的诊断与 HTTP/解析/调度安全语义照迁，不因为来源当前只有19个就排除任何今后加入来源。
7. **没有新的结果排序权重**：净 diff 中 priority/time/keyword 权重未改；新增 SJF 是任务启动顺序，不是结果排名。不能写“更新排序评分模型”。

## 2. Checker：协议、边界、Rust 处置

### 2.1 115：状态优先修复（迁移）

来源：`service/check_service.go`、`service/check_service_test.go`。

- 既有请求端点仍为 GET `https://115cdn.com/webapi/share/snap`。测试钉住 GET、host、path、`share_code=swff16b3ngy`、`receive_code=CNYY`；不是更换域名/API 的变更。
- 在 `response.state == true && errno == 0` 内，原实现一看 `list非空 / count>0 / snap_id非空 / share_title非空` 就 OK，即使已过期。
- 新逻辑：先取 `data.share_state`；**数值为0**时回落 `data.shareinfo.share_state`，不是只在字段缺失时回落。
- `reason = trim(data.shareinfo.forbid_reason)`。
- 只有 `reason == "" && (share_state == 0 || share_state == 1)`，并且 `share_state==1 || list非空 || count>0 || snap_id非空 || share_title非空` 才 OK。
- 其他情况：reason为空生成 `链接状态异常(share_state=N)`；reason含“密码”或“提取码” → Locked，其余 → Bad。
- 测试覆盖20组：state7 + list/count/snap/title均不能变OK；顶层0回落嵌套7；顶层7覆盖嵌套1；state1有取消原因仍Bad；理由去空白；密码错误即使有文件仍Locked；state1无文件OK；state0有内容OK；state0空对象Bad；API级 password/expired 错误保持原口径。
- Rust 具体修改位置：`src/check/one15.rs::check_normalized`。调整判断顺序，补 share_title，修改 `or_else` 为“顶层0才用嵌套”，补上述响应 fixtures / wiremock 用例。不要改变已有提取码、referer、query协议。

### 2.2 九 checker 共用读取/解压（迁移）

来源：`check_service.go::doRequest/decompressResponseBody`；为确定边界补读了 `util/http_body.go`。

- 裸 `io.ReadAll(resp.Body)` → `ReadAllLimited(...,16<<20)`；通过读 limit+1 区分“刚好16MiB”与超限，恰等上限允许。
- gzip、deflate(zlib) 解压均改 `ReadAllDecompressed(reader,len(raw))`，输出受64MiB绝对上限、100倍压缩比、400KiB小输入豁免共同约束。只约束压缩前16MiB并不充分。
- Rust 当前各 checker `.bytes()` 与共享 `response_text` 都没有同等读取边界。应由共享 HTTP读取 helper 实现，再让9 checker和其解压路径全部使用；不能只改 one15。若 reqwest 自动解压导致拿不到压缩前大小，必须明确读取层的位置，至少保证解压后硬上限，不可假装已经实现压缩比检查。
- 超限应是可观察的检测失败/Uncertain，不得 Bad；不要把超限半页当有效协议响应，不要带token/cookie/代理凭据入错误。
- 该分配范围没有其他8 checker的站点判定协议修改；Aliyun/Quark/Tianyi/Xunlei中本次正则提升到包级只是性能等价（分别 share ID、中文访问码、迅雷 share ID匹配不变）。

### 2.3 检测缓存错误、Mobile JSON（前者已覆盖；后者语言实现等价）

- `loadPersistentCache` 不再 `_ = DB.View` 吞错，解码损坏条目也返回错误并记录 `[CHECK] 读取检查缓存失败`，搜索/check仍继续。Rust `src/check/engine.rs` 已对 `cache.get` Err做 tracing::warn 且继续网络检测，对写缓存失败也告警；保留并加损坏缓存回归，不需复制 BoltDB。
- `check_mobile_crypto.go` 仅把 map/struct payload `encoding/json.Marshal` 改用项目 `util/json.Marshal`。AES key `PVGDwmcvfs1uV3d1`、加密模式、随机数、payload协议均无净变化。Rust仍用serde_json；不增加新加密方案，不声称Mobile API变了。

## 3. Telegram / 核心解析：逐能力结论

### 3.1 wrap级 inline keyboard（迁移修补当前缺口）

- 原仅扫 `.tgme_widget_message_text a`，新扫整个消息 wrap中的 `.tgme_widget_message_text a, .tgme_widget_message_inline_keyboard a[href]`，因为按钮常在 message正文节点之外，与 `.tgme_widget_message` 同级。
- 无href callback、example.com等非盘链接忽略；同正文/按钮重复链接去重；按钮文本也作为取码上下文；只按钮有盘链接的消息必须保留。
- 上游单测要求同一页11条支持链接：两条百度（含一条无密码）、Quark、Xunlei、Tianyi、UC、123、115、Aliyun、Guangya、Mobile；不是新增11个搜索Provider。
- 当前 Rust `src/search/telegram.rs::parse_telegram` 的 `message_node.select(keyboard_anchor)` 只能扫 message内部，**漏掉wrap同级按钮**。改从wrap扫描对应键盘区域，保持本消息边界，避免串到相邻消息。
- Rust已处理标签优先/正文兜底的按钮取码，但仍需与3.4的局部定位统一。

### 3.2 链接归集 / 百度无密码 / UC `123`（迁移）

- 新 `linkCandidates` 把正文a、按钮a、裸链接统一汇入一条处理路径，统一URL规范化、密码合并、去重。
- 六类需要合并密码的网盘：Baidu、Tianyi、UC、123、115、Aliyun；其余direct先输出，六类再按此顺序输出；同类型内部仍Go map无序，**不能承诺为有序结果的新特性**。
- 实质修复：原百度分支只在非空password时存map，导致完全无密码百度分享消失。新 `setPassword` 空码也建条目；后遇非空密码可丰富同baseURL，空码不得清空已有密码。Rust当前无密码保留已有，补回归；shared merge应保持有密码版本。
- 百度 `?pwd=` 从baseURL剥离后最后拼回；其他五类沿既有Clean函数，不是本次重新设计所有网盘URL。
- `CleanUCPanURL` 的结束标记列表移除字面`123`：`https://drive.uc.cn/s/abc123` 不能变`.../abc`。Rust regex当前不会因`123`截断，标记已覆盖，增加这个准确例子。
- 不必复制Go map随机序；Rust维持稳定canonical key及既有输出排序。

### 3.3 DOM正文与标题（迁移）

- 新 `messageTextWithBreaks` 直接递归DOM：TextNode拼文本、`<br>`变`\n`、其他element递归、comment/doctype忽略；HTML实体在DOM阶段已解码，避免再序列化/剥标签/解实体。
- **只有br产生新行，不是每个TextNode产生新行**：`<b>粗体</b>与<i>斜体</i>`应是`粗体与斜体`，`<span>上<br/>下</span>`是`上\n下`。当前 Rust `node.text().join("\n")` 会给内联标签多造行，需更换真正的DOM投影。
- 标题逐行跳过空行、日期头、元信息、以#开头且不含“名称”的行；`名称：`去前缀；一般作品行在最先出现的“简介”/“描述”前裁剪。
- 日期正则接受可选📅加中文`M月D日`、`YYYY年M月D日`、`YYYY-M-D`/`YYYY/M/D`/`YYYY.M.D`（具体实现首段年份是1~4位，不校验日期合法性）；没有新增日期范围判断。
- 元信息精确前缀：`类型：`、`分享：`、`网盘：`、`简介：`、`描述：`、`更多资源：`。
- 契约：`📅 9月9日<br>名称：仙逆 4K 合集`标题为`仙逆 4K 合集`；`#影视<br>遮天 全季` → `遮天 全季`；`兰香如故 &amp; 番外` → `兰香如故 & 番外`；注释不参与标题。
- Rust具体位置 `parse_telegram` 的content/title构造及`associate_work_titles`默认值。保留CLI现有标题长度限制等单独契约，不用日期当work_title导致关键词丢链。

### 3.4 按链接定位提取码（迁移，但修正上游实现陷阱）

上游实际新增算法：

1. `passwordFor` 先 `extractCodeNear(linkURL, primaryContext)`，取不到再调用整段 `ExtractPassword(fallbackContext,linkURL)`。
2. 链接锚候选：去`?/#`和尾`/`后的完整URL；无www的无scheme形式；有www的无scheme形式；最后路径末段长度 **>5 bytes** 时加带`/`的末段候选。
3. 从第一个命中的锚点之后取窗口；遇下一个小写字面`http`截断；最多 **60 bytes（不是60个中文字符）**，退回UTF-8边界。
4. 没找到可用锚/窗口为空时，整个上下文长度≤120 bytes可当短按钮标签，否则返回空进入fallback。
5. NearbyPattern匹配提取/访问/提取密/密码/pwd类标签及4~6位ASCII字母数字；fallback仍原URL码、整条首码等逻辑。
6. 正文a上下文为`buttonText + "\n" + textWithBreaks`；裸链接用textWithBreaks。fallback正文用不含br换行的旧plain text（a额外拼按钮文本）。

测试约束与限制：

- 多链接分别取得`1234/abcd/uc99/p123/p115`，无局部码的Aliyun因fallback仍得到整条首码`1234`，这是上游测试**有意保留的歧义**，不是已彻底消除串码。
- 下一行码可取；窗口本身不跨下一条http；www/scheme规范化仍能定位；长文本无锚不猜；短按钮`提取码：bt01`可取；不切出非法UTF-8。
- 同份LEO真实夹具新旧取码结果一致（多为URL自带pwd），它只证明不回归，不能声称这份夹具证明就近码修复。
- **重要真实代码陷阱**：`passwordFor`的near优先于URL解析，若URL自带码与邻近正文码冲突，near可能覆盖URL码；Rust已有`url_password_precedes_button_label`测试，不能破坏。
- **另一个真实代码陷阱**：Go raw正则中的`[：:\\s]*`不是正确的通用空白分隔写法，且`*`允许零分隔；不要按注释直接宣称支持全部空白。Rust已有合法冒号/空格分隔且拒绝`提取码c3d4`，应保留这些测试。

确定的CLI适配处置：在 `src/core/link.rs` 提供可复用的“显式URL码优先 → 链接局部码 → 消息共享码fallback”能力，供TG正文/按钮统一使用。局部定位迁入60-byte、UTF-8安全、下条链接截断的边界；保留Rust已支持的空白分隔和4~6位校验，不搬正则错误；fallback只在整条没有其他互相冲突的局部码或确有单一共享码时启用，不能为了逐字仿照Go把明确别人的码强填给无密码链接。用单独回归明确这一CLI适配，与Go原fallback歧义区分。

### 3.5 解析可信度（迁移为CLI可观察诊断）

- 新 `PageParseStatus`：OK、NoMessages、StructureChanged；保留旧函数包装；nextPage始终空串，**本次没有实现分页**。
- 解析器按是否识别出消息结构单独计数，不能用输出链接数判定结构失效。Go识别要求data-post可按`/`拆两段、存在RFC3339时间；有效消息无受支持链接可0结果且OK。
- 返回非零results先OK；否则`.tme_no_messages_found`→NoMessages；否则有`.tgme_widget_message_wrap`但recognized=0→StructureChanged；没有消息wrap也没有明确空标记时当前Go仍OK，不能夸大成健壮完整的反爬页检测。
- `searchChannel` 对StructureChanged只告警并返回空结果，不抛失败；不会因此自动重试。
- Rust迁移到 `src/search/telegram.rs` 的解析结果/诊断：明确空页不报错；有消息容器却无法识别消息时输出结构诊断，不与正常无链接混淆。Rust现在datetime为Option，CLI适配保留现有有效链接而不是照抄“缺time丢整条”；若全页预期时间节点缺失则可保留数据并告警。该差异应写明，不误写“缺时间必定无结果”的新要求。

### 3.6 TG关键词过滤（已覆盖）

- `mergeResultsByType`：当不skip过滤且keyword非空，保留`titleMatched || (Channel非空 && Content含keyword)`；不把Provider正文命中普遍作为放行条件。
- 新测试：日期标题`📅 9月10日`，目标关键词只在简介，百度链接仍保留。
- Rust `src/search/engine.rs::apply_query_filter` 已有Telegram正文命中提前保留和Provider正文不放行测试。无需重构评分/排名，补带日期标题案例即可。

### 3.7 真实fixture验收（迁移测试覆盖，区分Go计数与Rust去重契约）

- `tg_channel_shareAliyun_real.html`：真实Telegram频道HTML、消息wrap/data-post、时间、bubble内照片、a标签/mark/emoji、标签；20消息；20盘链接全部Aliyun，其中16 `www.aliyundrive.com` +4 `www.alipan.com`；非零时间；频道参与UniqueID防碰撞；首条`shareAliyun_45740`、标题`骄阳伴我(2023) S01E01-E05 4K`、URL`https://www.aliyundrive.com/s/7H1P3TqeXey`；nextPage空。
- `tg_channel_leoziyuan_with_codes.html`：真实混排页面，既有单资源段也有一条消息几十条`<br>`分隔Quark资源列表、图片/时间/标签；Go固定20结果、502链接、19带密码、每个链接有类型。大数量并不意味着502个来源。
- 确定处置：复制脱敏公开fixture或等价固定夹具到Rust tests，保留以上消息结构/标题/时间/域名/密码归属断言；使用Rust canonical去重后应先锁定语义对应的链接清单，不把Go map随机次序当兼容要求。502/19是上游已审阅计数，不能未经运行声称Rust已满足。

## 4. 服务编排、并发、状态：实际变化与CLI落地

### 4.1 批次结果分账与诊断（CLI适配）

来源 `batch_search.go` +317行测试。

- 每项带id/结果/err/duration，任务完成顺序不再被误认为提交顺序；returned集合与submitted列表计算missing，missing保持原提交顺序。
- 分类：HTTP429限流、403禁止、404不存在、>=500服务端、其他状态码；errors.Is deadline/cancel区分；net.Error单独类；其他错误保留类内首个错误样本。
- 区分 succeeded、failed、timedOut(missing)、yielded、empty；插件产出以**有效链接数**计，不是含0链接结果数量；频道以结果条数计。`complete()`只看failed/missing，全部返回零结果仍complete。
- 输出每个插件的产出，详细逐项日志开关可选；失败汇总top5按计数降序/同数类别字典序；首错误压空白截100bytes；slowest前5、至少200ms；零产出列表最多8、未返回名单最多12。
- Rust已有源id、typed SourceError、completed/failed/cancelled/not_started、取消后drop future；保留且测试，不把失败/取消误作空命中。CLI可增加每源duration、links/yield、错误摘要到stderr或现有events，不用Go fmt.Printf污染JSON stdout，不泄露敏感原始报错。当前接口若不改输出schema，可先用tracing诊断。

### 4.2 父取消、单次TG HTTP及请求隔离（迁移/已有覆盖）

- TG `searchChannelWithContext(parent)`，child timeout默认4s、配置`TG_CHANNEL_REQUEST_TIMEOUT_SECONDS`，父更短即跟随；所有非200都`httpStatusError`，不拿错误页当空搜索结果。
- TG body默认2MiB（配置`TGResponseMaxBytes`），但实际是`LimitReader(maxBytes)`，**超限只截断没有报错**。CLI适配应采用共享limit+1读取helper报告超限，默认TG2MiB边界可保留，不照抄静默半页。
- TG批截止配置`TGChannelTimeout`，未设依次follow单请求timeout、4s；并发仍等于频道数，不受新插件outbound gate控制。
- Provider任务ext复制并注入context及main_cache_key；不再逐请求修改Provider单例`SetMainCacheKey/SetCurrentKeyword`，避免A关键词结果写到B槽。Rust已有显式query参数与owned futures，应保留隔离，新增Provider不能用共享mutable当前query。
- RustHTTP已分429、404/410、其他非success；迁移时保留已有更细分类，明确只接受正确搜索页，不必为了Go只200而放弃合理现有处理。所有新读取/重试均在当前future内，不脱离取消。

### 4.3 新出口闸与并发公式（CLI适配，不搬破坏--jobs的公式）

- 新进程级懒加载semaphore，默认128，配置OutBoundMaxConcurrency>0覆盖；取消后获取槽位应立即失败，结束释放。
- Go有效扇出 `min(max(requestConc,taskCount),outboundCeiling)`；taskCount≤0特例保留正requestConc否则1；limit≤0回128。
- 测试明确conc10/71tasks→71，conc200/71→128，200tasks/limit64→64。这是修复服务客户端conc把71插件分多波的策略，**与Rust CLI --jobs的用户硬上限不同**。
- CLI确定处置：迁入有界出口/等待槽取消/槽释放不泄漏，统一覆盖TG与所有Provider的真实HTTP出网（含内部扇出）；`--jobs`仍是用户上限，不能抬到来源数。未启动任务不算源故障。Rust当前FuturesUnordered已经限制顶层source数，需防未来Provider内部扇出绕过，优先共享HTTP/session边界而非只外层Search一次占槽。
- 注意Go实际闸只包前台`plugin.Search`，不包TG、backfill、异步插件返回后继续的网络请求；不能把它描述成全仓HTTP请求级总限流已经实现。

### 4.4 批截止推导、短作业优先、自适应（界定服务专属，保留CLI明确预算）

上游事实：

- `deriveBatchDeadline`: override>0直接返回（**不受cap/min再次裁剪**）；否则`ceil(max(tasks,1)/max(conc,1))*p90 +1s`，无p90用4s，先cap（若>0）再抬到至少4s。例71/71→5s、71/36→9s、71/10且cap10→10s。因为cap先于min，cap若<4s最后仍4s。
- 每插件耗时窗口20样本，排序key为p50；未知2s；连续两轮missing或gate挡回老化key=-1优先；成功返回清老化；同key稳定注册顺序。聚合p90是合并全部样本后floor(0.9*(n-1))，不是最大单插件p90。普通返回错误不进入成功耗时样本。
- 新adaptive：初始taskCount/4，通常夹8到ceiling（默认128）；每轮任务p90，比最好floor；floor向下立即跟随，向上按差的1/8抬；p90>调整后floor×1.6或有dropped→并发×0.85；否则+12.5%；边界8..ceiling；浮点累积而最终取整；无样本本轮不调整，即使有dropped。进程全局共享，ceiling只能提高；整数上限变化才best-effort写`<CachePath>/adaptive_concurrency.json`，字段limit/floor_ns，损坏不阻断搜索。
- 测试覆盖71/128初始17、稳定延时增长、排队/丢弃后回退、floor上抬、状态跨重启保留/损坏降级；SJF老化恢复与20样本有界。

确定处置：

- **不引入Go的跨HTTP请求学习器、跨重启capacity状态文件或4秒异步发布窗口**；当前Rust一次命令每source只跑一次，没有Go请求内4秒响应/后台继续的同构窗口，因此历史p50排序和跨轮AIMD不能产生相同效益，机械持久化还会新增未授权daemon式状态与网络环境串扰。
- 适用目标通过CLI现有硬预算/上限完成：每源`--timeout`、整命令`--all-timeout`仍显式优先；`--jobs`不被adaptive抬高；任务提交保持请求顺序、截止后立即取消，不基于一轮零结果剔除慢源；新增/扩展来源一样遵守，不能只因现在19个就忽略资源边界。
- 若spec需要叙述SJF/AIMD，写为“上游服务专属不迁入当前一次性CLI；迁移其防饥饿、不静默丢源及有界可取消出口目标”，不要误写成当前Rust已具备20轮学习。结果排名保持 `src/core/rank.rs` 既有语义。

### 4.5 TG后台可达性门（daemon机制不适用；CLI保留真实请求失败）

- 新后台HEAD探测真实首个默认频道`https://t.me/s/<first>`，无频道则首页；UA `Mozilla/5.0 (compatible; PanSou-TGProbe)`；通过已配置代理的共享client；budget3s。
- **任意HTTP响应**包括403/404/405都网络可达；只DNS/连接/TLS/超时等transport error算探测失败；构造请求失败则fail-open。
- 默认可达；连续2次探测失败才关TG，第一次保留旧值；每3s检查，确认首次失败期间尽快复查；可达TTL5min，不可达60s；一次成功清连击、恢复TG；日志只首次结论/翻转等关键时刻。
- searchTG先查现有缓存，然后只读门状态，关闭时不发频道请求、不写网络失败空缓存、不计频道liveness失败。搜索路径不等待探测，不同步HEAD。
- 实际`TGReachable()`只读atomic，结论过期也不会当场重置true；旧false会保持到下一次后台探测成功。注释的“过期fail-open”不能被误读成已实现重置。
- CLI确定处置：不新增后台timer/全局门或持久“TG不可用”状态，不为了省4s而给每次CLI搜索先加3s HEAD；继续真实频道请求，由网络超时/错误报告，取消传播。若将来单命令要合并重复连接失败，只能记本命令transport诊断，不能因某频道403/404跳过其他频道。

### 4.6 Liveness状态/health（服务跨轮统计不适用，单轮诊断适配）

- 进程内按plugin/channel记录最近20轮，每项yield / zero / fail；至少5轮才分类：没yield且有任何fail=failing；没yield无fail=zero_yield；有yield但failed*2>rounds=degraded；否则ok；不足5=insufficient_data。
- 恢复产出离开failing，先可能degraded，20轮旧失败自然被冲刷；zero_yield绝不是失效依据，不自动删除来源。最近错误文本先经过100byte trim；常量120不是一定截120。
- health每分类最多40，按失败数降序同数名称升序；新增完整总数与截断说明，不能拿列表长度冒充总数。渠道报告虽计degraded状态数量但没有独立degraded_channels数组。
- 测试冻结分类、zero不算fail、恢复冲刷、已生成报告不被后续条目污染、50失败只列40但count50/hidden10；bench模拟71插件+110频道，无新产品协议。
- CLI确定处置：不建`/api/health`、20轮跨命令liveness数据库或按历史状态自动禁用Provider。迁移单次诊断的yield/zero/error区分，输出耗时与失败；0结果不是坏源，网络门跳过不是实际失败。适用新旧全部Provider/TG频道。

## 5. 主搜索缓存及backfill：审完但不引入CLI

### 5.1 新批次写缓存/补齐策略（不适用当前无搜索缓存CLI）

- `cacheTTL(full,partial)`：succeeded==0不写；插件启requireYield且yielded==0不写；其余一律fullTTL，**partialTTL参数已不参与结果**。
- TG成功但0结果可缓存；插件全0不缓存；部分失败/部分missing但有成功/可用产出也写full。测试full60min、partial3min明确“有missing仍60min”。search_service若注释仍写“超时写短TTL”，是过时注释，不能据此写spec。
- `shouldBackfill`需enabled、total>0、succeeded>0、missing>0，且`missing*3<=total`（恰1/3允许、超过不补）；只补missing，不重试已返回错误。
- TG补齐独立budget单频道request timeout默认4s、并发missing数量；plugin补齐budget配置PluginTimeout或10s，actual并发至少missing插件数；重用每任务context/key，合并有链接结果。不是给HTTP429自动加retry。
- 插件缓存key新增`ExtDigest(ext)`区分影响搜索的扩展参数；只是缓存命名空间，不是新CLI参数协议。
- CLI不引入refresh搜索缓存、partial/full TTL、后台等待写cache、命令退出后backfill。当前final结果应包含在全局截止前真实完成的全部批次；已有stream result_update非空密码不得丢，继续保持。

### 5.2 缓存竞态修复与探针（不适用存储机制，数据合并原则保留）

- 新64条FNV32a条带锁按缓存key串行化整段Get→Deserialize→mergeSearchResults→Serialize→Set；不同条带key可并行，同条带碰撞可以互阻，并非严格每key独立mutex。
- `mergeIntoMainCache`合并existing与incoming，先SetMemoryOnly；有写入manager则原始合并结果入队，最终priority1/普通2；没有manager时final双写，非final仅内存。
- `writeFinalMainCache`不再把当前请求小快照覆盖后台累积；TG/插件最终路径共用；两种backfill也走这个入口；同样按key互斥先合并后双写。
- 日志新增key前8字符、字节数/TTL、当前条数→合并后写入条数；manager回调只有已序列化快照所以仍直接SetBothLevels，文件守卫允许这些入口。
- 测试：16并发来源都保留；去锁对照确实丢；已有8+本轮1必须至少9，对照直接写只1；TG/plugin两个key已有5+本轮1必须6；补齐探针并发50被老快照8+补齐5覆盖只13（丢37），改合并保留；文件级guard阻新直接写。
- `probe_levels_test.go` **不是修复**：刻意冻结现状“盘54/内存50读50；后双写54才读54”；不能写已解决两级缓存较小版本遮蔽。
- Rust无主搜索cache，故不引入64锁/Bolt/二级cache/manager；`merge_search_results`和stream更新应继续唯一collector内合并，保证早完成来源、已知密码不会被后来的更小快照覆盖。无需为了这些测试新增后台缓存系统。

## 6. 模型/格式与明确非变化

- `model/plugin_result.go`、`model/request.go`、`model/response.go`：只有gofmt对齐/末尾换行，字段名、JSON名、校验与响应形状无净变化。不添加新API字段或Rust CLI flag。
- `cache_integration.go`多数空白整理，唯一行为是可选AsyncLog下落盘key前缀/字节数/TTL日志；没有独立新缓存算法。
- 本组没有新增站点Provider模块、没有删除来源、没有新增checker、没有搜索结果排序权重变更、没有TG分页实现。

## 7. 37文件逐项已审阅映射（无待核查）

| # | 分配文件 | 已审阅内容 / 结论位置 |
|---|---|---|
| 1 | model/plugin_result.go | 完整diff；仅格式/换行；§6，不迁行为 |
| 2 | model/request.go | 完整diff；无字段/JSON变化；§6 |
| 3 | model/response.go | 完整diff；无响应契约变化；§6 |
| 4 | service/adaptive_concurrency.go | 全280新增行；AIMD、floor、落盘、共享ceiling；§4.4 |
| 5 | service/adaptive_concurrency_test.go | 全130行；初始17、增长/回退/恢复/重启损坏；§4.4 |
| 6 | service/backfill_loss_probe_test.go | 全71行；50→13旧覆盖、合并保留对照；§5.2 |
| 7 | service/batch_search.go | 全389行；完整度、错误、yield、最终fullTTL、1/3补齐；§4.1/5.1 |
| 8 | service/batch_search_test.go | 全317行；TTL/补齐/提交序/失败类/全0产出；§4.1/5.1 |
| 9 | service/cache_integration.go | 完整diff；可选落盘日志/格式；§5.2/6 |
| 10 | service/check_mobile_crypto.go | 完整diff；只JSON适配器无密码学变化；§2.3 |
| 11 | service/check_service.go | 完整diff；115顺序、16MiB、解压、缓存错误、预编译；§2 |
| 12 | service/check_service_test.go | 全177行20案例；GET/host/path/share/receive和状态；§2.1 |
| 13 | service/liveness.go | 全288行；20轮/5轮/分类40条/完整count；§4.6 |
| 14 | service/liveness_bench_test.go | 全69行；71plugin+110channel性能基准；§4.6 |
| 15 | service/liveness_test.go | 全172行；恢复/zero/快照/50截40；§4.6 |
| 16 | service/main_cache_lock.go | 全33行；64条FNV32a锁；§5.2 |
| 17 | service/main_cache_lock_test.go | 全104行；同key互斥、异key、释放、按内容哈希；§5.2 |
| 18 | service/main_cache_merge_concurrent_test.go | 全267行；16源不丢/8+1/5+1/无锁对照；§5.2 |
| 19 | service/main_cache_write_guard_test.go | 全103行；直接Set入口文件守卫及注入对照；§5.2 |
| 20 | service/outbound_gate.go | 全153行；懒加载128闸/conc公式/波次deadline；§4.3/4.4 |
| 21 | service/plugin_timing.go | 全204行；20样本/p50/2次老化/合并p90；§4.4 |
| 22 | service/probe_levels_test.go | 全76行；仅冻结内存50遮盘54，不是修复；§5.2 |
| 23 | service/scheduling_test.go | 全124行；conc/deadline/SJF/有界窗口/取消槽；§4.3/4.4 |
| 24 | service/search_service.go | 全部591+/153-净diff；统一合并写、TG超时状态读限、正文过滤、backfill、任务ext、调度；§3/4/5 |
| 25 | service/search_service_test.go | 全27行；TG简介关键词保留百度链接；§3.6 |
| 26 | service/tg_reachability.go | 全240行；HEAD3s/2次失败/5min与60s/3s后台；§4.5 |
| 27 | service/tg_reachability_test.go | 全257行；404/405可达、HEAD真实路径、transport、连击恢复；§4.5 |
| 28 | util/parser_contract_test.go | 全235行；DOM文本/标题/4种URL密码/三状态/无链接OK；§3.3/3.4/3.5 |
| 29 | util/parser_real_codes_fixture_test.go | 全53行；LEO20/502/19、只无回归非修复证明；§3.7 |
| 30 | util/parser_real_fixture_test.go | 全105行；Aliyun20、域名16/4、首条、channel ID、无分页；§3.7 |
| 31 | util/parser_types_probe_test.go | 全86行；6类型顺序、各局部码、Aliyun首码fallback、按钮；§3.2/3.4 |
| 32 | util/parser_util.go | 完整459+/385-净diff；解析拆分、状态、就近码、wrap按钮、归集、DOM/标题；§3 |
| 33 | util/parser_util_test.go | 全126行；11支持按钮链接/无密码百度/日期头work_title；§3.1/3.2/3.3 |
| 34 | util/password_near_test.go | 全118行；旧首码对照、各码、换行/截下链/锚/长上下文/fallback/UTF8；§3.4 |
| 35 | util/regex_util.go | 完整28+/9-净diff；near4~6、预编译4URL正则、UC123；§3.2/3.4/2.2 |
| 36 | util/testdata/tg_channel_leoziyuan_with_codes.html | 新增真实大fixture按许可聚焦结构：head、wrap/data-post、正文/多链接br列表、图片/时间；完整对应断言已读；§3.7 |
| 37 | util/testdata/tg_channel_shareAliyun_real.html | 新增真实大fixture按许可聚焦结构：频道元数据、wrap、正文阿里链接/实体、bubble照片/time/tag；完整对应断言已读；§3.7 |
