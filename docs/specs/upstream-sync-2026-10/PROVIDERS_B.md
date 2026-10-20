# Provider 净差异与实施决策 B

本文件是[上游同步规格](../UPSTREAM_SYNC_2026_10_SPEC.md)的规范附录。

已完成端点 `beaa56133755a548ebc51b090b3816e2ae044aa6` → `221665fb2c9279f22af4684ea2d8c0c331eac1cc` 的分配净diff审阅，含测试、文档和fixture；未运行实现测试或live探测。以下“测试覆盖/测试要求”描述已阅读的上游断言和Rust实施要求，不宣称本地实现完成。

**决策优先级：主规格优先。** 本附录保留代码事实、上游行为和迁移分析；提到上游refresh/TLS开关/后台任务/默认值时不表示授权新增同名CLI能力。具体执行：不新增不安全TLS开关、不新增search refresh旗标、不启用daemon、不自动覆盖用户显式配置；新增来源默认加入策略已确认：新安装/无显式选择的升级跟随上游默认集合与已接入来源交集，默认集合内有状态来源就绪后自动加入，显式选择不覆盖；以主规格为准。详情/转存memo仅单命令实例内，部分结果终态和退出码按主规格。

## 1. 可直接写入 spec 的横向契约（以下逐来源明确落点）

### HTTP 限额 H

`util.ReadAllLimited` 的真实上限为 **16 MiB = 16,777,216 B**，通过读取 limit+1 判超限；等于上限合法，超过报错且不返回半截成功；真实读取失败不能吞掉。`util.NewCappedReader(..., MaxDecompressedBytes)` 上限为 **64 MiB = 67,108,864 B**，超限是错误而不是 EOF。本组 leijing/libvio 的 gzip 新调用只限制解压绝对大小，不是在这两个调用处新增压缩比约束；共享工具另有 100 倍/400 KiB 下限，但不能误记为这两个文件的直接净变化。

Rust 现有 `.text()` / `.json()` 路径无统一上限，应在 `src/http` 增加有界读取（含自动解压后的流），并在逐来源调用落地。不能只看 Content-Length；必须覆盖无 Content-Length/chunked、恰好限额、限额+1、半途读失败。后述 pan365=2 MiB、QQPD API=4 MiB、kpkuang=8 MiB、leso 搜索=6 MiB/详情=8 MiB 优先于普通 16 MiB。

### 代理 P / TLS T

新增 `ProxyFuncForTransport()` 的真实目标是防止自建 Transport 的零值 Proxy 永远直连。上游优先级为配置/PROXY → HTTPS_PROXY → HTTP_PROXY → ALL_PROXY，并遵守 NO_PROXY；支持 http/https/socks5/socks5h（Go 工具把 socks5h 归一成 socks5，不应把其注释当 Rust DNS 语义的规范）。Rust 所有搜索、详情、短链、登录/QR/续期客户端统一走 `HttpClientFactory` 及现有 CLI 配置的代理选择和 NO_PROXY；不能 Provider 内再 `Client::new()` 绕过。保留 Rust socks5/socks5h 的正确解析语义，日志隐去代理 userinfo。

本组 panyq/QQPD 的硬编码跳过 TLS 校验改成 `config.AllowInsecureTLS()`，即默认校验证书，只有明确的 `INSECURE_SKIP_TLS_VERIFY` 配置允许绕过。Rust保持默认校验，按主规格不引入不安全TLS兼容开关，不允许来源内硬编码危险模式。

### 重试 R

工具 `DoWithRetry.Attempts` 是**含首次**的总次数，不是额外 retries。以下多数来源成功条件仍是严格 HTTP 200（不是所有 2xx）；失败响应及时释放，成功后立即停止，最后一次失败后不再 sleep。默认本组不启用 jitter，不能把固定间隔/指数/线性曲线混为一种。CLI 取消/总截止必须打断等待；错误包含来源、真实 HTTP 状态/业务码和最后原因，不能把 nil 错误格式化为成功或空结果。

JSON 解码器从标准 encoding/json 换 util/json 的文件只是归一到 sonic（UseNumber=true/EscapeHTML=true），**NewDecoder 本身没有增加 body 上限**。Rust 保留 serde_json，不引入 Go/sonic 或声称这是新 API schema；数字 ID 保持字符串/无损整数。

## 2. 三个主要业务迁移 + 一个新增 API 来源

### 2.1 kpkuang（上游新增；Rust 新增 `src/providers/kpkuang.rs`）

**搜索协议**：主站 `https://www.kpkuang.org`，镜像 `.fun`、`.sbs`。先 GET `https://kpdata.flixfiend.top/esearch/index?kw=<keyword>&ts=<Unix毫秒>&callback=__kpkuang_cb`，Referer 主站 `/`，Origin 主站；解析 JSONP 首个 `{` 至最后 `}`，要求 `code=1` 且 `js` 非空，标准或无 padding Base64 解出 betaItem 数组。ID 接受字符串/JSON number；同 ID 去重；标题 vod_name → vod_name_org；封面 douban → imdb_poster → vod_pic；演员/导演/编剧/地区/年份组成摘要，图片补全相对 URL。

beta 没有项目（包括失败）则轮询三站 GET `/vodsearch/-------------.html?wd=<keyword>&limit=500`。选 `a.fed-list-pics[href*='/voddetail/']`，没有再兜底所有 voddetail anchor；ID 正则 `/voddetail/(\d+)(?:/|\.html)?`，标题先 title attribute 后 text。

