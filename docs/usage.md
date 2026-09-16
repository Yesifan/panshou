# 使用说明

## 搜索

```text
pansou search <QUERY>
  --source <all|tg|provider>
  --provider <NAME>        可重复
  --channel <NAME>         可重复
  --cloud <TYPE>           可重复
  --include <WORD>         可重复
  --exclude <WORD>         可重复
  --jobs <N>
  --timeout <SECONDS>
  --all-timeout <SECONDS>
  --proxy <URL>
  --format <table|json>
  --no-check
  --no-progress
  --verbose
  --quiet
```

示例：

```bash
# 只搜索 Telegram
pansou search "仙逆" --source tg --channel tgsearchers3

# 只搜索指定 provider，并筛选夸克链接
pansou search "仙逆" --source provider \
  --provider meitizy --provider cyg \
  --cloud quark

# 关键词过滤、并发和超时
pansou search "仙逆" --include 4K --exclude 预告 \
  --jobs 4 --timeout 20 --all-timeout 600

# 默认检测支持的网盘链接并只输出 ok；Magnet/ED2K 原样保留
pansou search "仙逆" --format json
pansou search "仙逆" --no-check --format json
```

默认搜索独立频道清单中启用的 Telegram 频道、15 个无登录 provider，以及已经配置且登录有效的
有状态 provider。TG 与 provider 共享默认 8 个并发名额，每个来源超时默认 30 秒，
取得名额后开始计时，不含排队。`--all-timeout` 默认 600 秒，包含排队与来源请求；
到期取消剩余搜索并保留已收到的结果。初始化和链接检测不计入该时限。Provider 的登录和配置参见
[Provider 配置](providers.md)。

资源命名可能不规范，建议使用简短的核心关键词；关键词过长可能降低搜索效果。
TG 每频道只请求一次公开搜索页，不翻页、不自动重试；解析正文和按钮中的资源链接，
正文命中完整关键词时保留该消息的链接。不提供中文自动分词或模糊搜索。

### 进度与最终输出

交互式终端默认在 stderr 显示动态进度条，分别反映来源搜索和链接检测进度；
`--no-progress` 可隐藏进度条。stderr 不是 TTY 时自动隐藏进度条；进度始终不写入 stdout，
不会污染重定向或管道中的机器可读结果。诊断信息仍写入 stderr；`--quiet` 同时隐藏进度并
压低非必要日志。

所有来源结束且链接检测完成后，stdout 一次性输出最终结果。默认 `table` 按稳定顺序展示
去重后的链接，并在末尾总结有效链接/候选链接数、来源完成/失败/取消/未开始数量和耗时。
`--format json` 输出单个格式化 JSON 文档，包含最终链接、来源错误、搜索摘要及检测摘要，
适合脚本读取。搜索和独立 `check` 命令均只支持 `table` 与 `json`。

搜索默认只在受支持的网盘链接检测状态为 `ok` 时保留。`bad`、`locked`、
`uncertain` 和 `unsupported` 均会被过滤。Magnet 和 ED2K 不适用网盘链接检测，
因此不会进入检测队列，并会原样保留且不附加 `check` 状态。`--no-check` 跳过检测并输出
全部搜索结果，末尾摘要标记为 `unchecked`。
搜索总时限结束后，已接收链接的检测仍可继续，因此命令总耗时可能超过 `--all-timeout`。

`pansou help` 根据本地配置显示来源数、并发和搜索阶段保守等待估算：

```text
min(ceil((provider 数 + channel 数) / jobs) × timeout, all-timeout)
```

例如 15 个 provider 加 128 个频道，8 并发、30 秒单来源超时，估算约 9 分钟。
该值不包含初始化、处理开销或检测，不会为计算帮助信息发送网络请求。

## Telegram 频道管理

```bash
pansou channel list
pansou channel path
pansou channel add @foo https://t.me/bar
pansou channel add baz --disabled
pansou channel add trusted_name --no-validate
pansou channel enable foo bar
pansou channel disable foo bar
pansou channel enable --all
pansou channel disable --all
pansou channel remove foo bar
pansou channel catalog
pansou channel import --builtin
pansou channel import --builtin --disable
pansou channel import ./channels.txt
pansou channel import https://example.com/channels.txt
```

