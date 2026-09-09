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
  --proxy <URL>
  --format <table|json|jsonl>
  --check
  --valid-only
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
  --jobs 4 --timeout 20

# 搜索后检测链接；valid-only 会隐式启用检测，只保留 ok
pansou search "仙逆" --check --format json
pansou search "仙逆" --valid-only --format jsonl
```

默认搜索配置中的 Telegram 频道、15 个无登录 provider，以及已经配置且登录有效的
有状态 provider。默认并发数为 8，每个来源的超时为 30 秒，输出格式为表格。单个来源
失败不会中断其他来源；错误写入 stderr，结果写入 stdout。Provider 的登录和配置参见
[Provider 配置](providers.md)。

JSON 使用稳定结果 envelope，包含 `query`、`total_results`、`total_links`、`results`、
`links_by_type`、`source_errors` 和 `duration_ms`。JSONL 适合流式 shell 处理。
`--quiet` 关闭日志但仍输出数据；`--verbose` 输出来源耗时、重试、challenge 和 cache hit
等诊断信息到 stderr。

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
  --format <table|json|jsonl>
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
channels = ["tgsearchers3"]
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
变量为 `PANSOU_PROXY`、`PANSOU_CHANNELS` 和 `PANSOU_PROVIDERS`；旧的 `PROXY`、
`HTTP_PROXY`、`HTTPS_PROXY`、`CHANNELS` 和 `ENABLED_PLUGINS` 仅作为 fallback。

PanSou 使用操作系统标准目录，而不是当前工作目录：

| 内容 | Linux 默认位置 | macOS / Windows |
| --- | --- | --- |
| 配置 | `$XDG_CONFIG_HOME/pansou/config.toml` | 由系统配置目录决定 |
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