**详情/限制**：最多 10 部；并发 4；搜索/一般请求 15s、每个详情请求 6s、单响应 8 MiB、每片最多 24 条。`orderedHosts` 的真实优先顺序不是主站优先：主站搜索项通常先 `.fun` → `.sbs` → `.org`；输入为 `.fun` 时 `.sbs` → `.fun` → `.org`。`maxHosts=3` 只是 slice 初始容量，并非强制截断，但固定来源集合正常就是三站。GET `/voddetail/{id}/`；保留搜索顺序输出；无链接不输出；最终关键词过滤。封面 `#cover_showbox[data-original]` → JS `def_cover`；标题 `h1.uk-card-title` 去尾部 `(年份)`；`.vodbox li` 抽主演/导演/编剧/分类/地区/年份/TAG/简介。

**解密**：扫描 `td[id^=td-]` 与老 `li[id^=magnet-/pan-/xunlei-]`，读后代 data-pan-url、data-clipboard-text。值 `@@@encrypted@@@` 单独占位丢弃；带此前缀的载荷去前缀后把**最后 5 字符移至开头**，Base64 解码，循环 XOR key `your_secret_key_here` 再反转字节；只有新 key 结果“不 useful”才尝试旧 key `DYWNn`。无前缀直链直接接收，否则尝试普通 Base64。清除 U+200B/U+200C/U+200D/U+FEFF。行内单独解码出的 3–32 字符无空格/斜杠 password 或“提取码/访问码/密码/取件码/password/pwd”文本作为候选；若候选非空优先于 URL 参数，URL 兜底键 pwd/password/passcode/code。按 type+URL+password 去重。

支持 magnet、百度、阿里、夸克、迅雷、UC、天翼、移动、115（含115cdn）、123pan.com、PikPak；未知域名丢弃。link.work_title=电影标题；ID `kpkuang-{movie_id}`。测试固定 JSONP 元数据、两个真实加密向量（阿里、带 pwd 的百度）、重复链接只留一条、详情标题/封面和 HTML 搜索解析。

**确定处置**：完整新增 CLI Provider 与离线 fixture，保留以上协议/数值/顺序与解密向量，不依赖 JS/browser。上游 8 MiB 使用 `LimitReader` 会静默截断，Rust 用 limit+1 明确报错，不能把截断 HTML 当有效页面。协议 Cookie/会话和代理使用统一工厂；不添加搜索缓存。

### 2.2 pan365（上游新增；Rust 新增 `src/providers/pan365/`）

站点 `https://pan.365wp.top`；优先级 3，核心关键词过滤。并发分别 GET `/api/interface/search?keyword=<kw>&disk_type=quark|baidu&page=1&pageSize=20`，不能只请求不指定 disk_type 的首页导致百度被淹没。code 必须 200；data/list 缺失是协议失败，空 list 正常，但空 list 且 search_stats 非空且全 success=false 是“线路全部失败”。先 HTML unescape/合并空白清理 title；按关键词空白分词、大小写不敏感**所有词**匹配；disk_type 必须匹配所请求线路，url 非空且 ≤8192 字节；按加密 url 去重，每类型最多 6 项；交替 quark/baidu 排队，总候选最多12。

对本来已是有效同类型分享的 URL 可直接使用。否则 POST JSON `/api/transfer-share/transfer-share`，body `{"encrypted_url": <搜索原样url>}`，**不得破坏 + / =**。只解析公开 `original_url/share_url/passcode`，不把 file_info/内部任务数据输出或记录。Accept 必须 `application/json, */*`（只填 application/json 会触发该站 WAF）；浏览器 UA、Referer，POST 增 Origin/Content-Type。HTTP 必须200且业务 code200。

URL 校验：仅 http/https，无 userinfo、无显式 port，精确 host `pan.quark.cn`/`pan.baidu.com`，路径 `^/s/[A-Za-z0-9_-]+/?$`。拒绝伪子域、userinfo 欺骗、外链嵌套、空 ID、无效 query 转义；小写 host、去路径尾斜线及空尾 `?`，保留其它 query 参数/fragment（query 重编码可能重排）。pwd → password（值须4–8位字母数字）优先；然后合法 passcode；再从尾部文本或 passcode 文本中抽取4–8位码。work_title 必须填 title。

稳定身份优先“同类型 original_url 的 type+NUL+path”，不含新生成 URL/query；没有合法原始链接则 type+NUL+source+NUL+title；结果ID为 `pan365-` + identity 的 SHA256 **前16字节十六进制**。搜索中同 identity 去重。

预算：搜索总 28s，搜索阶段10s，每请求12s；响应2 MiB（limit+1，超限报错）；实例解析总并发3，含跨多次 search 的信号量；部分线路/资源失败保留成功，最终零条且有错误则报错。HTTP403或业务42901 → CHALLENGE_REQUIRED；HTTP429 → RateLimited；本实例停止上游请求1分钟，后续调用不再打上游；失败不缓存。

成功转存 memo key=type+NUL+encrypted URL，TTL30分钟、最多256项，满时淘汰最早过期项；等 slot 后再查一次 memo；refresh=true 跳过 memo 和 Go 框架搜索缓存。站点“五分钟”文案**不是分享有效期判断**。CLI迁移只做单命令/Provider生命周期内的有界成功转存memo、请求去重，不新增search refresh旗标、不建磁盘搜索缓存或daemon，也不为了长缓存常驻进程；附录中的refresh行为只作为上游memo测试契约，不要求新增CLI入口。30分钟只是 memo 复用期限。

测试逐项约束：两 fixture 各产生2条本类型数据；包含 mixed search_stats 成功/失败不能误判全失败；6+6候选/并发≤3/不相关与重复过滤；12项中单个503、伪域各失败而保留10条；第二轮成功复用、失败重试、第三轮refresh新URL但ID稳定；调用方修改返回结果不污染 memo；响应+1、HTML200、schema缺失、业务失败、全线路失败、验证码/429冷却；直链 query/fragment/password 优先级与安全伪域拒绝；每resolver独立12s deadline。live仅显式 PAN365_LIVE_TEST=1，最多每类型尝试3项。