名称、`@名称`、`https://t.me/名称` 和 `https://t.me/s/名称` 统一规范化并去重。
`channel add` 默认联网确认目标具有公开用户名和 `https://t.me/s/<name>` 消息预览；
bot、私有邀请、不存在或没有公开消息页的目标会被拒绝。网络失败、Telegram 限流
或未知页面会报“无法确认”而不会冒充无效频道；`--no-validate` 可显式跳过联网校验。
添加默认启用；已存在的条目保留原状态。最多同时启用 128 个频道，
可保存更多禁用频道；批量操作超限时不会只应用一部分。

导入只新增缺失频道，新增项默认启用，`--disable` 可改为禁用；已有频道保留原状态。
`enable --all` 和 `disable --all` 分别启用、禁用所有已保存频道，不能与频道名称混用。
导入或全部启用超过 128 上限时整次拒绝，不部分修改；可先禁用部分频道或使用 `--disable` 导入。
输入为 UTF-8 文本，每行一个名称或频道 URL，
支持空行和 `#` 开头的整行注释。非法条目会报告行号并使整次导入失败。
`catalog` 是当前安装版本内置的候选清单，不代表所有频道均经过在线验证；升级不会自动
导入清单或恢复已删除频道。

CLI 帮助统一使用英语，各级子命令和选项均提供用途说明。若已保存的频道尚未包含完整
内置清单，帮助末尾会提示 `pansou channel import --builtin`，并说明新增项默认启用、
可用 `--disable` 仅保存。该提示只检查当前清单，不记录历史导入状态，也不会自动修改频道配置。

持久化频道统一保存在 `pansou channel path` 指向的 `channels.toml`：

```toml
version = 1

[[channels]]
name = "foo"
enabled = true
```

文件不存在时使用初始默认频道 `tgsearchers3`；显式空清单或全部禁用表示不搜索任何默认频道。
本次频道选择优先级为 `--channel` > `PANSOU_CHANNELS` > `CHANNELS` > 独立清单启用项。
各层整体覆盖；本次显式选择去重后也不能超过 128。搜索过程确认某个已保存的启用频道
是 bot、已删除或没有公开消息归档时，会自动将其设为 disabled 并在 stderr 提示。
限流、超时、网络/Telegram 服务错误、未知页面和关键词无结果不会触发禁用；CLI 或
环境变量临时选择且未保存的频道也不会写入清单。

旧 `config.toml` 的 `search.channels` 已废弃，忽略其值且不自动迁移。
看到提示后，请通过 `channel add` 或 `channel import` 重新设置，再删除旧字段。

## 更新与迁移

```bash
pansou update                  # 安装最新稳定版
pansou update --check          # 只检查，不安装
pansou update --version v0.2.0 # 指定 Release 版本
```

更新检查项目 GitHub Releases，匹配平台安装包并校验 SHA-256，备份现有程序后替换当前安装。
下载、校验或兼容性预检失败不会替换旧程序。不可写目录或包管理器管理的安装会给出处理提示，
不会自动提权。普通搜索不联网检查更新。Windows 的替换在原进程退出后由辅助进程完成。

新版按数据格式版本检查迁移，确定性的格式变更在备份后转换；手动替换二进制升级也会检查。
需要用户操作的事项会给出具体提示，未解决时在相关命令中继续提醒。本次旧频道字段不自动转换。
使用 JSONL 的脚本需要改用单个 JSON 文档。

若程序已更新但迁移失败，命令会明确报告，保留数据和备份。数据格式变化后不能仅恢复旧程序
就假定完成回滚；应先确认旧版的数据兼容性。`help`、版本显示和 `update` 可用于修复配置问题。

## 链接检测

支持 Aliyun、Quark、UC、Baidu、Tianyi、123、Xunlei、115 和 Mobile 共 9 种
checker。

```bash
# 单个或多个 URL
pansou check URL1 URL2 URL3

# 从 stdin 读取
cat links.txt | pansou check --stdin

# 强制指定类型；password 只允许用于单个 URL
pansou check URL --type baidu --password abcd

# 绕过缓存并更新，或完全禁用缓存
pansou check URL --refresh
pansou check URL --no-cache

# bad 或 locked 时返回退出码 10
pansou check URL --fail-invalid --format json
```

