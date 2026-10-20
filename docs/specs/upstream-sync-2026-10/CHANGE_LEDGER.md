# 已核查端点净差异与实施范围

本文件及所链接的五份规范附录是[同步规格](../UPSTREAM_SYNC_2026_10_SPEC.md)的组成部分，不是待填写的审计任务。

## 核查完成结论

- 仅比较固定端点 `beaa56133755a548ebc51b090b3816e2ae044aa6` → `221665fb2c9279f22af4684ea2d8c0c331eac1cc`。
- 2026-10-20 已确认远程 main 仍为该目标，已完整审阅全部276文件净diff（含测试、文档、fixture），没有逐commit清单或待核查文件。
- 本地代码实现尚未开始；没有运行Go/Rust测试或live探测。下述核查证据不能表述为实现验收通过。
- 协议与处置按主规格和对应附录执行，不另安排一次上游完整核查。

## 已确定的能力及处置

| 分组 | 文件数 | 具体更新、边界和处置 |
| --- | --- | --- |
| Provider A | 63 | [36来源逐项结论](PROVIDERS_A.md)：6上游新增来源、9已有Rust更新、21历史来源接入；每项协议/重试/额度及测试契约已给出。 |
| Provider B | 59 | [36来源逐项结论](PROVIDERS_B.md)：kpkuang/pan365、盘链现代协议及人工登录、QQPD官方API、LIBVIO PoW、Leso重定向，以及逐源安全/性能更新。 |
| Provider C | 45 | [36来源逐项结论](PROVIDERS_C.md)：sopanya/woniu、yingso/zhizhen/yunso、weibo截止交付和逐源HTTP更新；2个无适用净行为来源明确排除。 |
| 搜索/核心/检测 | 37 | [逐能力结论](SEARCH_CORE.md)：115状态优先、TG同级按钮/DOM/取码/诊断、编排取消/有界出口；缓存/backfill/学习器及daemon专属明确排除。 |
| 基础设施 | 72 | [逐能力及集合结论](INFRASTRUCTURE.md)：代理/NO_PROXY/有界读取/重试/配置隔离/存储原则；Go/API/部署专属排除；75注册/71默认/111频道精确集合。 |

### 来源范围

- 更新或验证已有19个： `pansearch`、`yunsou`、`djgou`、`hdmoli`、`meitizy`、`yulinshufa`、`clxiong`、`jsnoteclub`、`duanjuw`、`dyyj`、`jupansou`、`cldi`、`clmao`、`cyg`、`susu`、`qqpd`、`weibo`、`gying`、`panlian`。
- 新接入87个（包括上游新来源及本轮修改的历史来源）： `488run`、`5266ys`、`ahhhhfs`、`aikanzy`、`aipan`、`alupan`、`ash`、`bixin`、`btbtlb`、`buerchen`、`daishudj`、`diduan`、`discourse`、`duoduo`、`dy4k`、`dyyjpro`、`erxiao`、`erxiaopan`、`feikuai`、`gaoqing888`、`haisou`、`hdr4k`、`hjzhencai`、`huban`、`hunhepan`、`ikantv`、`javdb`、`jikepan`、`jutoushe`、`kkmao`、`kkv`、`kpkuang`、`labi`、`leijing`、`leso`、`libvio`、`lingjisp`、`lou1`、`melost`、`miaoso`、`mikuclub`、`miosou`、`mizixing`、`muou`、`nsgame`、`nyaa`、`ouge`、`pan365`、`pan666`、`panta`、`panwiki`、`panyq`、`panzun`、`pianku`、`qingying`、`qiwei`、`quark4k`、`quarkres`、`quarksoo`、`quarktv`、`qupanshe`、`qupansou`、`sdso`、`shandian`、`sopanya`、`sousou`、`thepiratebay`、`u3c3`、`wanou`、`woniu`、`wuji`、`xb6v`、`xdpan`、`xdyh`、`xiaoji`、`xiaoyu`、`xiaozhang`、`xinjuc`、`xys`、`yingso`、`yiove`、`ypfxw`、`yuhuage`、`yunso`、`zhizhen`、`zlxapp`、`zxzj`。
- `xiaokupan`仅Go JSON适配器替换，`xuexizhinan`仅格式化；不据此新增CLI来源。
- `dygang/haitunsou/rrbt/ting77`在注册/默认集合中但无局部净变化；公共框架继承不单独触发旧来源全面补齐。
- 实际注册、默认启用与来源代码目录不是同一集合；未注册来源的适用协议更新可显式接入，默认加入策略单独决定。

## 276文件已核查映射

下表逐文件给出已完成结论和规范附录定位，不需要实施者再填写。上游多处只改格式/库适配时已明确无业务变化；一个文件涉及多种行为时按对应附录的详细处置执行。

