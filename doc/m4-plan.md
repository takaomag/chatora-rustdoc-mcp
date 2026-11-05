# M4 (依存グラフと差分) の計画

- 作成日: 2026-11-05
- 状態: 計画。§5 は 2026-11-05 に決めた
- 前提: [requirements.md](requirements.md) v0.21。M4 の内容と完了条件は §12、M3 の結果は [m3-plan.md](m3-plan.md) の §6、M3 の終わりの決定は m3-plan.md の §7 (決定 136〜147)

---

## 1. 目的と範囲

M4 は、1 つの crate の中だけでは答えられない問い (依存グラフ全体の trait の実装者、Deref の先とプリミティブのメソッド) と、版をまたぐ問い (API の差分、依存の脆弱性の勧告、Rust のリリースで何が変わったか、MSRV で使えるか) に答える段階。M3 までのツールは 1 回に 1 つの crate を読み (横断の検索 `search_across` は workspace の member と直接依存と std の仲間まで。1 回の照会が re-export をたどって通る crate は `query::across` の `MAX_CRATES` の 4 まで)、他の crate の型は書いたままの名前で示していた。

**範囲** (要件の §12 の M4):

- 新しいツール 4 本: `api_diff` (§5.12 の R-AD-1〜5)・`audit_dependencies` (§5.13 の R-AU-1〜3)・`rust_release` (§5.14 の R-RR-1・2)・`search_source` (任意の R-SC-6。§5 の P9 で入れると決めた)。ツールは 17 本から 21 本になる
- 依存グラフ全体: `list_implementors` の `scope: dependency_graph` (R-TI-3)、逆引きの索引 (R-TI-4・R-PF-2・Q-5)、シグネチャの他 crate の型の正準パス (R-OUT-2)、`unnameable` (R-PR-2)、モジュールを丸ごと re-export した先の検索の索引 (R-SR-1)
- メソッドの解決: Deref の連鎖とプリミティブのメソッド (R-TI-1・R-PR-7)、`get_item` の `Type::method` のすべての経路、glob で trait がスコープに入るか (R-PR-2)、`#![no_std]` の member での core・alloc のパスと std にしか無い item の印 (R-PR-2)
- std と MSRV: MSRV を踏まえた std の表示 (R-STD-3) と `rust_release` の引用 (R-RR-2)
- 脆弱性の勧告: `list_crates`・`crate_metadata` の advisory の件数と `get_item` の warning (R-AU-3)
- feature の残り: `also_reexported_at` ごとと `reexport_of` の条件、`doc(auto_cfg(hide))` (R-FT-2)、`inspect_features` の再公開の先の条件 (m3-plan.md の P12)
- `build_external_docs` の `document_hidden_items` (R-WS-10・R-AD-3)

**範囲の外** (§5 の P5・P13): changelog の GitHub releases と token (Q-34)、vendor・crates.io でない registry のソース (Q-33)、決定 138 の 4 件 (`search.scope`・`build.prefer_local`・「(in a later version)」で答える場合・Windows の Job Object の隙間)、巨大な JSON の解析 (Q-22)、docs.rs のビルド時の依存の版の差 (R-PR-5)、stable に近い nightly の選択 (R-STD-6)、被依存の一覧 (決定 140)、シグネチャ検索と同梱の散文の検索 (R-SR-1)。どれも M5 以降。resources・`alwaysLoad` の比較 (M5)、Streamable HTTP (M6) も入れない

**完了条件** (要件の §12、§5 の J6): §3 の問い Q7・Q9・Q14 の破壊的変更・Q17〜Q19 と、`search_source` で Q21 に答えられる。確かめ方: eval の新しい 7 問 (§5 の P10) は、ツールの出力でしか得られない答え (記憶で答えられない版・件数・パス) か、呼んだツールの引数 (`from: deref`・`scope: dependency_graph` など) で採点し、すべて合格する。既存のケースは M3 の全件の結果 (36 回のうち 34 回が合格。m3-plan.md の §6.8 の H2) を基準線にし、それより悪くならない。§9.1 の目標値は、測る条件 (fixture・trait・温まったキャッシュか新しいプロセスか・p50 と p95) を A4 で決めて H6 で測り、届かなければそのときに最適化か要件の直しかを利用者に聞く

**M4 の利用者への影響**:

- 外へ出る先が増える: OSV の勧告の一括の zip (§5 の P3)、GitHub の API の contents (RELEASES.md。匿名だけ。§5 の P4)。README の「サーバーが接続する先」の約束 (R-SEC-6) とセキュリティの一覧ページを M4h で改める
- キャッシュに、crate ごとの逆引きの索引 (アーカイブの形式の版が上がる。Q-29)、OSV の勧告、RELEASES.md が増える。GC と `cache clear`・`cache ls` の対象に入れる
- 依存グラフ全体の照会は、多くの crate の取得を待ちうる。呼び出しが待つ上限 (R-ARCH-6 の 30 秒の 3 分の 2、既定 20 秒) を過ぎたら `partial` と読めていない crate を返す。docs.rs に JSON の無い依存はローカルビルドせず、`local_build_required` として読めていない crate に並べる (§5 の J2)
- ツールが 21 本になる。instructions (2,046/2,048 文字) には案内を足す余地が無いので、既存の文を縮める (§5 の P11)

## 2. 方針

