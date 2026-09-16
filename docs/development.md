# 开发指南

## 从源码构建

项目使用 stable Rust、edition 2024 和 rustls，无需安装 OpenSSL。

```bash
git clone https://github.com/Yesifan/pansou.git
cd pansou
cargo build --release
./target/release/pansou --help
```

## 验证

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

依赖第三方站点的 live tests 默认忽略，避免站点变化、Cloudflare、地域或限流导致 CI
不稳定。显式运行示例：

```bash
PANSOU_LIVE_TESTS=1 \
  cargo test provider_pansearch_live -- --ignored
```

实现模块、数据流、并发模型与安全边界参见[整体架构](architecture.md)。完整的 v1 设计
规格参见 [Rust CLI v1 规格](specs/RUST_CLI_V1_SPEC.md)。

频道管理、搜索输出及更新的行为契约见
[频道、搜索输出与自更新规格](specs/CHANNEL_STREAM_UPDATE_SPEC.md)。

## 变更验证重点

- 频道文件覆盖增删启停、128 上限、三种导入、并发保存和失败不留下部分修改。
- 搜索使用模拟来源验证动态进度、`--no-progress`、非 TTY stderr 自动隐藏、共享并发、单来源超时、
  总时限取消、最终稳定排序的 table/JSON、检测摘要以及 `partial`/退出码；计时测试不应
  真实等待 600 秒。
- Telegram fixture 覆盖正文/按钮链接、按钮密码、无正文消息及关键词正文回退。
- 更新使用模拟 Release 与本地安装包验证版本选择、校验、兼容性和失败恢复，不在测试中
  更新开发者正在使用的程序。Windows 替换流程需要对应平台验证。
- 修改数据格式时更新版本和迁移测试；自动迁移前保留备份，手动事项提供可操作提示。

## 发布

每次发布前：

1. 同步版本号、使用文档和候选频道清单，并完成上面的验证命令。
2. 新建 `docs/releases/vX.Y.Z.md`，文件名必须与 Release tag 一致。
3. 推送 `vX.Y.Z` tag，等待 Release workflow 构建并发布各平台安装包及 `SHA256SUMS`。
4. 检查 Release 页面、安装包和更新流程；跨平台构建不能代替平台上的安装与替换验证。

发布文档保持简短，只写用户可见的变化；存在配置迁移、命令行为变化或其他必要操作时，
再补充升级说明：

```markdown
# PanSou vX.Y.Z

一句话简要说明。

## Changes

- 用户可见的变化。

## Breaking Changes

不兼容变化以及用户需要执行的迁移操作。

## Upgrade

必要的升级或安装说明。
```

`Breaking Changes` 和 `Upgrade` 是可选章节；没有对应内容时，连同标题一起省略。Release
workflow 会读取这份文档，并在其后附加 GitHub 自动生成的变更记录。直接提交到主分支的
变更不会像合并 PR 一样被自动分类，因此人工摘要应覆盖所有重要的用户可见变化，无需逐项
罗列内部重构或提交记录。

Release tag 必须与二进制版本一致。检查通过情况以实际运行结果为准；已经发布的 tag 和
安装包不应被覆盖，发布内容有误时应修复后发布新的补丁版本。
