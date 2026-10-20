# M1 (MVP) の計画

- 作成日: 2026-10-19
- 状態: 計画。§5 の推す案は利用者が承認した (2026-10-19)
- 前提: [requirements.md](requirements.md) v0.18。M1 の内容と完了条件は §12。M0 の結果と M1 への申し送りは [m0-plan.md](m0-plan.md) の §6 と §7.2

---

## 1. 目的と範囲

M1 は、依存の crate と std の文書を、docs.rs と std の配布物から、版を合わせて引ける最小の製品にする段階。ローカルビルドはまだ無い (M2)。

**範囲** (要件の §12 の M1):

- 版の解決 (要件の §5.3) と、プロジェクトの文脈 (`cargo metadata`。R-VR-2・R-MCP-3)
- ツール: `list_crates`・`resolve_version`・`crate_overview`・`get_item`・`search_items`・`list_module`・`list_methods` (固有と trait 由来)・`list_impls`・`list_implementors` (crate の中だけ)
- 一覧の絞り込みと展開 (要件の §6.5)、一覧への feature の注記 (R-FT-2)、定義位置 (R-OUT-8)、正準パス、impl の構造的な分類、描画規則 (要件の §7)
- std (dist からの直接の取得を含む。R-STD-1・R-STD-6)、形式 46〜53 の shim (R-FV-2・R-FV-4)
- ディスクキャッシュ (原子的な書き込みまで。プロセスをまたぐロックは M2)、エラー設計、構造化ログと `request_id`
- 小さな eval と Claude Code の挙動の検知 (R-QA-6)、crates.io への最初の公開 (R-DIST-1)

**範囲の外**: ローカルビルドと workspace の文書 (private item を含む)・`build_external_docs`・プロセスをまたぐロック (M2)、feature・README・changelog・例・ソースのツール (M3)、Deref とプリミティブのメソッド・no_std のパス・依存グラフ全体の implementors・`api_diff` など (M4)、eval の比較と校正の仕上げ・プラグインの配布 (M5)。

**M1 の利用者への影響**: docs.rs は 2026-05-01 から、`targets` を書いていない crate を x86_64-unknown-linux-gnu だけでビルドする。求める target の JSON が無ければ M1 は `docs_unavailable` (`no_target_json`) を返す (R-TG-1) ので、Mac・Windows では新しいリリースの多くを引けない。M1 で公開する版は、実質的に Linux の利用者向けになる。README にそう書く。

## 2. 方針

- **ソースは利用者が書き、Claude は 1 段ずつ導いてレビューする**。各段は、やること・使う API (ferritin で確かめる)・要件と lint の落とし穴を示してから始め、終わりの印を満たしたらレビューして次へ進む
- **各段の終わりの印**には、`cargo +nightly fmt`・`cargo clippy --all-targets`・`cargo test` が通ることを常に含める (`RUSTUP_AUTO_INSTALL=0`)
- **層 (R-ARCH-1) と cargo feature `local` (R-ARCH-2) を守る**。cargo を起動するコード (`cargo metadata`・`cargo tree`・`rustc -vV`) は `local` の中に置き、`--no-default-features` でもビルドとテストが通るようにする (R-ARCH-8)
- **測った結果と決めたことは、日付と環境を付けて §6 に書く**。要件への反映は M1 の終わりにまとめて行う (§7)。要件の「M1 で決める」細部 (§4) は、それを使う段で決める。判断の要るものは、利用者に 1 件ずつ聞く
- **ホストの環境を変えない** (M0 と同じ)。足りない道具は入れる前に利用者に確かめる。外へ通信する実験では、利用者の git と cargo の設定と資格情報を使わない。外部への要求に、利用者のメールアドレスや名前を入れない (サーバーの User-Agent は製品のリポジトリの URL を含む)
- **公開と push は利用者が行う** (crates.io への公開、GitHub への push、Issue へのコメント)

---

## 3. 手順

段は 5 つ。M1a で M0 のコードの不具合を直して土台を固め、M1b でデータ層 (版と取得) を作り、M1c でツールを 1 本ずつ縦に作る (クエリ層・応答・説明文・テストまで通す)。M1d で横断的なものを仕上げ、M1e で eval・測定・公開を行う。M1c の最初の 2 本 (`get_item`・`resolve_version`) ができた時点で、小さな eval を一度回す (E1 の先行)。

### 3.1 M1a: 土台の修正 (M0 のコード)

m0-plan.md の §7.2 の申し送り。

| # | やること | 要件 | 終わりの印 |
| --- | --- | --- | --- |
| A1 | CI を作る (§5 の P2)。cargo feature `local` の枠を作る | R-ARCH-2・R-ARCH-8・R-QA-4 | CI が通る。`--no-default-features` のビルドとテストが通る |
| A2 | 型のメンバーを探すとき、impl の `for` がその型であることを確かめる。持ち主の impl の探索を決定的にする。同じパスに別の名前空間の item (trait と derive マクロなど) があれば候補にする | R-PR-7・R-OUT-4 | `reqwest::ClientBuilder::from`・`tokio::time::error::Elapsed::from`・`serde::Deserialize` のテスト (serde のスナップショットを直す) |
| A3 | 深い再帰を大きなスタックで行う (パス解決・描画・`Drop`) | R-PR-8 | 手作りの 12,000 段と 30,000 段の型で、描画と解放が落ちない |
| A4 | 公開パス索引の作成に、経路の数か作業量の上限を付ける | R-PR-3 | 層を重ねた glob の手作りの JSON (20 層) で、上限内で止まり warnings を出す |
| A5 | 取得した本体を検証してからキャッシュに書く。`read_file` を `O_NOFOLLOW` で開き、開いたハンドルで通常ファイルとサイズを確かめる。本体の容量の確保に上限を付ける | R-CA-4・R-NW-3 | JSON でない 200 の本体がキャッシュに残らないテスト |
| A6 | 小さなもの: `source_location` が package の外のときの warnings、`TooManyLayers` の `reason`、`SERVER_NAME` を `env!("CARGO_PKG_NAME")` に、新しすぎる形式の warnings の文言、stdio のテストでログを有効にする、テストの HTTP クライアントの `no_proxy`、`name` の無い `tools/call` への応答の確認、その他 §7.2 の小さなもの | R-OUT-8・R-DS-4・R-FV-5・R-SEC-1・R-ARG-1 | §7.2 の一覧の各項目が済む |

### 3.2 M1b: データ層 (版と取得)

| # | やること | 要件 | 終わりの印 |
| --- | --- | --- | --- |
| B1 | crates.io の sparse index のクライアント。名前の正規化 (`-`/`_`・大文字小文字)、TTL 10 分と ETag の条件付きリクエスト、連絡先付きの User-Agent | R-VR-6・R-PR-1・R-CA-3・R-NW-1・R-NW-2 | 固定の応答を返すローカルの HTTP サーバーでのテスト。`304` で本文を取り直さない |
| B2 | 版の解決: 完全一致 (build metadata を含む)・semver 要件・latest (yanked と pre-release の扱い)。`crate_not_found`・`version_not_found` | R-VR-1・R-VR-3・R-VR-4・R-DS-6 | 解決の規則の単体テスト (`"0.9.0" < "0.10.0"`、`2.1.0` → `2.1.0+zstd.1.5.7` など) |
| B3 | docs.rs の取得を直す: 解決した完全な版と target の URL で取り、返った `crate_version` を照合し、食い違えば使わない。否定キャッシュの種類と時間 | R-DS-2・R-DS-6・R-VR-4・R-ERR-3 | zstd-sys の `2.1.0` の場合の回帰テスト (固定の応答で) |
| B4 | target の選び方: ホストの target (中立のディレクトリの `rustc -vV`)、プロジェクトの `build.target`。求める target の JSON が無ければ `no_target_json` | R-TG-1・R-TG-2 | target の選び方の単体テスト |
| B5 | プロジェクトの文脈 (`local` の中): 既定のプロジェクトの決め方、`cargo metadata --locked` の resolve、Cargo.lock の直読み (`cargo_lock_raw`)、`lockfile_outdated`、取得元と checksum の照合、依存の名前 (extern の名前) からパッケージへの解決、`cargo tree --workspace -e normal` の feature の組 | R-MCP-3・R-VR-2・R-PR-1・R-FT-3・R-ARCH-3 | 合成の workspace (改名した依存・同名の複数版・path 依存・vendor) での結合テスト |
| B6 | キャッシュ: 正規化した名前の鍵、内容ダイジェスト、原子的な書き込み、否定キャッシュ、書きかけの後始末、非対応の形式の扱い | R-CA-1〜R-CA-5・R-FV-6 | 壊れたキャッシュを 1 回だけ取り直すテスト |
| B7 | 形式 46〜53 の shim (`attrs` の文字列の解析を含む) | R-FV-2・R-FV-4・R-QA-2 | testdata の形式 46・48・53 の JSON を読める |
| B8 | std: dist の manifest から `rust-docs-json` を取り (sha256 で検証)、手元の nightly はプロジェクトの toolchain より古くなければ使う。std の provenance | R-STD-1・R-STD-6・R-STD-2〜R-STD-4 | `std::vec::Vec` を dist と手元の nightly の両方から引ける。`Stability` の写し (形式 58〜60) を std の JSON で確かめる |
| B9 | 読み込みの時間を測り、アーカイブ (rkyv) を入れるかを決める (§5 の P4) | R-PF-1・R-PF-2・§9.1・Q-23 | core の JSON の読み込み時間を §6 に書き、入れるならアーカイブを作る |

### 3.3 M1c: ツールを 1 本ずつ

各ツールで、クエリ層・応答の形 (outputSchema)・説明文 (英語の Markdown。要件の §6.2)・結合テスト・スナップショットまで通す。最初の C1 で応答の骨格 (R-OUT-9) と共通の仕組み (detail・カーソル・応答の上限・エラー・warnings) を作り、以降のツールで使う。

| # | ツール | 主な要件 | 終わりの印 |
| --- | --- | --- | --- |
| C1 | `get_item` を M1 の形にする: 応答の骨格、`detail` (R-OUT-7)、応答の上限と切り詰めの印 (R-OUT-3)、`ambiguous_path` の候補の上限、cfg と feature の注記 (関連 item の親を含む 3 値の `enabled_in_project`)、crate をまたぐ re-export と型の正準パス (`html_root_url`・std の私的な依存の区別) | R-OUT-9・R-OUT-1・R-OUT-3・R-OUT-7・R-FT-1・R-FT-2・R-PR-4・R-OUT-2・R-ERR-2 | §3 の Q5・Q17・Q20 に答えられる。`reqwest::header::HeaderMap` を引ける |
| C2 | `resolve_version` | §5.3・R-VR-2 | §3 の Q2 |
| C3 | `list_module` と `crate_overview` (絞り込みと展開、`recursive`) | R-LS-1・§6.5 | §3 の Q3・Q6 |
| C4 | `search_items` (BM25 の初期値、`did_you_mean`) | R-SR-1〜R-SR-5 | §3 の Q4 |
| C5 | `list_impls` (構造的な分類、封印の検出) | R-TI-2・R-TI-5・R-SH-4 | §3 の Q8 |
| C6 | `list_methods` (固有と trait 由来) | R-TI-1・R-TI-4 | §3 の Q7 のうち固有と trait 由来 |
| C7 | `list_implementors` (crate の中) | R-TI-3 | §3 の Q9 のうち crate の中 |
| C8 | `list_crates` (`local` の中。ローカル機能が無い構成では出さない) | R-ARCH-4・§6.3 | §3 の Q1 |

### 3.4 M1d: 横断的なもの

| # | やること | 要件 | 終わりの印 |
| --- | --- | --- | --- |
| D1 | 構造化ログ (JSON Lines) と `request_id`。`log.filter` を要件どおりにする | R-OBS-1・R-ERR-2・R-DIST-3 | エラーの `request_id` とログの行が対応するテスト |
| D2 | 同時実行の上限、キャンセルで取得を止める、同じ crate の取得と解析を 1 つにまとめる | R-SEC-7・R-WS-6・R-NW-2 | キャンセルと上限の結合テスト |
| D3 | 決定性: 同じ入力を別々のプロセスで実行して出力が一致する | R-ARCH-5・R-OUT-4 | CI の検査 |
| D4 | ツール名・引数・スキーマの CI 検査、説明文が約束する動作のテスト | R-QA-5・R-MCP-10 | CI の検査 |

### 3.5 M1e: eval・測定・公開

| # | やること | 要件 | 終わりの印 |
| --- | --- | --- | --- |
| E1 | 小さな eval: §3 の問いのうち M1 のツールで答えるものの問題集と、モデルに渡った `tool_result` の形を確かめるケース (§5 の P5) | R-QA-6・R-OUT-1・R-ERR-2 | 結果を §6 に書く。ツールの分け方と応答の形に問題があれば直す |
| E2 | 性能とメモリを測り、§9.1 の M1 の目標を決める。ferritin のホスト版と代表の item で比べる。gnu 版の性能 (Q-27) | §9.1・R-MEM-1・R-QA-1・Q-27 | 結果を §6 に書く |
| E3 | live の docs.rs の smoke test と canary (配られる target を含む) | R-QA-4 | CI に定期実行の job がある |
| E4 | README を M1 の実態に合わせ (Linux 向けであること、導入手順)、crates.io に最初の版を公開する (利用者が行う) | R-DIST-1 | `cargo install --locked chatora-rustdoc-mcp` で入る |

---

## 4. 要件が「M1 で決める」とした細部

それを使う段で決め、§6 に書く。

| 論点 | 要件 | 段 |
| --- | --- | --- |
| docs.rs の試行で一時的エラーと 404 が混ざったときの返し方、展開や解析の失敗で打ち切るか | R-DS-2 | B3 |
| 形式 46〜53 の属性を写す範囲 | R-FV-2 | B7 |
| キャッシュ済みの JSON が非対応の形式だったときの再確認 | R-FV-6 | B6 |
| 使うパッケージの範囲、同名の複数版の絞り方、warnings の中身。`cargo metadata` をキャッシュするかと鍵 | R-VR-2 | B5 |
| 全版が pre-release・yanked のときの latest | R-VR-3 | B2 |
| ホストの target を取れないときの代わりの値 | R-TG-1 | B4 |
| どの glob で trait がスコープに入るかを情報として持つか | R-PR-2 | C1 |
| `html_root_url` を起点と参照元のどちらから先に見るか、crate をまたぐ re-export の行き先の中身まで返すか | R-PR-4 | C1 |
| 逆引き索引の構成と、あらかじめ作ってキャッシュするか | R-TI-4 | C6 |
| 封印の検出の方法を増やすか | R-TI-5 | C5 |
| 検索の順位付けの初期値 | R-SR-3 | C4 |
| 描画できないときに `source_signature` を添えるか | R-OUT-2 | C1 |
| カーソルを何に束縛するか、オフセットか keyset か | R-OUT-3 | C1・C3 |
| 応答の骨格と状態の値、crate を持たないツールと複数の crate の応答の必須の項目 | R-OUT-9 | C1・C8 |
| `docs_unavailable` の `reason` の一覧、`corrupt` の再試行までの時間 | §8・R-CA-4 | B3・B6 |
| 否定キャッシュの鍵と時間、書きかけの一時ファイルの後始末の判定 | R-CA-4 | B6 |
| 展開とメモリの上限の値 | R-NW-3・R-MEM-1 | A5・E2 |
| 代表の item ごとの `detail=normal` の予算 | R-QA-1 | E2 |
| §9.1 の M1 の目標の値 | §9.1 | E2 |
| ジェネリックな型での auto trait の扱い | Q-18 | C5 |

---

## 5. 計画の段階で決めること

| # | 論点 | 推す案 | 理由 |
| --- | --- | --- | --- |
| P1 | 段の進め方 | §3 のとおり、土台の修正 → データ層 → ツールを 1 本ずつ縦に → 横断 → eval・公開。最初の 2 本のツールができたら小さな eval を一度回す | 層ごとに作ると、使える形になるのが最後になり、eval で分かる問題 (ツールの分け方・応答の形) の手戻りが大きくなる (v0.18 の決定 11) |
| P2 | CI | M1a で GitHub Actions を作る。Linux・macOS・Windows で fmt・clippy・test・doc、`--no-default-features`、taplo、markdownlint-cli2、cargo-deny。live の docs.rs の smoke test は定期実行の別の job (E3) | M1 で公開するので、3 つの OS でのビルド (aws-lc-rs の C のビルドを含む) を早めに確かめる。markdownlint は CI で回せばホストに入れずに済む |
| P3 | 実データのテスト | testdata の一覧 (crate・版・sha256) から CI で docs.rs の JSON を取り、キャッシュする (actions/cache)。リポジトリには入れない | 大きな JSON をリポジトリに入れずに、今は `#[ignore]` の実データのテストを CI で動かせる。docs.rs が落ちているときは、その job だけ失敗する |
| P4 | アーカイブ (rkyv) | B9 で core の JSON の読み込みを測り、§9.1 の目安 (< 50ms) に届かなければ M1 で入れる | M0 で、`get_item` の時間はほぼ解析と解放だった。測らずに入れるとアーカイブの検証 (Q-23) と形式の扱いの手間が先に来る |
| P5 | eval の回し方 | `claude -p --output-format stream-json` を使う小さなスクリプトを `eval/` に置き、手で回す (費用は回したときだけ)。`claude plugin eval` はプラグインを作る M5 で移る | プラグインの形が決まるのは M5。headless の出力はモデルに渡った `tool_result` をそのまま見られる (R-OUT-1 の検知と同じ仕組み) |

