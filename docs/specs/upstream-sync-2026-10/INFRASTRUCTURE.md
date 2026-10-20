# 基础设施、配置与部署净差异

本文件是[上游同步规格](../UPSTREAM_SYNC_2026_10_SPEC.md)的规范附录。

已完成端点 `beaa56133755a548ebc51b090b3816e2ae044aa6` → `221665fb2c9279f22af4684ea2d8c0c331eac1cc` 的分配净diff审阅，含测试、文档和fixture；未运行实现测试或live探测。以下“测试覆盖/测试要求”描述已阅读的上游断言和Rust实施要求，不宣称本地实现完成。

**决策优先级：主规格优先。** 本附录保留代码事实、上游行为和迁移分析；提到上游refresh/TLS开关/后台任务/默认值时不表示授权新增同名CLI能力。具体执行：不新增不安全TLS开关、不新增search refresh旗标、不启用daemon、不自动覆盖用户显式配置；新增来源默认加入策略已确认：新安装/无显式选择的升级跟随上游默认集合与已接入来源交集，默认集合内有状态来源就绪后自动加入，显式选择不覆盖；以主规格为准。详情/转存memo仅单命令实例内，部分结果终态和退出码按主规格。

## 0. 必须先纠正的上游文档事实

- 最新 main.go **75 个生效 blank import**，不是设计文档写的 76；基准为 68。注释中的 erxiao/jupansou/qupanshe 不是注册。
- 最新 Dockerfile 与 compose 均默认启用 **71** 个插件，不是文档的 69；基准均 66。75 中仅 `gying,kpkuang,qiwei,xdpan` 四个不在部署默认启用列表，不是文档的 7。
- 最新 Docker 与 compose 默认频道相同、**111 个且无重复**。基准 Docker 112 个；基准 compose 129 项、128 唯一（ucshare 重复），二者基准原本不同。必须分别计算净增删，不能拿一份代替另一份。
- 代码无 CHANNELS 时仅默认 `tgsearchers7`，不是默认全部 111 个；基准代码 tgsearchers6，README 原文字 tgsearchers3。这三个层次不能合并。
- `api/limits.go` 注释所谓“频道/插件数量 512”实际只限制 `len(req.Channels)`，**没有插件数量校验**；1 MiB 也仅在 POST search 限制，不在 check handler。
- `SHARD_COUNT` 新功能实际仅影响**内存缓存**；磁盘仍自行 CPU×2、[4,32]、向上 2 的幂，不能写成两级均受 SHARD_COUNT 控制。
- `disk_cache.Delete` 新收集 firstErr，但函数末尾仍 `return nil`，不能声称删除失败已向调用者完整透出。
- `ExtDigest` 虽修复键边界和 ext 缺失，却用 fmt.Sprintf("%v", value)，未保留类型（字符串 "5" 与数值 5 可同摘要），不能当作类型安全 canonical JSON。
- 设计文档新增的大量性能结果是作者记录，不是本次核查重新运行证明的基准。

## 1. Provider 注册、默认启用、仅全局能力变化的源

### 1.1 实际注册净变化
新增注册 **10**：`488run,aipan,btbtlb,buerchen,erxiaopan,hjzhencai,kpkuang,pan365,sopanya,woniu`。
停止注册 **3**：`erxiao,jupansou,qupanshe`（代码包仍在，main.go 改为注释）。
默认启用净新增 **9**：`btbtlb,aipan,sopanya,hjzhencai,pan365,buerchen,erxiaopan,488run,woniu`；默认移除 **4**：`gying,qupanshe,erxiao,jupansou`。
其中 gying **仍注册**、只是移出默认；kpkuang **新注册但不默认启用**。erxiaopan 是不同插件标识，不能把 erxiao 的身份/持久化状态直接重命名冒充。

确定 Rust 处置：10 个新增注册源均列为 CLI 迁移范围，分别新增 Provider 实现、注册和确定性 fixtures；不能因当前 Rust 只有 19 源便省略。九个默认新增及 kpkuang 非默认是上游事实，不直接决定本地新安装/升级/登录后的默认加入策略；本地策略已按用户确认固定：采用上游默认集合与已接入来源交集并过滤未就绪来源，新安装和无显式选择的升级一致；不得覆盖用户显式选择。gying保留认证源能力；jupansou保留显式选择实现，不删除用户显式配置或凭据，不能把“停止上游注册”错误写成代码删除或证实站点永远失效。erxiao/qupanshe 不新增为默认源；若其他分组证明它们仍有 CLI 适用协议修改，则保留显式可选实现，绝不能将目录存在等同已启用。

### 1.2 不能遗漏的全局能力继承
`haitunsou,ting77,rrbt,dygang` 在两端注册和默认列表中均存在，且各自插件目录**没有本轮净修改文件**（从两端 diff 文件路径核对），但其基础客户端/框架现在继承：标准与项目代理统一接线、连接池调优、配置初始化后取超时、请求上下文取消、错误保真/缓存回退、ext 隔离。这四个不能被汇总误写成“完全不受影响”，也不能宣称有未经阅读的站点协议更新。若整体迁移新增这些源，使用同一 Rust HTTP/retry/cancel 边界；不能仅以无本地 diff 为由复制旧的独立客户端问题。
本分组中的注册/部署名单变动即上一节的全部 10+3+gying；不存在额外未列出的“仅配置新增源”。其他插件是否局部仅改代理/读取/重试由其实际协议分组负责，本报告提供通用约束供逐源引用。