**确定处置**：全部迁移为 CLI 来源（不是仅列“新增源”）；数值和上述限流/身份/URL契约写入验收。Go README 中 Docker、ENABLED_PLUGINS、Web API 示例不迁移。

### 2.3 panlian（Rust 已有但全部还是旧 PHP 接口；应升级）

**现代搜索**：GET `/api/videos?search=<kw>&sort=year_desc&page=1&page_size=10`，`success/message/data.list,data.page,page_size,total`；新增影片字段 id/title/alias/cover/intro/year/area/lang/remarks/score/type/actor/director，与旧 vod_* 字段归一，旧有非空字段优先。特别是 **type 和 type_name 必须是独立 JSON key**，然后 type_name → type 兜底，不能共用同 tag 导致二者都丢。上游新加 Go 反射测试说明 sonic 同层重名 tag 全忽略；Rust 不复制该结构 bug，要双字段/显式归一测试。

GET `/api/videos/{vodID}` 得 `data.video` 和平铺 `data.links`；链接只有 id/title/pan_type/is_magnet/note/has_code/username/created_at/updated_at/video_title 等 metadata，不是可直接输出的 URL。每片最多解锁 **3 条**以保护账号每日配额：先选不同 pan_type 的第一条，再按原顺序补足，不能简单 take(3) 全是百度。每条顺序 POST JSON `/api/videos/link-ticket` `{link_id}`，得到非空 ticket；紧接 GET `/api/videos/link-open/{linkID}?t=<ticket>` 得 data.url/code。ticket 的 expires_in 有字段但当前实现不缓存/计算期限；不能在 spec 虚构一个固定 TTL。ID≤0或空 ticket/非真实URL均失败；is_magnet 强制磁力类型，未知类型 others；密码用 opened.data.code；时间 updated_at→created_at；来源username。一般链接解锁失败跳过，登录失败中止。整次最多10影片，并发4；遗留路径每结果链接上限200不变。

保留旧 GET `/api/get_videos.php?wd=&pg=1`、`/api/search_pan_links.php?keyword=&vod_id=` 和 token 解析兼容链，不把老部署直接删除。**真实上游判断细节**：fetchVideos 在识别现代 envelope 后处理 success=false/login；fetchPanLinks 的 isModernVideoDetailResponse 却只在 success=true 且 video.id>0或links非nil 时成立，因此 HTTP200 的 success=false 现代详情会掉进旧接口，而非直接报告现代失败。Rust 应按可识别 envelope 先判 auth/业务错误，只对确实非现代/endpoint缺失做兼容回落，避免隐藏登录过期。这是迁移时避免复制已有缺陷，不是漏审。

**新人工登录三步**：
1. GET `/api/auth/captcha`，成功 data.id + data.image（Base64 image data URL），不要求 PHPSESSID 预登录，验证码 ID 即会话参数。
2. POST form `/api/auth/login`：username/password、remember=`1`（原on）、captcha_id/captcha_code；Referer `/login`、XHR。用户名密码或验证码缺失本地拒绝。HTTP200 **和400**均解析 JSON；details.captcha_error 或验证码错误消息→明确验证码错误；details.email_verify_required=true 且 confirm_id非空→待邮箱确认（不是登录已完成），返回掩码 email 提示；也允许直接 success=true + cookie 的路径。
3. 必要时 POST form `/api/auth/login/confirm-email`：confirm_id/email/remember=1；是输入账号原绑定邮箱核对，**不是收邮箱OTP**。成功得到 admin_session（测试MaxAge2592000，即30天）；仅此后持久化 active。邮箱不符、过期confirmID明确失败，可保留待确认状态重输。

自动用保存密码续期已被删除：cookie过期后标记expired、清空cookie并保存，解释“需图形验证码+邮箱确认，请重新登录”，不得继续静默POST旧登录。新的 saveUserOrLog 让成功搜索更新lastAccess、过期状态写盘失败可见。Rust 目前 `login/relogin_with_context` 仍调用旧api/get_types.php、api/login.php，必须替换；移除/废止以密码实现无交互自动续期的承诺。用现有 StateStore 安全保存 cookie/profile，不必照搬 Go 永久存密码；密码从 TTY / --password-stdin 读，captcha图片保存私有临时文件或显示并让用户回填，邮箱交互提示；不记录captchaID/confirmID/cookie/密码到日志。

**HTTP/重试**：GET 总3次，间隔200/400ms（线性函数在第三次后原本会多sleep600ms，迁移后最后不等）；每次独立20s；GET读取16MiB。建请求失败、登录过期（401，或非200正文“请先登录”/admin_auth_required；200非JSON但正文含login/请先登录）及不可解析schema立即Abort，不等待不重试；其它HTTP/传输/读失败重试。POST ticket单发20s、上游只读1MiB LimitReader，Rust显式判超限。不要把需要每日配额的POST当可随意重试GET。login/captcha/confirm邮箱响应采用16MiB。

新增URL分类别名 `pan.uc.cn`（原仅drive.uc.cn）、`123pan.cn`（包括分享子域）；Rust 应在来源/公共识别中补齐。测试具体覆盖人工流程所有分支（未完成邮箱不得active、成功近30天、secure cookie测试必须用https或非secure假站）、GET401只1次且无200/400ms等待、500/500/200共3次、type/type_name双字段、不同盘型选取以及UC/123别名。live step1/step2/http1/http2仅显式运行，不把真实凭据/验证码落仓。

