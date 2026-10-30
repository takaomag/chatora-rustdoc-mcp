# M3 (crate 情報とソース) の計画

- 作成日: 2026-10-24
- 状態: 計画。§5 は 2026-10-24 に決めた
- 前提: [requirements.md](requirements.md) v0.20。M3 の内容と完了条件は §12、M2 の既知の残りは [m2-plan.md](m2-plan.md) の §7.1 (決定 5・6・8・9)、M2 の結果は m2-plan.md の §6

---

## 1. 目的と範囲

M3 は、crate 単位の情報 (feature・README・changelog・例・メタデータ・crate の検索) とソースを引けるようにし、M2 の既知の残りを直す段階。M2 までのツールは item の文書 (rustdoc JSON) だけを返し、`source_location` で定義位置を示すだけだった。

**範囲** (要件の §12 の M3):

- 新しいツール 7 本: `get_source` (R-SC-1・R-SC-3・R-SC-5)・`inspect_features` (R-FT-1・R-FT-2)・`find_examples`・`readme` (R-SC-4)・`changelog`・`crate_metadata` (§5.9)・`search_crates` (§6.3・R-NW-1)。ツールは 10 本から 17 本になる
- 横断の部品: `.crate` の展開 (R-SC-2・R-NW-3)、ソースルートの判定 (R-SC-4・R-SEC-3)、std のソース (rust-src。R-SC-2・R-STD-6)、crates.io の API (R-NW-1)、ソースの本文の見える印 (R-SEC-7)
- M2 の既知の残り 9 件 (m2-plan.md の §7.1 の決定 5・6・8・9): R-WS-7 との 3 つのずれ、版の違うサーバーの結合テスト (Q-29)、`resolve_version` の M2e の決定 11、`search_items` の std の target、ローカル機能の無い構成の `search_items` の `crate` の required、外の workspace の `lockfile_outdated` の案内、ローカルビルドした member の `source_location.file`
- M2 から回したもの: R-OUT-8 の `path` (m2-plan.md の §7.1 の決定 2)、R-SC-4 の継承した `readme` と拡張子の無い `README`、ソースの本文の見える印 (m2-plan.md の F7 の決定 4)、`.crate` の展開 (R-NW-3)

**範囲の外**: `search_source`・`api_diff`・`unnameable`・`build_external_docs` の `document_hidden_items`・`also_reexported_at` ごとの feature の条件 (M4)、resources・`alwaysLoad` の比較 (M5)、Streamable HTTP (M6)。「M3 以降」とした 4 件 (`search.scope`・`build.prefer_local`・「(in a later version)」で答える場合・Windows の Job Object の隙間) も入れない (§5 の P15)。changelog の GitHub releases と R-NW-4 の token は、M3g の 2 つ目の段に分け、入れるかをその段に入るときに決める (§5 の P9。入れないと決めたら、§5.9 の changelog の「無ければ GitHub releases」を M4 以降に送ると §7 で要件に書く)

**完了条件** (要件の §12): §3 の問い Q10〜Q13・Q15・Q16 と、Q14 のうち changelog で答えられる部分に答えられる。M2 の既知の残りが直っている。eval の新しいケース (§5 の P14) がすべて合格し、それまでのケースが落ちないことで確かめる (Q14 のケースは `.crate` に CHANGELOG を含む crate で作る)

**M3 の利用者への影響**:

- 外へ出る先が増える: crates.io の API (`crate_metadata`・`search_crates`、`.crate` に README が無いときの `readme`)、dist の rust-src (手元に合うものが無いとき)、(G2 を入れれば GitHub の API)。README の「サーバーが接続する先」の約束 (R-SEC-6) を H3 で改める
- キャッシュに、展開した `.crate` のソースと rust-src が増える (GC と `cache clear` の対象に入れる)
- ソースを読むツールは、パッケージの中身だけを返すが、Claude Code の Read の deny をこのサーバー経由ですり抜ける経路になる (R-SC-4・R-SEC-2。README の Security に書く)
- ツールが 17 本になる。instructions (ローカル機能のある構成で 2,013/2,048 文字) と `get_item` の説明文 (2,046/2,048 文字) には案内を足す余地がほとんど無い

## 2. 方針

- **m2-plan.md の §2 を引き継ぐ**: 共通の印 (`cargo +nightly fmt`・`cargo clippy --all-targets`・`cargo test` が local ありとなしの両構成で通る)、層 (R-ARCH-1) と cargo feature `local` (R-ARCH-2)、測った結果と決めたことは日付と環境を付けて §6 に書き、要件への反映は M3 の終わりにまとめる (§7)、判断の要るものは利用者に 1 件ずつ聞く、ホストの環境を変えない、外部への要求に利用者のメールアドレスや名前を入れない、crates.io への公開は利用者が行い、commit と push は利用者に頼まれたときだけ Claude が行う
- **ソースを誰が書くか** (§5 の P2): M2 と同じ。Claude が段ごとに書き、サブエージェントのレビューの後に利用者が差分をレビューする。利用者が自分で書くと言った段だけ、Claude が導く
- **テストは外へ出ない**: crates.io の API・GitHub の API・dist の rust-src は、テストでは偽のサーバーで行う (M1 の `data::test_http` の形)。本物は CI の `testdata` と canary だけ。cargo・rustup・git を動かすテストは、M2 と同じく隔離した環境を渡す
- **ソースルートの判定は 1 か所に置く**: R-SC-4 の「パッケージの中身」とルートの判定 (symlink の実パス、ドットファイル、manifest 由来のパス) を、`get_source`・`readme`・`find_examples`・`changelog`・`source_location.path` が共有する
- **ローカル機能の無い構成** (R-ARCH-4): 新しいツールは、各段でこの構成の形も決めて作る。cargo を起動するものと既定のプロジェクトを読むもの (member・path・git の crate の `get_source`、`inspect_features` の `enabled_in_project`、引数 `project`・`git` など) は出さず、docs.rs・`.crate`・std の配布物・crates.io の API だけで答える。説明文は ⟨⟩⟪⟫ で出し分け、`descriptions_are_of_this_build` と README の検査 (R-QA-5) の「この構成に無いツール」の一覧も直す。§6.4 の共通引数 (`target`・`refresh`・`git` の鍵・`project`) を各ツールがどう受けるかも、その段で決める
- **オフラインと配布物の設定**: `network.offline` (R-CA-7) では crates.io の API・dist の rust-src・(GitHub) に要求せず、キャッシュに無ければ `offline_miss` を返す。`std.fetch_from_dist = false` では dist の rust-src を取らない。どちらも各段のテストに入れる
- **新しいキャッシュの中身** (R-CA-4・R-CA-5): 展開した `.crate` のソース、rust-src、crates.io の API の応答を、`layout` の分類 (docs の上限の側) と GC・`cache clear`・`cache ls` に入れる。展開は一時ディレクトリから名前を替えて置き (原子的に)、ロックの下で行い、同時に展開する複数のプロセスで壊れないようにする。GC は `.crate` を取得の時刻で判じているので、展開したソースを使い続けたときに消さないよう `used` の印を付ける。置き場のパスに形式の版を入れ、版の違うサーバーの GC が互いの置き場を消さないようにする (Q-29)。B1・B3・F1 で決める
- **テスト用の crate** は m2-plan.md の §2 のとおり `tests/fixtures/` の下に置き、その段に要るものだけを足す。`.crate` の展開の罠 (tar のパス・hardlink・Windows の名前) は、テストの中で `.crate` を作って偽の static.crates.io に置く

## 3. 手順

各段の終わりの印には共通の印を含める。段の中の小さな単位 (A1 など) は、m2-plan.md と同じく、着手するときに下調べ・判断・実装・レビューの順で進める。

| 段 | 中身 | 依存 |
| --- | --- | --- |
| **M3a** M2 の既知の残り | A1 小さな食い違い (`search_items` の std の `project` の target、ローカル機能の無い構成の `crate` の required、外の workspace の `lockfile_outdated` の文、`toolchain_too_old` の `rustup update`) / A2 `resolve_version` の M2e の決定 11 (`get_item` と共通の関数に) / A3 R-WS-7 のずれ 1・2 (ロック待ちの時間切れ (§5 の P3)、`cargo metadata` の git の `fetch_failed` を入力が同じ間覚え `refresh` を案内) / A4 版の違うサーバーの結合テスト (§5 の P4) / A5 `source_location` の `file` (パッケージのルートからの相対に直す) と、取得元ごとの span の形の実測 | なし |
| **M3b** ソースの土台 | B1 `.crate` の展開 (§5 の P6。パスの検証・上限・置き場、原子的な展開とロック、GC・`cache clear`・`cache ls` への登録) / B2 ソースルートと R-SC-4 の判定 / B3 rust-src (§5 の P7。手元の sysroot と dist、sha256、`toolchain_missing`、置き場と R-STD-6 の 7 日の片付けとの関係) / B4 ソースの本文の見える印 (R-SEC-7。`data::hidden` の表は制御文字を含むので、改行・タブ・CRLF の CR は残し、ほかを `<U+XXXX>` にする。examples の本文も対象) / B5 `source_location.path` (§5 の P5。B1・B2 のルートの判定を使う) | M3a の A5 |
| **M3c** `get_source` | C1 member と依存 (`manifest_path`)、`build_external_docs` で作った git の crate と外の workspace (記録から) / C2 docs.rs の crate (展開した `.crate`、docs.rs の span の絶対パスの写し替え) / C3 std / C4 前後の行・切り詰め・stale の warning・マクロの span (R-SC-1・R-SC-3・R-SC-5)、取得や展開を待つときの `status: building` | M3b |
| **M3d** `inspect_features` | D1 sparse index の `features`・`features2` (`v`)・`deps` の読み取り (§5.9 のとおり `.crate` が無ければ index から先に返す) / D2 閉包と `enabled_by`、member・path・git の crate は `manifest_path` の Cargo.toml から / D3 feature から item への逆引き (§5 の P12) と `enabled_in_project` | 展開は要らない (`.crate` の Cargo.toml は今も読める) |
| **M3e** `readme` と `find_examples` | E1 readme の決め方 (§5 の P8。`.crate` に無ければ crates.io の README) と見出し単位のページング / E2 examples (`examples/` と doc のコードブロック、`required-features`、並べ方は §5 の P13、本文の見える印) | M3b (E1 の crates.io は F1 の HTTP の部品) |
| **M3f** `crate_metadata` と `search_crates` | F1 crates.io の API のクライアント (§5 の P10。応答のキャッシュの分類と GC) / F2 `crate_metadata` (§5 の P11。index の `pubtime` の読み取り) / F3 `search_crates` (outputSchema の provenance の必須の項目) | M3b と並べられる |
| **M3g** `changelog` | G1 `.crate` の changelog の解析 (版の見出しで分け、`from`・`to`、N リリースと予算) と、どこからも取れないときの crates.io の版の履歴への案内 / G2 GitHub releases と R-NW-4 の token (§5 の P9。入れるかを G2 に入るときに決める) | M3b、G2 は F1 の HTTP の部品 |
| **M3h** 片付け | H1 instructions と説明文 (2,048 文字の内、`for_this_build`) / H2 eval (§5 の P14) / H3 README (ツールの表、状態の行と Planned、Security のソースを読むツールと接続する先、キャッシュ、設定) / H4 annotations と R-QA-5 の検査 (ローカル機能のある構成では `search_crates` だけ `readOnlyHint: true`、無い構成ではすべて `true`。`openWorldHint`・`idempotentHint` も) / H5 canary (crates.io の API と dist の rust-src の形の変化) | すべて |

