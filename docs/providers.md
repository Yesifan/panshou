# Provider 配置

PanSou 包含 19 个 provider：

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

## 管理命令

```bash
pansou provider list
pansou provider profiles qqpd
pansou provider status qqpd
pansou provider status qqpd --profile main
pansou provider logout qqpd --profile main
```

普通搜索会静默跳过尚未配置 profile 的有状态 provider；显式选择该 provider 时会返回
需要登录的错误。

## QQPD

```bash
pansou provider login qqpd --profile main

pansou provider configure qqpd --profile main \
  --channels pd97631607,languan8K115

# 也可以重复指定
pansou provider configure qqpd --profile main \
  --channel pd97631607 --channel languan8K115
```

登录二维码会在交互式终端中使用 Unicode 半块字符绘制。请使用手机 QQ 扫码并在手机端确认；
可通过 SSH 直接扫码，非交互输出或二维码解析失败时才会生成临时 PNG。二维码过期后命令会
退出，并提示重新运行登录命令。

登录成功后必须至少配置一个频道，该 profile 才会参与搜索。打开 QQ 频道网页版并复制
`https://pd.qq.com/g/<频道ID>` 地址；`--channel` 既接受频道 ID，也接受完整 URL。频道会被
规范化和去重，并缓存 `guild_id`。QQPD 在搜索前按需执行 session keepalive。

## Weibo

`profile` 是一套独立的微博登录会话（Cookie）的本地名称，并不是目标微博用户。可以使用
多个 profile 登录不同的微博账号；搜索时会使用所有已登录且已配置目标用户的 profile。每个
目标用户只会搜索一次；若多个 profile 配置了同一目标用户，会由其中一个 profile 执行搜索。

```bash
# 在终端显示二维码后，使用手机微博 App 扫码，并在手机端确认登录
pansou provider login weibo --profile main

# 添加一个或多个需要搜索的目标微博用户
pansou provider configure weibo add --profile main \
  --user 1234567890 --user https://weibo.com/u/2345678901

# 查看或移除当前 profile 的目标用户
pansou provider configure weibo list --profile main
pansou provider configure weibo del --profile main --user 1234567890
```

二维码会在交互式终端中显示；可通过 SSH 直接扫码。请使用手机微博 App 扫描，并在手机端
确认登录。二维码过期后，登录命令会退出并提示二维码已过期；重新运行
`pansou provider login weibo --profile main` 获取新的二维码。可用 `Ctrl-C` 取消等待。

登录成功后必须至少添加一个目标用户，该 profile 才会参与搜索。`add` 会合并并去重，适合
重复执行；`del` 只移除指定用户；`list` 显示当前已配置的目标用户。删除最后一个目标用户
不会注销该 profile，但它不会参与搜索。

`--user` 接受数字微博 UID，也接受包含数字 UID 的微博主页 URL。打开目标用户的微博主页，
从地址栏复制 URL 即可：`https://weibo.com/u/1234567890` 中的 UID 是 `1234567890`，可以
直接将该 URL 传给 `--user`，也可以只传该数字。昵称和无法从 URL 确定数字 UID 的自定义短链接
不能用于配置目标用户。

Pansou 会在目标用户的关键词搜索结果中，从微博正文、附带网页以及（前两者没有下载链接时）
第一条评论兜底中查找受支持的网盘、磁力或 ed2k 链接；没有下载链接的普通微博不会返回。它不会
遍历全部评论，也不会把任意普通网页链接作为结果输出。

## Gying 和 Panlian

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

PanSou 不提供明文 `--password` 参数，以免密码进入 shell history。需要保存账号密码时使用
`--remember-credentials`，并先设置 `PANSOU_STATE_KEY`。

Gying 和 Panlian 的配置示例：

```toml
[providers.gying]
base_url = "https://www.xn--wcv59z.com"

[providers.panlian]
blocked_clouds = []
```

## 状态与凭据安全

每个有状态 provider 的每个 profile 使用独立状态文件和 HTTP Cookie session。状态采用
临时文件加 rename 的原子写入；Unix 私有状态文件权限为 `0600`，目录为 `0700`。日志会
脱敏 Cookie、password、Authorization、token 和二维码凭据。

设置 `PANSOU_STATE_KEY` 后，Cookie、token 和用户明确要求记住的密码使用 AES-256-GCM
加密。未设置密钥时，Cookie 可以写入用户私有状态文件，但密码绝不明文持久化；此时
`--remember-credentials` 会被拒绝。Cookie 失效且没有保存凭据时，需要重新运行
`provider login`。

请使用高熵密钥并通过进程环境安全注入，不要将其提交到仓库：

```bash
export PANSOU_STATE_KEY='replace-with-a-long-random-secret'
```
