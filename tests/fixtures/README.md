# テスト用の crate

M2 からのテスト ([doc/m2-plan.md](../../doc/m2-plan.md) の §2) が使う、公開しない crate を置く。各段で要るものだけを足す。

## 約束

- 本体の package にも workspace にも入れない。それぞれが独立した package か workspace で、本体の `include` (Cargo.toml) にも入らないので、crates.io には出ない
- テストはここの crate を一時ディレクトリに写してから使う。ここで直接 cargo を動かさない (`target/` を作らず、祖先の `.cargo/config.toml` を拾わないため)
- cargo・rustup・git は、[tests/common/isolated.rs](../common/isolated.rs) の `Isolated::apply` を通した隔離した環境で起動する。利用者の `HOME`・`CARGO_HOME`・git の設定と資格情報を使わず、資格情報を問い合わせない (M0b で、git が利用者のデスクトップに askpass のダイアログを出したため)。toolchain は手元のものを使うだけで、入れない (`RUSTUP_AUTO_INSTALL=0`)
- crates.io の依存を持たない。registry の依存が要るときは、[tests/common/registry.rs](../common/registry.rs) の偽の crates.io にテストの中で crate を公開し、隔離した cargo home の `config.toml` で crates.io をそれに置き換える
- git の crate は、テストの中で bare リポジトリを作って指す。worktree は [tests/common/git.rs](../common/git.rs) の `add_worktree` (`git worktree add`) で作る
- 大きさ・深さを見る crate は、ここに置かずテストの中で生成する

## 種類

| 記号 | 種類 | 例 |
| --- | --- | --- |
| (a) | ローカルでビルドする crate (シナリオごと) | member が複数の workspace、path 依存、壊れた build.rs、proc-macro、`#![no_std]`、cfg と feature の組、rustdoc JSON の edge case |
| (b) | git 経由の crate | テストの中で作る bare リポジトリ (branch・tag・rev)。認証の失敗は 401 を返すローカルの HTTP サーバーで起こす |
| (c) | 罠の crate | 範囲の外に書き込む build.rs、`.cargo/config.toml` の `build.rustc`・`rustc-wrapper`、副作用のある proc-macro。隔離した環境でだけビルドする |
| (d) | プロジェクトの形 (ビルドしない) | Cargo.lock が無い・古い、同名の複数版、改名した依存、dev だけの依存、OS ごとの依存、別のレジストリ、vendor、入れ子の worktree |
| (e) | toolchain | 入っていない toolchain を指す `rust-toolchain.toml`、`rust-version`、cdylib だけの lib |
| (f) | crate をまたぐ re-export | A→B→C の連鎖、umbrella、crate をまたぐ glob |
| (g) | 大きさ・深さ | テストの中で生成する |

古い形式 (46〜60) の rustdoc JSON は、ここの crate からは作れないので [testdata/](../../testdata/) (docs.rs の実物) のまま。

## テスト

cargo・git を起動するテストは [tests/local/](../local/) に置く。道具そのもののテストは [tests/local/tools.rs](../local/tools.rs)。