- **m3-plan.md の §2 を引き継ぐ**: 共通の印 (`cargo +nightly fmt`・`cargo clippy --all-targets`・`cargo test` が local ありとなしの両構成で通る)、層 (R-ARCH-1) と cargo feature `local` (R-ARCH-2)、測った結果と決めたことは日付と環境を付けて §6 に書き、要件への反映は M4 の終わりにまとめる (§7)、判断の要るものは利用者に AskUserQuestion で 1 件ずつ聞く、ホストの環境を変えない、外部への要求に利用者のメールアドレスや名前を入れない、crates.io への公開は利用者が行い、commit と push は利用者に頼まれたときだけ Claude が行う
- **ソースを誰が書くか** (§5 の P14): M3 と同じ。Claude が段ごとに書き、サブエージェントのレビューの後に利用者が差分をレビューする
- **テストは外へ出ない**: OSV・GitHub の API は、テストでは偽のサーバーで行う (M3 の `CHATORA_RUSTDOC_TEST_*_URL` の形)。本物は CI の `testdata` と canary だけ。依存グラフ全体のテストは、testdata の docs.rs の JSON と fixture の workspace で行う
- **索引は crate ごとに持つ** (§5 の P1): 依存グラフ全体の索引を 1 つに結合せず、crate ごとのアーカイブに逆引きを足し、照会のときに必要な crate だけを読む。Cargo.lock が変わっても作り直さない。アーカイブの形式の版を上げ、版の違うサーバーが同じキャッシュを使えることを M3a の結合テストの形で確かめる (Q-29)
- **ローカル機能の無い構成** (R-ARCH-4): 既定のプロジェクトを読むものはこの構成に無い。ツールごとの形 (各段で細部を決める): `list_implementors` は `scope` の値から `dependency_graph` を外す (inputSchema も)。`audit_dependencies` は出し、引数で渡した `crate@版` の列を照合する (§5 の J4。OSV の zip もそこで取る)。`crate_metadata` の advisory の件数と `get_item` の warning は、手元にある OSV のデータで付く。`list_crates` はもともと無い。`api_diff` は docs.rs の JSON だけで比べ、hidden の項目は持たない (`removed_or_hidden` だけ)。`rust_release` と `search_source` (docs.rs の crate と `.crate` だけ) は出す。MSRV の基準と no_std は toolchain と std の既定のまま。ツールの数は local ありの構成で 21 本、無い構成で 19 本 (`list_crates`・`build_external_docs` が無い)
- **オフラインと古いデータ** (R-CA-7): `network.offline` では OSV・GitHub・docs.rs に要求しない。TTL の切れたデータはオフラインでも取得日時と warning 付きで使い、キャッシュに無ければ `offline_miss`。`dependency_graph` はキャッシュにある crate だけを読み、ほかは読めていない crate に並べる。`api_diff` は古い版の JSON がキャッシュに無ければ `offline_miss`。`search_source` は `.crate` が手元に無ければ M3 の `get_source` と同じく `offline_miss`。`get_item` の advisory の warning は手元にあるときだけで (§5 の P12)、データの取得日時を添え、zip が手元に無いときに「勧告なし」と読まれない書き方にする
- **新しいキャッシュの中身** (R-CA-4・R-CA-5・Q-29): crate ごとの逆引きの索引 (アーカイブの形式の版を A3 と B3 の分を合わせて 1 回だけ上げる。`data::formats` の `ARCHIVE`)、OSV の勧告 (zip と、`get_item` が毎回 zip を解析しないための crate ごとの派生の索引)、RELEASES.md を、版付きの置き場にし、`layout` の分類 (docs の上限の側) と GC・`cache clear`・`cache ls` に入れる。置き方は M3 と同じく一時ファイルから名前を替えて原子的に、ロックの下で行い、使い続けるものには `used` の印を付ける
- **応答の速さを守る**: `get_item` の p50 < 10ms (§9.1) を守るため、他 crate の正準パスはキャッシュにある crate だけで描き (§5 の P7)、advisory は手元のデータだけを見る (§5 の P12)
- **テスト用の crate** は m2-plan.md の §2 のとおり `tests/fixtures/` の下に置き、その段に要るものだけを足す。脆弱な版を固定する eval の fixture (Q18) は、このリポジトリの `cargo deny` に入らない場所に置く

## 3. 手順

各段の終わりの印には共通の印を含める。段の中の小さな単位 (A1 など) は、m3-plan.md と同じく、着手するときに下調べ・判断・実装・レビューの順で進める。

| 段 | 中身 | 依存 |
| --- | --- | --- |
| **M4a** 横断の土台 | A1 依存グラフの crate の集合: `cargo metadata` の resolve (package id 単位。`data::packages::PackageGraph`) から、評価する target の normal の依存 (dev は引数で) のうち、trait を定義した package (id と版で決める。`tower::Service` なら再公開の元の `tower-service`) を推移的に依存に含む package。同じ名前の別の版の trait だけに依存する package は除き、warning と、その版を指す引き方を hints に (§5 の J7)。docs.rs に JSON の無い依存はビルドせず `local_build_required` で読めていない crate に (§5 の J2) / A2 多くの crate を並べて読み、待つ上限を過ぎたら `partial` と読めていない crate (`search_across` の並べて読む部分の一般化) / A3 crate ごとのアーカイブの逆引きの索引 (型ごとの impl (プリミティブとパスの無い型を含む)、trait ごとの実装者 (negative impl を含む)、子から親) と形式の版。feature の item の数が trait の impl を数えない件 (m3-plan.md の §6.4) を、この索引で直せるか検討する (§5 の P13) / A4 Q-5 のベンチマーク (`eval/bench.py`) と R1・R9 の実測、§9.1 の測る条件を決める | なし |
| **M4b** 依存グラフの implementors | B1 `list_implementors` の `scope: dependency_graph` / B2 シグネチャの他 crate の型の正準パス (R-OUT-2。キャッシュにある crate だけで、正準化できなかった型に印を付ける。§5 の J1) と `unnameable` (R-PR-2) / B3 モジュールを丸ごと re-export した先の検索の索引 (R-SR-1。`Indexed.search` を変えるので A3 と同じ版の上げに入れる) | M4a |
| **M4c** メソッドの解決 | C1 Deref の連鎖 (§5 の P6。具体的な `Target` だけ、深さ 4 程度、`from: deref` と `via`、R-LS-3 の `from` の絞り込みの値 `deref`) / C2 プリミティブのメソッドを core・alloc・std で (R-PR-7。A3 の型ごとの impl の索引で、すべての impl の走査を見直す) / C3 `get_item` の `Type::method` のすべての経路と、探した経路をエラーに / C4 glob で trait がスコープに入るか (R-PR-2。公開パス索引) / C5 `#![no_std]` の member のパスと std にしか無い item の印 (§5 の P8・J5。member がすべて no_std のときだけ core・alloc のパスにし、混ざれば std のパスのまま core・alloc にもあることを添える) | M4a の A3 (C2 のため) |
| **M4d** std と Rust のリリース | D1 `rust_release` (§5 の P4。GitHub の API の contents を匿名で、TTL 24h と ETag。版の見出しは M3g の `query::changelog` と `query::markdown` を使い回す。プロジェクトを読まず cargo・rustc を起動しないので、R-MCP-4 のとおりローカル機能のある構成でも `readOnlyHint: true`。基準の版を省いたときに toolchain を調べない) / D2 R-STD-3 の MSRV の基準 (§5 の P8。member の `rust-version` の最小) と R-RR-2 の引用 | なし |
| **M4e** `audit_dependencies` | E1 OSV の取得 (§5 の P3。crates.io の勧告の一括の zip、TTL 24h。zip はメモリの中で自前の中央ディレクトリと flate2 で読み、エントリの名前で書き込み先を作らない。エントリ数と展開後の大きさに 4GiB より小さい専用の上限。summary・details は crates.io の文字列と同じく字数で切る (R-SEC-7)) / E2 照合の範囲は Cargo.lock の crates.io のパッケージすべて (dev・他の target も) で、各件に依存の種類 (normal・dev・build) を添える。crates.io でない registry・path・git は対象外 (§5 の J3。R-AU-2)。semver の範囲の照合 (R-AU-1)、種類と withdrawn / E3 `list_crates`・`crate_metadata` の件数と `get_item` の warning (R-AU-3。手元にあるときだけ、取得日時付き。§5 の P12)。ローカル機能の無い構成は `crate@版` の列を受ける (§5 の J4) | なし |
| **M4f** `api_diff` | F1 項目の作り方と正規化 (R-AD-1。自前。§5 の P2)。他 crate の型はキャッシュの有無によらない定義元のパスで比べ、表示だけ正準パスにする (§5 の J1) / F2 分類器 (R-AD-2。初期案の 13 個から、分からなければ `unknown`) / F3 `removed_or_hidden` と `document_hidden_items` (R-AD-3・R-WS-10) / F4 provenance・feature・docsrs の印 (R-AD-4) と応答の形 (R-AD-5。cosmetic や変化の無い blanket impl を既定で省くか) | M4a・M4b の B2・B3 (re-export の inline と、モジュールを丸ごと re-export した先の列挙) |
| **M4g** feature の残りと `search_source` | G1 R-FT-2 の `also_reexported_at`・`reexport_of` の条件と `doc(auto_cfg(hide))`、`inspect_features` の再公開の先の条件。再エクスポートの先の `item_not_found` で行き先の warning が落ちる件 (m3-plan.md の §7) も検討する (§5 の P13) / G2 `search_source` (R-SC-6。R-SC-4 の目録とソースルートの判定、見える印、正規表現。ファイルの絞り込み・上限・ページング・symlink のループ、正規表現のコンパイルの大きさと走査するバイト数・時間の上限は G2 で決める。README の Security に Read の deny をすり抜ける経路が広がることを書く) | G1 は M4a・M4b、G2 は M3 のソースの土台だけ |
| **M4h** 片付け | H1 instructions と説明文 (2,048 文字の内。§5 の P11) / H2 eval (§5 の P10・J6) / H3 README (ツールの表、状態の行と Planned、Known limitations、Security の接続先) とセキュリティの一覧ページ / H4 annotations と R-QA-5 の検査 (`checks.rs` の read-only の決め打ちを `rust_release` に広げ、決定 134 と同じ罠のテストに `rust_release` を足す) / H5 canary (OSV の zip と RELEASES.md の形) / H6 §9.1 の目標値の測定 (依存グラフの implementors、deref の連鎖を含む `list_methods`、advisory を見る `get_item` の p50) | すべて |