## 2. 公共 HTTP、安全与响应限额（CLI 必须迁移）

### 2.1 代理统一与连接复用
`config.getProxyURL` 优先级：第一个 trim 后非空的 `PROXY > HTTPS_PROXY > https_proxy > HTTP_PROXY > http_proxy > ALL_PROXY > all_proxy`；`NO_PROXY` 优先于 `no_proxy`，均 trim。显式代理作为 HTTP/HTTPS 全量代理，支持 http/https/socks5/socks5h；缺 scheme、缺 host、ftp 等拒绝。socks5h 归一为 socks5 使用 Go Transport 原生 SOCKS 支持，**不要把其源码注释的 DNS 说法当跨语言契约**；Rust 的 socks5 与 socks5h DNS 语义保持 reqwest 正确行为。
`BuildProxyFunc` 显式代理也尊重 NO_PROXY；未显式代理交还标准 ProxyFromEnvironment。`TuneDefaultTransport` 让 Transport=nil 的插件与共享 HTTP 客户端一致，手写 Transport 必须显式 ProxyFuncForTransport；新增源码守卫锁住此约束。无 config 时连接池回退 per-host20/idle90s；正常 config.Init 后新默认为 **每主机空闲110、保活600s**，总 idle max(100,per-host)，共享客户端原有 MaxConnsPerHost100/TLS handshake10s/TCP dial30s/keepalive30s/client timeout60s 不因本轮改变。默认 Transport 同步调优，而不是每次搜索重建客户端。
Rust 现状 `src/config/mod.rs` 仅 PROXY/HTTPS_PROXY/HTTP_PROXY 加 PANSOU_PROXY，缺小写/ALL_PROXY；`src/http/client.rs` 显式 reqwest::Proxy::all 未接显式 NO_PROXY；`src/http/proxy.rs` Display 已脱敏。确定处置：保留 CLI > PANSOU_* > 兼容环境 > file 的项目优先级，补全上述 fallback 及显式代理 NO_PROXY；统一所有 Provider/checker/session 客户端工厂，复用连接池并使 110/600 可配置。测试必须覆盖仅 PROXY、仅 ALL_PROXY、小写、优先级、NO_PROXY 命中/未命中、非法 scheme、独立 cookie jar。不得通过重建客户端破坏当前隔离 session。

### 2.2 凭据与路径安全
main 的三个代理地址日志改 MaskProxyURL：成功解析去全部 userinfo、保留 host；解析失败/host 空仅占位，不回显原始字符串；空值明确未配置。Rust Display 已覆盖正常地址，仍应覆盖 Debug/错误链（当前 ProxyUrl 派生 Debug 包含内部 URL）、解析失败地址和底层 reqwest 错误，不能只测 Display。
`BuildSearchURL` 的频道改 url.PathEscape，keyword 继续 QueryEscape，阻止 `?/#/空格` 注入。Rust 已有 channel 名称规范化，应保持严格校验，并在 URL 层以 path_segments_mut 构造安全路径，测试合法频道不变、恶意频道拒绝或转义，绝不可拼成另一 host/query。
默认 TLS 校验为 true：新增 INSECURE_SKIP_TLS_VERIFY 仅精确 `true`/`1` 开启（yes、TRUE 、false、0 均不开），配置 nil 安全为 false；供原 qqpd/panyq 硬编码跳过 TLS 的路径改为显式 opt-in。Rust当前rustls默认校验已达到本次安全目标；按主规格不新增不安全TLS风险开关，不使用danger_accept_invalid_certs。测试正常证书通过、过期/自签名被拒绝，各Provider不得私自绕过。

### 2.3 上游体量硬边界与解压
新 ReadAllLimited：缺 reader 报错；limit<=0 回退 **16 MiB**；只读 limit+1 判定，恰好 limit 成功，多 1 字节报可判定 ErrResponseTooLarge，不能先读完整体再判断，也不能静默截断交给 HTML/JSON 解析器。
解压绝对上限 **64 MiB**，已知压缩长度时上限为 `min(64MiB,max(400KiB,compressedBytes*100))`；未知压缩长度<=0 时仅绝对上限。用于流式解析的 NewCappedReader 同样 read limit+1，超限返回错误而非 EOF。自写缓存 DecompressData 只用64MiB绝对上限，明确不套100:1（合法重复JSON会超过100倍）。测试覆盖精确边界、无限流提前停、16MiB零字节gzip bomb、正常高重复小包、未知压缩大小、直接解析器流式读取。
Rust 现 `src/http/encoding.rs::response_text` 裸 response.bytes().await，必须改共享有界分块读；所有 Provider/checker/登录/动态页面/JSON和手写累积路径均受约束，不能只覆盖一个函数然后漏 response.text/json。reqwest 自动解压会隐藏压缩前长度，实施应明确：关闭/控制自动解压的公共读取层分别约束 encoded16MiB 与 decoded64MiB；已知 raw 大小时再套100倍/400KiB规则，未知时不伪造压缩比。不能把 Content-Length 当解压后长度，也不能认为有 Content-Length 预检查即可（chunked无头仍必须分块计数）。TG 使用其更小的 **2MiB** 响应配置边界，细节与 service/TG 分组对齐。
每次 HTTP 尝试结束及时释放 body/连接；特别重试 loop 不能把关闭 defer 到整个函数返回。本轮新增四类静态守卫（裸 Body ReadAll、手写 Read+append、gzip/zlib/flate 无 cap、重试 continue 中 defer Close）及根扫描范围真实性检查，Rust 用 mock/流测试与审计机械约束实现相同不变量，不直接照搬脆弱 Go 文本正则。