**确定处置**：现代协议+兼容回落+人工CLI登录+过期持久化+配额上限全部迁移到 `src/providers/panlian/mod.rs` / CLI auth；不迁移 Gin管理页、网页JS样式，但保留其等效动作与分阶段状态。离线 mock 现代search/detail/ticket/open整链应补足，上游新增测试并未完整覆盖这条链。

### 2.4 QQPD（Rust 已有；应替换 HTML guild 解析并修复状态/错误）

**官方解析 API**：不再 GET `/g/{短号}` 从 groupprohead图片提取ID，改 POST `https://pd.qq.com/qunng/guild/gotrpc/auth/trpc.group_pro.cmd0x907e.Cmd0x907e/HandleProcess?forGuildId=1&bkn=<hash>`，需要cookie中 p_skey；bkn现有算法不变。Header X-QQ-Client-AppId=`537246381`，x-oidb=`{"uint32_service_type":1}`，Origin/Referer pd.qq.com，JSON Accept。payload含 guild_num、cmd0xf57_req.msg_field_filter.msg_guild_info_filter.uint32_guild_number=1、rpt_req_guild_info_list=[{}]。读 `data.cmd0xf57_rsp.rpt_rsp_guild_info_list[0].uint64_guild_id`，uint32_result必须0。

数字ID合法范围是**十进制无前导零、首字符1–9、可解析u64**，即1..18446744073709551615；不是“全是数字就行”。直接合法数字不用网络/无需Cookie。无效例 0、01、+n、-1、1.0、溢出u64。cookie缺 p_skey 明确需重新登录，不回落到把短号当ID。

统一 requestAPI 每请求15s、响应**4MiB limit+1**；严格HTTP200；识别正文 EO-Bot-Js-Token 为EdgeOne验证，不解释成合法空结果；有效JSON必须含 retcode，要求retcode=0且error.code=0；保留retcode/code/message。Transport错误剥离url.Error中的认证URL，不能泄露bkn/cookie；不输出原始敏感响应。

**搜索**：NewSearch endpoint不变，x-oidb=`{"uint32_command":"0x9287","uint32_service_type":"2"}`，payload仍guild_id/query/cookie=""/member_cookie=""/search_type(type=0,feed_type=0)/cond(channel_ids=[],feed_rank_type=0,type_list=[2,3])。新增通用header AppId、认证错误检查。data缺失是错误；data存在但union_result=null合法空；guild_feeds=[]合法空。当前Rust只检查guild_feeds且不验证retcode，需修复。搜索前逐频道 refreshCookie 调用被移除；QR登录链仍存在，4个自建客户端（checkQR、fetchFullCookie、refreshCookie、generateQRCode）新遵守代理与默认TLS验证。

**索引修复**：加载账号时删除非数字/非法u64映射，但保留登录/频道配置；保存频道先去重/去移除和无效缓存，再按已有cookie解析；失败不缓存短号、不要求删账号/重登，下次搜索重试。首次搜索也修复/持久化缺失映射。并发写缓存序列化落盘；只给仍在该profile.channels里的频道写回，防止正在进行的解析复活已删除频道。上游解析/搜索并发信号量50，最多10用户负载均衡；Rust保留来源内现有 add_channels 解析并发8/搜索并发10，二者都不超过上游50频道并发上限，不把50套成全CLI全局并发；必须验收多频道并发无丢写。

**部分失败与截止**：上游executeTasks返回results+joined errors；全失败且零结果是失败，不是假空；有成功结果同时输出warning；管理搜索及set_channels响应都带warning。Rust Provider trait只有 Vec<Result> 返回无法表达partial warning，需要沿搜索诊断/输出通道到stderr或结构化diagnostics，stdout结果格式不被日志污染。当前 `QqpdProfile::is_ready` 要求**所有**缓存都valid，导致一个旧映射让整个profile不参与搜索，必须改成“cookie活跃即可进入repair”；不要只在add_channels做修复。

上游初始化后台 warmup + Go PublishBudget截止内先返回快结果、未开始任务超预算跳过、运行中后台结束，是规避Go异步窗口的方案；本次Rust无daemon/Go发布窗，**不引入warmup线程或抄4秒/3.2秒魔数**。等价CLI能力是命令内解析索引、持久化复用、在CLI自身总deadline内逐项收集已完成结果并取消其余任务；不应等最慢频道后将全部已成功结果丢弃。上游resolveChannelTasks本身在有界等待之前整体等完，因此不能声称其解析阶段也完全受PublishBudget约束。

**失效频道自动处理的重要审阅结论**：注释声称“明确code10003连续两次才摘除”，但真实 `qqpdReDeadChannel` 仅匹配 `频道 (\S+): [QQPD] 获取频道信息失败`，因此任意非零uint32_result（测试含code7）都可能计数；`deadChannelFailures`是全进程按短号累积，**没有成功后清零**，达到2次会从**所有用户**配置删除并落盘。不能在spec把注释当实现，也不应盲搬这种破坏性行为。CLI适用能力确定迁移为类型化“源站明确频道不存在(code10003)”识别、每profile连续失败计数且成功复位、达到2次移除/隔离该profile的失效频道并给可见诊断；传输/权限/未知code不能永久删配置。无daemon场景确定将该计数落到对应profile，跨CLI命令累计连续失败、成功立即清零，不能写一个退出即丢且声称跨搜索生效的计数器。

**测试/文档映射**：官方路径/headers/body/bkn/15s deadline；u64最大合法与0/前导零/溢出拒绝；retcode155、嵌套code221、缺retcode、缺guild、非数字ID、uint32_result7、EdgeOne/HTML、HTTP503、4MiB+1；transport错误不含bkn/私钥；旧ID修复后只lookup一次、两轮共四次搜索，cookie和两频道不变；失败lookup下一次会重试且不污染缓存；全错误vs部分成功warning；合法空union_result；12频道并发落盘映射不丢；set_channels失败仍保留频道并告警，删除频道不会被inflight恢复。live默认从本地账号只读载入、写临时目录，不修改真实账号。