| 上游文件 | 已完成核查结论 | 规范附录 |
| --- | --- | --- |
| `.gitignore` | 已审阅。仅根cache忽略与新增logs忽略；§6，不改变CLI协议。 | [详述](INFRASTRUCTURE.md) |
| `Dockerfile` | 已审阅。Go builder1.25、111频道/71启用Provider精确集合；§0/1/6及附录。 | [详述](INFRASTRUCTURE.md) |
| `README.md` | 已审阅。woniu文档入口、默认频道7、内存SHARD_COUNT范围、TLS显式风险开关；§0/6。 | [详述](INFRASTRUCTURE.md) |
| `api/auth_handler.go` | 已审阅。仅对齐格式，无认证语义更新；§6。 | [详述](INFRASTRUCTURE.md) |
| `api/check_handler.go` | 已审阅。空items之后增加256上限、257报400；无新增请求体上限；§6。 | [详述](INFRASTRUCTURE.md) |
| `api/filter.go` | 已审阅。全diff仅gofmt，无过滤语义变化；§6。 | [详述](INFRASTRUCTURE.md) |
| `api/handler.go` | 已审阅。POST1MiB上限、已知长度413/读取错400、channels512；plugins未限制；§0/6。 | [详述](INFRASTRUCTURE.md) |
| `api/limits.go` | 已审阅。新增1MiB/512/256及>边界；API专用不放宽Rust128；§6。 | [详述](INFRASTRUCTURE.md) |
| `api/limits_test.go` | 已审阅。超body、channels513、items257、恰等上限允许、空items先报错；全部断言已读；§6。 | [详述](INFRASTRUCTURE.md) |
| `api/middleware.go` | 已审阅。仅gofmt，无CORS/JWT/日志逻辑变化；§6。 | [详述](INFRASTRUCTURE.md) |
| `api/router.go` | 已审阅。health新增liveness与tg快照，其余格式；无Rust daemon接口；§6。 | [详述](INFRASTRUCTURE.md) |
| `audit/body_close_guard_test.go` | 已审阅。扫描retry循环continue+defer Body.Close；剔.git/vendor/node_modules；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `audit/decompress_guard_test.go` | 已审阅。gzip/zlib/flate需CappedReader或ReadAllDecompressed；植入对照确保守卫抓得到；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `audit/handrolled_read_guard_test.go` | 已审阅。for Read+append手写无界累积守卫，原quarktv形态对照；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `audit/plugin_audit_test.go` | 已审阅。AUDIT_PLUGIN_SOURCES=1显式live；先refresh、等35s后二查空；网络失败不等于结构坏；§8，不当本次实测。 | [详述](INFRASTRUCTURE.md) |
| `audit/readall_guard_test.go` | 已审阅。裸io.ReadAll(x.Body)守卫，忽略测试及注释；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `audit/setup_test.go` | 已审阅。TestMain chdir根，main/go.mod/plugin/service/util存在且Go文件>=200防空扫描假通过；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `config/config.go` | 已审阅。所有新增/改变getter与注册字段逐段已读，精确数值/代理/TLS/CPU/废弃TTL；§2/6。 | [详述](INFRASTRUCTURE.md) |
| `config/default_channels_test.go` | 已审阅。无环境默认7、环境a,b,c拆分；§0/7。 | [详述](INFRASTRUCTURE.md) |
| `config/insecure_tls_test.go` | 已审阅。nil安全默认校验，true/1 opt-in，false/0/yes/TRUE空格拒绝开启；§2.2。 | [详述](INFRASTRUCTURE.md) |
| `docker-compose.yml` | 已审阅。默认频道基准129项128唯一→111；默认Provider66→71；与Docker目标一致，基准不同；附录。 | [详述](INFRASTRUCTURE.md) |
| `docs/插件修复TODO-2026-09-01.md` | 已审阅。整文件删除61行；完整读过所有旧源状态，不能将删除文档当最新协议/失效证据；§6。 | [详述](INFRASTRUCTURE.md) |
| `docs/系统开发设计文档.md` | 已审阅。所有新增调度/资源/可达性/liveness/守卫段及被移除调优段已读；75/71/4纠正文档76/69/7；§0/6。 | [详述](INFRASTRUCTURE.md) |
| `go.mod` | 已核查：Go升级到1.25.0、删除toolchain固定、升级x/net、x/text、x/crypto、x/sync、x/sys；Go工具链和依赖版本不机械迁入Cargo，Rust保留自身工具链与锁文件 | [详述](INFRASTRUCTURE.md) |
| `go.sum` | 已审阅。上述五库对应校验和替换，无独立协议变更；§6。 | [详述](INFRASTRUCTURE.md) |
| `main.go` | 已审阅。10新增3停止注册、memlimit、后台TGprobe、代理脱敏与CPU日志；§0/1/2/6。 | [详述](INFRASTRUCTURE.md) |
| `model/plugin_result.go` | 完整diff；仅格式/换行；§6，不迁行为 | [详述](SEARCH_CORE.md) |
| `model/request.go` | 完整diff；无字段/JSON变化；§6 | [详述](SEARCH_CORE.md) |
| `model/response.go` | 完整diff；无响应契约变化；§6 | [详述](SEARCH_CORE.md) |
| `plugin/488run/488run.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A1: Cookie/增量poll/清洗/冷却；新增完整Provider，CLI deadline，不启动预热goroutine；新增Rust来源，详见A1 | [详述](PROVIDERS_A.md) |
| `plugin/488run/488run_test.go` | 已审阅；全部231行：多轮poll累计3匹配、排序、百度清洗、封面、WorkTitle、401/403刷新；live环境开关；新增Rust来源，详见A1 | [详述](PROVIDERS_A.md) |
| `plugin/488run/README.md` | 已审阅；全部34行：API/Cookie/增量队列/备用域名声明与实际代码区别；新增Rust来源，详见A1 | [详述](PROVIDERS_A.md) |
| `plugin/5266ys/5266ys.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: hash Regex预编译；新增来源并LazyLock；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/ahhhhfs/ahhhhfs.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 统一代理/非200状态错误；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/aikanzy/aikanzy.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 统一代理/4次200-400-800ms/固定Regex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/aikanzy/aikanzy_retry_test.go` | 已审阅；全部69行：两失败第三成功>=600ms、404真状态、无nil包装；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/aipan/aipan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A2: 搜索12/详情并发6/每片64链接/12s/8MiB；新增来源；新增Rust来源，详见A2 | [详述](PROVIDERS_A.md) |
| `plugin/aipan/aipan_test.go` | 已审阅；全部62行：5类链接含guangya/磁力/ed2k、密码、元数据、按URL识别；新增Rust来源，详见A2 | [详述](PROVIDERS_A.md) |
| `plugin/alupan/alupan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/搜索3详情2/200ms指数/状态错误；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/alupan/alupan_retry_test.go` | 已审阅；全部67行：第三次成功>=600ms、2次502错误真状态；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/ash/ash.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 非200状态/2次100ms；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/bixin/bixin.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 16MiB/3次固定500ms/逐轮释放；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/btbtlb/btbtlb.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A3: 三阶段10影片/12种子+4盘/总40/并发4-8；新增来源；新增Rust来源，详见A3 | [详述](PROVIDERS_A.md) |
| `plugin/btbtlb/btbtlb_test.go` | 已审阅；全部75行：搜索卡/相对图片/全页提取码/magnet/40hex fallback/CST与RFC3339；新增Rust来源，详见A3 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/README.md` | 已审阅；全部35行：双链路/配额/编码/预算/缓存/挑战，按CLI局部状态映射；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/buerchen.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A4: SSE 0-2-4+每日解密转存/18候选/3并发/编码/冷却；新增来源；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/buerchen_test.go` | 已审阅；全部252行：18-2结果、daily/direct、编码/ID稳定/cache clone/失败不缓存/共享3并发；live开关；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/source.go` | 已审阅；全部178行：host+path验证/4-8位密码/12s2MiB请求/1min cooldown/30min256缓存；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/stream.go` | 已审阅；全部139行：SSE framing/错误识别/6条/8192 URL/256KiB行/2MiB流；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/stream_test.go` | 已审阅；全部141行：fixture 2与2/2/0；所有CRLF/多行/DONE/错误/超限/冷却/encodeURIComponent；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/testdata/daily.json` | 已审阅；全部22行：success.data.data两行，quark/baidu有值，xunlei空；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/buerchen/testdata/search.sse` | 已审阅；全部14行：裸线路标记、2有效+1invalid、DONE；新增Rust来源，详见A4 | [详述](PROVIDERS_A.md) |
| `plugin/cldi/cldi.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 16MiB/状态错误/Regex复用；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/clmao/clmao.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 搜索+modern详情16MiB/Regex复用；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/clxiong/clxiong.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 搜索+详情16MiB/4 Regex复用；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/cyg/cyg.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 两API16MiB/状态错误/Regex复用；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/daishudj/daishudj.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/3次200-400ms/状态错误；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/diduan/diduan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 详情16MiB/文章IDRegex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/discourse/discourse.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 分页+Topic详情16MiB/清洗Regex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/djgou/djgou.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 代理/搜索+挑战16MiB/解析错误/状态/Regex；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/duanjuw/duanjuw.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 代理/3次200-400ms/状态错误；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/duoduo/duoduo.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/状态错误；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/dy4k/dy4k.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/搜索+debug读取16MiB；新增来源且不抄双代理；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/dyyj/dyyj.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 代理/HTML16MiB/JSON库/真正空白密码正则；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/dyyjpro/dyyjpro.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/16MiB/per-attempt timeout/读取失败重试；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/erxiao/erxiao.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 仅私有Transport统一代理；独立新增，不合并erxiaopan；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/erxiaopan/README.md` | 已审阅；全部41行：两页/下载区/验证码/标签/cache/DNS声明；不承诺代码未强制的新建连接；新增Rust来源，详见A5 | [详述](PROVIDERS_A.md) |
| `plugin/erxiaopan/erxiaopan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A5: 2页20详情5并发/25s4MiB/3次线性300-600ms/下载区及集数；新增来源；新增Rust来源，详见A5 | [详述](PROVIDERS_A.md) |
| `plugin/erxiaopan/erxiaopan_test.go` | 已审阅；全部323行：捕获页面/3盘密码/翻页cache/结构错误/URL合法性/集数区分/标签拆分/live开关；新增Rust来源，详见A5 | [详述](PROVIDERS_A.md) |
| `plugin/erxiaopan/testdata/detail.html` | 已审阅；全部128行：狂飙下载区三盘+内链噪声、inline天翼密码、7演员截6、封面；新增Rust来源，详见A5 | [详述](PROVIDERS_A.md) |
| `plugin/erxiaopan/testdata/search.html` | 已审阅；全部104行：两卡13756/254、干净h3、元数据、下一页与尾页同class；新增Rust来源，详见A5 | [详述](PROVIDERS_A.md) |
| `plugin/ext_context_key_consistency_test.go` | 已审阅。util/plugin两个_ctx常量一致测试；§4/5。 | [详述](INFRASTRUCTURE.md) |
| `plugin/feikuai/feikuai.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/API16MiB及fallback/Regex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/gaoqing888/gaoqing888.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 代理/16MiB/3次200-400ms/per-attempt timeout；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/gaoqing888/gaoqing888_retry_test.go` | 已审阅；全部73行：第3次成功/600ms/503失败/3*100ms+600ms<1.5s；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/gying/gying.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B+C: 零PoW等待/会话预热/三数组对齐/预算部分成功/错误区分；不迁移服务cache和daemon；更新现有Rust来源，详见B+C | [详述](PROVIDERS_A.md) |
| `plugin/gying/gying_diag_live_test.go` | 已审阅；全部386行：登录/响应诊断落盘/恢复/缺验证cookie/失效会话/各阶段计时/PoW零等待；仅opt-in且Rust不能抄Cookie泄漏日志；更新现有Rust来源，详见B+C | [详述](PROVIDERS_A.md) |
| `plugin/gying/gying_index_guard_test.go` | 已审阅；全部66行：title/d/i三数组、负index、缺字段、buildResult不panic；更新现有Rust来源，详见B+C | [详述](PROVIDERS_A.md) |
| `plugin/gying/gying_live_test.go` | 已审阅；净差异仅3行注释；不宣称实际修改skip条件；更新现有Rust来源，详见B+C | [详述](PROVIDERS_A.md) |
| `plugin/gying/gying_refresh_test.go` | 已审阅；全部64行：refresh旧结果fallback、正常cache命中、无cache无账号成功空；Go服务缓存不引入Rust；更新现有Rust来源，详见B+C | [详述](PROVIDERS_A.md) |
| `plugin/haisou/haisou.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 搜索+hsid读取16MiB/状态/Regex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/hdmoli/hdmoli.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；B: 详情16MiB/状态/Regex；就地更新；更新现有Rust来源，详见B | [详述](PROVIDERS_A.md) |
| `plugin/hdr4k/hdr4k.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 仅清洗Regex预编译；新增完整来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/hjzhencai/README.md` | 已审阅；全部35行：查询/翻页/限额/下载区/WorkTitle/cache/合法分享；新增Rust来源，详见A6 | [详述](PROVIDERS_A.md) |
| `plugin/hjzhencai/hjzhencai.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；A6: 3页48详情3并发/25s2MiB/下载分组WorkTitle；新增来源；新增Rust来源，详见A6 | [详述](PROVIDERS_A.md) |
| `plugin/hjzhencai/hjzhencai_test.go` | 已审阅；全部297行：3卡3盘密码/CST时间/下载区范围/分组WorkTitle/伪URL/分页循环/clone/部分失败/3并发/403超限合法空；新增Rust来源，详见A6 | [详述](PROVIDERS_A.md) |
| `plugin/hjzhencai/testdata/detail.html` | 已审阅；全部91行：module-info元数据/2026-02-05更新/三盘每组单第1集/重复按钮/天翼code；新增Rust来源，详见A6 | [详述](PROVIDERS_A.md) |
| `plugin/hjzhencai/testdata/search.html` | 已审阅；全部93行：三卡6522-6524/干净标题/海报占位/演员内容；新增Rust来源，详见A6 | [详述](PROVIDERS_A.md) |
| `plugin/huban/huban.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 仅私有Transport统一代理；新增完整来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/hunhepan/hunhepan.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 每页API16MiB；新增完整来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/ikantv/ikantv.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 搜索16MiB/状态可见性；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/javdb/javdb.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 16MiB/普通与429独立重试/默认429仅一次/Regex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/javdb/javdb_ratelimit_test.go` | 已审阅；已逐段审阅全部净差异（含全部新增代码及所有修改hunk）；D: 16MiB/普通与429独立重试/默认429仅一次/Regex；新增来源；新增Rust来源，详见D | [详述](PROVIDERS_A.md) |
| `plugin/jikepan/jikepan.go` | §3.1：搜索H16MiB，其余格式化 | [详述](PROVIDERS_B.md) |
| `plugin/jsnoteclub/jsnoteclub.go` | §3.2：decoder/P/3次指数与真实状态 | [详述](PROVIDERS_B.md) |
| `plugin/jupansou/jupansou.go` | §3.3：私有池P/decoder/3次指数 | [详述](PROVIDERS_B.md) |
| `plugin/jutoushe/jutoushe.go` | §3.4：状态修复/4个regex预编译 | [详述](PROVIDERS_B.md) |
| `plugin/kkmao/kkmao.go` | §3.5：P/搜索3详情2重试 | [详述](PROVIDERS_B.md) |
| `plugin/kkmao/kkmao_retry_test.go` | §3.5：3次成功/100+200ms/403不丢 | [详述](PROVIDERS_B.md) |
| `plugin/kkv/kkv.go` | §3.6：状态/4个regex | [详述](PROVIDERS_B.md) |
| `plugin/kpkuang/kpkuang.go` | §2.1：完整新来源747行、beta/HTML/镜像/解密/边界 | [详述](PROVIDERS_B.md) |
| `plugin/kpkuang/kpkuang_test.go` | §2.1：5项parser/解密/去重回归 | [详述](PROVIDERS_B.md) |
| `plugin/labi/labi.go` | §3.7：P/状态/取消 | [详述](PROVIDERS_B.md) |
| `plugin/leijing/leijing.go` | §3.8：gzip64MiB/2regex | [详述](PROVIDERS_B.md) |
| `plugin/leso/leso.go` | §3.9：显式一跳重定向/详情全失败区别 | [详述](PROVIDERS_B.md) |
| `plugin/leso/leso_live_test.go` | §3.9：显式live开关/3关键词或自定义/非空断言 | [详述](PROVIDERS_B.md) |
| `plugin/leso/leso_test.go` | §3.9：四种整链结果/测试未覆盖no-follow分支 | [详述](PROVIDERS_B.md) |
| `plugin/libvio/html结构分析.md` | §3.10：文档域名更正不是源码域名净变化/挑战说明 | [详述](PROVIDERS_B.md) |
| `plugin/libvio/libvio.go` | §3.10：CDN握手/单飞/TTL/CPU上界/gzip | [详述](PROVIDERS_B.md) |
| `plugin/libvio/libvio_test.go` | §3.10：真实参数/最小nonce1657和133839/熔断/判别/失效 | [详述](PROVIDERS_B.md) |
| `plugin/lingjisp/lingjisp.go` | §3.11：P/H/3次每次独立timeout/regex | [详述](PROVIDERS_B.md) |
| `plugin/lou1/lou1.go` | §3.12：搜索H/P/重试状态 | [详述](PROVIDERS_B.md) |
| `plugin/meitizy/meitizy.go` | §3.13：P/响应及回放请求H | [详述](PROVIDERS_B.md) |
| `plugin/melost/melost.go` | §3.14：只有搜索页H | [详述](PROVIDERS_B.md) |
| `plugin/miaoso/miaoso.go` | §3.15：搜索H/状态 | [详述](PROVIDERS_B.md) |
| `plugin/mikuclub/mikuclub.go` | §3.16：P/decoder/搜索3详情2 | [详述](PROVIDERS_B.md) |
| `plugin/miosou/miosou.go` | §3.17：仅Cookie客户端P | [详述](PROVIDERS_B.md) |
| `plugin/mizixing/mizixing.go` | §3.18：P/重试状态 | [详述](PROVIDERS_B.md) |
| `plugin/mizixing/mizixing_retry_test.go` | §3.18：3次成功和403真实错误 | [详述](PROVIDERS_B.md) |
| `plugin/muou/muou.go` | §3.19：P/状态/取消 | [详述](PROVIDERS_B.md) |
| `plugin/nsgame/nsgame.go` | §3.20：H/decoder/会话先状态且不吞读错/0..256挑战 | [详述](PROVIDERS_B.md) |
| `plugin/nsgame/nsgame_challenge_guard_test.go` | §3.20：8个非法值无panic及零难度可解 | [详述](PROVIDERS_B.md) |
| `plugin/nyaa/nyaa.go` | §3.21：P/状态 | [详述](PROVIDERS_B.md) |
| `plugin/ouge/ouge.go` | §3.22：P/搜索H/其余格式化 | [详述](PROVIDERS_B.md) |
| `plugin/pan365/README.md` | §2.2：接口/预算/memo/限流/测试说明，Web部署内容不搬 | [详述](PROVIDERS_B.md) |
| `plugin/pan365/pan365.go` | §2.2：双线路/候选/稳定ID/解析队列 | [详述](PROVIDERS_B.md) |
| `plugin/pan365/pan365_test.go` | §2.2：完整297行，协议/并发/memo/安全/错误/deadline/live | [详述](PROVIDERS_B.md) |
| `plugin/pan365/source.go` | §2.2：精确URL/码/HTTP/WAF/冷却/有界memo | [详述](PROVIDERS_B.md) |
| `plugin/pan365/testdata/search-baidu.json` | §2.2：2百度项，混合success/failed stats、NBSP标题 | [详述](PROVIDERS_B.md) |
| `plugin/pan365/testdata/search-quark.json` | §2.2：2夸克项，夸克/百度四线路stats混合 | [详述](PROVIDERS_B.md) |
| `plugin/pan666/pan666.go` | §3.23：可注入base/固定500ms重试/H | [详述](PROVIDERS_B.md) |
| `plugin/pan666/pan666_retry_test.go` | §3.23：成功/耗尽/隔离缓存和异步测试支架 | [详述](PROVIDERS_B.md) |
| `plugin/panlian/panlian.go` | §2.3：现代API/ticket配额/登录人工流程/过期/HTTP/持久化 | [详述](PROVIDERS_B.md) |
| `plugin/panlian/panlian_abort_test.go` | §2.3：401只1次无等待，500/500/200三次 | [详述](PROVIDERS_B.md) |
| `plugin/panlian/panlian_jsontag_test.go` | §2.3：type与type_name独立、真实反射冲突丢失 | [详述](PROVIDERS_B.md) |
| `plugin/panlian/panlian_live_test.go` | §2.3：step1/2和动作层http1/2，真实用户隔离storage | [详述](PROVIDERS_B.md) |
| `plugin/panlian/panlian_login_test.go` | §2.3：完整314行假站登录/邮箱/未激活/近30天/过期落盘 | [详述](PROVIDERS_B.md) |
| `plugin/panlian/panlian_test.go` | §2.3：现代字段归一/按盘型选/UC与123别名/auth消息 | [详述](PROVIDERS_B.md) |
| `plugin/pansearch/pansearch.go` | §3.24：两分页H/双时间格式/失败诊断 | [详述](PROVIDERS_B.md) |
| `plugin/panta/panta.go` | §3.25：3次100/200ms、仅5xx/传输、返回response契约 | [详述](PROVIDERS_B.md) |
| `plugin/panwiki/panwiki.go` | §3.26：仅8regex预编译，解析行为未变 | [详述](PROVIDERS_B.md) |
| `plugin/panyq/panyq.go` | §3.27：P/T/发现页与JS、凭证和结果等所有新增H/8regex | [详述](PROVIDERS_B.md) |
| `plugin/panzun/panzun.go` | §3.28：短链客户端P/搜索及详情H | [详述](PROVIDERS_B.md) |
| `plugin/pianku/pianku.go` | §3.29：仅非200错误修复，其余格式化 | [详述](PROVIDERS_B.md) |
| `plugin/plugin.go` | 已审阅。per-call_ctx/main key/keyword、延迟配置clients/workers/TTL、ext键隔离、失败回退；完整净diff已读；§4/5。 | [详述](INFRASTRUCTURE.md) |
| `plugin/plugin_config_lazy_test.go` | 已审阅。构造先于config、20s/60s/5h、worker7、nil默认、单实例复用；§4。 | [详述](INFRASTRUCTURE.md) |
| `plugin/plugin_context_test.go` | 已审阅。100ms取消使3s任务<1s返回partial、WithResult非final、无ctx正常/坏类型fallback；§4。 | [详述](INFRASTRUCTURE.md) |
| `plugin/plugin_fallback_test.go` | 已审阅。refresh错误有cache返回旧结果nilerror，无cache保留error；§4。 | [详述](INFRASTRUCTURE.md) |
| `plugin/plugin_main_cache_key_test.go` | 已审阅。ext优先及坏值回退，六关键词并发主键/keyword隔离；§4。 | [详述](INFRASTRUCTURE.md) |
| `plugin/plugin_transport_proxy_test.go` | 已审阅。全部手写&http.Transport需Proxy字段、扫描非空断言；§2.1。 | [详述](INFRASTRUCTURE.md) |
| `plugin/publish_budget.go` | 已审阅。clamp(window-800ms,1.8s,8s)，默认3.2s；小window可高于window；§4。 | [详述](INFRASTRUCTURE.md) |
| `plugin/publish_budget_test.go` | 已审阅。默认预算小于4s、上下界、floor>=500ms；非所有自定义窗口严格小于；§4。 | [详述](INFRASTRUCTURE.md) |
| `plugin/qingying/qingying.go` | §3.30：详情H/状态/3regex | [详述](PROVIDERS_B.md) |
| `plugin/qiwei/qiwei.go` | §3.31：P/验证与通用读取H/状态 | [详述](PROVIDERS_B.md) |
| `plugin/qqpd/README.md` | §2.4：官方解析/缓存修复/错误与warning/隔离live | [详述](PROVIDERS_B.md) |
| `plugin/qqpd/api.go` | §2.4：完整227行，官方API/校验u64/缓存与并发解析 | [详述](PROVIDERS_B.md) |
| `plugin/qqpd/qqpd.go` | §2.4：索引/partial/发布截止/实际宽泛累积摘除/QR P+T/页面转义 | [详述](PROVIDERS_B.md) |
| `plugin/qqpd/qqpd_live_test.go` | §2.4：本地登录只读载入、临时落盘、两关键词非空 | [详述](PROVIDERS_B.md) |
| `plugin/qqpd/qqpd_test.go` | §2.4：完整360行，认证API/错误/修复/合法空/并发/删除不复活 | [详述](PROVIDERS_B.md) |
| `plugin/quark4k/quark4k.go` | §3.32：固定500ms、总3次/H/及时释放 | [详述](PROVIDERS_B.md) |
| `plugin/quarkres/quarkres.go` | 01 quarkres：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/quarksoo/quarksoo.go` | 02 quarksoo：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/quarksoo/quarksoo_retry_test.go` | 02 quarksoo：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/quarktv/quarktv.go` | 03 quarktv：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/quarktv/quarktv_retry_test.go` | 03 quarktv：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/qupanshe/qupanshe.go` | 04 qupanshe：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/qupansou/qupansou.go` | 05 qupansou：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/sdso/sdso.go` | 06 sdso：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/shandian/shandian.go` | 07 shandian：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/sopanya/sopanya.go` | 08 sopanya（完整新来源）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/sopanya/sopanya_test.go` | 08 sopanya（完整新来源）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/sousou/sousou.go` | 09 sousou：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/susu/susu.go` | 10 susu（Rust已有）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/thepiratebay/thepiratebay.go` | 11 thepiratebay：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/u3c3/u3c3.go` | 12 u3c3：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/wanou/wanou.go` | 13 wanou：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/weibo/weibo.go` | 14 weibo（Rust已有）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/woniu/README.md` | 15 woniu（完整新来源）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/woniu/woniu.go` | 15 woniu（完整新来源）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/woniu/woniu_live_test.go` | 15 woniu（完整新来源）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/woniu/woniu_test.go` | 15 woniu（完整新来源）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/wuji/wuji.go` | 16 wuji：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xb6v/xb6v.go` | 17 xb6v：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xdpan/xdpan.go` | 18 xdpan：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xdyh/xdyh.go` | 19 xdyh：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xiaoji/xiaoji.go` | 20 xiaoji：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xiaokupan/xiaokupan.go` | 21 xiaokupan（Go专属实现替换）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xiaoyu/xiaoyu.go` | 22 xiaoyu：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xiaozhang/xiaozhang.go` | 23 xiaozhang：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xinjuc/xinjuc.go` | 24 xinjuc：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xuexizhinan/xuexizhinan.go` | 25 xuexizhinan（纯格式化）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/xys/xys.go` | 26 xys：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yingso/yingso.go` | 27 yingso：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yingso/yingso_test.go` | 27 yingso：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yiove/yiove.go` | 28 yiove：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/ypfxw/ypfxw.go` | 29 ypfxw：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yuhuage/yuhuage.go` | 30 yuhuage：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yulinshufa/yulinshufa.go` | 31 yulinshufa（Rust已有）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yunso/yunso.go` | 32 yunso：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yunso/yunso_test.go` | 32 yunso：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/yunsou/yunsou.go` | 33 yunsou（Rust已有）：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/zhizhen/json结构分析.md` | 34 zhizhen：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/zhizhen/zhizhen.go` | 34 zhizhen：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/zlxapp/zlxapp.go` | 35 zlxapp：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `plugin/zxzj/zxzj.go` | 36 zxzj：协议/限额/测试及CLI处置已核查 | [详述](PROVIDERS_C.md) |
| `service/adaptive_concurrency.go` | 全280新增行；AIMD、floor、落盘、共享ceiling；§4.4 | [详述](SEARCH_CORE.md) |
| `service/adaptive_concurrency_test.go` | 全130行；初始17、增长/回退/恢复/重启损坏；§4.4 | [详述](SEARCH_CORE.md) |
| `service/backfill_loss_probe_test.go` | 全71行；50→13旧覆盖、合并保留对照；§5.2 | [详述](SEARCH_CORE.md) |
| `service/batch_search.go` | 全389行；完整度、错误、yield、最终fullTTL、1/3补齐；§4.1/5.1 | [详述](SEARCH_CORE.md) |
| `service/batch_search_test.go` | 全317行；TTL/补齐/提交序/失败类/全0产出；§4.1/5.1 | [详述](SEARCH_CORE.md) |
| `service/cache_integration.go` | 完整diff；可选落盘日志/格式；§5.2/6 | [详述](SEARCH_CORE.md) |
| `service/check_mobile_crypto.go` | 完整diff；只JSON适配器无密码学变化；§2.3 | [详述](SEARCH_CORE.md) |
| `service/check_service.go` | 完整diff；115顺序、16MiB、解压、缓存错误、预编译；§2 | [详述](SEARCH_CORE.md) |
| `service/check_service_test.go` | 全177行20案例；GET/host/path/share/receive和状态；§2.1 | [详述](SEARCH_CORE.md) |
| `service/liveness.go` | 全288行；20轮/5轮/分类40条/完整count；§4.6 | [详述](SEARCH_CORE.md) |
| `service/liveness_bench_test.go` | 全69行；71plugin+110channel性能基准；§4.6 | [详述](SEARCH_CORE.md) |
| `service/liveness_test.go` | 全172行；恢复/zero/快照/50截40；§4.6 | [详述](SEARCH_CORE.md) |
| `service/main_cache_lock.go` | 全33行；64条FNV32a锁；§5.2 | [详述](SEARCH_CORE.md) |
| `service/main_cache_lock_test.go` | 全104行；同key互斥、异key、释放、按内容哈希；§5.2 | [详述](SEARCH_CORE.md) |
| `service/main_cache_merge_concurrent_test.go` | 全267行；16源不丢/8+1/5+1/无锁对照；§5.2 | [详述](SEARCH_CORE.md) |
| `service/main_cache_write_guard_test.go` | 全103行；直接Set入口文件守卫及注入对照；§5.2 | [详述](SEARCH_CORE.md) |
| `service/outbound_gate.go` | 全153行；懒加载128闸/conc公式/波次deadline；§4.3/4.4 | [详述](SEARCH_CORE.md) |
| `service/plugin_timing.go` | 全204行；20样本/p50/2次老化/合并p90；§4.4 | [详述](SEARCH_CORE.md) |
| `service/probe_levels_test.go` | 全76行；仅冻结内存50遮盘54，不是修复；§5.2 | [详述](SEARCH_CORE.md) |
| `service/scheduling_test.go` | 全124行；conc/deadline/SJF/有界窗口/取消槽；§4.3/4.4 | [详述](SEARCH_CORE.md) |
| `service/search_service.go` | 全部591+/153-净diff；统一合并写、TG超时状态读限、正文过滤、backfill、任务ext、调度；§3/4/5 | [详述](SEARCH_CORE.md) |
| `service/search_service_test.go` | 全27行；TG简介关键词保留百度链接；§3.6 | [详述](SEARCH_CORE.md) |
| `service/tg_reachability.go` | 全240行；HEAD3s/2次失败/5min与60s/3s后台；§4.5 | [详述](SEARCH_CORE.md) |
| `service/tg_reachability_test.go` | 全257行；404/405可达、HEAD真实路径、transport、连击恢复；§4.5 | [详述](SEARCH_CORE.md) |
| `util/cache/cache_key.go` | 已审阅。TG parser v2、plugin ext、长度列表编码、4096旁路cache限额；§5.1。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/cache_key_collision_test.go` | 已审阅。逗号/元素边界/排序不变/ext差异全部断言；§5.1。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/delayed_batch_write_manager.go` | 已审阅。专用meta锁+atomic逐字段快照、CPU取预算；其余gofmt；§5.6。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/disk_atomic_write_test.go` | 已审阅。整体替换/失败保旧/无临时/并发完整值/重启扫tmp/30s日志累计，264行全部审阅；§5。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/disk_cache.go` | 已审阅。data+meta分别atomic、残留tmp清理、不预删、GetExpiry；Delete未return firstErr；§0/5。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/enhanced_ttl_refill_test.go` | 已审阅。150ms写两级→删内存→回填→250ms后必须miss；§5.4。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/enhanced_two_level_cache.go` | 已审阅。剩余TTL回填/metadata fallback now/异步失败日志30s限流；§5。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/flush_meta_race_test.go` | 已审阅。双写双读200ms-race与7项meta roundtrip；§5.6。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/global_buffer_manager.go` | 已审阅。全部净diff为gofmt/空行，未新增global buffer业务；§5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/memory_cache.go` | 已审阅。仅gofmt；§5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/serializer.go` | 已审阅。仅gofmt；未改GOB类型/协议；§5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/shard_count_test.go` | 已审阅。32/17→32、100000→64、1→4、0/-3/abc回退、CPU4→8；§5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/sharded_disk_cache.go` | 已审阅。CPU口径变GOMAXPROCS、仍[4,32]、新增GetExpiry；SHARD_COUNT不管磁盘；§0/5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/sharded_memory_cache.go` | 已审阅。SHARD_COUNT优先、[4,64]上取2幂，CPU来源变；其余格式；§5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/cache/utils.go` | 已审阅。仅gofmt；无序列化行为修改；§5.7。 | [详述](INFRASTRUCTURE.md) |
| `util/compression.go` | 已审阅。自写cache解压64MiB cap、不用比率；response gzip中间件仅格式；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `util/convert.go` | 已审阅。仅格式，不改变StringToInt无效返回0；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/cpu/cpu.go` | 已审阅。SchedulableCount=GOMAXPROCS(0)>0 else1，独立叶子包避导入环；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/cpu/cpu_test.go` | 已审阅。GOMAXPROCS2/3/1跟随、至少1；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/ext_digest.go` | 已审阅。忽略4内部key、key排序长度编码MD5、noext；%v不保类型；§0/5.1。 | [详述](INFRASTRUCTURE.md) |
| `util/ext_digest_test.go` | 已审阅。pages差异/顺序无关/边界无碰撞/忽略ctx-refresh-mainkey/nil空；§5.1。 | [详述](INFRASTRUCTURE.md) |
| `util/http_body.go` | 已审阅。16MiB raw、64MiB解压、100:1+400KiB floor、limit+1报错而非EOF；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `util/http_body_test.go` | 已审阅。精确边界/无限reader/nil/0limit/bomb/正常/未知压缩/streaming全部212行断言已读；§2.3。 | [详述](INFRASTRUCTURE.md) |
| `util/http_util.go` | 已审阅。统一代理/NO_PROXY/默认与手写Transport/池/脱敏/TGpathescape/有界Fetch；§2。 | [详述](INFRASTRUCTURE.md) |
| `util/http_util_test.go` | 已审阅。配置SOCKS/NO_PROXY/非法scheme/标准fallback/池/DefaultTransport/customTransport/live opt-in；全部250行已读；§2。 | [详述](INFRASTRUCTURE.md) |
| `util/json/adapter.go` | 已审阅。sonic Decoder/Encoder别名+RawMessage/Number标准别名和构造器；Rust serde语义适配；§9。 | [详述](INFRASTRUCTURE.md) |
| `util/json/adapter_test.go` | 已审阅。编解码roundtrip、RawMessage对象/字段、12345678901234567890 Number精度/map类型；§9。 | [详述](INFRASTRUCTURE.md) |
| `util/json/json.go` | 已审阅。仅末尾newline，无sonic配置变更；§9。 | [详述](INFRASTRUCTURE.md) |
| `util/json/no_stdlib_json_test.go` | 已审阅。唯一adapter例外；全仓不能直接encoding/json；递归扫描并排除自身与adapter；§9。 | [详述](INFRASTRUCTURE.md) |
| `util/mask_proxy_test.go` | 已审阅。HTTP/SOCKS凭据不露、坏地址不回显、恶意channel转义、合法名不变；§2.2。 | [详述](INFRASTRUCTURE.md) |
| `util/memlimit/memlimit.go` | 已审阅。v2/v1配额9/10、GOMEMLIMIT优先、16MiB/1TiB sanity，不移植Go堆控制；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/memlimit/memlimit_test.go` | 已审阅。1GiB90%、显式512MiB不覆盖、无限/坏值/8MiB、不存在路径与第二路径256MiB；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/parser_contract_test.go` | 全235行；DOM文本/标题/4种URL密码/三状态/无链接OK；§3.3/3.4/3.5 | [详述](SEARCH_CORE.md) |
| `util/parser_real_codes_fixture_test.go` | 全53行；LEO20/502/19、只无回归非修复证明；§3.7 | [详述](SEARCH_CORE.md) |
| `util/parser_real_fixture_test.go` | 全105行；Aliyun20、域名16/4、首条、channel ID、无分页；§3.7 | [详述](SEARCH_CORE.md) |
| `util/parser_types_probe_test.go` | 全86行；6类型顺序、各局部码、Aliyun首码fallback、按钮；§3.2/3.4 | [详述](SEARCH_CORE.md) |
| `util/parser_util.go` | 完整459+/385-净diff；解析拆分、状态、就近码、wrap按钮、归集、DOM/标题；§3 | [详述](SEARCH_CORE.md) |
| `util/parser_util_test.go` | 全126行；11支持按钮链接/无密码百度/日期头work_title；§3.1/3.2/3.3 | [详述](SEARCH_CORE.md) |
| `util/password_near_test.go` | 全118行；旧首码对照、各码、换行/截下链/锚/长上下文/fallback/UTF8；§3.4 | [详述](SEARCH_CORE.md) |
| `util/pool/object_pool.go` | 已审阅。仅newline无对象重置语义改变；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/pool/worker_pool.go` | 已审阅。Task ctx、可取消send、panic隔离、超时及时partial与后台回收；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/pool/worker_pool_test.go` | 已审阅。2s/200ms、ctx deadline、全完成、结果队列满、20轮不泄漏、panic；§6。 | [详述](INFRASTRUCTURE.md) |
| `util/regex_util.go` | 完整28+/9-净diff；near4~6、预编译4URL正则、UC123；§3.2/3.4/2.2 | [详述](SEARCH_CORE.md) |
| `util/retry.go` | 已审阅。Attempts含首次/Abort/DelayFunc/±25%/WaitFunc取消/原错误链；§3。 | [详述](INFRASTRUCTURE.md) |
| `util/retry_test.go` | 已审阅。341行完整审阅：成功/次数/链/终次无sleep/固定指数cap+jitter/custom/abort/ctx等待；§3。 | [详述](INFRASTRUCTURE.md) |
| `util/testdata/tg_channel_leoziyuan_with_codes.html` | 新增真实大fixture按许可聚焦结构：head、wrap/data-post、正文/多链接br列表、图片/时间；完整对应断言已读；§3.7 | [详述](SEARCH_CORE.md) |
| `util/testdata/tg_channel_shareAliyun_real.html` | 新增真实大fixture按许可聚焦结构：频道元数据、wrap、正文阿里链接/实体、bubble照片/time/tag；完整对应断言已读；§3.7 | [详述](SEARCH_CORE.md) |