---

## 6. 結果

### 6.1 M1a

#### A1 (2026-10-19)

- cargo feature `local` を既定で有効にして作った (Cargo.toml。中身は B5 から)
- deny.toml: ライセンスの許可の一覧は、2026-10-19 の Cargo.lock の依存が使うもの (MIT・Apache-2.0・Apache-2.0 WITH LLVM-exception・BSD-3-Clause・CDLA-Permissive-2.0・ISC・Unicode-3.0・Zlib)。取得元は crates.io だけ、同じ crate の複数の版は警告
- .github/workflows/ci.yml の job:
  - `fmt`: nightly の rustfmt
  - `check`: Linux・macOS・Windows で、既定と `--no-default-features` の clippy (`-D warnings`) と test
  - `doc`: `RUSTDOCFLAGS=-D warnings` の rustdoc
  - `testdata`: testdata の一覧の JSON を docs.rs から取って actions/cache に置き、`#[ignore]` の実データのテストを回す
  - `config-lint`: taplo と markdownlint-cli2
  - `deny`: cargo-deny
- action は、出てから 2 週間以上たった版のコミットの SHA で固定した
- 手元で確かめたこと: fmt・clippy (既定と `--no-default-features`)・test・doc・taplo、testdata の job と同じコマンド (取得はキャッシュから、4 件が成功)。cargo-deny と markdownlint-cli2 はホストに無いので、CI の初回で確かめる
- CI の初回 (2026-10-19): config-lint 以外はすべて通った (Windows の aws-lc-rs のビルド、cargo-deny、docs.rs から取り直した testdata の sha256 を含む)。markdownlint-cli2 が、見出しで始まらない 2 つの Markdown (ツールの説明文 `src/mcp/tool/get_item.md` と、実験の crate の doc に取り込む `poc/m0b/b5-dep-info/probe/README.md`) を MD041 で落としたので、どちらも文書ではないとして対象から外した
- CI の 2 回目 (同日): すべての job が通った

#### A2 (2026-10-19)

- 型のメンバーは、その型の `impl` (`for_` がその型を指すもの) の中だけで探す (`path_index::own_impls`)。プリミティブ型は `impl` の `for_` が `[T]`・`&str` などさまざまなので、今までどおりすべてを見る (M4 で見直す)。`reqwest::ClientBuilder::from` と `tokio::time::error::Elapsed::from` は、他の型の `impl From<…> for …` の `from` ではなく、その型の `impl<T> From<T> for T` の `from` を返すようになった
- 同じ名前のメンバーが複数あるときは `AmbiguousMembers` を返し、候補はその型の `impl` から探す (データの順で決まる)。これまでは crate 全体の `HashMap` を走査して持ち主を探していたので、実行ごとに候補が変わった。項目の候補には種類を付ける (`struct c::same` と `macro c::same`)
- 同じパスにこの crate の item と、別の crate の item の re-export があるときは、この crate の item を返し、もう一方を引く次の呼び出しを hints に載せる (利用者が決めた。`serde::Deserialize` は trait を返し、serde_derive の derive マクロを hints で示す)。候補一覧 (R-PR-7) は、この crate の中に同じパスの item が複数あるときに限る

#### A3 (2026-10-19)

- tokio のランタイムのスレッド (ワーカーと blocking) のスタックを 16 MiB にした (`task::THREAD_STACK_SIZE`。R-PR-8)。blocking のスレッドもランタイムの設定を使うことを tokio 1.53.3 のソースで確かめた
- 解析の前に JSON の入れ子の深さを数え、10,000 段 (`rustdoc_json::MAX_NESTING`) を超えたら `ParseError::TooDeep` (MCP では `docs_unavailable` の `unparsable`) にする (利用者が決めた。R-FV-8 の「上限を設けない」は、M1 の終わりに「実際の crate より桁違いに大きい上限」に改める)
- 上限は測って決めた。16 MiB のスタックで、解析・索引・`get_item`・解放を通せた深さ (2026-10-19):

  | 形 | debug | release |
  | --- | --- | --- |
  | `[[…]]` (JSON の深さ = 型の段数) | JSON 2 万段まで (4 万段で落ちる) | JSON 10 万段まで (20 万段で落ちる) |
  | `Option<Option<…>>` (型 1 段が JSON 6 段) | JSON 6 万段まで (12 万段で落ちる) | JSON 30 万段まで (60 万段で落ちる) |

  2 MiB では、debug の `[[…]]` が JSON 5,000 段、`Option` が 1.2 万段で落ちた。上限の 10,000 段は、いちばん厳しい `[[…]]` でも debug で 2 倍、release で 10 倍の余裕があり、実際の crate (typenum で数百段) の数十倍
- tests/deep_types.rs: 上限ちょうどの深さの `[[…]]` を返す関数を、16 MiB のスレッドで解析・索引・`get_item`・解放まで通し、描画した文字列を全体で比べる。上限を超えた JSON を解析の前に断る単体テストも足した
- 実データ (testdata の 20 件) はどれも上限に掛からない。解析 (深さを数える分を含む) は rmcp の 15 MB で 139 ms (release)。深さを数える分だけは分けて測っていない (B9 で読み込みの時間を測るときに見る)

#### A4 (2026-10-19)

- 公開パス索引を作るとき、1 つの module に入るのは 64 回まで (`MAX_MODULE_VISITS`) にした。上限に当たったら索引に印を付け (`PathIndex::is_incomplete`)、`get_item` の warnings で「一部の公開パスをたどっていないので、パスが見つからないことがある」と知らせる。作業量は「64 × item の数」で頭打ちになる
- 上限は測って決めた: testdata の 20 件では、1 つの module に入る回数は最大 2 回 (clap) で、ほかはすべて 1 回だった (2026-10-19)
- 単体テスト: module を 2 つの名前で re-export する層を 40 重ねた (経路は 2 の 40 乗) 手作りの JSON で、索引がすぐに作られ、印が付き、item のパスが 64 個以内になる。計画の終わりの印は「glob を 20 層」だったが、名前を付けた re-export でも glob でも同じ上限を通るので、より深い 40 層で確かめた
- 上限で打ち切ったとき、深さ優先でたどるので、短いパスが先に見つかるとは限らない (正準パスを取り逃しうる)。実データでは上限に掛からないので、幅優先には変えていない

#### A5 (2026-10-19)

- docs.rs から取った本体は、`format_version` を先読みできたときだけキャッシュに書く (書く前の検査は blocking のスレッドで)。読めない本体 (プロキシのエラーページなど) も呼び出し元には返し、`unparsable` として報告させる。次の照会では取り直す
- キャッシュの読み出し (`Cache::read_file`。R-CA-4):
  - キャッシュのディレクトリからファイルまでの間のディレクトリにある symlink もたどらない (キャッシュのディレクトリ自体は symlink でもよい)
  - 通常のファイルでなければ読まない (ディレクトリ・FIFO など)
  - Unix では `O_NOFOLLOW | O_NONBLOCK` で開き、開いたハンドルが確かめたファイルと同じ (dev と inode) で通常のファイルであることを確かめる。確かめた後で symlink や FIFO に差し替えられても、たどらず、止まらない
  - Windows には symlink を断るフラグが無く、ファイルの同一性を見る API も安定していないので、確かめた時点の検査だけになる
  - 大きさは、メモリに置く JSON の上限 (`docs_rs::MAX_BYTES`、512 MiB) を超えたらエラーにする
- 本体の受け取りで先に確保する容量は、`content-length` によらず 64 MiB まで (`MAX_INITIAL_CAPACITY`)。それより大きい本体は届くにつれて伸ばす
- `libc` を Unix の依存に足した (`O_NOFOLLOW`・`O_NONBLOCK`・`ELOOP`)。もとから間接の依存で、Cargo.lock の版は変わらない
- テスト: JSON でない 200 の本体がキャッシュに残らない、JSON は残る (ローカルの HTTP サーバー。docs.rs のテストの道具を src/data/test_http.rs に移して共有した)。間のディレクトリの symlink、ディレクトリ、FIFO (止まらずに読まない)、上限を超える大きさ

#### A6 (2026-10-19)

m0-plan.md の §7.2 の小さなものを片付けた。

- `source_location` の span が package の外 (生成物の `OUT_DIR`、ビルドホストの絶対パス) なら、URL を付けずに warnings で知らせる (R-OUT-8)
- 展開の段数の上限を超えた本体 (`TooManyLayers`) を `too_large` から `unparsable` に改めた (R-DS-4 は「壊れている」とする)
- `SERVER_NAME` を `env!("CARGO_PKG_NAME")` にした (パッケージ名とバイナリ名が同じになったため)
- 新しすぎる形式の warnings を、R-FV-5 の例に寄せて「対応する形式・追加的な変更として読んだ・results unverified」の文言にした
- stdio の結合テストでログを `debug` で有効にした。stdout に JSON-RPC でない行が出ればテストが落ちるので、ログの経路も R-SEC-1 の検査を通る
- テストの HTTP クライアントはプロキシを使わない (`DocsRs::for_local_test`)。設定は本番と同じ `client_builder` から作る
- `name` の無い `tools/call` への rmcp の応答を確かめた: `-32601` (R-ARG-1 は `-32602`)。`arguments` がオブジェクトでない場合と同じく、ハンドラの外で起きるので受け入れる違いとしてテストで固定した。M1 の終わりに要件の R-MCP-2 の一覧に足す
- 応答のキーの順を確かめるテストを足した (`Value` の比較は順を見ないため。R-OUT-1・R-SH-1)。`Map::remove` は今もテストでしか使っていない
- そのほか: sleep のエラーの文言は上限の定数から作る、説明文の長さのテストは失敗時に文字数を出す、examples/fetch_docs_rs.rs はキャッシュの symlink をキャッシュとみなさない、tests/stdio.rs のコメントの参照先
- m0-plan.md の §7.2 の残りは、それを扱う段で直す: TLS の検証の失敗の分類 (B3)、`log.filter` (D1)、`ambiguous_path` の候補数・応答の大きさ・親が曖昧なときのメンバー・`tokio::select!` の書き方 (C1)、キャンセルと同時実行の上限 (D2)

### 6.2 M1b

#### B1 (2026-10-19)

- crates.io の sparse index のクライアント (`data::sparse_index::SparseIndex`) を作った。`entry(name)` が、公開された名前と、全版 (build metadata を含む版・checksum・yanked・`rust_version`) を返す
- 名前の正規化 (R-PR-1): 小文字にし、与えられた綴り → `_` をすべて `-` にした綴り → `-` をすべて `_` にした綴りの順に試す (index のファイルは公開された綴りの小文字の場所にあり、`tokio_util` では見つからない)。`-` と `_` を公開と違う混ぜ方で書いた名前は見つからない。公開された綴り (`Inflector`、`tokio-util`) は index のファイルの `name` から取る
- キャッシュ (R-CA-3): 本体と、取得の記録 (`etag`・`last-modified`・取得した時刻) の `.meta.json` を `index.crates.io/<index のパス>` に置く。TTL (10 分) の間は通信しない。過ぎたら `If-None-Match` (`etag` が無ければ `If-Modified-Since`) を付けて確かめ直し、`304` なら本体を取り直さずに時刻だけ更新する。本体は解析できたときだけ書く (A5 と同じ)
- HTTP の設定 (User-Agent・タイムアウト) と、上限付きで本体を読む処理を `data::http` に移し、docs.rs のクライアントと共有した。テスト用のローカルの HTTP サーバーに、複数の応答を順に返す `serve_sequence` を足した
- まだ無いもの: 再試行と同じ要求の重複抑止 (R-NW-2。B3・D2)、存在しない crate の否定キャッシュ (R-ERR-3。B3・B6)、MCP からの利用 (B2・C2)。ネットワークの失敗時に古いキャッシュを使うか (R-CA-7 はオフラインモードのとき) も、まだ決めていない
- テスト: index のパス、綴りの候補、取得・解析・キャッシュ (TTL の間は 2 回目に通信しない)、`304` で本体を取り直さない (`If-None-Match` が付く)、別の綴りでのやり直し、見つからない crate、index でない本体はエラーにしてキャッシュしない

#### B2 (2026-10-19)

- 版の指定と解決 (`data::version`。R-VR-1・R-VR-3・R-VR-4):
  - `VersionSpec` に解析する。`x.y.z[-pre][+build]` は完全一致 (`0.8.9` は Cargo の `^0.8.9` ではない。回帰テストで固定)、`^1`・`~1.2`・`1`・`1.2`・`>=1, <2` は semver の要件、`latest`・`newest`・`*` は latest (大文字小文字と前後の空白を無視)。解析できないものはエラーにし、黙って `*` にしない
  - 完全一致: build metadata の無い版は、build metadata だけが違う版が 1 つならそれにする (`2.1.0` → `2.1.0+zstd.1.5.6`)。2 つ以上あれば決めない。yanked の版を明示されたら、その版を返して warnings で知らせる
  - 要件と latest: yanked でない正式版の最大 → yanked でない pre-release の最大 → yanked を含めた最大、の順に探し、後の 2 段では warnings で知らせる (利用者が決めた。要件の「M1 で決める」の 1 つ)。pre-release は、semver の規則では要件に名前が無いと当たらないので、2・3 段目では pre-release を外した版で照合する
  - 見つからなければ、yanked でない版から候補を最大 5 個返す: 明示した版と同じ major・minor のもの、無ければ同じ major のもの、無ければ全体の、大きい順
  - 比較は semver の順 (`0.10.0` > `0.9.0`)
- まだ無いもの: `crate_not_found`・`version_not_found` のエラーへの写し (C1・C2)、プロジェクトの Cargo.lock からの解決 (B5)、`resolved_from` (C1)

#### B3 (2026-10-19)

- 返った JSON の照合 (R-VR-4): キャッシュに書く前に、`format_version` と `crate_version` を 1 回の走査で読み (`format_version::read_header`)、`crate_version` が頼んだ版と違えば使わずキャッシュもしない (`LoadError::WrongVersion`。MCP では `docs_unavailable` の `version_mismatch`)。本物の docs.rs で、zstd-sys の `2.1.0` に `2.1.2+zstd.1.5.7` が返り、断られることを確かめた
- 否定キャッシュ (R-ERR-3): docs.rs に JSON が無い (404・400) ことは 1 時間、一時的な失敗 (429・5xx・想定外のステータス・通信・証明書) は 30 秒、JSON の場所の隣の `.miss.json` に覚える。覚えている間は問い合わせず、JSON が無いなら `no_json`、一時的な失敗なら `network_error` に `retry_after_secs` を付けて返す。壊れた本体 (上限超え・展開の失敗) は覚えない。取れたら記録を消す
- 再試行 (R-NW-2): 429・5xx・接続の失敗・タイムアウトを、最大 3 回まで送り直す (`data::http::send`。docs.rs と sparse index で共有)。待ち時間は `Retry-After` (秒の形だけ) か、0.5 秒から倍にしていく。`Retry-After` が 5 秒を超えたら待たずに返す (1 回の照会の時間の上限を食いつぶさないため)
- 形式番号付きの URL (R-DS-2 の手順 3): 解析で対応外の形式になったら、対応する形式を新しい順に `/json/{n}` で試す。利用者が決めたとおり、一時的な失敗や壊れた本体があっても最後まで試し、読める形式が見つかればそれをキャッシュして引き直す。見つからなければ、途中に一時的な失敗があればそれを、無ければもとの `unsupported_format` を返す
- 証明書の検証の失敗 (A6 から回したもの): 原因のどこかに `certificate` を含むエラーを `FetchError::Certificate` に分け、再試行せず、`network_error` (retryable false) と、プロキシの CA 証明書を足すよう案内する。rustls のエラーの型は reqwest の外に出てこないので、文言で判定している (できる範囲で)
- target を指定した取得は、target の検証の型とあわせて B4 で入れる
- 要件に足すこと (M1 の終わり): §8 の `docs_unavailable` の `reason` に `version_mismatch`、一時的な失敗を覚えている間の `network_error` の `retry_after_secs`、再試行の回数と待ち時間の初期値
- テスト: 再試行 (503 の後に 200)、形式番号の URL、状態の分類 (503 は 3 回送る)、別の版を使わない、JSON が無いことと一時的な失敗を覚える、別の形式を見つけてキャッシュする、途中の一時的な失敗を返す、どの形式も無ければ `None`、証明書の失敗の判定