## 3. 统一重试能力（CLI 必须迁移，但次数由逐源协议决定）

新 util/retry.go 约定：Attempts=**总尝试数含首次**，<1 规范为1；首次成功无 sleep，后续成功即停，最后一次失败后不等待。每次 callback 收到0起始 attempt。普通耗尽返回最后错误并保留链，尝试>1 添加次数；**Abort(err)** 可 errors.Is(ErrAborted)，立即停、不 sleep、不 OnRetry、不包装成耗尽；Abort(nil)=nil。
等待优先级：DelayFunc(attempt) 完全覆盖其它等待字段（<=0不等）；否则 BaseDelay<=0 回退500ms，Multiplier>1 时指数增长，MaxDelay>0 先封顶，Jitter=true 再加 **[-25%,+25%)**（因此最后抖动值可超过 MaxDelay25%，不要误写最终硬cap）。Multiplier<=1 固定间隔。OnRetry 只在还会重试时调用，收到 wait；WaitFunc 即使 wait=0 也可执行，返回任何错误立即透出（典型 context canceled），不加耗尽包装。测试固定500ms、100→200→cap250、随机75..125ms、线性DelayFunc、零等待、中途Abort、80ms取消阻止5s等待均完整审阅。
确定 Rust 处置：新增 `src/http/retry.rs`（或现有HTTP模块公共API），有 typed RetryDecision/Abort、attempts、backoff/custom delay、取消等待、观测回调，保留原始 ProviderError；迁移所有本轮使用 retry 的新旧 Provider，但不能给所有站点一刀切固定次数/把4xx都重试。每个源具体 4xx/5xx/login/schema 的 retry/abort 判定按其他逐源报告落 spec。固定次数与时延必须有 mock计数+暂停时钟测试；每次响应必须关闭后才等待。不要照搬原实现 `fn=nil` 注释承诺（实际会 panic），Rust 类型系统无此分支。

## 4. 插件基础框架、发布预算与取消（按 CLI 生命周期适配）

- 新 `_ctx` 从 ext 每调用传递，未设置/类型错误 Background；框架到调用方 ctx cancel 后立即返回 partial，**不等原4s窗口**。并发共享插件实例不再保存上下文。
- `_main_cache_key` 同样 per-request，优先有效非空 string，否则实例字段兜底；keyword 从函数参数传到 updater，弃用 shared currentKeyword。测试六个并发关键词不会相互写错槽。
- 在构造插件的 init 阶段 config 尚nil，以前客户端4s/30s、TTL1h、后台worker20被锁死；现在短/长客户端首次使用 sync.Once 创建，TTL 调用时读取；worker首次取槽按 config 创建（<=0回退20）。配置测试构造后配置20s/60s/5h及worker7真实生效，客户端必须同实例复用。
- 插件失败优先回退非空旧缓存，即便 refresh=true；返回nil error并日志原因，避免上层丢弃结果；无缓存必须继续报错，不能失败伪装空。WithResult回退标final。Rust无搜索缓存，不增加伪缓存/后台常驻；原本无缓存的CLI依然如实 source_error，保留已完成其他源结果，不把上游缓存回退列为可直接实现的协议收益。
- 新 PublishBudget = clamp(asyncWindow-800ms, **1800ms..8s**)，默认4s窗口→**3.2s**，从搜索开始算绝对deadline；框架仍长请求后台填缓存。即使窗口配得小于1.8s也取floor，故不能声称总能严格小于用户窗口。此公共函数供 qqpd/weibo/gying 等慢源及时发布，具体源适配另见分组。
- 确定 Rust 处置：当前 `src/search/engine.rs` 以 owned futures、显式 jobs 上限、30s单源/600s全局deadline和中断drop任务，**保留CLI no-detached-task合同**。将 per-call immutable deadline/cancellation、共享配置生效/客户端复用、请求状态隔离迁入；源内部详情并发到deadline返回已收集partial并取消剩余请求，按Rust配置预算计算，**不要机械硬套3.2s后丢余下且声称会后台补缓存**。上游PLUGIN_TIMEOUT30→10属于daemon后台/批截止语义；Rust没有后台补偿，不能无证据把所有单源30s改10s造成永久漏结果。若未来用户选择新增独立 provider budget，10s作为明确可配置选项，而非本轮默认暗改。

## 5. 缓存全部净变化与 Rust 确定处置

Rust当前只有checker缓存，没有搜索结果缓存/daemon。本节完整审阅不是遗漏，但不为兼容上游凭空新增搜索缓存系统；仅把可复用正确性原则纳入现有checker/持久化模块。

