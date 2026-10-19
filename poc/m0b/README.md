# M0b: ローカルビルドの実験

[doc/m0-plan.md](../../doc/m0-plan.md) の §4 の実験 (B1〜B7) のスクリプトと、合成の workspace を置く。結果は同じ文書の §6.2 に書く。

## 共通の約束 ([env.sh](env.sh))

- ホストを変えない。`RUSTUP_AUTO_INSTALL=0` で、`cargo +nightly` や `rust-toolchain.toml` による toolchain の自動導入を防ぐ。足りない toolchain や target があっても入れない
- ビルドの生成物は `work/` (git では無視) の下に、実験ごとの `CARGO_TARGET_DIR` と `CARGO_BUILD_BUILD_DIR` で置く。利用者の `target/` は使わない (R-WS-2)
- git やレジストリの失敗を見る実験は `isolate_home` で、`HOME`・`XDG_CONFIG_HOME`・`CARGO_HOME` を `work/` の下に向ける (利用者の git と cargo の設定や資格情報を使わない)。資格情報の問い合わせも止める (`GIT_TERMINAL_PROMPT=0`、`SSH_ASKPASS`・`GIT_ASKPASS` を外し、`SSH_ASKPASS_REQUIRE=never`・`GCM_INTERACTIVE=never`)。止めないと、GitHub の存在しないリポジトリで git がデスクトップの askpass のダイアログを出す
- 「実際の workspace」はこのリポジトリ。cargo はリポジトリのルートで起動する (R-WS-2 の、プロジェクトの文脈で起動するときと同じ)

## 実験

| ディレクトリ | 実験 | 実行 |
| --- | --- | --- |
| [b1-rustc-bootstrap/](b1-rustc-bootstrap/) | B1 (Q-12): stable + `RUSTC_BOOTSTRAP=<crate 名>` の作り直しと文書の違い | `./b1-rustc-bootstrap/run.sh`。記録は `work/b1/log.txt` |
| [b2-doc-vs-rustdoc/](b2-doc-vs-rustdoc/) | B2 (Q-17): `cargo doc --output-format json` と `cargo rustdoc` | `./b2-doc-vs-rustdoc/run.sh` (記録は `work/b2/log.txt`)、`./b2-doc-vs-rustdoc/dup.sh` (同名で版の違う依存) |
| [b3-features/](b3-features/) | B3・B7-2 (Q-20): feature の組、B6 (R-MCP-3): `cargo tree` の形、B7-9: `-p` で名指しできる依存 | `./b3-features/run.sh`。記録は `work/b3/log.txt` |
| [b4-cfg-docsrs/](b4-cfg-docsrs/) | B4 (Q-11): すべての依存を `--cfg docsrs` 付きで文書化し、壊れるものを数える | `./b4-cfg-docsrs/run.sh`。記録は `work/b4/log.txt` と `work/b4/<toolchain>.tsv` |
| [b5-dep-info/](b5-dep-info/) | B5 (R-WS-4): dep-info を rustdoc の `--emit=dep-info` で取るか、`cargo check` から取るか | `./b5-dep-info/run.sh`。記録は `work/b5/log.txt` |
| [b7-cargo-failures/](b7-cargo-failures/) | B7-3・B7-4 (Q-30): `cargo metadata` の失敗と git の取得の失敗の文言 (外へ通信する) | `./b7-cargo-failures/run.sh`。記録は `work/b7f/log.txt` |
| [b7-toolchain/](b7-toolchain/) | B7-1・B7-6・B7-7・B7-8 (Q-30): `RUSTFLAGS` と `--target`、`rust-std` が無い target、実行ファイルの組み合わせ、target の JSON 仕様 | `./b7-toolchain/run.sh`。記録は `work/b7/log.txt` |