#### B4 (2026-10-19)

- `data::target`: target triple の型 `TargetTriple` (英数字・`-`・`_`・`.` だけで、`..` を含まず、英数字で始まる。URL とパスに入れても形が変わらない。R-SEC-4)
- ホストの target (R-TG-1): キャッシュのディレクトリ (プロジェクトの設定が効かない場所) で `rustc -vV` を実行し、`host:` の行を読む。`RUSTUP_AUTO_INSTALL=0` を付け、stdin・stdout を共有せず、10 秒で打ち切る。`local` の中に置いた (tokio の `process` は `local` が有効にする)
- 取れないとき (rustup に既定の toolchain が無い、rustc が無い、ローカル機能を外した構成) は、サーバー自身をビルドした target を使い、理由を返す (呼び出し側が warnings に書く)。musl の環境は gnu に読み替える (`x86_64-unknown-linux-musl` → `-gnu`、`armv7-unknown-linux-musleabihf` → `-gnueabihf`)。利用者が決めた。サーバーの target は build.rs で `CHATORA_RUSTDOC_MCP_BUILD_TARGET` に埋め込む (Cargo.toml の `include` に build.rs を足した)
- docs.rs の取得に target を足した (`/crate/{name}/{version}/{target}/json`。キャッシュも `docs.rs/<name>/<version>/<target>/rustdoc.json` に分ける。target を指定しないときは今までどおり)。target 付きの URL が 404 なら、target 無しの URL に `HEAD` を送り、そちらが 200 なら「その target の JSON だけが無い」として `LoadError::NoTargetJson` (MCP では `docs_unavailable` の `no_target_json`) を返し、1 時間覚える。HEAD が失敗したら、何も覚えずに `no_json` を返す
- ツールにつなぐのは C1 (今は target を指定しない)。今つなぐと、テスト用のキャッシュの場所が変わり、stdio の結合テストが docs.rs に問い合わせてしまうため
- 依存: tokio の `process` で signal-hook-registry が足された (ネットワークを使わずに Cargo.lock を更新した)
- 要件に足すこと (M1 の終わり): 代わりの値の取り方 (R-TG-1 の「M1 で決める」)、`HEAD` で JSON の有無を確かめること
- テスト: triple の検証、musl の読み替え、`rustc -vV` の読み取り、実際の `rustc -vV` (`local` のとき)、target 付きの URL と HEAD、target の JSON だけが無いことの判定と記憶、どちらにも無いときは `no_json`

#### B5a (2026-10-19)

B5 は大きいので 4 つに分けた: B5a (既定のプロジェクトと workspace のルート)、B5b (`cargo metadata --locked`・`lockfile_outdated`・Cargo.lock の直読み)、B5c (checksum の照合・extern の名前の解決・同名の複数版)、B5d (`cargo tree` の feature の組)。

- `data::project` (`local` の中): 既定のプロジェクト `DefaultProject::find` は、`CLAUDE_PROJECT_DIR` (空なら無いものとする) → カレントディレクトリの順でディレクトリを決め、どちらを使ったかを `ProjectFrom` で返す。そこに Cargo.toml があればその workspace、無ければ直下の子のディレクトリ (symlink はたどらない) のうち Cargo.toml を持つものの workspace をすべて返す。子の workspace を探すときは、決めたディレクトリより上には行かない。起動時に一度だけ呼ぶ前提 (つなぐのは C 段)
- workspace のルート `workspace_of` は、Cargo.toml を toml で読み、cargo を起動せずに決める: `[workspace]` があればそこ、`package.workspace` があればその先、どちらも無ければ上へたどる。上の workspace は、`exclude` していれば飛ばしてさらに上を見て (Cargo と同じ)、`members` に入っていればルートにし、どちらでもなければ拾わずに単独の package とする (Cargo はこの package をビルドしない)。`members` の直書きのパスは `exclude` より優先し、パターンは負ける。パターンは `*`・`?`・`**` を扱う (`[...]` は文字どおり)。これらは合成の木で `cargo metadata --no-deps --offline` と突き合わせて決めた (cargo 1.99.1)
- 扱わないこと: path 依存で暗黙に member になるもの (ルートの package の path 依存など)。単独の package と判定する。B5b で `cargo metadata` の `workspace_root` と食い違ったら、cargo の答えを使う
- 依存: toml 1.1.8 (`parse`・`serde` だけ。`local` の中)。toml_datetime・toml_parser・winnow が足された
- テスト: member・exclude・パターンの各規則、`package.workspace`、Cargo.toml が無いときのエラー、既定のプロジェクト (member のディレクトリから開いたとき、Cargo.toml が無く子が複数あるとき、2 段下の workspace を拾わないこと)

#### B5b (2026-10-19)

- `data::cargo` (`local` の中): プロジェクトの文脈で cargo を起動する共通の部品
  - workspace のルートで、常に `--locked` を付けて起動する。オフラインなら `--offline` と `RUSTUP_AUTO_INSTALL=0` も付ける
  - stdin は null、stdout と stderr はパイプにする。ほかに `LC_ALL=C`、`GIT_TERMINAL_PROMPT=0`、`GCM_INTERACTIVE=never`、`SSH_ASKPASS_REQUIRE=never` を付け、`SSH_ASKPASS`・`GIT_ASKPASS` を外す
  - Unix では `setsid` で新しいセッションに入れ、制御端末を外す。これがこのコードベースで最初の `unsafe` になる (`pre_exec`。SAFETY のコメント付き)。子がセッションの先頭になることはテストで確かめた (Linux)
  - 30 分で止める (`build.timeout_secs` の既定)
- 失敗の分け方 (R-WS-7 が認めた文言だけを照合する)
  - `lockfile_outdated`: `because --locked was passed`。Cargo.lock が無いときも古いときもこの文言になる (cargo 1.99.1 で確かめた)
  - `offline_miss`: オフラインを示す 2 つの文言。`no matching package named` は、サーバーが `--offline` を渡したときだけこれに数える (オンラインでは crate 名の誤りと同じ文言のため)
  - `fetch_failed`: git の取得の決定的な失敗。`reason` は `auth`・`no_repository`・`no_rev` とした。要件が名前を決めていたのは `auth` だけで、残りは `no_json` などにそろえた
  - `(signal: 9, SIGKILL` かシグナルでの終了は、一時的な失敗。それ以外も、区別できないものとして一時的な失敗にする
  - stderr は末尾の 8,000 文字を残す
- `data::metadata`: `cargo metadata --format-version 1` を実行し、使う部分 (packages・resolve・workspace_members・workspace_root) を読む
  - checksum は `cargo metadata` に出ないので、Cargo.lock も常に読む (`data::cargo_lock`)
  - 失敗したら Cargo.lock だけの結果 (`cargo_lock_raw`) を、失敗の分類と一緒に返す
  - Cargo.lock が無ければエラーにする (`lockfile_outdated`)
- `cargo metadata` のキャッシュ (R-VR-2 の「M1 で決める」。利用者が決めた)
  - プロセスのメモリに、workspace のルートごとに持つ。このリポジトリ (248 パッケージ) では 1 回約 170 ms、出力は 1.2 MB だった
  - 鍵は入力の中身で、次のものを含める: Cargo.lock、ルートの Cargo.toml、前回の結果で source が無いパッケージ (member と path 依存) の Cargo.toml、ルートの祖先の `.cargo/config(.toml)` と `rust-toolchain(.toml)`、`$CARGO_HOME/config(.toml)`、`members` の glob を展開した結果
  - glob の展開は、1 万エントリか深さ 32 を超えたら諦め、その結果はキャッシュしない
  - 実行中に Cargo.lock が変わったら、その結果はキャッシュしない
  - 成功だけを持つ。失敗は毎回 cargo を起動し直す
- 扱っていないこと
  - cargo の設定の `include` が指すファイルは鍵に入れていない (読み方は Q-31 で M2 に決める)
  - 子の後始末は M2 に回した: `PR_SET_PDEATHSIG` (spawn するスレッドの決め方 Q-29 と一緒に決める)、Windows の Job Object、ロック待ちの検出 (`waiting_for_lock`)
  - 同時の呼び出しを 1 つにまとめること (singleflight) と、一時的な失敗を 30 秒抑えることは、ツールにつなぐ C 段で決める
  - cargo の実行ファイルの指定 (設定の絶対パス) は、設定を入れるときに足す。今は PATH の `cargo` を使う
  - B5a から持ち越した「path 依存で暗黙に member になるもの」は、`Metadata::workspace_root` と比べられるようにした。比べて使うのは C 段
- 依存: tokio の `sync` を `local` に足した (新しい crate は増えない)
- テスト
  - 失敗の文言の分類、stderr の末尾、子がセッションの先頭になること
  - Cargo.lock の読み取り
  - 実際の cargo (`--offline`) で、結果を再利用すること、member の Cargo.toml を変えると取り直すこと、glob に合う member が増えると `lockfile_outdated` になり Cargo.lock だけの結果に落ちること、Cargo.lock が無いときのエラー
  - `members` の glob の展開

#### B5c (2026-10-19)

- `data::packages` (`local` の中): `cargo metadata` の結果 (失敗したときは Cargo.lock だけ) から、プロジェクトのパッケージと依存の辺を持つ `PackageGraph` を作る
  - package id で索引し、同名の複数版を共存させる (R-VR-2)
  - checksum は Cargo.lock の、名前・版・source が一致するものから取る
- 名前の引き方 (`look_up`。R-PR-1)
  - member の依存の extern の名前 (改名の後の名前)、パッケージ名、lib 名のどれでも当たる。`-` と `_`、大文字と小文字は区別しない
  - extern の名前とパッケージ名が別のパッケージに当たったら、`AmbiguousPackage` として候補を返す。どのエラーコードで返すかは C1 で決める (§8 の `ambiguous_version` は「同じ crate の複数版」の意味なので、そのままでは合わない)
  - 同じパッケージの複数版なら、member から直接依存している版が 1 つだけのときにそれを使い、ほかの版を `other_versions` として返す (warnings に書く想定)。0 個か 2 個以上なら `AmbiguousVersion` として、全部の版を古い順に、それぞれの依存元と一緒に返す (`ambiguous_version`)。直接の依存は、dev・build を含むすべての種類と、改名した依存を数える。member 自身も直接とみなす
- docs.rs を使ってよいか (`docs_rs_check`。R-VR-2): 取得元が crates.io で、Cargo.lock の checksum が sparse index の同じ版の `cksum` と一致するときだけ許す。それ以外を `NotCratesIo`・`NoChecksum`・`NotInIndex`・`ChecksumMismatch` に分ける。crates.io の source は `registry+https://github.com/rust-lang/crates.io-index` と `sparse+https://index.crates.io/` の 2 つの書き方を認める
- Cargo.lock だけのとき
  - 改名と lib 名は分からないので、パッケージ名から作った名前で引く
  - member と path 依存を区別できないので、source の無いものをすべて member とみなす
  - `dependencies` の `name`・`name version`・`name version (source)` の 3 つの書き方を読む
- このリポジトリの実際の `cargo metadata` の出力 (248 パッケージ) で確かめた
  - 直接の依存は 1 つに決まった (build metadata 付きの `toml@1.1.8+spec-1.1.0` も)
  - 間接にしか使われない 2 版 (windows-sys・getrandom) は、依存元付きの曖昧さになった
  - checksum は Cargo.lock から引けた
- テスト: 直接の版を選ぶこと、直接でない複数版の曖昧さと依存元、extern の名前とパッケージ名の衝突、Cargo.lock だけのときの引き方、docs.rs の判定と source の読み方

#### B5d (2026-10-19)

- `data::features` (`local` の中): `cargo tree --workspace --target <target> -e normal --prefix depth --format '{p}\t{f}'` で、通常のビルドの各パッケージの feature の組を取る (`enabled_in_project` の判定用。R-FT-2・R-FT-3)。`-e normal,dev` も取り、`enabled_in_dev_only` はその差で求められるようにした。resolver 1 では dev の feature が通常の組にも入るので、差は自然に空になる (合成の workspace で確かめた)
- host 用と target 用の組
  - resolver 2 以上では、同じパッケージが target 用と host 用 (proc-macro の下) で別の組で出る。workspace の中の path 依存は暗黙の member としてルートにも出て、行だけではどちらか分からない
  - そこで `--prefix depth` で木の形を読み、proc-macro とその下を host 用とした。パッケージの組は、ルート以外で target 用の位置に出た組、無ければルートの組、それも無ければ host 用の組 (proc-macro だけが使うパッケージ) にする
  - `-e no-proc-macro` では host 用のルートの行が消えなかったので使わない
  - 合成の workspace で、`cargo check --workspace --target <t>` の `compiler-artifact` の `features` と突き合わせた。workspace 全体のビルドでは、暗黙の member の proc-macro が依存に付けた feature が target 用にも統合され、cargo tree の組と一致した (target 用は `w,x,y`、host 用は `x`)。`-p a` だけのビルドでは target 用は `y` になる。要件どおり全 member を統合した組で判定する
- キャッシュ: target ごとに、`cargo metadata` の結果と同じ入力の鍵で持つ (`Resolver::features`)。入力が変われば一緒に捨てる。`cargo metadata` が失敗したとき (Cargo.lock だけのとき) は、同じく失敗するので実行せずエラーにする (`enabled_in_project: unknown` になる想定)
- `metadata::MetadataError` を、`cargo tree` の失敗にも使えるよう `CargoError` に改名し、subcommand を持たせた
- 扱っていないこと: 実際のビルド target の決め方 (`.cargo/config.toml` の `build.target` を読むか。読み方は Q-31 で M2)。今は呼び出し側が target を渡す。`--target` を付けない形での再確認 (Q-31)。B5 はこれで終わり
- テスト: 木の読み取り (host 用・target 用・ルートの組、proc-macro だけが使うパッケージ、読めない行)、実際の cargo で通常の組と dev を含む組、結果を再利用すること

#### B6 (2026-10-19)

利用者が決めた 4 件 (どれも推奨案): キャッシュの配置を要件の鍵に今合わせる、解析できない JSON は 1 回取り直して 1 時間覚える、書きかけの一時ファイルは古さで判定して書くついでに消す、読める形式が無いことは 1 時間覚えてサーバーの更新で捨てる。

- 配置 (R-CA-2): `docs.rs/<名前>/<版>/<target か _default>/<format_version>.json` と、その SHA-256 の `<format_version>.json.sha256`
  - 名前は `CrateName::cache_key` (小文字にし、`_` を `-` にする。crates.io が同じ crate とみなす名前は同じ場所になる)
  - `_default` は docs.rs の既定の target。target triple は英数字で始まるので、衝突しない
  - 形式ごとに別のファイルになったので、別の形式を探した結果は既定のものを上書きしない。読むときは、このサーバーが読める形式の新しいものを先に、無ければ一番新しいものを取る (R-FV-6。サーバーの更新で読める範囲が広がれば、手元のものをそのまま読む)
  - 既存のキャッシュは読まれない (取り直しになる)。CI の actions cache の鍵を `docs-rs-json-v2-…` に上げた。testdata の全件を新しい配置に取り直し、sha256 が一覧と一致し、testdata のテストが通ることを確かめた
- 内容ダイジェスト (R-CA-4): JSON を書いた後に digest を書く。読むときに合わなければ壊れたとみなし、消して 1 回だけ取り直し、warnings とログに出す。digest の無い JSON (古いサーバーが書いたもの、digest を書く前に止まったもの) は、無いものとして取り直す。sha2 を dev-dependency から通常の依存に移した (Cargo.lock は変わらない。B8 の std の検証でも使う)
- 失敗の記録は `miss.json` に種類・時刻・サーバーの版を書く
  - JSON が無い・target の JSON だけが無い (1 時間)、一時的な失敗 (30 秒) は今までどおり
  - 解析できない (`unparsable`)、読める形式が無い: 1 時間。記録したサーバーと版が違えば無視する (別の版なら読めるかもしれないため)
  - `corrupt`: 取り直して書いた JSON を読み直すと digest が合わない (ディスクの問題を疑う)。§8 の定義どおり再試行できるものとして、30 秒にした
