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

## 7. M1 の終わり方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を次の版にし、§14 に経緯を書く
3. M2 の計画を立てる (着手の前に Q-32 を確かめる)
