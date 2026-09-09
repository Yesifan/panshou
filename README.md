# PanSou CLI

PanSou 是一个以 Rust 编写的本地网盘资源搜索与链接检测工具。它从 Telegram
频道和多个 provider 并发检索内容，合并、过滤、排序并去重链接，也可以独立检测
网盘分享链接的有效性。

## 安装

```bash
curl -fsSL \
  https://raw.githubusercontent.com/Yesifan/panshou/main/scripts/install.sh | sh
```

## 卸载

```bash
curl -fsSL \
  https://raw.githubusercontent.com/Yesifan/panshou/main/scripts/uninstall.sh | sh
```

## 使用

```bash
# 搜索资源
pansou search "仙逆"

# 筛选网盘或搜索来源
pansou search "仙逆" --cloud quark
pansou search "仙逆" --provider meitizy --provider pansearch

# 检测分享链接
pansou check https://pan.quark.cn/s/xxxx
```

## 文档

- [使用说明](docs/usage.md)
- [Provider 配置](docs/providers.md)
- [开发指南](docs/development.md)
- [整体架构](docs/architecture.md)

## License

MIT
