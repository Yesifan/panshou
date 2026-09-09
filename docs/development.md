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
