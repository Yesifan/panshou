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

登录二维码优先绘制在终端；终端不支持时会生成临时 PNG。频道会被规范化和去重，并缓存
`guild_id`。QQPD 在搜索前按需执行 session keepalive。

## Weibo

```bash
pansou provider login weibo --profile main

pansou provider configure weibo --profile main \
  --users 1234567890,2345678901
```

`--user` 可以重复使用，也可以传入 `https://weibo.com/u/1234567890`；保存时会规范化为
数字用户 ID。

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
