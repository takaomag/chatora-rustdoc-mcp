# chatora-rustdoc-mcp 要件定義 (ドラフト v0.17)

- 作成日: 2026-10-13 (v0.1) / 最終改訂: 2026-10-19 (v0.17)
- 状態: たたき台。既存実装の精読と 2026-10-14 の再検証 (調査文書の三次調査)、M0 (PoC) の結果 ([m0-plan.md](m0-plan.md) の §6) を反映済み。採らなかった候補の良い点は付録 B、根拠は付録 A、経緯は §14。残る未決事項 (§11.2) を PoC・eval・実測で詰めてから v1.0 にする
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
| ツール数は多いほうが良い | **修正** | 数そのものに価値は無い。**「LLM の問い 1 種類にツール 1 個」という粒度**と、ツール同士が重ならないことが本質。Claude Code は MCP ツールを遅延ロード (tool search) するので、ツールが多くてもコンテキスト代は小さい。代わりに、名前と説明文で探されたときに迷わないことが必要 (§6)。tool search を持たないクライアント向けに、主要なツールだけを出す構成を M5 で検討する (R-MCP-12) |
| 説明文を「LLM がどう問うか」基準で書き、否定ルールを入れる | 妥当 | 採用する。説明文は 2,048 文字以内にし、冒頭に要点を置く (Claude Code の現行の上限は 4,096 文字。§6.2) |
| ソース表示 / trait implementors | 妥当・要補足 | implementors の**範囲**を決める必要がある。既存実装はどれも定義元 crate 内だけで、依存グラフ全体の逆引きは先行例が無い (§5.6) |
| feature flags / changelog / examples / README を個別ツールで | 妥当・要補足 | feature は**プロジェクトで有効か**まで突き合わせる。rustdoc JSON には cfg の専用フィールドが無いので、属性文字列の解析と祖先からの継承が要る (§5.4) |
| re-export 解決 (crate をまたぐものを含む) | 妥当・要補足 | rustdoc JSON の `paths` は**定義パス** (private module を含む) なので、そのまま見せると LLM が使えない `use` を書く。公開パスの索引を自前で作る必要がある (§5.5) |
| target triple の考慮とフォールバック | 妥当・要補足 | フォールバック先の具体的な triple まで provenance に載せる (R-TG-2) |
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

Claude Code (および他の MCP クライアント) が Rust のコードを書くときに、**記憶ではなく実際の定義**に基づいて API を使えるようにする。そのために、次のドキュメントを、正確なバージョンで、トークン効率よく、構造化された形で引ける MCP サーバーを作る。

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
| 正準パス (canonical path) | ユーザーが `use` できる公開パスのうち、次の順で選んだ 1 つ: hidden・unstable なモジュールを通るものを除く → 改名 (`as`) の無いものを優先 → 最短 → 辞書順 |
| ホスト版 | 公開サーバーとして動く版。「ferritin のホスト版」は ferritin が公開している MCP サーバー。本サーバーのホスト版は §9.10 の将来構想 |
| 任意 (要件の印) | 実装しなくても、その段階 (M) の完了条件を満たす。既定で無効という意味ではない |
| プロジェクト | Cargo の workspace (workspace に属さない単独の package はその package)。専用 target dir の単位 (R-MCP-3) |
| 既定のプロジェクト | `project` 引数を省いたときに R-MCP-3 の順で決まるプロジェクト。Claude Code を開いたディレクトリの workspace で、そこに Cargo.toml が無ければ直下の子の workspace (複数あればそのすべて) |
| 照会のツール / 外のツール | 外のツールは `build_external_docs` (R-WS-10)、照会のツールはそれ以外のツール。照会のツールが cargo を起動しローカルビルドするのは、既定のプロジェクトとその依存のうち、rust-analyzer の既定 (`cargo check --all-targets`) と同じ範囲だけ (範囲の定義は R-MCP-3。resolver 2 以上では、そのうち `-p` で名指しできない依存を除く。R-WS-5) |
| ローカルビルド | rustdoc で JSON を手元で生成すること。使う toolchain は、プロジェクトの toolchain に `RUSTC_BOOTSTRAP` を crate 名を指定して付けたものか、nightly (R-WS-8) |

---

## 5. 機能要件

### 5.1 データソース

| 対象 | 第一の供給元 | 代替 | 備考 |
| --- | --- | --- | --- |
| std / core / alloc / proc_macro / test | nightly の `rust-docs-json` コンポーネント | static.rust-lang.org の dist から `rust-docs-json` のアーカイブだけを直接取る (R-STD-6) | stable には無い。docs.rs も std の JSON は配っていない (404)。ソース表示には `rust-src` を使う |
| workspace member | ローカルビルド (`--document-private-items`) | 再ビルド中は直近に成功した JSON (stale と明示。失敗したときは `build_failed`) | §5.10 |
| crates.io の依存 crate | docs.rs の rustdoc JSON | ローカルビルド | docs.rs に JSON が無いとき (2025-05-23 より前のビルドは R-DS-3、docs.rs でのビルド失敗は R-DS-6、非対応の形式は R-FV-4) の代替。Cargo.lock に無い版と、実際のビルド target 以外の target での文書 (R-MCP-3) と、`-p` で名指しできない依存 (R-WS-5) は、docs.rs に無ければ外のツールで作る (R-WS-10) |
| path / git / 非公開レジストリの依存 | ローカルビルド | なし | docs.rs には存在しない |
| 引数 `git` で指した git リポジトリの crate | ローカルビルド (仮のプロジェクトの git 依存として。R-WS-5) | なし | 外のツールでだけビルドする (R-WS-10) |
| 依存ではない任意の crate | docs.rs の rustdoc JSON | ローカルビルド (外のツールで、仮のプロジェクトに。R-WS-5) | docs.rs に JSON が無いとき、照会のツールは `docs_unavailable` を返して外のツールを案内する (R-WS-10) |

- R-DS-1: 公開 crate は docs.rs から取るのを既定とする (確定)。依存 crate を常にローカルビルドする方式は、先行実装で遅さと MSRV 失敗のため撤回されている。ローカルビルドを優先するモードは設定で選べるようにする
- R-DS-2 取得手順:
  1. まず形式番号の無い URL `/crate/{name}/{version}/[{target}/]json` を取る。返るのはそのリリースの直近のビルドの形式 (再ビルドされていれば最も新しい形式)
  2. `format_version` を R-FV-3 のとおり先読みする
  3. 返った形式を読めないときだけ、対応範囲の形式番号付き URL (`/json/{n}`) を新しい順に試す。docs.rs はそのリリースで実際にビルドされた形式しか配らないので、効くのは複数回ビルドされたリリースだけ (404 は想定内)。試行中に一時的エラーと 404 が混ざったときの返し方と、展開や解析の失敗で打ち切るかは M1 で決める (v0.16)
- R-DS-3: docs.rs の JSON は、docs.rs が **2025-05-23 以降にビルドした**リリースにしか存在しない。基準は公開日ではなくビルド日で、古いリリースも docs.rs で再ビルドされれば JSON を持つ (serde 1.0.219 は 2025-03 公開だが JSON がある)。有無は R-DS-5 で確かめる。JSON が無ければローカルビルドで補うか、`docs_unavailable` を返す (候補の示し方は R-VR-4)
- R-DS-4 展開: **先頭が zstd のマジックナンバー (`28 B5 2F FD`) である限り展開を繰り返す**。初期のビルドには二重に zstd 圧縮され `content-type: text/plain` で配られるものがある。content-type を信用しない。展開の段数に上限を設け、それを超えるものは壊れているとみなす (初期値は 3 段。v0.16)
- R-DS-5: docs.rs の `status.json` の `doc_status` は HTML のビルド成功を表すだけで、JSON の有無ではない。JSON の有無はその URL を実際に確かめて判断し、`rustdoc_json: available | missing | unsupported_format` として別に返す
- R-DS-6 docs.rs の JSON の URL の扱い (docs.rs のソース `rust-lang/docs.rs@cca98f22` と実測で確認):
  - 形は `/crate/{name}/{ver}/[{target}/]json[/{fmt}][.gz|.zst]`。`{ver}` には `latest` と semver 要件も使える。正規でない版指定は 302 で完全な版にリダイレクトされる。`latest` はリダイレクトされず 200 が返り、本文の `crate_version` を見ないと実際の版が分からない。このため `latest` の URL は使わず、R-VR-3 で解決した完全な版の URL で取る (R-VR-3 と docs.rs の latest の食い違いを避ける)
  - 既定の圧縮は zstd。`.gz` を付ければ gzip で取れる
  - 不明な target や形式は 404、形式番号として解析できない値は 400。版として解析できない値も 400。HTML のビルドに失敗したリリースは JSON も 404
  - 404 は「crate や版が存在しない」と「JSON が無い」を区別しない (M0 で確かめた)。`crate_not_found`・`version_not_found` (§8) は crates.io の sparse index (R-VR-4) で存在を確かめて分ける (M1。v0.17)
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
- R-FV-2: 読み取り互換性を壊した変更ごとに typed shim を用意する。shim は `Crate` から変更されたフィールドまでの経路にある構造体だけを写し、残りは canonical の型を使う。形式 54 未満の `attrs: Vec<String>` は捨てずに解析し、形式 54 以降に該当する variant があればそれに写し、無いものと解析できないものは `Attribute::Other(String)` にする。文字列のまま残すと、derive の分類 (R-TI-2)、must_use / non_exhaustive の表示 (R-SH-6)、`api_diff` の分類 (R-AD-2) が黙って失敗し、形式をまたぐ比較で `must_use_changed` などを誤検出するため。写す範囲は M1 で形式 46〜53 のテストデータを見て決める (例: `#[automatically_derived]`、`#[must_use]`、`#[non_exhaustive]`、`#[repr(...)]`。v0.16)
  - 形式 54〜61 の shim (M0 で実装した。54〜57・60・61 は docs.rs の実データで、58〜60 の `Stability` の写しは手作りの JSON でだけ確かめた (`stability` を持つ JSON は std くらいで、M1 で std の JSON で確かめる)。v0.17): 54〜56 は `ExternalCrate::path` (57 で追加された必須フィールド) が無いので空のパスを入れ、「不明」として扱う。54 の `Attribute::Other("#[macro_export]")` は、55 で追加された `Attribute::MacroExport` に写す (rustdoc-types 0.55 の更新履歴は `MacroUse` と書いているが、型は `MacroExport`)。58〜60 は `Stability::level` の平らな表現を、61 の入れ子の表現に写す
- R-FV-3: パースの前に `format_version` だけを先読みして、使う shim を静的に決める。先読みは、本文の末尾からのバイト走査を主にし (`format_version` は、M0 で docs.rs から取った形式 46・48・53〜57・60・61 のすべてで最後のフィールドだった)、読めなければ全体の走査に落とす。全体の走査も R-FV-8 に従い、ネストの深さに上限を設けない。全体を解析した後に、本文の値と先読みの値を照合し、違えばエラーにする (M0 で決定。v0.17)
- R-FV-4: **対応範囲の下限は 46** とする (Q-4)
  - docs.rs には形式 46 (tokio 1.45.1、2025-05-24 ビルド) が実在し、48・53・54・55・56・57・60・61 も確認済み (49〜52・58・59 は M0 で取った版には無かった。v0.17)
  - **形式 47 は欠番で、存在しない** (rustdoc-types の CHANGELOG: 「format version 47 … was skipped, and will never exist」)
  - 46 → 48 の差は `#[inline]` の属性文字列の表記だけ (migrate_rustdoc_json の `v46.rs`)。よって形式 46 は形式 48 と同じ shim で読めると見込む。テストデータ (R-QA-2) で確かめる
  - 45 以前は意味の変更を含む (44 → 45 で Span の列番号が 1 始まりに、45 → 46 で repr の可視性の扱いが変わった)。docs.rs が JSON を作り始めたのは形式 46 の時期 (根拠は tokio 1.45.1 の 2025-05-24 のビルド。2025-05-23 に形式 45 のビルドが無いことは未確認) なので、45 以前は対象外とする
  - 下限より古い形式は `unsupported_format` を返し、ローカルビルドを案内する
- R-FV-5: canonical より新しい形式は、追加的な変更とみなしてそのまま読んでみる。読めなければ R-DS-2 の手順 3 を試し、それでも駄目なら `unsupported_format` (`found` と `supported` を含む) を返す。**形式の解析失敗を空の結果やログだけで済ませることは禁止する**。読めた場合も、追加的な変更だと確認できていない旨 (例: `format_version 62 is newer than supported 61; results unverified`) を warnings に出す。型を変えずに意味だけを変える変更 (44 → 45 の Span の列番号など) を黙って誤読しないため
- R-FV-6: 元の JSON は元の形式のまま保存し、読み込むときに正規化する。正規化の処理を直しても取り直さずに済む。キャッシュ済みの JSON が非対応形式だった場合の再確認は M1 で決める (v0.16)
- R-FV-7: 新しい形式に対応するときに古い形式のデータを捨てる (フィールドを空にして読めるようにする) ことは禁止する。正規化した結果は、`rustdoc-types` の定義どおりの意味を持たなければならない
- R-FV-8: パーサーは**ネストの深さに上限を設けない** (serde_json の `unbounded_depth` と `serde_stacker` など)。typenum は数百段、diesel も既定の再帰上限を超える。300 段以上のテストデータを持つ
- R-FV-9: shim から canonical 型への変換は、`..` を使わずにフィールドを網羅的に分解する。上流でフィールドが増えたときにコンパイルエラーで気付けるようにする

### 5.3 バージョン解決

優先順位: **明示指定 > Cargo.lock > latest**

- R-VR-1 明示指定:
  - `x.y.z[-pre][+build]` は完全一致として扱う (docs.rs の URL の意味に合わせる。Cargo の `^` ではない)。この違いはツール説明文に明記する (ferritin は `@1.2.3` を caret として扱うので、移行するユーザーが混乱しうる)
  - `^1` / `~1.2` / `1` などの semver 要件も受ける。解決した結果は provenance に `resolved_from: "explicit_req"` として載せる
  - `latest` / `newest` (大文字小文字・前後空白を無視) は `*` の別名とする
  - **解析できない指定は `invalid_argument` エラーにする**。黙って `*` にしない
  - `0.8.9` が完全一致になることを回帰テストで固定する (caret 扱いで実害が出た先例がある)
- R-VR-2 Cargo.lock:
  - プロジェクトの `cargo metadata` の resolve を使う。Cargo.lock を直接パースしない (path / git / patch / replace / workspace を cargo に解決させるため)
  - 例外として、`cargo metadata` が失敗したとき (オフラインモードで手元に `.crate` が無いとき、git の依存の取得に失敗したときなど。`cargo metadata` は Cargo.lock があっても、他の OS 用を含む全依存の `.crate` を取得する。2026-10-14 に確認。v0.15) は、docs.rs の crate の照会を止めないよう Cargo.lock を直接読み、`resolved_from: "cargo_lock_raw"` で区別して warnings に書く。この経路ではローカルビルドしない (範囲を判定できないため。R-MCP-3)。`cargo metadata` の失敗を `build_failed`・`fetch_failed`・`offline_miss` として返すのは (R-WS-7)、ローカルビルドを要する照会だけで、版の解決は Cargo.lock の直読みで続け、失敗の分類を warnings に書く (v0.17)。使うパッケージの範囲、同名の複数版の絞り方、warnings の中身は M1 で決める (v0.16)
  - パッケージは**名前ではなく package id で索引する**。同名の複数バージョンを共存させる
  - **同じ crate が複数バージョンある場合は、黙って選ばない**。対象のプロジェクト (R-MCP-3) の member から直接依存しているものが 1 つに決まれば、それを使う。決まらなければ `ambiguous_version` を返し、候補と各々を使っている依存元を添える
  - バージョンの大小は semver で比べる。文字列比較をしない (`"0.9.0" > "0.10.0"` になる)
  - docs.rs から取ってよいのは `source` が crates.io のパッケージだけ (docs.rs は crates.io の crate しか扱わない)。path / git / workspace のパッケージは、同名の crates.io の crate と取り違えないようローカルビルドに回す
  - `cargo metadata` は毎回起動しなくてよいが、`cargo update` など入力が変わったら必ず作り直す。キャッシュするかとキーの構成は M1 で所要時間を測って決める (キーの候補: Cargo.lock、workspace のルートと各 member の Cargo.toml、`.cargo/config.toml`、workspace 外の path 依存の Cargo.toml の内容ダイジェスト。v0.16)
  - **ユーザーの Cargo.lock を書き換えない**。`cargo metadata` も `cargo rustdoc` も、Cargo.lock が無いか古いと解決し直して書き込み、ネットワークで index を取りに行く。プロジェクトの文脈で cargo を呼ぶときは常に `--locked` を付ける。Cargo.lock が無いか古ければ、cargo は stderr に `because --locked was passed` を含むエラーを出して終了する (どちらも 2026-10-14 に確認)。これを `lockfile_outdated` として返し、利用者かエージェントが Bash で Cargo.lock を更新するよう案内する (文面は §8)。オフラインモードでは `--offline` も付ける。R-WS-5 の仮のプロジェクトは、キャッシュの下にあるサーバー自身のものなので対象外 (そこでの解決はネットワークを使い、オフラインモードでは `offline_miss` にする)
- R-VR-3 latest:
  - yanked と pre-release を除いた最大のバージョン (sparse index の `yanked` を見る)。docs.rs からはこの版の完全な URL で取る (R-DS-6)
  - プロジェクトの依存でない crate なら、`resolved_from: "latest"` と「プロジェクトの依存ではない」旨を載せる
- R-VR-4 差し替え禁止: 解決したバージョンのデータが取れなくても、別のバージョンで代用しない。`docs_unavailable` を返し、JSON がある近くのバージョンを候補として添える
- R-VR-5 crate をまたぐ参照 (§5.5) にも同じ規則を適用する
- R-VR-6 バージョン一覧の取得には crates.io の sparse index (`index.crates.io`) を優先する。結果は TTL 付きでキャッシュする (§9.3)。日次の名前一覧などを使う場合は鮮度を provenance に載せ、そこに無いことを「存在しない」と扱わない

### 5.4 target / features / cfg