**危険と、決めるための実測** (下調べ。2026-11-05、読むだけ。target/claude-notes/m4-research.md):

- R1 依存グラフ全体の速さ: A4 で、索引ありと冷えた取得 (docs.rs から) の両方を測る (取得は利用者の許可の上で)。§9.1 の「索引あり < 1s」が冷えた取得を含まないことを要件に書く
- R2 docs.rs が古いリリースの JSON をどこまで持つか: F1 に入るときに、Q14 の eval の対象 (axum 0.7 と 0.8 など) の JSON の有無と形式 (shim の範囲の 46 以降か) を確かめる (通信は利用者の許可の上で)。無ければ `build_external_docs` で作る
- R3 OSV の zip の大きさ・件数・ETag: E1 で実測する (利用者の許可の上で)。ホストは osv-vulnerabilities.storage.googleapis.com の見込み (記憶で、確度は中)
- R4 GitHub の API の匿名の rate limit: D1 で、1 日 1 回の取得が 60 回/時に収まることと ETag の 304 が数えられないかを確かめる。contents の raw が別のホストへリダイレクトしないか (R-SEC-6)、RELEASES.md が 1MB を超えて raw の media type が要るか (どちらも記憶で、確度は中)。CI の共有 runner からの匿名の要求は rate limit に当たりやすいので、H5 の canary が不安定にならない形を決める
- R5 no_std の判定: C5 で、member の rustdoc JSON の root の attrs に `#![no_std]` が出るかを確かめる。出なければソースの crate root を読む
- R6 instructions の字数: H1 で、4 本の案内を入れて 2,048 文字に収まるかを、ローカル機能の有無の両構成で数える
- R7 Q18 の fixture: 脆弱な版を固定した lock が、このリポジトリの `cargo deny` と GitHub の Dependabot に拾われない置き方を H2 で確かめる
- R9 依存グラフの crate の多くに docs.rs の JSON が無いこと: A4 で、このリポジトリ (274 package) と eval の fixture の Cargo.lock について、docs.rs に JSON がある割合、アーカイブの合計の大きさ (文書のキャッシュの上限 5GB の中で、同じ照会で読んだアーカイブを GC が追い出さないか)、冷えた取得の時間を測る (取得は利用者の許可の上で)
- R10 多くのアーカイブを mmap したまま持つと、Windows では GC が消せない (R-STD-6 の既知の話): A2 で、照会の後に写像を放すかを決める
- R8 依存の候補 (版と API は未確認): zip の読み取りは flate2 の raw deflate と自前の中央ディレクトリ (推す) か `zip` crate、`search_source` の正規表現は `regex` か `regex-automata` (0.4.18 が既に Cargo.lock にある)。足すときは `cargo add` で、ferritin で API を確かめる

## 4. 要件が「M4 で決める」とした細部

| 論点 | 要件 | 段 |
| --- | --- | --- |
| 依存グラフ全体の範囲 (target・normal・dev) と JSON の無い依存の扱い | R-TI-3 | A1 (方針は §5 の P1) |
| 逆引きの索引の構成とキャッシュ | R-TI-4・Q-5 | A3 (方針は §5 の P1) |
| Deref の連鎖の深さと見せ方 | R-TI-1 | C1 (方針は §5 の P6) |
| どの glob で trait がスコープに入るか | R-PR-2 | C4 |
| `api_diff` の分類器の一覧と節の並び | R-AD-2・R-AD-5 | F2・F4 |
| `doc(auto_cfg(hide))` | R-FT-2 | G1 |
| `search_source` のファイルの絞り込み・上限・ページング・symlink のループ | R-SC-6 | G2 |
| OSV のホストと R-SEC-6 の接続先 | R-NW-2・R-SEC-6 | E1 (方針は §5 の P3) |
| MSRV の基準と no_std の member が混在するとき | R-STD-3・R-PR-2 | D2・C5 (方針は §5 の P8・J5) |
| cosmetic や変化の無い blanket impl を既定で省くか | R-AD-5 | F4 |
| プリミティブの impl の見方 (M1 はすべての impl を見る) | R-PR-7 | C2 |
| §9.1 の M4 の目標値と測る条件 | §9.1 | A4・H6 (方針は §5 の J6) |

## 5. 計画の段階で利用者が決めること

2026-11-05 に、下調べ (サブエージェント、読むだけ。target/claude-notes/m4-research.md) の推す案と理由を見て、利用者が 1 件ずつ決めた。どれも推した案。