1. 缓存键：TG加parser version `v2`（此次支持inline keyboard）；插件主缓存加ExtDigest；单插件缓存从name:keyword变name:keyword:extDigest。ext忽略 `_ctx,refresh,_depth,_main_cache_key`，余项key排序、长度编码、md5，无业务ext返回noext；pages/pages_per_type、cyg per_page/page/order_by/order、miaoso title_en 不得混缓存。频道/插件列表编码从逗号拼接或无边界逐项hash改`length:content,`，排序不改变输入；小列表<5直接编码、大列表>=5 MD5规范编码；旁路hash map条目达到4096整体清空。测试锁住[a,b]与[a,b为单元素]、[ab,c,d,e,f]与[a,bc,d,e,f]不可碰撞、列表顺序无关、ext顺序无关、内部key无关。Rust没有这些搜索键故不创建；现有checker键/代理隔离用结构化typed serialization避免同类歧义，不复制fmt值字符串化风险。
2. DiskCache数据及meta改同目录临时文件→Write→fsync→Close→chmod0644→rename；失败清临时；Set不预删旧文件；meta失败尝试删data并日志；启动扫名称含.tmp残留，避免不计size永久泄漏。测试old/new整体替换、rename失败旧内容保留、missing dir错误、20次Set/10Delete无tmp、200KiB并发只完整值、重开扫stale tmp。注意它是分别原子写data/meta，**不是数据+meta的事务原子性**，也未fsync父目录；Rust已有持久化写法按自身设计保障，不将它误写为完全断电事务。
3. Delete缺文件不报错，其他删除错误尝试收集日志，但firstErr未return的问题见§0。不移植该bug。
4. 新 DiskCache/ShardedDiskCache GetExpiry，磁盘回填内存TTL必须剩余寿命，不重新给全60min；<=0不复活；modified meta缺失用now并日志而非0001年；150ms TTL→回填→250ms后miss测试。此原则可用于现有checker cache，不增加search cache。
5. EnhancedTwoLevelCache异步磁盘写不再完全吞错，**30s**日志限流，期间失败计数累计下一条显示。同步SetBothLevels仍错误可回传。将可观测、限流原则用于Rust现有cache/state，不套Go异步goroutine模型。
6. delayed writer元信息LastFlushTime/Trigger/BatchSize/TotalOperationsWritten改RWMutex；计数逐字段atomic快照，不再整结构复制造成data race；测试双写双读200ms race与roundtrip。自动batch size CPU口径改SchedulableCount，其余大段为gofmt，没有新的合并策略。
7. 内存分片新增SHARD_COUNT trim正数优先，非法回退CPU×2，夹[4,64]向上2幂；17→32、100000→64、1→4、0/-3/abc回退，磁盘未跟该变量而CPU×2夹[4,32]。MemoryCache/serializer/global_buffer_manager/utils仅gofmt（包括global_buffer_manager整大片无业务变化），不虚构新缓存行为。

## 6. 配置调度、API、工具链与文档的 CLI 处置

### 配置新增/变化的准确数值
- PLUGIN_TIMEOUT默认30s→10s，<=0/非法退10；注意base config=nil硬编码fallback仍30，不应说全路径都10。
- TG_CHANNEL_TIMEOUT_SECONDS 默认0（跟随单请求）、非法/<0退0；TG_CHANNEL_REQUEST_TIMEOUT_SECONDS默认4s、非法/<=0退4；TG_RESPONSE_MAX_BYTES默认2*1024*1024、非法/<=0退默认。
- TG_BACKFILL_ENABLED、PLUGIN_BACKFILL_ENABLED、PLUGIN_SJF_ENABLED 默认true，仅 false/0关。PLUGIN_BATCH_TIMEOUT_SECONDS默认0（回退PLUGIN_TIMEOUT）、非法/<0退0。
- OUTBOUND_MAX_CONCURRENCY默认128，正数覆盖，非法/<=0退128。
- CACHE_PARTIAL_TTL_MINUTES仍默认3、正数覆盖，**已弃用不再参与TTL选择**，不能新增Rust短TTL功能说是最终行为。
- UPSTREAM_IDLE_CONN_TIMEOUT_SECONDS默认600s，UPSTREAM_MAX_IDLE_CONNS_PER_HOST默认110，均正数覆盖、非法/<=0退默认。
- PLUGIN_SEARCH_DETAIL_LOG默认false，strconv.ParseBool有效值，否则false。AsyncLog仍默认true。
- 后台workers max(20,SchedulableCount×5)、tasks max(100,workers×5)、HTTP服务器连接max(1000,SchedulableCount×200)；本轮只改CPU来源不改最小值。CPU工具返回GOMAXPROCS(0)>0 else1，测试2/3/1随设值变。

API：POST search已知ContentLength>1MiB返回413，未知长度经MaxBytesReader读取出错返回400；512 channels恰好允许、513返回400且不静默截断；check items256允许、257拒绝400、空items仍先不能为空。auth_handler/filter/middleware除格式无行为更新；router health添加liveness与tg快照。Rust不建API/认证/CORS/gzip response/health接口；CLI继续已有128启用频道硬上限，不因匿名API的512而放宽；批量check可有有界并发/流式输入而非照搬HTTP 256请求体约束。响应限额、安全读取则§2全量迁移。

Worker pool：Task获得batch ctx；发送结果select取消防结果队列满永久阻塞；超时立即返回完成项、未结束回收后台并计BackgroundReclaims；wait=false不关闭results避免发送panic；panic收敛nil继续批次；完整审阅慢2s/预算200ms返回fast、6任务并发2不挂、20轮后台无goroutine泄漏、panic不杀进程测试。Rust现owned futures取消比后台回收更适合CLI，保留；单Provider panic隔离、全局deadline返回已有结果、队列背压不挂与退出无任务遗留必须有回归。