**确定处置**：升级 `src/providers/qqpd/search.rs` 和 `auth.rs`、频道CLI管理及StateStore；无需浏览器或额外JS运行时。官方API/认证脱敏/有效u64/自动repair/partial diagnostics/TLS代理全部迁移；网页escapeHTML、溢出样式不构成CLI UI需求，等价只输出纯文本诊断不插HTML。

## 3. 其余 32 来源：实际净变化与逐项确定处置

> 以下“新增”仅指 Rust 接入，不暗示上游本次新增。通用 H/P/R 的具体值已在第1节固定；每条写出真实调用位置和特殊边界。所有新增来源同时加入显式注册表/CLI来源列表、success/empty/malformed fixture，不因只有安全/重试改动而遗漏。

1. **jikepan**（Rust新增）：`doSearch` 的 JSON响应 `io.ReadAll`→H16MiB；POST name/is_all 的既有schema、ext.is_all、服务类型映射无净变化。迁移新增适配器时把该搜索响应改为超限/读失败显式来源错误；不是新加is_all功能。
2. **jsnoteclub**（已有 `src/providers/jsnoteclub.rs`）：Ghost posts解码换utiljson；自建连接池加入P；重试统一，**搜索/详情调用上限3次、200/400ms**（requestTimeout12s、detailTimeout10s仍原值），非200错误保留状态；无新增业务字段/关键词行为。Rust已有Ghost读取/详情8并发，但无此重试，应补统一请求策略并确保所有路径遵守代理。serde无需换库；不要以decoder替换为依据虚构16MiB净变化（可以作为统一HTTP安全策略补上）。
3. **jupansou**（已有 `src/providers/jupansou.rs`）：带cookie的私有Transport加入P；transfer JSON decoder归一；请求重试总**3次、200/400ms**，严格200、保留状态并释放失败体；搜索/transfer业务协议未改。Rust session/stream/transfer全部用工厂客户端，克隆可重放请求（multipart body不能第二轮变空），补对应重试；无需新写SSE协议。
4. **jutoushe**（Rust新增）：原3次200/400ms策略不变，只修复非200时nil清空lastErr；regex移为包级：pwd=、`/分类/数字.html`ID、【标签】、中文日期；模式完全不变。迁移明确200成功，失败报真实状态；Rust LazyLock预编译对应表达式即可，不能记录成扩大盘型支持。
5. **kkmao**（Rust新增）：自建Transport加入P；重试改共用工具，搜索总**3次**200/400ms、详情总**2次**200ms；非200恢复真实状态。新测试500/500/200共3次，base100ms时总等待至少300ms且不超过2s；持续403两次错误含403、不含nil格式错误。新增适配器保留已有8详情并发，重试依该来源策略，不复制后台缓存清理。
6. **kkv**（Rust新增）：非200真实状态修复，仍3次200/400ms；ID `?p=(数字)` 与提取码/密码/pwd **恰4位字母数字**的regex仅预编译。迁移这些语义和预编译，不把格式优化误写成下载链接结构变化。
7. **labi**（Rust新增）：自建Transport加入P；3次200/400ms中非200错误不再丢状态；context取消即停止分支保留。迁移搜索和详情都不得偷偷直连，取消时不再等下一轮；无站点API净变化。
8. **leijing**（Rust新增）：仅gzip解压reader→H64MiB超限错误；topicId数字和天翼 `https://cloud.189.cn/t/[A-Za-z0-9]+` regex仅预编译。迁移有界解压，不声称本文件同时新增16MiB裸体上限/100倍比值。
9. **leso**（Rust新增，见本条完整协议变化）：站点 `https://www.leso.cc`；POST search.php 后可能302到searchid结果页，新 `loadDocument` 对301/302/303/307/308显式取Location（相对URL按原请求解析）后**最多再发一次GET**，不依赖客户端是否自动跳转；缺Location/最终非200报状态。搜索6MiB、详情8MiB仍LimitReader，Rust改成显式超限；20s请求，最多30搜索项/详情并发6。详情失败从bool改error，空链接也算失败：搜索列表本身空→正常空；有条目但详情全失败→带条目数和首错的来源错误；部分失败保留成功且诊断首错。测试覆盖整链POST302→列表→夸克链接密码1234、全失败错误、正常空、两条中一条成功；live LESO_LIVE=1（可自定义kw）。测试默认客户端会自己跟随跳转，**没证明no-follow手动分支**，Rust补RedirectPolicy::None下相对Location/缺Location/第二次仍重定向测试。cookie需同Session保留；新Go代码本身没有新增CookieJar，不把live注释当实现证据。不迁移fmt.Printf到stdout。
10. **libvio**（Rust新增）：`https://libvio.host` **在基准源码已是此值**；本次文档把旧www.libvio.mov更正，不能把它当本次源码域名切换。净新增CDN PoW握手：只有403且header X-CDN-Challenge=required（忽略大小写）或正文含__cdn_pow才解题，普通地域封禁403不无限重试。探测正文64KiB；获取根路径挑战页上限1MiB（gzip经64MiB capped reader），regex取TS/SIG/DIFF/MODE；找最小nonce，使SHA256(SIG+十进制nonce)的hex以前缀DIFF匹配，nonce范围**0..2^22-1**，典型DIFF0000=16bit。提交根路径GET Cookie `__cdn_pow=TS_MODE_nonce_SIG`，该跳禁止重定向/禁Jar重复cookie，取Set-Cookie __cdn_verified，再重试原搜索/详情/播放请求。verified服务器max-age1800s，客户端缓存25min（提前5min）；并发单飞握手，持有cookie被403否定则清除重新解，不把旧cookie永久复用。已知向量最小nonce1657和133839；不可能难度跑满上限报错；header/body/地域封禁判别、过期/清空状态有测试。原搜索→详情直链优先→旧播放player_aaaa结构不属本次新增，regex仅预编译；gzip新增64MiB封顶。Rust新增Provider可直接完成纯HTTP SHA256，不需浏览器；阻塞CPU循环应尊重CLI取消；同命令共享握手cookie和singleflight，普通403不截断后当成功页面（上游探测会把普通403正文缩成64KiB，Rust不必复制这个诊断损失）。不迁移搜索缓存/后台TTL删除goroutine。
11. **lingjisp**（Rust新增）：Transport加入P；`doLingjiGET` 从无限append读体改H16MiB，读取失败不再以包含EOF字符串判定成功；总3次200/400ms；**每次**建立独立timeout（搜索20s、详情12s），不是整轮共用一次；响应每轮立即关闭；空白正则仅预编译。新增该来源严格保留per-attempt deadline+CLI总deadline，不能漏掉读体也应重试这一点。
12. **lou1**（Rust新增）：搜索JSON响应读体H16MiB；自建池加入P；总3次200/400ms，非200真实状态修复；请求/详情12s，搜索limit12/详情workers6未改。迁移上述调用，不能把CRC32/磁力解析等未改旧行为写成新增能力。
13. **meitizy**（已有 `src/providers/meitizy.rs`）：Transport加入P；搜索响应H16MiB，重试回放**请求body**的ReadAll也限制16MiB。业务POST search schema未变；Rust已序列化小JSON而不是消耗式读取旧body，只需可重放构建请求和有界响应，不模仿Go把req.Body读空的实现。验证代理经过POST路径、超限明确错误、重放body完整。
14. **melost**（Rust新增）：只有 `searchPage` 裸读响应改H16MiB，既有API/分页/重试没有净变化。新增适配器的每个搜索页都强制限额/读失败可见，不说“无CLI影响”。
15. **miaoso**（Rust新增）：搜索加密响应H16MiB；原3次200/400ms非200保留真实状态。AES/搜索API不是本次新协议；新增适配器先限额再解密解析，错误不得伪装空结果；不要把上游旧源码硬编码会话token作为可提交用户凭据迁移。
16. **mikuclub**（Rust新增）：Transport加入P；posts JSON decoder归一；搜索3次200/400ms、详情2次200ms，非200状态恢复，失败体立即释放；详情12并发/12s搜索10s详情原值不变。迁移源码适配器时保留这些请求级界限，不引入Go全局缓存清理task；serde不换库。
17. **miosou**（Rust新增）：**仅**CookieJar客户端自建Transport加入P，池32/每host8/max16/90s无变化。新增适配器必须带会话且cookie/session请求全部走CLI代理/NO_PROXY；不能因为没有解析净变化而遗漏。
18. **mizixing**（Rust新增）：Transport加入P；搜索/详情重试3次200/400ms，真实非200状态修复。新测试精确3次500/500/200、持续403错误含状态不含nil格式错误；12搜索项/详情并发6、12s搜索10s详情不变。新增Provider与同等mock验收，非全局替换成只重试5xx（原策略非200均重试）。
19. **muou**（Rust新增）：仅Transport加入P+非200lastErr修复，原3次200/400ms和context取消停止不变。迁移该来源搜索/详情代理和明确状态错误，保持取消语义。
20. **nsgame**（Rust新增）：搜索响应H16MiB；detail decoder归一（本身无新增限额）；postSessionRaw先判HTTP200再读，非200不先读无用巨大错误页；会话体H16MiB，读失败不能吞成半截成功。PoW difficultyBits合法**0..256含端点**，负数和>256直接返回无解；SHA256(`challenge:nonce`)检查前缀零位，nonce0..9,999,999最多1千万次；256仅完整32字节、remainder0不访问sum[32]，不要误说256被禁止。新增测试257/260/263/264/512/4096/-1/-8无panic且空解，0正常求解。重试仍3次200/400ms；新增统一状态捕获，但本文件原来在lastErr=nil之后已有HTTP状态兜底，所以这一处本身是错误处理归一、并非首次解决丢状态。Rust新增来源要保留真实状态并显式校验难度和总CPU/取消预算，不仅依赖Rust安全数组索引panic。
21. **nyaa**（Rust新增）：Transport加入P；3次200/400ms非200保留状态。没有磁力生成/排序净变化。新增来源让磁力搜索也经过CLI代理，403/5xx耗尽显式错误。
22. **ouge**（Rust新增）：自建池加入P；API搜索响应H16MiB；其余几乎格式化。新增来源无需虚构新搜索schema，聚合JSON读取必须按limit+1判断。
23. **pan666**（Rust新增）：API仍 `https://pan666.net/api/discussions`（常量变可注入变量是为了测试）；每页50；fetchPage重试总retries+1，默认retries2即**3次，固定500/500ms**；传输/读体/非200重试，H16MiB；读完即close而非循环defer积压。测试500/500/200至少3命中，耗尽错误（插件还有其它endpoint所以断言不是精确3）；测试隔离磁盘缓存和给异步框架30s只是测试支架，不是CLI要加缓存/30s窗口的证据。新增Provider及request-level精确重试mock，不复制Go框架。
24. **pansearch**（已有 `src/providers/pansearch.rs`）：firstPage和后续fetchPage均H16MiB；时间先RFC3339失败再 `2006-01-02 15:04:05`，仍失败保留无时间并诊断，而非丢解析错导致日期过滤莫名不见。Rust convert当前仅RFC3339，补 `NaiveDateTime` 当UTC的同格式兜底，双失败仍None且走现有无时间筛选契约，诊断到stderr而不是上游fmt.Printf stdout。没有新增“未知时间按现在”的行为。
25. **panta**（Rust新增）：仅重试收敛有语义关注点：maxRetries2即**3次**，指数 `min(100ms*2^attempt,5000ms)`，正常只等100/200ms；只对**传输错误或HTTP>=500**重试，所有<500响应（包括403/429）原样交调用者；每次记录响应耗时保持自适应并发统计。耗尽时有5xx响应→response+nil error由调用者判状态，只有无可用响应才error。新代码保留之前一次5xx变量，之后若纯传输失败也可能最终回旧5xx；Rust不必照搬stale response，必须保留最后失败的真实分类。现有响应进入下一次成功时的释放依RAII解决。不能泛化成“所有非200都重试”或“所有失败都返回Err”；来源边界最终仍需HTTP错误诊断，不输出5xx页面为资源。
26. **panwiki**（Rust新增）：**只**8个regex从函数移包级（searchid、tid、单行“作品丨盘名：URL”识别与提取、作品关联、去HTML、详情URL、回复/查看统计），表达式逐字未变；没有新API/解析格式/盘型支持。迁移新增Provider时静态/LazyLock预编译，保留现有单行多作品与关键词关联，不造功能差异；不加搜索缓存。不能因只perf变化便把来源留在范围外。
27. **panyq**（Rust新增）：Transport加入P；TLS从固定InsecureSkipVerify=true变显式配置默认false；action ID发现页（包括错误页）、_next JS文件、rawFinalLink、credentials、searchResults读取全部H16MiB；8个regex仅预编译（script路径、40hex action、sign、64hex sha、hash、URL/magnet、书名号/方括标题）。Next action发现/签名链本次未新增；Rust新增适配器需这些每一阶段限额与默认安全TLS，不只限制最终搜索JSON。
28. **panzun**（Rust新增）：**shortLinkClient**加入P（不是凭此断言所有第三方cloudscraper流量都已代理）；搜索每页和discussion详情响应H16MiB；既有有部分结果时容忍后页读取错误的分支保留。Rust新增来源让搜索、详情与短链都统一客户端代理，分阶段限额，保留partial结果并诊断，不能只测搜索直连。
29. **pianku**（Rust新增）：只修复非200lastErr，仍总3次200/400ms；站点仍 `https://btnull.pro`、搜索 `/search/-------------.html`。新增来源明确状态错误，磁力/ED2K解析不是本次新变更。
30. **qingying**（Rust新增）：详情body H16MiB；3次200/400ms非200恢复状态；voddetail ID、完整秒级时间和pwd4位regex只是预编译。新增适配器有界详情读；无新的123盘结构净变化。
31. **qiwei**（Rust新增）：Transport加入P；验证提交response和通用fetchBody响应都H16MiB；3次200/400ms非200恢复状态；既有验证/MD5等协议不变。新增来源不能只限搜索而漏验证，验证码/验证体超限不继续解析；日志不含认证内容。
32. **quark4k**（Rust新增）：API仍 `https://quark4k.com/api/discussions`、每页50；与pan666同形fetchPage收敛，默认额外retries2→总3次，固定500/500ms，H16MiB，及时关闭失败响应，错误耗尽显式返回；没有独立新增测试文件分配。新增来源补request-level500/500/200和持续失败测试，不能把固定退避改成指数而声称等价。

