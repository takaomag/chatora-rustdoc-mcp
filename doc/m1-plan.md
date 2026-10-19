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

(進めながら書く)

## 7. M1 の終わり方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を次の版にし、§14 に経緯を書く
3. M2 の計画を立てる (着手の前に Q-32 を確かめる)