- 解析できない JSON (`unparsable`): 取得時に先頭の検証を通ったのに解析に失敗したら (`Json`・`FormatMismatch`・`TooDeep`)、キャッシュから消して 1 回だけ取り直す。docs.rs が同じ中身を返したら、そこで 1 時間覚える。違う中身でも解析できなければ、ツールが `remember_unparsable` で覚える。先頭の検証を通らない本文 (プロキシのエラーページなど) は、今までどおりキャッシュせず、取り直しもしない
  - 質問のときに「corrupt を 1 時間覚える」と書いたが、§8 の定義では、解析できないものは `unparsable`、ディスクで壊れるものが `corrupt` だった。そこで 1 時間は `unparsable` に当てた
- 一時ファイルの後始末: 書き込みが成功したら、同じディレクトリの `.…tmp` のうち更新が 1 時間より古いものを消す。二度と書かない場所に残ったものは GC (R-CA-5) に任せる
- キャッシュの書き込みと削除は、キャッシュのディレクトリの下でなければ拒む
- ツール (`get_item`): 読み込みの警告を応答の `warnings` の先頭に入れる (`provenance` の直後という順は保つ)。`docs_unavailable` に `unparsable`・`corrupt` (どちらも `retry_after_secs` 付き) を足した
- 扱っていないこと: 同じ鍵への同時の書き込みの排他 (M2。今は原子的な rename だけ)。古い配置のファイルの片付け (GC の R-CA-5)。手元の `~/.cache/chatora-rustdoc-mcp` の古い配置のファイルは読まれないまま残る
- テスト: 配置と digest、読める形式を先に読むこと、digest の不一致と digest の無いファイル、古い一時ファイルだけを消すこと、壊れたキャッシュを 1 回だけ取り直すこと (終わりの印)、解析できない JSON の取り直しと記録、新しいビルドへの置き換え、別のサーバーの版の記録を無視すること、読める形式が無いことを覚えること、symlink・FIFO・ディレクトリを読まないこと

#### B7 (2026-10-19)

- `rustdoc_json::v46_to_53`: 形式 46・48〜53 の shim。対応範囲を 46 からにした (`SUPPORTED = 46..=61`)。47 は欠番なので、`supported_formats()` で除き、別の形式を探すときも docs.rs に問い合わせない
- 形式 54 とのスキーマの違いは、rustdoc-types の変更履歴で確かめた
  - `Item::attrs` が文字列の配列であること (54 で `Attribute` になった)。この shim の中心
  - 54〜56 と同じく `ExternalCrate::path` が無い (`v54_to_56` の型を使い回した)
  - 51 で `AssocItemConstraint::args` などが `Option<Box<GenericArgs>>` になったが、serde の `Option` は値そのものを受けるので、shim は要らない
  - 58 で足された `stability`・`const_stability` は `None` にする
- 属性の写す範囲 (R-FV-2 の「M1 で決める」): 形式 54 以降に variant があるものは、すべて写す (`non_exhaustive`・`must_use` (理由付き、52〜53 の `#[attr = MustUse {reason: "…"}]` も)・`macro_export`・`export_name`・`link_section`・`automatically_derived`・`repr` (`C`・`transparent`・`simd`・`Rust`・整数型・`align(n)`・`packed`・`packed(n)`)・`no_mangle` (`unsafe(…)` で包んだものも)・`target_feature`)。variant の無いものと解析できないものは、文字列のまま `Other` に残す
  - `#[inline]` (46) と `#[attr = Inline(Hint)]` (48 以降) は、形式 61 でも `Other` なので、書き換えずに残した。形式をまたいだ比較では表記が違って見えるが、`Other` の文字列は形式の版を上げずに変わりうると上流が明言しているので、揃えない
  - testdata (tokio 1.45.1・clap 4.5.39 の 46、syn 2.0.104 の 48、clap 4.5.41 の 53) で、実際に出る属性の文字列を数えて範囲を確かめた。どれも shim で読め、variant のある属性が文字列のまま残らないことを testdata のテストで確かめるようにした
- テスト: 属性の文字列の解析 (各 variant、文字列リテラルのエスケープ、`repr` の組み合わせ、解析できないものは `Other`)、形式 46・48・51・53 の手作りの JSON、45 と 47 は非対応、testdata の 4 件

#### B8 (2026-10-19)

利用者が決めた 2 件 (どちらも推奨案): アーカイブは xz を純 Rust の lzma-rust2 で展開する、crate をまたぐ re-export は B8 で std の仲間の間だけたどる。

- `data::dist`: nightly のチャネルの manifest から `rust-docs-json-preview` の xz のアーカイブを取り、SHA-256 を確かめてから展開する (R-STD-6)
  - manifest は 1 時間使う (取り直せなければ、古いものを warnings 付きで使う)。日付を指定した nightly (`/dist/<日付>/…`) も取れる。日付の manifest は変わらないので、一度だけ取る。形式 58〜60 の確認に使い、M4 の「ユーザーの stable に近い nightly を選ぶ」(R-STD-6 の任意) にも使える
  - 展開先は `std/<短いコミットハッシュ>/<アーカイブの target>/<crate>.json` (digest 付き)。最後に完了印 `complete.json` (日付・版・完全なコミットハッシュ・target) を書く (R-CA-4)。アーカイブの中の名前からは書き込み先を作らない (`..` を含むエントリを入れたテストで確かめた)。std・core・alloc・proc_macro・test だけを書き、展開の上限は合計 2 GiB、1 ファイル 512 MiB
  - manifest が他の target のアーカイブを指す場合 (x86_64-apple-darwin → aarch64 など) は、アーカイブのファイル名から実際の target を取る (R-STD-6)。アーカイブの URL は dist のベース URL の下に限る
  - 実測 (release ビルド、Linux x86-64、2026-10-19): アーカイブは 4.2 MB、展開後 125 MB (core.json が 96 MB)。取得・展開・書き込みで 1.1〜1.8 秒。比較として、C の liblzma (Python) の展開だけで 0.49 秒。gz のアーカイブは 15 MB
  - 依存: lzma-rust2 0.23 (`std`・`xz`・`optimization`。テストでだけ `encoder`)、tar 0.4 (既定の機能なし)、filetime (tar の依存)。toml を `local` の外の通常の依存にした (manifest を読むため)。cargo-deny のライセンスの検査を通る
- `data::local_std` (`local` の中): `rustup run nightly rustc -vV` と `--print sysroot` をキャッシュのディレクトリで (`RUSTUP_AUTO_INSTALL=0` で) 実行し、`share/doc/rust/json/` に std.json があれば使う。版の数字 (pre-release を除く) がプロジェクトの toolchain の版以上のときだけ使う (R-STD-1)。`rustc -vV` を走らせる部分は `data::tool` に切り出し、ホストの target (B4) と共有した
- `data::std_json::choose`: 手元の nightly が使えて、その host が要求した target と同じならそれを、そうでなければ dist を使い、理由を返す (provenance 用)。dist に要求した target の JSON が無ければ、ホストの target のもので代用し warnings に書く (R-STD-1)。dist を無効にする設定 (古い手元の nightly を warnings 付きで使う) は、設定を入れるときに足す
- `query::across` と `PathIndex::crossing`: 他の crate の item の re-export を通るパスを、その crate の中のパスに置き換えてたどる (`std::vec::Vec` → `alloc::vec::Vec`)。どの crate を読み込むかは呼び出し側が決める (今は std の仲間だけ)。docs.rs の crate どうしは C1 で R-PR-4 と一緒に決める
- 終わりの印
  - dist から: 最新の nightly で `std::vec::Vec` (alloc)・`std::option::Option` (core)・`std::collections::HashMap` (std) を引け、どれも stability がある
  - 形式 58〜60 の `Stability` の写し: 2026-06-25 (形式 58)・06-26 (59)・07-10 (60) の nightly の std で確かめた。`Vec` は `Stable { since: "1.0.0" }`、unstable の item は 970〜991、`const_stability` は 58 で 0、59・60 で 120 (59 で入ったとおり)
  - 手元の nightly から: このホストの nightly には `rust-docs-json` が無いので (入れるとホストの変更になる)、CI の testdata ジョブで `rustup toolchain install nightly --profile minimal --component rust-docs-json` をしてから `tests/std_dist.rs` を走らせるようにした。手元では、無いことを `NoComponent` として返し、何もインストールしないことを確かめた
- 要件に足すこと (M1 の終わり): xz と lzma-rust2、展開の上限、キャッシュの配置、manifest が取れないときに古いものを使うこと、日付の指定、手元の nightly を使う条件 (host が target と同じ)、std の仲間の間だけ re-export をたどること
- テスト: アーカイブの取得・検証・展開と再利用 (2 つ目の target は要求なし)、SHA-256 の不一致、target が無いとき、std の無いアーカイブ、`..` のエントリ、日付と URL の読み取り、`rustc -vV` の読み取り、版の比較、sysroot からの読み込み、ホストでの代用、`crossing`。実データは `tests/std_dist.rs` (ignored)

#### B9 (2026-10-19)

- 読み込みの時間を測った (`examples/measure_load.rs`。release ビルド、Core i5-1335U、ページキャッシュに載った状態、5 回の中央値)

  | crate (JSON の大きさ) | 読む | SHA-256 | 解析 | 索引 | 解放 | 合計 | アーカイブを開いて検証 |
  | --- | --- | --- | --- | --- | --- | --- | --- |
  | core (96 MB) | 61 ms | 86 ms | 710 ms | 58 ms | 126 ms | 1,041 ms | 42 ms |
  | std (17 MB) | 6 ms | 17 ms | 133 ms | 2 ms | 16 ms | 174 ms | 7 ms |
  | rmcp (15 MB) | 4 ms | 14 ms | 125 ms | 1 ms | 16 ms | 161 ms | 6 ms |
  | tokio 1.53.3 (6 MB) | 2 ms | 6 ms | 48 ms | 1 ms | 5 ms | 62 ms | 2 ms |

  - JSON のままでは §9.1 の目安 (巨大 crate の読み込み < 50 ms) に届かないので、P4 のとおり rkyv を入れる
  - rkyv の入れ方は利用者が決めた (推奨案): zero-copy。アーカイブを mmap し、開くときに一度だけ bytecheck で検証して、以後はコピーせずに読む。照会の層 (path_index・get_item・render) を `ArchivedCrate` の上で動かす書き換えは、`get_item` を作り直す C1 で行う
  - 比べた案: アーカイブから今の型に全部戻す方式は、core で約 260 ms (読む 57・検証 33・逆直列化 108・索引 59。解放は別に 80 ms) で、目安に届かない
- Q-23 (検証込みで目安に収まるか) の答え: 収まる。core のアーカイブ (公開パス索引を含めて 99 MB) を mmap し、ヘッダを比べ、bytecheck で検証するまでで 42 ms。書き出しは 362 ms (索引の作成を含む)
- `data::archive`: 任意の rkyv の型を、128 バイトのヘッダの後に書く (`write`)。mmap して、ヘッダが合い検証に通れば使う (`Mapped::open`)。合わなければ作り直す対象として `None` を返す (R-CA-4 の「派生物は壊れていても安全」)
  - ヘッダ (R-CA-2): マジックナンバー、スキーマの版 (`across::ARCHIVE_SCHEMA`)、rkyv の系列 (0.8)、配置の選択 (リトルエンディアン・32 ビットの相対ポインタ。rkyv の既定)、元の形式の版、元の JSON の SHA-256 (16 進)。JSON の SHA-256 は横のファイルにあるので、開くときに JSON を読まずに比べられる
  - rkyv の系列が Cargo.lock の rkyv と一致することをテストで強制する (R-CA-2)
  - `unsafe` が 2 か所増えた: `Mmap::map` (キャッシュの中のファイルは rename でしか置き換えないことを SAFETY に書いた。他のプロセスが同じファイルをその場で切り詰めると SIGBUS で落ちうる)、`access_unchecked` (開くときに検証済み)
  - データの層は照会の層の型を知らない: 公開パス索引と crate をまとめた `Indexed` は照会の層 (`query::across`) に置き、データの層の部品は型を問わない
- 依存: rkyv 0.8.18 (MIT。既定の機能 `std`・`bytecheck`)、memmap2 0.9.11、rustdoc-types の `rkyv_0_8`。bytecheck・rend・munge・ptr_meta・rancor などが入った。cargo-deny のライセンスの検査を通る。キャッシュの部品から、確かめて開くだけの `Cache::open_file` を切り出した
- C1 に回すこと: 照会の層の書き換え、読み込みの流れへの組み込み (JSON を取ったらアーカイブを作り、次からはアーカイブを開く)。Windows では mmap しているファイルを rename で置き換えられないので、アーカイブは上書きせず新しい名前で書く (R-CA-4)。冷えたページキャッシュでの時間 (このホストでは root 権限が要るので測っていない)
- 要件に足すこと (M1 の終わり): Q-23 の答え、ヘッダの中身、rkyv の配置の選択
- テスト: アーカイブを書いて mmap で読み戻すこと、別の形式・別の元・切り詰めたアーカイブを使わないこと、rkyv の系列と Cargo.lock

### 6.3 M1c

C1 は大きいので 5 つに分けた: C1a (照会の層をアーカイブの上で動かす)、C1b (応答の骨格と provenance。版の解決・target・プロジェクトの文脈・std をつなぐ)、C1c (`detail`・応答の上限と切り詰めの印・`ambiguous_path` の候補の上限)、C1d (cfg と feature の注記)、C1e (crate をまたぐ re-export と型の正準パス)。

#### C1a (2026-10-19)

- B9 で決めた zero-copy を、照会の層に入れた。ただし照会の層を `ArchivedCrate` の型で書き直すのではなく、R-PF-1 の「item は必要になったときに初めて取り出す」のとおりにした
  - `query::view::CrateView`: アーカイブの `Indexed` (crate と公開パス索引) を包む。item・`ItemSummary`・`ExternalCrate` は、引かれたときにだけ rkyv で rustdoc-types の型に戻し、照会の間 (elsa の `FrozenMap` に) 覚えておく。描画 (render) と get_item は今の型のまま動き、出力は変わらない (スナップショットと testdata のスナップショットがそのまま通った)
  - 公開パス索引: 作る側 (`PathIndex::build`) は owned の `Crate` のまま。引く側 (`resolve`・`canonical_path`・`public_paths`・`crossing`・`hidden_reexport`・`external_globs`・`is_incomplete`) を `ArchivedPathIndex` に移した。`Resolution` と `Crossing` は借用をやめて owned の `ExternalReexport` を持つ
  - 照会の層が crate を読むのは、すべて id による点の参照だった (全体の走査は索引を作るときだけ)
- 読み込みの流れ (`query::load`、ツール)
  - まずアーカイブを探す: キャッシュの JSON の形式と SHA-256 は、ファイル名と横の digest から分かる (`Cache::docs_rs_source`)。JSON 本体は読まない。合うアーカイブがあれば、それだけで引く
  - 無ければ、今までどおり JSON を用意して (取得・否定キャッシュ・取り直しはそのまま)、解析・索引・アーカイブを書いて開く。書けなければメモリ上のアーカイブで続ける (`Mapped::in_memory`)
  - アーカイブのファイル名は `<形式>.<SHA-256 の先頭 16 文字>.v<スキーマ>.rkyv`。JSON が変われば名前も変わるので、mmap 中のファイルを上書きしない (Windows では置き換えられないため。R-CA-4)
- 入れ子の上限 (`MAX_NESTING`) を 10,000 から 4,000 に下げた。16 MiB のスタックで `[[…]]` を測ると、アーカイブから item を取り出す (rkyv の逆直列化) のが最初に溢れる: debug ビルドで 6,000 段を超えたところ、release で 20,000 段を超えたところ (40,000 で溢れる)。書き出しと検証は debug で 9,000 段まで耐えた。実際の crate は数百段。要件の R-FV-8 に足す値 (M1 の終わり)
- 依存: elsa 1.11 (MIT/Apache-2.0。追加専用のマップ)
- 扱っていないこと: プロセスの中でアーカイブを使い回すこと (今は呼び出しのたびに開いて検証する。core で 42 ms)。std と docs.rs の他の crate への組み込みは C1b・C1e
- CI の修正: 286f3f4 の push で、Windows の clippy が `src/data/cargo.rs` のテストの `use super::*` を未使用として落ちていた (テストはすべて Unix 専用だった)。テストのモジュールごと Unix 専用にした

#### C1b-1 (2026-10-19)

C1b も 3 つに分けた: C1b-1 (版・target・provenance)、C1b-2 (プロジェクトの文脈)、C1b-3 (std)。