main另启动常驻TG HEAD可达性探测；docs描述2次失败关门、3s预算、可达5min/不可达60s、3s复检、所有HTTP响应包括403/404/405均证明网络可达，受网络阻断skip不写cache/不算频道失效。实际service逻辑由另一个分组审阅，本分组不以docs代替service实现证据。Rust没有daemon，不迁移后台常驻循环；如加CLI短生命周期可达性判定只能每次运行中缓存观测、拿不准fail-open、不因一次timeout永久禁用用户保存频道、不先串行HEAD拖慢首请求。

文档新增四层调度、波次deadline clamp[4s,PLUGIN_TIMEOUT]、adaptive/AIMD、SJF20轮及2轮老化、liveness20轮/至少5/每组40的描述由service分组核对；本分组确定不要误迁成Rust显式jobs仅下界或添加跨进程自适应状态，因CLI的jobs是明确资源上限、no daemon/no search cache。可迁HTTP全局出站信号量128与请求上下文，在Provider内部请求也纳入以防细节扇出绕过jobs。

工具链：Go从go1.24.1+toolchain1.24.9变go1.25.0，Docker builder1.24→1.25-alpine；x/net0.41→0.45、x/text0.26→0.29、x/crypto0.39→0.42、x/sync0.15→0.17、x/sys0.33→0.36，对应go.sum同步。不把Go依赖版本硬移植到Cargo；Rust维持自己的锁文件，公共CPU取容器可调度预算原则可用available_parallelism，不凭宿主核数无界扩大。
新memlimit读 `/sys/fs/cgroup/memory.max` 再v1 memory.limit_in_bytes；已设GOMEMLIMIT不覆盖；无文件/max/非数/<=0不设；<16MiB认为错误、>1TiB无限；合法取9/10 debug.SetMemoryLimit并日志。测试1GiB90%、显式512MiB不覆盖、256MiB第二路径等。Rust无Go GC软堆上限对应物，**不添加假GOMEMLIMIT或改allocator限额**，以有界读取/并发保真实内存预算。
.gitignore从cache改/cache并加/logs；README加woniu入口、频道7、SHARD_COUNT范围、TLS开关；删除过期插件修复TODO本身不能当“现状站点失效”证据。Rust文档按实际迁移能力更新，注明候选频道非实时可用保证。

## 7. 当前 Rust 默认频道/目录的精确差距
`src/channel/mod.rs::ChannelList::default` 和 `src/config/mod.rs::SearchConfig::default` 仍 tgsearchers3；`data/channels.txt` 已含最新名单前110项，只缺最终新增 **Panzi88com**。确定迁移：两处新安装fallback统一tgsearchers7，候选表补Panzi88com、来源标注固定目标hash/核查日期/候选而非可用保证；已有channels.toml包括用户显式tgsearchers3、禁用项、显式空集合不自动覆盖。候选111个仍不意味着默认全启用。tests里110应改111，默认频道相关断言同步。

## 8. 完整性和验证说明
72文件全部净diff含测试已审阅，无待核查项；无线上探测、无运行Go/Rust测试（这是审阅任务，不实施）。站点协议详情以对应Provider分组为权威，本报告不凭注册/文档反推出协议。本报告所有功能迁移有明确迁入/按CLI适配/不引入daemon搜索缓存处置，不将缺现有Provider误判不适用。

## 9. JSON封装迁移结论
sonic原有配置未改，新adapter补RawMessage/Number及Encoder/Decoder使所有业务走统一JSON入口；测试大整数12345678901234567890不丢精度。Rust采用现有serde_json，不迁Go库或全仓禁encoding/json规则；所有新Provider的ID/token不能先转f64，原始JSON片段/大整数以serde typed值或字符串保持。需按站点响应建malformed/重复字段约束测试而非假定两种库完全等价。

## 附录A：两端注册/部署/频道完整集合（全量，保留大小写与顺序）

### 注册Provider
最终75：
```text
488run,5266ys,aipan,btbtlb,buerchen,cldi,clmao,clxiong,cyg,diduan,djgou,duanjuw,duoduo,dy4k,dygang,dyyj,dyyjpro,erxiaopan,feikuai,gaoqing888,gying,haitunsou,hdmoli,hjzhencai,huban,hunhepan,ikantv,jsnoteclub,jutoushe,kkv,kpkuang,labi,leso,libvio,lingjisp,lou1,meitizy,melost,miosou,muou,nsgame,nyaa,ouge,pan365,panlian,pansearch,qiwei,qqpd,quark4k,quarkres,quarksoo,quarktv,rrbt,shandian,sopanya,sousou,susu,thepiratebay,ting77,u3c3,wanou,weibo,woniu,xb6v,xdpan,xiaokupan,xiaoyu,xiaozhang,yingso,yulinshufa,yunso,yunsou,zhizhen,zlxapp,zxzj
```

### 最终默认启用Provider 71（Docker/compose一致）
```text
dyyjpro,duoduo,djgou,feikuai,gaoqing888,hdmoli,haitunsou,hunhepan,ikantv,jutoushe,kkv,dy4k,libvio,lingjisp,lou1,melost,meitizy,miosou,nyaa,ouge,panlian,pansearch,qqpd,quark4k,quarksoo,quarktv,sousou,thepiratebay,ting77,wanou,weibo,xb6v,xiaokupan,xiaozhang,xiaoyu,yingso,yulinshufa,yunso,yunsou,zlxapp,zxzj,rrbt,quarkres,diduan,huban,labi,muou,shandian,zhizhen,clxiong,cyg,jsnoteclub,duanjuw,dyyj,nsgame,cldi,clmao,susu,u3c3,5266ys,dygang,leso,btbtlb,aipan,sopanya,hjzhencai,pan365,buerchen,erxiaopan,488run,woniu
```