1. P1 依存グラフ全体の索引: crate ごとのアーカイブに逆引きの索引を足し、照会のときに trait の crate に依存する crate だけを読む。範囲は評価する target の normal の依存 (dev は引数で)。JSON の無い依存は既存の照会の規則どおり、期限を過ぎたら `partial` と読めていない crate。Cargo.lock が変わっても作り直さず、既存のキャッシュと GC を使う
2. P2 `api_diff`: 自前で作る (R-AD-1・2)。分類器は初期案の 13 個から始め、分からないものは `unknown`。既存の形式の正規化 (shim) をそのまま使う。public-api・cargo-semver-checks・trustfall_rustdoc は使わない
3. P3 OSV: crates.io の勧告の一括の zip を取って手元で照合する (TTL 24h)。依存の一覧を第三者に送らない (OSV の API の querybatch は使わない)。zip は自前の読み取りと flate2 で、罠のエントリを拒む
4. P4 `rust_release`: GitHub の API の contents を token なしで、TTL 24h と ETag で使う。token の探索と rate limit に当たったときのやり直しは入れない (Q-34 を待たない)
5. P5 changelog の GitHub releases (Q-34): M5 以降。匿名では足りず、token の取り方をセキュリティと合わせて設計する必要がある
6. P6 Deref とプリミティブ: `list_methods` の同じ一覧に `from: deref` と `via` (たどった連鎖) を付けて混ぜる。`Target` が具体的な型のときだけたどり、ジェネリックな `T` (`Arc<T>` など) は hints で案内する。深さは 4 程度
7. P7 R-OUT-2 の他 crate の正準パス: キャッシュにある (読める) crate だけで描き、無ければ今の書き方のまま (`get_item` の p50 < 10ms を守る)
8. P8 MSRV の基準: 既定のプロジェクトの member の `rust-version` の最小。無ければ toolchain だけ (今と同じ)。no_std は member の JSON で取れればそれ、取れなければソースを読む (C5 で確かめる)
9. P9 `search_source`: 入れる (M4g の G2)。M3 のパッケージの中身の判定・ソースルート・見える印を使い回し、接続先も増えない。完了条件に Q21 が加わる
10. P10 eval: Q7 (`String` で `str` のメソッド)・Q9 (依存の中の `tower::Service` の実装者)・Q14 (`api_diff`)・Q17 (MSRV)・Q18 (advisory。脆弱な版を固定した fixture)・Q19 (`rust_release`)・Q21 (`search_source`) の 7 問を足す。段ごとにその問いだけを回し、M4h で全件を `--total-usd 6` で 1 回
11. P11 instructions と説明文: 上限 2,048 文字 (決定 127) はそのまま。M4h で既存の文を縮め、呼ぶ順の案内をまとめ直して 4 本を入れる。入らなければ、説明文だけで選ばれるツールを案内から外す
12. P12 `get_item` の advisory の warning: `audit_dependencies` が取って手元にある OSV のデータだけを見る (`get_item` のために取りに行かない)。yanked の近道 (R-VR-6) もそのまま
13. P13 小さな持ち越し (Q-33、決定 138 の 4 件、Q-22、R-PR-5、近い nightly、被依存の一覧、シグネチャ検索と散文の検索): どれも M5 以降。ただし、feature の item の数が trait の impl を数えない件 (m3-plan.md の D 段) と、再エクスポートの先の `item_not_found` で warning が落ちる件 (m3-plan.md の §7) は、M4 の部品で直せるので、M4a・M4g のついでに検討する
14. P14 段の順と書き手: §3 の 8 段の順。M4a の土台に M4b・c・f・g が乗るので先に置き、外へ出る M4d・M4e は独立、最大の `api_diff` は土台が固まった後。書き手は M3 と同じ (§2)

計画のレビュー (2026-11-05、サブエージェント、読むだけ) で判断の要った 7 件 (利用者が 1 件ずつ決めた。どれも推した案):

- J1 P7 で他 crate の型の書き方がキャッシュの状態で変わる: `api_diff` はキャッシュの有無によらない定義元のパスで比べ、表示だけ正準パスにする。`get_item` は正準化できなかった型に印を付ける
- J2 `dependency_graph` で docs.rs に JSON の無い依存: ローカルビルドを起こさず、`local_build_required` として読めていない crate に並べる。既にキャッシュにあれば読む (P1 の「既存の規則どおり」を改める)
- J3 `audit_dependencies` の範囲: Cargo.lock の crates.io のパッケージすべて (dev・他の target も) で、依存の種類を添える。crates.io でない registry・path・git は対象外
- J4 ローカル機能の無い構成の勧告: `audit_dependencies` を出し、`crate@版` の列を受ける (OSV の zip もそこで取る)。`crate_metadata` の件数と `get_item` の warning は手元のデータで付く
- J5 no_std の member と std の member が混ざるとき: member がすべて no_std のときだけ core・alloc のパスにし、混ざれば std のパスのまま core・alloc にもあることを添える
- J6 完了の基準: 既存のケースは M3 の結果 (36 回のうち 34 回) を基準線にし、悪くならないこと。新しい 7 問はツールの出力でしか得られない答えで採点し、すべて合格。§9.1 は測る条件を A4 で決めて測り、届かなければそのときに最適化か要件の直しかを聞く
- J7 同じ名前の別の版の trait: 照会した版の trait を依存に含む package だけを数え、別の版だけに依存するものは除き、warning と、その版を指す引き方を hints に書く

計画のレビューの対応 (判断の要らないもの): §1 の `MAX_CRATES` の説明の誤りと待つ上限 (30 秒の 3 分の 2) の直し、A1 の集合を package id と版で決める形 (`tower-service` の再公開)、R9 (docs.rs に JSON の無い依存の割合と大きさ) と R10 (mmap と Windows の GC) と R4 の追記、P13 の 2 件を A3 と G1 に割り振り、§2 にローカル機能の無い構成のツールごとの形・オフラインと古いデータ・新しいキャッシュの中身を書き足し、M4c の依存を A3 に、M4f の依存に M4b を、G2 の依存を M3 のソースの土台に、A3 の negative impl と C1 の `from` の値 `deref`、OSV の zip と `search_source` のセキュリティの上限、`rust_release` の annotations と罠のテスト、H6 の測る対象、§4 の 3 行、§7 の要件の書き換え

聞かずに決めたこと (要件や M3 の決まりが決めている): 新しいツールの annotations (`readOnlyHint` は cargo を起動しうるかで決める。R-MCP-4)、OSV・GitHub の取得のキャッシュと `offline_miss` (R-CA-7)、展開の拒否の規則 (R-NW-3。M3 の B1a と同じ)、出力のサニタイズ (R-SEC-7)、ソースの本文の見える印 (`search_source`。R-SEC-7)

## 6. 結果

### 6.1 M4a

**下調べ (2026-11-05、サブエージェント、読むだけ。target/claude-notes/m4a-research.md)**: 依存グラフの集合は `PackageGraph` の `dependents` の逆向きの閉包と `FeatureSets::normal` の積で作れる (`Direct` は依存の種類を捨てている)。並べて読む処理は `search_across` の中に閉じている。逆引きの索引は無く、`list_implementors` は自分の crate の `Trait.implementations` だけ、プリミティブは全 impl を走査する。他 crate の trait は `external_crates[crate_id].name` と `paths` のパスで引け、定義した側の `paths` と一致する (手元の 4 例)。索引はアーカイブを 1〜3% 増やす見込み (tokio 約 70KB、core 約 0.6MB)