- 引数 (§6.4): `crate` は `name` か `name@版`。`version` でも渡せる。両方あって食い違えば `invalid_argument`。版は完全一致・semver 要件・`latest` (B2 の `VersionSpec`)。省けば latest (Cargo.lock を読むのは C1b-2)。`target` は任意の target triple
- 版の解決: crates.io の sparse index (B1) で引き、`VersionSpec::resolve` (B2) で決める。index に無ければ `crate_not_found`、合う版が無ければ `version_not_found` に近い版を `candidates` で添える (§8)。index の失敗は `network_error` (一時的なものは再試行できる)。解決の警告 (pre-release・yanked だけのとき) は warnings に出す
  - 完全一致の版で、その target の JSON が既にキャッシュにあれば index を引かない (オフラインでも、テストでも引けるように)
  - crates.io の綴り (`Serde_Json` → `serde_json`) を docs.rs とキャッシュに使い、パスの先頭も code の書き方 (`-` を `_`) に直す。index を引かなかったときは、provenance の `crate` が頼まれた綴りのままになる (C1e で JSON から lib 名を取るときに直す)
- target: 引数が無ければ、起動時に `rustc -vV` で決めたホストの target (B4)。代わりの値を使ったときは warnings に理由を出す。JSON の target が求めたものと違えば warnings に出す (§7.3)。docs.rs にその target の JSON が無ければ `no_target_json` (B4。R-TG-1 のとおり M1 では代用しない)
- provenance: `resolved_from` は `explicit`・`explicit_req`・`latest` (§7.3)。outputSchema もこの 3 つにした
- 解析に失敗したときの立て直し (別の形式・取り直し) を `recover` に分けた
- 実際の crates.io と docs.rs で確かめた: latest (`serde` → 1.0.229)、semver 要件 (`serde@^1.0.200`)、完全一致 (`1.0.228`、形式 57)、無い版 (候補 5 つ)、無い crate、綴りの違い、`aarch64-apple-darwin` (`no_target_json`)。2 回目からはアーカイブから引き、ほぼ 0 秒
- テスト: 引数の解析 (版の 3 つの渡し方、食い違い、target、誤り)、パスの先頭の書き方。stdio の結合テストは `target` を明示し、キャッシュもその target の場所に置く (ホストによらないように)

#### C1b-2 (2026-10-20)

利用者が決めた 2 件 (どちらも推奨案): docs.rs を使えない取得元は `docs_unavailable` の新しい `reason: local_build_required` (M2 まで)、extern の名前と別のパッケージ名が衝突したら新しいコード `ambiguous_crate`。どちらも M1 の終わりに §8 へ足す。

- 起動時に既定のプロジェクト (B5a) と `cargo metadata` の Resolver (B5b) を作る (`local` の中)。`project` 引数 (workspace のディレクトリか Cargo.toml。相対なら既定のプロジェクトのディレクトリから) を足した
- 版を省いたときはプロジェクトに聞く (`mcp::tool::project`)
  - Cargo.lock の版 (B5c の `PackageGraph::look_up`) を使い、`resolved_from` は `cargo_lock`、`cargo metadata` が失敗したら `cargo_lock_raw` (失敗の種類と、Cargo.lock が古いときは `cargo fetch` の案内を warnings に出す)。`project_from` を provenance に足した (`argument`・`CLAUDE_PROJECT_DIR`・`cwd`)
  - 同じ crate のほかの版があれば warnings に出す。決まらなければ `ambiguous_version`、名前の衝突は `ambiguous_crate` (どちらも候補と依存元を `candidates` に)
  - 取得元が crates.io でなければ、crates.io の index を引く前に `local_build_required` を返す (同じ名前の crate が crates.io にあっても代わりにしない。実データで、member 自身が `crate_not_found` になる不具合を見つけて直した)。crates.io なら index の `cksum` と Cargo.lock の checksum を比べ (B5c の `docs_rs_check`)、合わなければ `local_build_required`
  - プロジェクトの依存でなければ latest にして warnings に出す (R-VR-3)。プロジェクトが無いときも latest
  - 既定のプロジェクトの直下に workspace が複数あって `project` が無ければ `project_not_found` に候補を付ける。既定のプロジェクトの外を `project` で指したら cargo を起動せず `no_record` (R-MCP-3。外のツールの記録は M2)。Cargo.lock が無ければ `lockfile_outdated` (`cargo fetch` を案内)
  - 版を明示したときはプロジェクトを読まない (R-VR-1 の「プロジェクトの取得元と一致すれば warnings」は、ローカルビルドを入れる M2 で扱う)
- ローカル機能が無い構成では、プロジェクトを読まずに latest にして warnings に書く (`project` は無視する)
- このリポジトリを既定のプロジェクトにして実際に確かめた: serde は Cargo.lock の 1.0.229 (`cargo_lock`)、windows-sys は 2 版で `ambiguous_version` (依存元付き)、member 自身は `local_build_required`、明示した `latest` は `latest`
- 扱っていないこと: `cargo metadata` の初回の依存の取得で時間がかかるときの待ち方 (R-WS-6 の `status: building` は M2)。オフラインモードの設定
- テスト: workspace の選び方 (既定・`project`・外・複数)。stdio の結合テストで、合成の workspace を `CLAUDE_PROJECT_DIR` にして member を引くと `local_build_required`、外の workspace を `project` で指すと `no_record`。stdio のテストのサーバーは、プロジェクトを指定しないときは `CLAUDE_PROJECT_DIR` を消してキャッシュのディレクトリで動かす (このリポジトリを読まないように)

#### C1b-3 (2026-10-20)

- `get_item` で std・core・alloc・proc_macro・test を引けるようにした (R-STD-1・R-STD-6)。B8 の `std_json::choose` で手元の nightly か dist を選び、`across::follow` で std の仲間をたどる (`std::vec::Vec` → `alloc::vec::Vec`。warnings に「re-export を、その crate で示した」旨を出す)
  - 手元の nightly を使ってよいかの基準 (プロジェクトの toolchain の版) は、既定のプロジェクト (workspace が 1 つのとき) のルートで `rustc -vV` を実行して取る (`RUSTUP_AUTO_INSTALL=0`)
  - std に `version` を渡されたら無視して warnings に書く (std は crates.io の版を持たない)
- アーカイブ: std の各 crate の JSON を、初回だけ解析してアーカイブにする (`load::archive_of`)。鍵は、dist の JSON なら横の SHA-256、手元の nightly ならコミットハッシュから作ったもの (sysroot のファイルはコミットごとに変わらないので、96 MB の core.json を毎回読んでハッシュしない)。アーカイブのヘッダの照合から形式の版を外した (元の SHA-256 が形式を決めるので。先に形式を知らなくても開ける)。既存のアーカイブはヘッダの配置が変わって合わなくなり、作り直される
- provenance: `source: "std"`、`toolchain` (rustc の版とコミット)、`std_from` (`dist`・`local` と理由)。std の `resolved_from` は出さない。std の JSON の `crate_version` は `1.101.0-nightly\t(コミット\t日付)` の形で semver として読めないので、toolchain の release を版にし、要求との比較もしない (`ItemRequest` に `Origin` (docs.rs か std か) を持たせた)
- std の定義位置 (`/rustc/<コミット>/library/…`) には、「パッケージの外」の警告を出さず、rust-lang/rust の GitHub のそのコミットの URL を付ける
- 解決の順を変えた: 「公開パス → 定義パス → 親の型のメンバー」から「公開パス → 親の型のメンバー → 定義パス」へ。nightly の std はメソッドも定義パス (`paths`) に載せていて、`Vec::push` が親も `impl` も無い単独の item として返っていたため。モジュールの下の item はメンバーにならないので、結果が変わるのはメンバーだけ (R-PR-1 の手順に足すこと。M1 の終わり)
- 実際のサーバーで確かめた (このホストの nightly には rust-docs-json が無いので dist): `std::vec::Vec`・`std::vec::Vec::push` (alloc)、`std::collections::HashMap::insert` (std)、`core::option::Option::map`、版を渡した std、無いパス。初回はアーカイブ作りで約 1.5 秒、2 回目からは約 0.1 秒
- テスト: 解決の順 (定義パスにも載ったメンバー)、std の名前の判定、std のソースの URL への変換、鍵付きのアーカイブを一度だけ作ること。std を通しで引くテストは `tests/std_dist.rs` (ignored。CI の testdata ジョブで動く)

#### C1c (2026-10-20)

- `detail` 引数 (R-OUT-7): `brief`・`normal` (既定)・`full`。それ以外は `invalid_argument`
  - `brief`: パス・種類・親・シグネチャの 1 行目 (`macro_rules!` の規則は省く) と `reexport_of` だけ
  - `normal`: 今までの内容に、ドキュメントの見出しごとの節 (`sections`) の見出しと行数を足した。`docs` は最初の見出しより前の第 1 段落
  - `full`: 最初の見出しより前の全文を `docs` に、各節の本文を `sections[].text` に、属性を `attrs` に出す
- ドキュメントの整形 (`query::docs`): コードブロックの外の ATX 見出し (`#`〜`######`) で節に分ける。Rust のコードブロック (info string が無いか、doctest の属性だけ) から隠し行 (`#` だけの行と、`#` の後に空白が続く行) を除き、行頭の `##` を `#` にする (R-OUT-6)。`text` などほかの言語のブロックは触らない
- `attrs`: rustdoc-types の `Attribute` を Rust の書き方に戻す (`#[repr(C, align(8))]` など)。`Other` はそのまま出すが、cfg の痕跡 (`#[<cfg`・`#[attr = Cfg…`・`#[doc(cfg`) は C1d の feature の注記に任せて除き、`#[doc = "…"]` と rustc 内部の属性 (`#[attr = Rustc…`。std の `RustcDiagnosticItem` など) も除く
- 応答の上限 (R-OUT-3): 応答の JSON を 32,000 文字 (トークンの見積もりを測るまでの仮の値。約 8,000 トークン) に収める。超えたら次の順に削り、`truncated: true` を付ける
  1. 節の本文を後ろから
  2. `docs` を半分ずつ (200 文字までは残し、末尾に `…`)
  3. フィールドかバリアントの多いほうを半分ずつ (`/* N more not shown */` を足し、rustdoc の `/* private fields */` は末尾に残す)。warnings に「全 N 件のうち M 件」と出す
- `ambiguous_path` の候補は最大 20 件にし、総数を `candidates_total` で添える
- outputSchema に `sections`・`attrs`・`truncated` を足した。ツールの説明に `detail` を足し、2,048 文字に収めるため例を 1 つ削った (2,040 文字)
- testdata のスナップショット: 8 件すべてに `sections` (見出しと行数) が増えた。それ以外は変わらない
- 実際のサーバーで確かめた: `tokio::sync::Mutex` (brief 267 文字、full 4,875 文字)、`tokio::select` (full 18,567 文字。節 7 つ、`#[macro_export]`)、`std::vec::Vec` (full。`#[doc(alias = "list")]` など)。実データで上限を超えるものはまだ見つけていない (切り詰めは単体テストで確かめた)
- テスト: 節への分割と隠し行、detail の 3 段階、属性の書き方と除くもの、上限への切り詰め (節の本文・docs・フィールドの順) と上限内なら削らないこと、`detail` の解析、候補の上限
- 扱っていないこと: 文字数とトークン数の対応の測定 (R-OUT-3 の見積もり)。MCP の応答は `structuredContent` と `content` の text に同じ JSON を二重に載せるので、送る量は上限のおよそ 2 倍になる

#### C1d-1 (2026-10-20)

C1d も 3 つに分けた: C1d-1 (cfg を読んで item の条件を出す)、C1d-2 (プロジェクトの feature の組で `enabled_in_project`・`enabled_in_dev_only` を出す)、C1d-3 (R-FT-1。`.crate` の `[package.metadata.docs.rs]` から docs.rs のビルドの feature を求める)。

- cfg の読み取り (`query::cfg`): 実データ (testdata の 20 crate、形式 46・48・57・61) で見つかった表記は 4 系統
  - `#[doc(cfg(...))]` (全形式で同じ) と、形式 46・48 の痕跡 `#[<cfg>(...)]` は Rust の構文。属性の並び (`name`、`name = "値"`、`name(...)`) として読み、`all`・`any`・`not`・`true`・`false` を式にする
  - 形式 57・61 の痕跡 `#[attr = CfgTrace([...])]` は rustc の Debug 表記。`NameValue { name, value, span }`・`All([...], span)`・`Any`・`Not`・`Bool` を読み、位置 (`span: src/macros/cfg.rs:480:19: 480:33 (#4230)`) は閉じ括弧まで読み飛ばす。並びの各項は「かつ」
  - `CfgAttrTrace` と `<cfg_attr>` は、ほかの属性に掛かる条件で item の条件ではないので読まない
  - 読めない cfg の属性は `cfg_source: unknown` にする。testdata の 20 crate では cfg の属性 12,977 件がすべて読めた (testdata の結合テストで確かめる)
- item の条件 = パス上の条件の「かつ」(R-FT-2): 正準パスがたどるモジュールと `use` (公開パス索引に記録するようにした。アーカイブのスキーマを 2 に上げた)、関連 item なら親の型・trait・enum とその正準パスと `impl`、最後に item 自身。各 item では `doc(cfg)` があればそれを、無ければ痕跡を使う。同じ条件は 1 度だけ
  - 通常のビルドでは `doc` と `docsrs` は偽なので畳む (`any(all(doc, docsrs), windows)` → `windows`)。畳むと偽になるもの (`#[cfg(docsrs)]` の item) は、文書にだけある item だと分かるように元のまま出す
- 出力: 一番外の「かつ」の各項を、feature だけのもの (`required_features`。名前だけで `any(rt, rt-multi-thread)` のように書き、複数なら `all(...)`) と、それ以外 (`other_cfg`。`cfg` の書き方で 1 項ずつ。feature とそれ以外が混ざった項もこちら) に分ける。`cfg_source` は item 自身の情報源 (`doc_cfg`・`cfg_trace`)、自身に無く親や経路から来たなら `inherited`、読めないものがあれば `unknown`。条件が無ければどれも出さない。`detail=brief` でも出す (R-OUT-7)
- 実データ: `tokio::spawn` は `rt` (doc_cfg)、`tokio::sync::Mutex::lock` は `sync` (モジュールから)、`tokio::runtime::Builder::new_multi_thread` は `all(rt, rt-multi-thread)`、`std::os::unix::fs::PermissionsExt` は `unix`、`reqwest::blocking::Client` は `blocking` と wasm 以外。testdata のスナップショット 8 件に注記が増えた
- 痕跡から継ぐ条件には、crate の内部の cfg も入る: `tokio::net::UnixStream` の `not(loom)` (`net` モジュールに `doc(cfg)` が無く、痕跡を使うため)、std の `not(no_global_oom_handling)`。出すのは正しいが、C1d-2 で `enabled_in_project` を求めるとき、feature 以外の cfg をどう評価するかを決める必要がある
- 扱っていないこと: `also_reexported_at` の各パスの条件 (正準パスと違うときだけ添える。R-FT-2)、`#[doc(auto_cfg(hide(...)))]`、crate の外への re-export (`reexport_of`) の条件
- テスト: 3 系統の読み取り (位置の読み飛ばし、入れ子、読めないもの)、`doc`・`docsrs` の畳み方、feature の書き方と 3 値の評価、re-export・`impl`・enum からの継承、条件の無い item

#### C1d-2 (2026-10-20)

利用者が決めた (推奨案): `enabled_in_project` では、feature 以外の条件を docs.rs のビルドと同じく成り立つとみなす (照会に使う JSON は、評価する target と同じ target のものだけなので、`unix`・`not(loom)` などは docs.rs と同じ値になる)。docs.rs が `[package.metadata.docs.rs]` の `--cfg` で立てたもの (`tokio_unstable` など) が絡むときだけ `unknown` にして条件を添える。立てた cfg は `.crate` を読まないと分からないので、C1d-2 と C1d-3 の順を入れ替え、`.crate` を読む段を先にした。

- Windows・macOS でも前提は崩れない: docs.rs の JSON を他の target で代用しない (R-TG-1) ので、使う JSON はいつも評価する target のもの。ただし次の 2 つは手当てか記録が要る
  - JSON の target が評価する target と違うとき (今は warnings だけ): feature 以外の条件を `unknown` にする (C1d-3)
  - プロジェクトの `.cargo/config.toml` の `build.target` をまだ読んでいない (R-TG-1 の残り)。たとえば Linux で wasm 向けにビルドするプロジェクトでも、ホストの target で評価する。feature の組 (`cargo tree --target`) も同じ target で取るので、内部の食い違いは無い