**危険と、決めるための実測** (下調べ。2026-10-24、読むだけ。下調べの番号を詰め直した):

- R1 `.crate` の展開の安全: tar のパス (`..`・絶対・Windows の `C:` と `\`・大文字小文字だけ違う名前・予約名・末尾の `.`)、hardlink・symlink・デバイス、PAX の長い名前、展開量。B1 で、手元の registry の展開後の大きさの分布を測り、罠の `.crate` をテストで作る
- R2 `$CARGO_HOME/registry/src` の読み方: `.cargo-ok` の形とディレクトリ名の対応を B1 で確かめる
- R3 std のソース: dist の rust-src の大きさと展開の時間を B3 で測る。手元の rust-src (約 79MB・3,721 ファイル) には commit を示すファイルが無いので、同じ sysroot の `rustc -vV` で照合する
- R4 ソースルートと symlink、dep-info の入力、`#[path]`: B2 の単体テストで (Unix の symlink、Windows の junction)
- R5 span の形: A5 で取得元ごとに確かめる (`path` は B5)。外の workspace の member は `words/src/lib.rs` の形 (workspace のルートからの相対) だと手元のキャッシュで確かめた。workspace の外の path 依存・ローカルビルドした crates.io の依存・git の依存は絶対パスになる見込み (cargo の記憶で、確度は中)
- R6 docs.rs の span の絶対パス (`/opt/rustwide/…`) の写し替え: C2 で testdata の tokio の span を分類する
- R7 GitHub releases と token: G2 に入るときに、`.crate` に CHANGELOG が無く GitHub releases にある crate の割合を調べてから決める (手元の registry で CHANGELOG のある crate は 38%)
- R8 crates.io の API: エンドポイントと応答の項目を F1 で文書から確かめる (外に出る実験は利用者の許可の上で)
- R9 feature から item への逆引きの大きさ: D3 で tokio・axum の feature ごとの item の数を testdata の JSON で数える
- R10 継承した readme: E1 で `cargo metadata` の `readme` の形と、`cargo package` が外の readme を中に写すかを fixture で確かめる
- R11 `find_examples` の絶対パスを Claude Code の Read が読めるか: E2 で headless の Claude Code で 1 回確かめる
- R12 ツールが 17 本になる: H2 の eval で選ばれるかを確かめる
- R13 crates.io の 1 秒 1 回と呼び出しの期限 30 秒: F2 で 1 回の `crate_metadata` に要る要求の数を数える

## 4. 要件が「M3 で決める」とした細部

| 論点 | 要件 | 段 |
| --- | --- | --- |
| 継承した `readme` と拡張子の無い `README` | R-SC-4 | E1 (方針は §5 の P8) |
| `crate_metadata` の項目と区分、項目ごとの取得元 | §5.9 | F2 (方針は §5 の P11) |
| `.crate` の展開の置き場とファイル数の上限 (10 万) の実装 | R-SC-2・R-NW-3 | B1 (方針は §5 の P6) |
| ソースの見える印の形 (`<U+202E>`) と warnings の文 | R-SEC-7 | B4 |
| `source_location.path` の範囲 | R-OUT-8 | B5 (方針は §5 の P5) |
| changelog の N の既定 (10) の設定の名前 | §5.9 | G1 |
| `search_crates` の outputSchema の provenance の必須の項目 | R-OUT-9 | F3 |
| ソースの見える印にする文字の範囲 (改行・タブ・CRLF を残す) | R-SEC-7 | B4 |

## 5. 計画の段階で利用者が決めること

2026-10-24 に、下調べ (サブエージェント、読むだけ) の推す案と理由を見て、利用者が 1 件ずつ決めた。

1. P1 段の順: M3a 既知の残り → M3b ソースの土台 → M3c `get_source` → M3d `inspect_features` → M3e `readme`・`find_examples` → M3f `crate_metadata`・`search_crates` → M3g `changelog` → M3h 片付け。要件が既知の残りを最初の段と決めており、`get_source`・`readme`・examples・changelog は `.crate` の展開とソースルートの判定を共有するので先に作る
2. P2 書き手: M2 と同じ (§2)
3. P3 時間切れの「ほとんどがロック待ち」: cargo の stderr の `Blocking waiting for file lock` の状態だった時間が、時間切れまでの半分を超えていれば一時的な失敗とする (計測は今の行の切り替わりで取れ、半分なら「ほとんど」に安全側)
4. P4 版の違うサーバーの結合テスト: debug ビルドのテスト用の口 (環境変数) で記録の形式の版を 1 つ上げた同じバイナリと普通のサーバーを、同じキャッシュで並べる。版の変わったコミットがまだ無いため。版の定数を 1 か所に集める
5. P5 `source_location.path`: その crate のパッケージの中に加え、R-SC-4 で読めるソースルート (依存の `.crate` の展開先など) の下にある実パスに付ける (derive が依存のソースを指す span にも。このリポジトリの JSON では crate 自身の item の span の 3,283/8,626 件)。`get_source` で読めるものとそろえる。`file` はパッケージのルートからの相対に直す (パッケージの外の span は R-OUT-8 のとおり span のままで `url` を付けず warning)。依存のソースを指す span の `path` は、cargo がビルドに使った `$CARGO_HOME/registry/src` や git の checkout の実パスで、このサーバーの展開の有無によらない (R-ARCH-5)。R-OUT-8 は「ローカルビルドした crate」の `path` だけを書くので、§7 で要件を改める
6. P6 `.crate` の中身: 初めて要るときに、キャッシュの `static.crates.io/<鍵>/<版>/src/` に `.crate` 全体を検証しながら展開する (絶対パス・`..`・symlink・hardlink を拒み、大きさと数に上限)。`$CARGO_HOME/registry/src` に展開済みならそれを読み、展開しない (R-SC-2)。以後のツールが同じ置き場を使え、`find_examples` のパスも開ける
7. P7 std のソース: std の JSON と同じ commit の手元の rust-src (同じ sysroot の `rustc -vV` で照合) を使い、無ければ dist の rust-src を既定で取る (R-SC-2 のまま)。dist の大きさは B3 で測り、「既定で取る」を見直す材料にする
8. P8 readme の規則: (a) member と path・git の依存で workspace から継承した readme (パッケージの外の `../README.md`) は、その workspace のルートの下で文書の拡張子なら読む (`.crate` では中に写されるので、読めないと docs.rs の crate と食い違う) (b) 拡張子の無いファイルは名前が `README` (大文字小文字を問わない) のときだけ読む (c) `.mkd` は足さない (手元の 1,625 crate で継承は 0 件、拡張子の無い `README` は 1 件、`README.mkd` は 2 件)
9. P9 changelog の GitHub: M3 では `.crate` の changelog ファイルと crates.io の版の履歴への案内だけを入れる。GitHub releases と R-NW-4 の token (gh・git credential・キーリング、偽の GitHub のテスト) は M3g の 2 つ目の段 (G2) に分け、入れるかを G2 に入るときに改めて決める。完了条件は「Q14 のうち changelog で答えられる部分」
10. P10 crates.io の API: プロセスごとに 1 秒 1 回の待ち行列。応答はキャッシュする (検索は 1 時間、DL 数は 1 日を目安に、細部は F1)。被依存の一覧は crate ごとに複数の要求が要り期限に収まりにくいので M3 では入れず、被依存の数だけ出す
11. P11 `crate_metadata`: 要件の初期案のまま (最新の安定版と最新版、最終リリース日、総 DL と 90 日の DL、被依存数、保守の状況 30・90・365 日、版ごとの feature・MSRV・ライセンスの変化、yanked)。取得元は index (版・yanked・MSRV・features・公開日 `pubtime`・宣言の依存) → `.crate` (その版の license・repository・docs.rs の target) → API (DL 数・被依存数・版ごとのライセンス) の順 (R-NW-1。版ごとのライセンスは全版の `.crate` を取らずに API から)。1 回に要る API の要求の数は F2 で数える
12. P12 `inspect_features` の逆引き: feature ごとに、その feature を条件に含む item の数と上位 N 件をページング付きで返す。条件は正準パスのものだけ (再公開の先の条件は M4)
13. P13 `find_examples` の並べ方: item を指したら、`examples/` のファイルと doc のコードブロックのうち、その item の名前 (最後の区切りと親の型) を識別子として含むものを先に並べる (字句の一致。型の解決はしない)
14. P14 eval: 要件の問い Q10〜Q16 から 7〜9 問を `eval/cases.json` に足し、M3h で全体を 1 回回す (予算 3〜5 USD)
15. P15 「M3 以降」の 4 件 (`search.scope`・`build.prefer_local`・「(in a later version)」で答える場合・Windows の Job Object の隙間): M3 には入れない。それぞれ別の設計 (R-SR-2・R-DS-1) を要し、Job Object の隙間は手元で確かめられないので要件の残りのまま

計画のレビュー (2026-10-26、サブエージェント、読むだけ) の対応 (18 件。どれも書き足しと直しで、新しい判断は無い): `path` を A5 から B5 に移す (ルートの判定と展開の後)、P5 が R-OUT-8 を広げることと展開の有無によらない形、ローカル機能の無い構成・オフライン・新しいキャッシュの中身の方針 (§2)、instructions の長さ (2,013)、P11 の版ごとのライセンスは API から、`search_crates` の outputSchema と見える印の文字の範囲を §4 に、README の crates.io のフォールバックと接続する先、P9 で GitHub を外すときの要件の扱い、`inspect_features` の member・path・git、`get_source` の git の crate と外の workspace と building、`readOnlyHint` の構成ごとの値、完了条件の合格の基準、危険の番号の詰め直し

聞かずに決めたこと (要件が決めている): `search_crates` だけ `readOnlyHint: true` (R-MCP-4)、`.crate` の `cksum` の照合 (R-SC-2)、展開の拒否の規則 (R-SC-2)、キャッシュの symlink の拒否 (R-CA-4)、サニタイズの出口 (m2-plan.md の F7)、`get_source` の切り詰め 2 万文字 (R-SC-5)

## 6. 結果

### 6.1 M3a

**下調べ (2026-10-26。サブエージェント、読むだけ。LSP は使わず grep と Read)**: A1〜A5 の直す場所と大きさ、テスト、判断の候補 10 件。新しく見つかった同じ種類のずれ: std の nightly を選ぶ所 (`project_toolchain`) も既定のプロジェクトだけを見る、照会のツールの `lockfile_outdated` が `cargo update --workspace` を勧める (§8 は `cargo fetch`)、`unsupported_format` の案内が rustup の無い環境でも rustup と書く。形式の版の定数が 5 か所に散り、版を直に書く所が 2 つある (inventory の今の版の判定と GC の `NOT_COLLECTED`)。コミットの順の案: A1+A2 → A3 → A5 → A4 (4a 版を集めて口を足す、4b 結合テスト)

**A1+A2 で利用者が決めたこと**:

1. A1-1 std の nightly を選ぶ所も、引数 `project` のプロジェクトの toolchain を基準にする (target と一緒に直す)
2. A1-2 照会のツールの `lockfile_outdated` の案内を `cargo fetch` に直す (§8 に合わせる)
3. A1-3 `unsupported_format` の案内も、rustup の有無で分ける (無ければ `build.*_path` で nightly を指す案内)
4. A2-1 `resolve_version` の「使わない crate」は、`get_item` と同じリリース単位で判じ、同じ関数を共有する

**A1+A2 の実装 (2026-10-26)**:

- A1(a) と A1-1: `search_items` の std の検索が引数 `project` を持ち、target (`default_target`) と nightly を選ぶ toolchain (`get_item::project_toolchain`) と `refresh` をその project で決める。`project_toolchain` は `project` を受け、既定のプロジェクトの中の workspace のときだけ `rustc -vV` を走らせる (外では何も起動しない。R-MCP-3)。`get_item` の std も同じ
- A1(b): ローカル機能の無い構成では、`search_items` の `required` を `["crate", "query"]` にし、`crate` の無い呼び出しを引数の解析で `invalid_argument` にする (`SearchArguments::Dependencies` はローカル機能のある構成だけ)
- A1(c): 外の workspace の `lockfile_outdated` は、Cargo.lock が無ければ `cargo generate-lockfile`、あれば `cargo fetch` を案内する (blocking のスレッドで有無を見る)
- A1(d)・A1-2・A1-3: `toolchain::rustup_here` を出し、ビルドの失敗の本文 (`failure_body`) と `ChooseError::TooOld` (`rustup` を持たせた) に rustup の有無を渡す。rustup が無ければ `toolchain_too_old` は「cargo が使う toolchain (PATH の rustc か build.rustc_path) を更新する」、`unsupported_format` は「build.rustc_path と build.rustdoc_path で nightly を指す」と案内し、rustup の名前を出さない。JSON を読んで分かった形式違いには nightly の文を付けない。照会のツールの `lockfile_outdated` は `cargo fetch` を案内する
- A2: `get_item` の M2e の決定 11 の 2 つの部分を `project::unused_target` (target が頼まれず、プロジェクトの target が host と違えば host を候補に) と `project::for_host` (プロジェクトの package が今分かるときだけ host にし、プロジェクトの target の warning を捨てる) に出し、`resolve_version` も使う。`resolve_version` は、プロジェクトが版を決めなかったリリースを `get_item::same_release`・`the_release_in_build` でリリース単位に判じる。説明文の target の既定は `TARGET_DEFAULT` に (`PROJECT_TARGET_DEFAULT` は `list_crates` だけ)
- テスト: `search_items` の単体テスト 2 件 (std の検索が `project` を持つ、`required` が構成ごと)、`a_toolchain_too_old_tells_rustup_only_where_it_is`、`failure_body` のテストに rustup なしと `cargo fetch`、stdio の `build_external_docs_tells_how_to_make_the_cargo_lock_of_a_workspace_outside` に古い Cargo.lock、決定 11 の stdio のテスト 2 件に `resolve_version` (直しを外すと落ちることを確かめた)。std の検索の target は、偽の dist が無く std のデータの要る stdio のテストでは確かめられないので、引数の解析の単体テストと、`get_item` の std と同じ `default_target` に任せた
- レビュー (サブエージェント、読むだけ) の対応: `resolve_version` はプロジェクトの package が今分かるとき (`known_now`) だけリリースを照合し、cargo の解決を待たない (target を決めるだけで、期限を過ぎればどのみちプロジェクトの target で答えるため)。`TooOld` が持つのは rustup の有無ではなく nightly に切り替えるか (`switchable`。rustdoc か rustc をパスで指せば rustup があっても切り替えない)、パスで指すときは `rustup --version` を起動しない短絡に戻した。std の失敗の記憶は、`refresh` で外のプロジェクトを指しても消す (std はプロジェクトに依らない。`get_item` も)。README の `lockfile_outdated` も `cargo fetch` に。テストの条件 (構成ごとの `if`) を外し、stdio の `resolve_version` の確かめに `status` が `building` でないことを足した
- 残したもの: stdio の決定 11 のテストは、host が x86_64 Linux でないと `resolve_version` の host の場合を確かめない (`get_item` の側と同じ。テスト用の JSON が x86_64 Linux のもののため)

**A3 で利用者が決めたこと**:

- 決定 5 (A3-1) `cargo metadata` の git の取得の決定的な失敗は、入力が同じ間ずっと、プロセスのメモリだけで覚える (`cargo metadata` の成功と同じ。新しいセッションでは 1 回 cargo を走らせ直す)

**A3 の実装 (2026-10-26)**:

- (1) cargo の stderr の行の切り替わりから、ロックを待っていた時間を数える (`LockClock`。ロック待ちの行から次の行まで)。時間切れは `RunError::TimedOut { limit, waiting_for_lock }` で、待ちが上限の半分を超えれば一時的な失敗 (`mostly_waited_for_lock`、P3)。`retryable: true` で、文に「その大半を別の cargo (rust-analyzer など) のロック待ちで過ごした」を足す。`cargo metadata`・`cargo tree` の時間切れも同じ値を持つ (もとから一時的)
- (2) `cargo metadata` の git の取得の失敗 (`fetch_failed`) は、入力が同じ間ずっとメモリに覚え (`CargoError::lasts`、A3-1)、`refresh` で忘れる (既存の `forget_failures`)。版を Cargo.lock から読んだときの warnings に、`refresh` で呼び直す案内を足した
- テスト: `LockClock` と `mostly_waited_for_lock` の単体テスト、sh でロック待ちの行を出して時間切れにするテスト (unix)、`is_transient` と `failure_body` の時間切れの場合、`cargo metadata` の失敗を覚える `lasts` の単体テスト、`raw_warnings` の `refresh` の案内
- 残したもの: `cargo tree` (feature の組と範囲) の git の失敗は 30 秒のまま (`cargo metadata` が通った後の git の失敗は稀なため)。本物の cargo でロックを握る結合テストは入れていない (ビルドの前の `cargo fetch` が先に同じロックで待ち、そちらはもとから一時的な失敗になるため)
- レビュー (サブエージェント、読むだけ) の対応: member やローカルビルドする依存の照会で `cargo metadata` が git で失敗したときの `metadata_failed` にも `refresh` の案内を足した (足さないと、案内の無いまま記憶が残り続けた)。失敗したときの入力に members の Cargo.toml を足した (前は root の Cargo.toml・Cargo.lock・設定だけで、member の Cargo.toml で git の依存を直しても記憶が残った)。sh のテストは `child::prepare` を呼び (時間切れでグループごと止まり、`sleep` が残らない)、上限を 3 秒にした (遅い CI でもロック待ちの行を読んでから時間切れになる)。`is_transient` の doc を直した。本物の cargo の結合テスト `a_failure_of_cargo_metadata_to_fetch_a_git_dependency_is_kept` (tests/local。無い `file://` のリポジトリを指す git の依存と Cargo.lock を書き、`fetch_failed` が記憶され、member の Cargo.toml の変更と `refresh` で解けることを確かめる。cargo の再試行の待ちで約 34 秒)
- 残したもの: requirements.md の R-WS-7 (時間切れは `refresh` まで固定、ずれ 2) は M3 の終わりの反映で改める

**A5 で利用者が決めたこと**:

- 決定 6 (A5-1) ローカルビルドした crate でも、パッケージの外を指す span の `source_location` には `url` を付けず warning を出す (P5・R-OUT-8 のとおり。B5 で `path` を付けるときに文を見直す)
- 決定 7 (A5-2) `source_location.file` の区切りは、Windows のローカルビルドでも `/` にそろえる (R-OUT-8 の「取得元とキャッシュの状態によらず同じ形」)

**A5 の実装 (2026-10-26)**:

- `Plan.package_root` (文書化する package のルート) と、ビルドの記録の `workspace_root`・`package_root` (`#[serde(default)]`、形式の版は上げない) を足し、`Origin::Local` に `roots` として渡す。`source_location` は、span が相対なら workspace のルートにつなぎ、`.` と `..` を文字の上で解き、パッケージのルートの下ならそこからの相対を `/` 区切りで `file` にする。下でなければ span のまま (区切りは `/`) で `url` を付けず「パッケージの外」の warning (A5-1・A5-2)。ルートの無い前の記録は、今までどおり span の形 (相対で `..` を含まない) で判じ、warning も出さない
- 取得元ごとの span の形の実測: 仮想 workspace の member は workspace のルートからの相対 (`one/src/lib.rs`。tests/local の `the_members_are_built_and_published` で、記録の 2 つのルートも確かめる)。root の package・workspace の外の path 依存 (span は絶対)・ローカルビルドした crates.io の写し・git の依存は、どれも stdio のテストで `file` が `src/…` になる (`in_its_package`)
- テスト: `a_definition_built_here_is_told_relative_to_its_package` (相対・root・絶対・`.` と `..` を通る中、外、ルートの無い前の記録)、`only_a_list_that_shows_the_files_warns_of_one_outside_the_package` (list_module)、stdio の 5 件 (仮想 workspace の member を足した。`in_its_package` は「パッケージの外」の warning が無いことも見る)、tests/local の 1 件
- 134fa5d の CI は config-lint だけ落ちた: m3-plan の決定のリストを 5 から始めた (MD029。一番上の階層の番号付きリストは 1 から)。決定の番号は `- 決定 5 (A3-1)` の箇条書きにした
- レビュー (サブエージェント、読むだけ) の対応: 「パッケージの外」の warning は `item_data` が出すので、位置を捨てる `search_items`・`list_module` (`normal`)・`list_implementors` の応答にも漏れていた (docs.rs の `OUT_DIR` の場合は前から)。warning を `SourceLocation` に持たせ (直列化しない)、位置を捨てる所で `Query::drop_source_location` が warning も取り消す。ローカルビルドの warning の文は「パッケージの外」だけにした (derive とは限らない)。warning はルートのある記録だけで出す。誤った注釈 (ローカルビルドに URL は無い) を消した。stdio の root package の確かめは同じ get_item の重複を除き、既存の応答で見る
- 残したもの: 前の記録にはルートが無く、照会のツールのビルドは鮮度の鍵が変わるまで作り直さないので、手元の前の記録では `file` が workspace のルートからの相対のまま (公開前の版では起きない)。形式の版を上げないので、前の版のサーバーが同じキャッシュでビルドし直すと記録は 2 つのルートの無いものになる (A4 の版の違うサーバーの結合テストで扱う)

**A4 で利用者が決めたこと**:

- 決定 8 (A4-1) テスト用の口 `CHATORA_RUSTDOC_TEST_FORMAT_BUMP` (debug ビルドで `CHATORA_RUSTDOC_TEST_TOOLS=1` のときだけ効く) は、すべての形式の版を一度に 1 つ上げる。どれか 1 つだけ上がる場合の食い違いは、A4a で版を 1 か所に集めて直書きを無くすことで防ぐ
- 決定 9 (A4-2) docs.rs と std のアーカイブのスキーマ (`v8t61` の 8) も上げる対象に入れる。JSON の取得が 1 回で済み、2 つの版のアーカイブが並んで互いを壊さないことをテストで確かめる
- 決定 10 (A4-3) 容量の上限を超えたときに、他の版のサーバーが使っている置き場が消えうること (ロックでは守られず、1 時間以内に使ったものは消さないことだけで守られる) は今のまま。テストでは 1 時間以内は消さないことと、30 日使われなければ消すことを確かめる
- A5 の残りの、同じ形式の版の前のサーバーが記録をルートの無いものに書き直す件は、版を上げる口のテストでは起きない (同じ版どうし)。起きても前の振る舞い (`file` は span の形、warning なし) に戻るだけで、鮮度の鍵が変わって今のサーバーがビルドし直せば直るので、テストは足さない

**A4 の実装 (2026-10-26)**:

- A4a: 形式の版を `data::formats` に集めた (`local`・`ext`・`procs`・`toolchains` の置き場の `v<n>` と、アーカイブのスキーマ)。使う所 (build・external・owner・toolchain・load・在庫・GC) はそこから取る。在庫の今の版の判定 (`"v1"` の直書き) は置き場ごとの今の版に、GC の `NOT_COLLECTED` は今の版の `ext/v<n>` の下の名前にした。テスト用の口の判定 (`CHATORA_RUSTDOC_TEST_TOOLS`) は mcp の層から crate の `testing` (debug ビルドだけ) に移し、data の層と CLI も読めるようにした。`CHATORA_RUSTDOC_TEST_FORMAT_BUMP=1` で、すべての版が 1 つ上がる (A4-1・A4-2)。release ビルドでは常に 0
- A4b: stdio のテスト `servers_of_different_formats_share_a_cache`。テストの持つキャッシュを、普通のサーバーと版を上げたサーバーが借りる (`Server::start_sharing`)。同じ member を同時にビルドして `local/v1` と `local/v2` に記録が 1 つずつでき、もう一度引いても互いの記録のために作り直さない (記録の mtime が変わらない)。docs.rs の JSON は 1 つで、アーカイブは `v8t61` と `v9t61` が並ぶ。`toolchains/v1` と `v2` ができる。`cache ls` は互いの置き場を other format と示し、`cache gc` は 1 時間以内に使った互いの置き場を消さない。31 日前に使ったことにすると、相手の `cache gc` が other format, unused の理由で消す (A4-3)。在庫の判定を `"v1"` の直書きに戻すと落ちることを確かめた
- レビュー (サブエージェント、読むだけ) の対応: 「1 時間以内に使った他の版の置き場は消さない」は容量の上限を超えたときだけ効くので、テストは `cache.target_max_size` を 1 バイトにして `cache gc` を走らせる形にした (`KEPT_FOR` を 0 にすると落ちることを確かめた)。テストのサーバーと CLI は、走らせるシェルに口の変数があっても普通の版で動くよう消してから渡す。アーカイブのスキーマはテストに直書きせず `formats::archive_schema()` から取る。`testing` の関数に `#[must_use]`。`alias`・`git_commits` の名前は在庫と GC で別々に持っていたので `layout::EXTERNAL_INDEXES` にまとめた。doc の直し (external の `local/v1`、release で切れるリンク、`cargo install --debug` の説明)。口の変数を立てて単体テストを走らせると、`v1` を直書きした単体テストは落ちる (口は統合テストのためのもので、そう走らせることはない)
- 確かめていないもの: `build_external_docs` の `ext/v<n>` (使う所は同じ関数から版を取るので、在庫と GC の判定だけを確かめた)、子プロセスの記録の他の版の掃除 (owner の単体テストが `v0` で確かめている)
- 839691a (A5) の CI は Windows の check で落ちた: tests/local の `the_members_are_built_and_published` が JSON の生の span を `one/src/lib.rs` と比べていた (Windows では `one\src\lib.rs`)。製品のコードは `/` にそろえている (stdio のテストは Windows でも通った)。テストを OS の区切りで比べる形に直した

### 6.2 M3b

**下調べ (2026-10-26。サブエージェント、読むだけ。LSP は使わず grep と Read。ネットワークなし)**: B1〜B5 の直す場所と大きさ、判断の候補。手元で測ったこと: registry の 1,625 crate の展開後のファイル数は中央値 22・p99 565・最大 8,868 (aws-sdk-ec2)、大きさは p99 17MB・最大 122.7MB (windows 0.58)・1 ファイルの最大 13.2MB。`.crate` 3,440 個の tar のエントリはすべて通常のファイルで、GNU の長い名前はあり PAX は 0 件、Windows で書けない名前・symlink は 0 件、registry/src のディレクトリの中身は `.crate` の目録と完全に一致した (R1・R2)。手元の nightly の rust-src は 80MB・3,802 ファイルで、キャッシュにある dist の std の commit とは 1 つも一致しない。core の JSON の span の 38,222 件が `library/core/src/../../stdarch/…` の形。docs.rs の span で rustwide の絶対パスは tokio 352 件、axum 1,015 件。コミットの順の案: B4 → B1a (展開の部品) → B1b (口・GC) → B2 → B5 → B3

**M3b で利用者が決めたこと**:

- 決定 11 (B4-1) ソースの本文の見える印は、`hidden::is_hidden` の対象のうち改行・タブ・CRLF の CR 以外をすべて `<U+XXXX>` にする。単独の CR・U+0085・U+2028/2029 も印にする (出口で改行に替えると行番号がずれるため)
- 決定 12 (B1-4) `.crate` の罠のエントリ (`..`・絶対・symlink・hardlink・デバイス・先頭の要素の違い・重複) は `.crate` ごと拒否して理由付きの `docs_unavailable`。その OS で書けない名前 (Windows の予約名・末尾の `.`・大文字小文字だけ違う名前) も `.crate` ごと拒否するが、判定はその OS でだけ行う
- 決定 13 (B1-5) 展開の上限は、合計を既存の `limits.extract_max_size` (既定 4GiB)、ファイル数を 10 万 (設定にしない) とし、1 ファイルの上限は設けない (読むときに `get_source` の上限で切る)
- 決定 14 (B2-2) ローカルビルドの記録に、rustdoc 自身の dep-info のファイルだけを別に持つ (`#[serde(default)]`、形式の版は上げない)。前の記録では作り直すまで dep-info 由来のファイルは読めない
- 決定 15 (B2-3) `$CARGO_HOME/registry/src` を読むときに `.crate` がどこにも無く、オフラインで目録を作れなければ `offline_miss` を返す (ディレクトリの中身では代えない)
- 決定 16 (B5-1) `source_location.path` はローカルビルドのデータにだけ付ける。docs.rs と std には付けない (展開の有無で結果が変わり R-ARCH-5 に反し、`get_item` が展開を待つことになるため)
- 決定 17 (B5-2) ローカルビルドの span が依存のソースを指すときの `path` は、`get_source` と同じ判定 (registry なら `.crate` の目録) を通ったものにだけ付ける。目録が無く今作れないときは付けない

**聞かずに決めたこと (下調べの推す案のまま。実装の細部)**: 展開先は `static.crates.io/<鍵>/<版>/src.v<n>/` に先頭の `<name>-<version>/` を剥がして置き、版は `formats` に足す (B1-1)。展開先は `.crate` と同じ置き場に含め、GC の使用中の判定は展開先のロックも見る (B1-2)。一時ディレクトリは置き場の隣に作って rename し、古い一時ディレクトリも片付ける (B1-3)。ファイルごとに `sync_all` してから完了印、rename (B1-6。R-CA-4)。registry/src は `.cargo-ok` が `v >= 1` のものだけ読み、読めるファイルは `.crate` の目録で決める (B1-7。R-SC-4)。判定は新しい `data::source_root` に 1 つ置く (B2-1)。symlink は実パスで比べる (B2-4)、`#[path]` は特別扱いしない (B2-5)。`path` は実パス (B5-3)。B3 (rust-src) の判断 (照合・置き場・既定で取るか・取れないときのエラー) は、ネットワークでの実測の許可と一緒に B3 に入るときに聞く

**B4 と B1a の実装 (2026-10-26)**:

- B4: `hidden::mark` (決定 11) と警告の文 `marked_warning`。印を付けた本文に出口の `strip` をかけても、消えるのは CRLF の CR だけで行は増えない。使う所は M3c の `get_source` から
- B1a: 新しい `data::crate_source::unpack`。1 回目は raw のエントリで、拡張の見出し (GNU の長い名前・PAX) の大きさ (64KiB まで)・エントリの数・合計の大きさを、メモリに読む前と書く前に確かめる。2 回目は、パスを検証しながら置き場の隣の一時ディレクトリに書き、ファイルごとに `sync_all` してから置き場と入れ替える (前のものは一時の名前に退かしてから消す)。伸ばした量にも天井を付ける。キャッシュの古い一時の片付けは、ディレクトリも消す
- レビュー (サブエージェント、読むだけ) の対応: PAX の `size` は raw の 1 回目と 2 回目でエントリの並びを変え、上限とメモリの検査をすり抜けられたので、`size` か `GNU.sparse` を持つ PAX の拡張は `.crate` ごと拒否し、2 回目でも大きさとファイル数を数える。同じ名前の一時ディレクトリが残っていれば空にしてから使う (同じ pid で再起動したとき)。書いている途中の一時ディレクトリを片付けが消さないよう、256 ファイルごとに中に印を作って消し (時刻が新しくなる)、終わりに一時ディレクトリがあることを確かめる。ファイルの後の同じ名前のディレクトリ・ファイルを通るパス・NUL を拒否の理由で返す (キャッシュの失敗にしない)。大文字小文字を区別しない OS (Windows・macOS) では、大文字小文字だけ違うパスをディレクトリでも拒否する。Windows の予約名に `COM0`・`LPT0`・上付き数字・`CONIN$`・`CONOUT$` を足した。エラーの文に入るエントリのパスは見える印にする (ログと CLI のため)。テスト: PAX の size と path、GNU の長い名前の `..`、ファイルを通るパス、残った一時ディレクトリ、`mark` の U+0085・U+2029・BOM・`\r\r\n`
- 残したもの: macOS の Unicode の正規化 (NFC と NFD) だけが違う名前の衝突は判じない (依存が要る。ファイルなら `create_new` が捕まえる)。gzip の CRC は読まない (tar は終わりの印で止まる。呼ぶ側が index の sha256 を展開の前に確かめる)

**B1b の実装 (2026-10-30)**:

- `crate_source::sources`: crates.io のリリースのソースを返す口。cargo が `$CARGO_HOME/registry/src/index.crates.io-*/<名前>-<版>/` を展開し終えていれば (`.cargo-ok` が JSON で `v >= 1`) そこを返し、無ければ `.crate` (`crate_file` と同じく、キャッシュ・cargo の registry/cache・取得の順) を `static.crates.io/<鍵>/<版>/src.v1/` に展開する。どちらでも、読めるファイルは `.crate` の目録 (`src.v1.json`、検証付き) で決める (cargo の展開にビルドスクリプトが書いたものは入らない)。同じリリースの呼び出しは `src.v1` のパスのロックで 1 つずつ進み、置き場に `used` を付ける。目録だけを作る `list` も足した。版は `formats::sources_dir`
- GC の `held()`: `.crate` の置き場では、`src.v<n>` (どの形式の版でも、目録と一時ディレクトリの名前からも見つける) と、まだ何も無い今の形式の `src.v1` もロックで試し取りする
- レビュー (サブエージェント、読むだけ) の対応: 展開の一時ディレクトリを書いている途中で GC が置き場ごと退かすと、`create_dir_all` が一時ディレクトリを作り直し、前半の欠けたものが完全なものとして置かれえたので、一時ディレクトリの下だけを 1 段ずつ作る (一時ディレクトリが無ければ失敗する) 形にし、GC は今の形式の `src.v1` を何も無いうちからロックする。registry/src は sparse index (`index.crates.io-…`、名前がホストを示す) のものだけを読む (`github.com-…` は GitHub 上の別の registry でもありうる。git index の利用者は自分の展開に落ちるだけ)。cargo の展開の判定はロックを取った後にもやり直す。`.cargo-ok` は symlink を辿らず FIFO で待たずに開く。上限より大きい目録は拒否する。展開済みで目録だけが無いときは展開し直さず目録だけを作る (Windows では開いているファイルのあるディレクトリを入れ替えられないため)。doc の直し。テスト: `{"v":0}`、git index の展開、`.cargo-ok` の symlink、目録と展開の片方だけがあるとき、上限、一時ディレクトリが無いときに作り直さないこと、GC が何も無いうちの `src.v1` のロックで置き場を飛ばすこと
- 残したもの: `sources` は `checksum` が無いと取得せず `NotCached` を返す (オフラインの `offline_miss` は `checksum` を渡したときだけ。M3c の呼ぶ側は index の checksum を渡す)。返したルートのファイルを読む間に GC が置き場を消しうる (読む側 (M3c) が `NotFound` を扱う)。同時の 2 回の呼び出しが 1 回だけ展開することは、ロックの仕組みに頼り、テストでは数えていない
- Windows で不安定だった stdio のテスト `servers_of_different_formats_share_a_cache` (b77409b の CI): 答えの後に別のタスクで dep-info を取るビルドが記録を書き直すのが Windows では遅く、テストが mtime を控えた後に来ていた。両方の記録に `dep_info: true` が付いてから控える形にした (ef852b6、製品のコードは変えていない)

**B2a の実装 (2026-10-30)**: B2 は B2a (土台と crates.io のリリースの規則) と B2b (local の `Package`・`Git` と、記録に rustdoc 自身の dep-info を持つ決定 14) に分けた。std の規則は B3 で足す。

- 新しい `data::source_root`: `resolve(root, asked)` が、読んでよいファイルの実パスか拒否の理由 (`Refusal`: `Outside`・`Dotfile`・`NotInPackage`・`SymlinkOutside`・`SymlinkInCache`・`Missing`・`NotAFile`) を返す。字面で `.` と `..` を解いてルートの下 (ルートの下の絶対パスも受ける) → どの段もドットで始まらない → パッケージのファイル (`Root::Release` は `.crate` の目録) → 実パス (キャッシュの中は symlink を 1 つでも拒否、cargo の展開は辿った先がルートの実パスの下で、同じ規則を通るもの) → 通常のファイル。`lexically` と `slash_separated` は query から移した (動きは同じ)
- レビュー (サブエージェント、読むだけ) の対応: 実パスは feature によらず `fs::canonicalize` で両側を作る (`dunce` は長いパスに `\\?\` を残し、短いルートと比べられなくなるため。clippy の禁止には理由付きの `#[expect]`)。ルートは `std::path::absolute` と字面の解決をしてから比べる (`$CARGO_HOME` が相対や `..` を含むと何も読めなかった)。cargo の展開で symlink を辿った先にも、ドットファイルと目録の判定を同じに掛ける。UTF-8 でない名前は目録に無いものとする (判定は lossy、開くのは元の名前、という食い違いを防ぐ)。`Missing` の文を「無いか見られない」に直した。テスト: Windows の `\` の区切り・UNC・verbatim・ドライブの根、大文字小文字だけ違う名前、`//` と末尾の `/`、`..` を含むルート、UTF-8 でない名前、キャッシュの中のディレクトリの symlink、symlink の先が目録に無い・ドットファイル

**B2b の実装 (2026-10-30)**:

- 決定 14: ローカルビルドの記録に `source_files` を足した (`#[serde(default)]`、形式の版は上げない)。rustdoc 自身の dep-info に載ったファイルだけで、ビルドの target ディレクトリの中のものと build script が見張らせたファイル (`rerun-if-changed`) は入らない。dep-info を取る裏のタスクが記録を書き直すときに入る
- `source_root` に local 機能だけの `Root::Package` (member と path 依存) と `Root::Checkout` (git の checkout、ドットファイル以外のすべて) を足した。`Package` は workspace のルートを基準に照合し、読めるのは、ターゲットのファイル (cargo の `src_path`) のディレクトリの下の `.rs`、rustdoc が読んだファイル、`Cargo.toml`、readme (P8 の規則)、example。この 2 つでは、symlink を辿った先はルートの実パスの下でドットファイルでなければ読む (要件の文言どおり。workspace の中なら他の member のファイルも読める)。呼ぶ側は M3c 以降
- 決定 18 (B2b、レビューの指摘から利用者が決めた): ターゲットのファイルがパッケージの直下にあるとき (`build.rs`、ルートに置いた `lib.rs` など) は、そのファイルだけを読む。直下の下の木は対象にしない (要件の文言どおりでは、workspace のルートのパッケージで他の member・`vendor/`・`target/` の生成物の `.rs` まで読めるため)。lib のモジュールのファイルは rustdoc の dep-info で読める。読めなくなるのは、直下に置いた bin の、dep-info に載らないモジュールくらい
- レビュー (サブエージェント、読むだけ) の対応: dep-info の `..` を含むパスは `freshness` が実パスにして記録するので、workspace が symlink の下にあると (macOS の `/var` など) 字面の照合で外れた。字面で合わなければ、両側の実パスでも比べる。doc の直し (`Package.root` は path・git の依存でもその workspace のルート、`Checkout` はドットファイルを除く)。テスト: workspace のルートのパッケージ (他の member と `target/` を読まない)、symlink の下の workspace、build script が見張らせたファイルが `source_files` に入らないこと (結合テスト)
- 残したもの: 作り直したビルドの記録は、`deps` を前の記録から引き継ぐが `source_files` は空から始まり、dep-info が取れるまで dep-info 由来のファイルは読めない (取れなかったときは次に作り直すまで)。形式の版を上げないので、`source_files` を知らない前の版のサーバーが記録を書き直すと (`refresh`)、`source_files` は落ちて作り直すまで戻らない (決定 14 の範囲)

**B5a の実装 (2026-10-30)**: B5 は B5a (パッケージの中の span) と B5b (依存を指す span、決定 17) に分けた。B5b は `.crate` の目録の得方 (照会の中で取得や展開を待つか) を、入るときに利用者に聞く。

- `source_location` に `path` (開ける実パス) を足した (outputSchema も)。付けるのは、ローカルビルドの member と path 依存の、パッケージの中のファイルで、`source_root::Package` の判定を通ったものだけ (決定 16)。判定のルートは、パッケージが workspace の下にあればそのルート、外にあればパッケージ自身のディレクトリ。読めるのは記録の `source_files` (rustdoc が読んだファイル) で、`get_source` が読むものの中でいちばん狭い。Windows では `\\?\` を外して返す (`dunce::simplified`)。`Package` と各ファイルの判定の結果は照会の中で 1 度だけ作る。crates.io・git などの依存のローカルビルドには付けない (B5b で、それぞれの目録や checkout の規則で付ける)
- レビュー (サブエージェント、読むだけ) の対応: workspace の外の path 依存は、判定のルートが workspace のルートで `Outside` になり `path` が付かなかったので、パッケージ自身のディレクトリにした。同じファイルの判定を照会の中で覚える (一覧の item ごとに実パスを取り直さない)。doc の直し。テスト: workspace の外の path 依存、git の依存には付かないこと、stdio の結合テストで付いた `path` を開いて中身を比べる (Windows の dep-info の遅さに合わせて待ちは 120 秒)
- 受け入れたこと: `path` は、ビルドの直後の答え (dep-info の前) には付かず、後の答えには付く。ファイルが消えたり symlink が変わったりしても変わる。同じデータの同じ問いでも答えが変わりうる (R-ARCH-5 の例外。`stale` と同じく手元の状態による)。`source_files` を持たない前の記録 (cc7a5e5 より前) には、作り直すまで付かない (決定 14 の範囲)。一覧の応答で `source_location` を捨てるもの (`search`・`list_module` の `normal`・`implementors`) でも判定は走る (ファイルごとに 1 度)

**B5b の実装 (2026-10-30)**:

- 決定 19 (B5b、利用者が決めた): 依存を指す span の `path` の照合に要る `.crate` の目録は、キャッシュに無ければ手元の `.crate` (このキャッシュのものか cargo の `registry/cache` のもの) から照会の中で作ってキャッシュに置く。取得はしない (決定 16 の「照会が取得を待たない」に合わせる)。cargo の `.crate` は checksum なしで信じる (cargo が取得のときに確かめ、同じ cargo が展開したものと同じ信頼のため)
- `crate_source::unpacked_at`: cargo が展開し終えた `$CARGO_HOME/registry/src/index.crates.io-…/<名前>-<版>/` の目録 (`src.v1.json`) を、無ければ手元の `.crate` から作る。名前と版はディレクトリ名から (名前として正しく、残りが版になる最初の `-` で分ける)。`source_root::Dependencies`: span の実パスが `$CARGO_HOME/registry/src/…` なら `Root::Release`、`$CARGO_HOME/git/checkouts/<リポジトリ>/<rev>/` で `.cargo-ok` があれば `Root::Checkout` で判定する。照会ごとに 1 つ作り、`Origin::Local` に渡す (`get_item` と `search_across`)。作った目録は照会の中で覚える (mutex は覚えた表を見る間だけ)
- レビュー (サブエージェント、読むだけ) の対応: 目録を作るときのロックは、同じプロセスの中では上限なしに待ち、`get_source` の取得の間も待ちうるので、先に `.crate` が手元にあるかを確かめ、ロックは待たずに試し取りする (取れなければその回は `path` を付けない)。checksum を確かめていない cargo の `.crate` から作った目録は、共有のキャッシュに印 (`unchecked`) を付けて置き、checksum を持つ `sources` はそれを使わずに作り直す (B1 の「目録は確かめた `.crate` から」を `get_source` の側で弱めない)。位置を捨てる一覧 (`search`・`list_module` の `normal`・`implementors`) では実パスの判定そのものを走らせない。cargo の `.crate` は symlink を辿らず上限付きで読む。目録を使ったら置き場に `used` を付ける。`$CARGO_HOME` は起動時に決めたもの (ほかのツールと同じ)。テスト: 印の付いた目録を `sources` が作り直すこと、ロックを他が持てば待たないこと、cargo の `.crate` の symlink
- 残したもの: vendor (`cargo metadata` の `manifest_path` が要る) と、crates.io 以外の registry の依存には付けない。`$CARGO_HOME` は、このサーバーが見るものと cargo が span に書いたものが同じであることに頼る (Windows で綴りの大文字小文字が違えば付かない。安全側)。`index.crates.io-*` が cargo の版で 2 つ並ぶと、`sources` は先に見つけたほうを、`path` は span が指すほうを使う (M3c の `get_source` が `path` を受けるときに合わせる)。目録を作れなかったこと (大きすぎるなど) は照会の中でしか覚えない

**B3 の実測 (2026-10-30、利用者の許可の上で 1 回。`channel-rust-nightly.toml` の 2026-10-30)**: `pkg.rust-src` は target `*` の 1 つで、`xz_url`・`xz_hash` と `git_commit_hash` (完全な commit) を持つ。xz は 6.0MB (gz は 9.6MB)、sha256 は manifest と一致。展開後は 74MB・ファイル 3,819・ディレクトリ 987。手元の `xz` と `tar` で展開 0.8 秒、`sync` 0.25 秒 (`lzma-rust2` とファイルごとの `sync_all` では未測定)。中は `rust-src-nightly/rust-src/lib/rustlib/src/rust/` の下に `library/` と `src/`、その外に 18 個のメタデータ (`git-commit-hash` は完全な commit、`version` など)。エントリは通常のファイルとディレクトリと GNU の長い名前 (`L`、1,201 個) だけで、symlink・hardlink・PAX は無い。ドットファイルは 85 個 (`library/.cargo` など)

**B3 で利用者が決めたこと**:

- 決定 20 (B3-1) 手元の rust-src は、span の完全な commit で照合して使う: std を手元の nightly から取ったときのその sysroot か、`rustc -vV` の commit-hash が span の commit と同じ nightly の sysroot (local 機能だけ。`local_std` が走らせた `rustc -vV` と `--print sysroot` の結果を使い、起動を増やさない)。ほかは dist から取る
- 決定 21 (B3-2) dist から取った rust-src は `std/<短い commit>/src.v1/` (target によらない 1 つ) に置く。今の 7 日の片付け・在庫・GC にそのまま乗る。JSON (target ごと) とは別に 7 日を数える
- 決定 22 (B3-3) 既定で取る。取るかどうかは今の `std.fetch_from_dist` に従い、設定は足さない。`get_source` で std を初めて引いたときだけ取る
- 決定 23 (B3-4) 読めないときのエラーは理由ごとに分ける: オフラインでキャッシュに無ければ `offline_miss`、`std.fetch_from_dist = false` なら `toolchain_missing`、dist の失敗は `network_error`。導入の案内は rustup があれば `rustup toolchain install nightly-<日付> --component rust-src`、無ければ `std.fetch_from_dist` を有効にする案内。どれも別の版のソースは返さない

**B3a の実装 (2026-10-30)**: B3 は B3a (dist から取る rust-src と std の規則) と B3b (手元の nightly の sysroot の照合、決定 20) に分けた。

- `dist::rust_src(cache, dist, nightly)`: JSON の nightly の日付の manifest (`nightly-<日付>.json`、前の版のサーバーが書いた要約に `rust-src` が無ければ取り直す) の `pkg.rust-src` (target `*`) を、ベース URL の下に限って取り、sha256 で照合し、`std/<短い commit>/src.v1/` に展開する (B1 の `crate_source::unpack` を xz にも広げた。アーカイブの最上位 `rust-src-nightly/` を剥がす)。中の `git-commit-hash` が JSON の完全な commit と違えば消して `CommitMismatch`。そのあと完了印 `complete.json` (JSON と同じ `Nightly`) を書くので、今の 7 日の片付け・在庫・GC にそのまま乗る。使ったら `used` を付ける。オフラインは `Offline`、`std.fetch_from_dist = false` は `Disabled`、manifest に無ければ `NoSource` (決定 23 の分け方は M3c の `get_source` で応答の形にする)
- `source_root::Root::RustSrc`: `library/` の下だけを読む (ドットファイルを除く)。キャッシュの中なら symlink を拒み、手元の toolchain なら辿った先も同じ規則。`std_span` は std の JSON の span (`/rustc/<commit>/library/…`) を、その commit のときだけ、字面で `..` を解いて `library/` の下のパスにする
- テスト: 取って照合して展開し、2 回目はオフラインでもキャッシュから。commit の違い・sha256 の不一致・manifest に `rust-src` が無いこと。オフラインと取得を止めた設定。std の規則と span の写し替え (`../../stdarch` を含む)。7 日の片付けで `src.v1` が消え、新しいものは残ること。在庫で `std/<commit>/src.v1` が JSON と別の置き場になること
- レビュー (サブエージェント、読むだけ) の対応: 前の版のサーバーが書いた日付の要約 (`rust-src` を読んでいない) が残っていると、取り直しの失敗 (オフラインなど) が `NoSource` に化けたので、要約に `rust-src` を読んだかの印 (`src_read`) を持たせ、印の無い要約しか無ければ失敗をそのまま返す。印があれば `rust-src` の無い日付でも manifest を取り直さない。`get_item` の対応表で、commit の違いと展開の拒否は再試行不可、展開のディスクの失敗は `internal_error` に分けた (M3c の `get_source` でも同じ分け方を使う)。`git-commit-hash` の無いアーカイブは壊れたアーカイブとして返す。doc の直し。テスト: 前の版の要約とオフライン、印のある要約で取らずに `NoSource`、xz の GNU の長い名前、キャッシュにあれば取得を止めた設定でも返ること
- 残したもの: 展開の途中でプロセスが落ちると `std/<commit>/.src.v1.<pid>.<n>.tmp` (最大 74MB ほど) が残り、同じ commit をもう一度展開するときの古い一時の片付けか GC の容量の上限まで消えない (`.crate` の展開と同じ設計)。同時の 2 回の呼び出しで取得と展開が 1 回になることは、ロックに頼り、テストでは数えていない

**B3b の実装 (2026-10-30)**:

- 決定 20: `local_std::rust_src(neutral_dir, commit)` は、手元の nightly の `rustc -vV` の commit-hash が JSON の完全な commit と同じとき、その sysroot の `lib/rustlib/src/rust` を返す。rust-src そのものの commit も確かめる: rustup が rust-src を入れた印 (`lib/rustlib/manifest-rust-src`) があり、rustup の channel の manifest (`multirust-channel-manifest.toml`) の `[pkg.rust-src]` の版の短い commit が JSON の commit の先頭と合うこと (manifest は 1MB ほどなので、TOML として解かずその節の行だけを読む)。`rustc -vV` と `--print sysroot` は `local_std` と同じコマンドで、同じディレクトリ (キャッシュのもの) なら 1 分は出力が覚えられているので起動は増えない。`std_json::rust_src(cache, dist, neutral_dir, json)` は選ばれた std の JSON から決める: dist の JSON なら、手元に合うものがあればそれ、無ければ `dist::rust_src`。手元の JSON なら、JSON の置き場 (`<sysroot>/share/doc/rust/json`) から sysroot を求め、プロセスを走らせずに同じ確かめ方をする (決定 20 の前半は「その sysroot を使う」だが、`rustup update` などで JSON の後に toolchain が変わりうるので、commit で照合し直す)。使えなければ `NoLocalSource` (理由は `NotRun`・`OtherCommit`・`NoComponent`。`toolchain_missing` で、走らせられず時間切れのときだけ再試行可。dist の日付が分からないので取らない)。`RustSrc.in_cache` で、手元のものは symlink を辿る側 (`Root::RustSrc` の `in_cache: false`) にする
- レビュー (サブエージェント、読むだけ) の対応: 使えない理由を 1 つにまとめていたのを分けた。手元の JSON では sysroot を JSON の置き場から求め、プロセスを走らせない。rust-src そのものの commit を rustup の manifest で確かめる。起動が増えない条件 (1 分以内、同じディレクトリ) を doc に書いた。テスト: commit の照合 (完全な commit だけ)、rust-src を入れた印と manifest の commit、手元の JSON の sysroot (何も走らせない)、手元のソースでは symlink を辿り、キャッシュでは拒むこと、このマシンの nightly で無い commit には何も返さないこと、手元に合うものが無ければ dist の JSON は dist の rust-src に落ちること
- 残したもの: M3c の `get_source` は、`neutral_dir` にキャッシュのディレクトリを渡し (起動を増やさないため)、`NoLocalSource` には `rustup component add rust-src --toolchain nightly` を案内する。dist の JSON で手元に合うものを使う経路は、このマシンの nightly の commit が JSON と合わないので通しでは確かめていない

### 6.3 M3c

**下調べ (2026-10-30。サブエージェント、読むだけ。LSP は使わず grep と Read。ネットワークなし)**: crate の種類ごとの読み方、ツールの形、エラーの対応表、テスト、判断の候補 8 件。手元のキャッシュで測ったこと: docs.rs の JSON で rustwide の絶対パスの span を持つのは依存の blanket impl の写しの `impl` だけ (tokio 356、axum 978)、パスで名指せる item の span はすべて相対、`OUT_DIR` の span は 0 件。span は item の本体を含み、外側の属性と doc コメントを含まない (tokio の `.crate` と照合)。マクロの中の定義は行の途中から始まる (`select!` は 559 行 7 列)。std の span はすべて `/rustc/<commit>/library/…` で、`../../stdarch`・`../../backtrace` も手元の rust-src にある

**M3c で利用者が決めたこと**:

- 決定 24 (D1) 引数は item の `path` か、ファイルの `file` (パッケージのルートからの相対で `source_location.file` と同じ形) と `lines` (省けば先頭から) のどちらか 1 つ。どちらも R-SC-4 の判定を通す。docs.rs と std には `source_location.path` が無いので、span の外の行や別のファイル、切り詰めた続きはこれで読む
- 決定 25 (D2) 本文は行番号を付けない 1 つの文字列 (`source`) に、span の行 (`lines`) と返した行の範囲 (`shown`) を添える
- 決定 26 (D3) 見える印を付けた後の 2 万文字で、行の境目で切る (R-SC-5)。`truncated: true` と、続きを読む呼び出し (`file` と `lines`) を hints に書く。`full` も `_meta` の大きな上限も入れない (R-OUT-3 の既定はページング)
- 決定 27 (D4) 前後の行 (`context`) は既定 0、上限 200。属性と doc コメントが要るときは `context` で読む
- 決定 28 (D5) span の無い item は `docs_unavailable` (`no_span`、再試行不可) で、親の型を引く案内を付ける。span があってマクロの生成物と判じたもの (`#[automatically_derived]`、または本文に item の名前が識別子として無い) は本文を返し、`data.macro` と warning で「マクロの呼び出し位置」と明示する (R-SC-3)
- 決定 29 (D6) `docs_unavailable` の reason に `no_span`・`outside_package` (ルートの外・ドットファイル・パッケージに無い・symlink が外へ)・`source_missing` (ファイルが無い)・`no_source` (dist に rust-src が無い) を足す。§8 の一覧は M3 の終わりに改める
- 決定 30 (D7) 応答に読んだファイルの実パス (`data.path`) を付ける (docs.rs と std でも)。キャッシュのパスは GC で消えうること、cargo の展開かキャッシュかで値が変わること (R-ARCH-5 の例外) は受け入れる
- 決定 31 (D8) vendor と crates.io 以外の registry の依存は M3c では読まず、`docs_unavailable` (`outside_package`) で `source_location.file` とクライアントの Read を案内する

**聞かずに決めたこと (計画・要件・前例のまま)**: 照会は `get_item` の道具立てを使う (`What::Source` を足し、re-export を辿った後の読み先で決める)。annotations は今の `annotations()` のまま (R-MCP-4)、`refresh`・`git`・`project` は既存の仕組みが足し消す。ローカル機能の無い構成は docs.rs と std だけ。`sources` に index の checksum を渡す (無ければ index を引く)。ローカルビルドの span が registry/src を指すときは span のディレクトリ (`Dependencies`)、無理なら名前と版で `sources`。`.crate` と rust-src の取得・展開は Flights に載せ、期限で `status: building` (`building_reason: fetching` で兼ねる)。エラーは決定 12・15・23 と B3a の分け方で、rust-src の案内は rust-src 用にする (`dist_error` の rust-docs-json の案内を使わない)。`hidden::mark` を切り詰めより前に、印の途中で切らない。stale の warning (R-SC-2) に加え、記録の inputs の SHA-256 と読んだファイルを比べられれば「ビルドの後に変わった」warning。GC と競った `NotFound` は 1 度だけ `sources` を呼び直す。B5 の穴 (ローカルビルドした crates.io・git の依存のパッケージの中の span に `path` が無い) は P5 のとおり埋める。偽の dist のテスト用の口 `CHATORA_RUSTDOC_TEST_DIST_URL` を足す。README の Tools の表に行を足し (検査が要る)、Security に R-SC-4 の 1 文を足す

**コミットの順の案**: C1a (ツールの骨組み・本文の作り方・docs.rs の相対の span。偽のサーバーの stdio のテストで確かめられる) → C1b (member・path 依存) → C1c (git・ローカルビルドした crates.io の依存・`build_external_docs` の記録・B5 の穴) → C2 (rustwide の絶対パスの写し替え・building・オフライン) → C3 (std) → C4 (マクロ・stale・続きの案内)。合計 2,700〜3,800 行ほど (見積り)

**M3 の終わり (§7) で要件に戻すもの**: R-SC-2 の `<name>-<ver>` の分け方 (名前として正しく残りが版になる最初の `-` で一通りに決まる) と、依存のソースを `manifest_path` から読むこと (vendor とオフライン)。std の失敗の分け方 (決定 23)。決定 18 の直下のターゲット。cargo の展開の symlink の扱い。`get_source` のオフライン (決定 15)。instructions と `get_item.md` の説明文の余地 (H1 で詰め直す)

**C1a の実装 (2026-10-30)**:

- 新しいツール `get_source` (src/mcp/tool/get_source.rs と説明文 get_source.md)。引数は `crate`・`version`・`target`・`project` (`get_item` と同じ) と、`path` か `file` (と `lines`) のどちらか (決定 24)、`context` (既定 0、上限 200。決定 27)。`get_item` の照会の道具立てをそのまま使う: `What::Source` を足し、item を `get_item` と同じく引いて (re-export を辿る)、辿り終えた所 (`conclude` と std の `finish_std`) で読む。`file` のときは crate のルートを引いて版と provenance を決め、そのファイルを読む
- docs.rs の crate: `crate_source::sources` を Flights (`crate_sources`、鍵 `sources:<名前>@<版>`) に載せ、期限を過ぎれば `status: building` (`fetching`)。読むファイルは `source_root::resolve(Root::Release)` で判定し、実パスを開く。本文は行番号なしの 1 つの文字列で、`lines` (span)・`shown` (返した行)・`path` (実パス。Windows は `\\?\` を外す)・`url` (docs.rs) を添える (決定 25・30)。印 (`hidden::mark`) を付けてから 2 万文字で行の境目で切り、`truncated` と続きを読む `file`+`lines` の呼び出しを hints に書く (決定 26)。UTF-8 でないファイルは U+FFFD で読み warning。span の無い item は `no_span` (決定 28)。読めない理由は `outside_package`・`source_missing`、`sources` の失敗は `offline_miss`・`network_error`・`docs_unavailable` (`no_source`・`too_large`・`corrupt`)・`internal_error` に分ける (決定 29)
- ローカルビルドと std はまだ読まず、`docs_unavailable` で `source_location.path` を案内する (C1b・C3 で埋める)。rustwide の絶対パスの span も `outside_package` (C2)。マクロの判定と stale の warning は C4
- README の Tools の表に行、Security に `get_source` が読む範囲の 1 文を足した。`ONE_CRATE` の応答の形の検査に足した
- テスト: 引数 (`path` か `file`、`lines` は `file` と、`context` の上限)、行の範囲、前後の行と改行、印と UTF-8 でないファイル、2 万文字で行の境目と長い 1 行 (印の途中で切らない)。stdio: span を持たせた `demo::hello` を、`.crate` から展開したソースで前後の行と印つきで返し、`file`+`lines` で別の行を読み、ドットファイル・外・`.crate` に無いファイルを `outside_package` で拒む
- レビュー (サブエージェント、読むだけ) の対応: re-export を辿った item では `get_item` の `show_at_entry` が `data.path` (`get_source` では実パス) を入口のパスで上書きしていたので、`get_source` の応答には専用の処理を当て、`item` を入口に、定義元を `defined_at` にする。待ち時間は照会の期限 (`Request.until`) を使う (新しく取り直すと合計で `request_timeout_secs` を超えうる)。checksum の無い照会では index から取ってから `sources` を呼ぶ (一時的な取得の失敗を `no_source` にしない)。`corrupt` は壊れたアーカイブなら再試行可、`.crate` ごとの拒否なら不可。`.crate` の目録に無く、ディスクにも無いファイルは `source_missing`。空のファイルは空の本文で返し、`file` の `lines` がファイルを越えれば `invalid_argument` (行数を添える)、span がファイルを越えれば warning。1 行目が 2 万文字を超えて行の中で切ったときは warning にし、読めない続きは案内しない。続きの呼び出しは JSON として組み (`target` も入れる)、`file` は判定で正規化したもの (`./` などを除く) を返す。ローカルビルドと std の未対応のエラーに reason (`no_source`) と、std には GitHub の `url` の案内。ローカル機能の無い構成でも Windows の `\\?\` を外す。説明文から C1a では効かない「workspace の crate をここでビルド」の句を外した (C1b で戻す)。テスト: 続きの案内 (JSON・`target`)、長い 1 行、re-export の入口、`no_span` と `sources` の失敗の分け方、`./` の正規化、`source_missing`、範囲外の `lines`

**C1b の実装 (2026-10-30)**: member と path 依存のソースを読む。`get_item` の照会が使ったローカルビルド (`LocalBuilt`) と workspace を `get_source` に渡し、`source_root::Package` を組む: 記録の `workspace_root`・`package_root`・`source_files` と、workspace の `cargo metadata` (照会と同じく覚えられたもの) のそのパッケージのターゲット (`src_path`)・example・readme。そのために `metadata::Package` に `readme`、`Target` に `src_path` を足した (`#[serde(default)]`。記録の metadata もそのまま読める)。判定のルートは、パッケージが workspace の下ならそのルート、外ならパッケージ自身 (B5a と同じ)。`file` はパッケージのルートからの相対 (絶対ならそのまま) で、応答の `file` もパッケージのルートからの相対にする。`cargo metadata` が無ければ rustdoc が読んだファイルだけで判じる。ルートを持たない前の記録は `docs_unavailable` (`no_source`、再試行可) で `refresh` を案内する。git・crates.io などの依存のローカルビルドはまだ読まない (C1c)。説明文に「workspace の crate はここでビルドする」の句を戻した。テスト (stdio): workspace の member の関数を前後の行つきで、member の `Cargo.toml` を `file` で読み、workspace の `Cargo.toml` (`../Cargo.toml`) を `outside_package` で拒む

- C1b のレビュー (サブエージェント、読むだけ) の対応: `build_external_docs` が記録した既定のプロジェクトの外の workspace では、`cargo metadata` を走らせず記録の metadata を使う (照会のツールは外で cargo を動かさない。M2e の E4b-3)。既定のプロジェクトで `cargo metadata` が期限までに終わらなければ `status: building` を返す。metadata のパッケージは id ではなく `manifest_path` の親で探す (cargo の版で id の形が違う)。パッケージの外を指す span (`#[path]` など、workspace のルートからの相対) は workspace のルートに付けて読む。応答の `file` はパッケージのルートからの `..` 付きの相対 (継承した readme は `../README.md`)。ターゲットのファイルの親がパッケージの下でなければ (workspace のルートから共有する build script など) そのファイルだけを読む (決定 18 を外にも広げた。他の member や `target/` を読ませない)。ルートを持たない前の記録の案内を `cache clear` に直し、再試行不可に。説明文に、ローカルビルドで読めるもの (ターゲット・rustdoc が読んだファイル・`Cargo.toml`・readme・example) を書いた。テスト: `#[path]` でパッケージの外のモジュール、継承した readme、example、member の中のドットファイル、共有の build script (単体)
- 残したもの: workspace の外の path 依存は、自分の workspace から継承した readme (`../README.md`) を読めない (判定のルートがパッケージ自身のため)。既定のプロジェクトの外の workspace の記録で cargo が走らないことは、通しのテストでは確かめていない (`recorded_workspace::is_outside` の分岐による)

## 7. M3 の終わり方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を次の版 (v0.21) にし、§14 に経緯を書く。セキュリティの仕様が変われば一覧ページも直して公開する
3. README を合わせる (M3h の H3)。`cargo publish --dry-run --locked` を確かめる。crates.io への公開は利用者が M3 の後にまとめて行う (m2-plan.md の §5 の P9)
4. M4 の計画を立てる
