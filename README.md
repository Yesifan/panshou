# PanSou CLI

PanSou 是一个以 Rust 编写的本地网盘资源搜索与链接检测工具。它从 Telegram 频道和 19 个 provider 并发检索内容，合并、过滤、排序并去重链接，也可以独立检测 9 种网盘分享链接的有效性。

PanSou 以单次 CLI 进程运行，不需要后台服务。

实现模块、数据流、并发模型与安全边界见
[整体架构说明](docs/ARCHITECTURE.md)。

## 快速开始

```bash
pansou search "仙逆"

pansou search "仙逆" --cloud quark

pansou search "仙逆" \
  --provider meitizy \
  --provider pansearch

pansou check https://pan.quark.cn/s/xxxx
```

默认搜索配置中的 Telegram 频道、15 个无登录 provider，以及已经配置且登录有效的有状态 provider。默认并发数为 8，每个来源的超时为 30 秒，输出格式为表格。单个来源失败不会中断其他来源；错误写入 stderr，结果写入 stdout。

## 安装

从 GitHub Release 下载适合平台的压缩包，解压后将 `pansou`（Windows 为 `pansou.exe`）放入 `PATH`：

- `pansou-linux-x86_64.tar.gz`
- `pansou-linux-aarch64.tar.gz`
- `pansou-macos-x86_64.tar.gz`
- `pansou-macos-aarch64.tar.gz`
- `pansou-windows-x86_64.zip`

可使用同一 Release 中的 `SHA256SUMS` 校验下载文件。

也可以从源码构建：

```bash
git clone https://github.com/fish2018/pansou.git
cd pansou
cargo build --release
./target/release/pansou --help
```

项目使用 stable Rust、edition 2024 和 rustls；无需安装 OpenSSL。

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

JSON 使用稳定结果 envelope，包含 `query`、`total_results`、`total_links`、`results`、`links_by_type`、`source_errors` 和 `duration_ms`。JSONL 适合流式 shell 处理。`--quiet` 关闭日志但仍输出数据；`--verbose` 输出来源耗时、重试、challenge 和 cache hit 等诊断信息到 stderr。

## Provider

PanSou v1 包含 19 个 provider：

| Provider | 登录 | 说明 |
| --- | --- | --- |
| pansearch | 无 | Next.js 搜索与真实链接解析 |
| yunsou | 无 | HTML 搜索与分页 |
| djgou | 无 | Z-Blog 搜索与详情页 |
| hdmoli | 无 | HTML 搜索 |
| meitizy | 无 | JSON API |
| yulinshufa | 无 | GBK 页面 |
| clxiong | 无 | 磁力链接搜索 |
| jsnoteclub | 无 | Ghost 页面 |
| duanjuw | 无 | 夸克、百度资源 |
| dyyj | 无 | Flarum API |
| jupansou | 无 | SSE 搜索与链接转换 |
| cldi | 无 | 磁力链接搜索 |
| clmao | 无 | Base64 页面与磁力链接 |
| cyg | 无 | WordPress API |
| susu | 无 | 多层资源解析 |
| qqpd | 扫码 | 多账号、频道配置 |
| weibo | 扫码 | 多账号、目标用户配置 |
| gying | 密码 | 多账号、三种 PoW challenge |
| panlian | 密码 | 多账号、token/jump 解析 |

管理命令：

```bash
pansou provider list
pansou provider profiles qqpd
pansou provider status qqpd
pansou provider status qqpd --profile main
pansou provider logout qqpd --profile main
```

普通搜索会静默跳过尚未配置 profile 的有状态 provider；显式选择该 provider 时会返回需要登录的错误。

### QQPD

```bash
pansou provider login qqpd --profile main

pansou provider configure qqpd --profile main \
  --channels pd97631607,languan8K115

# 也可以重复指定
pansou provider configure qqpd --profile main \
  --channel pd97631607 --channel languan8K115
```

登录二维码优先绘制在终端；终端不支持时会生成临时 PNG。频道会被规范化和去重，并缓存 `guild_id`。QQPD 在搜索前按需执行 session keepalive。

### Weibo

```bash
pansou provider login weibo --profile main

pansou provider configure weibo --profile main \
  --users 1234567890,2345678901
```

`--user` 可以重复使用，也可以传入 `https://weibo.com/u/1234567890`；保存时会规范化为数字用户 ID。

### Gying 和 Panlian

```bash
pansou provider configure gying \
  --base-url https://www.xn--wcv59z.com
pansou provider login gying --profile main --username USERNAME

pansou provider login panlian --profile main --username USERNAME
pansou provider configure panlian --blocked-cloud pikpak --blocked-cloud others
```