## 4. 59/59 文件已审阅映射（没有“待核查”）

每项后面是本文结论位置；测试/fixture全部实际阅读，不以文件名推测。

| # | 分配文件 | 已审阅结论 |
|---|---|---|
|1|plugin/jikepan/jikepan.go|§3.1：搜索H16MiB，其余格式化|
|2|plugin/jsnoteclub/jsnoteclub.go|§3.2：decoder/P/3次指数与真实状态|
|3|plugin/jupansou/jupansou.go|§3.3：私有池P/decoder/3次指数|
|4|plugin/jutoushe/jutoushe.go|§3.4：状态修复/4个regex预编译|
|5|plugin/kkmao/kkmao.go|§3.5：P/搜索3详情2重试|
|6|plugin/kkmao/kkmao_retry_test.go|§3.5：3次成功/100+200ms/403不丢|
|7|plugin/kkv/kkv.go|§3.6：状态/4个regex|
|8|plugin/kpkuang/kpkuang.go|§2.1：完整新来源747行、beta/HTML/镜像/解密/边界|
|9|plugin/kpkuang/kpkuang_test.go|§2.1：5项parser/解密/去重回归|
|10|plugin/labi/labi.go|§3.7：P/状态/取消|
|11|plugin/leijing/leijing.go|§3.8：gzip64MiB/2regex|
|12|plugin/leso/leso.go|§3.9：显式一跳重定向/详情全失败区别|
|13|plugin/leso/leso_live_test.go|§3.9：显式live开关/3关键词或自定义/非空断言|
|14|plugin/leso/leso_test.go|§3.9：四种整链结果/测试未覆盖no-follow分支|
|15|plugin/libvio/html结构分析.md|§3.10：文档域名更正不是源码域名净变化/挑战说明|
|16|plugin/libvio/libvio.go|§3.10：CDN握手/单飞/TTL/CPU上界/gzip|
|17|plugin/libvio/libvio_test.go|§3.10：真实参数/最小nonce1657和133839/熔断/判别/失效|
|18|plugin/lingjisp/lingjisp.go|§3.11：P/H/3次每次独立timeout/regex|
|19|plugin/lou1/lou1.go|§3.12：搜索H/P/重试状态|
|20|plugin/meitizy/meitizy.go|§3.13：P/响应及回放请求H|
|21|plugin/melost/melost.go|§3.14：只有搜索页H|
|22|plugin/miaoso/miaoso.go|§3.15：搜索H/状态|
|23|plugin/mikuclub/mikuclub.go|§3.16：P/decoder/搜索3详情2|
|24|plugin/miosou/miosou.go|§3.17：仅Cookie客户端P|
|25|plugin/mizixing/mizixing.go|§3.18：P/重试状态|
|26|plugin/mizixing/mizixing_retry_test.go|§3.18：3次成功和403真实错误|
|27|plugin/muou/muou.go|§3.19：P/状态/取消|
|28|plugin/nsgame/nsgame.go|§3.20：H/decoder/会话先状态且不吞读错/0..256挑战|
|29|plugin/nsgame/nsgame_challenge_guard_test.go|§3.20：8个非法值无panic及零难度可解|
|30|plugin/nyaa/nyaa.go|§3.21：P/状态|
|31|plugin/ouge/ouge.go|§3.22：P/搜索H/其余格式化|
|32|plugin/pan365/README.md|§2.2：接口/预算/memo/限流/测试说明，Web部署内容不搬|
|33|plugin/pan365/pan365.go|§2.2：双线路/候选/稳定ID/解析队列|
|34|plugin/pan365/pan365_test.go|§2.2：完整297行，协议/并发/memo/安全/错误/deadline/live|
|35|plugin/pan365/source.go|§2.2：精确URL/码/HTTP/WAF/冷却/有界memo|
|36|plugin/pan365/testdata/search-baidu.json|§2.2：2百度项，混合success/failed stats、NBSP标题|
|37|plugin/pan365/testdata/search-quark.json|§2.2：2夸克项，夸克/百度四线路stats混合|
|38|plugin/pan666/pan666.go|§3.23：可注入base/固定500ms重试/H|
|39|plugin/pan666/pan666_retry_test.go|§3.23：成功/耗尽/隔离缓存和异步测试支架|
|40|plugin/panlian/panlian.go|§2.3：现代API/ticket配额/登录人工流程/过期/HTTP/持久化|
|41|plugin/panlian/panlian_abort_test.go|§2.3：401只1次无等待，500/500/200三次|
|42|plugin/panlian/panlian_jsontag_test.go|§2.3：type与type_name独立、真实反射冲突丢失|
|43|plugin/panlian/panlian_live_test.go|§2.3：step1/2和动作层http1/2，真实用户隔离storage|
|44|plugin/panlian/panlian_login_test.go|§2.3：完整314行假站登录/邮箱/未激活/近30天/过期落盘|
|45|plugin/panlian/panlian_test.go|§2.3：现代字段归一/按盘型选/UC与123别名/auth消息|
|46|plugin/pansearch/pansearch.go|§3.24：两分页H/双时间格式/失败诊断|
|47|plugin/panta/panta.go|§3.25：3次100/200ms、仅5xx/传输、返回response契约|
|48|plugin/panwiki/panwiki.go|§3.26：仅8regex预编译，解析行为未变|
|49|plugin/panyq/panyq.go|§3.27：P/T/发现页与JS、凭证和结果等所有新增H/8regex|
|50|plugin/panzun/panzun.go|§3.28：短链客户端P/搜索及详情H|
|51|plugin/pianku/pianku.go|§3.29：仅非200错误修复，其余格式化|
|52|plugin/qingying/qingying.go|§3.30：详情H/状态/3regex|
|53|plugin/qiwei/qiwei.go|§3.31：P/验证与通用读取H/状态|
|54|plugin/qqpd/README.md|§2.4：官方解析/缓存修复/错误与warning/隔离live|
|55|plugin/qqpd/api.go|§2.4：完整227行，官方API/校验u64/缓存与并发解析|
|56|plugin/qqpd/qqpd.go|§2.4：索引/partial/发布截止/实际宽泛累积摘除/QR P+T/页面转义|
|57|plugin/qqpd/qqpd_live_test.go|§2.4：本地登录只读载入、临时落盘、两关键词非空|
|58|plugin/qqpd/qqpd_test.go|§2.4：完整360行，认证API/错误/修复/合法空/并发/删除不复活|
|59|plugin/quark4k/quark4k.go|§3.32：固定500ms、总3次/H/及时释放|