- `.crate` (`data::crate_file`): static.crates.io の `crates/<name>/<name>-<version>.crate` から取る。ビルドメタデータの `+` はそのままでも `%2B` でも通り、無いものは 403 (2026-10-20 に確認)。sparse index の `cksum` と照合してから (R-SC-2)、キャッシュの `static.crates.io/<鍵>/<版>/` に横の digest と一緒に置く。展開はせず、`<name>-<version>/Cargo.toml` のエントリだけを読む (cargo は `Cargo.toml` を先頭近くに書く。tokio では 4 番目)。上限: `.crate` 128 MiB、`Cargo.toml` を探すまでに展開する量 256 MiB、`Cargo.toml` 4 MiB (初期値)。依存に flate2 1.1.10 を足した (miniz_oxide の経路だけ)
  - 版を index なしで決めたとき (キャッシュにある完全一致の版) は、`.crate` がキャッシュに無ければ index を引いて `cksum` を得る。取れなければ warnings に書いて照会は続ける
- `[package.metadata.docs.rs]` から求めるもの (R-FT-1): `all-features`・`features`・`no-default-features` による有効な feature、`rustdoc-args`・`rustc-args` の `--cfg` の名前 (`docsrs` と `feature` を除く)、有効にならなかった feature。最後のものは、`[features]` と、`dep:` で名指しされない optional の依存 (`[target.*]` の下と build-dependencies も) が作る feature のうち、有効な feature から推移的にたどれないもの (`dep/feat` は依存の feature を有効にし、`dep?/feat` はしない)。feature 名と cfg 名は文法に合うものだけを受ける (R-SEC-4)
- provenance に `features` (有効にした feature。`all_features` のときは省く。tokio では 26 個になるため) と `all_features`、`cfg` (docs.rs が立てた cfg) を足した。有効にならなかった feature があれば、warnings に「docs.rs は [..] なしでビルドした。それらが要る item はこの文書に無い」と出す (§7.3 の例は有効な feature を挙げる形だったが、欠けたものを挙げるほうが答えに直結する)
- 実データ: tokio 1.53.3 と reqwest 0.13.5 は `all_features`、`cfg` はそれぞれ `tokio_unstable`・`reqwest_unstable`。serde 1.0.228 は `alloc` が、hyper 1.12.0 は `capi`・`nightly` が欠けて warnings が出た (hyper の `cfg` は `hyper_unstable_ffi`・`hyper_unstable_tracing`)。初回は `.crate` の取得で約 0.3 秒、2 回目からは 0.01 秒
- stdio の結合テスト: 合成の `.crate` をキャッシュに置く (crates.io に `demo` という crate が実在し、置かないと本物を取りに行った)
- 直した不具合: stdio の `without_a_version_the_project_decides_it` は、ローカル機能なしの構成 (`--no-default-features`) では毎回落ちていた (プロジェクトを読まないので latest になり、crates.io の本物の `demo` 0.1.1 を引いて `no_json` になる)。C1b-2 で足したテストで、CI では C1b-2 から落ちるはずだった。C1c の報告で「たまに落ちる」としたのは誤りで、再現を試したのが既定の構成だけだったため。テストをローカル機能のときだけにした
- テスト: feature の求め方 (`all-features`、名指し、`default`、`no-default-features`、`--cfg` の 2 つの書き方)、推移的にたどって欠けた feature を求めること、`Cargo.toml` の無い `.crate`、stdio での provenance

#### C1d-3 (2026-10-20)

- `enabled_in_project` (`true`・`false`・`"unknown"`) と `enabled_in_dev_only` (真のときだけ) を出す (R-FT-2)。プロジェクトが版を決めたとき (`cargo_lock`) だけ。B5d の `cargo tree` の feature の組 (通常のビルドと、dev を含むビルド。評価する target のもの) を使う。組を取れなければ (`cargo metadata` が失敗して `cargo_lock_raw` になったときなど) 出さず、warnings に書く。`detail=brief` でも出す
- 評価 (C1d-2 の利用者の決定どおり): item の条件の一番外の「かつ」の各項を 3 値で評価して「かつ」で結ぶ
  - feature を含む項: プロジェクトの組で評価する。`doc`・`docsrs` は偽、ほかの cfg は不明
  - feature を含まない項: docs.rs のビルドで成り立っていたので、真とみなす。ただし docs.rs が `--cfg` で立てた名前 (C1d-2 の `cfg`) を含むか、JSON の target が評価する target と違えば、上と同じく評価する (多くは不明)。`docsrs` だけの項 (文書にだけある item) は偽
  - 通常のビルドがその crate を作らなければ `false`。通常のビルドで真でなく、dev を含むビルドで真なら `enabled_in_dev_only`
- このリポジトリで確かめた (tokio 1.53.3。プロジェクトの組は `bytes,default,io-std,io-util,…,net,process,rt,rt-multi-thread,sync,time`): `tokio::spawn`・`tokio::net::TcpStream` (`not(loom)` があっても)・`tokio::sync::Mutex`・`tokio::process::Command` は `true`、`tokio::fs::File` は `false`、`tokio::task::Builder` (`tokio_unstable` と `tracing`) は `tracing` が無いので `false`、`RuntimeMetrics::spawned_tasks_count` (`tokio_unstable`) は `"unknown"`。dev-dependency だけの `tempfile::TempDir`・`insta::assert_json_snapshot` は `false` で `enabled_in_dev_only`。初回は `cargo tree` で約 0.6 秒、2 回目からは 0.01 秒
- 扱っていないこと: 版を明示して、それがプロジェクトの版と同じときの評価 (今はプロジェクトが版を決めたときだけ)。feature とそれ以外が混ざった項 (`any(feature = "a", unix)`) の、feature 以外の部分の評価 (不明として扱う)。プロジェクトの `build.target` (C1d-2 の記録のとおり)。一覧系のツールの各項目への注記 (C2 以降)
- テスト: feature で決まること、docs.rs が立てた cfg で不明になること、欠けた feature が不明より勝つこと、dev だけの item と crate、他の target の JSON で不明になること

#### C1e (2026-10-20)

利用者が決めた 2 件 (どちらも推奨案): 他の crate への re-export は行き先の中身まで自動でたどって返す (R-PR-4 の「M1 で決める」)。シグネチャの中の他の crate の型は、M1 では rustdoc が渡すソースの書き方のまま描き、他の crate の索引で正準パスにするのは M4 (依存グラフ全体の作業) に回す (§7 の型のパスの「M1 から」を M1 の終わりに改める)。

- 他の crate へ続くパス (R-PR-4): パスが見つからないか他の crate の item の re-export に当たったら、その先頭のうち最も長い re-export (公開パス索引の `crossing`) の行き先を求め、その crate で引き直す (`GetItemError::Elsewhere`)。たどるのは最大 3 crate 先まで
  - 行き先の版: 起点の版をプロジェクトが決めたときはプロジェクトの Cargo.lock の版 (同じ crate の版が複数なら、起点が docs.rs でビルドされたときの版と同じもの)。次に起点の JSON の `html_root_url` (docs.rs の URL に crates.io の名前と版が入っている。`resolved_from: "reexport"` を足した)。どちらでも決まらなければ latest にして warnings に書く。rustdoc は re-export の連鎖を最終の定義の id に解決しているので、起点と参照元のどちらの `html_root_url` を先に見るかという問いは実際には起きない
  - 行き先が std の仲間 (`html_root_url` が doc.rust-lang.org) なら std の経路で引く (`tokio::io::Error` → `core::io::Error`)。std の私的な依存 (`ExternalCrate::path` が sysroot の `rustlib` の下にあるもの) はたどらない (crates.io の同名の crate と取り違えないため。形式 54〜56 は `path` が無いので見分けられない)
  - たどれなかったら、起点を「たどらない」で引き直して M0 と同じく `reexport_of` と hint を返し、理由を warnings に書く。それもエラーなら、行き先の crate のエラー (`http@1.5.0 has no item at http::header::NoSuchThing`) を返す
  - warnings に「`reqwest::header` は `http::header` (http 1.5.0) の re-export なので、`reqwest::header::HeaderMap` を `http::header::HeaderMap` としてそこから示した」の形で書く
- 実データ: `reqwest::header::HeaderMap` (http 1.5.0)、`reqwest::Url` (url 2.5.8)、`compio::io::AsyncWriteAt` (compio-io 0.10.1。§3 Q20)、`axum::http::StatusCode` (http 1.3.1)、`tokio::io::Error` (core)。このリポジトリのプロジェクトで版を省くと、http もプロジェクトの Cargo.lock の 1.5.0 (`cargo_lock`) で、`enabled_in_project` も付く。初回は行き先の JSON の取得で 0.4〜0.7 秒
- std の安定性 (§3 Q17): 形式 61 の `Item::stability`・`const_stability` から `stability` (`stable_since`・`unstable_feature`・`const_stable_since`・`const_unstable_feature`) を出す。unstable なら必要な `#![feature(...)]` を、stable になった版がプロジェクトの toolchain より新しければその旨を warnings に書く (`std::never` は 1.100.0 で、プロジェクトの 1.99.1 より新しい)。docs.rs の普通の crate には入っていない
- 綴りと lib 名: 版を index なしで決めたとき (キャッシュにある完全一致の版) は、provenance の `crate` が頼まれた綴りのままだった。`.crate` の `Cargo.toml` の `package.name` を正とした (`Serde_Json@1.0.154` は `serde_json`)。パッケージ名と lib 名が違う crate (`md-5` の lib は `md5`) では、パスの先頭のパッケージ名を JSON のルートの名前 (lib 名) に置き換える (`md-5::Md5` も `md5::Md5` も引ける)
- テスト: 他の crate へ続くパス (re-export そのものと、その下のパス)、std の私的な依存をたどらないこと、std の仲間への行き先、std の安定性と warnings、lib 名への置き換え、`package.name` の読み取り

#### C1 の終わり (2026-10-20)

C1 の終わりの印は満たした:

- §3 Q5 (`tokio::sync::Mutex::lock` のシグネチャ・ドキュメント・panic 条件): `detail=full` でシグネチャ、`sync` の条件、本文、節 (Cancel safety・Examples。panic の節は無い) が返る
- §3 Q17 (この std API は自分の toolchain や MSRV で使えるか): `stability` と warnings で答えられる (MSRV を踏まえた表示は M4)
- §3 Q20 (`compio::io::AsyncWriteAt` はどこで定義されているか): compio-io 0.10.1 の `compio_io::AsyncWriteAt` として返り、warnings に re-export の経路が出る
- `reqwest::header::HeaderMap` を引ける (http の `http::HeaderMap`)

M1 の終わりに要件へ反映するもの (C1 の分): `detail` の各段の中身と `sections`・`attrs`・`truncated` (R-OUT-7・R-OUT-3)、応答の上限の初期値 32,000 文字と削る順、`candidates_total`、cfg の 4 系統の表記と `doc`・`docsrs` の畳み方 (R-FT-2)、`enabled_in_project` の評価の規則 (C1d-2 の決定)、provenance の `features`・`all_features`・`cfg` と欠けた feature の warnings (R-FT-1・§7.3)、`resolved_from: "reexport"`、他の crate へのパスを自動でたどること (R-PR-4)、型のパスの扱い (§7。M4 へ)、`stability` (Q17)、`.crate` の取得 (R-SC-2 の初期の上限)、R-NW-4 の見直し

#### C2 (2026-10-20)

`resolve_version` を足した (§6.3)。要件には表の 1 行と応答の骨格 (R-OUT-9) しか無いので、細部を次のように決めた (M1 の終わりに要件へ反映する)。

- 引数: `get_item` から `path`・`detail` を除いたもの (`crate[@版]`・`version`・`project`・`target`)。解析は `get_item` と共有した (`parse_release`)
- 版の決め方は `get_item` と同じ (明示 > プロジェクトの Cargo.lock > latest)。違いは 2 つ: プロジェクトの path・git・別レジストリのパッケージをエラー (`local_build_required`) にせず、取得元として答える。キャッシュの近道を使わず常に crates.io の index を読む (latest と yanked のため)。同名の複数版は `get_item` と同じく `ambiguous_version`
- 応答: provenance は `source` (`crates.io`、プロジェクトだけのパッケージは `project`)・`crate`・`version`・`resolved_from`・`project_from`・`target`。`data` は `version`・`source` (プロジェクトがどこから取るか: `crates.io`・`path`・`git`・`registry`)・`yanked` (真のときだけ)・`rust_version` (index の MSRV)・`latest` (crates.io の最新の stable)・`docs_rs_json`
- `docs_rs_json`: キャッシュにあれば `available`。無ければ docs.rs に `HEAD` で聞く (その target の JSON → 無ければ既定の target の JSON): `available`・`no_target_json`・`no_json`。プロジェクトの複製が crates.io のものでなければ `not_on_docs_rs` (理由を warnings に)。聞けなければ `unknown`
- std の仲間は crates.io の版を持たないので `invalid_argument` にし、`get_item` を案内する
- 実データ (§3 Q2): `axum@latest` は 0.8.9 (MSRV 1.80)、このリポジトリの `tokio` は Cargo.lock の 1.53.3 で latest は 1.54.0、この crate 自身は `path` で `not_on_docs_rs`、`serde@1.0.200` は `no_json` (2025-05-23 より前)、mac 向けの `tokio@1.53.3` は `no_target_json`、`windows-sys` は 2 版で `ambiguous_version`
- テスト: 引数の解析、outputSchema が応答を覆うこと、説明文の長さ、stdio で std と誤った引数がネットワークなしにエラーになること、`tools/list` の一覧

#### C3a (2026-10-20)

C3 を 2 つに分けた: C3a (`list_module` と一覧の共通の仕組み)、C3b (`crate_overview`)。利用者が決めた (推奨案): カーソルはオフセットにし、束縛を付ける (R-OUT-3 の「M1 で決める」)。

- カーソル: `c1.<要求のダイジェスト>.<データのダイジェスト>.<位置>`。要求のダイジェストは `cursor` と `limit` を除いた引数 (キーの順によらない) の SHA-256 の先頭 16 文字で、他の crate へたどった後も同じカーソルが通る。データのダイジェストは元の JSON の SHA-256 (std は鍵) の先頭 16 文字で、アーカイブに持たせた (`Indexed::source`)。食い違い・形の誤り・終わりを越えた位置は、理由付きの `invalid_argument` で最初から取り直させる。並びは決定的でデータは版ごとに固定なので、オフセットで飛びや重複は起きない。サーバーは状態を持たない
- `list_module` (R-LS-1・R-LS-2): 引数は `get_item` の版を名指すもの (`crate`・`version`・`project`・`target`) と `path` (省けば crate のルート)・`recursive`・`depth` (既定 3、最大 8)・`kind` (1 つか配列。`get_item` の kind の語)・`detail`・`limit` (既定 50、最大 200)・`cursor`
  - 子の列挙: 公開パス索引に「公開パス → 1 段下の公開パス (整列済み)」の表を足した (アーカイブのスキーマを 3 に上げた)。他の crate からの re-export も子として並べ、`reexport_of` を付ける。一覧のパスと正準パスが違う item (re-export) には `canonical` を付ける。並びはパスの辞書順で、`recursive` ではモジュールの直後にその中身が来る。同じモジュールは 1 度だけ (re-export の循環に備えて)。kind の絞り込みは切る前に行い、`total` は絞り込んだ後の件数 (R-OUT-4)
  - 各項目は `get_item` の `ItemData` を使い回す。`brief` は 1 行のシグネチャと cfg の注記 (R-FT-2) と `enabled_in_project`。`normal` は where 句・ドキュメントの第 1 段落・deprecated なども (fields・variants・節・定義位置は除く)。`full` は各 item を `get_item` の既定の段で。モジュール自身のドキュメントの第 1 段落は `normal` 以上で返す
  - `detail` の既定は `brief` にした (R-OUT-7 の既定は `normal` だが、一覧は件数が多く、R-LS-1 の骨格も `brief` との組み合わせを想定している。M1 の終わりに要件へ反映する)
  - 予算 (32,000 文字) を超えるページは、件数を減らして収め、続きをカーソルで示す (tokio の全体を `recursive` で引くと、`brief` で 150 件・約 25,000 文字、`normal` で 112 件・約 30,000 文字のページになった)
  - モジュールでない item を指したら `invalid_argument` (kind を添えて `get_item` を案内)。他の crate のモジュール (`reqwest::header`) は `get_item` と同じ仕組みでたどって一覧する (照会の流れに「何を返すか」(`Shape`) を持たせ、`get_item` と `list_module` で共有した)。std も同じ