**判断 (2026-11-05、利用者が 1 件ずつ決めた。どれも推した案)**:

- 決定 1 (M4a) std・core・alloc の trait (`Display`・`Iterator`・`std::error::Error` など) も `dependency_graph` で読む (全 package を上限と `partial` の下で)。auto trait (`Send`・`Sync` など) だけは全型が持つので `invalid_argument` にして `list_impls` を案内する
- 決定 2 (M4a) 既定のプロジェクトの workspace の member は、`search_across` と同じく文書が無ければビルドする (J2 の「ビルドしない」は依存の話)
- 決定 3 (M4a) 辺の絞り込みは、`cargo metadata` の辺の逆向きの閉包と `FeatureSets` (`cargo tree -e normal`、dev の引数なら `normal,dev`) の積に、`Direct` に足す依存の種類 (`dep_kinds`) で build だけ・dev だけの辺を外す。他の OS の cfg の辺は残る (余分に読むだけ)
- 決定 4 (M4a) J2 でキャッシュにある前のローカルビルドは、入力の鍵を計算せずに `stale: true` で読む
- 決定 5 (M4a) 依存グラフの照会で読むアーカイブは、crate ごとに引いて結果を所有の文字列にしてから写像を放し、最後の 32 個を開いたままにする仕組み (`load::keep`) にも入れない (R10)
- 決定 6 (M4a) 逆引きの索引: trait ごとの実装者からは blanket impl の写しと auto trait の合成 impl を除き (写しは数え重ね。元の impl は定義した crate に 1 回)、型ごとの impl には印付きで入れる。negative impl は trait ごとの索引に入れる
- 決定 7 (M4a) アーカイブの形式の版は A3 で 8 から 9 に、B3 で検索索引の形を変えたら 10 に上げる (同じ版で形の違うアーカイブを誤読しない)
- 決定 8 (M4a) §9.1 の「依存グラフの implementors が索引ありで 1 秒未満」は、(W) 同じプロセスの 2 回目からと (F) 新しいプロセスでアーカイブはディスクにある状態に当て、(R) アーカイブの作り直しと (C) docs.rs から冷えて取る状態は参考にする。eval/fixture の `tower::Service` と、このリポジトリの `serde::Serialize`・`futures_core::Stream` で、各 20 回の p50・p95
- 決定 9 (M4a) A4 の通信は、(a) 依存の約 300 crate に docs.rs の JSON があるかを確かめるだけ (約 600 回の要求、転送 ~1MB) と、(b) 測る trait の閉包の JSON を取る (転送は fixture で約 2〜3MB、このリポジトリで約 8〜25MB の見込み) を許可する。Q9 のテストに要る 4 crate (tower-service・axum-core・hyper-util・tower-http) を testdata に足す取得も含む。normal の依存グラフ全体 (c) は取らない。キャッシュは target/ の下に分け、測った後に消す
- 決定 10 (M4a) 1 回の照会で読む crate の数の上限は定数 300 で始め (超えた分は読めていない crate に `too_many`)、設定にするかは M5 の実測で決める

聞かずに決めたこと: 段の中の順は A1 → A3 → A2 → A4 (A2 は A1 の集合と A3 の索引を使う)

**A1 の実装 (2026-11-05)**: データ層。`data::packages` の `Direct` に依存の種類 (`Kinds`: normal と dev。`cargo metadata` の `dep_kinds` から、知らない種類は normal、種類の無い古い cargo と Cargo.lock だけのときは両方) を持たせ、`PackageGraph::dependents_closure` (package を推移的に依存に含む package。normal の辺と、引数のときだけ member の dev の辺を逆にたどり、build script だけの辺は通らない。package 自身が先頭で、近い順) と `through_other_versions` (同じ名前の別の版ごとに、その閉包のうち照会した版の閉包に入らない package。J7) を足した。他の OS の cfg の辺は残り、ツールの層で `FeatureSets` と積を取る (決定 3)。テスト: 2 つの版の trait の crate、build だけ・dev だけ (member と非 member)・両方の版に依存する member、Cargo.lock だけのとき

**A3 の実装 (2026-11-05)**: 新しい `query::reverse` の `ReverseIndex` を `Indexed` に足した: impl を型ごとに (`impls_by_type`。鍵は名前のある型なら `paths` の定義パス、無い型は `type_key` の形 (`str`・`[]`・`[;]`・`()`・`(,)`・`&`・`*`・`fn`・`dyn`・`impl`・`<>`・`_`)、`ImplKind` (Inherent・Trait・Blanket・Synthetic) の印付き)、trait ごとの実装 (`implementations_of`。鍵は trait の定義パス (`paths` の `ItemSummary.path` を `::` でつなぐ。crate 名から始まる)、negative も入れ、blanket の写しと合成 impl は除く。決定 6)、持ち主 (`parent_of`。impl・trait の item、variant、struct・union・variant のフィールド)。id の順に作り、同じ JSON から同じ索引になる。アーカイブの形式の版 `ARCHIVE` を 8 から 9 に (決定 7)。テスト: 自分の trait・他 crate の trait・blanket の写し・合成・negative・プリミティブの impl、名前の無い型の鍵

- A1・A3 のレビュー (サブエージェント、読むだけ。手元の axum・tokio・futures・std・alloc・core の JSON を python で数えて裏を取った) の判断 (利用者が決めた):
  - 決定 11 (M4a) `dependency_graph` は normal の依存だけを数え、dev を含める引数は作らない (決定 3 と計画の P1 の「dev は引数で」を改める)。`cargo rustdoc` の JSON にはテストのコード (cfg(test)) が入らず、dev 依存で足した crate から実装が見つかることはほぼ無い。dev の辺の先の member に依存する別の member まで入る誤りも無くなる
  - 決定 12 (M4a) 形式の版を上げると、使っているディレクトリの中に古い版の `.rkyv` が残り続ける (GC はディレクトリ単位でしか消さない。std は nightly ごとに約 146MB)。GC が使っているディレクトリの中の他の版の `.rkyv` も消すようにし、A5 として M4a に足す