密码默认通过隐藏输入的 TTY 提示读取。自动化场景使用 `--password-stdin`：

```bash
printf '%s\n' "$PANSOU_LOGIN_PASSWORD" | \
  pansou provider login gying --profile main --username USERNAME --password-stdin
```

PanSou 不提供明文 `--password` 参数，以免密码进入 shell history。需要保存账号密码时使用 `--remember-credentials`，并先设置 `PANSOU_STATE_KEY`。

## 链接检测

支持 Aliyun、Quark、UC、Baidu、Tianyi、123、Xunlei、115 和 Mobile 共 9 种 checker。

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

完整参数：

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

检测状态为：`ok`（有效）、`bad`（失效）、`locked`（需要提取码或密码错误）、`unsupported`（暂不支持）和 `uncertain`（网络或协议错误，无法确认）。重复的规范化 URL 在一次运行中只请求一次，输出仍保持输入顺序。

检测缓存持久化在平台缓存目录的 `pansou/check.redb`。缓存按网盘类型、规范化 URL 和代理的不可逆摘要隔离，不会把完整代理 URL 写入可读 key。`--refresh` 不读取旧值但会覆盖缓存；`--no-cache` 既不读也不写。搜索结果本身不缓存。

## 支持的链接类型

搜索和解析支持：Baidu、Aliyun、Quark、Guangya、Tianyi、UC、Mobile、115、PikPak、Xunlei、123、magnet、ed2k 和 others。链接检测只覆盖上一节列出的 9 种类型。

## 代理

`search` 和 `check` 均支持 HTTP、HTTPS 和 SOCKS5 代理：

```bash
pansou search "仙逆" --proxy http://127.0.0.1:7890
pansou check URL --proxy socks5://127.0.0.1:1080
```

也可以在配置文件中设置 `network.proxy`，或使用 `PANSOU_PROXY`。代理不会自动启用，必须由用户明确配置。

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

[providers.gying]
base_url = "https://www.xn--wcv59z.com"

[providers.panlian]
blocked_clouds = []
```

配置优先级为：命令行参数 > `PANSOU_*` 环境变量 > `config.toml` > 默认值。主要环境变量为 `PANSOU_PROXY`、`PANSOU_CHANNELS` 和 `PANSOU_PROVIDERS`；旧的 `PROXY`、`HTTP_PROXY`、`HTTPS_PROXY`、`CHANNELS` 和 `ENABLED_PLUGINS` 仅作为 fallback。

PanSou 使用操作系统标准目录，而不是当前工作目录：

| 内容 | Linux 默认位置 | macOS / Windows |
| --- | --- | --- |
| 配置 | `$XDG_CONFIG_HOME/pansou/config.toml` | 由系统配置目录决定 |
| Provider 状态 | `$XDG_STATE_HOME/pansou/providers/` | 由系统状态/本地数据目录决定 |
| Check 缓存 | `$XDG_CACHE_HOME/pansou/check.redb` | 由系统缓存目录决定 |

若 XDG 变量未设置，会使用对应的用户默认目录。跨平台的准确路径始终以 `pansou config path` 为准。

## 状态与凭据安全

每个有状态 provider 的每个 profile 使用独立状态文件和 HTTP Cookie session。状态采用临时文件加 rename 的原子写入；Unix 私有状态文件权限为 `0600`，目录为 `0700`。日志会脱敏 Cookie、password、Authorization、token 和二维码凭据。

设置 `PANSOU_STATE_KEY` 后，Cookie、token 和用户明确要求记住的密码使用 AES-256-GCM 加密。未设置密钥时，Cookie 可以写入用户私有状态文件，但密码绝不明文持久化；此时 `--remember-credentials` 会被拒绝。Cookie 失效且没有保存凭据时，需要重新运行 `provider login`。

请使用高熵密钥并通过进程环境安全注入，不要将其提交到仓库：

```bash
export PANSOU_STATE_KEY='replace-with-a-long-random-secret'
```

## 开发与验证

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

依赖第三方站点的 live tests 默认忽略，避免站点变化、Cloudflare、地域或限流导致 CI 不稳定。显式运行示例：

```bash
PANSOU_LIVE_TESTS=1 \
  cargo test provider_pansearch_live -- --ignored
```

## Exit code

| Code | 含义 |
| ---: | --- |
| 0 | 成功，或部分搜索来源失败但至少一个来源成功 |
| 2 | CLI 参数或配置错误 |
| 3 | 所有选中的搜索来源均失败 |
| 4 | 显式选择的 provider 需要登录 |
| 5 | 全局网络或初始化错误 |
| 10 | `check --fail-invalid` 发现 `bad` 或 `locked` |

## License

MIT
