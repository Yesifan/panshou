# Repository Guidelines

## Project Structure & Module Organization

```text
src/
├── main.rs        # CLI 入口
├── lib.rs         # 公共模块
├── core/          # 领域逻辑
├── search/        # 搜索编排
├── providers/     # 站点适配
├── check/         # 链接校验
├── http|config|state/  # 传输、配置、状态
└── cli|output/    # CLI 与输出

tests/             # 集成测试与 fixtures
docs/              # 文档
.github/workflows/ # 发布自动化
scripts/           # 安装与卸载脚本
```

部分有状态的 Provider 测试 fixture 会直接放在对应模块旁。

## Build, Test, and Development Commands

Read [development doc](./docs/development.md) for a complete list of commands.

## Docs

每次发布前更新 `docs/` 下的文档，确保文档与代码保持一致。

## Coding Style & Naming Conventions

Use standard rustfmt output (four-space indentation). Name modules, functions,
and test cases in `snake_case`; types and traits use `UpperCamelCase`; constants
use `SCREAMING_SNAKE_CASE`. Keep provider-specific protocol code inside its
provider module and return shared `SearchResult`, `Link`, or error types at the
boundary. Avoid broad abstractions or defensive branches without a demonstrated
case.

## Testing Guidelines

Use Rust's built-in test framework, `#[tokio::test]` for async behavior, and
`wiremock` for HTTP boundaries. Put focused unit tests beside implementation and
cross-module contracts in `tests/*.rs`. Provider changes should include fixtures
for success, empty, and malformed responses. There is no numeric coverage gate,
but every behavior change needs a regression test. Keep tests deterministic;
prefer fixtures or mock servers over live third-party endpoints.

## Security & Configuration

Never commit credentials, cookies, generated state, or cache data. Do not add
plaintext password flags; use TTY prompts or `--password-stdin`. Redact tokens,
authorization headers, proxy credentials, and QR-login data from errors and logs.

## Commit & Pull Request Guidelines

History follows concise Conventional Commit subjects. Use an imperative subject and
keep each commit focused. Pull requests should explain intent and user-visible
effects, list validation commands, link related issues when applicable, and add
sample CLI output when output contracts change. All formatting, Clippy, tests,
and platform builds must pass before merge.
