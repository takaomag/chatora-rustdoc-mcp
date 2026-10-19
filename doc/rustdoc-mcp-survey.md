# Rust ドキュメント系 MCP サーバー調査

- 調査日: 2026-10-13 (同日、主要な実装のソースを精読した二次調査で訂正)。2026-10-14 に三次調査 (全記載の再検証と追記。§9) で改訂し、2026-10-15 に整理した (§11)
- 目的: Claude Code で Rust 開発をするとき、現在使っている ferritin より優れた rustdoc 系 MCP があるかを判断する
- 調査時点の環境: ferritin のホスト版 (`https://ferritin.rs/mcp`, HTTP transport) を使っていた
- その後: この調査を受けて自作 (chatora-rustdoc-mcp) を決めた。要件は [requirements.md](requirements.md)。§1 の推奨は調査時点の判断

---

## 1. 結論

**乗り換え先の第一候補は `rustdoc-mcp` (jbr) です。** ferritin と同じ作者・同じリポジトリ (`jbr/ferritin`) から出ているローカル stdio 版で、使っているエンジン (`ferritin-common`) も同じです。ferritin に足りない次の点を補います。

| ferritin (ホスト版) に足りない点 | rustdoc-mcp (ローカル版) |
| --- | --- |
| Cargo.lock を読まない。バージョンは `crate@=x.y.z` で毎回手で渡す | `cargo metadata` 経由で、ロックされたバージョンを自動で使う |
| 自分の workspace や非公開 crate は見えない | workspace crate をローカルでビルドして引ける (private items も含む) |
| std はサーバー側の nightly 固定 | ローカル toolchain の `rust-docs-json` から std を引ける |
| ソースは見られない | `include_source` でソースも返せる (workspace と、ローカルでビルドした依存だけ) |
| 出力量を調整できない | `verbosity` (minimal/brief/full) と kind フィルタで出力を絞れる |

逆に、rustdoc-mcp がホスト版より劣る点もあります (三次調査で判明)。

| rustdoc-mcp (ローカル版) の弱点 | ferritin (ホスト版) |
| --- | --- |
| **trait implementors を出さない**。描画処理が ferritin とは別の独自実装で、implementors の節が無い | 出す (定義元 crate 内、20 件で打ち切り) |
| **ツール呼び出しのたびに Store を作り直す**。キャッシュは 1 回の呼び出しの中でしか効かず、毎回 `cargo metadata` を実行し、失敗したビルドも毎回やり直す | 長く生きる Store で、メモリ上のキャッシュと否定キャッシュ (1 時間) が効く |

また、rustdoc-mcp は **nightly toolchain が必須**です。依存 crate をローカルで `cargo doc` するので遅く、上の作り直しのため「初回だけ遅い」では済みません。std を引くには、nightly に `rust-docs-json` を入れる必要もあります。