- A1・A3 のレビューの対応: blanket impl の写しは元の impl と item を共有するので持ち主にしない (alloc の `to_owned` が写しを指していた)。proc-macro の crate は閉包に入れても広げない (`quote::ToTokens` などで全 package に広がっていた)。trait ごとの実装に trait の crate の id (`external_crates` の鍵、版を `html_root_url` で見分けるため) を持たせる。`&T` の impl は T の鍵でも引け (`by_ref` の印)、`dyn Trait` は `dyn <最初の trait の定義パス>` の鍵に。他 crate から取り込んだ impl (std の core のプリミティブの impl) は `inlined` の印を付け、trait ごとの実装からは除く。別の版は lib 名で見る (鍵が lib 名から始まるため)。テストを足した (写しが item を共有するとき、proc-macro、参照・`dyn`・取り込みの impl、trait の crate の id、variant とフィールドの持ち主)
- A1・A3 のレビューで残したもの: 型の別名に書いた impl は別名の鍵に入る (`RawFd` の impl は `i32` で引けない)。同じ crate の 2 つの版に直接依存する crate では、trait の鍵がぶつかり、`trait_crate` で見分けるのは A2 で

**A5 の実装 (2026-11-05)**: 決定 12。GC (`gc::collect`) に `remove_other_schemas` を足した: `docs.rs`・`std`・`local` (`ext` に `.rkyv` は無い) の下 (深さ 6 まで、リンクと `src.v*` の展開には降りない、一番上がリンクなら入らない、このサーバーが集める区分だけ) の `.rkyv` のうち、名前の版 (`v<n>t<m>`。`formats::archive_version` にまとめ、load と共有) が今より古く (新しい版は他の版のサーバーのもの)、同じディレクトリに今の版のアーカイブが作り直されていて (置き換え済みの残りだけ。在庫は場所の中の更新時刻の最大を最後に使った時刻にするので、消すとディレクトリが今使われたように見え、版ごと古い置き場 `local/v1` や使われていない場所を延命してしまうため)、書かれてから `KEPT_FOR` を過ぎ、アーカイブとそのディレクトリのロック (docs.rs はディレクトリのロックの下で書く) が取れるものを消す。ロックの下で metadata を取り直し、作り直されたものは残す。理由は新しい `OlderSchema` (「archive of an older version」)。在庫の前に走らせて上限の計画を消した後の大きさで立て (dry run では、消す予定の場所の中のものは数えない)、`until` で打ち切る。写像中のアーカイブに読む側のロックは無く、Unix では他の版のサーバーの写像は残り、消えていればその版が作り直す。Windows では消せないことがあり、そのときは次の GC に回る。テスト: 古い版・新しい版・最近の古い版・今の版・置き換えられていない古い版・版の形でない名前・std の置き場・`src.v1` の中・ロック中・dry run、`schema_of`。版の違うサーバーの結合テスト (`servers_of_different_formats_share_a_cache`) で、置き換え済みでない古い版の置き場を消さないことも確かめた

- A5 のレビュー (サブエージェント、読むだけ) の対応: 上の形のうち、在庫の前に走らせる (上限で場所ごと消す計画が古いアーカイブの大きさで立っていた)、古い版だけを消す (2 つの版のサーバーが互いに消し合っていた)、docs.rs のディレクトリのロック、ロックの下での取り直し、一番上のリンク、区分の絞り込み、`src.v*` に降りない、`until`、dry run の二重の数え、理由を分ける、版の文字列を 1 か所に。残したもの: std の `std/local/<commit>/<host>` (JSON を持たずアーカイブだけ) は、古い版を消した後は自分のものと判じられず 1 週間の片付けから外れうる (`used` の印で判じると、`used` だけを持つ他人のディレクトリまで消すので見送った。そのディレクトリが使われれば今の版が作られる)。 書いた時刻で判じるので、長く動く古い版のサーバーが使い続けるアーカイブも 2 時間ごとに消え、作り直される (使った時刻を記録しない)。最初の公開の前の名前 (`v<n>` だけ) は対象外

**A2 の実装 (2026-11-05)**: `search_across` から、多くの crate を並べて読むかビルドする部分 (`Member`・`Outcome`・`Kind`・`build_sets`・`project_all`・`project_outcome`・`planned_outcome`・`recorded_outcome`・`outcome_of`・`load_release`・`failure_of`) を新しい `mcp::tool::gather` に移した (動きは変えない)。そこに方針 `Builds` (`Allow`: 今の照会のとおりビルドする / `CachedOnly`: 依存はビルドせず、`query::local::rebuilding` が返す前のビルドを `stale` の印付きで読み、無ければ `docs_unavailable` の `local_build_required`。member はビルドする。J2・決定 2・4) と、1 回に読む crate の上限 `MAX_CRATES` 300 (超えた分は `docs_unavailable` の reason `too_many`。決定 10。§8 の reason の一覧には M4 の終わりに足す) を足した。`search_across` は `Allow` を渡す。`dependencies` の検索の範囲の組み立ては `scope_of` に切り出した (100 行の上限)。読み終わった crate を 1 つずつ受け取って写像を放す口 (決定 5) は、それを使う B1 で足す (使う形が見えてから)。`CachedOnly` と `too_many` のテストも B1 の stdio のテストで

- A2 のレビュー (サブエージェント、読むだけ) の対応: `query::local::rebuilding` はプロセスの中の作り直し中の表しか見ず、ディスクにある前のビルド (新しいもの、誰も作り直していない古いもの、起動し直した後のもの) を読めなかった (決定 4 に反する) ので、`query::local::published_before` を足した (作り直し中の表、無ければ `Plan::published_dir` の `build::published`。proc-macro はホストの target。`rebuilding` と `LocalBuilt` を作る部分 `stale_of` を共有)。それを新しい `Outcome::Cached` で返し、`stale_reason` を作り直し中 (`rebuilding`) と分けて `unchecked` にし、`retry_after_secs` も「呼び直せば新しくなる」hint も付けない (このツールは作り直さない)。上限 300 は `CachedOnly` のときだけ効かせ、`search_across` の動きは変えない。`too_many` は読めないもの (`unavailable`) の一覧に入れる。`local_build_required` の本体を 1 つに、`RETRY_AFTER_SECS` を `search_across` に戻し、doc を直した
- A2 のレビューで残したもの: 要件 §7.3 の `stale_reason` に `unchecked`、§8 の reason に `too_many` を、M4 の終わりに足す。`CachedOnly` で、get_item が初回のビルドをしている最中は `local_build_required` になる (少し後には読める)。`CachedOnly` のビルドが起きないこと、先にビルドした依存を読めること、起動し直した後に読めること、`too_many` の境目は B1 の stdio のテストで

**テストが利用者のキャッシュに触れていた件 (2026-11-05)**: A4 で手元のキャッシュ (`~/.cache/chatora-rustdoc-mcp`) を計測に使おうとしたところ、`.rkyv` が 1 つも無かった。std と docs.rs のディレクトリの更新時刻、`locks/` に 1 ミリ秒の間に作られた 19 個のロックから、14:21:40 (A5 の最初の版、「今と違う版のアーカイブは消す」を含む全検査の最中) にサーバーの起動時の GC が利用者のキャッシュで走ったと分かった。原因は `tests/common/schema.rs` の `tools()` (`tools/list` の schema を取る、ignore されない実データのテストからも呼ばれる) が、キャッシュの置き場を渡さずにサーバーを起動していたこと (M1 から)。それまでの GC は上限を超えたときしか消さないので目立たなかった。置き場をテストの一時ディレクトリに渡すように直した。ほかのサーバーの起動 (stdio の 5 か所) は置き場を渡しており、実データのクライアントは環境の置き場を使う作り (ignore されたテスト用。CI は置き場を渡す) で、そのままにした。消えたのはアーカイブだけで、JSON はあり、使うときに通信なしで作り直される