### 最终部署频道 111（Docker/compose一致）
```text
tgsearchers7,Aliyun_4K_Movies,yunpanx,yp123pan,yunpanxunlei,tianyifc,peccxinpd,gotopan,PanjClub,baicaoZY,MCPH02,MCPH03,bdwpzhpd,Q66Share,ucwpzy,shareAliyun,Quark_Movies,XiangxiuNBB,ucquark,xx123pan,yingshifenxiang123,zyfb123,Lsp115,taoxgzy,Channel_Shares_115,vip115hot,wp123zy,yunpan139,yunpan189,yunpanuc,yydf_hzl,leoziyuan,yoyokuakeduanju,TG654TG,QukanMovie,yeqingjie_GJG666,movielover8888_film3,Baidu_netdisk,D_wusun,FLMdongtianfudi,KaiPanshare,rjyxfx,PikPak_Share_Channel,newproductsourcing,QuarkFree,yunpanNB,kkdj001,xxzlzn,pxyunpanxunlei,jxwpzy,kuakedongman,xiangnikanj,solidsexydoll,guoman4K,zdqxm,kduanju,cilidianying,CBduanju,SharePanFilms,dzsgx,BooksRealm,douerpan,Netdisk_Movies,yunpanquark,ciliziyuanku,jzmm_123pan,wpan8,mqte5,regengguangya,regeng115,regeng123,yy80986098,pan_guangya,guangyapan_episode,guangya_hdhive,guangyapindao,quark_res,domgmingapk,dianying4k,tgbokee,ucshare,gokuapan,WFYSFX03,gimy100,gimy115iso,fcij5,xvth5,xuexiziliaobaibaoku,phzvip,jdbigdiscount,youxigs,zhoulanziyuan,seedhub_pro,jnjy_5,xxziliao,wpzyk,ruanjianfenxiang77,jpnd5,XunLeiPinDao,a123fxme,WPpindao,kuyupan,djya5,zh_vip,gdsharing,guangyaya2026,alyp_17362,baidyunpan,yunpans,rbzhwpzy,Panzi88com
```

### Dockerfile频道净变化
- 基准112项/112唯一；目标111唯一。
- 新增38：`tgsearchers7,guangya_hdhive,guangyapindao,quark_res,domgmingapk,dianying4k,tgbokee,ucshare,gokuapan,WFYSFX03,gimy100,gimy115iso,fcij5,xvth5,xuexiziliaobaibaoku,phzvip,jdbigdiscount,youxigs,zhoulanziyuan,seedhub_pro,jnjy_5,xxziliao,wpzyk,ruanjianfenxiang77,jpnd5,XunLeiPinDao,a123fxme,WPpindao,kuyupan,djya5,zh_vip,gdsharing,guangyaya2026,alyp_17362,baidyunpan,yunpans,rbzhwpzy,Panzi88com`
- 删除39：`tgsearchers6,bdbdndn11,bsbdbfjfjff,sbsbsnsqq,BaiduCloudDisk,txtyzy,kkxlzy,MCPH01,ysxb48,jdjdn1111,yggpan,MCPH086,zaihuayun,alyp_1,dianyingshare,ydypzyfx,tyypzhpd,tianyirigeng,cloudtianyi,hdhhd21,oneonefivewpfx,qixingzhenren,tyysypzypd,pikpakpan,Q_dongman,WFYSFX02,QQZYDAPP,btzhi,cctv1211,duan_ju,liangxingzhinan,Oscar_4Kmovies,baidu_yppan,Q_jilupian,ammmziyuan,cili8888,yunpanguangya,yoyokuakeduanjujiaoliuqun,AV688`

### docker-compose.yml频道净变化
- 基准129项/128唯一；目标111唯一。
- 新增30：`tgsearchers7,guangya_hdhive,guangyapindao,quark_res,fcij5,xvth5,xuexiziliaobaibaoku,phzvip,jdbigdiscount,youxigs,zhoulanziyuan,seedhub_pro,jnjy_5,xxziliao,wpzyk,ruanjianfenxiang77,jpnd5,XunLeiPinDao,a123fxme,WPpindao,kuyupan,djya5,zh_vip,gdsharing,guangyaya2026,alyp_17362,baidyunpan,yunpans,rbzhwpzy,Panzi88com`
- 删除47：`tgsearchers6,bdbdndn11,bsbdbfjfjff,sbsbsnsqq,BaiduCloudDisk,txtyzy,kkxlzy,MCPH01,ysxb48,jdjdn1111,yggpan,MCPH086,zaihuayun,alyp_1,dianyingshare,ydypzyfx,tyypzhpd,tianyirigeng,cloudtianyi,hdhhd21,oneonefivewpfx,qixingzhenren,tyysypzypd,Q_dongman,WFYSFX02,QQZYDAPP,btzhi,cctv1211,duan_ju,liangxingzhinan,Oscar_4Kmovies,baidu_yppan,Q_jilupian,ammmziyuan,cili8888,Q_dianying,q_dianshiju,godupan,gimy115,peccxin,Movie888035,xlwpzy,zyywpzy,wydwpzy,yunpanguangya,yoyokuakeduanjujiaoliuqun,AV688`

## 附录B：72/72已审阅映射（没有待核查文件）

