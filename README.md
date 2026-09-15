# PanSou CLI

PanSou 是一个以 Rust 编写的本地网盘资源搜索与链接检测工具。它从 Telegram
频道和多个 provider 并发检索内容，合并、过滤并增量去重输出链接，也可以独立检测
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

# 管理频道；导入的新频道默认启用，可用 --disable 仅保存
pansou channel add @foo https://t.me/bar
pansou channel import --builtin
pansou channel list

# 机器可读流式搜索
pansou search "仙逆" --format jsonl

# 检查并安装更新
pansou update --check
pansou update
```

搜索默认按来源完成顺序持续输出表格，也支持 JSONL 事件流；搜索 JSON 格式已移除，
独立 `check` 仍支持 JSON。TG 与 provider 共享默认 8 并发，单来源默认超时 30 秒，
搜索阶段总时限默认 600 秒（`--all-timeout`），不包含初始化和链接检测。
资源命名可能不规范，建议使用简短的核心关键词。

频道使用独立的 `channels.toml`，最多同时启用 128 个；旧 `config.toml` 的
`search.channels` 已废弃且不自动迁移，请使用频道命令重新添加或导入。

## 文档

- [使用说明](docs/usage.md)
- [Provider 配置](docs/providers.md)
- [开发指南](docs/development.md)
- [整体架构](docs/architecture.md)

## 致谢

本项目是对 [fish2018/pansou](https://github.com/fish2018/pansou) 的 Rust CLI
重构。Provider 协议、解析行为及部分内置频道数据参考了上游实现。

感谢原项目作者及所有贡献者的工作。

本项目为独立维护的 CLI 实现，与上游项目存在产品形态和实现差异。

## License

MIT