```text
pansou check [URL]...
  --stdin
  --type <cloud>
  --password <pwd>
  --jobs <N>
  --timeout <SECONDS>
  --proxy <URL>
  --refresh
  --no-cache
  --format <table|json>
  --fail-invalid
```

检测状态为：`ok`（有效）、`bad`（失效）、`locked`（需要提取码或密码错误）、
`unsupported`（暂不支持）和 `uncertain`（网络或协议错误，无法确认）。重复的规范化 URL
在一次运行中只请求一次，输出仍保持输入顺序。

检测缓存持久化在平台缓存目录的 `pansou/check.redb`。缓存按网盘类型、规范化 URL
和代理的不可逆摘要隔离，不会把完整代理 URL 写入可读 key。`--refresh` 不读取旧值但会
覆盖缓存；`--no-cache` 既不读也不写。搜索结果本身不缓存。

## 支持的链接类型

搜索和解析支持：Baidu、Aliyun、Quark、Guangya、Tianyi、UC、Mobile、115、PikPak、
Xunlei、123、magnet、ed2k 和 others。链接检测只覆盖上一节列出的 9 种类型。

## 代理

`search` 和 `check` 均支持 HTTP、HTTPS 和 SOCKS5 代理：

```bash
pansou search "仙逆" --proxy http://127.0.0.1:7890
pansou check URL --proxy socks5://127.0.0.1:1080
```

也可以在配置文件中设置 `network.proxy`，或使用 `PANSOU_PROXY`。代理不会自动启用，
必须由用户明确配置。

## 配置

查看生效配置及准确路径：

```bash
pansou config show
pansou config path
```

示例 `config.toml`：

```toml
[network]
proxy = ""
timeout_secs = 30

[search]
jobs = 8
all_timeout_secs = 600
providers = [
  "pansearch", "yunsou", "djgou", "hdmoli", "meitizy",
  "yulinshufa", "clxiong", "jsnoteclub", "duanjuw", "dyyj",
  "jupansou", "cldi", "clmao", "cyg", "susu",
]

[check]
enabled_cache = true
jobs = 8
```

配置优先级为：命令行参数 > `PANSOU_*` 环境变量 > `config.toml` > 默认值。主要环境
变量为 `PANSOU_PROXY`、`PANSOU_ALL_TIMEOUT_SECS`、`PANSOU_CHANNELS` 和 `PANSOU_PROVIDERS`；旧的 `PROXY`、
`HTTP_PROXY`、`HTTPS_PROXY`、`CHANNELS` 和 `ENABLED_PLUGINS` 仅作为 fallback。

PanSou 使用操作系统标准目录，而不是当前工作目录：

| 内容 | Linux 默认位置 | macOS / Windows |
| --- | --- | --- |
| 配置 | `$XDG_CONFIG_HOME/pansou/config.toml` | 由系统配置目录决定 |
| 频道 | `$XDG_CONFIG_HOME/pansou/channels.toml` | 与配置同目录 |
| Provider 状态 | `$XDG_STATE_HOME/pansou/providers/` | 由系统状态/本地数据目录决定 |
| Check 缓存 | `$XDG_CACHE_HOME/pansou/check.redb` | 由系统缓存目录决定 |

若 XDG 变量未设置，会使用对应的用户默认目录。跨平台的准确路径始终以
`pansou config path` 为准。

## Exit code

| Code | 含义 |
| ---: | --- |
| 0 | 成功，或部分搜索来源失败但至少一个来源成功 |
| 2 | CLI 参数或配置错误 |
| 3 | 所有选中的搜索来源均失败 |
| 4 | 显式选择的 provider 需要登录 |
| 5 | 全局网络或初始化错误 |
| 10 | `check --fail-invalid` 发现 `bad` 或 `locked` |
| 124 | 搜索阶段达到 `--all-timeout`，即使已有部分结果 |
| 130 | 用户按 Ctrl-C 中断搜索或检测 |