|文件|实际核查结论及正文位置|
|---|---|
| `.gitignore` | 已审阅。仅根cache忽略与新增logs忽略；§6，不改变CLI协议。 |
| `Dockerfile` | 已审阅。Go builder1.25、111频道/71启用Provider精确集合；§0/1/6及附录。 |
| `README.md` | 已审阅。woniu文档入口、默认频道7、内存SHARD_COUNT范围、TLS显式风险开关；§0/6。 |
| `api/auth_handler.go` | 已审阅。仅对齐格式，无认证语义更新；§6。 |
| `api/check_handler.go` | 已审阅。空items之后增加256上限、257报400；无新增请求体上限；§6。 |
| `api/filter.go` | 已审阅。全diff仅gofmt，无过滤语义变化；§6。 |
| `api/handler.go` | 已审阅。POST1MiB上限、已知长度413/读取错400、channels512；plugins未限制；§0/6。 |
| `api/limits.go` | 已审阅。新增1MiB/512/256及>边界；API专用不放宽Rust128；§6。 |
| `api/limits_test.go` | 已审阅。超body、channels513、items257、恰等上限允许、空items先报错；全部断言已读；§6。 |
| `api/middleware.go` | 已审阅。仅gofmt，无CORS/JWT/日志逻辑变化；§6。 |
| `api/router.go` | 已审阅。health新增liveness与tg快照，其余格式；无Rust daemon接口；§6。 |
| `audit/body_close_guard_test.go` | 已审阅。扫描retry循环continue+defer Body.Close；剔.git/vendor/node_modules；§2.3。 |
| `audit/decompress_guard_test.go` | 已审阅。gzip/zlib/flate需CappedReader或ReadAllDecompressed；植入对照确保守卫抓得到；§2.3。 |
| `audit/handrolled_read_guard_test.go` | 已审阅。for Read+append手写无界累积守卫，原quarktv形态对照；§2.3。 |
| `audit/plugin_audit_test.go` | 已审阅。AUDIT_PLUGIN_SOURCES=1显式live；先refresh、等35s后二查空；网络失败不等于结构坏；§8，不当本次实测。 |
| `audit/readall_guard_test.go` | 已审阅。裸io.ReadAll(x.Body)守卫，忽略测试及注释；§2.3。 |
| `audit/setup_test.go` | 已审阅。TestMain chdir根，main/go.mod/plugin/service/util存在且Go文件>=200防空扫描假通过；§2.3。 |
| `config/config.go` | 已审阅。所有新增/改变getter与注册字段逐段已读，精确数值/代理/TLS/CPU/废弃TTL；§2/6。 |
| `config/default_channels_test.go` | 已审阅。无环境默认7、环境a,b,c拆分；§0/7。 |
| `config/insecure_tls_test.go` | 已审阅。nil安全默认校验，true/1 opt-in，false/0/yes/TRUE空格拒绝开启；§2.2。 |
| `docker-compose.yml` | 已审阅。默认频道基准129项128唯一→111；默认Provider66→71；与Docker目标一致，基准不同；附录。 |
| `docs/插件修复TODO-2026-09-01.md` | 已审阅。整文件删除61行；完整读过所有旧源状态，不能将删除文档当最新协议/失效证据；§6。 |
| `docs/系统开发设计文档.md` | 已审阅。所有新增调度/资源/可达性/liveness/守卫段及被移除调优段已读；75/71/4纠正文档76/69/7；§0/6。 |
| `go.mod` | 已审阅。Go1.25、删除toolchain、五个x库版本升级；§6。 |
| `go.sum` | 已审阅。上述五库对应校验和替换，无独立协议变更；§6。 |
| `main.go` | 已审阅。10新增3停止注册、memlimit、后台TGprobe、代理脱敏与CPU日志；§0/1/2/6。 |
| `plugin/ext_context_key_consistency_test.go` | 已审阅。util/plugin两个_ctx常量一致测试；§4/5。 |
| `plugin/plugin.go` | 已审阅。per-call_ctx/main key/keyword、延迟配置clients/workers/TTL、ext键隔离、失败回退；完整净diff已读；§4/5。 |
| `plugin/plugin_config_lazy_test.go` | 已审阅。构造先于config、20s/60s/5h、worker7、nil默认、单实例复用；§4。 |
| `plugin/plugin_context_test.go` | 已审阅。100ms取消使3s任务<1s返回partial、WithResult非final、无ctx正常/坏类型fallback；§4。 |
| `plugin/plugin_fallback_test.go` | 已审阅。refresh错误有cache返回旧结果nilerror，无cache保留error；§4。 |
| `plugin/plugin_main_cache_key_test.go` | 已审阅。ext优先及坏值回退，六关键词并发主键/keyword隔离；§4。 |
| `plugin/plugin_transport_proxy_test.go` | 已审阅。全部手写&http.Transport需Proxy字段、扫描非空断言；§2.1。 |
| `plugin/publish_budget.go` | 已审阅。clamp(window-800ms,1.8s,8s)，默认3.2s；小window可高于window；§4。 |
| `plugin/publish_budget_test.go` | 已审阅。默认预算小于4s、上下界、floor>=500ms；非所有自定义窗口严格小于；§4。 |
| `util/cache/cache_key.go` | 已审阅。TG parser v2、plugin ext、长度列表编码、4096旁路cache限额；§5.1。 |
| `util/cache/cache_key_collision_test.go` | 已审阅。逗号/元素边界/排序不变/ext差异全部断言；§5.1。 |
| `util/cache/delayed_batch_write_manager.go` | 已审阅。专用meta锁+atomic逐字段快照、CPU取预算；其余gofmt；§5.6。 |
| `util/cache/disk_atomic_write_test.go` | 已审阅。整体替换/失败保旧/无临时/并发完整值/重启扫tmp/30s日志累计，264行全部审阅；§5。 |
| `util/cache/disk_cache.go` | 已审阅。data+meta分别atomic、残留tmp清理、不预删、GetExpiry；Delete未return firstErr；§0/5。 |
| `util/cache/enhanced_ttl_refill_test.go` | 已审阅。150ms写两级→删内存→回填→250ms后必须miss；§5.4。 |
| `util/cache/enhanced_two_level_cache.go` | 已审阅。剩余TTL回填/metadata fallback now/异步失败日志30s限流；§5。 |
| `util/cache/flush_meta_race_test.go` | 已审阅。双写双读200ms-race与7项meta roundtrip；§5.6。 |
| `util/cache/global_buffer_manager.go` | 已审阅。全部净diff为gofmt/空行，未新增global buffer业务；§5.7。 |
| `util/cache/memory_cache.go` | 已审阅。仅gofmt；§5.7。 |
| `util/cache/serializer.go` | 已审阅。仅gofmt；未改GOB类型/协议；§5.7。 |
| `util/cache/shard_count_test.go` | 已审阅。32/17→32、100000→64、1→4、0/-3/abc回退、CPU4→8；§5.7。 |
| `util/cache/sharded_disk_cache.go` | 已审阅。CPU口径变GOMAXPROCS、仍[4,32]、新增GetExpiry；SHARD_COUNT不管磁盘；§0/5.7。 |
| `util/cache/sharded_memory_cache.go` | 已审阅。SHARD_COUNT优先、[4,64]上取2幂，CPU来源变；其余格式；§5.7。 |
| `util/cache/utils.go` | 已审阅。仅gofmt；无序列化行为修改；§5.7。 |
| `util/compression.go` | 已审阅。自写cache解压64MiB cap、不用比率；response gzip中间件仅格式；§2.3。 |
| `util/convert.go` | 已审阅。仅格式，不改变StringToInt无效返回0；§6。 |
| `util/cpu/cpu.go` | 已审阅。SchedulableCount=GOMAXPROCS(0)>0 else1，独立叶子包避导入环；§6。 |
| `util/cpu/cpu_test.go` | 已审阅。GOMAXPROCS2/3/1跟随、至少1；§6。 |
| `util/ext_digest.go` | 已审阅。忽略4内部key、key排序长度编码MD5、noext；%v不保类型；§0/5.1。 |
| `util/ext_digest_test.go` | 已审阅。pages差异/顺序无关/边界无碰撞/忽略ctx-refresh-mainkey/nil空；§5.1。 |
| `util/http_body.go` | 已审阅。16MiB raw、64MiB解压、100:1+400KiB floor、limit+1报错而非EOF；§2.3。 |
| `util/http_body_test.go` | 已审阅。精确边界/无限reader/nil/0limit/bomb/正常/未知压缩/streaming全部212行断言已读；§2.3。 |
| `util/http_util.go` | 已审阅。统一代理/NO_PROXY/默认与手写Transport/池/脱敏/TGpathescape/有界Fetch；§2。 |
| `util/http_util_test.go` | 已审阅。配置SOCKS/NO_PROXY/非法scheme/标准fallback/池/DefaultTransport/customTransport/live opt-in；全部250行已读；§2。 |
| `util/json/adapter.go` | 已审阅。sonic Decoder/Encoder别名+RawMessage/Number标准别名和构造器；Rust serde语义适配；§9。 |
| `util/json/adapter_test.go` | 已审阅。编解码roundtrip、RawMessage对象/字段、12345678901234567890 Number精度/map类型；§9。 |
| `util/json/json.go` | 已审阅。仅末尾newline，无sonic配置变更；§9。 |
| `util/json/no_stdlib_json_test.go` | 已审阅。唯一adapter例外；全仓不能直接encoding/json；递归扫描并排除自身与adapter；§9。 |
| `util/mask_proxy_test.go` | 已审阅。HTTP/SOCKS凭据不露、坏地址不回显、恶意channel转义、合法名不变；§2.2。 |
| `util/memlimit/memlimit.go` | 已审阅。v2/v1配额9/10、GOMEMLIMIT优先、16MiB/1TiB sanity，不移植Go堆控制；§6。 |
| `util/memlimit/memlimit_test.go` | 已审阅。1GiB90%、显式512MiB不覆盖、无限/坏值/8MiB、不存在路径与第二路径256MiB；§6。 |
| `util/pool/object_pool.go` | 已审阅。仅newline无对象重置语义改变；§6。 |
| `util/pool/worker_pool.go` | 已审阅。Task ctx、可取消send、panic隔离、超时及时partial与后台回收；§6。 |
| `util/pool/worker_pool_test.go` | 已审阅。2s/200ms、ctx deadline、全完成、结果队列满、20轮不泄漏、panic；§6。 |
| `util/retry.go` | 已审阅。Attempts含首次/Abort/DelayFunc/±25%/WaitFunc取消/原错误链；§3。 |
| `util/retry_test.go` | 已审阅。341行完整审阅：成功/次数/链/终次无sleep/固定指数cap+jitter/custom/abort/ctx等待；§3。 |
