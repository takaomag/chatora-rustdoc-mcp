# chatora-rustdoc-mcp 要件定義 (ドラフト v0.20)

- 作成日: 2026-10-13 (v0.1) / 最終改訂: 2026-10-24 (v0.20)
- 状態: たたき台。既存実装の精読と 2026-10-14 の再検証 (調査文書の三次調査)、M0 (PoC) の結果 ([m0-plan.md](m0-plan.md) の §6) と、その後のレビューの決定 (v0.18)、M1 (MVP) の結果 ([m1-plan.md](m1-plan.md) の §6) とその決定 (v0.19)、M2 (workspace) の結果 ([m2-plan.md](m2-plan.md) の §6) とその決定 (v0.20) を反映済み。採らなかった候補の良い点は付録 B、根拠は付録 A、経緯は §14。残る未決事項 (§11.2) を PoC・eval・実測で詰めてから v1.0 にする
- 前提資料: [rustdoc-mcp-survey.md](rustdoc-mcp-survey.md) (既存の Rust ドキュメント系 MCP の調査)
- 主な参考実装:
  - [rustdoc-mcp / ferritin (jbr)](https://github.com/jbr/ferritin)
  - [docs-rs-mcp (syphar)](https://github.com/syphar/docs-rs-mcp)
  - [docsrs-mcp (dmvk)](https://github.com/dmvk/docsrs-mcp)
  - [rust-docs-mcp (snowmead)](https://github.com/snowmead/rust-docs-mcp)
  - [cargo-public-api](https://github.com/cargo-public-api/cargo-public-api)
  - [ruskel](https://github.com/cortesi/ruskel)
  - [cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks) (stable + `RUSTC_BOOTSTRAP` による JSON 生成)
  - [officina の rustdoc_processor / rustdoc_resolver (Pavex から分離)](https://github.com/LukeMathWalker/officina) (公開 import path の索引)

---

## 0. 叩き台のレビュー: 必要十分か

**結論: 方向性は妥当で、必要条件の大半は押さえている。ただし十分ではない。** 足りないのは「うまくいかないときの振る舞い」と「プロジェクトの文脈との突き合わせ」の 2 系統。さらに既存実装の精読で、「LLM に見せる形 (正準パス・impl の分類・出力予算)」が最大の弱点だと分かったので、そこを大きく補強した。

### 0.1 各項目の判定

| 叩き台の項目 | 判定 | コメント |
| --- | --- | --- |
| ツール数は多いほうが良い | **修正** | 数そのものに価値は無い。**「LLM の問い 1 種類にツール 1 個」という粒度**と、ツール同士が重ならないことが本質。Claude Code は MCP ツールを遅延ロード (tool search) するので、ツールが多くてもコンテキスト代は小さい。代わりに、名前と説明文で探されたときに迷わないことが必要 (§6)。tool search を持たないクライアント向けに、主要なツールだけを出す構成は将来検討する (R-MCP-12。v0.18) |
| 説明文を「LLM がどう問うか」基準で書き、否定ルールを入れる | 妥当 | 採用する。説明文は 2,048 文字以内にし、冒頭に要点を置く (Claude Code の現行の上限は 4,096 文字。§6.2) |
| ソース表示 / trait implementors | 妥当・要補足 | implementors の**範囲**を決める必要がある。既存実装はどれも定義元 crate 内だけで、依存グラフ全体の逆引きは先行例が無い (§5.6) |
| feature flags / changelog / examples / README を個別ツールで | 妥当・要補足 | feature は**プロジェクトで有効か**まで突き合わせる。rustdoc JSON には cfg の専用フィールドが無いので、属性文字列の解析と祖先からの継承が要る (§5.4) |
| re-export 解決 (crate をまたぐものを含む) | 妥当・要補足 | rustdoc JSON の `paths` は**定義パス** (private module を含む) なので、そのまま見せると LLM が使えない `use` を書く。公開パスの索引を自前で作る必要がある (§5.5) |
| target triple の考慮とフォールバック | 妥当・要補足 | docs.rs では別の target の文書で代用しない (R-TG-1。v0.18)。フォールバックするとき (std など) は、実際の triple まで provenance に載せる (R-TG-2) |
| バージョンを勝手に差し替えない | 妥当 | すべての成功応答に provenance を載せ (R-OUT-9)、データ本体から読んだ値と要求値を突き合わせる (§7.3) |
| 出力は構造化 JSON | 妥当・要補足 | 構造化 JSON で良い。ただし Claude Code ではその JSON がそのままプロンプトになるので、**シグネチャは Rust のコード文字列**にし、何を出さないかを決める (§7) |
| impl / trait の検索 | 妥当・要補足 | 「X で何が呼べるか」には trait メソッド・Deref 先・プリミティブ型まで含める。既存実装はどれも `Vec::iter` を引けない (§5.6) |
| workspace の private items | 妥当 | 採用する。ただし依存 crate と std の非公開 item は絶対に見せない (§7.2) |
| std は rust-docs-json? | 妥当 | nightly にしか無い (toolchain を入れずに dist から取る経路も持つ。R-STD-6)。ユーザーの toolchain・MSRV との差を安定化情報で警告する (§5.11) |
| workspace はローカル build + `--document-private-items`? | 妥当・要補足 | 先行実装はどれも、鮮度判定・target dir・失敗の扱いのどこかで誤っている (§5.10) |
| 一般 crate は docs.rs の JSON | 妥当 | docs.rs を第一にする (確定。根拠は R-DS-1) |
| rustdoc-types の幅広いサポート (最良の仕組みを採用) | 妥当 | ferritin 方式を採用。ただし ferritin の下限 48 では足りず、docs.rs には形式 46 が実在する (§5.2) |
| バージョン解決: 明示 > Cargo.lock > latest | 妥当・要補足 | 文字列比較で semver を比べる・同名の複数版を名前で潰す、といった不具合が既存実装に多い (§5.3) |
| ディスクキャッシュ | 妥当・要補足 | 原子的な書き込み、プロセス間ロック、mmap 前の検証、派生物の整合性まで要件化 (§9.3) |

### 0.2 叩き台に無かった観点

1. **失敗系の設計**: docs.rs に JSON が無い・ビルド失敗・形式非対応・オフラインでキャッシュに無い・ビルド中、のそれぞれで LLM が次の一手を打てる結果を返す (§8)
2. **プロジェクト文脈との突き合わせ**: 有効な feature、MSRV と toolchain、Cargo.lock の複数版、docs.rs がビルドに使った依存版との差
3. **LLM に見せる形**: 正準パス、impl の構造的な分類、既定で畳むもの・出さないもの (汎用 trait の blanket impl、不安定な auto trait、非公開 item)、出力予算 (§7)
4. **ローカルビルドの運用**: 内容ダイジェストによる鮮度、永続的な専用 target dir、進捗通知、ビルド中の応答 (§5.10)
5. **非機能要件**: 性能・メモリ・キャッシュ整合性・ネットワーク作法・セキュリティ (§9)
6. **Claude Code という具体的なクライアントの制約**: 説明文の切り詰め、出力サイズの閾値、`structuredContent` の扱い、プロジェクトディレクトリの渡し方 (§6、§7)
7. **評価方法**: eval、説明文の長さテスト、出力サイズの予算テスト、nightly canary (§9.7)
8. **差別化機能**: 依存グラフ全体の implementors、Deref・プリミティブを含むメソッド解決、feature の有効判定、MSRV を踏まえた std 表示、API 差分、学習データより新しい Rust の変更点 (§5.4、§5.6、§5.11、§5.12、§5.14)

---

## 1. 目的

MCP 2026-07-28 を話すクライアント (主に Claude Code) が Rust のコードを書くときに、**記憶ではなく実際の定義**に基づいて API を使えるようにする。そのために、次のドキュメントを、正確なバージョンで、トークン効率よく、構造化された形で引ける MCP サーバーを作る。他のクライアントは、MCP の最新版に追いついたものから使える (R-MCP-2。v0.18)。

- そのプロジェクトが実際に使っている依存 crate
- 自分の workspace の crate
- 標準ライブラリ

## 2. ゴールと非ゴール

### ゴール

- G1: そのプロジェクトで解決されるバージョンのドキュメントを、エージェントがバージョンを指定しなくても返す
- G2: workspace crate (private items を含む)、依存 crate、std を 1 つのサーバーで扱う
- G3: 「X で何が呼べるか」「X は Send か」「この trait を実装しているのは何か」「この機能はどの feature で有効になり、いま有効か」に 1〜2 回の呼び出しで答える
- G4: 正しい応答を返すか、正しくない可能性を明示する。バージョンは差し替えない (R-VR-4)。target のフォールバック、feature の差、推定した版 (R-PR-4) は必ず応答に明示する
- G5: **LLM がそのまま使える形**で返す。表示するパスはそのまま `use` できること、応答は既定で約 8,000 トークン以下 (Claude Code の警告閾値 1 万トークンより下。R-OUT-3) に収まること
- G6: キャッシュが効いている状態の応答が速い (§9.1)

### 非ゴール (v1)

- 埋め込みベクトルによる semantic search (将来、別機能として検討する)。v1 で採らない理由: 埋め込みモデルの外部 API への依存か、ローカルのモデルによる大きなメモリ消費 (先行例で約 1GB) が要り、結果も決定的でなくなる (R-OUT-4)。名前の分からない item を探す問い (§3 の Q4) は、BM25・`doc(alias)`・祖先のパス名の重み (R-SR-1、R-SR-3) で受け、eval (R-QA-6) で取りこぼしを測って再判断する
- LLM による要約 (サーバーは生データを返すだけ)
- コードの編集、`cargo build` / `check` / `clippy` / `fmt` の実行、Rust playground
  - Claude Code は Bash でプロジェクト本体に対してこれらを実行できるので、MCP で重複させない
  - `rustc --explain` と clippy の lint 説明も同じ理由でツールにしない。同梱する skill で使い方を案内する (§9.8)
- 公開ホスティング (まずローカル stdio。HTTP は M6 以降の任意の機能。R-MCP-1)。ただし、後から足せる層構造にしておく (§9.10)
- Rust 以外の言語
- 古い版の handshake (2025-11-25 以前の `initialize`) しか話さないクライアントと、stdio のサーバーを直接使えないクライアントへの個別の対応 (R-MCP-2)
- The Book / Reference などの散文ドキュメント (将来検討)。crate に同梱された README や `docs/` の散文は別扱いで、R-SR-1 で任意に検索の対象にする

---

## 3. 想定する問い

ツール設計 (§6) の入力になる一覧。eval (§9.7) の問題集の原型にもなる。番号 (Qn) は §11 の未決事項 (Q-n) とは別。

| # | 問い | 主に使うツール |
| --- | --- | --- |
| Q1 | このプロジェクトの依存と、各バージョンは? | `list_crates` |
| Q2 | axum の最新版は? このプロジェクトで使っている tokio は何版? | `resolve_version` |
| Q3 | この crate の全体像を掴みたい | `crate_overview` |
| Q4 | `Router` はどこにある? 名前は分からないがタイムアウトを扱う型はある? | `search_items` |
| Q5 | `tokio::sync::Mutex::lock` のシグネチャ、ドキュメント、panic 条件は? | `get_item` |
| Q6 | `axum::routing` モジュールには何がある? 何を re-export している? | `list_module` |
| Q7 | `Vec<T>` で呼べるメソッドは? (slice 由来のものも含めて) `str::len` は? | `list_methods` / `get_item` |
| Q8 | `Arc<Mutex<T>>` は Send か? `MyType` はどの trait を実装している? | `list_impls` |
| Q9 | `tower::Service` を実装している型は? (自分の依存の中で) | `list_implementors` |
| Q10 | この関数の実装を見たい | `get_source` |
| Q11 | websocket を使うにはどの feature を有効にする? いま有効か? | `inspect_features` / `get_item` |
| Q12 | この API の使用例は? | `find_examples` |
| Q13 | この crate の始め方は? | `readme` |
| Q14 | 0.7 → 0.8 で何が変わった? 破壊的変更は? | `changelog` / `api_diff` |
| Q15 | この crate の MSRV、ライセンス、リポジトリは? 保守されている? | `crate_metadata` |
| Q16 | MQTT クライアントの crate は何がある? どれが良い? | `search_crates` |
| Q17 | この std API は自分の toolchain や MSRV で使える? | `get_item` (stability 欄) |
| Q18 | 使っている依存に既知の脆弱性はある? | `audit_dependencies` |
| Q19 | 学習データより新しい Rust で何が安定化された? | `rust_release` |
| Q20 | (umbrella crate) `compio::io::AsyncWriteAt` はどこで定義されている? | `get_item` |
| Q21 | このエラーメッセージは依存のどこで出ている? この feature gate はソースのどこにある? | `search_source` |
| Q22 | この git リポジトリ (や別の workspace) の API は? | `build_external_docs` → `get_item` |

---

## 4. 用語

| 用語 | 意味 |
| --- | --- |
| provenance | 応答の元データの出どころ。source (docs.rs / local / std)、crate、version、バージョンの決め方、target、format_version、features、toolchain の組 |
| format_version | rustdoc JSON の形式番号 (`rustdoc_types::FORMAT_VERSION`。2026-10-13 時点の最新は 61) |
| canonical 形式 | サーバーが内部で扱う唯一の形式。ビルド時に依存する最新の `rustdoc-types` が定義する形式 (R-FV-1)。正準パス (canonical path) とは別の概念 |
| shim | 旧形式 JSON のうち、読み取り互換性を壊す変更があった部分だけを受けるための型定義 |
| 定義パス (definition path) | rustdoc JSON の `paths` に入っているパス。定義された場所を指し、**private module を含みうる** (例: `axum::json::Json`、`tokio::net::tcp::stream::TcpStream`) |
| 正準パス (canonical path) | ユーザーが `use` できる公開パスのうち、次の順で選んだ 1 つ: hidden・unstable なモジュールを通るものを除く → 改名 (`as`) の無いものを優先 → 最短 (セグメントの数。v0.18) → 辞書順。公開パスの無い workspace member の item は、crate 内でのパスと可視性で示す (R-PR-2。v0.18) |
| ホスト版 | 公開サーバーとして動く版。「ferritin のホスト版」は ferritin が公開している MCP サーバー。本サーバーのホスト版は §9.10 の将来構想 |
| 任意 (要件の印) | 実装しなくても、その段階 (M) の完了条件を満たす。既定で無効という意味ではない |
| プロジェクト | Cargo の workspace (workspace に属さない単独の package はその package)。専用 target dir の単位 (R-MCP-3) |
| 既定のプロジェクト | `project` 引数を省いたときに R-MCP-3 の順で決まるプロジェクト。Claude Code を開いたディレクトリの workspace で、そこに Cargo.toml が無ければ直下の子の workspace (複数あればそのすべて)。直下の子にも無ければ、git の作業ツリーのルートまで上へ探した最初の Cargo.toml の workspace (git の外では上へ探さない。v0.20)。同じ git リポジトリの別の作業ツリー (linked worktree など) の同じ場所にある workspace も、既定のプロジェクトと同じに扱う (R-MCP-3。v0.20) |
| 照会のツール / 外のツール | 外のツールは `build_external_docs` (R-WS-10)、照会のツールはそれ以外のツール。照会のツールが cargo を起動しローカルビルドするのは、既定のプロジェクト (同じ git リポジトリの別の作業ツリーの同じ場所の workspace を含む。v0.20) とその依存のうち、rust-analyzer の既定 (`cargo check --all-targets`) と同じ範囲だけ (範囲の定義は R-MCP-3。resolver 2 以上では、そのうち `-p` で名指しできない依存を除く。R-WS-5) |
| ローカルビルド | rustdoc で JSON を手元で生成すること。使う toolchain は、プロジェクトの toolchain に `RUSTC_BOOTSTRAP` を crate 名を指定して付けたものか、nightly (R-WS-8)。外のツールの仮のプロジェクト (R-WS-5) では、常に `RUSTC_BOOTSTRAP=1` を付ける (v0.20) |

---

## 5. 機能要件

### 5.1 データソース

| 対象 | 第一の供給元 | 代替 | 備考 |
| --- | --- | --- | --- |
| std / core / alloc / proc_macro / test | nightly の `rust-docs-json` コンポーネント | static.rust-lang.org の dist から `rust-docs-json` のアーカイブだけを直接取る (R-STD-6) | stable には無い。docs.rs も std の JSON は配っていない (404)。ソース表示には `rust-src` を使う |
| workspace member | ローカルビルド (`--document-private-items`) | 再ビルド中は直近に成功した JSON (stale と明示。失敗したときは `build_failed`) | §5.10 |
| crates.io の依存 crate | docs.rs の rustdoc JSON | ローカルビルド | docs.rs に JSON が無いとき (2025-05-23 より前のビルドは R-DS-3、docs.rs でのビルド失敗は R-DS-6、非対応の形式は R-FV-4、求める target の JSON が無いときは R-TG-1。v0.18) と、Cargo.lock の `checksum` が sparse index の `cksum` と一致しないとき (R-VR-2。v0.18) の代替。M2 で決めた範囲 (m2-plan.md の D7b。v0.20): docs.rs の読み込みが `no_target_json` か `no_json` (docs.rs の 404 と 400。2025-05-23 より前・docs.rs のビルドの失敗・対応しない形式) で失敗したときと、checksum の不一致・checksum 無し・index に無い版や crate (私的なミラー) のときに戻す。`unparsable`・`too_large`・`version_mismatch` では戻さない。戻すのは、プロジェクトが版を決めたもの (`cargo_lock`。`cargo_lock_raw` は除く)、そこからの re-export の行き先、完全一致で明示した版がプロジェクトの crates.io の版と同じもの。戻したビルドが失敗したら、その失敗を docs.rs を使わない理由と一緒に返す。Cargo.lock に無い版と、実際のビルド target 以外の target での文書 (R-MCP-3) と、`-p` で名指しできない依存 (R-WS-5) は、docs.rs に無ければ外のツールで作る (R-WS-10) |
| path / git / 非公開レジストリの依存 | ローカルビルド | なし | docs.rs には存在しない |
| 引数 `git` で指した git リポジトリの crate | ローカルビルド (仮のプロジェクトの git 依存として。R-WS-5) | なし | 外のツールでだけビルドする (R-WS-10) |
| 依存ではない任意の crate | docs.rs の rustdoc JSON | ローカルビルド (外のツールで、仮のプロジェクトに。R-WS-5) | docs.rs に JSON が無いとき、照会のツールは `docs_unavailable` を返して外のツールを案内する (R-WS-10)。プロジェクトが使わないと分かった crate は、`target` を省けばホストの target で docs.rs と外の記録を引く (v0.20)。外のツールは yanked の版を完全一致で指されても作らず、理由と yanked でない近い版を返す (v0.20) |

- R-DS-1: 公開 crate は docs.rs から取るのを既定とする (確定)。プロジェクトの依存で docs.rs を使う条件は R-VR-2 (取得元と checksum の照合。v0.18)。依存 crate を常にローカルビルドする方式は、先行実装で遅さと MSRV 失敗のため撤回されている。ローカルビルドを優先するモード (設定 `build.prefer_local`) は M3 以降で検討する (v0.20)
- R-DS-2 取得手順:
  1. まず形式番号の無い URL `/crate/{name}/{version}/[{target}/]json` を取る。返るのはそのリリースの直近のビルドの形式 (再ビルドされていれば最も新しい形式)
  2. `format_version` を R-FV-3 のとおり先読みする
  3. 返った形式を読めないときだけ、対応範囲の形式番号付き URL (`/json/{n}`) を新しい順に試す。docs.rs はそのリリースで実際にビルドされた形式しか配らないので、効くのは複数回ビルドされたリリースだけ (404 は想定内)。欠番の 47 は問い合わせない (R-FV-4)。試行の途中で一時的な失敗や壊れた本体 (展開や解析の失敗) に当たっても打ち切らず、最後まで試す。読める形式が見つかれば、それをキャッシュして引き直す。見つからなければ、途中に一時的な失敗があればそれを、無ければ元の `unsupported_format` を返す (M1 で決めた。m1-plan.md の B3。v0.19)
- R-DS-3: docs.rs の JSON は、docs.rs が **2025-05-23 以降にビルドした**リリースにしか存在しない。基準は公開日ではなくビルド日で、古いリリースも docs.rs で再ビルドされれば JSON を持つ (serde 1.0.219 は 2025-03 公開だが JSON がある)。有無は R-DS-5 で確かめる。JSON が無ければ、プロジェクトの依存はローカルビルドで補い (§5.1 の表)、依存でない crate には `docs_unavailable` を返して外のツールを案内する (R-WS-10。v0.20)。候補の示し方は R-VR-4
- R-DS-4 展開: **先頭が zstd のマジックナンバー (`28 B5 2F FD`) である限り展開を繰り返す**。初期のビルドには二重に zstd 圧縮され `content-type: text/plain` で配られるものがある。content-type を信用しない。展開の段数に上限を設け、それを超えるものは壊れているとみなす (初期値は 3 段。v0.16。`docs_unavailable` の `unparsable` で返す。v0.19)
- R-DS-5: docs.rs の `status.json` の `doc_status` は HTML のビルド成功を表すだけで、JSON の有無ではない。JSON の有無はその URL を実際に確かめて判断し、`resolve_version` の `docs_rs_json` として別に返す (v0.19 で M1 の実装に合わせ、名前と値を改めた): `available` (キャッシュにあるか、docs.rs が答えた)・`no_target_json` (その版の JSON はあるが、求める target のものが無い)・`no_json`・`not_on_docs_rs` (プロジェクトの取得元が crates.io でないか、checksum が合わない。理由は warnings)・`unknown` (確かめられなかった)。キャッシュに無ければ docs.rs に `HEAD` で聞く (求める target の JSON → 無ければ既定の target の JSON)。形式に対応しているかは、取って読むまで分からないので返さない。M2 で足した決まり方 (v0.20): プロジェクトの crates.io の package が写し (R-VR-2) なら、`data.source` は crates.io のまま `not_on_docs_rs` と理由の warning を返す (index に無ければ `latest` などは省く)。引数 `git` の crate は `not_on_docs_rs`。独自の target のプロジェクト (R-TG-1) で `target` を省いたときは、target の無い URL で確かめる
- R-DS-6 docs.rs の JSON の URL の扱い (docs.rs のソース `rust-lang/docs.rs@cca98f22` と実測で確認):
  - 形は `/crate/{name}/{ver}/[{target}/]json[/{fmt}][.gz|.zst]`。`{ver}` には `latest` と semver 要件も使える。正規でない版指定は 302 で完全な版にリダイレクトされる。ただし build metadata 付きの版を持つ crate では、302 にならず 200 で別の版が返ることがある (2026-10-19 に、zstd-sys の `2.1.0` で `2.1.2+zstd.1.5.7` が返ることを確かめた。版の頼み方は R-VR-1、返った版の照合は R-VR-4。v0.18)。`latest` はリダイレクトされず 200 が返り、本文の `crate_version` を見ないと実際の版が分からない。このため `latest` の URL は使わず、R-VR-3 で解決した完全な版の URL で取る (R-VR-3 と docs.rs の latest の食い違いを避ける)
  - 既定の圧縮は zstd。`.gz` を付ければ gzip で取れる
  - 不明な target や形式は 404、形式番号として解析できない値は 400。版として解析できない値も 400。HTML のビルドに失敗したリリースは JSON も 404
  - 404 は「crate や版が存在しない」と「JSON が無い」を区別しない (M0 で確かめた)。`crate_not_found`・`version_not_found` (§8) は crates.io の sparse index (R-VR-6) で存在を確かめて分ける (M1。v0.17)
  - `ETag` があり `If-None-Match` を処理する (完全な版の JSON は不変なので、通常は取り直さない)
  - レート制限のヘッダや公式の文書は見当たらない。R-NW-2 の作法 (重複抑止・再試行の制限) で控えめに使う

### 5.2 rustdoc JSON の形式バージョン対応

**方式: ferritin 方式を採用する。** canonical 形式に、読み取り互換性を壊した箇所だけを受ける typed shim を足し、1 回のストリーミングパースで正規化する。

| 実装 | 方式 | 対応範囲 | 評価 |
| --- | --- | --- | --- |
| **ferritin / rustdoc-mcp (jbr)** | 最新の `rustdoc-types` 1 つを canonical とし、読み取り互換性を壊した変更 (形式 54: `Item::attrs` の型変更、57: 必須フィールド `ExternalCrate::path` の追加、61: `Stability::level` の表現変更) だけを typed shim で受ける。それ以外の追加的な変更は serde がそのまま読める。`serde_json::Value` を経由しない 1 回のストリーミングパース | 48〜61 と、それより新しい追加的な形式 | **最良**。ただし下限 48 は「調査済みの下限」で、docs.rs に実在する 46 を「存在しない」扱いにしてしまう |
| docsrs-mcp (dmvk) | `serde_json::Value` の段階で 53〜57 を書き換えて 0.56 に合わせる。**全形式で `attrs` を空配列に置き換える**ほか、形式 56 未満には偽の `target` を注入する | 53〜57 | 属性 (must_use・non_exhaustive・doc(hidden)・cfg) を全部失う。反面教師 |
| trustfall_rustdoc (cargo-semver-checks) | 形式ごとに別の adapter crate を持ち、feature (`v57` / `v60` / `v61`) で切り替える | 3 世代だけ | 丁寧だが範囲が狭く、コード量が世代の数に比例する |
| mcp-rust-docs (46ki75) | `format_version` を読んでから、改名した 2 つの rustdoc-types (56・57) に振り分ける | 56 と 57 | 固定リストのため、現行 (61) で壊れている |
| docs-rs-mcp (syphar) | `rustdoc-types` 1 つだけ | 57 のみ | 狭い |
| rust-docs-mcp (snowmead) | nightly を特定の日付に固定する | 固定した 1 形式 | 新しい MSRV の crate でビルドに失敗する |
| docsrs-mcp (maan2003) | 形式ごとに別の版の `rustdoc-types` (0.46.1、0.48〜0.53) を持ち、形式番号で読み分ける | 46、48〜53 | 形式 46 を読める唯一の MCP 実装。1 日だけの実験で、保守されていない |
| migrate_rustdoc_json (BD103) | 形式を 1 つずつ新しい形へ移行する変換コードを積み重ねる (MCP ではなくライブラリ) | 41〜50 (開発版は 53 まで) | 形式ごとの破壊的変更の一覧として、shim を書くときの参照になる |

- R-FV-1: canonical 形式には、依存として使える最新の `rustdoc-types` を使う (2026-10-13 時点で 0.61.0、形式 61)
- R-FV-2: 読み取り互換性を壊した変更ごとに typed shim を用意する。shim は `Crate` から変更されたフィールドまでの経路にある構造体だけを写し、残りは canonical の型を使う。形式 54 未満の `attrs: Vec<String>` は捨てずに解析し、形式 54 以降に該当する variant があればそれに写し、無いものと解析できないものは `Attribute::Other(String)` にする。文字列のまま残すと、derive の分類 (R-TI-2)、must_use / non_exhaustive の表示 (R-SH-6)、`api_diff` の分類 (R-AD-2) が黙って失敗し、形式をまたぐ比較で `must_use_changed` などを誤検出するため
  - 写す範囲 (M1 で、形式 46・48・53 の testdata に実際に出る属性を数えて決めた。m1-plan.md の B7。v0.19): 形式 54 以降に variant がある属性はすべて写す (`non_exhaustive`、`must_use` (理由付き。52〜53 の `#[attr = MustUse {reason: "…"}]` も)、`macro_export`、`export_name`、`link_section`、`automatically_derived`、`repr` (`C`・`transparent`・`simd`・`Rust`・整数型・`align(n)`・`packed`・`packed(n)`)、`no_mangle` (`unsafe(…)` で包んだものも)、`target_feature`)。`#[inline]` (46) と `#[attr = Inline(Hint)]` (48 以降) は形式 61 でも `Other` なので、表記を揃えずに残す (`Other` の文字列は形式の版を上げずに変わりうると上流が明言しているため)
  - 形式 54〜61 の shim (M0 で実装した。54〜57・60・61 は docs.rs の実データで確かめた。v0.17。58〜60 の `Stability` の写しは、M1 で 2026-06-25 (形式 58)・06-26 (59)・07-10 (60) の nightly の std の JSON で確かめた。v0.19): 54〜56 は `ExternalCrate::path` (57 で追加された必須フィールド) が無いので空のパスを入れ、「不明」として扱う。54 の `Attribute::Other("#[macro_export]")` は、55 で追加された `Attribute::MacroExport` に写す (rustdoc-types 0.55 の更新履歴は `MacroUse` と書いているが、型は `MacroExport`)。58〜60 は `Stability::level` の平らな表現を、61 の入れ子の表現に写す
- R-FV-3: パースの前に `format_version` だけを先読みして、使う shim を静的に決める。先読みは、本文の末尾からのバイト走査を主にし (`format_version` は、M0 で docs.rs から取った形式 46・48・53〜57・60・61 のすべてで最後のフィールドだった)、読めなければ全体の走査に落とす。全体の走査も R-FV-8 の上限に従う (v0.19)。全体を解析した後に、本文の値と先読みの値を照合し、違えばエラーにする (M0 で決定。v0.17)
- R-FV-4: **対応範囲の下限は 46** とする (Q-4)
  - docs.rs には形式 46 (tokio 1.45.1、2025-05-24 ビルド) が実在し、48・53・54・55・56・57・60・61 も確認済み (49〜52・58・59 は M0 で取った版には無かった。v0.17。58〜60 は M1 で nightly の std の JSON で確かめた。v0.19)
  - **形式 47 は欠番で、存在しない** (rustdoc-types の CHANGELOG: 「format version 47 … was skipped, and will never exist」)。対応範囲は 46〜61 から 47 を除いたもので、別の形式を探すとき (R-DS-2) も `/json/47` を問い合わせない (M1。v0.19)
  - 46 → 48 の差は `#[inline]` の属性文字列の表記だけ (migrate_rustdoc_json の `v46.rs`)。形式 46 は 48〜53 と同じ shim で読めることを、M1 で testdata (tokio 1.45.1・clap 4.5.39 の 46、syn 2.0.104 の 48、clap 4.5.41 の 53) で確かめた (v0.19)
  - 45 以前は意味の変更を含む (44 → 45 で Span の列番号が 1 始まりに、45 → 46 で repr の可視性の扱いが変わった)。docs.rs が JSON を作り始めたのは形式 46 の時期 (根拠は tokio 1.45.1 の 2025-05-24 のビルド。2025-05-23 に形式 45 のビルドが無いことは未確認) なので、45 以前は対象外とする
  - 下限より古い形式は `unsupported_format` を返し、ローカルビルドを案内する
- R-FV-5: canonical より新しい形式は、追加的な変更とみなしてそのまま読んでみる。読めなければ R-DS-2 の手順 3 を試し、それでも駄目なら `unsupported_format` (`found` と `supported` を含む) を返す。**形式の解析失敗を空の結果やログだけで済ませることは禁止する**。読めた場合も、追加的な変更だと確認できていない旨 (例: `format_version 62 is newer than supported 61; results unverified`) を warnings に出す。型を変えずに意味だけを変える変更 (44 → 45 の Span の列番号など) を黙って誤読しないため
- R-FV-6: 元の JSON は元の形式のまま保存し、読み込むときに正規化する。正規化の処理を直しても取り直さずに済む。キャッシュは形式ごとに別のファイルにし (R-CA-2)、読むときは、このサーバーが読める形式の新しいものを先に、無ければ一番新しいものを取る (サーバーの更新で読める範囲が広がれば、手元のものをそのまま読む)。「読める形式が無い」ことは 1 時間覚え、記録したサーバーの版が違えば無視する (R-ERR-3。M1 で決めた。m1-plan.md の B6。v0.19)
- R-FV-7: 新しい形式に対応するときに古い形式のデータを捨てる (フィールドを空にして読めるようにする) ことは禁止する。正規化した結果は、`rustdoc-types` の定義どおりの意味を持たなければならない
- R-FV-8: パーサーは**ネストの深さに、実際の crate より桁違いに大きい上限だけを設ける** (v0.19 で「上限を設けない」から改めた)。serde_json の既定の再帰の上限は外し (`unbounded_depth` と `serde_stacker` など)、代わりに解析の前に入れ子の深さを数えて、上限を超えたら `docs_unavailable` (`reason: unparsable`) を返す。typenum は数百段、diesel も既定の再帰上限を超える。300 段以上のテストデータと、上限ちょうどの深さのテストデータを持つ
  - 上限の初期値は 4,000 段。16 MiB のスタック (R-PR-8) で、解析・索引・アーカイブの書き出しと検証・item の取り出し・`get_item`・解放が溢れない深さから決めた (いちばん先に溢れるのは rkyv の逆直列化で、debug ビルドの `[[…]]` で 6,000 段を超えたところ。release では 2 万段を超えても通る。M1 の実測。m1-plan.md の A3・C1a)。上限が無いと、細工した JSON でプロセスごと落ちる
- R-FV-9: shim から canonical 型への変換は、`..` を使わずにフィールドを網羅的に分解する。上流でフィールドが増えたときにコンパイルエラーで気付けるようにする

### 5.3 バージョン解決

優先順位: **明示指定 > Cargo.lock > latest**

- R-VR-1 明示指定:
  - `x.y.z[-pre][+build]` は完全一致として扱う (docs.rs の URL の意味に合わせる。Cargo の `^` ではない)。この違いはツール説明文に明記する (ferritin は `@1.2.3` を caret として扱うので、移行するユーザーが混乱しうる)
  - build metadata: `2.1.0` と書かれたら、sparse index に build metadata だけが違う版が 1 つあれば、その版 (`2.1.0+zstd.1.5.7` など) に解決する。docs.rs には、sparse index で解決した build metadata 込みの版で頼む (R-DS-6。v0.18)。そういう版が 2 つ以上あれば決めない (v0.19)
  - yanked の版を明示されたら、その版を返して warnings で知らせる (v0.19)
  - 明示した版と取得元: 明示した版がプロジェクトで解決された版 (R-VR-2) と一致すれば、プロジェクトの取得元 (git・patch・path・member) のものを返し、crates.io の同じ版とは別物でありうる旨を warnings に書く。一致しなければ crates.io (docs.rs) のその版を返す (v0.18)。M2 で決めた形 (m2-plan.md の D7d。v0.20):
    - 照合するのは版を完全一致で書いたときだけ。semver 要件と `latest` は crates.io の版を決める問いなので照合しない
    - 同じ名前 (パッケージ名か lib 名) と版のパッケージが複数あれば、member → crates.io のパッケージ → ひとつに決まる crates.io でないパッケージの順に選び、ほかにもあることを warnings に書く。crates.io のパッケージが写し (R-VR-2) ならローカルビルドし、release なら docs.rs から取る
    - `cargo metadata` が期限を過ぎても解決中なら `status: building` を返す (member も。答えを揺らさないため)。ただし待つ前に Cargo.lock を直に読み、その名前と版が無ければ待たずに crates.io を引く
    - プロジェクトにある版で crates.io の同名の版を引きたいときは、プロジェクトの外のディレクトリを `project` に渡すよう案内する (そのための引数は足さない)
  - `^1` / `~1.2` / `1` などの semver 要件も受ける。解決した結果は provenance に `resolved_from: "explicit_req"` として載せる
  - `latest` / `newest` (大文字小文字・前後空白を無視) は `*` の別名とする
  - **解析できない指定は `invalid_argument` エラーにする**。黙って `*` にしない
  - `0.8.9` が完全一致になることを回帰テストで固定する (caret 扱いで実害が出た先例がある)
- R-VR-2 Cargo.lock:
  - プロジェクトの `cargo metadata` の resolve を使う。版を決めるために Cargo.lock を直接パースしない (path / git / patch / replace / workspace を cargo に解決させるため)。ただし checksum は `cargo metadata` に出ないので、下の checksum の照合のために Cargo.lock も常に読む (名前・版・source が一致するものの checksum を使う。v0.19)
  - 例外として、`cargo metadata` が失敗したとき (オフラインモードで手元に `.crate` が無いとき、git の依存の取得に失敗したときなど。`cargo metadata` は Cargo.lock があっても、他の OS 用を含む全依存の `.crate` を取得する。2026-10-14 に確認。v0.15) は、docs.rs の crate の照会を止めないよう Cargo.lock を直接読み、`resolved_from: "cargo_lock_raw"` で区別して warnings に書く。この経路ではローカルビルドしない (範囲を判定できないため。R-MCP-3)。`cargo metadata` の失敗を `build_failed`・`fetch_failed`・`offline_miss`・`lockfile_outdated` として返すのは (R-WS-7・下の `--locked`)、ローカルビルドを要する照会だけで、版の解決は Cargo.lock の直読みで続け、失敗の分類を warnings に書く (v0.17)。Cargo.lock が古いときは、版が古いかもしれない旨と更新の案内も warnings に書く。Cargo.lock が無ければ直読みもできないので、プロジェクトの版の解決を要する照会は `lockfile_outdated` を返す (v0.17)。M1 で決めたこと (m1-plan.md の B5b・B5c・C1b-2。v0.19):
    - 直読みでは改名と lib 名が分からないので、パッケージ名から作った名前で引く。member と path 依存を区別できないので、source の無いパッケージをすべて member とみなす。`dependencies` の `name`・`name version`・`name version (source)` の 3 つの書き方を読む
    - warnings には、直読みにしたことと失敗の種類、Cargo.lock が古いときは `cargo fetch` の案内を書く。同じ crate のほかの版があれば、それも書く
    - 版を省いた照会で、crate がプロジェクトの依存でなければ latest にして warnings に書く (R-VR-3)
  - パッケージは**名前ではなく package id で索引する**。同名の複数バージョンを共存させる
  - **同じ crate が複数バージョンある場合は、黙って選ばない**。対象のプロジェクト (R-MCP-3) の member から直接依存しているものが 1 つに決まれば、それを使い、ほかの版を warnings に書く。決まらなければ (0 個か 2 個以上) `ambiguous_version` を返し、全部の版を古い順に、各々を使っている依存元と一緒に `candidates` に添える。直接の依存には、dev・build を含むすべての種類と改名した依存を数え、member 自身も直接とみなす (M1 で決めた。v0.19)
  - バージョンの大小は semver で比べる。文字列比較をしない (`"0.9.0" > "0.10.0"` になる)
  - docs.rs から取ってよいのは `source` が crates.io のパッケージで、かつ Cargo.lock の `checksum` が sparse index の `cksum` と一致するものだけ (docs.rs は crates.io の crate しか扱わない。一致すれば、ソースの置き換え (vendor など) の中身が crates.io と同じだと分かる。v0.18)。path / git / workspace のパッケージは、同名の crates.io の crate と取り違えないようローカルビルドに回す (crates.io の index を引く前に判定し、同じ名前の crate が crates.io にあっても代わりにしない。v0.19)。checksum が一致しないものもローカルビルドに回す (v0.18)。crates.io の source は `registry+https://github.com/rust-lang/crates.io-index` と `sparse+https://index.crates.io/` の 2 つの書き方を認める (v0.19)。`[source.crates-io] replace-with` (directory source) では、`cargo metadata` の source は crates.io のまま、`manifest_path` だけが vendor を指した (2026-10-19 に確認。ソースの読み方は R-SC-2)。M2 で決めた形 (m2-plan.md の D7b。v0.20):
    - 照合はプロジェクトのパッケージ名で index を引いて行う (lib 名や改名で引かれても、別の crate の index を読まない)。checksum の不一致・checksum 無し・index に無い版・index に無い crate (私的なミラー) のものを写しとし、依存としてローカルビルドして、docs.rs を使わない理由を warnings に 1 行書く (provenance は `source: local`・`package_source: crates.io`)。vendor のパッケージも `cargo rustdoc --locked -p <id>` で vendor の中身が文書化されることを確かめた (2026-10-22、cargo 1.99.1)
    - index に版や crate が無ければ、TTL によらず条件付きの要求で 1 回だけ読み直す (同じ crate の読み直しは TTL の間に 1 回)。オフラインや通信の失敗で古い index しか読めず版が無ければ、写しと断定せず `offline_miss`・`network_error` を返す
    - Cargo.lock の版 1 (checksum を `[metadata]` に持つ形) の checksum も読む。cargo は版 1 も `--locked` で受け、書き換えない
    - re-export の行き先 (R-PR-4) も照合する。起点がここでビルドした crate なら写しをローカルビルドし、起点が docs.rs の crate なら、docs.rs がそれでビルドした crates.io の版を warning 付きで使う
    - `docs_unavailable` (`reason: local_build_required`) は、lib の無い依存だけに返す
  - `cargo metadata` は毎回起動しなくてよいが、`cargo update` など入力が変わったら必ず作り直す。M1 では、成功した結果だけをプロセスのメモリに workspace のルートごとに持つ (このリポジトリの 248 パッケージで 1 回約 170ms。m1-plan.md の B5b。v0.19)
    - 鍵は入力の中身: Cargo.lock、ルートの Cargo.toml、前回の結果で source の無いパッケージ (member と path 依存) の Cargo.toml、ルートの祖先の `.cargo/config(.toml)` と `rust-toolchain(.toml)`、`$CARGO_HOME/config(.toml)`、`members` の glob の展開結果。cargo の設定の `include` が指すファイルも入れる (読み方は R-WS-2。v0.20)
    - glob の展開が 1 万エントリか深さ 32 を超えたら諦め、その結果はキャッシュしない。実行中に Cargo.lock が変わった結果もキャッシュしない
    - `cargo metadata` は workspace ごとの共有の仕事にして、同時の呼び出しを 1 回にまとめる。呼び出しは期限 (R-ARCH-6) まで待ち、過ぎたら版が決まる前の `status: building` を返して、解決は裏で続ける。失敗 (と、そのとき Cargo.lock を直に読んだ結果) は、入力が同じ間 30 秒だけ覚える (一時的な失敗の TTL。R-ERR-3)。成功は入力が同じ間ずっと使う (v0.20)
    - `cargo tree` (R-FT-3 の feature の組、R-MCP-3 の範囲、docs.rs の組の範囲の確かめ) の結果も同じ鍵で持ち、(workspace, target) ごとの共有の仕事にする。期限までに分からなければ、ビルドの計画では `status: building` を返し、結果の注記 (`enabled_in_project` など) にだけ使う照会では、分からない旨の warning を付けて結果を返す (v0.20)
  - **ユーザーの Cargo.lock を書き換えない**。`cargo metadata` も `cargo rustdoc` も、Cargo.lock が無いか古いと解決し直して書き込み、ネットワークで index を取りに行く。プロジェクトの文脈で cargo を呼ぶときは常に `--locked` を付ける。Cargo.lock が無いか古ければ、cargo は stderr に `because --locked was passed` を含むエラーを出して終了する (どちらも 2026-10-14 に確認)。これを `lockfile_outdated` として返し (返す照会の範囲は上の例外のとおり。v0.17)、利用者かエージェントが Bash で Cargo.lock を更新するよう案内する (文面は §8)。オフラインモードでは `--offline` も付ける。R-WS-5 の仮のプロジェクトは、キャッシュの下にあるサーバー自身のものなので対象外 (そこでの解決はネットワークを使い、オフラインモードでは `offline_miss` にする)。仮のプロジェクトの Cargo.lock は、最初に 1 回だけ `--locked` なしで作り、以後は `--locked` で使う (呼び直しで推移的な版が動いて鍵が変わらないように。引数 `git` の ref を解決し直すときは作り直し、ハッシュの rev では作り直さない。v0.20)。外のツールで指した外の workspace に Cargo.lock が無いか古ければ、`lockfile_outdated` と `cargo generate-lockfile` の案内を返し、サーバーは書かない (v0.20)
- R-VR-3 latest:
  - yanked と pre-release を除いた最大のバージョン (sparse index の `yanked` を見る)。docs.rs からはこの版の完全な URL で取る (R-DS-6)。そういう版が無いときは、yanked でない pre-release の最大 → yanked を含めた最大の順に探し、warnings で知らせる。semver 要件も同じ 3 段で探し、後の 2 段では pre-release を外した版で照合する (要件に pre-release の名前が無いと当たらないため。M1 で決めた。v0.19)
  - プロジェクトの依存でない crate なら、`resolved_from: "latest"` と「プロジェクトの依存ではない」旨を載せる
- R-VR-4 差し替え禁止: 解決したバージョンのデータが取れなくても、別のバージョンで代用しない。`docs_unavailable` を返し、JSON がある近くのバージョンを候補として添える。返った JSON の `crate_version` が解決した版と違えば、その JSON を使わず、キャッシュにも書かない (R-DS-6 の 200 で別の版が返る場合。v0.18)。そのときは `docs_unavailable` (`reason: version_mismatch`) を返す (v0.19)
  - 合う版が無いとき (`version_not_found`) の候補は、yanked でない版から最大 5 個を大きい順に出す。明示した版と同じ major・minor のもの、無ければ同じ major のもの、無ければ全体から選ぶ (M1。v0.19)
- R-VR-5 crate をまたぐ参照 (§5.5) にも同じ規則を適用する
- R-VR-6 バージョン一覧の取得には crates.io の sparse index (`index.crates.io`) を優先する。結果は TTL 付きでキャッシュする (§9.3)。完全一致の版を求め、その target の JSON がすでにキャッシュにあれば、index を引かない (オフラインでも引けるように。この近道では yanked を確かめない。`resolve_version` は近道を使わず常に index を読む。M1。v0.19)。完全一致の版で、外のツールの記録 (R-WS-10) の置き場があるときも、index を読まずに版を決める (v0.20)。日次の名前一覧などを使う場合は鮮度を provenance に載せ、そこに無いことを「存在しない」と扱わない

### 5.4 target / features / cfg

- R-TG-1 target の既定値:
  - ローカルビルドでは、プロジェクトの `build.target` (`.cargo/config.toml` など、cargo が実際に読む設定。R-WS-1。配列なら先頭の要素を既定にして warnings と provenance に出し、ほかの要素は `target` 引数で指す。v0.16。配列の並びは cargo の設定の合成の規則で決まる。R-WS-2。v0.18)、なければホストの target。`build.target` は cargo の設定の合成 (R-WS-2) で読む。`"host-tuple"` (cargo 1.91 から) はホストの target とし、target として解析できない要素が 1 つでもあれば全体をホストの target とみなして warnings に書く (v0.20)。docs.rs の照会・`enabled_in_project` の評価 (R-FT-2)・feature の組の `cargo tree --target` も、照会の target と同じ target で行う。`target` を省いた照会の target は、プロジェクトが使う crate なら `build.target` (の先頭)、プロジェクトが使わないと分かった crate と引数 `git` の crate はホストの target にする (外のツールが既定でホストの target で作るのと合わせる。R-WS-5。v0.20)
  - ホストの target は実行時に `rustc -vV` から取る (サーバーをビルドしたときの target ではない。実行ファイルのパスを指定していれば R-WS-1 で決めた rustc で)。ホストの target はプロジェクトの設定によらないので、プロジェクトのディレクトリではなく中立のディレクトリ (キャッシュの下) で実行する。ローカルビルドで `--target` を省くかの判定には、これではなく、ビルドに使う toolchain の `rustc -vV` の host を使う (R-WS-1。v0.17)
    - 中立のディレクトリでの `rustc -vV` は、rustup に既定の toolchain が無い環境では失敗する (2026-10-16 に確かめた)。取れずに代わりの値を使ったときは warnings に理由を書く。代わりの値は、サーバー自身をビルドした target (ビルド時に埋め込む) を、musl なら gnu に読み替えたもの (`x86_64-unknown-linux-musl` → `-gnu`、`armv7-unknown-linux-musleabihf` → `-gnueabihf`) にする (M1 で決めた。v0.19)
    - `rustc -vV` には `RUSTUP_AUTO_INSTALL=0` を付け、10 秒で打ち切る。起動時に裏の task で求め始め、`server/discover` と `tools/list` は待たせず、ツールの呼び出しだけが結果を待つ (R-MCP-11。v0.19)。ビルドに使う toolchain の `rustc -vV`・`rustdoc -vV` (R-WS-1) も 10 秒で打ち切る。時間切れは一時的な失敗として `build_failed` (`retryable: true`) で返し、失敗は 60 秒覚える (遅い CI の runner で 10 秒を越えたことがある。v0.20)
  - ローカルビルドの前に、使う toolchain の sysroot に `lib/rustlib/<target>` があるかを見て、無ければ cargo を起動せずに `toolchain_missing` を返す (rustup の環境なら `rustup target add` を案内する。rustup の無い環境では rustup のコマンドを案内しない)。このディレクトリの有無が rustup の一覧と一致し、無い target で cargo を起動すると依存のビルドの途中で失敗することを M0 で確かめた (Q-30。v0.17)。host の target のディレクトリは rustc の component だけでもできるので、host の `rust-std` の有無は `lib/rustlib/<host>/lib` に libcore があるかで見る (v0.18)。この確かめは `RUSTUP_AUTO_INSTALL=0` で行い、プロジェクトの toolchain が入っていない (rustup が「not installed」と言う) ときは、オンラインならビルドの cargo に rustup の自動インストールを任せ (その間は `status: building`)、オフラインなら `toolchain_missing` を返す (v0.20)
  - 独自の target で std を自分で作るプロジェクト (`build.target` が target の JSON 仕様 (`.json`) のもの、cargo の設定に `[unstable] build-std` があるもの) は v1 では対応しない。member と依存は、どの `target` でもビルドせずに `docs_unavailable` (`reason: target_unsupported`) を返し、member は利用者が手元で作る方法を案内する。M2 で決めた形 (m2-plan.md の D7c。v0.20): 起動しない cargo はビルド (`cargo rustdoc`) と `cargo tree` (feature の組と範囲) で、`enabled_in_project` は出さずにその旨を warnings に書く。版の解決の `cargo metadata` は走らせる (ビルドも build script も走らない)。`[unstable] build-std` だけで `build.target` が無いものも独自の target とする。docs.rs の照会は、`target` を省いたときだけその crate の docs.rs での既定 target にフォールバックして R-TG-2 のとおり明示し (docs.rs を読むツールすべて)、`target` を明示すればその target の JSON を引く。std はホストの target を使い、その旨を warnings に書く。JSON の仕様は、今の cargo では stable でも nightly でも `cargo tree` からして unstable な指定を求め、文書化には `-Z build-std` も要る (M0 で確かめた。m0-plan.md の §6.2 の B7-8。v0.17)。上の sysroot の判定は、これらに当たらない構成だけで行う
  - docs.rs では、求める target (プロジェクトの `build.target`、なければホストの target) でビルドされた JSON だけを使い、**別の target の文書で代用しない** (独自の target のプロジェクトだけは例外。R-TG-2。v0.18)。その JSON が docs.rs に無いとき、既定のプロジェクトの依存はローカルビルドする (照会のツールがビルドする範囲は R-MCP-3、戻す対象は §5.1 の表。v0.20)。ローカルビルドしないもの (依存でない crate など) には、`docs_unavailable` (`reason: no_target_json`。§8) を返す (v0.18)。target 付きの URL が 404 なら、target の無い URL に `HEAD` を送り、200 なら `no_target_json` を返して 1 時間覚える。`HEAD` が失敗したら、何も覚えずに `no_json` を返す (M1。v0.19)
  - docs.rs がビルドする target (公式の告知 <https://blog.rust-lang.org/2026/04/04/docsrs-only-default-targets>): 2026-05-01 からは、`[package.metadata.docs.rs]` に `targets` を書いていない crate を、既定の target (`default-target`、無ければ `x86_64-unknown-linux-gnu`) だけでビルドする。対象は新しいリリースとビルドし直しで、それより前のリリースは 5 つの target (`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、`aarch64-apple-darwin`、`x86_64-pc-windows-msvc`、`i686-pc-windows-msvc`。Apple の既定は 2025-10-16 の告知で `aarch64-apple-darwin` になった) のまま。2026-10-19 に、tokio 1.52.0 は確かめた 3 つの target のどれも 200、1.53.3 は Linux 以外が 404 だった。crate は `targets` / `default-target` / `additional-targets` で変えられる。したがって、x86_64 Linux 以外のホスト (Mac・Windows など) では、新しいリリースの多くでホストの target の JSON が無い前提で作る (v0.18)。ローカルビルドの無い M1 の版は実質的に Linux 向けで、README にそう書いた (`target` に Linux を渡せば引ける。v0.19)。M2 からは、macOS・Windows でも既定のプロジェクトの依存をローカルビルドで引ける (3 つの OS の CI の `real_data` で確かめた。初めて引く依存はビルドを待つ)。依存でない crate の、docs.rs に無い target の文書は外のツールで作る (R-WS-10。v0.20)
- R-TG-2: target をフォールバックした場合 (std の R-STD-1・R-STD-6 と、R-TG-1 の独自の target のプロジェクトでの docs.rs の照会。それ以外の docs.rs の照会では代用しない。R-TG-1。v0.18) は、**実際に使われた triple** (JSON の `target.triple` から読む。Q-13) を provenance に載せ、warnings に明示する。R-TG-1 で選んだ target で 404 だったことはディスクに記録し、次回の無駄な試行を省く
- R-FT-1: docs.rs の JSON は、どの feature でビルドしたかを記録していない。`.crate` 内の `Cargo.toml` の `[package.metadata.docs.rs]` (`all-features` / `features` / `no-default-features`) から求めて provenance に載せる。あわせて「docs.rs のビルドで偽だった cfg の item は JSON に存在しない」旨を必要に応じて warnings に出す
  - M1 で決めた形 (m1-plan.md の C1d-2。v0.19): `.crate` (R-SC-2) の Cargo.toml から、有効な feature、`rustdoc-args`・`rustc-args` の `--cfg` の名前 (`docsrs` と `feature` を除く)、有効にならなかった feature を求める。最後のものは、`[features]` と、`dep:` で名指しされない optional の依存 (`[target.*]` の下と build-dependencies も) が作る feature のうち、有効な feature から推移的にたどれないもの (`dep/feat` は依存の feature を有効にし、`dep?/feat` はしない)。feature 名と cfg 名は R-SEC-4 の文法に合うものだけを受ける。外のツールの仮のプロジェクト (R-WS-5) でも、features の組と `--cfg` は同じく `.crate` を先に取って読む。そこで `.crate` が取れなければ、既定の組では作らずに `network_error` (オフラインなら `offline_miss`) を返す (v0.20)
  - provenance には `features` (有効にした feature。`all_features` のときは省く。tokio では 26 個になるため)、`all_features`、`cfg` (docs.rs が立てた cfg) を載せる。warnings は、有効にならなかった feature を挙げる形 (「docs.rs はこの crate を [..] なしでビルドした。それらが要る item はこの文書に無い」) にする (有効な feature を挙げるより、答えに直結するため)
  - `.crate` を読めなければ (index を引けず `cksum` が得られないときなど)、warnings に書いて照会は続ける。依存の横断の検索 (R-SR-2) では、範囲が広いと時間がかかるので `.crate` を読まず、provenance の `features` を出さない
- R-FT-2 **item ごとの有効 cfg を求める** (Q-3):
  - rustdoc JSON には cfg の専用フィールドが無い。情報は `Attribute::Other` の文字列としてだけ残る
  - 子 item には親の cfg が付かない (`axum::extract::ws` モジュールには cfg があるが、その中の `WebSocket` の `attrs` は空)
  - したがって、**有効 cfg = その item 自身の cfg ∧ 関連 item なら親 (impl ブロック・trait・enum。親の正準パス上の cfg も) の cfg ∧ 公開パス上の全祖先モジュールの cfg ∧ re-export の連鎖上の各 `Use` の cfg** として求める (関連 item の親は v0.18、enum は v0.19 で足した)。M1 では、正準パスがたどるモジュールと `use` を公開パス索引に記録して使う。各 item では `doc(cfg)` があればそれを、無ければ痕跡 (下の情報源 2) を使い、同じ条件は 1 度だけ数える。公開パスが複数あれば、正準パスの有効 cfg を出し、`also_reexported_at` (R-SR-4) の各パスには、正準パスと違う場合だけそのパスの有効 cfg を添える (`also_reexported_at` の各パスの条件と、crate の外への re-export (`reexport_of`) の条件は、M4 で feature の機能と一緒に入れる。v0.19)
  - 情報源の優先順:
    1. `#[doc(cfg(...))]` (形式 46〜61 で表記が安定している)
    2. cfg の痕跡 (trace) 属性。**表記は format_version ではなく実際の文字列の形で判別する**。同じ形式 57 の中でも変化している (`#[<cfg>(...)]`、`#[<cfg_trace>(...)]`、`#[attr = CfgTrace([NameValue { ... }])]` の 3 系統を確認済み。同じ形式 61 でも、stable 1.99 と nightly 1.101 で属性の表記が違うものがあった (`CfgAttrTrace`。M0 の B1。Q-14。v0.17))。現行の表記は `span: lib.rs:10:7: 10:20 (#0)` のようにファイルパスと位置を含むので、解析はこれを読み飛ばせるようにする
       - M1 で testdata の 20 crate (形式 46・48・57・61) を読むと、表記は `doc(cfg)` を含めて 4 系統だった: `#[doc(cfg(...))]` (全形式で同じ) と形式 46・48 の `#[<cfg>(...)]` は Rust の構文で読み、形式 57・61 の `#[attr = CfgTrace([...])]` は rustc の Debug 表記 (`NameValue`・`All`・`Any`・`Not`・`Bool`。位置は閉じ括弧まで読み飛ばす。並びの各項は「かつ」) で読む。`CfgAttrTrace` と `<cfg_attr>` は、ほかの属性に掛かる条件で item の条件ではないので読まない。cfg の属性 12,977 件がすべて読めた (m1-plan.md の C1d-1。v0.19)
    3. (任意) `.crate` のソースを `syn` で解析した `#[cfg(...)]`。ソースの item と JSON の item の対応づけが要り重いので、v1 は 1・2 と `cfg_source: unknown` で受け、eval で取りこぼしを測ってから採否を決める (v0.16)
  - 通常のビルドでは `doc` と `docsrs` は偽なので畳む (`any(all(doc, docsrs), windows)` → `windows`)。畳むと偽になるもの (`#[cfg(docsrs)]` の item) は、文書にだけある item だと分かるように元のまま出す (M1。v0.19)
  - 出力: `required_features` (`any` / `all` / `not` の構造を保つ)、`other_cfg` (`tokio_unstable`、`target_os` など)、`cfg_source` (`doc_cfg` / `cfg_trace` / `inherited` / `source_parse` / `unknown`)、`enabled_in_project`、`enabled_in_dev_only`
    - M1 の形 (v0.19): 条件の一番外の「かつ」の各項を、feature だけのもの (`required_features`。feature 名だけで `any(rt, rt-multi-thread)` のように書き、複数なら `all(...)`) と、それ以外 (`other_cfg`。cfg の書き方で 1 項ずつ。feature とそれ以外が混ざった項もこちら) に分ける。`cfg_source` は item 自身の情報源 (`doc_cfg`・`cfg_trace`)、自身に無く親や経路から来たなら `inherited`、読めないものがあれば `unknown` (`source_parse` は情報源 3 を入れたときだけ)。条件が無ければどれも出さない。`detail=brief` でも出す
  - `enabled_in_project` は `true` / `false` / `unknown` の 3 値にする (v0.18)。評価の規則 (M1 で利用者が決めた。m1-plan.md の C1d-2・C1d-3。v0.19 で、feature 以外の cfg があれば `unknown` とした v0.18 の規則を改めた):
    - 条件の一番外の「かつ」の各項を 3 値で評価して「かつ」で結ぶ。feature を含む項はプロジェクトの組で評価する (`doc`・`docsrs` は偽、feature 以外の部分は不明)
    - feature を含まない項は、docs.rs のビルドと同じく成り立つとみなす。照会に使う JSON は評価する target と同じ target のものだけ (R-TG-1) なので、`unix`・`not(loom)` などは docs.rs と同じ値になる。ただし、docs.rs が `[package.metadata.docs.rs]` の `--cfg` で立てた名前 (`tokio_unstable` など。R-FT-1) を含む項と、JSON の target が評価する target と違うときは、上と同じく評価し (多くは `unknown`)、条件の cfg を添える。`docsrs` だけの項 (文書にだけある item) は偽
    - 通常のビルドがその crate を作らなければ `false`
    - 出すのは、プロジェクトが版を決めたとき (`resolved_from: cargo_lock`) だけ。feature の組が取れなければ (`cargo_lock_raw` など) 出さずに warnings に書く。版を引数で決めた照会 (完全一致・semver 要件・`latest`) でも、解決した版がプロジェクトの crates.io のパッケージと同じ版で、Cargo.lock の checksum が sparse index と合えば、プロジェクトのビルドで評価して出し、provenance に `project_from` を載せる。checksum が合わなければ出さずに warnings に書く (m2-plan.md の B8。v0.20)
  - `enabled_in_project` は、実際のビルド target (R-TG-1) での通常のビルド (dev-dependency を除く) の feature の組で判定する。dev-dependency の経路でだけ有効なものは `enabled_in_project: false`・`enabled_in_dev_only: true` で示す (出荷するコードで使えない API を「有効」と言わず、テストでは使えると分かるように。確認なしでビルドする範囲 (R-MCP-3) が dev を含むこととは別の基準。v0.15。resolver 2 以上では、dev だけの依存はローカルビルドの範囲から除く。R-WS-5)。組の得方は R-FT-3。`enabled_in_dev_only` は、resolver 2 以上では `-e normal,dev` の組と `-e normal` の組の差で求める。resolver 1 では dev の feature も通常のビルドに統合されるので、常に `false` になる (v0.17)
  - `#[doc(auto_cfg(hide(...)))]` を尊重する (M4 で feature の機能と一緒に入れる。v0.19)
  - **cfg が見つからないことを「feature 不要」と断定しない**。判断できなければ `cfg_source: unknown` とする
  - **一覧系の各項目にも付ける**: `list_module`・`search_items`・`list_methods`・`list_impls`・`list_implementors` の各項目に (`detail=brief` でも)、必要な feature の 1 行の要約 (例: `feature: sync`、`feature: any(rt, rt-multi-thread)`) と `enabled_in_project` (dev の経路でだけ有効なら `enabled_in_dev_only` も。`unknown` なら条件の cfg の注記も。v0.18) を付ける。「どの feature で有効か」に `get_item` を追加で呼ばずに答えるため (groxide の `[feature: sync]`、truesight の `#[cfg(feature = …)] pub fn …` の 1 行表記に倣う)。feature が不要な項目には付けない。M1 では、`list_module`・`search_items`・`list_methods` の各項目には item の条件を、`list_impls`・`list_implementors` の各項目にはその impl 自身の条件 (`required_features`・`other_cfg`・`cfg_source`・`enabled_in_project`・`enabled_in_dev_only`) を付ける。型や trait の条件は `get_item` の側にあるので繰り返さない (v0.19)
- R-FT-3 ローカルビルドの feature (Q-10):
  - **workspace member**: プロジェクトの通常のビルドで有効な feature の組を `--no-default-features --features ...` で再現する (その組の得方は下記)。その member の Cargo.toml に `[package.metadata.docs.rs]` の `features`・`all-features`・`no-default-features` があれば、その組で文書化する (文書のための組をプロジェクトが Cargo.toml で決められるように。docs.rs と同じ書き方。v0.14)。`enabled_in_project` (R-FT-2) は、この場合も通常のビルドの組で判定する。照会のツールは、この組で確認なしにビルドする範囲を広げない (R-MCP-3)。その組でビルドが壊れたら (相互排他の feature や、システムライブラリの要る optional の依存など)、通常のビルドの組でやり直して warnings に書く。metadata から読んだ feature 名も R-SEC-4 で検証する。M2 で決めた形 (m2-plan.md の D3d・D3e・D4d・E4c。v0.20):
    - 通常の組: その member が `cargo tree --workspace -e normal` で持つ組 (全 member を統合した組。`enabled_in_project` と同じで、rust-analyzer が既定で見る `--workspace` の組とも合う。下の Q-31 の実測) を、`cargo metadata` で宣言された features に絞って `--no-default-features --features` で渡し、`--manifest-path` で member を指す (resolver 1 でルートにパッケージがある workspace では、指さないと features がルートのパッケージに効く)。組が得られなければ (`cargo tree` の失敗、Cargo.lock の直読み、target 向けに作られない member)、`-p` の既定でビルドして warnings に書く
    - docs.rs の組を使うのは、`features`・`all-features`・`no-default-features` のどれかがあり、通常の組と違うときだけ (`rustdoc-args` だけなら通常の組で、その `--cfg` は付ける)。その組で `cargo tree -p <member>` を走らせ、通常の組の範囲 (R-MCP-3) に無いパッケージが増えなければその組でビルドする。増えるか確かめられなければ、通常の組でビルドし、外のツールに渡す引数を warnings で案内する。外のツールはその組で離れた置き場に作り (R-WS-10)、照会のツールはその記録があれば優先する
    - 壊れたとき: docs.rs の組と `--cfg docsrs` (と `rustdoc-args` の `--cfg`) で始め、文書化する crate のエラーなら `--cfg docsrs` を外し、それでもコンパイラのエラーで失敗するか、依存のコンパイラのエラーなら、通常の組に戻す (`--cfg docsrs` は元に戻す)。ビルドは最大 3 回。後退した形で通ったことは置き場の前回の成功の記録に残し、toolchain・`rustdoc-args` の cfg・docs.rs の組・サーバーの版が同じ間は、最初から後退した形でビルドする (それらが変わるか `refresh` で試し直す。編集のたびに試し直すと rustdoc 1 回分 (このリポジトリで約 5 秒) 余計にかかるため)。通常の組で壊れるときは `-p` の既定に戻さず、ビルドの失敗を返す (`cargo build --workspace` も壊れるプロジェクトの問題として)
  - **依存 crate**: feature のフラグを**渡さない**。workspace のルートから `cargo rustdoc -p <dep>@<ver>` を実行すれば、workspace で統合された feature が効く (2026-10-14 に確認: path 依存 b の `#[cfg(feature = "x")]` の item が、member a が有効にした feature x のとおりに出力された)。ただし統合されるのは**通常の依存の辺**の feature だけで、dev-dependency や build-dependency だけが有効にする feature は効かない (resolver 2。2026-10-14 に確認)。どの組で作られたかは、予測せずに下記のとおり記録する (v0.18)
  - 依存 crate に `--features` や `--no-default-features` を渡すと、cargo は `cannot specify features for packages outside of workspace` で失敗する (KevinT3Hu の実装はこの方式で、依存の生成が毎回失敗している)
  - `--all-features` を既定にしない (相互排他の feature がある crate で壊れる)
  - 要求した feature と実際に使った feature の両方を provenance に載せる。**`features_effective` と `enabled_in_project` (R-FT-2) を `cargo metadata` の `resolve.nodes[].features` から取ってはならない**。これは dev・build・target 別の依存の feature まで統合した値で、通常のビルドより多い (dev-dependency だけが有効にする feature が入る)。workspace member を `--features <resolve.nodes の値>` で再現すると、同じ理由で通常のビルドより多い feature で文書を作ってしまう
  - **文書の feature の組は、ビルドしたときの cargo の `compiler-artifact` のメッセージの `features` を正として記録する** (`features_effective`)。事前に予測しない (Q-20。v0.18 で改めた)。M0 の B3 で決めた「`cargo tree --workspace --target <t> -e normal` の組は、`cargo rustdoc --lib` が実際にビルドに使う組と一致する」は、member が 1 つの workspace でしか成り立たない: member a・b が依存 shared の別の feature を使うと、`cargo tree --workspace` は `[x,y]`、a の文書化は `[x]` になる (2026-10-19 に確認)。workspace の中の path 依存は暗黙の member になり、自身の既定の組で作られる。`default-members` もある。これらでの確認は Q-31。M2 の実測 (2026-10-21、cargo 1.99.1、resolver 2。m2-plan.md の D3c): `cargo rustdoc -p a` は引数なしの `cargo build` (default-members) と同じ組、`cargo tree --workspace` は `cargo build --workspace` と同じ組で、別の member だけが有効にする feature があると両者は食い違う。そこで member は全 member を統合した組で作ると決めた (上の通常の組。v0.20)
  - **`enabled_in_project` (R-FT-2) は、全 member を統合した組で判定する**: `cargo tree --workspace --target <target> -e normal` で得る (target は実際のビルド target。R-TG-1。M0 の実測は `--target <host>` を付けて行った。付けない形でも M0 と同じ結論だった (2026-10-20、cargo 1.99.1。Q-31。v0.20))。M1 は `--prefix depth --format '{p}\t{f}'` で木の形を読む。resolver 2 以上では同じパッケージが target 用と host 用 (proc-macro の下) で別の組で出るので、proc-macro とその下を host 用とし、パッケージの組は、ルート以外で target 用の位置に出た組 → ルートの組 → host 用の組 (proc-macro だけが使うパッケージ) の順に選ぶ (`-e no-proc-macro` は host 用のルートの行を消さなかったので使わない。m1-plan.md の B5d。v0.19)。`enabled_in_dev_only` の `-e normal,dev` の組も同じ形で取る。`cargo metadata` が失敗したときは実行しない。target ごとの依存・build-dependency・dev-dependency で feature が分かれる場合も、この組で判定する (v0.18)。dev-dependency の feature は `--all-targets` のビルドにだけ混ざり、build-dependency の feature は host 向けの別の unit にだけ効く (resolver 2)。範囲の判定 (R-MCP-3・R-WS-5) の `cargo tree` は、build と dev の辺が `--prefix indent` の見出し (`[build-dependencies]`・`[dev-dependencies]`) にしか出ない (`--prefix depth` では出ない) ので、indent の形を読む。target 用と host 用の 2 つの根に出る member (proc-macro や build-dependency として使われるもの) は、その member の `-i <id> --no-dedupe` の逆向きの木で見分ける。行は名前と版で、決まらなければ取得元でも照合して `cargo metadata` のパッケージ id に対応付け、それでも決まらなければ範囲を判定できないものとする (m2-plan.md の B7。v0.20)。feature の組の host 用と target 用の選び方は上のまま

### 5.5 パスの解決と re-export

- R-PR-1: 入力として次の形を受け付ける
  - 正準パス、定義パス
  - re-export 経由のパス (`axum::Router`)
  - メソッド・関連 item のパス (`tokio::sync::Mutex::lock`、`Iterator::map`)
  - プリミティブ型とそのメソッド (`slice::sort_by`、`str::len`、`f64::sqrt`)
  - `crate@version::path`
  - 任意 (LLM がその形で問うかを eval で見て採否を決める。受けない形は `invalid_argument` で書き方を案内する。v0.16): trait 修飾のパス (`<Vec<T> as IntoIterator>::into_iter`)、docs.rs の URL、パスの区切りごとの rustdoc の区別子 (`struct@Foo`、`fn@foo`、`mod@x` など、intra-doc link と同じ書き方。名前だけでは曖昧なものを区別する。ferritin の `resolver.rs` に倣う)
  - **crate の名前** (v0.18): `crate` 引数 (と `crate@version::path` の crate) は、パッケージ名とコードに書く名前 (lib 名・依存の改名) の両方を受け、文脈で解く。プロジェクトの文脈があれば、`cargo metadata` の依存の名前 (extern の名前) からパッケージに解き、パッケージ名でも引ける。extern の名前とパッケージ名が別のパッケージに当たれば、候補を付けて `ambiguous_crate` (§8) を返す (v0.19)。文脈が無ければパッケージ名として crates.io を引く (`-` と `_`、大文字と小文字の違いは sparse index で正規化する。`tokio-util` と `tokio_util`)。path の先頭は lib 名 (JSON のルートの名前) に合わせ、パッケージ名と lib 名が違えば provenance に両方出す。キャッシュの鍵も正規化した名前にする (R-CA-2)
    - M1 の形 (v0.19): sparse index では、小文字にし、与えられた綴り → `_` をすべて `-` にした綴り → `-` をすべて `_` にした綴りの順に試す (`-` と `_` を公開と違う混ぜ方で書いた名前は見つからない)。公開された綴りは index の `name` か、`.crate` の Cargo.toml の `package.name` を正とする (`Serde_Json@1.0.154` は `serde_json`)。パッケージ名と lib 名が違う crate (`md-5` の lib は `md5`) では、パスの先頭のパッケージ名を lib 名に置き換える (`md-5::Md5` も `md5::Md5` も引ける)。provenance の `crate` はパッケージ名、応答のパスの先頭は lib 名なので、両方が応答に出る (引数で両方を受けることと合わせて、この形で上の要件を満たすとした)
  - 解決の手順: 公開パス索引 (R-PR-2) で引く。見つからなければ、親を同じ手順で引いてから、子を impl・trait・enum の中でたどる (`paths` に載らない impl の中の item 用。ferritin の 2 段のフォールバック。v0.17)。それでも無ければ定義パス (`paths`) で引く (v0.19 で、定義パスを親のメンバーより後にした。nightly の std はメソッドも `paths` に載せていて、`Vec::push` が親も impl も無い単独の item として返っていたため。モジュールの下の item はメンバーにならないので、結果が変わるのはメンバーだけ。m1-plan.md の C1b-3)
- R-PR-2 **公開パス索引**: ルートモジュールから `Module.items` と `Use` item (glob・非 glob・crate をまたぐもの) をたどって、item ごとの公開パスの集合を作る
  - `paths` の定義パスを、使えるパスとして見せてはならない (private module を含みうる。dmvk の Issue #5 や、umbrella crate で引けない問題の根本原因)
  - 表示するのは正準パス。定義パスは異なる場合だけメタデータとして返す
  - 公開パスを持たない型が公開シグネチャに現れる場合は `unnameable` として示す。M2 では入れておらず、M4 で依存グラフ全体の索引 (R-TI-3) と一緒に入れる。それまでは、依存の非公開の item を引かないので (R-PR-3)、依存の名前で書けない戻り値の型は `get_item` で引けない (v0.20)
  - `pub use Trait as _;` は名前を作らないので、公開パスにしない (`prelude::_` は `use` できない。M0 で rustdoc_processor との突き合わせで見つけた。v0.17)。どの glob で trait がスコープに入るかを情報として持つかは、M4 で決める (v0.19)
  - std の item は公開窓口のパス (`std::iter::Iterator`) で示す。`core::iter::traits::iterator::Iterator` のような定義パスを見せない。別の crate の item を re-export のパスで引いたとき (std の中の std → alloc・core と、docs.rs の crate → 別の crate・std の両方) の示し方は次のとおり (v0.19 で M1 の実装をこれに合わせた。m1-plan.md の §6 の「要件への反映の判断」):
    - `data.path` は引かれたパス (`std::vec::Vec`、`reqwest::header::HeaderMap`) にし、定義元の crate でのパス (`alloc::vec::Vec`、`http::HeaderMap`) は `data.defined_at` に出す。`data` の中のパスのうち定義元のパスの下にあるもの (`list_methods` のメソッド、`list_module` の item、メンバーの親の型) も、引かれたパスの下に移す。2 つ以上の crate をたどったとき (futures → futures_util → core) は内側から順に移し、`defined_at` は最後の crate のパスにする。対象は `get_item`・`list_module`・`list_methods`・`list_impls`・`list_implementors`
    - provenance の `crate` (と版) は定義元の crate のまま (データの出どころを示すため)。warnings にも re-export の経路を書く
    - シグネチャの中の型のパス (`alloc::alloc::Global` など) と `also_reexported_at` は、定義元の crate のパスのまま (シグネチャの型を正準パスで描くのは M4。R-OUT-2)
    - `crate: "std"` の検索 (R-SR-2) では、alloc・core で見つけた item を std が re-export するパスで示す (`Vec push` は `std::vec::Vec::push`)。std の re-export (定義パスで指すものと、alloc がさらに core から re-export するものを含む) のうち、残りを足して最も短くなるパスを選ぶ。std 自身の re-export の項目 (prelude など) は、行き先の item が alloc・core で見つかるので結果から除く。std が glob で re-export するもの (`std::hash` の `core::hash::*`) は、`get_item` が crate をまたぐ glob をたどらない (R-PR-3) ので、alloc・core のパスのまま示す (v0.19)
  - **no_std** (v0.18。M4): 既定のプロジェクトの member が `#![no_std]` なら、std の窓口ではなく core・alloc のパスで示し、std にしか無い item にはその旨を付ける。判定は member のルート (crate root) の属性で行う。作るのは primitive と Deref を扱う M4
  - **private item のパス** (v0.18): workspace の member の item は、公開パスが無ければ crate 内でのパス (`crate::a::b::Item`) と可視性 (`pub(crate)` など) を示す (検索での扱いは R-SR-4)。M2 で決めた形 (m2-plan.md の D6a。v0.20):
    - 引数の `path` も `crate::` で始めてよい。公開パスの無い item に、公開パスが無い旨の warning は付けない
    - 可視性は Rust の書き方の文字列 (`pub`・`pub(crate)`・`pub(in crate::…)`・`private`) で、member の `get_item` は常に `visibility` を付ける。docs.rs と std の item には付けない
    - 含むモジュールと同じ範囲のものは `private` (シグネチャには何も付けない)。crate のルートの private は rustdoc が `crate` で出すので `pub(crate)` (同じ意味)。`pub(super)` は JSON から見分けられないので `pub(in crate::…)` と書く。固有の impl の item は、impl が型と同じモジュールにあるとみなして判じる
    - 可視性を持たない item は親から継ぐ: trait の item とこの crate の trait の impl の item は trait の、外の trait の impl の item は型の、variant は enum の可視性。親が分からなければ付けない
- R-PR-3 `Use` の解決順 (ferritin の実績のある順序):
  1. `id` が無い → プリミティブ型の re-export。種類付きのパス索引で引く (無視すると `bool` や `str` で無関係な crates.io の crate を読みに行く)
  2. `id` がこの crate の `index` にある → その item
  3. `id` が `paths` にある → crate をまたぐ参照。定義パスの逆引き索引で解決する
  4. `source` 文字列は最後の手段 (先頭がローカルの別名でありうる)
  - 循環検出とともに深さの上限 (目安 256) を設ける。glob で互いを re-export しあう crate 群でスタックが溢れた先例がある
  - 公開パス索引 (R-PR-2) の作成には、深さだけでなく経路の数 (または作業量) にも上限を設ける。経路の組み合わせが爆発しうるため (M0 のコードは 20KB の JSON で 24 秒・5.4GB を使った。2026-10-19 に実測。v0.18)。M1 は、1 つのモジュールに入るのを 64 回までにした (作業量は「64 × item の数」で頭打ちになる。testdata の 20 crate では最大 2 回)。上限に当たったら索引に印を付け、パスが見つからないことがある旨を warnings で知らせる。深さ優先でたどるので、上限に当たると正準パスを取り逃しうる (実データでは当たらないので、幅優先にはしていない。m1-plan.md の A4。v0.19)
  - glob re-export は crate 内のものを再帰的に展開する。crate 外の glob は、相手の JSON がキャッシュにあるか取得できればサーバー側で展開する。展開できなければ `unexpanded_external_globs: [{prefix, source_crate, source_version}]` として返し、次に呼ぶべきツールを hints に示す
    - M2 の形 (m2-plan.md の D6c。v0.20): `get_item` は、crate をまたぐ glob (`pub use other_member::*;`) を起点と行き先がどちらも member のときだけたどる。行き先は `use` の行き先の id から取った定義パスで決め (`source` の文字列は使わない)、長い接頭辞の順に候補を試す。docs.rs の crate と std の glob は今までどおりたどらない
  - `#[doc(hidden)]` の item は既定で隠す。`#[doc(inline)]` / `#[doc(no_inline)]` を正しく扱う
    - M2 の形 (m2-plan.md の D6b。v0.20): 依存と std では、hidden な item を検索・一覧・`crate_overview` から除く (hidden なモジュールの中身、ほかの crate の item の hidden な再公開、hidden な impl のメソッド、この crate の hidden な trait の item も)。公開パスの一部が hidden を通るだけなら除かず、正準パスは hidden を通らないものを選ぶ。`get_item` で正しいパスを指せば引ける。member は `--document-hidden-items` で作り、hidden な item も `hidden` の印を付けて出す
- R-PR-4 crate をまたぐ参照: 参照先 crate のバージョンは、プロジェクトの依存グラフを最優先にし、決まらなければ `external_crates[*].html_root_url` (docs.rs のビルドは `https://docs.rs/axum-macros/0.5.1/x86_64-unknown-linux-gnu/` のように、crates.io の名前・版・target を含む。M0 で確かめた。v0.17) を使う。どれでも決まらなければ latest で解決し、版を推定した旨を warnings に明示する。決めた版のデータが取れない場合は R-VR-4 に従う
  - M1 の順 (m1-plan.md の C1e。v0.19): 起点の版をプロジェクトが決めたときはプロジェクトの Cargo.lock の版 (同じ crate の版が複数なら、起点が docs.rs でビルドされたときの版と同じもの) → 起点の JSON の `html_root_url` (`resolved_from: "reexport"`) → latest (warnings 付き)。起点と参照元のどちらの `html_root_url` を先に見るかという問いは、実際には起きない (rustdoc は re-export の連鎖を最終の定義の id に解決しているため)
  - `html_root_url` は crates.io の名前と crate 内部の名前 (`sha1` と `sha-1` など) の両方で索引する
  - std の私的な依存 (sysroot の hashbrown など) は `ExternalCrate::path` で見分け、crates.io の同名の crate と取り違えない。形式 54〜56 は `path` が無い (R-FV-2) ので見分けられない (v0.18)
  - ローカルビルドの JSON は版付きの `html_root_url` を持たないことが多いので、ローカル crate からの参照では依存グラフが必須
    - M2 の形 (m2-plan.md の D6c・D7a。v0.20): ここでビルドした crate (member と依存) の JSON の `html_root_url` (crate が自分で書いたもので古いことがある) は、版の手がかりにしない。行き先のパッケージは、起点のパッケージの依存を依存の名前 (改名) → lib 名の順に照合して決め、プロジェクトが版を教えなければ latest にせず re-export を返す。行き先が member・path・git の依存なら、直接引いたときと同じ共有の仕事でビルドして `build.wait_secs` まで待ち、間に合わなければ前回のビルド (`stale`) か `status: building` を返す。行き先がプロジェクトの crates.io のパッケージなら R-VR-2 のとおり照合する。docs.rs の crate からは、プロジェクトの member・path・git の依存へは入らない (docs.rs は crates.io の版でビルドしたため)
  - crate をまたぐ re-export を引かれたら、行き先の中身まで自動でたどって返す (M1 で利用者が決めた。最大 3 crate 先まで。示し方は R-PR-2。v0.19)。行き先が std の仲間 (`html_root_url` が doc.rust-lang.org) なら std の経路で引く (`tokio::io::Error` → `core::io::Error`)。たどれなければ、M0 と同じく行き先 (`crate@版` と定義パス) と、行き先を引く次の呼び出しを示す hint を返し (M0 で実装し、Claude Code がそれに従って行き先を引けた。v0.17)、たどれなかった理由を warnings に書く
- R-PR-5 (任意。M4): docs.rs がビルドに使った依存のバージョン (`html_root_url` や span のパスから分かる) とプロジェクトの Cargo.lock の版が違う場合、その依存から来た item (inline された re-export や blanket impl) に warnings を付ける。それまでは、docs.rs 由来の文書は docs.rs のビルド時の依存で作られている旨を provenance で示すだけにする。span のパスから版が分かるかは M4 で確かめる (v0.16)。docs.rs の JSON の自分の item の span は package root からの相対パスで版を含まない (M0 で確かめた。v0.17)
- R-PR-6: 1 回の問い合わせで同じ crate の異なるバージョンが必要になった場合は、黙って片方に寄せず warnings で知らせる
- R-PR-7: パスが曖昧な場合 (同名の item が複数) は候補一覧を返す (候補は最大 20 件にし、総数を `candidates_total` で添える。v0.19)。候補一覧を返すのは、この crate の中に同じパスの item が複数あるときだけにする。同じパスにこの crate の item と別の crate の item の re-export があれば、この crate の item を返し、もう一方を引く次の呼び出しを hints に示す (`serde::Deserialize` は trait を返し、serde_derive の derive マクロを hints で示す。M1 で利用者が決めた。v0.19)。型のメンバー (`Type::member`) は、固有の impl → trait の impl → blanket impl の段の順に探し、先に見つかった段で決める (同じ段に複数あれば候補一覧。M0 の実装。初期案。v0.17)。探すときは、impl の `for` がその型であることを確かめる (rustdoc の `Struct.impls` などには、その型が trait の型引数に現れる impl も入る)。持ち主の impl の探索は決定的にする (R-OUT-4。v0.18)。プリミティブ型は impl の `for` が `[T]`・`&str` などさまざまなので、M1 ではすべての impl を見る (M4 で見直す)。同じ段の候補には種類を付ける (`struct c::same` と `macro c::same`。v0.19)。見つからない場合は、まず別の解決経路 (trait 経由・Deref 経由・re-export・同名の別モジュール) を試し、それでも無ければ `did_you_mean` を返す。**候補には類似度の下限を設け、無関係な候補を出すくらいなら候補なしにする**。編集距離はバイトではなく文字で数え、ASCII 以外の名前で閾値が狂わないことを回帰テストで固定する (dmvk はバイト長と文字数を取り違えている)
- R-PR-8: 再帰の深いパス解決・型の描画は、大きなスタック (目安 16MB) を持つ専用のスレッドで行うか、明示的なスタックを使う反復に書き換える。非同期ランタイムのワーカーの既定のスタックで深い再帰をしない (ferritin は 16MB のワーカーで走らせている)。M1 は tokio のワーカーと blocking のスレッドのスタックを 16 MiB にした (v0.19)

### 5.6 trait / impl / メソッド解決

- R-TI-1 `list_methods`: その型で呼べるメソッドを、出どころ別に返す
  - 固有メソッド
  - trait メソッド (どの trait 由来か、`use` が必要か、required / provided か)。上書きされていない default メソッドも含める
  - blanket impl 由来のメソッド (汎用 trait 由来のものの出し方は R-SH-4)
  - **Deref 先のメソッド** (`String` → `str`、`Vec<T>` → `[T]`。連鎖も追う)
  - プリミティブ型の固有メソッドは core / alloc / std に分かれて定義されているので、すべてを合わせて返す (`#![no_std]` の member でのパスと印は R-PR-2。v0.18)
  - 固有メソッドと trait 由来のメソッドは M1、Deref 先とプリミティブは M4 で作る (v0.16)
  - M1 の形 (m1-plan.md の C6・E1。v0.19): 対象は `list_impls` (R-TI-2) と同じ impl (合成と negative の impl は除く)。各メソッドに `from`・`trait`・`needs_use` (trait が std の prelude (2021・2024) に無いとき。prelude は名前の一覧で持つ)・`provided` を付け、パスは `Type::method` にする。`provided` と既定のメソッドのシグネチャが分かるのは trait が同じ crate にあるときで、別の crate の trait の上書きされていない既定のメソッドは名前だけを出す。Deref 先は、M4 までは `Target` を hints に書いて、このツールを呼び直すよう案内する (`Vec` は `[T]`、`String` は `str`)。プリミティブ型を core 以外の crate で引いたら、ほとんどのメソッドは core にある旨を hints に書く
  - member と依存 (m2-plan.md の D6b。v0.20): member では `pub` でない固有メソッドも出し、その項目に `visibility` を付ける (`pub` なら付けない)。公開パスの無い自分の trait の impl のメソッドも出す。依存と std では、`pub` でない固有メソッド・hidden なメソッド・hidden な impl・この crate の hidden な trait の item を除く
  - `get_item` でも `Type::method` をこれらすべての経路で解決する。見つからない場合は、どの経路を探したかをエラーに含める
- R-TI-2 `list_impls`: 型が実装している trait を返す
  - 対象の impl は、`for` がその型の impl (プリミティブ型はすべて) と blanket impl。その型が trait の型引数に現れるだけの impl (`impl From<Elapsed> for io::Error`) は除く (v0.19)
  - **分類は構造的なフィールドだけで決める**: `is_synthetic` → auto、`blanket_impl` → blanket、`trait_` が無い → inherent、`is_negative` → negative、`Attribute::AutomaticallyDerived` → derive、それ以外 → 明示的な trait impl (v0.19 で M1 の実装の順に改めた。v0.18 までは derive を inherent より先に見ていた)。trait の名前で推測しない (`Freeze` や `UnsafeUnpin` のように新しい auto trait が増えるうえ、手書きの `impl Display` を derive と誤認した先例がある)
  - auto trait は条件付きで 1 つにまとめる (例: `auto_traits: {Send: "yes", Sync: "if T: Sync", Unpin: "yes"}`)。「X は Send か」に必ず答えられるようにする
    - M1 の形 (m1-plan.md の C5。v0.19): 値は `yes`・`no`・`if <条件>`。auto trait かどうかは、その crate のデータで合成された impl に出てくる trait と、`is_auto` の trait で決める。合成された impl のほか、明示的な impl (`unsafe impl<T: Sync + Send> Send for Arc<T>`) と negative impl (`Rc` の `!Send`) も入れる。ジェネリックな型の条件は、impl の型引数の境界と where 句から作り、何も縛らない `?Sized` は除く (Q-18)
  - 不安定な auto trait (`Freeze`、`UnsafeUnpin`) は既定で隠す (名前の一覧で隠す。分類ではなく表示の既定なので、上の「名前で推測しない」には当たらない。v0.19)
  - 条件付き実装は where 句を描画した文字列で返す
  - 公開されていない trait の impl (std 内部の `SpecExtend` など) は出さない。同じ crate の trait は、`use` できる正準パスで示し、公開パスの無いものの impl は出さない (member では、公開パスの無い自分の trait の impl も crate 内のパス `crate::…` で出す。依存と std では、hidden な impl と、この crate の hidden な trait の impl も除く。v0.20)。別の crate の trait は公開かどうかを判定できないので、隠された trait (`core::mem::SizedTypeProperties`) や隠されたメソッドが名前で出うる (既知の限界。v0.19)
  - `impls` の並びは出どころ (R-SH-4) → 分類 → trait → 見出し。各項目は `kind`・`trait` (定義のパス)・`crate` (別の crate のとき)・`header` (where 句込みの impl の見出し) (M1。v0.19)
- R-TI-3 `list_implementors`: trait を実装している型を返す。`scope` で範囲を選ぶ (依存グラフ全体の時間と容量は Q-5)
  - `crate`: trait を定義している crate の中だけ。既定
  - `dependency_graph`: プロジェクトの依存グラフ全体。**先行実装が無い差別化点** (ferritin もディレクトリを予約しただけ)。`Trait.implementations` と、他 crate の impl に対する逆引き索引を合わせる (M4。M1 では渡すと理由付きの `invalid_argument` にし、型ごとに `list_impls` で確かめるよう案内する。v0.19)
  - M1 の形 (m1-plan.md の C7。v0.19): 実装者ごとに、型 (`&T`・`[u8]`・`Box<S>` なども描く)・分類 (R-TI-2 と同じ)・impl の見出しを返し、型・見出しの順に並べる。同じ crate の型で公開パスを持たないものへの impl は出さない (R-SH-5。member では、公開パスの無い型の実装も出す。依存と std では、hidden な型の実装と hidden な impl も除く。v0.20)。trait の情報として `signature`・`items` (メソッド・関連型・関連定数ごとの `provided`)・`dyn_compatible`・`auto`・`unsafe`・`has_negative_impls`・`sealed` (R-TI-5) を出す
  - `for_` が `ResolvedPath` 以外 (`&T`、`[T]`、タプル、ジェネリクスの `T`) の impl も扱う
  - 同名の型が複数ある場合は正準パスで区別する
- R-TI-4 impl と実装者を引くための逆引き索引を持つ
  - **パスを持たない型 (プリミティブ、スライス、配列、`&T`、タプル、`dyn Trait`) も対象にする** (ferritin の索引はこれらを落としている)
  - 並びは決定的にする
  - 索引の構成 (案: 型ごとの固有 impl・trait impl、trait ごとの実装者 (negative impl を含む)、子 item → 親 item (trait の item と union のフィールドを含む)) は、M4 で依存グラフ全体の索引 (R-TI-3 の `dependency_graph`) と一緒に決める (v0.19)。M1 は 1 つの crate の中だけを扱うので、別の逆引き索引を作らず、rustdoc の `impls`・`implementations` をそのまま使った。あらかじめ作ってキャッシュするのは、検索の索引だけ (R-PF-2)
- R-TI-5 trait を表示するときは、各メソッドに required / provided を付け、次の境界情報を出す: `sealed`、`dyn_compatible`、negative impl の有無。`sealed` は検出できたときだけ根拠付きで出し (例: 公開されていない supertrait を持つ)、検出できないことを「封印されていない」と断定しない (private な型を引数に取るメソッドなど、ほかの封印の仕方は拾えないため)。M1 は、supertrait に同じ crate の公開パスの無い trait があるときだけ、その名前を根拠に出す (検出の方法は増やさなかった。v0.19)。型には `non_exhaustive` と `has_private_fields` を出す。member の JSON では private なフィールドが除かれないので、`has_private_fields` は `pub` でないフィールドがあるときも真にする。private なフィールドに見える印は足さない (eval で、`pub(crate)` と出たフィールドをモデルが外から触れないと正しく答えた。m2-plan.md の F4b。v0.20)

### 5.7 検索

- R-SR-1 対象: item 名、**`#[doc(alias = "...")]` の別名**、ドキュメント本文。kind で絞り込める (上位 N 件を切り出す前に。R-OUT-4)
  - 別名を索引すると、`Vec` を「list」で、整数の `midpoint` を「average」で探すような問いに答えられる (syphar の実装に倣う。v0.18 までの例の `HashMap` の「dict」は、今の nightly の std には無い。M1 で確かめた。v0.19)
  - M1 の索引の対象 (m1-plan.md の C4a。v0.19): 公開パスを持つ item、型のメンバー (固有の impl の item・trait の item・enum の variant。trait の impl の item は trait 側と重なるので除く)、他の crate からの re-export (名前とパスだけ。std の公開 item の多くは core・alloc の re-export なので、名前で引けるように)。std の unstable な item は除く (R-SR-4)。モジュールを丸ごと re-export した先の item は索引に入らない (`reqwest::header` の中の `HeaderMap` は、索引には `reqwest::header` という名前しか入らないので、直接依存の横断で見つからない)。入れるのは M4 で依存グラフ全体の索引と一緒に (v0.19)
  - 引数 `fields` で照合する欄 (`name` / `alias` / `docs` / `path`) を選べるようにする。既定はすべて (ruskel の `search_spec` に倣う。`signature` の欄は下のシグネチャ検索を入れたときに足す。v0.16)
  - **crate に同梱された散文の検索** (M4 で検討。任意): crate の README と `docs/` の散文を検索できるようにする (rust-knowledge-mcp に倣う)
  - **シグネチャを対象にした検索** (M4 で検討。任意): 引数や戻り値の型で探す (例: 「`Result<Vec<u8>>` を返す関数」。ruskel の `search_spec: signature` に倣う)
- R-SR-2 範囲: crate 1 つ / workspace のみ / workspace + 直接依存 / 依存グラフ全体。**既定は workspace + 直接依存**。範囲を選ぶ設定 `search.scope` は M3 以降で検討する (入れるなら、`search.scope = crate` を既定にしたとき、crate の引数が無ければ `invalid_argument` にする。v0.20)。初回は範囲の crate の取得が要り、時間の上限 (ビルドを待つ R-WS-6 の上限と、1 リクエストの R-ARCH-6 の上限) で `partial` (R-SR-5) になりうる。起動時の先取りの設定 (R-CA-6) で避けられることを hints で示す
  - M1 の既定の範囲 (利用者が決めた。m1-plan.md の C4b。v0.19): `crate` を省くと、プロジェクトの member の直接依存のうち、評価する target の通常のビルドか dev を含むビルドに入るもの (R-FT-3 の feature の組で判定) を横断する。build の依存と他の OS 向けは除く。feature の組が取れなければ、直接依存をすべて範囲にして warnings に書く。std は含めず、`crate: "std"` のときに std・alloc・core を横断する。workspace の分と、path・git・別レジストリの依存は下の M2 の形 (M1 は後者を `sources` に `unavailable` (`local_build_required`) として載せた)。プロジェクトが無いときと、ローカル機能が無い構成では、`crate` が要る (`invalid_argument`)
  - M2 の範囲 (m2-plan.md の D7d・E4b。v0.20): workspace のすべての member (lib、lib が無く bin が 1 つならその bin) と、上の直接依存。member、crates.io でない依存、写し (R-VR-2)、docs.rs に JSON の無い crates.io の依存はローカルビルドする。範囲の分のビルドは一度に共有の仕事として起こし、期限まで待つ (鮮度の合うものはすぐ返る)
    - 作れないと決まっているもの (`local_build_required`・`build_unsupported`・`target_unsupported`・`scope_unknown`・`no_json`・`no_target_json`) は `sources` に `unavailable`、作ろうとして失敗したもの (`build_failed`・`toolchain_missing`・`cargo metadata` の失敗など) は `failed` で載せる。ローカルビルドの失敗には、原因を直した後に `get_item` の `refresh` で引くよう hints で案内する
    - 作り直し中の member は前回のビルドで検索し、その source に `stale: true`・`stale_reason: rebuilding` を付ける。照合で index が読めない (期限までに読めないものも) crates.io の依存は、docs.rs で検索して warnings に 1 行書く
    - 外の workspace (R-WS-10) では記録だけを読み、その target の記録が無ければ全体を `no_record`、作成中なら `status: building` で返す。workspace を移した先のパスは、前の場所の記録と結び付けない
  - 範囲の crate を並行に読み込み、`limits.request_timeout_secs` (R-ARCH-6) の 3 分の 2 (既定 20 秒。M1 は 20 秒の固定。v0.20) で待つのをやめる。終わっていない crate は `sources` に `building` (`building_reason: fetching`・`retry_after_secs`) で載せ、応答に `partial: true` と `retry_after_secs` を付ける (R-SR-5)。取得は裏で続けてキャッシュに入れる (v0.19)
- R-SR-3 順位付け (ferritin の調整済みの方式を基本とする。原則は、名前の完全一致を上位にする、最後の語を前方一致で展開しない、並びを決定的にする、の 3 つ。下の数値と、段の順・減点する語・被参照数・量子化は初期案で、M1 で仮に決め (下の「M1 の初期値」。v0.19)、M5 の eval (R-QA-6) で調整する。v0.16):
  - BM25 (k1 = 1.2、b = 0)、Lucene 型の IDF (負にならない)、検索対象の crate 全体での統計
  - 重み: item 名 20、ドキュメントの第 1 段落 3、残り 1。祖先のパス名を距離に応じて 3 / 2 / 1 で加える (`hashmap insert` で `HashMap::insert` が引ける)。コードブロックは索引しない
  - トークン化: CamelCase (頭字語を含む)・snake_case・kebab-case・`::` で分割し、複合語そのものも残す
  - **最後の語を前方一致で展開しない**。エージェントは語を最後まで打つので、展開すると頼んでいない item が混ざる (ferritin はエージェント向けの検索で展開を切っている: `QueryCompletion::Complete`)
  - **名前の完全一致を上位**にする段を設ける (散文の多い説明ページが本物の型より上に来るのを防ぐ)。段の順: パスの一致 (クエリが `sync::Mutex` のようなパスで、item の公開パスの末尾と `::` の区切りごとに一致する) > item 名の完全一致 (クエリ全体が `Mutex` と一致する) > 最後の語の一致 (複数語のクエリ `async mutex` の最後の語が item 名と一致する) > 別名 (`doc(alias)`) の一致 > 部分一致
  - 正準パスを優先し、`__`・`_private`・`internal` を含むパスを減点する
  - 被参照数 (intra-doc link の入次数) を権威として掛ける
  - 同点はスコアを量子化したうえで crate 名・パスで並べ、決定的にする
  - M1 の初期値 (m1-plan.md の C4a・C4b。v0.19):
    - トークン: 語を丸ごと残し、`_` と CamelCase の切れ目 (頭字語を含む。`HTTPServer` は `http`・`server`) でも分ける。1 文字の部分は捨てる
    - 名前の完全一致は大文字小文字を除いて比べ、語をつないだ名前 (`unwrap or default` → `unwrap_or_default`、`hash map` → `HashMap`) も含める。同じ段の中では、大文字小文字まで一致するものが先
    - 重みは名前 20・別名 10・第 1 段落 3・残り 1・祖先のパス名 3/2/1。被参照数 n で `1 + 0.1 × ln(1 + n)` を掛け、減点する語を含むパスは半分にする。並びは (段、スコアを 1000 倍して丸めた値、パス)
    - 複数の crate を横断するときは、BM25 の統計 (item の総数と各トークンの文書頻度) を範囲のすべての crate で合わせ、結果を 1 つの並びにする
- R-SR-4 結果: item ごとに重複を除き、正準パスで示す (他の公開パスは `also_reexported_at`)。`#[doc(hidden)]`・private module・unstable を通るパスは既定で除く。ただし自分の workspace の crate を検索するときだけ、既定で private な item も含める (crate 内でのパスと可視性で示す。R-PR-2。v0.18)。数値の rustdoc Id は出さない
  - M2 の形 (m2-plan.md の D6b。v0.20): member は private と hidden の item も含め、項目には `pub` でないときだけ `visibility` を、hidden なら `hidden` を付ける。依存と std は、非公開の item と hidden な item (hidden な再公開を含む) を、結果からも綴りの候補 (`did_you_mean`) からも除く。BM25 の統計 (R-SR-3) は除いた item も数える (std・core・alloc で上位の順位がほぼ変わらないことを確かめた)
  - 既定の件数は固定値で切らず、**スコアの落差で自動的に打ち切る** (ferritin の `rustdoc-mcp/src/tools/search.rs:103-116` に倣う。初期値は、最上位の 5% 未満、直前の半分未満、すでに返した結果のスコアの累積が全ヒットのスコアの合計の 30% を超えた、のいずれかで切る。少なくとも 1 件は返す)。`limit` を指定したらそれに従う。打ち切った場合も `page` (R-OUT-3) で続きを取れるようにする
    - M1 の形 (v0.19): `limit` を省くと、最初のページは部分一致より上の段の全部と、部分一致を上の条件で切ったところまで (少なくとも 1 件、多くて 50 件)。続きは 20 件ずつ (`limit` の最大は 200)。直接依存を横断すると、完全一致の無いクエリでは部分一致が似た点数で並び、最初のページが 50 件の上限に張り付きやすい (打ち切りの初期値は eval で調整する)
- R-SR-5 範囲内にビルド中・失敗・未取得の crate (既定のプロジェクトの外で `build_external_docs` の記録が無いものを含む) がある場合は `partial: true` を付け、欠けた crate ごとの状態と理由は provenance の `sources` に載せる (R-OUT-9)。warnings には、欠けた crate の数と内訳の要約を 1 行だけ書く (同じ情報を 2 回返さず、warnings だけを読むモデルにも欠けを伝えるため。v0.17)。0 件を「存在しない」と誤読させない。作り直し中で前回のビルドで検索した crate (R-SR-2) も「まだ来る」側に数え、`partial: true` と `retry_after_secs` を付ける (v0.20)
  - 0 件のときは、item 名との編集距離による `did_you_mean` を、R-PR-7 と同じ類似度の下限付きで返す (綴りの誤りを許す。possdog の fuzzy 検索と snowmead の Tantivy の FuzzyTermQuery の利点を、0 件のときに限って取り込む)。M1 の下限は、編集距離 (文字で数える) が 3 以下で、長いほうの 70% 以上が一致すること。距離・名前の順に最大 5 件を返す (v0.19)
  - M1 の warnings の 1 行は「N 個中 M 個を検索した。取得中 X、使えない Y」の形で、warnings の先頭に置く (v0.19)
- R-SR-6 crate そのものを探す検索は別ツール (`search_crates`) に分ける。説明文とキーワードには語幹処理をかけ、語ごとの得点は名前・説明・キーワードの**最大値**をとる (合計すると有名 crate が上位から落ちる)。0 件のときは「語を減らす・言い換える」案内を返す
  - 選定の材料は、検索の API の応答 (DL 数・説明・リポジトリ。1 回で返る) と、結果ごとの sparse index (最新安定版・最終リリース日・MSRV) から作り、結果ごとに API を呼ばない。被依存数は `search_crates` では返さず、`crate_metadata` で返す (crate ごとに API が要り、R-NW-1 の間隔で結果の件数の秒数だけ待たせるため)

### 5.8 ソース表示

- R-SC-1: item の span から該当範囲を返す。前後に何行付けるかは指定できる
- R-SC-2: ソースの取得元
  - 依存 crate: プロジェクトの依存のソースは、常に `cargo metadata` の `manifest_path` のディレクトリから読む (`$CARGO_HOME/registry/src` のそのパッケージの source id のディレクトリ、git の依存の `$CARGO_HOME/git/checkouts`、ソースの置き換え (vendor など) の場所。別のレジストリの同名同版を使わない。vendor もそのまま当たる。R-VR-2。v0.18)。`cargo metadata` を使えないとき (R-VR-2 の Cargo.lock の直読み) と依存でない crate は、`$CARGO_HOME/registry/src` のそのパッケージの source id のディレクトリに既にあればそれを使い、無ければ `.crate` アーカイブ (static.crates.io) を取得・展開してキャッシュする
  - std: std の JSON と同じ toolchain (同じ commit-hash。R-STD-6 で取ったものなら同じ dist の日付) の `rust-src` から読み、span の `/rustc/<hash>/library/…` をそこへ写し替える。版の合う `rust-src` が手元に無ければ、既定で、JSON と同じ日付の dist から `rust-src` を取り (manifest の `[pkg.rust-src.target."*"]` (2026-10-14 に確認) の sha256 で検証し、下の展開の上限を付ける)、キャッシュに置く (v0.16 の「任意」を v0.18 で既定にした)。それでも取れなければ導入コマンド付きの `toolchain_missing` を返し、別の版のソースを返さない (行がずれて別の箇所を読むため)
  - workspace: そのままのファイル
  - docs.rs の span のパス: 自分の item は package root からの相対パス (`src/sync/mutex.rs`。M0 で確かめた) なので、その crate のソースのディレクトリに付ける。依存から inline された item などでは、ビルドホストの絶対パス (`/opt/rustwide/cargo-home/registry/src/<index>/<name>-<ver>/…`) が入る (tokio 1.53.3 の JSON に 356 件あることを 2026-10-19 に確かめた。v0.18)。これは crate 名と版に写し替える。`<name>-<ver>` の分解は一意でない (名前にも `-` と数字が入る) ので、Cargo.lock の名前と版で照合する (v0.18)
  - static.crates.io から取得した `.crate` は、sparse index の `cksum` (sha256) と照合し、合わなければ破棄して `network_error` (`retryable: true`) を返す
    - M1 の形 (R-FT-1 のために Cargo.toml だけを読む。m1-plan.md の C1d-2。v0.19): URL は `crates/<name>/<name>-<version>.crate` (build metadata の `+` はそのままでも `%2B` でも通り、無いものは 403)。照合した `.crate` を、キャッシュの `static.crates.io/<鍵>/<版>/` に内容ダイジェストと一緒に置く。展開はせず、`<name>-<version>/Cargo.toml` のエントリだけを読む。上限の初期値は `.crate` 128 MiB、Cargo.toml を探すまでに展開する量 256 MiB、Cargo.toml 4 MiB。版を index なしで決めたとき (R-VR-6 の近道) は、`.crate` がキャッシュに無ければ index を引いて `cksum` を得る
    - 手元の cargo のキャッシュ (m2-plan.md の B6。v0.20): `.crate` が自分のキャッシュに無ければ、static.crates.io に要求する前に `$CARGO_HOME/registry/cache` の crates.io のレジストリから同じ名前と版の `.crate` を探し、sparse index の `cksum` と合えば使って自分のキャッシュに入れる (オフラインでも使える)。static.crates.io からの取得はこのサーバーの User-Agent (R-NW-1) で送るので、crates.io のダウンロード数に数えられない (crates.io は 2025-10-29 から cargo の User-Agent の要求だけを数える)
  - アーカイブ (`.crate` の tar.gz、R-STD-6 の tar.xz、R-AU-3 の OSV の zip) の展開では、絶対パスや `..` を含むエントリ、symlink・hardlink・デバイスのエントリを拒否し、展開先をキャッシュの下のそのアーカイブ用のディレクトリに限る。展開後の総サイズ (`limits.extract_max_size`。R-NW-3 と共通) とファイル数 (初期値は 10 万。v0.16) に上限を設け、超えたら部分的な展開を消す。`.crate` と std のアーカイブは `docs_unavailable` (`reason: too_large`) を返し、OSV の zip は前回のミラーを使って `audit_dependencies` を続け、warnings に書く (v0.15)
  - `stale: true` の workspace の JSON の span でソースを読むときは、行がずれうる旨を warnings に出す
- R-SC-3: macro が生成した item は span が無いか、macro の呼び出し位置を指す。その旨を明示する
- R-SC-4: **読めるのは既知のソースルートの下の、パッケージの中身だけ**。span のファイル名が絶対パスや `..` を含む場合も、正規化してソースルートと照合してから読む。manifest 由来のパス (`readme`、`[[example]].path`、`build`) も同様に検証する。manifest 由来のパスと span は、その crate 自身のソースルートとだけ照合する (第三者の crate の `readme = "../../<別のプロジェクト>/.env"` のように、他のルートの下を指させない)
  - **パッケージの中身** (v0.14): workspace と path 依存では、ターゲット (build script を含む) のソースのディレクトリの `.rs`、前回のビルドの dep-info に載った入力ファイル (`include_str!` で読むものなど。R-WS-4)、`Cargo.toml`、manifest が指す readme と example だけ。registry と git の依存では、`.crate` (git なら checkout) に入っているファイルだけ。std は `rust-src` の中だけ。ドットファイル (`.env` など) は除き、`readme` は文書の拡張子 (`.md`・`.markdown`・`.txt`・`.rst`・`.adoc`) のものだけを受ける (workspace から継承した `readme` と、拡張子の無い `README` の扱いは、使う場所のあるソースと README のツールと一緒に M3 で決める。v0.18。v0.20 で M2 から M3 に送った)。Claude Code の Read の deny やサンドボックスの読み取りの制限を、このサーバー経由ですり抜けさせないため (rust-analyzer や LSP のツールはファイルの中身をエージェントに返さないので、このサーバーにしか無い経路になる)
  - symlink は、行き先をたどった実際のパスで判定する。実際のパスが既知のソースルート (問い合わせの対象のプロジェクトの workspace のルートと path 依存のルート、`$CARGO_HOME/registry/src` のそのパッケージの source id のディレクトリの `<name>-<ver>` と `$CARGO_HOME/git/checkouts` の対象の crate、依存の `manifest_path` のディレクトリ (vendor を含む。R-SC-2。v0.18)、展開した `.crate`、`rust-src`) の中なら読み (workspace の中の正当な symlink は読める)、外を指していれば読まずに理由を返す (第三者のリポジトリ・git の依存・crate に入っている、`~/.ssh/id_rsa` などを指す symlink で読み出させない)
  - 開くのは判定した実際のパスにする。判定と開く間に symlink を差し替える攻撃への対策 (`openat2` の `RESOLVE_BENEATH` など) は設けない (差し替えには手元で動くプロセスが要り、それができるエージェントはファイルを直接読めるので。v0.14)
  - 展開した `.crate` の中 (R-SC-2) とキャッシュの中 (R-CA-4) の symlink は拒否する (サーバーが作る場所で、symlink が入る正当な理由が無い)
- R-SC-5: 長いソースは既定で切り詰め、`truncated` を返す (目安 2 万文字)
- R-SC-6 **ソースの中を検索する (`search_source`。M4、任意)**: 依存 crate (`.crate` を展開したもの)、std (`rust-src`)、workspace のソースを、正規表現か固定文字列で検索する
  - 用途: エラーメッセージの出どころ、内部の feature gate、ドキュメントに書かれていない挙動を追う (syphar の未マージのブランチにある `search_source` に倣う)
  - 検索できるのは R-SC-4 のパッケージの中身だけ。symlink は R-SC-4 の規則でたどる。ファイルの絞り込み、件数などの上限とページング、symlink のループへの備えは M4 で決める (v0.16)

### 5.9 crate 単位の情報

| 機能 | 取得元 | 要件 |
| --- | --- | --- |
| features | `.crate` 内の `Cargo.toml` | 各 feature が有効にする feature と依存 (`dep:` 構文、`pkg?/feat`、optional 依存が暗黙に作る feature。`[target.*]` 配下の optional 依存も含める)、default、プロジェクトで有効か、その feature で有効になる item (R-FT-2)。各 feature について、**推移的に有効になる feature の閉包**と、逆向きの **`enabled_by`** (その feature を有効にする feature の一覧) も返す。「X を有効にするには何を指定すればよいか」に 1 回で答えるため (syphar の未マージのブランチに倣う)。`.crate` がまだ無いときは、sparse index の `features` / `features2` から先に一覧を返し (解決した版の行を使い、最新版の行で代用しない)、詳細 (どの item が有効になるか) は `.crate` の取得後に返す (mmgeorge に倣う) |
| README | `Cargo.toml` の `readme` が指すファイル。無ければ `README.md` などの既定名、さらに無ければ crates.io | 見出し単位でページングする。切り詰めたら `truncated: true` |
| changelog | `.crate` 内の CHANGELOG / CHANGES / RELEASES / HISTORY / NEWS。無ければ GitHub releases (token は任意) | `from` / `to` でバージョン範囲を指定できる。**既定は新しい順に、応答の上限 (R-OUT-3) まで、最大 N リリースを返す** (N は設定項目で既定 10。続きは `page` で示す。件数だけで決めると、リリースノートの長い crate で上限を超えるため。v0.15)。どこからも取れなければ、crates.io のバージョン履歴 (feature・MSRV・ライセンスの変化、yanked) と `api_diff` を案内する |
| examples | `.crate` 内の `examples/` (manifest に列挙されていなくても見つける)、ドキュメント中のコードブロック (item ごと) | item を指定すると、その item を参照している例を優先する。**既定ではファイルの絶対パスを返し**、クライアントが自分のファイル読み取りツールで開けるようにする。中身は指定時だけ返す。manifest の `[[example]]` に `required-features` があれば添える (その例を動かすのに要る feature) |
| metadata | sparse index、`.crate` 内の `Cargo.toml`、crates.io の API (順は R-NW-1) | MSRV、ライセンス、リポジトリ、**宣言された**依存 (normal / dev / build、`[target.*]` を含む)、docs.rs のビルド対象 target。crate 選定の材料とバージョンごとの履歴 (初期案: 最新安定版と最新版、最終リリース日、総 DL 数と直近 90 日の DL 数、被依存数、保守状況の区分 ≤30 / ≤90 / ≤365 日。版ごとの feature・MSRV・ライセンスの変化、yanked。項目と区分は M3 で決める。v0.16)。任意: 要求されたときは、主な被依存 crate の一覧を DL 数の順に、上限と `page` 付きで返す (mmgeorge の `crate_dependents_list` に倣う。crate ごとに API が要る。v0.16)。sparse index の `rust_version` の無い版は MSRV を「宣言なし」とし、推定しない (serde では 316 版のうち 100 版にだけある。2026-10-19 に確認)。`pubtime` の無い版は最終リリース日を不明とし、API の `updated_at` で代えない (公開日時ではないため)。項目ごとの取得元は M3 で決める (v0.16) |
| **API 差分** | 2 バージョン分の JSON | §5.12 |
| **脆弱性情報** | RustSec / OSV のオフラインミラー | §5.13 |

- R-CI-1: ツール名は範囲を正確に表す。`crate_metadata` は**宣言された**情報 (manifest)、`list_crates` は**解決された**情報 (プロジェクトのグラフ) と説明文で書き分ける。名前が中身を誇張しない (「dependency_tree」と名付けて 1 段の一覧を返し、後で改名した先例がある)

### 5.10 workspace 対応 (ローカルビルド)

- R-WS-1 生成コマンドの基本形 (M0 で確かめた。toolchain の選び方は R-WS-8。v0.17):

  ```text
  [RUSTC_BOOTSTRAP=<crate>[,<crate>…]] cargo [+<toolchain>] rustdoc --locked [--offline] -j <jobs> -p <package id> (--lib | --bin <bin>) [--target <target>] \
      [--manifest-path <Cargo.toml>] [--no-default-features --features <組>] \
      --message-format=json -- -Z unstable-options --output-format json [--cfg docsrs] [--cfg <名前>…] [--document-private-items] [--document-hidden-items]
  ```

  - `+<toolchain>` は、サーバーが自分で選んだ toolchain (nightly など。R-WS-8) のときだけ付ける。プロジェクトの toolchain では付けず、rustup にカレントディレクトリ (R-WS-2) から決めさせる。rustup の無い環境では付けない (v0.17)。`+<toolchain>` を付けるときは `RUSTUP_AUTO_INSTALL=0` も付ける (v0.20)。子の cargo には `RUSTUP_TOOLCHAIN` を渡さず (サーバーが toolchain を選んだときは `+<toolchain>` で指す)、外したことをログに出す (v0.18)
    - M2 の実測 (rustup 1.29.1、2026-10-21。m2-plan.md の D1。v0.20): 環境の `RUSTUP_TOOLCHAIN` は `rust-toolchain.toml` に勝ち、`+<toolchain>` はその両方に勝つ。`rustup run` は子に `RUSTUP_TOOLCHAIN` を渡す
    - そこで `RUSTUP_TOOLCHAIN` は、プロジェクトで走らせる子すべて (ビルドの cargo、`cargo metadata`・`cargo tree`、プロジェクトで走らせる `rustc -vV` と形式の試し (R-WS-8)) から外し、環境にあれば起動時に 1 回ログに書く。中立のディレクトリで走らせる host の `rustc -vV` (R-TG-1) はそのまま走らせる (v0.20)
    - cargo には `CARGO_TERM_COLOR=never` を渡す (m2-plan.md の B2。v0.20)
  - `-j` を付けて、ビルドの並列度を抑える。設定 `build.jobs` で変えられる (v0.18)。既定は CPU の半分で、CPU が 2 つ以上なら 2 以上 (1 つなら 1)。F5 で測って決めた: 3 CPU の macOS の CI の runner で、このリポジトリの冷えたビルドは `-j` 1 で 181 秒、2 で 128 秒かかり、手元の 12 CPU では半分から全部にしても 11% しか速くならない (m2-plan.md の F5。v0.20)
  - `--manifest-path` は、member ではその member の Cargo.toml (resolver 1 でルートにパッケージがある workspace では、指さないと `--features` がルートのパッケージに効く)、依存では workspace のルートの Cargo.toml (依存の Cargo.toml を指すと、cargo がそれを根にして Cargo.lock を作り直す)。カレントディレクトリは R-WS-2 のまま。`--no-default-features --features` は member にだけ R-FT-3 の組で渡し、依存には渡さない (`-p` で指した依存はプロジェクトの通常のビルドの組で文書化されることを確かめた)。`--cfg <名前>` は下の `--cfg docsrs` の項で決める (m2-plan.md の D3d・D7a。v0.20)
  - `--target` は、target (R-TG-1 で決めた値か、照会・外のツールで指した値) が host と違うときと、プロジェクトに `build.target` があるときに付ける。target が host で `build.target` も無いときは付けない (Q-30。M0 で決定。v0.17)。「`build.target` がある」は、cargo が実際に読む設定 (カレントディレクトリから祖先と `$CARGO_HOME` のすべての `config.toml`、環境変数 `CARGO_BUILD_TARGET`。合成の規則は R-WS-2。v0.18) で判断する。`build.target` の `"host-tuple"` (cargo 1.91 から) は host とする (R-TG-1。v0.20)。host は、ビルドに使う toolchain の `rustc -vV` の host とする (R-TG-1 の中立のディレクトリの値とは違いうる)。`--target` を省いたビルドでは、JSON の `target.triple` を記録の target と照合し、違えば warnings を出して実際の値で記録する。JSON を公開する前に、根の item の名前・`crate_version` と、`--target` を渡し proc-macro でないときは `target.triple` を頼んだものと照らし、違えば公開せずに `build_failed` を返す (m2-plan.md の D2b。v0.20)。R-TG-1 で代わりの値を使ったときは省かない。理由: `--target` を付けると、`RUSTFLAGS` と `build.rustflags` が host 向けの build script と proc-macro に効かなくなり、それらが生成する item が利用者の普段のビルド (`--target` なし) と違ってくる (M0 で確かめた)。`RUSTFLAGS` は rustc にだけ効き、文書化する crate の rustdoc にはどちらでも効かない。`build.target` が配列だと、`--target` の無い `cargo rustdoc` は `extra arguments to rustdoc can only be passed to one target` で失敗する (2026-10-16 に実測)
  - 出力は、`--target` を付けたときは `<target dir>/<triple>/doc/`、付けないときは `<target dir>/doc/` に出る (v0.17)。proc-macro の crate は `--target` を付けても host 用に作られ、JSON は `<target dir>/doc/` に、`target.triple` は host で出る (cargo の artifact のメッセージの `filenames` も空。2026-10-16 のレビューで確認)。他の target での文書化では、proc-macro の member は host の文書として扱う (v0.15)。他の target で proc-macro の member を引かれたら、照会のツールはプロジェクトのビルドで作る host の文書を、求めた target でなく host のデータである旨の warning 付きで返す。外のツール (R-WS-10) も proc-macro の member は他の target 用に作らず、照会のツールに任せる (`query_tools_build`。m2-plan.md の E4a。v0.20)
  - `RUSTDOCFLAGS` は使わない。すべての crate に効いて fingerprint を変えるうえ、プロジェクトの `build.rustdocflags` を上書きする
  - `-p` には、取得元まで含む package id の形 (`cargo metadata` の package id) を渡す (同名の複数版や、取得元の違う同名同版で曖昧にならないように。v0.18)。cargo 1.77 より前の形式の package id (`<name> <version> (<source>)`。古い toolchain の `cargo metadata` が出す) は `-p` に渡せないので `<name>@<version>` を渡し、cargo のメッセージとの照合も (名前, 版) で行う。取得元に query (`?branch=` など) のある git の package id は、cargo が glob と読んで失敗するので、`git+` と query を外した `<URL>#<name>@<version>` を渡す (m2-plan.md の D2c・D7a。v0.20)
  - `--lib` / `--bin` は `cargo metadata` のターゲット情報から選ぶ。lib を持たない crate と proc-macro crate も扱う。パスの先頭を member の lib の crate 名、次に bin の名前と照らす。lib と bin が同名なら lib を選び、その bin は引けない。`[lib] name` がパッケージ名と違う member はパッケージ名でも引ける。文書化するのは頼まれた lib か bin だけ (m2-plan.md の D2・D2c。v0.20)
  - 出力ファイルの場所は、パッケージ名ではなく**ターゲット名** (`[lib] name`) から求める
  - **lib と bin の crate 名が同じだと、JSON は同じファイルに出て、後のビルドが上書きする** (パッケージ `my-app` の lib と bin はどちらも `target/doc/my_app.json`)。bin の rustdoc は lib を HTML でも文書化する副作用があり、cargo は `output filename collision` を警告する (2026-10-14 に確認)。ターゲットごとに 1 回ずつビルドし、終わるたびに JSON を作業用ディレクトリからキャッシュへ移す (R-WS-6)
  - **`cargo doc --output-format json`** (unstable。cargo#17025 で 2026-05-27 に追加) は使わない (Q-17。M0 で決定。v0.17)。nightly の経路でも使わない (m2-plan.md の D2。v0.20)。M0 で `cargo rustdoc` と比べた結果 (m0-plan.md の §6.2 の B2):
    - 1 crate の所要時間は同じくらい。2 回目は `cargo doc` が全部 fresh で速く (`cargo rustdoc` は文書化する crate の rustdoc を毎回やり直す)、依存をまとめて文書化できる (146 crate で 38 秒)。cargo#16773 以降、JSON は unit ごとに `build/<pkg>/<hash>/out/<name>.json` に出て、`target/doc/` へ uplift される
    - **同名で版の違う依存は、片方しか文書化されない**: 両方を `-p` で指すと `document output filename collision` で失敗し、`-p` なしだと古い方が警告なしに飛ばされる (2026-10-14 の「別々のファイルに出る」は、2026-10-19 の nightly cargo では再現しなかった)
    - **R-WS-8 の既定 (crate 名を指定した `RUSTC_BOOTSTRAP`) と両立しない**。使うには nightly の cargo か、プロセス全体への `RUSTC_BOOTSTRAP=1` が要る (stable の cargo は、crate 名を指定すると「`-Z` は nightly の Cargo でしか受けない」と拒否する。2026-10-14 に確認)
  - `--cfg docsrs` は、既定で、文書化する crate の最終の rustdoc にだけ渡す (Q-11。docs.rs と同じ。`#[doc(cfg(...))]` が JSON に残り、R-FT-2 の第 1 の情報源が使える)。渡してビルドが壊れたら (`docsrs` のときだけ使う nightly の機能の改名など)、渡さずにやり直し、warnings に書く。設定 `build.cfg_docsrs` (既定 true) で常に渡さないこともできる (設定の名前は v0.20)。M0 で測ったやり直しの頻度は、このリポジトリの依存のうち文書化できた 136 個中 1 個で、原因は `--cfg docsrs` を前提にした crate 自身の feature がプロジェクトの組で有効でないことだった (v0.17)。ただしこれは新しい版だけの標本で、削除された `doc_auto_cfg` を使う版 (`#![cfg_attr(docsrs, feature(doc_auto_cfg))]`) が手元の registry に 82/1,588 あり、`--cfg docsrs` を渡すと E0557 で壊れる (2026-10-19 に確認。v0.18)。M2 で決めた形 (m2-plan.md の D3b・D3e・D4d。v0.20):
    - 外してやり直すのは、compiler の `error` が文書化する crate 自身のもの (package id で照らす) のときだけ。`--cfg docsrs` は最後の rustdoc にしか渡さないので、壊れるのはそこだけ。依存のエラーやネットワークの失敗では外さない
    - 外して通ったことは、案だった別の記録 (版と toolchain を鍵にしたもの) を作らず、置き場の前回の成功の記録で覚える。toolchain・`rustdoc-args` の cfg・docs.rs の組・サーバーの版が同じ間は最初から外してビルドし、それらが変わるか `refresh` で試し直す (R-FT-3 と同じ。内容の鍵で覚えると、編集のたびに試し直して rustdoc 1 回分余計にかかるため)
    - member の `[package.metadata.docs.rs]` の `rustdoc-args` からは、`--cfg <名前>` と `--cfg=<名前>` だけを、名前が cfg の文法に合うときに最後の rustdoc に渡す (値付きの `--cfg k="v"`、`docsrs`、`rustc-args` の分は除く)。`--cfg docsrs` と一緒に付け外しする。依存の `[package.metadata.docs.rs]` は読まない
  - cargo・rustdoc・rustc の実行ファイルは `rustup run` 経由のほか、設定で絶対パスを指定できるようにする (Nix・distro の cargo。rustc は `rustc -vV` に使う。R-TG-1・R-WS-8)。rustc と rustdoc のパスは子の cargo に `RUSTC`・`RUSTDOC` として渡す。パスを指定したときは toolchain を切り替えず (R-WS-8 の nightly への切り替えをしない)、出る形式が対応範囲の外なら `unsupported_format` を返し、rustup のコマンドを案内しない。一部だけ指定されたら、残りは同じディレクトリから探す (rustc と rustdoc は同じ toolchain でないと、依存のメタデータが読めず `E0786` で壊れる。cargo は別の toolchain でもよい。M0 で確かめた)。見つからなければ、起動時にログで知らせ (SessionStart の hook でも知らせるかは R-DIST-5 で M5 に決める)、照会ではエラーにする。rustup が無くパスも指定しない環境 (Nix など) では、PATH の cargo・rustc・rustdoc を使い、toolchain を切り替えない (Q-30。M0 で決定。v0.17)。M2 で決めた形 (Q-31。m2-plan.md の D1・D1a・D2c。v0.20):
    - 設定は `build.cargo_path`・`build.rustc_path`・`build.rustdoc_path` (絶対パス)。一部だけ設定されたら、rustc は rustdoc の隣、次に cargo の隣で、rustdoc と cargo は rustc の隣で探す。指定された名前の版の接尾辞をまねて探し (`rustc-1.80` なら `rustdoc-1.80`)、無ければ接尾辞なしで探す。symlink はたどらず、指定されたパスのディレクトリで探す
    - rustc と rustdoc の `-vV` の commit-hash が違えば、ビルドせずに両方の版を書いたエラーにする
    - 事前の確かめ (R-TG-1 の sysroot、R-WS-8 の形式の試し) にも、cargo と同じ順で決まる rustc・rustdoc を使う: サーバーの設定、環境変数 `RUSTC`・`RUSTDOC`、`CARGO_BUILD_RUSTC`・`CARGO_BUILD_RUSTDOC`、プロジェクトの cargo の設定の `build.rustc`・`build.rustdoc`、toolchain のもの。`build.rustc-wrapper` は見ない
    - nightly へ切り替える (R-WS-8) のは、rustc と rustdoc がどちらも rustup の proxy (PATH の素の `rustc`・`rustdoc`。設定・環境変数・プロジェクトの設定が別のものを指していない) で rustup があるときだけ (rustc だけを別のものにしたまま切り替えると toolchain が混ざるため)
- R-WS-2: **専用の `CARGO_TARGET_DIR`** をキャッシュの下に workspace ごとに作り、**永続化する** (依存のコンパイル結果を再利用するため)。ユーザーの `target/` は使わない (cargo・rust-analyzer とのロック競合、別の toolchain の生成物の混入)。中間生成物の場所 (`build.build-dir`) も `CARGO_BUILD_BUILD_DIR` で同じ専用の場所に向ける (プロジェクトの `.cargo/config.toml` の `build-dir` は `CARGO_TARGET_DIR` では上書きされず、プロジェクトの中に書かれる。2026-10-15 に実測)。ビルドごとの使い捨て dir も使わない (毎回コールドビルドになる)
  - 使えない target dir (止めなかった子孫がいるもの。R-WS-6) は使わず、別の名前で作り直す。古い方は GC で消す (v0.16)。M2 で決めた名前と伝え方 (Q-29。m2-plan.md の D2・E4a・E4c。v0.20):
    - `<キャッシュ>/t/<workspace のルートの正規化したパスの SHA-256 の先頭 12 桁>/` の下に `target/` (`CARGO_TARGET_DIR`) と `build/` (`CARGO_BUILD_BUILD_DIR`) を置く。この既定の dir の名前には toolchain と target を入れない (cargo が中で分ける。外のツールが既定のプロジェクトを他の target で作る dir は下のとおり (workspace, target) ごとに分ける。v0.20)
    - 起動時の片付け (R-WS-6) が止めなかった子孫の記録を残すとき、その target dir に `unusable` の印を置く。ビルドは `<12 桁>`・`<12 桁>.1`・`<12 桁>.2`… のうち印の無い最初のものを使い、ほかのプロセスも印を見て知る。印の付いた dir は、子孫の記録が消えた後に GC が消す
    - 外のツール (R-WS-10) が既定のプロジェクトを他の target で作るときは、(workspace, target) ごとの target dir を使う (他の target の長い初回のビルドの間、照会のツールのビルドをロックで待たせないため)。プロジェクトの target で member を docs.rs の組で作るときも、target dir を照会のツールのものと分ける。仮のプロジェクト (R-WS-5) は、すべてで 1 つの target dir を共有する
  - **子プロセスの cargo・rustc を起動するディレクトリ**: cargo と rustup は、`.cargo/config(.toml)` と `rust-toolchain(.toml)` をカレントディレクトリから祖先へ探し、`--manifest-path` はこの探索に効かない (2026-10-15 に実測)。そこで次のように決める
    - プロジェクトの文脈で起動するときは、カレントディレクトリを workspace のルートにする (利用者が手元で `cargo build` するのと同じ設定が効く)。workspace のルートより上の祖先の設定も効くので、鮮度の鍵 (R-VR-2・R-WS-4) の `.cargo/config.toml` には、cargo が読む全階層の設定 (`$CARGO_HOME` のものを含む) を入れる
    - **cargo の設定の合成** (v0.18): R-TG-1 の「配列なら先頭を既定」と、鮮度の鍵 (R-WS-4)・`cargo metadata` のキャッシュの鍵 (R-VR-2) の「`.cargo/config.toml` の内容」は、cargo の設定の合成の規則に依る。実測 (cargo 1.99.1、2026-10-19): 配列の `build.target` は全階層で連結され、祖先や `$CARGO_HOME/config.toml` の値が先頭に来る。`CARGO_BUILD_TARGET` は配列の後ろに足される。`include` が stable で効く。拡張子の無い `.cargo/config` も読まれる。member のディレクトリの `.cargo/config.toml` は、ルートから起動すると効かない。M2 で決めた読み方 (Q-31。cargo 自身に読ませる手段は stable に無いので、nightly の cargo 1.101 の `cargo -Zunstable-options config get` を物差しに確かめた。m2-plan.md の B3a。v0.20):
      - 読むファイル: 起動するディレクトリとその上のすべての `.cargo/config` か `.cargo/config.toml` (両方あれば拡張子の無いほう。cargo は警告を出す) と、`$CARGO_HOME/config(.toml)`。`$CARGO_HOME` が上のどれかの `.cargo` と同じなら 1 回だけ読む。member のディレクトリの設定は、ルートで起動するので読まない
      - 優先度 (低い順): `$CARGO_HOME`、上のディレクトリを上から順に、環境変数 `CARGO_BUILD_TARGET`。`include` が指すファイルは、指したファイルのすぐ下 (入れ子はさらに下)
      - 合成: 文字列は優先度の高いものが勝ち、配列は低い順につなぐ。同じキーに文字列と配列が混ざると cargo は設定を読めずに失敗する。`CARGO_BUILD_TARGET` は文字列を置き換え、配列なら末尾に足す
      - `include` は、パスか `{ path, optional }` の表の配列で、パスは指したファイルからの相対。文字列で書くと失敗し (stable 1.99.1 でも)、`optional` でないファイルが無くても失敗する
    - 仮のプロジェクト (R-WS-5)、引数 `git` の取得、ホストの target を調べる `rustc -vV` (R-TG-1) では、カレントディレクトリをキャッシュの下のそのディレクトリにする (祖先の設定の有無は確かめない。共有の場所の下にキャッシュを置かないよう README で案内する。R-SEC-2。v0.14)
  - 専用の target dir でビルドディレクトリのロックは避けられるが、`~/.cargo` のパッケージキャッシュのロックは rust-analyzer の `cargo metadata` / `cargo check` と共有する。stderr に `Blocking waiting for file lock` が出たら、`building_reason: waiting_for_lock` を付けて返す (応答の形は R-OUT-9。stderr の文字列照合の例外で、一覧は R-WS-7)。照合は `Blocking waiting for file lock` までにする (後ろはロックの対象で `on package cache`・`on build directory` などに変わる)。ロック待ちの行の後にほかの行が出たら、ロックが取れて動いているとみなす (m2-plan.md の B2。v0.20)。ロックと CPU を rust-analyzer や利用者の cargo と取り合うことと、その対処を README に書く (並列度は R-WS-1 の `-j`。v0.18)
  - `cargo metadata` を毎回起動しないことで (R-VR-2)、ロックを取る回数を減らす。nightly の `-Z fine-grain-locking` は、nightly の cargo を使う経路でだけ使える (既定の stable では cargo の `-Z` を受けない。R-WS-8)。nightly の経路でも使わない (Q-17 のとおり `cargo rustdoc` を使う。m2-plan.md の D2。v0.20)
  - Windows の MAX_PATH (260 文字) を超えないよう、target dir のパスは短く保つ (workspace の識別子はハッシュの短い接頭辞にする)。git worktree や eval の一時ディレクトリごとに target dir が増えすぎないよう、GC (R-CA-5) の対象として最終使用日時で管理する
- R-WS-3: `--document-private-items` と `--document-hidden-items` は workspace member のビルドにだけ付ける (`--document-hidden-items` は v0.20)。member でない依存は、ローカルでビルドしても公開の item だけを出す (R-SH-5 のまま。依存の private を出すと、モデルが使えない item を使い、検索と一覧が内部の item で埋まり、取得元で同じ crate の見え方が変わるため。m2-plan.md の D7a。v0.20)。応答には可視性を必ず付ける (表示の規則は R-SH-5)。可視性は R-PR-2 の書き方 (`pub`・`pub(crate)`・`pub(in crate::…)`・`private`) で、member の `get_item` は常に付け、検索と一覧の項目には `pub` でないときだけ付ける (m2-plan.md の D6。v0.20)
- R-WS-4 鮮度: **JSON が存在すること・バージョンが同じことを鮮度の根拠にしない** (先行 5 実装がすべてここで誤っている)。内容ダイジェストの鍵は、workspace member だけでなくローカルビルドするすべてのパッケージに適用する (workspace 外の path 依存、git 依存、docs.rs に JSON が無い registry 依存、R-WS-5 の仮のプロジェクト)。path 依存はそのパッケージ自身のソース・dep-info・Cargo.toml も鍵に含める
  - 入力の内容ダイジェストはビルドの前に取り、ビルドの後に dep-info で増えたファイルを足す。ビルドの前より新しい mtime のファイルがあれば、次回は鮮度切れにする (ビルドの間にエージェントが編集した内容を、古い JSON の鍵に入れないため。v0.15)。M2 では、ビルドの前に入力を取った時刻の 3 秒前より後に変わった dep-info のファイルを、digest を持たないものとして記録する形にした (次の判定で作り直す。ビルドの直前に外のファイルを編集すると 1 回余計に作り直す。m2-plan.md の D4b。v0.20)
  - member ごとに次の入力の**内容ダイジェスト**を鍵にする: その member のパッケージのディレクトリ全体 (`cargo metadata` が示す全ターゲットの `src_path` 配下と、build.rs が読む `proto/` などを含む。集め方は下記。v0.20)、`build.rs`、`Cargo.toml`、workspace の `Cargo.lock`、`.cargo/config.toml` (合成の規則は R-WS-2。v0.18)、path 依存 (workspace の外にあるものを含む) のソース、feature の組、rustdoc の引数、`RUSTC_BOOTSTRAP` の値 (のダイジェスト。値で derive が生成した item の span が変わることを M0 で確かめた。R-WS-8。v0.17。member にも member 以外のパッケージにも値の全体を入れる。v0.17 では、全体を入れると無関係な依存を足すだけで全部の記録が鮮度切れになるとして、member 以外にはその部分木に現れる crate 名だけの部分集合を入れるとしたが、依存の文書も R-WS-8 の値を固定した target dir ごとに作るので、値の全体が変われば依存はどのみち作り直され、鍵もそれに合う。m2-plan.md の §7.1 の決定 3。v0.20)、target、**実際に生成に使った toolchain** の `rustc -vV`、子に渡す環境のうちビルドに効く変数 (`RUSTFLAGS`・`RUSTDOCFLAGS`・`CARGO_BUILD_*`・`RUSTC_WRAPPER` など。サーバーの環境は Claude Code の起動時のもので、direnv などでシェルにだけ入る値は含まれない)。feature の組以降は member 以外のパッケージにも適用し、さらに source id、その依存から先の解決結果 (依存先の package id と feature) のダイジェスト、ローカルビルドに効く実効の cargo の設定 (`[source]`・`[patch]`・`build.rustdoc`・`build.rustc`・`build.rustc-wrapper`) のダイジェスト、使った cargo と rustdoc の実行ファイルのパスを入れる (ソースの置き換えや実行ファイルの設定で、別のソースや別の実行ファイルで作った JSON が、他のプロジェクトに配られないように)
  - M2 で決めた鍵の作り方 (m2-plan.md の D4・D4a・D4b・D7a。v0.20):
    - 鍵は計画した入力 (再試行で変わる前の docs.rs の組・通常の組・`--cfg docsrs` の設定・`rustdoc-args` の cfg) で作り、実際に使ったもの (後退した形など) は記録に残す (Q-31)
    - 集めるファイル: `cargo metadata` の鮮度の入力 (R-VR-2。Cargo.lock・path のパッケージのマニフェスト・cargo の設定と `include`・`rust-toolchain`) と、member とそれが通常の依存かビルドの依存としてたどる path パッケージのディレクトリ全体。git の作業ツリーの中は `git ls-files --cached --others --exclude-standard` (追跡中と、無視されていない未追跡) で集め、外はたどって `CACHEDIR.TAG` のあるディレクトリを除く。どちらも `target`・`.claude` はパッケージの直下だけ、`.git` はどの深さでも、入れ子のパッケージは除く (全部たどると、このリポジトリでは無視された build dir まで読んで 30,109 ファイル・11 GB になった。git の一覧にして 193 ファイル)。5 万ファイルを超えたら鮮度を保たず、毎回ビルドする
    - 各ファイルは (パス・大きさ・mtime・SHA-256) を記録し、大きさか mtime が違うものと、記録の時刻から 3 秒以内の mtime のもの (粒度の粗いファイルシステムのため) だけ読み直して内容を比べる。ビルドの後に初めて見つかったファイルは、Unix では ctime も見る (`mv`・`cp -p` などで古い mtime のまま置き換えたものを拾うため)
    - 計画の項目: package id・マニフェスト・lib か bin・`rustc -vV`・サーバーが選んだ toolchain・`RUSTC_BOOTSTRAP` の値・`--target` (proc-macro では入れない)・`--cfg docsrs` の設定・`rustdoc-args` の cfg・通常の組・docs.rs の組・private と hidden を文書化するか・サーバーの版・環境変数。環境変数は `RUSTFLAGS`・`RUSTDOCFLAGS`・`CARGO_ENCODED_RUSTFLAGS`・`CARGO_ENCODED_RUSTDOCFLAGS`・`RUSTC`・`RUSTDOC`・`RUSTC_WRAPPER`・`RUSTC_WORKSPACE_WRAPPER` と `CARGO_BUILD_*`・`CARGO_TARGET_*`・`CARGO_PROFILE_*`・`CARGO_UNSTABLE_*` で、サーバーが上書きする `CARGO_TARGET_DIR`・`CARGO_BUILD_TARGET_DIR`・`CARGO_BUILD_BUILD_DIR` と、出力に効かない `CARGO_BUILD_JOBS` は除く
    - dep-info (下記) から足すもの: `.d` のファイル、member と path 依存の build script の `rerun-if-changed` のファイルとディレクトリと `rerun-if-env-changed` の変数 (cargo が build script を走らせ直す条件と同じ)、`.d` の `env-dep` のうちサーバーの環境の値と同じもの。`.d` の `env-dep` に出るのは文書化する crate の `env!`・`option_env!` だけで、build script が読む変数は入らない (D4 の決定 8 の前提は誤りだった)。build script が target dir に書いたもの (OUT_DIR) は出力なので除き、前回の dep-info だけが挙げたものは次の鍵から除く。依存 (registry・git) の build script の `rerun-if-env-changed` (`OPENSSL_DIR` など) は鍵に入らない
    - 置き場は crate ごとに 1 つで、鍵と入力の一覧はビルドの記録に残す
  - mtime は一次の絞り込みにだけ使う
  - `src_path` 配下だけでは、`include_str!` / `include_bytes!` で読むファイル (`#![doc = include_str!("../README.md")]` など)、`#[path]` で指したファイル、build.rs が生成するコードの変化を拾えないので、前回のビルドの dep-info (cargo が unit ごとに書く `.d` ファイル) に載った入力ファイルも鍵に含める。ただし `cargo rustdoc` は、文書化する crate 自身の dep-info を書かない (依存の check unit だけが書く。2026-10-14 に確認)。rustdoc に `--emit=dep-info` を渡すと、その crate の dep-info (JSON と同じ場所。`--target` を付けたら `<target dir>/<triple>/doc/<crate>.d`、付けなければ `<target dir>/doc/<crate>.d`。R-WS-1。`include_str!` で読んだファイルも入る) が書かれるが、JSON は出なくなり、`--emit` のほかの値は JSON の出力と併用できない (2026-10-16、stable 1.99 + `RUSTC_BOOTSTRAP` で確認)。そのため dep-info は別の呼び出しで取る: 同じ引数 (`--cfg docsrs` を含む) に `--emit=dep-info` を付けて rustdoc をもう一度実行する (M0 で決定。v0.17)。費用はその crate の rustdoc 1 回分 (M0 で member 0.5 秒、tokio 0.9 秒。依存は fresh のまま)。JSON は書き換えないので、JSON を移した後に実行してよい。M2 では、2 回目の rustdoc を応答の後に裏で走らせ (作り直しの応答を遅らせない。このリポジトリで約 6 秒)、書き先を `--emit=dep-info=<作業用ディレクトリの中のパス>` と指定する (指定しないと、古い nightly が利用者の workspace のルートに `<crate>.d` を書く)。走っている間に来た呼び出しは、dep-info を足す前の鍵で判定する。2 回目の rustdoc はそのときのソースを読むので、判定のときに見た入力と同じかを前後で確かめ、違えば使わずに次の呼び出しで作り直させる。失敗は記録に書かずにプロセスの間覚え、そのビルドを新しいと判定した応答に warning を付ける (m2-plan.md の D4b。v0.20)。その crate の `cargo check` の dep-info は、依存なら作り直さずに使えるが、`#[cfg(docsrs)]` のときだけ読むファイルを取りこぼす (tokio では 250 件のうち 5 件) ので使わない
  - ワークスペースのルートの `src/` だけを見る方式は、仮想 workspace で誤る
  - 再ビルド中は、直近に成功した JSON を `stale: true` と `built_at` 付きで返す (R-WS-6)。失敗したときは R-WS-7 のとおり `build_failed` を返す
- R-WS-5 依存 crate のローカルビルドは、必ずプロジェクトの workspace と Cargo.lock の文脈で行う。`~/.cargo/registry/src` の中で cargo を実行しない (Cargo.lock の生成・依存の再解決・ネットワークアクセスが起きる)。`-p` で cargo が失敗する依存は、照会のツールではローカルビルドせず `docs_unavailable` (`reason: build_unsupported`) とし、docs.rs に JSON があればそれを、ほかの target でだけ使う依存ならその `target` の指定を、それ以外 (docs.rs に JSON の無い crates.io の依存、path・git の依存) は `build_external_docs` を案内する。外のツールは、その依存 (`crate@version`、path・git ならその指定) だけに依存する仮のプロジェクト (下記) で文書化し、feature の組がプロジェクトの組と違いうる旨を provenance と warnings に出す (v0.17)。resolver 2 以上 (edition 2021 以降の既定) では、dev-dependency だけ・build-dependency だけ・ほかの target でだけ使う依存と、proc-macro からしか使われない依存 (syn・quote・proc-macro2 など) を `-p` で名指しすると、cargo の feature resolver が panic する (`did not find features for … NormalOrDev`。stable 1.99・nightly 1.101 の両方で M0 で確かめた。このリポジトリの依存 146 個のうち 10 個が当たった。M0 の実測はどれも `--target <host>` を付けて行い、付けない形 (R-WS-1) でも同じ結果だった。2026-10-20、cargo 1.99.1。m2-plan.md の B7。v0.20)。resolver 1 では起きない。そこで resolver 2 以上の workspace では、member から target 向けの通常の辺でたどれない依存 (proc-macro の下にしか無いものを含む) を `cargo tree` (R-MCP-3) で起動前に判定し、cargo を起動せずに返す。stderr の panic の文言は照合しない (Q-30。M0 で決定。v0.17)。M2 で決めた形 (m2-plan.md の B7・D7a・E2b。v0.20):
  - 名指しできる依存は、dev を含まない解決 (`cargo tree -e normal,build`) で target 用に出る package と proc-macro とする (build の依存の下の proc-macro は名指しでき、build の依存の lib、dev の依存の先の依存、dev の feature でだけ有効になる optional の依存は panic した)。proc-macro の member の下の依存は、cargo は名指しできるが `cargo check --all-targets` は host 用にしか作らないので、安全側に名指しできないとする (多くは docs.rs に JSON のある crates.io の crate)
  - resolver の版は、ルートの Cargo.toml の `resolver` (`[workspace]` か `[package]`)、無ければルートの package の edition (2021 以降なら 2 以上)、仮想 workspace で無ければ 1 として読む
  - resolver 1 の build-dependency (proc-macro でない) は名指しできるが、host でビルドし、頼まれた target と違えば warnings に書く
  - M2 で外のツールが作る名指しできない依存は、crates.io の依存だけ。依存でない crate と同じく、その `crate@version` だけに依存する仮のプロジェクト (下記) で作り、記録の鍵も同じ (crate・版・target)。プロジェクトのビルドが host 向けにだけ作るもの (build script や proc-macro だけが使うもの) は host の target で作る。照会のツールは `build_unsupported` の代わりにその記録を読み (crates.io の写し (R-VR-2) は除く)、プロジェクトのビルドと features が違いうる旨を warnings に書く。path・git・別のレジストリの名指しできない依存は、案内に「(in a later version)」と書き、M3 以降で検討する
  - **プロジェクトの Cargo.lock に無い版や crate** をローカルでビルドする場合は、外のツール (R-WS-10) で、キャッシュの下に**仮のプロジェクト** (対象の `crate = "=x.y.z"`、git なら `crate = { git = "…", rev = "…" }` だけに依存する Cargo.toml) を作り、その中で `cargo rustdoc -p <crate>@<ver>` を実行する。target は、引数 `target` があればその値、無ければホストの target とし、`--target` を付けるかは R-WS-1 の規則で決める (仮のプロジェクトはキャッシュの下で起動するので、プロジェクトの `build.target` は効かない。プロジェクトの組み込み向けの `build.target` で、std の要る crate を作って失敗しないように。v0.17 で書き方を改めた。照会のツールも、依存でない crate を `target` なしで引くときはホストの target で探す。v0.15)。必要な feature は仮のプロジェクトの Cargo.toml の依存の指定に書く (依存への `--features` は拒否されるため。cargo-semver-checks の `generate.rs` の方式)。feature の既定は `[package.metadata.docs.rs]` の指定 (R-FT-1)、無ければ default feature とし、使った組を provenance に載せる。`[package.metadata.docs.rs]` の組でビルドが壊れたら、default の組でやり直して warnings に書く。壊れた組の扱いは M2 で次のように決めた (m2-plan.md の E2b。v0.20): 仮のプロジェクトのディレクトリを組ごとに分け (docs.rs の組と既定の組。それぞれ Cargo.lock を 1 回だけ作る)、やり直すのは `[package.metadata.docs.rs]` が features を求めていて、docs.rs の組がコンパイラのエラー (文書化する crate か依存) で壊れたときだけ (取得・ロック・オフラインの失敗ではやり直さない)。docs.rs の組の失敗の記録 (コンパイラのエラーだったか) が残るので、作り直すときは docs.rs の組を試さずに既定の組で作る (外のツールの呼び直しで外の記録が失敗か壊れているときは、docs.rs の組から試し直す)。provenance の `features_requested` は実際に使った組で、docs.rs の組は warnings に書く。docs.rs の組は crate が宣言する features に絞り、外したものは warnings に書く。外のツールに `features` 引数は v1 では持たせない (v0.15)。ユーザーのプロジェクトの Cargo.lock は変えない。仮のプロジェクト (とその target dir) は、専用 target dir と同じ GC (R-CA-5) の対象にする。仮のプロジェクトの Cargo.toml は TOML のライブラリで組み立て、引数や metadata の文字列を連結しない
    - M2 で決めた置き場と形 (m2-plan.md の M2e・E2a。v0.20): 鍵ごとに `<キャッシュ>/ext/v1/release/<digest>/project` (既定の組は `project-default`) に置いて残す。Cargo.toml の lib は `lib.rs`、依存はリリース 1 つを `=x.y.z`・`default-features = false`・docs.rs の組の features で書き (`default` は `default-features = true` に写す)、空の `[workspace]` で上の workspace に入らないようにする。edition は 2021 (resolver 2 は最新の版を選ぶので docs.rs に近く、古い cargo でも読める)。`RUSTC_BOOTSTRAP` は常に `1` (R-WS-8 の例外)。toolchain は置き場で rustup が選ぶ既定のもので、rustdoc JSON の形式が古ければ R-WS-8 の規則で nightly にする。target dir はすべての仮のプロジェクトで 1 つ (R-WS-2)
- R-WS-6 長時間のビルド (Claude Code の実際の挙動を前提にする):
  - 照会のツールは、既定のプロジェクトのキャッシュミスのとき暗黙にビルドする (既定のプロジェクトの外は R-MCP-3)。**task ID をポーリングさせる専用ツールは作らない**
  - ビルド中は `notifications/progress` を送る (Claude Code では表示され、アイドルタイムアウトがリセットされる)。送るのは、要求に `_meta.progressToken` があるときだけで、その値を付ける (MUST)。`progress` は単調に増やす (MUST。完了した compile unit の数や経過秒数を使い、`total` は分かるときだけ付ける)。`message` には人が読める状況 (例: `building serde 1.0.219 (12/80)`) を入れる。間隔は 1 秒以上 30 秒以内
    - M2 の形 (m2-plan.md の C3b。v0.20): 送るのは `tools/call` の入口の 1 か所で、間隔は 3 秒。`progress` は送るたびに経過秒数を 3 ずつ増やし、`total` は付けない。`message` は呼び出しが待っている仕事とその段 (`fetching serde 1.0.228 (3 s)`。仕事が複数なら件数と最初の 1 つ、仕事を待っていなければ `looking it up`)。試験用の `sleep` は自分で送る (送り手が 2 つになると値が単調でなくなるため)
  - 進捗通知は、その要求に結果 (`status: building` を含む) かエラーを返した時点、またはキャンセルを受けた時点で止める (MUST)。singleflight で共有するビルドの進捗は、待っている要求ごとに、応答を返すまでの間だけ、その要求の token で送る
  - ビルド (と cargo のロック待ち) を待つ上限 (`build.wait_secs`、既定 15 秒。v0.18 で 30 秒から 10 秒に短くし (理由の 1 つは R-MCP-4)、M2 の F5 で測って 15 秒にした。編集の後の作り直しは手元で約 7 秒、CI の macOS・Windows の runner で約 8〜15 秒で、その多くと tokio 級の依存のローカルビルドが 1 回で返る (axum 級は runner によって 15 秒を越え `building` になる)。m2-plan.md の F5。v0.20) を超えたら待つのをやめて返す。上限はビルドを待ち始めたときから数え、呼び出しが裏の仕事を待つ上限 (呼び出しの期限の 3 分の 2。R-ARCH-6) より長ければ、そちらを使う (v0.20)。ビルドの後の索引の作成に入っていれば、呼び出しの期限まで待つ (m2-plan.md の D3a。v0.20)。この上限は 1 リクエストあたりの時間の上限 (`limits.request_timeout_secs`、既定 30 秒。R-ARCH-6) より短く、ビルドを待つ間はこちらが先に効く。ビルド以外の処理 (docs.rs からの取得や索引の作成。下記) は R-ARCH-6 の上限で返す。形は R-OUT-9 に従い、古いデータがあれば (R-WS-4) それに `stale_reason: rebuilding` と `retry_after_secs` を付け、無ければ `status: building` と `retry_after_secs` を返す (v0.17)
  - Claude Code は tasks 拡張に対応しておらず、2 分を超える呼び出しはバックグラウンドに回されるので、ブロックし続けない
  - 同じ対象への同時のビルドは 1 つにまとめる (singleflight)。**プロセスをまたぐファイルロック** (R-CA-4) も取る (複数の Claude Code セッションが同時に動く)
    - M2 で広げた範囲 (m2-plan.md の C2b・C3・D2c・D4a と止まる危険の直し 2。v0.20): ローカルビルドに加えて、docs.rs の crate の取得からアーカイブの作成まで、std の配布物の取得と std・alloc・core のアーカイブの作成、`cargo metadata`、`cargo tree` (feature の組と範囲。(workspace, target) ごと) も、呼び出しの間で共有する 1 つの仕事にした。どれも待つ側が `retry_after_secs` を付けて返したら切り離す (下記)。キャッシュのロックは blocking の作業が終わるまで持つ (待つ側が取り消されても、別の呼び出しが同じ解析を二重に始めない)
    - 期限を過ぎて裏で続いたローカルビルドの結果は、鮮度の鍵 (R-WS-4) を取れなかった成功と、記録できなかった失敗だけを次の 1 回の呼び出しに渡す (鍵のあるものは、次の呼び出しが鮮度の判定と失敗の記録 (R-WS-7) で見つける。鍵を取れない member が、ビルドが上限を超えるたびに作り直して `status: building` から抜けなくなるのを防ぐ。m2-plan.md の D2c・D4a・D5a。v0.20)
  - ビルドはサーバー全体のロックを取らず、他の crate への問い合わせを止めない
  - キャンセルされたら子プロセスを kill する。ただし singleflight で共有しているビルドは、待っている要求がすべてキャンセルされたときだけ kill する。一度 `retry_after_secs` を付けた応答 (`status: building`、`stale_reason: rebuilding`、`partial`。R-OUT-9。v0.17) を返したビルド (裏で続けるもの。R-WS-10・R-ARCH-6) は切り離した状態とし、キャンセルでは止めない。止まるのは 1 回のビルドの最大時間 (下記) とサーバーの終了だけ (v0.15)。キャンセルされた要求には、以後は結果も進捗通知も送らない (MUST NOT)。未知の ID や応答済みの要求へのキャンセルは無視する。M1 (ビルドが無い) は、キャンセルで呼び出しの future を落とし、ツールの task を abort する (取得中の要求も止まる)。blocking のスレッドの解析は途中で止められないので最後まで走らせ、アーカイブを書く (次の呼び出しで使える)。キャンセルした呼び出しはログに `cancelled` で残す。依存の横断の検索で待つ上限を過ぎて応答した後の task は切り離し、裏でキャッシュを満たし続ける (m1-plan.md の D2。v0.19)。HTTP でのキャンセルの扱いは M6 で決める (v0.16)。サーバーから `notifications/cancelled` を送りうるのは `subscriptions/listen` を閉じるときだけ (扱いは R-MCP-16)
  - 将来の任意 (M6 で検討): tasks 拡張を宣言したクライアントには、ローカルビルドを伴う `tools/call` で task を返してよい。プロトコルの機能なので、「ポーリング用のツールを作らない」方針とは矛盾しない。宣言していないクライアントには `status: building` を返す (v0.16)
  - 1 回のビルドの最大時間 (`build.timeout_secs`。既定 30 分で、Claude Code の stdio のアイドルの上限に揃える) を超えたら子プロセスを止め、`build_failed` (理由 `timeout`) を返す。毎回超えるものとして、一時的な失敗にせず、決定的な失敗として `refresh` まで固定する (R-WS-7。v0.18)。上限は cargo の 1 回の実行 (ビルド・`cargo metadata`・`cargo tree`) ごとに数え、サーバーのロックとビルドの枠を待つ時間は数えない (m2-plan.md の D3a。v0.20)
  - 巨大な crate の docs.rs からの取得と索引の作成にも、同じ進捗通知と `status: building` (`building_reason: indexing`) を適用する。`cargo metadata` の初回の依存の取得 (R-VR-2) も同じ (`building_reason: fetching`)。どちらも M2 で入れた (M1 は 1 つの crate の照会に時間の上限を設けていなかった。R-ARCH-6。v0.19。m2-plan.md の C3a・C3c。v0.20)。std の配布物の取得 (`fetching`) と std・alloc・core のアーカイブの作成 (`indexing`) も同じ。`cargo tree` (feature の組と範囲) も期限付きで待ち、その結果を注記にだけ使うツールは、期限までに分からなければ分からない旨の warnings を付けて返す (m2-plan.md の止まる危険の直し 2。v0.20)
  - stdio でもリクエストは並行に処理する。JSON の解析や索引の作成など CPU を占める処理は専用のスレッドで行い、キャッシュ済みの問い合わせを止めない。同じ crate の解析は 1 つにまとめる
  - **サーバーの終了時も子プロセスを確実に止める**。Claude Code は Unix では SIGINT → 100ms 後に SIGTERM → さらに 400ms 後に SIGKILL を送り、Windows では stdin を閉じて 300ms 後にプロセスツリーを終了する
    - 子の cargo は Unix では新しいセッション (`setsid`。プロセスグループも新しくなる)、Windows では Job Object (閉じたときに中のプロセスをすべて止める設定) に入れ、stdin の EOF・SIGINT・SIGTERM を受けたらまとめて止める
    - 子が利用者に尋ねて止まらないようにする: Unix では `setsid` で制御端末を外す (git の資格情報や ssh の host key・パスフレーズの入力が `/dev/tty` を開けずにすぐ失敗する)。`GIT_TERMINAL_PROMPT=0` と `GCM_INTERACTIVE=never` (Windows の Git Credential Manager の画面) を渡し、GUI の askpass も止める (`SSH_ASKPASS`・`GIT_ASKPASS` を外し、`SSH_ASKPASS_REQUIRE=never` を渡す)。制御端末を外しても、デスクトップの環境では git の CLI が `SSH_ASKPASS` のダイアログを出す (M0 で、GitHub の存在しないリポジトリで起きた。GitHub は存在しないリポジトリにも認証を求める。`net.git-fetch-with-cli` の経路では cargo が失敗を `net.retry` の回数だけやり直すので、ダイアログが繰り返し出る)。保存済みの資格情報 (`credential.helper`・ssh-agent) はこれまでどおり使われる。cargo の既定の経路 (libgit2) も、`credential.helper` と ssh-agent だけを試し、人に問い合わせない (cargo 1.99.1 のソースで確かめた。v0.17)。ただし、agent や helper の側が尋ねるもの (1Password・Secretive の SSH agent、macOS のキーチェーンの許可、gnome-keyring) は、これらでは止まらない。このことを README に書く (v0.18)。認証で失敗したら (`fetch_failed` の `auth`。R-WS-7)、利用者の端末で資格情報を通すよう案内する (案内の文面は §8)
    - M1 は、版の解決と feature の組のために起動する `cargo metadata`・`cargo tree` にも、同じ起動の仕方 (workspace のルートで `--locked`、オフラインなら `--offline`、stdin を null、上の環境変数、Unix の `setsid`、30 分の上限) を使う。`PR_SET_PDEATHSIG`・Job Object は M2 (v0.19)。M2 で入れた (m2-plan.md の B1。v0.20)
    - SIGKILL は捕まえられないので、Linux では子に `PR_SET_PDEATHSIG` を設定し、親が先に死んでも子の cargo が残らない (パッケージキャッシュのロックを持ち続けない) ようにする
    - M2 の形 (m2-plan.md の B1 と止まる危険の直し 1・2。v0.20):
      - 子は、プロセスと同じだけ生きる専用のスレッドから起動する (`PR_SET_PDEATHSIG` は子を起動したスレッドの終了で発火し、非同期の実行環境の blocking のスレッドは使われないと終わるため)。Linux では SIGKILL の `PR_SET_PDEATHSIG` を設定し、設定の後の親の PID がサーバーでなければ (設定の前にサーバーが死んでいれば) 起動を失敗にする。Windows の Job Object は閉じたときに中のプロセスを止める設定で自前に作る
      - 止めるのはグループごと (Unix はプロセスグループに SIGKILL、Windows は Job の終了)。取り消し・時間の上限のときだけでなく、正常に終わった後も残った孫を止める
      - 子が終わった後の出力は 2 秒だけ読み、閉じなければグループを止めてもう 2 秒読み、それでも閉じなければ (セッションを抜けた子孫が持つ) 読めた分で進む (build script が起こした常駐のプロセスが出力を持ち続け、ビルドの枠とロックを持ったまま `build.timeout_secs` の間止まるのを防ぐ)
      - stdin の EOF かシグナルを受けてから 10 秒たっても終わらないか、2 回目のシグナルが来たら、処理中の要求を待たずに終わる (stdout を読まないクライアントで終わらなくなるのを防ぐ。残った子孫は次の起動の片付けに任せる)
      - 既知の残り (M3 以降): Windows で、子を Job Object に入れる前のごく短い間に子が起動したプロセスは Job から漏れる。子を止めた状態 (`CREATE_SUSPENDED`) で起動し、Job に入れてから再開する形で塞ぐ (Windows だけの API を手元で確かめられないため、M2 では入れなかった。m2-plan.md の §6.7)
    - それでも残る子孫 (`PR_SET_PDEATHSIG` で止まるのは cargo だけで、cargo が起動した rustc や build script は残りうる。macOS にはこの仕組みが無い) は、次回の起動時に片付ける。PID は再利用され、複数のセッションのサーバーが同時に動くので、確かでなければ止めない (無関係なプロセスを止めないことを優先する)。止めなかった子孫がいる target dir は使わない (R-WS-2。残った子孫は target dir のロックを持たないので、ロックが取れても書き続けている場合がある)。記録するもの、照合の規則、子を spawn するスレッド、版の違う記録の扱いは Q-29 で決める (v0.16)。M2 で決めた形 (Q-29。m2-plan.md の C4・C4a〜C4c。子を起動するスレッドは上記。v0.20):
      - 記録の置き場: キャッシュの `procs/v1/<実体 ID>.<番号>.lock` と `.json`。サーバーは起動時に乱数の実体 ID を作り、終了まで `locks/` の `owner:<実体 ID>` のロックを持つ。持ち主の生死はこのロックが取れるかで判定する (PID の再利用を気にせず、3 OS で同じに動く。案だった PID と起動時刻での判定はしない)
      - Unix では、子を起動する前に記録のロックを取り、その fd を子の中でだけ継承させる (close-on-exec を子の中で外す)。子と子孫は同じロックを持つので、持ち主が止まった後もロックが取れなければ、子孫が残っていると確かに分かる。Linux・macOS では、子のセッション ID と起動時刻と、マシンと起動の識別 (Linux の boot_id など) を記録し、子が終わったら記録を消す
      - 起動時の照合: 持ち主の止まった記録で、継承させたロックが取れなければ、マシンと起動の識別が自分と同じときだけ、そのセッションに属し起動時刻が記録以降のプロセスを止める。セッションの長が生きていて起動時刻が記録と違えば、ID が使い回されたとみなして止めない。Linux では止める各プロセスが継承したロックを開いていることも確かめ、macOS はセッションの長 (cargo) が生きているときだけ止める
      - Windows は記録しない (Job Object がサーバーの終了で中のプロセスを止める。Windows の継承は同時に起動した別の子にもハンドルを渡し、子孫が残っていると誤って判定するため)
      - 確かでないとき (別のマシンや起動の記録、起動時刻が読めない、列挙できない、該当するプロセスが無い = セッションを抜けた子孫) は止めず、ログに書いて記録を残し、その target dir に `unusable` の印を置く (R-WS-2)。片付けは起動を待たせず裏で行い、全体で 5 秒を超えたら次の起動に回す
      - 版の違う記録: ファイル名の実体 ID から、中身を読まずに持ち主の生死が分かる。持ち主が止まって更新から 7 日たった他の版の記録は、子孫を止めずに記録だけ消す (案の「読めない記録には触らない」を改めた)
  - ビルドは作業用ディレクトリで行い、成功したものだけを原子的に公開する (失敗したビルドや、終了で中断したビルドがキャッシュヒットにならない)。起動時に、持ち主のプロセスが生きていない作業用ディレクトリを消す。作業用ディレクトリはキャッシュの `work/<実体 ID>/` で、起動時に作る (使う前に落ちても掃除が見つけられるように)。起動時の掃除が持ち主の止まった `work/<実体 ID>/` とそのロックファイルを消し、終了時には (どの終わり方でも) 自分の分を消す (m2-plan.md の C4a。v0.20)
- R-WS-7 ビルド失敗:
  - `--message-format=json` の level=error の診断を先頭 N 件付けて `build_failed` を返す。エラーの text は 1 万文字以内 (R-ERR-2) なので、code・message・suggestions・request_id の余地を残して収める。初期値は、N (`build.max_diagnostics`) は 2、1 件は 3,500 文字以内、診断の合計は 8,000 文字以内 (どちらも `content[0].text` の JSON にエスケープした後の長さ) とした (v0.16)。M2 の F5 で、場面ごとにソースを壊して実際の診断を測り、この値に据え置いた (どれも切れず、連鎖した 27 件のエラーは中央値 411 文字で 3 件目以降はほぼ同じ形だった。m2-plan.md の F5。v0.20)。見せなかった件数は `diagnostics_omitted` に書き、rustc の `aborting due to N previous errors` (code も span も無い error) は数えない。ビルドが覚える診断は 20 件まで (m2-plan.md の D5a。v0.20)。診断は `message.rendered` を使い、サニタイズ (R-SEC-7) の後に文字の境界で切る。JSON の診断が無い失敗 (build.rs の失敗など) では、cargo の stderr の末尾を同じ上限で付ける (stderr もサニタイズしてから切る。v0.20)。失敗の判別に stderr の文字列照合を使わない。例外は、R-WS-2 のロック待ち、`requires rustc` と toolchain の未導入 (下記)、R-VR-2 の `--locked` の失敗、下記の git の取得の決定的な失敗、下記のオフラインでキャッシュに無いことによる失敗、cargo の stderr の `(signal: 9, SIGKILL` (cargo の子が殺された。一時的な失敗。v0.18) だけとする (R-WS-5 の `-p` で失敗する依存は、起動前に判定するので照合しない。v0.17)。いずれも終了コードでは見分けられない。文言は M2 でテストに固定する (v0.18)。固定した (git の取得の失敗は 5 つの失敗 × libgit2 と CLI × 3 つのコマンドの 30 通り、rustup の「not installed」、`requires rustc`。m2-plan.md の B5・D1a・D5a。v0.20)
  - **ビルド失敗を「存在しない」に畳み込まない**。「存在しない」としてキャッシュしない (ferritin の `NotAvailable` の誤り)。path / git の依存で黙って docs.rs に切り替えない。直近に成功した JSON があっても常に `build_failed` を返す (古い結果で壊れていることを隠さない)
  - 失敗は 2 種類に分ける
    - **一時的な失敗**: 終了の仕方で分かるものだけ。シグナルによる終了 (OOM killer など。cargo の子が殺されたときは、上の stderr の文言で分かる) と、サーバーが止めたもの (サーバーの終了による中断、ロック待ちの打ち切り)。ビルドの最大時間の超過 (`timeout`) は決定的な失敗にする (R-WS-6)。キャンセルで止めたビルドは失敗として記録しない (v0.18)。依存の取得はビルドの前に `cargo fetch --locked` (仮のプロジェクトでは `--locked` なし。R-VR-2) で分けて行い、その失敗も一時的とする。一時的な失敗は `retryable: true` で返し、R-ERR-3 の一時的な失敗の TTL (目安 30 秒) の間だけ再ビルドを抑える。git の取得・`cargo metadata`・`cargo fetch`・`--offline` の失敗は次のとおり扱う
      - M2 の形 (m2-plan.md の D5a・E2a・F4b。v0.20): 一時的とするのは、cargo の子が殺された、cargo を起動できない、JSON を公開できない、cargo が残したものが頼んだビルドのものでない (別のビルドが書きうるため)、`cargo fetch` の失敗 (`fetch_failed`・`lockfile_outdated`・`offline_miss` を除く。fetch の時間切れと起動の失敗も一時的)、toolchain の調べ (`rustc -vV`・`rustdoc -vV` の 10 秒の上限) の時間切れ (その失敗は 60 秒覚える)。サーバーの終了で止めたビルドも記録しない。`cargo fetch --locked` は作り直すときだけ、ビルドの前に 1 回走らせ、ビルドが `--target` を渡すときは fetch にも渡す。仮のプロジェクトは、最初に 1 回だけ `--locked` なしで Cargo.lock を作り、その後は `--locked` で走らせる (呼び直しのたびに作り直すと推移的な依存の版が動き、鍵が変わるため。git の ref の解決し直しは R-WS-10)
      - 失敗の記録は member の置き場の `failed.json` (入力の鍵・`rustc -vV` の toolchain・一時的か・時刻・返したエラーの本文) で、キャッシュの中にあるので、再起動や別のセッションでも効く。鮮度の鍵 (R-WS-4) を取った後、ビルドの間にソースが変わっていれば記録しない (編集の途中の失敗を固定しないため)。記録は、同じ鍵のビルドの公開と同じロックの下で書き、公開に成功したら消す
      - **git の取得の決定的な失敗**: 認証の失敗・リポジトリが無い・rev (ブランチ・タグ・コミット) が無い、のどれかと cargo の文言で判定できたものは、決定的な失敗として `fetch_failed` (`reason` 付き) を返し、下の決定的な失敗と同じく `refresh` までキャッシュする (再試行を繰り返させないため)。判定できなければ一時的な失敗のまま扱う。判定に使う文言は次のとおり (libgit2 と git の CLI で違い、取得先でも違う。cargo 1.99.1・git 2.56.0 で M0 で確かめ、`file://` の rev と CLI の認証の文言は 2026-10-19 に足した。`cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update` で同じ): rev が無い → GitHub では、libgit2 は `revision <rev> not found` (通信の失敗を示す `network failure seems to have happened` も一緒に出て、cargo が再試行するので遅いが、`revision … not found` があれば決定的な失敗とする)、CLI は `fatal: remote error: upload-pack: not our ref <rev>` (と cargo の `revision <rev> not found`)。`file://` では libgit2 も CLI も `revspec '<rev>' not found` (v0.18)。ブランチが無い → libgit2 は `cannot locate remote-tracking branch`、CLI は `fatal: couldn't find remote ref`。リポジトリが無い → `file://` では libgit2 が `failed to resolve path`、CLI が `does not appear to be a git repository`。GitHub の https では、存在しないリポジトリも認証の失敗になる (libgit2 は `failed to authenticate when downloading repository`、CLI は `could not read Username`)。CLI の `Authentication failed for` も `auth` にする (v0.18)。git を取得しうる子 (`cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update`) には `LC_ALL=C` を渡す (git は翻訳があれば地域化する。M0 の環境には翻訳が無く、確かめられなかった) (Q-30。M0 で決定。v0.17)
        - M2 で足した文言 (m2-plan.md の B5。v0.20): 無いタグは、libgit2 では cargo 自身の行 `failed to find tag` (と `reference 'refs/remotes/origin/tags/…' not found`)、CLI では `couldn't find remote ref refs/tags/…`。無いブランチの cargo 自身の行 `failed to find branch` も足した (M1 は libgit2 の無いタグを拾えなかった)。cargo は、無いブランチのような決定的な失敗も `net.retry` (既定 3) の回数だけ間を置いて試し直す (B5 の 30 通りで合計 170 秒)。製品は利用者の設定のままにし、この待ちは R-WS-6 の時間の上限と `status: building` が受け止める
      - **`cargo metadata`・`cargo fetch` の失敗**: 既存の例外 (R-VR-2 の `lockfile_outdated`、上の `fetch_failed`) に当たらないものは、決定的な失敗として固定せず、`build_failed` (`retryable: true`) として cargo の診断を添えて返し (v0.16 で R-ERR-2 から `stage` を外したので付けない)、一時的な失敗の TTL だけ抑える。Cargo.toml の誤りや存在しない版も通信の失敗と同じ終了コード 101 で失敗し、見分けるには文言の照合しかないため (`network_error` とも決めつけない。M0 で確かめた。v0.17)。M1 の `cargo metadata`・`cargo tree` は失敗を覚えず、呼ぶたびに起動し直す (抑えるのは M2。R-VR-2。v0.19)。M2 では、`cargo metadata` が失敗して Cargo.lock を直に読んだ結果と `cargo tree` の失敗を、入力が同じ間 30 秒覚え、その間は cargo を起動せずに返す (成功は入力が同じ間ずっと使う。m2-plan.md の B4。v0.20)。版の解決で `cargo metadata` が git の認証の失敗で落ちたときは、warnings に `cargo fetch` と `gh auth setup-git` の案内を書く (R-NW-4。v0.19)
      - **オフラインでキャッシュに無いことによる失敗**: 次の 3 つの文言は、どれも `offline_miss` にする。`no matching package named` (crate 名の誤りと同じ文言)、`attempting to make an HTTP request, but --offline was specified`、`you are in the offline mode (--offline)`。サーバー自身が `--offline` を渡したときだけでなく、利用者の cargo の設定 `net.offline` でオフラインのときも同じ (Q-30。M0 で決定。v0.17。v0.18 で文言と `net.offline` を足した)。M1 は、`no matching package named` (オンラインでは crate 名の誤りと同じ文言) を、サーバーが `--offline` を渡したときだけ `offline_miss` に数える。利用者の `net.offline` を判定するのは、ビルドと一緒に M2 (M1 の終わりに利用者が決めた。v0.19)。M2 で入れた: cargo の設定の `net.offline` と環境変数 `CARGO_NET_OFFLINE` (読み方は R-WS-2 の合成。環境変数が勝つ) でオフラインなら、`no matching package named` も `offline_miss` に数える (`cargo metadata`・`cargo tree` とも。ビルドのオフラインの判定にも同じものを使う。m2-plan.md の B4・D2c。v0.20)
    - **決定的な失敗**: それ以外のすべて (JSON の診断を出さない build.rs の失敗も含む。システムライブラリの不足など)。結果 (診断の要約を含む) を R-WS-4 の鍵 (使った toolchain を含む) でキャッシュし、鍵が変わるか、ツールの引数 `refresh` (§6.4) を指定するまでは、再ビルドせずに同じ `build_failed` を `retryable: false` と失敗の時刻付きで返す。hints には「原因 (システムライブラリの不足など) を直したら、照会のツールは `refresh`、外のツールは同じ引数での呼び直しで再試行する」と書く (rustdoc-mcp は失敗したビルドを呼び出しのたびにやり直す)。記録は上の `failed.json` で、`refresh` の案内は記録した決定的な失敗にだけ付ける (一時的な失敗と、記録しなかった失敗には付けない。m2-plan.md の D5b。v0.20)
    - 既知の残り (M3 の最初の段で直す。m2-plan.md の D5a・D5b の残りと §7.1 の決定 5・8。v0.20): 要件は上のままとし、実装の次の 3 つのずれを直す
      1. ビルドの最大時間の超過 (`timeout`) は、その時間のほとんどが cargo のロック待ちだった場合も決定的な失敗として覚える (ロック待ちの打ち切りは一時的な失敗とする上の規則とずれる。`build.timeout_secs` はサーバーのロックと枠の待ちを数えないので稀)
      2. `cargo metadata` の git の取得の決定的な失敗 (`fetch_failed`) は 30 秒しか覚えず、`refresh` の案内も付かない (上の「`refresh` までキャッシュする」とずれる。一時的な失敗として扱っているので安全側)
      3. `toolchain_too_old` の suggestions は、rustup の無い環境でも `rustup update` を例に書く (rustup の無い環境では rustup のコマンドを案内しない規則 (R-TG-1) とずれる)
  - R-WS-8 で選んだ toolchain が使えない場合 (オフラインモードや、rustup の自動導入が失敗した場合) は `toolchain_missing` と導入コマンドを返す (std は R-STD-4)。サーバー自身は `rustup` や `cargo install` で toolchain・コンポーネント・ツールを導入しない (R-STD-6 と R-SC-2 のキャッシュへの取得は除く)
    - rustup は、プロジェクトの `rust-toolchain.toml` が指す未導入の toolchain を既定で自動で導入する (rustup 1.29.1)。サーバーが自動導入を許すのはこれだけ (rust-analyzer や利用者の cargo でも同じく導入されるので、止めても守りにならない。v0.14)。サーバー自身が選ぶ toolchain (R-WS-8 の nightly や日付を固定したもの。`cargo +<toolchain>` にも rustup の自動導入は効く。2026-10-16 に実測) と、オフラインモード (R-CA-7) では、子プロセスに `RUSTUP_AUTO_INSTALL=0` を付けて止め (効くことは 2026-10-15 に確認)、未導入なら `toolchain_missing` を返す。R-CA-8 を実装した場合は、static.rust-lang.org を無効にしたときも同じく止め、ミラーを設定したときは `RUSTUP_DIST_SERVER` も子に渡す (v0.16)
    - プロジェクトの toolchain が入っていないとき (m2-plan.md の D1・D2c。v0.20): 事前の確かめは `RUSTUP_AUTO_INSTALL=0` で行い、rustup の「not installed」で分かる。オンラインならビルドの cargo に rustup の自動導入を任せ (その間は `status: building`)、入った後は保持していた「not installed」の結果を捨てて確かめ直す。オフラインなら `toolchain_missing`
  - E0658 (`--message-format=json` の診断コード) と、cargo が stderr に出す `requires rustc` は `toolchain_too_old` に分類し、使った toolchain の更新を案内する (R-WS-8 で選んだ toolchain に合わせる。プロジェクトの toolchain が `rust-toolchain.toml` で固定されていればその更新、rustup の無い環境では rustup のコマンドを案内しない。v0.17)。M2 の suggestions (m2-plan.md の D5a。v0.20): サーバーが選んだ nightly なら `build.nightly_date` を後の日にするか `rustup update nightly`、`rust-toolchain.toml` (か `rust-toolchain`) がプロジェクトのルートかその上にあればその更新、無ければ `rustup update`。docs.rs の組での再試行 (R-FT-3) は E0658 の失敗でも行う (docs.rs の組が nightly の機能を有効にしうるため)
- R-WS-8 toolchain の選び方:
  - **既定は、プロジェクトの toolchain (`rust-toolchain.toml`、なければ既定の toolchain) に `RUSTC_BOOTSTRAP=<crate 名>` を付けたもの** (Q-12。M0 で決定。v0.17)。値はパッケージ名ではなく、ターゲットの crate 名 (アンダースコアの形。`[lib] name`) にする。どの crate 名を並べるかは下の「値を安定させる」に従う
    - 実測 (2026-10-14、stable 1.99): rustdoc に `RUSTC_BOOTSTRAP=<crate 名>` を付けると形式 61 の JSON が出る (付けなければ `-Z` が拒否される)。`cargo rustdoc -- -Z…` 経由でも crate 名の指定で成立した: lib、bin、proc-macro crate 自身、build.rs (`rustc-env` と `rustc-cfg`) と依存の proc-macro (derive) を持つ crate、`--document-private-items` 付き。build.rs が設定した cfg の item も、derive が生成した impl も JSON に出た。パッケージ名 (`my-app`) を指定すると失敗する。build.rs が `RUSTC_BOOTSTRAP` を見ない依存は、値を変えても再 check されなかった
    - **値を変えると作り直される依存がある**: proc-macro2・anyhow・thiserror などの build.rs は stable で `cargo:rerun-if-env-changed=RUSTC_BOOTSTRAP` を出すので、値を変えるたびに作り直される (2026-10-14 に確認。proc-macro2 の下には syn と多くの proc-macro がぶら下がる)
    - **そこで値を安定させる**: 値を変えるたびの作り直しを避け、依存のための値で member の文書が変わらないようにする。利用者の環境に `RUSTC_BOOTSTRAP` があっても常に上書きし、その旨をログに出す。値は、プロジェクトの専用 target dir ごとに、**依存の解決結果のすべての lib (cdylib・staticlib・dylib だけの lib を含む。v0.18)・proc-macro の crate 名と member の bin の crate 名を辞書順にカンマでつないだもの**に固定する (Q-12。M0 で決定。v0.17)。値は Cargo.lock と member の構成が変わらない限り変わらないので、作り直すのは文書化する crate の rustdoc だけになり、target dir も 1 つで済む (M0 では、値を変えると proc-macro2 の build.rs から syn と proc-macro を経て 54〜65 unit・10〜13 秒の作り直しが起き、固定すれば 0.2〜0.9 秒だった)。自分の名前で nightly の機能を調べる依存 (proc-macro2・thiserror など) は nightly の経路でコンパイルされ、利用者の stable のビルドと item が違いうる (v0.18)。M0 の B1 では、derive が生成した item の span の列が少し変わり (docs.rs と同じ側になる)、それ以外の文書の中身は変わらず、thiserror の文書は docs.rs と一致した。ただし B1 で調べたのは 2 crate で、`#[path]` のモジュールを見落とす探し方だった (v0.18)。その経路でビルドが壊れても、member の名前だけの値でやり直さない (依存の作り直しを招き、壊れた例も無いため。例が出たら考える。Q-31。m2-plan.md の §7.1 の決定 4。v0.20)。値の長さの上限は 3 OS 共通で 30,000 バイトとし、超えたら `RUSTC_BOOTSTRAP=1` に落として、ログと応答の warnings に書く (Q-31。m2-plan.md の D2。v0.20)。案として挙げていた「member の名前の固定の一覧」「依存用の別の target dir と build-dir」「文書化した依存の名前を足していく一覧」は、作り直しが起きるか、target dir が 2 つになるので採らない
    - 実測 (2026-10-16、stable 1.99): `RUSTC_BOOTSTRAP` はカンマ区切りで複数の crate 名を受ける (空白は不可)。member 用の値に依存の名前を足すと、anyhow などがその名前で nightly 用の機能を有効にし、member の文書まで stable と違いうる。build-dir を共有すると中間生成物と fingerprint が共有され、値を切り替えるたびに作り直された。値を固定すれば作り直されないことを確かめた (member `a`・`b` が proc-macro2 に依存する workspace で、`RUSTC_BOOTSTRAP=a,b` のまま `a`・`b`・`a` の順に文書化すると、proc-macro2 のビルドは最初の 1 回だけだった。値を crate ごとに変えると毎回ビルドされた)。bin の crate 名も値に入れる必要がある (`a,b` のままでは `--bin my-app` が `-Z` を拒まれ、`a,b,my_app` で通った)
    - cargo-semver-checks は本番で `RUSTC_BOOTSTRAP=1` を使っている。本サーバーは crate 名を指定する。値が解決結果の全 crate 名になるので、`RUSTC_BOOTSTRAP=1` との違いは、rustc の判定の上では、cargo 自身の `-Z` (cargo は値が `1` のときだけ受ける) と、build script (crate 名は `build_script_build`) に効かないことだけになる (build script が別の crate 名で行う検査用のコンパイルや、値そのものを読む build.rs では違いうる。確かめていない)。cargo の unstable な機能を黙って有効にしないことと、build script の挙動を利用者の普段のビルドとそろえることのために、crate 名の指定を続ける (v0.17)
    - 例外 (m2-plan.md の E2a。v0.20): 外のツールの仮のプロジェクト (R-WS-5) は、常に `RUSTC_BOOTSTRAP=1` を付ける。すべての仮のプロジェクトが target dir を共有する (R-WS-2) ので、値が変わらず作り直しが起きない。docs.rs も nightly で作るので、依存でない crate の文書はむしろ docs.rs に近い。既定のプロジェクトは上のとおり crate 名を指定する
    - 利点: nightly が不要で、プロジェクトが実際に使う toolchain と同じコンパイラで生成できる (MSRV や新しい構文での失敗が起きない)
    - 出る形式は toolchain の版で決まる (1.97 / 1.98 / 1.99 はそれぞれ形式 57 / 60 / 61。1.97・1.98 は rust のソースのタグの `FORMAT_VERSION` で、1.99 は上の実測で確認。2026-10-16)。対応範囲 (R-FV-4) より古い形式しか出ない古い toolchain では、nightly に切り替える
    - 依存の多い実際の workspace (このリポジトリ、242 パッケージ) で、作り直しの範囲と時間、文書の違いを確かめて既定に確定した (Q-12。結果は m0-plan.md の §6.2 の B1。v0.17)
  - nightly を使う場合は最新の `nightly` を既定にする。**日付を固定した nightly を既定にしない** (新しい MSRV の crate でビルドに失敗する)。固定は設定で選べるようにする (設定 `build.nightly_date`。std の配布物の選び方には効かせない。m2-plan.md の D1b。v0.20)
  - toolchain が出す形式が canonical より新しく読めなければ、黙って読まず、`unsupported_format` を返してサーバーの更新と toolchain の固定の方法を suggestions に示す。調べた結果はプロセスの寿命で固定せず、新しい toolchain を入れたら再起動なしで反映する。M1 は照会のための toolchain のコマンドの出力を 60 秒だけ持つ (R-PF-1。v0.19)。M2 で決めた調べ方と切り替え (Q-31。m2-plan.md の D1・D1b・D2。v0.20):
    - 時期と方法: 初めて使う前に、作業用ディレクトリに書いた 1 行の crate に rustdoc を直接走らせ (cargo を使わない。`RUSTC_BOOTSTRAP=probe`。`--edition` は 1.56 より前の rustdoc が受けないので付けない)、出た JSON の `format_version` を読む (stable 1.99.1 で約 0.1 秒、nightly で 0.4〜0.8 秒)
    - キャッシュ: rustdoc の `-vV` の commit-hash と rustdoc のパスと `+toolchain` を鍵に、キャッシュの `toolchains/v1/` に記録する。commit-hash の無い rustdoc は記録しない。`rustc -vV` は今どおり 60 秒だけ持つので、toolchain を入れ替えれば再起動なしで反映される
    - 切り替え: 形式が対応範囲の下限 46 (R-FV-4) 以上ならプロジェクトの toolchain を使う。61 より新しくても、作って読めれば確かめていない旨の warnings を付けて使い、読めなければ `unsupported_format` (別の toolchain には切り替えない。R-FV-5 と同じ)。46 未満なら、rustc と rustdoc がどちらも rustup の proxy (設定・環境変数・プロジェクトの設定が別のものを指していない) で rustup があるときだけ、`+nightly` (`build.nightly_date` があれば `+nightly-<日付>`) を `RUSTUP_AUTO_INSTALL=0` で試す。nightly も入っていなければ `toolchain_missing`、切り替えられなければ `unsupported_format`
    - ビルドの中で toolchain が入ったとき (R-WS-7) は、できた JSON の形式で選び直し、古すぎれば同じ仕事の中で nightly で作り直す
  - std の JSON (`rust-docs-json`) は nightly の配布物にしか無い (stable 1.99 と beta 1.100 の manifest にも節はあるが、全 target が `available = false`。2026-10-15 に確認)。std だけは手元の nightly の `rust-docs-json` か、R-STD-6 の dist からの直接取得を使う (§5.11)
- R-WS-9: workspace の JSON は最後に成功したビルドの時点を表す。応答に `built_at` を付け、ソースがその後に変わっていれば LSP を使うよう hints で示す (§9.9)。作り直しの間に前回のビルドを返す stale の応答 (R-WS-6) にはすべて、LSP を案内する hint と、span の行がずれうる warning を付ける。他の target や外の workspace の記録を入力の変化で stale として返すとき (`inputs_changed`。R-WS-10) も、warning と、LSP と外のツールの呼び直しを案内する hint を付ける (m2-plan.md の D4c・E4a-3。v0.20)
- R-WS-10 **`build_external_docs`** (§6.3。R-MCP-3・R-SEC-2): 既定のプロジェクトの外のコードの文書を作り、照会のツールが読めるように記録する
  - 対象と記録 (外のツールの範囲はここを正とする): 依存でない crate と Cargo.lock に無い版 (`crate`・`version`。`api_diff` の比較相手で docs.rs に JSON が無い版を含む)、既定のプロジェクトの member と依存を、実際のビルド target 以外の target で文書化したもの (`target` は必須。member は `project`、依存は `crate`。版は Cargo.lock のもの。R-MCP-3)、git リポジトリの crate (`git` と `rev`・`branch`・`tag`)、既定のプロジェクトの外の workspace (`project`)、`-p` で名指しできない crates.io の依存 (R-WS-5。v0.20)、既定のプロジェクトの member を `[package.metadata.docs.rs]` の組で文書化したもの (その組が照会のツールの範囲の外を作るとき。`crate`。下の表。v0.20)。外の workspace と、既定のプロジェクトを他の target で文書化するときは、まず `cargo metadata` の結果と入力ファイルの内容ダイジェスト (R-WS-4) を記録する (照会のツールがその記録を R-MCP-3 で読むため。v0.16)。M2 で、`cargo metadata` を記録するのは外の workspace だけにした。既定のプロジェクトの他の target では記録せず (ビルドごとの入力のダイジェストだけ)、版は照会のときにプロジェクトの `cargo metadata` で解く (m2-plan.md の E4a の決定 1。v0.20)
    - 記録の置き場と鍵 (m2-plan.md の M2e の決定 1 と E1b〜E4c。v0.20): 本体 (JSON とビルドの記録) は照会のツールのローカルビルドと同じ `local/v1` に置き (アーカイブ・鮮度・作り直し中・失敗の仕組みを使い回す)、外の照会の鍵から引く索引だけを `ext/v1/<種類>/<鍵のハッシュ>/record.json` (鍵・本体の場所・本体の SHA-256・記録の時刻) に持つ。鍵の種類は、crates.io のリリース (crate・版・target・hidden)、git の要求 (正規化した URL・ref の完全な名前・crate・target・hidden) と commit (URL・commit・crate・target・hidden)、既定のプロジェクトの他の target と docs.rs の組の member (workspace・package id・target・hidden・crate)、外の workspace (workspace) と (workspace, target)。hidden は `document_hidden_items` を宣言する M4 のために鍵に含めておく
    - 外の記録から返す応答と外のツールの応答の provenance は `source: local` に `recorded_by: "build_external_docs"` を付ける (作り直すには外のツールを呼び直す、と分かるように。m2-plan.md の E1b。v0.20)
  - **外の workspace (`project`)**: 既定では、全 member の lib (lib の無い member は bin を 1 つ。proc-macro の member は host 向け。v0.20) と、docs.rs で引けない依存 (path・git・非公開のレジストリのうち `-p` で名指しできるもの。名指しできない path・git の依存は「(in a later version)」で、M3 以降に既定のプロジェクトの同じ場合と一緒に扱う。v0.20) をビルドする。member は docs.rs の組で作る (v0.20)。範囲はその workspace の `cargo tree --target` の集合 (R-MCP-3 と同じ組の規則) に絞る。crates.io の依存で docs.rs に JSON が無いかは照会のときに確かめ、無ければ記録に「まだビルドしていない依存」として残し、外のツールでの作り方を案内する (ビルドのときに全依存の docs.rs を確かめる要求を出さないため。v0.15)。M2 では、照会のツールは docs.rs に無い crates.io の依存に `crate` (`<名前>@<版>`) と `project` を渡す案内をし、外のツールはそう頼まれたら docs.rs を先に見て、無ければその workspace の中で作る (m2-plan.md の E4b の決定 4。v0.20)。任意の `crate` で 1 つの member か依存に絞れる。docs.rs に JSON がある依存は、記録した版の解決で照会のツールが docs.rs から引く。記録は専用 target dir と同じ GC の枠 (R-CA-5) に入れ、照会のツールが読んだときも最後に使った日時を更新する。照会のツールは、外の記録が壊れていれば作り直さずに `docs_unavailable` を返して外のツールを案内し (R-CA-4 の取り直しは外の記録には使わない)、外のツールは記録があっても検証に失敗したら作り直す (v0.15)
    - M2 で決めた記録の形 (m2-plan.md の E4b。v0.20): workspace の鍵に、cargo metadata の出力・Cargo.lock・入力の digest・`members` の glob が合うディレクトリ・ビルドの target を記録する (内容が前と同じなら記録の時刻を引き継ぐ)。(workspace, target) の鍵に、`cargo tree` (normal と normal,dev) の出力と、package ごとの結果 (作った・docs.rs で引く (とその target か host か)・名指しできない) と、計画の元にした workspace の記録の時刻を記録する。照会のツールはこれを読み直して版と features を解く
    - 記録と計画は (workspace, target) ごとの共有の仕事で、呼び出しの期限を過ぎても続け、`status: building` を返す。期限を過ぎて終わった結果は次の 1 回の呼び出しに渡す (記録し直す仕事が毎回期限を越えて終わらなくならないように)。別名 (渡したパス・root・member のディレクトリ・渡したパスが root を指すときはその下の member) も記録し、ディレクトリが消えたときに引く (R-MCP-3)。Cargo.lock が無い・古いときは `cargo generate-lockfile` を案内し、このサーバーは書かない
  - **呼び直し** (v0.15): どの対象でも、同じ引数で呼び直したら決定的な失敗のキャッシュ (R-WS-7) を解いて取り直す (外のツールには `refresh` を案内しない)。同じ鍵のビルドが進行中なら合流する (v0.16)。ブランチなどで、合流した仕事の解決がその呼び出しより前に終わっていて成功なら、終わるのを待ってから 1 回だけ解決し直す (失敗ならその失敗を返し、次の呼び出しがやり直す。Q-29。m2-plan.md の M2e の決定 13・E3b-1。v0.20)。何を取り直すかは対象ごとに決める: `crate@version` は、記録が有効ならそれを返し (作り直さない)、外の記録が失敗か壊れているときだけ、失敗の記録 (features のやり直しの記憶を含む) を解いて作り直す (m2-plan.md の E2a。v0.20)。同じ `branch`・`tag`・`refs/…` の `rev` は解決し直す。v0.16 の案の `cargo update -p` ではなく、要求の鍵の仮のプロジェクトの Cargo.lock を作り直して解決し直す (ハッシュの `rev` は作り直さない。v0.20)。解決し直して決定的な失敗が続く (消えた branch など) ときは前の解決を忘れ (照会のツールが古いコミットを返し続けないため)、一時的な失敗なら残す (m2-plan.md の E3b-1。v0.20)。workspace は metadata とダイジェストを記録し直し、変わったものだけ作り直す (`stale` を解消する手段)
  - **入力から処理を決める表** (v0.15):

    | 入力 | 処理 |
    | --- | --- |
    | `project` (既定のプロジェクトの外) | 外の workspace (上記) |
    | `project` (既定のプロジェクト) と `target` | 他の target での文書化。既定は全 member の lib (lib の無い member は bin を 1 つ。member ごとの共有の仕事を一度に始めて `build.wait_secs` まで待ち、member ごとに `built`・`building`・`failed`・`query_tools_build` (proc-macro の member は照会のツールが host 向けに作る) を並べる。一部の失敗で呼び出しを失敗にしない。m2-plan.md の E4a。v0.20) |
    | `project` (既定のプロジェクト) で、`target` が無いか実際のビルド target と同じ | 照会のツールの範囲なのでビルドせず、その旨を返す (v0.16。`document_hidden_items` を付けたときを除く。下の行。v0.18) |
    | `crate` が既定のプロジェクトの member で、`target` が無いかプロジェクトの target (v0.20) | その member の `[package.metadata.docs.rs]` の組が照会のツールの範囲 (R-MCP-3) の外を作るときだけ、その組で照会のツールと離れた置き場と target dir に作って記録する (照会のツールはその記録を優先して読み、入力が変わっていれば `inputs_changed`)。docs.rs の組がコンパイラのエラーになれば失敗として記録し、プロジェクトの組に戻さない。範囲内なら照会のツールの範囲なのでビルドしない (`query_tools_build`)。新しい引数は足さない (m2-plan.md の M2e の決定 10・E4c) |
    | `crate` が既定のプロジェクトの依存 (版は Cargo.lock のもの) | `target` が実際のビルド target と違えば、他の target での文書化 (プロジェクトの workspace で)。同じなら照会のツールの範囲なのでビルドしない (`document_hidden_items` を付けたときを除く。下の行。v0.18)。ただし `-p` で名指しできない依存 (R-WS-5) は、仮のプロジェクトで文書化する (v0.17。版を省けば Cargo.lock の版で、複数あれば `invalid_argument` と候補。プロジェクトのビルドが host 向けにだけ作る依存は host で作る。v0.20)。他の target でだけ使う依存で `target` が無ければ `invalid_argument` (使われる cfg を添える。v0.20) |
    | `crate` が依存でないか、`version` が Cargo.lock に無い | 仮のプロジェクト (R-WS-5。target は引数か host)。`crate@latest` と範囲の指定はプロジェクトに引き取らず crates.io のリリースとして扱う。member・依存・crates.io の同名が重なれば `invalid_argument` と候補 (m2-plan.md の E4a。v0.20) |
    | `git` と `rev`・`branch`・`tag` のどれかと `crate` | 仮のプロジェクトの git 依存 (R-WS-5) |
    | どれにも `document_hidden_items` | 付けられる (`api_diff` の比較用)。docs.rs に読める JSON があっても、照会のツールが同じ target でビルドする範囲でも、ビルドする (hidden 付きのビルドは両側とも外のツールが作る。R-AD-3。v0.18)。引数は M4 (`api_diff`) まで宣言しない (記録の鍵には hidden かを含めておく。m2-plan.md の M2e の決定 9。v0.20) |

  - **他の target での文書化** (依存でない `crate` と `git` の crate も、引数 `target` で他の target を指せる。R-WS-5): 依存は `crate` に版付きの pkgid で指す。member と依存のどちらか決まらなければ `invalid_argument` にする。`target` を省いたら `invalid_argument` を返し、他の target 向けの依存 (`[target.'cfg(…)'.dependencies]` など) なら Cargo.toml の条件 (`cfg(windows)` など) を添える。docs.rs がその版でビルドした target (R-TG-1。2026-05-01 以降のリリースは多くが 1 つ。v0.18) のうち条件に合うものを計算して候補に添えるのは任意とする (v0.16)。依存は、docs.rs にその target の JSON が無ければ、既定のプロジェクトの workspace で `cargo rustdoc -p <package id> --target <target>` (R-WS-1) を実行する (版はプロジェクトの Cargo.lock のもの。Linux で `x86_64-pc-windows-gnu` を指して、Windows でだけ使う依存も、member の `#[cfg(windows)]` の item も文書化でき、JSON の `target.triple` もその target になることを 2026-10-16 に確かめた)。その target の `rust-std` が無ければ (ビルドの前に sysroot の `lib/rustlib/<target>` で判定する。判定できることを M0 で確かめた。R-TG-1。v0.17)、`rustup target add` を添えて `toolchain_missing` を返す (rustup の自動導入は target を入れない。v0.15)。他の target のビルドは (workspace, target) ごとの target dir で行い (R-WS-2)、member は docs.rs の組で作る (壊れたら失敗として記録し、プロジェクトの組に戻さない)。範囲の確かめはしない (m2-plan.md の E4a の決定 2・E4c の決定 4。v0.20)
  - 鍵: target は照会のツールと同じ規則 (R-TG-1) で決め (他の target での文書化では要求した target)、照会のツールが R-MCP-3 の鍵で引けるようにする。toolchain は R-WS-8 で選び、照会の鍵には入れずに記録と provenance に載せる (R-MCP-3)。応答で、照会に使う鍵 (`crate`・`version`・`git`・`rev`・`project`・`target`。git の `rev` には解決したコミットのハッシュを入れる) と provenance を返す。git の URL は、cargo の `CanonicalUrl` に合わせて正規化してから鍵にする (末尾の `/` を外してから `.git` を外し、github.com の URL は小文字にする)。ただし scheme は書き換えずに鍵に含める (cargo は github.com の URL の scheme を `https` にそろえるが、平文で取った記録を https の照会に返さないため。v0.15)。照会のツールも同じ正規化をしてから引く
    - git の crate は 2 段で作る (m2-plan.md の E3・E3b。v0.20): 要求の鍵 (正規化した URL・ref の完全な名前・crate・target・hidden) の仮のプロジェクトで解決し、commit の鍵の仮のプロジェクト (`rev = <commit>` に固定) で作る。同じ commit を指す別の ref は作り直さない。要求の鍵の置き場に解決の記録 (commit・リポジトリでの package 名・版・解決の時刻) と前の commit を残し、crate ごとに記録した commit の一覧を持つ (短いハッシュの照合用)。package 名は完全一致で探し、無ければ `-` と `_` を入れ替えて探す。provenance は平らに `git` (正規化した URL)・`rev` (commit)・`ref` (指した branch・tag・`refs/…`)・`resolved_at`・`plaintext` (http・git のときだけ) を載せる。cargo に git の CLI を強いない (利用者の cargo の設定に従う)
    - 照会のツールは ref を解決の記録で引き、外のツールが ref を新しい commit に解決し直して作っている間は、前の commit のデータに `stale: rebuilding` と `retry_after_secs` を付けて返す。短いハッシュは記録のある commit だけで照合する (R-MCP-3。m2-plan.md の E3b-2。v0.20)
  - 進捗と、ビルドを待つ上限 (R-WS-6) と 1 リクエストあたりの時間の上限 (R-ARCH-6) は照会のツールと同じ。上限を超えたら `status: building` を返し、**ビルドは裏で続ける**。続きは照会のツールで待つよう hints で案内する (外のツールを呼び直すと、利用者が ask を書いていれば呼ぶたびに確認が出るため。v0.15)。docs.rs に読める JSON があるものと、照会のツールが同じ target でビルドする範囲 (R-MCP-3) のものは、ビルドせずにその旨を返す (`document_hidden_items` を付けたときを除く。上の表)
  - 照会のツールは、外の記録 (ビルドの結果) がまだ無くても進行中なら、`docs_unavailable` ではなく `status: building` と `retry_after_secs` を返す。進行中の印は、持ち主のサーバーが死ねば OS が外すロック (R-CA-4) で表す (PID の再利用を気にせずに判定でき、3 OS で同じに動く)。裏のビルドが失敗したら、その結果 (診断と時刻) も照会の鍵で記録し、照会のツールは `build_failed` を返す (R-WS-7 の鍵には `rustc -vV` が入り、照会のツールは組み立てられないため)。印の取り方 (試し取りの手順、ビルドの枠を待つ間も持つか、git で要求の鍵と解決したコミットの鍵の両方を持つか) は Q-29 で決める (v0.16)。M2 で決めた印の取り方と読む順 (Q-29。m2-plan.md の M2e の決定 13・E1b。v0.20):
    - 外のツールは、仮のプロジェクトを作る前から公開まで (ビルドの枠を待つ間も) 鍵ごとの排他ロック (`ext:<鍵のハッシュ>`) を持ち、照会のツールは共有ロックの試し取りで覗く。git は要求の鍵、commit の鍵の順で両方を持つ
    - 照会のツールが外の記録を読む順: 有効な記録を返す → ロックが持たれていれば `status: building` → 失敗の記録があれば、古さによらず `build_failed` と外のツールに渡す引数の案内 → 索引か元の JSON が壊れていれば `record_broken` (取り直さない。アーカイブは JSON から作り直す) → 無ければ `docs_unavailable` と外のツールに渡す引数の案内。読んだら最後に使った日時を 1 時間に 1 回まで更新する (R-CA-5)
  - glob の展開、R-PR-4 の latest による推定、R-CA-6 の先取りなど、照会のツールの暗黙の取得は docs.rs からだけ取り、外のローカルビルドを起こさない

### 5.11 std 対応

- R-STD-1: 手元の nightly に `rust-docs-json` があれば、`rustup run <nightly> rustc --print sysroot` の `share/doc/rust/json/` から std / core / alloc / proc_macro / test を読み込む (無ければ R-STD-6)。ただし手元の nightly を使うのは、その版がプロジェクトの toolchain の版以上のときだけにし、古ければ dist の新しいもの (R-STD-6) を使う。使ったものと理由を provenance に出す。dist を無効にしていれば、古いものを warnings 付きで使う (v0.18。dist を無効にする設定は、設定ファイルと一緒に M2 で入れる。v0.19)。M2 で設定 `std.fetch_from_dist` (既定 true) を入れた。false なら dist に要求を出さず、前に dist から取ってキャッシュにある std も使わない。手元の nightly が無ければ (使えなければ) `toolchain_missing` に導入コマンドを添える (R-STD-4。m2-plan.md の A3b-1。v0.20)。provenance には `rustc -vV` の全体 (release・commit-hash・commit-date。dist から取った場合はアーカイブの日付) と **target** を載せる。std の JSON は target ごとに中身が違う (`std::os::windows` などがある) ため
  - M1 で決めた細部 (m1-plan.md の B8・C1b-3。v0.19): 手元の nightly を使うのは、版の数字 (pre-release を除く) がプロジェクトの toolchain の版以上で、かつその host が要求した target と同じときだけ。プロジェクトの toolchain の版は、既定のプロジェクト (workspace が 1 つのとき) のルートで `rustc -vV` を実行して得る。手元の nightly でも、形式が対応範囲 (R-FV-4) の外なら使わない
  - provenance は `source: "std"`・`toolchain` (`rustc -vV` の 1 行目の形。release・短い commit-hash・commit-date)・`std_from` (`dist`・`local` と理由)。`resolved_from` は出さない。std の JSON の `crate_version` は semver として読めない形 (`1.101.0-nightly\t(コミット\t日付)`) なので、`version` には toolchain の release を入れ、要求との照合 (R-VR-4) はしない
  - std の照会に `version` を渡されたら、無視して warnings に書く (std は crates.io の版を持たない)
  - target を省いた std の照会は、ほかの crate と同じくプロジェクトの `build.target` (R-TG-1) を使う。独自の target (target の JSON・build-std) のプロジェクトでは host を使い、その旨を warnings に書く (docs.rs の crate のように既定の target には替えない。std は配布物から読むため。m2-plan.md の B3b・D7c-2。v0.20)。既知の残り (M3 の最初の段で直す。m2-plan.md の §7.1 の決定 6): `search_items` で std を検索するときは、`project` で指したプロジェクトの target をまだ見ない (既定のプロジェクトの target を使う)
  - `rust-docs-json` が配られていない target (`wasm32-unknown-unknown`、`thumbv7em-none-eabihf` など。配られているのはホストになれる target だけ) では、ホストの target の JSON で代用し、R-TG-2 のとおり実際に使った triple を provenance に載せて warnings で知らせる。core と alloc は target による差が小さいが、std は `std::os` などが食い違い、その target に std が無いこともある旨も warnings に書く
- R-STD-2: std の item には `Item::stability` (形式 58 以降) と `const_stability` (形式 59 以降) を返す。stable か unstable か、安定化されたバージョン、unstable なら feature 名を含める。**unstable な item と const-unstable なメソッドには必ず印を付ける** (ferritin は印なしで `push` を `const fn` と表示している)。M1 は `stability` に `stable_since`・`unstable_feature`・`const_stable_since`・`const_unstable_feature` を出す (v0.19)
  - 形式 60 以降の `default_unstable` (trait の関数・関連定数・関連型の**既定の実装**が不安定であること) も表示する。「この trait を実装するとき、この default に頼れるか」の判断に要る。M1 は `stability.default_unstable` に feature 名を出し、stable の toolchain で実装するときは書く必要がある旨を warnings に出す (v0.19)
  - rustdoc JSON の unstable には、tracking issue の番号も reason も入っていない (`feature` 名だけ。2026-10-14 に staged_api の crate で確認)。issue へのリンクを推測で作らず、feature 名だけを示す
- R-STD-3: **ユーザーが使っている toolchain** (`rust-toolchain.toml` か `rustc -vV`) と **MSRV** (`rust-version`) を基準に、次の item に警告を付ける (toolchain の版が分からなければ MSRV だけを基準にする)
  - まだ安定化されていない item
  - 基準より後のバージョンで安定化された item (安定化の版は `Item::stability` から取り (R-STD-2)、該当するリリースの要点を R-RR-1 から引用する)
  - M1 は toolchain だけを基準にする: unstable なら必要な `#![feature(...)]` を、stable になった版がプロジェクトの toolchain より新しければその旨を warnings に書く (`std::never` は 1.100.0 で、プロジェクトの 1.99.1 より新しい)。MSRV を基準にすることと R-RR-1 の引用は M4 (v0.19)
- R-STD-4: `rust-docs-json` が手元に無く、R-STD-6 を設定で無効にしているときは、導入コマンド (rustup の環境なら `rustup component add rust-docs-json --toolchain nightly`。v0.17) を添えた `toolchain_missing` を返す。オフラインモードでキャッシュにも無ければ、`offline_miss` (R-CA-7) に導入コマンドを添える
- R-STD-6 **nightly の toolchain を入れずに std を引く** (ferritin のホスト版は、手元に何も無くても std を引ける。その利点を取り込む):
  - nightly のチャネルの manifest (`https://static.rust-lang.org/dist/channel-rust-nightly.toml`) の `[pkg.rust-docs-json-preview.target.<triple>]` に、`rust-docs-json` 単体のアーカイブの URL と sha256 が載っている (2026-10-14 に確認。x86_64 Linux の xz で約 4.2MB)。これを取得し、**sha256 を検証してから**キャッシュに展開する。manifest の 17 の target は別の target のアーカイブを指している (`x86_64-apple-darwin` が aarch64 のもの、FreeBSD・illumos などが x86_64 Linux のもの。2026-10-15 に確認) ので、R-TG-2 と同じく実際の triple を読み、要求と違えば warnings で知らせる (M1 はアーカイブのファイル名から読む。v0.19)
  - M1 で決めた細部 (m1-plan.md の B8。v0.19): xz は純 Rust の lzma-rust2 で展開する (利用者が決めた)。アーカイブの URL は dist のベース URL の下に限る。アーカイブの中の名前からは書き込み先を作らず、std・core・alloc・proc_macro・test だけを書く。展開の上限は合計 2 GiB・1 ファイル 512 MiB (初期値)。展開先は `std/<短いコミットハッシュ>/<target>/<crate>.json` (内容ダイジェスト付き) で、最後に完了印 `complete.json` (日付・版・完全なコミットハッシュ・target) を書く。x86_64 Linux のアーカイブは 4.2MB、展開後は 125MB (core.json が 96MB)
  - **読める nightly へのさかのぼり** (v0.19。公開前の見直しで足した): std は常に最新の nightly を使うので、nightly の形式が上がった日に、公開済みの版のサーバーの std の照会が壊れうる。そこで、最新の nightly の `std.json` の形式 (末尾から先読みする) が対応範囲の外なら、前に覚えた日付と、1〜30 日前の日付付きの manifest を順に試し、読める nightly を使って warnings で知らせ、見つけた日付を覚える
  - 古い nightly の std の片付け (v0.19。R-CA-5 の GC のうち std の分を先に入れた): nightly ごとに約 175MB 増えるので、std を選ぶたびにその nightly のディレクトリの使用時刻を更新し (同じディレクトリはプロセスごとに 1 時間に 1 回)、裏のスレッドで 7 日使われていないディレクトリを消す。消すのは完了印かアーカイブのあるディレクトリだけで、symlink はたどらない。消せないもの (Windows でほかのプロセスが mmap しているもの) は残して続ける。M2 では、この 7 日の規則を GC (R-CA-5) の部品に移し、std を読んだときにも同じ部品を呼ぶ。使ったことの印はその nightly のディレクトリのロックの下で書き、片付けは候補ごとにロックを待たずに試して、取れなければ使用中として残す (別のプロセスが使い始めた nightly を消さないため。m2-plan.md の C2c・F1。v0.20)
  - 手元の nightly に `rust-docs-json` があればそちらを優先する (版が古いときの扱いは R-STD-1)。どちらを使ったかも provenance に載せる
  - std のソース表示に使う `rust-src` も、手元に合うものが無ければ同じ dist から取る (R-SC-2。v0.18)
  - 既定で有効とし、オフラインモード (R-CA-7) と設定 (`std.fetch_from_dist`。R-STD-1。v0.20) で無効にできる。R-WS-8 の既定 (プロジェクトの toolchain + `RUSTC_BOOTSTRAP`) と組み合わせると、**nightly を一切入れずに**依存・workspace・std を引ける
  - 任意 (M4 で検討。v0.16): ユーザーの stable の版に近い nightly の `rust-docs-json` を選べるようにする (unstable な API の混入を減らす)
  - manifest の日付が変わったときだけアーカイブを取り直す。manifest の取得には TTL (1h) を付ける。取り直せなければ、古い manifest を warnings 付きで使う。日付を指定した nightly の manifest (`/dist/<日付>/`) は変わらないので一度だけ取る (M1。v0.19)

### 5.12 API 差分 (`api_diff`)

方式は cargo-public-api と cargo-semver-checks の分析で決めた (Q-6)。

- R-AD-1 基本層 (cargo-public-api のアルゴリズム):
  - **import できる公開パスごとに 1 項目**。re-export は inline する
  - 項目は正規化したトークン列で比べる。引数名は比較から外す (名前だけの変更は R-AD-2 の `cosmetic` に分ける)
  - impl の項目には、上書きされていない default メソッドも含める (上書きの有無が追加・削除に見えるのを防ぐ)
  - 多重集合 (bag) として差を取り、同じパスの削除と追加を `changed` に組にする
  - 並びは決定的にする
- R-AD-2 分類層: cargo-semver-checks の lint に倣った少数の構造的な分類器で、各項目に変化の種類と semver 上の重さを付ける。分類器の一覧は M4 で決める (v0.16)
  - 初期案: Major は `removed`、`param_count_changed`、`generics_changed`、`now_async`、`now_unsafe`、`const_removed`、`became_non_exhaustive`、`auto_trait_impl_removed`、`trait_method_added_required`、`default_impl_removed`。Minor は `deprecated`、`must_use_changed`、`added`
  - 影響なし: `cosmetic` (引数名だけの変更。Rust に名前付き引数は無いので呼び出し側は壊れないが、`len` → `capacity` のように意味の変化の手がかりになる)
  - 分類できない変化は重さを `unknown` にする (推測で Major / Minor を付けない)。cargo-semver-checks 自身も引数の型の変化は検出しない
  - trustfall_rustdoc には依存しない (対応形式が 3 世代だけで、hidden item 込みの JSON を前提にするため)
- R-AD-3 **docs.rs の JSON だけで見た「削除」は信用しない**。docs.rs の JSON は `#[doc(hidden)]` の item を含まないので、deprecated かつ hidden になっただけの item が消えたように見える (tokio の `TcpSocket::tos` で確認済み)。両側が `--document-hidden-items` 付きのローカルビルドでない限り `removed_or_hidden` (確度 低) として返す。hidden 付きのビルドは、両側とも外のツール (`build_external_docs` の `document_hidden_items`。R-WS-10) が作り、`api_diff` 自身は作らない (v0.18)。鍵の rustdoc の引数が変わるので、通常の照会のキャッシュとは別になる (R-WS-4)。比較相手の版が Cargo.lock に無く docs.rs に JSON も無ければ、`docs_unavailable` を返して `build_external_docs` (`document_hidden_items`) を案内する
- R-AD-4 正規化と provenance:
  - 両側の provenance を返す。format_version・toolchain・target・feature の組 (docs.rs のものも、ローカルビルドのものも。プロジェクトで統合された組と、仮のプロジェクトの組は違いうる) が違えば warnings を出す (v0.17)
  - 両側の feature の組が違うとき、片側の組でしか有効にならない item の `removed`・`added` は確度を下げ、Major の判定 (R-AD-2) に使わない (v0.18)
  - toolchain の違いによる見かけの差 (片側にだけある不安定 auto trait、重複した blanket impl) は既定で除く
  - docs.rs のビルドでだけ現れる item (`docsrs` cfg 由来) は、差分に残して印 (`docsrs_only` など) を付ける
- R-AD-5 応答の形: **変わらなかった項目は返さない**。各項目に確度 (`confidence`。`removed_or_hidden` は low) を付ける。ページングは R-OUT-3 に従う。節の並び (案: 件数の要約 → removed → changed → deprecated → added → cosmetic) と、cosmetic や変化の無い blanket impl を既定で省くかは M4 で決める (v0.16)

### 5.13 脆弱性情報 (`audit_dependencies`)

- R-AU-1: プロジェクトの Cargo.lock にある registry のパッケージについて、RustSec / OSV の advisory を**解決されたバージョンに一致するものだけ**返す。「そのパッケージのどれかの版に過去 advisory があった」で判定しない (誤検出の先例がある)
- R-AU-2: advisory の種類 (vulnerability / unmaintained / unsound) と withdrawn を区別する。path / git の依存は「対象外」として示す
- R-AU-3: データは OSV の `crates.io/all.zip` (約 3.6MB、毎日更新。2026-10-15 に確認) のオフラインミラーを持ち、1 日 1 回、条件付きのリクエストで取り直す (TTL 24 時間。文書のキャッシュの容量の枠に入れる。R-CA-5)。`list_crates` と `crate_metadata` にも件数を添え、advisory のある版の item を `get_item` で返すときは warnings を付ける

### 5.14 Rust のリリース情報 (`rust_release`)

- R-RR-1: Rust の `RELEASES.md` (GitHub API の contents で `rust-lang/rust` のものを raw で取る。ETag 付きの条件付きリクエストにし、TTL は 24 時間。R-CA-8 を実装した場合は、ミラーと無効化は GitHub API の設定に従う。rate limit の扱いは R-NW-2) を取り込み、1 つの版、最新版、または指定した版より新しいすべての版の要点を返す。既定のブランチの `RELEASES.md` には、リリース日より前に次の版が載りうるので、リリース日が今日以前の版だけを返す (v0.17)。目的は「**学習データより新しい Rust の変更を LLM が追いつけるようにする**」こと
- R-RR-2: R-STD-3 の警告の引用元としても使う

---

## 6. MCP インターフェース

### 6.1 原則

- R-MCP-1 トランスポート: **v1 は stdio だけを実装する**。Streamable HTTP は、ホスト版か共有キャッシュサーバー (§9.10) を作る段階 (M6 以降) まで実装しない。実装するときも任意の機能とし、自分用の常駐プロセスとして使う場合は localhost に bind する (R-SEC-5)
  - 理由: ローカルで HTTP の利点が最も効くのは、同時に複数のセッションを使うときに 1 プロセスを共有できることだが、ディスクキャッシュはもともとプロジェクトをまたいで共有され (R-CA-1)、mmap したアーカイブのページは OS のページキャッシュでプロセス間でも共有される (R-PF-1)。HTTP で上乗せされるのは、各プロセスの匿名メモリ (解析後の構造体・逆引き索引)、`cargo metadata` の重複、プロセスの起動コストの節約だけで、次の負担に見合わない
  - HTTP で新たに要るもの: プロジェクトの文脈 (R-SEC-5)、常駐プロセスの起動と停止、更新後に古いプロセスが残る版の食い違い、ポートの衝突、複数のクライアントにまたがるメモリの管理と公平性 (R-MEM-1)、SSE で進捗を送る経路とクライアントの切断をビルドのキャンセルにつなぐ処理 (R-WS-6)、トランスポートごとのテスト
  - Claude Code の時間制限もトランスポートで違う。stdio はアイドル 30 分 (進捗通知でリセット) で、リクエストごとのタイマーが無い。HTTP はアイドル 5 分で、最初の応答バイトまでのタイマー (60 秒以上) がある。R-WS-6 (ビルドは既定 15 秒 (M2 の F5 で測って 10 秒から改めた。v0.20)、それ以外は R-ARCH-6 の呼び出しの期限 (既定 30 秒) の 3 分の 2 で `status: building` を返す。v0.18。v0.20) はどちらにも収まるように作る
  - 後から HTTP を安く足せるよう、R-MCP-3 と R-ARCH-5 のテストを v1 から持つ
- R-MCP-2 対象とする仕様と追従の方針 (Q-2):
  - **MCP の最新版だけに対応する**。v1 の時点では **2026-07-28** だけで、2025-11-25 以前の `initialize` の handshake には対応しない。Claude Code の v2 ランタイム (MCP TypeScript SDK 2.0) は、サーバーが 2026-07-28 に対応しているかを尋ね、対応していなければ従来の handshake に戻る。v2 ランタイムを既定で使うのは、feature flag を取得するセッションでは v2.1.232 以降、それ以外のセッション (Bedrock、テレメトリ無効など) では v2.1.274 以降 (Claude Code の MCP の文書)。ただし stdio サーバーとの 2026-07-28 の交渉が全環境で既定になったのは v2.1.292 (CHANGELOG。v2.1.274 の変更は直接接続の HTTP サーバーが対象)。README に書く Claude Code の最低版は v2.1.292 とする。最初の判定への応答が遅いと、7 日間は従来の方式で接続される (v2.1.292 の CHANGELOG) ので、`server/discover` には即座に応える (2026-10-15 に確認)
  - **新しい版が出たら追従する**。追従と、古い版を外すことはリリースノートに書く。2 つの版をどう並べるかは、次の版が出たときに決める (v0.16)
  - **古い版のクライアント**: stdio の `initialize` には、必須の `_meta` の検査より先にメソッド名で判定して、版の交渉 (下記) と同じ `-32022` を返す (`data.requested` は `initialize` の `protocolVersion`)。`message` に対応する版と、Claude Code なら必要な版を書く (`basic/versioning` の SHOULD。古い版のクライアントには版を上げる仕組みが無く、このエラーが利用者に見える唯一の手がかりになる)。繋がらないのは、Claude Code の古い版、`MCP_PROTOCOL_NEGOTIATION=legacy` / `MCP_SDK_GENERATION=v1` で従来の版に戻した環境、2025-11-25 以前しか話さないクライアント (2026-10-14 時点の Gemini CLI と Codex CLI の既定を含む)。README に、これらが繋がらないことも書く。あわせて、最初の判定への応答が遅れると Claude Code が 7 日間は従来の方式に戻り、このサーバーにつながらないことも README に書く (戻し方は M5 で確かめる。v0.18)
  - **stdio を直接使えないクライアント** (ChatGPT などリモートの MCP だけを使うもの): 利用者が stdio を中継するトンネルを自分で用意する前提とし、サーバーでは個別に対応しない (M6 の HTTP は共有のための機能で、この目的のものではない。R-MCP-1)。トンネルを通しても、クライアントが最新版を話せなければ繋がらないことを README に書く
  - **版の交渉** (`basic/versioning`): 要求の `_meta["io.modelcontextprotocol/protocolVersion"]` が対応外の版なら、`UnsupportedProtocolVersionError` (`-32022`) を返し、`data.supported` に対応する版 (v1 では `["2026-07-28"]`)、`data.requested` に要求された版を入れる
  - 要求は状態なしで処理し、サーバーから JSON-RPC 要求を送らない (MRTR も使わない。R-SEC-2)。2026-07-28 で削除された `ping`・`logging/setLevel`・`notifications/roots/list_changed` には応じない (要求なら未知のメソッドとして `-32601`)
  - 必須の `_meta` (`protocolVersion`、`clientCapabilities`) を欠く要求 (`initialize` を除く) は `-32602` で拒否する (`basic/index`)
  - この項目の挙動は、SDK に任せる部分も結合テストで確かめる
  - rmcp 3.5.1 の挙動で、この要件と違うが受け入れるもの (ハンドラからは変えられない。結合テストで固定し、rmcp が変われば気付けるようにする。M0 で確かめた。v0.17): セッション開始前の `_meta` の無い `ping` に空の結果を返す (`-32601` ではない)。`arguments` がオブジェクトでない `tools/call` と、`name` の無い `tools/call` (M1 で確かめた。v0.19) に `-32601` を返す (R-ARG-1 は `-32602`)。`initialize` を拒んだ後にプロセスが終わる (古いクライアントは拒まれた後に続けられないので実害は無い)。`_meta` で対応外の版を名乗った要求への `-32022` の `message` は rmcp の固定の文言 (独自の `message` を求めるのは `initialize` だけ)
- R-MCP-3 **セッション状態に頼らない**:
  - 対象のプロジェクトは、ツール引数 `project` (`Cargo.toml` へのパスかディレクトリ。相対パスなら既定のプロジェクトのディレクトリから解く。v0.19) で渡せる。`CLAUDE_PROJECT_DIR` の無い起動のしかたもあるので、**`project` 引数の経路を主経路の 1 つとしてテストする** (環境変数を消した状態の結合テストを持つ)
  - 省略したら次の順で決める (こうして決まるプロジェクトを既定のプロジェクトと呼ぶ): 環境変数 `CLAUDE_PROJECT_DIR` (空なら無いものとする。v0.19) → プロセスのカレントディレクトリ。どれを使ったかを provenance の `project_from` に載せる (`roots/list` の扱いは下記)。worktree (`claude -w`・subagent の worktree) と `--add-dir` で、サーバーが受け取る値と worktree の置き場所は、M2 の前に確かめた (Q-32。v0.18。下記。v0.20)
    - 実測 (Claude Code 2.1.304、Linux。m2-plan.md の §6.1。v0.20): 通常の起動では、サーバーのカレントディレクトリと `CLAUDE_PROJECT_DIR` はどちらも Claude Code を起動したディレクトリ (git のルートではなく、サブディレクトリで起動したならサブディレクトリ)。`claude --worktree <name>` では、どちらも worktree のルート (`<git のルート>/.claude/worktrees/<name>`)。subagent の worktree (`isolation: worktree`) は `<git のルート>/.claude/worktrees/agent-<id>` に作られるが、subagent は親と同じサーバーのプロセスを使い、サーバーからは見えない (`roots/list` も親のもので、変更の知らせも来ない)。`--add-dir` のディレクトリは、環境変数にもカレントディレクトリにも出ず、`roots/list` にだけ出る (旧来の `initialize` の接続で測った)
    - 決めたこと (m2-plan.md の §5 の P3〜P5 と A2。v0.20): 同じ git リポジトリの別の作業ツリーの同じ場所 (作業ツリーのルートからの相対パスが同じ) にある workspace は、既定のプロジェクトと同じに扱う (同じリポジトリのコードで、subagent はそこで Bash から cargo を実行できるため。linked worktree から、メインの作業ツリーを指した場合も含める)。判定はファイルを読むだけで行い、git を起動しない: 作業ツリーは上へたどって最初の `.git` で決め、`.git` がファイルなら、`gitdir:` が指す記録の `gitdir` がその `.git` を指し返し、`commondir` が共有の git ディレクトリを指すときだけ認める (写した `.git` や手で書いたものでは加わらない)。instructions と各ツールの `project` の説明で、worktree で作業しているなら `project` に作業ディレクトリを渡すよう案内する。`--add-dir` のディレクトリは `project` で指す (`roots/list` は使わない。下記)。入れ子の worktree (`.claude/worktrees/`) が `members` の glob にも cargo の workspace の探索にも新しい失敗を足さないことを確かめた (`members = ["*"]` は worktree が無くても `.git` に当たって失敗する)
  - **プロジェクトの単位は Cargo の workspace** (workspace に属さない単独の package はその package): `project` に member のディレクトリや Cargo.toml を渡しても、それが属する workspace の全体を 1 つのプロジェクトとして扱う。専用 target dir (R-WS-2) は、この単位で決める。workspace のルートは、Cargo.toml を上へ辿り、`package.workspace` と `[workspace]` の `members` / `exclude` を Cargo の規則どおりデータとして読んで決める (member でない上位の workspace は拾わない)。M1 で `cargo metadata --no-deps` と突き合わせて決めた規則: `members` の直書きのパスは `exclude` より優先し、パターンは `exclude` に負ける。パターンは `*`・`?`・`**` を扱い、`[...]` は文字どおりに読む (v0.19)。path 依存で暗黙に member になるもの (ルートの package の path 依存など) は、M1 では単独の package と判定する (v0.19)。M2 では、Cargo.lock が無いときは `cargo metadata` を起動し、その `workspace_root` が上にあれば、cargo の答えのルートの Cargo.lock と入力のファイルを使う。`project` で既定のプロジェクトの外から指した暗黙の member は、cargo を起動しない規則 (下記) のまま外の扱い (m2-plan.md の B4。v0.20)
  - 決めたディレクトリに `Cargo.toml` が無ければ、すぐには上方向に探さない (無関係な上位の workspace を拾わないため。git の作業ツリーの中でだけ、直下の子にも無いときに上へ探す。下記。v0.20)。代わりに、**直下の子のディレクトリのうち Cargo.toml を持つ workspace を、どれも既定のプロジェクトとして扱う** (直下の子を探すときは symlink をたどらない。v0.19。rust-analyzer が、開いたフォルダの直下に Cargo.toml が無いときに読み込む範囲と同じ。2026-10-16 に確かめた。v0.14)。照会ではそのどれかを `project` で選び、省略したら、候補が 1 つならそれを使い、複数なら `project_not_found` に候補を添えて返す。ただし候補が複数で `project` が無いときも、std の照会 (どの子の toolchain も使わなかった旨を warnings に書く。R-STD-3) と、版を明示した docs.rs の照会は続ける。版を省いた docs.rs の照会は `project_not_found` にする (プロジェクトの版がありうるのに黙って latest を使わない。G1。v0.16)。直下の子は起動時に一度だけ数え、セッションの途中で増えた子 (エージェントが clone したものなど) は既定のプロジェクトにしない (確認なしにビルドする範囲が勝手に広がらないように。`project` で指せば外のツールの扱いになる。v0.15)。直下の子にも無ければ、git の作業ツリーの中なら、作業ツリーのルートまで上へたどり、最初の Cargo.toml の workspace を既定のプロジェクトにする (`src/` などのサブディレクトリで Claude Code を開いたとき。直下の子の workspace を探すときも作業ツリーのルートまで上を見るので、`crates/` で開くと `crates/a` を単独の package でなく、リポジトリの workspace の member として扱う。git の外では上へ探さない。利用者が決めた。m2-plan.md の A2。v0.20)。それでも無ければ、プロジェクトを要する処理は `project_not_found` (§8) を返す (docs.rs と std の照会は、プロジェクトの文脈なしで続ける。R-ARCH-3)。2 段以上深い workspace は既定のプロジェクトにならないので、suggestions に、その workspace のディレクトリで Claude Code を開くか `build_external_docs` で指す手段を添える。README にも同じことを書く。`cargo metadata` の起動の仕方は R-WS-2
  - **照会のツールが cargo を起動しビルドするのは、既定のプロジェクトだけ** (v0.14 で決定): 既定のプロジェクト (同じ git リポジトリの別の作業ツリーの同じ場所の workspace を含む (上記。v0.20)。workspace の全体と、実際のビルド target (`.cargo/config.toml` の `build.target`、無ければホストの target。配列ならそのすべてで、`target` を省いた照会は先頭の要素。R-TG-1) で `cargo check --all-targets` がビルドするその依存。rust-analyzer の既定と同じ範囲で、dev-dependency を含み、他の target 向けの依存を含まない。他の target 向けの依存は、rust-analyzer も利用者の `cargo check` もビルドしないので、このサーバーだけがその build.rs を動かす経路にしない) では、確認なしに cargo を起動し、ローカルビルドをする。それ以外 (R-WS-10 の対象) のコードを動かすのは外のツールだけで、照会のツールはそこで作ったキャッシュを読む。どちらのツールでも、このサーバー自身は確認を求めない。外のツールに確認を出すかは、利用者が Claude Code の許可ルールで決める。理由と README に書くことは R-SEC-2
    - 範囲の判定は、実際のビルド target ごとに `cargo tree --workspace --target <target> -e normal,build,dev` で得た crate の集合で行う (配列ならその和集合)。cargo は `cargo rustdoc -p` で名指しされた依存を、範囲の外でも作ろうとしうるので、サーバーが判定してから起動する (resolver 1 の workspace では、他の target 用の依存を別の target で作ったり、host 用の build-dependency を build target で作ったりして、どちらの集合にも無い依存の build.rs が動いた。resolver 2 以上では、member から target 向けの通常の辺でたどれない依存 (dev だけ・build だけ・ほかの target だけ・proc-macro の下だけ) を名指しすると cargo が panic する (M0 で確かめた。扱いは R-WS-5。Q-30。v0.17)。「他の target 用の依存もホストの target のまま文書化する」は一般には成り立たない)。`cargo tree` の起動は target ごとに 2 回 (target 用と host 用の 2 つの根に出る member があれば 3 回)。出力の読み方は R-FT-3、結果は `cargo metadata` の結果と一緒に target ごとに持ち、同時の呼び出しで合流し、失敗は入力が同じ間 30 秒覚える (R-VR-2。m2-plan.md の B4・B7。v0.20)
    - **ローカルビルドしてよい組** (v0.15): 照会のツールがローカルビルドしてよいのは、(package, target) の組が、cargo が `cargo check --all-targets` でその target のために作る単位に含まれるときだけにする。target 用の依存 (通常の依存と dev-dependency) は、要求した target の集合に target 用として現れるときにその target で作り、build-dependency と proc-macro は host 用として現れるときに host で作る。種類 (host 用か target 用か) を判定できないときはローカルビルドせず、docs.rs か外のツールの記録を使う。種類は stable の `cargo tree` で判定する: `-e build` の辺の先と、`(proc-macro)` の印が付いた node の下にある依存が host 用、それ以外が target 用 (`-e normal` の木には proc-macro の依存も出るので、木の上の位置で判断する)。`cargo tree` は package id を出さない (`{p}` は名前・版と、path・git の取得元だけ) ので、名前・版・取得元から `cargo metadata` の package id に対応付ける。nightly の `--unit-graph` (unit ごとの package id・platform・feature を出す) は、nightly と `-Z` が要るので使わない (Q-30。M0 で決定。v0.17)。ただし resolver 2 以上で `-p` で名指しできない依存 (上記。R-WS-5) は、照会のツールではローカルビルドせずに `build_unsupported` を返す (外のツールが仮のプロジェクトで作れる。R-WS-5。v0.17)。`cargo tree` の出力に解析できない行があれば、範囲を判定できないものとして扱い (`scope_unknown`)、依存をローカルビルドしない (範囲の判定は、どの build.rs を動かすかを決めるため。v0.17)
    - **feature の組** (v0.15): 範囲は通常の feature の組で判定する。member の文書化の組 (`[package.metadata.docs.rs]` の `all-features`・`features`。R-FT-3) で通常の組の範囲に無い依存が増えても、照会のツールはその依存を確認なしにビルドしない (増えた optional の依存の build.rs を動かさないため。v0.16)。そのときは通常の組で作り、外のツールに渡す引数 (`crate`) を warnings で案内する。docs.rs の組での文書は外のツールで作れ (R-WS-10)、照会のツールはその記録があれば優先して読む (記録した入力が変わっていれば `inputs_changed`)。`enabled_in_project` は通常の組のまま (R-FT-3。m2-plan.md の D3e と M2e の決定 10・E4c。v0.20)
    - `cargo metadata --filter-platform` は build-dependencies の cfg を build target で評価してしまい、ホストで評価する実際のビルドと食い違うので使わない (2026-10-16 に確認)。版の解決 (R-VR-2) には絞らない `cargo metadata` を使う。範囲を判定できないとき (オフラインで Cargo.lock を直接読む経路など。R-VR-2) は、依存のローカルビルドをしない。`cargo tree` の集合は、`cargo check --all-targets --target <target>` がビルドする crate の集合と一致した (2026-10-16、cargo 1.99.1。ホストと `x86_64-pc-windows-gnu` の 2 つで、通常の依存・target 別の依存・proc-macro とその依存・dev-dependency と target 別の dev-dependency・build-dependency と target 別の build-dependency・依存の build-dependency・使わない optional の依存を持つ workspace で確かめた)。`cargo tree` の出力の形は、stable の cargo 1.99.1 と nightly の cargo 1.101.0 で同一だった (2026-10-19。`-e all`、feature 付き)。形が崩れたら nightly の `--unit-graph` を検討する (v0.15。v0.17)
    - 照会のツールに、実際のビルド target (配列ならそのどれか) と違う `target` が渡されたら、既定のプロジェクトの中でもローカルビルドせず、docs.rs → 外のツールがその target で作った記録 (R-WS-10) の順に探す。std は R-STD-6 の配布物からその target の JSON を取る。どれにも無ければ `docs_unavailable` で外のツールを案内する (`target` 引数で他の target 向けの依存の build.rs を確認なしに動かす抜け道を作らず、他の target の文書も外のツールで作れば見られるようにするため。v0.15)
      - M2 の形 (m2-plan.md の D7b-1・E4a-3。v0.20): この判定は member にも当てる。どれにも無ければ `docs_unavailable` (`reason: no_json`・`retryable: false`) に、実際のビルド target の一覧と、外のツールに渡す引数 (`{"crate": "<package>@<版>", "target": …}`) を添える。host を明示したときは、実際のビルド target (先頭) の範囲で host 用の依存 (build-dependency・proc-macro とその下) だけをローカルビルドする。proc-macro の member は、他の target でもプロジェクトの先頭の target で計画し、host のデータを warnings 付きで返す。外のツールの記録があれば読み、記録した入力が変わっていれば、古い記録に `stale: inputs_changed` を付けて返す (R-WS-9)。外の記録を読む応答では `refresh` を効かせず、その旨を warnings に書く
    - **照会のツールは、既定のプロジェクトの外で cargo・rustc・rustup を一切起動しない** (外のツールに分けた意味を保つため。R-QA-5 の結合テストで固定する)。外のプロジェクトでは、`build_external_docs` が記録したもの (R-WS-10) だけを使う: 版の解決には記録した `cargo metadata` の結果、鮮度の判定には記録した入力ファイル (R-WS-4 の入力) の内容ダイジェスト、toolchain には記録したときのもの (下記)。ソースの読み取り (R-SC-4) はファイルを読むだけなので、記録が無くても行う。記録の後の照会 (crate なしの検索を含む) でも外で何も起動しないことを、起動されたら印を残す罠を置いた結合テストで固定した (m2-plan.md の E4b-3。v0.20)
    - 外の記録の鍵は、照会のツールが組み立てられるものにする (R-WS-4 の鍵には `rustc -vV` が入り、照会のツールは組み立てられないため)。プロジェクトの依存はそのプロジェクトの feature の組と source で作るので、別のプロジェクトや仮のプロジェクトの記録と混ぜない。`document_hidden_items` 付きの記録は `api_diff` だけが使う (R-AD-3)。鮮度の判定では toolchain の更新を見ない。provenance に記録したときの toolchain を出し、toolchain の更新を検知していない旨を warnings に書く。M2 で決めた形 (m2-plan.md の E1b〜E4b。v0.20): 鍵の組は R-WS-10 の種類のとおり。照合は、記録した入力の内容ダイジェスト (前に確かめたときとファイルの長さか時刻が違うものだけ hash し直す) と `members` の glob で行う。workspace の入力 (Cargo.toml・Cargo.lock・設定・glob の member) が記録の後に変わったら、古い記録で答えて `stale: true`・`stale_reason: inputs_changed` と外のツールの呼び直しの案内を付ける (docs.rs で引く依存・`list_crates`・`resolve_version` の応答にも)。crate なしの検索では、workspace の変化を最上位の provenance に、crate 自身の入力の変化をその crate の `sources` の項目に付ける。`target` を省いた照会は記録した target の先頭を使う。記録の無い target の照会と crate なしの検索は、全体を `no_record` (外のツールに渡す `project`・`target` を案内する) か、作成中なら `status: building` にする
      - プロジェクトのディレクトリが消えたとき (m2-plan.md の E4b-3。v0.20): root の Cargo.toml が無ければ、記録した別名 (渡したパス・root・member のディレクトリ・渡したパスが root を指すときはその下の member。`Cargo.toml` で終わるパスはそのディレクトリでも引く) から記録のある workspace を引き、記録で答えて `stale: project_missing` と、移した先のパスで `build_external_docs` を呼ぶ案内を付ける (照会のツールすべて)。member のディレクトリだけが消えて root が残るときは `inputs_changed`。Cargo.toml が壊れているときは別名に落ちず、理由付きの `project_not_found`。別名はあるが記録の無いパスと、別名も無いパスは `project_not_found`。移した先の新しいパスは、前の場所の記録と結び付けず別の workspace として扱う (`no_record`)。外のツールは消えたパスを `project_not_found` で断る
    - 記録が無ければ `docs_unavailable` を返して `build_external_docs` を案内する (同じ鍵のビルドが進行中なら `status: building`。R-WS-10)。記録はあるが鮮度が切れていれば、古いものに `stale: true` と warnings を付けて返し、hints で `build_external_docs` を案内する (読むだけならコードは動かない)
    - git の crate は、照会のツールでも `git` と `rev`・`branch`・`tag` で指せる (読み取りの鍵としてだけ受け、取得もビルドもしない。記録が無ければ `docs_unavailable`)。`branch`・`tag`・`refs/…` で指したときは、記録した解決が古くなりうることを示し、hints で外のツールの呼び直しを案内する (v0.16)。M2 で決めた形 (m2-plan.md の M2e の決定 7・E3b-2。v0.20): ref は外のツールの解決の記録で引く。解決から 1 日を過ぎたら (`resolved_at` が読めないか、時計より 5 分以上先のときも)、記録に `stale: true`・`stale_reason: ref_ttl_expired` と warnings、外のツールの呼び直しの案内を付けて返す (`retry_after_secs` は付けない)。期限内でも `resolved_at` と、解決し直すには外のツールを呼ぶことを hints に書く。外のツールが解決し直している最中は、前の解決の commit のデータを返す (stale にしない)。解決し直して ref が新しい commit に動き、その作成中なら、前の commit のデータに `stale: rebuilding` と `retry_after_secs` を付け、作成が失敗したらその失敗を返す。`target` を省けば host。`refresh` は効かせず、その旨を warnings に書く (外の記録は照会のツールで取り直さない。R-WS-10)。短いハッシュの照合は §6.4
  - Claude Code (CLI) が stdio サーバーをどのディレクトリで起動するかは文書化されていない。文書化されているのは `CLAUDE_PROJECT_DIR` を渡すことだけ (Agent SDK の TypeScript 版には `projectConfigRoot` で起動ディレクトリを決める記述がある)
  - **`roots/list` は使わない**: Roots は 2026-07-28 で deprecated になり、新しい実装は対応を足さない (SHOULD NOT)。サーバーから送れるのも MRTR (`InputRequiredResult` を返して再試行させる往復) の中だけになった。仕様は代わりに「ディレクトリはツールの引数や設定で渡せ」としており、`project` 引数を持つ本方針と一致する
  - `set_working_directory` や `register_project` のような方式は採らない (先行実装では、設定前の呼び出しがすべて失敗する)
- R-MCP-4 annotations (v0.9 で決定。基準を v0.13 で改め、分類を v0.15 で確定):
  - **cargo・rustc を起動しうるツール** (§6.3 のうち `search_crates`・`rust_release` 以外のすべて) には、`readOnlyHint: false`、`destructiveHint: false`、`idempotentHint: true` を付ける (`build_external_docs` だけは、同じ引数で呼び直すと取り直すので `idempotentHint: false`。R-WS-10)。build.rs と proc-macro は任意のコードで、`cargo metadata` も依存を取得してディスクに書き、プロジェクトの設定 (`rustc-wrapper` など) で任意の実行ファイルを動かしうる (R-SEC-2) ので、「環境を変更しない」という定義を保証できないため (v0.13 で基準を「ビルドを起こしうるか」から改めた)
    - ローカルビルドを起こしうるツール (キャッシュに無い workspace の item や、docs.rs に JSON が無い依存を引きうる `crate_overview`・`search_items`・`get_item`・`list_*`・`get_source`・`inspect_features`・`api_diff`、`build_external_docs` など) に加え、ビルドはしないが `cargo metadata` や `rustc -vV` を起動するもの (版を省くと Cargo.lock から版を決める `list_crates`・`resolve_version`・`audit_dependencies`・`crate_metadata`・`readme`・`changelog`・`find_examples` など、std の照会 (R-STD-3 の `rustc -vV`)) を含む
  - **cargo・rustc を起動しないツール** (`search_crates`・`rust_release` の 2 つ) には `readOnlyHint: true` を付ける。ローカル機能を含まない構成 (R-ARCH-2) では、どのツールも cargo を起動しないので、すべて `readOnlyHint: true` にする
  - docs.rs や crates.io に出うるツールには、オフラインモード (R-CA-7) でも `openWorldHint: true` を付ける
  - 採らなかった案: ビルドしうるツールにも `readOnlyHint: true` を付ける案 (照会の感覚に合い、annotations で自動承認するクライアントで確認が出ない) は、定義に反するので採らない。annotations を権限確認に使うクライアントでは照会のたびに確認が出うるが、仕様への正直さを優先する
  - Claude Code の MCP の文書には readOnlyHint を使う記述が無い。ただし実際には、`readOnlyHint: true` のツールは並列に、`false` のツールは 1 つずつ順に実行される (Agent SDK の文書の記述どおり。CLI 2.1.301 で、1 つのメッセージで 2 つ呼ばせて 2026-10-16 に確かめた)。照会のツールが順に実行されることは受け入れる (キャッシュに当たる照会は数十ミリ秒で、差が出るのは docs.rs から初めて取る crate が複数ある場合くらい。同じ target dir のビルドはもともと cargo のロックで 1 つずつになる。v0.15 で、並列のために `true` を付ける案を改めて退けた)。ただし、ビルドを待つ照会は、同じメッセージの他の呼び出しを待つ間止める。このため、ビルドを待つ上限を短くした (R-WS-6。v0.18。M2 で測って 15 秒にした。v0.20)。README に書く許可ルールは R-SEC-2
  - 各ツールが実際に cargo・rustc を起動しうるかと annotations の一致を CI で検査する (R-QA-5)。rustdoc-mcp は、`cargo doc` で書き込むのに `readOnlyHint: true` を付けている
- R-MCP-5: `tools/list` は毎回同じ順で返す。2026-07-28 で必須になった `ttlMs` (目安 1 時間) と `cacheScope` を、仕様がキャッシュ可能とする結果 (`server/discover`、`tools/list`。R-MCP-13・R-MCP-18 を入れたときのものは M5 で決める。v0.16) に付ける (`server/utilities/caching`)。`tools/call` の結果には付けない (プロジェクトに依存するかどうかは provenance で示す)。`server/discover`・`tools/list` は `"public"`。`tools.profile` と `output.mode` は利用者の設定で起動時に決まり、1 つのプロセスの中ではどの呼び出しにも同じ一覧を返すので、public に合う。Claude Code がセッションをまたいで一覧を保存するのは discovery cache だけで、対象はリモートの HTTP / SSE サーバー、既定では無効、エントリはサーバー単位、有効期限は Claude Code 側の設定 (既定 15 分 / 最大 4 時間) で決まり、`ttlMs` と `cacheScope` を見るという記述は無い (Claude Code の MCP の文書と環境変数の文書で確認。2026-10-15)。stdio のサーバーは毎回一覧を取り直されるので、プロジェクトごとの違いは問題にならない。`serverInfo.version` は `env!("CARGO_PKG_VERSION")` から取る (固定の文字列にしない)。serverInfo は表示とログのためのもので、クライアントがキャッシュを捨てる判断は `ttlMs` と再起動に任せる (仕様は serverInfo で挙動を変えないことを求めている)。`tools/list` はページに分けない (`nextCursor` を返さず、`cursor` 付きの要求は無効なカーソルとして `-32602`)。設定 (`tools.profile`、`output.mode` による `outputSchema` の有無) を変えて一覧が変わる場合は、サーバーの再起動で反映する (README に書く)
  - 全結果に `resultType` を付け (このサーバーは `"complete"` だけを返す。R-SEC-2)、結果の `_meta` に `io.modelcontextprotocol/serverInfo` を載せる (SDK が対応していればそれに任せる)
- R-MCP-6: サーバーの `instructions` には、何を文書にするか、呼び出し順の連鎖 (`list_crates` / `search_items` → `get_item` → `list_methods` / `list_impls`)、バージョンと target の規則、応答の状態 (`status: building`・`partial`・`stale`) の扱い、`build_external_docs` を先に呼ぶ場合、LSP との境界 (§9.9) だけを書く (v0.20 で、M2 の F4a の決定に合わせて足した)。問いとツールの対応は各ツールの説明文の冒頭に置く (instructions に問いの一覧を詰め込んだ先例は、後で削られている)
  - M1 の instructions (1,139 文字。m1-plan.md の D4a。v0.19): 呼び出しの順 (探す: `list_crates`・`crate_overview`・`list_module`・`search_items` → `get_item` → `list_methods`・`list_impls`・`list_implementors`。前もって呼ぶものは無い)、版の規則 (Cargo.lock → latest、`provenance` を見る、`resolve_version` は先に要らない)、target と std の nightly (`stability` を見る)、この workspace 自身のコードは rust-analyzer に任せること。ローカル機能の無い構成では `list_crates` を挙げず、版は latest になるので Cargo.lock の版を渡すよう書く
  - M2 の instructions (m2-plan.md の F4a-1。v0.20): 1 文目は何を文書にするか (この workspace の crate はここでビルドし private も、依存はプロジェクトの版、std、docs.rs)。`status: building` と、`retry_after_secs` のある `partial` は、その秒数の後に呼び直す (失敗だけの `partial` は呼び直しても変わらない)。`stale` は `stale_reason` と hints が言うことに従う (待っても直らない理由もある)。外の workspace と git の crate は先に `build_external_docs` を呼び (信頼するコードだけ)、docs.rs にデータの無い版と target は応答が案内したときに呼ぶ (照会のツールはまず docs.rs を読むので、要らないローカルビルドを誘わない)。target の既定は、プロジェクトが使う crate ならその `build.target`、ほかは host。worktree で作業しているなら `project` に作業ディレクトリを渡す (R-MCP-3)。LSP の文は §9.9 の境界 (この workspace の item もパス・名前で引くならこのツール、LSP は位置・参照・診断と `built_at` の後に編集したコード) に書き直した (M1 の「この workspace 自身のコードは rust-analyzer に任せる」は、M2 の試しでモデルが自分の item にこのサーバーを使わなかった最も有力な原因の候補で、直した後の eval では使った)。長さはローカル機能のある構成で 2,013 文字、無い構成で約 1,165 文字 (無い構成は M1 の文に近いまま、`build_external_docs`・`stale`・`project` を書かない)
- R-MCP-7: ツール名は動詞と名詞の snake_case にする。説明文の冒頭 1 文に主要な語 (rust、crate、trait、docs) を入れる
  - Claude Code が付ける接頭辞 (`mcp__<server>__`、プラグイン経由なら `mcp__plugin_<plugin>_<server>__`) を含めたフル名を **128 文字以内**に収める (超えるとモデルに送られない)
  - ツール名は MCP の推奨 (1〜128 文字、`[A-Za-z0-9_.-]`。`server/tools`) にも従う。引数のトップレベルのプロパティ名を 1〜64 文字の `[A-Za-z0-9_.-]` にするのは Claude API の制約による (Claude Code が読み込み時に検査し、違反したツールを除外する。MCP の規則ではない)
  - 各ツールに人が読む `title` (英語。例: `Get Rust item docs`) を付ける (表示名の優先は `title` → `annotations.title` → `name`)。`annotations.title` は使わない
  - serverInfo に `icons` を付けてよい (M5 で検討。任意)。ネットワークの URL を使わない (R-SEC-6)
- R-MCP-8: 入口になるツール (`get_item`、`search_items` など 2〜3 個) に `_meta["anthropic/alwaysLoad"]` を付けて常に読み込ませるかは、eval で決める (Q-15)
  - M2 の F4b (Claude Code 2.1.309。m2-plan.md の F4b。v0.20): `alwaysLoad` を付けず、組み込みのツール (Read・Grep・Glob・LSP) と tool search を有効にして、名指しの無い自分の item の問いを 3 回回した。3 回ともまず tool search でこのサーバーのツールを読み込み、2 回はこのサーバーで、1 回は読み込んだ後に Grep と Read でソースを読んで答えた (答えは正しい)。`alwaysLoad` は M5 の比較で決める
- R-MCP-9: 1 つの引数の有無で 2 つの別データを切り替える「二役のツール」を作らない (ferritin の `search` は crate の有無で item 検索と crate 探しを切り替え、誤用を防ぐために強い文言を重ねている)
- R-MCP-10: 入出力の JSON Schema は標準の 2020-12 に従う。他のクライアントへの配慮のうち、ここに挙げるもの (スキーマを素直に保つ) は安く済むので v1 から行う (v0.18。手間のかかるものは R-MCP-12 のように将来に回す)。符号なし整数は `{type: integer, minimum: 0}` とし、schemars が出す `format: uint64` などの非標準の format を出さない (クライアント側の検証器が警告を出す)
  - スキーマのルートに `anyOf` / `oneOf` / `allOf` を置かない (Claude Code は平坦化してしまう)
  - `$ref` は同じスキーマの中の `#/$defs/…` だけにし、外部の URI を参照しない (検証器は外部の `$ref` を辿らない。`basic/index`)
  - 省略可能な引数は `required` から外すだけにし、`Option` を `type: ["string", "null"]` のような null 許容の型で出さない。Gemini / Vertex など、null を含むスキーマを拒む厳しいクライアントでも使えるようにする (possdog は生成したスキーマから `null` を剥がしている)
- R-MCP-11 **起動を待たせない**: `server/discover` と `tools/list` には、索引の読み込み・`cargo metadata`・ネットワークへのアクセスを待たずに即座に応える。重い準備は最初のツール呼び出しまで遅らせるか、裏で進める。Claude Code の起動タイムアウト (`MCP_TIMEOUT`) は既定で 30 秒
  - ホストの target を求める `rustc -vV` (R-TG-1。上限 10 秒) も待たせない。M1 は起動時に裏の task で求め始め、stdio にはすぐ応え、ツールの呼び出しだけが結果を待つ (公開前の見直しで、stdio を読み始める前に走らせていたのを直した。`rustc` を 5 秒かかるスクリプトにした PATH でも、`server/discover` が 0.1 秒以内に返ることを Unix のテストで確かめる。v0.19)
- R-MCP-12 **ツールの構成を選べるようにする** (将来。他のクライアント向けで手間がかかるので、v0.18 で M5 から後回しにした): 設定 `tools.profile` で、主要なツールだけを `tools/list` に出す構成を選べるようにする。tool search を持たないクライアントで、ツール定義の固定費を抑えるため (ferritin のホスト版はツール 2 個で負担が最小)。構成に無いツールは、説明文の「代わり」の節 (§6.2) やエラーの案内でも勧めない。`tools/list` の順と内容は構成ごとに決定的にする (R-MCP-5)。入れるツールと既定は eval (R-QA-6) で決める (v0.16)
- R-MCP-13 **resources** (M5 で検討。任意): item を resource として公開し、Claude Code の `@server:…` で利用者が会話に添付できるようにする。中身は `get_item` の `detail=normal` と同じ。モデルが自分で引くのはツールで足りるので、利用者が明示的に添付する経路としてだけ用意する。URI の形、一覧に出す範囲、キャッシュのヒント、エラーの返し方は M5 で決める (v0.16)
  - 反面教師 (cargo-doc-mcp は `rustdoc://` resource のパスを検証せず、任意のファイルを読める) は次で防ぐ: resource URI はパーセントデコードと正規化の後に crate 名と版を R-SEC-4 で検証し、item のパスは R-PR-1 の item パスとしてだけ解釈してファイルシステムのパスには使わない
- R-MCP-14 **`server/discover` の中身** (`server/discover`): `supportedVersions` (R-MCP-2 の対応する版。v1 では `["2026-07-28"]`)、`capabilities`、`instructions` (R-MCP-6)、`_meta["io.modelcontextprotocol/serverInfo"]`、`ttlMs`、`cacheScope: "public"` を返す
  - `capabilities` は `tools: {}` とし、R-MCP-13 を実装したら `resources: {}`、R-MCP-18 なら `prompts: {}`、R-MCP-19 なら `completions: {}` を足す。v1 は `listChanged` と `logging` を宣言しない (設定の変更は再起動で反映する。R-MCP-5)。一覧を動的に差し替える機能は、需要が出たら `listChanged` と `subscriptions/listen` で足す
- R-MCP-15 **Logging を使わない**: MCP の Logging (2026-07-28 で deprecated) は使わない。`logging` capability を宣言せず、`notifications/message` を送らない (2026-07-28 では `logLevel` の無い要求に送ることが禁止されている)。ログは R-SEC-1 の stderr と R-OBS-1 のファイルに出し、SDK のクライアントへのログ転送は無効にする
- R-MCP-16 **`subscriptions/listen`**: 受けたら、まず `_meta["io.modelcontextprotocol/subscriptionId"]` にその要求の ID を入れた `notifications/subscriptions/acknowledged` (`notifications` は空。R-MCP-14 で何も宣言しないため) を送り (MUST)、クライアントが閉じるまで保持する。サーバーから閉じるときの扱いは仕様のページ間で記述が食い違う (cancellation は `notifications/cancelled` を MUST、subscriptions は完了の結果を SHOULD) が、stdio では、listen を保持したまま、サーバーが自分から正常に閉じる経路が無い (サーバーが終わるのは、クライアントが stdin を閉じたとき、シグナル、`initialize` の拒否、内部の panic だけ) ので、「つながったままサーバーから閉じる」場面は v1 では起きない (M0 で確かめた。食い違いは M6 の HTTP で扱う。v0.17)。rmcp 3.5.1 は、`accepted_subscription_filter` が既定 (`None`) のままだと `-32601` を返すので、空のフィルターを返して受ける。stdin の EOF を受けると、rmcp は処理中の要求 (保持中の listen を含む) の応答を最大 5 秒待ってから終わる
- R-MCP-17 **Streamable HTTP** (M6。R-MCP-1): 作るときは仕様の Streamable HTTP に適合させ、適合テストを CI に置く。細部はそのときの仕様の版で決める (v0.16)
- R-MCP-18 **prompts** (M5 で検討。任意): 再利用できる手順 (例: 版の移行の手引き、crate の選定) を prompt として出す (Claude Code ではスラッシュコマンド `/mcp__<server>__<prompt>` になり、skill を持たないクライアントにも手順を配れる)。`prompts/get` はサーバー側でツールを実行しない (ビルドを起こさない)。引数は空白を含まない 1 トークンにする (Claude Code は空白で分割する)。どの prompt を出すかは M5 で決める (v0.16)
- R-MCP-19 **completions** (M5 で検討。任意。R-MCP-13 か R-MCP-18 を入れた場合): 引数を補完する。補完ではネットワークにもビルドにも触れず、呼び出しの頻度に上限を設ける (`completion` の Security の MUST。初期値は 1 秒に 20 回。R-SEC-7)。件数と並びは M5 で決める (v0.16)

### 6.2 説明文の書き方

各ツールの説明文は、次の順で構成する。

1. **答える問い**: このツールが答える問いを、LLM が考えそうな言い回しで書く (§3 の問いを流用する)
2. **使う場面と使わない場面**: 使う場面と、使うべきでない場面・代わりのツール (否定ルール)
3. **戻り値**: 何が返るかを 1 行で
4. **網羅性の注意** (該当するツールだけ): 「結果は順位付けされ上限がある。結果に無いことは存在しない証拠ではない。`page` を見よ」(rust-analyzer-mcp の説明文に倣う)。`list_implementors` の `scope=crate` には「trait を定義した crate の中だけ」と書く

制約:

- 各ツールの説明文とサーバーの `instructions` は **2,048 文字以内**にする (CI で検査する。R-QA-5)。Claude Code の現行の上限は 4,096 文字 (instructions と、先頭で読み込むツールの説明文) / 16,384 文字 (tool search 経由で読み込むツール) で、v2.1.296 より前は 2,048 文字だった。切り詰めは末尾から起きるので、要点を先頭に置く。tool search は先頭の 4,096 文字だけを照合するので、探されるときの語も先頭に置く
  - 参考: syphar は説明文を 150 語 (約 1,000 文字) 以内に収め、テストで検査している
- 説明文は 1 ツール 1 ファイル (Markdown) で管理し、ソースに埋め込む
- 説明文はローカル機能の有無 (R-ARCH-4) で出し分ける。機能のある構成だけの部分を `⟨…⟩`、無い構成だけの部分を `⟪…⟫` で囲み、ツールの定義を作るときに構成に合わせて印を外す (閉じ忘れと入れ子は debug ビルドで落とす)。機能の無い構成の説明文はプロジェクト・workspace・member を書かず、instructions は `project` を書かない (CI で検査する。R-QA-5)。引数の `version`・`target` の既定の書き方はツールの間でそろえる (照会のツールの `target` は §6.4)。上限 2,048 文字は変えず、繰り返していた注記 (docs.rs の 2025-05-23 の注記、`get_item` の std の nightly の詳しい説明など) を削って空けた。送られる長さは `get_item` が 2,046 文字、`search_items` が 1,924 文字 (m2-plan.md の F4a。v0.20)
- 説明文に書いたツール名が実在するかも CI で検査する (R-QA-5。snowmead の prompt が改名後に存在しないツール名を案内し続けている先例による)
- ツールの引数に実装の詳細 (`format_version` など) を出さない

### 6.3 ツール一覧 (案)

| ツール | 答える問い | 使うべきでない場面 → 代わり |
| --- | --- | --- |
| `list_crates` | プロジェクトの workspace member と依存の一覧 (**解決された**バージョン、取得元、有効な feature、advisory の件数、分かっている範囲での rustdoc JSON の有無)。crate を指定すると、その crate を引き込んでいる経路 (workspace member からの依存の連鎖。`cargo tree -i` に相当) を返す。M1 の形 (m1-plan.md の C8。v0.19): `scope` (`direct` が既定で member とその直接依存、`all` で全パッケージ)。`crate` を渡すと、その名前のパッケージを版ごとに、`used_by` (member からの連鎖。短い順に 5 本まで) 付きで返す。各項目は `crate`・`version`・`source` (`crates.io`・`path`・`git`・`registry`)・`member`・`direct`・`build` (評価する target の通常のビルドなら `normal`、dev を含むビルドにだけなら `dev`、どちらでもなければ `none`)・`features`・`docs_rs_json: cached` (キャッシュにあるときだけ。ネットワークには出ない)。advisory の件数は M4。M2 の形 (v0.20): 版を決める前 (`cargo metadata` の解決中) は `status: building`。外の workspace は記録から答え (`resolved_from: external`)、記録の後に入力が変われば `stale` (R-MCP-3)。`docs_rs_json: cached` は照会と同じ target の置き場で見る (独自の target のプロジェクトでは docs.rs の既定の target。R-TG-1) | crate の中身を知りたい → `crate_overview` |
| `resolve_version` | X の latest は? このプロジェクトで解決される X は? この版は存在する? JSON はある? M1 の形 (m1-plan.md の C2。v0.19): 版の決め方は `get_item` と同じだが、プロジェクトの path・git・別レジストリのパッケージをエラーにせず取得元として答え、キャッシュの近道を使わず常に index を読む。`data` は `version`・`source` (`crates.io`・`path`・`git`・`registry`)・`yanked` (真のときだけ)・`rust_version` (index の MSRV)・`latest` (最新の正式版)・`docs_rs_json` (R-DS-5)。provenance の `source` は `crates.io` か `project` (プロジェクトにだけあるパッケージ)。std の仲間は `invalid_argument` にして `get_item` を案内する。M2 の形 (m2-plan.md の D7d-1・E3b-2・E4b-3。v0.20): 完全一致の版でもプロジェクトのパッケージを探し、crates.io でないパッケージと member なら `source: project` と R-VR-1 の warning。プロジェクトの crates.io のパッケージは package 名で index を読んで照合し、写し (R-VR-2) なら `data.source` は `crates.io` のまま `docs_rs_json: not_on_docs_rs` と理由の warning。git の crate は外のツールの記録から答え、`data.source: git`・`docs_rs_json: not_on_docs_rs`。外の workspace は記録から版を決め、記録の後に入力が変われば `stale` (R-MCP-3)。既知の残り (M3 の最初の段で直す。m2-plan.md の §7.1 の決定 6): `target` を省くと、プロジェクトが使わない crate でもプロジェクトの target で docs.rs を確かめる (照会のツールは host。R-TG-1) | **他のツールの前に呼ぶ必要は無い** (各ツールが自分で解決する)。ドキュメントそのもの → `get_item` |
| `crate_overview` | crate の全体像 (ルートのドキュメントの要約、主要なモジュールと item、feature の概要)。M1 の形 (m1-plan.md の C3b。v0.19): `data` は `path`・`docs` (ルートのドキュメントの第 1 段落。見出しから始まるものは最初の節の第 1 段落)・`sections`・`counts` (公開パスを持つ item の種類ごとの数)・`modules` (ルートのモジュールと、その最初の 1 文と cfg の注記)・`items` (ルートのほかの item を `brief` で最大 50 件と `items_total`。超えたら `list_module` を hints で案内)・`features` (`.crate` を読めたとき: `all`・`default` が推移的に有効にするもの・docs.rs で欠けたもの `not_in_docs`・プロジェクトが版を決めたときは `enabled_in_project`)。予算を超えたら、節の本文 → ルートの item → モジュールの 1 文 → ルートの本文の順に削る | 特定の item → `get_item` |
| `search_items` | 名前や説明文で item を探す。範囲と kind で絞れる。`crate` を省いた範囲は、workspace の member (lib か、lib が無く bin が 1 つならその bin) と直接依存 (R-SR-2。v0.20)。ローカル機能の無い構成では `crate` が要る (既知の残り: その構成の `inputSchema` で `crate` を required にしていない。M3 の最初の段で直す。m2-plan.md の §7.1 の決定 6。v0.20) | crate そのものを探す → `search_crates` |
| `get_item` | シグネチャ、ドキュメント (Panics / Errors / Safety / Examples の節)、属性、stability、deprecated、必要な feature、re-export の経路、定義位置 (R-OUT-8)。メソッドやプリミティブも引ける。M2 から、workspace の member (private の item も。`visibility` と `crate::` で始まるパス。R-PR-2・R-SH-5) と、ローカルビルドした依存 (公開の item だけ) も引ける (v0.20) | メソッドの一覧 → `list_methods`。ソース → `get_source` |
| `list_module` | モジュールの中身と re-export。`recursive` で子孫のモジュールまで展開でき、`detail=brief` と組み合わせると crate 全体の公開 API の骨格になる (R-LS-1) | 横断的に探したい → `search_items` |
| `list_methods` | その型で呼べるメソッド (固有 / trait / blanket / Deref) | 実装している trait の一覧 → `list_impls` |
| `list_impls` | 型が実装している trait (auto / blanket / derive / 条件付き)。「X は Send か」 | trait の実装者 → `list_implementors` |
| `list_implementors` | trait を実装している型 (`scope`: crate / dependency_graph。dependency_graph は M4) | 型の側から見たい → `list_impls` |
| `get_source` | item のソース | API を知りたいだけ → `get_item`。文字列でソースを探したい → `search_source` |
| `search_source` | 依存・std・workspace のソースを文字列や正規表現で探す (R-SC-6。M4、任意) | item の名前や説明で探す → `search_items` |
| `inspect_features` | feature の一覧、依存関係、default、プロジェクトで有効か、その feature で有効になる item | 依存一覧 → `crate_metadata` |
| `find_examples` | item や crate の使用例 (既定はファイルの絶対パス) | 導入手順 → `readme` |
| `readme` | README (見出し単位でページング) | API の詳細 → `get_item` |
| `changelog` | バージョン範囲の変更履歴 (既定は新しい順に応答の上限まで、最大 N リリース。§5.9) | 変更履歴が取れない、または API の差分が欲しい → `api_diff` |
| `api_diff` | 2 つのバージョン間の公開 API の差分と semver 上の重さ | 文章での経緯が欲しい → `changelog` |
| `crate_metadata` | **宣言された**情報 (MSRV、ライセンス、リポジトリ、依存、docs.rs の target)、選定の材料、バージョン履歴 | feature の詳細 → `inspect_features` |
| `search_crates` | crates.io で crate を探す (選定の材料つき。被依存数は除く) | **crate が既に分かっているときは使わない** → `search_items` |
| `audit_dependencies` | 使っている依存の、解決された版に該当する advisory | 個々の crate の情報 → `crate_metadata` |
| `rust_release` | Rust のリリースごとの安定化・変更点 (指定した版より新しいもの) | 個々の std API → `get_item` |
| `build_external_docs` | 照会のツールがビルドしないコード (依存でない crate・Cargo.lock に無い版、既定のプロジェクトの他の target での文書、git の crate、外の workspace。R-WS-10) の文書を、ローカルビルドで作る。作ったものはキャッシュに入り、照会のツールが読む。build.rs と proc-macro を実行する (R-WS-10・R-SEC-2。名前は v0.15 で確定。利用者が ask のルールに書くので変えない)。M2 の形 (m2-plan.md の E1〜E4。v0.20): ローカル機能のある構成だけに出す。外の workspace・既定のプロジェクトの他の target・`-p` で名指しできない crates.io の依存・git の crate・依存でない crate と Cargo.lock に無い版を作る。docs.rs に読める JSON があるものと照会のツールが作る範囲は作らず、`data.built: false` と `reason` (`docs_rs`・`query_tools_build`) を返す。応答は `data` (`built` と照会の鍵) と、照会のツールに渡す JSON の hint。`build.wait_secs` を過ぎたら `status: building` と、続きは照会のツールで待ち外のツールを呼び直さない旨の hint。まだ作らない行 (名指しできない path・git の依存など) は「(in a later version)」と返す (版は書かず、M3 以降で対応を検討する) | docs.rs に JSON がある crate や、既定のプロジェクトの照会 → 照会のツール (必要なら自分でビルドする。ただし照会のツールが `build_unsupported` を返した依存は、このツールで作る。R-WS-5。v0.17) |

### 6.4 共通引数

| 引数 | 意味 |
| --- | --- |
| `crate` | crate 名。`crate@version` も可 (`version` と両方指定して食い違えば `invalid_argument`)。パッケージ名とコードに書く名前 (lib 名・依存の改名) の両方を受ける。解き方と正規化は R-PR-1 (v0.18) |
| `version` | 任意。省略したら §5.3 の順で解決する。semver 要件も受ける |
| `path` | item のパス (R-PR-1 の形を受ける) |
| `project` | 任意。workspace のディレクトリか Cargo.toml。相対パスなら既定のプロジェクトのディレクトリから解く (v0.19)。省略時の決め方は R-MCP-3。ローカル機能の無い構成では宣言しない (R-ARCH-4)。照会のツールで既定のプロジェクトの外を指したときは、`build_external_docs` が記録したものだけを使い、cargo を起動しない (R-MCP-3)。説明に、worktree で作業しているなら作業ディレクトリを渡すよう書く (R-MCP-3。v0.20) |
| `target` | 任意。省略時は R-TG-1。照会のツールの既定は、プロジェクトが使う crate ならその `build.target` (配列なら先頭。無ければ host)、ほかの crate (依存でない crate・git の crate) は host。`list_crates` はプロジェクトの target。`resolve_version` も、プロジェクトが使う crate はその target、使わない crate は host (m2-plan.md の M2e の決定 11・F4a-1。M2 の実装は使わない crate にもプロジェクトの target を使う既知の残りで、M3 の最初の段で直す。m2-plan.md の §7.1 の決定 6。v0.20)。照会のツールで実際のビルド target (配列ならそのどれか) と違う値を渡すと、ローカルビルドせず docs.rs と外のツールの記録を探す (R-MCP-3)。`build_external_docs` で他の target での文書化をするときは必須 (R-WS-10) |
| `limit` / `cursor` | ページング (R-OUT-3) |
| `detail` | `brief` / `normal` / `full` (§7.1)。既定はツールごとに決める: 1 つの item を返すツール (`get_item`・`crate_overview`) は `normal`、一覧のツール (`list_module`・`search_items` など) は `brief` (R-OUT-7。v0.19) |
| `git` / `rev` / `branch` / `tag` | `build_external_docs` で git リポジトリの crate を文書化する (`crate` と組み合わせる)。照会のツールでは、そうして作ったものを指す読み取りの鍵としてだけ受ける (R-MCP-3)。`git` はリポジトリの URL。`rev` (コミットのハッシュか `refs/…`)・`branch`・`tag` は cargo の git 依存と同じ意味で、指定できるのは 1 つだけ (どれも省けば既定のブランチ)。cargo の `rev` には、ブランチの名前だけを渡すと失敗する (タグの名前・`refs/heads/…`・短いハッシュは通る。2026-10-16 に実測) ので、引数を分けて cargo の同名のキーに写す。文書化の仕方 (仮のプロジェクトの git 依存なので private item は出さない。feature の既定) は R-WS-5、鍵 (解決したコミットのハッシュ) は R-WS-10。照会のツールに渡した短いハッシュ (7 文字以上の 16 進) は、記録 (作成中・失敗を含む) のある commit だけと前方一致で照合し、1 つなら使い、複数なら `invalid_argument` と候補、一致したがどれも記録が無ければ `no_record` にする。記録のある commit に一致しなければ、その短いハッシュを ref にした要求の鍵の記録も読む (作成中なら `status: building`、失敗ならその失敗)。40・64 桁のハッシュは commit の鍵を直接読む (解決したコミットが古いとみなす時間は R-MCP-3。m2-plan.md の M2e の決定 8・E3b-2。v0.20)。4 つの引数は、ローカル機能のある構成で `crate` を取るツール (`list_crates` を除く) にだけ宣言する。`version` (`crate@version` を含む。コミットが版を決めるため)・`project` とは一緒に受けず、`crate` が要り、`crate` なしの検索と std には `invalid_argument` を返す。ローカル機能の無い構成で渡されたら `invalid_argument` (黙って crates.io の crate を返さない。m2-plan.md の E3a。v0.20)。オフラインモードでキャッシュに無ければ `offline_miss` (R-VR-2・R-CA-7)。URL と参照名の検証は R-SEC-4、取り込んだリポジトリの設定の扱いは R-SEC-2 |
| `refresh` | 任意。照会のツールのうち cargo を起動しうるもの (R-MCP-4 の分類) で、その鍵の決定的な失敗の固定 (R-WS-7) を解いて、取得やビルドをし直す。同時ビルド数の上限 (R-SEC-7) はそのまま効く。既定のプロジェクトの外を指したときは無視する (ビルドしないため)。`build_external_docs` には持たせない (同じ引数の呼び直しで解くため。R-WS-10)。成功した結果は作り直さない。M2 で決めた解く記録 (m2-plan.md の D5・D5b。v0.20): member は、失敗の記録 (R-WS-7) を読まず、前回の後退 (`--cfg docsrs` を外した・通常の組に戻した。R-FT-3) で始め方を決めない (ビルドが新しいまま後退していたら、後退の印を消すだけで作り直さない)。dep-info の失敗の記憶も消す。docs.rs の crate は、その版の否定キャッシュ (頼んだ target と既定の target の置き場)・`.crate` の取得の失敗・index に名前が無かった記憶と index の取得の失敗。std は配布物の取得の失敗。どの呼び出しでも、ワークスペースの分 (`cargo metadata` と target ごとの `cargo tree` の失敗、`rustc -vV`・rustup の失敗) を解き、1 つの crate を指さない呼び出し (`list_crates`・`crate` なしの `search_items`) はそれだけを解く。何を解いたかは応答に書かずログに書く。同じ対象 (プロジェクトと crate、プロジェクトだけ、std) には 1 度だけ効かせ、その対象の呼び出しが呼び直しを求めない応答 (エラーか、`retry_after_secs` の無い結果) で終わるまで、続く `refresh` は効かせない (最長 10 分。付けたまま呼び直し続けても、作り直しを繰り返さないように)。進行中の仕事に合流した呼び出しの `refresh` は、その仕事を変えない。ローカル機能の無い構成では宣言しない。カーソルの照合には入れない (付けた呼び出しの次のページを、付けずに頼める)。外の記録 (外のツールの記録・git) を読む応答では効かせず、その旨を warnings に書く |

- R-ARG-1: 引数の誤り (`inputSchema` に合わない型、必須の引数の欠落、形式の誤ったバージョン、無効なカーソル) は、JSON-RPC の invalid-params ではなく、提案付きのツール結果エラー (`invalid_argument`。§8) として返す (`server/tools` は入力の検証エラーをツールの実行エラーに分類している)。型を厳しくしすぎて入口で弾かない。JSON-RPC の `-32602` にするのは、未知のツール名と、`CallToolRequest` の形に合わない要求 (`name` が無い、`arguments` がオブジェクトでないなど) だけとする (`name` が無い要求と `arguments` がオブジェクトでない要求に rmcp 3.5.1 が `-32601` を返すのは、受け入れる違いとして R-MCP-2 に書いた。v0.17。v0.19)。M1 で `invalid_argument` を返すのは、ほかに、無効なカーソル (R-OUT-3)、一覧のツールにモジュールでない item・型でない item・trait でない item を渡したとき、`list_implementors` の `scope: dependency_graph` (M4)、std の `resolve_version`、範囲を決められずに `crate` を省いた `search_items` (R-SR-2)、誤った `detail`。どれも理由と、代わりに呼ぶツールを添える (v0.19)。M2 で足したもの (v0.20): 外のツールの引数の誤りと組み合わせ (`crate` も `project` も無い、std の仲間、`crate` なしの `version`、他の target でだけ使う依存で `target` が無い、member・依存・crates.io の同名の重なり。R-WS-10)、`git` の形と組み合わせ、曖昧な短いハッシュ、ローカル機能の無い構成での `git` (§6.4)、真偽値でない `refresh`。rmcp の `Parameters<T>` は逆直列化の失敗を `-32602` にするので、引数は JSON のオブジェクトで受けて自前で検証する

### 6.5 一覧系ツールの絞り込みと展開

- R-LS-1 **crate の骨格**: `list_module` に `recursive` (と深さの上限 `depth`) を設け、子孫のモジュールの item まで返せるようにする。`detail=brief` と組み合わせると、crate 全体の公開 API を 1 行のシグネチャの並び (骨格) として `page` 付きで返せる (ruskel の骨格表示、rustdoc-mcp の `recursive` に倣う。ruskel と違い、上限とページングを必ず持つ)
  - M1 の形 (m1-plan.md の C3a。v0.19): 引数は版を名指すもの (`crate`・`version`・`project`・`target`) と `path` (省けば crate のルート)・`recursive`・`depth` (既定 3、最大 8)・`kind` (1 つか配列)・`detail` (既定 `brief`)・`limit` (既定 50、最大 200)・`cursor`。他の crate からの re-export も子として並べて `reexport_of` を付け、一覧のパスと正準パスが違う item には `canonical` を付ける。並びはパスの辞書順で、`recursive` ではモジュールの直後にその中身が来る。同じモジュールは 1 度だけ並べる (re-export の循環に備えて)。予算 (R-OUT-3) を超えるページは件数を減らして収める。モジュールでない item を指したら `invalid_argument` (kind を添えて `get_item` を案内する)。他の crate のモジュール (`reqwest::header`) は R-PR-4 のとおりたどって一覧する
- R-LS-2 **kind の絞り込み**: `list_module` に kind の絞り込みを設ける。上限で切る前に適用する (R-OUT-4)
- R-LS-3 **メソッドの絞り込み**: `list_methods` に、名前の部分一致と出どころ (`inherent` / `trait` / `deref` / `blanket`) の絞り込みを設ける (syphar の未マージのブランチに倣う)。切る前に適用し (R-OUT-4)、`page` の `total` は絞り込み後の件数にする。M1 の引数は `name` (大文字小文字を区別しない部分一致) と `from` (`inherent`・`trait`・`blanket`。`deref` は M4)。並びは固有 → trait (trait のパス順) → blanket、その中は名前順。汎用 trait 以外の blanket impl のメソッドは、`expand_blanket` を渡したときだけ 1 つずつ並べる (R-SH-4。v0.19)

---

## 7. 出力形式

### 7.1 方針

- R-OUT-1 **Claude Code の実際の挙動を前提にする** (Q-1):
  - `structuredContent` があると、モデルには `JSON.stringify(structuredContent)` だけが渡り、text の content は捨てられる (Claude Code 2.0.21 以降。2.1.301 (2026-10-16) と 2.1.303 (2026-10-19。M0。v0.17) でも同じことを確かめた。Issue #79944 は 2026-10 時点で open。直る可能性があるので、M1 から始める eval (R-QA-6) に確かめるケースを置き、Claude Code の版を上げたときに回す。v0.18。M1 の eval で、2.1.304 でも同じことを確かめた。v0.19)
  - よって **`structuredContent` がそのままプロンプトになる**。モデルが必要とする情報 (provenance、warnings、hints、page、truncated) はすべて `structuredContent` に入れる
  - `structuredContent` のキーは、応答の構造体のフィールドの順に並べる (R-SH-1 の「シグネチャを散文より先に」の前提)。rmcp の `structuredContent` は `serde_json::Value` で、serde_json の既定の Map はキーを辞書順に並べ替えるので、`preserve_order` を有効にする (M0 で、無効のままでは `docs` が `signature` より前に出た)。`Value` を後から加工するときも順を保つ (`preserve_order` の下の `Map::remove` は最後の要素で穴を埋めて順を崩すので使わない)。キーの順はテストで確かめる (`Value` どうしの比較は順を見ない。v0.17)
  - structured モードでは、同じ JSON を直列化した text の content も返す (`server/tools` の SHOULD。後方互換のため)。`structuredContent` に無い情報を text にだけ載せてはならない。このため、送る量は応答の上限 (R-OUT-3) のおよそ 2 倍になる (モデルに渡るのは片方だけ。v0.19)。text は出口で、サニタイズ (R-SEC-7) した `structuredContent` から作り直す (後から `structuredContent` に足した hints が text に無かった応答も、これで揃った。m2-plan.md の F7。v0.20)
  - `isError: true` の結果には `structuredContent` を付けず、中身は `content[0].text` で完結させる (R-ERR-2。Claude Code もエラー時は `structuredContent` を見ない)。仕様の `outputSchema` の MUST に、エラーを除く旨の明文は無いが、エラーを schema に収めない意図的な選択とする
  - structured モードでは `outputSchema` を宣言し、`structuredContent` を必ずそれに合わせる (MUST)。`status: building`・各 `detail`・`page` 付き・`partial` の応答もすべて同じ schema に収める。outputSchema のルートに `anyOf` / `oneOf` を置かず、状態による差は省略可能なプロパティで表す (CI での検査は R-QA-5)
  - v1 の出力は structured だけにする。比較用の text (簡潔な Markdown) と toon (gnufood に倣った TOON 形式) は、M5 の eval の準備として設定 `output.mode` で足す (v0.16)。text モードの表示の規則の候補: 本文中の見出しを構造上の見出し (Methods など) より 1 段下げる、切り詰めの印を `[+N lines]` のように実際の単位で書く (R-SH-7。v0.16)
- R-OUT-2 **JSON は LLM が読む前提で設計する**。効くのは形式の選択 (JSON / Markdown / TOON の差は 10〜25%) より、**何を描画済み文字列にし、何を出さないか**:
  - シグネチャは rustdoc-types の木ではなく **Rust のコード文字列**として描画する (例: `pub async fn lock(&self) -> MutexGuard<'_, T>`)。描画に rustfmt などの外部ツールを実行時に要求しない (ruskel は要る)。木のままだと数倍から十数倍に膨らむ (syphar で約 5〜6 倍、ferritin の JSON で 8〜12 倍。M0 の実測では、シグネチャ・where 句・fields・variants の部分木と描画した文字列の比で、おおむね 8〜17 倍だった (tokio・axum・serde の 9 item。m0-plan.md の §6.1 の A8)。v0.17)
  - **描画できない形**: 描画処理は網羅的な `match` で作り、知らない部品 (新しい形式で増えた型や構文の種類など) に出会ったら、描画を誤魔化さず warnings で示す。item の span からソースの宣言文を切り出して `source_signature` として添える案 (syphar の未マージのブランチに倣う) は、採らない (描けない例が見つかったら戻す。M1 の終わりに利用者が決めた。v0.19)
  - 引数名を含める。型のパスは**正準パス** (R-PR-2) で描画し、ソースに書かれたままの相対パス (`self::super::Size`) や定義パスを使わない。ほかの crate の型の正準パスは、その crate の公開パス索引 (R-PR-4) で求める。これは M4 で依存グラフ全体の作業と一緒に入れ、M1 ではほかの crate の型を rustdoc が渡すソースの書き方のまま描く (M1 で利用者が決め、v0.17 の「M1 から」を改めた。v0.19)
  - 制限付きの可視性 (`pub(crate)`、`pub(in path)`) を描画する。M2 の形 (m2-plan.md の D6a。v0.20): 含むモジュールと同じ範囲 (`pub` を付けずに書いたもの) は何も付けず、ほかは `pub(crate)` か `pub(in crate::…)` で描く (`pub(super)` は JSON から分からないので書かない)。member の item には `data.visibility` (`pub`・`pub(crate)`・`pub(in crate::…)`・`private`) を付ける (trait の item と enum の variant は親のもの。R-SH-5)
  - fields / variants / generics / where 句は、描画済み文字列の配列にする
  - `null` や空の値は出さない。キー名は短すぎず、意味が分かるものにする
  - 数値の rustdoc Id を出さない (ビルドごとに変わり、LLM には意味が無い)
  - `macro_rules!` は、rustdoc が出す規則の一覧で示す (規則の本体は rustdoc がすでに `{ ... }` に省いているので、そのまま残す。v0.17)。`async_trait` の展開形は `async fn` に戻して `via: async_trait` を付ける (任意)
- R-OUT-3 予算とページング:
  - MCP の既定の応答の上限は**推定 8,000 トークン**とする (G5)。推定は R-OBS-1 の換算係数で文字数から出し、校正するまでは **32,000 文字**を上限とする (英文で約 8,000 トークン。JSON・コード・CJK は同じ文字数でもトークンが多い)。Claude Code は 1 万トークン超で警告し、5 万文字超の結果をファイルに退避してモデルには経路だけを渡す
  - 上限を超えたときの削り方 (M1。m1-plan.md の C1c。v0.19): `get_item` は、節の本文を後ろから → `docs` を半分ずつ (200 文字までは残し、末尾に `…`) → フィールドかバリアントの多いほうを半分ずつ (`/* N more not shown */` を足し、warnings に「全 N 件のうち M 件」と書く) の順に削り、`truncated: true` を付ける。一覧のツールは、ページの件数を減らして続きをカーソルで示す。`crate_overview` の削る順は §6.3
  - 大きな本文を意図的に返すツール (`get_source` の full、`readme`、`api_diff`) は、ツールごとに `_meta["anthropic/maxResultSizeChars"]` (上限 50 万文字) を付けてよい。ただし既定はページング
  - 上限のある一覧 (implementors、methods、impls、モジュールの子、検索結果、changelog など) はすべて `page` (`shown` / `total` / `has_more` / `next_cursor`) を返す。「… and N more」と書いて続きを取る手段が無い状態は禁止する
  - 本文 (ドキュメント・README・ソース) を切り詰めたときは `truncated: true` を付ける
  - カーソルは不透明で、**同じツール**が受ける。前のページと食い違えば理由付きの `invalid_argument` で拒否し、最初から取り直させる (黙って飛ばしたり重複させたりしない)
    - M1 で決めた形 (利用者が決めた。m1-plan.md の C3a・C4b と公開前の見直し。v0.19): `c1.<要求のダイジェスト>.<データのダイジェスト>.<位置>`。要求のダイジェストは、ツール名と、`cursor`・`limit` を除いた引数 (キーの順によらない) の SHA-256 の先頭 16 文字 (同じ引数のほかのツールのカーソルは拒む)。データのダイジェストは元の JSON の SHA-256 (std は鍵) の先頭 16 文字で、他の crate へたどったときは、たどった後のデータで確かめる。位置はオフセットにする (並びは決定的で、データは版ごとに固定なので、飛びや重複は起きない)。サーバーは状態を持たない。形の誤り・食い違い・終わりを越えた位置は、理由付きの `invalid_argument` にする
    - 複数の crate の応答 (`partial` を含む) では、範囲の crate のダイジェストを並べたものの SHA-256 をデータのダイジェストにする。取得が進んで範囲の中身が変わると、前のカーソルは「データが変わった」で拒まれ、最初から取り直させる。ビルド中も、カーソルは検索した crate のダイジェストに束縛したままで、読み込み中の crate が揃うと同じく拒まれる。読み込み中の crate があって次のページがあるときは、そのことと、最初から検索し直すことを hints で案内する (利用者が決めた。m2-plan.md の C3c-1。v0.20)
  - **呼び出しごとの予算** (M5。任意): 共通引数 `max_chars` で、その呼び出しの上限を既定より小さくも大きくもできるようにする (lockdocs の `--tokens` に倣う)。上限に合わせて `detail` の段階を自動で下げ、`truncated` と `page` で続きを示す。`detail` と `limit` で足りるかを eval で確かめてから採否を決める
- R-OUT-4: 並び順は決定的にする (同じ入力には同じ出力)。絞り込み (kind・名前・出どころ) と並べ替えは、上限で切る前に行う
- R-OUT-5 ドキュメント本文:
  - intra-doc link は Markdown パーサーで書き換える。表示テキストとバッククォートを残し、リンク先を**正準パス**にする。コードブロックの中は書き換えない。解決できないリンクや外部リンクは URL を残す。リンク先を消して表示テキストだけにしない (ferritin は「The complete list is here.」のように行き先を失っている)
  - インラインコードのバッククォートを残す
- R-OUT-6 doctest の隠し行 (`#` と空白で始まる行と、`#` だけの行) は既定で除く。`##` は `#` に戻す。`rust` 系のすべてのコードブロック (`no_run`、`should_panic`、`edition20xx` など) に適用し、`should_panic` / `compile_fail` は見える印として残す。M1 は、info string が無いか doctest の属性だけのブロックを Rust とみなし、`text` などほかの言語のブロックは触らない (v0.19)
- R-OUT-7 `detail` の段階 (Q-8。数値は eval で調整する):
  - `brief`: パス、kind、親、1 行のシグネチャ (`macro_rules!` の規則は省く)、`reexport_of`、必要な feature の 1 行の要約と `enabled_in_project` (dev の経路でだけ有効なら `enabled_in_dev_only` も。R-FT-2。該当する項目だけ)
  - `normal`: シグネチャと where 句 (rustdoc は境界の多くを where 句に移すので、入れないと `tokio::spawn` の `F: Future + Send + 'static` が消える。M0 で決定。v0.17)、ドキュメントの第 1 段落 (最初の見出しより前のもの。`docs_truncated` 付き。段階で絞ったことを表し、予算で切り詰めた R-OUT-3 の `truncated` とは別)、残りの節の見出しと行数 (`sections`)、fields / variants の文字列。impl は `list_impls` で引き、`get_item` には載せない (v0.19 で「impl の見出し (メンバーなし)」を外した)
  - `full`: ドキュメント全文 (最初の見出しより前は `docs`、各節の本文は `sections[].text`)、属性 (`attrs`。Rust の書き方に戻し、cfg の痕跡 (R-FT-2 の注記に任せる)・`#[doc = "…"]`・rustc 内部の属性 (`#[attr = Rustc…`) は除く。v0.19。生の rustdoc の部分木は返さない。数値の Id を含み (R-OUT-2)、描画した文字列の 8〜17 倍に膨らむため。v0.17)
  - ドキュメントは、コードブロックの外の ATX 見出し (`#`〜`######`) で節に分ける (v0.19)
  - **既定はツールごとに決める** (v0.19 で、全体の既定を `normal` とした v0.18 までの形を改めた): 1 つの item を返すツール (`get_item`・`crate_overview`) は `normal`、一覧のツール (`list_module`・`search_items` など) は `brief`。一覧は件数が多く、R-LS-1 の骨格も `brief` との組み合わせを想定しているため。設定 `output.detail` (R-DIST-3) は、1 つの item を返すツールの既定だけを変える。`search_items` には `full` が無い (`brief` は 1 行のシグネチャ・cfg の注記・ドキュメントの最初の 1 文)
  - モジュールや crate のルートは散文が大半を占める (tokio::sync では 69%) ので、一覧系の既定の `brief` では本文を返さない (v0.19 で、`docs: none` を選べるようにする案を改めた)。`list_module` は、`normal` 以上のときだけモジュール自身のドキュメントの第 1 段落を返す
- R-OUT-8 リンクやファイルの場所 (docs.rs の URL、ソースファイルのパス) は `structuredContent` の中の文字列として返す。`resource_link` ブロックに頼らない (R-OUT-1 により捨てられる)
  - `get_item` は item の**定義位置** `source_location` を返し、`get_source` やクライアントのファイル読み取りで直接開けるようにする (possdog の `file:line`、lockdocs の `pkg@ver path:line` の引用に倣う)。項目は `file` (package root からの相対パス。取得元とキャッシュの状態によらず同じ形)・`lines` (`434-466`、1 行なら `301`)・`url`。ローカルビルドした crate (workspace・path・git) では、開ける絶対パスを `path` に足す (M2 では入れず、M3 で `get_source` と一緒に入れる。m2-plan.md の §7.1 の決定 2。v0.20)。docs.rs の crate の中身は `get_source` で読む (v0.17)。既知の残り (M3 の最初の段で `path` と一緒に直す。m2-plan.md の §7.1 の決定 9。v0.20): ローカルビルドした member の `file` は span をそのまま返すので、package root ではなく workspace のルートからの相対パスになるとみられる (コードを読んだだけで、実測はしていない)
  - docs.rs 由来の crate では、`file` は package root からの相対パス (docs.rs の JSON の span そのまま)、`url` は docs.rs のソースブラウザの URL に行のアンカーを付けたもの (`https://docs.rs/crate/<name>/<version>/source/<file>#434-466`。ソースブラウザも rustdoc のソースページも `#N` と `#N-M` で行を強調表示することを M0 で確かめた。ソースブラウザは span のパスがそのまま使え、src の外のファイルも指せる。v0.17)
  - span が package root の外 (ビルドの生成物の `OUT_DIR` やビルドホストの絶対パス) なら `url` を付けず、warnings で知らせる
  - std の定義位置 (`/rustc/<コミット>/library/…`) には、rust-lang/rust の GitHub のそのコミットの URL を付け、「package root の外」の警告は出さない (M1。v0.19)

### 7.2 何を出し、何を出さないか

- R-SH-1 関数・メソッドでは**シグネチャを散文より先に**置く
- R-SH-2 `kind` は rustdoc の `ItemKind` の語 (`struct`・`function`・`assoc_type` など) に合わせ、impl と trait の中の関数だけを `method` にする (M0 の実装。初期案。v0.17)。メソッドや関連 item の見出しにも、親の型、trait (あれば)、crate、バージョンを含める (ferritin は「Function lock」としか出さない)。trait は別の欄にせず、impl の見出し (`impl<T> Trait for Type`。where 句も付ける) で示す (欄を分けると同じ情報が 2 回出る。M0 で決定。v0.17)
- R-SH-3 **impl は「辺」として扱う**: impl や implementor の項目には、その辺に固有の情報 (impl の見出し、where 句、関連型・関連定数の束縛、独自のドキュメント) だけを載せ、trait のメソッドを implementor ごとに繰り返さない (繰り返すと `Iterator` の実装者 1 つで 18KB になった先例がある)
- R-SH-4 blanket impl の既定の扱い (Q-9。eval で調整する):
  - core / alloc の汎用 trait (`From`、`Into`、`TryFrom`、`TryInto`、`Borrow`、`BorrowMut`、`Any`、`ToOwned`、`ToString`、`CloneToUninit`) の blanket impl は、where 句を省いて**名前だけを 1 行に畳む** (隠さない)。条件付きのもの (`ToString` は `Display`、`ToOwned` と `CloneToUninit` は `Clone`、`Any` は `'static` が条件) は、その型が条件を満たす場合だけ載せる (rustdoc JSON の `impls` は条件で絞り込み済みなので、そのまま使う。Q-18)。例: `std_blanket: ["From", "Into", "TryFrom", "TryInto", …]`
  - `list_methods` では、これらに由来するメソッド (`to_string`、`to_owned`、`into` など) を名前だけ並べる
  - impl の一覧は出どころの順に並べる: その crate の中の impl → 依存 crate 由来の impl → std 由来の impl (ferritin に倣う)。同じ出どころの中は R-OUT-4 のとおり決定的に並べる
  - 汎用 trait 以外の blanket impl (`Instrument`、`StreamExt`、`VZip` など) は trait ごとに見出し 1 行とし、trait の crate を添える (v0.19 で、提供元の crate ごとにまとめる形を M1 の実装に合わせて改めた。`list_impls` では `impls` に 1 行ずつ、`list_methods` では `blanket` に trait ごとに 1 行でメソッド名の一覧)。メソッドは要求されたとき (`expand_blanket`) だけ展開する (条件付きの絞り込みは Q-18)
  - 根拠: axum の `WebSocketUpgrade` では impl 25 個のうち 16 個、tokio の `Mutex` では 20 個のうち 9 個が blanket impl。cargo-public-api の例では行数の 87% が auto / blanket / derive の impl
- R-SH-5 依存 crate と std の**非公開 item は出さない**。private items は workspace member に限り、可視性を付けて出す。trait の item と enum の variant は親の可視性を継承する (ferritin は trait メソッドを「Private」と誤表示する)
  - 依存は、ここでローカルビルドしても公開の item だけを出す (依存の private を出すと、モデルが使えない item を使い、検索と一覧が内部の item で埋まり、取得元によって同じ crate の見え方が変わるため)。依存と std のフィールドは、`pub` でないか hidden なら除いて `/* private fields */` とし、hidden な variant は除いて `/* some variants omitted */` とする。variant のフィールドは enum と同じだけ公開なので見せる (m2-plan.md の D6b と D7 の決定 2。v0.20)
- R-SH-6 `deprecated` (since と注記) を見出しに出す。`must_use`、`non_exhaustive` も出す
- R-SH-7 struct のフィールドを宣言とフィールド一覧の 2 回に分けて出さない。切り詰めた量を示すときは、実際の単位 (行数など) で示す (text モードの書式は R-OUT-1。v0.16)

### 7.3 共通の応答の形

```json
{
  "provenance": {
    "source": "docs.rs",
    "crate": "tokio",
    "version": "1.47.1",
    "resolved_from": "cargo_lock",
    "project_from": "claude_project_dir",
    "target": "x86_64-unknown-linux-gnu",
    "format_version": 57,
    "features": ["full"]
  },
  "warnings": ["docs.rs built tokio without features [test-util]; items that need them are missing from these docs"],
  "data": { "signature": "..." },
  "page": { "shown": 20, "total": 103, "has_more": true, "next_cursor": "..." },
  "hints": ["call list_methods on tokio::sync::Mutex to see all methods"]
}
```

- 例の `warnings` は、docs.rs のビルドで有効にならなかった feature を挙げる形 (R-FT-1。v0.19 で改めた)
- `source` は `docs.rs` / `local` / `std` のいずれか。**実際にデータを返した取得元**を書く (ferritin は docs.rs のキャッシュを LocalDependency と誤表示している。テストで固定する)。データでなく版や依存の情報を返すツールでは、その情報の出どころを書く: `resolve_version` は `crates.io` か `project` (プロジェクトにだけあるパッケージ)、`list_crates` は `project` (v0.19)。ただし `resolve_version` の git の crate は、外のツールの記録から答えるので `local` (`recorded_by` 付き。v0.20)
- `resolved_from` は `explicit` / `explicit_req` / `cargo_lock` / `cargo_lock_raw` (Cargo.lock を直接読んだもの。R-VR-2) / `external` (`build_external_docs` が記録した metadata。R-WS-10) / `git` (引数 `git` の crate) / `latest` / `reexport` (他の crate への re-export の行き先の版を、起点の `html_root_url` で決めたもの。R-PR-4。v0.19) のいずれか。std では出さない (v0.19)。`external` と `git` は M2 で実装した (v0.20)
- `project_from` は `argument` / `claude_project_dir` / `cwd` のいずれか (R-MCP-3)。環境変数の名前のまま値にしていた `CLAUDE_PROJECT_DIR` を、ほかの値と同じ snake_case に改めた (m2-plan.md の §5 の P8。v0.20)
- `version`・`target`・`format_version` は**データ本体 (JSON の `crate_version`・`target.triple`・`format_version`) から読む**。`version` が解決した版と食い違えば、そのデータを使わない (R-VR-4)。`target`・`format_version` が要求値と食い違えば warnings に出す (v0.18)。std の `version` だけは、`crate_version` が semver として読めないので toolchain の release を入れる (R-STD-1。v0.19)
- ローカルビルドと std では `toolchain` (`rustc -vV` の要約)、`built_at`、`stale`、`features_requested` / `features_effective` を加える。std では `std_from` (`dist`・`local` と理由。R-STD-1) も加える (v0.19)
- M2 で足した項目 (v0.20): `stale_reason` (`stale` の理由。R-OUT-9)、`package_source` (ここでローカルビルドした依存の取得元: `path`・`git`・`registry`・`crates.io`。member には付けない。m2-plan.md の D7a)、`recorded_by` (外のツールの記録から返すとき。R-WS-10)、git の crate の `git`・`rev`・`ref`・`resolved_at`・`plaintext` (R-WS-10)、`index_fetched_at` (TTL を過ぎた sparse index で版を決めたときだけ。UTC の RFC 3339 で秒まで。R-CA-7。m2-plan.md の A3b-2)
- docs.rs の crate では、`.crate` を読めれば `features` (`all_features` のときは省く)・`all_features`・`cfg` (docs.rs が立てた cfg) を加える (R-FT-1。v0.19)
- `list_crates` では `workspace` (workspace のルート) を加える (v0.19)
- ビルド中は応答の最上位に `status: "building"` と `retry_after_secs` を加える (エラーではない。付ける条件は R-OUT-9)
- `warnings`・`page`・`hints` は該当するときだけ出す
- `warnings` の順: 読み込みの警告 (キャッシュの取り直しなど) を先頭に置く (`provenance` の直後という順は保つ)。複数の crate の応答で欠けた crate があれば、その要約の 1 行を先頭に置く (R-SR-5。v0.19)
- `request_id` (R-ERR-2) はエラーの text にだけ入れ、成功の応答には入れない (同じ入力なら同じ出力にするため。R-ARCH-5。v0.19)
- モデル向けの文字列 (説明文・instructions・warnings・hints・エラーのメッセージ・prompt) は英語に統一する。CLI の人向けの出力とログのメッセージも英語にする (Q-19)

- R-OUT-9: **provenance はすべての成功応答に必須とする** (`status: building` の応答を含む)
  - 応答の骨格と下の状態を表す値 (`resolved_from` などの上の値の一覧を含む) は、M1 で outputSchema を作って確定した (v0.19)。ビルドの状態は M2 で足した (下記。v0.20)。outputSchema には、`get_item` の `sections`・`attrs`・`truncated` も足した
  - 応答の骨格 (v0.15。outputSchema はこれに従う): 1 つの crate の応答の provenance で必須にするのは `source`・`crate`・`version` (データが無いときは要求した値か解決した値。版を解決する前の `status: building` では `version`・`source` を省ける。v0.18)・`target` だけで、複数の crate の応答の共通部分で必須にするのは `target` だけ (M1 で決めた。v0.19)。M2 で、outputSchema の必須を次のように緩めた (ルートに `anyOf` を置かない R-OUT-1 の決まりのもとで、版を解決する前の応答と、docs.rs の既定の target の JSON を読む前の応答 (R-TG-1) を収めるため。実際の値が分かれば載せる。m2-plan.md の C3a・C3c・D7c-2。v0.20): 応答の最上位の必須は `provenance` だけ。provenance の必須は、1 つの crate を引くツールは `crate` だけ、`resolve_version` は `crate`・`target`、`crate` なしの検索の共通部分は無し、`sources` の各項目は `crate`・`version`。crate を持たないツール (`search_crates`・`rust_release`) の必須の項目は、そのツールを作る M3・M4 で決める (v0.19)。データ本体から読む項目 (`format_version` など) は任意にする。状態の項目 (`status`・`retry_after_secs`・`building_reason`・`partial`) は応答の最上位に置く。`stale`・`stale_reason` は provenance に置く。`sources` の各項目は、1 つの crate の provenance の項目に、`status`・`stale_reason`・`building_reason`・`retry_after_secs` と、失敗したものなら `code`・`reason`・`retryable` を足したものにする。1 つの crate の応答でも、ビルド中にデータがあれば最上位の `status` を付けず、`stale_reason: rebuilding` と `retry_after_secs` にする (複数の crate の応答の `partial` とそろえる)
  - 1 つの crate の `status: building` の応答 (m2-plan.md の C3a・C3c。v0.20): `provenance`・`status`・`building_reason`・`retry_after_secs` (10)・`warnings`・`hints` の順に並べる。path が他の crate に続くとき (R-PR-4) は、続く先の crate が期限に間に合わなければ、その crate の `status: building` を返す (呼び直せば続く先で引ける)。版を決める前 (`cargo metadata` の解決中や、std の nightly を選ぶ前) の provenance には `version`・`source` が無い。`resolve_version`・`list_crates` も `status: building` を返す
  - 複数の crate にまたがる応答 (`search_items`、`list_implementors` の `dependency_graph`、`audit_dependencies` など) では、provenance を共通部分と、crate ごとの `sources` の配列に分ける (各項目の形は上の骨格のとおり)。M1 の `search_items` の共通部分は `target`・`project_from`。`list_crates` はプロジェクトの解決結果を 1 つの出どころとして返すので、`sources` に分けず、provenance を 1 つにする (`source: project`・`workspace`・`project_from`・`resolved_from`・`target`。v0.19 で M1 の実装に合わせた)。範囲の一部がビルド中や取得中でも、結果が 1 件でもあれば `partial: true` (R-SR-5) で返し (ビルド中のものがあれば `retry_after_secs` も付ける)、`status: building` は結果が 0 件でビルド中のときだけにする。結果が 0 件で読み込み中 (取得・索引の作成・ビルド) の crate があれば、最上位に `status: building` と `building_reason` (索引の作成中のものがあれば `indexing`) を付け、`partial`・`retry_after_secs`・`data`・`page` もそのまま返す (`did_you_mean` が役に立ちうるため。m2-plan.md の C3c-1。v0.20)。結果が 0 件でも、失敗や未取得の crate があれば `partial: true` にする。カーソルの束縛は R-OUT-3
  - 状態を表す値 (v0.15): `sources` の各項目の `status` は `ok`・`building`・`failed`・`unavailable` (応答の最上位の `status` は `building` だけ)。`building_reason` は `building` (ローカルビルドの実行中)・`waiting_for_lock` (cargo のロック。ビルドのほか `cargo metadata`・`cargo tree` のロック待ちにも使う。R-WS-2。v0.20)・`waiting_for_slot` (ビルドの枠。R-SEC-7)・`fetching`・`indexing`。`stale` を付けたら `stale_reason` を `rebuilding` (再ビルド中。R-WS-4)・`inputs_changed` (外の記録の入力が変わった。R-MCP-3)・`ref_ttl_expired` (ブランチなどの解決が古い。R-MCP-3)・`project_missing` (外の workspace の root の Cargo.toml が消え、記録した別名から引いた記録で答えた。R-MCP-3。v0.20) のいずれかで付ける。outputSchema には、`building_reason` の 5 つの値と、そのツールが返しうる `stale_reason` の値を宣言する (`get_item` などの照会は 4 つ、`resolve_version` は `rebuilding` を除く 3 つ、`list_crates` は `inputs_changed`・`project_missing`。m2-plan.md の C3a・E3b-2・E4a-3・E4b-3。v0.20)

---

## 8. エラー設計

- R-ERR-1: ツールの実行時エラーは、プロトコルエラーではなくツールの結果として返す (`isError: true`)。LLM が分岐して回復できるようにするため (先行実装の多くは JSON-RPC のエラーか、成功扱いの「見つかりません」テキストを返していて、LLM が判別できない)
  - ツールの処理中の panic も呼び出し単位で捕捉して `internal_error` の `isError: true` の結果として返し、サーバーや他の呼び出しを止めない (SDK が panic をプロトコルエラーに変えると、mmgeorge と同じ失敗になる)。仕様はサーバー内部のエラーをプロトコルエラーの例に挙げているが、同じ理由で意図的にツールの結果にする
  - panic の後も動き続けるので、共有状態のロックには poison の無いロック (parking_lot や `tokio::sync` のもの) を使い、std の `Mutex` / `RwLock` は使わない (1 回の panic で poison になり、以後そのロックを取る呼び出しがすべて失敗する。clippy.toml の `disallowed-types` で禁じている)。共有状態は差し替えで更新し、panic した呼び出しの書きかけの状態が他の呼び出しに見えないようにする
  - `LazyLock` と `Once` も、初期化の関数が panic すると poison する (`LazyLock` は以後のアクセスがすべて panic し、回復できない。`Once` は以後の `call_once` が panic する)。初期化で panic しうる処理はせず、するなら poison しない `OnceLock` で行う (禁止はせず、レビューで守る)
- R-ERR-2: **エラー結果では `content[0].text` が自己完結していること**。Claude Code はエラー時に `structuredContent` を無視する (Issue #86032)。Claude Code は、text のブロックが複数あれば改行でつないでモデルに渡す (2.1.301 で 2026-10-16 に確かめた。画面の表示は確かめていない。2.1.303 では、エラーの text がモデルの側で `<error>…</error>` に囲まれて見えた。M0 の A9。v0.17)。text ブロックは 1 つにまとめ、`code`、`message`、`retryable`、`suggestions` (次の一手)、`request_id` を JSON で入れる。該当するときだけ、`reason`・`found`/`supported`・`did_you_mean`・候補 (キーは `candidates`。初期案。v0.17)・診断 (R-WS-7)・失敗の時刻・`retry_after_secs` (再試行できるエラー) を足す (v0.15)。M2 で決めたキー (m2-plan.md の D5a。v0.20): 見せなかった診断の件数は `diagnostics_omitted`、記録した失敗 (R-WS-7) を返すときの失敗の時刻は `failed_at`。`request_id` は呼び出しごとに払い出し、R-OBS-1 のログにも同じ値を記録する (複数のセッションが同時に動いても、報告されたエラーとログの行を 1 対 1 に対応付けるため。gnufood の `diagnostic_id` に倣う。ログと一緒に M1 から。v0.17)。形はプロセスごとの 16 進 8 桁の乱数と呼び出しの通し番号 (`a318412a-2`) で、成功の応答には入れない (§7.3。v0.19)。1 万文字以内に収める (長いエラー本文は先頭と末尾だけが残る)

| code | 状況 | suggestions の例 | `retryable` |
| --- | --- | --- | --- |
| `crate_not_found` | crate が存在しない | 似た名前の crate | false |
| `invalid_argument` | 引数を解析できない、または引数どうしが食い違う (R-ARG-1) | 正しい書式 | false |
| `version_not_found` | 指定したバージョンが存在しない | 存在するバージョンの候補 (最大 5 個。R-VR-4) | false |
| `ambiguous_version` | Cargo.lock に同じ crate が複数バージョンある | 候補と、それぞれを使っている依存元 | false |
| `ambiguous_crate` | `crate` の名前が、プロジェクトの依存の extern の名前 (改名の後の名前) と、別のパッケージのパッケージ名の両方に当たった (R-PR-1。M1 で足した。v0.19) | 候補と、それぞれを使っている依存元 (`candidates`) | false |
| `ambiguous_path` | 同名の item が複数ある | 候補の正準パス (最大 20 件と `candidates_total`。R-PR-7) | false |
| `docs_unavailable` | 文書を返せない。原因は `reason` (表の下) で示す | JSON がある近くのバージョン、ローカルビルドの可否 (既定のプロジェクトの外のコードなら `build_external_docs`。R-MCP-3)、`readme` (API の代わりに導入手順だけでも得られる。mmgeorge は docs.rs にビルドが無いとき README に切り替える) | `reason` による (`corrupt` は true。`unparsable` は、否定キャッシュに覚えて `retry_after_secs` を付けたときは true (新しい版のサーバーや docs.rs の再ビルドで読めるようになりうるため)、それ以外は false。ほかは false。v0.19) |
| `unsupported_format` | 形式に非対応 (`found` と `supported` を含む) | サーバーの更新、ローカルビルド | false |
| `item_not_found` | パスが解決できない (探した経路を含む) | `did_you_mean` (類似度の下限あり) | false |
| `build_failed` | ローカルビルドに失敗した。`cargo metadata`・`cargo fetch` の失敗 (既存の例外に当たらないもの) と、ビルドの最大時間の超過も含む (R-WS-7。v0.17) | 診断の要約 | 一時的な失敗は true、決定的な失敗は false。`cargo metadata`・`cargo fetch` の失敗は常に true (R-WS-7)。toolchain の調べ (`rustc -vV`・`rustdoc -vV` の 10 秒の上限) の時間切れも true (m2-plan.md の F4b。v0.20) |
| `fetch_failed` | git の取得の決定的な失敗 (認証の失敗・リポジトリが無い・rev が無い。`reason` は `auth`・`no_repository`・`no_rev`。v0.19。R-WS-7) | `auth` なら、まず URL の誤りを疑うこと (GitHub は無いリポジトリにも認証を求めるので、URL の誤りも `auth` になる。2026-10-16 に確認)。次に、既定のプロジェクトの依存は利用者の端末で `cargo fetch` を実行して資格情報を通すこと、外のツールの `git` は端末で `git ls-remote <url>` が通るかを確かめること。GitHub のリポジトリなら、gh を使っている利用者は `gh auth setup-git` を一度実行すれば、gh の認証が git と cargo に効くこと (cargo は git の `credential.helper` を使う。v0.19)。それ以外は URL と rev の確認。直したら、照会のツールは `refresh`、外のツールは同じ引数での呼び直しで再試行 (suggestions は `reason` ごとに分けて返す。m2-plan.md の D5a。v0.20) | false |
| `toolchain_missing` | ローカルビルドに使う toolchain (R-WS-8) か、ローカルビルドに使う target の `rust-std` (R-TG-1) が手元に無い。または、std の JSON (R-STD-4)、std のソースに使う版の合う `rust-src` (R-SC-2) が手元に無く、取得もできない (v0.17) | 導入コマンド | false |
| `toolchain_too_old` | ビルドに使った toolchain が古く、crate の MSRV を満たさない | 使った toolchain の更新コマンド (`rustup update nightly` など。rustup の無い環境でも rustup のコマンドを書く件は R-WS-7 の既知の残り。v0.20) | false |
| `offline_miss` | オフラインでキャッシュに無い | キャッシュにあるバージョン | false |
| `network_error` | 一時的なネットワーク障害。docs.rs が想定外の HTTP ステータスを返したときもこれにする (v0.17)。TLS の証明書の検証の失敗もこれにする (再試行しない。v0.19) | 再試行 (否定キャッシュに覚えている間は `retry_after_secs` を付ける。R-ERR-3。v0.19)。想定外の HTTP ステータスでは、報告のしかたとサーバーの更新。証明書の検証の失敗では、プロキシの CA 証明書を足すこと (v0.19) | true (想定外の HTTP ステータスと証明書の検証の失敗は false) |
| `internal_error` | 処理中の panic など、サーバー内部の不具合 (R-ERR-1) | ログの場所と報告のしかた (壊れたキャッシュは R-CA-4 でサーバーが取り直すので、`cache clear` は案内しない) | false |
| `project_not_found` | 決めたディレクトリにも直下の子にも `Cargo.toml` が無いか、既定のプロジェクトの候補が複数で `project` が無く、照会を続けられない (R-MCP-3)。外の workspace のディレクトリが消えたときは、記録した別名から引ければ記録で答えて `stale: project_missing` を付け、`project_not_found` にするのは、Cargo.toml が壊れているとき (理由付き)、別名はあるが記録の無いパス、別名も無いパス、外のツールに渡した消えたパスだけ (R-MCP-3。m2-plan.md の E4b-3。v0.20) | 候補の中から `project` で選ぶこと、workspace のディレクトリで Claude Code を開き直すこと、または `build_external_docs` で `project` を指すこと | false |
| `lockfile_outdated` | Cargo.lock が無いか、Cargo.toml と合っていない (R-VR-2) | Bash で `cargo fetch` を実行すると、ビルドせずに Cargo.lock が必要な分だけ更新される (`cargo check` はコンパイルまでするので、Cargo.lock を直すだけなら遅い。`cargo update` と、既存の lock への `cargo generate-lockfile` は全依存を新しい版に上げるので勧めない)。外の workspace (R-WS-10) では、その workspace の端末で Cargo.lock を作るか直すよう先頭に案内する (`cargo generate-lockfile` を例に挙げる。このサーバーは `--locked` でビルドし、Cargo.lock を書かない。m2-plan.md の E4b。v0.20)。既知の残り (M3 の最初の段で直す。m2-plan.md の §7.1 の決定 9。v0.20): 外の workspace では、Cargo.lock が古いときにも `cargo generate-lockfile` を例に挙げている (無いときだけにする) | false |
| `busy` | 同時に処理する呼び出しの数が上限 (R-SEC-7) を超えた | 少し待って再試行 (`retry_after_secs: 1`。v0.19) | true |

- `docs_unavailable` には原因 `reason` を付ける (v0.15。値の一覧は初期案で、M1・M2 で実装しながら確定する。v0.16。M1 で使うものは M1 で確定し、`version_mismatch` と `local_build_required` を足した。v0.19。M2 で残りを実装して確定した。v0.20): `no_json` (docs.rs にも外のツールの記録にも無い。照会のツールに実際のビルド target と違う `target` を渡した場合を含む (R-MCP-3。member と依存で外のツールの記録も無いときは、実際のビルド target の一覧と外のツールに渡す引数を添える。v0.20)。docs.rs の 404 と 400 (版として解析できない値) はこれにする。ただし M1 からは、crates.io の sparse index で crate と版の存在を確かめて、無ければ `crate_not_found`・`version_not_found` を返す (docs.rs の 404 は両者を区別しないため。R-DS-6)。v0.17)・`no_record` (既定のプロジェクトの外で `build_external_docs` の記録が無い。R-MCP-3・R-WS-10)・`record_broken` (外の記録が壊れている。R-WS-10。この 2 つは M2 で実装した。v0.20)・`corrupt` (書き込んだ後に読み直すと内容ダイジェストが合わず、取り直しても合わなかった。ディスクの問題を疑う。再試行でき、待つ時間は 30 秒。R-CA-4・R-ERR-3。v0.19)・`build_unsupported` (`-p` で cargo が失敗する依存。R-WS-5。外のツールがまだ作らない行 (名指しできない path・git の依存など) の「(in a later version)」の応答にも使う。その文言は版を書かずに残し、M3 以降で対応を検討する。m2-plan.md の E1a と M2f の残りの行き先の決定 4。v0.20)・`too_large` (展開の上限。R-NW-3。メモリに置く JSON の上限 512MiB を超えたものも。v0.19)・`unparsable` (展開できないか、JSON として解析できない。先読みした形式が対応範囲内なのに逆直列化に失敗したもの (サーバーの不具合) を含む。展開の段数の上限 (R-DS-4) と入れ子の深さの上限 (R-FV-8) を超えたものも。v0.19。範囲の外の形式は `unsupported_format`。R-DS-2・R-CA-4)・`version_mismatch` (返った JSON の `crate_version` が解決した版と違う。R-VR-4。v0.19)・`local_build_required` (プロジェクトの取得元が crates.io でないか、checksum が sparse index と合わないので docs.rs を使えず、ローカルビルドが要る。R-VR-2。v0.19。M2 でそれらをローカルビルドするようになり、lib の無い依存だけに使う。m2-plan.md の D7a・D7b-2。v0.20)・`scope_unknown` (範囲を判定できずにローカルビルドしなかった。R-MCP-3)・`target_unsupported` (独自の target で std を自分で作るプロジェクト。R-TG-1。v0.17)・`no_target_json` (docs.rs にその版の JSON はあるが、求める target の JSON が無い。R-TG-1。v0.18)

- R-ERR-3: 否定的な結果のキャッシュは種類で分ける。「確定的に存在しない」(目安 1 時間。ただし存在しない版は sparse index の TTL (10 分。R-CA-3) に従う。v0.18)、「crate が見つからない」(公開直後の可能性があるので短め、目安 5 分)、「一時的な失敗」(目安 30 秒)。一時的な失敗を「存在しない」としてキャッシュしない。複数の取得元を順に試すとき、一時的な失敗は保留し、どの取得元も答えなかった場合だけ `network_error` として返す。docs.rs が想定外の HTTP ステータスを返したときも、一時的な失敗と同じく保留して次の取得元を試し、目安 30 秒だけキャッシュする (どこも答えなければ `network_error`。`retryable` と suggestions は上の表。v0.17)
  - M1 の鍵と時間 (m1-plan.md の B3・B6。v0.19): docs.rs の失敗は、JSON の場所の隣の `.miss.json` に、種類・時刻・サーバーの版を書く。JSON が無い・target の JSON だけが無いは 1 時間、一時的な失敗 (429・5xx・想定外のステータス・通信・証明書) は 30 秒、`unparsable` と読める形式が無いは 1 時間 (記録したサーバーの版が違えば無視する。別の版なら読めるかもしれないため)、`corrupt` は 30 秒。壊れた本体 (上限超え・展開の失敗) は覚えない。取れたら記録を消す。覚えている間は問い合わせず、`no_json` などをそのまま返し、一時的な失敗と `unparsable`・`corrupt` には `retry_after_secs` を付ける
  - M2 で足したもの (m2-plan.md の C3c-4・A3b-2。v0.20): sparse index・`.crate`・std の配布物 (要約とアーカイブ) の一時的な失敗 (通信の失敗・5xx・429) も、項目のパスごとの `<パス>.miss.json` に 30 秒覚える (キャッシュの中なので、キャッシュを共有するほかのプロセスにも効く)。オフライン・無い (404)・予期しない状態・大きすぎる・checksum の不一致は覚えない (予期しない状態を覚えると、1 回目は `retryable: false` なのに 2 回目から true に変わるため)。覚えている間は取りに行かず、`network_error` (`retryable: true`) に `retry_after_secs` を付ける。ただし sparse index は、古いキャッシュがあれば待たずにそれを使い、配布物の要約も前に取ったものを warnings 付きで使う。オフラインモード (R-CA-7) の失敗は、どの取得元でも否定キャッシュに入れない (オンラインに戻したらすぐ取れるように)。ローカルビルドの失敗の記録は R-WS-7
  - 「crate が見つからない」は、crates.io の index に無い crate を 5 分覚え、その間は index を引かない (M1 の公開前に入れた。v0.19)
- R-ERR-4: JSON-RPC のエラーコードは、JSON-RPC 2.0 の標準コード (`-32700`、`-32600`〜`-32603`) と、仕様が定める `-32020`〜`-32022` だけを使う。独自のエラーはツール結果の `code` で返す。`-32020`〜`-32099` のうち仕様に無いコードと、廃止された `-32002`・`-32042` は出さない (MUST NOT)。`-32000`〜`-32019` も使わない (SHOULD NOT。`basic/index`)

---

## 9. 非機能要件

### 9.1 性能

測る指標と目標を次に挙げる。目標の値は、M1 (読み込みと照会) と M4 (依存グラフ全体の implementors) のベンチマークで測って決める (v0.16)。M1 の目標は、M1 の測定 (m1-plan.md の E2。12 コア・16GB・ローカルの SSD の Linux、release ビルド、gnu 版) から決めた (v0.19。測る道具は `eval/bench.py` で、M4・M5 でも回す)。

| 指標 | 目標 (v0.19) | M1 の実測 (2026-10-20) |
| --- | --- | --- |
| キャッシュが効いている `get_item` / `list_methods` (同じプロセス) | p50 < 10ms、p95 < 50ms (v0.18 までの目安は p50 < 50ms、p95 < 200ms) | `get_item` p50 1.4ms・p95 4.4ms、`list_methods` p50 3.4ms・p95 4.9ms |
| 新しいプロセスで巨大 crate (core の JSON は約 96MB。v0.18 までの「約 61MB」は以前の nightly) を初めて開く | < 150ms (アーカイブを開くときに検証するので、v0.18 までの目安の < 50ms から緩めた。2 回目からはプロセスの中で使い回す。Q-23。ferritin は検証を省いて約 7ms) | core 88ms (ほかの crate の多くは 6〜12ms) |
| docs.rs から取った JSON の正規化 (解析・索引・アーカイブ。取得を除く) | 中規模の crate (JSON 5〜15MB) < 500ms、core < 3s。取得込みの p50 < 3s は参考 (取得の時間は docs.rs と回線で決まるため。v0.17) | axum (5.1MB) 118ms、tokio (6.5MB) 181ms、rmcp (14.9MB) 258ms、core 1.5s |
| 解析中のメモリの山 | JSON の 6 倍程度 (R-MEM-1 の既定の 1GB に core が収まる) | JSON の約 5〜7 倍 (core 509MB)。解析の後は 33MB 以下、照会だけなら 8MB |
| 依存グラフ全体の implementors (索引あり) | < 1s (時間と容量は Q-5。M4 で測る) | — |
| 約 1GB の JSON (windows-rs) | プロセスを落とさない (扱う方式は Q-22 で M4 以降。性能・メモリの試験ケースに含める) | 試していない。M1 はメモリに置く JSON の上限 (512MiB。R-NW-3) で `too_large` を返す見込み |

目標は gnu 版 (R-DIST-1) で測る。musl 版は標準のメモリアロケータが遅いので、M5 で測ってから扱いを決める (Q-27)。

M2 の実測 (参考。v0.20): ローカルビルドの時間と大きさは、目標でなく参考として記録する (目標の値は M5 の eval で決める。m2-plan.md の §7.1 の決定 7)。この値から `build.jobs` と `build.wait_secs` の既定を決めた (R-WS-1・R-WS-6)。測る道具は `eval/build_bench.py` (CI では手で回す workflow `measure.yml`)。手元は 12 コアの Linux、release のサーバー、rustc 1.99.1 (stable)、利用者の `~/.cargo` をオフラインで (2026-10-23。m2-plan.md の F5a)。CI は GitHub の runner (CPU は ubuntu 4・macOS 3 (arm64)・Windows 4) で各 1 回 (runner の速さは揺れる。F5c)。時間は member を引く 1 回の呼び出しのもの。

| 測ったもの | 手元 | CI (ubuntu / macOS / Windows) |
| --- | --- | --- |
| このリポジトリ (Cargo.lock に 274 package) の冷えたビルド | `-j` 1: 113〜115 秒、2: 73〜76 秒、3: 65 秒、6: 56〜58 秒、12: 51 秒 | `-j` 1・2・4 で 97・55・48 秒 / 181・128・102 秒 / 231・142・114 秒 |
| 同じく、編集の後の作り直し (作り直すのは member の 1 unit だけ) | 6.7〜7.4 秒 (`-j` 6)。応答の後の dep-info の記録は約 6 秒 | 6.2 秒 / 7.8〜15.1 秒 / 10.9〜13.9 秒 (dep-info は 5.5〜14.6 秒) |
| 同じく、鮮度が合ったときの照会 | 同じプロセスで 0.0 秒、別のプロセスで 0.8 秒 (debug のサーバー。m2-plan.md の D4d) | — |
| eval の fixture の冷えたビルド | `-j` 2: 19 秒、6: 15 秒、12: 14 秒。作り直しは 0.3 秒 | `-j` 1・2・4 で 24・14・13 秒 / 41・21・12 秒 / 53・31・25 秒 |
| eval の fixture の依存の最初の照会 (docs.rs に target の JSON が無ければローカルビルド。順に引き、後のものは前の target dir を使う) | — | docs.rs から読む (tokio 1.1 秒、ほかは 0.2 秒以下) / tokio 7.6・axum 14.3・serde 1.9 秒 / tokio 11.1・axum 16.5・serde 2.7 秒 |
| target dir の大きさ | このリポジトリ 535MB (ほかに `local/v1` の記録と JSON が 47MB)、eval の fixture 178MB | このリポジトリで 542MB / 319MB / 395MB |

- R-PF-1: JSON の解析結果を、mmap できるアーカイブ (rkyv など) として保存する。item は必要になったときに初めて取り出す。上流の `rustdoc-json-types` にある `rkyv_0_8` feature も評価する
  - M1 の形 (m1-plan.md の B9・C1a・E2。v0.19): rkyv 0.8 と rustdoc-types の `rkyv_0_8` を使い、zero-copy にする。アーカイブを mmap し、開くときに一度だけ bytecheck で検証して (R-CA-4)、以後はコピーせずに読む。item は引かれたときにだけ rustdoc-types の型に戻し、その照会の間覚える。開いて検証したアーカイブは、プロセスの中で 32 個まで持って使い回す (mmap なので R-MEM-1 では数えない)。アーカイブを書けなければ、メモリの上のアーカイブで続ける
  - toolchain のコマンド (`rustup run nightly rustc -vV`・`--print sysroot`、プロジェクトの `rustc -vV`) の出力は 60 秒持つ (std の照会が毎回 50〜100ms かかっていたため。セッションの途中で toolchain を入れたら、最大 60 秒遅れて気付く。ローカルビルドでの扱いは R-WS-8。v0.19)
- R-PF-2: 逆引き索引と検索索引も同じ場所に保存する。M1 は検索の索引をアーカイブと一緒に作って持つ (core でアーカイブが 100MB から 113MB に増えた)。逆引き索引は M4 (R-TI-4。v0.19)

### 9.2 メモリ

- R-MEM-1: プロセスのメモリ上限を設定できるようにする (既定は 1GB)。mmap したページは数えない (カーネルが回収できるため)。いま処理中のクエリが使っている crate は捨てない。OS ごとに何を測るか (案: Linux は `RssAnon`、macOS は `phys_footprint`、Windows は private bytes) と、超えたときの捨て方は M5 で測って決める (v0.19 で M5 にした。M1 は `limits.memory` もまだ持たない。M1 で測った匿名メモリは §9.1)
- R-MEM-2: 解析直後の大きな構造体は、アーカイブを書き終えたら直ちに手放す。上限を超える単一の crate (windows-rs の約 1GB の JSON) でもプロセスを落とさない。扱えなければ理由付きで `docs_unavailable` (`reason: too_large`) を返す。扱う方式 (ストリーミングで索引を作るかなど) は Q-22 で、M4 以降に決める (v0.19)
  - 手放したメモリを glibc の malloc は OS に返さない (core を解析した後に 220MB 残った) ので、Linux の glibc ではアーカイブを作り終えたら `malloc_trim(0)` を呼ぶ (core の後は 33MB になった。M1。Q-27。v0.19)

### 9.3 ディスクキャッシュ

- R-CA-1 配置: プロジェクトをまたいで共有する。場所は R-DIST-3 と同じ規則で決める

  | OS | 既定の場所 |
  | --- | --- |
  | Linux / macOS | `$XDG_CACHE_HOME/chatora-rustdoc-mcp/` (`XDG_CACHE_HOME` が未設定なら `~/.cache`) |
  | Windows | `%LOCALAPPDATA%\chatora-rustdoc-mcp\cache\` |

  - 環境変数 `CHATORA_RUSTDOC_MCP_CACHE__DIR` (設定の `cache.dir`。名前の規則は R-DIST-3) で場所を変えられるようにする。相対パスが渡されたら、起動を止めずに既定の場所を使い、そのことをログで知らせる (起動するディレクトリで場所が変わらないように。M1 の公開前の見直しで、起動しなかったのを改めた。v0.19)
  - キャッシュ・state (R-DIST-3)・ログ (R-OBS-2) のディレクトリの権限と所有者は扱わない。umask のまま作り、所有者も確かめない (cargo の `target/` や `~/.cargo`、rust-analyzer と同じ。既定の場所はホームの下にあり、ホームの権限が他の利用者から守る。v0.15)。他の利用者が読み書きできる場所に置かないよう README で案内する (R-SEC-2)
  - ログ (R-OBS-2) はキャッシュに置かない。プロセスをまたぐロック (`locks/`)・子プロセスの記録 (`procs/v1/`)・作業用ディレクトリ (`work/<実体 ID>/`)・使えない target dir の印 (R-WS-2) は、キャッシュの下に置く (キャッシュを共有するプロセスがすべて見られ、state の場所が違っても排他が効くため。v0.19 までの「ビルドのプロセスの記録は state に置く」を改めた。m2-plan.md の C1・C4 の決定 1。v0.20)
- R-CA-2 キー: `{source}/{crate}/{version}/{target}/{format_version}.json` を元データとし (`{crate}` は R-PR-1 で正規化した名前。v0.18) (std の `{version}` には rustc の commit-hash を使う。nightly は同じ版の文字列が約 6 週間続くため)、派生物 (アーカイブ・索引・展開したソース) を同じ場所に置く
  - 派生物のファイル名とヘッダには、マジックナンバー、スキーマの版、format_version、アーキテクチャ、**元データの内容ダイジェスト**を含め、合わなければ作り直す (mtime だけに頼らない)
  - スキーマの版には、アーカイブ形式を決めるライブラリ (rkyv) の互換な版の系列 (例: 0.8 系) も含め、Cargo.lock の rkyv の版と一致することをテストで強制する。依存の更新でアーカイブの配置が黙って変わるのを防ぐ (ferritin の `RKYV_SERIES` に倣う)
  - M1 の配置 (m1-plan.md の B1・B6・B8・B9・C1a・C1d-2 と公開前の見直し。v0.19): docs.rs の JSON は `docs.rs/<名前>/<版>/<target か _default>/<format_version>.json` とその SHA-256 の `.sha256` (`_default` は docs.rs の既定の target。名前は小文字にして `_` を `-` にしたもの)。sparse index は `index.crates.io/<index のパス>` と取得の記録の `.meta.json`。`.crate` は `static.crates.io/<鍵>/<版>/`。std は `std/<短いコミット>/<target>/` (手元の nightly のアーカイブは `std/local/<コミット>/<target>/`)
  - M1 のアーカイブ: 名前は `<形式>.<元の JSON の SHA-256 の先頭 16 文字>.v<スキーマの版>t<rustdoc-types の形式>.rkyv` (例: `v5t61`)。128 バイトのヘッダに、マジックナンバー、スキーマの版、rkyv の系列 (0.8)、配置 (リトルエンディアン・32 ビットの相対ポインタ。上の「アーキテクチャ」に当たる)、`types_version` (アーカイブの型を決める rustdoc-types の `FORMAT_VERSION`)、元の JSON の SHA-256 を持ち、開くときに比べる。元の JSON の形式は SHA-256 が決めるので、照合から外した。`types_version` を名前にも入れるのは、rustdoc-types を上げた版のサーバーが、古いアーカイブを新しい型として読んだり、同じ名前のファイルを置き換えにいったりしないため (Windows はほかのプロセスが mmap しているファイルを置き換えられない。R-CA-4。公開前の見直しで足した)。JSON の SHA-256 は隣のファイルにあるので、JSON を読まずにアーカイブを探せる。M2 の終わりのスキーマの版は 8 (`v8t61`。v0.20)
  - M2 で足した配置 (m2-plan.md の C1・C4・D2・D4・E1b〜E4c・F1a。v0.20):
    - `t/<12 桁>/`: 専用 target dir (R-WS-2)
    - `local/v1/<workspace のルートの 12 桁>/<package id の 12 桁>/<target>/<lib か bin-<名前>>/`: ローカルビルドの JSON (`<format_version>.json` と `.sha256`)・そのアーカイブ・ビルドの記録 `record.json`・失敗の記録 `failed.json` (R-WS-7)。12 桁は SHA-256 の先頭。外のツールが既定のプロジェクトの中で作るものは、名前に `.apart` を付けて照会のツールのビルドと分ける (R-WS-10)
    - `ext/v1/<種類>/<鍵のハッシュ>/`: 外のツールの記録の索引 (R-WS-10。種類は `release`・`git`・`git_ref`・`target`・`workspace`・`workspace_target`)。ほかに、名前を変えた workspace の別名 (`alias`) と、git の crate ごとの commit の一覧 (`git_commits`)
    - `toolchains/v1/`: toolchain の rustdoc の形式を調べた記録 (R-WS-8)
    - `locks/`・`procs/v1/`・`work/<実体 ID>/`・`work/gc/`: ロック (R-CA-4)・子プロセスの記録と作業用ディレクトリ (R-WS-6)・GC が消す前に移す場所 (R-CA-5)
    - ルートの所有マーカー `chatora-rustdoc-mcp.cache` と `CACHEDIR.TAG` (R-CA-5)
  - ローカルビルドの置き場は、crate (と target、lib か bin) ごとに 1 つで、最後のビルドだけを置く。鍵 (R-WS-4 の内容ダイジェスト) と入力の一覧は、置き場の `record.json` に持つ (v0.19 までの「元データの鍵は内容ダイジェスト」を改めた。m2-plan.md の D4。v0.20)
  - 派生物は rustup の toolchain ディレクトリ (読み取り専用でありうる) やユーザーの `target/` に書かず、すべてキャッシュの下に置く
- R-CA-3 不変性:
  - docs.rs の完全一致バージョンの取得済み JSON は不変として扱う (JSON が無いという結果は R-ERR-3 の TTL に従う。再ビルドで現れることがある。R-DS-3)
  - crates.io の sparse index のファイルの TTL は 10 分とし (index の `cache-control: max-age=600` に合わせる)、TTL が切れたら、保存した ETag を `If-None-Match` に付けた条件付きのリクエストで確かめ直す (ETag が無ければ `Last-Modified` を `If-Modified-Since` に付ける)。`304` なら保存した中身をそのまま使って TTL を延ばし、`200` なら中身と ETag を置き換える。新しい版の公開に早く気付きつつ、変わっていない crate の取り直しを本文なしで済ませるため (serde の index は 164,874 バイトで、ETag を付けると `304` で 0 バイトだった。2026-10-19 に確認)。index から導く「latest の解決結果」(semver 要件の解決も同じ) は独自の TTL を持たず、index を確かめ直したときに導き直す (v0.16 までの 1h の TTL は v0.17 で消した)。ミラー (R-CA-8) が条件付きのリクエストに応じなければ、毎回 `200` で取り直すだけで、結果は変わらない
    - TTL の切れた index を取り直そうとして通信できなかったとき (通信の失敗・一時的な失敗・想定外のステータス) は、オフラインモードでなくても、キャッシュの index を使い、新しい版が抜けうる旨を warnings に出す (M1 の終わりに利用者が決め、公開前に入れた。v0.19)
  - ローカルビルドの結果は鮮度判定 (R-WS-4) で管理する
- R-CA-4 整合性:
  - **すべての書き込み** (元データの JSON を含む) は一時ファイルに書き、fsync してからアトミックに rename する。一時ファイルの名前はプロセスごと・書き込みごとに一意にする (PID だけ、固定の `.tmp` だけでは衝突した先例がある)
  - アーカイブの展開 (`.crate` など) は最後に完了印のファイルを書く。完了印の前に、展開した中身を fsync する (v0.18)。印が無ければ未完了として作り直す
  - 取得した本体は、検証 (少なくとも format_version の先読み。R-FV-3) してからキャッシュに書く (v0.18)。M1 では、先読みできない本体 (プロキシのエラーページなど) も呼び出し元には返して `unparsable` とし、キャッシュには書かず、次の照会で取り直す (v0.19)
  - 書きかけの一時ファイルと、中断したビルドの作業用ディレクトリは後始末する。他のプロセスが使っているものは消さない。一時ファイルは古さで判定する: 書き込みが成功したときに、同じディレクトリの一時ファイルのうち更新が 1 時間より古いものを消す。二度と書かない場所に残ったものは GC (R-CA-5) に任せる (M1 で決めた。v0.19)。作業用ディレクトリはキャッシュの `work/<実体 ID>/` で、持ち主の止まったものは起動時の掃除が消す (R-WS-6。m2-plan.md の C4a。v0.20)
  - 同じキーへの同時書き込みとビルドは、**プロセスをまたぐファイルロック**で排他する (M2 から。M1 のキャッシュは原子的な rename だけで、複数のプロセスが同じキーを重ねて取得するのは許す。結果は同じで壊れないため。v0.16)。取得とビルドは、ロックを取ってからキャッシュを確かめ直し、既にあればそれを使う (待っていた側が、相手の終えた取得やビルドを繰り返さないように。v0.15)
    - M2 で入れた形 (m2-plan.md の C2a・C2c・止まる危険の監査と直し 1。v0.20): OS のロックが取れない (ロックファイルを作れないなど) ときは、ログに書いてプロセスの中の排他だけで続ける (失うのは二重の取得だけで、書き込みは原子的なので壊れない)。キャッシュの項目のロックの待ちには上限を置き、同期の待ちは 1 分、非同期の待ちは 10 分 (持つ側は大きな JSON の取得と解析の間持つため) で諦めて、ロックなしで進む。ビルドの `target:`・`doc:` のロック (下) は上限なしで待つ (過ぎて進むと同名の crate の JSON の上書きが戻るため。止まったプロセスが持てば `building_reason: waiting_for_lock` のまま)
  - **版の違うサーバーの同居** (v0.15): プラグインの更新の後は、古いセッションのサーバーと新しいサーバーが同じキャッシュと state を同時に使う。サーバーが書く記録は形式の版を持ち、版の違う記録は壊れたとみなさず、無いものとして扱う (互いに取り直しを繰り返さないため)。ロックの名前には形式の版を入れない (版の違うサーバーどうしでも排他するため)。M2 で決めた形 (Q-29。m2-plan.md の C1・C4 の決定 6・F1b。v0.20): 形式の版は、記録の中身でなくパス (`local/v1`・`ext/v1`・`procs/v1`・`toolchains/v1` の `v<版>`) に入れる (中身に入れると、互いに相手の記録を無いものとみなして書き直し合うため)。キャッシュの下の古い版の置き場は、GC が target dir の保持日数 (既定 30 日) 使われなければ消す (R-CA-5)。子プロセスの記録は、持ち主が止まって更新から 7 日経った他の版のものを、子孫を止めずに記録だけ消す (Q-29 の案「読めない記録には触らない」を改めた)。M1 からの記録 (JSON のダイジェストの `.sha256`・否定キャッシュ) は版を持たず、形式を変える段で版を付ける。既知の残り: 版の違うサーバーを並べて動かす結合テストがまだ無い (M3 の最初の段で足す。m2-plan.md の §7.1 の決定 5)
  - **プロセスをまたぐロックの原則** (v0.15):
    - ロックはすべて OS のロック (Linux・macOS は `flock`、Windows は `LockFileEx`) で取る。ファイルがあるかどうかでロックを表さない
    - ロックファイルは GC でも `cache clear` でも消さない (消すと、消される前に開いて待っていたプロセスと、後から作り直したプロセスが両方ともロックを取れてしまう。空のファイルなので溜まっても小さい)。例外はログのロック (R-OBS-2) で、プロセスごとのファイルなので、古い日付のものは消す (v0.19)。もう 1 つの例外はサーバーの生死を表す `owner:<実体 ID>` のロック (R-WS-6) で、持ち主が止まった後に起動時の掃除が消す (その ID をほかのサーバーが使うことは無いため。m2-plan.md の C4a。v0.20)
    - M2 で決めた細部 (Q-29。m2-plan.md の C1・C2c・D2・F1b。v0.20):
      - 置き場所はキャッシュの `locks/` (state ではない。R-CA-1)。名前は、種類の前置きを付けた鍵 (`cache:<キャッシュの中のパス>`・`target:<target dir の名前>`・`doc:<target dir の名前>`・`ext:<鍵のハッシュ>`・`owner:<実体 ID>`・`gc`) の SHA-256 の先頭 32 桁に `.lock` を付けたもので、形式の版を入れない。名前の作り方はテストで固定する。Unix では `O_NOFOLLOW` と 0600 で開く。NFS では効かないことがある旨を README に書く
      - 取る順は、仕事の鍵 → target dir (`target:`) → `doc:<target dir>` → ビルドの枠 (プロセスごと。R-SEC-7)。target dir のロックは、ビルドが共有、GC と使えなくなった古い target dir の削除が排他で取る。`doc:` は排他で、cargo の実行から JSON の移動と dep-info まで持つ (同じ target dir の同名の crate の JSON と dep-info の上書きを防ぐ)
      - GC と `cache clear` は、消す置き場ごとに書き手と同じロックを試し取りし、取れなければ使用中として飛ばす (R-CA-5)
  - mmap して検証なしで読むことはしない。rkyv の `bytecheck` で検証するか、長さとチェックサムを持つヘッダで確かめる (途中で切れたファイルを mmap すると未定義動作や SIGBUS になる)。M1 は開くときに bytecheck で検証する (R-PF-1)。キャッシュのファイルは rename でしか置き換えない前提で mmap する (他のプロセスがその場で切り詰めると SIGBUS で落ちうる。v0.19)
  - 派生物は「壊れていても安全」であること。読めなければ元データから作り直す
  - 元データ (docs.rs・std の JSON、ローカルビルドの JSON) は、書き込むときに内容ダイジェストを記録する。読むときにダイジェストが合わなければ壊れたとみなし、捨てて 1 回だけ取り直す (ローカルビルドなら作り直す。外のツールで作ったものは R-WS-10)。取り直しても壊れていれば `docs_unavailable` を返し、照会のたびには取り直さない (否定キャッシュ)。取り直したことは warnings とログに出す (キャッシュの回復を人やエージェントの `cache clear` に頼らず、どの transport でも効くように)
    - M1 で決めた分け方 (m1-plan.md の B6。v0.19): ダイジェストは JSON を書いた後に書く。ダイジェストの無い JSON (古いサーバーが書いたもの、書く前に止まったもの) は無いものとして取り直す。取得時の検証 (先読み) に通らない本体は、キャッシュせず取り直しもしない (上のとおり)。ダイジェストが合うのに解析に失敗したら (`unparsable`)、キャッシュから消して 1 回だけ取り直し、同じく解析できなければ否定キャッシュに覚える。非対応の形式は R-FV-6。否定キャッシュの鍵と時間は R-ERR-3
  - キャッシュの読み書きでは symlink をたどらない。キャッシュの中に symlink を見つけたら、その派生物を壊れたものとして作り直す。キャッシュのディレクトリ自体は symlink でもよい (v0.19)
    - M1 の読み出し (m1-plan.md の A5。v0.19): キャッシュのディレクトリからファイルまでの間のディレクトリの symlink もたどらず、通常のファイルだけを読む。Unix では `O_NOFOLLOW` と `O_NONBLOCK` で開き、開いたハンドルが確かめたファイルと同じ (dev と inode) で通常のファイルであることを確かめる (確かめた後に symlink や FIFO に差し替えられても、たどらず、止まらない)。Windows には symlink を断るフラグが無く、ファイルの同一性を見る API も安定していないので、確かめた時点の検査だけになる。書き込みと削除は、キャッシュのディレクトリの下でなければ拒む
  - Windows では、他のプロセスが mmap しているファイルは rename で置き換えることも削除することもできないので、派生物は上書きせず新しい名前で書く (R-CA-2)。M1 の std の片付け (R-STD-6) は、消せないものを残して続ける (v0.19)。GC と `cache clear` は、置き場をまずキャッシュの `work/gc/` へ rename してから消す。Windows で写像中のファイルがあれば rename が失敗して置き場は丸ごと残り (半端には残らない)、次の GC で試す。`work/gc/` に消し残したものは次の GC が消す (Windows で rename が失敗することを CI で確かめた。m2-plan.md の F1b・F1c-2。v0.20)
- R-CA-5 GC: 容量の上限を超えたら、最後に使ってから最も時間の経ったものから消す。上限は 2 つに分ける: 文書のキャッシュ (元データ・派生物・展開したソース・OSV のミラー) と、専用 target dir (R-WS-2。仮のプロジェクトのものと、外のツールの記録 (R-WS-10。記録した文書とメタデータ) と、照会のツールのローカルビルドの文書 (`local/v1`。ビルドでしか作り直せないため。v0.20) を含む) で、target dir は最後に使ってから一定の日数を過ぎたものも消す (大きな target dir が文書のキャッシュを追い出したり、文書の GC が target dir を追い出してコールドビルドを繰り返したりしないため)。どれも設定で変えられる。初期値は 5GB・20GB・30 日で、M5 の実測で見直す (v0.16)。既定の上限と変え方を README に書く (M2 で書いた。v0.20)。GC (CLI の `cache gc` と容量の上限) は、target dir が増え始める M2 で入れた (v0.20)。ただし std は nightly ごとに約 175MB 増えるので、7 日使われていない nightly の std を消す片付け (R-STD-6) を M1 で先に入れた (M2 で GC の一部にした。v0.20)。古い配置のキャッシュ (M1 の途中で配置を変える前のもの) は読まれずに残るので、GC が消す (v0.20)。CLI に `cache ls` / `cache gc` / `cache clear` を用意する (サーバーが動いていなくても GC と後始末ができるように。ロックはサーバーと同じものを取る)。`cache clear` に `crate@版` を渡すとその分だけ消す (不変として扱う docs.rs の JSON が壊れていて、後から直った場合などに取り直すため)
  - キャッシュのルートに**所有マーカー** (このツールが作ったことを示すファイル) を置き、マーカーの無いディレクトリは消さない。設定の誤りで別のディレクトリを指したときの誤削除を防ぐ (ruskel の `cache/layout.rs` に倣う)
    - M2 の形 (m2-plan.md の F1 の決定 5 と F1a。v0.20): マーカーは見えるファイル `chatora-rustdoc-mcp.cache` (中身は人向けの説明) で、通常のファイルとしてあることだけで判じる。新しいキャッシュには最初から書く。マーカーの無い既存のキャッシュ (M1 の版が作ったもの) は、中身がこのサーバーの置き場の名前 (とその一時ファイル、`CACHEDIR.TAG`、`.DS_Store` など OS が作るもの) だけで、`docs.rs`・`index.crates.io`・`static.crates.io`・`toolchains`・`procs` のどれかがあれば、マーカーを書いて引き取る (`t`・`work` のような他の道具も使う名前だけでは引き取らない)。引き取りはサーバーの起動時 (ローカルビルドの機能が無い構成でも) と `cache gc`・`cache clear` で行う。知らないものがあれば引き取らず、ログに書いて何も消さない。一番上の symlink は知らないものとみなす
  - キャッシュのルートに `CACHEDIR.TAG` を置き、バックアップツールの対象から外れるようにする
  - 削除では symlink をたどらない。ロック中 (ビルド中) の専用 target dir は消さない
  - M2 で入れた GC と CLI (m2-plan.md の F1・F1a〜F1c。v0.20):
    - いつ走るか: CLI の `cache gc` と、サーバーの裏。裏の GC は、起動時の引き取りと掃除の後に 1 回と、ビルドを公開した後にプロセスごとに 1 時間に 1 回まで走り、在庫を 30 秒まで見る。他のプロセスが GC 中 (`gc` のロック) なら飛ばす。在庫を全部見られなかったときは、見た分に日数の規則と古い配置の規則だけを当て、容量の上限は当てない。ローカルビルドの機能が無い構成は文書の側だけを GC する
    - 最後に使った時刻: 置き場の `used` のファイルで判じる (読んだときに、プロセスの中で場所ごとに 1 時間に 1 回まで書く)。sparse index と `.crate` は `used` を書かず、取得の記録と時刻で判じる。最後に使って 1 時間以内のもの (印は 1 時間に 1 回なので、印の古さで 2 時間以内) は、上限を超えていても消さない。今より後の時刻は今使ったとみなす
    - 消すもの: 分類ごとに上限を超える分を、最後に使ったのが古い順に。target dir・`local/v1`・外の記録と、他の形式の版の置き場 (`local/v2` など) は、保持日数を超えて使われなければ消す。古い配置は、この版が知っている形だけを常に消す。7 日使われない nightly の std も消す (R-STD-6)。外の記録は、それだけが指す `local/v1` の本体と 1 組にし、記録を先に消す。知らない名前・`locks/`・`procs/`・`work/` (`work/gc/` を除く)・`toolchains/` (文書の側に数えるが消さない)・git の commit の一覧・止まったサーバーの子孫が使う target dir には触らない。外の workspace の別名は、指す記録が消えたものだけを消す
    - 使用中のもの: 置き場ごとに書き手と同じロック (R-CA-4) を試し取りし、取れなければ飛ばして表示する。在庫を取った後に使われたものも飛ばす。サーバーが動いている間も CLI を走らせてよい
    - 設定: `cache.docs_max_size`・`cache.target_max_size`・`cache.target_max_age_days` (R-DIST-3)。誤った値と日数の 0 は、既定に戻してログと `cache ls` に出す
    - `cache ls`: 人向けの表 (分類ごとの使用量と上限、保持日数を超えた target dir の数、大きいもの 10 件と最後の使用と説明)。JSON は出さない。大きさは見かけの大きさで、Unix の hard link は 1 回だけ数え、知らないものは数えない
    - `cache gc [--dry-run]`: 消したもの (dry run では消すもの) を理由と大きさ付きで 20 件まで、使用中で飛ばしたもの、消せなかったものを表示する
    - `cache clear [--dry-run] [<crate>[@<version>]]`: 引数なしは、文書と target dir の分類のすべて (docs.rs・index・`.crate`・std・`toolchains/`・`t/`・`local/`・`ext/`、古い配置と他の形式の版) を消す。`locks/`・`procs/`・`work/`・マーカー・`CACHEDIR.TAG`・知らない名前は残す。`crate@版` は、その release の docs.rs の JSON (全 target) と派生物、`.crate`、外のツールの記録 (crates.io のリリース・git・他の target の種類と、それだけが指す本体) を消す。照会のツールがプロジェクトの依存として作ったローカルビルド (鮮度の判定と `refresh` がある) と外の workspace の記録は消さない。版を省くとその crate の全版と、sparse index のその crate のファイルも消す。版は完全な版だけを受け、build metadata を省けば無視して照合する。clear は 1 時間の猶予を当てない (ロックで使用中のものは飛ばす)
    - 終了コード: 0 はすべて消せたか、当たるものが無いとき。1 は自分のものでないディレクトリ (何も消さない)、他のプロセスが GC 中、`cache clear` で使用中か消せずに残ったものがあるとき。2 は使い方の誤り (完全でない版、`std` などの std の crate、引数の重なり)
- R-CA-6: 任意の設定として、プロジェクトの Cargo.lock にある依存を、サーバーの起動時にまとめて先に取得する (既定は無効)
  - 任意の設定として、Cargo.lock の内容ダイジェストが変わったら、増えた依存だけを裏で取得する (既定は無効。rust-mcp の自動索引の利点)
- R-CA-7 オフラインモード: ネットワークに一切出ない (cargo を呼ぶときは `--offline` を付ける。R-VR-2)。キャッシュに無ければ `offline_miss` を返す。`cargo metadata` が失敗したときの版の解決は R-VR-2。TTL の切れた解決結果 (latest・index) もキャッシュにあれば使い、取得した日時を provenance に載せて、古い可能性を warnings で示す (オフラインモードでないときに通信できなかった場合も、index は同じく使う。R-CA-3。v0.19)。オフラインモードの設定 `network.offline` は、設定ファイルと一緒に M2 で入れた (m2-plan.md の A3b-2。v0.20):
  - sparse index は TTL を過ぎていても使い、TTL を過ぎた index で版を決めたときは取得した日時を provenance の `index_fetched_at` (UTC の RFC 3339) に載せる (通信できずに古いものを使ったときも)。古い index に無い版は、オフラインなら `offline_miss`、通信の失敗なら `network_error` を返す (R-VR-2)
  - docs.rs の JSON はキャッシュに無ければ `offline_miss`。std の配布物の manifest は古くても使い、無ければ導入コマンドを添えた `offline_miss` (R-STD-4)。`.crate` が無ければ、docs.rs のビルドの feature が分からない旨の warnings だけで照会を続ける
  - `offline_miss` の suggestions は、その crate の docs.rs の JSON がキャッシュにある版を新しい順に 5 つまでと、`network.offline` を外す案内。オフラインによる失敗は否定キャッシュに残さない (オンラインに戻したときにすぐ取れるように)
- R-CA-8 隔離環境 (任意。M5 で検討): 取得元 (docs.rs・crates.io の sparse index と API・static.crates.io・static.rust-lang.org・GitHub API・OSV) のベース URL は、今のうちから 1 か所で持つ (後から社内のミラーに差し替えられるように)。差し替えと取得元ごとの無効化の設定、プロキシ、`http://` のミラーの扱いは M5 で決める (v0.16)

### 9.4 ネットワークの作法

- R-NW-1: crates.io の [data access policy](https://crates.io/data-access) と [crawler policy](https://crates.io/policies#crawlers) に従う。**連絡先を含む User-Agent** を送り (先行実装はどれも連絡先が無い)、毎秒 1 リクエスト程度に抑える。この間隔はプロセスごとに守る (対象は crates.io の API。sparse index は CDN なので対象外。複数のセッションのサーバーが同時に動くと合計は超えうるが、API を使うのは crate の検索と DL 数・被依存数などの取得くらいで量が少なく、プロセスをまたいで揃える仕組みに見合わないため受け入れる。v0.16)
  - crates.io の情報の取得元は、sparse index (`index.crates.io`) → static.crates.io の `.crate` → API の順に選び、API は前の 2 つで取れないもの (crate の検索、DL 数、被依存数と被依存の一覧、版ごとのライセンス) だけに使う。data access policy が示す順で、sparse index にはレートリミットが無く、static.crates.io にも今のところ無い (「at present」の条件付き。2026-10-19 にポリシーの原文で確認)。項目ごとの振り分けは §5.9 の metadata の行
  - static.crates.io の `.crate` の取得は、crates.io のダウンロード数に数えられない (2025-10-29 から cargo の User-Agent の要求だけを数え、このサーバーは自分の User-Agent を送るため)。手元の cargo のキャッシュに checksum の合う `.crate` があれば、取りに行かずにそれを使う (R-SC-2。m2-plan.md の B6。v0.20)
- R-NW-2: docs.rs、crates.io の API と sparse index、static.crates.io、static.rust-lang.org、GitHub API、OSV へのリクエスト (User-Agent は R-NW-1 と同じ。crates.io の API の間隔は R-NW-1。crates.io を足したのは v0.16):
  - 接続と本体の受信を分けたタイムアウト。既定は接続 10 秒、本体の受信は無通信の時間で 120 秒 (全体の時間では数えないので、core の約 96MB や windows-rs の約 1GB も、遅い回線で途中で切らない)。設定で変えられる
  - 再試行は一時的エラー (429・5xx・通信エラー) だけ。R-DS-2・R-ERR-3 の一時的エラーもこの範囲を指す。`Retry-After` に従い、指数バックオフで行う。M1 の初期値: 429・5xx・接続の失敗・タイムアウトを最大 3 回まで送り直す。待ち時間は `Retry-After` (秒の形だけを読む) か、0.5 秒から倍にしていく。`Retry-After` が 5 秒を超えたら、待たずに一時的な失敗として返す (1 回の照会の時間の上限を食いつぶさないため。v0.19)
  - GitHub API の rate limit は、403 と `x-ratelimit-remaining: 0` で分かる (v0.18)。当たったときは次の順にする (v0.19):
    1. token を持たずに当たったら、R-NW-4 の手順で token を取り、その要求を 1 回だけやり直す
    2. それでも当たったら (token を使っていた場合と、token を取れなかった場合)、待たずに `network_error` (`retryable: true`) を返し、`retry_after_secs` に `x-ratelimit-reset` までの秒数を入れる。token を取れなかったときは、token の渡し方も案内する
    - v0.18 の「`x-ratelimit-reset` まで待つ」は改めた。reset は最大 1 時間先で、ツールの呼び出しの中で待つと止まって見えるため。`Retry-After` が 5 秒を超えたら待たずに返す規則 (上) と揃えた
  - 同じリクエストの重複抑止。M1 は、キャッシュのパスごとにプロセスの中のロックを取り、同じものを求める 2 つ目以降の呼び出しは、1 つ目が終わるのを待ってからキャッシュを見る (取得も解析もしない)。1 つ目がキャンセルされたら、2 つ目は自分で取り直す。取得を共有して全員がキャンセルしたときだけ止める形 (singleflight。R-WS-6) は、M2 で入れた (m2-plan.md の C2b・C3a・C3c。v0.20): docs.rs の crate は取得からアーカイブの作成までを、std は配布物の取得から索引の作成までを、プロセスの中の 1 つの共有の仕事にし、待つ側がすべて取り消したときだけ止める (時間の上限で切り離した後は止めない。R-WS-6)。プロセスをまたいでは、キャッシュのロック (R-CA-4) を取ってから確かめ直す。`.crate` と sparse index は小さく速いので共有せず、一時的な失敗 (通信の失敗・5xx・429) を項目ごとのファイルに 30 秒覚えて、その間は取りに行かない (キャッシュを共有するほかのプロセスにも効く。古い index があればすぐそれを使う)
  - 同時に動かす数の上限 (M1 の公開前の見直しで足した。v0.19): プロセス全体で、docs.rs からの JSON の取得は同時に 4 つ、JSON の解析とアーカイブの作成は同時に 2 つまで (依存の横断の検索 (R-SR-2) が、取得と解析を上限なしに並べて起動していたため)
  - HTTP のリダイレクトは、同じスキーム・ホスト・ポートのものにだけ従う (10 回まで)。別のホストへのリダイレクトはエラーにする (R-SEC-6。v0.19)
- R-NW-3: 本体は上限付きのストリームで読む (以下の数値は初期値。v0.16)。メモリ上で展開する場合の上限は 512MiB とする (core の約 96MB は収まる。M1 の値。v0.19)。本体の受け取りで先に確保する容量は、`content-length` によらず 64MiB までにし、それより大きい本体は届くにつれて伸ばす (M1。v0.19)。エラー時の本体は 16KiB まで
  - 512MiB を超えるもの (windows-rs は約 1GB) は、M1 では `docs_unavailable` (`reason: too_large`) を返す。v0.18 までの「ディスクへ直接展開する」は、解析のストリーム化 (Q-22) と一緒に M4 以降で決める (M1 の終わりに利用者が決めた。v0.19)
  - アーカイブの展開 (`.crate`・std・OSV の zip。R-SC-2) には、総サイズの上限 (`limits.extract_max_size`。既定 4GiB) を設け、超えたら打ち切って部分ファイルを消し、理由付きの `docs_unavailable` を返す。M1 の std の展開は、合計 2GiB・1 ファイル 512MiB の固定の上限で行った (R-STD-6。v0.19)。M2 で合計の上限を設定 `limits.extract_max_size` (既定 4GiB) にした。1 ファイルの上限 (512MiB。メモリで解析する上限) は固定のまま。M2 でディスクに展開するのは std の配布物だけで、`.crate` の展開は M3 (m2-plan.md の A3b-1。v0.20)
- R-NW-4: GitHub token は任意とする。ログには出さない。token が無くても匿名で GitHub API を使う (公開リポジトリだけを読むので。token は rate limit を緩めるためだけ)。v0.19 で、v0.15 の「設定でだけ受け、`GITHUB_TOKEN` は読まない」を改めた (M1 の終わりに利用者が決めた。m1-plan.md の「R-NW-4 の見直し」):
  - **探す順**: 設定の `github.token` → 環境変数 `GH_TOKEN` → `GITHUB_TOKEN` → gh の設定ファイル (`hosts.yml` の `oauth_token`)。v0.15 の理由 (他のツール向けの token を使わない) は、守りとして弱かった。信頼したリポジトリは hook で環境変数も読める (R-SEC-2) ので、守るべきは送り先にする
  - **送り先**: token は公式の `api.github.com` にだけ送り、ミラー (R-CA-8) には送らない
  - **rate limit に当たったときだけ、OS のキーリングを読む**: 今の gh は既定で token を OS のキーリングに置き (`hosts.yml` に token が無い)、取り出すときにキーリングの許可のダイアログが出うる。そこで、上の順で token が無いまま rate limit に当たったときだけ、`gh auth token` (gh があれば)、次に `git credential fill` で `https://github.com` の資格情報 (gh の `setup-git` のほか、libsecret・Git Credential Manager・macOS のキーチェーンに保存した token も拾える) の順で取り、取れたらその要求をやり直す。これで gh の利用者は設定なしで token が効き、ダイアログが出うるのも稀な rate limit のときだけになる。子は git の依存の取得 (R-WS-6) と同じく端末と askpass の問い合わせを止め、時間の上限を付ける。取れた token はプロセスのメモリにだけ持ち、ディスクには書かない。`git credential approve`・`reject` は呼ばず、git の保存内容を変えない。どちらでも取れなければ、token の渡し方 (`export GH_TOKEN=$(gh auth token)` か設定) を案内する
  - **GitHub CLI (gh) は必須にしない** (利用者が決めた): GitHub API は匿名でも読める公開リポジトリにだけ使い、token は rate limit を緩めるためだけなので、導入の条件を gh にしない。`gh api` を毎回子で呼ぶ案は、キーリングのダイアログが出うるうえ、ETag・rate limit の待ち・ミラー・オフラインの扱いが gh の出力の解析になるので採らない
  - git の依存の取得 (子の cargo) は別の話で、サーバーは token を扱わない。利用者の資格情報が使われ、認証で失敗したら利用者の端末で通すよう案内する (R-WS-6・R-WS-7。§8 の `fetch_failed` の `auth` の案内に `gh auth setup-git` を足した)。GitHub 以外 (GitLab・Bitbucket・自前のサーバー) は cargo と git の設定に任せる
  - 信頼したリポジトリの `.claude/settings.json` の `env` は、GitHub API のベース URL (と設定ファイルの場所) も変えられるが、同じリポジトリは hook で token を読めるので、このサーバーでだけ止める守りは設けない (R-SEC-2 の考え方。v0.15)。token を探す順と、rate limit のときに gh・git の資格情報を読みうること (キーリングのダイアログが出うること) を README に書く

### 9.5 セキュリティ

- R-SEC-1: **stdout には JSON-RPC 以外を一切出さない**。ログは stderr かファイルに出す (mcp-rustdoc-parser にはこの不具合がある)。clippy の `print_stdout` だけでは守れない (`io::stdout()` への直接の書き込みや、依存 crate の出力は lint に掛からない)。cargo・rustdoc などの子プロセスは stdin を null、stdout と stderr をパイプにして起動し、サーバーの stdin / stdout を継承させない (build.rs や cargo の出力が JSON-RPC に混ざる、または子が JSON-RPC の入力を読み取るのを防ぐ)。1 メッセージを 1 行の compact JSON で出し、メッセージの中に改行を含めない (`basic/transports/stdio` の MUST NOT)。結合テストで、`serve` の stdout の各行が単独で解析できる JSON-RPC のメッセージであることを確かめる (ローカルビルドを伴う呼び出しを含める)
  - M2 で、member を実際にビルドして答える stdio の結合テストでも、stdout の全行が JSON-RPC であることを確かめた。ログのファイルは自前の書き込み専用のスレッドで書く (tracing-appender の `non_blocking` は、終わるときにチャネルが詰まっていると、その `WorkerGuard` が stdout に書くので使わない。m2-plan.md の D2c-2・止まる危険の直し 2。v0.20)
- R-SEC-2: **ローカルビルドと cargo の起動は、確認を求めずに行う** (v0.14 で決定。R-MCP-3)。ローカルビルドは build.rs と proc-macro を実行し、`cargo metadata` と `rustc -vV` もプロジェクトの設定を通じて任意の実行ファイルを動かしうる (`rust-toolchain.toml` の `[toolchain] path` は両方に効き、`.cargo/config.toml` の `build.rustc` と `build.rustc-wrapper` は (`--no-deps` でない) `cargo metadata` に効く。2026-10-15 に実測)
  - **守りの範囲の考え方** (v0.14): 比べる相手は、同じことができる他の経路 (エージェントの Bash、rust-analyzer、利用者自身の cargo)。それらで同じくらい簡単にできることを、このサーバーでだけ止める守りは設けない。stdio の MCP サーバーは起動した相手を信頼する作りで、エージェントは Bash で cargo を実行できる。さらに、プロジェクトの Cargo.toml に依存を書けば、rust-analyzer (IDE か Claude Code の LSP で動いている場合) がそれを取得して build.rs を確認なしに、サンドボックスの外で動かす (path 依存と `file://` の git 依存で確かめた。crates.io の依存とネットワークの git 依存も、同じく `cargo metadata` で取得されると推定。rust-analyzer 0.3.3081、2026-10-16)。v0.13 までの確認とそれを支える仕組み、子プロセスの環境変数の denylist はやめた (一覧は §14 の v0.14)。守るのは、エージェントの意図によらない脅威 (第三者のデータ、ネットワーク、同じマシンの他の利用者、事故) と、このサーバーにしか無い目立たない経路 (下の外のツール) だけ (R-SEC-1・R-SEC-3・R-SEC-4・R-SEC-5・R-SEC-6・R-SEC-7・R-SC-2・R-SC-4・R-CA-4・R-WS-4・R-DIST-3。R-SEC-6 は v0.16 で足した)。同じマシンの他の利用者からの守りは、キャッシュ・state・ログの置き場所 (ホームの下にあることと、共有の場所に置かない README の案内) に任せ、サーバーはディレクトリの権限を扱わない (R-CA-1。v0.15)
  - **外のコードを引き込む操作は、別のツールにする** (v0.14 のレビューで決定): 既定のプロジェクトの外のコードのビルド (R-WS-10 の対象) は、他の経路 (Cargo.toml を書き換えて rust-analyzer に拾わせる) と違って、Edit の許可も差分も要らず、「文書を見る」に見える 1 回の呼び出しで済む。攻撃者は、docs.rs のビルドだけをわざと失敗させて (環境変数 `DOCS_RS` で見分けられる)、ローカルビルドに落とせる。そこでこれらは `build_external_docs` だけが行い、利用者が Claude Code の許可ルールでこのツールにだけ確認を出せるようにする。Claude Code の許可ルールは、MCP のツールを引数では分けられない (括弧付きのルールは読み飛ばされる。文書で確認。2026-10-16)。`_meta["anthropic/requiresUserInteraction"]` は付けない (理由は付録 B)
  - **確認なしにビルドする既定のプロジェクトの範囲** (m2-plan.md の P3・A2。v0.20): 既定のプロジェクト (R-MCP-3) には、同じ git リポジトリの別の作業ツリーで同じ場所にある workspace (subagent の worktree など) と、Cargo.toml の無いサブディレクトリで開いたときに git の作業ツリーのルートまで上へ探した workspace も入る。どちらも同じリポジトリのコードで、エージェントはそこで Bash から cargo を実行できる (上の守りの範囲の考え方)。作業ツリーの判定はファイルを読むだけで行い、git を起動しない
  - **ツールの引数で指した場所のビルド**: git の場合、取り込んだリポジトリの cargo の設定と toolchain の指定は効かない (仮のプロジェクトの git 依存としてビルドするため。R-WS-2・R-WS-5)。`project` で指した外の workspace では、その workspace の設定が効く。clone は cargo が行うので、非公開のリポジトリは利用者の git の認証で取れる。cargo に git の CLI を使わせる設定は強いず、利用者の cargo の設定に従う (m2-plan.md の E3 の決定 3。v0.20)
  - **Claude Code の Bash のサンドボックスは MCP サーバーを覆わない** (Claude Code の文書: 「Claude's file tools, MCP servers, and hooks run outside it」。ローカルの MCP サーバーは利用者の全権限で動く)。サーバーが起動する cargo もサンドボックスの外で動く
  - **子プロセス (cargo・rustdoc) には、サーバーの環境をそのまま渡す** (v0.14 で denylist をやめた。ただしビルドの制御のために外す・足す・上書きする変数は別: R-WS-2 の target dir (`CARGO_TARGET_DIR`・`CARGO_BUILD_BUILD_DIR`)、R-WS-6 の問い合わせの抑止、R-WS-7 の `LC_ALL`・`RUSTUP_AUTO_INSTALL`・`RUSTUP_DIST_SERVER`、R-WS-8 の `RUSTC_BOOTSTRAP`、R-WS-1 の `RUSTC`・`RUSTDOC`・`CARGO_TERM_COLOR=never` (v0.20) と、外す `RUSTUP_TOOLCHAIN` (v0.18)。外のツールの仮のプロジェクトの `RUSTC_BOOTSTRAP` は `1` で (R-WS-8)、cargo 自身の `-Z` (cargo の設定の `[unstable]`) も効かせるが、仮のプロジェクトはキャッシュの下にあり、効く cargo の設定は利用者の `$CARGO_HOME` とキャッシュの祖先のものだけで、自前の std を作る設定 (`build-std`) は cargo の前に断る (R-TG-1。m2-plan.md の E2a。v0.20)。秘密を除く目的の denylist は設けない。v0.17): 悪意のある build.rs は親プロセスの環境を読める (Linux の `/proc/<pid>/environ` など) うえ、同じ依存の build.rs を rust-analyzer や利用者の cargo がすべての環境変数付きで動かすので、このサーバーでだけ除いても守りにならない。`*_API_KEY` のような型で除くと、`env!` で鍵を埋め込むアプリの workspace の文書のビルドを壊す
  - 任意で、OS のサンドボックス (Linux の bubblewrap / Landlock、macOS の `sandbox-exec`) の中でビルドするモードを検討する (v1 では対象外。Windows には実用的な手段が無い)
  - 依存の crate のドキュメントは既定で docs.rs から取る (docs.rs はネットワーク無し・読み取り専用のサンドボックスでビルドしている)。ただし workspace を rustdoc にかけると、その**依存もすべて check され、依存の build.rs と proc-macro が実行される** (2026-10-14 に確認)。ローカルで実行されるコードは、workspace とその依存すべてに及ぶ (§5.1)
  - **MRTR を使う場合**: 今はどの要求にも `InputRequiredResult` を返さない (確認をすべてやめたため)。将来使う場合は、仕様の MUST と SHOULD (`requestState` の完全性など。`basic/patterns/mrtr`) に従う (v0.16)
  - **README に書くこと** (要点。v0.16。M2 の F6 で README の Security の節 (と Projects・Cache の節) に書いた。v0.20)
    - 照会のツールは、既定のプロジェクト (R-MCP-3。同じリポジトリの別の作業ツリーの同じ場所の workspace を含む。v0.20) のコードを、確認なしに、Claude Code のサンドボックスの外でビルドする (build.rs と proc-macro を実行する)。外のツール `build_external_docs` は、エージェントが指したそれ以外のコード (R-WS-10) をビルドする
    - ビルドしなくても `cargo metadata`・`cargo tree`・`rustc -vV` を起動し、プロジェクトの設定が任意の実行ファイルを動かしうる (`cargo tree` も `build.rustc`・`rustc-wrapper` を動かすことを 2026-10-16 に確かめた)。M2 で、プロジェクトの `rustdoc -vV` (toolchain の形式の記録が無ければ、作業用ディレクトリの 1 行の crate を rustdoc にかけて形式を試す。R-WS-8) と、鮮度の入力を集める `git ls-files` (`core.fsmonitor=false`・`GIT_OPTIONAL_LOCKS=0` を付ける。R-WS-4) が加わった。作り直すときは、ビルドの前に `cargo fetch --locked` も走らせる (R-WS-7。m2-plan.md の D1b・D4a・D5a。v0.20)。信頼できないリポジトリで Claude Code を開いて使わず、`build_external_docs` でも指させない
    - 許可ルールの書き方: `allow` に `mcp__chatora-rustdoc-mcp__*` を、`ask` に `mcp__chatora-rustdoc-mcp__build_external_docs` を書く (`ask` を書かずに `*` だけを許可すると、外のコードも確認なしにビルドされる)。これは手動で登録したときのツール名で、プラグイン (R-DIST-5) で入れたときは接頭辞が `mcp__plugin_<プラグイン名>_<サーバー名>__` になる (R-MCP-7) ので、README には両方の登録のルールを書く。名前が合わないと ask が当たらず、許可の画面で「常に許可」を選ぶと外のビルドも確認なしに走る。実際の接頭辞は M5 で Claude Code で確かめる (v0.17)。ask のルールが効かない権限モードがある (端末の `bypassPermissions` と VS Code の auto。実測と追っている Issue は [claude-code-issues.md](claude-code-issues.md))。README には、Claude Code 2.1.301 で確かめた場所と権限モードごとの表を、Issue へのリンク付きで載せた (端末と VS Code の測り直しはしていない。m2-plan.md の F6 の決定 2。v0.20)
    - M2 の F6 で README に足したもの (v0.20): ビルドが利用者に尋ねないこと (端末と askpass の問い合わせは止めるが、SSH agent や資格情報ヘルパーの問い合わせは出うる。R-WS-6)、サーバー自身と子の cargo の通信先 (R-SEC-6)、応答から見えない文字を除くこと (R-SEC-7)、`--no-default-features` の構成は cargo も build script も起動しないこと (R-ARCH-2)
    - 同じことはこのサーバーが無くてもできる (Cargo.toml に依存を書けば rust-analyzer が build.rs を動かし、エージェントは Bash で cargo を実行できる)
    - キャッシュを他の利用者も書ける場所 (`/tmp` など) に置かない。キャッシュ・state・ログは umask のまま作る
    - キャッシュの親のディレクトリの `rust-toolchain.toml` は、キャッシュの下で動かす `rustc`・`rustup` (起動時のホストの target の `rustc -vV` を含む。R-TG-1) に効く。プロジェクトの `rust-toolchain.toml` が入っていない toolchain を指すと、`cargo metadata` のために rustup がそれを導入しうる (R-WS-7 で許している。v0.19)
    - GitHub の token を探す順と、rate limit のときに gh・git の資格情報を読みうること (R-NW-4。v0.19)
    - HTTP のリダイレクトは同じホストにだけ従うことと、プロキシの扱い (R-NW-2。v0.19)
    - 信頼したリポジトリの `.claude/settings.json` の `env` で、このサーバーの設定も変えられる (2026-10-16 に確かめた)
    - 信頼できない PR を扱う CI では使わない (依存の build.rs が CI の秘密を持ったまま動く)
    - Read の deny を設定していても、ソースはこのサーバー経由で読める (パッケージの中身 (R-SC-4) と、`include_str!` などで文書の JSON に入ったファイル)
    - `tools` を列挙している subagent には MCP のツールが渡らないので、`tools` に足す (2026-10-15 に文書で確認)
- R-SEC-3: ソースの読み取りは既知のソースルートの下に限る (R-SC-4)
- R-SEC-4: crate 名 (crates.io の名前の文法) とバージョン (semver) を、ファイルパスや URL に使う前に検証する (先行実装にはツール引数からのパストラバーサルがある)
  - target triple も、キャッシュのパスに使う前に検証する (syphar は検証せずに使っている)。M1 の規則: 英数字・`-`・`_`・`.` だけで、`..` を含まず、英数字で始まる (URL とパスに入れても形が変わらない。v0.19)
  - git の URL (引数 `git`) は `https://`・`ssh://`・`http://`・`git://` を受ける。scheme の無い形 (scp の形 `[user@]host:path` を含む) は `invalid_argument` にし、`ssh://` での書き方を案内する (scp の形は cargo も受けない。2026-10-16 に実測。v0.16)。平文の `http://`・`git://` は通信の途中で取得するコードを差し替えられうるので、その旨を warnings と provenance に出す (拒否はしない。同じ URL を Cargo.toml に書けば cargo も rust-analyzer も取得する (2026-10-16 に実測) ので、このサーバーでだけ止めない。v0.15)。`file://` は受けない (ローカルのパスは `project` で指すため)。`https://`・`http://`・`git://` の URL に userinfo (`user@`・`token@`・`user:pass@`) があれば拒否し、`ssh://` の URL はパスワードがあれば拒否する (provenance やログに資格情報が漏れるため)。URL と `project` のパスに、制御文字・双方向の制御文字・見えない書式文字があれば拒否する (`build_external_docs` の許可の確認画面 (利用者が `ask` を書いた場合) や provenance・ログで、ホストやパスを偽装させないため)。`rev` はコミットのハッシュか `refs/…` として、`branch`・`tag` は git の参照名として検証する。`-` で始まる `tag` など (タグ名には付けられる) は拒否しない (エージェントが渡す値への守りで、同じものを Cargo.toml に書かれれば効かない。v0.14)
    - M2 で入れた形 (m2-plan.md の E3a と E3 の決定 1。v0.20): 文字は、生の文字列で解析の前に確かめる (URL の解析は制御文字やタブを黙って消すため)。見えない文字の表は R-SEC-7 で除く文字と 1 つにする。ほかに次を断る: ホスト名の非 ASCII の文字 (似た文字での偽装を防ぐ。punycode (`xn--…`) で書くよう案内する)、`\` (`https://evil.com\@good.com/…` は画面の見た目と取得先が違う)、書いたホストと解析したホストが違うもの (percent-encoding や数の形のホスト)、空のホストと空の path、`-` で始まる ssh のホストとユーザー (ssh のオプションに読まれる)、query と fragment (Cargo.lock の `git+URL?branch=x#<commit>` の形を貼られたら `branch`・`tag`・`rev` を案内する)。`file://` と scp の形は案内付きで断る。エラーの文に URL を載せない
    - `rev` は 7〜64 桁の 16 進 (小文字にそろえる) か `refs/…` の参照名、`branch`・`tag` は `git check-ref-format` の規則 (`@` だけの名前と `@{` を含む名前を断る) で確かめ、どれか 1 つだけを受ける
    - 記録の鍵は cargo と同じ規則で正規化した URL (末尾の `/`、github.com の path の小文字、最後の `.git`。scheme は残す) で、cargo に渡すのは検証したままの URL
    - `project` のパスの文字の確かめは、`project` を取るすべての照会 (`list_crates` と `crate` なしの検索を含む) で行う
    - 呼び出しの記録 (R-OBS-1) は検証の前の引数を書くので、`git` は受けた URL か受けない理由だけを書き、userinfo は伏せる。rmcp は debug の水準で要求を引数ごと stderr に書くので、ログの絞り込み (`log.filter`) が rmcp を名指ししない限り `rmcp=info` に抑える
  - **feature 名**を cargo の引数に渡す前に検証する。cargo の feature 名の文法に合わないもの、`-` で始まるもの、制御文字を含むものは拒否する (cargo の引数として誤って解釈されないように。snowmead の `features.rs` に倣う)
  - 索引 (SQLite などを使う場合) への問い合わせはすべてパラメータ化し、引数の値を文字列連結しない (MasYaaz は `kind` を SQL に連結しており、インジェクションが可能)
- R-SEC-5: HTTP モードを自分用の常駐プロセスとして使う場合 (M6 以降。R-MCP-1) は、localhost に bind し、`Origin` / `Host` ヘッダを検証する。`Origin` があって許可していない値なら 403 を返す (MUST。DNS rebinding で、ブラウザから localhost に届くのを防ぐ)
  - 認証の有無 (localhost のポートには同じマシンのどのプロセスと利用者からもつながる。仕様の Authorization は SHOULD)、扱えるプロジェクトを限るか、プロジェクトの受け渡し (ヘッダか引数か) は M6 で決める (v0.14 の「最初の段階では認証を設けない」「扱えるプロジェクトは限らない」を v0.16 で M6 に戻した)。Claude Code のプラグインの設定なら、HTTP サーバーの `url` / `headers` でも `${CLAUDE_PROJECT_DIR}` が置換される (v0.16)
  - `cacheScope` をアクセス制御の代わりにしない
- R-SEC-6: サーバーが通信する先は、R-NW-1・R-NW-2 に列挙した取得元 (と R-CA-8 で設定したミラーとプロキシ) に限る。HTTP のリダイレクトは同じスキーム・ホスト・ポートのものにだけ従い、別のホストへのリダイレクトはエラーにする (接続するホストを挙げた README の約束に合わせる。M1 の公開前の見直しで足した。v0.19)。GitHub の token を得るために起動する `gh auth token`・`git credential fill` (R-NW-4) は、資格情報を読むだけで、その token は `api.github.com` にだけ送る (v0.19)。子プロセスの cargo の通信のうち、利用者のプロジェクトが設定したレジストリや git の依存は対象外で、cargo の設定に従う。rustup の自動導入の通信も子プロセスのもので、R-WS-7 のとおり扱う。引数 `git` の取得も子の cargo が行い、ホストは限らない (秘密を URL に入れて持ち出す経路になりうるが、同じことは Cargo.toml に git の依存を書けば rust-analyzer でもできる。R-SEC-2)。テレメトリや利用統計は機能として持たず、ツールの引数と応答の中身を第三者のサービス (LLM の API を含む) に送らない
  - テストのための取得元の差し替え (`CHATORA_RUSTDOC_TEST_DOCS_RS_URL`・`CHATORA_RUSTDOC_TEST_INDEX_URL`・`CHATORA_RUSTDOC_TEST_STATIC_URL`) は設定ではなく、debug ビルドで `CHATORA_RUSTDOC_TEST_TOOLS=1` のときだけ効く (release ビルドには無い。R-QA-5。m2-plan.md の C3a・E2a-2。v0.20)
  - 手元の cargo のキャッシュに checksum の合う `.crate` があれば、static.crates.io に取りに行かない (R-NW-1。v0.20)
- R-SEC-7 (`server/tools` の Security の MUST):
  - **出力のサニタイズ**: 第三者の文字列 (README・doc・changelog・ソース・crates.io の説明・OSV の advisory・リリースノート・manifest の文字列) とビルドの診断から、制御文字 (改行とタブを除く)、ANSI エスケープ、Unicode の双方向の制御文字 (U+202A〜U+202E・U+2066〜U+2069) と双方向の印 (U+200E・U+200F・U+061C)、ゼロ幅の文字 (U+200B〜U+200D・U+2060)、異体字セレクタ (U+FE00〜U+FE0F・U+E0100〜U+E01EF)、タグ文字 (U+E0000〜U+E007F。人には見えずモデルにだけ読める指示に使われる) を除いて返す (仕様は「Sanitize tool outputs」とだけ定め、何を除くかは決めていないので、ここで決める)。サニタイズは出力の直前の 1 か所で、すべての文字列 (属性の文字列、const の値、`doc(alias)`、ファイル名、`did_you_mean`・hints・エラーの本文に埋め込む値を含む) に一律に適用する
    - M2 で入れた形 (m2-plan.md の F7。v0.20):
      - 除く文字は、git の URL と `project` のパスで断る文字 (R-SEC-4) と 1 つの表にする。上の一覧に、見えない書式の文字 (U+00AD・U+034F・U+115F〜U+1160・U+180B〜U+180F・U+2061〜U+206F・U+3164・U+FEFF・U+FFA0・U+FFF9〜U+FFFB・U+1D173〜U+1D17A) と行・段落の区切り (U+2028・U+2029) を足した。制御文字には C1 (U+0080〜U+009F) も入る
      - ANSI のエスケープは、`ESC` だけでなく列ごと除く: CSI (`ESC [` から終端の文字まで)、OSC・DCS・SOS・PM・APC の文字列 (`ESC ]`・`ESC P`・`ESC X`・`ESC ^`・`ESC _` から BEL か ST まで。終わりが無ければ行末か次のエスケープの前まで)、2 文字の `ESC` の列、8 ビットの導入の文字 (U+009B の CSI、U+009D の OSC など)。単独の `ESC` や導入の文字は、端末と同じく後ろの文字を列として取る。解析は手書き (依存を足さない)
      - 改行でない行の区切り (単独の CR・U+0085・U+2028・U+2029) は、行がつながらないよう改行に置き換え、改行の前の CR (CRLF) は除く
      - 出口は 3 か所: ツールの呼び出しの結果 (成功・ツールのエラー・`busy`・取り消し・panic の `internal_error`)、JSON-RPC のエラーの本文 (`unknown tool` に埋めるモデルが送った名前)、進捗の通知の文。JSON は値として読んで直し (text の中のエスケープした `\u001b` を残さないため)、キーも直す。成功の結果の text は、直した `structuredContent` から作り直す。応答の上限とカーソルは出口より前に決まり、除くだけなので応答は縮むだけ
      - ビルドの診断と cargo の stderr の末尾は、切る前に直し (色の列の途中で切って `31m` を残さないため)、出口でもう一度かける (同じ処理を 2 度かけても変わらない)
      - 除いたことは応答に書かない。ログの呼び出しの行に、出口で除いた文字の数 (`hidden_chars`) を書く (R-OBS-1)
      - 識別子に入りうる ZWJ・ZWNJ・異体字セレクタ (XID_Continue にある) も一律に除き、照合は変えない。そういう名前の item は、応答に出るパスでは引けない (既知の制限。rustc も `uncommon_codepoints` で警告する。決定 3)。docs の絵文字も ZWJ と U+FE0F を除くので分かれる
    - ソースの本文 (`get_source`・examples) では、双方向の制御文字やゼロ幅の文字などを除かずに `<U+202E>` のような見える印に置き換え、warnings で知らせる (Trojan Source を隠さないため)。文書 (doc コメントなど) では、これまでどおり除く (v0.18)。印は M3 の `get_source` と一緒に作り、ソースを返すツールが出口より前に ASCII の印へ置き換える (出口の一律の処理はその後で何も見つけない。m2-plan.md の F7 の決定 4。v0.20)
  - **呼び出しのレート制限** (`completion` の Security の MUST も兼ねる): 同時に処理するツール呼び出しの数、ローカルビルドの数、`completion/complete` の頻度に、設定できる上限を設ける (初期値は呼び出し 16、ローカルビルド 2、補完の頻度は R-MCP-19。どれもプロセスごとに数える。複数のセッションが同時にビルドすると CPU を奪い合って遅くなるが、結果は正しく、同じ target dir のビルドは cargo のロックで 1 つずつになるので受け入れる。v0.16)。ツール呼び出しが上限 (`limits.concurrent_calls`) を超えたら、常に待たせずに `busy` (`retryable: true`、`retry_after_secs: 1`) を返す (M1 の形。v0.19)。v0.19 までの「ビルドを待っている場合は `status: building`」はやめた (上限を超えた応答は、その呼び出しが何を待つかを知る前に返すため。m2-plan.md の M2f の残りの行き先の決定 2。v0.20)
    - ローカルビルドの数の上限 (`limits.concurrent_builds`) は M2 で入れた (m2-plan.md の D3a。v0.20): 枠は cargo の 1 回の実行ごとに持ち、再試行のたびに手放して取り直す。`cargo metadata`・`cargo tree`・形式の試し (R-WS-8) は数えない。取るのは `doc:` のロックの後で cargo の直前 (ロックの順は R-CA-4)。待つ間は `building_reason: waiting_for_slot`。呼び出しの期限を過ぎて裏で続くビルドは、枠を持ち続ける (R-WS-6)

### 9.6 可観測性

- R-OBS-1: ツール呼び出しごとに、構造化ログ (JSON Lines) をファイルに出す。記録する項目は、`request_id` (R-ERR-2)、ツール名、引数 (パス以外は要約)、遅延、応答のバイト数と推定トークン数、エラーコード、キャッシュの当否、取得元。OTLP だけに頼らない (syphar は呼び出しごとのファイルログをやめて OTLP だけにした結果、手元で分析できなくなっている)
  - M1 の形 (m1-plan.md の D1。v0.19): 呼び出しの記録 (`kind: call`) は `ts`・`request_id`・`tool`・`arguments`・`ms`・`bytes`・`tokens`・`error` (エラーコード。JSON-RPC のエラーは `jsonrpc -32602` の形)・`sources` (provenance の取得元)・`cache` (2xx の応答が 1 つも無ければ `hit`。sparse index の再検証の 304 はダウンロードに数えない)・`fetches` (送った要求ごとの URL (query・fragment・資格情報を除く)・最後の状態コード・試行回数・ヘッダまでの時間)。起動時に `kind: start` の行 (版と PID) を書く。引数は、crate・`path`・`project`・`cursor` を 2,000 文字まで、ほかの文字列を 200 文字に切り、引数と配列の要素は 32 個までにする。`bytes`・`tokens` はモデルに渡る形 (`structuredContent` の JSON、エラーは text) の長さで、トークンは 4 文字で 1 と見積もる (校正は M5)。ファイルには warn 以上のログも書き、呼び出しの中で出たものには `request_id` を付ける (利用者が決めた)
  - M2 で足したもの (v0.20): 応答が `status: building` の呼び出しは `status` を書き、`cache` を書かない (通信しなかったので `hit` と記録されていた。m2-plan.md の C3c-1)。出口で応答から除いた見えない文字の数 `hidden_chars` (除いたときだけ。R-SEC-7)。引数 `git` は、受けた URL か受けない理由だけを書き、userinfo を残さない (R-SEC-4。m2-plan.md の E3a)
  - 推定トークン数は文字数からの換算で出す。換算係数の校正の手順は M5 の eval (R-QA-6) で決める (v0.16)。換算係数はモデルの世代で変わるので、eval のたびに校正し直す (v0.18)。**実行時には count_tokens などの外部の API を呼ばない** (応答の中身を外部に送ることになる)
- R-OBS-2: ログの出力先とローテーション (数値は初期値。v0.16。ファイル名と規則は M1 で決めた。m1-plan.md の D1・Windows の CI の修正・公開前の見直し。v0.19)
  - 既定の場所は state のディレクトリ (R-DIST-3) の `logs/`: Linux / macOS は `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs/` (未設定なら `~/.local/state`)、Windows は `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs\` (キャッシュの下に置くと `cache clear` で消える)。設定と環境変数で場所を変えられる
  - プロセスごとに 1 ファイル (`<日付>-<PID>.jsonl`) に書く。複数のセッションのサーバーが同時に動いても、行が混ざらず、書き込みの排他も要らない。1 ファイルの上限を超えたら次のファイル (`<日付>-<PID>.2.jsonl` …) に切り替える (初期値 20MB)
  - 保持期間と合計の上限 (初期値 14 日・50MB) を超えた古いログを消す。他のプロセスが書いているファイルと、所有マーカー (R-CA-5) の無いディレクトリのファイルは消さない。どれも設定で変えられる
    - 時期: 起動時と、次のファイルに切り替えるとき。保持期間を過ぎたものと、合計の上限を超えている間は古い順に消す。合計には消せるファイルだけを数え、個々の失敗では止まらない (最後の失敗を stderr に出す)。symlink はたどらない
    - 所有マーカーは `.chatora-rustdoc-mcp-logs` で、ディレクトリを作ったとき (空のとき) にだけ置く
    - 動いているプロセスの判定: ログそのものはロックせず (Windows のロックは強制で、サーバーが動いている間ほかのプログラムがログを読めなくなるため)、プロセスごとに `<日付>-<PID>.lock` を作り、プロセスが終わるまで OS のロック (R-CA-4) で押さえる。ロックが取れない (動いているプロセスがいる) stem のログは、いま書いている 1 ファイル (番号の最も大きいもの) だけを守り、切り替えた古い分は消せる
    - ログのロックのファイルはプロセスごとのものなので、終わったプロセスのものは、その stem のログがすべて消えたら消す (R-CA-4 の「ロックファイルは消さない」の例外)。ただし今日の日付のものは消さない (同じ日に同じ PID のプロセスがロックのファイルを取り直して使い、ログの番号を続きから振るのと競合しないため。M1 の終わりに利用者が決めた)
  - ログにはツールの引数 (パスはそのまま、それ以外は要約) が残り、プロジェクトのパスには利用者名が入りうるので、ログの場所と記録する項目を README に書く (トークンは出さない。R-NW-4)

### 9.7 テストと評価

- R-QA-1 スナップショット:
  - **モデルに見せる形**の出力を、ツールごとにスナップショットテスト (insta) で固定する
  - **テストデータごとにサイズの予算 (バイト数) を検査する**。`Vec` で 26KB のような退行を CI で落とす。R-OUT-3 の上限は上限であって目標ではない (survey で冗長と判定した 46ki75 の `tokio::sync::Mutex` の 28,850 文字も、この上限には収まる)。代表的な item (`tokio::sync::Mutex`、`std::vec::Vec` など) ごとに `detail=normal` の予算を定める。値は M1 で ferritin のホスト版の出力と比べて決めた (下。v0.19)
    - M1 の予算 (m1-plan.md の E2。v0.19): `get_item` (`detail=normal`) の大きさの約 1.5 倍。tokio の `Mutex` 1,000 バイト、`Mutex::lock` 1,300、`Vec` 1,700、`HashMap` 1,700、core の `Option` 1,500、axum の `Router` 900、serde の `Serialize` 1,300、serde_json の `Value` 1,400。CI の実データの job で、超えたら落とす。比べた ferritin のホスト版の `get` は、ドキュメント全文・メソッドの一覧・実装の一覧を 1 回で返し、tokio の `Mutex` で約 5,000 文字、`Vec` で約 21,000 文字だった (2026-10-20)。こちらは `get_item` が 0.6〜1.2KB で、メソッドは `list_methods`、実装は `list_impls` に分かれ、必要な分だけ引けば小さい
- R-QA-2 テストデータ:
  - docs.rs で実在を確認した各 format_version (46、48、53、54、55、56、57、60、61。一覧は `testdata/docs-rs-json.txt`。v0.17) の JSON
  - 300 段以上のネスト (typenum 相当)
  - 二重 zstd 圧縮の JSON
  - cfg の痕跡属性の各系統の表記 (M1 の testdata の 20 crate では 4 系統。R-FT-2。v0.19)
  - umbrella crate (compio、bevy)、glob で互いを re-export する crate 群、private module 内の公開型
- R-QA-3 docs.rs と crates.io はモックサーバーに置き換え、CI をオフラインで回せるようにする。M1 は、単体と結合のテストを固定の応答を返すローカルの HTTP サーバーで回す。実データのテスト (testdata の JSON を使うもの) は、CI の別の job で testdata の一覧 (crate・版・sha256) の JSON を docs.rs から取って actions/cache に置き、それで回す (大きな JSON をリポジトリに入れないため。docs.rs が落ちているときは、その job だけ失敗する。m1-plan.md の P3。v0.19)
  - M2 の形 (v0.20):
    - 実データのテストは 3 つの OS で回す (m2-plan.md の A4b・D7d の決定 1)。testdata の JSON は Linux のものなので、テストのクライアントが `target` の無い照会に `x86_64-unknown-linux-gnu` を足す (std は配布物から host の target で読むので足さない。問いに `"target": null` と書けば足さずに外す。外の workspace の問いは host 用に作るため)。target の選び方は stdio のテストが守る
    - CI のサーバーのキャッシュの場所を runner の一時ディレクトリ (`$RUNNER_TEMP`) の下にした (v0.19 の `..` を含むパスは actions/cache に断られ、一度も保存されていなかった。m2-plan.md の F5 の決定 6)
    - ローカルビルドのテストは通信しない: 偽の crates.io (sparse index と static.crates.io) を、隔離した cargo home の設定の `[source.crates-io] replace-with` で cargo の registry に使い、git の依存は隔離した git の設定の `url.<…>.insteadOf` で手元の bare リポジトリから取らせる (m2-plan.md の A1・E3)
- R-QA-4 **nightly canary**: 定期実行の CI で最新の nightly で JSON を生成し、次を早期に検知する
  - 新しい format_version
  - `Attribute::Other` の中の cfg の表記の変化 (同じ形式の中でも変わる)
  - std を dist の最新の nightly から引けること (代表の item が引けること)。大きさの変化は R-QA-1 の予算で捕まえる。std のスナップショットの比較はしない (nightly ごとに文面が変わり、Issue が開きっぱなしになるため。v0.19 で M1 の形に改めた)
  - beta (次の stable) の toolchain に crate 名を指定した `RUSTC_BOOTSTRAP` (ローカルビルドの既定。R-WS-8) を付けて、合成の workspace をローカルビルドし、形式と受け付けの変化を検知する (v0.17)
    - M2 で足した形 (m2-plan.md の F3b。v0.20): canary の別の job `beta` (Linux) で、beta だけを入れ nightly を入れない (形式の試しに失敗しても nightly に逃げられず落ちる)。`tests/fixtures/local-build` を隔離した環境 (HOME・CARGO_HOME・git の設定) の一時ディレクトリに写して `rust-toolchain.toml` (`channel = "beta"`) を置き、stdio のサーバー経由で member の lib・build script の cfg と env・proc-macro・bin の item を `get_item` で引く。どれも `source: local` で、toolchain が `-beta.` を含み、形式が対応範囲にあり、warnings が無いこと (docsrs を外した再試行や `RUSTC_BOOTSTRAP=1` への後退は warnings に出る) を確かめる。beta が対応範囲より新しい形式を出したら落とす。テストが 1 件走ったこと (`1 passed`) も確かめる
  - M1 の canary (m1-plan.md の E3。v0.19): 毎日 03:17 UTC (と手動) に回す。最新の nightly で、その場で作る小さな crate (feature で隠れた関数、Unix だけの関数、`doc(cfg)` 付きの型) の JSON を docs.rs と同じく `--cfg docsrs` で作り、読めることと cfg の条件を読めることを確かめる。GitHub は公開リポジトリで 60 日活動が無いと定期実行を止めるので、止まったときの戻し方を workflow の冒頭に書いた
  - あわせて **live の docs.rs に対する smoke test** を回し、docs.rs が新しい形式で配り始めたことや、URL の挙動 (R-DS-6) の変化、配られる target (R-TG-1) の変化を検知する (target は v0.18 で足した。M1 は、空のキャッシュで serde・tokio・anyhow の最新版を読めること、存在しない crate と版の `crate_not_found`・`version_not_found`、2025-05-23 より前の版の `no_json`、tokio の最新版の `aarch64-apple-darwin` の `no_target_json` を確かめる。v0.19)
    - smoke test はプロジェクトの外 (サーバーの cwd をこのリポジトリの外) で回し、期待は `no_target_json` のままにする (このリポジトリを既定のプロジェクトにすると、依存の tokio が他の target のローカルビルドの経路 (R-MCP-3) に入って `no_json` になり、2026-10-22 から canary が落ちていた。m2-plan.md の F3a。v0.20)。canary のテストは CI では回らないので、応答を変える変更を push したら canary を手で回す (v0.20)
  - 失敗したら Issue を自動で開き、直ったら自動で閉じる (ferritin の nightly ジョブに倣う。M1 は label `canary` の Issue を開き、続けて失敗したらコメントを足す。v0.19。M2 で、Issue を job (nightly と beta) ごとにタイトルで分けて開閉し、各 job に時間の上限を付けた。v0.20)。**失敗を放置しない** (46ki75 は live の CI が 5 週続けて失敗したまま放置され、その間に現行の形式が読めなくなっている)
- R-QA-5 説明文とスキーマの検査 (CI):
  - 説明文と instructions の長さ (2,048 文字以内。§6.2)、戻り値の説明と代わりのツールを示す節の有無。M1 の検査 (m1-plan.md の D4a。v0.19): ツール名は snake_case で 2 語以上。説明文は最初の文に Rust を入れ、答える問い (`Answers questions such as`) と戻り値 (`Returns`) とほかのツールの名前を含む。スキーマのキーワードは許可した一覧 (`type`・`properties`・`required`・`items`・`additionalProperties`・`enum`・`minimum`・`maximum`・`description`・`anyOf`・`$defs`・`$ref`) に限り、入力の整数には `minimum` を付ける
  - 説明文・instructions・prompt・同梱の skill に出てくるツール名が、すべて実在すること (§6.2。R-MCP-12 を入れたら `tools.profile` の構成ごとに確かめる)。M1 は、§6.3 の予定のツール名をソースの定数に持ち (公開物に要件の文書は入らないため)、その構成に無いツールを名指ししていないことを確かめる。定数と §6.3 の表が一致することは、要件の文書があるとき (リポジトリの中) だけ確かめる (v0.19)
    - M2 で、説明文をローカル機能の有無で出し分けた (§6.2・R-ARCH-4)。機能の無い構成では、説明文がプロジェクト・workspace・member を書かず、instructions が `project` を書かないことを確かめる。出し分けの印の閉じ忘れと入れ子は debug ビルドで落とす。説明文の問いのうち機能のある構成だけのもの (この workspace の型など) は、機能の無い構成の照合で飛ばす (m2-plan.md の F4a-1。v0.20)
  - すべての JSON Schema が標準どおりであること (R-MCP-10)。ルートに combinator が無いこと、null 許容の型が無いこと
  - ツールのフル名 (プラグインの接頭辞込み) が 128 文字以内、プロパティ名が R-MCP-7 の文法に合うこと
  - **宣言した引数が実装で使われていること**。引数ごとに、値を変えると結果が変わるテストを持つ (rustdoc-mcp の `list_crates` は `workspace_member` を宣言しただけで使っていない)。M1 は、ツールごとに宣言したすべての引数について、成功する基の呼び出しと、その引数だけを変えた呼び出しの結果 (文字列) が違うことを確かめ、表に無い引数があれば落とす (v0.19)。成功した結果を変えない引数 (`refresh` など) と、実データでは確かめられない引数は、ほかのテストで確かめる印を付けて表に置く (m2-plan.md の D5b。v0.20)
  - annotations が R-MCP-4 の前提と合っていること
  - 照会のツールが、既定のプロジェクトの外で子プロセスを一切起動しないこと (外のプロジェクトに罠の `rustc-wrapper` と toolchain の `path` を置いて確かめる。R-MCP-3)。M1 は罠を `build.rustc` に置き (`cargo metadata` と `cargo tree` が `build.rustc` を起動することは手で確かめた)、既定のプロジェクトでは印ができること (陽性の対照) も確かめる。Unix とローカル機能のある構成だけで回す (v0.19。`build.rustc`・`rustc-wrapper` の罠は M2 でも Unix だけ。照会の経路は OS によらないため。m2-plan.md の F2。v0.20)
  - 照会のツールが、範囲の外の依存の build.rs を動かさないこと (罠の依存は path 依存にするか docs.rs のモックで 404 を返し、印のファイルを書く build.rs を置く。他の target でだけ使う依存、build target の条件 (`cfg(target_arch = "wasm32")` など) の build-dependency (cargo は build-dependency の `[target.'cfg(…)']` を `build.target` でなく host で評価するので作らないが、target で評価する素朴な実装なら作る。M2 の F2 で確かめ、v0.19 までの「host と違う `build.target` での target 用の build-dependency」を改めた。v0.20)、member の docs.rs の feature の組でだけ有効になる optional の依存を、resolver 1 と 2 の workspace で試す。同じ罠を範囲の中の依存にも置き、印ができること (陽性の対照) も確かめる。R-MCP-3。v0.15)
    - M2 で入れた形 (m2-plan.md の F2。v0.20): fixture `tests/fixtures/traps` (member 1 つと path 依存 5 つ。範囲の中の陽性の対照は通常の依存と host の条件の build-dependency、範囲の外は wasm32 だけの依存・wasm32 の条件の build-dependency・docs.rs の組でだけ有効な optional の依存)。build.rs の罠は 3 つの OS で、host と `build.target = wasm32-wasip1` の 2 つの場合に回す。照会 (`get_item`・`crate_overview`・`search_items`・`list_crates` と、範囲の外の依存を名指しした照会) の応答も確かめ (member の照会は誤りでない、docs.rs の組が範囲の外の依存を作る旨の warning、範囲の外の名指しは `build_unsupported`)、サーバーはオフラインで起動し、応答の後に裏で続く仕事を見逃さないよう印の集合が変わらなくなるまで待つ
  - `output.mode = structured` では、全ツールの `structuredContent` を宣言した `outputSchema` で検証する (`status: building`・各 `detail`・`page` 付き・`partial` の応答を含む。R-OUT-1)。M1 の検証器は、スキーマに宣言の無いフィールドも誤りにする (応答とスキーマのずれを捕まえるため。標準より厳しい。v0.19)
  - **説明文が約束する動作**: 説明文の「答える問い」に書いた問いごとに、そのツールで実際に答えられることを確かめる結合テストか eval のケース (R-QA-6) を 1 つ以上持ち、対応の欠けを検出する (syphar は引けないメソッドを `get_item` へ誘導し、dmvk は未実装の implementors を説明文で約束している)。M1 は説明文から問いを抜き出し、それぞれに答える呼び出しと答えの確かめ方を持つ (23 問。これで `list_implementors` の説明文の例が M1 では答えられないことが見つかり、例を替えた。v0.19)
  - README に出てくる MCP のツール名も検査の対象にする。許可のルールの例が、手動の登録とプラグインの登録の両方で実在のツール名に当たることを確かめる (R-SEC-2。v0.17)
    - M2 の形 (m2-plan.md の F6 の決定 3。v0.20): lib の単体テストで README を `include_str!` で読み、ツール名がその構成のツールの一覧にあること (Tools の節の表は一覧と一致し、ローカル機能の無い構成に無いツールは `build_external_docs`・`list_crates`)、許可ルールの例の登録の名前・`*` の置き場所 (allow だけ)・接頭辞を付けた長さの上限を確かめる。プラグインの接頭辞の形は M5 まで確かめられないので、末尾のツール名で合わせる
  - §8 のエラーコードごとに、MCP の応答で `isError: true` が付き、`content[0].text` が自己完結した JSON であることを結合テストで確かめる (ferritin は `isError` を計算して捨てている)
  - MCP の適合性テスト (公式のテストスイートか MCP Inspector の CLI) を、対応するすべての版 (v1 では 2026-07-28 だけ) で CI に載せる。`initialize` に対応する版を名指ししたエラーを返すことも確かめる
  - テストの口 (M2。v0.20): 試験用のツール (`sleep`) と取得元の差し替え (R-SEC-6) は、debug ビルドで環境変数 `CHATORA_RUSTDOC_TEST_TOOLS=1` があるときだけ効く。設定ではないので、設定の接頭辞の外の名前にした (m2-plan.md の A4a・C3a・E2a-2)
  - 外の workspace の実データの問いは、`tests/fixtures/other-workspace` を一時ディレクトリに写して行う (fixture の場で cargo を動かさない。m2-plan.md の E4b-4。v0.20)
- R-QA-6 **eval**: §3 の問いを元に問題集を作り、Claude Code (headless) で回す
  - 進め方 (v0.18): M1 から小さく始める。§3 の問いのうち M1 のツールで答えるものを少数の問題集にして回し、ツールの分け方と応答の形を早めに確かめる。各マイルストーンで問題を足し、M5 で比較と校正を仕上げる
  - Claude Code の挙動の検知 (v0.18): headless の `stream-json` で、モデルに渡った `tool_result` の形 (text か `structuredContent` か、エラーの text) を確かめるケースを M1 から置き (R-OUT-1・R-ERR-2)、Claude Code の版を上げたときに回す
  - 測るもの: 期待どおりのツールを選んだか、呼び出し回数、応答の総トークン数、答えが正しいか
  - トークンは 3 つに分けて測る: ① セッション開始時の固定費 (tool search が有効ならツール名と instructions、無効なら全ツールの定義)、② ツールの結果ごとの p50 / p95、③ タスク全体の総トークン数と呼び出し回数。換算には R-OBS-1 の校正値を使う
  - 必ず含めるケース: メソッドのパス (`Vec::iter`、`str::len`)、umbrella crate、feature で隠れた item、「X は Send か」、ビルド中の応答 (ビルド中と stale の応答のケースは M2 で入れた。v0.20)
  - 比較する設定: `output.mode` (structured / text / toon)、`detail` の既定値、`alwaysLoad` の有無、`max_chars` の有無 (`tools.profile` は R-MCP-12 を入れたとき。v0.18)
  - 基盤には Claude Code の `claude plugin eval` を使う。プラグインあり・なしの比較 (Δ) が取れ、`--json` で CI に載せられる。プラグインの形が決まる M5 までは、`eval/` のスクリプトで `claude -p --output-format stream-json` を問いごとに起動し、手で回す (このサーバーだけを読み (`--strict-mcp-config`)、組み込みのツールを外し (`--tools ""`)、利用者の設定を読まない (`--setting-sources project`)。利用者の CLAUDE.md は読まれる。m1-plan.md の P5・E1。v0.19)
  - M1 の結果 (Claude Code 2.1.304、2026-10-20。v0.19): §3 の Q1〜Q9・Q17・Q20 の 16 問と `tool_result` の形の 2 問の 18 問すべて正解、呼び出し 26 回、1.01 ドル。セッション開始時の固定費は 14,716 トークン、`tool_result` は p50 1,090 文字・p95 5,530 文字。ツールの分け方を変えるほどの問題は無かった。tool search が有効な場合の固定費は測っていない
  - M2 の形 (m2-plan.md の F4a。v0.20): 問いごとに fixture を一時ディレクトリに写して git のリポジトリにし、そこで回す。問いに、サーバーに渡す環境変数 (`env`) と下準備 (`setup`: 別のサーバーで先にビルドする、写しを書き換える) を書ける。モデルがこのサーバーを選ぶかを測る「選ぶ側」の問いは、組み込みのツール (Read・Grep・Glob・LSP・ToolSearch。Bash は外す) を使えるようにして `--setting-sources user,project` で回す (LSP の server を利用者のプラグインから読むため)。利用者の CLAUDE.md と rules を読まない手段は無かった (`--bare` は LSP と利用者のログインも外す) ので、読まれる前提で回して結果に書く。採点に、最初のツール、組み込みのツールの呼び出しの数、`status: building` の後に呼び直したか、照会のツールに渡った引数を足した。fixture に自前の型 (pub と private のフィールド、trait とその impl、private な関数) を足し、9 問 (member の型の trait・private な item・名指しの無い自分の item (選ぶ側、3 回)・名前を知らずに探す・ビルド中・stale・外の workspace・他の target・worktree) を足した。git の crate は通信と第三者の build script を動かすので入れない
  - M2 の結果 (Claude Code 2.1.309、既定のモデル、2026-10-23。m2-plan.md の F4b。v0.20): 27 問をくり返し込みで 29 回、28 回が合格、2.29 USD。M1 の 18 問はすべて合格。固定費 (最初の要求の入力トークン) は最小 9,424、`tool_result` は p50 885 文字・p95 6,343 文字。自分の item の問いはどれもこのサーバーで答えた。選ぶ側は 3 回とも ToolSearch でこのサーバーのツールを読み込み、2 回はこのサーバーで、1 回は Grep と Read でソースを読んで答えた (答えは正しい。M2d の試しでは名指しが無いと一度も使わなかったので、instructions の直し (R-MCP-6・§9.9) が効いた)。ビルド中の問いは `building` を受けて呼び直し、stale の問いは前のビルドで答えた。他の target の問いは `build_external_docs` を呼ばずに Linux の docs.rs のデータで正しく答えた。private なフィールドは `pub(crate)` と出るが、モデルはどの回も外から触れないと答えたので、印は足さない

### 9.8 配布と設定

- R-DIST-1: Linux / macOS / Windows を同等に扱う。配布の仕組みは ferritin に倣う
  - crates.io (`cargo install --locked chatora-rustdoc-mcp`) と、GitHub Releases のビルド済みバイナリで配る。README に正規の入手先を明記する (名前の似たプロジェクトや無断コピーが多い。survey §3)
  - crates.io への最初の公開は M1 で、使える版ができたら公開して名前を取る (中身の無い版は出さない)。公開するまでは、README の Installation に、まだ公開していないので同じ名前の crate を入れないよう書く (v0.18)。M1 で README を実態に合わせ (0.1 は docs.rs と std の照会ができ、ローカルビルドはまだ。Linux 向けであること (R-TG-1)、Rust 1.99 以上、Claude Code 2.1.292 以上、登録のコマンド、接続する 4 つのホスト)、`cargo publish --dry-run --locked` が通ることを確かめた。公開は利用者が行う (m1-plan.md の E4。v0.19)。`cargo publish --dry-run --locked` は M2 で CI の `package` の job に置いた (m2-plan.md の A4a。v0.20)
  - M2 の終わりでも crates.io には公開していない (公開は M3 の後に利用者がまとめて行う)。README の状態の行に「crates.io に未公開」と今できること (ローカルビルドを含む) を書き、Installation は `cargo install --locked --git <リポジトリの URL>` にした。公開したら crates.io の 1 行に差し替える (m2-plan.md の F6 の決定 1。v0.20)
  - M1 の CI (m1-plan.md の A1。v0.19): fmt (nightly の rustfmt)、3 つの OS で既定と `--no-default-features` の clippy と test、`RUSTDOCFLAGS=-D warnings` の rustdoc、testdata の実データのテスト (R-QA-3。M1 は Linux だけ)、taplo と markdownlint-cli2、cargo-deny (取得元は crates.io だけ)。action はコミットの SHA で固定する
  - M2 で足した CI (v0.20): testdata の job を 3 つの OS に (R-QA-3)、`package` の job、check の job に `wasm32-wasip1` の target (他の target のビルドと罠のテスト。R-QA-5)、testdata の job に `x86_64-pc-windows-gnu` の target (他の target の実データの問い)。canary (`canary.yml`) に beta の job (R-QA-4)。手で回すときだけ動く測定の workflow `measure.yml` (3 つの OS で、ローカルビルドの時間と大きさを `eval/build_bench.py` で測る。§9.1。m2-plan.md の F5c)
  - 配る target (musl 版を含めるか。Q-27)、ビルド済みバイナリの作り方 (dist・`cargo binstall`・インストーラー)、リリースの流れ (release-plz など)、公式 MCP レジストリへの登録、依存の更新の bot、リリース前後の CI の検査、成果物の sha256 と attestation は M5 で決める (v0.16)
  - CI は 3 つの OS で、ビルド・テスト・clippy・rustdoc (`RUSTDOCFLAGS="-D warnings"`) を回す。OS 依存のコード (`std::os::unix` など) は lint では見つからない。toolchain は浮動の stable で、lint の多くを deny にしているので、6 週ごとの新しい stable で lint が増えるとコードを変えなくても CI が落ちうる。そのときはその都度直す (版を固定しない。v0.13 で決定)
  - crate は 1 つにし、MCP サーバーと CLI (`cache`) を同じバイナリに入れる (R-DIST-2)。パッケージ・lib の crate・バイナリ・MCP の serverInfo・設定とキャッシュと state のディレクトリ・環境変数の接頭辞 (`CHATORA_RUSTDOC_MCP_`)・README で勧める MCP サーバーの登録名は、すべて `chatora-rustdoc-mcp` にそろえる (v0.17)。ferritin が workspace で crate を分けているのは、CLI/TUI と MCP サーバーが別のバイナリだから。分けると、バイナリが依存する crate もすべて crates.io に公開する必要がある
- R-DIST-2: サブコマンドは `serve` (stdio)、`serve --http` (M6 以降。R-MCP-1)、そして人が使う CLI。CLI で照会はしない (エージェントは MCP を使い、人は docs.rs で足りる。CI で公開 API を検査するなら cargo-public-api がある。v0.14 でやめた)
  - `cache ls` / `cache gc [--dry-run]` / `cache clear [--dry-run] [<crate>[@<version>]]` (R-CA-5。M2 で入れた。m2-plan.md の F1。v0.20)。引数の解析は手書き (依存とビルドの時間を増やさない)。`help`・`-h`・`--help` は使い方を stdout に出す。終了コードは 0 (成功、当たるものが無い)・1 (自分のものでないディレクトリ、他のプロセスが GC 中、`cache clear` で使用中か消せずに残ったもの)・2 (使い方の誤り)。出力は人向けで、JSON は出さない
  - `install-skill` (任意。R-DIST-6)
  - CLI はエージェントに案内しない (skill は MCP のツールを使う。R-DIST-5)
- R-DIST-3: 設定ファイルの場所

  | OS | 既定の場所 |
  | --- | --- |
  | Linux / macOS | `$XDG_CONFIG_HOME/chatora-rustdoc-mcp/config.toml` (`XDG_CONFIG_HOME` が未設定なら `~/.config`) |
  | Windows | `%APPDATA%\chatora-rustdoc-mcp\config.toml` |

  - macOS も `~/Library/Application Support` ではなく XDG の規則に従う。開発者向け CLI の多くがそうしており、Linux と案内を揃えられる。[etcetera](https://crates.io/crates/etcetera) の `choose_base_strategy` (Windows では Windows の規則、それ以外では XDG) がこの規則と一致する
  - パスを `HOME` から自前で組み立てない (Windows には `HOME` が無い)
  - state のディレクトリ (Linux / macOS は `$XDG_STATE_HOME/chatora-rustdoc-mcp/` (未設定なら `~/.local/state`)、Windows は `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\`) には、ログ (R-OBS-2) だけを置く。v0.19 までの「ビルドのプロセスの記録 (R-WS-6) は state のディレクトリに置く」は改め、プロセスをまたぐロック (`locks/`)・子プロセスの記録 (`procs/v1/`)・作業用ディレクトリ (`work/<実体 ID>/`)・使えない target dir の印は、キャッシュの下に置く (R-CA-1。キャッシュを共有するプロセスがすべて見られ、state の場所が違っても排他が効くため。m2-plan.md の C1・C4 の決定 1。v0.20)。キャッシュの下のこれらの書き込みも、R-CA-4 と同じく一時ファイルからの原子的な置き換えとロックで行う。GC と `cache clear` は `locks/`・`procs/`・`work/` (GC が消す前に移す `work/gc/` を除く) に触らない (動いているビルドの記録を消さないため。R-CA-5)
  - 環境変数 `CHATORA_RUSTDOC_MCP_CONFIG` で利用者の設定ファイルの場所を変えられ、個々の設定値も環境変数で上書きできる。環境変数の名前は `CHATORA_RUSTDOC_MCP_` に、キーを大文字のスネークケースにしたものをつなぐ (表の入れ子は `__` でつなぐ。例: `network.offline` は `CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE`)。誤った値は、起動を止めずに既定に戻してログに出す (M1。v0.19)
    - 環境変数で受けられる値 (M2 で決めた。m2-plan.md の A3a。v0.20): 数は 10 進の数字 (前後の空白は許す)、真偽は `true`・`false` (大文字小文字を問わない)、容量は設定ファイルと同じ文字列、パスは文字列そのもの (UTF-8 でなくてよい)。空の環境変数は設定していないものとする (`CLAUDE_PROJECT_DIR` と同じ)。設定ファイルでは TOML の型で書く (数を文字列で書くと誤った値)
  - **設定ファイル** (v0.19 の「M1 は環境変数だけを読む」を、M2 で設定ファイルを入れて改めた。m2-plan.md の A3a。v0.20): 既定の場所 (上の表) のファイルが無いのは問題にしない。`CHATORA_RUSTDOC_MCP_CONFIG` で指したファイルが読めなければ問題とし、相対パスなら問題として既定のファイルを読む (`cache.dir` と同じ考え方)。TOML として読めなければ、ファイル全体を無視して問題にする。問題 (読めないファイル、知らないキー、誤った値) は、ログの最初の行 (`start`。R-OBS-1) の後に出す。このバージョンが読むキーの一覧に無いキーは、下の表にあってもまだ使っていないものとして、知らないキーと同じに知らせる (効かない設定が黙って無視されないため)
  - **設定は、利用者の設定ファイルと環境変数だけ** (v0.14 で、プロジェクト単位の設定ファイル `.config/chatora-rustdoc.toml`・`.local.toml` をやめた。Q-24): 書く項目のほとんどはマシンや利用者の都合で決まるもので、プロジェクトに固有のもの (ローカルビルドの feature の組と rustdoc の `--cfg`) は Cargo.toml の `[package.metadata.docs.rs]` (R-FT-3・R-WS-1) に、nightly の指定は `rust-toolchain.toml` に、もともと書く場所がある。clone した第三者のリポジトリが、このサーバー独自の設定ファイルで取得元や上限を変える経路も無くなる (cargo の設定・`rust-toolchain.toml`・`[package.metadata.docs.rs]` と、Claude Code で信頼したリポジトリの `.claude/settings.json` の `env` は効く。R-SEC-2)。同じキーは環境変数が勝つ。設定ファイルは TOML。知らない `CHATORA_RUSTDOC_MCP_*` の環境変数 (`CHATORA_RUSTDOC_MCP_CONFIG` は数えない) と設定ファイルの知らないキーは、起動を止めず、応答には出さずにログで知らせる (綴りの誤りで設定が黙って効かないのを防ぐ。応答に出すと R-ARCH-5 の「同じ入力なら同じ出力」が崩れる。v0.15)。SessionStart の hook でも知らせるかは M5 で決める (R-DIST-5。v0.16)
    - **読む時期**: MCP サーバーは起動時に読み、動いている間の書き換えは再起動まで効かない。CLI は呼び出しごとに読む
  - **設定項目の一覧** (暫定。M5 のリリースで凍結し、以後は README で案内するので変えない。crates.io への最初の公開は M1 だが、凍結は M5 のまま。v0.18。実装してみて要らないキーや名前の変更は、それまで自由に行う。v0.16)。容量は単位付きの文字列 (`"5GB"` など)、時間は単位をキー名に入れた整数 (`_secs` など) で書く。容量の単位は `B`・`KB`・`MB`・`GB` (1000 の冪) と `KiB`・`MiB`・`GiB` (1024 の冪) で、大文字小文字を問わない (M1 で決めた。v0.19)。1h の TTL (R-STD-6)、sparse index の 10 分の TTL (R-CA-3)、OSV と `RELEASES.md` の 24 時間の TTL (R-AU-3、R-RR-1) は固定値で、設定項目にしない
    - M2 の終わりの版が読むのは、`build.*` (`build.prefer_local` を除く)・`cache.*`・`limits.concurrent_builds`・`limits.concurrent_calls`・`limits.extract_max_size`・`limits.request_timeout_secs`・`log.*`・`network.offline`・`network.connect_timeout_secs`・`network.read_timeout_secs`・`std.fetch_from_dist` の 26 個 (README の設定の表と同じ)。ほかの行はまだ読まず、知らないキーとして知らせる (v0.20)

    | 項目 | キー | 既定 | 要件 |
    | --- | --- | --- | --- |
    | 取得元のミラーのベース URL | `sources.<取得元>.url` (取得元は `docs_rs`・`crates_io_index`・`crates_io_api`・`static_crates_io`・`static_rust_lang`・`github_api`・`osv`) | 公式の取得元 | R-CA-8 (実装した場合。M5 で検討) |
    | 取得元ごとの無効化 | `sources.<取得元>.enabled` | `true` | R-CA-8 (実装した場合。M5 で検討) |
    | プロキシ | `network.proxy` | なし (標準の環境変数 `HTTPS_PROXY` などに従う) | R-CA-8 (実装した場合。M5 で検討) |
    | オフラインモード | `network.offline` | `false` | R-CA-7 |
    | 通信のタイムアウト (接続 / 受信) | `network.connect_timeout_secs` / `network.read_timeout_secs` | 10 / 120 | R-NW-2 |
    | GitHub token | `github.token` (無ければ `GH_TOKEN` → `GITHUB_TOKEN` → gh の `hosts.yml` の順に探す。v0.19) | なし | R-NW-4 |
    | cargo・rustdoc・rustc の実行ファイルの絶対パス | `build.cargo_path` / `build.rustdoc_path` / `build.rustc_path` | なし (rustup があればその toolchain、無ければ PATH のもの。R-WS-1) | R-WS-1・R-TG-1 |
    | ローカルビルドを docs.rs より優先するモード (M3 以降で検討。README には載せない。v0.20) | `build.prefer_local` | `false` | R-DS-1 |
    | `--cfg docsrs` を渡すか | `build.cfg_docsrs` | `true` (壊れたら渡さずにやり直す。Q-11) | R-WS-1 |
    | nightly の日付の固定 | `build.nightly_date` | なし (固定しない) | R-WS-8 |
    | 1 回のビルドの最大時間 | `build.timeout_secs` | 1800 | R-WS-6 |
    | ビルド (とロック待ち) を待つ上限 (呼び出しが仕事を待つ `limits.request_timeout_secs` の 3 分の 2 より長ければ、そちらが効き、起動時にログで知らせる。v0.20) | `build.wait_secs` | 15 (M2 の F5 で測って 10 から改めた。v0.20) | R-WS-6 (v0.18) |
    | ローカルビルドの cargo の並列度 (`-j`) | `build.jobs` | CPU の半分で、2 つ以上あれば 2 以上 (M2 の F5 で測って決めた。v0.20) | R-WS-1 (v0.18) |
    | `build_failed` に付ける診断の件数 | `build.max_diagnostics` | 2 (1 件 3,500 文字、合計 8,000 文字以内。文字数は固定。M2 の F5 で測って据え置いた。v0.20) | R-WS-7 |
    | キャッシュの場所 (相対パスなら既定の場所を使ってログで知らせる。v0.19) | `cache.dir` | OS ごとの既定 (R-CA-1) | R-CA-1 |
    | GC の上限 (文書のキャッシュ / 専用 target dir / target dir の保持日数) | `cache.docs_max_size` / `cache.target_max_size` / `cache.target_max_age_days` | `"5GB"` / `"20GB"` / 30 (日数の 0 は誤りとして既定に戻す。v0.20) | R-CA-5 |
    | 起動時の一括取得 / Cargo.lock の変化での裏の取得 | `prefetch.on_startup` / `prefetch.on_lockfile_change` | `false` / `false` | R-CA-6 (任意) |
    | dist から std の JSON を取るか / stable の分岐日に近い nightly を選ぶか | `std.fetch_from_dist` / `std.match_stable_branch_date` | `true` / `false` (後者は任意で M4 で検討。R-CA-8 を実装して `sources.static_rust_lang.enabled = false` にしたら、`std.fetch_from_dist` によらず取らない) | R-STD-6 |
    | ツールの構成 (`full` / `core`) | `tools.profile` | `"full"` | R-MCP-12 (将来。v0.18) |
    | 出力の形式 (`structured` / `text` / `toon`) | `output.mode` | `"structured"` | R-OUT-1 (text・toon は M5) |
    | 応答の上限 | `output.max_chars` | 32000 (推定 8,000 トークン) | R-OUT-3 |
    | 1 つの item を返すツールの `detail` の既定 (一覧のツールは `brief`。v0.19) | `output.detail` | `"normal"` | R-OUT-7 |
    | 検索の既定の範囲 (`crate` / `workspace` / `workspace_and_direct` / `dependency_graph`。M3 以降で検討。README には載せない。v0.20) | `search.scope` | `"workspace_and_direct"` | R-SR-2 |
    | changelog で返す直近のリリースの数 | `changelog.max_releases` | 10 (応答の上限まで) | §5.9 |
    | メモリの上限 | `limits.memory` | `"1GB"` | R-MEM-1 (M5。v0.19) |
    | 同時に処理する呼び出し・ローカルビルドの数 (cargo の 1 回の実行ごとに数える。v0.20) / 補完の頻度の上限 (1 秒あたり) | `limits.concurrent_calls` / `limits.concurrent_builds` / `limits.completions_per_sec` | 16 / 2 / 20 (初期値) | R-SEC-7・R-MCP-19 |
    | ディスクへの展開の上限 | `limits.extract_max_size` | `"4GiB"` | R-NW-3・R-SC-2 |
    | 1 リクエストあたりの時間の上限 (ビルドを待つ間は、より短い `build.wait_secs` が先に効く) | `limits.request_timeout_secs` | 30 | R-ARCH-6 |
    | ログの場所 / 1 ファイルの上限 / 保持期間 / 合計の上限 | `log.dir` / `log.file_max_size` / `log.retention_days` / `log.total_max_size` | `~/.local/state/chatora-rustdoc-mcp/logs/` など / `"20MB"` / 14 / `"50MB"` | R-OBS-2 |
    | stderr のログの絞り込み (tracing の `EnvFilter` の書き方。R-OBS-1 の呼び出しごとの記録は絞らない。誤った指定が 1 つでもあれば全体を既定に戻し、そのことをログに出す) | `log.filter` | `"info"` | R-SEC-1 (v0.17) |

- R-DIST-4: ライセンスは MIT OR Apache-2.0 とする (`LICENSE-MIT` / `LICENSE-APACHE`)
  - 他の実装のコードを流用するのは、MIT か MIT OR Apache-2.0 のものに限り、出典と著作権表示を `THIRD-PARTY-NOTICES` に残す。AGPL (possdog)・LGPL (KevinT3Hu)・Apache-2.0 のみ (dmvk、46ki75)・ライセンス本文の無いもの (syphar、Xevion) は、設計の参考にとどめてコードを写さない
  - 依存のライセンス・既知の脆弱性・取得元・禁じる crate を、CI の cargo-deny (`licenses`・`advisories`・`sources`・`bans`) で検査する
- R-DIST-5 **Claude Code プラグイン**として配布する:
  - プラグインの名前とサーバーの名前は、どちらも `chatora-rustdoc-mcp` にする (v0.17。許可のルールの名前は R-SEC-2)
  - `plugin.json` の `mcpServers` でサーバーを起動する (`command: chatora-rustdoc-mcp serve`)
  - skill を 1 つ同梱する: いつ使うかの対応表 (MCP のツールを使う。CLI は案内しない)、LSP との境界、`rustc --explain` / `cargo clippy --explain` の案内。Rust の文脈 (`.rs` と `Cargo.toml`) に限定する (書き方は M5。v0.16)。バージョンの取得元を正確に書く (ferritin の skill は「プロジェクトで解決された版」と書きつつ既定は docs.rs の latest という食い違いがある)
  - SessionStart hook では、前提が欠けていれば 1 行で知らせ、揃っていれば何も出さない。何を確かめるか (案: バイナリ、std の取得手段 (手元の `rust-docs-json` か R-STD-6 の dist)、プラグインと手動設定の重複登録、知らない設定、実行ファイルのパスの組み合わせ (R-WS-1。v0.17)) と確かめ方 (バイナリに確かめるためのサブコマンドを足すかなど) は M5 で決める (v0.16)
  - 任意で、依存の監査などのコマンドを同梱する
  - プラグインと手動設定の両方で登録するとツールが重複することを README に書く
  - 利用者ごとの設定はプラグインの `userConfig` で受け取る (項目は M5 で決める。v0.16)。token は `sensitive` を指定する
  - キャッシュは `${CLAUDE_PLUGIN_DATA}` に置かない (プラグインのアンインストールで消えるうえ、CLI やプラグイン外の起動と共有できない)。R-CA-1 の場所を使う
- R-DIST-6 skill の単独インストール (任意。M5): プラグインを使わない利用者向けに、CLI で skill を入れられるようにする。利用者が編集した skill を黙って上書きしない (akesson/docsrs に倣う)。引数の形は M5 で決める (v0.16)

### 9.9 LSP との役割分担

- Claude Code には rust-analyzer の `LSP` ツールが組み込まれている。**位置 (行・桁) に基づく問い・参照・診断と、最後のビルド (`built_at`) の後に編集したコード**は LSP の担当とする (workspace の JSON の鮮度は R-WS-9)。v0.19 までの「この会話の中で編集中のコード」を改めた: この workspace の item も、パスや名前で引くなら本サーバーの担当とする (member は private も含めてここでビルドする)。instructions が自分の workspace の item を LSP へ誘導していたことが、M2 の試しでモデルが自分の item にこのサーバーを使わなかった原因の候補で、直した後の eval では使った (R-QA-6。m2-plan.md の F4a の決定 1。v0.20)
- 本サーバーの担当は、**パスで引く、バージョンを固定した API リファレンス**、crate 全体の構造、横断的な検索、この workspace の crate (private を含む)・依存と std のドキュメント
- 本サーバーは位置 (行・桁) を入力として受け付けない (LSP ツールは 1 始まり、rust-analyzer-mcp は 0 始まりで、混乱の元になる)
- サーバーの instructions (R-MCP-6) とツール説明文に、この境界を書く

### 9.10 将来のホスト版に備えた層構造

v1 では公開ホスティングをしない (§2 の非ゴール)。ただし、後からホスト版 (docs.rs と std だけを出す公開サーバー) や、チームで共有するキャッシュサーバーを足せる構造にしておく。今のうちに守れば安く済み、後から直すと高くつく項目に限る。

- R-ARCH-1 層を分ける。上の層は下の層だけに依存し、MCP の型は最上位の層にしか現れない

  | 層 | 責務 |
  | --- | --- |
  | データ層 | 取得元 (docs.rs / std / local)、形式の正規化、ディスクキャッシュ、索引、バージョン解決 |
  | クエリ層 | パスの解決、re-export、メソッドと impl の解決、cfg の計算、検索、応答モデル (§7.3) の組み立て |
  | フロントエンド層 | MCP (stdio / HTTP)、CLI (`cache`) |

- R-ARCH-2 取得元を 2 種類に分け、cargo feature で切り離せるようにする
  - **リモート専用の取得元**: docs.rs、std。プロジェクトの文脈を必要としない
  - **ローカル依存の取得元**: ローカルビルド、workspace、`cargo metadata`。cargo feature `local` (既定で有効) で囲む
  - `--no-default-features` でビルドすると、cargo を起動するコードや build.rs を実行するコードがバイナリに含まれなくなる。ホスト版で任意コードを実行しないことを、実行時の設定ではなくコンパイル時に保証するため
- R-ARCH-3 docs.rs と std の経路を、プロジェクトの文脈に依存させない
  - クエリ層の関数は、プロジェクトの文脈を `Option` で受け取る
  - 文脈が無い場合、バージョン解決は「明示指定 > latest」になる。プロジェクトで有効な feature の注記などの項目は、応答から省く。ローカル機能が無い構成で版を省くと、latest にした旨を warnings に書く (v0.19)
- R-ARCH-4 文脈が無いと意味をなさないツール・引数・引数の値は、ローカル機能が無い構成では `tools/list` (ツールの一覧と `inputSchema`) に出さない
  - 対象の一覧は、ローカル機能が無い構成を作るときに決める (例: `list_crates`、`project` 引数、`build_external_docs`。v0.16)。M1 では、`list_crates` を出さず、どのツールにも `project` 引数を宣言しない (渡されたら無視して warnings で知らせる。v0.19)。M2 で、ローカル機能の無い構成には `build_external_docs` も出さず、`git`・`rev`・`branch`・`tag` も宣言しない (渡されたら黙って crates.io の crate を返さず `invalid_argument`。m2-plan.md の E1a・E3a。v0.20)。説明文は構成ごとに出し分け、機能の無い構成ではプロジェクト・workspace・member を書かない (v0.19 で残っていた、説明文の「答える問い」のプロジェクトについての問いは、M2 の F4a-1 で直した。§6.2・R-QA-5。v0.20)。既知の残り: 機能の無い構成の `search_items` の `inputSchema` で、`crate` を required にしていない (説明文は「`crate` is required」。§6.3。M3 の最初の段で直す。m2-plan.md の §7.1 の決定 6。v0.20)
  - 出しておいてエラーを返すと、LLM がそのツールを選んでしまうため。出す・出さないは構成で決まるので、R-MCP-5 (毎回同じ順) とも矛盾しない
- R-ARCH-5 結果が「バイナリの版、入力、データの識別子」だけで決まるようにする
  - セッションごとの状態が結果に影響しないようにする (R-MCP-3 を強化したもの)
  - 応答に map を入れるときはキーで並べる (`HashMap` をそのまま直列化しない)。serde_json の `preserve_order` (R-OUT-1) の下では、map の順が挿入の順になり、`HashMap` 由来の値の順がプロセスごとに変わるため (v0.17)
  - CI で検査する: 同じ入力を、別々のプロセスで、間に別の問い合わせを挟んで実行し、出力が一致することを確かめる (map を含む応答を必ず入れる)。出力は文字列で比べる (`preserve_order` の下の `serde_json::Value` の等価は map の順を見ないので、値で比べると `HashMap` の順の揺れを見逃す。M1 で確かめた。エラーは `request_id` を除いて比べる。v0.19)
  - これを守ると、ホスト版で ETag による HTTP キャッシュを導入できる。ferritin は、応答の本体を作る前に識別子から ETag を決める方式で、読み込みなしに約 1.5ms で 304 を返している
- R-ARCH-6 資源の上限を、どの構成でも設定できるようにしておく
  - 対象: メモリ (R-MEM-1)、1 リクエストあたりの時間、応答サイズ (R-OUT-3)
  - 1 リクエストあたりの時間の上限は既定 30 秒 (HTTP の最初の応答バイトまでの時間制限にも収まる)。ローカルビルドとそのロック待ちは、これより短い R-WS-6 の上限 (`build.wait_secs`、既定 15 秒。M2 の F5 で測って 10 秒から改めた。v0.20) で待つのをやめる (v0.18)。超える処理 (索引の作成、依存グラフ全体の取得など) は裏で続け、`retry_after_secs` を付けて返す (v0.15)。形は R-OUT-9 に従う: データが無ければ `status: building`、古いデータがあれば `stale_reason: rebuilding`、複数の crate にまたがる応答で結果が 1 件でもあれば `partial: true` (v0.17)。M1 で入れたのは、依存の横断の検索 (R-SR-2) の 20 秒の待ちと `partial` だけ (v0.19)
    - M2 で入れた形 (m2-plan.md の C3a・D3a。v0.20): 上限を設定 `limits.request_timeout_secs` (既定 30 秒) で読む。期限は呼び出しの始めから数え、呼び出しが裏の仕事 (取得・索引の作成・ビルド) を待つのはその 3 分の 2 (既定 20 秒) までで、残りは版の解決など仕事の前後に使う。1 つの crate の照会も横断の検索も、この 3 分の 2 で待つのをやめて `status: building` か `partial` を返す (M1 の横断の検索の 20 秒もここから出す)。ビルドを待つ `build.wait_secs` はビルドを待ち始めたときから数え、この 3 分の 2 より長ければ 3 分の 2 を使う
  - ホスト版では、これらが利用者をまたいだ資源の分配に使われる
- R-ARCH-7 (先送り。v0.16) ディスクキャッシュの読み書きの trait 化は、共有キャッシュサーバーを作ると決めたときに行う。それまでは、キャッシュの読み書きを 1 か所 (データ層) にまとめておく。共有するのは正規化済みの元データとし、アーカイブは各自で作る (rkyv のアーカイブはアーキテクチャをまたいで使えないため)
- R-ARCH-8 CI で `--no-default-features` の構成もビルドとテストを回し、リモート専用の経路がローカル機能に依存していないことを保証する

ホスト版を実際に作る段階になったら、次を別途要件にする (いまは対象外)。

- ローカルビルドを使う場合のサンドボックス
- クライアントごとのレート制限と濫用対策
- 上流 (crates.io・docs.rs) へのリクエストの集約と負荷の調整
- TLS と認証
- ログとプライバシーの方針
- 応答の形を変えるときの互換性の管理

---

## 10. 実装方針の候補 (拘束しない)

| 論点 | 候補 | 現時点の推し |
| --- | --- | --- |
| MCP SDK | [rmcp](https://crates.io/crates/rmcp) 3.5.1 / tower-mcp 0.24.0 (`protocol-2026-07-28` feature。v0.16 の時点は 0.23.2) / rust-mcp-sdk 2.0.0 / pmcp 2.22.7 (この 4 つの版は M0 で比べたもの。v0.17) / [mcplease](https://crates.io/crates/mcplease) 0.4.2 | **rmcp ≥ 3.5.1**。3.0.0 (2026-07-28 リリース) から 2026-07-28 版 (stateless、`server/discover`、SEP-2243 のヘッダ、緩和された `structuredContent`) に対応している。2026-07-28 の機能は v0.10 で確認した (discover・MRTR・`RequestStateCodec`・elicitation・キャッシュのヒント・tasks 拡張・subscriptions・prompts・completions・resource templates・`title`・icons はあり、Skills 拡張は無し)。v1 は MRTR も elicitation も使わないので、cargo feature の `request-state` と `elicitation` は要らない (将来 MRTR を使うなら `request-state`。R-SEC-2)。mcplease は stdio が同期的で 1 リクエストずつしか処理せず、進捗通知もキャンセルも無いので R-WS-6 と両立しない。rmcp の `CallToolResult::structured` は text にも同じ JSON を入れるので、R-OUT-1 と合う。引数の解析エラーが自動で -32602 になる点は R-ARG-1 のとおり自前の検証に替える。M0 で、同じ JSON-RPC を流して tower-mcp・rust-mcp-sdk・pmcp と比べ、rmcp を続けると決めた (tower-mcp は panic の扱いと引数の扱いが良いが、`server/discover` のキャッシュの指定と `initialize` の拒否の文言を変えられず (R-MCP-5・R-MCP-2)、7 か月で破壊的な minor が 16 回ある。rust-mcp-sdk はキャンセルした要求にも応答を送る (R-WS-6 の MUST NOT)。pmcp は stdio で 2026-07-28 を話さない。比較の表は m0-plan.md の §6.1。受け入れる rmcp の挙動は R-MCP-2。v0.17) |
| データ層 | 自作 / [ferritin-common](https://crates.io/crates/ferritin-common) 0.18.0 (MIT OR Apache-2.0) を依存として使う / [rustdoc_processor・rustdoc_resolver](https://github.com/LukeMathWalker/officina) 0.1.2 (Apache-2.0 のみなので、依存にはできるがコードは流用しない。R-DIST-4。Pavex から分離。cargo rustdoc → SQLite キャッシュ → 公開 import path と crate 外の re-export の索引) | **自作**。rustdoc_processor は R-PR-2 の公開パス索引と同じ問題を実運用で解いているので、索引の設計とテストケースの比較対象にする。M0 で評価し、依存にしないと決めた (rustdoc-types を 0.57 に固定していて形式 57 しか読めない、guppy・SQLite (bundled)・rayon・rkyv などで依存が重い、API が cargo rustdoc の起動を前提にしている)。同じ docs.rs の JSON で 11 crate の公開パスを突き合わせると、違いは rustdoc_processor の側 (マクロを索引に入れない、`use` で再エクスポートしたモジュール自身のパスを記録しない) と正準パスの規則の違い (改名) だけで、両方に共通する誤り (`pub use … as _`) が 1 つ見つかった (m0-plan.md の §6.1 の A10。突き合わせは poc/m0a-a10 でやり直せる。v0.17)。形式の正規化・rkyv の sidecar・逆引き索引・検索の調整値は ferritin の設計を踏襲するが、ferritin には要件と食い違う既知の欠陥が多い (付録 A の ferritin の行) |
| JSON の形式 | `rustdoc-types` の最新 + 独自 shim | §5.2 のとおり。形式ごとの差分は [migrate_rustdoc_json](https://github.com/BD103/migrate_rustdoc_json) の移行コードと rustdoc-types の CHANGELOG で洗い出す |
| JSON パーサー | serde_json (`unbounded_depth`、`preserve_order`) + serde_stacker | sonic-rs は 255 段の上限があり使えない。`preserve_order` は応答のキーの順のため (R-OUT-1・R-ARCH-5。v0.17) |
| アーカイブ | rkyv (`bytecheck` かチェックサムで検証。R-CA-4) | rkyv の mmap は ferritin で実績がある (R-PF-1)。ただし ferritin は検証を省いている。M1 で rkyv 0.8 を入れ、開くときに bytecheck で検証しても目標に収まった (Q-23。v0.19) |
| 検索 | 自作の BM25 / tantivy / SQLite FTS5 | 自作の BM25 (ferritin の調整値)。tantivy の既定のトークナイザは CamelCase を分割しない |
| シグネチャの描画 | 自作 | ruskel の型描画表 (`dyn` / `impl` の括弧、1 要素タプル、修飾パス) と cargo-public-api の最良パス選択を参考にする。ruskel の「書かれたままのパス」「名前による derive 判定」「`pub` だけの可視性」は真似しない |
| メタデータ | `cargo_metadata` crate | — |
| changelog の解析 | `parse-changelog` crate | syphar の実績 |
| HTTP | reqwest (rustls、暗号は aws-lc-rs) + zstd | M0 で採用。aws-lc-rs は C のビルドが要るので、3 つの OS での `cargo install` を R-DIST-1 の CI で確かめる (M0 で確かめたのは Linux だけ。v0.17。M1 の CI で 3 つの OS のビルドが通った。v0.19) |
| 非同期 | tokio | — |
| ロック | parking_lot / `tokio::sync` | poison が無い (R-ERR-1)。M2 で決めた使い分け (m2-plan.md の止まる危険の直し 2。v0.20): 持ったまま `.await`・I/O・長い待ちをするものは `tokio::sync` にするかロックの外に出し、表や数を一瞬だけ触るもの、`Drop` や blocking のスレッドの中のものは parking_lot のまま (parking_lot のガードは `Send` でないので、`.await` をまたぐ持ち方を型で防げる)。async の中の長い同期の処理は `spawn_blocking` に回す |
| ログのファイルの書き込み | 自前の書き込みスレッド / tracing-appender の `non_blocking` | **自前** (上限付きのチャネルで 1 本のスレッドに渡す)。`non_blocking` の `WorkerGuard` は終わるときにチャネルが詰まっていると stdout に書き、R-SEC-1 に反する (M2。v0.20) |
| Windows の Job Object | windows-sys で自前に作る / 専用の crate | **windows-sys** (tokio が使うのと同じ版で、crate が増えない。R-WS-6。M2。v0.20) |
| TOML の書き出し (仮のプロジェクトの Cargo.toml) | toml の `display` feature | toml の読み込みに使う crate に `display` を足した (R-WS-5。M2。v0.20) |

---

## 11. 未決事項・要調査

### 11.1 解決したもの

| # | 内容 | 結論 |
| --- | --- | --- |
| Q-1 | Claude Code は `structuredContent` と text のどちらをモデルに渡すか | `structuredContent` だけを渡す (R-OUT-1)。エラー時は `structuredContent` を無視し、text のブロックを改行でつないで渡す (R-ERR-2) |
| Q-2 | MCP の版と SDK の対応 | 最新版 (v1 では 2026-07-28) だけに対応し、新しい版に追従する。2 つの版をどう並べるかは、次の版が出たときに決める (R-MCP-2。v0.11 の決定を v0.16 で改めた)。rmcp ≥ 3.0 が対応 (推しは ≥ 3.5.1。§10) |
| Q-3 | item と feature の対応が取れるか | 取れるが、`Attribute::Other` の文字列解析と祖先からの継承が必要 (R-FT-2) |
| Q-4 | 形式 45 以前の有無、46〜48 の読み取り互換性 | 下限は 46。47 は欠番。46 → 48 の差は `#[inline]` の表記だけ (R-FV-4)。v0.5 で解決 |
| Q-6 | `api_diff` の粒度 | cargo-public-api の方式 + 分類器 (§5.12) |
| Q-7 | 公開 crate の取得元の既定値 | docs.rs 優先 (R-DS-1) |
| Q-8 | `detail` の各段階 | 案が決まった (R-OUT-7)。数値は eval で調整 |
| Q-9 | blanket impl の展開 | 案が決まった (R-SH-4)。eval で調整 |
| Q-10 | 依存の `cargo rustdoc -p` に `--features` を渡せるか | 渡せない (workspace 外のパッケージへの指定は cargo が拒否する)。渡さなくても統合された feature が効く (R-FT-3)。v0.5 で解決 |
| Q-11 | ローカルビルドで `--cfg docsrs` を渡すかの既定値 | 既定で渡し (docs.rs と同じ)、壊れたら渡さずにやり直す (R-WS-1)。v0.13 で決定。壊れた crate を記録して次から渡さないかは M2 で決めるとした (v0.16)。M0 で測ったやり直しの頻度は、このリポジトリの依存のうち文書化できた 136 個中 1 個 (rustls-platform-verifier。`--cfg docsrs` を前提にした crate 自身の `docsrs` feature が、プロジェクトの組で有効でなかった)。stable と nightly で同じ (v0.17)。ただしこれは新しい版だけの標本で、削除された `doc_auto_cfg` を使う版 (`#![cfg_attr(docsrs, feature(doc_auto_cfg))]`) が手元の registry に 82/1,588 あり、`--cfg docsrs` を渡すと E0557 で壊れる (2026-10-19 に確認。v0.18)。M2 で、文書化する crate のエラーのときだけ外してやり直し、外して通ったことを置き場の前回の成功の記録に残して、toolchain・`rustdoc-args` の cfg・docs.rs の組・サーバーの版が同じ間は最初から渡さないと決めた (それらが変わるか `refresh` で試し直す。R-WS-1・R-FT-3。m2-plan.md の D3b・D4d。v0.20) |
| Q-12 | stable + crate 名を指定した `RUSTC_BOOTSTRAP` を、ローカルビルドの既定にしてよいか | 既定にする。値は専用 target dir ごとに、依存の解決結果のすべての lib (cdylib・staticlib・dylib だけの lib を含む。v0.18)・proc-macro の crate 名と member の bin の crate 名に固定する (値を変えると proc-macro2 から 54〜65 unit が作り直される。固定すれば作り直しは文書化する crate だけ)。値を R-WS-4 の鍵に入れる。M0 で決定 (R-WS-8。v0.17)。M2 で、値の長さの上限を 3 OS 共通で 30,000 バイトとし、超えたら `1` に落として warnings に書くと決めた。外のツールの仮のプロジェクト (R-WS-5) は常に `1`。依存の鮮度の鍵にも値の全体を入れる (R-WS-4。m2-plan.md の D2・E2a と §7.1 の決定 3。v0.20) |
| Q-13 | crate ごとに `default-target` を上書きしている場合に、docs.rs が使った triple を JSON の `target.triple` だけで判別できるか | できる。`default-target` を上書きした windows-registry 0.100.0 と、`targets` を並べた winreg 0.56.0 で、`target.triple` が実際にビルドした triple だった。ビルドしていない target を名指しすると 404 で、別の target に差し替えられない (2026-10-16 に確認。R-TG-1・R-TG-2) |
| Q-16 | OSV のミラーの容量と更新の手間 | 小さい。`crates.io/all.zip` は 3.6MB で毎日更新される (2026-10-15 に HEAD で確認)。1 日 1 回、条件付きのリクエストで取り直す (R-AU-3)。v0.13 で解決 |
| Q-17 | `cargo doc --output-format json` と `cargo rustdoc` の違い | `cargo rustdoc` を使う。`cargo doc` は 2 回目が速く依存をまとめて作れるが、nightly の cargo かプロセス全体への `RUSTC_BOOTSTRAP=1` が要り、同名で版の違う依存は片方しか文書化されない。nightly で依存をまとめて文書化する用途の候補として M2 に残した。M0 で決定 (R-WS-1。v0.17)。M2 でも使わなかった (nightly の経路でも `cargo rustdoc` で、`-Z fine-grain-locking` も使わない。m2-plan.md の D2。v0.20) |
| Q-18 | rustdoc JSON の型ごとの `impls` が、条件付きの blanket impl を絞り込んでいるか | 絞り込んでいる。Display の無い型には `ToString` が、Clone の無い型には `ToOwned` と `CloneToUninit` が載らない (2026-10-15 に stable 1.99、形式 61 で確認)。R-SH-4 は JSON の `impls` をそのまま使えばよい。v0.13 で解決。ジェネリックな型の auto trait は、M1 で、impl の型引数の境界と where 句から `if <条件>` を作る形で答えた (`std::sync::Arc` の `Send` は `if T: Sync + Send, A: Allocator + Send + Sync`。R-TI-2。m1-plan.md の C5。v0.19) |
| Q-19 | モデル向けの文字列の言語 | 英語に統一する (説明文・instructions・warnings・hints・エラーのメッセージ・prompt。CLI の人向けの出力とログも英語)。v0.13 で決定 |
| Q-20 | target ごとの依存・build-dependency・dev-dependency で feature が分かれる crate を、どの組でビルドするか (R-FT-3) | v0.18 で改めた: 文書の組は、ビルドしたときの cargo の `compiler-artifact` の `features` を正として記録し、事前に予測しない。`enabled_in_project` は、全 member を統合した `cargo tree --workspace --target <t> -e normal` の組で判定する (R-FT-3)。M0 の決定 (`cargo tree --workspace` の組が `cargo rustdoc --lib` の組と、resolver 1 でも 2 でも一致する。v0.17) は、member が 1 つの workspace でしか成り立たなかった (2026-10-19 に確認。複数 member などでの確認は Q-31)。M2 の実測で、`cargo rustdoc -p <member>` は引数なしの `cargo build` (default-members) と、`cargo tree --workspace` は `cargo build --workspace` と同じ組で、別の member だけが有効にする feature があると両者が食い違うと分かった。member は全 member を統合した組を `--no-default-features --features` で明示してビルドし、`enabled_in_project` と一致させる (R-FT-3。m2-plan.md の D3c・D3d。v0.20) |
| Q-21 | 新しく足された依存の初回のローカルビルドに確認を求めるか | v0.8 で求めると決めたが、v0.14 で確認をすべてやめた (同じことを rust-analyzer やエージェントの Bash でもできるため。R-SEC-2) |
| Q-23 | mmap 前の検証 (R-CA-4 の bytecheck やチェックサム) を数百 MB のアーカイブにかけても、読み込みの目標に収まるか | v0.19 で解決: アーカイブは開くときに bytecheck で検証し続け、開いたものをプロセスの中で使い回す (32 個まで。R-PF-1)。core のアーカイブ (公開パス索引を含めて 99MB) を mmap して検証するまで 42ms (B9)、新しいプロセスで core を初めて引くまで 88ms (E2) で、§9.1 の M1 の目標 (< 150ms) に収まる。2 回目からは検証しない。検証を省く案は採らない (M1 の終わりに利用者が決めた。m1-plan.md の B9・E2) |
| Q-24 | 設定ファイルのプロジェクト単位の上書きを許すか | v0.13 で許すと決めたが、v0.14 でやめた。プロジェクトに固有のものは Cargo.toml の `[package.metadata.docs.rs]` と `rust-toolchain.toml` で足りる (R-DIST-3) |
| Q-26 | CLI のスクリプト向けの出力形式と出力の上限 | v0.14 で照会の CLI をやめたので、決める必要が無くなった (R-DIST-2) |
| Q-29 | プロセスをまたぐ仕組みの細部 (R-CA-4・R-WS-6・R-WS-10): ロック (置き場所・名前・取る順・target dir のロック・NFS)、版の違うサーバーの同居 (版の置き場所・古い版の記録の片付け)、残った子孫 (記録するもの・照合の規則・子を spawn するスレッド・切り替えた target dir)、進行中の印 (試し取り・枠を待つ間・git の 2 つの鍵・呼び直し)、その他 (置き換えられた古いビルドの枠、Windows で写像中の派生物、同名の crate の並行のビルド、結合テストの一覧) (v0.15。v0.16 で本文の細部をここへ移した) | M2 で決めた (v0.20。m2-plan.md の B1・C1・C2・C4・D2・D3a・F1b と M2e の決定 13)。**ロック**: 置き場所は state でなくキャッシュの `locks/` (キャッシュを共有するプロセスどうしなら state の場所が違っても排他が効く。子の記録と作業用ディレクトリもキャッシュの下へ移した。R-CA-1・R-DIST-3)。名前は種類の前置きを付けた鍵の SHA-256 の先頭 32 桁で、形式の版を入れない。取る順は仕事の鍵 → target dir (`target:`。ビルドは共有、GC と使えなくなった target dir の削除は排他) → `doc:<target dir>` (同名の crate の JSON と dep-info の上書きを防ぐ) → ビルドの枠。NFS では効かないことがある旨を README に書いた (R-CA-4)。**版の違うサーバーの同居**: 形式の版は記録の中身でなくパス (`local/v1` など) に入れ、古い版の置き場は GC が消す。子の記録は、持ち主が止まって 7 日たった他の版のものを、子孫を止めずに記録だけ消す (案の「読めない記録には触らない」を改めた。R-CA-4)。**残った子孫**: 子はプロセスと同じだけ生きる専用のスレッドから起動する。持ち主の生死は PID と起動時刻でなく、サーバーの実体 ID のロックで判定し、子孫は子に継承させたロックと、セッション ID・起動時刻・マシンと起動の識別で照合する。確かでなければ止めずに、その target dir に `unusable` の印を置き、ビルドは `<12 桁>.1`… のうち印の無い最初の dir を使う。Windows は記録せず、Job Object に任せる (R-WS-6・R-WS-2)。**進行中の印**: 案のとおり。外のツールは仮のプロジェクトを作る前から公開まで (枠を待つ間も) 鍵ごとの排他ロックを持ち、照会のツールは共有ロックの試し取りで覗く。git は要求の鍵と commit の鍵の両方を持ち、合流した呼び直しは終わるのを待ってから解決し直す (R-WS-10)。**その他**: 呼び出しの期限を過ぎて裏で続くビルドは枠を持ち続ける (R-WS-6)。2 つのプロセスが同じ crate を同時に引いても取得と解析は 1 回で、Windows で相手が写像中のアーカイブを置き換えにいかないことを CI で確かめた。GC と `cache clear` は置き場を `work/gc/` へ rename してから消す (R-CA-5)。結合テストは 2 プロセス・取り消し・範囲の外の build.rs の罠 (R-QA-5) を入れた。版の違うサーバーを並べて動かす結合テストは無く、既知の残りとして M3 の最初の段で足す (m2-plan.md の §7.1 の決定 5) |
| Q-30 | M0 で確かめる cargo の挙動 | すべて M0 で決めた (v0.17): host で `build.target` も無ければ `--target` を付けない (R-WS-1)。依存の種類は stable の `cargo tree` で判定し、package id は `cargo metadata` に対応付ける (R-MCP-3)。ローカルビルドの経路での `cargo metadata` の失敗は、例外 (`lockfile_outdated`・`fetch_failed`・`offline_miss`) に当たらなければ `build_failed` (一時的) にまとめ、版の解決は Cargo.lock の直読みで続ける (R-WS-7・R-VR-2)。git の決定的な失敗は libgit2 と CLI の文言で判定し、子に `LC_ALL=C` を渡す (R-WS-7)。通信の途中で切れた本体は zstd の展開より先に HTTP の層で失敗し、一時的な失敗になる (`content-length` がある場合。m0-plan.md の A4)。`rust-std` の有無は sysroot で先に判定する (R-TG-1)。実行ファイルの一部だけの指定は同じディレクトリから補い、rustup が無ければ PATH のものを使う (R-WS-1)。target の JSON 仕様は v1 では対応しない (R-TG-1)。resolver 2 以上で `-p` が panic する依存は起動前に判定する (R-WS-5) |
| Q-31 | ローカルビルドの細部で、M0 で決めきれず M2 に回したもの (v0.17・v0.18): `--target` を付けない形での M0 の再確認、同じ package が host 用と target 用の両方に現れるときの feature の組、仮のプロジェクトの toolchain・`RUSTC_BOOTSTRAP` の値・target dir、鮮度の鍵を要求した組と実際に使った組のどちらで作るか、`RUSTC_BOOTSTRAP` の値の長さの上限と nightly の経路で壊れたときの member の名前だけの値でのやり直し、`-Z fine-grain-locking` と `cargo doc --output-format json`、`--cfg docsrs` で壊れた crate の記録、複数の member・path 依存・`default-members` での組、cargo の設定の読み方、実行ファイルを探す規則 | M2 で決めた (v0.20): `--target` を付けない形でも M0 の B1・B3・B5 と同じ結論だった (m2-plan.md の B7。R-WS-1)。同じ package が host 用と target 用に出るときの `enabled_in_project` の組は M1 の選び方のままで、host 向けにだけ作られる依存は host でビルドする (D7a。R-FT-3・R-TG-1)。仮のプロジェクトは、置き場で rustup が選ぶ既定の toolchain (形式が古ければ R-WS-8 の nightly)、`RUSTC_BOOTSTRAP=1`、すべての仮のプロジェクトで共有する 1 つの target dir (M2e の決定 2・3 と E2a。R-WS-5・R-WS-8)。鮮度の鍵は計画した入力で作り、実際に使ったもの (後退した形) は記録に残す (D4 の決定 1。R-WS-4)。`RUSTC_BOOTSTRAP` の値の上限は 30,000 バイトで、超えたら `1` (D2)。nightly の経路で壊れても member の名前だけの値でやり直さない (依存の作り直しを招き、壊れた例も無い。例が出たら考える。m2-plan.md の §7.1 の決定 4。R-WS-8)。`-Z fine-grain-locking` と `cargo doc --output-format json` は nightly の経路でも使わない (D2。Q-17)。`--cfg docsrs` で壊れたことは置き場の前回の成功の記録で覚える (D3b・D4d。Q-11)。複数の member などでは `cargo rustdoc -p` と `cargo tree --workspace` の組が食い違うと実測し、member は全 member を統合した組で作る (D3c・D3d。R-FT-3・Q-20)。cargo の設定は、隔離した環境で合成の規則を確かめて自前で読む (`$CARGO_HOME` と起動するディレクトリから上の `.cargo/config(.toml)`、`include`、`CARGO_BUILD_TARGET`。stable には cargo に読ませる手段が無い。B3a。R-WS-2)。実行ファイルの一部だけの指定は、指定された名前の版の接尾辞をまねて同じディレクトリで探し (`rustc-1.80` なら `rustdoc-1.80`)、symlink はたどらず、rustc と rustdoc の commit-hash が合わなければエラーにする (D1。R-WS-1) |
| Q-32 | worktree (`claude -w`・subagent の worktree) と `--add-dir` で、サーバーが受け取る `CLAUDE_PROJECT_DIR` とカレントディレクトリ、worktree の置き場所 (R-MCP-3。v0.18) | M2 の前に実測して決めた (Claude Code 2.1.304、Linux。m2-plan.md の §6.1 と §5 の P3〜P5・A2。v0.20): 通常の起動ではどちらも Claude Code を起動したディレクトリ、`claude --worktree` ではどちらも worktree のルート (`.claude/worktrees/<name>`)。subagent の worktree は親と同じサーバーのプロセスを使い、サーバーからは見えない。`--add-dir` のディレクトリは `roots/list` にだけ出る。そこで、同じ git リポジトリの別の作業ツリーの同じ場所の workspace を既定のプロジェクトと同じに扱い (git を起動せず、ファイルを読むだけで判定する)、worktree では `project` に作業ディレクトリを渡すよう instructions と説明で案内し、`--add-dir` のディレクトリは `project` で指す (`roots/list` は使わない)。サブディレクトリで起動したときは、git の作業ツリーのルートまで上へ探す (§4。R-MCP-3) |

### 11.2 残る未決事項

| # | 内容 | 決め方 |
| --- | --- | --- |
| Q-5 | 依存グラフ全体の implementors にかかる時間と容量 (先行例なし) | 設計とベンチマーク |
| Q-14 | cfg の痕跡属性の表記が今後どう変わるか | nightly canary (R-QA-4) で追う (M1 で canary を入れ、表記が変われば落ちるようにした。v0.19。M2 で beta の toolchain の canary を別の job に足した。m2-plan.md の F3b。v0.20) |
| Q-15 | `alwaysLoad` の効果、`inputSchema` の `examples` がモデルに見えるか | eval (M1 では測っていない。M5。v0.19)。M2 の F4b で、`alwaysLoad` なしの選ぶ側 (名指しせずに自分の workspace の item を問う) を 3 回測り、2 回は ToolSearch で読み込んでこのサーバーを使った。`alwaysLoad` との比較と `examples` は M5 のまま (m2-plan.md の F4b。v0.20) |
| Q-22 | 解析後にメモリ上限 (R-MEM-1) を超える巨大な crate (windows-rs) を扱う方式 (案: 全体を構造体に読み込まずにストリーミングで索引にする。R-MEM-2)。メモリに置けない JSON をどう受け取るか (v0.18 までの R-NW-3 の「ディスクへ直接展開する」。v0.19) | 設計とベンチマーク。M4 以降 (R-NW-3 と一緒に。M1 は 512MiB を超えると `too_large`。v0.19) |
| Q-25 | cargo#16720 (`cargo doc --output-format=markdown`) が実装されたら、取得元に加えるか、本サーバーの役割を見直すか | 上流の動きを見て判断する |
| Q-27 | メモリアロケータ (mimalloc など) を替えるか。musl 版 (R-DIST-1) の性能が §9.1 の目標から大きく遅れる場合と、gnu 版でも parse と解放が速くなる場合 (M0 の A8 で、`get_item` の時間はほぼ parse と解放だった) | gnu 版は M1 で答えが出た: 手放したメモリを `malloc_trim(0)` で OS に返せば足り、替えない (R-MEM-2。m1-plan.md の E2。v0.19)。musl 版は M5 で測って決める (v0.17) |

---

## 12. マイルストーン (案)

| 段階 | 内容 | 完了条件 |
| --- | --- | --- |
| M0 PoC (完了。2026-10-19。結果は [m0-plan.md](m0-plan.md) の §6) | stdio サーバー (rmcp) と `get_item` (docs.rs の JSON、形式の正規化 (54〜61)、公開パス索引込み)。ローカルビルドの PoC (`RUSTC_BOOTSTRAP`、`cargo doc --output-format json`、feature の組、`--cfg docsrs`、dep-info) | Claude Code から呼べる。R-OUT-1 の挙動を再確認。Q-12・Q-17・Q-20 と、R-FV-3 の先読みの方法、R-WS-4 の dep-info の取り方が決まる。Q-11 の、渡さずにやり直す頻度を測る。R-MCP-16 のサーバーから閉じるときの扱い、R-OUT-2 の膨張率、R-MCP-3 の `cargo tree` の出力の形の安定性、Q-30 の cargo の挙動、rustdoc_processor を依存にするか (§10) を確かめる |
| M1 MVP (実装は 2026-10-20 に終えた。結果は [m1-plan.md](m1-plan.md) の §6。crates.io への公開は利用者が行う。v0.19) | 形式 46〜53 の shim (R-FV-2・R-FV-4)、バージョン解決 (§5.3)、`list_crates` / `resolve_version` / `crate_overview` / `get_item` / `search_items` / `list_module` / `list_methods` (固有と trait 由来。R-TI-1) / `list_impls` / `list_implementors` (crate 内のみ)、一覧の絞り込みと展開 (§6.5)、一覧への feature の注記 (R-FT-2)、定義位置 (R-OUT-8)、std (dist からの直接取得 R-STD-6 を含む)、正準パス、impl の構造的な分類、描画規則 (§7)、ディスクキャッシュ (原子的な書き込みだけ。プロセスをまたぐロックは M2。R-CA-4)、エラー設計、構造化ログ (R-OBS-1。`request_id` と合わせて。R-ERR-2)、小さな eval (M1 のツールで答える §3 の問いと、Claude Code の挙動の検知のケース。R-QA-6。v0.18)、使える版ができたら crates.io への最初の公開 (R-DIST-1。v0.18)。本文が M1 で決めるとした細部 (R-OUT-9 の応答の骨格、R-OUT-3 のカーソル、R-CA-4 の否定キャッシュと後始末、R-TI-4 の索引の構成など) も、M1 の計画で本文から拾う。`search_items` の既定の範囲 (R-SR-2) のうち workspace の分は M2、`list_crates` の advisory の件数は M4 から。**到達点** (v0.19): 9 本のツールが揃い、§3 の Q1〜Q6・Q8・Q17・Q20 に答えられる。Q7 は固有と trait 由来のメソッドだけ (Deref 先とプリミティブは M4)、Q9 は trait の crate の中だけ (依存グラフ全体は M4)。小さな eval の 18 問はすべて正解 (R-QA-6)。M1 で決めた細部は本文の各所に「M1」として書いた (v0.19)。公開前の見直しで、std のキャッシュの片付け (R-STD-6・R-CA-5)、アーカイブの鍵の `types_version` (R-CA-2)、読める nightly へのさかのぼり (R-STD-6)、起動時の `rustc -vV` を待たせないこと (R-MCP-11)、取得と解析の同時実行の上限とリダイレクト (R-NW-2)、ログの削除 (R-OBS-2)、カーソルのツール名 (R-OUT-3) を直した。M1 の終わりの判断で、窓口のパス (R-PR-2)、古い index の利用 (R-CA-3)、`default_unstable` (R-STD-2)、`list_impls`・`list_implementors` の feature の注記 (R-FT-2)、存在しない crate の否定キャッシュ (R-ERR-3)、今日の日付のログのロックを消さないこと (R-OBS-2) を公開前に入れた | 依存 crate と std について、ferritin のホスト版より正確で小さい答えが返る (R-QA-1 の代表的な item のスナップショットで比較する。eval での比較は M5)。§3 の Q20 (umbrella crate) に答えられる。`--no-default-features` の構成でもビルドとテストが通る (R-ARCH-8)。3 つとも満たした (ferritin との比較は R-QA-1。v0.19) |
| M2 workspace (完了。2026-10-24。結果は [m2-plan.md](m2-plan.md) の §6。v0.20) | 着手の前に Q-32 を確かめる (v0.18)。Q-29・Q-31 の細部を決める。ローカルビルド (R-WS-8 の toolchain の選び方、内容ダイジェストの鮮度判定、永続 target dir、プロセス間ロック、ビルド中の応答、ビルド失敗のキャッシュ)、private items (公開パスの無い item のパスと検索での扱い。R-PR-2・R-SR-4。v0.18)、docs.rs に JSON が無い場合のローカルビルド (求める target の JSON が無い場合を含む。R-TG-1。v0.18)、`build_external_docs` (R-WS-10)、CLI の `cache` と GC の最小限 (容量の上限。R-CA-5。v0.16)。M1 から回したもの (v0.19): プロジェクトの `build.target` を読むこと (R-TG-1・Q-31)、明示した版とプロジェクトの取得元の照合とそのときの `enabled_in_project` (R-VR-1・R-FT-2)、暗黙の member と `cargo metadata` の `workspace_root` の突き合わせ (R-MCP-3)、`cargo metadata`・`cargo tree` の同時の呼び出しの合流と失敗の保持 (R-VR-2・R-WS-7)、cargo の設定の `include` を `cargo metadata` の鍵に入れること (R-VR-2)、1 つの crate の照会の 30 秒の上限と `status: building`・`building_reason: fetching` (R-ARCH-6・R-WS-6)、子の後始末 (`PR_SET_PDEATHSIG`・Job Object・`waiting_for_lock`。R-WS-6・R-WS-2)、プロセスをまたぐロック (R-CA-4) と取得の singleflight (R-NW-2)、利用者の `net.offline` での `offline_miss` (R-WS-7)、設定ファイルと知らない設定の知らせ (R-DIST-3。オフラインモード (R-CA-7) と dist を無効にする設定 (R-STD-1) もここで)、範囲の外の依存の build.rs の罠のテスト (R-QA-5)、beta の toolchain の canary (R-QA-4)。公開前の見直しでレビュアーが挙げた残り (M2 以降。どの段で入れるかは M2 の計画で振り分ける): プロセスをまたぐロック (Windows で 2 つのセッションが同じアーカイブを同時に書くと、相手が mmap しているファイルを置き換えにいく)、取り消しや時間切れで孫のプロセス (git など) が残ること、`.crate` の取得が crates.io のダウンロード数に数えられうること、`project_from` の値 (`CLAUDE_PROJECT_DIR` など) の名前、`--debug` でインストールすると試験用の `sleep` ツールが出ること、実データのテストと罠のテストが Linux でしか回らないこと、`cargo publish --dry-run` が CI に無いこと。**到達点** (v0.20): 自分の workspace の member をここでビルドし、private と hidden の item まで引ける (m2-plan.md の D2・D6)。編集の後は、計画した入力の内容ダイジェストと dep-info で鮮度を判定して作り直し、作り直しの間は前回のビルドを `stale` で返す (D4)。docs.rs に JSON の無い依存もローカルビルドし (D7)、`crate` なしの検索は workspace の member と直接依存を引く (D7d-2)。`build_external_docs` で、依存でない crate・名指しできない crates.io の依存・git の crate・既定のプロジェクトの他の target・外の workspace を作り、照会のツールで読める (E1〜E4)。ビルドの失敗の記録と `refresh` (D5)、ビルド中の応答と時間の上限 (C3・D3a)、プロセスをまたぐロックと子の後始末 (B1・C1・C2・C4) も入れた。M1 から回したものと公開前の見直しの残りは、すべて M2 の段で入れた (`.crate` の取得は crates.io のダウンロード数に数えられないと分かった。m2-plan.md の §3.7・B6)。M2f で、キャッシュの CLI と GC (F1)、範囲の外の build.rs と rustc の罠のテスト (F2)、beta の canary (F3)、instructions と説明文の直しと、eval の自分の item と選ぶ側のケース (F4)、ローカルビルドの時間と診断の測定 (F5)、README (F6)、出力のサニタイズ (F7) を入れた。実データのテストは 3 つの OS で回り、CI の 11 の job が緑。crates.io には公開していない (M3 の後に利用者がまとめて公開する。m2-plan.md の §5 の P9)。回したものは M3〜M5 の行に書いた | 自分の workspace を引ける。編集後に正しく再ビルドされる。外のプロジェクトと git の crate を引ける。3 つとも満たした (v0.20) |
| M3 crate 情報 | features (`inspect_features`。cfg の計算自体は M1 の R-FT-2) / README / changelog / examples / metadata / `search_crates` / `get_source`。M2 から回したもの (v0.20): 最初の段で M2 の既知の残りを直す (m2-plan.md の §7.1 の決定 5・6・8・9): R-WS-7 との 2 つのずれ (ほとんど cargo のロック待ちだった時間切れも決定的な失敗として覚えること、`cargo metadata` の git の決定的な失敗 `fetch_failed` を 30 秒しか覚えず `refresh` の案内も無いこと) と、`toolchain_too_old` の suggestions が rustup の無い環境でも `rustup update` を書くこと (R-WS-7)、版の違うサーバーを並べて動かす結合テスト (R-CA-4・Q-29)、`resolve_version` で `target` を省くと、プロジェクトが使わない crate もプロジェクトの target で docs.rs を確かめること (M2e の決定 11。§6.3)、`search_items` で std を検索するとき `project` で指したプロジェクトの target を見ないこと (R-STD-1)、ローカル機能の無い構成の `search_items` の `inputSchema` で `crate` が required でないこと (R-ARCH-4)、外の workspace の `lockfile_outdated` が古い Cargo.lock にも `cargo generate-lockfile` を勧めること (§8)、ローカルビルドした member の `source_location.file` が workspace のルートからの相対パスらしいこと (下の `path` と一緒に。R-OUT-8)。ほかに: ローカルビルドした crate の `source_location` に開ける絶対パスの `path` を足す (`get_source` と一緒に。R-OUT-8。m2-plan.md の §7.1 の決定 2)、ソースの本文の見える印 (`get_source` と一緒に。R-SEC-7。m2-plan.md の F7 の決定 4)、workspace から継承した `readme` と拡張子の無い `README` (R-SC-4)、`.crate` の展開 (R-NW-3。ソースのツールと一緒に)、M3 の後に crates.io へまとめて公開する (利用者が行う。R-DIST-1。m2-plan.md の §5 の P9)。M3 以降 (版を決めない): 設定 `search.scope`・`build.prefer_local` (R-SR-2・R-DS-1)、「(in a later version)」を返す場合 (名指しできない path・git の依存など。R-WS-5・R-WS-10)、Windows で子を Job Object に入れる前に子が起動したプロセスが漏れる隙間 (R-WS-6) (m2-plan.md の M2f の残りの行き先の決定 3・4 と §6.7) | §3 の Q10〜Q13・Q15・Q16 と、Q14 のうち changelog で答えられる部分に答えられる。M2 の既知の残りが直っている (v0.20) |
| M4 差別化 | 依存グラフ全体の implementors、Deref とプリミティブを含むメソッド解決、`#![no_std]` の member での core・alloc のパス (R-PR-2。v0.18)、MSRV を踏まえた std 表示、`api_diff`、`audit_dependencies`、`rust_release`。任意: `search_source` (R-SC-6)、シグネチャ検索と同梱の散文の検索 (R-SR-1)、docs.rs のビルド時の依存の版との差の警告 (R-PR-5)、stable の版に近い nightly の `rust-docs-json` の選択 (R-STD-6)。M1 から回したもの (v0.19): シグネチャの中のほかの crate の型を正準パスで描くこと (R-OUT-2)、`also_reexported_at` ごとの条件・`doc(auto_cfg(hide))`・`reexport_of` の条件 (R-FT-2)、モジュールを丸ごと re-export した先の item の検索の索引と R-TI-4 の逆引き索引 (依存グラフ全体の索引と一緒に。R-SR-1・R-TI-4)、どの glob で trait がスコープに入るか (R-PR-2)、プリミティブ型のメンバーの探し方 (R-PR-7)。M4 以降: メモリに置けない巨大な JSON の扱い (R-NW-3・Q-22)。M2 から回したもの (v0.20): 公開パスを持たない型の `unnameable` (依存グラフ全体の索引と一緒に。R-PR-2。m2-plan.md の §7.1 の決定 2)、`build_external_docs` の `document_hidden_items` (`api_diff` と一緒に。R-WS-10・R-AD-3。m2-plan.md の M2e の決定 9) | 既存実装では答えられないか不正確な問い (§3 の Q7、Q9、Q14 の破壊的変更、Q17〜Q19) に答えられる。任意の `search_source` を入れた場合は Q21 も |
| M5 仕上げ | eval の比較と校正の仕上げ (問題集は M1 から各段階で足す。R-QA-6。v0.18)、性能目標、GC の既定値の見直し、メモリの上限 (`limits.memory`。OS ごとに測るものと、超えたときの捨て方。R-MEM-1。v0.19)、eval の基盤を `claude plugin eval` に移す (R-QA-6。v0.19)、Claude Code プラグインとしての配布、配布の方式 (配る target (Q-27)・リリースの流れ・公式 MCP レジストリに登録するかなど) を決める (R-DIST-1)。任意: resources (R-MCP-13)、prompts (R-MCP-18)、completions (R-MCP-19)、icons (R-MCP-7)、skill の単独インストール (R-DIST-6)、隔離環境 (R-CA-8)。eval で採否や既定を決めるもの: `output.mode` (R-OUT-1)、`detail` の既定 (R-OUT-7)、`alwaysLoad` (R-MCP-8・Q-15)、`max_chars` (R-OUT-3) (`tools.profile` (R-MCP-12) は v0.18 で将来に回した)。M2 から回したもの (v0.20): `alwaysLoad` の比較は、M2 の F4b の選ぶ側のケース (名指しせずに自分の workspace の item を問う) でも行う (R-MCP-8・Q-15)、プラグインの接頭辞の実物で README の許可ルールの例を確かめる (M2 は末尾のツール名で合わせた。R-SEC-2・R-QA-5) | eval の問題集で §3 の問いに答えられる。M1・M4 で測って決めた §9.1 の目標を満たす。GC が容量の上限を守る。Claude Code のプラグインとして 3 OS で導入できる (任意の機能と M6 は含めない。v0.16) |
| M6 共有 (任意) | Streamable HTTP (R-MCP-1、R-SEC-5、R-MCP-17)。任意: tasks 拡張 (R-WS-6)、認証 (静的なトークンか OAuth。R-SEC-5)。ホスト版か共有キャッシュサーバー (§9.10) を作ると決めた場合だけ着手する | 複数のセッションから 1 プロセスを共有でき、`Origin` と `Host` の検査 (R-SEC-5) が効く |

---

## 13. 参照

- 調査: [rustdoc-mcp-survey.md](rustdoc-mcp-survey.md)
- ferritin の設計: [ARCHITECTURE.md](https://github.com/jbr/ferritin/blob/main/ARCHITECTURE.md)、[conversions](https://github.com/jbr/ferritin/tree/main/ferritin-common/src/conversions) (形式の正規化と shim)
- docs.rs の JSON 配布: <https://docs.rs/about/rustdoc-json>
- rustdoc-types: <https://crates.io/crates/rustdoc-types> (0.61.0 = 形式 61、2026-07-29)。上流の定義: [rust-lang/rust `src/rustdoc-json-types/lib.rs`](https://github.com/rust-lang/rust/blob/master/src/rustdoc-json-types/lib.rs)。形式 47 の欠番: [CHANGELOG](https://github.com/rust-lang/rustdoc-types/blob/trunk/CHANGELOG.md)
- rustdoc JSON の安定化の tracking issue: [rust-lang/rust#76578](https://github.com/rust-lang/rust/issues/76578)
- cargo の rustdoc JSON 出力: [cargo#17025](https://github.com/rust-lang/cargo/pull/17025) (`cargo doc --output-format json`)、[cargo#16773](https://github.com/rust-lang/cargo/pull/16773) (unit ごとの出力)、[cargo#13283](https://github.com/rust-lang/cargo/issues/13283) (`cargo rustdoc --output-format` の tracking issue)。監視対象: [cargo#16720](https://github.com/rust-lang/cargo/issues/16720) (`cargo doc --output-format=markdown`。エージェント向けの API ドキュメントを Cargo.lock から作る提案)
- migrate_rustdoc_json (形式の移行コード): <https://github.com/BD103/migrate_rustdoc_json>
- officina (Pavex の rustdoc_processor / rustdoc_resolver): <https://github.com/LukeMathWalker/officina>
- cargo-public-api: <https://github.com/cargo-public-api/cargo-public-api>
- cargo-semver-checks: <https://github.com/obi1kenobi/cargo-semver-checks>
- ruskel: <https://github.com/cortesi/ruskel>
- trustfall_rustdoc (cargo-semver-checks): <https://crates.io/crates/trustfall_rustdoc>
- MCP 2026-07-28 の仕様 (v0.10 で参照): changelog、basic/versioning、basic/index (`_meta`・`resultType`・エラーコード)、server/discover、server/tools、transports (stdio・streamable-http)、patterns (mrtr・progress・cancellation・subscriptions)、server/utilities/caching、server/resources、client/elicitation、deprecated (いずれも <https://modelcontextprotocol.io/specification/2026-07-28/> の下)
- rmcp のリリース: <https://github.com/modelcontextprotocol/rust-sdk/releases>
- Claude Code の MCP の文書: <https://code.claude.com/docs/en/mcp>、環境変数: <https://code.claude.com/docs/en/env-vars>、[CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)
- Claude Code の Issue: [#79944](https://github.com/anthropics/claude-code/issues/79944) (`structuredContent` だけがモデルに渡る)、[#86032](https://github.com/anthropics/claude-code/issues/86032) (エラー時は `structuredContent` を無視する)。追っている Issue と、直ったときに直すところは [claude-code-issues.md](claude-code-issues.md) にまとめた
- crates.io の data access policy と crawler policy: <https://crates.io/data-access>、<https://crates.io/policies#crawlers>

---

## 14. 改訂履歴

### v0.20 (2026-10-24)

M2 (workspace) の記録 ([m2-plan.md](m2-plan.md) の §6) から、決めたことと事実を反映した版。M2 の各段で利用者が決めたこと (§6 の各段の「下調べと決定」) を本文の各所に「M2」として書き、反映のときに判断の要った 9 件は M2 の終わりに利用者が決めた (m2-plan.md の §7.1。下の 22)。

1. プロジェクトの決め方: worktree と `--add-dir` で受け取る値を M2 の前に実測し (Q-32)、同じ git リポジトリの別の作業ツリーの同じ場所の workspace を既定のプロジェクトと同じに扱う (git を起動せず、ファイルを読むだけで判定する)。起動したディレクトリにも直下の子にも Cargo.toml が無ければ、git の作業ツリーのルートまで上へ探す。`--add-dir` のディレクトリは `project` で指し、`roots/list` は使わない。`project_from` の値を `claude_project_dir` に改め、暗黙の member を `cargo metadata` の `workspace_root` と突き合わせる (§4・R-MCP-3・§6.4・§7.3。m2-plan.md の §6.1・§5 の P3〜P5・P8・A2・B4)
2. ローカルビルドの toolchain: 初めて使う toolchain は、キャッシュの中の小さな crate に rustdoc を直接走らせて形式を調べ、`rustc -vV` の commit-hash を鍵に覚える。古すぎれば nightly (`build.nightly_date`) に切り替え、新しすぎても読めれば warnings 付きで使う。子から `RUSTUP_TOOLCHAIN` を外し、実行ファイルの一部だけの指定は版の接尾辞をまねて同じディレクトリで探す。`RUSTC_BOOTSTRAP` の値の上限は 30,000 バイトで、超えたら `1`。`--target` を付けない形も M0 と同じ結論で、`-Z fine-grain-locking` と `cargo doc --output-format json` は使わない (R-WS-1・R-WS-8・Q-12・Q-17・Q-31。D1・D2・B7)
3. target dir とロック: 専用の target dir は `<キャッシュ>/t/<12 桁>/` の `target/` と `build/`。止めなかった子孫のいる dir には `unusable` の印を置いて別の名前で作り直す。他の target・docs.rs の組の member・仮のプロジェクトは target dir を分ける。ロックはキャッシュの `locks/` に、種類の前置きを付けた鍵のハッシュの名前で置き、仕事の鍵 → `target:` → `doc:` → ビルドの枠の順に取る。キャッシュの項目のロックの待ちには上限を置き、ビルドのロックは上限なしで待つ (R-WS-2・R-CA-4・Q-29。C1・C2・D2・E4a)
4. 記録の置き場と子の後始末: ロック・子の記録・作業用ディレクトリを state からキャッシュの下へ移した。子はプロセスと同じだけ生きる専用のスレッドから起動し、正常に終わった後も残った孫をグループごと止め、出力を読む時間に上限を置く。残った子孫は、サーバーの実体 ID のロックと子に継承させたロックで照合し、確かでなければ止めずに target dir を使わない。Windows は記録せず Job Object に任せ、Job に入る前の隙間は M3 以降 (R-WS-6・R-CA-1・R-DIST-3・Q-29。B1・C4・止まる危険の監査と直し 1・2)
5. feature の組と `--cfg docsrs`: member は全 member を統合した組を `--no-default-features --features` で明示してビルドし、`enabled_in_project` と一致させる。docs.rs の組は、通常の組の範囲に無いパッケージが増えないときだけ使う。壊れたら `--cfg docsrs` を外し、次に通常の組に戻し (最大 3 回)、後退したことは置き場の前回の成功の記録で覚える (R-FT-3・R-WS-1・Q-11・Q-20。D3b〜D3e・D4d)
6. 鮮度: 鍵は計画した入力で作り、実際に使ったものは記録に残す。ファイルは (パス・大きさ・mtime・SHA-256) で記録し、git の作業ツリーの中は `git ls-files` で集める。dep-info の 2 回目の rustdoc は応答の後に裏で走らせ、build script の `rerun-if-changed`・`rerun-if-env-changed` も鍵に入れる。置き場は crate ごとに 1 つで、鍵と入力の一覧は `record.json` に持つ (R-WS-4・R-CA-2・Q-31。D4a・D4b)
7. ビルド中の応答: ビルドは `build.wait_secs` まで待ち、過ぎたら `status: building` と `building_reason`・`retry_after_secs` を返して裏で続ける。1 つの crate の照会にも時間の上限を置き、取得とビルドは共有の仕事にして、待つ呼び出しがすべて取り消されたときだけ止める。待つ間は進捗を通知し、ビルド中もカーソルを使える。作り直しの間は前回のビルドを `stale: true`・`stale_reason: rebuilding` で返し、LSP を案内する (R-WS-6・R-ARCH-6・R-OUT-9・R-OUT-3・R-WS-9・R-NW-2。C2b・C3・D2c・D3a・D4c)
8. 失敗の分類と記録: 決定的なビルドの失敗はディスクの `failed.json` に覚え、入力が変わるか `refresh` まで同じ失敗を返す。作り直すときはビルドの前に `cargo fetch --locked` を走らせて取得の失敗を分け、`toolchain_too_old` を分ける。診断の上限を決め、`refresh` はその crate の失敗と後退をすべて解く。実装とのずれ 3 件は既知の残りとして書いた (R-WS-7・§6.4・§8。D5a・D5b・B5)
9. private と hidden: member は `--document-private-items` と `--document-hidden-items` で作り、可視性 (`pub(crate)`・`pub(in crate::…)`・`private`) と `crate::` のパスを示す。検索と一覧にも private を含め、依存は公開の item だけに解決する。member の間の re-export と glob をたどる。private なフィールドに印は足さない (R-PR-2・R-PR-3・R-PR-4・R-WS-3・R-SH-5・R-SR-4・R-TI-1〜R-TI-5・R-OUT-2。D6a〜D6c・F4b)
10. docs.rs に無い依存のローカルビルド: docs.rs の `no_target_json`・`no_json`、checksum の不一致と欠け、index に無い版で、プロジェクトが版を決めた依存・その re-export の行き先・プロジェクトと同じ明示した版をローカルビルドに戻す。明示した版は完全一致だけを照合する。実際のビルド target を判定し、独自の target は `target_unsupported`。target を省いた docs.rs の照会は既定の target で引く。`.crate` は手元の cargo のキャッシュから読む (R-DS-1・R-DS-3・R-DS-5・R-VR-1・R-VR-2・R-TG-1・R-SC-2・R-NW-1。B6・D7a〜D7d-1)
11. `crate` なしの検索: workspace の member (lib か、lib が無ければ bin 1 つ) と直接依存を 1 つの共有の仕事で引く。作れないものは理由を分けて `sources` に載せ、作り直し中のものは stale として含める。外の workspace は記録から引く (R-SR-2・R-SR-5。D7d-2・E4b-3b)
12. `build_external_docs`: 本体はローカルビルドの置き場に置き、外の照会の鍵から引く索引だけを別に持つ。進行中の印はロックで表し、照会のツールが記録を読む順を決めた。依存でない crate (仮のプロジェクトは `RUSTC_BOOTSTRAP=1`、yanked の版は作らない)、名指しできない crates.io の依存、features のやり直し、git の crate (URL の検証、要求の鍵と commit の鍵、1 日の古さ、短いハッシュ)、既定のプロジェクトの他の target、docs.rs の組の member (M2e の決定 10)、外の workspace を作る。依存でない crate は `target` を省けば host で引く (M2e の決定 11) (R-WS-10・R-WS-5・R-MCP-3・§6.3・§6.4。E1a〜E4c)
13. stale の理由と消えたプロジェクト: `stale_reason` に `rebuilding`・`inputs_changed`・`ref_ttl_expired`・`project_missing` を設け、それぞれに理由と hints を添える。外の workspace が消えたら、記録のある root と別名は古い記録を `project_missing` で返し、記録の無いパスは `project_not_found` (R-OUT-9・§8・R-MCP-3・R-WS-9。D4c・E3b-2・E4a-3・E4b-3)
14. 照会の時間と失敗の記憶: std の配布物・`cargo metadata` の初回・features と範囲の判定も共有の仕事にして期限で `status: building` を返す。失敗を覚えていなかった取得 (std の要約・sparse index・`.crate`) は短く覚え (`.miss.json`)、`cargo metadata`・`cargo tree` の呼び出しを合流して失敗を保持する。cargo の設定の `include` を鍵に入れ、cargo の `net.offline` で `offline_miss` を返す (R-NW-2・R-ERR-3・R-CA-7・R-VR-2・R-WS-7。B3a・C3c・止まる危険の監査と直し 2)
15. 設定: 設定ファイルを入れ、知らないキーと読めない値は起動を止めずにログで知らせる。環境変数で受ける値の形を決めた。`std.fetch_from_dist`・`limits.extract_max_size`・`network.offline`・`limits.concurrent_builds`・`build.*`・`cache.*` を足した。`build.prefer_local` と `search.scope` は M3 以降で検討し、README に載せない (R-DIST-3・R-STD-1・R-NW-3・R-CA-7・R-DS-1・R-SR-2。A3a・A3b・M2f の残りの行き先の決定 3)
16. キャッシュの CLI と GC: `cache ls`・`cache gc [--dry-run]`・`cache clear [--dry-run] [<crate>[@<version>]]` を入れた。文書と target dir の容量の上限と保持日数で GC し、所有マーカー (`chatora-rustdoc-mcp.cache`) と引き取りの条件を決めた。起動時とビルドの後 (1 時間に 1 回) に裏で GC し、置き場を `work/gc/` へ rename してから消す。使用中のものは飛ばす。std の 7 日の規則は GC の部品にした (R-CA-5・R-DIST-2・R-STD-6・R-CA-4。F1a〜F1c)
17. 出力のサニタイズ: 制御文字 (改行とタブを除く)・ANSI のエスケープの列・双方向の制御と印・見えない文字とゼロ幅の文字・異体字セレクタ・タグ文字を、すべての応答・エラー・JSON-RPC のエラー・進捗の通知から 1 か所で除く。改行以外の行の区切りは改行にする。cargo の stderr と診断は切る前に直し、除いた数をログに書く。ソースの本文の見える印は M3。呼び出しの上限を超えたら常に `busy` を返すよう要件を改めた。git の URL の検証を広げ、呼び出しの記録で userinfo を伏せる (R-SEC-7・R-SEC-4・R-OBS-1・R-OUT-1。F7・E3a・M2f の残りの行き先の決定 1・2)
18. instructions と説明文: instructions が自分の workspace の item を LSP へ誘導していたのを改め、LSP との境界 (§9.9) だけを書く。説明文はローカル機能の有無で出し分け、`version`・`target` の既定の食い違いを直した。ローカル機能の無い構成の説明文に残っていたプロジェクトについての問いを直した (M6 から外した) (R-MCP-6・§6.2・§9.9・R-ARCH-4。F4a-1)
19. テストと評価: 実データのテストを 3 つの OS で回す (windows-gnu の問いと外の workspace の fixture を含む)。範囲の外の build.rs と rustc の罠のテスト、beta の canary (job ごとの Issue)、プロジェクトの外で回す canary を入れた。eval に自分の item・ビルド中・stale・外の workspace・他の target・worktree の 9 問と選ぶ側のケースを足し、F4b で 29 回のうち 28 回が合格。README のツール名と許可ルールの例を検査し、`cargo publish --dry-run --locked` を CI に置いた。テストの口は debug ビルドで環境変数のあるときだけ効く (R-QA-3〜R-QA-6・R-DIST-1・R-SEC-6・R-MCP-8。A4・F2・F3・F4・F6)
20. 測って決めた値: `build.wait_secs` を 10 秒から 15 秒にし、`build.jobs` の既定を CPU の半分 (2 つ以上あれば 2 以上) にした。診断の上限は据え置いた。§9.1 に、ローカルビルドの時間と大きさを目標でなく「M2 の実測 (参考)」の表として足した (目標は M5) (R-WS-6・R-WS-1・R-WS-7・R-MCP-4・§9.1。m2-plan.md の F5・§7.1 の決定 7)
21. README と配布: README を M2 の実態に合わせた (ローカルビルドと `build_external_docs`、toolchain、プロジェクト、答えが揃わないときの応答、許可ルールと Claude Code 2.1.301 の権限モードの表を含む Security の節、キャッシュ)。確認なしにビルドする範囲が別の作業ツリーにも広がったことと、ビルドしなくても走るもの (`rustdoc -vV`・`git ls-files`・`cargo fetch --locked`) を書いた。ログは自前の書き込みスレッドで書き、stdout に何も出さない。同期ロックと非同期ロックの使い分けを決めた。crates.io への公開は M3 の後 (R-SEC-1・R-SEC-2・R-DIST-1・§10。m2-plan.md の F6・止まる危険の監査と直し 2・§5 の P9)
22. M2 の終わりの判断 (m2-plan.md の §7.1): 版は v0.20 の 1 つにまとめた。R-OUT-8 の `path` は M3、R-PR-2 の `unnameable` は M4 に送った。R-WS-4 の依存の鮮度の鍵の `RUSTC_BOOTSTRAP` は値の全体とし (要件を実装に合わせた)、nightly の経路で壊れても member の名前だけの値でやり直さない。R-WS-7 との 2 つのずれ、`toolchain_too_old` の rustup の案内、版の違うサーバーを並べる結合テスト、小さな食い違い 3 件 (`resolve_version` の target、`search_items` の std の target、ローカル機能の無い構成の `crate`)、外の workspace の `lockfile_outdated` の案内、member の `source_location.file` は、要件をあるべき姿のままにして既知の残りと書き、M3 の最初の段で直す (R-OUT-8・R-PR-2・R-WS-4・R-WS-8・R-WS-7・R-CA-4・R-STD-1・R-ARCH-4・§6.3・§8・§9.1)
23. 細部の追加 (M2 で決めた形・値・順序。「M2 で決める」をすべて置き換えた): 明示した版とプロジェクトの取得元の照合 (同名同版・解決中は `status: building`。R-VR-1)、外の記録がある版は index を読まない (R-VR-6)、features と `--cfg` のために `.crate` を先に読む (R-FT-1)、re-export の行き先の版 (R-PR-4)、`has_private_fields` (R-TI-5)、継承した `readme` は M3 (R-SC-4)、`.crate` の展開は M3 (R-NW-3)、std の target と dist の無効化 (R-STD-1・R-STD-6)、アーカイブのスキーマの版と M2 の配置 (R-CA-2)、版の違うサーバーの記録 (R-CA-4)、時間の上限の設定 (R-ARCH-6・R-MCP-1)、ログの `status` と `hidden_chars` (R-OBS-1)、ローカルビルドを伴う stdout のテスト (R-SEC-1)、outputSchema の必須を緩めた (§7.3)、`refresh`・`git` の引数 (§6.4・R-ARG-1)、reason の確定 (`local_build_required` は lib の無い依存だけ。§8)、ビルドの数の上限 (R-SEC-7)、CI の追加 (R-DIST-1・R-QA-3)
24. 未決事項: Q-29・Q-31・Q-32 を解決にし、Q-11・Q-12・Q-17・Q-20 に M2 の答えを、Q-14 に beta の canary を、Q-15 に F4b の選ぶ側の測定を足した (§11)
25. マイルストーン: M2 の到達点と、M3・M4・M5 に回したもの (M3 の最初の段で直す既知の残りを含む) を足し、M6 から M2 で済んだ項目を外した (§12)

### v0.19 (2026-10-20)

M1 (MVP) の記録 ([m1-plan.md](m1-plan.md) の §6) から、決めたことと事実を反映した版。反映するものの一覧を作り、判断の要るものは利用者が 17 件決めた (m1-plan.md の §6 の「要件への反映の判断」)。あわせて、最初の公開の前の見直し 12 件 (同じく「公開前の見直し」) と、GitHub の token の扱いの見直し (「R-NW-4 の見直し」) を反映した。

1. 窓口のパス: 別の crate の item を re-export で引いたら、`data.path` は引かれたパス、定義元のパスは `data.defined_at`。provenance の crate は定義元のまま、シグネチャの中の型と `also_reexported_at` は M4 まで定義元のまま。`crate: "std"` の検索も、alloc・core の item を std のパスで示す (glob で re-export するものを除く) (R-PR-2・R-OUT-2・R-SR-2)
2. 巨大な JSON: 「ディスクへ直接展開する」をやめ、メモリに置けない JSON の扱いは Q-22 と一緒に M4 以降。M1 は 512MiB を超えると `too_large` (R-NW-3・R-MEM-2・Q-22・§9.1)
3. 性能の目標: §9.1 に M1 の目標 (キャッシュ済みの照会 p50 < 10ms・p95 < 50ms、新しいプロセスで core を開く < 150ms、正規化 < 500ms / < 3s) と実測を書いた。アーカイブは開くときに検証し、プロセスの中で使い回す形で Q-23 を解決にした (§9.1・R-PF-1・Q-23・§10)
4. `detail` の既定: ツールごとにし (1 つの item は `normal`、一覧は `brief`)、`output.detail` は 1 つの item を返すツールの既定だけを変える (R-OUT-7・§6.4・R-DIST-3)
5. provenance: `list_crates` は provenance を 1 つにする。`source` に `crates.io`・`project`、`resolved_from` に `reexport`、std の `std_from`・`toolchain`、docs.rs の `features`・`all_features`・`cfg`、複数の crate の応答の必須は `target` だけ (R-OUT-9・§7.3・R-FT-1・R-STD-1)
6. `resolve_version` の `docs_rs_json`: R-DS-5 の `rustdoc_json` を実装の名前と値に改めた (R-DS-5・§6.3)
7. M2 に回したもの: 利用者の `net.offline` での `offline_miss`、`cargo metadata`・`cargo tree` の合流と失敗の保持、設定ファイルと知らない設定の知らせ、1 つの crate の照会の 30 秒の上限と `status: building`、暗黙の member の突き合わせ、`build.target`、明示した版と取得元の照合 (R-WS-7・R-VR-2・R-DIST-3・R-ARCH-6・R-WS-6・R-MCP-3・R-TG-1・R-VR-1・§12)
8. ログ: プロセスごとの `<日付>-<PID>.lock` で動いているプロセスを判定し、古い日付のロックのファイルは消す (今日の日付のものは消さない)。削除の規則と所有マーカーを書いた (R-OBS-2・R-CA-4)
9. canary: std のスナップショットの比較をやめ、代表の item が引けることと R-QA-1 の予算で代える (R-QA-4)
10. M4 に回したもの: `also_reexported_at` ごとの条件・`doc(auto_cfg(hide))`・`reexport_of` の条件、モジュールを丸ごと re-export した先の検索の索引と逆引き索引、どの glob で trait がスコープに入るか (R-FT-2・R-SR-1・R-TI-4・R-PR-2・§12)。`source_signature` は添えない (R-OUT-2)
11. 古い index: 取り直しが通信で失敗したら、キャッシュの index を warnings 付きで使う (R-CA-3・R-CA-7)
12. `enabled_in_project` の評価: feature 以外の条件は docs.rs のビルドと同じく成り立つとみなし、docs.rs が `--cfg` で立てた名前と target の違いのときだけ `unknown` にする。v0.18 の「feature 以外の cfg があれば `unknown`」を改めた (R-FT-2)
13. 一覧への feature の注記: `list_impls`・`list_implementors` は impl 自身の条件を付ける。`default_unstable`、存在しない crate の 5 分の否定キャッシュも M1 で入れた (R-FT-2・R-STD-2・R-ERR-3)
14. ネストの深さ: 「上限を設けない」を「実際の crate より桁違いに大きい上限」(初期値 4,000 段) に改めた (R-FV-8・R-FV-3)
15. GitHub の token: 設定 → `GH_TOKEN` → `GITHUB_TOKEN` → gh の `hosts.yml` の順に探し、`api.github.com` にだけ送る。rate limit のときだけ `gh auth token`・`git credential fill` で取る。gh は必須にしない。rate limit に当たったら、token が無ければ取って 1 回だけやり直し、それでも当たれば `x-ratelimit-reset` まで待たずに `retry_after_secs` 付きで返す (v0.18 の「reset まで待つ」を改めた)。`fetch_failed` の `auth` に `gh auth setup-git` の案内を足した (R-NW-4・R-NW-2・R-DIST-3・R-SEC-2・R-SEC-6・§8・R-WS-7)
16. 公開前の見直し: std の古い nightly を 7 日で消す、アーカイブの名前とヘッダの `types_version`、読める nightly へ 30 日までさかのぼる、起動時の `rustc -vV` を待たせない、取得 4・解析 2 の同時実行の上限、同じホストだけのリダイレクト、相対パスのキャッシュのディレクトリ、カーソルの要求のダイジェストにツール名、否定キャッシュに覚えた `unparsable` は `retryable: true`、レビュアーの残りは M2 以降 (R-STD-6・R-CA-5・R-CA-2・R-MCP-11・R-TG-1・R-NW-2・R-SEC-6・R-CA-1・R-OUT-3・§8・§12)
17. 実装に合わせた直し: 解決の順を「公開パス → 親のメンバー → 定義パス」に、impl の分類の順、汎用 trait 以外の blanket impl は 1 行ずつ、`Cargo.lock` も checksum のために常に読む、手元の nightly は host が要求の target と同じときだけ、std の triple はアーカイブの名前から読む、R-OUT-7 の normal から impl の見出しと `docs: none` を外した、§7.3 の warnings の例 (R-PR-1・R-TI-2・R-SH-4・R-VR-2・R-STD-1・R-STD-6・R-OUT-7・§7.3)
18. 新しいエラーと reason: `ambiguous_crate`、`version_mismatch`、`local_build_required` (M2 まで)、`fetch_failed` の `no_repository`・`no_rev`、証明書の検証の失敗、`network_error` の `retry_after_secs`、`busy` の `retry_after_secs` (§8・R-PR-1・R-VR-4・R-VR-2)
19. 細部の追加 (M1 で決めた形・値・順序。「M1 で決める」をすべて置き換えた): 形式番号付きの URL の試し方と 47 (R-DS-2・R-FV-4)、写す属性の範囲 (R-FV-2)、形式ごとのキャッシュ (R-FV-6)、latest の 3 段の探し方と候補 (R-VR-3・R-VR-4)、`cargo metadata` のキャッシュと同名の複数版 (R-VR-2)、ホストの target の代わりの値と `HEAD` (R-TG-1)、docs.rs の feature と `--cfg` (R-FT-1)、cfg の 4 系統の表記と出力 (R-FT-2・R-QA-2)、`cargo tree` の読み方 (R-FT-3)、名前の正規化と lib 名 (R-PR-1)、公開パス索引の上限 (R-PR-3)、行き先の版と中身のたどり方 (R-PR-4)、メンバーの探し方 (R-PR-7)、`list_methods`・`list_impls`・`list_implementors` の形 (R-TI-1・R-TI-2・R-TI-3・R-TI-5)、検索の索引・トークン・順位・打ち切り・範囲 (R-SR-1〜R-SR-5)、`.crate` の取得 (R-SC-2)、std の取得と provenance と stability (R-STD-1〜R-STD-3・R-STD-6)、`project` と workspace のルート (R-MCP-3・§6.4)、instructions (R-MCP-6)、ローカル機能の無い構成 (R-ARCH-3・R-ARCH-4)、`resolve_version`・`crate_overview`・`list_crates` の中身 (§6.3)、`list_module` (R-LS-1・R-LS-3)、`invalid_argument` の使いどころ (R-ARG-1・R-MCP-2)、応答の上限の削り方とカーソル (R-OUT-1・R-OUT-3)、ドキュメントの整形と属性 (R-OUT-6・R-OUT-7)、std の定義位置 (R-OUT-8)、`request_id` の形 (R-ERR-2)、否定キャッシュの鍵と時間 (R-ERR-3)、アーカイブ (R-PF-1・R-PF-2・R-MEM-2)、キャッシュの配置と守り (R-CA-2・R-CA-4)、再試行・重複抑止 (R-NW-2)、target triple の検証 (R-SEC-4)、呼び出しの上限 (R-SEC-7)、キャンセル (R-WS-6)、ログの記録 (R-OBS-1)、予算・CI・canary・検査・eval (R-QA-1・R-QA-3〜R-QA-6・R-DIST-1)、決定性の検査 (R-ARCH-5)、設定は環境変数だけと容量の単位 (R-DIST-3)
20. 未決事項: Q-23 を解決にし、Q-18 にジェネリックな型の答えを足した。Q-22 は M4 以降、Q-27 は gnu 版の答え (musl は M5)、Q-14・Q-15 の現状を書いた (§11)
21. マイルストーン: M1 の到達点と、M2・M4・M5・M6 に回したものを足した (§12)

### v0.18 (2026-10-19)

M0 の後のレビューで利用者が決めた 27 件を反映した版 (M1・M2 への申し送りは [m0-plan.md](m0-plan.md) の §7.2)。

1. docs.rs の target: 2026-05-01 から docs.rs は `targets` の無い crate を既定の target だけでビルドする。求める target の JSON が無ければ、既定のプロジェクトの依存は M2 からローカルビルドし、M1 の間は `docs_unavailable` (`no_target_json`) を返す。別の target の文書で代用しない。live の smoke test で配られる target も見る (R-TG-1・R-TG-2・§5.1・§7.3・§8・R-QA-4・R-WS-10)
2. crate の名前: パッケージ名とコードに書く名前の両方を受け、文脈で解く。path の先頭は lib 名、キャッシュの鍵は正規化した名前 (R-PR-1・§6.4・R-CA-2)
3. build metadata の版: docs.rs には build metadata 込みの版で頼み、返った `crate_version` が違えば使わない (R-VR-1・R-VR-4・R-DS-6・§7.3)
4. feature の組: 文書の組は `compiler-artifact` の `features` を記録し、`enabled_in_project` は全 member を統合した `cargo tree --workspace` の組で判定する (R-FT-3・Q-20・Q-31)
5. M0 のコードの不具合: M1 の計画の最初の段で直す。impl の `for` の確認と決定的な探索 (R-PR-7)、公開パス索引の経路の数の上限 (R-PR-3)、検証してからキャッシュに書く (R-CA-4)
6. crates.io の名前: M1 で使える版ができたら最初の公開をする。それまでは README に、同じ名前の crate を入れないよう書く (R-DIST-1・§12)
7. ローカルビルドの失敗の振り分け: SIGKILL・オフラインの 3 つの文言 (`net.offline` でも)・`revspec`・CLI の `Authentication failed for` を足し、`timeout` は決定的な失敗、キャンセルは記録しない。文言は M2 でテストに固定する (R-WS-7・R-WS-6)
8. `enabled_in_project`: 関連 item の親の cfg を足し、true / false / unknown の 3 値にする (R-FT-2)
9. cargo の設定の合成: 実測を注記し、読み方を M2 で決める (R-WS-2・R-TG-1・R-WS-4・R-VR-2・Q-31)
10. 事実に合わせる直し: `RUSTC_BOOTSTRAP` に cdylib などだけの lib (R-WS-8・Q-12)、`-p` に package id (R-WS-1)、std の私的な依存 (R-PR-4)、版の解決前の provenance (R-OUT-9)、展開の fsync (R-CA-4)、存在しない版の TTL (R-ERR-3)、GitHub の rate limit (R-NW-2・R-RR-1)、host の `rust-std` の判定 (R-TG-1)、正準パスの「最短」(§4)、`doc_auto_cfg` (Q-11・R-WS-1)、同名の crate の並行ビルド (Q-29)、7 日間の従来の方式 (R-MCP-2)
11. eval: M1 から小さく始め、各マイルストーンで足し、M5 で仕上げる (R-QA-6・§12)
12. 対象のクライアント: MCP 2026-07-28 を話すクライアント (主に Claude Code) に狭めた。R-MCP-12 は将来に回した (§1・R-MCP-10・R-MCP-12・§12)
13. worktree: Q-32 を足し、M2 の前に確かめる (R-MCP-3)
14. Claude Code の挙動の検知: eval に `tool_result` の形を確かめるケースを置く (R-OUT-1・R-QA-6)
15. no_std: `#![no_std]` の member では core・alloc のパスで示す。M4 (R-PR-2・R-TI-1・§12)
16. private item のパス: 公開パスが無ければ crate 内でのパスと可視性。自分の workspace の検索は private も含める。M2 (§4・R-PR-2・R-SR-4・§12)
17. vendoring: ソースは `manifest_path` から読み、docs.rs は checksum が一致したときだけ使う (R-SC-2・R-SC-4・R-VR-2・R-DS-1・§5.1)
18. 明示した版と取得元: 解決された版と一致すればプロジェクトの取得元のもの (R-VR-1)
19. std のソース: 合う `rust-src` が無ければ dist から取るのを既定にした (R-SC-2・R-STD-6)
20. `api_diff`: 片側の feature の組でだけ有効な item は Major に使わない。hidden 付きのビルドは両側とも外のツールが作る (R-AD-3・R-AD-4・R-WS-10)
21. 古い手元の nightly: プロジェクトの toolchain の版より古ければ dist の新しいものを使う (R-STD-1・R-STD-6)
22. 制御文字: ソースの本文では見える印に置き換えて warnings で知らせる (R-SEC-7)
23. CPU: cargo に `-j` (初期値はコア数の半分) と設定 `build.jobs`。ロックの取り合いと対処を README に (R-WS-1・R-WS-2・R-DIST-3)
24. 待つ上限: ビルドとロック待ちは `build.wait_secs` (初期値 10 秒) で、1 リクエストの上限 (30 秒) より先に効く (R-WS-6・R-ARCH-6・R-MCP-4・R-MCP-1・R-WS-10・R-SR-2・R-DIST-3)
25. `RUSTUP_TOOLCHAIN`: 子の cargo に渡さず、ログに出す (R-WS-1・R-SEC-2)
26. 注記と申し送り: agent や helper の問い合わせは止まらない (R-WS-6)、版の接尾辞の付く実行ファイル (R-WS-1・Q-31)、全 crate 名の値で item が違いうる (R-WS-8)、全版が pre-release・yanked のときの latest (R-VR-3)、docs.rs の span の絶対パス (R-SC-2)、継承した readme と `README` (R-SC-4)、members の glob (R-VR-2)、換算係数の校正 (R-OBS-1)、コードの申し送り (m0-plan.md の §7.2)
27. 独自の target のプロジェクト (`target_unsupported`) の docs.rs の照会は、1 の例外として docs.rs の既定の target で代用し、実際の triple を provenance と warnings に示す (R-TG-1・R-TG-2)

### v0.17 (2026-10-19)

M0 の結果を反映した版 (結果と根拠は [m0-plan.md](m0-plan.md) の §6、反映の一覧は §7.1、反映した後の見直しと申し送りは §7.2)。

- 名前をすべて `chatora-rustdoc-mcp` にそろえた: パッケージ、lib の crate、バイナリ、MCP の serverInfo、設定・キャッシュ・state のディレクトリ、環境変数の接頭辞 (`CHATORA_RUSTDOC_MCP_`)、README で勧める MCP サーバーの登録名 (R-DIST-1・R-DIST-3・R-DIST-5・R-CA-1・R-OBS-2・R-SEC-2)。v0.4 で `chatora-rustdoc-mcp` から `chatora-rustdoc` に変えた理由 (CLI としても使う) は、v0.14 で照会の CLI をやめて人向けの `cache` と `install-skill` だけになり、弱まった。§14 の過去の版の記述は当時の名前のまま残す
- **M0 で決めたことを反映した**:
  - 未決事項: Q-12・Q-17・Q-20・Q-30 を §11.1 に移した。Q-11 にやり直しの頻度 (136 個中 1 個) と原因を足した
  - ローカルビルド: 既定は stable + `RUSTC_BOOTSTRAP` で、値は解決結果の全 crate 名に固定する (R-WS-8)。`cargo doc` は使わない (R-WS-1)。target が host で `build.target` も無ければ `--target` を付けない (仮のプロジェクトも同じ規則。R-WS-1・R-WS-5)。`+<toolchain>` はサーバーが選んだ toolchain のときだけ付ける (R-WS-1)。dep-info は rustdoc の `--emit=dep-info` で取り、`RUSTC_BOOTSTRAP` の値を鮮度の鍵に入れる (R-WS-4)。feature の組は `cargo tree -e normal --target <t>` (R-FT-3)。resolver 2 以上で `-p` が panic する依存は起動前に判定する (R-WS-5・R-MCP-3)。依存の種類は stable の `cargo tree` で判定する (R-MCP-3)。`rust-std` の有無は sysroot で先に判定し、target の JSON 仕様は v1 で対応しない (R-TG-1)。実行ファイルの一部だけの指定と rustup の無い環境の既定 (R-WS-1)。rustup の無い環境では rustup のコマンドを案内しない (R-TG-1・R-WS-7・R-STD-4)。`cargo metadata` の失敗のまとめ方と、git の失敗の文言・`LC_ALL=C` (R-WS-7)。子プロセスでは GUI の askpass も止める (R-WS-6。M0 で git の CLI が利用者のデスクトップにダイアログを出した)
  - 形式・索引・パス: 先読みの方法 (R-FV-3)、形式 54〜61 の shim (R-FV-2)、docs.rs で確かめた形式に 48・55 を足した (R-FV-4・R-QA-2)、形式 61 でも stable と nightly で属性の表記が違うものがある (R-FT-2)、`pub use … as _` はパスを作らない (R-PR-2)、解決の手順で親も公開パス索引から引く (R-PR-1)、型のメンバーを探す段の順 (R-PR-7)、`html_root_url` の形と、crate をまたぐ re-export には行き先と次の呼び出しの hint を返すこと (R-PR-4)、docs.rs の span は package root からの相対パス (R-PR-5・R-SC-2)
  - MCP と出力: rmcp で受け入れる挙動 (R-MCP-2・R-ARG-1)、stdio ではサーバーから listen を閉じる場面が無い (R-MCP-16)、Claude Code 2.1.303 で確かめたこと (R-OUT-1・R-ERR-2)、`preserve_order` (R-OUT-1)、膨張率の実測値と `macro_rules!` の示し方 (R-OUT-2)、ほかの crate の型の正準パスは M1 から (R-OUT-2)、where 句を `normal` に (R-OUT-7)、`source_location` の形と行のアンカー (R-OUT-8)、trait は impl の見出しで (R-SH-2)、`request_id` は M1 から・候補のキーは `candidates` (R-ERR-2)、docs.rs の 400 と想定外のステータス (§8・R-ERR-3)、404 から `crate_not_found`・`version_not_found` を分けるのは M1 から (R-DS-6・§8)、`log.filter` (R-DIST-3)
  - 技術選定: SDK の比較 (rmcp を続ける)、rustdoc_processor は依存にしない (§10)
  - M0 を完了とした (§12)。`-Z fine-grain-locking` の評価を M2 に回した (R-WS-2)
- **反映した後の見直し** (前提を持たないレビュアー 4 人で並列に読み、食い違いと抜けを直した):
  - 判断したこと: 照会のツールが `build_unsupported` を返す依存は、外のツールが仮のプロジェクトで文書化する (R-WS-5・R-WS-10・§6.3)。`--target` を省くかの判定に使う host は、ビルドに使う toolchain の `rustc -vV` の host とする (R-WS-1・R-TG-1)。`RUSTC_BOOTSTRAP` の値を鮮度の鍵に入れるとき、member 以外のパッケージには部分木に現れる crate 名だけを入れる (R-WS-4)。target の JSON 仕様と build-std は `target_unsupported` とする (R-TG-1・§8)。`full` でも生の rustdoc の部分木を返さない (R-OUT-7)。プラグインの名前は `chatora-rustdoc-mcp` とし、README には手動とプラグインの両方の登録の許可ルールを書き、実在のツール名に当たるかを CI で確かめる (R-DIST-5・R-SEC-2・R-QA-5)。`log.filter` は stderr のログだけを絞り、誤った指定が 1 つでもあれば全体を既定に戻す (設定の表・R-SEC-1)
  - そのまま直したこと: `-p` で名指しできない依存の列挙に dev だけの依存を足した (R-MCP-3)、`RUSTC_BOOTSTRAP` の値にメンバーの bin の名前を入れる (R-WS-8)、子プロセスの環境の例外 (R-SEC-2)、`cargo metadata` の失敗を返すのはローカルビルドの照会だけ (R-VR-2)、`cargo fetch` の失敗は `cargo metadata` の失敗と同じく返し、ビルドの最大時間の超過も `build_failed` にする (R-WS-7・§8)、`build.*_path` の既定 (R-DIST-3)、`enabled_in_dev_only` の求め方 (R-FT-2)、`api_diff` で両側の feature の組が違えば warnings (R-AD-4)、`rust_release` はリリース日が今日以前の版だけを返す (R-RR-1)、provenance の必須の項目は 1 つの crate の応答のもので、ほかは M1 で決める (R-OUT-9)、`kind` の語彙 (R-SH-2)、map を整列する (R-ARCH-5)、beta と `RUSTC_BOOTSTRAP` の先行テスト (R-QA-4)、§9.1 の指標、§10 の JSON の parser・HTTP・SDK の版、M0 の形式の範囲 (§12)、Q-27 を広げた
  - 回したこと: 形式 46〜53 の shim を M1 に (§12)、`name` の無い要求の確認を M1 に (R-ARG-1)、ローカルビルドの細部を M2 に (Q-31 を足した)
- **文書の見直し** (担当を分けた 7 人で、要件・計画・調査文書・README・コメント・一覧ページを読み、誤り・食い違い・重複を直した。参照の誤り (R-DS-6 の R-VR-6、R-SEC-2 の `RUSTUP_AUTO_INSTALL` の置き場所など)、`docs_unavailable` の表を `reason` の一覧に寄せた、§12 の M1 に構造化ログと本文の「M1 で決める」細部の拾い上げを足した、など)。判断したこと:
  - ビルド中の応答の形を R-OUT-9 にそろえた: 古いデータがあれば `stale_reason: rebuilding` と `retry_after_secs`、無ければ `status: building`。キャンセルで止めないビルドの条件にも入れた (R-WS-2・R-WS-6・R-ARCH-6)
  - Cargo.lock が古いときも、docs.rs の照会は直読みで続けて warnings で知らせ、`lockfile_outdated` はローカルビルドの照会と Cargo.lock が無いときに返す (R-VR-2)
  - 複数の crate の応答で欠けた crate の状態と理由は `sources` に載せ、warnings は要約の 1 行にする (R-SR-5)
  - `source_location.file` は package root からの相対パスに固定し、ローカルビルドした crate だけ開ける絶対パスを `path` に足す (R-OUT-8)
  - 「latest の解決結果」の 1h の TTL を消し、sparse index の TTL (10 分) に従わせる (R-CA-3・R-DIST-3)

### v0.16 (2026-10-19)

要件を区切った後の見直し。crates.io の取得の仕方を改め、実装前の仕様として過剰な細部を、原則を残して未決事項と各マイルストーンへ移した。

- **crates.io の取得の仕方** (R-NW-1・R-CA-3・R-CA-4・Q-29・R-SR-6・§5.9・§6.3・R-DIST-3)
  - crates.io への送信の間隔は、プロセスをまたいで揃えるのをやめ、プロセスごとに守ることにした (API の量が少なく、ロックファイルと時計のずれの扱いに見合わないため)
  - crates.io の情報は sparse index → static.crates.io → API の順に取り、API は検索・DL 数・被依存・版ごとのライセンスだけに使うことにした (sparse index にはレートリミットが無い)。`search_crates` は被依存数を返さず、選定の材料を検索の応答と sparse index から作る (結果の件数だけ API を呼んで、その秒数を待たせないため)
  - sparse index のファイルの TTL を 1 時間から 10 分にし、切れたら ETag の条件付きのリクエストで確かめ直すことにした (新しい版に早く気付き、変わっていなければ本文を取り直さないため)
- **実装前の仕様として過剰な細部を外した** (実装や PoC で動くものを見ながら決めれば足りる細部を、未決事項 (Q-12・Q-22・Q-29・Q-30) か各マイルストーンへ移した。移した先の「案」は、移す前に本文で決めていたもの)
  - 未決事項と二重になっていた細部を移した: プロセスをまたぐロックの置き場所・取る順・target dir の共有と排他・枠のファイル、版の違うサーバーの同居の細部、残った子孫の記録と照合と spawn するスレッド、進行中の印の取り方と git の 2 つの鍵、呼び直しのブランチの解決の待ち (以上 Q-29)。`RUSTC_BOOTSTRAP` の値を安定させる方式 (Q-12)。git の取得の失敗の文言と `LC_ALL=C`、`cargo metadata` の失敗の返し方、実行ファイルのパスを一部だけ指定したときと rustup が無いときの既定 (Q-30)。本文には原則 (OS のロックで取る、ロックファイルを消さない、版の違う記録は無いものとして扱う、確かでなければ子孫を止めない、など) を残した (R-CA-4・R-WS-6・R-WS-8・R-WS-10・R-WS-7・R-WS-1)
  - git の URL の scp の形は受けず、`ssh://` での書き方を案内することにした (`~` が通るか確かめていない書き換えをやめ、見分け方の問題も無くした。R-SEC-4・R-WS-10・Q-30)
  - ローカルビルドの同時数は、プロセスごとに数えることにした (複数のセッションで CPU を奪い合っても結果は正しいため。R-SEC-7・R-CA-4・R-DIST-3)
  - 起こりにくい状況の細かい場合分けを、原則と実装時の判断に分けた: 元データが壊れていたとき (R-CA-4)、ホストの target を取れなかったとき (R-TG-1)、`cargo metadata` が失敗して Cargo.lock を直接読むとき (R-VR-2)、形式番号付き URL の再試行 (R-DS-2・R-FV-6)、`[package.metadata.docs.rs]` の組で失敗したとき (R-FT-3・R-WS-5。確認なしにビルドする範囲を広げない原則は R-MCP-3 に残した)、Windows の mmap (R-CA-4)。開いたディレクトリの直下に候補が複数あって `project` が無いときは、std と版を明示した docs.rs の照会だけ続けることにした (R-MCP-3)。配列の `build.target` の既定は先頭の要素にした (R-TG-1・R-MCP-3)
  - 数値と名前と語彙は、初期値・初期案と明記した: 応答の骨格と状態の値、`docs_unavailable` の `reason` (R-OUT-9・§8)、診断の上限 (R-WS-7)、検索の重みと打ち切り (R-SR-3・R-SR-4)、展開の上限 (R-NW-3)、ログ (R-OBS-2)、設定項目の一覧 (凍結は最初の公開リリースで。R-DIST-3)。複数の crate の応答の `sources` の定義が骨格と食い違っていたのを直した
  - 実装の方式を M0〜M2 で測って決めることにした: `format_version` の先読み (R-FV-3)、`cargo metadata` のキャッシュ (R-VR-2)、逆引き索引の構成 (R-TI-4)、`rustc -vV` のキャッシュ (R-WS-8)、メモリの測り方と捨て方 (R-MEM-1)、巨大な crate の扱い (R-MEM-2・Q-22)、外の記録の鍵の組・短いハッシュの照合・`refresh` が解く記録 (R-MCP-3・§6.4)、形式 54 未満の属性の写し方 (R-FV-2)、crate をまたぐ参照の版の決め方の順 (R-PR-4)、描画できない形の `source_signature` (R-OUT-2)
  - v1 に要らない機能の細部を縮めた: HTTP (R-MCP-17・R-SEC-5・§7.3)、MRTR (R-SEC-2)、resources・prompts・completions・icons・`tools.profile`・`output.mode`・`max_chars` (R-MCP-13・R-MCP-18・R-MCP-19・R-MCP-7・R-MCP-12・R-OUT-1・§6.4。§8 の表の `resources/read` の列を消した)、隔離環境 (R-CA-8)、配布の流れ (R-DIST-1)、対応リリースの目標 (R-QA-4 から削った)、換算係数の校正 (R-OBS-1)、次のプロトコルの版との併存 (R-MCP-2)、cfg のソース解析 (R-FT-2)、`api_diff` の分類器と応答の並び (R-AD-2・R-AD-5)、rust-src の dist からの取得 (R-SC-2)、散文とシグネチャの検索 (R-SR-1)、nightly の選び方 (R-STD-6)、§5.9 metadata の取得元の振り分け
  - 入力の形のうち、trait 修飾のパス・docs.rs の URL・区別子を任意にした (R-PR-1)。docs.rs のビルド時の依存の版の差の警告を M4 の任意にした (R-PR-5)。`sealed` は検出できたときだけ出すことにした (R-TI-5)。キャッシュの trait 化を先送りした (R-ARCH-7。R-ARCH-4 は原則だけにした)
  - R-SEC-2 の「README に書く」を要点の一覧にし、権限モードごとの ask の効き方の実測は [claude-code-issues.md](claude-code-issues.md) に寄せた
- **2 回目の洗い出し** (上の反映で生じた食い違いを直し、残っていた過剰な細部を外した)
  - 食い違いを直した: R-CA-8 (M5) を前提にしていた記述を「実装した場合」と条件付きにした (設定表、R-WS-7、R-STD-6、R-RR-1、環境変数の例)。M1 のキャッシュは原子的な書き込みだけで重複取得を許し、プロセスをまたぐロックは M2 からにした。GC の最小限 (`cache gc` と容量の上限) を M2 に入れた。M5 の完了条件を確かめる項目の列挙にした。使えない target dir は別の名前で作り直すことを R-WS-2 に書いた。Q-2 を R-MCP-2 に合わせた。R-MCP-5 から resources・prompts のキャッシュの細部を外した。ディレクトリが消えたときの §8 の記述を M2 に送った。R-ERR-2 から `stage` を外した。R-SR-1 の `fields` から `signature` を外した。数値の初期値の印をそろえた (R-DS-4・R-SC-2・R-SEC-7)。任意の機能の設定キーと `install-skill` に印を付けた。R-PR-5 の確認を M4 にした。M5 の MCP レジストリを「登録するかを決める」にした。`RUSTC_BOOTSTRAP` の値を鍵に入れるかを Q-12 に寄せた。Windows で派生物を作り直すときの名前を Q-29 に足した
  - 細部を実装時や各マイルストーンに移した: §9.1 の性能目標を目安にし、値は M1・M4 で測って決める。カーソルは原則だけにした (R-OUT-3。束縛の規則と「無効にしない」の食い違いも消えた)。SessionStart hook は原則だけにした (R-DIST-5・R-DIST-3)。検索の順位付けの構造を初期案にした (R-SR-3)。`--cfg docsrs` の失敗の記録と `rustdoc-args` の取り込みを M2 に (R-WS-1)。形式の調べ方と toolchain の自動の切り替えを M2 に (R-WS-8)。metadata の項目を初期案にし、被依存の一覧を任意にした (§5.9)。照会のツールでの git の解決の選び方と古さの時間を M2 に (R-MCP-3・§6.4・R-DIST-3)。GC の数値を初期値にした (R-CA-5)。プラグインの細部を M5 に (R-DIST-5)。設定の書き方を 1 文に縮めた (R-DIST-3)。一時ファイルとログの後始末の方式を実装時に (R-CA-4・R-OBS-2)。HTTP の認証と扱えるプロジェクトを M6 に戻した (R-SEC-5)。skill の単独インストールを 1 文に (R-DIST-6)。Markdown の表示規則を text モードに移した (R-OUT-5・R-SH-7)。HTTP のキャンセルと tasks を M6 に (R-WS-6)。他の target の候補の計算を任意にした (R-WS-10)。`search_source` は守りだけにした (R-SC-6)
  - 前回決めた件 (残った子孫の後始末、`fetch_failed`、`cargo_lock_raw`、形式番号付き URL、member の docs.rs の組) は蒸し返さなかった。R-CA-6 の先取りと R-VR-2 の `lockfile_outdated` の判定は今のままにした
- **文書の見直し** (間違い・矛盾・重複・あいまいさ・過剰さを、意味を変えずに直した。決定の中身は次の 6 件を除いて変えていない)
  - R-WS-5 と R-MCP-3 で resolver 2 の panic の記述が食い違っていたので、両方から M0 で確かめ直すことにし、Q-30 に足した
  - 上の反映の取り残しを直した: Q-11 を R-WS-1 に合わせ、R-NW-2 を一時的エラーの定義にし (R-DS-2 の手順 4 を消したため)、§0.1 の説明文の長さを §6.2 に合わせ、§12 に本文が M0・M4・M5 に送った項目を足した
  - `scope_unknown` を §8 の `reason` の列挙に入れた。R-SC-4 の「パッケージの中身」に build script が入ること、R-CA-5 の「外のツールの記録」が何を指すかを書いた
  - 決めたこと: R-TI-1 は固有と trait 由来を M1、Deref 先とプリミティブを M4 にした。R-NW-2 の対象に crates.io の API と sparse index を足した。R-SEC-2 の守るものの列挙に R-SEC-6 を足した。R-WS-1 から Q-30 の `--target` の問いを指した。R-WS-10 に、既定のプロジェクトを `target` なしで指したときの行を足し、metadata を記録する場合をはっきりさせた。v0.15 の「3 回目のレビュー」の節の名前を「3・4 回目」にした

### v0.15 (2026-10-16)

未決だった仕様を決め、実測で前提を正し、rust-analyzer と cargo より厳しかった守りを緩めた。

- **撤去の取り残しの整理**
  - 撤去で役目を失った記述を整理した (環境変数の配列の例、URL の形の制限と重なる `ext::` の記述、確認画面を指していた拒否の理由、CLI の名前の区別の検査。付録 B は、照会の CLI の行から境界に触れた理由を消し、elicitation の行を全モードの 1 行にまとめた)
  - 元データの JSON も、壊れていたら (内容ダイジェストで判定) サーバーが捨てて 1 回だけ取り直すことにし、`internal_error` の suggestions から `cache clear` を外した (CLI はエージェントに案内しない決まりと食い違い、サーバーが別のマシンにあると効かないため。R-CA-4・§8)
- **実測で確かめた事実** (Claude Code 2.1.301、rust のソース、docs.rs の実データ)
  - 確かめた事実を反映した: 1.97・1.98 の JSON の形式、Q-13 (解決)、#79944 が 2.1.301 でも続くこと、エラーの結果は text のブロックをつないで渡すこと (R-ERR-2)、CLI で `readOnlyHint` が並列の実行に効くこと (R-MCP-4)
  - ask のルールが端末の Manual・`acceptEdits`・auto・plan では効き、端末の `bypassPermissions` と VS Code の auto では効かずに外のツールが確認なしで動くことを確かめ、README の案内を直した。後者はどちらも文書と食い違う不具合 (R-SEC-2)
  - 追っている Claude Code の Issue と、直ったときに直すところを [claude-code-issues.md](claude-code-issues.md) にまとめた
  - `RUSTC_BOOTSTRAP` の値を固定すれば、member を切り替えても proc-macro2 などが作り直されないこと、bin の crate 名も値に要ることを確かめた (R-WS-8)。git の取得の失敗の文言を libgit2 と git の CLI で集め、GitHub の https では無いリポジトリも認証の失敗になることを確かめた (R-WS-7・§8)
  - cargo は他の target でだけ使う依存も `-p` で名指しされれば文書化する (build.rs も動く) ので、範囲の判定をサーバーが行うことにした (方法は下の 3・4 回目のレビューで `cargo tree` に改めた)。`--target` を指した他の target での文書化と、`ssh://host/~/path` を cargo が受けることも確かめた (R-MCP-3・R-WS-10・R-SEC-4)
- **未決だった仕様の決定** (今の情報で決められるものを洗い出して決めた)
  - `build_external_docs` は、待ち時間を超えたら `building` を返してビルドを裏で続け、続きは照会のツールで待つことにした。照会のツールは進行中の印を見て `status: building` を返す (呼び直しのたびに ask の確認が出ないように。R-WS-10・R-MCP-3)
  - 確認なしにビルドする範囲の target を、ホストの target から実際のビルド target (`build.target`、無ければホスト) に改め、照会のツールに違う `target` が渡されたらローカルビルドしないことにした (利用者の `cargo check` と範囲を合わせ、`target` 引数の抜け道を閉じるため。R-MCP-3)
  - 他の target 向けの依存は、外のツールで `target` を必須にし、docs.rs に無ければ既定のプロジェクトの workspace でその target を指してビルドすることにした (ホストの target でビルドして cfg で消えた文書を作らないため。R-WS-10)
  - git の取得の認証の失敗・リポジトリが無い・rev が無いを、cargo の文言で判定して決定的な失敗 `fetch_failed` にした (一時的な `network_error` として再試行を繰り返させないため。R-WS-7・§8)
  - git の引数を cargo と同じ `rev`・`branch`・`tag` に分け、外のツールは解決したコミットを鍵として返し、照会のツールはブランチやタグでも最も新しい解決結果を引けるようにした (TTL を過ぎたら `stale`)。git の URL は cargo と同じ規則で正規化して鍵にする (§6.4・R-WS-10・R-MCP-3)
  - 外の workspace を指したときは、既定で全 member の lib と docs.rs で引けない依存をビルドし、呼び直しは取り直しとした。記録は target dir と同じ GC の枠に入れ、照会で読んでも最後に使った日時を更新する。壊れた外の記録は外のツールで作り直す (R-WS-10)
  - 元データは書き込み時の内容ダイジェストで壊れたかを見分け、壊れたときだけ取り直すことにした。正しいデータの解析の失敗は (ダイジェスト, サーバーの版) を鍵に否定キャッシュし、ローカルビルドの再失敗は決定的な失敗にする (R-CA-4)
  - 既定のプロジェクトの候補が複数のとき、std の照会は続け、docs.rs の照会は、その crate を持つ候補で版が 1 つに決まるか、どの候補にも無いときに続けることにした。直下の子は起動時に一度だけ数える (R-MCP-3)
  - `build_failed` に付ける診断を、既定 2 件・1 件 3,500 文字・合計 8,000 文字以内に決めた (エラーの text の 1 万文字の上限から。4KB 単位の書き方をやめた)。JSON の診断が無い失敗には stderr の末尾を付ける (R-WS-7)
  - ホストの target を取る `rustc -vV` が中立のディレクトリで失敗したら、既定のプロジェクトのルート、最後にサーバーのビルド時の target (musl は gnu に読み替え) の順に試すことにした (R-TG-1)
  - 照会のツールは、外の記録を (プロジェクトのルートか crate・git の鍵、target) で引き、鮮度はファイルの内容ダイジェストだけで判定する。toolchain の更新は判定せず、記録時の toolchain を provenance に出す (R-MCP-3)
  - `enabled_in_project` は通常のビルドの組で判定し、dev の経路でだけ有効なものは `enabled_in_dev_only` で示すことにした (R-FT-2。Q-20 は組の得方だけが残る)
  - `RUSTC_BOOTSTRAP` の値を、プロジェクトの target dir では全 member の crate 名の固定の一覧にし、依存のローカルの文書化は依存用の target dir 1 つで、文書化した依存の crate 名を足していく一覧にした (文書化する crate を切り替えるたびの作り直しを避けるため。R-WS-8)
  - 仮のプロジェクトでも `[package.metadata.docs.rs]` の組が壊れたら default でやり直すことにし、外のツールの `features` 引数は v1 では持たせない。`document_hidden_items` のときは docs.rs に JSON があってもビルドする (R-WS-5・R-WS-10)
  - annotations の分類を確定した: `readOnlyHint: true` は `search_crates`・`rust_release` だけ (`crate_metadata` などは版の解決で `cargo metadata` を起動する)。照会のツールが CLI で順に実行されることは受け入れた (R-MCP-4)
  - 1 リクエストの時間の上限 (30 秒)、changelog の件数 (応答の上限まで、最大 10)、resources の `ttlMs` (プロジェクトに依存するものは 60 秒) を決め、`build_external_docs` の名前を確定した (R-ARCH-6・§5.9・R-MCP-13・§6.3)
  - 設定のキー名を決め、設定項目の表に載せた。取得元の無効化を取得元ごとの真偽値にし、環境変数での配列の書き方の規則を消した。GitHub token は `GITHUB_TOKEN` を読まない (R-DIST-3・R-NW-4)
  - 他の target の文書: 照会のツールに違う `target` を渡したら docs.rs → 外のツールの記録の順に探し、std は配布物から取る。外のツールは既定のプロジェクトの member と依存も他の target で文書化できるようにした。`build.target` が配列なら先頭を既定にする (R-MCP-3・R-WS-10・R-TG-1)
  - 進行中の印を、全 OS 共通の OS のファイルロック (`flock`・`LockFileEx`) にした (R-WS-6 の記録は Linux・macOS だけで、Windows では印が無かったため)。残った子孫を見分ける規則を、グループの長の起動時刻以降に起動したものに直した (R-WS-10・R-WS-6)
- **v0.15 のレビューを受けた修正** (R-WS-6・R-WS-5・R-WS-7・R-WS-8・R-WS-10・R-MCP-3・R-MCP-4・R-MCP-13・R-CA-4・R-SEC-4・R-NW-2・R-NW-4・R-OUT-9・R-DIST-3・R-FT-2・R-TG-1・§6.4・§8)
  - 一度 `building` を返したビルドは、キャンセルでは止めない
  - 仮のプロジェクトでも `--target` を明示し、プロジェクトのルートは実際のパスで比べる
  - git の URL の正規化を cargo の `CanonicalUrl` に合わせ、scheme は書き換えずに鍵に含める。scp の形は `ssh://host/~/path` に直す。`file://` は受けない理由を書き、userinfo の拒否を `git://` にも広げた。`refs/…` の rev にも TTL を付け、短いハッシュは前方一致で照合する
  - 取り直しても壊れていた元データは一時的な失敗として否定キャッシュに入れ、取得したデータはダイジェストの記録の前に検証する。作り直しの失敗は R-WS-7 の分類に従う
  - `fetch_failed` の判定のために git を取得しうる子プロセスに `LC_ALL=C` を渡し、`cargo metadata` の失敗も振り分ける。外のツールの git の認証の案内を `git ls-remote` にした
  - `RUSTC_BOOTSTRAP` の値に lib・bin・proc-macro の crate 名を入れ、環境にある値は上書きする
  - 複数の crate にまたがる応答の provenance を `sources` に分け、`partial` と `building` の使い分けを決めた
  - `build.wait_secs` を消して `limits.request_timeout_secs` に統一し、`build.rustc_path` を足した。診断の上限はエスケープ後の長さで数える。容量の単位、受信のタイムアウト (無通信の時間)、固定の TTL を明記した
  - `build_external_docs` を `idempotentHint: false` にした。GitHub API の URL を変えたときは、token と URL の両方を設定ファイルに書いた場合だけ token を送る (下の「判断待ちだったものを決めた」で、URL が環境変数で変えられていたら送らない、に改めた)。知らない設定の名前は warnings に出す
  - プロジェクトのディレクトリが消えたときのエラー、外のツールの呼び直しで失敗のキャッシュを解くこと、外の記録の鮮度の照合範囲、`rustc -vV` をやり直すルート、`resources/read` のエラーコードの表 (§8)、一覧の `enabled_in_dev_only` を決めた
- **v0.15 の 3・4 回目のレビューを受けた修正** (R-MCP-3・R-WS-1・R-WS-6・R-WS-8・R-WS-10・Q-29)
  - 範囲の判定を `cargo tree --target … -e normal,build,dev` に改めた (`--filter-platform` は build-dependencies の cfg を build target で評価してしまうため)。判定できないときは依存をローカルビルドしない
  - `cargo rustdoc` には常に `--target` を渡す (配列の `build.target` で失敗するため)
  - 残った子孫は、確かでなければ止めず、新しい target dir に切り替える
  - 依存用の target dir では build-dir も分ける
  - 既定のプロジェクトの依存を他の target で作った記録の鍵に、プロジェクトのルートと package id を入れる
  - git の進行中の印を、要求したブランチと解決したコミットの両方の鍵で持つ
  - プロセスをまたぐ仕組みの細部は Q-29 にまとめて M2 で決めることにした
  - 判断待ちだったものを決めた: 外のツールの呼び直しは、どの対象でも失敗のキャッシュを解き、進行中のビルドには合流する。依存でない `crate`・`git` も他の target で文書化でき、仮のプロジェクトの `--target` の値を決めた。GitHub token は URL が環境変数で変えられていたら送らない (この制限は下でやめた)。`cargo metadata` の失敗は `cargo fetch --locked` で振り分ける。外の workspace の既定のビルドに docs.rs に JSON が無い依存を入れる。初回の取得で壊れていた docs.rs の JSON は「確定的に存在しない」にする。消えたプロジェクトは古い記録を `stale` で返す。§8 に `retryable` の列と `docs_unavailable` の `reason` を足し、`sources`・`status`・`building_reason`・`stale_reason` の値とカーソルの扱いを決めた。`refresh` を cargo を起動しうるツールすべてに広げた。実行ファイルのパスを指定したときの toolchain の扱い、知らない設定の warnings の出し方、展開の上限、scp の形の絶対パス、範囲の判定の罠のテストを決めた (R-WS-10・R-WS-5・R-NW-4・R-WS-7・R-CA-4・R-MCP-3・R-OUT-9・§6.4・§8・R-WS-1・R-DIST-3・R-SC-2・R-SEC-4・R-QA-5)
  - `cargo tree` の範囲が、dev-dependency・proc-macro・target 別の依存まで `cargo check --all-targets` と一致することを確かめた (R-MCP-3)
  - 文書化する crate 自身の dep-info は rustdoc の `--emit=dep-info` で取れるが JSON と同時には出せないこと (R-WS-4)、通常のビルドの feature の組を `cargo tree -e normal` で得られること (Q-20)、nightly の `cargo doc` が JSON を出すこと (Q-17) を確かめた
  - `cargo tree` も、`cargo metadata` と同じくプロジェクトの `build.rustc`・`build.rustc-wrapper` を動かすことを確かめ、README の注意に足した (R-SEC-2)
  - GitHub token の送り先を URL の出どころで制限する規則をやめた (設定ファイルの場所を変える環境変数で迂回でき、信頼したリポジトリは hook で token を読めるため。R-NW-4)
  - 照会のツールがローカルビルドしてよいのを、cargo がその target のために作る (package, target) の組 (target 用の依存はその target、build-dependency と proc-macro は host) に限り、種類を判定できなければビルドしないことにした。member の docs.rs の feature の組で依存が増えるなら通常の組で文書化する。「他の target 用の依存もホストの target のまま文書化する」という前の記述の誤り (試した依存が member だった) を直した (R-MCP-3・R-FT-3)
  - 4 回目のレビューの文面の取り残しを直した (git の記録とロックの鍵に crate、記録の鍵に `document_hidden_items`、proc-macro の出力先、resolver 2 の panic と案内、`corrupt`・`unparsable` の定義、R-ERR-2 の任意の項目、罠のテストの組み方)。cargo の挙動を試してから決めるものを Q-30 にまとめた
  - 4 回目のレビューの意味の決め事をまとめて決めた: 外のツールの合流と呼び直し、`scope_unknown`、dep-info の取り方は M0、dist からの std と取得元の無効化の関係、展開の上限を超えたときの扱い、裏のビルドの失敗を照会の鍵で記録すること、`cargo metadata` の失敗は一時的な失敗として固定しない、依存でない crate の既定の target は host、ダイジェストはビルドの前に取る、`refresh` が解く記録、metadata が失敗しても docs.rs の照会は Cargo.lock で続ける、配列の `build.target` の既定、実行ファイルのパスは全部か無しか、外のツールの入力の表、知らない設定は応答に出さない、応答の骨格、カーソルの束縛、外の記録の形、外の workspace の範囲、`rust-std` の判定、候補が複数のときの文脈、容量の書き方 (R-WS-10・R-WS-7・R-WS-5・R-WS-4・R-WS-1・R-VR-2・R-TG-1・R-MCP-3・R-OUT-3・R-OUT-9・R-SC-2・§6.4・R-DIST-3・R-SR-2)
- **複数のサーバーが同時に動く場合の穴を塞いだ** (stdio を複数、HTTP を複数、両方を同時に起動する場合。R-NW-1・R-SEC-7・R-CA-4・R-WS-6・R-MCP-3)
  - crates.io への毎秒 1 リクエストの間隔と、ローカルビルドの同時数の上限を、プロセスをまたいで守る (ロックファイルで。v0.16 で、どちらもプロセスごとに戻した)
  - 取得とビルドは、ロックを取ってからキャッシュを確かめ直す
  - 残った子孫の後始末で target dir を作り直すのは、そのロックを取れたときだけにし、読めない記録には触らない (作り直しは、3・4 回目のレビューで新しい target dir への切り替えに改めた)
  - HTTP モードでは、直下の子をそのプロジェクトを初めて受けた時点で数える
  - 版の違うサーバーが同時に動くときに備え、サーバーが書く記録すべてに形式の版を入れ、違う版の記録は無いものとして扱うことにした (R-CA-4)
  - プロセスをまたぐロックの原則を決めた: OS のロックで取る、ロックファイルは state の `locks/` に置いて GC と `cache clear` で消さない、取る順は「鍵 → target dir → ビルドの枠」、枠の数が違うプロセスが同居したら最も大きい値が実際の上限 (R-CA-4)。v0.16 で、OS のロックで取ることとロックファイルを消さないことだけを本文に残し、ほかは Q-29 に移した
- **rust-analyzer・cargo より厳しかった守りを緩めた** (R-SEC-2 の考え方に合わせた)
  - git の URL で `http://`・`git://` も受けることにし、平文の取得は warnings と provenance に出す (Cargo.toml の git 依存なら cargo も rust-analyzer も取得するので、このサーバーでだけ止めない。R-SEC-4)
  - キャッシュ・state・ログのディレクトリの 0700・所有者の確認・権限の修正をやめ、cargo と同じく umask のまま作ることにした。他の利用者からの守りはこの置き場所に任せ、他の利用者が読み書きできる場所に置かないことを README で案内する (R-CA-1・R-SEC-2)

### v0.14 (2026-10-16)

CLI の役割と、守りの範囲を見直した。基準は「同じことが他の経路 (エージェントの Bash、rust-analyzer、利用者自身の cargo) で同じくらい簡単にできるなら、このサーバーでだけ止めない」。

- **ビルドの確認をすべてやめた** (R-SEC-2・R-MCP-3・R-MCP-4・R-MCP-13・R-SEC-4・R-SEC-5・R-SEC-6・R-SC-4・R-WS-2・R-VR-2・R-DIST-3・§5.1・Q-21。Q-28 は消した)
  - stdio の MCP サーバーは起動した相手を信頼する作りで、エージェントは Bash で cargo を実行できる。プロジェクトの Cargo.toml に crates.io の crate・任意のホストの git の依存・任意のディレクトリの path 依存を書けば、rust-analyzer がそれを取得して build.rs を確認なしに動かす (path 依存と `file://` の git 依存で確かめ、ほかは推定。rust-analyzer 0.3.3081)
  - やめたもの: ローカルビルドの境界と、境界の外で cargo を起動しない扱い (Cargo.lock の直接読み)、確認 (プロジェクトの信頼・新しい依存・依存でない crate と Cargo.lock に無い版・git のホスト)、elicitation と MRTR、確認の記録、CLI の `approve` / `revoke`、信頼するプロジェクトの一覧・ビルドを許す crate の一覧・git のホストの一覧・自動承認の設定、エラーコード `not_allowed`、HTTP モードでプロジェクトを限る規則、仮のプロジェクトの祖先の設定の検査、git の URL の `ext::` と `-` の拒否、子プロセスの環境変数の denylist、`openat2` による判定と開く間の差し替えへの対策
  - 残したもの: エージェントの意図によらない脅威 (第三者のデータ、ネットワーク、同じマシンの他の利用者、事故) への対策 (ソースルートの判定、アーカイブの検証と上限、キャッシュの整合性、出力のサニタイズ、資格情報入りの URL の拒否、HTTP モードの `Origin` と `Host` の検査)。MRTR を将来使う場合の `requestState` の扱いは、仕様どおりにすると明記した
  - README には、指した場所のコードと設定が確認なしにサンドボックスの外で動くこと (信頼できないリポジトリで Claude Code を開いて照会するだけでも、`cargo metadata` と `rustc -vV` が設定の実行ファイルを動かしうること) と、MCP のツールを許可する意味を書く
- **v0.14 のレビューを受けた修正** (R-MCP-3・§6.3・§6.4・§5.1・R-WS-5・R-SEC-4・R-SEC-5・R-SEC-7・R-SC-2・R-SC-4・R-WS-4・R-WS-7・R-CA-1・R-OBS-2・R-DIST-3・R-MCP-13・§10・§12)
  - git の URL を `https://` と `ssh://` に限る規則は残した (平文の通信でコードを差し替えられるため)。資格情報は userinfo の形を問わず拒否する
  - 依存のローカルビルドの鍵に、source id、実効の cargo の設定、cargo と rustdoc の実行ファイルのパスを入れた。`registry/src` は source id のディレクトリだけを使い、manifest のパスと span はその crate 自身のソースルートとだけ照合する (第三者のデータが共有のキャッシュや他のプロジェクトに効かないように)
  - キャッシュ・state・ログのディレクトリを 0700 で作り、所有者を確かめる。ログは state の下に置き、所有マーカーを置く
  - 出力のサニタイズで、双方向の制御文字とタグ文字も除き、対象を第三者の文字列すべてに広げた。アーカイブの展開の規則を、std の tar.xz と OSV の zip にも適用する
  - プロジェクト単位の設定ファイル (`.config/chatora-rustdoc.toml`・`.local.toml`) をやめ、設定は利用者の設定ファイルと環境変数だけにした。ローカルビルドの feature の組と rustdoc の `--cfg` は、workspace member の `[package.metadata.docs.rs]` から読む (R-DIST-3・R-FT-3・R-WS-1・Q-24)
  - symlink の守りの理由を、第三者のリポジトリに入った symlink に改めた。`include_str!` で外のファイルが文書に入ることも README に書く
  - rustup による toolchain の自動導入を止める規則と設定をやめた。自動導入はプロジェクトの `rust-toolchain.toml` の toolchain にだけ許し、オフラインモードでは止める (R-WS-7・R-DIST-3)
  - HTTP モードの最初の段階では認証 (トークン) を設けず、将来の予定にした (R-SEC-5・§12)。Unix ドメインソケットでの待ち受けも検討したが、Claude Code の MCP クライアントが HTTP のサーバーにソケットでつなげず (文書で確認)、使うには形式を変換する中継が要るのでやめた。仮のプロジェクトの祖先の設定の検査は設けず、キャッシュを `/tmp` のような共有の場所の下に置かないよう README で案内する (R-WS-2)
  - 外のコードを引き込む操作 (依存でない crate と Cargo.lock に無い版のローカルビルド、引数 `git`、既定のプロジェクトの外の `project`) を、別のツール `build_external_docs` に分けた。照会のツールが cargo を起動しビルドするのは、既定のプロジェクトだけにした。README で、allow に `*`、ask に外のツールを書く許可ルールを案内する (許可ルールは MCP のツールを引数で分けられず、`requiresUserInteraction` は headless で使えず外せないため。どちらも 2026-10-16 に確認)
  - 外のツールと照会のツールの決めごとを足した (R-WS-10・R-MCP-3): 照会のツールは外で cargo・rustc・rustup を起動せず、`build_external_docs` が記録した metadata とダイジェストだけを使う。git の crate も照会で指せる。鮮度が切れた外の記録は `stale` で返す。`[package.metadata.docs.rs]` の組で壊れたら通常の組でやり直す (R-FT-3)。サニタイズ・鍵・URL の検証を広げた
  - ask のルールは `--allowedTools`・`bypassPermissions`・`dontAsk`・hook の `allow` より優先すること (headless でのこと。対話の端末の `bypassPermissions` と VS Code の auto では効かないと v0.15 で分かった) と、Claude Code の設定の `env` が MCP サーバーにも渡ることを確かめた。後者は README の残るリスクに書く (R-SEC-2・R-DIST-3)
  - 照会のツールが確認なしにビルドする依存を、このホストの target で `cargo check --all-targets` がビルドするもの (rust-analyzer の既定と同じ範囲) に限った。他の target 向けの依存は外のツールで作る (R-MCP-3・R-WS-10)。§4 に「既定のプロジェクト」「照会のツール / 外のツール」を足し、外のツールの範囲の列挙を R-WS-10 に寄せた
  - Claude Code を開いたディレクトリに Cargo.toml が無ければ、直下の子の workspace を既定のプロジェクトとして扱う (rust-analyzer と同じ範囲。R-MCP-3)
  - ソースの読み取りを、パッケージの中身 (`.rs`・dep-info の入力・`Cargo.toml`・manifest の readme と example、依存は `.crate` の中身) に限った。ドットファイルは除き、readme は文書の拡張子だけを受ける (R-SC-4・R-SC-6。Read の deny をすり抜けさせないため)
  - 取り残しを直した: M6 の完了条件、R-CA-1 の承認の記録、R-STD-3 の cargo を起動しないプロジェクト、§10 の rmcp の feature、`resources/read` のエラーの対応づけ、仮のプロジェクトの `cargo fetch --locked`
- **CLI の縮小** (R-DIST-2・R-CA-5・R-CA-6・R-CA-8・R-SR-3・付録 B)
  - 照会の `get` / `search`、公開 API の書き出しの `api`、先取りの `prefetch`、キャッシュの書き出しと取り込みの `cache export` / `import` をやめた。残すのは `cache ls` / `gc` / `clear` と `install-skill`
  - CLI の境界の規則 (`CLAUDE_CODE_CHILD_SESSION` による判定など)、`--config`、CLI の出力形式 (Q-26)、検索の前方一致の段を消した
  - `--refresh` をやめ、キャッシュの取り直しは `cache clear <crate@版>` で行う。ビルドの失敗の固定を解くのはツールの引数 `refresh` だけ
  - 起動時の先取りと、Cargo.lock の差分の先取りは、サーバーの設定として残した
- **調べたこと** (確認をやめる前に行った)
  - Claude Code 2.1.301 の MRTR の確認: 端末版は確認画面を出し、`accept` / `decline` を再要求で返す。headless は即座に `cancel` を、VS Code 拡張は画面を出さずに即座に `decline` を返す (権限モードによらない。既知の不具合 anthropics/claude-code#98256)
  - Claude Code は MCP サーバーに `CLAUDE_CODE_CHILD_SESSION` を付けない (Bash から起動した `claude` でも)。メッセージの受信口の `CLAUDE_CODE_MESSAGING_SOCKET`・`CLAUDE_CODE_MESSAGING_TOKEN` は MCP サーバーにも渡る
  - rust-analyzer は、開いたフォルダの直下に Cargo.toml があれば、その下に clone したリポジトリを読み込まない (直下に無いときだけ直下の子を読む)

### v0.13 (2026-10-15)

v0.12 のレビューで残った判断待ちの事項を 1 つずつ決め、決めた内容をもう一度見直した。

- **ローカルビルドと cargo の起動の境界** (R-MCP-3・R-VR-2・R-TG-1・R-SEC-5・R-DIST-3)
  - プロジェクトの単位を Cargo の workspace にした (member のディレクトリで起動しても workspace の全体が既定のプロジェクトになる)
  - cargo・rustc・rustup を起動するのは、既定のプロジェクト、信頼するプロジェクト、確認の記録があるプロジェクトだけにした (`cargo metadata` や `rustc -vV` も、`rust-toolchain.toml` の `path` や `.cargo/config.toml` の `build.rustc` / `rustc-wrapper` で任意の実行ファイルを動かすため)。それ以外では版を Cargo.lock から直接読み、`resolved_from: "cargo_lock_raw"` で区別する。ホストの target は中立のディレクトリで調べる。一度信頼したプロジェクトは信頼し続け、やめるのは `approve --revoke` だけ
  - 境界の外のプロジェクトでローカルビルドが要るときは、`not_allowed` にせず、確認画面でプロジェクトの信頼を新しい依存の確認と一緒に尋ねる。`not_allowed` と cargo を起動しなかったときの suggestions・hints は、プロジェクトのパス入りの利用者への案内にし、エージェントに `approve` を勧めない
  - 信頼するプロジェクトの一覧の書き方を決めた (絶対パスと `~/` だけ、1 要素 1 プロジェクト。相対パス・環境変数の展開・glob は受けない)。書けるのは既定の場所の利用者の設定ファイルだけで、プロジェクト単位の設定・環境変数・CLI の `--config`・`CHATORA_RUSTDOC_CONFIG` で指した設定ファイルでは変えられない
  - 信頼するプロジェクトの一覧を読む設定ファイルと state のディレクトリの場所は、ほかの設定と同じく XDG などの環境変数から決める (OS のユーザー情報から決める案は、コンテナで引けないのでやめた。CLI に環境変数を付けて偽の一覧や承認の記録を読ませる経路は、README に残るリスクとして書く)
  - ツールの引数 `git` / `rev` で git リポジトリの crate を引けるようにした。仮のプロジェクトの git 依存として、依存でない crate と同じ確認の規則でビルドする。URL は `https://` と `ssh://` だけを受ける
  - CLI は人と CI のスクリプトのためのものにした。エージェントの環境変数 `CLAUDE_CODE_CHILD_SESSION` が無ければ境界を適用しない (端末かどうかは見ない。`CLAUDECODE` は IDE の統合ターミナルにも付くので使わない)。あるときは信頼を作らない (既定のプロジェクトを持たず、プロジェクト単位の設定を読まず、記録を作らず、信頼済みか確認済みのものだけビルドする。ミラーの環境変数と `--config` も無視する)。skill からの CLI の利用と、`CLAUDECODE` による出力形式の切り替えをやめ、skill は MCP のツールを使う
- **新しい依存の確認** (R-SEC-2・R-WS-5・R-DIST-2)
  - 既定のプロジェクトと信頼するプロジェクトでは、初めて使うときの Cargo.lock のパッケージを確認なしに記録し、以後に増えたものだけを確認する。member と workspace の中の path 依存は確認の対象から外す
  - 確認の一覧を、`cargo metadata` を起動できるプロジェクトでは build.rs と proc-macro を持つパッケージに絞る
  - 依存でない crate と Cargo.lock に無い版のビルドは、ビルドを許す crate の一覧かその場の確認で許可する (「既定では無効」から変更)。仮のプロジェクトの依存のグラフに build.rs と proc-macro が無ければ確認しない
  - 信頼したプロジェクトでは、設定ファイル (`rust-toolchain.toml`・`.cargo/config.toml`・プロジェクト単位の設定) の書き換えでも確認を求めない (build.rs と同じく、Claude Code の deny ルールとサンドボックスで防ぐ)
  - 自動承認とビルドを許す crate の一覧は、設定ファイルでも環境変数でも有効にできるままにし、MCP サーバーは起動時にだけ読む
  - CLI にも新しい依存の確認を適用した。`approve` は対話端末でだけ動き、エージェントの環境変数があれば拒否する。`approve` に `--project` を足した
  - 承認の記録を state のディレクトリに移し、GC・`cache clear`・`export` / `import` の対象から外した。Windows のキャッシュを `%LOCALAPPDATA%\chatora-rustdoc\cache\` に変えた (state と分けるため)
  - `DATABASE_URL` を、子プロセスの環境変数の既定の denylist から外した (sqlx のマクロがコンパイル時に使うため)。denylist に当たっても渡す変数の一覧を足した
  - README に、Claude Code の deny ルールの設定例 (CLI の deny を含む)、subagent の `tools` に MCP のツールを足す方法、残るリスク (信頼したプロジェクトでの書き換え、設定や CLI の環境変数による確認の省略、承認の偽造、CLI の環境変数で実行ファイルを選ぶ経路 (対策しない)、`Elicitation` hook) を書く
  - 2 つの allowlist を「信頼するプロジェクトの一覧」と「ビルドを許す crate の一覧」に呼び分けた
- **設定** (R-DIST-2・R-DIST-3)
  - Q-24 を決めた: プロジェクト単位の設定を `.config/chatora-rustdoc.toml` (共有) と `.config/chatora-rustdoc.local.toml` (メンバー・実行環境ごと。優先) に書ける。読むのは利用者が信頼したプロジェクトのものだけで、守りを緩める項目も書ける。全プロジェクトで共有するものと秘密情報は書けない。`CHATORA_RUSTDOC_CONFIG` は利用者の設定ファイルの場所だけを変え、プロジェクト単位の設定は重ねて読む
  - 設定の層はキーごとにマージし、配列は連結して重複を取り除く (denylist から外すには渡す変数の一覧に書く)
  - 環境変数の名前の付け方と値の書き方を決めた。上書きできるのは単純な値と文字列の配列だけで、配列はカンマ区切りでも書ける。CLI に `--config <キー>=<値>` (TOML) を足した (環境変数より優先)
  - R-DIST-3 に設定項目の一覧を置いた
- **ローカルビルドの運用** (R-WS-4・R-WS-6・R-WS-7・R-VR-2・R-CA-5・R-CA-7・R-CA-8・R-OBS-2・R-SEC-7・R-NW-2)
  - ビルドに失敗したら、直近の成功があっても常に `build_failed` を返す (v0.12 では stale を返す側に揃えていた)。stale を返すのは再ビルド中だけ
  - 一時的な失敗は、終了の仕方で分かるもの (シグナル、タイムアウト、中断、ロック待ちの打ち切り) と、ビルドの前に分けて行う `cargo fetch --locked` の失敗だけにした。それ以外 (build.rs の失敗を含む) は決定的な失敗として、鍵が変わるまで固定する
  - 前回のビルドの子孫のプロセスは、記録したサーバーが死んでいて起動時刻が一致するときだけ、次回の起動時に止める
  - rustup の自動導入を、子プロセスへの `RUSTUP_AUTO_INSTALL=0` で既定では止める (設定で許せる)
  - `cargo metadata` は全依存の `.crate` を取得するので、オフラインモードで失敗したら Cargo.lock の直接読みに落とす。初回の取得にも進捗通知を送る
  - GC の上限を、文書のキャッシュ (5GB) と専用 target dir (20GB、最終使用から 30 日) に分け、最大 25GB を使いうることを README に書く。GC の抜け (ビルドのプロセスの記録、中断したビルドの作業用ディレクトリ、書きかけの一時ファイル、仮のプロジェクト、OSV のミラー) を埋めた
  - 既定値を決めた: 同時実行の上限 (呼び出し 16・ローカルビルド 2・補完 1 秒に 20 回)、通信のタイムアウト (10 秒 / 120 秒)、ビルドの最大時間 (30 分)
  - ログの既定の場所 (state の `logs/`)、プロセスごとのファイル、GC (保持 14 日・合計 50MB) を決めた
  - ミラーへの差し替えをすべての取得元に広げ、取得元ごとの無効化とプロキシの扱いを決めた (R-SEC-6 も)
- **その他**
  - R-SC-4 / R-SC-6: symlink は行き先の実際のパスで判定し、既知のソースルートの中なら読み、外を指していれば読まない (`search_source` で workspace の中の symlink もたどるようにした)
  - R-ERR-1 / §10: 共有状態に poison の無いロックを使い、差し替えで更新する。clippy.toml の `allow-unwrap-types` を削り、`disallowed-types` で std の `Mutex` / `RwLock` を禁じた。`LazyLock` / `Once` の初期化で panic しうる処理をしない
  - R-MCP-13: `resources/read` でもローカルビルドと MRTR の確認を行い、失敗は JSON-RPC のエラー (`-32021`・`-32602`・`-32603` と `data` の §8 の `code`) で返す。Claude Code が MRTR に応えるかは M5 の PoC で確かめる
  - R-STD-1: `rust-docs-json` が無い target では、ホストの target の JSON で代用し、warnings で知らせる
  - R-DIST-1 / §9.1 / Q-27: 配布する target に musl (x86_64・aarch64) を足し、性能目標は gnu 版で測る。toolchain は浮動の stable のままとし、新しい stable の lint で CI が落ちたらその都度直す
  - R-SR-3: 検索の段を、パスの一致 > item 名の完全一致 > 最後の語の一致 > 前方一致 (CLI のみ) > 別名 > 部分一致 と定義し直した
  - R-AD-1 / R-AD-2 / R-AD-5: 引数名だけの変更を分類 `cosmetic` (影響なし) とし、既定では件数だけを返す。`removed_or_hidden` は removed に含める
  - 未決事項: Q-11 (`--cfg docsrs` は既定で渡し、壊れたら渡さずにやり直して、壊れた crate を記録する。docs.rs は常に渡し、groxide は docs.rs の設定で試して失敗を記録する。ほかの候補は渡していない)、Q-19 (英語に統一)、Q-26 (CLI の `--json` は MCP と同じ JSON) を決め、Q-16・Q-18 を確かめて解決した
- **v0.13 のレビュー (blindspot と 4 つのレビュー) を受けた決定**
  - CLI のエージェントの判定を `CLAUDECODE` から `CLAUDE_CODE_CHILD_SESSION` に変え、エージェントから呼ばれた CLI は信頼を作らないことにした (`cd` した先が既定のプロジェクトとして信頼される穴を塞ぐため。細部は上の「境界」の項)
  - 「基点の下」を境界の内側とする規則をやめ、境界を上の 3 つ (既定のプロジェクト・信頼するプロジェクト・確認の記録) にした
  - 信頼するプロジェクトの一覧を利用者の設定ファイルだけに書けるようにし (プロジェクト単位の設定から外した)、一覧と state の場所を XDG などの環境変数で決めることにした (理由は上の「境界」の項)
  - 子の cargo は、プロジェクトでは workspace のルートで起動し、仮のプロジェクトでは祖先に cargo の設定と toolchain の指定が無いことを確かめる (cargo は設定をカレントディレクトリの祖先から探す)。`CARGO_BUILD_BUILD_DIR` も専用の場所に向ける
  - 引数 `git` は、git のホストの一覧 (利用者の設定だけ。既定は github.com・gitlab.com・codeberg.org) に無いホストを取得の前に確認する
  - 自動承認が省くのは信頼済みのプロジェクトの新しい依存だけにし、記録を書かない。プロジェクト単位の設定はプロセスで初めて使うときに 1 回だけ読む。ミラー・取得元の無効化・プロキシは利用者の設定だけ
  - 確認画面の本文に、承認すると効くようになるもの (cargo の設定・toolchain の指定・プロジェクト単位の設定) を書き、サニタイズを強めた。Cargo.lock が無い境界の外のプロジェクトは、照会を latest、ビルドを `lockfile_outdated` にした。CLI のその場の確認もエージェントの環境変数で拒否する
  - 子プロセスを `setsid` で制御端末から外し、`GIT_TERMINAL_PROMPT=0` などを渡す。denylist に `CARGO_REGISTRY_TOKEN`・`ACTIONS_ID_TOKEN_REQUEST_TOKEN` などを足した。`lockfile_outdated` では `cargo fetch` を案内する
  - ツールの引数 `refresh` を足した。readOnlyHint の基準を「cargo を起動しうるか」にした
  - 対策せず README の残るリスクとしたもの: workspace の中に clone して path 依存や glob の member に足すこと、revoke の後の初回の自動記録、同じパスへの置き直し、state が空になる CI、ソースの読み取りによる Read の deny のすり抜け
  - Claude Code が stdio サーバーと 2026-07-28 を既定で交渉するのは v2.1.292 以降と訂正した。`rustc -vV` に `rustc-wrapper` は効かず、`build.rustc` も実行ファイルを動かすと訂正した
  - 小さな仕様: `api_diff` のローカルビルドに `--document-hidden-items` を付ける、`rust_release` は GitHub API から取る、複数の公開パスの cfg、latest は sparse index で解決する、`docsrs_only` の印、正準パスの選び方の順、tools/list の `cacheScope` の理由、検索の打ち切りの分母

### v0.12 (2026-10-14)

5 本のレビュー (blindspot の第三者レビュー、§0〜§5・§6〜§9・§10 以降と調査文書・設定とコメントのレビュー) の指摘のうち、方針を変えないものを反映した。方針の判断が要る指摘は別に扱う。

- **仕様との食い違いの訂正**
  - R-MCP-17: HTTP の `initialize` は必須ヘッダを欠くので、`-32022` ではなく 400 と `-32020` を返す。R-MCP-2: stdio の `initialize` は `_meta` の検査より先に判定する
  - R-ARG-1: `inputSchema` に合わない引数もツール結果エラーにする (rmcp の `Parameters<T>` の既定の `-32602` に頼らない)
  - R-MCP-5: `resultType: "input_required"` の結果にはキャッシュのヒントを付けない。`resources/list` は `"private"`
  - R-ERR-4 (新設): JSON-RPC の標準コード (`-32700`・`-32600`) を使ってよいコードに加え、廃止された `-32002`・`-32042` を出さない
  - R-SEC-2: elicitation の `accept` でも `approve` が `true` でなければ承認しない。associated data にメソッド名と resource URI を入れる。`-32021` を使わない理由。Claude Code の文書の引用を現行の文言に直した
  - R-MCP-16 (`subscriptionId`)、R-MCP-7 (PNG のアイコン、プロパティ名は Claude API の制約)、R-SEC-7 (`completion/complete` の頻度の上限と `busy`)
- **事実の訂正 (実測)**
  - R-WS-8 / Q-12: `RUSTC_BOOTSTRAP` を見る依存 (proc-macro2・anyhow・thiserror など) は、値を変えるたびに作り直され、その crate 自身の文書は nightly 用の API を含みうる
  - R-WS-4: `cargo rustdoc` は文書化する crate 自身の dep-info を書かない。子に渡す環境変数を鮮度の鍵に加えた
  - R-FV-2: `#[doc(hidden)]` に対応する `Attribute` の variant は無い。R-FV-3: 末尾の `format_version` は形式 46 の実データでも確認
  - §8 の `lockfile_outdated`: `cargo update` と既存の lock への `cargo generate-lockfile` は全依存を上げるので勧めない
- **矛盾の解消**: G4 と R-VR-4 (バージョンは差し替えない)、R-WS-4 と R-WS-7 (失敗時は直近の成功を stale で返す)、R-STD-2 と R-STD-3 (安定化の版は `Item::stability` から)、R-OUT-3 と G5 (上限は推定トークン)、R-SEC-5 と R-MCP-3 (HTTP は allowlist だけ)、R-ARCH-5 (承認の状態は例外)、Q-26 と R-OUT-3
- **参照と ID の訂正**: §5.1 (R-DS-6)、§14 v0.6 (R-PR-8)、付録 A (ferritin-common、R-QA-5)、§13 (basic/versioning)、`toolchain_missing` の対象に `rust-src`
- **§12 のマイルストーン**: M0 に PoC の項目、M2 にローカルビルドの境界と確認、M5 に eval で決める項目と任意の項目を足した。M1 の比較を eval からスナップショットに、M3・M4 の Q14 の重複を直した
- **重複と冗長の整理**: R-MCP-3 の Roots、R-OUT-1 の `outputSchema`、R-SEC-1 の結合テスト、R-SEC-2 の `not_allowed`、R-SEC-5 の 403、R-FT-3、R-WS-2〜4、R-STD-6、R-CA-3、§10 の MCP SDK の行など。句点が抜けて 2 文がつながっていた箇所 (R-DS-4、R-FV-5、R-ARG-1、R-ERR-1、R-SEC-1) を直した

### v0.11 (2026-10-14)

MCP の版の方針を決めた (Q-2)。

- R-MCP-2: MCP の最新版 (v1 では 2026-07-28) だけに対応し、2025-11-25 以前の `initialize` の経路を削った。新しい版が出たら追従し、1 つ前の版は Claude Code が新しい版を既定にするまで残す。古い版のクライアントの `initialize` には、対応する版を名指ししたエラーを返す。stdio を直接使えないクライアントは、利用者がトンネルを用意する前提にした
- 従来の経路に触れた記述を削った: R-MCP-3 (`roots/list`)、R-MCP-5、R-MCP-11、R-MCP-14 (`supportedVersions`)、R-MCP-15、R-MCP-17、R-SEC-2 (`elicitation/create`)、R-QA-5 の適合テスト
- §2 の非ゴールと付録 B に、古い版だけを話すクライアントと stdio を直接使えないクライアントへの個別の対応を足した

### v0.10 (2026-10-14)

MCP 2026-07-28 の仕様 (基盤とサーバー・クライアントの機能、拡張) と rmcp 3.5.1 の対応状況を一次資料で照合し、準拠の不足を直して、使える機能を取り込んだ。Claude 以外のクライアント (Gemini CLI、Codex CLI、ChatGPT など) は調査だけで、要件は足していない。

- **準拠の不足と誤りの訂正**
  - R-MCP-2: `-32022` の `UnsupportedProtocolVersionError`、必須の `_meta` を欠く要求の `-32602`、両世代の要求ごとの振り分け
  - R-MCP-5: `ttlMs` / `cacheScope` はキャッシュ可能な結果 (`server/discover`・`tools/list`・`prompts/list`・`resources/*`) に付け、`tools/call` には付けない (v0.9 までの誤読を訂正)。`resultType` は `"input_required"` もある。serverInfo を挙動の判断に使わない。`tools/list` はページに分けない
  - R-MCP-3: 従来の handshake でも `roots/list` を使わない (Roots は deprecated で、新しい実装は対応を足さない)
  - R-MCP-7: プロパティ名の規則の出典を Claude Code に訂正 (MCP の規則ではない)
  - R-SEC-2: MRTR の安全性。elicitation の対応を要求ごとに判定、`requestState` の完全性の保護 (HMAC、associated data、有効期限)、`InputRequiredResult` の返し方、decline / cancel の扱い、form のスキーマ
  - R-WS-6: progressToken がある要求にだけ進捗を送る、単調増加、応答後とキャンセル後に止める、キャンセルされた要求には何も送らない
  - R-OUT-1: 同じ JSON を text でも返す (仕様の SHOULD)、エラーには `structuredContent` を付けない、`outputSchema` に必ず合わせる (MUST)
  - R-SEC-1: 1 メッセージ 1 行。R-ARG-1: 未知のツールと形の誤りは `-32602`。§8: JSON-RPC のエラーコードは仕様のものだけ。R-MCP-10: `$ref` はローカルだけ
  - R-SEC-7 (新設): 出力のサニタイズと呼び出しの上限 (`server/tools` の Security の MUST)
  - R-SEC-5: `Origin` の 403、`cacheScope` をアクセス制御に使わない、静的トークンと OAuth の関係、`x-mcp-header` を使わない
- **新設**: R-MCP-14 (`server/discover` の中身と capabilities。`listChanged` と `logging` は宣言しない)、R-MCP-15 (Logging を使わない)、R-MCP-16 (`subscriptions/listen`)、R-MCP-17 (Streamable HTTP の適合。M6)
- **使える機能の取り込み**: ツールの `title` (R-MCP-7)、icons (R-MCP-7、M5 任意)、prompts (R-MCP-18、M5 任意)、completions (R-MCP-19、M5 任意)、resource template の URI の見直しと `resources/list`・キャッシュのヒント・`-32602` (R-MCP-13)、tasks 拡張 (R-WS-6、M6 任意)
- 付録 B に、elicitation の URL モード、`anthropic/requiresUserInteraction`、Skills 拡張、MCP Apps を足した。§10 に rmcp の対応状況を、§13 に参照した仕様のページを足した

### v0.9 (2026-10-14)

調査文書の全候補 (Tier S〜D、三次調査で見つかった候補、隣接ツール、§7 の所見) の pros と cons を 1 項目ずつ要件と突き合わせた (独立した監査 2 本)。pros はほぼ取り込み済みで、cons は次の抜けを埋めた。

- **防げていなかった cons**
  - R-WS-7: ビルド失敗の結果をキャッシュし、鍵が変わるまで再ビルドしない (rustdoc-mcp は呼び出しのたびにやり直す)
  - R-WS-4: 内容ダイジェストの鍵を、workspace member 以外のローカルビルド (path / git 依存、仮のプロジェクトなど) にも適用する
  - R-SEC-1: 子プロセスの stdin / stdout を継承させない (mcp-rustdoc-parser の stdout 汚染と同じ失敗の防止)
  - R-SC-2: `.crate` の `cksum` の照合と、展開時のトラバーサル・symlink・サイズの制限。std のソースは std の JSON と同じ版の `rust-src` から読む (dist から取得できることを確認)
  - R-ERR-1 / §8: panic を `internal_error` の `isError` で返す
  - R-QA-1: 32,000 文字は上限であって目標ではない (46ki75 の冗長な出力も収まる)。代表的な item ごとの予算を M1 で決める
  - R-QA-5: 説明文が約束する動作、エラーコードごとの `isError`、README のツール名、MCP の適合性のテスト
- **その他の追加**: R-DS-4 と R-NW-3 (展開の段数とディスク上の上限)、R-FV-5 (canonical より新しい形式を読めたときの warnings)、R-WS-5 (仮のプロジェクトの feature の既定)、R-WS-6 (singleflight とキャンセル、ビルドの最大時間、索引作成の進捗、並行処理)、R-WS-8 (新しすぎる形式のときの切り替え、`rustc -vV` のキャッシュ)、R-WS-7 (自動で導入しない)、R-STD-6 (任意: stable に近い日付の nightly)、R-MCP-3 (`project_not_found` の候補)、R-MCP-12 (`core` を勧める)、R-MCP-13 (resource URI の検証)、R-OUT-2 (実行時に rustfmt を要求しない)、R-CA-4 (symlink)、R-CA-7 (オフラインでの TTL 切れのキャッシュ)、R-CA-8 (隔離環境: ミラーとキャッシュの持ち込み)、R-SEC-6 (取得元以外へ通信しない)、R-DIST-1 (依存の更新、パッケージの検査、導入手段の CI、attestation と正規の入手先)、R-DIST-4 (他実装のコードの流用はライセンスで限る、cargo-deny)、R-DIST-5 (二重登録の警告)、§5.9 (sparse index の feature は解決した版の行)
- 未決事項に Q-25 (cargo#16720) と Q-26 (CLI の出力形式。付録 B の JSON Lines の行は先送りだったので移した) を足した

その後、判断が要る項目を 1 つずつ決めた:

- R-MCP-4: ローカルビルドを起こしうるツールは `readOnlyHint: false` (`destructiveHint: false` を併記)。ビルドしないツールとローカル機能を含まない構成は `true`
- R-QA-4: 新しい format_version への対応リリースの目標を 14 日とし、7 日を超えたら既知の問題として掲示する
- R-ERR-2 / R-OBS-1: 呼び出しごとの `request_id` をエラーとログに入れる (付録 B の `diagnostic_id` の行を削除)
- R-OUT-1 / R-QA-6: `output.mode` に `toon` を加えて eval で比べる (付録 B の TOON の行を削除)
- R-DIST-2: CLI の `api` に、任意で llms.txt / Markdown の出力を加える
- R-OUT-2: 描画できない形に出会ったら `source_signature` を退避策として添える (付録 B の行を削除)

### v0.8 (2026-10-14)

4 本のレビュー (blindspot の第三者レビュー、本文書・調査文書・設定とコメントのレビュー) の指摘を反映した。主な指摘は手元の実験で再確認した。

- **誤りの訂正 (実験で確認)**
  - R-FT-3 / R-FT-2: `cargo metadata` の `resolve.nodes[].features` は dev・build・target 別の依存の feature まで統合した値で、実際のビルドより多い。依存をフラグなしでビルドして統合されるのは通常の依存の辺の feature だけ。`features_effective` と `enabled_in_project` をここから取らない。Q-20 を広げた
  - R-VR-2: `cargo metadata` と `cargo rustdoc` はユーザーの Cargo.lock を作ったり書き換えたりする。常に `--locked` を付け、`lockfile_outdated` を返す
  - R-WS-1: `cargo doc --output-format json` は、crate を絞った `RUSTC_BOOTSTRAP` では動かない (`RUSTC_BOOTSTRAP=1` か nightly の cargo が要る)
  - R-SEC-2: workspace を rustdoc にかけると、依存の build.rs と proc-macro もすべて実行される
- **セキュリティ**: R-MCP-3 にローカルビルドの境界 (`CLAUDE_PROJECT_DIR` の下と allowlist) を足した。R-SEC-2 に、MCP 経由のビルドは Claude Code の権限確認を通らないこととプロンプトインジェクションの経路を書き、Q-21 を足した。R-WS-5 の仮のプロジェクトでのビルドに許可を必須にした。子プロセスの環境変数は、allowlist をやめて denylist にした (allowlist では private registry・sys crate・Windows・macOS でビルドが壊れる)。R-SEC-1 に stdout の結合テストを足した
- **方針変更後の直し漏れ**: nightly が前提の記述 (R-WS-4 の鮮度の鍵、R-WS-7、§8 の `toolchain_missing`、R-DIST-5 の hook、R-WS-8 の std の行) を、R-WS-8 と R-STD-6 に合わせた。§0.1 の説明文の上限、§7.3 の例の target、§2 と R-SEC-5 の HTTP の条件を直した
- **矛盾と抜けの解消**: R-MCP-3 の決め方から `roots/list` を外した。R-WS-7 の stderr の扱い、R-STD-4 と R-CA-7、R-PR-4 と R-VR-5、R-MCP-5 の `cacheScope` (stdio にも適用)、R-OUT-1 の text モードと `outputSchema`、R-QA-6 の比較する設定、R-NW-2 の対象と一時的エラーの範囲をそろえた。§12 のマイルストーンに `list_crates` などを足し、完了条件の問いの番号を直した
- **その他の追加**: R-FV-2 (形式 54 未満の属性文字列を構造化して写す)、R-TG-1 (docs.rs でも `build.target` を見る)、R-WS-2 (target dir のパス長)、R-WS-4 (dep-info を鮮度の鍵に含める)、R-WS-6 (`PR_SET_PDEATHSIG`)、R-WS-8 (`RUSTC_BOOTSTRAP` の値はターゲットの crate 名)、R-MEM-1 (OS ごとの指標)、R-MEM-2 と Q-22 (巨大な crate のストリーミング)、Q-23 (検証と性能)、R-DIST-1 (`aarch64-unknown-linux-gnu`)、§4 (「ホスト版」「任意」の定義)、§8 (`project_not_found`、`lockfile_outdated`)、Q-24 (R-DIST-3 のプロジェクト単位の設定の上書き。「検討する」を未決事項にした)。ローカルビルドの境界は CLI には適用しない
- **重複の整理**: 同じ規則を複数の節に書いていたもの (URL の確認、一時的エラー、絞り込みの順序、CI の検査項目、ファイルロック、stale の返却など) を 1 か所に寄せ、他は参照にした。「(v0.x の未決事項 Q-n は解決)」を「(Q-n)」に縮めた
- 付録 A: possdog の HEAD の日付を 2026-10-01 に、ビルド失敗の行の実装を ferritin-common に直した。§13 の重複した行を削った

その後、残ったリスクを 1 つずつ確認して決めた事項と、追加の検証:

- Q-21 を決定: 新しい依存の初回のローカルビルドは、elicitation で利用者の確認を要する (R-SEC-2)。CLI に `approve` を足した (R-DIST-2)
- 子プロセスの環境変数は denylist の方式で確定した (R-SEC-2)
- プロジェクトの文脈で cargo を呼ぶときは常に `--locked` を付ける方式で確定した (R-VR-2)
- R-STD-6 (dist からの std の取得) を既定で有効にすることを確定した
- docs.rs の `DEFAULT_TARGETS` と URL の挙動 (302 / 400 / 404 / ETag と 304)、`cargo metadata` による Cargo.lock の作成と `--locked` での失敗、`cargo doc --output-format json` と `RUSTC_BOOTSTRAP` の関係、同名の複数版での artifact メッセージの食い違いを、まとめた側で再確認した
- R-WS-8 / R-WS-1 / Q-12: stable + `RUSTC_BOOTSTRAP` が、cargo 経由で lib・bin・proc-macro・build.rs を持つ crate に使えることを確認した。lib と bin の crate 名が同じだと JSON が上書きされることを足した

### v0.7 (2026-10-14)

トランスポートの方針を決めた。

- R-MCP-1: v1 は stdio だけを実装し、Streamable HTTP はホスト版か共有キャッシュサーバーを作る段階 (M6 以降、任意) まで遅らせる。理由 (ディスクキャッシュと mmap の共有で、複数セッションの節約の大半はすでに得られている) と、HTTP で新たに要るものを書いた。Claude Code の時間制限のトランスポートごとの違いも書いた
- R-SEC-5: HTTP で扱えるプロジェクトを allowlist で限ること、プラグインでは `${CLAUDE_PROJECT_DIR}` をヘッダで渡せること、プロジェクトに依存する結果の `cacheScope` を `private` にすること、トークンの扱いを足した
- R-MCP-3 / R-ARCH-5: 後から HTTP を足せるよう、`project` 引数の経路と、セッションの状態に依存しないことをテストで守る
- §2、R-DIST-2、§12: HTTP の時期を M6 以降に揃え、マイルストーン M6 を足した

### v0.6 (2026-10-14)

調査文書の全候補 (Tier S〜D、三次調査で見つかった候補、隣接ツール、参考ライブラリ) の良い点を 1 つずつ要件と突き合わせた (独立した監査と、まとめた側の照合の 2 系統)。抜けていたものを足し、採らなかったものは理由とともに付録 B に残した。

- **追加した要件**
  - R-FT-2 / R-OUT-7: 一覧系の各項目にも必要な feature の要約を付ける (groxide、truesight)
  - R-OUT-8 / §6.3: `get_item` に定義位置 `source_location` を返す (possdog、lockdocs)
  - §6.5 (新設): `list_module` の `recursive` による crate の骨格 (ruskel、rustdoc-mcp)、`list_module` の kind の絞り込み、`list_methods` の名前と出どころの絞り込み (syphar のブランチ)
  - R-STD-6: nightly の toolchain を入れずに、dist から `rust-docs-json` のアーカイブだけを取って std を引く (ferritin のホスト版の利点。manifest に URL と sha256 があることを確認)
  - R-SR-1: 照合する欄の選択 `fields` (ruskel)、crate に同梱された README と `docs/` の散文の検索 (rust-knowledge-mcp。M4、任意)
  - R-SR-5: 0 件のときの `did_you_mean` (possdog、snowmead)
  - R-MCP-12: 入口のツールだけを出す `tools.profile = core` (ferritin のツール 2 個の利点。tool search を持たないクライアント向け)
  - R-MCP-13: resource template (`rustdoc://…`) で利用者が会話に添付できるようにする (cargo-doc-mcp の発想。M5、任意)
  - R-PR-8: 深い再帰は大きなスタックのスレッドか反復で行う (ferritin)
  - R-SH-4: impl を出どころの順に並べる (ferritin)
  - R-WS-5: Cargo.lock に無い版や crate のローカルビルドは仮のプロジェクトで行う (cargo-semver-checks)
  - R-CA-2: アーカイブのスキーマの版に rkyv の版の系列を含め、テストで強制する (ferritin)
  - R-CA-5 / R-CA-6: `--refresh`、任意の一覧の `prefetch`、Cargo.lock の変化での自動取得 (dmvk、rust-docsbox、rust-mcp)
  - §5.9: sparse index の feature を先に返す、被依存 crate の一覧 (mmgeorge)。§6.3 の `list_crates`: 依存の経路と JSON の有無 (snowmead、lockdocs)。§8: `docs_unavailable` で `readme` を案内する (mmgeorge)
  - R-DIST-1: 公式 MCP レジストリへの登録 (ferritin、lockdocs)。R-DIST-2: CLI の `api` (truesight、cargo-aidoc)
- **理由の追記**: §2 の非ゴールに、semantic search を v1 で採らない理由と、同梱の散文は別扱いであることを書いた。§0.1 のツール数の行に R-MCP-12 を添えた
- §12 のマイルストーンを更新。付録 B を新設

### v0.5 (2026-10-14)

調査文書の三次調査 (既存実装・docs.rs・rustdoc JSON・MCP 仕様・Claude Code の文書の再検証) と、手元での実験の結果を反映した。

- **誤りの訂正**
  - R-DS-3: docs.rs の JSON の有無を決めるのは公開日ではなくビルド日。公開日から推定しない
  - R-FT-3 / Q-10: 依存 crate に `--features` を渡す方式は cargo が拒否する。依存はフラグなしでビルドすれば統合された feature が効く (実験で確認)
  - R-MCP-2 / Q-2: Claude Code が 2026-07-28 を使い始める版は 2.1.292 ではなく、v2 ランタイムの v2.1.232 / v2.1.274
  - R-MCP-3: 2026-07-28 の経路では `roots/list` を使わない (MRTR でしか送れない)
  - R-SR-3: 前方一致の展開を MCP では行わない (CLI だけ)
  - §6.2: Claude Code の説明文の上限を現行の値 (4,096 / 16,384) に更新
  - 付録 A: syphar・dmvk・possdog の行番号が、どれも main のファイルに実在しない行を指していた。各 HEAD の SHA を併記して訂正した
- **未決事項の解決**: Q-4 (下限 46。47 は欠番)、Q-10 (上記)。Q-12・Q-13・Q-17 は判明した事実で範囲を絞った。Q-20 を追加
- **方針の変更**: R-WS-8 で、ローカルビルドの既定の候補を「プロジェクトの toolchain + 対象 crate に限った `RUSTC_BOOTSTRAP`」にした (stable 1.99 で形式 61 が出ることを確認)。確定は Q-12 の PoC の後
- **追加した要件**
  - データ取得: R-DS-6 (docs.rs の URL・圧縮・ETag・エラーの挙動)、R-TG-1 (docs.rs の既定 target。`x86_64-apple-darwin` が無い)、R-WS-1 (`cargo doc --output-format json` と、同名の複数版での artifact メッセージの食い違い)
  - パスと検索: R-PR-1 (rustdoc の区別子 `struct@` など、crate 名のハイフンとアンダースコアの同一視、2 段のフォールバック)、R-SR-1 (`doc(alias)`、シグネチャ検索)、R-SR-4 (スコアの落差による件数の自動打ち切り)、R-PR-7 (編集距離を文字で数える)
  - ソースと crate 情報: R-SC-6 / `search_source` (ソースの検索)、§5.9 (feature の閉包と `enabled_by`、example の `required-features`)
  - std: R-STD-1 (provenance に target)、R-STD-2 (`default_unstable`、unstable に issue 番号が無いこと)
  - MCP: R-MCP-4 (annotations を実態と合わせる)、R-MCP-5 (`cacheScope`、`resultType`)、R-MCP-7 (フル名 128 文字、プロパティ名の文法)、R-MCP-10 (ルートの combinator 禁止、null 許容の型を出さない)、R-MCP-11 (起動を待たせない)
  - 出力: R-OUT-3 と共通引数 `max_chars` (呼び出しごとの予算。M5 で採否)
  - ローカルビルドの運用: R-WS-2 (パッケージキャッシュのロック待ちの検知)、R-WS-6 (終了時に子プロセスを止める)
  - セキュリティ: R-SEC-2 (子プロセスの環境変数の allowlist、`.cargo/config.toml` も信頼の範囲、Claude Code のサンドボックスは MCP サーバーを覆わない)、R-SEC-4 (target triple と feature 名の検証、索引への問い合わせのパラメータ化)
  - キャッシュ: R-CA-5 (所有マーカー、`CACHEDIR.TAG`、symlink をたどらない GC)
  - 品質: R-OBS-1 (トークン換算の校正)、R-QA-4 (live の docs.rs の smoke test、失敗時の Issue の自動開閉)、R-QA-5 (ツール名の実在、引数の使用、スキーマの制約の CI 検査)、R-QA-6 (トークンの 3 分類、`claude plugin eval`)
  - 配布: R-DIST-5 (`userConfig`、`${CLAUDE_PLUGIN_DATA}` を使わない)、R-DIST-6 (skill の単独インストール)
- **参考実装と参照の追加**: cargo-semver-checks、officina (Pavex)、maan2003 と migrate_rustdoc_json (§5.2)、cargo の Issue と PR (§13)
- §3 に Q21 (ソースの検索) を追加。§12 のマイルストーンを更新

### v0.4 (2026-10-14)

配布と設定を Windows を含む 3 つの OS に広げた。

- 名前を `chatora-rustdoc-mcp` から `chatora-rustdoc` に変えた。MCP サーバーだけでなく CLI としても使うため。crate 名、バイナリ名、設定とキャッシュのディレクトリ名、環境変数の接頭辞 (`CHATORA_RUSTDOC_`) をこれに揃える
- R-DIST-1: Windows を「検討する」から対象に変えた。配布の仕組み (dist、release-plz、binstall のタグ) と CI の OS を ferritin に倣って決めた。crate を 1 つにする理由を書いた
- R-DIST-3: 設定ファイルの場所を OS ごとに決めた (v0.3 までは `~/.config` 固定で、Windows と macOS の扱いが無かった)
- R-DIST-4: ライセンスを確定した
- R-CA-1: キャッシュの場所を R-DIST-3 と同じ規則にした。R-CA-4 に、Windows で mmap 中のファイルを置き換えられない件を足した

### v0.3 (2026-10-13)

誤り・重複・曖昧さを整理した。

- **誤りの訂正**
  - R-SH-4: 汎用 trait の blanket impl を「隠す」と、`list_methods` から `to_string()` などが消えていた (R-TI-1 と矛盾)。「名前だけを 1 行に畳む」に変更し、条件付きのものは条件を満たす型にだけ載せる
  - R-OUT-3: 「8,000 トークンかつ 40,000 文字」は食い違っていた (40,000 文字は約 1 万トークン)。32,000 文字に統一
  - R-VR-2: docs.rs は crates.io の crate しか扱わないので、「設定したレジストリ」を削除
  - §7.3 の例: 既定 target へのフォールバックの例が成り立っていなかった。文字列の言語も統一
- **重複の削除**: R-STD-5 (R-SH-5 と R-CA-2 に統合)、R-WS-9 と §9.9、R-DS-3 と R-VR-4
- **曖昧さの解消**
  - 一覧は `page`、本文は `truncated` と使い分けを決めた
  - エラーコード `invalid_argument` を追加。`crate@version` と `version` が食い違う場合の扱いを決めた
  - R-PR-4 の「起点」、G5 の「十分下回る」を具体化
- **未決事項の追加**: Q-18 (条件付き blanket impl の扱い)、Q-19 (モデル向け文字列の言語)

### v0.2 (2026-10-13)

既存実装 20 件超のソースを精読して反映した (クローンは読むだけで、ビルドも実行もしていない)。追加した観点は §0.2、解決した未決事項は §11.1、根拠は付録 A。v0.1 の誤りは次のとおり訂正した。

- R-DS-2: 「docs.rs は rebuild 後も古い形式を保持する」は不正確だった。docs.rs が配るのはそのリリースで実際にビルドされた形式だけで、古い形式が残るのは再ビルドされたリリースに限る
- R-FV-4: 下限 48 は不足。docs.rs には形式 46 が実在する
- R-MCP-3: 「Claude Code はプロジェクトのディレクトリで stdio サーバーを起動する」は文書化されていない。`CLAUDE_PROJECT_DIR` を使う
- §5.2 の比較表: dmvk が属性を全部捨てていること、46ki75 の対応形式を訂正

### v0.1 (2026-10-13)

- 初版

---

## 付録 A. 既存実装から得た教訓と、対応する要件

先行実装で実際に見つかった問題 (ソースまたは実測で確認したもの) と、それを防ぐ要件の対応表。行番号は次の HEAD のもの (2026-10-14 に再確認)。

| 実装 | HEAD |
| --- | --- |
| ferritin / rustdoc-mcp (jbr) | `8fbe4a7` (2026-10-02) |
| docs-rs-mcp (syphar) | `26c1e67` (2026-08-21) |
| docsrs-mcp (dmvk) | `cab1d4a` (2026-02-08) |
| mcp-rustdoc (possdog、GitHub ミラー) | `c05cf13` (2026-10-01) |
| rust-docs-mcp (snowmead) | `8c89df0` (2026-09-21) |
| その他 | 2026-10-13〜14 時点の各 HEAD |

| 実装 | 見つかった問題 | 根拠 | 対応する要件 |
| --- | --- | --- | --- |
| ferritin | 下限 48 で、docs.rs に実在する形式 46 を「JSON が無い」扱いにする | `sources/docsrs/client.rs:280-290`、tokio 1.45.1 の実データ | R-FV-4 |
| ferritin-common (ferritin と rustdoc-mcp が使う) | ユーザーの `target/` と `RUSTDOCFLAGS` を使う。同名の複数版が同じファイルを上書きしあう | `sources/local.rs:74,100-102,357-375` | R-WS-1、R-WS-2、R-VR-2 |
| ferritin-common | 鮮度をワークスペースのルートの `src/` の mtime だけで判定し、仮想 workspace で再ビルドされない | `sources/local.rs:222-234` | R-WS-4 |
| ferritin-common | ビルド失敗を握りつぶし、「存在しない」として 1 時間キャッシュする (TTL は `store.rs:60`。rustdoc-mcp では次の行のとおり毎回やり直す) | `sources/local.rs:262-271,325-335`、`store.rs:790`、PR #442 | R-WS-7、R-ERR-3 |
| rustdoc-mcp | ツール呼び出しのたびに Store を作り直す (コメントは「1 つの Navigator でセッション全体を賄う」と書いているのに逆)。キャッシュが 1 回の呼び出しの中でしか効かず、失敗したビルドを毎回やり直し、毎回 `cargo metadata` を実行する | `rustdoc-mcp/src/request.rs:43-49`、`tools/get_item.rs:167` | R-VR-2、R-ARCH-5、R-PF-1 |
| rustdoc-mcp | `cargo doc` でユーザーの `target/` に書き込み build.rs を実行しうるのに、annotations に `readOnlyHint: true` を付ける (main のみ。0.10.0 には annotations 自体が無い) | `rustdoc-mcp/src/tools.rs:4-16` | R-MCP-4 |
| rustdoc-mcp | 引数 `workspace_member` を宣言しただけで、実装で使っていない | `rustdoc-mcp/src/tools/list_crates.rs:14-16,37-81` | R-QA-5 |
| rustdoc-mcp | 独自の描画処理を持ち、trait の implementors を出さない (ferritin は出す)。固有メソッドを「Associated Types:」の見出しの下に出す | `rustdoc-mcp/src/formatting/trait.rs:5-56`、`formatting/impls.rs:31` | R-TI-3、R-QA-1 |
| ferritin | プロセス間ロックが無い。元データの書き込みが原子的でない。mmap したアーカイブを検証せずに読む | `archive.rs:226,235`、`client.rs:591` | R-CA-4 |
| ferritin | パスを持たない型 (プリミティブ・`&T`・スライス) の impl を逆引き索引に入れない | `indexes.rs:59-70` | R-TI-4 |
| ferritin | `Vec::iter`・`Vec::into_iter`・`std::primitive::slice` を引けない | ホスト版での実測 | R-TI-1、R-PR-1 |
| ferritin | 定義パス (`tokio::sync::mutex::Mutex`) や hidden モジュール経由のパスを表示・検索結果に出す | ホスト版での実測、`search_agent_mode` スナップショット | R-PR-2、R-SR-4 |
| ferritin | unstable な API・private フィールド・std 内部の trait を印なしで出す。trait メソッドを「Private」と表示 | ホスト版での実測、`format/mod.rs:386` | R-STD-2、R-SH-5 |
| ferritin | intra-doc link の行き先とインラインコードのバッククォートを捨てる | `renderer/agent.rs:577-586` | R-OUT-5 |
| ferritin | 「… and 103 more」の続きを取る手段が MCP に無い | `format/trait.rs:705`、実測 | R-OUT-3 |
| ferritin | エラーを `isError` にしない。無関係な「もしかして」を出す | ホスト版での実測 | R-ERR-1、R-PR-7 |
| ferritin | `Vec` 1 つで約 26.7k 文字。生の rustdoc JSON の部分木は、描画済みの Markdown の 8〜12 倍 | ホスト版での実測、スナップショット | R-OUT-2、R-OUT-3、R-QA-1 |
| syphar | シグネチャを生の `ItemEnum` で返し、描画済み文字列の約 5〜6 倍。`list_methods(Router)` で約 8.7k トークン | axum のテストデータでの計測 | R-OUT-2 |
| syphar | trait のパスが書かれたままの短い名前 (別物の `ServiceExt` が 3 つ区別できない。`axum::service_ext`・`tower::util`・`tower_http::service_ext`) | `src/client/list_impls.rs:33-35` | R-OUT-2、R-PR-2 |
| syphar | `get_item` でメソッドを引けない (モジュールしかたどらない) のに、`list_methods` の説明文はそこへ誘導する | `src/client/get_item.rs:94-147`、`instructions/tools/list_methods.md:2-3` | R-PR-1、R-QA-5 |
| syphar | `doc_status` (HTML のビルド成功) を JSON の有無と取り違える | docs.rs のソース `build_status.rs` | R-DS-5 |
| syphar | ダウンロードと展開が原子的でなく、途中のファイルが永久にキャッシュヒットになる | `src/client/mod.rs:54-69`、`src/client/get_docs.rs:102-105`、`src/client/get_source.rs:34-37` | R-CA-4 |
| syphar | changelog が全リリースを返す (axum で 90 件、約 10 万文字)。README を印なしで 4,000 文字に切り詰める | `src/client/changelog.rs:62-72`、`src/tools/crate_overview.rs:53` | §5.9、R-OUT-3 |
| syphar | 検索が部分一致 + パス順で、順位付けも件数の表示も無い | `src/client/search_items.rs:84-95` | R-SR-3、R-SR-4 |
| syphar | 「blanket impl を省く」というフィルタが効かない (rustdoc は blanket impl の `for_` に具体的な型を入れるため。axum の `Router` では 44 件中 18 件が blanket 由来) | `src/client/list_methods.rs:40-43` | R-SH-4、R-TI-2 |
| syphar | manifest の `readme` と `[[example]].path`、crate 名と target を検証せずにパスへ使う | `src/client/readme.rs:43-45`、`src/client/find_examples.rs:50-57`、`src/client/mod.rs:40-52` | R-SC-4、R-SEC-4 |
| dmvk | 全形式で `attrs` を空にし、属性をすべて失う | `src/docs/fetcher.rs:58-104` | R-FV-2、R-FV-7 |
| dmvk | Cargo.lock の版を文字列で比較する。起動時に一度しか読まない | `src/cargo_lock.rs:28-36`、`src/main.rs:35` | R-VR-2 |
| dmvk | 編集距離をバイト長で数え、ASCII 以外の名前で閾値が狂う | `src/docs/index.rs:262-274` | R-PR-7 |
| dmvk / possdog | 定義パスを使えるパスとして扱い、umbrella crate で引けない (Issue #5) | dmvk `src/docs/parser.rs:39-66`、axum の実データ | R-PR-2 |
| possdog | ビルドやパースの失敗で黙って docs.rs に切り替え、同名の別 crate を取ってくる | `src/server.rs:160-163` | R-WS-7、R-VR-2 |
| possdog | キャッシュのパスに crate 名と版を検証せずに使う (パストラバーサル。絶対パスを渡すとキャッシュの基点ごと置き換わる) | `src/cache.rs:33-38` | R-SEC-4 |
| snowmead | ビルドを task ID のポーリングにし、1 つのビルドがサーバー全体のロックを握って他の問い合わせを止める | `cache/tools.rs:774-882`、`docs/tools.rs:188` | R-WS-6 |
| snowmead | nightly の日付固定で、新しい MSRV の crate が失敗する (main では日付の更新と環境変数での上書きが入ったが、形式 57 を出す toolchain に限られる) | Issue #64、`rustdoc.rs:88-104` | R-WS-8 |
| snowmead | `--all-features` を既定にし、相互排他の feature で壊れる (main で修正済み。crates.io の 0.1.1 は未修正) | Issue #59 | R-FT-3 |
| snowmead | シグネチャの描画で情報を落とす (generics は `<...>`、引数は名前だけ、戻り値は `-> ...`) | `rust-docs-mcp/src/docs/query.rs:386-428` | R-OUT-2 |
| snowmead | span のファイル名を検証せずにソースのルートに連結して読む (絶対パスならキャッシュの外を読む) | `rust-docs-mcp/src/docs/query.rs:557` | R-SC-4 |
| snowmead | prompt が、改名して存在しないツール名 (`cache_crate_from_cratesio` など) を案内し続けている | `rust-docs-mcp/src/service.rs:286-288` | §6.2、R-QA-5 |
| cratedex / MasYaaz / KevinT3Hu | JSON が存在すれば新しいとみなす | `docs.rs:263`、`indexer.rs:286-298`、`doc_gen.rs:35` | R-WS-4 |
| snowmead | ローカルのソースを Cargo.toml の版をキーにキャッシュし、編集しても `update=true` を渡すまで反映しない | 調査文書 §4 の snowmead (ソースの精読) | R-WS-4 |
| KevinT3Hu | 依存 crate に `--no-default-features --features …` を渡して feature を再現しようとし、cargo に拒否されて依存の生成が毎回失敗する | `index.rs:64-80`、`doc_gen.rs:49-53`、手元の実験 | R-FT-3 |
| MasYaaz | 引数の `kind` を SQL に文字列連結する (インジェクション)。`ORDER BY rank` が `IN` サブクエリの中にあり、FTS の順位が捨てられる。保存時と問い合わせ時で kind の名前が食い違い、5 つのツールが常に空を返す | `queries.rs:69-74,217,249,302,355,453`、`indexer.rs:22-45` | R-SEC-4、R-SR-3、R-QA-1 |
| JLFN | 形式 60 だけを完全一致で読む (57 や 61 でビルドされたリリースは読めない) | `docs_rs.rs:68` | R-FV-4、R-FV-5 |
| ruskel | crates.io の 0.0.11 は形式 57 しか読めないが、浮動の `nightly` を使う (main は形式 61 に追従済みだがリリースされていない)。リリースが形式の変化に遅れると、利用者の手元では壊れる | `Cargo.toml` (v0.0.11 と main)、`toolchain.rs:25` | R-QA-4、R-FV-5 |
| mmgeorge | エラーを JSON-RPC の `internal_error` で返す。`crate_item_get` が要求した版ではなく最新安定版の feature を付ける | `crate_item_get.rs:43` | R-ERR-1、R-VR-4 |
| 46ki75 | live の docs.rs に対する週次の CI が 5 週続けて失敗したまま放置され、その間に現行の形式が読めなくなった | `ci-live.yml` の実行履歴 (2026-09-14〜10-12) | R-QA-4 |
| cratedex | 依存 crate のローカルビルドを非現実的として撤回 | commit a5f8236 | R-DS-1 |
| MasYaaz | 形式の解析失敗を 0 件として黙って返す | `indexer.rs:97-100` | R-FV-5 |
| Xevion | registry のソースディレクトリの中で cargo を実行し、依存を再解決する | `workspace/rustdoc.rs:187-205` | R-WS-5 |
| ruskel | trait 名で derive を推測し、手書きの impl を derive と誤表示する | `selection.rs:13-31` | R-TI-2 |
| cargo-public-api | 型のパスの代替に定義パスを使う。deprecated を描画しない | `render.rs:561-584` | R-PR-2、R-SH-6 |
| docs.rs の JSON | hidden になった item が「削除」に見える (tokio `TcpSocket::tos`) | tokio 1.53.1 の実データとソース | R-AD-3 |
| docs.rs の JSON | 初期のビルドが二重 zstd 圧縮で配られる | tokio 1.45.1 の実データ | R-DS-4 |
| cratesio-mcp | 脆弱性を版を問わず照合し、誤検出する | `audit.rs:168,185` | R-AU-1 |
| Claude Code | `structuredContent` だけがモデルに渡る。エラー時は `structuredContent` を無視し、text のブロックをつないで渡す | Issue #79944、#86032、2.1.301 での実測 (2026-10-16) | R-OUT-1、R-ERR-2 |
| Claude Code | 説明文と instructions は 4,096 文字 (tool search 経由のツールは 16,384 文字) で切り詰め (v2.1.296 より前は 2,048)。1 万トークンで警告、5 万文字でファイル退避 | 公式文書 (2026-10-14 取得)、CHANGELOG | §6.2、R-OUT-3 |
| Claude Code | MCP サーバーは Bash のサンドボックスの外で動く。起動タイムアウトは既定 30 秒。終了時は Unix で SIGINT → SIGTERM → SIGKILL (計 500ms)、Windows で stdin を閉じて 300ms 後にツリーを終了 | 公式文書 (2026-10-14 取得) | R-SEC-2、R-MCP-11、R-WS-6 |

## 付録 B. 調査した候補の良い点のうち、採らなかったもの

調査文書 ([rustdoc-mcp-survey.md](rustdoc-mcp-survey.md)) の候補の良い点は v0.6 と v0.9 で、MCP 2026-07-28 の機能と古い版への対応は v0.10・v0.11 で、要件と突き合わせた。取り込んだものは各要件に出典を書いてある。ここには、検討したうえで採らなかったものと、その理由を残す。

| 良い点 | 出典 | 採らない理由 |
| --- | --- | --- |
| 公開ホスティング (インストール不要) | ferritin、46ki75、cratesio-mcp | v1 の非ゴール (§2)。層構造で将来に備える (§9.10) |
| LLM による要約を返す | Govcraft | 非ゴール (§2)。損失とハルシネーションがあり、生データを返す方針と合わない |
| HTML の索引・スクレイピング (stable だけで完全オフライン) | BrightX、kieled ほか | 精度が rustdoc JSON に劣る (impl・re-export・macro 生成 API を正しく扱えない)。stable だけで使いたい要求は R-WS-8 と R-STD-6 で満たす |
| tree-sitter によるソース解析 | lockdocs、Daipendency | 同上。impl block や macro 生成の API が抜ける |
| 埋め込みによる semantic search | RimoVR、ferrisfetch | v1 の非ゴール (理由は §2) |
| clippy・rustfmt・playground・`rustc --explain` のツール化、clippy の lint ガイドの検索 | rust-docsbox、rustkb | 非ゴール (§2)。Claude Code は Bash で実行できる。症状から lint を探す使い方は、同梱の skill で案内する (R-DIST-5) |
| JSON パッチによる形式の移行 | gnufood | typed shim を採用 (§5.2)。`serde_json::Value` を経由すると遅く、型の検査も効かない |
| 形式ごとの adapter crate (trustfall_rustdoc) | cargo-semver-checks | 対応形式が 3 世代だけで、hidden item 込みの JSON を前提にする (R-AD-2) |
| 内容アドレスによる版をまたいだ重複除去 | ferrisfetch、BrightX | 実装の複雑さに比べて、ディスクの節約は小さい。容量は GC (R-CA-5) で管理する |
| 公開ライブラリ API として組み込める | gnufood | lib は binary のための内部用で、API の安定を約束しない (crate doc に明記)。層分け (R-ARCH-1) で、将来の切り出しの余地は残す |
| TUI、照会の CLI (`get` / `search`) | ferritin | 利用者はエージェント。人が読む用途は docs.rs で足りる (CLI は v0.14 でやめた) |
| 公開 API の書き出し (`api`) | truesight、cargo-aidoc | CI での公開 API の検査は cargo-public-api で足りる。出力の形式が利用者の CI との約束になる (v0.14 でやめた) |
| 先取りの CLI (`prefetch`)、キャッシュの書き出しと取り込み (`cache export` / `import`) | rust-docsbox | 先取りはサーバーの設定 (R-CA-6) で、隔離環境はミラー (R-CA-8) で足りる。取り込んだ docs.rs の JSON は照合できず、共有のキャッシュに入る (v0.14 でやめた) |
| Prometheus のメトリクス、Docker イメージ、Redis キャッシュ、常駐サービス化 (`install-service`) | KingingWang、cratedex | ローカルの stdio が主で、運用向けの機能は要らない。可観測性はファイルの構造化ログ (R-OBS-1)。共有キャッシュは R-ARCH-7 で将来に備える |
| プラグインへの subagent の同梱 | rustkb | ドキュメントの照会は主エージェントが直接ツールを呼べば足り、subagent に分ける利点が見当たらない |
| item を正規表現で絞り込む | rexplore | `search_items` の順位付けと `fields`、ソースは `search_source` (R-SC-6) で足りる |
| 事前クロールしたスナップショット | Context7 | 任意の crate@version を引ける設計のほうが要件 (G1) に合う |
| elicitation (form・URL の両モード) | MCP 2026-07-28 | v0.14 で確認そのものをやめた (R-SEC-2)。URL モードは、秘密情報も外部の認可も扱わないので、もともと要らない |
| `_meta["anthropic/requiresUserInteraction"]` | Claude Code | 照会のツールに付けると、照会のたびに確認が出る。`build_external_docs` にだけ付ける案も検討したが、headless で使えなくなり、利用者の設定でも外せない (2026-10-16 に確認) ので、確認するかは許可ルールに任せた (R-SEC-2) |
| Skills 拡張 (`io.modelcontextprotocol/skills`) | MCP 2026-07-28 | Claude Code は extensions の client matrix に載っておらず、rmcp も対応していない。R-DIST-5・6 の skill で足りる |
| MCP Apps (UI) | MCP 2026-07-28 | エージェントが主な利用者で、使い道が無い |
| 2025-11-25 以前の版 (`initialize` の handshake) | MCP | 最新版だけに対応する (R-MCP-2)。Claude Code は v2.1.292 以降で、stdio サーバーと 2026-07-28 を既定で交渉する。古い版しか話さないクライアントは対象外 |
