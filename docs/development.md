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

频道管理、流式搜索及更新的行为契约见
[频道、流式搜索与自更新规格](specs/CHANNEL_STREAM_UPDATE_SPEC.md)。

## 变更验证重点

- 频道文件覆盖增删启停、128 上限、三种导入、并发保存和失败不留下部分修改。
- 搜索使用模拟来源验证先完成先输出、共享并发、单来源超时、总时限取消、元数据更新
  以及 `partial`/退出码；计时测试不应真实等待 600 秒。
- Telegram fixture 覆盖正文/按钮链接、按钮密码、无正文消息及关键词正文回退。
- 更新使用模拟 Release 与本地安装包验证版本选择、校验、兼容性和失败恢复，不在测试中
  更新开发者正在使用的程序。Windows 替换流程需要对应平台验证。
- 修改数据格式时更新版本和迁移测试；自动迁移前保留备份，手动事项提供可操作提示。

发布前同步使用文档、候选频道清单和升级说明，并确保 Release tag 与二进制版本一致、
各平台安装包及 `SHA256SUMS` 完整。检查通过情况以实际运行结果为准，跨平台构建不能代替
平台上的安装与替换验证。