## 5. 主代理写 spec 时必须避免的错误归纳

1. 不得把Rust已有19来源当迁移白名单；本组应6升级+30新接入。也不得把30个全说成上游本次新增。
2. 不得把本组都写“代理/安全优化”；panlian/QQPD是认证与搜索主链重构，libvio有PoW，leso有重定向/错误语义，kpkuang/pan365为完整新来源。
3. 不得把普通读取16MiB覆盖所有专用限额，或声称decoder替换自动有大小限制。
4. 不得把所有重试写成“3次指数500ms”；pan666/quark4k固定500，panta100/200且只5xx，盘链GET线性200/400且auth/parse立即中止，kkmao/mikuclub详情只有2次。
5. 不得把 libvio.host 写成相对本基准新改源码域名，基准已经是它。
6. 不得把QQPD“连续两次10003摘除”的注释直接当真实实现：代码任意uint32_result非0匹配+跨用户累计+不复位，有确定迁移安全处置见§2.4。
7. 不得把Go异步发布预算、warmup/清理goroutine、搜索缓存和网页搬成Rust daemon；CLI适用效果须用命令内deadline/partial诊断、持久profile索引、手动登录阶段来实现。
8. 所有新读错/限额/HTTP失败必须可观测但脱敏，写stderr/diagnostics；不能照搬Go新增fmt.Printf污染CLI JSON stdout。

审阅状态：59/59已映射；没有遗留待核查文件、没有修改当前Rust项目文件；没有根据历史推断、没有Notion操作。