**補完役として `docs-rs-mcp` (syphar) にも注目する価値があります。** 作者の syphar は [docs.rs リポジトリ](https://github.com/rust-lang/docs.rs)の最多コミッターです (§9)。trait implementors、feature flags、changelog、re-export 解決といった「LLM が実際に聞きたいこと」に合わせた 14 個のツールを持っています。ただしまだ alpha (作者自身が「実験」と明言) で、std とローカル crate は扱えません。さらに**現状のコードは rustdoc JSON の形式 57 しか読めません** (§4)。main は 2026-08-21 から止まっていますが、検索の順位付け・描画済みシグネチャ・changelog の範囲指定などを実装した未マージのブランチがあります。

**それ以外の候補は、ferritin 系を上回る理由が見当たりません。** 大半が星 15 未満の単独作者プロジェクトか、HTML スクレイピング型か、すでに放棄されています。

### 推奨構成

1. **主力**: `rustdoc-mcp` (jbr) をローカルに導入する。project の依存・workspace・std をまとめて引ける。
2. **当面の併用**: ホスト版 ferritin を残す。プロジェクト外の crate を軽く引くときや、nightly ビルドが失敗したときの逃げ道になる。rustdoc-mcp は trait implementors を出さないので、implementors を聞くときもホスト版を使う。同系統のサーバーが 2 つ並ぶと Claude がどちらを使うか迷いやすいので、使い分けを rules に書いておく。
3. **任意で試す**: `docs-rs-mcp` (syphar)。trait の実装者や feature flags を構造化データで欲しい場面が多い場合に入れる。ただし形式 57 でビルドされたリリースしか読めないので、現時点で試す価値は限られる。

Cargo.lock・workspace・std をまとめて扱えるのは依然として rustdoc-mcp だけなので順位は変えませんが、上の弱点 (詳細は §4 の Cons) のため、**ホスト版に対する優位は二次調査の時点の評価より小さい**です。並べて試すときの観点は §9 にあります。

**新しい比較軸** (三次調査で追加。§5.1): 依存のコードを実行するか、パスの検証、MCP への準拠 (`isError`・`structuredContent`)、コードを流用できるライセンスか、評価の仕組みがあるか。この軸で見ると、どの候補にも大きな穴があります (§7)。

---

## 2. 評価基準

Claude Code で Rust を書くときに効く順に並べています。

1. **正確さ (データソース)**: rustdoc JSON > rustdoc HTML のスクレイピング > tree-sitter によるソース解析 > LLM による要約。macro が生成した API・re-export・impl block を正しく扱えるのは rustdoc JSON だけ。
2. **対象範囲**: Cargo.lock に合ったバージョン / workspace と非公開 crate / std・core・alloc / trait implementors / ソース。
3. **トークン効率**: 出力が簡潔か。ページ単位の丸ごとダンプになっていないか。ツール数がコンテキストを圧迫しないか。
4. **保守状況**: 最終コミット、リリース頻度、作者の信頼性。rustdoc JSON は `format_version` が頻繁に変わるため、**保守が止まると壊れる**。
5. **導入と運用のコスト**: nightly の要否、API キーの要否、ビルド時間、常駐プロセスの数。
6. **安全性** (三次調査で追加): 依存の build.rs や proc-macro を実行するか、任意のパスや URL のビルドを受け付けるか、ファイルパスと引数を検証するか。Claude Code の Bash のサンドボックスは MCP サーバーを覆わないので、サーバー側の作りがそのまま危険の大きさになる。
7. **MCP への準拠と品質保証** (三次調査で追加): エラーを `isError` のツール結果で返すか (プロトコルエラーにすると LLM が回復しにくい)、`structuredContent` / `outputSchema` / annotations を正しく使うか、テスト・CI・LLM による評価があるか、Windows を含めて動くか、コードを流用できるライセンスか。

---

## 3. 名前の衝突マップ

候補の名前は、同名・類似名の別プロジェクトがかなり多くあります。名前だけで導入すると別物を入れてしまう恐れがあります。**ferritin を除く 10 の名前のうち、別プロジェクトとの衝突が無いのは mcp-rustdoc-parser・cargo-doc-mcp・Daipendency の 3 つだけです。**

| 聞いたことのある名前 | 実際のプロジェクト (★ は GitHub スター) |
| --- | --- |
| ferritin | **[jbr/ferritin](https://github.com/jbr/ferritin)**: ホスト MCP・CLI・TUI・Web。crate [`ferritin`](https://crates.io/crates/ferritin) |
| rustdoc-mcp | crates.io 上は **[jbr/ferritin 内の `rustdoc-mcp`](https://github.com/jbr/ferritin/tree/main/rustdoc-mcp)** (ferritin のローカル版、crate [`rustdoc-mcp`](https://crates.io/crates/rustdoc-mcp))。GitHub には同名の別物として [Xevion/rustdoc-mcp](https://github.com/Xevion/rustdoc-mcp) (alpha) と [TheCactusBlue/rustdoc-mcp](https://github.com/TheCactusBlue/rustdoc-mcp) (HTML スクレイピング) がある |
| rust-docs-mcp | crates.io 上は **[snowmead/rust-docs-mcp](https://github.com/snowmead/rust-docs-mcp)** (★135)。他に [Opensource-telegraphs-org/rust-docs-mcp](https://github.com/Opensource-telegraphs-org/rust-docs-mcp) (実際は MCP ではない) と [nanameru/Rust-Docs-MCP](https://github.com/nanameru/Rust-Docs-MCP) (Rust Book のみ) |
| docsrs-mcp | **[dmvk/docsrs-mcp](https://github.com/dmvk/docsrs-mcp)** (crates.io)。他に [RimoVR/docsrs-mcp](https://github.com/RimoVR/docsrs-mcp) (Python) と [maan2003/docsrs-mcp](https://github.com/maan2003/docsrs-mcp) (1 日だけの実験) |
| mcp-docsrs | **[vexxvakan/mcp-docsrs](https://github.com/vexxvakan/mcp-docsrs)** (★13)。他に [shuakami/mcp-docsrs](https://github.com/shuakami/mcp-docsrs) と [CarteraMesh/mcp-docsrs](https://github.com/CarteraMesh/mcp-docsrs) (中身はほぼ空) |
| rust-docs-mcp-server | **[Govcraft/rust-docs-mcp-server](https://github.com/Govcraft/rust-docs-mcp-server)** (★299)。他に [laptou/rust-docs-mcp-server](https://github.com/laptou/rust-docs-mcp-server) (壊れている) と **shark-hunt/rust-docs-mcp-server** (Govcraft の無断コピー。導入禁止。§4 Tier D) |
| rustdoc-mcp-server | [KevinT3Hu/rustdoc-mcp-server](https://github.com/KevinT3Hu/rustdoc-mcp-server) (crates.io) と [MasYaaz/rustdoc-mcp-server](https://github.com/MasYaaz/rustdoc-mcp-server) |
| mcp-rust-docs | **[46ki75/mcp-rust-docs](https://github.com/46ki75/mcp-rust-docs)** (v1 で書き直し、ホスト版あり)。他に [46ki75/mcp-rust-docs-deprecated](https://github.com/46ki75/mcp-rust-docs-deprecated) (archived。npm の [`mcp-rust-docs`](https://www.npmjs.com/package/mcp-rust-docs) はこちら)、[0xKoda/mcp-rust-docs](https://github.com/0xKoda/mcp-rust-docs) (★47、放棄)、[trungnguyenvn/mcp-rust-docs](https://github.com/trungnguyenvn/mcp-rust-docs) |
| mcp-rustdoc-parser | [automataIA/mcp-rustdoc-parser](https://github.com/automataIA/mcp-rustdoc-parser) (PoC のみ) |
| cargo-doc-mcp | [spacemeowx2/cargo-doc-mcp](https://github.com/spacemeowx2/cargo-doc-mcp) |
| Daipendency | [daipendency org](https://github.com/daipendency) (作者 Gus Narea)。MCP 本体は [daipendency/daipendency-mcp](https://github.com/daipendency/daipendency-mcp) (npm [`@daipendency/mcp`](https://www.npmjs.com/package/@daipendency/mcp)) |

紛らわしい類似名がほかにもあります: `docs-rs-mcp` ([syphar](https://github.com/syphar/docs-rs-mcp) / [nuskey8](https://github.com/nuskey8/docs-rs-mcp) / [kazzix14](https://github.com/kazzix14/docs-rs-mcp) の npm パッケージ。kazzix14 は shuakami の fork)、`mcp-rustdoc` ([kieled](https://github.com/kieled/mcp-rustdoc) / [possdog](https://tangled.org/poss.dog/mcp-rustdoc))、`rustdocs-mcp` ([JLFN](https://github.com/JLFN/rustdocs-mcp))、`mcp-docs-rs` ([BrightX](https://github.com/BrightX/mcp-docs-rs))。

三次調査で見つかった類似名: [0xDAEF0F/mcp-rust-docs-embed](https://github.com/0xDAEF0F/mcp-rust-docs-embed) (★4)、[ai-tools-all/rust_docs_mcp](https://github.com/ai-tools-all/rust_docs_mcp)、[emk/tiny-rustdoc-mcp](https://github.com/emk/tiny-rustdoc-mcp)、[iflow-mcp/lazhenyi-rs-docsrs-mcp](https://github.com/iflow-mcp/lazhenyi-rs-docsrs-mcp)、[liamwh/docs-rs-mcp-server](https://github.com/liamwh/docs-rs-mcp-server) (★2)、[akesson/docsrs](https://github.com/akesson/docsrs) (crates.io の `docsrs` は無関係の別 crate)。なお cargo-doc-mcp の `package.json` の name は `"docs-rs-mcp"` だが、private で npm に公開されていないので、npm で取り違えることは無い (リポジトリの検索では紛らわしい)。

---

## 4. ランキング (優れた順)

順位は §2 の基準で付けています。**どれもインストールして動かしてはいません** (根拠の範囲は §9)。

### Tier S: 推奨

#### 1. rustdoc-mcp (jbr)

- リポジトリ: <https://github.com/jbr/ferritin/tree/main/rustdoc-mcp>
- crates.io: <https://crates.io/crates/rustdoc-mcp>
- バージョン: crate `rustdoc-mcp` 0.10.0 (2026-08-07)。累計 2,809 DL (2026-10-14)。リリース PR #363 に 0.11.0 が待機中
- ライセンス: MIT OR Apache-2.0 (GitHub の API は Apache-2.0 と誤検出している)
- 導入: `cargo install rustdoc-mcp` / `cargo binstall rustdoc-mcp` / GitHub Releases のバイナリ (Linux x86_64、macOS 2 種、Windows x86_64) → `rustdoc-mcp serve` (stdio)
- ツール (4):
  - `set_working_directory`: 対象の Cargo プロジェクトを設定する
  - `get_item`: `name` / `include_source` / `recursive` / `filter` (kind) / `verbosity` を指定できる
  - `list_crates`: workspace crate と依存 crate を一覧する (引数 `workspace_member` は宣言だけで、実装で使われていない)
  - `search`: crate 内を検索する (`crate_name` / `query` / `limit`)
- データソース (std → ローカル → docs.rs の順に試し、先に見つかったものを使う):
  - workspace と依存 crate は、ローカル nightly で生成した rustdoc JSON (`--document-private-items` は workspace member だけに付く)
  - それ以外の crate は docs.rs の rustdoc JSON
  - std は `rust-docs-json` コンポーネント
- キャッシュ: docs.rs から取った JSON は `~/.cache/ferritin` (`$FERRITIN_HOME` → `$XDG_CACHE_HOME` → `~/.cache` の順)。**ローカルビルドの JSON はユーザーのプロジェクトの `target/doc` に書く**。rkyv の sidecar と検索索引も JSON の隣に書くので、std では rustup の toolchain ディレクトリにも書き込む。0.8.0 以降は crate 名の一覧をローカルに持ち、オフラインでもバージョンを解決できる (一覧は crates.io ではなく jbr/crate-names の GitHub Releases に置かれた日次のファイルで、最大で約 1 日遅れる)

**特徴**: ferritin と同じ取得・正規化のエンジン (`ferritin-common`) で動くローカル版。Cargo.lock・workspace・std をまとめて扱える唯一の選択肢です。ただし**描画処理は ferritin とは別の独自実装** (`rustdoc-mcp/src/formatting/*`) で、出力の中身は ferritin と同じではありません。ARCHITECTURE.md には、将来 rustdoc-mcp を ferritin の薄い層にするとあります。

**Pros**

- Cargo.lock に合ったバージョンを自動で引く。`@=ver` を毎回指定する手間と、指定ミスが無くなる
- 自分の workspace crate (private items 含む) を引ける
- std / core / alloc / proc_macro / test を引ける (要 `rust-docs-json`)
- ツールは 4 個だけ。`verbosity` で出力を絞れる
- ソースを表示できる (workspace と、ローカルでビルドした依存だけ)
- 保守は続いている (ferritin と同時にリリースされる)。ただしリリースは 0.10.0 (2026-08-07) から滞留し、main のコミットは 2026-10-02 までは 2〜3 日おきの nightly のスナップショット更新が中心で、以後は無い (10-13 の push は bot のブランチ)

**Cons**

- nightly toolchain が必須。rustdoc JSON の形式変更でパースが壊れうる (README は新しい形式を best-effort で扱うと記載)
- 依存 crate を初めて引くときに `cargo doc` が走る。遅いうえ、feature の組み合わせ次第ではビルドに失敗しうる
- セッションごとに `set_working_directory` が必要 (ステートフル)。設定するまで全ツールが失敗する
- ローカルビルドは `RUSTDOCFLAGS` を使い、ユーザーの `target/` に書く。cargo や rust-analyzer とロックを取り合う。呼ぶのは `rustup run nightly cargo doc` に固定で、rustup が必須、日付付きの nightly も選べない
- 鮮度の判定が workspace のルートの `src/` の mtime だけ。仮想 workspace では member を編集しても再ビルドされない。workspace 外の path 依存は版の一致しか見ないので、編集しても再ビルドされない
- **trait implementors を出さない** (独自の描画処理に implementors の節が無い)。固有メソッドを「Associated Types:」という見出しの下に出す不具合もある
- **ツール呼び出しのたびに Store を作り直す** (`request.rs:43-49`)。キャッシュが 1 回の呼び出しの中でしか効かず、毎回 `cargo metadata` と `rustup run nightly rustc` を実行する
- ビルドの失敗 (nightly が無い場合を含む) を握りつぶし、「その crate は無い」として扱う。否定キャッシュの 1 時間は上の作り直しのため効かず、**失敗したビルドを呼び出しのたびにやり直す** (遅い)
- 同名の crate の複数バージョンを持てない。ローカルの crate 表が名前だけをキーにしているので 1 つの版しか持たず、もう一方の版は docs.rs に回される。どちらを使うかは `cargo metadata` の並び順で黙って決まる
- annotations は crates.io の 0.10.0 には無い (未リリースの main だけ)。main の `openWorldHint: false` は実態 (docs.rs に出る) と合わず、`readOnlyHint: true` も、`cargo doc` で `target/` に書き込み build.rs を実行しうる実態と合わない
- stdio の処理が同期的で 1 件ずつ。`cargo doc` の実行中は全リクエストが止まり、進捗通知もキャンセルも無い
- `include_source` は span のパスを検証しない。docs.rs 由来の crate では存在しないファイルか別のファイルを読むおそれがあり、std の span は黙って空になる
- `MCP_SHARED_SESSION_PATH` を設定すると作業ディレクトリをプロセス間で共有するが、session 名が固定の "default" なので、並行する別プロジェクトのセッションと上書きしあう
- パスの解決の問題 (定義パスの表示、Deref 経由のメソッドを引けない) は、データ層 (ferritin-common) を共有しているので下の ferritin と共通とみられる (rustdoc-mcp では未実測)。一方、本文は生の Markdown をそのまま出すので、バッククォートは残るが intra-doc link は未解決の参照形式 (`` [`Vec<T>`] ``) のまま。private フィールドや hidden・unstable なモジュールも出る
- README のツール名はハイフン区切り (`get-item`) だが、これは CLI の `--help` のサブコマンド名。MCP のツール名はアンダースコア (`get_item`)
- CI は ubuntu だけ。Windows は「手動で確認済み」と README にあるだけ
- スター数は 38 (ferritin と同じリポジトリ)、fork は 2 で、利用者はまだ少ない

#### 2. ferritin (jbr、ホスト版)

- リポジトリ: <https://github.com/jbr/ferritin>
- Web: <https://ferritin.rs> / MCP エンドポイント: `https://ferritin.rs/mcp`
- crates.io: <https://crates.io/crates/ferritin>
- バージョン: crate `ferritin` 0.17.0 (2026-08-07)。ライブのサーバーも `serverInfo.version` に 0.17.0 を返すが、**実際に動いているのは未リリースの main** (mcplease 0.4、protocolVersion 2025-11-25、ツール一覧に `ttlMs` と annotations を返す)。main の最終コミットは 2026-10-02 (10-13 の push は bot のブランチ)。リリース PR ([#363](https://github.com/jbr/ferritin/pull/363)。ferritin 0.18.0 / ferritin-common 0.19.0 / rustdoc-mcp 0.11.0) が 2026-08-08 から open のままで、未リリースの変更が溜まっている
- 公式 MCP レジストリ ([registry.modelcontextprotocol.io](https://registry.modelcontextprotocol.io)) に載っている唯一の Rust ドキュメント専用サーバー (`rs.ferritin/rustdoc`。2026-10-14 に再確認)。登録されている版は `2026.7.21` と `0.15.1` で、7 月から更新されていない
- ツール (2): `get(path)` / `search(crate?, query)`。バージョンは `crate@1` (semver 要件) か `crate@=1.40` (完全一致) で指定する
- データソース: docs.rs の rustdoc JSON と crates.io。std はサーバー側の nightly の `rust-docs-json` (実測で `1.101.0-nightly 2026-10-11`)

**特徴**: 調査時点で使っていたもの。設定ゼロ、ツール 2 個、トークン効率が良い。

**Pros**

- ローカルに何も要らない。nightly も API キーも不要
- ツールが 2 個だけで、コンテキストへの負担が最小
- trait implementors を出す (定義元 crate 内のものだけ)
- std を引ける (nightly 版)
- 同じリポジトリで、CLI (`ferritin get --source`)・TUI・Claude Code 用 skill ([`skills/SKILL.md`](https://github.com/jbr/ferritin/blob/main/skills/SKILL.md))・セルフホスト (`--features mcp` → `ferritin serve`) も提供されている

**Cons**

- 第三者がホストするサービスに依存する。ネットワーク必須。問い合わせ内容が外部に出る (サーバーは全リクエストをログに出す実装)。レート制限は、実装上は環境変数 `FERRITIN_MCP_RATELIMIT` を設定したときだけ有効で、本番の設定値は分からない
- Cargo.lock を読まないので、エージェントが自分で `@=ver` を渡す必要がある
- 自分の workspace や非公開 crate は見えない
- docs.rs に rustdoc JSON が無い古いバージョンは失敗する (実測: `serde@=1.0.100` は "no rustdoc JSON on docs.rs")
- std は nightly 固定なので、stable に無い unstable API が混ざる。しかも**印が付かない** (unstable の `Vec::push_mut` にも印が無い。描画処理が stability を一切参照しない)。deprecated と doc(hidden) も扱わない。private フィールドや std 内部の trait (`SpecExtend` など) も混ざる
- MCP 経由ではソースを見られない。出力量も調整できない (`std::vec::Vec` は約 26.7k 文字)
- `Vec::iter` のように Deref や trait 経由で呼べるメソッドと、プリミティブ型のメソッドを引けない (実測で「Could not find」。`std::primitive::str::len` も同じ)。プリミティブ型そのものは `std::primitive::str` などで引けるが、`std::primitive::slice` は引けない (2026-10-14 実測)。impl の逆引き索引がパスを持つ型しか入れていないため
- 見出しに定義パス (`tokio::sync::mutex::Mutex`。`mutex` は private module) が出る。検索結果には hidden モジュール経由のパスが出る
- ドキュメント中のリンクの行き先と、インラインコードのバッククォートが消える
- implementors は 20 件で打ち切られ (`serde::Serialize` では 100 件以上が `… and N more` に省略される)、続きを MCP から取る手段が無い
- 「見つからない」も通常の結果 (`isError` なし) で返り、無関係な候補が並ぶ。実装は `is_error` を計算しながら捨てている (`mcp.rs:104-116`)

### Tier A: 有力な補完候補

#### 3. docs-rs-mcp (syphar)

- リポジトリ: <https://github.com/syphar/docs-rs-mcp>
- crates.io: <https://crates.io/crates/docs-rs-mcp>
- 備考: crate のメタデータには `github.com/rust-lang/docs-rs-mcp` とあるが、その URL は 404
- バージョン: crate `docs-rs-mcp` 0.1.0-alpha.4 (2026-06-20)。累計 105 DL。main の最終コミット 2026-08-21 (説明文を 150 語以内に書き直してテストを追加)、最終 push 2026-09-17 (dependabot)。★1
- 未マージのブランチ `rename-manifest-dependencies` (2026-06-20、約 2,000 行): 検索の順位付け、描画済みシグネチャ、changelog の範囲・件数・文字数の指定、`list_methods` の絞り込みと件数表示、`compare_versions`、`list_versions`、`.crate` のソース検索 (`search_source`)、トラバーサルを拒否するファイル読み取り、feature の推移閉包と `enabled_by`。下の Cons のうち順位付け・トークン量・changelog はこれで解消するが、マージの見込みは不明
- ライセンス: crate は MIT OR Apache-2.0 と宣言しているが、**リポジトリにも .crate にもライセンスの本文が無い** (README は存在しない LICENSE-* を指している)
- 導入: `cargo install docs-rs-mcp` (stdio。引数なしで動く。alpha.2 から任意の CLI 引数と `~/.config/docs-rs-mcp/config.toml` もある)
- ツール (14): `resolve_version` / `search_items` / `list_module` / `get_item` / `list_methods` / `list_impls` / `list_implementors` / `inspect_feature_flags` / `crate_metadata` / `crate_overview` / `manifest_dependencies` / `changelog` / `find_examples` / `readme`
- データソース: docs.rs の rustdoc JSON と `.crate` のソースアーカイブ。`~/.cache/docs-rs-mcp/` にディスクキャッシュする

**特徴**: docs.rs の主要メンテナが作った「LLM 向け docs.rs API」。各ツールの説明文が「LLM がどう問うか」を基準に書かれていて、「X には使うな、Y を使え」という否定ルールまで入っています。

**Pros**

- 作者が docs.rs の中の人。docs.rs 公式 MCP の原型になる可能性がある (README に「うまくいけば拡張版をホストすることも考える」とある)
- trait 実装者の一覧、型が実装している trait の一覧 (Send/Sync 判定を含む)、feature flags、changelog、examples、README を個別のツールで引ける。この組み合わせは ferritin 系にも無い
- crate 内の re-export を解決する (`axum::Router` → `axum::routing::Router`)。crate をまたぐ re-export は、参照先の crate と版を示すところまで。ただし結果の `path` は `paths` の定義パスで、private module を含みうる (axum では 55 件。例: `axum::form::Form`)
- `#[doc(alias)]` も検索の対象にする
- target triple を意識する。ホストの target で docs.rs のビルドが無ければ、docs.rs 既定の target にフォールバックする (ただしホストの target はサーバーをビルドしたときのもので、フォールバック先の具体的な triple は返さない)
- バージョンを勝手に差し替えない。`resolve_version` 以外のツールは `MAJOR.MINOR.PATCH` まで確定していないと拒否する
- 各ツールの説明文を 1 ファイルずつ管理し、長さ (150 語以内) や戻り値の記述をテストで検査している

**Cons**

- alpha。作者自身が「ローカルで使っている実験」と書いている
- **rustdoc JSON の形式 57 しか読めない** (形式番号の完全一致で判定。README の「nightly 2025-08-02 以降」とは食い違う)。docs.rs で形式 56 以前か形式 58 以降 (2026-06 頃以降のビルド) でビルドされたリリースはエラーになる
- docs.rs 専用なので、std (docs.rs は std の JSON を配っていない)・workspace・非公開 crate は扱えない。Cargo.lock も読まない
- 当初は必ず `resolve_version` を先に呼ぶ規約だった (2026-08-21 から「Cargo.lock の版か完全な版が手元にあれば不要」に緩和)
- ツールが 14 個あり、ツール定義がコンテキストを食う (Claude Code は MCP ツールを遅延ロードするので、影響は限定的)
- シグネチャを rustdoc の生の構造 (数値 Id を含む) で返すため、描画済みの文字列の約 5〜6 倍のトークンを使う。`list_methods(Router)` で約 8.7k トークン (再現値: 44 メソッドで 36,318 文字)。説明文は「blanket impl を省く」と言うが、44 件中 18 件は blanket impl 由来で、フィルタが効いていない
- `get_item` でメソッド (`Router::route`) を引けない (モジュールしかたどらない) のに、`list_methods` の説明文は `get_item` へ誘導する。trait 名を短い名前で返すため、別物の `ServiceExt` 3 つ (`axum::service_ext`・`tower::util`・`tower_http::service_ext`) を区別できない
- 検索は部分一致をパス順に並べるだけで、順位付けが無い (main)
- feature flags は Cargo.toml の宣言だけで、どの item がどの feature で有効になるかは分からない
- changelog は範囲を指定しないと全リリース (axum で 90 件、約 10 万文字) を返す
- エラーはすべて JSON-RPC のプロトコルエラーで返る (`isError` を一度も使わない)
- crate 名と target を検証せずにキャッシュのパスへ使う。manifest の `readme` と `[[example]].path` も検証せずに join する。ダウンロードと展開が原子的でない
- メモリ上のキャッシュ (moka) に上限が無い

#### 4. mcp-rustdoc (possdog)

- リポジトリ: <https://tangled.org/poss.dog/mcp-rustdoc> (2026-10-14 時点の本家)
- GitHub mirror: <https://github.com/possdog/mcp-rustdoc> (2026-09-30 作成。Tangled と同じ HEAD `c05cf13`)
- 旧本家: <https://codeberg.org/possdog/mcp-rustdoc> (**2026-07-27 に archived**。HEAD は `970ddef` で、8〜10 月の 4 commits とタグが無い古い内容)
- バージョン: tag v0.1.0。最終コミット 2026-10-01 (以後の活動なし)。★0
- ライセンス: AGPL-3.0-only (コードは流用できない)
- ツール (6): `list-workspace-crates` / `list-items` / `get-item` / `search` (fuzzy) / `implementations` / `implementors`
- データソース: 公開 crate は docs.rs の rustdoc JSON。workspace crate はローカルの `cargo +nightly rustdoc`

**特徴**: 設計の方向性は rustdoc-mcp (jbr) とほぼ同じです。docs.rs とローカルを併用し、Cargo.lock からバージョンを推定し、feature gate の注記と intra-doc link の解決も行います。ただし、いずれも実装は簡易的です (下記 Cons)。

**Pros**: Cargo.lock 対応、workspace 対応、implementors / implementations の両方向を引ける、ツール数が少ない

**Cons**

- crates.io 未公開でソースからビルドするしかない。AGPL-3.0。README に「written by a human-led AI agent」とある。利用者はゼロ。jbr 版より優位な点が見当たらない
- 8〜10 月の変更 (latest を crates.io API から取る、24 時間 / 404 用 1 時間の TTL のキャッシュ、single-flight) では、下の Cons は 1 つも直っていない
- ソースは表示しない (`file:line` と docs.rs のソース URL だけを返す)
- workspace のビルドに `--document-private-items` を付けないので、公開 item だけ
- 版を省いた呼び出しのたびに workspace の crate を `cargo +nightly rustdoc` でビルドし、呼び出しごとに `cargo metadata` を実行する。メモリ上のキャッシュが無く、毎回ディスクの非圧縮 JSON を解析する
- キャッシュの鍵の target が `x86_64-unknown-linux-gnu` に固定されている。CI は無い
- std には非対応
- Cargo.lock に同名の複数版があると、最初に見つかった版を黙って使う
- feature gate の注記は、item 自身の属性文字列をそのまま出すだけ (親モジュールからの継承が無く、誤検出もある)
- intra-doc link の解決は文字列置換で、表示テキストを定義パス (private なものを含む) で置き換える
- `implementations` は auto trait の impl を除くので、「Send か」に答えられない
- workspace のビルドはユーザーの `target/` を使い、失敗すると黙って docs.rs に切り替える (同名の別 crate を取ってきうる)
- キャッシュのパスに crate 名と版を検証せずに使う (絶対パスを渡すと、`join` がキャッシュの基点ごと置き換える)

#### 5. docsrs-mcp (dmvk)

- リポジトリ: <https://github.com/dmvk/docsrs-mcp>
- crates.io: <https://crates.io/crates/docsrs-mcp>
- バージョン: crate `docsrs-mcp` 0.1.0 (2026-02-08、唯一のリリース)。最終コミット 2026-02-08。★6、累計 482 DL
- fork の [befedo/docsrs-mcp](https://github.com/befedo/docsrs-mcp) (2026-09-17) に、Issue #5 を直すコミット (umbrella crate の公開 re-export の解決、語ごとの検索) があるが、本家への PR は出ていない
- ライセンス: Apache-2.0 のみ (MIT OR Apache-2.0 のプロジェクトにコードを写すと MIT 側の条件を満たせないので、発想だけを借りる)
- 導入: `cargo install docsrs-mcp`
- ツール (4): `lookup_crate_items` / `lookup_item` / `search_crate` (exact > prefix > 名前に含む > パスに含む > ドキュメントに含む の順で順位付け、Levenshtein で「もしかして」を提示) / `lookup_impl_block` (型の trait 実装。説明文は「trait の実装者も引ける」とするが未実装)
- データソース: docs.rs の rustdoc JSON。形式 53〜57 を 0.56 の形に正規化して読む。その際、すべての item の属性 (`attrs`) を空にする。形式の上限は判定していない (README は「53–57+」。58 以降が読めるかは未確認)

**特徴**: nightly 不要の docs.rs JSON 型で、**バージョン解決が「明示指定 > Cargo.lock > latest」** の順になっています。docs.rs 型の中では ferritin の弱点 (Cargo.lock を読まない) を直接埋める設計です。

**Pros**

- nightly も API キーも不要。単一バイナリ
- Cargo.lock を読む (作業ディレクトリから上方向に探す。ただし起動時に一度だけで、`cargo update` の後も起動し直すまで古いバージョンを使う)
- 型の trait 実装を引ける。Issue 報告者が Claude Code での体験を「impl block lookups … genuinely great」と評価している
- ディスクキャッシュがある (`--no-cache` / `--clear-cache`)

**Cons**

- 2026-02 から機能開発が止まっている。dependabot の PR (#12、#14〜#17) が 3 月から未マージのままで、これまでにマージされた PR は 0 件
- 「もしかして」の編集距離をバイト長で数えており、ASCII 以外の名前で値が狂う
- 「見つからない」は成功扱いで返る (取得の失敗は `isError: true`)。メソッドの無い trait impl に「auto-derived」と誤った印を付ける
- std は扱えない (README の例は `std` だが、docs.rs が std の JSON を返さないので失敗するとみられる。実行しての確認はしていない)
- workspace と非公開 crate は扱えない
- re-export 中心の crate (`compio` など) では、内部 crate 名で引く必要がある ([Issue #5](https://github.com/dmvk/docsrs-mcp/issues/5))。原因は、item を定義パス (private module を含む) でしか索引しておらず、`use` による re-export をたどらないこと
- Cargo.lock に同じ crate の複数バージョンがあると、バージョンを**文字列で比較**して大きいほうを黙って選ぶ (`0.9.0` が `0.10.0` より大きいと判定される不具合)
- 属性をすべて捨てるので、must_use・non_exhaustive・doc(hidden)・cfg が分からない
- 検索はクエリ全体を 1 つの部分文字列として照合する (複数の語では見つからない)

#### 6. rust-docs-mcp (snowmead)

- リポジトリ: <https://github.com/snowmead/rust-docs-mcp>
- crates.io: <https://crates.io/crates/rust-docs-mcp>
- バージョン: crate 0.1.1 (2025-10-03)。main ブランチは 0.1.2 で未リリース。最終コミット 2026-09-21 (PR #65 と #48 をマージし、Issue #49・#50・#59・#61・#64 を close)。★135、累計 1,555 DL
- ライセンス: MIT
- 導入: `cargo install` (古い版になる)、`install.sh` (git clone してビルド)、またはソースからビルド
- ツール (15 + prompt 1):
  - キャッシュ管理 6 個
  - ドキュメント照会 6 個 (`search_items_preview` / `get_item_source` を含む)
  - 解析 3 個 (`get_dependencies` / `structure` / `search_items_fuzzy`、Tantivy による全文検索)
- データソース: crates.io・GitHub・ローカルパスから crate のソースを取り、既定では `nightly-2026-05-22` で rustdoc JSON を自前で生成する (環境変数 `RUST_DOCS_MCP_TOOLCHAIN` で上書きでき、PATH 上の cargo への切り替えもある)。docs.rs は使わない

**特徴**: 完全オフラインで動き、非公開の GitHub リポジトリも対象にでき、ソース・依存ツリー・モジュール構造まで出せます。

**Pros**

- キャッシュした後は完全オフラインで動く
- 非公開リポジトリを扱える (`GITHUB_TOKEN`)。feature の組み合わせごとにキャッシュを分ける (main のみ。PR #65)。ビルドの成果物に、要求した feature と実際の feature・toolchain・形式を記録し、作業用の場所から rename で原子的に公開する
- crate 名を検証し、`-` で始まる feature 名を拒否する
- ソース表示が一番充実している (前後の行も付く)

**Cons**

- README に「entirely vibe coded … without human oversight」と明記されている
- crates.io 版は 1 年古く、実際には main からビルドすることになる
- ツールが 15 個あり、コンテキストを食う。preview → details の ID 経由で何手も要る
- `cache_crate` はバックグラウンドタスクで task ID を返すので、エージェントに管理させる必要がある。一方、照会系のツールはキャッシュに無ければその場で同期的にダウンロードとビルドを行う (rustdoc のタイムアウトは 1,800 秒)
- Cargo.lock の自動同期は無い (prompt で依存を 1 つずつキャッシュさせる方式)。しかも prompt の本文が、改名して存在しないツール名 (`cache_crate_from_cratesio` など) を案内し続けている
- std には対応していない。private items も含めない (`--document-private-items` を渡さない)
- trait implementors を出す機能が無い (`Trait.implementations` を一度も参照していない)
- **シグネチャの描画で情報を落とす**: generics は `<...>`、引数は名前だけで型が無く、戻り値は `-> ...` になる
- 日付を固定した nightly では新しい MSRV の crate が失敗する。この Issue #64 は main で close された (固定日付の更新、環境変数での上書き、MSRV エラーの案内) が、使える toolchain は形式 57 を出すもの (rustdoc-types 0.57.3) に限られ、現行の nightly (形式 61) は使えないので、根本の問題は残る。crates.io の 0.1.1 は未修正
- 1 つのビルドがサーバー全体のロックを握り、その間は他の crate への問い合わせも止まる (PR #65 で variant ごとの OS ロックも足したが、全体のロックは残っている)
- ローカルのソースは Cargo.toml の版をキーにキャッシュするので、編集しても `update=true` を渡すまで反映されない
- ビルドごとに使い捨ての target dir を使うので、毎回コールドビルドになる
- `--all-features` を最初に試すため、相互排他の feature を持つ crate で失敗する (Issue #59)。main では全ツールに `features` / `no_default_features` / `all_features` が付き、省略時は all → default → no-default の順に試して `compile_error!` も検出するようになった (PR #65)。crates.io の 0.1.1 は未修正
- ツールの引数で任意のローカルパスと GitHub URL を受けてビルドする (build.rs と proc-macro が走る)。`get_item_source` は span のファイル名を検証せずに連結して読む (絶対パスならキャッシュの外を読む)
- エラーは「# Error」の Markdown を成功として返す (`isError` が付かない)。`serverInfo.version` が "0.1.1" に固定されている。MCP SDK は古い rmcp 0.8

### Tier B: 用途を限れば使える

#### 7. ruskel (cortesi、MCP モード)

- リポジトリ: <https://github.com/cortesi/ruskel>
- crates.io: <https://crates.io/crates/ruskel>
- crate 0.0.11 (**2026-01-22 公開で、以後リリースが無い**)。最終コミット 2026-09-25。★30、約 31.9k DL。MIT
- `ruskel --mcp` で、ツール `ruskel` を 1 個だけ公開する。crate の公開 API の「骨格」(シグネチャとドキュメント、本体なし) を返す。引数 `search` / `search_spec` (name・doc・signature・path) / `direct_match_only` で絞り込める (MCP には 2025-09-17 から)
- データソース: ローカル nightly の rustdoc JSON。std は `rust-docs-json` から。`--offline` と `--private` がある

**Pros**: crate 全体の API の形を一度で掴める。シグネチャを対象にした検索がある。std に対応。DL 数は候補中で最多級。保守が継続している (2026-08〜09 に約 25 commits。workspace の API catalogue、async_trait の復元、symlink を拒否するキャッシュなど。いずれも未リリース)。キャッシュの作りが丁寧 (所有マーカー、`CACHEDIR.TAG`、toolchain の識別子、symlink をたどらない削除)

**Cons**

- 粒度が粗く、大きな crate だと出力が巨大になる (上限もページングも無い)。implementors の一覧は無い。「調べる」より「全体を読ませる」用途向き
- **main は rustdoc JSON の形式 61 しか読めず、`cargo install` で入る 0.0.11 は形式 57 しか読めない** (rustdoc-types 0.57.0)。浮動の `nightly` を使うので、リリース版は現行の nightly では解析に失敗する見込み (未実行)。実行時に rustfmt が要る
- trait の名前で derive を推測するので、手書きの impl を `#[derive]` と誤表示しうる
- 型のパスをソースに書かれたまま (`self::super::Size` など) 表示する。公開パスへの解決 (`PublicPathIndex`) は snapshot / catalogue のモードでしか使わない
- MCP SDK は自作の tmcp。TCP モードは認証が無い。任意の `crate@ver` とローカルパスを受けてビルドする。CI が無い

#### 8. docs-mcp (mmgeorge)

- リポジトリ: <https://github.com/mmgeorge/docs-mcp>
- crates.io: <https://crates.io/crates/docs-mcp>
- crate `docs-mcp` 0.1.2 (2026-09-27)。★5。MIT
- ツール (12): `crate_item_list` / `crate_item_get` / `crate_impls_list` / `crate_versions_list` / `crate_dependencies_list` / `crate_dependents_list` / `crate_downloads_get` など
- データソース: docs.rs の rustdoc JSON、crates.io API、sparse index。nightly 不要

**Pros**: crates.io の情報 (dependents・ダウンロード数) も一緒に引ける。sparse index の feature map を使う

**Cons**: Cargo.lock・std・ローカル crate に非対応。利用者は少ない (96 DL)。ferritin に対する決定的な優位は無い。保守は活発ではない (全 18 commits のうち 15 が 2026-02-18〜20 の 3 日間、残る 3 が 09-27 の rmcp 3.4 への移行)。プロトコルを 2024-11-05 に固定している。エラーを JSON-RPC の `internal_error` で返す。`crate_item_get` が要求した版ではなく最新安定版の feature を付ける。全データの TTL が一律 1 日で、不変の版まで捨てる

#### 9. rustdocs-mcp (JLFN) / rustdoc-query (gnufood)

どちらも 2026 年夏に出た docs.rs JSON 型で、★0 です。

- **[rustdocs-mcp (JLFN)](https://github.com/JLFN/rustdocs-mcp)** ([crates.io](https://crates.io/crates/rustdocs-mcp)): crate 0.1.1 (2026-08-05)。ツール 11 個 (`get_source` / `list_impls` / `resolve_version` など)。`.crate` アーカイブからソースも返す。**読める形式は 60 だけ** (完全一致。57 や 61 でビルドされたリリースは読めない)。エラーを `{"error": ...}` の成功として返す。テストも CI も無い
- **[rustdoc-query (gnufood)](https://github.com/gnufood/rustdoc-query)** ([crates.io](https://crates.io/crates/rustdoc-query)): crate 0.2.0 (2026-09-10)。ツール 3 個。JSON か TOON 形式で出力する (形式は起動時に選び、呼び出しごとには選べない)。ライブラリとして組み込むこともできる。JSON パッチで形式 50 から 61 へ移行して読む。**MCP への準拠は候補中で最良**: `structuredContent` と `outputSchema` を出し、`isError` を正しく付け、エラーに `code` / `diagnostic_id` / `retryable` を持つ。確定した版は不変、`latest` は TTL 5 分とキャッシュを分ける。cargo-dist で配るが Linux だけ

**Pros**: nightly 不要で、設計は今どきの方式。どちらも MIT OR Apache-2.0 で、コードを流用しやすい

**Cons**: どちらも issue も PR も 0 件で、実績が無い。公開後のコミットも無い (JLFN は 10 commits がすべて 1 時間以内。gnufood は 2026-06〜09 の 22 commits を、公開日 (09-10) に作った GitHub のリポジトリへまとめて載せ、以後のコミットが無い)。Cargo.lock・std・ローカル crate に非対応

#### 10. mcp-rust-docs (46ki75、v1)

- リポジトリ: <https://github.com/46ki75/mcp-rust-docs>
- crates.io: <https://crates.io/crates/mcp-rust-docs>
- crate 1.0.0-alpha.1 (2026-05-24)。★0。Apache-2.0
- ホスト版: `https://mcp-rust-docs.vercel.app/mcp` (MCP エンドポイント。2026-10-14 に稼働を確認)
- ツール (4): `search_crates` / `get_crate_docs` (docs.rs の HTML を Markdown 化) / `search_crate_symbols` / `search_crate_docs` (rustdoc JSON の全文検索)

**Pros**: ホスト版があるのでインストール不要。バージョンの完全一致指定ができる (二次調査の実測)。テストと CI がある。全ツールに annotations を付け、`isError` を正しく使う (protocolVersion 2025-06-18)。応答の本体と zstd の展開に上限を設けている

**Cons**

- **`search_crate_docs` が現在壊れている** (実測: 現行の tokio と serde で "tried [57, 56]" エラー。2026-10-14 に再確認。コード上の対応形式は 56 と 57 のまま)
- **live の docs.rs に対する週次の CI が、2026-09-14〜10-12 の 5 回連続で失敗したまま放置されている**。その後の変更は dependabot だけ
- std は扱えない (実測で 404)
- ページ出力が冗長 (`tokio::sync::Mutex` で 28,850 文字。2026-10-14 は 28,486 文字)。`resolved_version` が "latest" のままで、具体的な版に解決しない
- 第三者の無料 Vercel 環境に依存する。配布バイナリに Windows が無い

### Tier C: 実験的・小規模・保守停止気味

| 名前 | 概要 | 主な難点 |
| --- | --- | --- |
| [cratedex (0xCarbon)](https://github.com/0xCarbon/cratedex) | crate 0.5.3 (2026-02)。ローカル nightly JSON を SQLite FTS5 で索引化。workspace 対応。HTTP デーモンとして共有もできる | 2026-02 から停止。★0。依存 crate はビルドしない (キャッシュ済みのものだけ)。JSON があれば再生成しないので、コードを変えても索引が古いまま |
| [rustdoc-mcp (Xevion)](https://github.com/Xevion/rustdoc-mcp) | ローカル JSON。nightly の `rust-docs-json` から std を読む (導入方法も案内する)。検索は調べたローカルビルド型の中で最も良い (CamelCase 分割、正準パス優先)。最終コミット 2026-09-21 で活動中 | README に「ALPHA」と明記。ライセンスは Cargo.toml で MIT OR Apache-2.0 と宣言しているが、LICENSE ファイルが無い (GitHub 上は none)。crates.io 未公開。依存のビルドを `~/.cargo/registry/src` の中で実行し、依存を再解決する (nightly cargo の feature resolver の panic を避けるためとコードに書かれている) |
| [rustdoc-mcp-server (MasYaaz)](https://github.com/MasYaaz/rustdoc-mcp-server) | ローカル JSON を SQLite FTS5 で索引化。ツール 10 個 | 2026-09 に作られたばかり。直接依存しか扱わない。保存時と問い合わせ時で kind の名前が食い違っており (`"Function"` と `'function'` など)、`get_function_signature` など 5 つのツールが常に空を返す。**引数の `kind` を SQL に文字列連結しており、SQL インジェクションが可能**。`ORDER BY rank` が `IN` サブクエリの中にあり、FTS の順位が捨てられる。stable に `RUSTC_BOOTSTRAP=1` を付けて生成する。JSON-RPC が自前実装 |
| [rustdoc-mcp-server (KevinT3Hu)](https://github.com/KevinT3Hu/rustdoc-mcp-server) | crate 0.1.1。Cargo.lock 対応、ローカル JSON | 「プロジェクトで解決された feature を再現する」は**依存 crate では必ず失敗する** (§7。手元の実験で確認)。rustdoc-types 0.57 固定で、形式番号を確かめずにデシリアライズするので、現行の nightly では壊れている可能性が高い。ユーザーの `target/` に書き、JSON があれば新しいとみなす。2026-02 から停止。LGPL-3.0 |
| [mcp-docs-rs (BrightX)](https://github.com/BrightX/mcp-docs-rs) | stable の `cargo doc` HTML を索引化。完全オフライン。ツール 18 個 | 2026-10 に作られたばかり。HTML 由来で精度が劣る |
| [mcp-docsrs (vexxvakan)](https://github.com/vexxvakan/mcp-docsrs) | docs.rs JSON。target triple を指定できる。`outputSchema` と `structuredContent` を使う | README が言う binaries と Docker image が実在しない (releases は 0 件、ghcr は匿名で取れない)。crate 内検索が無い。`symbolType` の指定が必須。main の最終コミットは 2026-04-20 (10-04 の push は bot のブランチ) |
| [mcp-rustdoc (kieled)](https://github.com/kieled/mcp-rustdoc) | npm 5.0.0。docs.rs と doc.rust-lang.org の HTML から std も取る。ツール 13 個 | HTML スクレイピング。2026-02 から停止 |
| [jons-mcp-docs.rs (jonmmease)](https://github.com/jonmmease/jons-mcp-docs.rs) | Python。docs.rs JSON。Cargo.lock から crate 一覧を作り、**バージョンも解決**して取得元を `explicit` / `cargo_lock` / `default` のラベルで返す。ツール 16 個 (implementors、`compare_versions`、`lookup_feature_flags`、`lookup_release_notes` など) | ★1。PyPI 未公開。2026-02 から停止 |
| [docsrs-mcp (RimoVR)](https://github.com/RimoVR/docsrs-mcp) | Python。ローカル埋め込みで semantic 検索。std 対応 | 2025-09 から停止。メモリ最大約 1GB。引数を文字列で渡す必要がある |
| [crates-docs (KingingWang)](https://github.com/KingingWang/crates-docs) | docs.rs HTML。HTTP/SSE、Redis キャッシュ、Prometheus 対応。crates.io に `crates-docs` 1.0.0 (2026-05-31、498 DL)、Docker Hub にも公開。★14 | 運用向けで重い。HTML 由来。最終コミット 2026-08-04 |
| [docsrs-mcp (maan2003)](https://github.com/maan2003/docsrs-mcp) | docs.rs JSON。形式ごとに別の版の rustdoc-types (0.46.1、0.48〜0.53) を持ち、形式番号で読み分ける。**形式 46 を読める唯一の MCP 実装** | 1 日だけの実験 (2025-06)。★0。現行の形式は読めない |

**三次調査で見つかった候補** (小規模なものが中心。rustdoc JSON 型でないものと、最終行の 2026 年初めまでに止まったものを含む):

| 名前 | 概要 | 主な難点 / 参考になる点 |
| --- | --- | --- |
| [rust-knowledge-mcp (afnanenayet)](https://github.com/afnanenayet/rust-knowledge-mcp) | `cargo metadata` の PackageId を鍵にする。nightly の JSON、intra-doc link の解決。README と `docs/*.md` も tantivy で索引する | ★0、2026-09。散文 (README・docs) も横断検索する点が参考になる |
| [groxide (jhartquist)](https://github.com/jhartquist/groxide) | CLI (crate 0.1.0)。ローカル nightly の JSON。std・依存・crates.io を自動取得。skill 同梱。JSON Lines 出力 | ★1、2026-05。一覧に `[feature: sync]` を注記する。「build.rs / proc-macro が実行される」と明記している |
| [docsrs / mx-docsrs (akesson)](https://github.com/akesson/docsrs) | docs.rs とローカルを `crate[@ver][::path]` で引く。MCP と skill | ★0、2026-05。`--install-skill [--scope project]` が冪等で、編集済みの skill は `--force` が無いと上書きしない |
| [truesight (manaforged)](https://github.com/manaforged/truesight) | rustdoc JSON から API リファレンス・`api/<pkg>.txt`・llms.txt を作り、CI で差分を検査する (MCP ではない) | ★0、2026-10。定義パスが非公開なら impl を最短の公開パスの下に載せる。`#[cfg(feature=…)] pub fn path(sig)` の 1 行表記 |
| [cargo-aidoc / aidoc-mcp (ynishi)](https://github.com/ynishi/cargo-aidoc) | rustdoc JSON → llms.txt / Markdown / 決定的な JSON | crate 0.4.0、1,021 DL。成果物の生成用 |
| [rust-mcp (cylewitruk)](https://github.com/cylewitruk/rust-mcp) | 35 ツール。Docker と組み込み PostgreSQL。`~/.cargo/registry` を監視して自動で索引。OSV | ★0、2026-05。ツール数と運用の重さは反面教師。新しくダウンロードされた crate の自動索引は参考になる |
| [ferrisfetch (jcdickinson)](https://github.com/jcdickinson/ferrisfetch) | Go。docs.rs JSON の埋め込み検索 (Voyage AI 必須)。内容アドレスでバージョン間の重複を除く。`CLAUDECODE=1` でヘルプ出力を変える | ★1、2026-08 |
| [dociium (labiium)](https://github.com/labiium/dociium) | Rust / Python / Node のドキュメントと import の解決 (crate 1.0.0) | ★1、2026-09 |
| [md-docrs-proxy (ThomAub)](https://github.com/ThomAub/md-docrs-proxy) | docs.rs JSON を Markdown に変換。Cloudflare Worker 版 | ★3、2026-05 |
| [rexplore (colossoi)](https://github.com/colossoi/rexplore) | ローカル nightly の JSON から API を `use` 付きの Rust コードとして出す。正規表現で絞り込める | ★0、2026-09 |
| [bevy_manual_mcp (kinkirill)](https://github.com/kinkirill/bevy_manual_mcp) | Bevy 専用 | ★12、2026-10 |
| 2026 年初めまでに止まった見落とし | [cursor-rust-tools](https://github.com/terhechte/cursor-rust-tools) ★83 (cargo doc の HTML と rust-analyzer)、[mcp-package-docs](https://github.com/sammcj/mcp-package-docs) ★79 (多言語、docs.rs)、[bookworm](https://github.com/dcdpr/bookworm) ★4 (docs.rs を SQLite に)、[cargo-mcp](https://github.com/camshaft/cargo-mcp) ★4、[windows-rs-mcp](https://github.com/preludeorg/windows-rs-mcp) ★14 (windows crate 専用)、[Doc-Mcp](https://github.com/mosif16/Doc-Mcp) ★11 | 新しい観点は無い |

### Tier D: 非推奨

| 名前 | 理由 |
| --- | --- |
| **[rust-docs-mcp-server (Govcraft)](https://github.com/Govcraft/rust-docs-mcp-server)** ★299 | OpenAI API キーが必須で、ドキュメントを OpenAI に送る。返すのは生のドキュメントではなく GPT-4o-mini の要約 (損失があり、ハルシネーションもありうる)。1 プロセスで 1 crate しか扱えず、複数起動しても全インスタンスのツール名 (`query_rust_docs`) と説明文が同じで、crate 名は instructions にしか出ないので、LLM がどれを呼べばよいか区別できない。最終リリースは 2025-05。作者自身が「it only works part of the time」と書いている ([Issue #18](https://github.com/Govcraft/rust-docs-mcp-server/issues/18)) |
| **[Daipendency](https://github.com/daipendency/daipendency-mcp)** | ツールが 1 個で crate 全体を丸ごと返す。tree-sitter 解析なので **impl block が丸ごと抜ける** ([Issue #1](https://github.com/daipendency/daipendency-extractor-rust/issues/1)) (メソッドも trait 実装も出ない)。macro 生成の API も見えない。2025-02 から停止 |
| **[cargo-doc-mcp (spacemeowx2)](https://github.com/spacemeowx2/cargo-doc-mcp)** | ローカルの `cargo doc` HTML を丸ごと Markdown 化するのでトークン効率が悪い。検索は部分一致のみ。`rustdoc://` リソースがパスを検証しないので、任意のローカルファイルを読める (コードを読んでの推定)。npm 未公開。2025-03 から停止 |
| [mcp-rustdoc-parser (automataIA)](https://github.com/automataIA/mcp-rustdoc-parser) | PoC。stdio サーバーなのに `println!` で stdout に書いており、JSON-RPC が壊れる可能性が高い。`rustdoc-md` を勝手に `cargo install` する |
| [mcp-rust-docs (0xKoda)](https://github.com/0xKoda/mcp-rust-docs) ★47 / [trungnguyenvn](https://github.com/trungnguyenvn/mcp-rust-docs) | docs.rs の crate ルートページしか取れない。放棄されている |
| [rust-docs-mcp-server (laptou)](https://github.com/laptou/rust-docs-mcp-server) | 「Fails to get documentation for any crate」([Issue #3](https://github.com/laptou/rust-docs-mcp-server/issues/3))。放棄されている |
| [mcp-rust-docs-deprecated (46ki75)](https://github.com/46ki75/mcp-rust-docs-deprecated) | archived。`cargo install` が失敗する ([Issue #80](https://github.com/46ki75/mcp-rust-docs-deprecated/issues/80)) |
| [mcp-docsrs (shuakami)](https://github.com/shuakami/mcp-docsrs)、[docs-rs-mcp (nuskey8)](https://github.com/nuskey8/docs-rs-mcp)、[crates-mcp (pato)](https://github.com/pato/crates-mcp)、[mcp_rustdoc (cyberelf)](https://github.com/cyberelf/mcp_rustdoc)、[cratedocs-mcp (lnay)](https://github.com/lnay/cratedocs-mcp) | docs.rs HTML スクレイピング型で、2025 年のうちに停止 |
| **rust-docs-mcp-server (shark-hunt)** | Govcraft の README とコードの無断コピーで、自動コミットで更新を装っている。サプライチェーン攻撃の餌の典型的な形。**入れないこと**。誤クリック防止のためリンクにしていない (`github.com/shark-hunt/rust-docs-mcp-server`)。三次調査で API だけを使って確認: README は末尾の改行を除いて同一。git tree の 17 blob のうち 13 が SHA まで一致し (`src/*.rs`・Cargo.toml・Cargo.lock・release.yml を含む)、違うのは README・CHANGELOG.md・`.chglog/CHANGELOG.tpl.md`・`.chglog/config.yml` の 4 つだけ。fork ではない。29 commits はすべて「add content / delete format」の自動的な反復。アカウントは 2026-02-27 作成で、AI / MCP 系をコピーした公開リポジトリが 50 個ある。**HEAD のソースは 2026-10-14 時点で未改変**で、危険は今後の差し替えやリリースにある |

---

## 5. 主要候補の機能比較

| | [rustdoc-mcp (jbr)](https://github.com/jbr/ferritin/tree/main/rustdoc-mcp) | [ferritin (ホスト版)](https://github.com/jbr/ferritin) | [docs-rs-mcp (syphar)](https://github.com/syphar/docs-rs-mcp) | [mcp-rustdoc (possdog)](https://tangled.org/poss.dog/mcp-rustdoc) | [docsrs-mcp (dmvk)](https://github.com/dmvk/docsrs-mcp) | [rust-docs-mcp (snowmead)](https://github.com/snowmead/rust-docs-mcp) |
| --- | --- | --- | --- | --- | --- | --- |
| Transport | stdio | HTTP (リモート) | stdio | stdio | stdio | stdio |
| ツール数 | 4 | 2 | 14 | 6 | 4 | 15 + prompt 1 |
| 読める形式 (format_version) | 48〜61 (より新しい追加的な形式も) | 同左 | 57 のみ | 57 (旧形式は一部補正) | 53〜57 (属性は捨てる。上限の判定なし) | 57 (自前生成) |
| データソース | ローカル nightly JSON + docs.rs JSON | docs.rs JSON | docs.rs JSON + .crate | docs.rs JSON + ローカル nightly | docs.rs JSON | 自前生成の nightly JSON |
| nightly | 必須 | 不要 | 不要 | workspace 用に必要 | 不要 | 必須 (既定は日付固定) |
| API キー | 不要 | 不要 | 不要 | 不要 | 不要 | 任意 (GitHub) |
| Cargo.lock 連動 | ○ (同名の複数版は衝突) | × (`@=` で手動) | × | △ (複数版は最初に見つかったもの) | △ (起動時に一度だけ。複数版は文字列比較) | × (手動) |
| workspace / 非公開 crate | ○ (private items 含む) | × | × | ○ (公開 item のみ) | × | ○ (private items は含まない) |
| std / core / alloc | ○ (要 `rust-docs-json`) | ○ (nightly) | × | × | × | × |
| trait implementors | **×** (独自の描画処理に節が無い) | ○ (定義元 crate 内、20 件で打ち切り) | ○ (定義元 crate 内) | ○ (定義元 crate 内) | × (説明文のみで未実装) | × (機能が無い) |
| ソース表示 | △ (workspace とローカルでビルドした依存のみ) | × (CLI のみ) | △ (item のソースは無い。examples は `include_content=true` で中身も返す) | × (`file:line` と docs.rs の URL のみ) | × | ◎ |
| feature flags / changelog | × | × | ○ | 注記のみ | × | × |
| オフライン | キャッシュ済みとローカルビルド分は可 | 不可 | キャッシュ分のみ | キャッシュ分のみ | キャッシュ分のみ (`latest` は除く) | ○ |
| 出力量の調整 | ○ (`verbosity`) | × | ○ (`get_item` の `verbosity`、検索の `limit`、examples の `include_content`) | × | × | preview モード、limit / offset |
| キャッシュの寿命 | 1 回の呼び出しの中だけ (Store を毎回作り直す) | 長命 (メモリ + ディスク) | ディスク + 上限なしのメモリ | ディスクのみ (毎回解析) | ディスク + メモリ | ディスク |
| 最終リリース | 0.10.0 (2026-08-07) | 0.17.0 (2026-08-07。ライブは未リリースの main) | 0.1.0-alpha.4 (2026-06-20) | v0.1.0 (タグのみ) | 0.1.0 (2026-02-08) | 0.1.1 (2025-10-03) |
| 保守状況 | 継続 (main は 10-02 が最後、リリースは 2 か月滞留) | 同左 | main は 08-21 で停止 (未マージのブランチあり) | 10-01 以降なし (Tangled に移転) | 停止 (fork に修正あり) | 断続的 (main に大きな修正、未リリース) |

### 5.1 安全性・MCP への準拠・流用可否の比較 (三次調査で追加)

§2 の基準 6・7 による比較です。根拠は主にソースの精読です (ferritin のエラーの扱いはホスト版での実測を含む)。

| | rustdoc-mcp (jbr) | ferritin (ホスト版) | docs-rs-mcp (syphar) | mcp-rustdoc (possdog) | docsrs-mcp (dmvk) | rust-docs-mcp (snowmead) | rustdoc-query (gnufood) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| ライセンスと流用 | MIT OR Apache-2.0。流用可 | 同左 | 宣言は MIT OR Apache-2.0 だが本文なし | **AGPL-3.0-only。流用不可** | Apache-2.0 のみ。発想だけ借りる | MIT。帰属表示で流用可 | MIT OR Apache-2.0。流用可 |
| 依存のコードを実行するか | する (`cargo doc`。依存の build.rs・proc-macro) | しない (docs.rs の JSON) | しない | する (workspace の crate を呼び出しのたびにビルド) | しない | する (任意の crate をビルド) | しない |
| 任意のパス・URL のビルドを受け付けるか | 受け付ける (`set_working_directory` で指した先) | — | — | 未確認 | — | **受け付ける** (ローカルパスと GitHub URL) | — |
| パスの検証 | span の絶対パスを検証しない。crate 名の検証は見当たらない (推定) | — (MCP にソース表示が無い)。crate 名の検証は見当たらない (推定) | crate 名・target・manifest のパスを検証しない | crate 名・版を検証しない (トラバーサル可能) | 検証している | crate 名と feature 名は検証。span のパスは検証しない | — |
| エラーの返し方 | 失敗は `isError`。「見つからない」とビルド失敗は成功扱いの文字列 | `isError` を計算して捨てる。全部成功扱い | **すべて JSON-RPC のプロトコルエラー** | -32603 のプロトコルエラー。「見つからない」は成功扱い | 取得の失敗は `isError`。「見つからない」は成功扱い | 「# Error」の Markdown を成功扱い | **`isError` を正しく付け、`code` / `retryable` を持つ** |
| `structuredContent` / `outputSchema` | なし (文字列) | なし | text と structured に同じ JSON を二重に入れる。`outputSchema` なし | 明示的に None | なし (Markdown) | なし | **両方あり** |
| annotations | main のみ (実態と合わない) | あり (main) | なし | 明示的に None | なし | なし | なし |
| MCP SDK / プロトコル | mcplease 0.2 (0.10.0) / 0.4 (main) | mcplease 0.4、2025-11-25 | rmcp 1.7 (2.x の PR が保留) | rmcp 0.14、2025-06-18 | rmcp 0.14 | rmcp 0.8 (古い) | rmcp 1.7 |
| 処理の並行性 | stdio を同期的に 1 件ずつ。ビルド中は全部止まる | HTTP サーバー | 非同期。同時の読み込みは 1 つにまとめる | single-flight | 同時の取得をまとめない | ビルド中はサーバー全体のロック | 非同期 |
| テスト・CI | insta のスナップショット約 50。CI は ubuntu のみ | スナップショット 221。毎日の nightly ジョブが Issue を自動で開閉 | 55 テスト。オフラインのフィクスチャ | 6 テスト。CI なし | 44 テスト | CI は 3 OS | 未確認 |
| LLM による評価 (eval) | なし | なし | なし (説明文の長さと戻り値の記述をテストで検査) | なし | なし | なし | なし |
| 配布と OS | binstall、4 target のバイナリ (Windows あり) | — | cargo install | ソースからのビルドのみ (docs の target は `x86_64-unknown-linux-gnu` に固定) | cargo install | cargo install / install.sh | cargo-dist (Linux のみ) |

---

## 6. 隣接ツール (Rust ドキュメント専用 MCP ではないもの)

| ツール | 位置付け |
| --- | --- |
| **[rust-analyzer-mcp (zeenix)](https://github.com/zeenix/rust-analyzer-mcp)** | crate 0.4.0 (2026-08-22)、★84。`rust_analyzer_hover` で、依存グラフ上のシンボルのドキュメントを引ける。Claude Code ではすでに組み込みの `LSP` ツールが同じ役割を果たすので、追加する意味は薄い |
| **[cratesio-mcp (joshrotenberg)](https://github.com/joshrotenberg/cratesio-mcp)** | crate 0.4.1、★17。crates.io のメタデータ・OSV 監査に加え、docs.rs JSON から `get_doc_item` / `search_docs` を引ける。ホスト版 (<https://cratesio-mcp.fly.dev/>) あり (2026-10-14 に稼働を確認)。**crate 選定や脆弱性確認**用で、ferritin を補完する。ただし脆弱性の照合はバージョンを問わないため誤検出しうる (版指定の照合関数はあるが使われていない) |
| [rustkb (JSBtechnologies)](https://github.com/JSBtechnologies/rustkb) | Claude Code プラグイン + MCP。docs.rs JSON から完全一致バージョンの `get_item` を引けるほか、clippy lint や advisory のガイドを BM25 (tantivy) で検索する。advisory は Cargo.lock の解決済みの版で照合する。skill・agent・コマンド・SessionStart hook を同梱したプラグインの構成が参考になる |
| [rust-docsbox-mcp (afterrealism)](https://github.com/afterrealism/rust-docsbox-mcp) | std と人気 crate のオフラインコーパスに加え、clippy・rustfmt・playground・`rustc --explain` を扱う。**README のホスト版のドメインは NXDOMAIN で消えている** (2026-10-14)。コミットは 2 件で、2026-04-27 から放置 |
| [lockdocs (SylphxAI)](https://github.com/SylphxAI/lockdocs) | 公式 MCP レジストリに載っている多言語 (npm / PyPI / crates.io / Go) のドキュメントサーバー (crate 0.6.0)。Cargo.lock を読む。Rust は `~/.cargo/registry/src` を **tree-sitter** で解析するので、精度は rustdoc JSON 型に劣る。呼び出しごとのトークン予算 (`--tokens`、既定 1,200) と、全区画に `pkg@ver path:line` の引用を付ける設計は参考になる |
| [officina の rustdoc_processor / rustdoc_resolver (LukeMathWalker)](https://github.com/LukeMathWalker/officina) | MCP ではないライブラリ (各 0.1.2、Pavex から分離)。cargo rustdoc → SQLite キャッシュ → 公開 import path と crate 外の re-export の索引。「その item を `use` できる正準パス」を実運用で解いている |
| [Context7](https://github.com/upstash/context7) | ★62,995。docs.rs の人気 crate を事前クロールしたスナップショットを持つ (一部は版付き。例: `/websites/rs_tokio_1_49_0`)。任意の crate@version は引けないので、rustdoc 系の代わりにはならない |
| [arabold/docs-mcp-server](https://github.com/arabold/docs-mcp-server) / [GitMCP](https://github.com/idosal/git-mcp) / [DeepWiki](https://deepwiki.com) | 汎用のドキュメント・リポジトリ Q&A。API リファレンスとしての精度は rustdoc JSON 系に劣る |
| [ferritin skill (`jbr/ferritin/skills/SKILL.md`)](https://github.com/jbr/ferritin/blob/main/skills/SKILL.md) | MCP を使わず、ローカルの `ferritin` CLI をシェルから叩く Claude Code skill (`Bash(ferritin:*)`)。MCP のツール定義でコンテキストを消費しない |
| 上流の動き: [cargo#16720](https://github.com/rust-lang/cargo/issues/16720) | 「`cargo doc --output-format=markdown`: Cargo.lock からエージェント向けの API ドキュメントを作る」提案 (open)。実現すれば、この分野の MCP サーバーの役割の一部を cargo 自体が担う。あわせて cargo#17025 で `cargo doc --output-format json` が unstable として入った (2026-05-27) |

---

## 7. 横断的な所見

- **ビルドしない 2026 年の新しい実装は docs.rs の rustdoc JSON に収束している。** syphar・mmgeorge・JLFN・gnufood・46ki75 (一部のツール)・cratesio-mcp・rustkb がそうです。HTML 型は 2025 年に作られて止まったものが大半で、2026 年のものは kieled (2026-02 に停止)、KingingWang (運用向け)、BrightX (ローカルの `cargo doc` HTML) くらいです。
- **rustdoc JSON の `format_version` の変化が共通の弱点。** 保守が止まったサーバーは、現行の nightly や docs.rs の新しいビルドを読めなくなります。46ki75 は実際に壊れており (実測)、KevinT3Hu も壊れている疑いが強い (ソースからの推定) です。「最近まで保守されているか」は機能差以上に重要です。
- **docs.rs 依存型の構造的な限界**:
  - std / core / alloc を配っていない (`docs.rs/crate/std/latest/json` は 404。2026-10-14 に再確認)
  - 非公開 crate は扱えない
  - docs.rs が 2025-05-23 より前にビルドしたリリースには JSON が無い。基準は**ビルド日**で公開日ではない (古いリリースも再ビルドされれば JSON を持つ。serde 1.0.219 は 2025-03 公開だが JSON がある)
  - 既定でビルドされる target は 5 つで、**`x86_64-apple-darwin` が入っていない** (Intel Mac のホストではほぼ必ずフォールバックになる)

  これらを超えられるのは、ローカルで rustdoc JSON を生成する型 (jbr の rustdoc-mcp、snowmead、ruskel、possdog、Xevion など) だけです。std まで扱えるのは、このうち rustdoc-mcp・ruskel・Xevion です。
- **trait implementors はどの実装でも「定義元 crate 内」に限られる。** crate をまたいだ逆引きができるものは見つかりませんでした。
- **docs.rs はリリースごとに、実際にビルドされた形式の JSON しか配らない。** 再ビルドされたリリースだけは古い形式も残ります。最古では形式 46 が実在します (tokio 1.45.1)。**形式 47 は欠番で存在しません** (rustdoc-types の CHANGELOG)。形式 46 を読める MCP 実装は maan2003 (§4 Tier C) だけで、ferritin の下限は 48、ほかの主要な実装はさらに狭い範囲しか読めません。形式の移行コードとしては migrate_rustdoc_json (41〜50、開発版は 53 まで) があります。
- **rustdoc JSON の `paths` は定義パス (private module を含む) を持つ。** これを使えるパスとして見せる実装 (dmvk、syphar、possdog、ferritin の表示) は、LLM が `use` できないパスを示します。
- **調べたローカルビルド型の多く (jbr・snowmead・cratedex・MasYaaz・KevinT3Hu) は、鮮度判定を誤っている。** 「JSON があれば新しい」とみなすか、見るファイルの範囲を誤っています。
- **読める形式を固定した実装が多く、リリースが形式の変化に遅れると利用者の手元で壊れる。** syphar (57)、JLFN (60)、ruskel のリリース版 (57、main は 61)、snowmead (57)、46ki75 (56・57) がそうです。形式番号と日付 (master に入った日) は、57 が 2025-11-21、58 が 2026-05-31、59 が 2026-06-24、60 が 2026-06-28、61 が 2026-07-28 です。stable の 1.97 / 1.98 はそれぞれ 57 / 60 を出すと推定され、1.99 が 61 を出すことは手元で確認しました (次項)。
- **stable の rustdoc でも JSON を出せる。** `RUSTC_BOOTSTRAP=<crate 名>` (パッケージ名ではなく、アンダースコアの crate 名。副作用は requirements.md の R-WS-8) を付けると、stable 1.99 の rustdoc が形式 61 の JSON を出しました (2026-10-14 に手元で確認)。cargo-semver-checks は本番でこの方式 (`RUSTC_BOOTSTRAP=1`) を使っています。nightly を必須とするローカルビルド型の前提は、絶対ではありません。ただし std の JSON (`rust-docs-json`) は nightly にしかありません (stable と beta の manifest では `available = false`)。
- **std の stability は形式 58 以降の JSON に入っている** (`Item::stability`、59 で `const_stability`、60 で `default_unstable`)。それでも stability を表示する実装は見つかりませんでした。なお unstable には tracking issue の番号も reason も入っていません。
- **依存の feature をフラグで再現する方式は成り立たない。** workspace 外のパッケージに `--features` を渡すと cargo が拒否します (KevinT3Hu の方式)。逆に、workspace のルートからフラグなしで `cargo rustdoc -p <依存>` を実行すれば、workspace で統合された feature が効きます (2026-10-14 に手元で確認)。ただし統合されるのは通常の依存の辺の feature だけで、dev / build 依存だけが有効にする feature は効きません (requirements.md の R-FT-3)。
- **安全性**: ローカルビルド型は依存の build.rs と proc-macro を実行しますが、Claude Code の Bash のサンドボックスは MCP サーバーを覆いません。docs.rs はネットワーク無し・読み取り専用のサンドボックスでビルドしているので、「registry の crate は docs.rs から取り、ビルドするのは workspace だけ」にすると危険を小さくできます。ただし workspace を rustdoc にかけると、その依存もすべて check されるので、依存の build.rs と proc-macro はやはり実行されます。パスや引数を検証しない実装も多く、cargo-doc-mcp (任意のファイルを読める。コードからの推定) と MasYaaz (SQL インジェクション) は実害につながる形です。
- **MCP への準拠はまちまち。** `isError` を正しく使う実装は少数 (gnufood、ruskel、46ki75 など) で、syphar と mmgeorge は失敗を JSON-RPC のプロトコルエラーで返します。`structuredContent` と `outputSchema` まで揃っているのは gnufood と vexxvakan くらいです。
- **LLM による評価 (eval) を持つ実装は 1 つも無い。** 説明文の質を機械的に検査しているのは syphar (150 語以内、戻り値の記述) だけです。
- **MCP 仕様と Claude Code の変化**: MCP の現行仕様 (2026-07-28) は stateless になり、Roots を非推奨にして「ディレクトリはツールの引数で渡せ」と明記しました。Claude Code は MCP サーバーの環境に `CLAUDE_PROJECT_DIR` を入れます。rustdoc-mcp の `set_working_directory` のような状態を持つ方式は、もう必要ありません。

---

## 8. このホストでの導入メモ

(個人の環境の記述なので、2026-10-15 にリポジトリの外へ移した)

---

## 9. 検証の限界

- **どのサーバーもローカルにインストールして動かしていない。** 機能の記述は README・ソースコード・レジストリ API に基づきます。主要な実装 (jbr、syphar、dmvk、possdog、snowmead、cratedex、KevinT3Hu、MasYaaz、Xevion、ruskel など。ほかに requirements.md の参考実装の cargo-public-api) は、二次調査でソースを精読して挙動を確かめました。ホスト版への問い合わせは、下の三次調査の項を参照。
- 実行して確かめていない点: dmvk の std 非対応と、KevinT3Hu が rustdoc-types の形式変更で壊れていること (どちらもソースからの推定)。§5.1 の各セル (ferritin のエラーの扱いを除く) と、cargo-doc-mcp の任意のファイルの読み取り (§4 Tier D) も同様。
- 二次調査では次も実データで確かめました: docs.rs の JSON の形式 (46・53・54・56・57・60・61)、ホスト版 ferritin の出力サイズと応答。
- syphar が docs.rs の主要メンテナであることは、`rust-lang/docs.rs` のコントリビューター 1 位 (1,194 commits、2026-10-14) であることで確認しました。
- スター数・DL 数・日付は GitHub API と crates.io API の値です。日付を書いていない数値は 2026-10-13 時点、2026-10-14 に取り直したものには日付を併記しました。
- **三次調査 (2026-10-14)**: 5 つの観点 (jbr、syphar・dmvk・possdog、Tier A〜B の残り、Tier C〜D と名前と新しい候補、docs.rs・rustdoc JSON・MCP 仕様・Claude Code の文書) に分けて、全記載をソースと API で再検証しました。各リポジトリは clone して読んだだけで、ビルドも実行もしていません。実際に実行したのは、ホスト版 (ferritin、46ki75、cratesio-mcp) への JSON-RPC の問い合わせ、docs.rs への少数のリクエスト、手元の最小の crate での実験 (stable rustdoc + `RUSTC_BOOTSTRAP`、依存の feature の統合、`cargo doc --output-format json` の出力先) だけです。主要な指摘 (rustdoc-mcp の implementors と Store の作り直し、形式 47 の欠番、`RUSTC_BOOTSTRAP`、feature の統合) は、調査をまとめた側で再度確かめました。
- 三次調査でも確かめられなかった点: Windows での実際の動作 (CI で 3 OS を回している snowmead を含む)、ferritin のホスト版のレート制限の設定値、Reddit での評判 (API がブロックされた)。
- 確定させるには、rustdoc-mcp (jbr) とホスト版 ferritin を実際のプロジェクトで並べて試すのが確実です。比べる観点は、同じ質問に対する正確さ・出力トークン量・初回と 2 回目の待ち時間・trait implementors の有無です。

---

## 10. 主な出典

- jbr/ferritin: [リポジトリ](https://github.com/jbr/ferritin) ([`rustdoc-mcp/`](https://github.com/jbr/ferritin/tree/main/rustdoc-mcp)、[`skills/SKILL.md`](https://github.com/jbr/ferritin/blob/main/skills/SKILL.md))
- crates.io: [rustdoc-mcp](https://crates.io/crates/rustdoc-mcp) / [ferritin](https://crates.io/crates/ferritin) / [docs-rs-mcp](https://crates.io/crates/docs-rs-mcp) / [docsrs-mcp](https://crates.io/crates/docsrs-mcp) / [rust-docs-mcp](https://crates.io/crates/rust-docs-mcp) / [rustdoc-mcp-server](https://crates.io/crates/rustdoc-mcp-server) / [mcp-rust-docs](https://crates.io/crates/mcp-rust-docs)
- [syphar/docs-rs-mcp](https://github.com/syphar/docs-rs-mcp) / docs.rs のコントリビューター: [rust-lang/docs.rs](https://github.com/rust-lang/docs.rs)
- [dmvk/docsrs-mcp](https://github.com/dmvk/docsrs-mcp) ([Issue #5](https://github.com/dmvk/docsrs-mcp/issues/5))
- [snowmead/rust-docs-mcp](https://github.com/snowmead/rust-docs-mcp)
- possdog/mcp-rustdoc: [Tangled](https://tangled.org/poss.dog/mcp-rustdoc) / [GitHub ミラー](https://github.com/possdog/mcp-rustdoc) / [Codeberg (archived)](https://codeberg.org/possdog/mcp-rustdoc)
- [Govcraft/rust-docs-mcp-server](https://github.com/Govcraft/rust-docs-mcp-server) ([Issue #18](https://github.com/Govcraft/rust-docs-mcp-server/issues/18))
- [46ki75/mcp-rust-docs](https://github.com/46ki75/mcp-rust-docs)
- daipendency: [daipendency-mcp](https://github.com/daipendency/daipendency-mcp) / [daipendency](https://github.com/daipendency/daipendency) / [daipendency-extractor-rust (Issue #1)](https://github.com/daipendency/daipendency-extractor-rust/issues/1)
- [spacemeowx2/cargo-doc-mcp](https://github.com/spacemeowx2/cargo-doc-mcp)
- [cortesi/ruskel](https://github.com/cortesi/ruskel)
- [公式 MCP レジストリ](https://registry.modelcontextprotocol.io)
- [Show HN (ferritin)](https://news.ycombinator.com/item?id=48941375)
- [Show HN (snowmead)](https://news.ycombinator.com/item?id=44329314)
- 三次調査で参照したもの:
  - docs.rs のソース (`rust-lang/docs.rs@cca98f22`) と [rustdoc JSON の説明ページ](https://docs.rs/about/rustdoc-json)
  - rustdoc-types の [CHANGELOG](https://github.com/rust-lang/rustdoc-types/blob/trunk/CHANGELOG.md) (形式 47 の欠番) と、rust-lang/rust の `src/rustdoc-json-types/lib.rs` の履歴
  - [migrate_rustdoc_json](https://github.com/BD103/migrate_rustdoc_json)、[cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks) (`src/data_generation/generate.rs`)、[officina](https://github.com/LukeMathWalker/officina)
  - cargo の [#17025](https://github.com/rust-lang/cargo/pull/17025)・[#16773](https://github.com/rust-lang/cargo/pull/16773)・[#13283](https://github.com/rust-lang/cargo/issues/13283)・[#16720](https://github.com/rust-lang/cargo/issues/16720)
  - MCP 仕様の [versioning](https://modelcontextprotocol.io/specification/versioning) と [2026-07-28 の changelog](https://modelcontextprotocol.io/specification/2026-07-28/changelog)
  - Claude Code の [MCP の文書](https://code.claude.com/docs/en/mcp) と [環境変数の文書](https://code.claude.com/docs/en/env-vars) (2026-10-14 取得)
- 二次調査と三次調査の根拠 (行番号付き) は [requirements.md の付録 A](requirements.md#付録-a-既存実装から得た教訓と対応する要件) にまとめた (三次調査で行番号を訂正し、各リポジトリの HEAD の SHA を併記した)

---

## 11. 改訂履歴

### 2026-10-15 (要件 v0.13 のレビューによる整理)

- 冒頭に、この調査を受けて自作を決めたことと要件へのリンクを足した。§8 (このホストでの導入メモ) と冒頭の個人の環境の記述を、リポジトリの外へ移した
- §7: 「新しい実装は docs.rs JSON に収束」をビルドしない実装に限り、「どれも鮮度判定を誤る」を該当する実装に限った。`RUSTC_BOOTSTRAP` の値が crate 名であることと、stable / beta の manifest の `available = false` を補った。§5.1 の syphar の根拠の無い「3 OS 対応」を削った

### 2026-10-14 (2 回目のレビューによる整理)

- §7: HTML 型の所見を、2026 年の HTML 型 (kieled、KingingWang、BrightX) と食い違わないように直した。「1 つの形式しか読めない」を「読める形式を固定した」に直した (46ki75 は 2 つ読む)
- §1 の「代わりに」の係り先、rustdoc-mcp の保守の頻度、syphar の形式の条件、JLFN / gnufood の Cons、§5.1 の possdog の target の意味を明確にした。§1 の重複と §11 の重複した注記を削った

### 2026-10-14 (レビューによる訂正と整理)

- §1 の推奨構成 2 を三次調査の結果に合わせた (rustdoc-mcp は implementors を出さないので、ホスト版を外さず使い分ける)
- 誤りの訂正: ferritin は `std::primitive::slice` を引けない。syphar のソース表示は「△」(examples は中身を返せる)。std を扱えるローカル型に Xevion を追加。migrate_rustdoc_json の範囲。stable 1.99 の形式 61 は確認済み (推定ではない)。§5 の possdog のリンク。shark-hunt の blob の内訳
- 重複 (rustdoc-mcp の implementors と Store の作り直し、`target/doc`、サンドボックス、eval の不在など) を、要約・詳細・表の 3 か所以内にまとめた。日付の無い数値の基準日を §9 に明記した
- §7 の安全性の所見に、workspace を rustdoc にかけると依存の build.rs と proc-macro も実行されることを足した

### 2026-10-14 (三次調査による訂正と追記)

全記載をソースと API で再検証し、最新の変更を確かめ、見落とした候補と新しい比較軸を足した。

- **結論に関わる訂正**
  - rustdoc-mcp (jbr): 「trait implementors も出せる (ferritin と同じエンジン)」は誤り。描画処理は独自実装で、implementors を出さない (§1、§4、§5)
  - rustdoc-mcp: 「ビルド失敗を 1 時間キャッシュする」は誤り。ツール呼び出しのたびに Store を作り直すので、失敗したビルドを毎回やり直す
  - rustdoc-mcp: annotations は crates.io の 0.10.0 には無い (main のみ)
- **個別の訂正**
  - §7: 形式 47 は欠番。形式 46 を読める実装は maan2003 にある。docs.rs の JSON の有無はビルド日で決まる。定義パスを見せる実装に syphar と possdog を追加
  - syphar: `ServiceExt` は 3 つ。`resolve_version` の規約は緩和済み。ライセンスの本文が無い。未マージのブランチを追記
  - possdog: archived は 2026-07-27。本家のリンクを Tangled に変更。ソース表示は ×
  - snowmead: Issue #59・#64 は main で close (crates.io 版は未修正)。implementors は ×。シグネチャの描画の欠陥を追加
  - ruskel: 検索はある。crates.io の 0.0.11 は形式 57 しか読めない
  - mmgeorge: 「保守が活発」を訂正
  - JLFN: 形式 60 しか読めないことを追加
  - 46ki75: live の CI の放置を追加
  - KevinT3Hu: 「解決された feature を再現する」は依存では成り立たない (手元の実験で確認)
  - Xevion: ライセンスは Cargo.toml で宣言している
  - rust-docsbox: ホスト版のドメインが消えている
  - Govcraft: 「ツール名が衝突する」を「同名・同じ説明文で区別できない」に訂正
- **追記**
  - §1 に、rustdoc-mcp がホスト版より劣る点の表を追加
  - §2 に評価基準 6 (安全性) と 7 (MCP への準拠と品質保証) を追加し、§5.1 にその比較表を新設
  - §5 に「キャッシュの寿命」の行を追加し、保守状況を更新
  - §7 に、形式の固定による破損、stable rustdoc + `RUSTC_BOOTSTRAP`、std の stability、依存の feature、安全性、MCP への準拠、eval の不在、MCP 仕様と Claude Code の変化の所見を追加
  - §3 に類似名、§4 Tier C に maan2003 と三次調査で見つかった候補、§6 に lockdocs・officina・cargo#16720 を追加
  - shark-hunt のコピーの実態 (blob の一致など) を API だけで確認して追記
  - §8 に rustdoc-mcp を試すときの注意、§9 に三次調査の方法と限界を追加

### 2026-10-13 (表現の整理)

- §3: 名前の数を 11 から 10 に訂正 (`rust-docs-mcp-server` が重複していた)。衝突の無い名前に mcp-rustdoc-parser を追加
- §9: 並べて試す候補を、ランキングと食い違っていた「上位 3 件」から rustdoc-mcp とホスト版 ferritin に訂正
- §5: syphar のソース表示を「△」から「×」に訂正 (item のソースを表示するツールは無い)
- §1 と §4 の重複、ferritin の Pros と Cons の重複を整理

### 2026-10-13 (二次調査による訂正)

主要な実装のソースを精読した結果、次の記述を訂正・追記した。

- **syphar**: 「nightly 2025-08-02 以降に対応」を「形式 57 しか読めない」に訂正。「crate をまたぐ trait の re-export も追う」を「参照先の crate と版を示すだけ」に訂正。構造化 JSON の冗長さを実測値に置き換え、メソッドを引けない・短い trait 名・順位付けの無い検索などの欠点を追記
- **dmvk**: 「trait の実装者を引ける」を「説明文のみで未実装」に訂正。「複数版は最新のもの」を「文字列比較による不具合」に訂正。属性を捨てること、Cargo.lock を起動時にしか読まないことを追記
- **rustdoc-mcp (jbr)**: `--document-private-items` は workspace member だけ。ローカルビルドの JSON はユーザーの `target/doc` に書く。鮮度判定・ビルド失敗・複数版の欠陥を追記
- **ferritin (ホスト版)**: 実測した欠陥 (Deref 経由のメソッドやプリミティブを引けない、定義パスの表示、unstable や private の混入、リンクの欠落、続きを取れない打ち切り、`isError` を使わないエラー) を追記
- **possdog**: archived であることを追記。std を「確認できず」から「非対応」に訂正。各機能の実装が簡易的であることを追記
- **snowmead・ruskel・Xevion・MasYaaz・KevinT3Hu・cratedex**: ソースで確かめた欠陥と利点を追記。Xevion の std 対応を「未検証」から確認済みに訂正
- **§5 の比較表**: 読める形式の行を追加。dmvk の implementors、possdog の std、Cargo.lock 連動の評価を訂正
- **§7**: docs.rs の形式の配り方、定義パス、鮮度判定についての所見を追加。JSON が無い期間を「2025 年頃より前」から「2025-05-23 より前」に訂正
