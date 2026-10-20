# Provider 配置

当前注册 106 个来源（101 个无状态、5 个有状态）。固定上游默认集合与已接入来源的交集为 66 个；
全新安装尚未登录时，其中 62 个参与默认搜索，其余 4 个在就绪后加入。Gying 不属于默认集合。
完整来源列表与就绪状态以 `pansou provider list` 为准。下表列出原有来源及需要登录的来源；新增无登录来源也通过同一命令发现和显式选择。

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
| panlian | 密码、图形验证码，必要时核对绑定邮箱 | 多账号、现代 videos/ticket/open 搜索 |
| woniu | 密码 | 独立 Cookie、可配置站点、电影及分集网盘链接 |

`zhizhen` 的固定目标站点是 `http://www.mihdr.top`，使用明文 HTTP；不自动切换到 HTTPS 或轮询旧域名。
来源可发现、默认启用、已登录和第三方站点当前可用是不同概念；默认测试不作线上可用性保证。

## 管理命令

```bash
pansou provider list
pansou provider profiles qqpd
pansou provider status qqpd
pansou provider status qqpd --profile main
pansou provider logout qqpd --profile main
```

默认来源采用固定目标上游默认集合与 CLI 已接入来源的交集；其中需要登录的来源未就绪时静默跳过。
登录不会把非默认来源自动加入默认搜索；Gying、Jupansou、Kpkuang 等仍可通过 `--provider` 或显式配置选择。
已有显式来源清单（包括空清单）不追加、不删除；未显式选择的升级用户默认搜索范围和请求量可能扩大。
显式选择未就绪的有状态 provider 时会返回需要登录的错误。

## Miaoso

`miaoso` 可通过环境变量 `PANSOU_MIAOSO_TOKEN` 接收用户自己的 `satoken`，仅在本次进程中使用，
不写入配置或状态文件。不要把 token 放入命令行参数或提交到仓库；程序不内置上游源码中的会话凭据。
未设置或为空时不发送 `satoken`，站点是否允许匿名搜索由第三方服务决定。

## QQPD

```bash
pansou provider login qqpd --profile main

pansou provider configure qqpd add --profile main \
  --channels pd97631607,languan8K115

# 也可以重复指定
pansou provider configure qqpd add --profile main \
  --channel pd97631607 --channel languan8K115

pansou provider configure qqpd list --profile main
pansou provider configure qqpd del --profile main --channel pd97631607
```

登录二维码会在交互式终端中使用 Unicode 半块字符绘制。请使用手机 QQ 扫码并在手机端确认；
可通过 SSH 直接扫码，非交互输出或二维码解析失败时才会生成临时 PNG。二维码过期后命令会
退出，并提示重新运行登录命令。

登录成功后必须至少配置一个频道，该 profile 才会参与搜索。`add` 会合并并去重，`del` 移除
指定频道，`list` 显示当前频道；删除最后一个频道不会注销 profile，但它不会参与搜索。打开
QQ 频道网页版并复制
`https://pd.qq.com/g/<频道ID>` 地址；`--channel` 既接受频道 ID，也接受完整 URL。频道会被
规范化和去重，并通过官方 gotrpc 解析及持久化 `guild_id`；旧索引在搜索前按需修复。
仅明确“频道不存在”的 code10003 按 profile 连续两次隔离，成功复位；传输、权限和未知错误不删除频道。
重新执行该 profile 的 `configure qqpd add` 可恢复隔离目标。QQPD 在搜索前按需执行 session keepalive。

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

## Gying

```bash
pansou provider configure gying \
  --base-url https://www.xn--wcv59z.com
pansou provider login gying --profile main --username USERNAME
```

Gying 登录成功后即可搜索。默认使用当前配置的 `base_url`；只有服务地址变化或使用镜像时才
需要运行 `provider configure gying`。登录过程中需要完成服务端 challenge，可能会等待片刻。

密码默认通过隐藏输入的 TTY 提示读取。自动化场景使用 `--password-stdin`：

```bash
printf '%s\n' "$PANSOU_LOGIN_PASSWORD" | \
  pansou provider login gying --profile main --username USERNAME --password-stdin
```

PanSou 不提供明文 `--password` 参数，以免密码进入 shell history。使用
`--remember-credentials` 时必须先设置 `PANSOU_STATE_KEY`；凭据会加密保存，并在 Cookie
失效时用于一次自动重新登录。不保存凭据时，需要手动重新运行 login。

```toml
[providers.gying]
base_url = "https://www.xn--wcv59z.com"
```

## Panlian

```bash
pansou provider login panlian --profile main --username USERNAME
pansou provider configure panlian --blocked-cloud pikpak --blocked-cloud others
```

登录时先获取图形验证码，终端提示打开私有临时图片并输入图片中的字符；如站点要求，再输入账号已绑定的邮箱核对。
这不是邮件 OTP，未完成核对不会保存为 active。空输入取消，图片随登录流程结束删除。
密码默认通过隐藏输入的 TTY 提示读取。非 TTY 输入必须显式传 `--interactive`；配合
`--password-stdin` 时第一行是密码，后续按提示输入验证码与必要的绑定邮箱，不能只管道传入密码就视为完成登录。
Panlian 不保存密码，`--remember-credentials` 已拒绝；Cookie 过期后必须人工重新登录。

登录成功后即可搜索。`blocked-cloud` 是跨 profile 的可选全局过滤；可重复指定，命令完整覆盖旧列表，
不传 `--blocked-cloud` 会清空过滤。

Panlian 的配置示例：

```toml
[providers.panlian]
blocked_clouds = []
```

## Woniu

```bash
pansou provider login woniu --profile main --username USERNAME
pansou provider status woniu --profile main
pansou provider configure woniu --base-url https://wn4k.com
pansou provider logout woniu --profile main
```

密码使用隐藏 TTY 提示或 `--password-stdin`，不会保存。登录后保留独立 Cookie；状态检查按需验证会话。
只有真实修改站点才使已有会话失效并要求重登，不把旧站 Cookie 发送到新站或跨站重定向。
退出会通知站点，即使站点不可用也删除本地会话。游客锁定占位链接不作为搜索结果。

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