- R-TG-1 target の既定値:
  - ローカルビルドでは、プロジェクトの `build.target` (`.cargo/config.toml`。配列なら先頭の要素を既定にして warnings と provenance に出し、ほかの要素は `target` 引数で指す。v0.16)、なければホストの target
  - ホストの target は実行時に `rustc -vV` から取る (サーバーをビルドしたときの target ではない。実行ファイルのパスを指定していれば R-WS-1 で決めた rustc で)。ローカルビルドで `--target` を省くか (R-WS-1) の判定には、これではなく、ビルドに使う toolchain の `rustc -vV` の host を使う (v0.17)。プロジェクトのディレクトリではなく中立のディレクトリ (キャッシュの下) で実行する。ホストの target はプロジェクトの設定によらないため
    - 中立のディレクトリでの `rustc -vV` は、rustup に既定の toolchain が無い環境では失敗する (2026-10-16 に確かめた)。取れずに代わりの値を使ったときは warnings に書く。代わりの値の取り方 (既定のプロジェクトのルートでやり直す、サーバーのビルド時の target を使う、musl を gnu に読み替える、など) は M1 で決める (v0.16)
  - ローカルビルドの前に、使う toolchain の sysroot に `lib/rustlib/<target>` があるかを見て、無ければ cargo を起動せずに `toolchain_missing` を返す (rustup の環境なら `rustup target add` を案内する。rustup の無い環境では rustup のコマンドを案内しない)。このディレクトリの有無が rustup の一覧と一致し、無い target で cargo を起動すると依存のビルドの途中で失敗することを M0 で確かめた (Q-30。v0.17)
  - 独自の target で std を自分で作るプロジェクト (`build.target` が target の JSON 仕様 (`.json`) のもの、cargo の設定に `[unstable] build-std` があるもの) は v1 では対応しない。cargo を起動せずに `docs_unavailable` (`reason: target_unsupported`) を返し、member は利用者が手元で作る方法を案内する。docs.rs の照会は、その crate の docs.rs での既定 target にフォールバックして R-TG-2 のとおり明示する。JSON の仕様は、今の cargo では stable でも nightly でも `cargo tree` からして unstable な指定を求め、文書化には `-Z build-std` も要る (M0 で確かめた。m0-plan.md の §6.2 の B7-8。v0.17)。上の sysroot の判定は、これらに当たらない構成だけで行う
  - docs.rs では、プロジェクトの `build.target` (なければホストの target) でビルドされた版があればそれ、なければその crate の docs.rs での既定 target (`default-target`。未指定なら `targets` の先頭、どちらも無ければ `x86_64-unknown-linux-gnu`)
  - docs.rs が既定でビルドする target は 5 つ (`DEFAULT_TARGETS`: `x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、`aarch64-apple-darwin`、`x86_64-pc-windows-msvc`、`i686-pc-windows-msvc`)。crate は `[package.metadata.docs.rs]` の `targets` / `default-target` / `additional-targets` で変えられる。**`x86_64-apple-darwin` は既定に入っていない**ので、Intel Mac のホストではほとんどの crate でフォールバックが起きる前提で作る
- R-TG-2: target をフォールバックした場合は、**実際に使われた triple** (JSON の `target.triple` から読む。Q-13) を provenance に載せ、warnings に明示する。R-TG-1 で選んだ target で 404 だったことはディスクに記録し、次回の無駄な試行を省く
- R-FT-1: docs.rs の JSON は、どの feature でビルドしたかを記録していない。`.crate` 内の `Cargo.toml` の `[package.metadata.docs.rs]` (`all-features` / `features` / `no-default-features`) から求めて provenance に載せる。あわせて「docs.rs のビルドで偽だった cfg の item は JSON に存在しない」旨を必要に応じて warnings に出す
- R-FT-2 **item ごとの有効 cfg を求める** (Q-3):
  - rustdoc JSON には cfg の専用フィールドが無い。情報は `Attribute::Other` の文字列としてだけ残る
  - 子 item には親の cfg が付かない (`axum::extract::ws` モジュールには cfg があるが、その中の `WebSocket` の `attrs` は空)
  - したがって、**有効 cfg = その item 自身の cfg ∧ 公開パス上の全祖先モジュールの cfg ∧ re-export の連鎖上の各 `Use` の cfg** として求める。公開パスが複数あれば、正準パスの有効 cfg を出し、`also_reexported_at` (R-SR-4) の各パスには、正準パスと違う場合だけそのパスの有効 cfg を添える
  - 情報源の優先順:
    1. `#[doc(cfg(...))]` (形式 46〜61 で表記が安定している)
    2. cfg の痕跡 (trace) 属性。**表記は format_version ではなく実際の文字列の形で判別する**。同じ形式 57 の中でも変化している (`#[<cfg>(...)]`、`#[<cfg_trace>(...)]`、`#[attr = CfgTrace([NameValue { ... }])]` の 3 系統を確認済み。同じ形式 61 でも、stable 1.99 と nightly 1.101 で属性の表記が違うものがあった (`CfgAttrTrace`。M0 の B1。Q-14))。現行の表記は `span: lib.rs:10:7: 10:20 (#0)` のようにファイルパスと位置を含むので、解析はこれを読み飛ばせるようにする
    3. (任意) `.crate` のソースを `syn` で解析した `#[cfg(...)]`。ソースの item と JSON の item の対応づけが要り重いので、v1 は 1・2 と `cfg_source: unknown` で受け、eval で取りこぼしを測ってから採否を決める (v0.16)
  - 出力: `required_features` (`any` / `all` / `not` の構造を保つ)、`other_cfg` (`tokio_unstable`、`target_os` など)、`cfg_source` (`doc_cfg` / `cfg_trace` / `inherited` / `source_parse` / `unknown`)、`enabled_in_project`、`enabled_in_dev_only`
  - `enabled_in_project` は、実際のビルド target (R-TG-1) での通常のビルド (dev-dependency を除く) の feature の組で判定する。dev-dependency の経路でだけ有効なものは `enabled_in_project: false`・`enabled_in_dev_only: true` で示す (出荷するコードで使えない API を「有効」と言わず、テストでは使えると分かるように。確認なしでビルドする範囲 (R-MCP-3) が dev を含むこととは別の基準。v0.15。resolver 2 以上では、dev だけの依存はローカルビルドの範囲から除く。R-WS-5)。組の得方は R-FT-3。`enabled_in_dev_only` は、resolver 2 以上では `-e normal,dev` の組と `-e normal` の組の差で求める。resolver 1 では dev の feature も通常のビルドに統合されるので、常に `false` になる (v0.17)
  - `#[doc(auto_cfg(hide(...)))]` を尊重する
  - **cfg が見つからないことを「feature 不要」と断定しない**。判断できなければ `cfg_source: unknown` とする
  - **一覧系の各項目にも付ける**: `list_module`・`search_items`・`list_methods`・`list_impls`・`list_implementors` の各項目に (`detail=brief` でも)、必要な feature の 1 行の要約 (例: `feature: sync`、`feature: any(rt, rt-multi-thread)`) と `enabled_in_project` (dev の経路でだけ有効なら `enabled_in_dev_only` も) を付ける。「どの feature で有効か」に `get_item` を追加で呼ばずに答えるため (groxide の `[feature: sync]`、truesight の `#[cfg(feature = …)] pub fn …` の 1 行表記に倣う)。feature が不要な項目には付けない
- R-FT-3 ローカルビルドの feature (Q-10):
  - **workspace member**: プロジェクトの通常のビルドで有効な feature の組を `--no-default-features --features ...` で再現する (その組の得方は下記)。その member の Cargo.toml に `[package.metadata.docs.rs]` の `features`・`all-features`・`no-default-features` があれば、その組で文書化する (文書のための組をプロジェクトが Cargo.toml で決められるように。docs.rs と同じ書き方。v0.14)。`enabled_in_project` (R-FT-2) は、この場合も通常のビルドの組で判定する。照会のツールは、この組で確認なしにビルドする範囲を広げない (R-MCP-3)。その組でビルドが壊れたら (相互排他の feature や、システムライブラリの要る optional の依存など)、通常のビルドの組でやり直して warnings に書く。通常の組の範囲に無い依存が増えるときの扱いと、壊れた組を記録して次から使わないかは M2 で決める (v0.16)。metadata から読んだ feature 名も R-SEC-4 で検証する
  - **依存 crate**: feature のフラグを**渡さない**。workspace のルートから `cargo rustdoc -p <dep>@<ver>` を実行すれば、workspace で統合された feature が効く (2026-10-14 に確認: path 依存 b の `#[cfg(feature = "x")]` の item が、member a が有効にした feature x のとおりに出力された)。ただし統合されるのは**通常の依存の辺**の feature だけで、dev-dependency や build-dependency だけが有効にする feature は効かない (resolver 2。2026-10-14 に確認)
  - 依存 crate に `--features` や `--no-default-features` を渡すと、cargo は `cannot specify features for packages outside of workspace` で失敗する (KevinT3Hu の実装はこの方式で、依存の生成が毎回失敗している)
  - `--all-features` を既定にしない (相互排他の feature がある crate で壊れる)
  - 要求した feature と実際に使った feature の両方を provenance に載せる。**`features_effective` と `enabled_in_project` (R-FT-2) を `cargo metadata` の `resolve.nodes[].features` から取ってはならない**。これは dev・build・target 別の依存の feature まで統合した値で、通常のビルドより多い (dev-dependency だけが有効にする feature が入る)。workspace member を `--features <resolve.nodes の値>` で再現すると、同じ理由で通常のビルドより多い feature で文書を作ってしまう
  - 通常のビルドの feature の組は `cargo tree --workspace --target <target> -e normal -f '{p} [{f}]'` で得る (target は実際のビルド target。R-TG-1)。この組は、`cargo rustdoc --lib` が実際にビルドに使う組と、resolver 1 でも 2 でも一致する (M0 の実測は `--target <host>` を付けて行った。付けない形での再確認は M2。Q-31)。target ごとの依存・build-dependency・dev-dependency で feature が分かれる場合も、ビルドにはこの組を使う (Q-20。M0 で決定。v0.17。member の `[package.metadata.docs.rs]` があればそちらを使うのは上記のとおり)。dev-dependency の feature は `--all-targets` のビルドにだけ混ざり、build-dependency の feature は host 向けの別の unit にだけ効く (resolver 2)

### 5.5 パスの解決と re-export

- R-PR-1: 入力として次の形を受け付ける
  - 正準パス、定義パス
  - re-export 経由のパス (`axum::Router`)
  - メソッド・関連 item のパス (`tokio::sync::Mutex::lock`、`Iterator::map`)
  - プリミティブ型とそのメソッド (`slice::sort_by`、`str::len`、`f64::sqrt`)
  - `crate@version::path`
  - 任意 (LLM がその形で問うかを eval で見て採否を決める。受けない形は `invalid_argument` で書き方を案内する。v0.16): trait 修飾のパス (`<Vec<T> as IntoIterator>::into_iter`)、docs.rs の URL、パスの区切りごとの rustdoc の区別子 (`struct@Foo`、`fn@foo`、`mod@x` など、intra-doc link と同じ書き方。名前だけでは曖昧なものを区別する。ferritin の `resolver.rs` に倣う)
  - crate 名のハイフンとアンダースコアは同一視する (`tokio-util` と `tokio_util`)。crate 名の比較と索引のキーを正規化して吸収する
  - 解決の手順: 公開パス索引 (R-PR-2) で見つからなければ定義パス (`paths`) で引く。それでも無ければ、親を同じ手順 (公開パス索引 → 定義パス) で引いてから、子を impl・trait・enum の中でたどる (`paths` に載らない impl の中の item 用。ferritin の 2 段のフォールバック。v0.17)
- R-PR-2 **公開パス索引**: ルートモジュールから `Module.items` と `Use` item (glob・非 glob・crate をまたぐもの) をたどって、item ごとの公開パスの集合を作る
  - `paths` の定義パスを、使えるパスとして見せてはならない (private module を含みうる。dmvk の Issue #5 や、umbrella crate で引けない問題の根本原因)
  - 表示するのは正準パス。定義パスは異なる場合だけメタデータとして返す
  - 公開パスを持たない型が公開シグネチャに現れる場合は `unnameable` として示す
  - `pub use Trait as _;` は名前を作らないので、公開パスにしない (`prelude::_` は `use` できない。M0 で rustdoc_processor との突き合わせで見つけた。どの glob で trait がスコープに入るかを情報として持つかは M1 で決める。v0.17)
  - std の item は公開窓口のパス (`std::iter::Iterator`) で示す。`core::iter::traits::iterator::Iterator` や `alloc::vec::Vec` を見せない
- R-PR-3 `Use` の解決順 (ferritin の実績のある順序):
  1. `id` が無い → プリミティブ型の re-export。種類付きのパス索引で引く (無視すると `bool` や `str` で無関係な crates.io の crate を読みに行く)
  2. `id` がこの crate の `index` にある → その item
  3. `id` が `paths` にある → crate をまたぐ参照。定義パスの逆引き索引で解決する
  4. `source` 文字列は最後の手段 (先頭がローカルの別名でありうる)
  - 循環検出とともに深さの上限 (目安 256) を設ける。glob で互いを re-export しあう crate 群でスタックが溢れた先例がある
  - glob re-export は crate 内のものを再帰的に展開する。crate 外の glob は、相手の JSON がキャッシュにあるか取得できればサーバー側で展開する。展開できなければ `unexpanded_external_globs: [{prefix, source_crate, source_version}]` として返し、次に呼ぶべきツールを hints に示す
  - `#[doc(hidden)]` の item は既定で隠す。`#[doc(inline)]` / `#[doc(no_inline)]` を正しく扱う
- R-PR-4 crate をまたぐ参照: 参照先 crate のバージョンは、プロジェクトの依存グラフを最優先にし、決まらなければ `external_crates[*].html_root_url` (docs.rs のビルドは `https://docs.rs/axum-macros/0.5.1/x86_64-unknown-linux-gnu/` のように、crates.io の名前・版・target を含む。M0 で確かめた。v0.17) を使う。起点 (その問い合わせで最初に引いた crate) と参照元のどちらの `html_root_url` を先に見るかは M1 で実際の crate を見て決める (v0.16)。どれでも決まらなければ latest で解決し、版を推定した旨を warnings に明示する。決めた版のデータが取れない場合は R-VR-4 に従う
  - `html_root_url` は crates.io の名前と crate 内部の名前 (`sha1` と `sha-1` など) の両方で索引する
  - ローカルビルドの JSON は版付きの `html_root_url` を持たないことが多いので、ローカル crate からの参照では依存グラフが必須
  - crate をまたぐ re-export を引かれたら、行き先 (`crate@版` と定義パス) と、行き先を引く次の呼び出しを示す hint を返す (M0 で実装し、Claude Code がそれに従って行き先を引けた。行き先の中身まで返すかは M1 で決める。v0.17)
- R-PR-5 (任意。M4): docs.rs がビルドに使った依存のバージョン (`html_root_url` や span のパスから分かる) とプロジェクトの Cargo.lock の版が違う場合、その依存から来た item (inline された re-export や blanket impl) に warnings を付ける。それまでは、docs.rs 由来の文書は docs.rs のビルド時の依存で作られている旨を provenance で示すだけにする。span のパスから版が分かるかは M4 で確かめる (v0.16)。docs.rs の JSON の自分の item の span は package root からの相対パスで版を含まない (M0 で確かめた。v0.17)
- R-PR-6: 1 回の問い合わせで同じ crate の異なるバージョンが必要になった場合は、黙って片方に寄せず warnings で知らせる
- R-PR-7: パスが曖昧な場合 (同名の item が複数) は候補一覧を返す。型のメンバー (`Type::member`) は、固有の impl → trait の impl → blanket impl の段の順に探し、先に見つかった段で決める (同じ段に複数あれば候補一覧。M0 の実装。初期案。v0.17)。見つからない場合は、まず別の解決経路 (trait 経由・Deref 経由・re-export・同名の別モジュール) を試し、それでも無ければ `did_you_mean` を返す。**候補には類似度の下限を設け、無関係な候補を出すくらいなら候補なしにする**。編集距離はバイトではなく文字で数え、ASCII 以外の名前で閾値が狂わないことを回帰テストで固定する (dmvk はバイト長と文字数を取り違えている)
- R-PR-8: 再帰の深いパス解決・型の描画は、大きなスタック (目安 16MB) を持つ専用のスレッドで行うか、明示的なスタックを使う反復に書き換える。非同期ランタイムのワーカーの既定のスタックで深い再帰をしない (ferritin は 16MB のワーカーで走らせている)

### 5.6 trait / impl / メソッド解決

- R-TI-1 `list_methods`: その型で呼べるメソッドを、出どころ別に返す
  - 固有メソッド
  - trait メソッド (どの trait 由来か、`use` が必要か、required / provided か)。上書きされていない default メソッドも含める
  - blanket impl 由来のメソッド (汎用 trait 由来のものの出し方は R-SH-4)
  - **Deref 先のメソッド** (`String` → `str`、`Vec<T>` → `[T]`。連鎖も追う。M4)
  - プリミティブ型の固有メソッドは core / alloc / std に分かれて定義されているので、すべてを合わせて返す (M4)
  - 固有メソッドと trait 由来のメソッドは M1、Deref 先とプリミティブは M4 で作る (v0.16)
  - `get_item` でも `Type::method` をこれらすべての経路で解決する。見つからない場合は、どの経路を探したかをエラーに含める
- R-TI-2 `list_impls`: 型が実装している trait を返す
  - **分類は構造的なフィールドだけで決める**: `is_synthetic` → auto、`blanket_impl` → blanket、`Attribute::AutomaticallyDerived` → derive、`trait_` が無い → inherent、`is_negative` → negative、それ以外 → 明示的な trait impl。trait の名前で推測しない (`Freeze` や `UnsafeUnpin` のように新しい auto trait が増えるうえ、手書きの `impl Display` を derive と誤認した先例がある)
  - auto trait は条件付きで 1 つにまとめる (例: `auto_traits: {Send: "yes", Sync: "if T: Sync", Unpin: "yes"}`)。「X は Send か」に必ず答えられるようにする
  - 不安定な auto trait (`Freeze`、`UnsafeUnpin`) は既定で隠す
  - 条件付き実装は where 句を描画した文字列で返す
  - 公開されていない trait の impl (std 内部の `SpecExtend` など) は出さない
- R-TI-3 `list_implementors`: trait を実装している型を返す。`scope` で範囲を選ぶ (依存グラフ全体の時間と容量は Q-5)
  - `crate`: trait を定義している crate の中だけ。既定
  - `dependency_graph`: プロジェクトの依存グラフ全体。**先行実装が無い差別化点** (ferritin もディレクトリを予約しただけ)。`Trait.implementations` と、他 crate の impl に対する逆引き索引を合わせる
  - `for_` が `ResolvedPath` 以外 (`&T`、`[T]`、タプル、ジェネリクスの `T`) の impl も扱う
  - 同名の型が複数ある場合は正準パスで区別する
- R-TI-4 impl と実装者を引くための逆引き索引を持つ
  - **パスを持たない型 (プリミティブ、スライス、配列、`&T`、タプル、`dyn Trait`) も対象にする** (ferritin の索引はこれらを落としている)
  - 並びは決定的にする
  - 索引の構成 (案: 型ごとの固有 impl・trait impl、trait ごとの実装者 (negative impl を含む)、子 item → 親 item (trait の item と union のフィールドを含む)) と、あらかじめ作ってキャッシュするかは、M1 で読み込み時間を測って決める (v0.16)
- R-TI-5 trait を表示するときは、各メソッドに required / provided を付け、次の境界情報を出す: `sealed`、`dyn_compatible`、negative impl の有無。`sealed` は検出できたときだけ根拠付きで出し (例: 公開されていない supertrait を持つ)、検出できないことを「封印されていない」と断定しない (private な型を引数に取るメソッドなど、ほかの封印の仕方は拾えないため。検出の方法を増やすかは M1 で決める。v0.16)。型には `non_exhaustive` と `has_private_fields` を出す

### 5.7 検索

- R-SR-1 対象: item 名、**`#[doc(alias = "...")]` の別名**、ドキュメント本文。kind で絞り込める (上位 N 件を切り出す前に。R-OUT-4)
  - 別名を索引すると、`HashMap` を「dict」で、`Vec::push` を「append」で探すような問いに答えられる (syphar の実装に倣う)
  - 引数 `fields` で照合する欄 (`name` / `alias` / `docs` / `path`) を選べるようにする。既定はすべて (ruskel の `search_spec` に倣う。`signature` の欄は下のシグネチャ検索を入れたときに足す。v0.16)
  - **crate に同梱された散文の検索** (M4 で検討。任意): crate の README と `docs/` の散文を検索できるようにする (rust-knowledge-mcp に倣う)
  - **シグネチャを対象にした検索** (M4 で検討。任意): 引数や戻り値の型で探す (例: 「`Result<Vec<u8>>` を返す関数」。ruskel の `search_spec: signature` に倣う)
- R-SR-2 範囲: crate 1 つ / workspace のみ / workspace + 直接依存 / 依存グラフ全体。**既定は workspace + 直接依存**。`search.scope = crate` を既定にしたとき、crate の引数が無ければ `invalid_argument` にする。初回は範囲の crate の取得が要り、R-WS-6 の時間の上限で `partial` (R-SR-5) になりうる。起動時の先取りの設定 (R-CA-6) で避けられることを hints で示す
- R-SR-3 順位付け (ferritin の調整済みの方式を基本とする。原則は、名前の完全一致を上位にする、最後の語を前方一致で展開しない、並びを決定的にする、の 3 つ。下の数値と、段の順・減点する語・被参照数・量子化は初期案で、M1 の eval (R-QA-6) で決める。v0.16):
  - BM25 (k1 = 1.2、b = 0)、Lucene 型の IDF (負にならない)、検索対象の crate 全体での統計
  - 重み: item 名 20、ドキュメントの第 1 段落 3、残り 1。祖先のパス名を距離に応じて 3 / 2 / 1 で加える (`hashmap insert` で `HashMap::insert` が引ける)。コードブロックは索引しない
  - トークン化: CamelCase (頭字語を含む)・snake_case・kebab-case・`::` で分割し、複合語そのものも残す
  - **最後の語を前方一致で展開しない**。エージェントは語を最後まで打つので、展開すると頼んでいない item が混ざる (ferritin はエージェント向けの検索で展開を切っている: `QueryCompletion::Complete`)
  - **名前の完全一致を上位**にする段を設ける (散文の多い説明ページが本物の型より上に来るのを防ぐ)。段の順: パスの一致 (クエリが `sync::Mutex` のようなパスで、item の公開パスの末尾と `::` の区切りごとに一致する) > item 名の完全一致 (クエリ全体が `Mutex` と一致する) > 最後の語の一致 (複数語のクエリ `async mutex` の最後の語が item 名と一致する) > 別名 (`doc(alias)`) の一致 > 部分一致
  - 正準パスを優先し、`__`・`_private`・`internal` を含むパスを減点する
  - 被参照数 (intra-doc link の入次数) を権威として掛ける
  - 同点はスコアを量子化したうえで crate 名・パスで並べ、決定的にする
- R-SR-4 結果: item ごとに重複を除き、正準パスで示す (他の公開パスは `also_reexported_at`)。`#[doc(hidden)]`・private module・unstable を通るパスは既定で除く。数値の rustdoc Id は出さない
  - 既定の件数は固定値で切らず、**スコアの落差で自動的に打ち切る** (ferritin の `rustdoc-mcp/src/tools/search.rs:103-116` に倣う。初期値は、最上位の 5% 未満、直前の半分未満、すでに返した結果のスコアの累積が全ヒットのスコアの合計の 30% を超えた、のいずれかで切る。少なくとも 1 件は返す)。`limit` を指定したらそれに従う。打ち切った場合も `page` (R-OUT-3) で続きを取れるようにする
- R-SR-5 範囲内にビルド中・失敗・未取得の crate (既定のプロジェクトの外で `build_external_docs` の記録が無いものを含む) がある場合は `partial: true` と該当 crate・理由を warnings に載せる。0 件を「存在しない」と誤読させない
  - 0 件のときは、item 名との編集距離による `did_you_mean` を、R-PR-7 と同じ類似度の下限付きで返す (綴りの誤りを許す。possdog の fuzzy 検索と snowmead の Tantivy の FuzzyTermQuery の利点を、0 件のときに限って取り込む)
- R-SR-6 crate そのものを探す検索は別ツール (`search_crates`) に分ける。説明文とキーワードには語幹処理をかけ、語ごとの得点は名前・説明・キーワードの**最大値**をとる (合計すると有名 crate が上位から落ちる)。0 件のときは「語を減らす・言い換える」案内を返す
  - 選定の材料は、検索の API の応答 (DL 数・説明・リポジトリ。1 回で返る) と、結果ごとの sparse index (最新安定版・最終リリース日・MSRV) から作り、結果ごとに API を呼ばない。被依存数は `search_crates` では返さず、`crate_metadata` で返す (crate ごとに API が要り、R-NW-1 の間隔で結果の件数の秒数だけ待たせるため)

### 5.8 ソース表示

- R-SC-1: item の span から該当範囲を返す。前後に何行付けるかは指定できる
- R-SC-2: ソースの取得元
  - 依存 crate: `$CARGO_HOME/registry/src` の、そのパッケージの source id に対応するディレクトリに既にあればそれを優先し (別のレジストリの同名同版を使わない。git の依存は `$CARGO_HOME/git/checkouts`)、無ければ `.crate` アーカイブ (static.crates.io) を取得・展開してキャッシュする (docs.rs の crate も同じ)
  - std: std の JSON と同じ toolchain (同じ commit-hash。R-STD-6 で取ったものなら同じ dist の日付) の `rust-src` から読み、span の `/rustc/<hash>/library/…` をそこへ写し替える。版の合う `rust-src` が手元に無ければ導入コマンド付きの `toolchain_missing` を返し、別の版のソースを返さない (行がずれて別の箇所を読むため)。dist の manifest の `[pkg.rust-src.target."*"]` (2026-10-14 に確認) から sha256 を検証して取得する経路は任意とする (v0.16)
  - workspace: そのままのファイル
  - docs.rs の span のパス: 自分の item は package root からの相対パス (`src/sync/mutex.rs`。M0 で確かめた) なので、その crate のソースのディレクトリに付ける。依存から inline された item などでビルドホストの絶対パス (`/opt/rustwide/cargo-home/registry/src/*/<name>-<ver>/...`) が入る場合は、crate 名と版に写し替える (確かめていない。v0.17)
  - static.crates.io から取得した `.crate` は、sparse index の `cksum` (sha256) と照合し、合わなければ破棄して `network_error` (`retryable: true`) を返す
  - アーカイブ (`.crate` の tar.gz、R-STD-6 の tar.xz、R-AU-3 の OSV の zip) の展開では、絶対パスや `..` を含むエントリ、symlink・hardlink・デバイスのエントリを拒否し、展開先をキャッシュの下のそのアーカイブ用のディレクトリに限る。展開後の総サイズ (`limits.extract_max_size`。R-NW-3 と共通) とファイル数 (初期値は 10 万。v0.16) に上限を設け、超えたら部分的な展開を消す。`.crate` と std のアーカイブは `docs_unavailable` (`reason: too_large`) を返し、OSV の zip は前回のミラーを使って `audit_dependencies` を続け、warnings に書く (v0.15)
  - `stale: true` の workspace の JSON の span でソースを読むときは、行がずれうる旨を warnings に出す
- R-SC-3: macro が生成した item は span が無いか、macro の呼び出し位置を指す。その旨を明示する
- R-SC-4: **読めるのは既知のソースルートの下の、パッケージの中身だけ**。span のファイル名が絶対パスや `..` を含む場合も、正規化してソースルートと照合してから読む。manifest 由来のパス (`readme`、`[[example]].path`、`build`) も同様に検証する。manifest 由来のパスと span は、その crate 自身のソースルートとだけ照合する (第三者の crate の `readme = "../../<別のプロジェクト>/.env"` のように、他のルートの下を指させない)
  - **パッケージの中身** (v0.14): workspace と path 依存では、ターゲット (build script を含む) のソースのディレクトリの `.rs`、前回のビルドの dep-info に載った入力ファイル (`include_str!` で読むものなど。R-WS-4)、`Cargo.toml`、manifest が指す readme と example だけ。registry と git の依存では、`.crate` (git なら checkout) に入っているファイルだけ。std は `rust-src` の中だけ。ドットファイル (`.env` など) は除き、`readme` は文書の拡張子 (`.md`・`.markdown`・`.txt`・`.rst`・`.adoc`) のものだけを受ける。Claude Code の Read の deny やサンドボックスの読み取りの制限を、このサーバー経由ですり抜けさせないため (rust-analyzer や LSP のツールはファイルの中身をエージェントに返さないので、このサーバーにしか無い経路になる)
  - symlink は、行き先をたどった実際のパスで判定する。実際のパスが既知のソースルート (問い合わせの対象のプロジェクトの workspace のルートと path 依存のルート、`$CARGO_HOME/registry/src` のそのパッケージの source id のディレクトリの `<name>-<ver>` と `$CARGO_HOME/git/checkouts` の対象の crate、展開した `.crate`、`rust-src`) の中なら読み (workspace の中の正当な symlink は読める)、外を指していれば読まずに理由を返す (第三者のリポジトリ・git の依存・crate に入っている、`~/.ssh/id_rsa` などを指す symlink で読み出させない)
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
  [RUSTC_BOOTSTRAP=<crate>[,<crate>…]] cargo [+<toolchain>] rustdoc --locked [--offline] -p <name>@<version> (--lib | --bin <bin>) [--target <target>] \
      --message-format=json -- -Z unstable-options --output-format json [--cfg docsrs] [--document-private-items] [--document-hidden-items]
  ```

  - `+<toolchain>` は、サーバーが自分で選んだ toolchain (nightly など。R-WS-8) のときだけ付ける。プロジェクトの toolchain では付けず、rustup にカレントディレクトリ (R-WS-2) から決めさせる。rustup の無い環境では付けない (v0.17)
  - `--target` は、target (R-TG-1 で決めた値か、照会・外のツールで指した値) が host と違うときと、プロジェクトに `build.target` があるときに付ける。target が host で `build.target` も無いときは付けない (Q-30。M0 で決定。v0.17)。「`build.target` がある」は、cargo が実際に読む設定 (カレントディレクトリから祖先と `$CARGO_HOME` のすべての `config.toml`、環境変数 `CARGO_BUILD_TARGET`) で判断する。host は、ビルドに使う toolchain の `rustc -vV` の host とする (R-TG-1 の中立のディレクトリの値とは違いうる)。`--target` を省いたビルドでは、JSON の `target.triple` を記録の target と照合し、違えば warnings を出して実際の値で記録する。R-TG-1 で代わりの値を使ったときは省かない。理由: `--target` を付けると、`RUSTFLAGS` と `build.rustflags` が host 向けの build script と proc-macro に効かなくなり、それらが生成する item が利用者の普段のビルド (`--target` なし) と違ってくる (M0 で確かめた)。`RUSTFLAGS` は rustc にだけ効き、文書化する crate の rustdoc にはどちらでも効かない。`build.target` が配列だと、`--target` の無い `cargo rustdoc` は `extra arguments to rustdoc can only be passed to one target` で失敗する (2026-10-16 に実測)
  - 出力は、`--target` を付けたときは `<target dir>/<triple>/doc/`、付けないときは `<target dir>/doc/` に出る。proc-macro の crate は `--target` を付けても host 用に作られ、JSON は `<target dir>/doc/` に、`target.triple` は host で出る (cargo の artifact のメッセージの `filenames` も空。2026-10-16 のレビューで確認)。他の target での文書化では、proc-macro の member は host の文書として記録する (v0.15)
  - `RUSTDOCFLAGS` は使わない。すべての crate に効いて fingerprint を変えるうえ、プロジェクトの `build.rustdocflags` を上書きする
  - `-p` には版付きの pkgid を渡す (同名の複数版で曖昧にならないように)
  - `--lib` / `--bin` は `cargo metadata` のターゲット情報から選ぶ。lib を持たない crate と proc-macro crate も扱う
  - 出力ファイルの場所は、パッケージ名ではなく**ターゲット名** (`[lib] name`) から求める
  - **lib と bin の crate 名が同じだと、JSON は同じファイルに出て、後のビルドが上書きする** (パッケージ `my-app` の lib と bin はどちらも `target/doc/my_app.json`)。bin の rustdoc は lib を HTML でも文書化する副作用があり、cargo は `output filename collision` を警告する (2026-10-14 に確認)。ターゲットごとに 1 回ずつビルドし、終わるたびに JSON を作業用ディレクトリからキャッシュへ移す (R-WS-6)
  - **`cargo doc --output-format json`** (unstable。cargo#17025 で 2026-05-27 に追加) は使わない (Q-17。M0 で決定。v0.17)。nightly を使うときに依存をまとめて文書化する用途の候補として M2 に残す。M0 で `cargo rustdoc` と比べた結果 (m0-plan.md の §6.2 の B2):
    - 1 crate の所要時間は同じくらい。2 回目は `cargo doc` が全部 fresh で速く (`cargo rustdoc` は文書化する crate の rustdoc を毎回やり直す)、依存をまとめて文書化できる (146 crate で 38 秒)。cargo#16773 以降、JSON は unit ごとに `build/<pkg>/<hash>/out/<name>.json` に出て、`target/doc/` へ uplift される
    - **同名で版の違う依存は、片方しか文書化されない**: 両方を `-p` で指すと `document output filename collision` で失敗し、`-p` なしだと古い方が警告なしに飛ばされる (2026-10-14 の「別々のファイルに出る」は、2026-10-19 の nightly cargo では再現しなかった)
    - **R-WS-8 の既定 (crate 名を指定した `RUSTC_BOOTSTRAP`) と両立しない**。使うには nightly の cargo か、プロセス全体への `RUSTC_BOOTSTRAP=1` が要る (stable の cargo は、crate 名を指定すると「`-Z` は nightly の Cargo でしか受けない」と拒否する。2026-10-14 に確認)
  - `--cfg docsrs` は、既定で、文書化する crate の最終の rustdoc にだけ渡す (Q-11。docs.rs と同じ。`#[doc(cfg(...))]` が JSON に残り、R-FT-2 の第 1 の情報源が使える)。渡してビルドが壊れたら (`docsrs` のときだけ使う nightly の機能の改名など)、渡さずにやり直し、warnings に書く。設定で常に渡さないこともできる。壊れた crate を記録して次から渡さないか (案: 版と toolchain を鍵に記録する。groxide の `docsrs-failures.json` と同じ考え方。M0 で測ったやり直しの頻度は、このリポジトリの依存 136 個中 1 個で、原因は `--cfg docsrs` を前提にした crate 自身の feature がプロジェクトの組で有効でないことだった。v0.17) と、workspace member の `[package.metadata.docs.rs]` の `rustdoc-args` から `--cfg` を取り出して渡すか (案: `--cfg <値>` と `--cfg=<値>` の形だけを、cfg の文法に合うものに限って渡す。v0.14) は M2 で決める (v0.16)
  - cargo・rustdoc・rustc の実行ファイルは `rustup run` 経由のほか、設定で絶対パスを指定できるようにする (Nix・distro の cargo。rustc は `rustc -vV` に使う。R-TG-1・R-WS-8)。rustc と rustdoc のパスは子の cargo に `RUSTC`・`RUSTDOC` として渡す。パスを指定したときは toolchain を切り替えず (R-WS-8 の nightly への切り替えをしない)、出る形式が対応範囲の外なら `unsupported_format` を返し、rustup のコマンドを案内しない。一部だけ指定されたら、残りは同じディレクトリから探す (rustc と rustdoc は同じ toolchain でないと、依存のメタデータが読めず `E0786` で壊れる。cargo は別の toolchain でもよい。M0 で確かめた)。見つからなければ、起動時にログで知らせ (SessionStart の hook でも知らせるかは R-DIST-5 で M5 に決める)、照会ではエラーにする。rustup が無くパスも指定しない環境 (Nix など) では、PATH の cargo・rustc・rustdoc を使い、toolchain を切り替えない (Q-30。M0 で決定。v0.17)
- R-WS-2: **専用の `CARGO_TARGET_DIR`** をキャッシュの下に workspace ごとに作り、**永続化する** (依存のコンパイル結果を再利用するため)。ユーザーの `target/` は使わない (cargo・rust-analyzer とのロック競合、別の toolchain の生成物の混入)。中間生成物の場所 (`build.build-dir`) も `CARGO_BUILD_BUILD_DIR` で同じ専用の場所に向ける (プロジェクトの `.cargo/config.toml` の `build-dir` は `CARGO_TARGET_DIR` では上書きされず、プロジェクトの中に書かれる。2026-10-15 に実測)。ビルドごとの使い捨て dir も使わない (毎回コールドビルドになる)
  - 使えない target dir (止めなかった子孫がいるもの。R-WS-6) は使わず、別の名前で作り直す。古い方は GC で消す。名前の決め方とほかのプロセスへの伝え方は Q-29 (v0.16)
  - **子プロセスの cargo・rustc を起動するディレクトリ**: cargo と rustup は、`.cargo/config(.toml)` と `rust-toolchain(.toml)` をカレントディレクトリから祖先へ探し、`--manifest-path` はこの探索に効かない (2026-10-15 に実測)。そこで次のように決める
    - プロジェクトの文脈で起動するときは、カレントディレクトリを workspace のルートにする (利用者が手元で `cargo build` するのと同じ設定が効く)。workspace のルートより上の祖先の設定も効くので、鮮度の鍵 (R-VR-2・R-WS-4) の `.cargo/config.toml` には、cargo が読む全階層の設定 (`$CARGO_HOME` のものを含む) を入れる
    - 仮のプロジェクト (R-WS-5)、引数 `git` の取得、ホストの target を調べる `rustc -vV` (R-TG-1) では、カレントディレクトリをキャッシュの下のそのディレクトリにする (祖先の設定の有無は確かめない。共有の場所の下にキャッシュを置かないよう README で案内する。R-SEC-2。v0.14)
  - 専用の target dir でビルドディレクトリのロックは避けられるが、`~/.cargo` のパッケージキャッシュのロックは rust-analyzer の `cargo metadata` / `cargo check` と共有する。stderr に `Blocking waiting for file lock` が出たら、`status: building` に `building_reason: waiting_for_lock` を付けて返す (stderr の文字列照合の例外。一覧は R-WS-7)
  - `cargo metadata` を毎回起動しないことで (R-VR-2)、ロックを取る回数を減らす。nightly の `-Z fine-grain-locking` は、nightly の cargo を使う経路でだけ使える (既定の stable では cargo の `-Z` を受けない。R-WS-8)。その経路で使うかは、プロセスをまたぐロックを入れる M2 で評価する (M0 では評価しなかった。v0.17)
  - Windows の MAX_PATH (260 文字) を超えないよう、target dir のパスは短く保つ (workspace の識別子はハッシュの短い接頭辞にする)。git worktree や eval の一時ディレクトリごとに target dir が増えすぎないよう、GC (R-CA-5) の対象として最終使用日時で管理する
- R-WS-3: `--document-private-items` は workspace member のビルドにだけ付ける。応答には可視性 (`pub` / `pub(crate)` / `pub(in path)` / private) を必ず付ける (表示の規則は R-SH-5)
- R-WS-4 鮮度: **JSON が存在すること・バージョンが同じことを鮮度の根拠にしない** (先行 5 実装がすべてここで誤っている)。内容ダイジェストの鍵は、workspace member だけでなくローカルビルドするすべてのパッケージに適用する (workspace 外の path 依存、git 依存、docs.rs に JSON が無い registry 依存、R-WS-5 の仮のプロジェクト)。path 依存はそのパッケージ自身のソース・dep-info・Cargo.toml も鍵に含める
  - 入力の内容ダイジェストはビルドの前に取り、ビルドの後に dep-info で増えたファイルを足す。ビルドの前より新しい mtime のファイルがあれば、次回は鮮度切れにする (ビルドの間にエージェントが編集した内容を、古い JSON の鍵に入れないため。v0.15)
  - member ごとに次の入力の**内容ダイジェスト**を鍵にする: `cargo metadata` が示すその member の全ターゲットの `src_path` 配下、`build.rs`、`Cargo.toml`、workspace の `Cargo.lock`、`.cargo/config.toml`、path 依存 (workspace の外にあるものを含む) のソース、feature の組、rustdoc の引数、`RUSTC_BOOTSTRAP` の値 (のダイジェスト。値で derive が生成した item の span が変わることを M0 で確かめた。member には値の全体を、member 以外のパッケージにはその部分木 (そのパッケージと依存先) に現れる crate 名だけの部分集合を入れる。値は解決結果の全 crate 名なので、全体を入れると無関係な依存を足すだけで全部の記録が鮮度切れになるため。R-WS-8。v0.17)、target、**実際に生成に使った toolchain** の `rustc -vV`、子に渡す環境のうちビルドに効く変数 (`RUSTFLAGS`・`RUSTDOCFLAGS`・`CARGO_BUILD_*`・`RUSTC_WRAPPER` など。サーバーの環境は Claude Code の起動時のもので、direnv などでシェルにだけ入る値は含まれない)。feature の組以降は member 以外のパッケージにも適用し、さらに source id、その依存から先の解決結果 (依存先の package id と feature) のダイジェスト、ローカルビルドに効く実効の cargo の設定 (`[source]`・`[patch]`・`build.rustdoc`・`build.rustc`・`build.rustc-wrapper`) のダイジェスト、使った cargo と rustdoc の実行ファイルのパスを入れる (ソースの置き換えや実行ファイルの設定で、別のソースや別の実行ファイルで作った JSON が、他のプロジェクトに配られないように)
  - mtime は一次の絞り込みにだけ使う
  - `src_path` 配下だけでは、`include_str!` / `include_bytes!` で読むファイル (`#![doc = include_str!("../README.md")]` など)、`#[path]` で指したファイル、build.rs が生成するコードの変化を拾えないので、前回のビルドの dep-info (cargo が unit ごとに書く `.d` ファイル) に載った入力ファイルも鍵に含める。ただし `cargo rustdoc` は、文書化する crate 自身の dep-info を書かない (依存の check unit だけが書く。2026-10-14 に確認)。rustdoc に `--emit=dep-info` を渡すと、その crate の dep-info (JSON と同じ場所。`--target` を付けたら `<target dir>/<triple>/doc/<crate>.d`、付けなければ `<target dir>/doc/<crate>.d`。R-WS-1。`include_str!` で読んだファイルも入る) が書かれるが、JSON は出なくなり、`--emit` のほかの値は JSON の出力と併用できない (2026-10-16、stable 1.99 + `RUSTC_BOOTSTRAP` で確認)。そのため dep-info は別の呼び出しで取る: 同じ引数 (`--cfg docsrs` を含む) に `--emit=dep-info` を付けて rustdoc をもう一度実行する (M0 で決定。v0.17)。費用はその crate の rustdoc 1 回分 (M0 で member 0.5 秒、tokio 0.9 秒。依存は fresh のまま)。JSON は書き換えないので、JSON を移した後に実行してよい。その crate の `cargo check` の dep-info は、依存なら作り直さずに使えるが、`#[cfg(docsrs)]` のときだけ読むファイルを取りこぼす (tokio では 250 件のうち 5 件) ので使わない
  - ワークスペースのルートの `src/` だけを見る方式は、仮想 workspace で誤る
  - 再ビルド中は、直近に成功した JSON を `stale: true` と `built_at` 付きで返す (R-WS-6)。失敗したときは R-WS-7 のとおり `build_failed` を返す
- R-WS-5 依存 crate のローカルビルドは、必ずプロジェクトの workspace と Cargo.lock の文脈で行う。`~/.cargo/registry/src` の中で cargo を実行しない (Cargo.lock の生成・依存の再解決・ネットワークアクセスが起きる)。`-p` で cargo が失敗する依存は、照会のツールではローカルビルドせず `docs_unavailable` (`reason: build_unsupported`) とし、docs.rs に JSON があればそれを、ほかの target でだけ使う依存ならその `target` の指定を、それ以外 (docs.rs に JSON の無い crates.io の依存、path・git の依存) は `build_external_docs` を案内する。外のツールは、その依存 (`crate@version`、path・git ならその指定) だけに依存する仮のプロジェクト (下記) で文書化し、feature の組がプロジェクトの組と違いうる旨を provenance と warnings に出す (v0.17)。resolver 2 以上 (edition 2021 以降の既定) では、dev-dependency だけ・build-dependency だけ・ほかの target でだけ使う依存と、proc-macro からしか使われない依存 (syn・quote・proc-macro2 など) を `-p` で名指しすると、cargo の feature resolver が panic する (`did not find features for … NormalOrDev`。stable 1.99・nightly 1.101 の両方で M0 で確かめた。このリポジトリの依存 146 個のうち 10 個が当たった。M0 の実測はどれも `--target <host>` を付けて行った。付けない形 (R-WS-1) での再確認は M2)。resolver 1 では起きない。そこで resolver 2 以上の workspace では、member から target 向けの通常の辺でたどれない依存 (proc-macro の下にしか無いものを含む) を `cargo tree` (R-MCP-3) で起動前に判定し、cargo を起動せずに返す。stderr の panic の文言は照合しない (Q-30。M0 で決定。v0.17)
  - **プロジェクトの Cargo.lock に無い版や crate** をローカルでビルドする場合は、外のツール (R-WS-10) で、キャッシュの下に**仮のプロジェクト** (対象の `crate = "=x.y.z"`、git なら `crate = { git = "…", rev = "…" }` だけに依存する Cargo.toml) を作り、その中で `cargo rustdoc -p <crate>@<ver>` を実行する。target は、引数 `target` があればその値、無ければホストの target とし、`--target` を付けるかは R-WS-1 の規則で決める (仮のプロジェクトはキャッシュの下で起動するので、プロジェクトの `build.target` は効かない。プロジェクトの組み込み向けの `build.target` で、std の要る crate を作って失敗しないように。v0.17 で書き方を改めた。照会のツールも、依存でない crate を `target` なしで引くときはホストの target で探す。v0.15)。必要な feature は仮のプロジェクトの Cargo.toml の依存の指定に書く (依存への `--features` は拒否されるため。cargo-semver-checks の `generate.rs` の方式)。feature の既定は `[package.metadata.docs.rs]` の指定 (R-FT-1)、無ければ default feature とし、使った組を provenance に載せる。`[package.metadata.docs.rs]` の組でビルドが壊れたら、default の組でやり直して warnings に書く (壊れた組を記録するかは R-FT-3 の member と同じく M2 で決める)。外のツールに `features` 引数は v1 では持たせない (v0.15)。ユーザーのプロジェクトの Cargo.lock は変えない。仮のプロジェクト (とその target dir) は、専用 target dir と同じ GC (R-CA-5) の対象にする。仮のプロジェクトの Cargo.toml は TOML のライブラリで組み立て、引数や metadata の文字列を連結しない
- R-WS-6 長時間のビルド (Claude Code の実際の挙動を前提にする):
  - 照会のツールは、既定のプロジェクトのキャッシュミスのとき暗黙にビルドする (既定のプロジェクトの外は R-MCP-3)。**task ID をポーリングさせる専用ツールは作らない**
  - ビルド中は `notifications/progress` を送る (Claude Code では表示され、アイドルタイムアウトがリセットされる)。送るのは、要求に `_meta.progressToken` があるときだけで、その値を付ける (MUST)。`progress` は単調に増やす (MUST。完了した compile unit の数や経過秒数を使い、`total` は分かるときだけ付ける)。`message` には人が読める状況 (例: `building serde 1.0.219 (12/80)`) を入れる。間隔は 1 秒以上 30 秒以内
  - 進捗通知は、その要求に結果 (`status: building` を含む) かエラーを返した時点、またはキャンセルを受けた時点で止める (MUST)。singleflight で共有するビルドの進捗は、待っている要求ごとに、応答を返すまでの間だけ、その要求の token で送る
  - 1 リクエストあたりの時間の上限 (`limits.request_timeout_secs`、既定 30 秒。R-ARCH-6) を超えたら待つのをやめ、`status: building` と `retry_after_secs` を返す。古い結果は R-WS-4 のとおり一緒に返す
  - Claude Code は tasks 拡張に対応しておらず、2 分を超える呼び出しはバックグラウンドに回されるので、ブロックし続けない
  - 同じ対象への同時のビルドは 1 つにまとめる (singleflight)。**プロセスをまたぐファイルロック** (R-CA-4) も取る (複数の Claude Code セッションが同時に動く)
  - ビルドはサーバー全体のロックを取らず、他の crate への問い合わせを止めない
  - キャンセルされたら子プロセスを kill する。ただし singleflight で共有しているビルドは、待っている要求がすべてキャンセルされたときだけ kill する。一度 `status: building` か `retry_after_secs` 付きの `partial` (R-OUT-9) を返したビルド (裏で続けるもの。R-WS-10・R-ARCH-6) は切り離した状態とし、キャンセルでは止めない。止まるのは 1 回のビルドの最大時間 (下記) とサーバーの終了だけ (v0.15)。キャンセルされた要求には、以後は結果も進捗通知も送らない (MUST NOT)。未知の ID や応答済みの要求へのキャンセルは無視する。HTTP でのキャンセルの扱いは M6 で決める (v0.16)。サーバーから `notifications/cancelled` を送りうるのは `subscriptions/listen` を閉じるときだけ (扱いは R-MCP-16)
  - 将来の任意 (M6 で検討): tasks 拡張を宣言したクライアントには、ローカルビルドを伴う `tools/call` で task を返してよい。プロトコルの機能なので、「ポーリング用のツールを作らない」方針とは矛盾しない。宣言していないクライアントには `status: building` を返す (v0.16)
  - 1 回のビルドの最大時間 (`build.timeout_secs`。既定 30 分で、Claude Code の stdio のアイドルの上限に揃える) を超えたら子プロセスを止め、`build_failed` (理由 `timeout`。一時的な失敗。R-WS-7) を返す
  - 巨大な crate の docs.rs からの取得と索引の作成にも、同じ進捗通知と `status: building` (`building_reason: indexing`) を適用する。`cargo metadata` の初回の依存の取得 (R-VR-2) も同じ (`building_reason: fetching`)
  - stdio でもリクエストは並行に処理する。JSON の解析や索引の作成など CPU を占める処理は専用のスレッドで行い、キャッシュ済みの問い合わせを止めない。同じ crate の解析は 1 つにまとめる
  - **サーバーの終了時も子プロセスを確実に止める**。Claude Code は Unix では SIGINT → 100ms 後に SIGTERM → さらに 400ms 後に SIGKILL を送り、Windows では stdin を閉じて 300ms 後にプロセスツリーを終了する
    - 子の cargo は Unix では新しいセッション (`setsid`。プロセスグループも新しくなる)、Windows では Job Object (閉じたときに中のプロセスをすべて止める設定) に入れ、stdin の EOF・SIGINT・SIGTERM を受けたらまとめて止める
    - 子が利用者に尋ねて止まらないようにする: Unix では `setsid` で制御端末を外す (git の資格情報や ssh の host key・パスフレーズの入力が `/dev/tty` を開けずにすぐ失敗する)。`GIT_TERMINAL_PROMPT=0` と `GCM_INTERACTIVE=never` (Windows の Git Credential Manager の画面) を渡し、GUI の askpass も止める (`SSH_ASKPASS`・`GIT_ASKPASS` を外し、`SSH_ASKPASS_REQUIRE=never` を渡す)。制御端末を外しても、デスクトップの環境では git の CLI が `SSH_ASKPASS` のダイアログを出す (M0 で、GitHub の存在しないリポジトリで起きた。GitHub は存在しないリポジトリにも認証を求める。`net.git-fetch-with-cli` の経路では cargo が失敗を `net.retry` の回数だけやり直すので、ダイアログが繰り返し出る)。保存済みの資格情報 (`credential.helper`・ssh-agent) はこれまでどおり使われる。cargo の既定の経路 (libgit2) も、`credential.helper` と ssh-agent だけを試し、人に問い合わせない (cargo 1.99.1 のソースで確かめた。v0.17)。認証で失敗したら (`fetch_failed` の `auth`。R-WS-7)、利用者の端末で資格情報を通すよう案内する (案内の文面は §8)
    - SIGKILL は捕まえられないので、Linux では子に `PR_SET_PDEATHSIG` を設定し、親が先に死んでも子の cargo が残らない (パッケージキャッシュのロックを持ち続けない) ようにする
    - それでも残る子孫 (`PR_SET_PDEATHSIG` で止まるのは cargo だけで、cargo が起動した rustc や build script は残りうる。macOS にはこの仕組みが無い) は、次回の起動時に片付ける。PID は再利用され、複数のセッションのサーバーが同時に動くので、確かでなければ止めない (無関係なプロセスを止めないことを優先する)。止めなかった子孫がいる target dir は使わない (R-WS-2。残った子孫は target dir のロックを持たないので、ロックが取れても書き続けている場合がある)。記録するもの、照合の規則、子を spawn するスレッド、版の違う記録の扱いは Q-29 で決める (v0.16)
  - ビルドは作業用ディレクトリで行い、成功したものだけを原子的に公開する (失敗したビルドや、終了で中断したビルドがキャッシュヒットにならない)。起動時に、持ち主のプロセスが生きていない作業用ディレクトリを消す
- R-WS-7 ビルド失敗:
  - `--message-format=json` の level=error の診断を先頭 N 件付けて `build_failed` を返す。エラーの text は 1 万文字以内 (R-ERR-2) なので、code・message・suggestions・request_id の余地を残して収める。初期値は、N (`build.max_diagnostics`) は 2、1 件は 3,500 文字以内、診断の合計は 8,000 文字以内 (どちらも `content[0].text` の JSON にエスケープした後の長さ) で、M2 で実際の診断を見て決める (v0.16)。診断は `message.rendered` を使い、サニタイズ (R-SEC-7) の後に文字の境界で切る。JSON の診断が無い失敗 (build.rs の失敗など) では、cargo の stderr の末尾を同じ上限で付ける。失敗の判別に stderr の文字列照合を使わない。例外は、R-WS-2 のロック待ち、`requires rustc` と toolchain の未導入 (下記)、R-VR-2 の `--locked` の失敗、下記の git の取得の決定的な失敗と、サーバーが `--offline` を渡したときのキャッシュに無いことによる失敗 (`no matching package named`) だけとする (R-WS-5 の `-p` で失敗する依存は、起動前に判定するので照合しない。v0.17) (いずれも終了コードでは見分けられない。文言の照合はテストで固定する)
  - **ビルド失敗を「存在しない」に畳み込まない**。「存在しない」としてキャッシュしない (ferritin の `NotAvailable` の誤り)。path / git の依存で黙って docs.rs に切り替えない。直近に成功した JSON があっても常に `build_failed` を返す (古い結果で壊れていることを隠さない)
  - 失敗は 2 種類に分ける
    - **一時的な失敗**: 終了の仕方で分かるものだけ。シグナルによる終了 (OOM killer など) と、サーバーが止めたもの (タイムアウト、中断、ロック待ちの打ち切り)。依存の取得はビルドの前に `cargo fetch --locked` (仮のプロジェクトでは `--locked` なし。R-VR-2) で分けて行い、その失敗も一時的とする。返し方は下の `cargo metadata` の失敗と同じにする (Cargo.toml の誤りや存在しない版も、通信の失敗と同じ終了コード 101 で `cargo fetch` が失敗するので、`network_error` と決めつけない。v0.17)。一時的な失敗は `retryable: true` で返し、R-ERR-3 の一時的な失敗の TTL (目安 30 秒) の間だけ再ビルドを抑える。ただし git の取得で、認証の失敗・リポジトリが無い・rev (ブランチ・タグ・コミット) が無い、のどれかと cargo の文言で判定できたものは、決定的な失敗として `fetch_failed` (`reason` 付き) を返し、下の決定的な失敗と同じく `refresh` までキャッシュする (再試行を繰り返させないため)。判定できなければ一時的な失敗のまま扱う。判定に使う文言は次のとおり (libgit2 と git の CLI で違う。cargo 1.99.1・git 2.56.0 で M0 で確かめた。`cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update` で同じ。文言はテストで固定する): rev が無い → libgit2 は `revision <rev> not found` (通信の失敗を示す `network failure seems to have happened` も一緒に出て、cargo が再試行するので遅いが、`revision … not found` があれば決定的な失敗とする)、CLI は `fatal: remote error: upload-pack: not our ref <rev>` (と cargo の `revision <rev> not found`)。ブランチが無い → libgit2 は `cannot locate remote-tracking branch`、CLI は `fatal: couldn't find remote ref`。リポジトリが無い → `file://` では libgit2 が `failed to resolve path`、CLI が `does not appear to be a git repository`。GitHub の https では、存在しないリポジトリも認証の失敗になる (libgit2 は `failed to authenticate when downloading repository`、CLI は `could not read Username`)。git を取得しうる子 (`cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update`) には `LC_ALL=C` を渡す (git は翻訳があれば地域化する。M0 の環境には翻訳が無く、確かめられなかった) (Q-30。M0 で決定。v0.17)。`cargo metadata` の失敗のうち、既存の例外 (R-VR-2 の `lockfile_outdated`、上の `fetch_failed`) に当たらないものは、決定的な失敗として固定せず、`build_failed` (`retryable: true`) として cargo の診断を添えて返し (v0.16 で R-ERR-2 から `stage` を外したので付けない)、一時的な失敗の TTL (30 秒) だけ抑える。終了コードは Cargo.toml の誤りも通信の失敗も 101 で、見分けるには文言の照合しかないため (M0 で確かめた)。キャッシュに無いことによる失敗 (`no matching package named …`。crate 名の誤りと同じ文言) は、サーバー自身が `--offline` を渡したときだけ `offline_miss` にする (Q-30。M0 で決定。v0.17)
    - **決定的な失敗**: それ以外のすべて (JSON の診断を出さない build.rs の失敗も含む。システムライブラリの不足など)。結果 (診断の要約を含む) を R-WS-4 の鍵 (使った toolchain を含む) でキャッシュし、鍵が変わるか、ツールの引数 `refresh` (§6.4) を指定するまでは、再ビルドせずに同じ `build_failed` を `retryable: false` と失敗の時刻付きで返す。hints には「原因 (システムライブラリの不足など) を直したら、照会のツールは `refresh`、外のツールは同じ引数での呼び直しで再試行する」と書く (rustdoc-mcp は失敗したビルドを呼び出しのたびにやり直す)
  - R-WS-8 で選んだ toolchain が使えない場合 (オフラインモードや、rustup の自動導入が失敗した場合) は `toolchain_missing` と導入コマンドを返す (std は R-STD-4)。サーバー自身は `rustup` や `cargo install` で toolchain・コンポーネント・ツールを導入しない (R-STD-6 と R-SC-2 のキャッシュへの取得は除く)
    - rustup は、プロジェクトの `rust-toolchain.toml` が指す未導入の toolchain を既定で自動で導入する (rustup 1.29.1)。サーバーが自動導入を許すのはこれだけ (rust-analyzer や利用者の cargo でも同じく導入されるので、止めても守りにならない。v0.14)。サーバー自身が選ぶ toolchain (R-WS-8 の nightly や日付を固定したもの。`cargo +<toolchain>` にも rustup の自動導入は効く。2026-10-16 に実測) と、オフラインモード (R-CA-7) では、子プロセスに `RUSTUP_AUTO_INSTALL=0` を付けて止め (効くことは 2026-10-15 に確認)、未導入なら `toolchain_missing` を返す。R-CA-8 を実装した場合は、static.rust-lang.org を無効にしたときも同じく止め、ミラーを設定したときは `RUSTUP_DIST_SERVER` も子に渡す (v0.16)
  - E0658 (`--message-format=json` の診断コード) と、cargo が stderr に出す `requires rustc` は `toolchain_too_old` に分類し、使った toolchain の更新を案内する (R-WS-8 で選んだ toolchain に合わせる。プロジェクトの toolchain が `rust-toolchain.toml` で固定されていればその更新、rustup の無い環境では rustup のコマンドを案内しない。v0.17)
- R-WS-8 toolchain の選び方:
  - **既定は、プロジェクトの toolchain (`rust-toolchain.toml`、なければ既定の toolchain) に `RUSTC_BOOTSTRAP=<crate 名>` を付けたもの** (Q-12。M0 で決定。v0.17)。値はパッケージ名ではなく、ターゲットの crate 名 (アンダースコアの形。`[lib] name`) にする。どの crate 名を並べるかは下の「値を安定させる」に従う
    - 実測 (2026-10-14、stable 1.99): rustdoc に `RUSTC_BOOTSTRAP=<crate 名>` を付けると形式 61 の JSON が出る (付けなければ `-Z` が拒否される)。`cargo rustdoc -- -Z…` 経由でも crate 名の指定で成立した: lib、bin、proc-macro crate 自身、build.rs (`rustc-env` と `rustc-cfg`) と依存の proc-macro (derive) を持つ crate、`--document-private-items` 付き。build.rs が設定した cfg の item も、derive が生成した impl も JSON に出た。パッケージ名 (`my-app`) を指定すると失敗する。build.rs が `RUSTC_BOOTSTRAP` を見ない依存は、値を変えても再 check されなかった
    - **値を変えると作り直される依存がある**: proc-macro2・anyhow・thiserror などの build.rs は stable で `cargo:rerun-if-env-changed=RUSTC_BOOTSTRAP` を出すので、値を変えるたびに作り直される (2026-10-14 に確認。proc-macro2 の下には syn と多くの proc-macro がぶら下がる)
    - **そこで値を安定させる**: 値を変えるたびの作り直しを避け、依存のための値で member の文書が変わらないようにする。利用者の環境に `RUSTC_BOOTSTRAP` があっても常に上書きし、その旨をログに出す。値は、プロジェクトの専用 target dir ごとに、**依存の解決結果のすべての lib・proc-macro の crate 名と member の bin の crate 名を辞書順にカンマでつないだもの**に固定する (Q-12。M0 で決定。v0.17)。値は Cargo.lock と member の構成が変わらない限り変わらないので、作り直すのは文書化する crate の rustdoc だけになり、target dir も 1 つで済む (M0 では、値を変えると proc-macro2 の build.rs から syn と proc-macro を経て 54〜65 unit・10〜13 秒の作り直しが起き、固定すれば 0.2〜0.9 秒だった)。自分の名前で nightly の機能を調べる依存 (proc-macro2・thiserror など) は nightly の経路でコンパイルされ、derive が生成した item の span の列が少し変わる (docs.rs と同じ側になる。それ以外の文書の中身は変わらず、thiserror の文書は docs.rs と一致した)。その経路でビルドが壊れたときに、member の名前だけの値でやり直すかは M2 で決める。案として挙げていた「member の名前の固定の一覧」「依存用の別の target dir と build-dir」「文書化した依存の名前を足していく一覧」は、作り直しが起きるか、target dir が 2 つになるので採らない
    - 実測 (2026-10-16、stable 1.99): `RUSTC_BOOTSTRAP` はカンマ区切りで複数の crate 名を受ける (空白は不可)。member 用の値に依存の名前を足すと、anyhow などがその名前で nightly 用の機能を有効にし、member の文書まで stable と違いうる。build-dir を共有すると中間生成物と fingerprint が共有され、値を切り替えるたびに作り直された。値を固定すれば作り直されないことを確かめた (member `a`・`b` が proc-macro2 に依存する workspace で、`RUSTC_BOOTSTRAP=a,b` のまま `a`・`b`・`a` の順に文書化すると、proc-macro2 のビルドは最初の 1 回だけだった。値を crate ごとに変えると毎回ビルドされた)。bin の crate 名も値に入れる必要がある (`a,b` のままでは `--bin my-app` が `-Z` を拒まれ、`a,b,my_app` で通った)
    - cargo-semver-checks は本番で `RUSTC_BOOTSTRAP=1` を使っている。本サーバーは crate 名を指定する。値が解決結果の全 crate 名になるので、`RUSTC_BOOTSTRAP=1` との違いは、rustc の判定の上では、cargo 自身の `-Z` (cargo は値が `1` のときだけ受ける) と、build script (crate 名は `build_script_build`) に効かないことだけになる (build script が別の crate 名で行う検査用のコンパイルや、値そのものを読む build.rs では違いうる。確かめていない)。cargo の unstable な機能を黙って有効にしないことと、build script の挙動を利用者の普段のビルドとそろえることのために、crate 名の指定を続ける (v0.17)
    - 利点: nightly が不要で、プロジェクトが実際に使う toolchain と同じコンパイラで生成できる (MSRV や新しい構文での失敗が起きない)
    - 出る形式は toolchain の版で決まる (1.97 / 1.98 / 1.99 はそれぞれ形式 57 / 60 / 61。1.97・1.98 は rust のソースのタグの `FORMAT_VERSION` で、1.99 は上の実測で確認。2026-10-16)。対応範囲 (R-FV-4) より古い形式しか出ない古い toolchain では、nightly に切り替える
    - 依存の多い実際の workspace (このリポジトリ、242 パッケージ) で、作り直しの範囲と時間、文書の違いを確かめて既定に確定した (Q-12。結果は m0-plan.md の §6.2 の B1。v0.17)
  - nightly を使う場合は最新の `nightly` を既定にする。**日付を固定した nightly を既定にしない** (新しい MSRV の crate でビルドに失敗する)。固定は設定で選べるようにする
  - toolchain が出す形式が canonical より新しく読めなければ、黙って読まず、`unsupported_format` を返してサーバーの更新と toolchain の固定の方法を suggestions に示す。形式を調べる時期 (案: 使う前に 1 行の crate で rustdoc を実行する。生成した JSON を見る方式もある) と、対応範囲の形式を出す別の toolchain (設定で日付を固定した nightly など) に自動で切り替えるかは M2 で決める (v0.16)。調べた結果はプロセスの寿命で固定せず、新しい toolchain を入れたら再起動なしで反映する。この結果と `rustc -vV` の結果をキャッシュするか、その鍵 (案: toolchain のコミットハッシュ、toolchain のディレクトリと `rust-toolchain.toml` の内容ダイジェスト) は M2 で決める (v0.16)
  - std の JSON (`rust-docs-json`) は nightly の配布物にしか無い (stable 1.99 と beta 1.100 の manifest にも節はあるが、全 target が `available = false`。2026-10-15 に確認)。std だけは手元の nightly の `rust-docs-json` か、R-STD-6 の dist からの直接取得を使う (§5.11)
- R-WS-9: workspace の JSON は最後に成功したビルドの時点を表す。応答に `built_at` を付け、ソースがその後に変わっていれば LSP を使うよう hints で示す (§9.9)
- R-WS-10 **`build_external_docs`** (§6.3。R-MCP-3・R-SEC-2): 既定のプロジェクトの外のコードの文書を作り、照会のツールが読めるように記録する
  - 対象と記録 (外のツールの範囲はここを正とする): 依存でない crate と Cargo.lock に無い版 (`crate`・`version`。`api_diff` の比較相手で docs.rs に JSON が無い版を含む)、既定のプロジェクトの member と依存を、実際のビルド target 以外の target で文書化したもの (`target` は必須。member は `project`、依存は `crate`。版は Cargo.lock のもの。R-MCP-3)、git リポジトリの crate (`git` と `rev`・`branch`・`tag`)、既定のプロジェクトの外の workspace (`project`)。外の workspace と、既定のプロジェクトを他の target で文書化するときは、まず `cargo metadata` の結果と入力ファイルの内容ダイジェスト (R-WS-4) を記録する (照会のツールがその記録を R-MCP-3 で読むため。v0.16)
  - **外の workspace (`project`)**: 既定では、全 member の lib と、docs.rs で引けない依存 (path・git・非公開のレジストリ) をビルドする。範囲はその workspace の `cargo tree --target` の集合 (R-MCP-3 と同じ組の規則) に絞る。crates.io の依存で docs.rs に JSON が無いかは照会のときに確かめ、無ければ記録に「まだビルドしていない依存」として残し、外のツールでの作り方を案内する (ビルドのときに全依存の docs.rs を確かめる要求を出さないため。v0.15)。任意の `crate` で 1 つの member か依存に絞れる。docs.rs に JSON がある依存は、記録した版の解決で照会のツールが docs.rs から引く。記録は専用 target dir と同じ GC の枠 (R-CA-5) に入れ、照会のツールが読んだときも最後に使った日時を更新する。照会のツールは、外の記録が壊れていれば作り直さずに `docs_unavailable` を返して外のツールを案内し (R-CA-4 の取り直しは外の記録には使わない)、外のツールは記録があっても検証に失敗したら作り直す (v0.15)
  - **呼び直し** (v0.15): どの対象でも、同じ引数で呼び直したら決定的な失敗のキャッシュ (R-WS-7) を解いて取り直す (外のツールには `refresh` を案内しない)。同じ鍵のビルドが進行中なら合流する (ブランチなどで、進行中のビルドが要求より前に解決を済ませていた場合の扱いは Q-29。v0.16)。何を取り直すかは対象ごとに決める: `crate@version` は、記録が有効なら失敗のキャッシュを解くだけにする。同じ `branch`・`tag`・`refs/…` の `rev` は解決し直す (仮のプロジェクトで `cargo update -p`)。workspace は metadata とダイジェストを記録し直し、変わったものだけ作り直す (`stale` を解消する手段)
  - **入力から処理を決める表** (v0.15):

    | 入力 | 処理 |
    | --- | --- |
    | `project` (既定のプロジェクトの外) | 外の workspace (上記) |
    | `project` (既定のプロジェクト) と `target` | 他の target での文書化。既定は全 member の lib |
    | `crate` が既定のプロジェクトの依存 (版は Cargo.lock のもの) | `target` が実際のビルド target と違えば、他の target での文書化 (プロジェクトの workspace で)。同じなら照会のツールの範囲なのでビルドしない。ただし `-p` で名指しできない依存 (R-WS-5) は、仮のプロジェクトで文書化する (v0.17)。他の target でだけ使う依存で `target` が無ければ `invalid_argument` |
    | `project` (既定のプロジェクト) で、`target` が無いか実際のビルド target と同じ | 照会のツールの範囲なのでビルドせず、その旨を返す (v0.16) |
    | `crate` が依存でないか、`version` が Cargo.lock に無い | 仮のプロジェクト (R-WS-5。target は引数か host) |
    | `git` と `rev`・`branch`・`tag` のどれかと `crate` | 仮のプロジェクトの git 依存 (R-WS-5) |
    | どれにも `document_hidden_items` | 付けられる (`api_diff` の比較用)。docs.rs に読める JSON があってもビルドする (R-AD-3) |

  - **他の target での文書化** (依存でない `crate` と `git` の crate も、引数 `target` で他の target を指せる。R-WS-5): 依存は `crate` に版付きの pkgid で指す。member と依存のどちらか決まらなければ `invalid_argument` にする。`target` を省いたら `invalid_argument` を返し、他の target 向けの依存 (`[target.'cfg(…)'.dependencies]` など) なら Cargo.toml の条件 (`cfg(windows)` など) を添える。docs.rs の既定の 5 つの target (R-TG-1) のうち条件に合うものを計算して候補に添えるのは任意とする (v0.16)。依存は、docs.rs にその target の JSON が無ければ、既定のプロジェクトの workspace で `cargo rustdoc -p <crate>@<ver> --target <target>` を実行する (版はプロジェクトの Cargo.lock のもの。Linux で `x86_64-pc-windows-gnu` を指して、Windows でだけ使う依存も、member の `#[cfg(windows)]` の item も文書化でき、JSON の `target.triple` もその target になることを 2026-10-16 に確かめた)。その target の `rust-std` が無ければ (ビルドの前に sysroot の `lib/rustlib/<target>` で判定する。判定できることを M0 で確かめた。R-TG-1。v0.17)、`rustup target add` を添えて `toolchain_missing` を返す (rustup の自動導入は target を入れない。v0.15)
  - 鍵: target は照会のツールと同じ規則 (R-TG-1) で決め (他の target での文書化では要求した target)、照会のツールが R-MCP-3 の鍵で引けるようにする。toolchain は R-WS-8 で選び、照会の鍵には入れずに記録と provenance に載せる (R-MCP-3)。応答で、照会に使う鍵 (`crate`・`version`・`git`・`rev`・`project`・`target`。git の `rev` には解決したコミットのハッシュを入れる) と provenance を返す。git の URL は、cargo の `CanonicalUrl` に合わせて正規化してから鍵にする (末尾の `/` を外してから `.git` を外し、github.com の URL は小文字にする)。ただし scheme は書き換えずに鍵に含める (cargo は github.com の URL の scheme を `https` にそろえるが、平文で取った記録を https の照会に返さないため。v0.15)。照会のツールも同じ正規化をしてから引く
  - 進捗と 1 リクエストあたりの時間の上限は R-WS-6 と同じ。上限を超えたら `status: building` を返し、**ビルドは裏で続ける**。続きは照会のツールで待つよう hints で案内する (外のツールを呼び直すと、利用者が ask を書いていれば呼ぶたびに確認が出るため。v0.15)。docs.rs に読める JSON があるものと、照会のツールが同じ target でビルドする範囲 (R-MCP-3) のものは、ビルドせずにその旨を返す (`document_hidden_items` を付けたときを除く。上の表)
  - 照会のツールは、外の記録 (ビルドの結果) がまだ無くても進行中なら、`docs_unavailable` ではなく `status: building` と `retry_after_secs` を返す。進行中の印は、持ち主のサーバーが死ねば OS が外すロック (R-CA-4) で表す (PID の再利用を気にせずに判定でき、3 OS で同じに動く)。裏のビルドが失敗したら、その結果 (診断と時刻) も照会の鍵で記録し、照会のツールは `build_failed` を返す (R-WS-7 の鍵には `rustc -vV` が入り、照会のツールは組み立てられないため)。印の取り方 (試し取りの手順、ビルドの枠を待つ間も持つか、git で要求の鍵と解決したコミットの鍵の両方を持つか) は Q-29 で決める (v0.16)
  - glob の展開、R-PR-4 の latest による推定、R-CA-6 の先取りなど、照会のツールの暗黙の取得は docs.rs からだけ取り、外のローカルビルドを起こさない

### 5.11 std 対応

- R-STD-1: 手元の nightly に `rust-docs-json` があれば、`rustup run <nightly> rustc --print sysroot` の `share/doc/rust/json/` から std / core / alloc / proc_macro / test を読み込む (無ければ R-STD-6)。provenance には `rustc -vV` の全体 (release・commit-hash・commit-date。dist から取った場合はアーカイブの日付) と **target** を載せる。std の JSON は target ごとに中身が違う (`std::os::windows` などがある) ため
  - `rust-docs-json` が配られていない target (`wasm32-unknown-unknown`、`thumbv7em-none-eabihf` など。配られているのはホストになれる target だけ) では、ホストの target の JSON で代用し、R-TG-2 のとおり実際に使った triple を provenance に載せて warnings で知らせる。core と alloc は target による差が小さいが、std は `std::os` などが食い違い、その target に std が無いこともある旨も warnings に書く
- R-STD-2: std の item には `Item::stability` (形式 58 以降) と `const_stability` (形式 59 以降) を返す。stable か unstable か、安定化されたバージョン、unstable なら feature 名を含める。**unstable な item と const-unstable なメソッドには必ず印を付ける** (ferritin は印なしで `push` を `const fn` と表示している)
  - 形式 60 以降の `default_unstable` (trait の関数・関連定数・関連型の**既定の実装**が不安定であること) も表示する。「この trait を実装するとき、この default に頼れるか」の判断に要る
  - rustdoc JSON の unstable には、tracking issue の番号も reason も入っていない (`feature` 名だけ。2026-10-14 に staged_api の crate で確認)。issue へのリンクを推測で作らず、feature 名だけを示す
- R-STD-3: **ユーザーが使っている toolchain** (`rust-toolchain.toml` か `rustc -vV`) と **MSRV** (`rust-version`) を基準に、次の item に警告を付ける (toolchain の版が分からなければ MSRV だけを基準にする)
  - まだ安定化されていない item
  - 基準より後のバージョンで安定化された item (安定化の版は `Item::stability` から取り (R-STD-2)、該当するリリースの要点を R-RR-1 から引用する)
- R-STD-4: `rust-docs-json` が手元に無く、R-STD-6 を設定で無効にしているときは、導入コマンド (rustup の環境なら `rustup component add rust-docs-json --toolchain nightly`) を添えた `toolchain_missing` を返す。オフラインモードでキャッシュにも無ければ、`offline_miss` (R-CA-7) に導入コマンドを添える
- R-STD-6 **nightly の toolchain を入れずに std を引く** (ferritin のホスト版は、手元に何も無くても std を引ける。その利点を取り込む):
  - nightly のチャネルの manifest (`https://static.rust-lang.org/dist/channel-rust-nightly.toml`) の `[pkg.rust-docs-json-preview.target.<triple>]` に、`rust-docs-json` 単体のアーカイブの URL と sha256 が載っている (2026-10-14 に確認。x86_64 Linux の xz で約 4.2MB)。これを取得し、**sha256 を検証してから**キャッシュに展開する。manifest の 17 の target は別の target のアーカイブを指している (`x86_64-apple-darwin` が aarch64 のもの、FreeBSD・illumos などが x86_64 Linux のもの。2026-10-15 に確認) ので、R-TG-2 と同じく実際の triple を JSON の `target.triple` から読み、要求と違えば warnings で知らせる
  - 手元の nightly に `rust-docs-json` があればそちらを優先する (R-STD-1)。どちらを使ったかも provenance に載せる
  - 既定で有効とし、オフラインモード (R-CA-7) と設定で無効にできる。R-WS-8 の既定 (プロジェクトの toolchain + `RUSTC_BOOTSTRAP`) と組み合わせると、**nightly を一切入れずに**依存・workspace・std を引ける
  - 任意 (M4 で検討。v0.16): ユーザーの stable の版に近い nightly の `rust-docs-json` を選べるようにする (unstable な API の混入を減らす)
  - manifest の日付が変わったときだけアーカイブを取り直す。manifest の取得には TTL (1h) を付ける (R-CA-3)

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
- R-AD-3 **docs.rs の JSON だけで見た「削除」は信用しない**。docs.rs の JSON は `#[doc(hidden)]` の item を含まないので、deprecated かつ hidden になっただけの item が消えたように見える (tokio の `TcpSocket::tos` で確認済み)。両側が `--document-hidden-items` 付きのローカルビルドでない限り `removed_or_hidden` (確度 低) として返す。`api_diff` の比較のためにローカルビルドするときは `--document-hidden-items` を付ける (鍵の rustdoc の引数が変わるので、通常の照会のキャッシュとは別になる。R-WS-4)。比較相手の版が Cargo.lock に無く docs.rs に JSON も無ければ、`docs_unavailable` を返して `build_external_docs` (`document_hidden_items`。R-WS-10) を案内する
- R-AD-4 正規化と provenance:
  - 両側の provenance を返す。format_version・toolchain・target・feature の組 (docs.rs のものも、ローカルビルドのものも。プロジェクトで統合された組と、仮のプロジェクトの組は違いうる) が違えば warnings を出す (v0.17)
  - toolchain の違いによる見かけの差 (片側にだけある不安定 auto trait、重複した blanket impl) は既定で除く
  - docs.rs のビルドでだけ現れる item (`docsrs` cfg 由来) は、差分に残して印 (`docsrs_only` など) を付ける
- R-AD-5 応答の形: **変わらなかった項目は返さない**。各項目に確度 (`confidence`。`removed_or_hidden` は low) を付ける。ページングは R-OUT-3 に従う。節の並び (案: 件数の要約 → removed → changed → deprecated → added → cosmetic) と、cosmetic や変化の無い blanket impl を既定で省くかは M4 で決める (v0.16)

### 5.13 脆弱性情報 (`audit_dependencies`)

- R-AU-1: プロジェクトの Cargo.lock にある registry のパッケージについて、RustSec / OSV の advisory を**解決されたバージョンに一致するものだけ**返す。「そのパッケージのどれかの版に過去 advisory があった」で判定しない (誤検出の先例がある)
- R-AU-2: advisory の種類 (vulnerability / unmaintained / unsound) と withdrawn を区別する。path / git の依存は「対象外」として示す
- R-AU-3: データは OSV の `crates.io/all.zip` (約 3.6MB、毎日更新。2026-10-15 に確認) のオフラインミラーを持ち、1 日 1 回、条件付きのリクエストで取り直す (TTL 24 時間。文書のキャッシュの容量の枠に入れる。R-CA-5)。`list_crates` と `crate_metadata` にも件数を添え、advisory のある版の item を `get_item` で返すときは warnings を付ける

### 5.14 Rust のリリース情報 (`rust_release`)

- R-RR-1: Rust の `RELEASES.md` (GitHub API の contents で `rust-lang/rust` のものを raw で取る。ETag 付きの条件付きリクエストにし、TTL は 24 時間。R-CA-8 を実装した場合は、ミラーと無効化は GitHub API の設定に従う。R-NW-2) を取り込み、1 つの版、最新版、または指定した版より新しいすべての版の要点を返す。既定のブランチの `RELEASES.md` には、リリース日より前に次の版が載りうるので、リリース日が今日以前の版だけを返す (v0.17)。目的は「**学習データより新しい Rust の変更を LLM が追いつけるようにする**」こと
- R-RR-2: R-STD-3 の警告の引用元としても使う

---

## 6. MCP インターフェース

### 6.1 原則

- R-MCP-1 トランスポート: **v1 は stdio だけを実装する**。Streamable HTTP は、ホスト版か共有キャッシュサーバー (§9.10) を作る段階 (M6 以降) まで実装しない。実装するときも任意の機能とし、自分用の常駐プロセスとして使う場合は localhost に bind する (R-SEC-5)
  - 理由: ローカルで HTTP の利点が最も効くのは、同時に複数のセッションを使うときに 1 プロセスを共有できることだが、ディスクキャッシュはもともとプロジェクトをまたいで共有され (R-CA-1)、mmap したアーカイブのページは OS のページキャッシュでプロセス間でも共有される (R-PF-1)。HTTP で上乗せされるのは、各プロセスの匿名メモリ (解析後の構造体・逆引き索引)、`cargo metadata` の重複、プロセスの起動コストの節約だけで、次の負担に見合わない
  - HTTP で新たに要るもの: プロジェクトの文脈 (R-SEC-5)、常駐プロセスの起動と停止、更新後に古いプロセスが残る版の食い違い、ポートの衝突、複数のクライアントにまたがるメモリの管理と公平性 (R-MEM-1)、SSE で進捗を送る経路とクライアントの切断をビルドのキャンセルにつなぐ処理 (R-WS-6)、トランスポートごとのテスト
  - Claude Code の時間制限もトランスポートで違う。stdio はアイドル 30 分 (進捗通知でリセット) で、リクエストごとのタイマーが無い。HTTP はアイドル 5 分で、最初の応答バイトまでのタイマー (60 秒以上) がある。R-WS-6 (約 30 秒で `status: building` を返す) はどちらにも収まるように作る
  - 後から HTTP を安く足せるよう、R-MCP-3 と R-ARCH-5 のテストを v1 から持つ
- R-MCP-2 対象とする仕様と追従の方針 (Q-2):
  - **MCP の最新版だけに対応する**。v1 の時点では **2026-07-28** だけで、2025-11-25 以前の `initialize` の handshake には対応しない。Claude Code の v2 ランタイム (MCP TypeScript SDK 2.0) は、サーバーが 2026-07-28 に対応しているかを尋ね、対応していなければ従来の handshake に戻る。v2 ランタイムを既定で使うのは、feature flag を取得するセッションでは v2.1.232 以降、それ以外のセッション (Bedrock、テレメトリ無効など) では v2.1.274 以降 (Claude Code の MCP の文書)。ただし stdio サーバーとの 2026-07-28 の交渉が全環境で既定になったのは v2.1.292 (CHANGELOG。v2.1.274 の変更は直接接続の HTTP サーバーが対象)。README に書く Claude Code の最低版は v2.1.292 とする。最初の判定への応答が遅いと、7 日間は従来の方式で接続される (v2.1.292 の CHANGELOG) ので、`server/discover` には即座に応える (2026-10-15 に確認)
  - **新しい版が出たら追従する**。追従と、古い版を外すことはリリースノートに書く。2 つの版をどう並べるかは、次の版が出たときに決める (v0.16)
  - **古い版のクライアント**: stdio の `initialize` には、必須の `_meta` の検査より先にメソッド名で判定して、版の交渉 (下記) と同じ `-32022` を返す (`data.requested` は `initialize` の `protocolVersion`)。`message` に対応する版と、Claude Code なら必要な版を書く (`basic/versioning` の SHOULD。古い版のクライアントには版を上げる仕組みが無く、このエラーが利用者に見える唯一の手がかりになる)。繋がらないのは、Claude Code の古い版、`MCP_PROTOCOL_NEGOTIATION=legacy` / `MCP_SDK_GENERATION=v1` で従来の版に戻した環境、2025-11-25 以前しか話さないクライアント (2026-10-14 時点の Gemini CLI と Codex CLI の既定を含む)。README に、これらが繋がらないことも書く
  - **stdio を直接使えないクライアント** (ChatGPT などリモートの MCP だけを使うもの): 利用者が stdio を中継するトンネルを自分で用意する前提とし、サーバーでは個別に対応しない (M6 の HTTP は共有のための機能で、この目的のものではない。R-MCP-1)。トンネルを通しても、クライアントが最新版を話せなければ繋がらないことを README に書く
  - **版の交渉** (`basic/versioning`): 要求の `_meta["io.modelcontextprotocol/protocolVersion"]` が対応外の版なら、`UnsupportedProtocolVersionError` (`-32022`) を返し、`data.supported` に対応する版 (v1 では `["2026-07-28"]`)、`data.requested` に要求された版を入れる
  - 要求は状態なしで処理し、サーバーから JSON-RPC 要求を送らない (MRTR も使わない。R-SEC-2)。2026-07-28 で削除された `ping`・`logging/setLevel`・`notifications/roots/list_changed` には応じない (要求なら未知のメソッドとして `-32601`)
  - 必須の `_meta` (`protocolVersion`、`clientCapabilities`) を欠く要求 (`initialize` を除く) は `-32602` で拒否する (`basic/index`)
  - この項目の挙動は、SDK に任せる部分も結合テストで確かめる
  - rmcp 3.5.1 の挙動で、この要件と違うが受け入れるもの (ハンドラからは変えられない。結合テストで固定し、rmcp が変われば気付けるようにする。M0 で確かめた。v0.17): セッション開始前の `_meta` の無い `ping` に空の結果を返す (`-32601` ではない)。`arguments` がオブジェクトでない `tools/call` に `-32601` を返す (R-ARG-1 は `-32602`)。`initialize` を拒んだ後にプロセスが終わる (古いクライアントは拒まれた後に続けられないので実害は無い)。`_meta` で対応外の版を名乗った要求への `-32022` の `message` は rmcp の固定の文言 (独自の `message` を求めるのは `initialize` だけ)
- R-MCP-3 **セッション状態に頼らない**:
  - 対象のプロジェクトは、ツール引数 `project` (`Cargo.toml` へのパスかディレクトリ) で渡せる。`CLAUDE_PROJECT_DIR` の無い起動のしかたもあるので、**`project` 引数の経路を主経路の 1 つとしてテストする** (環境変数を消した状態の結合テストを持つ)
  - 省略したら次の順で決める (こうして決まるプロジェクトを既定のプロジェクトと呼ぶ): 環境変数 `CLAUDE_PROJECT_DIR` → プロセスのカレントディレクトリ。どれを使ったかを provenance の `project_from` に載せる (`roots/list` の扱いは下記)
  - **プロジェクトの単位は Cargo の workspace** (workspace に属さない単独の package はその package): `project` に member のディレクトリや Cargo.toml を渡しても、それが属する workspace の全体を 1 つのプロジェクトとして扱う。専用 target dir (R-WS-2) は、この単位で決める。workspace のルートは、Cargo.toml を上へ辿り、`package.workspace` と `[workspace]` の `members` / `exclude` を Cargo の規則どおりデータとして読んで決める (member でない上位の workspace は拾わない)
  - 決めたディレクトリに `Cargo.toml` が無ければ、上方向には探さない (無関係な上位の workspace を拾わないため)。代わりに、**直下の子のディレクトリのうち Cargo.toml を持つ workspace を、どれも既定のプロジェクトとして扱う** (rust-analyzer が、開いたフォルダの直下に Cargo.toml が無いときに読み込む範囲と同じ。2026-10-16 に確かめた。v0.14)。照会ではそのどれかを `project` で選び、省略したら、候補が 1 つならそれを使い、複数なら `project_not_found` に候補を添えて返す。ただし候補が複数で `project` が無いときも、std の照会 (どの子の toolchain も使わなかった旨を warnings に書く。R-STD-3) と、版を明示した docs.rs の照会は続ける。版を省いた docs.rs の照会は `project_not_found` にする (プロジェクトの版がありうるのに黙って latest を使わない。G1。v0.16)。直下の子は起動時に一度だけ数え、セッションの途中で増えた子 (エージェントが clone したものなど) は既定のプロジェクトにしない (確認なしにビルドする範囲が勝手に広がらないように。`project` で指せば外のツールの扱いになる。v0.15)。直下の子にも無ければ、プロジェクトを要する処理は `project_not_found` (§8) を返す (docs.rs と std の照会は、プロジェクトの文脈なしで続ける。R-ARCH-3)。2 段以上深い workspace は既定のプロジェクトにならないので、suggestions に、その workspace のディレクトリで Claude Code を開くか `build_external_docs` で指す手段を添える。README にも同じことを書く。`cargo metadata` の起動の仕方は R-WS-2
  - **照会のツールが cargo を起動しビルドするのは、既定のプロジェクトだけ** (v0.14 で決定): 既定のプロジェクト (workspace の全体と、実際のビルド target (`.cargo/config.toml` の `build.target`、無ければホストの target。配列ならそのすべてで、`target` を省いた照会は先頭の要素。R-TG-1) で `cargo check --all-targets` がビルドするその依存。rust-analyzer の既定と同じ範囲で、dev-dependency を含み、他の target 向けの依存を含まない。他の target 向けの依存は、rust-analyzer も利用者の `cargo check` もビルドしないので、このサーバーだけがその build.rs を動かす経路にしない) では、確認なしに cargo を起動し、ローカルビルドをする。それ以外 (R-WS-10 の対象) のコードを動かすのは外のツールだけで、照会のツールはそこで作ったキャッシュを読む。どちらのツールでも、このサーバー自身は確認を求めない。外のツールに確認を出すかは、利用者が Claude Code の許可ルールで決める。理由と README に書くことは R-SEC-2
    - 範囲の判定は、実際のビルド target ごとに `cargo tree --workspace --target <target> -e normal,build,dev` で得た crate の集合で行う (配列ならその和集合)。cargo は `cargo rustdoc -p` で名指しされた依存を、範囲の外でも作ろうとしうるので、サーバーが判定してから起動する (resolver 1 の workspace では、他の target 用の依存を別の target で作ったり、host 用の build-dependency を build target で作ったりして、どちらの集合にも無い依存の build.rs が動いた。resolver 2 以上では、member から target 向けの通常の辺でたどれない依存 (dev だけ・build だけ・ほかの target だけ・proc-macro の下だけ) を名指しすると cargo が panic する (M0 で確かめた。扱いは R-WS-5。Q-30。v0.17)。「他の target 用の依存もホストの target のまま文書化する」は一般には成り立たない)。
    - **ローカルビルドしてよい組** (v0.15): 照会のツールがローカルビルドしてよいのは、(package, target) の組が、cargo が `cargo check --all-targets` でその target のために作る単位に含まれるときだけにする。target 用の依存 (通常の依存と dev-dependency) は、要求した target の集合に target 用として現れるときにその target で作り、build-dependency と proc-macro は host 用として現れるときに host で作る。種類 (host 用か target 用か) を判定できないときはローカルビルドせず、docs.rs か外のツールの記録を使う。種類は stable の `cargo tree` で判定する: `-e build` の辺の先と、`(proc-macro)` の印が付いた node の下にある依存が host 用、それ以外が target 用 (`-e normal` の木には proc-macro の依存も出るので、木の上の位置で判断する)。`cargo tree` は package id を出さない (`{p}` は名前・版と、path・git の取得元だけ) ので、名前・版・取得元から `cargo metadata` の package id に対応付ける。nightly の `--unit-graph` (unit ごとの package id・platform・feature を出す) は、nightly と `-Z` が要るので使わない (Q-30。M0 で決定。v0.17)。ただし resolver 2 以上では、`-p` で名指しできない依存 (dev だけ・build だけ・ほかの target だけ・proc-macro の下だけ。R-WS-5) は、照会のツールではローカルビルドせずに `build_unsupported` を返す (外のツールが仮のプロジェクトで作れる。R-WS-5。v0.17)。`cargo tree` の出力に解析できない行があれば、範囲を判定できないものとして扱い (`scope_unknown`)、依存をローカルビルドしない (範囲の判定は、どの build.rs を動かすかを決めるため。v0.17)
    - **feature の組** (v0.15): 範囲は通常の feature の組で判定する。member の文書化の組 (`[package.metadata.docs.rs]` の `all-features`・`features`。R-FT-3) で通常の組の範囲に無い依存が増えても、照会のツールはその依存を確認なしにビルドしない (増えた optional の依存の build.rs を動かさないため。そのときの文書化の仕方は M2 で決める。v0.16)。docs.rs の組での文書は、外のツールで作れる (R-WS-10)
    - `cargo metadata --filter-platform` は build-dependencies の cfg を build target で評価してしまい、ホストで評価する実際のビルドと食い違うので使わない (2026-10-16 に確認)。版の解決 (R-VR-2) には絞らない `cargo metadata` を使う。範囲を判定できないとき (オフラインで Cargo.lock を直接読む経路など。R-VR-2) は、依存のローカルビルドをしない。`cargo tree` の集合は、`cargo check --all-targets --target <target>` がビルドする crate の集合と一致した (2026-10-16、cargo 1.99.1。ホストと `x86_64-pc-windows-gnu` の 2 つで、通常の依存・target 別の依存・proc-macro とその依存・dev-dependency と target 別の dev-dependency・build-dependency と target 別の build-dependency・依存の build-dependency・使わない optional の依存を持つ workspace で確かめた)。`cargo tree` の出力の形は、stable の cargo 1.99.1 と nightly の cargo 1.101.0 で同一だった (2026-10-19。`-e all`、feature 付き)。形が崩れたら nightly の `--unit-graph` を検討する (v0.15。v0.17)
    - 照会のツールに、実際のビルド target (配列ならそのどれか) と違う `target` が渡されたら、既定のプロジェクトの中でもローカルビルドせず、docs.rs → 外のツールがその target で作った記録 (R-WS-10) の順に探す。std は R-STD-6 の配布物からその target の JSON を取る。どれにも無ければ `docs_unavailable` で外のツールを案内する (`target` 引数で他の target 向けの依存の build.rs を確認なしに動かす抜け道を作らず、他の target の文書も外のツールで作れば見られるようにするため。v0.15)
    - **照会のツールは、既定のプロジェクトの外で cargo・rustc・rustup を一切起動しない** (外のツールに分けた意味を保つため。R-QA-5 の結合テストで固定する)。外のプロジェクトでは、`build_external_docs` が記録したもの (R-WS-10) だけを使う: 版の解決には記録した `cargo metadata` の結果、鮮度の判定には記録した入力ファイル (R-WS-4 の入力) の内容ダイジェスト、toolchain には記録したときのもの (下記)。ソースの読み取り (R-SC-4) はファイルを読むだけなので、記録が無くても行う
    - 外の記録の鍵は、照会のツールが組み立てられるものにする (R-WS-4 の鍵には `rustc -vV` が入り、照会のツールは組み立てられないため)。プロジェクトの依存はそのプロジェクトの feature の組と source で作るので、別のプロジェクトや仮のプロジェクトの記録と混ぜない。`document_hidden_items` 付きの記録は `api_diff` だけが使う (R-AD-3)。鮮度の判定では toolchain の更新を見ない。provenance に記録したときの toolchain を出し、toolchain の更新を検知していない旨を warnings に書く。記録の種類ごとの鍵の組、照合の絞り込み、プロジェクトのディレクトリが消えたときの扱いは M2 で決める (v0.16)
    - 記録が無ければ `docs_unavailable` を返して `build_external_docs` を案内する (同じ鍵のビルドが進行中なら `status: building`。R-WS-10)。記録はあるが鮮度が切れていれば、古いものに `stale: true` と warnings を付けて返し、hints で `build_external_docs` を案内する (読むだけならコードは動かない)
    - git の crate は、照会のツールでも `git` と `rev`・`branch`・`tag` で指せる (読み取りの鍵としてだけ受け、取得もビルドもしない。記録が無ければ `docs_unavailable`)。`branch`・`tag`・`refs/…` で指したときは、記録した解決が古くなりうることを示し、hints で外のツールの呼び直しを案内する。どの解決結果を使うかと、古いとみなす時間は M2 で決める (v0.16)
  - Claude Code (CLI) が stdio サーバーをどのディレクトリで起動するかは文書化されていない。文書化されているのは `CLAUDE_PROJECT_DIR` を渡すことだけ (Agent SDK の TypeScript 版には `projectConfigRoot` で起動ディレクトリを決める記述がある)
  - **`roots/list` は使わない**: Roots は 2026-07-28 で deprecated になり、新しい実装は対応を足さない (SHOULD NOT)。サーバーから送れるのも MRTR (`InputRequiredResult` を返して再試行させる往復) の中だけになった。仕様は代わりに「ディレクトリはツールの引数や設定で渡せ」としており、`project` 引数を持つ本方針と一致する
  - `set_working_directory` や `register_project` のような方式は採らない (先行実装では、設定前の呼び出しがすべて失敗する)
- R-MCP-4 annotations (v0.9 で決定):
  - **cargo・rustc を起動しうるツール** (§6.3 のうち `search_crates`・`rust_release` 以外のすべて) には、`readOnlyHint: false`、`destructiveHint: false`、`idempotentHint: true` を付ける (`build_external_docs` だけは、同じ引数で呼び直すと取り直すので `idempotentHint: false`。R-WS-10)。build.rs と proc-macro は任意のコードで、`cargo metadata` も依存を取得してディスクに書き、プロジェクトの設定 (`rustc-wrapper` など) で任意の実行ファイルを動かしうる (R-SEC-2) ので、「環境を変更しない」という定義を保証できないため (v0.13 で基準を「ビルドを起こしうるか」から改めた)
    - ローカルビルドを起こしうるツール (キャッシュに無い workspace の item や、docs.rs に JSON が無い依存を引きうる `crate_overview`・`search_items`・`get_item`・`list_*`・`get_source`・`inspect_features`・`api_diff`、`build_external_docs` など) に加え、ビルドはしないが `cargo metadata` や `rustc -vV` を起動するもの (版を省くと Cargo.lock から版を決める `list_crates`・`resolve_version`・`audit_dependencies`・`crate_metadata`・`readme`・`changelog`・`find_examples` など、std の照会 (R-STD-3 の `rustc -vV`)) を含む
  - **cargo・rustc を起動しないツール** (`search_crates`・`rust_release` の 2 つ) には `readOnlyHint: true` を付ける。ローカル機能を含まない構成 (R-ARCH-2) では、どのツールも cargo を起動しないので、すべて `readOnlyHint: true` にする
  - docs.rs や crates.io に出うるツールには、オフラインモード (R-CA-7) でも `openWorldHint: true` を付ける
  - 採らなかった案: ビルドしうるツールにも `readOnlyHint: true` を付ける案 (照会の感覚に合い、annotations で自動承認するクライアントで確認が出ない) は、定義に反するので採らない。annotations を権限確認に使うクライアントでは照会のたびに確認が出うるが、仕様への正直さを優先する
  - Claude Code の MCP の文書には readOnlyHint を使う記述が無い。ただし実際には、`readOnlyHint: true` のツールは並列に、`false` のツールは 1 つずつ順に実行される (Agent SDK の文書の記述どおり。CLI 2.1.301 で、1 つのメッセージで 2 つ呼ばせて 2026-10-16 に確かめた)。照会のツールが順に実行されることは受け入れる (キャッシュに当たる照会は数十ミリ秒で、差が出るのは docs.rs から初めて取る crate が複数ある場合くらい。同じ target dir のビルドはもともと cargo のロックで 1 つずつになる。v0.15 で、並列のために `true` を付ける案を改めて退けた)。README に書く許可ルールは R-SEC-2
  - 各ツールが実際に cargo・rustc を起動しうるかと annotations の一致を CI で検査する (R-QA-5)。rustdoc-mcp は、`cargo doc` で書き込むのに `readOnlyHint: true` を付けている
- R-MCP-5: `tools/list` は毎回同じ順で返す。2026-07-28 で必須になった `ttlMs` (目安 1 時間) と `cacheScope` を、仕様がキャッシュ可能とする結果 (`server/discover`、`tools/list`。R-MCP-13・R-MCP-18 を入れたときのものは M5 で決める。v0.16) に付ける (`server/utilities/caching`)。`tools/call` の結果には付けない。`tools/call` の結果がプロジェクトに依存するかどうかは provenance で示す。`server/discover`・`tools/list` は `"public"`。`tools.profile` と `output.mode` は利用者の設定で起動時に決まり、1 つのプロセスの中ではどの呼び出しにも同じ一覧を返すので、public に合う。Claude Code がセッションをまたいで一覧を保存するのは discovery cache だけで、対象はリモートの HTTP / SSE サーバー、既定では無効、エントリはサーバー単位、有効期限は Claude Code 側の設定 (既定 15 分 / 最大 4 時間) で決まり、`ttlMs` と `cacheScope` を見るという記述は無い (Claude Code の MCP の文書と環境変数の文書で確認。2026-10-15)。stdio のサーバーは毎回一覧を取り直されるので、プロジェクトごとの違いは問題にならない。`serverInfo.version` は `env!("CARGO_PKG_VERSION")` から取る (固定の文字列にしない)。serverInfo は表示とログのためのもので、クライアントがキャッシュを捨てる判断は `ttlMs` と再起動に任せる (仕様は serverInfo で挙動を変えないことを求めている)。`tools/list` はページに分けない (`nextCursor` を返さず、`cursor` 付きの要求は無効なカーソルとして `-32602`)。設定 (`tools.profile`、`output.mode` による `outputSchema` の有無) を変えて一覧が変わる場合は、サーバーの再起動で反映する (README に書く)
  - 全結果に `resultType` を付け (このサーバーは `"complete"` だけを返す。R-SEC-2)、結果の `_meta` に `io.modelcontextprotocol/serverInfo` を載せる (SDK が対応していればそれに任せる)
- R-MCP-6: サーバーの `instructions` には、呼び出し順の連鎖 (`list_crates` / `search_items` → `get_item` → `list_methods` / `list_impls`)、バージョンと target の規則、LSP との境界だけを書く。問いとツールの対応は各ツールの説明文の冒頭に置く (instructions に問いの一覧を詰め込んだ先例は、後で削られている)
- R-MCP-7: ツール名は動詞と名詞の snake_case にする。説明文の冒頭 1 文に主要な語 (rust、crate、trait、docs) を入れる
  - Claude Code が付ける接頭辞 (`mcp__<server>__`、プラグイン経由なら `mcp__plugin_<plugin>_<server>__`) を含めたフル名を **128 文字以内**に収める (超えるとモデルに送られない)
  - ツール名は MCP の推奨 (1〜128 文字、`[A-Za-z0-9_.-]`。`server/tools`) にも従う。引数のトップレベルのプロパティ名を 1〜64 文字の `[A-Za-z0-9_.-]` にするのは Claude API の制約による (Claude Code が読み込み時に検査し、違反したツールを除外する。MCP の規則ではない)
  - 各ツールに人が読む `title` (英語。例: `Get Rust item docs`) を付ける (表示名の優先は `title` → `annotations.title` → `name`)。`annotations.title` は使わない
  - serverInfo に `icons` を付けてよい (M5 で検討。任意)。ネットワークの URL を使わない (R-SEC-6)
- R-MCP-8: 入口になるツール (`get_item`、`search_items` など 2〜3 個) に `_meta["anthropic/alwaysLoad"]` を付けて常に読み込ませるかは、eval で決める (Q-15)
- R-MCP-9: 1 つの引数の有無で 2 つの別データを切り替える「二役のツール」を作らない (ferritin の `search` は crate の有無で item 検索と crate 探しを切り替え、誤用を防ぐために強い文言を重ねている)
- R-MCP-10: 入出力の JSON Schema は標準の 2020-12 に従う。符号なし整数は `{type: integer, minimum: 0}` とし、schemars が出す `format: uint64` などの非標準の format を出さない (クライアント側の検証器が警告を出す)
  - スキーマのルートに `anyOf` / `oneOf` / `allOf` を置かない (Claude Code は平坦化してしまう)
  - `$ref` は同じスキーマの中の `#/$defs/…` だけにし、外部の URI を参照しない (検証器は外部の `$ref` を辿らない。`basic/index`)
  - 省略可能な引数は `required` から外すだけにし、`Option` を `type: ["string", "null"]` のような null 許容の型で出さない。Gemini / Vertex など、null を含むスキーマを拒む厳しいクライアントでも使えるようにする (possdog は生成したスキーマから `null` を剥がしている)
- R-MCP-11 **起動を待たせない**: `server/discover` と `tools/list` には、索引の読み込み・`cargo metadata`・ネットワークへのアクセスを待たずに即座に応える。重い準備は最初のツール呼び出しまで遅らせるか、裏で進める。Claude Code の起動タイムアウト (`MCP_TIMEOUT`) は既定で 30 秒
- R-MCP-12 **ツールの構成を選べるようにする** (M5 で検討): 設定 `tools.profile` で、主要なツールだけを `tools/list` に出す構成を選べるようにする。tool search を持たないクライアントで、ツール定義の固定費を抑えるため (ferritin のホスト版はツール 2 個で負担が最小)。構成に無いツールは、説明文の「代わり」の節 (§6.2) やエラーの案内でも勧めない。`tools/list` の順と内容は構成ごとに決定的にする (R-MCP-5)。入れるツールと既定は eval (R-QA-6) で決める (v0.16)
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
- 説明文に書いたツール名が実在するかも CI で検査する (R-QA-5。snowmead の prompt が改名後に存在しないツール名を案内し続けている先例による)
- ツールの引数に実装の詳細 (`format_version` など) を出さない

### 6.3 ツール一覧 (案)

| ツール | 答える問い | 使うべきでない場面 → 代わり |
| --- | --- | --- |
| `list_crates` | プロジェクトの workspace member と依存の一覧 (**解決された**バージョン、取得元、有効な feature、advisory の件数、分かっている範囲での rustdoc JSON の有無)。crate を指定すると、その crate を引き込んでいる経路 (workspace member からの依存の連鎖。`cargo tree -i` に相当) を返す | crate の中身を知りたい → `crate_overview` |
| `resolve_version` | X の latest は? このプロジェクトで解決される X は? この版は存在する? JSON はある? | **他のツールの前に呼ぶ必要は無い** (各ツールが自分で解決する)。ドキュメントそのもの → `get_item` |
| `crate_overview` | crate の全体像 (ルートのドキュメントの要約、主要なモジュールと item、feature の概要) | 特定の item → `get_item` |
| `search_items` | 名前や説明文で item を探す。範囲と kind で絞れる | crate そのものを探す → `search_crates` |
| `get_item` | シグネチャ、ドキュメント (Panics / Errors / Safety / Examples の節)、属性、stability、deprecated、必要な feature、re-export の経路、定義位置 (R-OUT-8)。メソッドやプリミティブも引ける | メソッドの一覧 → `list_methods`。ソース → `get_source` |
| `list_module` | モジュールの中身と re-export。`recursive` で子孫のモジュールまで展開でき、`detail=brief` と組み合わせると crate 全体の公開 API の骨格になる (R-LS-1) | 横断的に探したい → `search_items` |
| `list_methods` | その型で呼べるメソッド (固有 / trait / blanket / Deref) | 実装している trait の一覧 → `list_impls` |
| `list_impls` | 型が実装している trait (auto / blanket / derive / 条件付き)。「X は Send か」 | trait の実装者 → `list_implementors` |
| `list_implementors` | trait を実装している型 (`scope`: crate / dependency_graph) | 型の側から見たい → `list_impls` |
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
| `build_external_docs` | 照会のツールがビルドしないコード (依存でない crate・Cargo.lock に無い版、既定のプロジェクトの他の target での文書、git の crate、外の workspace。R-WS-10) の文書を、ローカルビルドで作る。作ったものはキャッシュに入り、照会のツールが読む。build.rs と proc-macro を実行する (R-WS-10・R-SEC-2。名前は v0.15 で確定。利用者が ask のルールに書くので変えない) | docs.rs に JSON がある crate や、既定のプロジェクトの照会 → 照会のツール (必要なら自分でビルドする。ただし照会のツールが `build_unsupported` を返した依存は、このツールで作る。R-WS-5。v0.17) |

### 6.4 共通引数

| 引数 | 意味 |
| --- | --- |
| `crate` | crate 名。`crate@version` も可 (`version` と両方指定して食い違えば `invalid_argument`)。ハイフンとアンダースコアは同一視する (R-PR-1) |
| `version` | 任意。省略したら §5.3 の順で解決する。semver 要件も受ける |
| `path` | item のパス (R-PR-1 の形を受ける) |
| `project` | 任意。省略時の決め方は R-MCP-3。照会のツールで既定のプロジェクトの外を指したときは、`build_external_docs` が記録したものだけを使い、cargo を起動しない (R-MCP-3) |
| `target` | 任意。省略時は R-TG-1。照会のツールで実際のビルド target (配列ならそのどれか) と違う値を渡すと、ローカルビルドせず docs.rs と外のツールの記録を探す (R-MCP-3)。`build_external_docs` で他の target での文書化をするときは必須 (R-WS-10) |
| `limit` / `cursor` | ページング (R-OUT-3) |
| `detail` | `brief` / `normal` / `full` (§7.1) |
| `git` / `rev` / `branch` / `tag` | `build_external_docs` で git リポジトリの crate を文書化する (`crate` と組み合わせる)。照会のツールでは、そうして作ったものを指す読み取りの鍵としてだけ受ける (R-MCP-3)。`git` はリポジトリの URL。`rev` (コミットのハッシュか `refs/…`)・`branch`・`tag` は cargo の git 依存と同じ意味で、指定できるのは 1 つだけ (どれも省けば既定のブランチ)。cargo の `rev` には、ブランチの名前だけを渡すと失敗する (タグの名前・`refs/heads/…`・短いハッシュは通る。2026-10-16 に実測) ので、引数を分けて cargo の同名のキーに写す。文書化の仕方 (仮のプロジェクトの git 依存なので private item は出さない。feature の既定) は R-WS-5、鍵 (解決したコミットのハッシュ) は R-WS-10。照会のツールに短いハッシュを渡したときの照合は M2 で決める (解決したコミットが古いとみなす時間は R-MCP-3。v0.16)。オフラインモードでキャッシュに無ければ `offline_miss` (R-WS-5、R-SEC-2、R-SEC-4) |
| `refresh` | 任意。照会のツールのうち cargo を起動しうるもの (R-MCP-4 の分類) で、その鍵の決定的な失敗の固定 (R-WS-7) を解いて、取得やビルドをし直す。同時ビルド数の上限 (R-SEC-7) はそのまま効く。既定のプロジェクトの外を指したときは無視する (ビルドしないため)。`build_external_docs` には持たせない (同じ引数の呼び直しで解くため。R-WS-10)。成功した結果は作り直さない。決定的な失敗のほかに解く記録 (否定キャッシュ、`--cfg docsrs` の記録など) の一覧は M2 で決める (v0.16) |

- R-ARG-1: 引数の誤り (`inputSchema` に合わない型、必須の引数の欠落、形式の誤ったバージョン、無効なカーソル) は、JSON-RPC の invalid-params ではなく、提案付きのツール結果エラー (`invalid_argument`。§8) として返す (`server/tools` は入力の検証エラーをツールの実行エラーに分類している)。型を厳しくしすぎて入口で弾かない。JSON-RPC の `-32602` にするのは、未知のツール名と、`CallToolRequest` の形に合わない要求 (`name` が無い、`arguments` がオブジェクトでないなど) だけとする (`name` の無い要求への rmcp の応答は M0 で確かめていないので、M1 の結合テストで確かめる。rmcp 3.5.1 は、`arguments` がオブジェクトでない要求に `-32601` を返す。受け入れる違いとして R-MCP-2 に書いた。v0.17)。rmcp の `Parameters<T>` は逆直列化の失敗を `-32602` にするので、引数は JSON のオブジェクトで受けて自前で検証する

### 6.5 一覧系ツールの絞り込みと展開

- R-LS-1 **crate の骨格**: `list_module` に `recursive` (と深さの上限 `depth`) を設け、子孫のモジュールの item まで返せるようにする。`detail=brief` と組み合わせると、crate 全体の公開 API を 1 行のシグネチャの並び (骨格) として `page` 付きで返せる (ruskel の骨格表示、rustdoc-mcp の `recursive` に倣う。ruskel と違い、上限とページングを必ず持つ)
- R-LS-2 **kind の絞り込み**: `list_module` に kind の絞り込みを設ける。上限で切る前に適用する (R-OUT-4)
- R-LS-3 **メソッドの絞り込み**: `list_methods` に、名前の部分一致と出どころ (`inherent` / `trait` / `deref` / `blanket`) の絞り込みを設ける (syphar の未マージのブランチに倣う)。切る前に適用し (R-OUT-4)、`page` の `total` は絞り込み後の件数にする

---

## 7. 出力形式

### 7.1 方針

- R-OUT-1 **Claude Code の実際の挙動を前提にする** (Q-1):
  - `structuredContent` があると、モデルには `JSON.stringify(structuredContent)` だけが渡り、text の content は捨てられる (Claude Code 2.0.21 以降。2.1.301 (2026-10-16) と 2.1.303 (2026-10-19。M0) でも同じことを確かめた。Issue #79944 は 2026-10 時点で open。直る可能性があるので CI で再確認し続ける)
  - よって **`structuredContent` がそのままプロンプトになる**。モデルが必要とする情報 (provenance、warnings、hints、page、truncated) はすべて `structuredContent` に入れる
  - `structuredContent` のキーは、応答の構造体のフィールドの順に並べる (R-SH-1 の「シグネチャを散文より先に」の前提)。rmcp の `structuredContent` は `serde_json::Value` で、serde_json の既定の Map はキーを辞書順に並べ替えるので、`preserve_order` を有効にする (M0 で、無効のままでは `docs` が `signature` より前に出た)。`Value` を後から加工するときも順を保つ (`preserve_order` の下の `Map::remove` は最後の要素で穴を埋めて順を崩すので使わない)。キーの順はテストで確かめる (`Value` どうしの比較は順を見ない) (v0.17)
  - structured モードでは、同じ JSON を直列化した text の content も返す (`server/tools` の SHOULD。後方互換のため)。`structuredContent` に無い情報を text にだけ載せてはならない
  - `isError: true` の結果には `structuredContent` を付けず、中身は `content[0].text` で完結させる (R-ERR-2。Claude Code もエラー時は `structuredContent` を見ない)。仕様の `outputSchema` の MUST に、エラーを除く旨の明文は無いが、エラーを schema に収めない意図的な選択とする
  - structured モードでは `outputSchema` を宣言し、`structuredContent` を必ずそれに合わせる (MUST)。`status: building`・各 `detail`・`page` 付き・`partial` の応答もすべて同じ schema に収める。outputSchema のルートに `anyOf` / `oneOf` を置かず、状態による差は省略可能なプロパティで表す (CI での検査は R-QA-5)
  - v1 の出力は structured だけにする。比較用の text (簡潔な Markdown) と toon (gnufood に倣った TOON 形式) は、M5 の eval の準備として設定 `output.mode` で足す (v0.16)。text モードの表示の規則の候補: 本文中の見出しを構造上の見出し (Methods など) より 1 段下げる、切り詰めの印を `[+N lines]` のように実際の単位で書く (R-SH-7。v0.16)
- R-OUT-2 **JSON は LLM が読む前提で設計する**。効くのは形式の選択 (JSON / Markdown / TOON の差は 10〜25%) より、**何を描画済み文字列にし、何を出さないか**:
  - シグネチャは rustdoc-types の木ではなく **Rust のコード文字列**として描画する (例: `pub async fn lock(&self) -> MutexGuard<'_, T>`)。描画に rustfmt などの外部ツールを実行時に要求しない (ruskel は要る)。木のままだと数倍から十数倍に膨らむ (syphar で約 5〜6 倍、ferritin の JSON で 8〜12 倍。M0 の実測では、シグネチャ・where 句・fields・variants の部分木と描画した文字列の比で、おおむね 8〜17 倍だった (tokio・axum・serde の 9 item。m0-plan.md の §6.1 の A8)。v0.17)
  - **描画できない形**: 描画処理は網羅的な `match` で作り、知らない部品 (新しい形式で増えた型や構文の種類など) に出会ったら、描画を誤魔化さず warnings で示す。そのとき item の span からソースの宣言文を切り出して `source_signature` として添えるか (syphar の未マージのブランチに倣う案) は、M1 で実際にそういう場面があるかを見て決める (v0.16)
  - 引数名を含める。型のパスは**正準パス** (R-PR-2) で描画し、ソースに書かれたままの相対パス (`self::super::Size`) や定義パスを使わない。ほかの crate の型の正準パスは、その crate の公開パス索引 (R-PR-4) で求める (M1 から。v0.17)
  - 制限付きの可視性 (`pub(crate)`、`pub(in path)`) を描画する
  - fields / variants / generics / where 句は、描画済み文字列の配列にする
  - `null` や空の値は出さない。キー名は短すぎず、意味が分かるものにする
  - 数値の rustdoc Id を出さない (ビルドごとに変わり、LLM には意味が無い)
  - `macro_rules!` は、rustdoc が出す規則の一覧で示す (規則の本体は rustdoc がすでに `{ ... }` に省いているので、そのまま残す。v0.17)。`async_trait` の展開形は `async fn` に戻して `via: async_trait` を付ける (任意)
- R-OUT-3 予算とページング:
  - MCP の既定の応答の上限は**推定 8,000 トークン**とする (G5)。推定は R-OBS-1 の換算係数で文字数から出し、校正するまでは **32,000 文字**を上限とする (英文で約 8,000 トークン。JSON・コード・CJK は同じ文字数でもトークンが多い)。Claude Code は 1 万トークン超で警告し、5 万文字超の結果をファイルに退避してモデルには経路だけを渡す
  - 大きな本文を意図的に返すツール (`get_source` の full、`readme`、`api_diff`) は、ツールごとに `_meta["anthropic/maxResultSizeChars"]` (上限 50 万文字) を付けてよい。ただし既定はページング
  - 上限のある一覧 (implementors、methods、impls、モジュールの子、検索結果、changelog など) はすべて `page` (`shown` / `total` / `has_more` / `next_cursor`) を返す。「… and N more」と書いて続きを取る手段が無い状態は禁止する
  - 本文 (ドキュメント・README・ソース) を切り詰めたときは `truncated: true` を付ける
  - カーソルは不透明で、**同じツール**が受ける。前のページと食い違えば理由付きの `invalid_argument` で拒否し、最初から取り直させる (黙って飛ばしたり重複させたりしない)。何に束縛するか (案: ツール、`cursor` と `limit` を除いた引数、データの内容ダイジェストの集合)、オフセットか keyset か、`partial` の応答とビルドの完了との関係は M1 で決める (ビルド中の扱いは M2。v0.16)
  - **呼び出しごとの予算** (M5。任意): 共通引数 `max_chars` で、その呼び出しの上限を既定より小さくも大きくもできるようにする (lockdocs の `--tokens` に倣う)。上限に合わせて `detail` の段階を自動で下げ、`truncated` と `page` で続きを示す。`detail` と `limit` で足りるかを eval で確かめてから採否を決める
- R-OUT-4: 並び順は決定的にする (同じ入力には同じ出力)。絞り込み (kind・名前・出どころ) と並べ替えは、上限で切る前に行う
- R-OUT-5 ドキュメント本文:
  - intra-doc link は Markdown パーサーで書き換える。表示テキストとバッククォートを残し、リンク先を**正準パス**にする。コードブロックの中は書き換えない。解決できないリンクや外部リンクは URL を残す。リンク先を消して表示テキストだけにしない (ferritin は「The complete list is here.」のように行き先を失っている)
  - インラインコードのバッククォートを残す
- R-OUT-6 doctest の隠し行 (`#` と空白で始まる行と、`#` だけの行) は既定で除く。`##` は `#` に戻す。`rust` 系のすべてのコードブロック (`no_run`、`should_panic`、`edition20xx` など) に適用し、`should_panic` / `compile_fail` は見える印として残す
- R-OUT-7 `detail` の段階 (Q-8。数値は eval で調整する):
  - `brief`: パス、kind、1 行のシグネチャ、必要な feature の 1 行の要約と `enabled_in_project` (dev の経路でだけ有効なら `enabled_in_dev_only` も。R-FT-2。該当する項目だけ)
  - `normal` (既定): シグネチャと where 句 (rustdoc は境界の多くを where 句に移すので、入れないと `tokio::spawn` の `F: Future + Send + 'static` が消える。M0 で決定。v0.17)、ドキュメントの第 1 段落 (`docs_truncated` 付き。段階で絞ったことを表し、予算で切り詰めた R-OUT-3 の `truncated` とは別)、残りの節の見出しと行数、fields / variants の文字列、impl の見出し (メンバーなし)
  - `full`: ドキュメント全文 (節ごと)、属性 (生の rustdoc の部分木は返さない。数値の Id を含み (R-OUT-2)、描画した文字列の 8〜17 倍に膨らむため。v0.17)
  - モジュールや crate のルートは散文が大半を占める (tokio::sync では 69%) ので、一覧系では `docs: none` を選べるようにする
- R-OUT-8 リンクやファイルの場所 (docs.rs の URL、ソースファイルのパス) は `structuredContent` の中の文字列として返す。`get_item` は item の**定義位置** (`source_location`: `file` (正規化したファイルパス)・`lines` (`434-466`、1 行なら `301`)・`url`。docs.rs 由来の crate では、`file` は package root からの相対パス (docs.rs の JSON の span そのまま)、`url` は docs.rs のソースブラウザの URL に行のアンカーを付けたもの (`https://docs.rs/crate/<name>/<version>/source/<file>#434-466`。ソースブラウザも rustdoc のソースページも `#N` と `#N-M` で行を強調表示することを M0 で確かめた。ソースブラウザは span のパスがそのまま使え、src の外のファイルも指せる。v0.17)。手元にソースがあればその絶対パス。span が package root の外 (ビルドの生成物の `OUT_DIR` やビルドホストの絶対パス) なら `url` を付けず、warnings で知らせる) を返し、`get_source` やクライアントのファイル読み取りで直接開けるようにする (possdog の `file:line`、lockdocs の `pkg@ver path:line` の引用に倣う)。`resource_link` ブロックに頼らない (R-OUT-1 により捨てられる)

### 7.2 何を出し、何を出さないか

- R-SH-1 関数・メソッドでは**シグネチャを散文より先に**置く
- R-SH-2 `kind` は rustdoc の `ItemKind` の語 (`struct`・`function`・`assoc_type` など) に合わせ、impl と trait の中の関数だけを `method` にする (M0 の実装。初期案。v0.17)。メソッドや関連 item の見出しにも、親の型、trait (あれば)、crate、バージョンを含める (ferritin は「Function lock」としか出さない)。trait は別の欄にせず、impl の見出し (`impl<T> Trait for Type`。where 句も付ける) で示す (欄を分けると同じ情報が 2 回出る。M0 で決定。v0.17)
- R-SH-3 **impl は「辺」として扱う**: impl や implementor の項目には、その辺に固有の情報 (impl の見出し、where 句、関連型・関連定数の束縛、独自のドキュメント) だけを載せ、trait のメソッドを implementor ごとに繰り返さない (繰り返すと `Iterator` の実装者 1 つで 18KB になった先例がある)
- R-SH-4 blanket impl の既定の扱い (Q-9。eval で調整する):
  - core / alloc の汎用 trait (`From`、`Into`、`TryFrom`、`TryInto`、`Borrow`、`BorrowMut`、`Any`、`ToOwned`、`ToString`、`CloneToUninit`) の blanket impl は、where 句を省いて**名前だけを 1 行に畳む** (隠さない)。条件付きのもの (`ToString` は `Display`、`ToOwned` と `CloneToUninit` は `Clone`、`Any` は `'static` が条件) は、その型が条件を満たす場合だけ載せる (rustdoc JSON の `impls` は条件で絞り込み済みなので、そのまま使う。Q-18)。例: `std_blanket: ["From", "Into", "TryFrom", "TryInto", …]`
  - `list_methods` では、これらに由来するメソッド (`to_string`、`to_owned`、`into` など) を名前だけ並べる
  - impl の一覧は出どころの順に並べる: その crate の中の impl → 依存 crate 由来の impl → std 由来の impl (ferritin に倣う)。同じ出どころの中は R-OUT-4 のとおり決定的に並べる
  - 汎用 trait 以外の blanket impl (`Instrument`、`StreamExt`、`VZip` など) は trait ごとに見出し 1 行とし、提供元の crate ごとにまとめる。メソッドは要求されたとき (`expand_blanket`) だけ展開する (条件付きの絞り込みは Q-18)
  - 根拠: axum の `WebSocketUpgrade` では impl 25 個のうち 16 個、tokio の `Mutex` では 20 個のうち 9 個が blanket impl。cargo-public-api の例では行数の 87% が auto / blanket / derive の impl
- R-SH-5 依存 crate と std の**非公開 item は出さない**。private items は workspace member に限り、可視性を付けて出す。trait の item と enum の variant は親の可視性を継承する (ferritin は trait メソッドを「Private」と誤表示する)
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
    "project_from": "CLAUDE_PROJECT_DIR",
    "target": "x86_64-unknown-linux-gnu",
    "format_version": 57,
    "features": ["full"]
  },
  "warnings": ["no docs.rs build for host target x86_64-apple-darwin; used the docs.rs default target"],
  "data": { "signature": "..." },
  "page": { "shown": 20, "total": 103, "has_more": true, "next_cursor": "..." },
  "hints": ["call list_methods on tokio::sync::Mutex to see all methods"]
}
```

- `source` は `docs.rs` / `local` / `std` のいずれか。**実際にデータを返した取得元**を書く (ferritin は docs.rs のキャッシュを LocalDependency と誤表示している。テストで固定する)
- `resolved_from` は `explicit` / `explicit_req` / `cargo_lock` / `cargo_lock_raw` (Cargo.lock を直接読んだもの。R-VR-2) / `external` (`build_external_docs` が記録した metadata。R-WS-10) / `git` (引数 `git` の crate) / `latest` のいずれか
- `project_from` は `argument` / `CLAUDE_PROJECT_DIR` / `cwd` のいずれか (R-MCP-3)
- `version`・`target`・`format_version` は**データ本体 (JSON の `crate_version`・`target.triple`・`format_version`) から読み**、要求値と食い違えば warnings に出す
- ローカルビルドと std では `toolchain` (`rustc -vV` の要約)、`built_at`、`stale`、`features_requested` / `features_effective` を加える
- ビルド中は応答の最上位に `status: "building"` と `retry_after_secs` を加える (エラーではない。付ける条件は R-OUT-9)
- `warnings`・`page`・`hints` は該当するときだけ出す
- モデル向けの文字列 (説明文・instructions・warnings・hints・エラーのメッセージ・prompt) は英語に統一する。CLI の人向けの出力とログのメッセージも英語にする (Q-19)

- R-OUT-9: **provenance はすべての成功応答に必須とする** (`status: building` の応答を含む)
  - 応答の骨格と下の状態を表す値 (`resolved_from` などの上の値の一覧を含む) は初期案で、M1 で outputSchema を作るときに確定する (ビルドの状態は M2。v0.16)
  - 応答の骨格 (v0.15。outputSchema はこれに従う): 1 つの crate の応答の provenance で必須にするのは `source`・`crate`・`version` (データが無いときは要求した値か解決した値)・`target` だけで (crate を持たないツール (`search_crates`・`rust_release`) と、複数の crate の応答の共通部分の必須の項目は、M1 で outputSchema を作るときに決める。v0.17)、データ本体から読む項目 (`format_version` など) は任意にする。状態の項目 (`status`・`retry_after_secs`・`building_reason`・`partial`) は応答の最上位に置く。`stale`・`stale_reason` は provenance に置く。`sources` の各項目は、1 つの crate の provenance の項目に、`status`・`stale_reason`・`building_reason`・`retry_after_secs` と、失敗したものなら `code`・`reason`・`retryable` を足したものにする。1 つの crate の応答でも、ビルド中にデータがあれば最上位の `status` を付けず、`stale_reason: rebuilding` と `retry_after_secs` にする (複数の crate の応答の `partial` とそろえる)
  - 複数の crate にまたがる応答 (`search_items`、`list_implementors` の `dependency_graph`、`list_crates`、`audit_dependencies` など) では、provenance を共通部分と、crate ごとの `sources` の配列に分ける (各項目の形は上の骨格のとおり)。範囲の一部がビルド中や取得中でも、結果が 1 件でもあれば `partial: true` (R-SR-5) で返し (ビルド中のものがあれば `retry_after_secs` も付ける)、`status: building` は結果が 0 件でビルド中のときだけにする。結果が 0 件でも、失敗や未取得の crate があれば `partial: true` にする。カーソルの束縛は R-OUT-3
  - 状態を表す値 (v0.15): `sources` の各項目の `status` は `ok`・`building`・`failed`・`unavailable` (応答の最上位の `status` は `building` だけ)。`building_reason` は `building` (ローカルビルドの実行中)・`waiting_for_lock` (cargo のロック。R-WS-2)・`waiting_for_slot` (ビルドの枠。R-SEC-7)・`fetching`・`indexing`。`stale` を付けたら `stale_reason` を `rebuilding` (再ビルド中。R-WS-4)・`inputs_changed` (外の記録の入力が変わった。R-MCP-3)・`ref_ttl_expired` (ブランチなどの解決が古い。§6.4)・`project_missing` (外のプロジェクトのディレクトリが消えた。扱いは M2。R-MCP-3) のいずれかで付ける

---

## 8. エラー設計

- R-ERR-1: ツールの実行時エラーは、プロトコルエラーではなくツールの結果として返す (`isError: true`)。LLM が分岐して回復できるようにするため (先行実装の多くは JSON-RPC のエラーか、成功扱いの「見つかりません」テキストを返していて、LLM が判別できない)
  - ツールの処理中の panic も呼び出し単位で捕捉して `internal_error` の `isError: true` の結果として返し、サーバーや他の呼び出しを止めない (SDK が panic をプロトコルエラーに変えると、mmgeorge と同じ失敗になる)。仕様はサーバー内部のエラーをプロトコルエラーの例に挙げているが、同じ理由で意図的にツールの結果にする
  - panic の後も動き続けるので、共有状態のロックには poison の無いロック (parking_lot や `tokio::sync` のもの) を使い、std の `Mutex` / `RwLock` は使わない (1 回の panic で poison になり、以後そのロックを取る呼び出しがすべて失敗する。clippy.toml の `disallowed-types` で禁じている)。共有状態は差し替えで更新し、panic した呼び出しの書きかけの状態が他の呼び出しに見えないようにする
  - `LazyLock` と `Once` も、初期化の関数が panic すると poison し、以後のアクセスがすべて panic する (回復できない)。初期化で panic しうる処理はせず、するなら poison しない `OnceLock` で行う (禁止はせず、レビューで守る)
- R-ERR-2: **エラー結果では `content[0].text` が自己完結していること**。Claude Code はエラー時に `structuredContent` を無視する (Issue #86032)。Claude Code は、text のブロックが複数あれば改行でつないでモデルに渡す (2.1.301 で 2026-10-16 に確かめた。画面の表示は確かめていない。2.1.303 では、エラーの text がモデルの側で `<error>…</error>` に囲まれて見えた。M0 の A9)。text ブロックは 1 つにまとめ、`code`、`message`、`retryable`、`suggestions` (次の一手)、`request_id` を JSON で入れる (`request_id` は、突き合わせる相手の R-OBS-1 のログと一緒に M1 から。v0.17)。該当するときだけ、`reason`・`found`/`supported`・`did_you_mean`・候補 (キーは `candidates`。初期案。v0.17)・診断 (R-WS-7)・失敗の時刻・`retry_after_secs` (再試行できるエラー) を足す (v0.15)。`request_id` は呼び出しごとに払い出し、R-OBS-1 のログにも同じ値を記録する (複数のセッションが同時に動いても、報告されたエラーとログの行を 1 対 1 に対応付けるため。gnufood の `diagnostic_id` に倣う)。1 万文字以内に収める (長いエラー本文は先頭と末尾だけが残る)

| code | 状況 | suggestions の例 | `retryable` |
| --- | --- | --- | --- |
| `crate_not_found` | crate が存在しない | 似た名前の crate | false |
| `invalid_argument` | 引数を解析できない、または引数どうしが食い違う (R-ARG-1) | 正しい書式 | false |
| `version_not_found` | 指定したバージョンが存在しない | 存在するバージョンの候補 | false |
| `ambiguous_version` | Cargo.lock に同じ crate が複数バージョンある | 候補と、それぞれを使っている依存元 | false |
| `ambiguous_path` | 同名の item が複数ある | 候補の正準パス | false |
| `docs_unavailable` | docs.rs に JSON が無い (照会のツールに実際のビルド target と違う `target` を渡し、外のツールの記録も無い場合を含む。R-MCP-3)、既定のプロジェクトの外で `build_external_docs` の記録が無いか壊れている (R-MCP-3・R-WS-10)、取り直しても元データが壊れている (R-CA-4)、cargo が `-p` で失敗する依存 (R-WS-5)、展開の上限を超えた (R-NW-3)、展開できないか JSON として解析できない (R-DS-2・R-CA-4)、範囲を判定できずにローカルビルドしなかった (R-MCP-3) | JSON がある近くのバージョン、ローカルビルドの可否 (既定のプロジェクトの外のコードなら `build_external_docs`。R-MCP-3)、`readme` (API の代わりに導入手順だけでも得られる。mmgeorge は docs.rs にビルドが無いとき README に切り替える) | `reason` による (`corrupt` は true、それ以外は false) |
| `unsupported_format` | 形式に非対応 (`found` と `supported` を含む) | サーバーの更新、ローカルビルド | false |
| `item_not_found` | パスが解決できない (探した経路を含む) | `did_you_mean` (類似度の下限あり) | false |
| `build_failed` | ローカルビルドに失敗した。`cargo metadata`・`cargo fetch` の失敗 (既存の例外に当たらないもの) と、ビルドの最大時間の超過も含む (R-WS-7。v0.17) | 診断の要約 | 一時的な失敗は true、決定的な失敗は false。`cargo metadata`・`cargo fetch` の失敗は常に true (R-WS-7) |
| `fetch_failed` | git の取得の決定的な失敗 (認証の失敗・リポジトリが無い・rev が無い。`reason` 付き。R-WS-7) | `auth` なら、まず URL の誤りを疑うこと (GitHub は無いリポジトリにも認証を求めるので、URL の誤りも `auth` になる。2026-10-16 に確認)。次に、既定のプロジェクトの依存は利用者の端末で `cargo fetch` を実行して資格情報を通すこと、外のツールの `git` は端末で `git ls-remote <url>` が通るかを確かめること。それ以外は URL と rev の確認。直したら、照会のツールは `refresh`、外のツールは同じ引数での呼び直しで再試行 | false |
| `toolchain_missing` | ローカルビルドに使う toolchain (R-WS-8) か、ローカルビルドに使う target の `rust-std` (R-TG-1) が手元に無い。または、std の JSON (R-STD-4)、std のソースに使う版の合う `rust-src` (R-SC-2) が手元に無く、取得もできない (v0.17) | 導入コマンド | false |
| `toolchain_too_old` | ビルドに使った toolchain が古く、crate の MSRV を満たさない | 使った toolchain の更新コマンド (`rustup update nightly` など) | false |
| `offline_miss` | オフラインでキャッシュに無い | キャッシュにあるバージョン | false |
| `network_error` | 一時的なネットワーク障害。docs.rs が想定外の HTTP ステータスを返したときもこれにする (v0.17) | 再試行 (想定外の HTTP ステータスでは、報告のしかたとサーバーの更新。R-ERR-3) | true (想定外の HTTP ステータスは false) |
| `internal_error` | 処理中の panic など、サーバー内部の不具合 (R-ERR-1) | ログの場所と報告のしかた (壊れたキャッシュは R-CA-4 でサーバーが取り直すので、`cache clear` は案内しない) | false |
| `project_not_found` | 決めたディレクトリにも直下の子にも `Cargo.toml` が無いか、既定のプロジェクトの候補が複数で `project` が無く、照会を続けられない (R-MCP-3)。指したプロジェクトのディレクトリが消えたときの扱いは M2 で決める (v0.16) | 候補の中から `project` で選ぶこと、workspace のディレクトリで Claude Code を開き直すこと、または `build_external_docs` で `project` を指すこと | false |
| `lockfile_outdated` | Cargo.lock が無いか、Cargo.toml と合っていない (R-VR-2) | Bash で `cargo fetch` を実行すると、ビルドせずに Cargo.lock が必要な分だけ更新される (`cargo check` はコンパイルまでするので、Cargo.lock を直すだけなら遅い。`cargo update` と、既存の lock への `cargo generate-lockfile` は全依存を新しい版に上げるので勧めない) | false |
| `busy` | 同時に処理する呼び出しの数が上限 (R-SEC-7) を超えた | 少し待って再試行 | true |

- `docs_unavailable` には原因 `reason` を付ける (v0.15。値の一覧は初期案で、M1・M2 で実装しながら確定する。v0.16): `no_json` (docs.rs にも外のツールの記録にも無い。docs.rs の 404 と 400 (版として解析できない値) はこれにする。ただし M1 からは、crates.io の sparse index で crate と版の存在を確かめて、無ければ `crate_not_found`・`version_not_found` を返す (docs.rs の 404 は両者を区別しないため。R-DS-6)。v0.17)・`no_record` (既定のプロジェクトの外で記録が無い)・`record_broken` (外の記録が壊れている)・`corrupt` (書き込んだ後に読み直すと内容ダイジェストが合わず、取り直しても合わなかった。ディスクの問題を疑う。30 秒後に再試行できる)・`build_unsupported` (`-p` で cargo が失敗する依存。R-WS-5)・`too_large` (展開の上限。R-NW-3)・`unparsable` (展開できないか、JSON として解析できない。先読みした形式が対応範囲内なのに逆直列化に失敗したもの (サーバーの不具合) を含む。範囲の外の形式は `unsupported_format`。R-DS-2・R-CA-4)・`scope_unknown` (範囲を判定できずにローカルビルドしなかった。R-MCP-3)・`target_unsupported` (独自の target で std を自分で作るプロジェクト。R-TG-1。v0.17)

- R-ERR-3: 否定的な結果のキャッシュは種類で分ける。「確定的に存在しない」(目安 1 時間)、「crate が見つからない」(公開直後の可能性があるので短め、目安 5 分)、「一時的な失敗」(目安 30 秒)。一時的な失敗を「存在しない」としてキャッシュしない。複数の取得元を順に試すとき、一時的な失敗は保留し、どの取得元も答えなかった場合だけ `network_error` として返す。docs.rs が想定外の HTTP ステータスを返したときも、一時的な失敗と同じく保留して次の取得元を試し、目安 30 秒だけキャッシュする (どこも答えなければ `network_error`、`retryable: false`、suggestions は報告のしかたとサーバーの更新。v0.17)
- R-ERR-4: JSON-RPC のエラーコードは、JSON-RPC 2.0 の標準コード (`-32700`、`-32600`〜`-32603`) と、仕様が定める `-32020`〜`-32022` だけを使う。独自のエラーはツール結果の `code` で返す。`-32020`〜`-32099` のうち仕様に無いコードと、廃止された `-32002`・`-32042` は出さない (MUST NOT)。`-32000`〜`-32019` も使わない (SHOULD NOT。`basic/index`)

---

## 9. 非機能要件

### 9.1 性能

測る指標と目安を次に挙げる。目標の値は、M1 (読み込みと照会) と M4 (依存グラフ全体の implementors) のベンチマークで測って決める (v0.16)。

| 指標 | 目安 |
| --- | --- |
| キャッシュが効いている `get_item` / `list_methods` | p50 < 50ms、p95 < 200ms |
| キャッシュが効いている巨大 crate (core の JSON は約 61MB) の読み込み | < 50ms。ferritin は rkyv の mmap で約 7ms を実現している (mmap 前の検証の分は Q-23) |
| docs.rs から取った JSON の正規化 (中規模の crate) | 正規化 (取得を除く) の時間を目標にする。取得込みの p50 < 3s は参考 (取得の時間は docs.rs と回線で決まるため。v0.17) |
| 依存グラフ全体の implementors (索引あり) | < 1s (時間と容量は Q-5) |
| 約 1GB の JSON (windows-rs) | プロセスを落とさない (扱う方式は Q-22。性能・メモリの試験ケースに含める) |

目標は gnu 版 (R-DIST-1) で測る。musl 版は標準のメモリアロケータが遅いので、M5 で測ってから扱いを決める (Q-27)。

- R-PF-1: JSON の解析結果を、mmap できるアーカイブ (rkyv など) として保存する。item は必要になったときに初めて取り出す。上流の `rustdoc-json-types` にある `rkyv_0_8` feature も評価する
- R-PF-2: 逆引き索引と検索索引も同じ場所に保存する

### 9.2 メモリ

- R-MEM-1: プロセスのメモリ上限を設定できるようにする (既定は 1GB)。mmap したページは数えない (カーネルが回収できるため)。いま処理中のクエリが使っている crate は捨てない。OS ごとに何を測るか (案: Linux は `RssAnon`、macOS は `phys_footprint`、Windows は private bytes) と、超えたときの捨て方は M1 / M5 で測って決める (v0.16)
- R-MEM-2: 解析直後の大きな構造体は、アーカイブを書き終えたら直ちに手放す。上限を超える単一の crate (windows-rs の約 1GB の JSON) でもプロセスを落とさない。扱えなければ理由付きで `docs_unavailable` (`reason: too_large`) を返す。扱う方式 (ストリーミングで索引を作るかなど) は Q-22 で決める (v0.16)

### 9.3 ディスクキャッシュ

- R-CA-1 配置: プロジェクトをまたいで共有する。場所は R-DIST-3 と同じ規則で決める

  | OS | 既定の場所 |
  | --- | --- |
  | Linux / macOS | `$XDG_CACHE_HOME/chatora-rustdoc-mcp/` (`XDG_CACHE_HOME` が未設定なら `~/.cache`) |
  | Windows | `%LOCALAPPDATA%\chatora-rustdoc-mcp\cache\` |

  - 環境変数 `CHATORA_RUSTDOC_MCP_CACHE__DIR` (設定の `cache.dir`。名前の規則は R-DIST-3) で場所を変えられるようにする
  - キャッシュ・state (R-DIST-3)・ログ (R-OBS-2) のディレクトリの権限と所有者は扱わない。umask のまま作り、所有者も確かめない (cargo の `target/` や `~/.cargo`、rust-analyzer と同じ。既定の場所はホームの下にあり、ホームの権限が他の利用者から守る。v0.15)。他の利用者が読み書きできる場所に置かないよう README で案内する (R-SEC-2)
  - state のディレクトリのもの (ビルドのプロセスの記録など。R-DIST-3) とログ (R-OBS-2) はキャッシュに置かない
- R-CA-2 キー: `{source}/{crate}/{version}/{target}/{format_version}.json` を元データとし (std の `{version}` には rustc の commit-hash を使う。nightly は同じ版の文字列が約 6 週間続くため)、派生物 (アーカイブ・索引・展開したソース) を同じ場所に置く
  - 派生物のファイル名とヘッダには、マジックナンバー、スキーマの版、format_version、アーキテクチャ、**元データの内容ダイジェスト**を含め、合わなければ作り直す (mtime だけに頼らない)
  - スキーマの版には、アーカイブ形式を決めるライブラリ (rkyv) の互換な版の系列 (例: 0.8 系) も含め、Cargo.lock の rkyv の版と一致することをテストで強制する。依存の更新でアーカイブの配置が黙って変わるのを防ぐ (ferritin の `RKYV_SERIES` に倣う)
  - ローカルビルドの元データの鍵は R-WS-4 の内容ダイジェストとする
  - 派生物は rustup の toolchain ディレクトリ (読み取り専用でありうる) やユーザーの `target/` に書かず、すべてキャッシュの下に置く
- R-CA-3 不変性:
  - docs.rs の完全一致バージョンの取得済み JSON は不変として扱う (JSON が無いという結果は R-ERR-3 の TTL に従う。再ビルドで現れることがある。R-DS-3)
  - 「latest の解決結果」や crates.io の index には TTL を付ける (1h)
  - ただし sparse index のファイルの TTL は 10 分とし (index の `cache-control: max-age=600` に合わせる)、TTL が切れたら、保存した ETag を `If-None-Match` に付けた条件付きのリクエストで確かめ直す (ETag が無ければ `Last-Modified` を `If-Modified-Since` に付ける)。`304` なら保存した中身をそのまま使って TTL を延ばし、`200` なら中身と ETag を置き換える。新しい版の公開に早く気付きつつ、変わっていない crate の取り直しを本文なしで済ませるため (serde の index は 164,874 バイトで、ETag を付けると `304` で 0 バイトだった。2026-10-19 に確認)。index から導く「latest の解決結果」も、index を確かめ直したときに導き直す。ミラー (R-CA-8) が条件付きのリクエストに応じなければ、毎回 `200` で取り直すだけで、結果は変わらない
  - ローカルビルドの結果は鮮度判定 (R-WS-4) で管理する
- R-CA-4 整合性:
  - **すべての書き込み** (元データの JSON を含む) は一時ファイルに書き、fsync してからアトミックに rename する。一時ファイルの名前はプロセスごと・書き込みごとに一意にする (PID だけ、固定の `.tmp` だけでは衝突した先例がある)
  - アーカイブの展開 (`.crate` など) は最後に完了印のファイルを書く。印が無ければ未完了として作り直す
  - 書きかけの一時ファイルと、中断したビルドの作業用ディレクトリは後始末する。他のプロセスが使っているものは消さない。判定の方式 (持ち主のプロセスの生死か、古さか) と時期は M1 で決める (v0.16)
  - 同じキーへの同時書き込みとビルドは、**プロセスをまたぐファイルロック**で排他する (M2 から。M1 のキャッシュは原子的な rename だけで、複数のプロセスが同じキーを重ねて取得するのは許す。結果は同じで壊れないため。v0.16)。取得とビルドは、ロックを取ってからキャッシュを確かめ直し、既にあればそれを使う (待っていた側が、相手の終えた取得やビルドを繰り返さないように。v0.15)
  - **版の違うサーバーの同居** (v0.15): プラグインの更新の後は、古いセッションのサーバーと新しいサーバーが同じキャッシュと state を同時に使う。サーバーが書く記録は形式の版を持ち、版の違う記録は壊れたとみなさず、無いものとして扱う (互いに取り直しを繰り返さないため)。ロックの名前には形式の版を入れない (版の違うサーバーどうしでも排他するため)。版をどこに入れるか、対象の記録の一覧、古い版の記録の片付けは Q-29 で決める (v0.16)
  - **プロセスをまたぐロックの原則** (v0.15):
    - ロックはすべて OS のロック (Linux・macOS は `flock`、Windows は `LockFileEx`) で取る。ファイルがあるかどうかでロックを表さない
    - ロックファイルは GC でも `cache clear` でも消さない (消すと、消される前に開いて待っていたプロセスと、後から作り直したプロセスが両方ともロックを取れてしまう。空のファイルなので溜まっても小さい)
    - 置き場所、複数のロックを取る順、target dir のロックの取り方 (ビルドと GC の排他) は Q-29 で決める (v0.16)
  - mmap して検証なしで読むことはしない。rkyv の `bytecheck` で検証するか、長さとチェックサムを持つヘッダで確かめる (途中で切れたファイルを mmap すると未定義動作や SIGBUS になる)
  - 派生物は「壊れていても安全」であること。読めなければ元データから作り直す
  - 元データ (docs.rs・std の JSON、ローカルビルドの JSON) は、書き込むときに内容ダイジェストを記録する。読むときにダイジェストが合わなければ壊れたとみなし、捨てて 1 回だけ取り直す (ローカルビルドなら作り直す。外のツールで作ったものは R-WS-10)。取り直しても壊れていれば `docs_unavailable` を返し、照会のたびには取り直さない (否定キャッシュ)。取り直したことは warnings とログに出す (キャッシュの回復を人やエージェントの `cache clear` に頼らず、どの transport でも効くように)。取得時の検証に通らない場合、ダイジェストが合うのに解析に失敗する場合、非対応の形式の場合の分け方と、否定キャッシュの鍵と時間は、M1 でキャッシュを作るときに決める (v0.16)
  - キャッシュの読み書きでは symlink をたどらない。キャッシュの中に symlink を見つけたら、その派生物を壊れたものとして作り直す
  - Windows では、他のプロセスが mmap しているファイルは rename で置き換えることも削除することもできないので、派生物は上書きせず新しい名前で書く (R-CA-2)。GC での扱いは Windows の CI で試して決める (v0.16)
- R-CA-5 GC: 容量の上限を超えたら、最後に使ってから最も時間の経ったものから消す。上限は 2 つに分ける: 文書のキャッシュ (元データ・派生物・展開したソース・OSV のミラー) と、専用 target dir (R-WS-2。仮のプロジェクトのものと、外のツールの記録 (R-WS-10。記録した文書とメタデータ) を含む) で、target dir は最後に使ってから一定の日数を過ぎたものも消す (大きな target dir が文書のキャッシュを追い出したり、文書の GC が target dir を追い出してコールドビルドを繰り返したりしないため)。どれも設定で変えられる。初期値は 5GB・20GB・30 日で、M5 の実測で見直す (v0.16)。既定の上限と変え方を README に書く (書き方は M5)。GC の最小限 (CLI の `cache gc` と容量の上限) は、target dir が増え始める M2 で入れる (v0.16)。CLI に `cache ls` / `cache gc` / `cache clear` を用意する (サーバーが動いていなくても GC と後始末ができるように。ロックはサーバーと同じものを取る)。`cache clear` に `crate@版` を渡すとその分だけ消す (不変として扱う docs.rs の JSON が壊れていて、後から直った場合などに取り直すため)
  - キャッシュのルートに**所有マーカー** (このツールが作ったことを示すファイル) を置き、マーカーの無いディレクトリは消さない。設定の誤りで別のディレクトリを指したときの誤削除を防ぐ (ruskel の `cache/layout.rs` に倣う)
  - キャッシュのルートに `CACHEDIR.TAG` を置き、バックアップツールの対象から外れるようにする
  - 削除では symlink をたどらない。ロック中 (ビルド中) の専用 target dir は消さない
- R-CA-6: 任意の設定として、プロジェクトの Cargo.lock にある依存を、サーバーの起動時にまとめて先に取得する (既定は無効)
  - 任意の設定として、Cargo.lock の内容ダイジェストが変わったら、増えた依存だけを裏で取得する (既定は無効。rust-mcp の自動索引の利点)
- R-CA-7 オフラインモード: ネットワークに一切出ない (cargo を呼ぶときは `--offline` を付ける。R-VR-2)。キャッシュに無ければ `offline_miss` を返す。`cargo metadata` が失敗したときの版の解決は R-VR-2。TTL の切れた解決結果 (latest・index) もキャッシュにあれば使い、取得した日時を provenance に載せて、古い可能性を warnings で示す
- R-CA-8 隔離環境 (任意。M5 で検討): 取得元 (docs.rs・crates.io の sparse index と API・static.crates.io・static.rust-lang.org・GitHub API・OSV) のベース URL は、今のうちから 1 か所で持つ (後から社内のミラーに差し替えられるように)。差し替えと取得元ごとの無効化の設定、プロキシ、`http://` のミラーの扱いは M5 で決める (v0.16)

### 9.4 ネットワークの作法

- R-NW-1: crates.io の [data access policy](https://crates.io/data-access) と [crawler policy](https://crates.io/policies#crawlers) に従う。**連絡先を含む User-Agent** を送り (先行実装はどれも連絡先が無い)、毎秒 1 リクエスト程度に抑える。この間隔はプロセスごとに守る (対象は crates.io の API。sparse index は CDN なので対象外。複数のセッションのサーバーが同時に動くと合計は超えうるが、API を使うのは crate の検索と DL 数・被依存数などの取得くらいで量が少なく、プロセスをまたいで揃える仕組みに見合わないため受け入れる。v0.16)
  - crates.io の情報の取得元は、sparse index (`index.crates.io`) → static.crates.io の `.crate` → API の順に選び、API は前の 2 つで取れないもの (crate の検索、DL 数、被依存数と被依存の一覧、版ごとのライセンス) だけに使う。data access policy が示す順で、sparse index にはレートリミットが無く、static.crates.io にも今のところ無い (「at present」の条件付き。2026-10-19 にポリシーの原文で確認)。項目ごとの振り分けは §5.9 の metadata の行
- R-NW-2: docs.rs、crates.io の API と sparse index、static.crates.io、static.rust-lang.org、GitHub API、OSV へのリクエスト (User-Agent は R-NW-1 と同じ。crates.io の API の間隔は R-NW-1。crates.io を足したのは v0.16):
  - 接続と本体の受信を分けたタイムアウト。既定は接続 10 秒、本体の受信は無通信の時間で 120 秒 (全体の時間では数えないので、core の約 61MB や windows-rs の約 1GB も、遅い回線で途中で切らない)。設定で変えられる
  - 再試行は一時的エラー (429・5xx・通信エラー) だけ。R-DS-2・R-ERR-3 の一時的エラーもこの範囲を指す。`Retry-After` に従い、指数バックオフで行う
  - 同じリクエストの重複抑止
- R-NW-3: 本体は上限付きのストリームで読む (以下の数値は初期値で、M1 で測って決める。v0.16)。メモリ上で展開する場合の上限は 512MiB とし (core の約 61MB は収まる)、超えるもの (windows-rs は約 1GB) はディスクへ直接展開する。ディスクへの展開にも総サイズの上限 (`limits.extract_max_size`。既定 4GiB) を設け、超えたら打ち切って部分ファイルを消し、理由付きの `docs_unavailable` を返す。エラー時の本体は 16KiB まで
- R-NW-4: GitHub token は任意とする。ログには出さない。設定 (`github.token`) でだけ受け、環境変数 `GITHUB_TOKEN` は読まない (他のツール向けに置かれた token を勝手に使わないため。v0.15)。token は GitHub API のベース URL (R-CA-8) へ送る。信頼したリポジトリの `.claude/settings.json` の `env` は、その URL (と設定ファイルの場所) も変えられるが、同じリポジトリは hook で token を読めるので、このサーバーでだけ止める守りは設けない (R-SEC-2 の考え方。v0.15)。このことを README に書く

### 9.5 セキュリティ

- R-SEC-1: **stdout には JSON-RPC 以外を一切出さない**。ログは stderr かファイルに出す (mcp-rustdoc-parser にはこの不具合がある)。clippy の `print_stdout` だけでは守れない (`io::stdout()` への直接の書き込みや、依存 crate の出力は lint に掛からない)。cargo・rustdoc などの子プロセスは stdin を null、stdout と stderr をパイプにして起動し、サーバーの stdin / stdout を継承させない (build.rs や cargo の出力が JSON-RPC に混ざる、または子が JSON-RPC の入力を読み取るのを防ぐ)。1 メッセージを 1 行の compact JSON で出し、メッセージの中に改行を含めない (`basic/transports/stdio` の MUST NOT)。結合テストで、`serve` の stdout の各行が単独で解析できる JSON-RPC のメッセージであることを確かめる (ローカルビルドを伴う呼び出しを含める)
- R-SEC-2: **ローカルビルドと cargo の起動は、確認を求めずに行う** (v0.14 で決定。R-MCP-3)。ローカルビルドは build.rs と proc-macro を実行し、`cargo metadata` と `rustc -vV` もプロジェクトの設定を通じて任意の実行ファイルを動かしうる (`rust-toolchain.toml` の `[toolchain] path` は両方に効き、`.cargo/config.toml` の `build.rustc` と `build.rustc-wrapper` は (`--no-deps` でない) `cargo metadata` に効く。2026-10-15 に実測)
  - **守りの範囲の考え方** (v0.14): 比べる相手は、同じことができる他の経路 (エージェントの Bash、rust-analyzer、利用者自身の cargo)。それらで同じくらい簡単にできることを、このサーバーでだけ止める守りは設けない。stdio の MCP サーバーは起動した相手を信頼する作りで、エージェントは Bash で cargo を実行できる。さらに、プロジェクトの Cargo.toml に依存を書けば、rust-analyzer (IDE か Claude Code の LSP で動いている場合) がそれを取得して build.rs を確認なしに、サンドボックスの外で動かす (path 依存と `file://` の git 依存で確かめた。crates.io の依存とネットワークの git 依存も、同じく `cargo metadata` で取得されると推定。rust-analyzer 0.3.3081、2026-10-16)。v0.13 までの確認とそれを支える仕組み、子プロセスの環境変数の denylist はやめた (一覧は §14 の v0.14)。守るのは、エージェントの意図によらない脅威 (第三者のデータ、ネットワーク、同じマシンの他の利用者、事故) と、このサーバーにしか無い目立たない経路 (下の外のツール) だけ (R-SEC-1・R-SEC-3・R-SEC-4・R-SEC-5・R-SEC-6・R-SEC-7・R-SC-2・R-SC-4・R-CA-4・R-WS-4・R-DIST-3。R-SEC-6 は v0.16 で足した)。同じマシンの他の利用者からの守りは、キャッシュ・state・ログの置き場所 (ホームの下にあることと、共有の場所に置かない README の案内) に任せ、サーバーはディレクトリの権限を扱わない (R-CA-1。v0.15)
  - **外のコードを引き込む操作は、別のツールにする** (v0.14 のレビューで決定): 既定のプロジェクトの外のコードのビルド (R-WS-10 の対象) は、他の経路 (Cargo.toml を書き換えて rust-analyzer に拾わせる) と違って、Edit の許可も差分も要らず、「文書を見る」に見える 1 回の呼び出しで済む。攻撃者は、docs.rs のビルドだけをわざと失敗させて (環境変数 `DOCS_RS` で見分けられる)、ローカルビルドに落とせる。そこでこれらは `build_external_docs` だけが行い、利用者が Claude Code の許可ルールでこのツールにだけ確認を出せるようにする。Claude Code の許可ルールは、MCP のツールを引数では分けられない (括弧付きのルールは読み飛ばされる。文書で確認。2026-10-16)。`_meta["anthropic/requiresUserInteraction"]` は付けない (理由は付録 B)
  - **ツールの引数で指した場所のビルド**: git の場合、取り込んだリポジトリの cargo の設定と toolchain の指定は効かない (仮のプロジェクトの git 依存としてビルドするため。R-WS-2・R-WS-5)。`project` で指した外の workspace では、その workspace の設定が効く。clone は cargo が行うので、非公開のリポジトリは利用者の git の認証で取れる
  - **Claude Code の Bash のサンドボックスは MCP サーバーを覆わない** (Claude Code の文書: 「Claude's file tools, MCP servers, and hooks run outside it」。ローカルの MCP サーバーは利用者の全権限で動く)。サーバーが起動する cargo もサンドボックスの外で動く
  - **子プロセス (cargo・rustdoc) には、サーバーの環境をそのまま渡す** (v0.14 で denylist をやめた。ただしビルドの制御のために外す・足す・上書きする変数は別: R-WS-2 の target dir、R-WS-6 の問い合わせの抑止、R-WS-7 の `LC_ALL`、R-WS-8 の `RUSTC_BOOTSTRAP`・`RUSTUP_AUTO_INSTALL`、R-WS-1 の `RUSTC`・`RUSTDOC`。秘密を除く目的の denylist は設けない。v0.17): 悪意のある build.rs は親プロセスの環境を読める (Linux の `/proc/<pid>/environ` など) うえ、同じ依存の build.rs を rust-analyzer や利用者の cargo がすべての環境変数付きで動かすので、このサーバーでだけ除いても守りにならない。`*_API_KEY` のような型で除くと、`env!` で鍵を埋め込むアプリの workspace の文書のビルドを壊す
  - 任意で、OS のサンドボックス (Linux の bubblewrap / Landlock、macOS の `sandbox-exec`) の中でビルドするモードを検討する (v1 では対象外。Windows には実用的な手段が無い)
  - 依存の crate のドキュメントは既定で docs.rs から取る (docs.rs はネットワーク無し・読み取り専用のサンドボックスでビルドしている)。ただし workspace を rustdoc にかけると、その**依存もすべて check され、依存の build.rs と proc-macro が実行される** (2026-10-14 に確認)。ローカルで実行されるコードは、workspace とその依存すべてに及ぶ (§5.1)
  - **MRTR を使う場合**: 今はどの要求にも `InputRequiredResult` を返さない (確認をすべてやめたため)。将来使う場合は、仕様の MUST と SHOULD (`requestState` の完全性など。`basic/patterns/mrtr`) に従う (v0.16)
  - **README に書くこと** (要点。文面は M5 で README を書くときに決める。v0.16)
    - 照会のツールは、既定のプロジェクト (R-MCP-3) のコードを、確認なしに、Claude Code のサンドボックスの外でビルドする (build.rs と proc-macro を実行する)。外のツール `build_external_docs` は、エージェントが指したそれ以外のコード (R-WS-10) をビルドする
    - ビルドしなくても `cargo metadata`・`cargo tree`・`rustc -vV` を起動し、プロジェクトの設定が任意の実行ファイルを動かしうる (`cargo tree` も `build.rustc`・`rustc-wrapper` を動かすことを 2026-10-16 に確かめた)。信頼できないリポジトリで Claude Code を開いて使わず、`build_external_docs` でも指させない
    - 許可ルールの書き方: `allow` に `mcp__chatora-rustdoc-mcp__*` を、`ask` に `mcp__chatora-rustdoc-mcp__build_external_docs` を書く (`ask` を書かずに `*` だけを許可すると、外のコードも確認なしにビルドされる)。これは手動で登録したときのツール名で、プラグイン (R-DIST-5) で入れたときは接頭辞が `mcp__plugin_<プラグイン名>_<サーバー名>__` になる (R-MCP-7) ので、README には両方の登録のルールを書く。名前が合わないと ask が当たらず、許可の画面で「常に許可」を選ぶと外のビルドも確認なしに走る。実際の接頭辞は M5 で Claude Code で確かめる (v0.17)。ask のルールが効かない権限モードがある (端末の `bypassPermissions` と VS Code の auto。実測と追っている Issue は [claude-code-issues.md](claude-code-issues.md))
    - 同じことはこのサーバーが無くてもできる (Cargo.toml に依存を書けば rust-analyzer が build.rs を動かし、エージェントは Bash で cargo を実行できる)
    - キャッシュを他の利用者も書ける場所 (`/tmp` など) に置かない。キャッシュ・state・ログは umask のまま作る
    - 信頼したリポジトリの `.claude/settings.json` の `env` で、このサーバーの設定も変えられる (2026-10-16 に確かめた)
    - 信頼できない PR を扱う CI では使わない (依存の build.rs が CI の秘密を持ったまま動く)
    - Read の deny を設定していても、ソースはこのサーバー経由で読める (パッケージの中身 (R-SC-4) と、`include_str!` などで文書の JSON に入ったファイル)
    - `tools` を列挙している subagent には MCP のツールが渡らないので、`tools` に足す (2026-10-15 に文書で確認)
- R-SEC-3: ソースの読み取りは既知のソースルートの下に限る (R-SC-4)
- R-SEC-4: crate 名 (crates.io の名前の文法) とバージョン (semver) を、ファイルパスや URL に使う前に検証する (先行実装にはツール引数からのパストラバーサルがある)
  - target triple も、キャッシュのパスに使う前に検証する (syphar は検証せずに使っている)
  - git の URL (引数 `git`) は `https://`・`ssh://`・`http://`・`git://` を受ける。scheme の無い形 (scp の形 `[user@]host:path` を含む) は `invalid_argument` にし、`ssh://` での書き方を案内する (scp の形は cargo も受けない。2026-10-16 に実測。v0.16)。平文の `http://`・`git://` は通信の途中で取得するコードを差し替えられうるので、その旨を warnings と provenance に出す (拒否はしない。同じ URL を Cargo.toml に書けば cargo も rust-analyzer も取得する (2026-10-16 に実測) ので、このサーバーでだけ止めない。v0.15)。`file://` は受けない (ローカルのパスは `project` で指すため)。`https://`・`http://`・`git://` の URL に userinfo (`user@`・`token@`・`user:pass@`) があれば拒否し、`ssh://` の URL はパスワードがあれば拒否する (provenance やログに資格情報が漏れるため)。URL と `project` のパスに、制御文字・双方向の制御文字・見えない書式文字があれば拒否する (`build_external_docs` の許可の確認画面 (利用者が `ask` を書いた場合) や provenance・ログで、ホストやパスを偽装させないため)。`rev` はコミットのハッシュか `refs/…` として、`branch`・`tag` は git の参照名として検証する。`-` で始まる `tag` など (タグ名には付けられる) は拒否しない (エージェントが渡す値への守りで、同じものを Cargo.toml に書かれれば効かない。v0.14)
  - **feature 名**を cargo の引数に渡す前に検証する。cargo の feature 名の文法に合わないもの、`-` で始まるもの、制御文字を含むものは拒否する (cargo の引数として誤って解釈されないように。snowmead の `features.rs` に倣う)
  - 索引 (SQLite などを使う場合) への問い合わせはすべてパラメータ化し、引数の値を文字列連結しない (MasYaaz は `kind` を SQL に連結しており、インジェクションが可能)
- R-SEC-5: HTTP モードを自分用の常駐プロセスとして使う場合 (M6 以降。R-MCP-1) は、localhost に bind し、`Origin` / `Host` ヘッダを検証する。`Origin` があって許可していない値なら 403 を返す (MUST。DNS rebinding で、ブラウザから localhost に届くのを防ぐ)
  - 認証の有無 (localhost のポートには同じマシンのどのプロセスと利用者からもつながる。仕様の Authorization は SHOULD)、扱えるプロジェクトを限るか、プロジェクトの受け渡し (ヘッダか引数か) は M6 で決める (v0.14 の「最初の段階では認証を設けない」「扱えるプロジェクトは限らない」を v0.16 で M6 に戻した)。Claude Code のプラグインの設定なら、HTTP サーバーの `url` / `headers` でも `${CLAUDE_PROJECT_DIR}` が置換される (v0.16)
  - `cacheScope` をアクセス制御の代わりにしない
- R-SEC-6: サーバーが通信する先は、R-NW-1・R-NW-2 に列挙した取得元 (と R-CA-8 で設定したミラーとプロキシ) に限る。子プロセスの cargo の通信のうち、利用者のプロジェクトが設定したレジストリや git の依存は対象外で、cargo の設定に従う。rustup の自動導入の通信も子プロセスのもので、R-WS-7 のとおり扱う。引数 `git` の取得も子の cargo が行い、ホストは限らない (秘密を URL に入れて持ち出す経路になりうるが、同じことは Cargo.toml に git の依存を書けば rust-analyzer でもできる。R-SEC-2)。テレメトリや利用統計は機能として持たず、ツールの引数と応答の中身を第三者のサービス (LLM の API を含む) に送らない
- R-SEC-7 (`server/tools` の Security の MUST):
  - **出力のサニタイズ**: 第三者の文字列 (README・doc・changelog・ソース・crates.io の説明・OSV の advisory・リリースノート・manifest の文字列) とビルドの診断から、制御文字 (改行とタブを除く)、ANSI エスケープ、Unicode の双方向の制御文字 (U+202A〜U+202E・U+2066〜U+2069) と双方向の印 (U+200E・U+200F・U+061C)、ゼロ幅の文字 (U+200B〜U+200D・U+2060)、異体字セレクタ (U+FE00〜U+FE0F・U+E0100〜U+E01EF)、タグ文字 (U+E0000〜U+E007F。人には見えずモデルにだけ読める指示に使われる) を除いて返す (仕様は「Sanitize tool outputs」とだけ定め、何を除くかは決めていないので、ここで決める)。サニタイズは出力の直前の 1 か所で、すべての文字列 (属性の文字列、const の値、`doc(alias)`、ファイル名、`did_you_mean`・hints・エラーの本文に埋め込む値を含む) に一律に適用する
  - **呼び出しのレート制限** (`completion` の Security の MUST も兼ねる): 同時に処理するツール呼び出しの数、ローカルビルドの数、`completion/complete` の頻度に、設定できる上限を設ける (初期値は呼び出し 16、ローカルビルド 2、補完の頻度は R-MCP-19。どれもプロセスごとに数える。複数のセッションが同時にビルドすると CPU を奪い合って遅くなるが、結果は正しく、同じ target dir のビルドは cargo のロックで 1 つずつになるので受け入れる。v0.16)。ツール呼び出しが上限を超えたら、ビルドを待っている場合は `status: building`、それ以外は `busy` (`retryable: true`) を返す

### 9.6 可観測性

- R-OBS-1: ツール呼び出しごとに、構造化ログ (JSON Lines) をファイルに出す。記録する項目は、`request_id` (R-ERR-2)、ツール名、引数 (パス以外は要約)、遅延、応答のバイト数と推定トークン数、エラーコード、キャッシュの当否、取得元。OTLP だけに頼らない (syphar は呼び出しごとのファイルログをやめて OTLP だけにした結果、手元で分析できなくなっている)
  - 推定トークン数は文字数からの換算で出す。換算係数の校正の手順は M5 の eval (R-QA-6) で決める (v0.16)。**実行時には count_tokens などの外部の API を呼ばない** (応答の中身を外部に送ることになる)
- R-OBS-2: ログの出力先とローテーション (ファイル名と数値は初期値で、実装時に決める。v0.16)
  - 既定の場所は state のディレクトリ (R-DIST-3) の `logs/`: Linux / macOS は `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs/` (未設定なら `~/.local/state`)、Windows は `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs\` (キャッシュの下に置くと `cache clear` で消える)。設定と環境変数で場所を変えられる
  - プロセスごとに 1 ファイル (`<日付>-<PID>.jsonl` など) に書く。複数のセッションのサーバーが同時に動いても、行が混ざらず、書き込みの排他も要らない。1 ファイルの上限を超えたら次のファイルに切り替える (初期値 20MB)
  - 保持期間と合計の上限 (初期値 14 日・50MB) を超えた古いログを消す。他のプロセスが書いているファイルと、所有マーカー (R-CA-5) の無いディレクトリのファイルは消さない。どれも設定で変えられる。判定の方式と GC の時期は実装時に決める (v0.16)
  - ログにはツールの引数 (パスはそのまま、それ以外は要約) が残り、プロジェクトのパスには利用者名が入りうるので、ログの場所と記録する項目を README に書く (トークンは出さない。R-NW-4)

### 9.7 テストと評価

- R-QA-1 スナップショット:
  - **モデルに見せる形**の出力を、ツールごとにスナップショットテスト (insta) で固定する
  - **テストデータごとにサイズの予算 (バイト数) を検査する**。`Vec` で 26KB のような退行を CI で落とす。R-OUT-3 の上限は上限であって目標ではない (survey で冗長と判定した 46ki75 の `tokio::sync::Mutex` の 28,850 文字も、この上限には収まる)。代表的な item (`tokio::sync::Mutex`、`std::vec::Vec` など) ごとに `detail=normal` の予算を定める。値は M1 で ferritin のホスト版の出力と比べて決める
- R-QA-2 テストデータ:
  - docs.rs で実在を確認した各 format_version (46、48、53、54、55、56、57、60、61。一覧は `testdata/docs-rs-json.txt`。v0.17) の JSON
  - 300 段以上のネスト (typenum 相当)
  - 二重 zstd 圧縮の JSON
  - cfg の痕跡属性の 3 系統の表記
  - umbrella crate (compio、bevy)、glob で互いを re-export する crate 群、private module 内の公開型
- R-QA-3 docs.rs と crates.io はモックサーバーに置き換え、CI をオフラインで回せるようにする
- R-QA-4 **nightly canary**: 定期実行の CI で最新の nightly で JSON を生成し、次を早期に検知する
  - 新しい format_version
  - `Attribute::Other` の中の cfg の表記の変化 (同じ形式の中でも変わる)
  - std のスナップショットの変化
  - beta (次の stable) の toolchain に crate 名を指定した `RUSTC_BOOTSTRAP` (ローカルビルドの既定。R-WS-8) を付けて、合成の workspace をローカルビルドし、形式と受け付けの変化を検知する (v0.17)
  - あわせて **live の docs.rs に対する smoke test** を回し、docs.rs が新しい形式で配り始めたことや、URL の挙動 (R-DS-6) の変化を検知する
  - 失敗したら Issue を自動で開き、直ったら自動で閉じる (ferritin の nightly ジョブに倣う)。**失敗を放置しない** (46ki75 は live の CI が 5 週続けて失敗したまま放置され、その間に現行の形式が読めなくなっている)
- R-QA-5 説明文とスキーマの検査 (CI):
  - 説明文と instructions の長さ (2,048 文字以内。§6.2)、戻り値の説明と代わりのツールを示す節の有無
  - 説明文・instructions・prompt・同梱の skill に出てくるツール名が、すべて実在すること (§6.2。`tools.profile` の構成ごとに確かめる)
  - すべての JSON Schema が標準どおりであること (R-MCP-10)。ルートに combinator が無いこと、null 許容の型が無いこと
  - ツールのフル名 (プラグインの接頭辞込み) が 128 文字以内、プロパティ名が R-MCP-7 の文法に合うこと
  - **宣言した引数が実装で使われていること**。引数ごとに、値を変えると結果が変わるテストを持つ (rustdoc-mcp の `list_crates` は `workspace_member` を宣言しただけで使っていない)
  - annotations が R-MCP-4 の前提と合っていること
  - 照会のツールが、既定のプロジェクトの外で子プロセスを一切起動しないこと (外のプロジェクトに罠の `rustc-wrapper` と toolchain の `path` を置いて確かめる。R-MCP-3)
  - 照会のツールが、範囲の外の依存の build.rs を動かさないこと (罠の依存は path 依存にするか docs.rs のモックで 404 を返し、印のファイルを書く build.rs を置く。他の target でだけ使う依存、host と違う `build.target` での target 用の build-dependency、member の docs.rs の feature の組でだけ有効になる optional の依存を、resolver 1 と 2 の workspace で試す。同じ罠を範囲の中の依存にも置き、印ができること (陽性の対照) も確かめる。R-MCP-3。v0.15)
  - `output.mode = structured` では、全ツールの `structuredContent` を宣言した `outputSchema` で検証する (`status: building`・各 `detail`・`page` 付き・`partial` の応答を含む。R-OUT-1)
  - **説明文が約束する動作**: 説明文の「答える問い」に書いた問いごとに、そのツールで実際に答えられることを確かめる結合テストか eval のケース (R-QA-6) を 1 つ以上持ち、対応の欠けを検出する (syphar は引けないメソッドを `get_item` へ誘導し、dmvk は未実装の implementors を説明文で約束している)
  - README に出てくる MCP のツール名も検査の対象にする。許可のルールの例が、手動の登録とプラグインの登録の両方で実在のツール名に当たることを確かめる (R-SEC-2。v0.17)
  - §8 のエラーコードごとに、MCP の応答で `isError: true` が付き、`content[0].text` が自己完結した JSON であることを結合テストで確かめる (ferritin は `isError` を計算して捨てている)
  - MCP の適合性テスト (公式のテストスイートか MCP Inspector の CLI) を、対応するすべての版 (v1 では 2026-07-28 だけ) で CI に載せる。`initialize` に対応する版を名指ししたエラーを返すことも確かめる
- R-QA-6 **eval**: §3 の問いを元に問題集を作り、Claude Code (headless) で回す
  - 測るもの: 期待どおりのツールを選んだか、呼び出し回数、応答の総トークン数、答えが正しいか
  - トークンは 3 つに分けて測る: ① セッション開始時の固定費 (tool search が有効ならツール名と instructions、無効なら全ツールの定義)、② ツールの結果ごとの p50 / p95、③ タスク全体の総トークン数と呼び出し回数。換算には R-OBS-1 の校正値を使う
  - 必ず含めるケース: メソッドのパス (`Vec::iter`、`str::len`)、umbrella crate、feature で隠れた item、「X は Send か」、ビルド中の応答
  - 比較する設定: `output.mode` (structured / text / toon)、`tools.profile` (full / core)、`detail` の既定値、`alwaysLoad` の有無、`max_chars` の有無
  - 基盤には Claude Code の `claude plugin eval` を使う。プラグインあり・なしの比較 (Δ) が取れ、`--json` で CI に載せられる

### 9.8 配布と設定

- R-DIST-1: Linux / macOS / Windows を同等に扱う。配布の仕組みは ferritin に倣う
  - crates.io (`cargo install --locked chatora-rustdoc-mcp`) と、GitHub Releases のビルド済みバイナリで配る。README に正規の入手先を明記する (名前の似たプロジェクトや無断コピーが多い。survey §3)
  - 配る target (musl 版を含めるか。Q-27)、ビルド済みバイナリの作り方 (dist・`cargo binstall`・インストーラー)、リリースの流れ (release-plz など)、公式 MCP レジストリへの登録、依存の更新の bot、リリース前後の CI の検査、成果物の sha256 と attestation は M5 で決める (v0.16)
  - CI は 3 つの OS で、ビルド・テスト・clippy・rustdoc (`RUSTDOCFLAGS="-D warnings"`) を回す。OS 依存のコード (`std::os::unix` など) は lint では見つからない。toolchain は浮動の stable で、lint の多くを deny にしているので、6 週ごとの新しい stable で lint が増えるとコードを変えなくても CI が落ちうる。そのときはその都度直す (版を固定しない。v0.13 で決定)
  - crate は 1 つにし、MCP サーバーと CLI (`cache`) を同じバイナリに入れる (R-DIST-2)。パッケージ・lib の crate・バイナリ・MCP の serverInfo・設定とキャッシュと state のディレクトリ・環境変数の接頭辞 (`CHATORA_RUSTDOC_MCP_`)・README で勧める MCP サーバーの登録名は、すべて `chatora-rustdoc-mcp` にそろえる (v0.17)。ferritin が workspace で crate を分けているのは、CLI/TUI と MCP サーバーが別のバイナリだから。分けると、バイナリが依存する crate もすべて crates.io に公開する必要がある
- R-DIST-2: サブコマンドは `serve` (stdio)、`serve --http` (M6 以降。R-MCP-1)、そして人が使う CLI。CLI で照会はしない (エージェントは MCP を使い、人は docs.rs で足りる。CI で公開 API を検査するなら cargo-public-api がある。v0.14 でやめた)
  - `cache ls` / `cache gc` / `cache clear [<crate@版>]` (R-CA-5)
  - `install-skill` (任意。R-DIST-6)
  - CLI はエージェントに案内しない (skill は MCP のツールを使う。R-DIST-5)
- R-DIST-3: 設定ファイルの場所

  | OS | 既定の場所 |
  | --- | --- |
  | Linux / macOS | `$XDG_CONFIG_HOME/chatora-rustdoc-mcp/config.toml` (`XDG_CONFIG_HOME` が未設定なら `~/.config`) |
  | Windows | `%APPDATA%\chatora-rustdoc-mcp\config.toml` |

  - macOS も `~/Library/Application Support` ではなく XDG の規則に従う。開発者向け CLI の多くがそうしており、Linux と案内を揃えられる。[etcetera](https://crates.io/crates/etcetera) の `choose_base_strategy` (Windows では Windows の規則、それ以外では XDG) がこの規則と一致する
  - パスを `HOME` から自前で組み立てない (Windows には `HOME` が無い)
  - ビルドのプロセスの記録 (R-WS-6) は、state のディレクトリに置く (ロックファイルの置き場所は Q-29): Linux / macOS は `$XDG_STATE_HOME/chatora-rustdoc-mcp/` (未設定なら `~/.local/state`)、Windows は `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\`。書き込みは R-CA-4 と同じく、一時ファイルからの原子的な置き換えとロックで行う。キャッシュ (R-CA-1) と分け、GC と `cache clear` の対象にしない (動いているビルドの記録を消さないため)
  - 環境変数 `CHATORA_RUSTDOC_MCP_CONFIG` で利用者の設定ファイルの場所を変えられ、個々の設定値も環境変数で上書きできる。環境変数の名前は `CHATORA_RUSTDOC_MCP_` に、キーを大文字のスネークケースにしたものをつなぐ (表の入れ子は `__` でつなぐ。例: `network.offline` は `CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE`)。環境変数で受けられる値の制約は実装時に決める (v0.16)
  - **設定は、利用者の設定ファイルと環境変数だけ** (v0.14 で、プロジェクト単位の設定ファイル `.config/chatora-rustdoc.toml`・`.local.toml` をやめた。Q-24): 書く項目のほとんどはマシンや利用者の都合で決まるもので、プロジェクトに固有のもの (ローカルビルドの feature の組と rustdoc の `--cfg`) は Cargo.toml の `[package.metadata.docs.rs]` (R-FT-3・R-WS-1) に、nightly の指定は `rust-toolchain.toml` に、もともと書く場所がある。clone した第三者のリポジトリが、このサーバー独自の設定ファイルで取得元や上限を変える経路も無くなる (cargo の設定・`rust-toolchain.toml`・`[package.metadata.docs.rs]` と、Claude Code で信頼したリポジトリの `.claude/settings.json` の `env` は効く。R-SEC-2)。同じキーは環境変数が勝つ。設定ファイルは TOML。知らない `CHATORA_RUSTDOC_MCP_*` の環境変数 (`CHATORA_RUSTDOC_MCP_CONFIG` は数えない) と設定ファイルの知らないキーは、起動を止めず、応答には出さずにログで知らせる (綴りの誤りで設定が黙って効かないのを防ぐ。応答に出すと R-ARCH-5 の「同じ入力なら同じ出力」が崩れる。v0.15)。SessionStart の hook でも知らせるかは M5 で決める (R-DIST-5。v0.16)
    - **読む時期**: MCP サーバーは起動時に読み、動いている間の書き換えは再起動まで効かない。CLI は呼び出しごとに読む
  - **設定項目の一覧** (暫定。最初の公開リリース (M5) で凍結し、以後は README で案内するので変えない。実装してみて要らないキーや名前の変更は、それまで自由に行う。v0.16)。容量は単位付きの文字列 (`"5GB"` など)、時間は単位をキー名に入れた整数 (`_secs` など) で書く (単位の一覧などは実装時に決める。v0.16)。1h の TTL (R-CA-3、R-STD-6) と sparse index の 10 分の TTL (R-CA-3) は固定値で、設定項目にしない

    | 項目 | キー | 既定 | 要件 |
    | --- | --- | --- | --- |
    | 取得元のミラーのベース URL | `sources.<取得元>.url` (取得元は `docs_rs`・`crates_io_index`・`crates_io_api`・`static_crates_io`・`static_rust_lang`・`github_api`・`osv`) | 公式の取得元 | R-CA-8 (実装した場合。M5 で検討) |
    | 取得元ごとの無効化 | `sources.<取得元>.enabled` | `true` | R-CA-8 (実装した場合。M5 で検討) |
    | プロキシ | `network.proxy` | なし (標準の環境変数 `HTTPS_PROXY` などに従う) | R-CA-8 (実装した場合。M5 で検討) |
    | オフラインモード | `network.offline` | `false` | R-CA-7 |
    | 通信のタイムアウト (接続 / 受信) | `network.connect_timeout_secs` / `network.read_timeout_secs` | 10 / 120 | R-NW-2 |
    | GitHub token | `github.token` (環境変数 `GITHUB_TOKEN` は読まない) | なし | R-NW-4 |
    | cargo・rustdoc・rustc の実行ファイルの絶対パス | `build.cargo_path` / `build.rustdoc_path` / `build.rustc_path` | なし (rustup があればその toolchain、無ければ PATH のもの。R-WS-1) | R-WS-1・R-TG-1 |
    | ローカルビルドを docs.rs より優先するモード | `build.prefer_local` | `false` | R-DS-1 |
    | `--cfg docsrs` を渡すか | `build.cfg_docsrs` | `true` (壊れたら渡さずにやり直す。Q-11) | R-WS-1 |
    | nightly の日付の固定 | `build.nightly_date` | なし (固定しない) | R-WS-8 |
    | 1 回のビルドの最大時間 | `build.timeout_secs` | 1800 | R-WS-6 |
    | `build_failed` に付ける診断の件数 | `build.max_diagnostics` | 2 (1 件 3,500 文字、合計 8,000 文字以内) | R-WS-7 |
    | キャッシュの場所 | `cache.dir` | OS ごとの既定 (R-CA-1) | R-CA-1 |
    | GC の上限 (文書のキャッシュ / 専用 target dir / target dir の保持日数) | `cache.docs_max_size` / `cache.target_max_size` / `cache.target_max_age_days` | `"5GB"` / `"20GB"` / 30 | R-CA-5 |
    | 起動時の一括取得 / Cargo.lock の変化での裏の取得 | `prefetch.on_startup` / `prefetch.on_lockfile_change` | `false` / `false` | R-CA-6 (任意) |
    | dist から std の JSON を取るか / stable の分岐日に近い nightly を選ぶか | `std.fetch_from_dist` / `std.match_stable_branch_date` | `true` / `false` (後者は任意で M4 で検討。R-CA-8 を実装して `sources.static_rust_lang.enabled = false` にしたら、`std.fetch_from_dist` によらず取らない) | R-STD-6 |
    | ツールの構成 (`full` / `core`) | `tools.profile` | `"full"` | R-MCP-12 (M5 で検討) |
    | 出力の形式 (`structured` / `text` / `toon`) | `output.mode` | `"structured"` | R-OUT-1 (text・toon は M5) |
    | 応答の上限 | `output.max_chars` | 32000 (推定 8,000 トークン) | R-OUT-3 |
    | `detail` の既定 | `output.detail` | `"normal"` | R-OUT-7 |
    | 検索の既定の範囲 (`crate` / `workspace` / `workspace_and_direct` / `dependency_graph`) | `search.scope` | `"workspace_and_direct"` | R-SR-2 |
    | changelog で返す直近のリリースの数 | `changelog.max_releases` | 10 (応答の上限まで) | §5.9 |
    | メモリの上限 | `limits.memory` | `"1GB"` | R-MEM-1 |
    | 同時に処理する呼び出し・ローカルビルドの数 / 補完の頻度の上限 (1 秒あたり) | `limits.concurrent_calls` / `limits.concurrent_builds` / `limits.completions_per_sec` | 16 / 2 / 20 (初期値) | R-SEC-7・R-MCP-19 |
    | ディスクへの展開の上限 | `limits.extract_max_size` | `"4GiB"` | R-NW-3・R-SC-2 |
    | 1 リクエストあたりの時間の上限 | `limits.request_timeout_secs` | 30 | R-ARCH-6 |
    | ログの場所 / 1 ファイルの上限 / 保持期間 / 合計の上限 | `log.dir` / `log.file_max_size` / `log.retention_days` / `log.total_max_size` | `~/.local/state/chatora-rustdoc-mcp/logs/` など / `"20MB"` / 14 / `"50MB"` | R-OBS-2 |
    | stderr のログの絞り込み (tracing の `EnvFilter` の書き方。R-OBS-1 の呼び出しごとの記録は絞らない。誤った指定が 1 つでもあれば全体を既定に戻し、そのことをログに出す) | `log.filter` | `"info"` | R-SEC-1 (v0.17) |

- R-DIST-4: ライセンスは MIT OR Apache-2.0 とする (`LICENSE-MIT` / `LICENSE-APACHE`)
  - 他の実装のコードを流用するのは、MIT か MIT OR Apache-2.0 のものに限り、出典と著作権表示を `THIRD-PARTY-NOTICES` に残す。AGPL (possdog)・LGPL (KevinT3Hu)・Apache-2.0 のみ (dmvk、46ki75)・ライセンス本文の無いもの (syphar、Xevion) は、設計の参考にとどめてコードを写さない
  - 依存のライセンス・既知の脆弱性・取得元・禁じる crate を、CI の cargo-deny (`licenses`・`advisories`・`sources`・`bans`) で検査する
- R-DIST-5 **Claude Code プラグイン**として配布する:
  - プラグインの名前とサーバーの名前は、どちらも `chatora-rustdoc-mcp` にする (v0.17。許可のルールの名前は R-SEC-2)
  - `plugin.json` の `mcpServers` でサーバーを起動する (`command: chatora-rustdoc-mcp serve`)
  - skill を 1 つ同梱する: いつ使うかの対応表 (MCP のツールを使う。CLI は案内しない)、LSP との境界、`rustc --explain` / `cargo clippy --explain` の案内。Rust の文脈 (`.rs` と `Cargo.toml`) に限定する (書き方は M5。v0.16)。バージョンの取得元を正確に書く (ferritin の skill は「プロジェクトで解決された版」と書きつつ既定は docs.rs の latest という食い違いがある)
  - SessionStart hook では、前提が欠けていれば 1 行で知らせ、揃っていれば何も出さない。何を確かめるか (案: バイナリ、std の取得手段 (手元の `rust-docs-json` か R-STD-6 の dist)、プラグインと手動設定の重複登録、知らない設定、実行ファイルのパスの組み合わせ (R-WS-1)) と確かめ方 (バイナリに確かめるためのサブコマンドを足すかなど) は M5 で決める (v0.16)
  - 任意で、依存の監査などのコマンドを同梱する
  - プラグインと手動設定の両方で登録するとツールが重複することを README に書く
  - 利用者ごとの設定はプラグインの `userConfig` で受け取る (項目は M5 で決める。v0.16)。token は `sensitive` を指定する
  - キャッシュは `${CLAUDE_PLUGIN_DATA}` に置かない (プラグインのアンインストールで消えるうえ、CLI やプラグイン外の起動と共有できない)。R-CA-1 の場所を使う
- R-DIST-6 skill の単独インストール (任意。M5): プラグインを使わない利用者向けに、CLI で skill を入れられるようにする。利用者が編集した skill を黙って上書きしない (akesson/docsrs に倣う)。引数の形は M5 で決める (v0.16)

### 9.9 LSP との役割分担

- Claude Code には rust-analyzer の `LSP` ツールが組み込まれている。**位置 (行・桁) に基づく問いと、この会話の中で編集中のコード**は LSP の担当とする (workspace の JSON の鮮度は R-WS-9)
- 本サーバーの担当は、**パスで引く、バージョンを固定した API リファレンス**、crate 全体の構造、横断的な検索、依存と std のドキュメント
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
  - 文脈が無い場合、バージョン解決は「明示指定 > latest」になる。プロジェクトで有効な feature の注記などの項目は、応答から省く
- R-ARCH-4 文脈が無いと意味をなさないツール・引数・引数の値は、ローカル機能が無い構成では `tools/list` (ツールの一覧と `inputSchema`) に出さない
  - 対象の一覧は、ローカル機能が無い構成を作るときに決める (例: `list_crates`、`project` 引数、`build_external_docs`。v0.16)
  - 出しておいてエラーを返すと、LLM がそのツールを選んでしまうため。出す・出さないは構成で決まるので、R-MCP-5 (毎回同じ順) とも矛盾しない
- R-ARCH-5 結果が「バイナリの版、入力、データの識別子」だけで決まるようにする
  - セッションごとの状態が結果に影響しないようにする (R-MCP-3 を強化したもの)
  - 応答に map を入れるときはキーで並べる (`HashMap` をそのまま直列化しない)。serde_json の `preserve_order` (R-OUT-1) の下では、map の順が挿入の順になり、`HashMap` 由来の値の順がプロセスごとに変わるため (v0.17)
  - CI で検査する: 同じ入力を、別々のプロセスで、間に別の問い合わせを挟んで実行し、出力が一致することを確かめる (map を含む応答を必ず入れる)
  - これを守ると、ホスト版で ETag による HTTP キャッシュを導入できる。ferritin は、応答の本体を作る前に識別子から ETag を決める方式で、読み込みなしに約 1.5ms で 304 を返している
- R-ARCH-6 資源の上限を、どの構成でも設定できるようにしておく
  - 対象: メモリ (R-MEM-1)、1 リクエストあたりの時間、応答サイズ (R-OUT-3)
  - 1 リクエストあたりの時間の上限は既定 30 秒 (R-WS-6 のビルドもこの上限で待つのをやめる。HTTP の最初の応答バイトまでの時間制限にも収まる)。超える処理 (索引の作成、依存グラフ全体の取得など) は裏で続け、`status: building` (複数の crate にまたがる応答で結果が 1 件でもあれば `partial: true`。R-OUT-9) と `retry_after_secs` を返す (v0.15)
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
| MCP SDK | [rmcp](https://crates.io/crates/rmcp) 3.5.1 / tower-mcp 0.24.0 (`protocol-2026-07-28` feature。v0.16 の時点は 0.23.2) / rust-mcp-sdk 2.0.0 / pmcp 2.22.7 / [mcplease](https://crates.io/crates/mcplease) 0.4.2 (版は M0 で比べたもの。v0.17) | **rmcp ≥ 3.5.1**。3.0.0 (2026-07-28 リリース) から 2026-07-28 版 (stateless、`server/discover`、SEP-2243 のヘッダ、緩和された `structuredContent`) に対応している。2026-07-28 の機能は v0.10 で確認した (discover・MRTR・`RequestStateCodec`・elicitation・キャッシュのヒント・tasks 拡張・subscriptions・prompts・completions・resource templates・`title`・icons はあり、Skills 拡張は無し)。v1 は MRTR も elicitation も使わないので、cargo feature の `request-state` と `elicitation` は要らない (将来 MRTR を使うなら `request-state`。R-SEC-2)。mcplease は stdio が同期的で 1 リクエストずつしか処理せず、進捗通知もキャンセルも無いので R-WS-6 と両立しない。rmcp の `CallToolResult::structured` は text にも同じ JSON を入れるので、R-OUT-1 と合う。引数の解析エラーが自動で -32602 になる点は R-ARG-1 のとおり自前の検証に替える。M0 で、同じ JSON-RPC を流して tower-mcp・rust-mcp-sdk・pmcp と比べ、rmcp を続けると決めた (tower-mcp は panic の扱いと引数の扱いが良いが、`server/discover` のキャッシュの指定と `initialize` の拒否の文言を変えられず (R-MCP-5・R-MCP-2)、7 か月で破壊的な minor が 16 回ある。rust-mcp-sdk はキャンセルした要求にも応答を送る (R-WS-6 の MUST NOT)。pmcp は stdio で 2026-07-28 を話さない。比較の表は m0-plan.md の §6.1。受け入れる rmcp の挙動は R-MCP-2。v0.17) |
| データ層 | 自作 / [ferritin-common](https://crates.io/crates/ferritin-common) 0.18.0 (MIT OR Apache-2.0) を依存として使う / [rustdoc_processor・rustdoc_resolver](https://github.com/LukeMathWalker/officina) 0.1.2 (Apache-2.0 のみなので、依存にはできるがコードは流用しない。R-DIST-4。Pavex から分離。cargo rustdoc → SQLite キャッシュ → 公開 import path と crate 外の re-export の索引) | **自作**。rustdoc_processor は R-PR-2 の公開パス索引と同じ問題を実運用で解いているので、索引の設計とテストケースの比較対象にする。M0 で評価し、依存にしないと決めた (rustdoc-types を 0.57 に固定していて形式 57 しか読めない、guppy・SQLite (bundled)・rayon・rkyv などで依存が重い、API が cargo rustdoc の起動を前提にしている)。同じ docs.rs の JSON で 11 crate の公開パスを突き合わせると、違いは rustdoc_processor の側 (マクロを索引に入れない、`use` で再エクスポートしたモジュール自身のパスを記録しない) と正準パスの規則の違い (改名) だけで、両方に共通する誤り (`pub use … as _`) が 1 つ見つかった (m0-plan.md の §6.1 の A10。突き合わせは poc/m0a-a10 でやり直せる。v0.17)。形式の正規化・rkyv の sidecar・逆引き索引・検索の調整値は ferritin の設計を踏襲するが、ferritin には要件と食い違う既知の欠陥が多い (付録 A の ferritin の行) |
| JSON の形式 | `rustdoc-types` の最新 + 独自 shim | §5.2 のとおり。形式ごとの差分は [migrate_rustdoc_json](https://github.com/BD103/migrate_rustdoc_json) の移行コードと rustdoc-types の CHANGELOG で洗い出す |
| JSON パーサー | serde_json (`unbounded_depth`、`preserve_order`) + serde_stacker | sonic-rs は 255 段の上限があり使えない。`preserve_order` は応答のキーの順のため (R-OUT-1・R-ARCH-5。v0.17) |
| アーカイブ | rkyv (`bytecheck` かチェックサムで検証。R-CA-4) | rkyv の mmap は ferritin で実績がある (R-PF-1)。ただし ferritin は検証を省いており、検証込みの読み込み時間は Q-23 で測る |
| 検索 | 自作の BM25 / tantivy / SQLite FTS5 | 自作の BM25 (ferritin の調整値)。tantivy の既定のトークナイザは CamelCase を分割しない |
| シグネチャの描画 | 自作 | ruskel の型描画表 (`dyn` / `impl` の括弧、1 要素タプル、修飾パス) と cargo-public-api の最良パス選択を参考にする。ruskel の「書かれたままのパス」「名前による derive 判定」「`pub` だけの可視性」は真似しない |
| メタデータ | `cargo_metadata` crate | — |
| changelog の解析 | `parse-changelog` crate | syphar の実績 |
| HTTP | reqwest (rustls、暗号は aws-lc-rs) + zstd | M0 で採用。aws-lc-rs は C のビルドが要るので、3 つの OS での `cargo install` を R-DIST-1 の CI で確かめる (M0 で確かめたのは Linux だけ。v0.17) |
| 非同期 | tokio | — |
| ロック | parking_lot / `tokio::sync` | poison が無い (R-ERR-1) |

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
| Q-11 | ローカルビルドで `--cfg docsrs` を渡すかの既定値 | 既定で渡し (docs.rs と同じ)、壊れたら渡さずにやり直す (R-WS-1)。v0.13 で決定。壊れた crate を記録して次から渡さないかは M2 で決める (v0.16)。M0 で測ったやり直しの頻度は、このリポジトリの依存 136 個中 1 個 (rustls-platform-verifier。`--cfg docsrs` を前提にした crate 自身の `docsrs` feature が、プロジェクトの組で有効でなかった)。stable と nightly で同じ (v0.17) |
| Q-12 | stable + crate 名を指定した `RUSTC_BOOTSTRAP` を、ローカルビルドの既定にしてよいか | 既定にする。値は専用 target dir ごとに、依存の解決結果のすべての lib・proc-macro の crate 名に固定する (値を変えると proc-macro2 から 54〜65 unit が作り直される。固定すれば作り直しは文書化する crate だけ)。値を R-WS-4 の鍵に入れる。M0 で決定 (R-WS-8。v0.17) |
| Q-13 | crate ごとに `default-target` を上書きしている場合に、docs.rs が使った triple を JSON の `target.triple` だけで判別できるか | できる。`default-target` を上書きした windows-registry 0.100.0 と、`targets` を並べた winreg 0.56.0 で、`target.triple` が実際にビルドした triple だった。ビルドしていない target を名指しすると 404 で、別の target に差し替えられない (2026-10-16 に確認。R-TG-1・R-TG-2) |
| Q-16 | OSV のミラーの容量と更新の手間 | 小さい。`crates.io/all.zip` は 3.6MB で毎日更新される (2026-10-15 に HEAD で確認)。1 日 1 回、条件付きのリクエストで取り直す (R-AU-3)。v0.13 で解決 |
| Q-17 | `cargo doc --output-format json` と `cargo rustdoc` の違い | `cargo rustdoc` を使う。`cargo doc` は 2 回目が速く依存をまとめて作れるが、nightly の cargo かプロセス全体への `RUSTC_BOOTSTRAP=1` が要り、同名で版の違う依存は片方しか文書化されない。nightly で依存をまとめて文書化する用途の候補として M2 に残す。M0 で決定 (R-WS-1。v0.17) |
| Q-18 | rustdoc JSON の型ごとの `impls` が、条件付きの blanket impl を絞り込んでいるか | 絞り込んでいる。Display の無い型には `ToString` が、Clone の無い型には `ToOwned` と `CloneToUninit` が載らない (2026-10-15 に stable 1.99、形式 61 で確認)。R-SH-4 は JSON の `impls` をそのまま使えばよい。ジェネリックな型での扱いは M1 のテストデータで確かめる。v0.13 で解決 |
| Q-19 | モデル向けの文字列の言語 | 英語に統一する (説明文・instructions・warnings・hints・エラーのメッセージ・prompt。CLI の人向けの出力とログも英語)。v0.13 で決定 |
| Q-20 | target ごとの依存・build-dependency・dev-dependency で feature が分かれる crate を、どの組でビルドするか (R-FT-3) | `cargo tree --workspace --target <t> -e normal` の組 (`cargo rustdoc --lib` が使う組と、resolver 1 でも 2 でも一致した)。M0 で決定 (R-FT-3。v0.17) |
| Q-21 | 新しく足された依存の初回のローカルビルドに確認を求めるか | v0.8 で求めると決めたが、v0.14 で確認をすべてやめた (同じことを rust-analyzer やエージェントの Bash でもできるため。R-SEC-2) |
| Q-24 | 設定ファイルのプロジェクト単位の上書きを許すか | v0.13 で許すと決めたが、v0.14 でやめた。プロジェクトに固有のものは Cargo.toml の `[package.metadata.docs.rs]` と `rust-toolchain.toml` で足りる (R-DIST-3) |
| Q-26 | CLI のスクリプト向けの出力形式と出力の上限 | v0.14 で照会の CLI をやめたので、決める必要が無くなった (R-DIST-2) |
| Q-30 | M0 で確かめる cargo の挙動 | すべて M0 で決めた (v0.17): host で `build.target` も無ければ `--target` を付けない (R-WS-1)。依存の種類は stable の `cargo tree` で判定し、package id は `cargo metadata` に対応付ける (R-MCP-3)。ローカルビルドの経路での `cargo metadata` の失敗は、例外 (`lockfile_outdated`・`fetch_failed`・`offline_miss`) に当たらなければ `build_failed` (一時的) にまとめ、版の解決は Cargo.lock の直読みで続ける (R-WS-7・R-VR-2)。git の決定的な失敗は libgit2 と CLI の文言で判定し、子に `LC_ALL=C` を渡す (R-WS-7)。通信の途中で切れた本体は zstd の展開より先に HTTP の層で失敗し、一時的な失敗になる (`content-length` がある場合。m0-plan.md の A4)。`rust-std` の有無は sysroot で先に判定する (R-TG-1)。実行ファイルの一部だけの指定は同じディレクトリから補い、rustup が無ければ PATH のものを使う (R-WS-1)。target の JSON 仕様は v1 では対応しない (R-TG-1)。resolver 2 以上で `-p` が panic する依存は起動前に判定する (R-WS-5) |

### 11.2 残る未決事項

| # | 内容 | 決め方 |
| --- | --- | --- |
| Q-5 | 依存グラフ全体の implementors にかかる時間と容量 (先行例なし) | 設計とベンチマーク |
| Q-14 | cfg の痕跡属性の表記が今後どう変わるか | nightly canary (R-QA-4) で追う |
| Q-15 | `alwaysLoad` の効果、`inputSchema` の `examples` がモデルに見えるか | eval |
| Q-22 | 解析後にメモリ上限 (R-MEM-1) を超える巨大な crate (windows-rs) を扱う方式 (案: 全体を構造体に読み込まずにストリーミングで索引にする。R-MEM-2) | 設計とベンチマーク |
| Q-23 | mmap 前の検証 (R-CA-4 の bytecheck やチェックサム) を数百 MB のアーカイブにかけても、読み込みの目標 (§9.1 の < 50ms) に収まるか。ferritin の約 7ms は検証を省いた値 | ベンチマーク |
| Q-25 | cargo#16720 (`cargo doc --output-format=markdown`) が実装されたら、取得元に加えるか、本サーバーの役割を見直すか | 上流の動きを見て判断する |
| Q-27 | メモリアロケータ (mimalloc など) を替えるか。musl 版 (R-DIST-1) の性能が §9.1 の目標から大きく遅れる場合と、gnu 版でも parse と解放が速くなる場合 (M0 の A8 で、`get_item` の時間はほぼ parse と解放だった) | M1 で gnu 版を測り、musl 版は M5 で測って決める (v0.17) |
| Q-31 | ローカルビルドの細部で、M0 で決めきれず M2 に回したもの (v0.17): `--target` を付けない形 (R-WS-1) での M0 の B1・B3・B5 の再確認と、同じ package が host 用と target 用の両方に現れるときの feature の組 (R-FT-3)。仮のプロジェクトの toolchain・`RUSTC_BOOTSTRAP` の値・target dir (R-WS-5)。鮮度の鍵を、要求した引数・feature の組と、やり直しで実際に使ったもののどちらで作るか (R-WS-4。`--cfg docsrs` と `[package.metadata.docs.rs]` の組のやり直し)。`RUSTC_BOOTSTRAP` の値の長さの上限 (Windows の環境変数は 32,767 文字。M0 では 238 名で 2,806 文字) と超えたときの扱い、nightly の経路で壊れたときに member の名前だけの値でやり直すか (R-WS-8)。`-Z fine-grain-locking` (R-WS-2) と `cargo doc --output-format json` (R-WS-1) を nightly の経路で使うか。`--cfg docsrs` で壊れた crate を記録するか (Q-11)。ほかに本文が M2 で決めるとしたもの (R-FT-3・R-WS-1・R-WS-7・R-MCP-3 など) も、M2 の計画で本文から拾う | M2 で実装しながら決める |
| Q-29 | プロセスをまたぐ仕組みの細部 (R-CA-4・R-WS-6・R-WS-10)。原則は要件で決め、細部は動くものを作りながら決める (v0.15。v0.16 で本文の細部をここへ移した。「案」は移す前に本文で決めていたもの)。**ロック**: 置き場所 (案: state の `locks/`。キャッシュだけを共有する構成で排他が効くか、state の rename の規則との関係)、名前の作り方と増え方、複数のロックを取る順 (案: 鍵 → target dir → ビルドの枠)、target dir のロック (案: ビルドは共有ロック、GC と切り替えた古い target dir の削除は排他ロック)、NFS 上の `flock`。**版の違うサーバーの同居**: 版をファイル名かパスのどこに入れるか、対象の記録 (元データの内容ダイジェストの記録、否定キャッシュ、外のツールの記録、ビルドの失敗の記録、R-WS-6 のプロセスの記録など)、元データのダイジェストの記録と版の関係 (記録が無いときの扱い)、古い版の記録の片付け (案: キャッシュの下は GC、state の R-WS-6 の記録は書いた版のサーバーが消し、読めない記録には触らない)。**残った子孫** (R-WS-6): 記録するもの (案: サーバーの PID と起動時刻、子のプロセスグループの ID とその起動時刻を state に記録し、ビルドが終わったら消す)、照合の規則 (案: 記録したサーバーが死んでいて、残ったプロセスが記録したグループに属し、その起動時刻がグループの長の起動時刻以降のときだけ止める)、判定の上限、子を spawn するスレッド (`PR_SET_PDEATHSIG` は spawn した**スレッド**の終了で発火するので、長命の専用スレッドから spawn する案。PoC で確かめる)、切り替えた target dir の名前とほかのプロセスへの伝え方。**進行中の印** (R-WS-10): 試し取りの手順と記録の確かめ直しの順、ビルドの枠を待つ間も印を持つか (案: 持つ)、git で要求した rev・branch・tag の鍵と解決したコミットの鍵の両方を持つか (案: 両方)、呼び直しのとき進行中のビルドが要求より前にブランチを解決していた場合 (案: 終わるのを待ってから解決し直す)。**その他**: 置き換えられた古いビルドが枠を占め続けること、Windows で mmap 中の派生物を作り直すときの名前 (R-CA-2 の名前は元データのダイジェストで決まるので、同じ元データから作り直すと同じ名前になる。R-CA-4。v0.16)、結合テストの一覧 (2 プロセス、版の違うサーバー、キャンセル、罠の build.rs) | M2 の PoC |

---

## 12. マイルストーン (案)

| 段階 | 内容 | 完了条件 |
| --- | --- | --- |
| M0 PoC (完了。2026-10-19。結果は [m0-plan.md](m0-plan.md) の §6) | stdio サーバー (rmcp) と `get_item` (docs.rs の JSON、形式の正規化 (54〜61)、公開パス索引込み)。ローカルビルドの PoC (`RUSTC_BOOTSTRAP`、`cargo doc --output-format json`、feature の組、`--cfg docsrs`、dep-info) | Claude Code から呼べる。R-OUT-1 の挙動を再確認。Q-12・Q-17・Q-20 と、R-FV-3 の先読みの方法、R-WS-4 の dep-info の取り方が決まる。Q-11 の、渡さずにやり直す頻度を測る。R-MCP-16 のサーバーから閉じるときの扱い、R-OUT-2 の膨張率、R-MCP-3 の `cargo tree` の出力の形の安定性、Q-30 の cargo の挙動、rustdoc_processor を依存にするか (§10) を確かめる |
| M1 MVP | 形式 46〜53 の shim (R-FV-2・R-FV-4)、バージョン解決 (§5.3)、`list_crates` / `resolve_version` / `crate_overview` / `get_item` / `search_items` / `list_module` / `list_methods` (固有と trait 由来。R-TI-1) / `list_impls` / `list_implementors` (crate 内のみ)、一覧の絞り込みと展開 (§6.5)、一覧への feature の注記 (R-FT-2)、定義位置 (R-OUT-8)、std (dist からの直接取得 R-STD-6 を含む)、正準パス、impl の構造的な分類、描画規則 (§7)、ディスクキャッシュ (原子的な書き込みだけ。プロセスをまたぐロックは M2。R-CA-4)、エラー設計。`search_items` の既定の範囲 (R-SR-2) のうち workspace の分は M2、`list_crates` の advisory の件数は M4 から | 依存 crate と std について、ferritin のホスト版より正確で小さい答えが返る (R-QA-1 の代表的な item のスナップショットで比較する。eval での比較は M5)。§3 の Q20 (umbrella crate) に答えられる。`--no-default-features` の構成でもビルドとテストが通る (R-ARCH-8) |
| M2 workspace | Q-29・Q-31 の細部を決める。ローカルビルド (R-WS-8 の toolchain の選び方、内容ダイジェストの鮮度判定、永続 target dir、プロセス間ロック、ビルド中の応答、ビルド失敗のキャッシュ)、private items、docs.rs に JSON が無い場合のローカルビルド、`build_external_docs` (R-WS-10)、CLI の `cache` と GC の最小限 (容量の上限。R-CA-5。v0.16) | 自分の workspace を引ける。編集後に正しく再ビルドされる。外のプロジェクトと git の crate を引ける |
| M3 crate 情報 | features (`inspect_features`。cfg の計算自体は M1 の R-FT-2) / README / changelog / examples / metadata / `search_crates` / `get_source` | §3 の Q10〜Q13・Q15・Q16 と、Q14 のうち changelog で答えられる部分に答えられる |
| M4 差別化 | 依存グラフ全体の implementors、Deref とプリミティブを含むメソッド解決、MSRV を踏まえた std 表示、`api_diff`、`audit_dependencies`、`rust_release`。任意: `search_source` (R-SC-6)、シグネチャ検索と同梱の散文の検索 (R-SR-1)、docs.rs のビルド時の依存の版との差の警告 (R-PR-5)、stable の版に近い nightly の `rust-docs-json` の選択 (R-STD-6) | 既存実装では答えられないか不正確な問い (§3 の Q7、Q9、Q14 の破壊的変更、Q17〜Q19) に答えられる。任意の `search_source` を入れた場合は Q21 も |
| M5 仕上げ | eval の問題集、性能目標、GC の既定値の見直し、Claude Code プラグインとしての配布、配布の方式 (配る target (Q-27)・リリースの流れ・公式 MCP レジストリに登録するかなど) を決める (R-DIST-1)。任意: resources (R-MCP-13)、prompts (R-MCP-18)、completions (R-MCP-19)、icons (R-MCP-7)、skill の単独インストール (R-DIST-6)、隔離環境 (R-CA-8)。eval で採否や既定を決めるもの: `output.mode` (R-OUT-1)、`detail` の既定 (R-OUT-7)、`alwaysLoad` (R-MCP-8・Q-15)、`max_chars` (R-OUT-3)、`tools.profile` (R-MCP-12) | eval の問題集で §3 の問いに答えられる。M1・M4 で測って決めた §9.1 の目標を満たす。GC が容量の上限を守る。Claude Code のプラグインとして 3 OS で導入できる (任意の機能と M6 は含めない。v0.16) |
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

### v0.17 (2026-10-19)

M0 の結果を反映した版 ([m0-plan.md](m0-plan.md) の §6 と、反映の一覧の §7.1)。

- 名前をすべて `chatora-rustdoc-mcp` にそろえた: パッケージ、lib の crate、バイナリ、MCP の serverInfo、設定・キャッシュ・state のディレクトリ、環境変数の接頭辞 (`CHATORA_RUSTDOC_MCP_`)、README で勧める MCP サーバーの登録名 (R-DIST-1・R-DIST-3・R-DIST-5・R-CA-1・R-OBS-2・R-SEC-2)。v0.3 で `chatora-rustdoc-mcp` から `chatora-rustdoc` に変えた理由 (CLI としても使う) は、v0.14 で照会の CLI をやめて人向けの `cache` だけになり、弱まった。§14 の過去の版の記述は当時の名前のまま残す
- **M0 で決めたことを反映した** (根拠と測った値は m0-plan.md の §6):
  - 未決事項: Q-12・Q-17・Q-20・Q-30 を §11.1 に移した。Q-11 にやり直しの頻度 (136 個中 1 個) を足した
  - ローカルビルド: 既定は stable + `RUSTC_BOOTSTRAP` で、値は解決結果の全 crate 名に固定する (R-WS-8)。`cargo doc` は使わない (R-WS-1)。host で `build.target` も無ければ `--target` を付けない (R-WS-1)。dep-info は rustdoc の `--emit=dep-info` で取り、`RUSTC_BOOTSTRAP` の値を鮮度の鍵に入れる (R-WS-4)。feature の組は `cargo tree -e normal --target <t>` (R-FT-3)。resolver 2 以上で `-p` が panic する依存は起動前に判定する (R-WS-5・R-MCP-3)。依存の種類は stable の `cargo tree` で判定する (R-MCP-3)。`rust-std` の有無は sysroot で先に判定し、target の JSON 仕様は v1 で対応しない (R-TG-1)。実行ファイルの一部だけの指定と rustup の無い環境の既定 (R-WS-1)。`cargo metadata` の失敗のまとめ方と、git の失敗の文言・`LC_ALL=C` (R-WS-7)。子プロセスでは GUI の askpass も止める (R-WS-6。M0 で git の CLI が利用者のデスクトップにダイアログを出した)
  - 形式と索引: 先読みの方法 (R-FV-3)、形式 54〜61 の shim (R-FV-2)、`pub use … as _` はパスを作らない (R-PR-2)
  - MCP と出力: rmcp で受け入れる挙動 (R-MCP-2・R-ARG-1)、stdio ではサーバーから listen を閉じる場面が無い (R-MCP-16)、`preserve_order` (R-OUT-1)、膨張率の実測とほかの crate の型の正準パスは M1 から (R-OUT-2)、where 句を `normal` に (R-OUT-7)、`source_location` の形と行のアンカー (R-OUT-8)、trait は impl の見出しで (R-SH-2)、`request_id` は M1 から (R-ERR-2)、docs.rs の 400 と想定外のステータス (§8)、`log.filter` (R-DIST-3)
  - 技術選定: SDK の比較 (rmcp を続ける)、rustdoc_processor は依存にしない (§10)
  - M0 を完了とした (§12)。`-Z fine-grain-locking` の評価を M2 に回した (R-WS-2)
- **反映した後の見直し** (前提を持たないレビュアー 4 人で並列に読み、食い違いと抜けを直した):
  - 判断したこと: `build_unsupported` の依存は仮のプロジェクトを作って外部ツールとしてビルドする (R-WS-5)。host は利用するビルドの toolchain の host とする (R-TG-1)。依存の鮮度の鍵は鍵の一部だけで作る (R-WS-4)。target の JSON 仕様と build-std は `target_unsupported` とする (R-TG-1・§8)。`full` では未加工の部分木を落とす (R-OUT-9)。plugin の名前は `chatora-rustdoc-mcp` で、両方の規則名を許し、CI で確かめる (R-DIST-5・R-SEC-2)。`log.filter` は stderr のログだけを絞り、誤った指定が 1 つでもあれば全体を既定に戻す (設定の表・R-SEC-1)
  - そのまま直したこと: dev だけの依存 (R-MCP-3)、`RUSTC_BOOTSTRAP` の値にメンバーの bin の名前を入れる (R-WS-8)、子プロセスの環境の例外 (R-SEC-2)、`cargo metadata` の失敗を返すのはローカルビルドの照会だけ (R-VR-2)、`build.*_path` の既定、`kind` の語彙 (R-SH-2)、map を整列する (R-ARCH-5)、beta と `RUSTC_BOOTSTRAP` の先行テスト (R-QA-4)、§9.1 の指標、§10 の JSON の parser・HTTP・SDK の版、M0 の形式の範囲 (§12)、Q-27 を広げた
  - 回したこと: 形式 46〜53 の shim を M1 に (§12)、`name` の無い要求の確認を M1 に (R-ARG-1)、ローカルビルドの細部を M2 に (Q-31 を足した)

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