**A4 の計測 (2026-11-05、手元の Linux、12 CPU)**:

- 逆引きの索引の大きさと作る時間 (`examples/measure_load` に、逆引きの索引だけを作ってアーカイブにする行を足した。nightly 708c94ef0 の std と docs.rs の JSON、通信なし): core はアーカイブ 117.2 MB のうち 735 KB (0.63%)・20 ms、std は 20.6 MB のうち 192 KB (0.93%)・4.4 ms、tokio 1.53.3 は 7.6 MB のうち 75 KB (0.98%)・4.3 ms、axum 0.8.9 は 5.0 MB のうち 54 KB (1.08%)・1.0 ms。下調べの見積もり (1〜3%) より小さい
- R9 (a) docs.rs に JSON があるか (決定 9 の許可の上で、docs.rs の `crate/<name>/<version>/json` に HEAD を 1 件ずつ 0.25 秒あけて。User-Agent は一般の名前): このリポジトリと eval の fixture の Cargo.lock の crates.io のパッケージ 283 件のうち 271 件 (96%) にある (このリポジトリ 273 件中 262、fixture 95 件中 94)。無い 12 件は windows-sys 0.52 の系列 10 件 (Windows の target だけ)、core-foundation 0.9.4 (macOS だけ)、matchit 0.8.4 (axum の依存、Linux の normal のビルドに入る)。R9 の「`local_build_required` だらけになる」危険は小さい
- 依存グラフの照会の速さ (決定 8 の W と F) と R1 の (b) 閉包の冷えた取得は、照会のツールができる B1 の後に測る (今は測る物が無い)

### 6.2 M4b

**下調べ (2026-11-05、サブエージェント、読むだけ。target/claude-notes/m4b-research.md)**: B1 は、今の crate の照会で trait を解いて (re-export をたどる) 定義した package と定義パス (`paths` の、`implementations_of` の鍵) を得て、`dependents_closure` と `FeatureSets::normal` の積を `gather` の `CachedOnly` で読み、crate ごとに `implementations_of` を引いて今の `implementors_of` の描き方を使い回す。B2 は、他 crate の型の id から `paths` の定義パスと `external_crates` の版を得て、その crate の既存のアーカイブの `by_definition_path` と `canonical_path` で描く。B3 は、アーカイブを作るときには行き先の中身が無い (reqwest の `paths` の `http::header` の下は 26 件だけ) ので、照会のときに行き先を読んで付け替える

**判断 (2026-11-05、利用者が 1 件ずつ決めた。どれも推した案)**:

- 決定 13 (M4b) `dependency_graph` の `sources` は、読めていない crate (ビルドが要る・多すぎる・JSON が無い・失敗・取得中) と、前のビルドをそのまま読んだ crate (stale) だけを並べ、読んだ数を `crates_read` で返す (全件は 300 crate で約 6 万字になり、応答の上限 32,000 字を超える)
- 決定 14 (M4b) 実装者は、trait を定義した crate の分を先に、次に package 名と版の順、その中で型と見出しの順に並べる
- 決定 15 (M4b) std の trait は、std・alloc・core と通常のビルドの全 package (proc-macro を除く) を読み、member → 直接依存 → 深い順に並べる (上限で切るときに利用者に近い crate を残す)
- 決定 16 (M4b) impl を持たない組み込みの marker (`Sized`・`Unsize`・`Tuple` など) は、auto trait と同じく `invalid_argument` にして `list_impls` を案内する (名前の一覧で見分ける)
- 決定 17 (M4b) J7 で版を判じられない impl (ローカルビルドで版の URL が無く、同じ crate の 2 つの版に依存するなど) は、warning 付きで実装者に入れる
- 決定 18 (M4b) B2 の std の型は、prelude の名前 (`Option`・`Vec`・`String`・`Result`・`Box` など) は短いまま、ほかは std の正準パス (`std::fmt::Formatter`) で描く (`$crate::fmt::Formatter` のような書き方が消える)
- 決定 19 (M4b) B2 は、キャッシュにある既存のアーカイブを開いてプロセスで保持し、新しくは作らない (形式の版を上げない)。新しいプロセスの初回が遅ければ (core 88ms)、小さな写像のファイルに切り替える
- 決定 20 (M4b) J1 の印は `data.not_canonical` の配列 (型・crate・版・`get_item` で引ける定義パス `lookup`) に、warning 1 行と「`get_item` で引けば次から正準パスで描く」の hint
- 決定 21 (M4b) 公開パスの無い型 (`unnameable`) は、定義パスで `get_item` で引けるようにし、`unnameable` の印を付ける (R-PR-2 v0.20 の「依存の名前で書けない戻り値の型は引けない」を解く)
- 決定 22 (M4b) B3 は照会のときに展開する: 検索の範囲の crate のモジュールの re-export の行き先を読み、その当たりを re-export した側のパスに付け替える (アーカイブの形は変えず、版を 10 に上げない)
- 決定 23 (M4b) B3 の行き先の crate が手元に無ければ、範囲の crate と同じく docs.rs から取り、期限を過ぎれば `partial`
- 決定 24 (M4b) glob で re-export した先も B3 に入れ、`get_item` の glob の追従を docs.rs の crate に広げる (std は今のまま。検索で出たパスを `get_item` で引けるように)
- 決定 25 (M4b) B3 で行き先の crate も範囲にあれば、行き先のパスで 1 回だけ出す (範囲に無いときだけ re-export した側のパスで)
- 決定 26 (M4b) `CachedOnly` で前のローカルビルドの JSON はあるがアーカイブが無いとき (形式の版を上げた後や GC の後) は、ビルドはせずに JSON からアーカイブを作り直して読む

- 決定 27 (M4b、B1 のレビューの後) `dependency_graph` で上限 (300) を超えて読まなかった crate (`too_many`) は、`sources` に 1 件ずつ並べず、件数と先頭 20 件の名前 (`name@version`) を warning 1 行にまとめ、`partial` にする (依存グラフが大きいと `too_many` だけで応答の上限 32,000 字を超えうるため)

聞かずに決めたこと: proc-macro の package は `dependency_graph` の範囲から外す (item を外へ出せない)。trait の定義パスと auto trait かどうかは、内部用の照会で、re-export をたどった先の crate の `paths` から得る。`too_many` の境目のテストは、debug ビルドだけの口 (`CHATORA_RUSTDOC_TEST_MAX_CRATES`) で上限を小さくして確かめる。段の中の順は B1 → B2 → B3

