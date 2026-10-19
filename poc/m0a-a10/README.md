# A10: rustdoc_processor との公開パスの突き合わせ

[doc/m0-plan.md](../../doc/m0-plan.md) の A10 で使う。結果は同じ文書の §6.1 に書く。

同じ docs.rs の rustdoc JSON から、chatora-rustdoc-mcp の公開パス索引 (`query::path_index`) と [rustdoc_processor](https://crates.io/crates/rustdoc_processor) 0.1.2 の索引を作り、item ごとに公開パスの集合と正準パスを比べる。

- rustdoc_processor は rustdoc-types 0.57 (形式 57) で読むので、JSON をこのサーバーの parse で読み、`format_version` を 57 に書き換えて 0.57 の型で読み直す。形式 57〜61 の違いは stability などの欄だけで、0.57 の型は知らない欄を無視する
- rustdoc_processor が索引に入れない item (マクロなど) は「only ours」に出る

このディレクトリは独立した cargo プロジェクト (空の `[workspace]` で、Cargo.lock と `target/` を別に持つ) で、chatora-rustdoc-mcp の crate には同梱されない (`Cargo.toml` の `include` の外)。依存に rusqlite の `bundled` があるので、初回のビルドで SQLite の C のソースをコンパイルする (C コンパイラが要る)。

## 使い方

JSON はキャッシュから読む。無ければリポジトリのルートで取ってくる。

```sh
cargo run --release --example fetch_docs_rs -- tokio@1.53.3   # リポジトリのルートで
cargo run --release -- tokio@1.53.3 axum@0.8.9                # このディレクトリで
```