- std の re-export の `reexport_of` で「版が分からない」と warnings に出ていたのをやめた (std の仲間は toolchain の版なので、crate 名だけにする)。「他の crate の item がこのデータに無い」の warnings にパスを入れた
- 実データ (§3 Q6): `axum::routing` は 31 件 (`Router` は `canonical: axum::Router`)、`reqwest::header` は http 1.5.0 の `http::header` を 104 件、tokio のモジュールにはそれぞれ必要な feature (`fs`・`rt`・`sync` など)、`std::collections` は alloc・core の re-export に `reexport_of`。カーソルで次のページが取れる
- テスト: 子の並びと re-export、`recursive` と kind の絞り込み、ページとカーソル (データの食い違い、終わりの先)、モジュールでない item、引数の誤り、要求のダイジェストがキーの順によらないこと、stdio で合成の crate を一覧すること

#### C3b (2026-10-20)

`crate_overview` を足した (§6.3。§3 Q3)。要件は「ルートのドキュメントの要約、主要なモジュールと item、feature の概要」の 1 行なので、中身を次のように決めた (M1 の終わりに要件へ反映する)。

- 引数: `get_item` の版を名指すものと `detail` (`brief`・`normal` (既定)・`full`)。照会の流れの `Shape` を「item・一覧・全体像」の 3 種にし、版の決定・キャッシュ・std の経路を共有した
- `data`: `path` (crate のルート)、`docs` (ルートのドキュメントの第 1 段落。見出しから始まるもの (serde・std) は最初の節の第 1 段落)、`sections` (`normal` で見出しと行数、`full` で本文も)、`counts` (公開パスを持つ item の種類ごとの数)、`modules` (ルートのモジュール。それぞれドキュメントの最初の 1 文と cfg の注記。他の crate から re-export したモジュールもここ)、`items` (ルートのほかの item を最大 50 件、`brief` で。`items_total` を添え、超えたら `list_module` を hints で案内)、`features` (`.crate` を読めたとき: `all`・`default` が有効にするもの・docs.rs で欠けたもの `not_in_docs`・プロジェクトが版を決めたときは `enabled_in_project`)
- 予算を超えたら、節の本文 → ルートの item → モジュールの 1 文 → ルートの本文の順に削り、`truncated` を付ける
- `.crate` の読み取りに、宣言された feature の一覧と、`default` が推移的に有効にする feature を足した
- std の中から参照される crate (`std_detect` など) は、std の JSON では `html_root_url` が `docs.rs/<name>/latest/`、パスが toolchain のビルドの場所なので、std を引いているときは版の warnings を出さないようにした
- 実データ: tokio (このリポジトリの Cargo.lock の 1.53.3) は struct 162・function 73・module 37 など、モジュール 11 個にそれぞれ必要な feature、feature 26 個でプロジェクトで有効な 16 個。axum は `default` が有効にする 11 個。serde は `not_in_docs: [alloc]`。std はモジュール 60 余り・ルートの item 75 件で、残りを `list_module` に案内
- テスト: 数・モジュール・item の並び、最初の 1 文の切り方、引数、説明文の長さ

#### C4a (2026-10-20)

利用者が決めた (推奨案): C4 を C4a (crate を指定した検索) と C4b (`crate` を省いたときのプロジェクトの直接依存の横断。`partial`・`sources`) に分ける。workspace の分は計画どおり M2。C4a の間は `crate` を省くと `invalid_argument`。

- 索引 (R-TI-4 の「M1 で決める」の一部): アーカイブを作るときに検索の索引も作り、一緒に持つ (アーカイブのスキーマを 4 に上げた)。対象は、公開パスを持つ item、型のメンバー (固有の impl の item・trait の item・enum の variant。trait の impl の item は trait 側と重なるので除く)、他の crate からの re-export (名前とパスだけ。std の公開 item の多くは core・alloc の re-export なので、名前で引けるように)。std の unstable な item は除く (R-SR-4)。各 item について、トークンごとに名前・`doc(alias)`・ドキュメントの第 1 段落・残り・祖先のパス名 (近い順に 3・2・1) の出現数を持つ (`fields` で欄を選べるように)。コードブロックは索引しない。core では作り直しが初回に約 2.5 秒、アーカイブが 100MB から 113MB (+13%) になった
- トークン: 語を丸ごとと、`_` と CamelCase の切れ目 (頭字語を含む。`HTTPServer` は `http`・`server`) で分けたもの。1 文字の部分は捨てる。最後の語を前方一致で展開しない (R-SR-3)
- 順位 (R-SR-3。数値は初期案): 段の順に、パスの一致 (クエリが `sync::Mutex` のようなパスで、正準パスの末尾と一致)、名前の完全一致 (大文字小文字を除いて。語をつないだ名前 `unwrap or default` → `unwrap_or_default`・`hash map` → `HashMap` も)、最後の語の一致、別名の一致、部分一致。同じ段の中では大文字小文字まで一致するものが先。部分一致は BM25 (k1 = 1.2、b = 0、Lucene 型の IDF)。重みは名前 20・別名 10・第 1 段落 3・残り 1・祖先のパス名 3/2/1。被参照数 (intra-doc link の入次数) を `1 + 0.1 × ln(1 + n)` で掛け、`__`・`_private`・`internal` を含むパスは半分にする。並びは段、スコアを 1000 倍して丸めた値、パスの順で決定的にする (R-OUT-4)
- 打ち切り (R-SR-4): `limit` を省くと、最初のページは部分一致より上の段の全部と、部分一致を、最上位の 5% 未満・直前の半分未満・それまでの累積が全体の 30% を超えた、のどれかで切ったところまで (少なくとも 1 件、多くて 50 件)。続きは `limit` が無ければ 20 件ずつ。カーソルは `list_module` と同じ形 (要求とデータのダイジェストに束縛)
- 0 件のとき (R-SR-5): 名前との編集距離 (文字で数える) が 3 以下で、長いほうの 70% 以上が一致するものを、距離・名前の順に 5 件まで `did_you_mean` に返す (同じ名前ならメンバーでない短いパス)。似ていなければ返さない (R-PR-7)
- 引数: `crate` (C4a では必須)・`query` (必須)・`version`・`project`・`target`・`fields` (`name`・`alias`・`docs`・`path`)・`kind`・`detail` (`brief` (既定。1 行のシグネチャ・cfg の注記・ドキュメントの最初の 1 文)・`normal`)・`limit` (最大 200)・`cursor`
- 実データ: tokio で `Mutex` → `tokio::sync::Mutex`、`spawn blocking` → `tokio::task::spawn_blocking`、`timeout` → `tokio::time::timeout`、`read file to string` → `tokio::fs::read_to_string`、`Mutx` → `did_you_mean: [tokio::sync::Mutex]`。core で `Option map` → `Option::map`、`unwrap or default` → `Result::unwrap_or_default`・`Option::unwrap_or_default`。std の `average` (fields: alias) → 整数型の `midpoint`。2 回目からの検索は 0.01〜0.1 秒
- 分かっている限り: std を検索しても、core・alloc の item のメンバー (`Vec::push` など) は出ない (std の JSON には名前とパスしか無いため)。std の仲間の横断は C4b で扱う。今の nightly の `HashMap` には `doc(alias = "dict")` が無い
- テスト: トークンの分け方、名前の一致が上に来ること、メンバーと他の crate の re-export、別名、語をつないだ名前、`did_you_mean` (似ていなければ出さない)、最初のページの打ち切り、引数の誤り、stdio で合成の crate を検索すること

#### C4b (2026-10-20)

利用者が決めた (推奨案): `crate` を省いた `search_items` は、プロジェクトの member の直接依存のうち、評価する target の通常のビルドか dev を含むビルドに入るもの (`cargo tree` の feature の組で判定) を横断する。build の依存と他の OS 向けは除く。std は含めず、`crate: "std"` のときに std・alloc・core を横断する。

- 範囲 (R-SR-2): 依存グラフに「member の直接依存 (member を除く)」を返す口を足した。feature の組を取れなければ (`cargo_lock_raw` など) 直接依存をすべて範囲にして warnings に書く。path・git・別レジストリの依存は、docs.rs に無いので `sources` に `unavailable` (`local_build_required`) として載せる。プロジェクトが無いとき・ローカル機能が無い構成では `crate` が要る (`invalid_argument`)
- 読み込み: 範囲の crate を並行に、アーカイブがあれば開き、無ければ docs.rs から JSON を取ってアーカイブを作る (`.crate` は読まない。範囲が広いと時間がかかるため、provenance の `features` は出さない)。20 秒 (R-ARCH-6 の 1 リクエストの上限 30 秒の内) で待つのをやめ、終わっていない crate は `sources` に `building` (`building_reason: fetching`・`retry_after_secs: 10`) で載せ、応答の最上位に `partial: true` と `retry_after_secs` を付ける。取得は裏で続き、キャッシュに入るので、次の呼び出しで使われる。失敗したものは `failed` に、`get_item` と同じ `code`・`reason`・`retryable` を付ける
- 順位: BM25 の統計 (item の総数と各トークンの文書頻度) を範囲のすべての crate で合わせ、結果を 1 つの並びにする (R-SR-3)。カーソルのデータのダイジェストは、範囲の crate のダイジェストを並べたものの SHA-256 (取得が進んで範囲の中身が変わると、前のカーソルは「データが変わった」で拒まれる)
- 応答 (R-OUT-9 の複数の crate の形): provenance は共通部分 (`target`・`project_from`) と、crate ごとの `sources` (検索したものは 1 つの crate の provenance に `status: ok` を足したもの)。欠けがあれば warnings の先頭に「N 個中 M 個を検索した。取得中 X、使えない Y」の 1 行だけを書く (R-SR-5)。outputSchema の provenance の必須は `target` だけにした
- `kind` の絞り込みに `method`・`assoc_const`・`assoc_type` を足した (検索の結果にはメンバーが入るため)
- 実データ: このリポジトリの直接依存は 27 crate で、キャッシュ済みなら初回の約 1.6 秒 (アーカイブを開き直す)、2 回目から 0.03 秒。`sha256 digest` → `sha2::Digest`・`sha2::Sha256`。`crate: "std"` の `Vec push` → `alloc::vec::Vec::push`
- 分かっている限り: `HeaderMap` は直接依存の横断では見つからない (http は直接依存でなく、reqwest は `header` モジュールを丸ごと re-export しているので、索引には `reqwest::header` という名前しか入らない)。直接依存を横断すると完全一致が無いクエリでは部分一致が似た点数で並び、最初のページが上限の 50 件になりやすい (R-SR-4 の打ち切りの初期値は eval で調整する)
- テスト: 2 つの crate を合わせた順位、stdio でプロジェクトが無いと `crate` が要ること

#### C5 (2026-10-20)

`list_impls` を足した (R-TI-2・R-SH-4・R-TI-5 の型の分。§3 Q8)。

- 対象の impl: rustdoc が型に付けた impl の一覧から、`for` がその型のもの (プリミティブ型はすべて) と blanket impl を取る。`get_item` が使う `own_impls` は blanket impl を落とす (`for` が `T` なので) ため使わない。その型が trait の型引数に現れるだけの impl (`impl From<Elapsed> for io::Error`) は除く
- 分類は構造のフィールドだけ (R-TI-2): `is_synthetic` → auto、`blanket_impl` → blanket、trait が無い → inherent、`is_negative` → negative、`#[automatically_derived]` → derive、ほかは trait
- `auto_traits` (R-TI-2): auto trait ごとに `yes`・`no`・`if <条件>` を 1 つにまとめる。auto trait かどうかは、その crate のデータで合成された impl に出てくる trait と、その crate の `is_auto` の trait で決める。合成された impl のほか、明示的な impl (`unsafe impl<T: Sync + Send> Send for Arc<T>`) と negative impl (`Rc` の `!Send`) もここに入れる。条件は impl の型引数の境界と where 句から作り、何も縛らない `?Sized` は除く。不安定な `Freeze`・`UnsafeUnpin` は隠す (名前の一覧で。分類ではなく表示の既定なので R-TI-2 の「名前で推測しない」には当たらない)
- `std_blanket` (R-SH-4): core・alloc・std の汎用 trait (`From`・`Into`・`TryFrom`・`TryInto`・`Borrow`・`BorrowMut`・`Any`・`ToOwned`・`ToString`・`CloneToUninit`) の blanket impl は名前だけを並べる。ほかの blanket impl は `impls` に 1 行ずつ (trait の crate を添える)
- `impls`: 出どころの順 (この crate → 依存 → std)、同じ出どころの中は分類・trait・見出しの順 (R-OUT-4)。各項目は `kind`・`trait` (定義のパス)・`crate` (別の crate のとき)・`header` (where 句込みの impl の見出し)。型には `non_exhaustive` と `has_private_fields` (R-TI-5) を付ける。ページングは `list_module` と同じカーソル
- 型でない item を指したら `invalid_argument` (`NotAType`)。他の crate から re-export された型はたどる (`std::sync::Arc` → `alloc::sync::Arc`)
- 実データ: `tokio::sync::Mutex` は `Send`・`Sync` が `if T: Send`、tracing の `Instrument` などが blanket。`std::sync::Arc` は `Send`・`Sync` が `if T: Sync + Send, A: Allocator + Send + Sync`。`std::rc::Rc` は `Send`・`Sync` が `no`。これで「`Arc<Mutex<T>>` は Send か」に、2 つの条件を組み合わせて答えられる。axum の `WebSocketUpgrade` は blanket impl が 8 個 (`VZip`・`Same`・`ErasedDestructor` など)
- 扱っていないこと: trait 側の境界情報 (`sealed`・`dyn_compatible`・required / provided。R-TI-5) は C7 (`list_implementors`) で扱う。`Arc<Mutex<T>>` のような型の式をそのまま引くことはできない (型ごとに引いて組み合わせる)
- テスト: 条件の作り方 (型引数の境界・where 句・`?Sized`・入れ子の `<>`)、型の impl、モジュールを指したときのエラー、引数

#### C6 (2026-10-20)

`list_methods` を足した (R-TI-1 の M1 の分・R-LS-3・R-SH-4。§3 Q7 のうち固有と trait 由来)。Deref 先のメソッドとプリミティブ型の core・alloc・std にまたがるメソッドは計画どおり M4。

- 対象は `list_impls` と同じ impl (合成と negative の impl はメソッドを持たないので除く)。固有の impl の関数 → `inherent`。trait の impl → `trait`: impl に書かれたメソッドと、上書きされていない既定のメソッド (`provided_trait_methods`。rustdoc は上書きしたものもここに入れるので、impl に書かれた名前は除く)。trait が同じ crate にあれば既定のメソッドもシグネチャ付きで、別の crate なら名前だけ
- 各メソッドに `from`・`trait`・`needs_use` (trait が std の prelude (2021・2024) に無いとき。prelude の trait は名前の一覧で持つ)・`provided` (trait が同じ crate にあるか、上書きされていない既定のメソッドのときだけ分かる) を付ける。パスは `Type::method`
- blanket impl (R-SH-4): std の汎用 trait (`From`・`Into`・`Any` など) のものは `std_blanket` にメソッド名だけ (`to_string`・`into`・`type_id` など)。ほかの blanket impl は、`expand_blanket` が無ければ `blanket` に trait ごとに 1 行 (メソッド名の一覧) で、あれば 1 つずつ `from: blanket` で並べる
- 絞り込み (R-LS-3): `name` (名前の部分一致。大文字小文字を区別しない) と `from` (`inherent`・`trait`・`blanket`) を切る前に適用し、`total` は絞り込んだ後の件数。並びは固有 → trait (trait のパス順) → blanket、その中は名前順 (R-OUT-4)。ページングは `list_module` と同じカーソル
- trait の示し方を直した (`list_impls` も): 同じ crate の trait は、private なモジュールを通る定義パス (`tokio::io::util::async_read_ext::AsyncReadExt`) ではなく `use` できる正準パス (`tokio::io::AsyncReadExt`) で示す。同じ crate の trait で公開パスの無いもの (std 内部の `SpecExtend`・`BufferedWriterSpec` など) の impl は出さない (R-TI-2)。別の crate の trait は公開かどうかを判定できない (`core::mem::SizedTypeProperties` のような隠された trait や、`Eq::assert_fields_are_eq` のような隠されたメソッドが名前で出うる)
- 実データ: `tokio::sync::Mutex` は固有メソッド 10 個 (`lock` など)、`Debug`・`Default`・`From` 由来、tracing の `Instrument`・`WithSubscriber` が blanket。`std::time::Duration` の `name: checked` → `checked_add`・`checked_sub`・`checked_mul`・`checked_div`。`tokio::fs::File` の blanket を展開すると `AsyncReadExt::read` などがシグネチャ付きで 65 個
- テスト: 固有メソッド、`from`・`name` の絞り込み、引数