**B1 の実装 (2026-11-05)**: `list_implementors` に `scope: "dependency_graph"` を足した (ローカル機能のある構成だけ。新しい `mcp::tool::implementors_across`)。まず今の照会で trait を引き (re-export をたどる)、auto trait と impl を持たない組み込みの marker は `invalid_argument` にする (決定 1・16)。プロジェクトの `PackageGraph` から trait を定義した package を名前と版で探し (無ければ `invalid_argument`。std の trait (決定 1・15) は B1c で)、`dependents_closure` から proc-macro を除き `FeatureSets::normal` で絞った範囲を、`gather::project_each` で読む。crate ごとに読み終えたらすぐ逆引きの索引で実装を引き (`query::implementors::implementors_in`)、アーカイブを手放す (決定 5)。依存は `CachedOnly` で読み (前のビルドが無ければ `local_build_required`)、trait の package だけは自分で引いた crate として `Allow` で読み、上限 (300) の前に先頭に置く。J7: 実装した package が trait の crate (lib 名で見る) のどの版に normal で依存するかで判じ、1 つでなければ trait の crate の `html_root_url` の版を互換 (Cargo の規則) で比べ、別の版の trait の実装を除いてその数を warning に、版の分からないものは warning 付きで入れる (決定 17)。別の版にだけ依存する package (範囲と同じく normal のビルドで proc-macro でないもの、その版の package 自身は除く) は warning とその版で引く hint に。並べ順は決定 14、`sources` と `crates_read` は決定 13、カーソルは読んだ crate のダイジェストに束縛する。`partial` (読めていない crate があるとき)・`status: building` (実装者が 0 件で読み込み中のものがあるとき)・`retry_after_secs`・要約の warning 1 行は `search_items` と同じ規則 (R-SR-5・R-OUT-9。`building_reason` を決める部分を `gather` に移して共有)。outputSchema に実装者の `crate`・`version`、provenance の `crates_read`・`sources` (`search_items` と共有)、`partial` を足した。決定 26: `query::local::published_before` で、前のビルドの JSON があってアーカイブが無ければ JSON から作り直して読む (`load` の `archived_unkept`。開いた crate の LRU に入れない)。テスト: stdio に 2 件 (依存グラフをまたぐ: プロジェクトの実装が出る、ビルドしていない依存は `local_build_required`、`get_item` でビルドした後は `unchecked` で読める、別の版の trait の実装は出ず warning と hint。上限: `CHATORA_RUSTDOC_TEST_MAX_CRATES=1` で trait の crate だけを読み、残りは `too_many` で `partial`)、単体テスト (J7 の版の読み方、stale の source、組み込みの marker、別の版の依存者)

- B1 のレビュー (サブエージェント、読むだけ) の対応: `_definition` を消すときに `shift_remove` にする (`preserve_order` の `remove` は最後のキーを前に移し、`scope: crate` の data でも `implementors` が先頭に来ていた)。`stale_reason` の enum と説明に `unchecked` を足す (A2 から `search_items` も返していたのに無く、outputSchema に合わなかった。`stale_reason_schema` に分けた)。J7 は上の形にする (docs.rs の JSON の `html_root_url` は docs.rs が解決した版で、プロジェクトの版と完全一致で比べると互換の版の実装まで別の版として捨てていた。ローカルビルドは URL が無く、毎回「版が分からない」になっていた)。応答の大きさを `scope: crate` と同じく 4 分の 1 ずつ減らして上限に収める。応答を組み直し、状態 (`status`・`building_reason`・`partial`・`retry_after_secs`) と warnings を provenance の直後に置く。`too_many` は決定 27。trait の package は normal のビルドに無くても範囲に入れる (dev・build の依存だけだと黙って 0 件になっていた)。trait の package は `get_item` と同じ照合 (`explicit_package`。package 名と lib 名、大文字小文字) で探し、trait の crate かどうかは package で比べる。task や crate ごとの処理が答えなく終わった crate を `internal_error` の失敗として数える (A2 で `gather` に移したときに、件数にも `sources` にも入らず消えるようになっていた)。`CachedOnly` で docs.rs から作るアーカイブと、決定 26 で JSON から作り直すアーカイブも、開いた crate の LRU に入れない (`load::Keep` を公開して `docs_rs_archive` と `archive_from_unkept` に通す)。決定 26 の作り直しは `until` までしか待たず、過ぎたら `building` (`indexing`) にする (作り直しは裏で続く)。カーソルのダイジェストに各 crate の元の JSON の SHA-256 を含める (作り直しで中身が変わっても名前と版だけでは同じになっていた)。他の crate の negative impl も `has_negative_impls` に反映する。最初の照会の hints は「他の crate は並べない」の 1 件だけを除いて残す。`add_warnings` は既にある warning を足さない。`build_sets` の warning の文言を範囲に依らないものに (「all direct ones are searched」は検索の文言だった)。説明文に、member はここでビルドすること、`stale` で読んだ crate も `sources` に入ること、`partial`、並び順を足した。テストを足した (キーの順、ページ送り、版の判じ方、版の互換、`too_many` の warning)
- B1 のレビューで残したもの: `gather::unread_source` と `search_across` の `sources` の分岐は重複したまま (`search_across` は target を付けるなど違いがある。B3 で検索を触るときにまとめる)。auto trait・組み込みの marker・プロジェクトに無い package の `invalid_argument` と、実装者が 0 件で読み込み中の `status: building` は stdio のテストが無い (前の 2 つは単体テストで名前の一覧を、残りは `search_items` と同じ組み立てで)。`CHATORA_RUSTDOC_TEST_MAX_CRATES` に 0 を渡すと trait の crate まで `too_many` になる (テスト専用の口)

### 6.3 M4c

### 6.4 M4d

### 6.5 M4e

### 6.6 M4f

### 6.7 M4g

### 6.8 M4h

## 7. M4 の終わり方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を次の版 (v0.22) にし、§14 に経緯を書く。セキュリティの仕様が変われば一覧ページも直して公開する (OSV と GitHub の接続先は必ず変わる)。P4 で GitHub を匿名だけにしたので、R-NW-2 の rate limit の手順 1 (token を取ってやり直す) と R-NW-4・R-SEC-6 の token の取り方・設定 `github.token` は M5 以降に送り、手順 2 は token の案内を除いて残す (R-RR-1 の「rate limit の扱いは R-NW-2」も合わせる)。M5 以降に送ったもの (R-SR-1 の散文の検索とシグネチャ検索、R-STD-6 の近い nightly、R-PR-5 の span の確認、Q-22) の「M4 で検討」「M4 で確かめる」も書き換える
3. README を合わせる (M4h の H3)。`cargo publish --dry-run --locked` を確かめる (CI の package の job)
4. M5 の計画を立てる (M4 で M5 以降に回したもの: §1 の範囲の外)