#### C7 (2026-10-20)

`list_implementors` を足した (R-TI-3 の `scope: crate`・R-TI-5 の trait の分。§3 Q9 のうち crate の中)。`scope: dependency_graph` は計画どおり M4 で、渡すと理由付きの `invalid_argument` にし、型ごとに `list_impls` で確かめるよう案内する。

- 実装者: trait の `implementations` の impl ごとに、型 (`render_type` で描く。`&T`・`[u8]`・`Box<S>` なども)・分類 (`list_impls` と同じ構造の判定)・見出し。同じ crate の型で公開パスを持たないもの (core の `FlattenCompat`・`CaseMappingIter` など) への impl は出さない (R-SH-5)。並びは型・見出しの順。ページングは `list_module` と同じカーソル
- trait の情報 (R-TI-5): `signature`、`items` (メソッド・関連型・関連定数ごとに `provided`。既定の本体・型・値があるか)、`dyn_compatible`、`auto`、`unsafe`、`has_negative_impls`、`sealed`。`sealed` は、supertrait に同じ crate の公開パスの無い trait があるときだけ、その名前を根拠に出す。見つからなくても「封印されていない」とは言わない (ほかの封印の仕方は拾えない。検出の方法を増やすかは R-TI-5 の「M1 で決める」で、今は supertrait だけにした)
- trait でない item を指したら `invalid_argument` (`NotATrait`)。他の crate から re-export された trait はたどる (`tower::Service` → `tower_service::Service`、`std::iter::Iterator` → `core::iter::Iterator`)
- 実データ: `tower::Service` は items がすべて required で dyn compatible、crate の中の実装者は `&'a mut S` と `Box<S>` の 2 つ (tower-service の中だけなので。Q9 の「自分の依存の中で」は M4)。`std::iter::Iterator` は `next` と `Item` が required、ほかは provided で、negative impl (`!Iterator for [T]`) があり、公開の実装者が 115。`std::marker::Send` は `unsafe auto trait` で、`!Send for *const T` などの negative impl を含む 544
- テスト: required / provided、非公開の supertrait による `sealed`、実装者と見出し、`scope` の引数

#### C8 (2026-10-20)

`list_crates` を足した (§6.3。§3 Q1)。プロジェクトが要るので、ローカル機能のある構成でだけ組み込み、無い構成では `tools/list` にも出さない (R-ARCH-4)。

- 範囲: 既定 (`scope: direct`) は workspace member とその直接依存、`scope: all` で依存グラフのすべてのパッケージ。`crate` を渡すと、その名前のパッケージ (版ごと) だけを、`used_by` (member からその crate までの依存の連鎖。`cargo tree -i` に相当。短い順に 5 つまで) 付きで返す。並びは member → 直接依存 → ほか、その中は名前・版の順。ページングは `list_module` と同じカーソル (データのダイジェストはパッケージと版の一覧から)
- 各項目: `crate`・`version` (Cargo.lock の解決された版)・`source` (`crates.io`・`path`・`git`・`registry`)・`member`・`direct`・`build` (評価する target の通常のビルドに入るなら `normal`、dev を含むビルドにだけなら `dev`、どちらにも入らなければ `none`。`cargo tree` の feature の組で判定)・`features` (通常のビルド、無ければ dev を含むビルドの feature)・`docs_rs_json: cached` (キャッシュにあるとき。ネットワークには出ない)。advisory の件数は計画どおり M4
- provenance は 1 つ (`source: project`・`workspace`・`project_from`・`resolved_from`・`target`)。`cargo metadata` が失敗して Cargo.lock を直接読んだとき (`cargo_lock_raw`) は、member と path の依存を見分けられず、ビルドに入るかも分からない旨を warnings に書く
- 依存グラフに、member か・直接依存か・member からの連鎖を返す口を足した (連鎖の探索は幅優先で、循環を避け、調べる部分連鎖を 1 万までに抑える)
- 実データ (このリポジトリ): member と直接依存の 28 crate (`insta`・`tempfile` は `build: dev`)、全体で 269。`crate: windows-sys` は 0.52.0 と 0.61.2 の 2 版で、0.52.0 は reqwest → quinn → quinn-proto → ring などの経路、0.61.2 は etcetera・tempfile・tokio から (どちらも Linux では `build: none`)
- テスト: 引数、stdio で合成の workspace の crate を一覧すること、`tools/list` の一覧 (ローカル機能の有無で `list_crates` が出るか)

#### M1c の終わり (2026-10-20)

C2〜C8 で M1 のツール (`list_crates`・`resolve_version`・`crate_overview`・`get_item`・`search_items`・`list_module`・`list_methods`・`list_impls`・`list_implementors`) が揃った。§3 の問いのうち、Q1 (`list_crates`)・Q2 (`resolve_version`)・Q3 (`crate_overview`)・Q4 (`search_items`)・Q5 (`get_item`)・Q6 (`list_module`)・Q8 (`list_impls`)・Q17 (`get_item` の `stability`)・Q20 (`get_item` の追跡) に答えられる。Q7 は固有と trait 由来だけ (Deref 先とプリミティブは M4)、Q9 は trait の crate の中だけ (依存グラフ全体は M4)。

#### D1 (2026-10-20)

構造化ログと `request_id` を入れた (R-OBS-1・R-OBS-2・R-ERR-2・R-DIST-3)。利用者が決めた (推奨案): ファイルには呼び出しごとの記録に加えて warn 以上のログも書き、呼び出しの中で出たものには `request_id` を付ける。ローテーションと古いログの削除も D1 で入れる。

- ファイル: state のディレクトリの `logs/` (Linux・macOS は `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs/`、Windows は `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs\`)。プロセスごとに `<日付>-<PID>.jsonl`、上限を超えたら `<日付>-<PID>.2.jsonl` と続ける。書いているファイルは std の `File::try_lock` (プロセスをまたぐ勧告ロック) で押さえる
- 古いログの削除: 起動時と次のファイルに移るときに、保持期間を過ぎたものと、合計の上限を超えている間は古い順に消す。ロックを取れないファイル (動いているサーバーが書いている) と、所有マーカー (`.chatora-rustdoc-mcp-logs`) の無いディレクトリのファイルは消さない。マーカーは、ディレクトリを作ったとき (空のとき) にだけ置く。symlink はたどらない
- 設定は環境変数だけ (`CHATORA_RUSTDOC_MCP_LOG__DIR`・`__FILE_MAX_SIZE`・`__RETENTION_DAYS`・`__TOTAL_MAX_SIZE`・`__FILTER`)。設定ファイル (R-DIST-3) を読む仕組みはまだ無い。容量の書き方は整数と単位 (`B`・`KB`・`MB`・`GB` は 1000 の冪、`KiB`・`MiB`・`GiB` は 1024 の冪、大文字小文字は問わない)。誤った値は既定に戻してログに出す
- `log.filter`: 誤った指定が 1 つでもあれば全体を既定 (`info`) に戻し、そのことをログに出す (それまでは誤った指定だけを捨てていた)。stderr だけを絞り、ファイルは絞らない
- 呼び出しの記録 (`kind: call`): `ts`・`request_id`・`tool`・`arguments`・`ms`・`bytes`・`tokens`・`error` (エラーコード。JSON-RPC のエラーは `jsonrpc -32602` の形)・`sources` (provenance の取得元)・`cache`・`fetches`。起動時に `kind: start` の行 (版と PID) を書く
  - 引数: crate・`path`・`project`・`cursor` はそのまま (2,000 文字まで)、ほかの文字列は 200 文字に切る。引数と配列の要素は 32 個まで
  - `bytes`・`tokens`: モデルに渡る形 (`structuredContent` の JSON、エラーは text) の長さ。トークンは 4 文字で 1 と見積もる (R-OUT-3 の 32,000 文字 = 8,000 トークンと同じ初期値。校正は M5)
  - `fetches`: 呼び出しの中で送った要求 (URL は query・fragment・資格情報を除く、最後の状態コード、試行回数、ヘッダまでの時間)。すべての要求が `http::send` を通るので、そこで task-local の記録に書く。呼び出しが生む task には `mcp::tool::spawn` で記録と span を引き継ぐ
  - `cache`: 何もダウンロードしなかった (2xx の応答が無い) なら `hit`、あれば `miss`。sparse index の再検証 (304) はダウンロードに数えない。provenance の無い応答 (引数の誤りなど) には付けない
- `request_id`: プロセスごとの乱数 8 桁 (16 進) と呼び出しの通し番号 (`a318412a-2`)。std の `RandomState` を乱数源に使い、依存を増やしていない。エラーの text の JSON に足し、成功の応答には入れない (同じ入力なら同じ出力 R-ARCH-5 を保つため。D3 の比較はエラーの `request_id` を除く)。呼び出しを `call` の span (`request_id`・`tool`) で包み、blocking のスレッドにも `crate::task::spawn_blocking` で span を引き継ぐ。stderr のログにも `call{request_id=…}` が付く
- 依存: `parking_lot` を足した (std の `Mutex` は clippy で禁じている。R-ERR-1)。tracing-subscriber の `json` 機能は使わず、小さな Layer を書いた (記録の形を自由に決めるため)
- 実データ: tokio の `get_item` は sparse index の 304 だけで `cache: hit`、存在しない crate は index の 404 2 回で `crate_not_found`、依存を横断する `search_items` は新しく入った依存 (parking_lot) の JSON の取得を記録した。誤った `log.filter` (`info,foo=bogus`) は既定に戻り、警告がファイルと stderr の両方に出た
- README に、ログの場所・記録する項目・環境変数を書いた (R-OBS-2)
- テスト: stdio で、エラーの `request_id` がログの行と一致し、成功の応答に `request_id` が無いこと。Layer が呼び出しの中の warn に `request_id` を付け、info を書かないこと。ファイルの切り替えと削除 (保持期間・合計・使用中・マーカー)。`log.filter` の全体の差し戻し、容量の解析、要求の記録が query と資格情報を除くこと
- まだ: 知らない `CHATORA_RUSTDOC_MCP_*` の環境変数をログで知らせること (R-DIST-3) は、設定ファイルを読む仕組みと一緒にする

#### D2 (2026-10-20)

同時実行の上限、キャンセル、同じものの取得と解析の合流を入れた (R-SEC-7・R-WS-6・R-NW-2)。

- 上限: 同時に処理するツール呼び出しはプロセスごとに 16 まで (`limits.concurrent_calls`、環境変数 `CHATORA_RUSTDOC_MCP_LIMITS__CONCURRENT_CALLS`。誤った値は既定に戻してログに出す)。超えた呼び出しは待たせずに `busy` (`retryable: true`、`retry_after_secs: 1`) を返す。M1 にはビルドが無いので `status: building` の場合は無い。ローカルビルドの数の上限は M2
- キャンセル: `notifications/cancelled` で `context.ct` が切れたら、呼び出しの future を落とし、ツールの task を abort する (それまでは rmcp が応答を捨てるだけで、task は最後まで走っていた)。取得中の要求も一緒に止まる。blocking のスレッドの解析は途中で止められず、終わればアーカイブを書く (次の呼び出しで使える)。キャンセルした呼び出しはログに `error: cancelled` で残る
  - `search_items` の依存の横断: 待っている間にキャンセルされたら、各 crate の読み込みの task も abort する。待つ上限を過ぎて応答した後の task は切り離し、裏でキャッシュを満たし続ける (R-WS-6 の「切り離した状態」)
- 合流: キャッシュのパスごとのプロセス内のロック (`Cache::lock`。tokio の `Mutex` を `Weak` で持ち、使われなくなったものは次に作るときに忘れる)。同じものを求める 2 つ目以降の呼び出しは、1 つ目が終わるのを待ってからキャッシュを見るので、取得も解析もしない。取る場所: crate の JSON の取得とアーカイブ (`get_item` 系と依存の横断)、std のアーカイブ (blocking のスレッドで)、nightly の manifest と `rust-docs-json` の展開、`.crate`、sparse index のファイル
  - 1 つ目がキャンセルされると、2 つ目は自分で取り直す (取得を共有して、全員がキャンセルしたときだけ止める R-WS-6 の形にはしていない。M1 の取得は数秒で、取り直しの損は小さい。ビルドを共有する M2 で singleflight にする)
  - ロックは同じ task で入れ子に取らない (取っている間に別の crate へパスを追う前に放す)。プロセスをまたぐロックは M2 で同じ場所に足す (R-CA-4)
- 実データ: キャッシュに無い itoa 1.0.15 への 4 つの同時の呼び出し (`get_item` 2 つ・`search_items`・`list_module`) で、`.crate` と JSON の取得は 1 回ずつ、ほかの 3 つは `cache: hit` で 0.5 秒。docs.rs に JSON の無い itoa 1.0.10 では、1 つ目の 404 を否定キャッシュに残し、ほかは取得しなかった。web-sys の取得中 (1.5 秒) のキャンセルでは JSON はキャッシュに書かれず、4 秒 (取得の後の解析中) のキャンセルでは JSON だけが残った
- 依存: tokio の `sync` 機能を明示した (それまでは他の依存が有効にしていた)
- テスト: stdio で、上限 1 のとき 2 つ目が `busy` になり、1 つ目をキャンセルすると次の呼び出しが通り、ログに `cancelled` と `busy` が残ること (`sleep` を使うので debug ビルドだけ)。`run_tool` を落とすとツールの task が止まること。上限の出入り。同じパスのロックを順に取ること

#### R-NW-4 の見直し (2026-10-20。M1 の終わりに要件へ反映する)

利用者が決めた (推奨案): GitHub API の token を、設定の `github.token` → 環境変数 `GH_TOKEN` → `GITHUB_TOKEN` → gh の設定ファイル (`hosts.yml` の `oauth_token`) の順で探す。

- v0.15 の「`GITHUB_TOKEN` は読まない」の理由 (他のツール向けの token を使わない) は、守りとして弱かった。同じ要件が書くとおり、信頼したリポジトリは hook で環境変数も読める。守るべきは送り先なので、token は公式の `api.github.com` にだけ送り、ミラー (R-CA-8) には送らない
- token が無くても匿名で使う。キーリングを読むのは rate limit に当たったときだけにする (利用者が決めた。2026-10-20。最初は「`gh auth token` は実行しない」としたが改めた): 今の gh は既定で token を OS のキーリングに置き (このホストでも `hosts.yml` に token は無かった)、取り出すときにキーリングの許可のダイアログが出うる (M0 で git の CLI が出した askpass のダイアログと同じ問題)。読む場面を稀な rate limit のときに限れば、gh の利用者は設定なしで token が効き、ダイアログが出うるのもその場面だけになる
  - rate limit に当たったら、`gh auth token` (gh があれば)、次に `git credential fill` で `https://github.com` の資格情報 (gh の `setup-git` のほか、libsecret・Git Credential Manager・macOS のキーチェーンに保存した token も拾える) の順で取り、取れたらその要求をやり直す。子は git の依存の取得と同じく端末と askpass の問い合わせを止め、時間の上限を付ける。取れた token はプロセスのメモリにだけ持ち、ディスクには書かない。`git credential approve`・`reject` は呼ばず、git の保存内容を変えない
  - どちらでも取れなければ、token の渡し方 (`export GH_TOKEN=$(gh auth token)` か設定) を案内する
- git の依存の取得 (子の cargo) は別の話で、サーバーは token を扱わない。利用者の資格情報が使われ、認証で失敗したら利用者の端末で通すよう案内する (R-WS-6・R-WS-7。変更なし)
- GitHub CLI (gh) は必須にしない (利用者が決めた。推奨案。2026-10-20): GitHub API は匿名でも読める公開リポジトリにだけ使い、token は rate limit を緩めるためだけなので、その小さな機能のために導入の条件を gh にしない。`gh api` を毎回子で呼ぶとキーリングの許可のダイアログが出うるうえ、ETag・rate limit の待ち・ミラー (R-CA-8)・オフラインの扱いが gh の出力の解析になる。git の依存は子の cargo が取得し、cargo は libgit2 の経路でも git の CLI の経路でも git の `credential.helper` を使うので、利用者が `gh auth setup-git` を実行していれば gh の認証情報がそのまま効く。そこで、`cargo metadata` が git の認証の失敗で落ちたときの warnings (今は Cargo.lock を直接読む経路だけ) に `cargo fetch` と `gh auth setup-git` の案内を足した。§8 の `fetch_failed` の `auth` の案内と README にも M1 の終わりに足す。GitHub 以外 (GitLab・Bitbucket・自前のサーバー) は cargo と git の設定に任せる (変更なし)

## 7. M1 の終わり方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を次の版にし、§14 に経緯を書く
3. M2 の計画を立てる (着手の前に Q-32 を確かめる)
