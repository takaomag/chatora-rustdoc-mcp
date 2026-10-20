# M2 (workspace) の計画

- 作成日: 2026-10-20
- 状態: 計画。§5 は 2026-10-20 に決めた
- 前提: [requirements.md](requirements.md) v0.19。M2 の内容と完了条件は §12、M2 で決める未決事項は §11.2 の Q-29・Q-31・Q-32。M1 の結果と M2 に送った件は [m1-plan.md](m1-plan.md) の §6、M0 の申し送りは [m0-plan.md](m0-plan.md) の §6.1 (A2 の「M1・M2 への申し送り」) と §7.2

---

## 1. 目的と範囲

M2 は、自分の workspace と、docs.rs で引けないコードの文書を、ローカルビルドで作って引けるようにする段階。M1 は docs.rs と std の配布物だけを使い、path・git・別のレジストリの依存と member には `docs_unavailable` (`local_build_required`) を返していた。

**着手前の確認**: 要件の §12 が M2 の前に確かめるとした Q-32 (worktree と `--add-dir` で、サーバーが受け取るディレクトリ) は、2026-10-20 に測った。結果と計画への含意は §6.1 に書いた。

**範囲** (要件の §12 の M2):

- Q-29 (プロセスをまたぐ仕組み)・Q-31 (ローカルビルドの細部) の細部を決める
- ローカルビルド: toolchain の選び方 (R-WS-8)、内容ダイジェストによる鮮度 (R-WS-4)、永続する専用 target dir (R-WS-2)、プロセスをまたぐロック (R-CA-4)、ビルド中の応答 (R-WS-6・R-OUT-9)、ビルドの失敗のキャッシュ (R-WS-7)
- private item (公開パスの無い item のパスと可視性、検索での扱い。R-PR-2・R-SR-4・R-SH-5・R-WS-3)
- docs.rs に JSON が無い依存のローカルビルド (求める target の JSON が無い場合を含む。R-TG-1・R-DS-1・R-VR-2)。照会のツールがビルドしてよい範囲 (R-MCP-3・R-WS-5)
- `build_external_docs` (R-WS-10)
- CLI の `cache` と GC の最小限 (容量の上限。R-CA-5・R-DIST-2)
- M1 から回したもの: プロジェクトの `build.target` を読むこと (R-TG-1・Q-31)、明示した版とプロジェクトの取得元の照合とそのときの `enabled_in_project` (R-VR-1・R-FT-2)、暗黙の member と `cargo metadata` の `workspace_root` の突き合わせ (R-MCP-3)、`cargo metadata`・`cargo tree` の同時の呼び出しの合流と失敗の保持 (R-VR-2・R-WS-7)、cargo の設定の `include` を `cargo metadata` の鍵に入れること (R-VR-2)、1 つの crate の照会の 30 秒の上限と `status: building`・`building_reason: fetching` (R-ARCH-6・R-WS-6)、子の後始末 (`PR_SET_PDEATHSIG`・Job Object・`waiting_for_lock`。R-WS-6・R-WS-2)、プロセスをまたぐロックと取得の singleflight (R-CA-4・R-NW-2)、利用者の `net.offline` での `offline_miss` (R-WS-7)、設定ファイルと知らない設定の知らせ (R-DIST-3。オフラインモード (R-CA-7) と dist を無効にする設定 (R-STD-1) も)、範囲の外の依存の build.rs の罠のテスト (R-QA-5)、beta の toolchain の canary (R-QA-4)
- 公開前の見直しでレビュアーが挙げた残り: 入れる段は §3.7 で振り分けた

**範囲の外**: feature・README・changelog・例・ソースのツール (M3)、Deref とプリミティブのメソッド・`#![no_std]` の member の core・alloc のパス・依存グラフ全体の implementors・`api_diff` (M4)、eval の比較と校正の仕上げ・プラグインの配布・メモリの上限・GC の既定値の見直し (M5)、Streamable HTTP (M6)。独自の target (target の JSON 仕様・`build-std`) のプロジェクトと OS のサンドボックスでのビルドは v1 の対象外のまま (R-TG-1・R-SEC-2)。

**完了条件** (要件の §12): 自分の workspace を引ける。編集後に正しく再ビルドされる。外のプロジェクトと git の crate を引ける。

**M2 の利用者への影響**:

- macOS・Windows では、docs.rs に求める target の JSON が無い依存 (2026-05-01 以降の新しいリリースの多く。R-TG-1) を、既定のプロジェクトの依存ならローカルビルドで引けるようになる。M1 の版が「実質的に Linux 向け」だった制限が外れる。代わりに、初めて引く依存はビルドを待つ (`build.wait_secs` を超えたら `status: building` を返す)
- 照会のツールが、既定のプロジェクトのコード (build.rs と proc-macro) を確認なしにビルドするようになる (R-SEC-2)。README の Security の節と許可ルールの表 (M1 の E4 でビルドを入れるときに戻すとした) をここで戻す
- キャッシュの下に専用 target dir ができ、容量が増える。GC の最小限 (R-CA-5) を同じ段階で入れる

## 2. 方針

- **m1-plan.md の §2 を引き継ぐ**:
  - 各段の終わりの印に `cargo +nightly fmt`・`cargo clippy --all-targets`・`cargo test` が通ることを常に含める (`RUSTUP_AUTO_INSTALL=0`。clippy と test は local あり (既定) となし (`--no-default-features`) の両構成で)。§3 では「共通の印」と書く
  - 層 (R-ARCH-1) と cargo feature `local` (R-ARCH-2) を守る。ローカルビルド・`build_external_docs`・cargo の設定の読み取り・CLI の `cache` のうち cargo を起動するものは `local` の中に置き、ローカル機能の無い構成では `build_external_docs` を `tools/list` に出さない (R-ARCH-4)
  - 測った結果と決めたことは、日付と環境を付けて §6 に書く。要件への反映は M2 の終わりにまとめて行う (§7)。要件の「M2 で決める」細部 (§4) は、それを使う段で決める。判断の要るものは、利用者に 1 件ずつ聞く
  - ホストの環境を変えない。足りない道具は入れる前に利用者に確かめる。外へ通信する実験では、利用者の git と cargo の設定と資格情報を使わない。外部への要求に、利用者のメールアドレスや名前を入れない
  - 公開と push は利用者が行う
- **ソースを誰が書くか**は §5 の P2 で決める。m1-plan.md の §2 は「ソースは利用者が書き、Claude は 1 段ずつ導いてレビューする」としたが、実際の M1 の段は、利用者の「書いてみてくれ」を受けて Claude が書き、利用者がレビューした
- **テストはホストを変えず、外へ出ない**: cargo・rustup・git を動かすテストには、隔離した環境 (一時の `HOME`・`CARGO_HOME`・git の設定、`GIT_CONFIG_NOSYSTEM=1`、端末と askpass の問い合わせを止める変数、`RUSTUP_AUTO_INSTALL=0`) を渡す。poc/m0b/env.sh の `isolate_home` と同じ考え方で、M0b で git の CLI が利用者のデスクトップに askpass のダイアログを出したことによる。toolchain は手元のものを読むだけにし、テストで入れない。crates.io・docs.rs・GitHub に出るのは、CI の `testdata` と canary の job だけにする
- **テスト用の crate** (利用者が決めた。2026-10-20):
  - 公開しないテスト用の crate を `tests/fixtures/` の下に、本体の workspace に入れない独立したパッケージか workspace として置く (`poc/` と同じ扱い)。本体を cargo の workspace に変えることは今はしない (本体と並べて公開する crate ができたときに改めて決める)
  - 種類は次のとおり。§3 の各段で、その段に要るものだけを足す (一度に全部は作らない)。各段の表の「テスト用の crate」の欄に、記号で割り当てた
    - (a) ローカルでビルドする crate を、シナリオごとに: member が複数の workspace、path 依存、壊れた build.rs、proc-macro、`#![no_std]`、cfg と feature の組、rustdoc JSON の edge case
    - (b) git 経由の crate: テストの実行時に一時ディレクトリに bare リポジトリを作り、`file://` で指す (branch・tag・rev・無い rev・無いリポジトリ)。認証の失敗は、テストの中の 401 を返す小さな HTTP サーバーで起こす。本物の GitHub は、canary のように公開リポジトリから取れることを確かめる場合だけ使う。git を動かすテストは HOME と git の設定を隔離する
    - (c) 罠の crate (範囲の外に書き込む build.rs、`.cargo/config.toml` の `build.rustc`・`rustc-wrapper`、副作用のある proc-macro): 専用のディレクトリに置き、隔離した環境でだけビルドする
    - (d) プロジェクトの形の crate (Cargo.lock が無い・古い、同名の複数版、改名した依存、dev だけの依存、OS ごとの依存、別のレジストリ、vendor): 版の解決用で、ビルドしない
    - (e) toolchain の crate (入っていない toolchain を指す `rust-toolchain.toml`、`rust-version`、stable の `RUSTC_BOOTSTRAP`、cdylib だけの lib)
    - (f) crate をまたぐ re-export の crate 群 (A→B→C の連鎖、umbrella、crate をまたぐ glob)
    - (g) 大きさ・深さの crate (テストの中で生成する)
  - テスト用の crate は crates.io の依存を持たない。registry の依存が要るテストは、テストの中の偽の crates.io (sparse index と static.crates.io) に、テストの中で作った `.crate` を置いて行う。crates.io への公開は、偽のサーバーで足りない件が出たときに改めて判断する (公開は消せない: 削除できるのは公開から 72 時間以内か、所有者が 1 人・月 1,000 ダウンロード未満・被依存なしのときだけ。規約に「package index を本来の目的外に使うこと」の禁止がある)
  - 古い形式 (46〜60) の rustdoc JSON はテスト用の crate では作れないので、今の testdata (docs.rs の実物) のまま。worktree は、テストの中で `git worktree add` で作る

---

## 3. 手順

段は 6 つ。M2a でテストの道具と設定ファイルを用意し、Q-32 の結果を既定のプロジェクトに反映する。M2b で子プロセスとプロジェクトの文脈 (ビルドの前に要るもの) を固め、M2c でプロセスをまたぐ仕組みとビルド中の応答の形を作る。M2d でローカルビルドを作って照会のツールにつなぎ、M2e で外のツール `build_external_docs` を作る。M2f で GC・罠のテスト・canary・eval・測定・README を仕上げる。

依存の順: 設定ファイル (A3) はビルドの設定 (`build.*`) と実行ファイルのパスを使う B・D の前に要る。子の後始末 (B1) とロック (C2) はビルド (D2) の前に要る。ビルド中の応答の形 (C3) は、ビルドを待つ D3 の前に、取得 (docs.rs) で先に作る。外のツール (E) は、ビルド (D) と記録の置き場 (C4) の後。

**共通の印**: `cargo +nightly fmt`・`cargo clippy --all-targets` (local あり・なしの両構成)・`cargo test` が通る (`RUSTUP_AUTO_INSTALL=0`)。各段の終わりの印の先頭に書く。

### 3.1 M2a: 土台

| # | やること | 要件 | テスト用の crate | 終わりの印 |
| --- | --- | --- | --- | --- |
| A1 | テストの道具: `tests/fixtures/` の置き場 (README に種類と使い方)、隔離した環境のヘルパー (`tests/common` に。`isolate_home` と同じ変数)、偽の crates.io (sparse index と static.crates.io。M1 の `data::test_http` を基に)、`git worktree add` で worktree を作るヘルパー。偽の crates.io を cargo の registry として使えるか (隔離した `CARGO_HOME` の設定で `[source.crates-io] replace-with` を `sparse+http://127.0.0.1:<port>/` に向けられるか) を確かめる | R-QA-3・R-SEC-1 | 道具だけ (crate は使う段で足す) | 共通の印。隔離した環境で起動した cargo が利用者の `~/.cargo` と git の設定を読まないことのテスト。偽の crates.io から cargo が依存を取れるか (取れなければ代わりの方法) を §6 に書く |
| A2 | Q-32 の反映 (§6.1): subagent の worktree を既定のプロジェクトと同じに扱う (§5 の P3)、案内 (P5)、`project_from` の値の名前 (P8)。`roots/list` は使わない (P4)。入れ子の worktree が既定のプロジェクトの決め方・`members` の glob の展開 (cargo と M1 の B5b の両方)・cargo の workspace の探索に与える影響を、`git worktree add` で作った worktree で確かめる。Claude Code をサブディレクトリで起動したとき (`CLAUDE_PROJECT_DIR` がサブディレクトリ) の既定のプロジェクトの挙動を確かめ、変えるかを利用者に聞く | R-MCP-3・R-MCP-6・§7.3・Q-32 | (d) 入れ子の worktree を持つ workspace (仮想 workspace、単独の package、`members` が glob のもの)。worktree は `git worktree add` で作る | 共通の印。P3・P5・P8 の決定どおりの実装とテスト (linked worktree の判定は、別のリポジトリの worktree・壊れた `.git` ファイル・記録が worktree を指し返さないものを既定と扱わないことを含む)。instructions と説明文が 2,048 文字以内 (R-QA-5 の検査が通る)。確かめた結果を §6 に書く |
| A3 | 設定ファイル (R-DIST-3): 置き場所と `CHATORA_RUSTDOC_MCP_CONFIG`、環境変数での上書き、知らないキーと知らない `CHATORA_RUSTDOC_MCP_*` の環境変数をログで知らせる、誤った値は既定に戻してログに出す。M2 で使う項目: `network.offline` (R-CA-7。子の cargo に `--offline`、通信が要るものは `offline_miss`)、`network.*_timeout_secs`、`std.fetch_from_dist` (R-STD-1)、`build.*`、`cache.*`、`limits.concurrent_builds`・`limits.request_timeout_secs`・`limits.extract_max_size`。M1 の環境変数だけの設定 (`log.*`・`limits.concurrent_calls`・`cache.dir`) も同じ仕組みに移す | R-DIST-3・R-CA-7・R-STD-1・R-NW-3 | なし | 共通の印。優先順 (環境変数 > 設定ファイル > 既定)、知らないキーの知らせ、誤った値、オフラインモードでどの取得元にも通信しないことのテスト。README の設定の節 |
| A4 | CI と小さな残り (§3.7): `cargo publish --dry-run --locked` を CI に置く、実データのテスト (`tests/real_data`) を macOS・Windows でも回す、`--debug` で入れたバイナリに試験用の `sleep` ツールが出ないようにする (テストのときだけ出す仕組みに替える) | R-DIST-1・R-QA-3・R-QA-5 | なし | 共通の印。CI の 3 つの OS で `testdata` の job が通る。debug ビルドの `tools/list` に `sleep` が無い |

### 3.2 M2b: 子プロセスとプロジェクトの文脈

どれも `local` の中。ビルドそのものはまだ無く、ビルドの前に要る部品と、M1 の `cargo metadata`・`cargo tree` の経路の残りを片付ける。

| # | やること | 要件 | テスト用の crate | 終わりの印 |
| --- | --- | --- | --- | --- |
| B1 | 子の起動と後始末: 子を長命の専用スレッドから spawn する (Q-29。`PR_SET_PDEATHSIG` は spawn したスレッドの終了で発火するため)、Linux の `PR_SET_PDEATHSIG`、Windows の Job Object、取り消しと時間切れでは子のプロセスグループ (Windows は Job) ごと止める (孫の git などを残さない。§3.7)、stdin の EOF・SIGINT・SIGTERM で子を止める (M0 の申し送り。rmcp が EOF の後に処理中の要求を最大 5 秒待つ間も子が動き続けるため) | R-WS-6・Q-29 | (c) `build.rustc` に置いた、孫を起動して待ち続けるスクリプト | 共通の印。取り消し・時間切れ・stdin の EOF で子と孫が残らないテスト (3 つの OS の CI で確かめられる範囲)。spawn するスレッドで `PR_SET_PDEATHSIG` が効く条件を §6 に書く |
| B2 | ロック待ちの検出: 子の stderr を流しながら読み、`Blocking waiting for file lock` で `building_reason: waiting_for_lock` を出せるようにする (応答につなぐのは C3・D3) | R-WS-2・R-WS-7 | なし (A3 の `build.cargo_path` で、その行を出して待つ偽の cargo を使う) | 共通の印。偽の cargo で状態が `waiting_for_lock` に変わるテスト |
| B3 | cargo の設定の読み方 (Q-31): 合成の規則 (祖先と `$CARGO_HOME` の全階層、`include`、拡張子の無い `.cargo/config`、`CARGO_BUILD_TARGET`、member のディレクトリの設定は効かない)。プロジェクトの `build.target` を読み、評価する target (R-TG-1) と `cargo tree --target` に使う (配列なら先頭を既定にし、warnings と provenance に出す)。独自の target (`.json`・`[unstable] build-std`) は `target_unsupported`。`include` が指すファイルを `cargo metadata` の鍵に入れる。cargo 自身に読ませる手段があるかも確かめる | R-WS-2・R-TG-1・R-VR-2・R-WS-4・Q-31 | (d) に設定の形を足す (祖先の `.cargo/config.toml`、`include`、配列の `build.target`、`CARGO_BUILD_TARGET`) | 共通の印。M1 の B5b の実測 (cargo 1.99.1) の各規則のテスト。`build.target` が wasm のプロジェクトで、`get_item` の provenance と `enabled_in_project` がその target になる |
| B4 | プロジェクトの文脈の残り: 暗黙の member を `cargo metadata` の `workspace_root` と突き合わせて cargo の答えを使う (M1 の B5a・B5b)。利用者の `net.offline` を B3 の読み方で判定し、`no matching package named` を `offline_miss` に数える (M1 はサーバーが `--offline` を渡したときだけ)。`cargo metadata`・`cargo tree` の同時の呼び出しを 1 つにまとめ、失敗を一時的な失敗の TTL (目安 30 秒) だけ覚える。`cargo metadata` の初回の依存の取得を、C3 で `building_reason: fetching` にできるよう印を付ける | R-MCP-3・R-WS-7・R-VR-2・R-ERR-3 | (d) 暗黙の member (ルートの package の path 依存)、`net.offline` を設定した workspace | 共通の印。同時の 4 つの呼び出しで `cargo metadata` が 1 回だけ起動するテスト。失敗が 30 秒覚えられるテスト |
| B5 | git の取得の失敗の文言をテストに固定する: `fetch_failed` の `auth`・`no_repository`・`no_rev` を、libgit2 と git の CLI (`net.git-fetch-with-cli`) の両方で。`cargo fetch`・`cargo metadata`・`cargo tree` で同じこと | R-WS-7・§8 | (b) bare リポジトリ (branch・tag・rev・無い rev・無いリポジトリ) と 401 を返す HTTP サーバー | 共通の印。各文言の分類のテスト (隔離した HOME で、askpass のダイアログが出ない)。文言と cargo・git の版を §6 に書く |
| B6 | `.crate` の取得 (§3.7): 手元の `$CARGO_HOME/registry/cache` に同じ版の `.crate` があれば、sparse index の `cksum` で確かめて使い、static.crates.io から取らない。static.crates.io からの取得が crates.io のダウンロード数に数えられるかを、公開されている文書で確かめる (確かめていない) | R-SC-2・R-NW-1 | (d) 隔離した `CARGO_HOME` に置いた `.crate` | 共通の印。手元にあれば通信しないテスト。数えられるかと、数えられるならほかに打つ手 (README に書くなど) を §6 に書く |
| B7 | ビルドの範囲の判定 (照会のツールがローカルビルドしてよい (package, target) の組): `cargo tree --workspace --target <t> -e normal,build,dev` の集合、host 用と target 用の判定、`cargo metadata` の package id への対応付け、解析できない行は `scope_unknown`。resolver 2 以上で `-p` に名指しできない依存を起動前に判定する (`build_unsupported`)。`--target` を付けない形での M0 の B1・B3・B5 の再確認 (Q-31) | R-MCP-3・R-WS-5・R-FT-3・Q-31 | (d) resolver 1 と 2 の workspace に、OS ごとの依存・dev だけ・build だけ・proc-macro の下だけの依存 | 共通の印。M0 の実測 (2026-10-16) と同じ集合になるテスト。判定だけで、cargo rustdoc はまだ起動しない |
| B8 | 明示した版とプロジェクトの照合のうち、crates.io から取るもの (R-VR-1): 明示した版がプロジェクトの版と同じなら、`enabled_in_project` (R-FT-2) を出す。path・git などの取得元を返す分は、ローカルビルドと一緒に D7 で | R-VR-1・R-FT-2 | (d) | 共通の印。版を明示しても、同じ版なら `enabled_in_project` が付くテスト |

### 3.3 M2c: プロセスをまたぐ仕組みとビルド中の応答

| # | やること | 要件 | テスト用の crate | 終わりの印 |
| --- | --- | --- | --- | --- |
| C1 | Q-29 のロックの細部を決める: 置き場所 (案: state の `locks/`)、名前の作り方、取る順 (案: 鍵 → target dir → ビルドの枠)、target dir のロック (案: ビルドは共有、GC と古い target dir の削除は排他)、版の違うサーバーの記録 (版をどこに入れるか、対象、片付け)。OS のロック (`flock`・`LockFileEx`) の挙動を 3 つの OS で確かめる (§5 の P6)。NFS は確かめられないので README に書く | R-CA-4・Q-29 | なし | 共通の印。決めたことを §6 に書く。3 つの OS の CI で、2 つのプロセスの排他・持ち主が死んだときのロックの解放のテスト |
| C2 | プロセスをまたぐ排他と取得の singleflight: M1 の `Cache::lock` (プロセスの中) に OS のロックを足し、ロックを取ってからキャッシュを確かめ直す (R-CA-4)。同じものの取得をプロセスの中で共有し、待っている要求がすべて取り消されたときだけ止める (R-NW-2・R-WS-6。M1 は 1 つ目が取り消されると 2 つ目が取り直していた)。Windows で 2 つのセッションが同じアーカイブを書き、相手が mmap しているファイルを置き換えにいく件 (§3.7) | R-CA-4・R-NW-2・R-WS-6・Q-29 | なし | 共通の印。2 つのプロセスが同じ crate を同時に引いて、ローカルの HTTP サーバーへの取得が 1 回のテスト。取り消しの組み合わせのテスト。Windows の CI で、2 つのプロセスが同じアーカイブを作っても失敗しないテスト |
| C3 | 1 つの crate の照会の時間の上限と `status: building`: 1 リクエストの上限 (`limits.request_timeout_secs`、既定 30 秒) を超えた docs.rs の取得・索引の作成・`cargo metadata` の初回の取得は、裏で続けて `status: building` (`building_reason: fetching`・`indexing`) と `retry_after_secs` を返す。`notifications/progress` (`progressToken` があるときだけ、単調に増やす、1〜30 秒の間隔、応答か取り消しで止める)。`retry_after_secs` を付けた後は切り離し、取り消しでは止めない。ビルド中のカーソルの扱い (R-OUT-3)。outputSchema に状態の項目を足す (R-OUT-9) | R-ARCH-6・R-WS-6・R-OUT-9・R-OUT-3・R-QA-5 | なし (応答の遅いローカルの HTTP サーバー) | 共通の印。遅い取得で `status: building` が返り、次の呼び出しで結果が返るテスト。進捗通知の MUST (token・単調・止める) のテスト。`status: building` の応答が outputSchema の検証を通る |
| C4 | state の記録と起動時の後始末: ビルドの作業用ディレクトリの置き場と、持ち主の死んだものの削除 (R-WS-6・R-CA-4)。残った子孫の記録と照合の規則 (案: サーバーの PID と起動時刻、子のプロセスグループの ID とその起動時刻)、確かでなければ止めない。止めなかった子孫がいる target dir を使わず、別の名前で作り直してほかのプロセスに伝える (R-WS-2) | R-WS-6・R-WS-2・R-CA-4・Q-29 | (c) 親が死んだ後も書き続ける孫 | 共通の印。持ち主の死んだ作業用ディレクトリが起動時に消え、生きているものは消えないテスト。PID の再利用で無関係なプロセスを止めないテスト |

### 3.4 M2d: ローカルビルド

| # | やること | 要件 | テスト用の crate | 終わりの印 |
| --- | --- | --- | --- | --- |
| D1 | toolchain と形式: プロジェクトの toolchain (rustup にカレントディレクトリから決めさせる。`+<toolchain>` はサーバーが選んだときだけ)、子に `RUSTUP_TOOLCHAIN` を渡さない (rustup の挙動を実測する)、実行ファイルのパスの設定 (`build.*_path`。一部だけなら同じディレクトリから探す。版の接尾辞と symlink は Q-31)、sysroot の `lib/rustlib/<target>` と host の libcore で `toolchain_missing`、出る形式を調べて対応範囲より古ければ nightly に切り替え、新しすぎて読めなければ `unsupported_format` | R-WS-8・R-WS-1・R-TG-1・R-WS-7・Q-31 | (e) 入っていない toolchain を指す `rust-toolchain.toml` (`RUSTUP_AUTO_INSTALL=0` で入れずに確かめる)、`rust-version` | 共通の印。toolchain の選び方と `toolchain_missing` のテスト。調べた結果のキャッシュと鍵を §6 に書く |
| D2 | 1 つの member を文書化する: R-WS-1 の基本形 (`--locked`・`-j`・package id の `-p`・`--lib`/`--bin`・`--target` の規則・`--document-private-items`)。`RUSTC_BOOTSTRAP` の値の固定 (全 lib・proc-macro と member の bin の crate 名。長さの上限は Q-31)、専用 target dir と `CARGO_BUILD_BUILD_DIR` (短いハッシュの名前。Windows の MAX_PATH)、作業用ディレクトリで作り成功したものだけを原子的に公開、ターゲットごとに 1 回ずつ (lib と bin が同名のとき)、proc-macro は host の文書として記録、`compiler-artifact` の `features` を `features_effective` に。provenance (`source: local`・`toolchain`・`built_at`)。同名の crate を並行にビルドして JSON・`.d` を上書きしうる件 (Q-29) | R-WS-1・R-WS-2・R-WS-3・R-WS-8・R-WS-6・R-OUT-9・Q-29・Q-31 | (a) 1 つの lib の member、lib と bin が同名の package、proc-macro の member、build.rs が cfg と env を出す member。(e) stable の `RUSTC_BOOTSTRAP`、cdylib だけの lib | 共通の印。member の item を `get_item` で引けるテスト。stdout の各行が JSON-RPC のメッセージであることを、ローカルビルドを伴う呼び出しで確かめる (R-SEC-1)。値を固定すれば 2 回目は文書化する crate だけが作り直されるテスト |
| D3 | feature の組・`--cfg docsrs`・ビルドの枠と待ち: member の `[package.metadata.docs.rs]` の組 (feature 名は R-SEC-4 で検証)、その組で壊れたら通常の組でやり直す、通常の組に無い依存が増えるときの扱い (R-MCP-3)。`--cfg docsrs` で壊れたら渡さずにやり直す (壊れた crate を記録するかは Q-11)。同時のビルドの上限 (`limits.concurrent_builds`、初期値 2) と `waiting_for_slot`、ビルドを待つ上限 (`build.wait_secs`、初期値 10 秒) と `status: building`、1 回のビルドの最大時間 (`build.timeout_secs`)。ビルドの singleflight と、取り消しで止めるのは待つ要求がすべて取り消されたときだけ | R-FT-3・R-WS-1・R-WS-6・R-SEC-7・R-SEC-4・R-MCP-3・Q-11・Q-31 | (a) cfg と feature の組 (相互排他の feature、`docsrs` のときだけ使う nightly の機能)、member が複数で依存の feature が分かれる workspace、workspace の中の path 依存、`default-members` | 共通の印。組のやり直しと `--cfg docsrs` のやり直しが warnings に出るテスト。枠を超えたビルドが `waiting_for_slot` になるテスト。複数の member での文書の組と `enabled_in_project` の組の違いを §6 に書く (Q-31) |
| D4 | 鮮度: 内容ダイジェストの鍵 (R-WS-4 の一覧)、mtime は一次の絞り込みだけ、dep-info は同じ引数に `--emit=dep-info` を付けた 2 回目の rustdoc で取る、ビルドの前に鍵を取り、それより新しい mtime があれば次回は鮮度切れ。再ビルド中は直近の JSON を `stale: true`・`stale_reason: rebuilding`・`built_at` 付きで返す。ソースがその後に変わっていれば LSP を案内する (R-WS-9)。入れ子の worktree や target dir を走査しない (§6.1) | R-WS-4・R-WS-9・R-CA-2・Q-31 | (a) `include_str!` で README を読む member、`#[path]` のモジュール、build.rs が生成するコード、workspace の外の path 依存 | 共通の印。member を編集すると再ビルドされ、編集しなければされないテスト (完了条件の「編集後に正しく再ビルドされる」)。ビルドの間の編集が次回に拾われるテスト |
| D5 | 失敗: `build_failed` に診断を先頭 N 件 (`build.max_diagnostics`、初期値 2。1 件 3,500 文字・合計 8,000 文字以内)、JSON の診断が無ければ stderr の末尾。一時的な失敗 (シグナル・サーバーが止めたもの・`cargo fetch --locked` の失敗) と決定的な失敗 (`refresh` まで R-WS-4 の鍵でキャッシュ。`timeout` を含む) を分ける。`toolchain_too_old` (E0658・`requires rustc`)。引数 `refresh` と、それが解く記録の一覧 (§6.4)。失敗の判定に使う文言をテストに固定する | R-WS-7・R-ERR-2・§6.4・§8 | (a) 壊れた build.rs、コンパイルエラーのある member。(e) `rust-version` が toolchain より新しい crate | 共通の印。決定的な失敗が 2 回目にビルドせずに返り、`refresh` でやり直すテスト。エラーの text が 1 万文字以内のテスト |
| D6 | private item と workspace の照会: 公開パスの無い member の item を crate 内のパス (`crate::a::b::Item`) と可視性で示す。trait の item と enum の variant は親の可視性を継ぐ。依存と std の非公開 item は出さない。自分の workspace の crate の検索は private な item も含める。ローカルの JSON は版付きの `html_root_url` を持たないので、member どうしの参照の版は依存グラフで決める | R-PR-2・R-SH-5・R-SR-4・R-WS-3・R-PR-4 | (a) private のモジュール・`pub(crate)`・`pub(in path)`・private な型を返す公開の関数。(f) member の間の A→B→C の re-export、umbrella の member、crate をまたぐ glob | 共通の印。`list_impls` で自分の型 (§3 の Q8 の `MyType`) に答えられる。private な item を検索で引けるテスト。依存の private な item が出ないテスト |
| D7 | docs.rs で引けない依存を照会のツールにつなぐ: `local_build_required` (path・git・別レジストリ・checksum の不一致) と、既定のプロジェクトの依存で求める target の JSON が無いとき (`no_target_json`) をローカルビルドに置き換える (ビルドしてよい組は B7。名指しできない依存は `build_unsupported`、判定できなければ `scope_unknown`)。照会のツールに実際のビルド target と違う `target` を渡されたらビルドしない。独自の target (`.json`・`[unstable] build-std`) のプロジェクトは、ローカルビルドせず `target_unsupported`、docs.rs の照会はその crate の docs.rs での既定 target にフォールバックして明示する (R-TG-1・R-TG-2。B3b から回した)。明示した版がプロジェクトの版と同じなら、プロジェクトの取得元のものを返して warnings を付ける (R-VR-1 の残り)。`crate` を省いた `search_items` の範囲に workspace を足す (R-SR-2)。ビルド中の crate は `sources` に `building` | R-TG-1・R-DS-1・R-VR-2・R-VR-1・R-MCP-3・R-WS-5・R-SR-2・R-SR-5 | (d) と (a) を組み合わせた workspace (path 依存、別のレジストリの依存 (偽の crates.io を別のレジストリとして)、checksum の合わない vendor)。(b) git の依存。(g) 依存の多い workspace (テストの中で生成) | 共通の印。path・git・別レジストリの依存を引けるテスト。docs.rs のモックで target の JSON が無い依存がローカルビルドされるテスト。範囲の外の target を渡すとビルドしないテスト。CI の `testdata` の job で、実データのテスト (`real_data`) を macOS・Windows でも回す (A4b で Linux だけにした条件を外す) |

### 3.5 M2e: 外のツール `build_external_docs`

| # | やること | 要件 | テスト用の crate | 終わりの印 |
| --- | --- | --- | --- | --- |
| E1 | ツールの枠と記録: 説明文 (§3 の Q22)、annotations (`idempotentHint: false`)、入力から処理を決める表、外の記録の鍵 (照会のツールが組み立てられるもの) と置き場所、進行中の印 (C1 のロック)、照会のツールが記録を読む経路 (`no_record`・`record_broken`・`stale` の `inputs_changed`。cargo を起動しない)、ビルドを待つ上限を超えたら `status: building` で裏で続ける。ローカル機能の無い構成では出さない | R-WS-10・R-MCP-3・R-MCP-4・R-ARCH-4・R-QA-5 | なし (E2〜E4 で使う) | 共通の印。説明文・annotations・引数の検査 (R-QA-5) が通る。記録が無ければ照会のツールが `no_record` を返し、外のツールを案内するテスト |
| E2 | 仮のプロジェクト: 依存でない crate と Cargo.lock に無い版、`-p` で名指しできない依存。Cargo.toml を TOML のライブラリで組み立てる、feature の既定は `[package.metadata.docs.rs]` (壊れたら default の組)、target は引数か host。仮のプロジェクトの toolchain・`RUSTC_BOOTSTRAP` の値・target dir (Q-31)。オフラインでは `offline_miss` | R-WS-5・R-WS-10・R-FT-1・Q-31 | 偽の crates.io に置いた crate。(e) cdylib だけの lib | 共通の印。依存でない crate を作って、照会のツールで引けるテスト (偽の crates.io で)。同じ引数の呼び直しで失敗のキャッシュが解けるテスト |
| E3 | git の crate: 仮のプロジェクトの git 依存 (`rev`・`branch`・`tag` を cargo の同名のキーに写す)、URL の検証と正規化 (R-SEC-4・cargo の `CanonicalUrl` に合わせる)、平文の URL の warnings、呼び直しで `branch`・`tag` を解決し直す、照会のツールでの短いハッシュの照合と解決の古さ (`ref_ttl_expired`)。テストの bare リポジトリは `file://` で置くが、引数 `git` は `file://` を受けない (R-SEC-4) ので、テストでの指し方を決める (§4) | R-WS-10・R-WS-5・R-SEC-4・R-MCP-3・§6.4 | (b) bare リポジトリ (branch・tag・rev・無い rev・無いリポジトリ) | 共通の印。branch・tag・rev で作って引けるテスト。`fetch_failed` の 3 つの理由のテスト |
| E4 | 他の target と外の workspace: 既定のプロジェクトの member と依存を他の target で文書化する (`--target`、その target の `rust-std` が無ければ `toolchain_missing`)。外の workspace (`project`) は `cargo metadata` の結果と入力の内容ダイジェストを記録し、照会のツールはその記録で版を解く。プロジェクトのディレクトリが消えたとき (`project_missing`・`project_not_found`) | R-WS-10・R-MCP-3・R-TG-1・§8 | (d) OS ごとの依存。(a) 外の workspace | 共通の印。Linux で Windows 向けの依存と `#[cfg(windows)]` の item を引けるテスト (target の `rust-std` がある場合)。外の workspace を記録して引けるテスト (完了条件の「外のプロジェクトと git の crate を引ける」) |

### 3.6 M2f: 片付け・検査・測定・README

| # | やること | 要件 | テスト用の crate | 終わりの印 |
| --- | --- | --- | --- | --- |
| F1 | CLI の `cache ls`・`cache gc`・`cache clear [<crate@版>]` と GC の最小限: 文書のキャッシュと専用 target dir の容量の上限 (初期値 5GB・20GB) と target dir の保持日数 (初期値 30 日)、所有マーカーと `CACHEDIR.TAG`、ロック中の target dir を消さない、symlink をたどらない、M1 の途中で配置を変える前の古いキャッシュの片付け。Windows で mmap 中のファイルを消せないときの扱い。引数の解析の仕方 (M0 の D3) | R-CA-5・R-DIST-2・R-CA-4 | (g) テストの中で作る大きなキャッシュ | 共通の印。上限を超えたら古い順に消え、ロック中の target dir とマーカーの無いディレクトリは消えないテスト。サーバーと CLI が同時に動いても壊れないテスト |
| F2 | 罠のテスト: 照会のツールが範囲の外の依存の build.rs を動かさないこと (他の target だけの依存、host と違う `build.target` での target 用の build-dependency、member の docs.rs の組でだけ有効になる optional の依存を、resolver 1 と 2 で)。同じ罠を範囲の中の依存にも置いて印ができること (陽性の対照)。M1 の罠のテストも合わせ、3 つの OS で回す (§3.7) | R-QA-5・R-MCP-3 | (c) 印を書く build.rs・`build.rustc`・`rustc-wrapper`・副作用のある proc-macro。Windows でも動く形 (シェルのスクリプトに頼らない) | 共通の印。CI の 3 つの OS で罠のテストが通る (Windows で作れない罠があれば、理由を §6 に書く) |
| F3 | canary に beta の toolchain を足す: crate 名を指定した `RUSTC_BOOTSTRAP` で合成の workspace をローカルビルドし、形式と受け付けの変化を検知する | R-QA-4 | (a) と (f) から小さなものを選ぶ | 共通の印。canary の job が手元と手動の実行で通る |
| F4 | eval: ビルド中の応答 (R-QA-6 が必ず含めるとしたもの)、自分の型 (§3 の Q8 の `MyType`)、外の workspace と git の crate (§3 の Q22)、worktree の案内 (§5 の P5) が効くかのケース | R-QA-6 | eval の fixture に member を足す | 結果を §6 に書く。ツールの分け方と応答の形に問題があれば直す |
| F5 | 測定: `build.wait_secs`・`build.jobs` (`-j`)・診断の上限 (`build.max_diagnostics` と文字数) の値を決める。このリポジトリと eval の fixture で、初回と 2 回目のビルドの時間、作り直しの範囲、target dir の大きさ。macOS・Windows で docs.rs に target の JSON が無い依存をローカルビルドしたときの待ち時間 (CI で測れる範囲) | R-WS-6・R-WS-1・R-WS-7・§9.1 | (g) 依存の多い workspace | 結果と決めた値を §6 に書く |
| F6 | README を M2 の実態に合わせる: ローカルビルドと `build_external_docs`、Security の節 (照会のツールが既定のプロジェクトを確認なしにビルドすること、許可ルールの表 (手動の登録とプラグインの登録の両方))、worktree の使い方 (§6.1)、ロックと CPU を rust-analyzer と取り合うこと、NFS、設定ファイル、`cache` の CLI、「Linux 向け」の記述を改める。公開は M3 の後 (§5 の P9) | R-SEC-2・R-WS-2・R-DIST-1・R-DIST-3・R-QA-5 | なし | README のツール名と許可ルールの例の検査 (R-QA-5) が通る。`cargo publish --dry-run --locked` が通る (公開はしない) |

### 3.7 M1 から回したものと公開前の見直しの残りの振り分け

要件の §12 の M2 の「M1 から回したもの」と、m1-plan.md の §6 の「公開前の見直し」の末尾でレビュアーが挙げた残りを、入れる段に振り分けた。M2 の外に回すものは無い。

| 件 | 出どころ | 段 |
| --- | --- | --- |
| プロジェクトの `build.target` を読む | M1 の B5d・C1d-2 | B3 |
| 明示した版とプロジェクトの取得元の照合、そのときの `enabled_in_project` | M1 の C1b-2・C1d-3 | B8 (crates.io の分)・D7 (path・git の分) |
| 暗黙の member と `workspace_root` の突き合わせ | M1 の B5a・B5b | B4 |
| `cargo metadata`・`cargo tree` の合流と失敗の保持 (レビュアーも挙げた) | M1 の B5b・要件への反映の判断 13 | B4 |
| cargo の設定の `include` を鍵に入れる | M1 の B5b | B3 |
| 1 つの crate の照会の 30 秒の上限と `status: building`・`fetching` | M1 の C1b-2・要件への反映の判断 17 | C3 |
| `PR_SET_PDEATHSIG`・Job Object | M1 の B5b | B1 |
| `waiting_for_lock` | M1 の B5b | B2 |
| プロセスをまたぐロックと取得の singleflight | M1 の B6・D2 | C2 |
| 利用者の `net.offline` での `offline_miss` | 要件への反映の判断 7 | B4 |
| 設定ファイルと知らない設定の知らせ、オフラインモード、dist を無効にする設定、cargo の実行ファイルのパス | M1 の B5b・B8・D1・要件への反映の判断 13 | A3 (パスを使うのは D1) |
| ローカルビルドの数の上限 | M1 の D2 | D3 |
| 範囲の外の依存の build.rs の罠のテスト | M1 の D4b | F2 |
| beta の toolchain の canary | M1 の E3 | F3 |
| eval のビルド中の応答 | M1 の E1 | F4 |
| README の許可ルールの表 | M1 の E4 | F6 |
| 古い配置のキャッシュの片付け | M1 の B6 | F1 |
| プロセスをまたぐロック (Windows で 2 つのセッションが同じアーカイブを同時に書くと、相手が mmap しているファイルを置き換えにいく) | レビュアー | C1・C2 |
| 取り消しや時間切れで孫のプロセス (git など) が残る | レビュアー | B1 |
| `.crate` の取得が crates.io のダウンロード数に数えられうる | レビュアー | B6 |
| `project_from` の値 (`CLAUDE_PROJECT_DIR` など) の名前 | レビュアー | A2 (§5 の P8) |
| `--debug` で入れると試験用の `sleep` ツールが出る | レビュアー | A4 |
| 実データのテストと罠のテストが Linux でしか回らない | レビュアー | A4 (実データ)・F2 (罠) |
| `cargo publish --dry-run` が CI に無い | レビュアー | A4 |

---

## 4. 要件が「M2 で決める」とした細部

それを使う段で決め、§6 に書く。Q-31 が「本文が M2 で決めるとしたものも M2 の計画で本文から拾う」としたものを含む。

| 論点 | 要件 | 段 |
| --- | --- | --- |
| Q-32 の結果の反映 (worktree・サブディレクトリでの起動)、`project_from` の値の名前 | R-MCP-3・§7.3・Q-32 | A2 |
| 環境変数で受けられる値の制約 | R-DIST-3 | A3 |
| `cargo publish --dry-run` を CI に置くか | R-DIST-1 | A4 |
| stdin の EOF・SIGINT・SIGTERM でビルドを止める処理 (M0 の申し送り) | R-WS-6 | B1 |
| 子を spawn するスレッド | Q-29 | B1 |
| cargo の設定の読み方 (合成の規則、`include`、拡張子の無い `config`) | R-WS-2・R-TG-1・R-VR-2・Q-31 | B3 |
| `--target` を付けない形での M0 の B1・B3・B5 の再確認 | R-WS-1・R-WS-5・Q-31 | B7 |
| 同じ package が host 用と target 用の両方に現れるときの feature の組 | R-FT-3・Q-31 | B7・D3 |
| ロックの置き場所・名前・増え方・取る順、target dir のロック、NFS 上の `flock` | R-CA-4・Q-29 | C1 |
| 版の違うサーバーの記録 (版の入れ場所・対象の記録・元データのダイジェストの記録との関係・古い版の片付け) | R-CA-4・Q-29 | C1・C2 |
| Windows で mmap 中の派生物を作り直すときの名前 | R-CA-4・Q-29 | C2 |
| ビルドの状態の値と応答の骨格 (`status: building` の provenance の省略など) | R-OUT-9 | C3 |
| ビルド中のカーソルの扱い | R-OUT-3 | C3 |
| 作業用ディレクトリの後始末 | R-CA-4・R-WS-6 | C4 |
| 残った子孫の記録・照合の規則・判定の上限、使えない target dir の名前とほかのプロセスへの伝え方 | R-WS-6・R-WS-2・Q-29 | C4 |
| rustup の挙動 (子に `RUSTUP_TOOLCHAIN` を渡さないとき) の実測 | R-WS-1 | D1 |
| 形式を調べる時期、対応範囲の形式を出す toolchain への自動の切り替え、`rustc -vV` と調べた結果をキャッシュするかとその鍵 | R-WS-8 | D1 |
| 実行ファイルを同じディレクトリから探す規則 (版の接尾辞が付く配布物と symlink) | R-WS-1・Q-31 | D1 |
| `RUSTC_BOOTSTRAP` の値の長さの上限と超えたときの扱い、nightly の経路で壊れたときに member の名前だけの値でやり直すか | R-WS-8・Q-31 | D2 |
| `-Z fine-grain-locking` と `cargo doc --output-format json` を nightly の経路で使うか | R-WS-2・R-WS-1・Q-17・Q-31 | D2 |
| 同名の crate を並行にビルドして JSON・`.d` を上書きしうること | Q-29 | D2 |
| member の文書化の組で通常の組に無い依存が増えるときの文書化の仕方、壊れた組を記録するか | R-FT-3・R-MCP-3 | D3 |
| 複数の member・workspace の中の path 依存・`default-members` での文書の組と `enabled_in_project` の組 | R-FT-3・Q-31 | D3 |
| `--cfg docsrs` で壊れた crate を記録するか、member の `rustdoc-args` から `--cfg` を取り出して渡すか | R-WS-1・Q-11・Q-31 | D3 |
| 置き換えられた古いビルドが枠を占め続けること | Q-29 | D3 |
| 鮮度の鍵を、要求した引数・feature の組とやり直しで実際に使ったもののどちらで作るか | R-WS-4・Q-31 | D4 |
| `build_failed` の診断の件数と文字数 | R-WS-7 | D5・F5 |
| 失敗の判定に使う文言をテストに固定する | R-WS-7 | B5 (git)・D5 (ほか) |
| `refresh` が解く記録の一覧 (決定的な失敗のほか、否定キャッシュ、`--cfg docsrs` の記録など) | §6.4 | D5 |
| 外の記録の種類ごとの鍵の組、照合の絞り込み | R-MCP-3 | E1 |
| 進行中の印の取り方 (試し取りの手順、ビルドの枠を待つ間も持つか、git の要求の鍵と解決したコミットの鍵の両方を持つか、呼び直しのとき進行中のビルドが先にブランチを解決していた場合) | R-WS-10・Q-29 | E1・E3 |
| 仮のプロジェクトの toolchain・`RUSTC_BOOTSTRAP` の値・target dir、壊れた組を記録するか | R-WS-5・Q-31 | E2 |
| 照会のツールでの git の解決の選び方と古いとみなす時間、短いハッシュの照合 | R-MCP-3・§6.4 | E3 |
| テストの `file://` の bare リポジトリを、`file://` を受けない引数 `git` (R-SEC-4) でどう指すか | R-SEC-4 | E3 |
| 指したプロジェクトのディレクトリが消えたときの扱い | R-MCP-3・§8 | E4 |
| CLI の引数の解析 (clap などを使うか。M0 の D3) | R-DIST-2 | F1 |
| GC で Windows の mmap 中のファイルを消せないときの扱い | R-CA-5・R-CA-4 | F1 |
| `build.wait_secs` と `-j` (`build.jobs`) の値 | R-WS-6・R-WS-1 | F5 |
| workspace から継承した `readme` と拡張子の無い `README` の扱い | R-SC-4 | M2 では使う場所が無い (ソースと README のツールは M3)。§7 で M3 に送る |

---

## 5. 計画の段階で利用者が決めること

1 件ずつ独立に決められるように書いた。

**決定 (2026-10-20、利用者)**: P1・P2・P3・P5・P6・P7・P8 は推す案、P9 は「M3 の後にまとめて公開する」。P4 は聞かずに要件のまま「使わない」とした (要件の R-MCP-3 が、2026-07-28 ではサーバーから `roots/list` を送れるのは MRTR の中だけとしており、このサーバーは MRTR を使わないので、決まっている件だった)。

| # | 論点 | 推す案 | 理由 | ほかの案 |
| --- | --- | --- | --- | --- |
| P1 | 段の進め方 | §3 のとおり、土台 → 子プロセスとプロジェクトの文脈 → プロセスをまたぐ仕組み → ローカルビルド → 外のツール → 片付けと測定。M2d の D2 (1 つの member を引ける) ができた時点で、手元で Claude Code から一度引いてみる | ビルドの前に要る部品 (子の後始末・ロック・ビルド中の応答の形) を先に固めると、ビルドを入れてから直す範囲が小さい。ビルド中の応答の形は、docs.rs の取得で先に作れる (C3) | ビルドを先に縦に通し (D2 を先頭に)、後から子の後始末とロックを足す。動くものは早いが、子が残る・ロックが無い状態で手元で試すことになる |
| P2 | ソースを誰が書くか | Claude が段ごとに書き、利用者がレビューする (M1 の実際のやり方)。利用者が自分で書きたい段は、その段の始めに言えばそこだけ導きに切り替える | M1 はこの形で回り、段ごとのレビューで不具合も見つかった。M2 は OS ごとの挙動 (プロセス・ロック) を CI で確かめながら直す回数が多く、書く人と試す人が同じほうが速い | m1-plan.md の §2 のとおり、利用者が書き、Claude が 1 段ずつ導いてレビューする |
| P3 | subagent の worktree の扱い (§6.1。サーバーは subagent の worktree を知れない) | 既定のプロジェクトの git リポジトリの linked worktree を `project` で指されたら、既定のプロジェクトと同じに扱う (cargo を起動し、確認なしでビルドする)。判定はファイルを読むだけで行い (worktree の `.git` ファイルが既定のプロジェクトのリポジトリの worktree の記録を指し、その記録も worktree を指すこと。形は A2 で確かめる)、git の CLI は起動しない。要件の R-MCP-3 を改める | 同じリポジトリのコードで、subagent はそこで Bash から cargo を実行できる (R-SEC-2 の守りの範囲の考え方)。外の扱いのままだと、版の解決まで `no_record` になり、案内に従って `project` を渡しても `build_external_docs` を経ないと何も引けない (利用者が ask を書いていれば確認が出る)。R-MCP-3 の「途中で増えた子を既定のプロジェクトにしない」は、エージェントが clone した第三者のコードのための規則で、同じリポジトリの worktree とは違う | (b) 要件どおり外の扱い (`build_external_docs` が要る)。(c) 外の扱いのまま、記録が無ければ Cargo.lock を直接読んで docs.rs の照会の版を決める (cargo を起動しない。member の文書は外のツール) |
| P4 | `roots/list` を既定のプロジェクトの決定に使うか | 使わない (要件の R-MCP-3 のまま) | Roots は 2026-07-28 で deprecated で、サーバーから送れるのは MRTR の中だけ。このサーバーは MRTR を使わない (R-SEC-2・§10)。測った `roots/list` は旧来の `initialize` の接続のもの。得られても subagent の worktree は入らない (親の roots だけで、変更の知らせも来ない)。`--add-dir` のディレクトリは `project` で指せる | 2026-07-28 で得られれば、`--add-dir` のディレクトリを既定のプロジェクトの候補に足す (確認なしで cargo を起動する範囲が、利用者が足したディレクトリに広がる) |
| P5 | worktree の案内を書く場所 | instructions に 1 文、各ツールの `project` 引数の説明に 1 文 (「worktree で作業しているなら、`project` に作業ディレクトリを渡す」)。eval (F4) で効き目を見る | subagent に instructions が渡るかは確かめていない。`project` 引数の説明はツールの定義と一緒に subagent に届く。instructions は今 1,139 文字で、2,048 文字に収まる | instructions だけ。またはツールの説明文だけ |
| P6 | OS の挙動 (ロック・子の後始末) の確かめ方 | 別の PoC を作らず、製品のコードに入れて 3 つの OS の CI のテストで確かめる。CI で分からないものだけ `poc/m2-*` で手元で確かめる | 手元は Linux だけで、macOS・Windows の挙動はどのみち CI で確かめる。PoC を CI で回すには別の workflow が要り、二度手間になる | Q-29 の要件どおり、`poc/` に PoC を置き、CI の一時の job で回してから製品に入れる |
| P7 | ローカルビルドのテストの回し方 | 依存の無い小さなテスト用の crate のローカルビルドは、普段の `cargo test` で 3 つの OS で回す。大きなもの (このリポジトリや eval の fixture の文書化、依存の多い生成の workspace) は `#[ignore]` にし、Linux の `testdata` の job で回す | macOS・Windows の利用者に効く機能 (target の JSON が無い依存のローカルビルド) なので、3 つの OS で確かめたい。依存の無い crate なら 1 件数秒で済む (見込み。D2 で測る) | すべて Linux だけ。またはすべて 3 つの OS |
| P8 | `project_from` の値の名前 (レビュアー) | `CLAUDE_PROJECT_DIR` を `claude_project_dir` に改め、`argument`・`cwd` はそのままにする。A2 で入れ、要件の §7.3 を改める | ほかの値 (`cargo_lock`・`explicit_req` など) は snake_case で、環境変数の名前だけが混ざっている。値の意味は変わらない。crates.io に公開した後なら、応答の値が変わることを変更履歴に書く | そのまま。または `env` (どの環境変数かは README に書く) |
| P9 | M2 の終わりに crates.io へ新しい版を公開するか | 公開する (利用者が行う)。F6 で README を合わせ、`cargo publish --dry-run --locked` を確かめる | M2 で macOS・Windows の「実質的に Linux 向け」の制限が外れ、自分の workspace も引けるようになる。設定項目の凍結は M5 のまま (R-DIST-3) なので、公開しても設定の名前は変えられる | M3 の後にまとめて公開する |

---

## 6. 結果

### 6.1 着手前の確認 (Q-32。2026-10-20)

Claude Code 2.1.304、Linux で測った。

- **通常の起動**: MCP サーバーのカレントディレクトリと `CLAUDE_PROJECT_DIR` は、どちらも Claude Code を起動したディレクトリ。git のルートではなく、サブディレクトリで起動したならサブディレクトリ
- **`claude --worktree <name>`**: worktree はメインのリポジトリの中の `<git のルート>/.claude/worktrees/<name>` に作られ、ブランチは `worktree-<name>`。サーバーのカレントディレクトリと `CLAUDE_PROJECT_DIR` は worktree のルート (サブディレクトリから起動しても worktree のルート)。環境変数 `PWD` だけは起動したディレクトリのまま
- **subagent の worktree** (Agent ツールの `isolation: worktree`): worktree は `<git のルート>/.claude/worktrees/agent-<id>` に作られるが、subagent は親と同じ MCP サーバーのプロセス (同じ pid) を使う。サーバーから見えるのは親のディレクトリ (メインのリポジトリ) で、`roots/list` も親のものだけ、roots の変更の知らせ (`notifications/roots/list_changed`) も来ない。つまりサーバーは subagent の worktree を知る手段が無い。subagent が worktree の Cargo.toml・Cargo.lock を変えても、`project` 引数で worktree を渡されない限り、親のプロジェクトの版で答える
- **`--add-dir <dir>`**: サーバーの環境変数とカレントディレクトリには出ない。`roots/list` の応答には、起動したディレクトリと足したディレクトリの両方が `file://` の URI で入る。ただし、この測定のサーバーは旧来の `initialize` (クライアントは protocolVersion 2025-11-25、capabilities に `roots.listChanged: true`) で接続したもので、本体が話す MCP 2026-07-28 (`initialize` の無い方式) でサーバーから `roots/list` を問い合わせられるか、どう問い合わせるかは確かめていない (要件の R-MCP-3 のとおり MRTR の中でしか送れず、このサーバーは MRTR を使わないので、確かめない。§5 の P4)

計画への含意 (A2 と §5 の P3・P4・P5 で扱う):

- subagent の worktree: サーバーからは見えないので、instructions と各ツールの `project` 引数の説明で「worktree で作業しているなら `project` に作業ディレクトリを渡す」と案内する (P5)。渡された worktree は、既定のプロジェクトの git リポジトリの linked worktree なら既定のプロジェクトと同じに扱う (P3。要件のままでは外になり、`build_external_docs` の記録が要った)
- worktree がメインのリポジトリの中に入れ子になる。確かめること (A2・D4):
  - 既定のプロジェクトの決め方: `.claude` は Cargo.toml を持たないので、直下の子の探索 (R-MCP-3) には当たらない見込み。`members` の glob (`*` や `**`) が `.claude/worktrees/*` に当たるか (cargo と M1 の B5b の展開の両方)
  - cargo の workspace の探索: worktree の中から上へたどると、メインのリポジトリのルートの Cargo.toml に届く。worktree のブランチとメインの checkout で `[workspace]` の有無が違うと、worktree の package が member でない上の workspace を見つけうる (M1 の B5a の `workspace_of` は、その package を単独の package として扱う)
  - 鮮度の鍵とファイルの走査 (R-WS-4): メインのリポジトリのルートの下を走査する処理があれば、入れ子の worktree と target dir を含めない
  - 専用 target dir (R-WS-2): worktree ごとに workspace のルートが違うので、別の target dir になり、初回はコールドビルドになる。GC は最終使用日時で管理する (R-WS-2 のとおり)
- サブディレクトリでの起動: `CLAUDE_PROJECT_DIR` がサブディレクトリになる。そこが member のディレクトリなら M1 の規則で workspace のルートに届くが、Cargo.toml の無いディレクトリ (`src/` や `docs/` など) なら、上へは探さないので `project_not_found` になりうる (R-MCP-3)。A2 で確かめ、変えるかは利用者に聞く
- `claude --worktree` で開いたセッションは、worktree のルートが既定のプロジェクトになるので、今の規則のままで足りる
- `--add-dir` のディレクトリは `roots/list` にしか現れない。`roots/list` は使わない (P4) ので、`--add-dir` のディレクトリは `project` で指す

### 6.2 M2a

**A1 (テストの道具。2026-10-20、Linux、cargo 1.99.1、git 2.56.0)**:

- 置き場所: 道具は `tests/common/` の `isolated.rs` (隔離した環境)・`registry.rs` (偽の crates.io)・`git.rs` (リポジトリと worktree)。cargo と git を起動するテストは、統合テストの `tests/local/` にまとめた (道具のテストは `tests/local/tools.rs`)。テスト用の crate の約束と種類は `tests/fixtures/README.md`
- 隔離した環境 (`Isolated::apply`): `HOME`・`XDG_CONFIG_HOME`・`CARGO_HOME` を一時ディレクトリに向ける。git の設定は、`GIT_CONFIG_NOSYSTEM=1` と空の `GIT_CONFIG_GLOBAL` で読ませない。問い合わせは `GIT_TERMINAL_PROMPT=0`・`GCM_INTERACTIVE=never`・`SSH_ASKPASS_REQUIRE=never` で止める。テストの環境から、`CARGO`・`__CARGO`・`RUSTC`・`RUSTDOC`・`RUSTFLAGS`・`GIT_`・`SSH_ASKPASS` で始まる変数を外す (`cargo test` が渡す `CARGO_*` と、利用者の `CARGO_TARGET_DIR`・`RUSTC_WRAPPER` などを子に渡さない)。`RUSTUP_HOME`・`RUSTUP_TOOLCHAIN`・`PATH` は残す (rustup の proxy が toolchain を見つけるため。rustup は proxy で起動したプロセスに `RUSTUP_HOME` と `RUSTUP_TOOLCHAIN` を渡すので、テストの子の cargo はテストと同じ toolchain になる)
  - 確かめたこと: 隔離した cargo home の `config.toml` の alias が効く。`git config --list --show-origin --show-scope` が、コマンドラインで渡した識別子のほかに何も出さない
  - libgit2 (cargo の既定の git) は `GIT_CONFIG_NOSYSTEM` を見ないので、`/etc/gitconfig` は読みうる。利用者ごとの設定と資格情報は `HOME` を移すことで外れる
  - 製品のコードが起動する cargo (`data::cargo::project_command`) にはこの環境は届かない。製品の経路で外へ出るテストは、サーバーのバイナリを隔離した環境で起動して行う (要る段で決める)
- 偽の crates.io (`FakeRegistry`): 隔離した cargo home の `config.toml` で `[source.crates-io] replace-with` を `sparse+http://127.0.0.1:<port>/index/` に向けると、cargo は http の sparse index から依存を取った (確かめていなかった件。取れた)。要求は `config.json`・index の各 crate・`dl` の `.crate` だけ。Cargo.lock の `source` は crates.io (`registry+https://github.com/rust-lang/crates.io-index`) になるので、テストのプロジェクトは crates.io に依存するプロジェクトと同じ形になる。`.crate` はテストの中で tar と gzip で作り (`<名前>-<版>/` の下に Cargo.toml とファイル)、`cargo check` で依存として展開・コンパイルできた
- worktree: `git worktree add` で `<リポジトリ>/.claude/worktrees/agent-1` に作った linked worktree の `.git` はファイルで、`gitdir: <リポジトリ>/.git/worktrees/agent-1` を持つ (A2 の P3 の判定に使う形)

**A2 (Q-32 の反映。2026-10-20、Linux、cargo 1.99.1、git 2.56.0)**:

- P3 (worktree を既定のプロジェクトと同じに扱う): `DefaultProject::contains` が、既定のプロジェクトの workspace のほかに、同じ git リポジトリの別の作業ツリーの同じ場所 (作業ツリーのルートからの相対パスが同じ) にある workspace を含める。作業ツリーは、上へたどって最初の `.git` で決める。`.git` がディレクトリならそれが共有の git ディレクトリ。ファイルなら linked worktree で、`gitdir:` が指す記録 (`<リポジトリ>/.git/worktrees/<名前>`) の `gitdir` ファイルがその `.git` ファイルを指し返し、`commondir` ファイルが共有の git ディレクトリを指すときだけ認める (写した `.git` ファイルや手で書いたものでは加わらない)。git は起動しない
  - 決定 (P3) は「linked worktree」だったが、実装では「同じリポジトリの別の作業ツリー」とした。`claude --worktree` で開いたセッション (既定のプロジェクトが linked worktree) から、メインの作業ツリーを `project` で指した場合も同じ理由で含める
  - 作業ツリーの別の場所の workspace (worktree の中の、既定のプロジェクトの workspace と違う相対パスのもの) は含めない。メインの作業ツリーでも、既定のプロジェクトの workspace 以外は既定のプロジェクトでないのと同じ
  - 記録のパスは、絶対 (git の既定) と相対 (`worktree.useRelativePaths`) の両方を読む。単体テストは手で書いた記録で、`tests/local/worktree.rs` は `git worktree add` で作った記録で確かめた
- サブディレクトリでの起動 (利用者が決めた。2026-10-20): 決めたディレクトリに Cargo.toml が無く、直下の子にも無ければ、git の作業ツリーのルートまで上へたどり、最初の Cargo.toml の workspace を既定のプロジェクトにする。直下の子の workspace を探すときも、作業ツリーのルートまで上を見る (`crates/` で開くと、`crates/a` を単独の package ではなく、リポジトリの workspace の member として扱う)。git の外では、今までどおり上へ探さない。要件の R-MCP-3 の「上方向には探さない」を改める (§7)
- P5: instructions の版の規則に 1 文、9 つのツールの `project` 引数の説明の末尾に 1 文 (共通の定数 `PROJECT_IN_A_WORKTREE`) を足した。長さの検査 (R-QA-5) は通る
- P8: `project_from` の値を `claude_project_dir` に改めた (3 つのツールの outputSchema の enum も)
- 入れ子の worktree の影響 (cargo 1.99.1 で確かめた):
  - `members = ["*"]` や `["**"]` は、worktree が無くてもリポジトリの `.git` ディレクトリに当たり、cargo が「Cargo.toml が無い」で失敗する。worktree (`.claude/`) が増やす新しい失敗は無い。`crates/*` のような形は当たらない
  - worktree の中で cargo を起動すると、worktree のルートの workspace が使われる (メインのリポジトリの workspace は拾わない)。既定のプロジェクトは `.claude` が Cargo.toml を持たないので、そこを含めない
- README: プロジェクトの決め方の段落と Security の節 (cargo を起動する範囲) に、worktree とサブディレクトリでの起動を書いた

**A3a (設定ファイルの仕組み。2026-10-20)**: A3 は、仕組みと M1 の設定を移す A3a と、M2 の新しい項目を足す A3b に分けた。

- 置き場所: `src/config.rs` (`Settings`)。`logging`・`task` と同じく、どの層からも使う。起動時に 1 回読み (`Settings::load`)、`logging::init`・`Cache::open`・`CallLimit::from_settings` がそこから読む
- 設定ファイル: 既定は `etcetera` の `config_dir` の下の `chatora-rustdoc-mcp/config.toml` (Linux・macOS は `$XDG_CONFIG_HOME`、Windows は `%APPDATA%`)。既定のファイルが無いのは問題にしない。`CHATORA_RUSTDOC_MCP_CONFIG` で指したファイルが読めなければ問題としてログに出す。相対パスなら問題として、既定のファイルを読む (`cache.dir` と同じ考え方)。TOML として読めなければ、ファイル全体を無視して問題にする
- 環境変数で受けられる値の制約 (R-DIST-3 が実装時に決めるとしたもの): 数は 10 進の数字 (前後の空白は許す)、真偽は `true`・`false` (大文字小文字を問わない)、容量は設定ファイルと同じ文字列、パスは文字列そのもの (UTF-8 でなくてよい)。空の環境変数は設定していないものとする (`CLAUDE_PROJECT_DIR` と同じ)。設定ファイルでは TOML の型で書く (数を文字列で書くと誤った値)
- 問題 (読めないファイル、知らないキー、誤った値) は `Settings` にためて、ログのファイルの最初の行 (`start` の記録。R-OBS-1) の後に出す。誤った値の文言は「<キーか環境変数> is <値>, but it must be <期待>, so the default is used」
- このバージョンが読むキーは `KEYS` の一覧で、それ以外は「このバージョンの設定ではない」として無視する (要件の表にあっても、まだ使っていないキーは知らないキーとして知らせる。効かない設定が黙って無視されないため)
- ビルド時の変数 `CHATORA_RUSTDOC_MCP_BUILD_TARGET` を `CHATORA_RUSTDOC_BUILD_TARGET` に改めた。cargo は `rustc-env` の変数を `cargo run`・`cargo test` で起動したプロセスにも渡すので、設定の接頭辞と重なると、テストで知らない設定として報告された
- テストのサーバーは、空の設定ファイルを `CHATORA_RUSTDOC_MCP_CONFIG` で指して起動し、テストを回す人の設定ファイルを読まない
- 途中で、build.rs を書き換えた直後に rust-analyzer の検査と cargo が競合したらしく、build script の古いバイナリが残り、新しい変数が見えなかった。`cargo clean -p chatora-rustdoc-mcp` で直した (このパッケージの生成物 57.7 GiB が消え、作り直した)

**A3b-1 (通信のタイムアウト・展開の上限・dist の無効化。2026-10-20)**: A3b を、すでに動きのある設定をつなぐ A3b-1 と、オフラインモードの A3b-2 に分けた。`build.*`・`cache.*`・`limits.concurrent_builds`・`limits.request_timeout_secs` は、それを使う段 (C3・D・F) で足す (使わないキーを読む一覧に入れると、効かない設定が黙って受け付けられるため)

- `network.connect_timeout_secs`・`network.read_timeout_secs`: `data::Network` に持ち、各取得元 (docs.rs・sparse index・static.crates.io・dist) の `new` に渡す。1 以上の整数の秒
- `limits.extract_max_size` (既定 4GiB): 今ディスクに展開するのは std の dist のアーカイブだけなので、その合計の上限 (M1 の固定の 2GiB) を置き換えた。1 ファイルの上限 (512MiB。メモリで解析する上限) はそのまま。`.crate` の展開 (R-SC-2) は M3
- `std.fetch_from_dist` (既定 true): false なら dist に要求を出さない。手元の nightly は、プロジェクトの toolchain より古くても warnings を付けて使う (R-STD-1)。手元に無ければ (または使えなければ) `toolchain_missing` に導入コマンドを添えて返す (R-STD-4。`DistError::Disabled`)。dist から前に取ってキャッシュにある std も使わない (設定の意味を「dist のものを使わない」と読んだ)

**A3b-2 (オフラインモード `network.offline`。2026-10-20)**:

- `data::Network` に `offline` を持たせ、各取得元 (docs.rs・sparse index・static.crates.io・dist) は要求を送る関数の入口で `Offline` のエラーを返す。cargo の解決 (`Resolver`) には M1 からある `--offline` を渡す
- sparse index: キャッシュにあれば TTL を過ぎていても使い、`stale` に「オフライン」を書く (R-CA-7。通信できなかったときの M1 の扱いと同じ経路)。どの綴りもキャッシュに無ければ `offline_miss`。オフラインの失敗は「index に無い」否定キャッシュに入れない
- docs.rs: キャッシュに無ければ `offline_miss`。オフラインの失敗は失敗の記録 (否定キャッシュ) に残さない (オンラインに戻したときにすぐ取れるように)。別の形式を探す処理でも、見つからないと記録しない
- dist: manifest がキャッシュにあれば古くても使い (M1 の、取れなかったときの経路。warnings に理由を書く)、無ければ `offline_miss` に導入コマンドを添える (R-STD-4)
- static.crates.io (`.crate`): キャッシュに無ければ、docs.rs のビルドの feature が分からない旨の warnings だけで照会を続ける (M1 の扱いのまま)
- `offline_miss` の suggestions (§8 の「キャッシュにあるバージョン」): その crate の docs.rs の JSON がキャッシュにある版を、新しい順に 5 つまで (`name@version`) と、`network.offline` を外す案内。失敗の記録だけの版は数えない
- 古い index を使ったときの provenance (R-CA-7。M1 から抜けていた): `index_fetched_at` (UTC の RFC 3339、秒まで) を、TTL を過ぎた index で版を決めたときだけ出す。`get_item` と、その経路を使う一覧のツール (mcp の `look_up_in` で足す)、`resolve_version`。名前は要件に無かったので決めた。オフラインのときと、通信できずに古いものを使ったときの両方
- 古い index に無い版を求められたとき: M1 は `version_not_found` (「crates.io に無い」) を返していたが、古い index には新しい版が抜けうるので、オフラインなら `offline_miss` (キャッシュにある版を添える)、通信できなかったなら `network_error` (`retryable: true`) にした。統合テストで見つけた

**A4a (試験用のツールと公開の確認。2026-10-20)**: A4 を、ここで済むもの (A4a) と、実データのテストを 3 つの OS で回して結果を見てから扱いを決めるもの (A4b) に分けた

- 試験用の `sleep` ツール: debug ビルドで、環境変数 `CHATORA_RUSTDOC_TEST_TOOLS=1` があるときだけ出す (release ビルドには今までどおり無い)。統合テストはサーバーを起動するときにこの変数を渡す。`cargo install --debug` で入れたバイナリは、変数が無ければ出さない。変数は設定ではないので、設定の接頭辞 (`CHATORA_RUSTDOC_MCP_`) の外の名前にした。変数を空にしたサーバーが `sleep` を出さず、呼ぶと `-32602` になることのテストを足した
- CI に `package` の job を足し、`cargo publish --dry-run --locked` を回す (R-DIST-1)。公開される package のファイルは `src/`・ライセンス・README・build.rs・Cargo.toml・Cargo.lock (`cargo package --list` で確かめた)

**A4b (実データのテストを 3 つの OS で。2026-10-20)**: CI の `testdata` の job を 3 つの OS の matrix にした (キャッシュの鍵に OS を入れ、テストデータの取得の段は bash で回す)。

- `testdata` (docs.rs の JSON の読み込み) と `std_dist` (std) は、macOS・Windows でも通った
- `real_data` は、macOS (`aarch64-apple-darwin`) と Windows (`x86_64-pc-windows-msvc`) で、4 つのテストがすべて `no_target_json` で落ちた (macOS で 14 件、Windows で 13 件の応答。ほかの原因は無かった)。照会の多くが target を省いてホストの target を引き、docs.rs が新しいリリースを Linux の target でしか作らないため。M1 の「実質的に Linux 向け」の制限そのもの
- 利用者が決めた: D7 (ローカルビルド) まで `real_data` は Linux だけで回し、D7 の終わりの印に「3 つの OS で `real_data` が通る」を足した

### 6.3 M2b

**B1 (子の起動と後始末。2026-10-20、Linux で確かめ、macOS・Windows は CI)**:

- 置き場所: `data::child`。cargo (`data::cargo::run`) と、toolchain の短いコマンド (`data::tool::output`。`rustc -vV`・`rustup run`) の子は、すべてここから起動する
- 起動するスレッド: 子は、プロセスと同じだけ生きる専用のスレッド (`child-starter`) から起動する。呼び出し側の Tokio の runtime に入って (`Handle::enter`) `spawn` し、パイプはその runtime に登録する。`PR_SET_PDEATHSIG` は子を起動したスレッドの終了で発火し、Tokio の blocking のスレッドは使われないと終わるため (Q-29 の案のとおり)
- グループ: Unix では新しいセッション (`setsid`。プロセスグループも新しくなり、制御端末が外れる。M1 から)、Linux ではさらに `PR_SET_PDEATHSIG` に SIGKILL を設定し、設定の後に `getppid` がサーバーでなければ起動を失敗にする (設定の前にサーバーが死んでいた場合)。Windows では Job Object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) に入れる。Job Object は windows-sys で自前に作る (利用者が決めた。tokio が使う 0.61.2 と同じ版で、新しい crate は増えない。子を Job に入れる前のごく短い間に孫が起動されると、Job から漏れる隙間が残る)
- 止め方: `Child` を落とすと (取り消し・30 分の上限・正常な終了の後のいずれも) グループごと止める。Unix ではプロセスグループに SIGKILL、Windows では `TerminateJobObject`。正常に終わった後も、残った孫を止める
- サーバーの終了: stdin を包み、EOF か読み取りの失敗で `stop_all` を呼ぶ (rmcp は EOF の後に処理中の要求を最大 5 秒待つが、その間も子を動かさない)。SIGINT・SIGTERM (Windows は Ctrl+C) では、`stop_all` の後に rmcp の `cancellation_token` でサーバーを止める。`stop_all` の後は新しい子を起動しない (統合テストで見つけた: 止めた `cargo metadata` の代わりに Cargo.lock を直接読む経路が `cargo tree` を起動し、それが rmcp の 5 秒の待ちの間ハングして残った)
- テスト: 単体テスト (孫を起動する子を落とすと孫も止まる。3 つの OS。テストが本当に検出することを、止める処理を外して落ちることで確かめた。Linux で子がセッションの長であること)。統合テスト (Unix): `build.rustc` に孫を起動してハングする rustc を置き、`list_crates` で cargo を起動してから、stdin を閉じると 2 秒以内に cargo・rustc・孫が止まり、別の cargo も起動されない。SIGTERM でも同じで、サーバーも終わる。Linux では、サーバーを SIGKILL しても cargo が止まる (`PR_SET_PDEATHSIG`。cargo が起動した rustc と孫は残る。C4 で起動時に片付ける)
- Windows の Job Object と孫の扱いは、手元でコンパイルできない (依存の C のコードのため) ので、CI で確かめる

**B2 (ロック待ちの検出。2026-10-20、cargo 1.99.1)**:

- cargo は、ほかの cargo がパッケージキャッシュのロックを持っていると、stderr に `Blocking waiting for file lock on package cache` (行頭に空白が付く) を出して待つ (隔離した `CARGO_HOME` の `.package-cache` を `flock` で握って `cargo metadata` で確かめた)。ロックの対象で `on build directory` などに変わるので、照合は `Blocking waiting for file lock` までにした
- `data::child::Child::output_watching` で stderr を 1 行ずつ読みながら待ち、`data::cargo::run_reporting` が `Activity` (`Running`・`WaitingForLock`) の変わり目を知らせる。ロック待ちの行の後にほかの行が出たら、ロックが取れて動いているとみなして `Running` に戻す。応答の `building_reason: waiting_for_lock` につなぐのは C3・D3
- テスト: std の `File::lock` でパッケージキャッシュのロックを握り (cargo と同じく Unix は `flock`、Windows は `LockFileEx`)、本物の cargo が `WaitingForLock` を知らせ、ロックを外すと正常に終わることを 3 つの OS で確かめる

**B3a (cargo の設定の読み方。2026-10-20)**: B3 を、設定を読む部品と `cargo metadata` の鍵 (B3a) と、プロジェクトの `build.target` を照会に使うこと (B3b) に分けた

- 合成の規則は、nightly の cargo 1.101 の `cargo -Zunstable-options config get build.target` を物差しにして、隔離した環境で確かめた (stable の cargo 1.99.1 では設定の値を出せない。`include` の効き目は、取り込んだファイルの `[alias]` が効くかで stable でも確かめた)。cargo 自身に読ませる手段は、stable には無い
  - 読むファイル: cargo を起動するディレクトリとその上のすべての `.cargo/config` (`config.toml` と両方あれば拡張子の無いほうを使い、cargo は警告を出す) か `.cargo/config.toml`、と `$CARGO_HOME/config(.toml)`。`$CARGO_HOME` が上のどれかの `.cargo` と同じなら、そこで 1 回だけ読む
  - 優先度 (低い順): `$CARGO_HOME`、上のディレクトリを上から順に、環境変数 `CARGO_BUILD_TARGET`。`include` が指すファイルは、指したファイルのすぐ下 (入れ子はさらに下)
  - 合成: 文字列は優先度の高いものが勝つ。配列は低い順につなぐ。同じキーに文字列と配列が混ざると cargo は設定を読めずに失敗する (`cargo version` 以外のコマンド)。`CARGO_BUILD_TARGET` は文字列を置き換え、配列なら末尾に足す
  - `include`: パスか `{ path, optional }` の表の配列で、パスは指したファイルからの相対。文字列で書くと失敗する (stable 1.99.1 でも)。`optional` でないファイルが無くても失敗する
  - member のディレクトリの設定が効かないことは、cargo を workspace のルートで起動している (M1 の B5b) ので、ルートから上だけを読めば足りる
- `data::cargo_config::read`: 上の規則で `build.target` (文字列か配列) と `[unstable] build-std` の有無を求め、読みうるファイルの一覧 (無いものと、`include` が指すものを含む) を返す。cargo が失敗する場合はエラーにする
- `cargo metadata` の鍵 (M1 の B5b の残り): `include` が指すファイルを足した。取り込んだファイルを変えると結果を使い直さないことをテストで確かめた

**B3b (プロジェクトの `build.target` を照会の target に。2026-10-20)**:

- `mcp::tool::project::default_target`: `target` を省いた照会の target を、プロジェクト (`project` 引数の、なければ既定のプロジェクトの workspace) の cargo の設定の `build.target` (B3a の読み方。環境変数 `CARGO_BUILD_TARGET` はサーバーの環境のもの) にする。配列なら先頭を使い、ほかの target を挙げて `target` で選べることを warnings に書く。プロジェクトが無い・設定が無いときはホストの target (代用のときはその旨の warnings。M1 から)
- 使う所: `get_item` (docs.rs と std)・`crate_overview`・`list_*` (`get_item` の経路)・`search_items` (1 crate・std・依存の横断)・`list_crates`・`resolve_version`。`enabled_in_project` の評価と `cargo tree --target` は、照会の target と同じものを使う (M1 から)
- 設定が読めない (cargo も失敗する) ときと、`build.target` が target の名前でないときは、ホストの target を使い、warnings に書く
- 独自の target (`.json`・`[unstable] build-std`) は、利用者が決めて D7 に回した。それまでは今までどおりホストの target を使い、その旨を warnings に書く (今より悪くはならない)
- テスト: `build.target` を `[Linux の target, wasm32]` にしたプロジェクトで、target を省いた `get_item` が Linux の target の JSON をキャッシュから引き、warnings に複数の target が出る (どの OS でも通信しない)。`.json` の target のプロジェクトで、`list_crates` の warnings に独自の target の旨が出る

**B4 (プロジェクトの文脈の残り。2026-10-20)**:

- 暗黙の member (M1 の B5a・B5b): ルートの package の path 依存のように、`members` に無くても member になる package は、cargo を起動しない判定 (`workspace_of`) では単独の package になる。そこを既定のプロジェクトとして版を引くと、Cargo.lock が無いので `lockfile_outdated` になっていた。Cargo.lock が無いときは `cargo metadata` を起動し、`workspace_root` がその上なら、cargo の答えのルートの Cargo.lock と入力のファイルを使う。`project` 引数で既定のプロジェクトの外から指された暗黙の member は、cargo を起動しない規則 (R-MCP-3) のまま外の扱い
- 利用者の `net.offline`: cargo の設定 (B3a の読み方。ファイルの `net.offline`、環境変数 `CARGO_NET_OFFLINE` が勝つ) でオフラインなら、`no matching package named` を `offline_miss` に数える (`cargo metadata`・`cargo tree` の両方)
- 合流: 同じ workspace の `resolve` は、workspace ごとのロックを持って順に行う。先のものが結果を残すので、後のものは cargo を起動しない。`features` (`cargo tree`) は、残した結果の feature の表のロックを持ったまま起動する。4 つ同時の `resolve` と 2 つ同時の `features` が、それぞれ 1 つの結果を共有することをテストで確かめた (ロックを外すと落ちることも確かめた)
- 失敗の保持: `cargo metadata` が失敗して Cargo.lock を直接読んだ結果と、`cargo tree` の失敗は、入力が同じ間、30 秒だけ cargo を起動せずに返す (R-WS-7 の一時的な失敗の TTL)。成功は入力が同じ間ずっと
- 取得中の印: `cargo::Activity` に `Fetching` を足し、stderr の状態の語 `Updating`・`Downloading`・`Downloaded` で知らせる (`building_reason: fetching` につなぐのは C3)。`Locking` は通信しないこともあるので数えない

**B5 (git の取得の失敗の文言。2026-10-20、cargo 1.99.1・git 2.56.0)**:

- 隔離した環境で、`file://` の bare リポジトリと 401 を返すローカルの HTTP サーバーを相手に、libgit2 (cargo の既定) と git の CLI (`net.git-fetch-with-cli`) の両方で、`cargo metadata`・`cargo fetch`・`cargo tree` の失敗の文言を集めた (`LC_ALL=C`・色なし)
  - 無いブランチ: libgit2 は `cannot locate remote-tracking branch`、CLI は `couldn't find remote ref refs/heads/…`
  - 無いタグ: libgit2 は `failed to find tag` と `reference 'refs/remotes/origin/tags/…' not found`、CLI は `couldn't find remote ref refs/tags/…`
  - 無い rev (40 桁): どちらも `revspec '…' not found` (CLI でも cargo が libgit2 で引く)
  - 無いリポジトリ: libgit2 は `failed to resolve path`、CLI は `does not appear to be a git repository`
  - 認証: libgit2 は `failed to authenticate when downloading repository`、CLI は `could not read Username for '…': terminal prompts disabled`
- M1 の照合は libgit2 の無いタグを拾えず (`Other` になっていた)、`failed to find tag` と `failed to find branch` (cargo 自身の行。後ろにバッククォートで囲んだ名前が続く) を足した。R-WS-7 の照合の一覧に足す (§7)
- テスト (`tests/local/git_failures.rs`): 5 つの失敗 × 2 つの取り方 × 3 つのコマンドの 30 通りで `classify` が `fetch_failed` の理由を当てることと、同じリポジトリのタグ `v1` は取れること (対照) を、3 つの OS で確かめる。`classify` を公開した (統合テストから呼ぶため)
- cargo は、git の CLI の失敗と、一時的かもしれない要求の失敗を、`net.retry` (既定 3) の回数だけ間を置いて試し直す。無いブランチのような決定的な失敗も試し直すので、30 通りで合計 170 秒 (1 回あたり平均約 6 秒) かかった。テストは `net.retry = 0` にした。製品は利用者の設定のまま (C3 の時間の上限と `status: building` がこの待ちを受け止める)

**B6 (`.crate` を手元の cargo のキャッシュから読む。2026-10-20)**:

- `StaticCrates` は、`.crate` が自分のキャッシュに無ければ、static.crates.io に要求する前に `$CARGO_HOME/registry/cache/` の crates.io のレジストリ (`index.crates.io-…`・`github.com-…`。名前の末尾のハッシュは cargo の版で変わりうる) から `<name>-<version>.crate` を探し、crates.io の index の checksum と合えば使って自分のキャッシュに入れる。合わなければ取りに行く。オフラインでも使える。`$CARGO_HOME` の求め方は `data::cargo_home` にまとめ、`Resolver` と共有した
- static.crates.io からの取得がダウンロード数に数えられるか (レビュアーが挙げた件): crates.io は 2024-03 から static.crates.io の CDN のログでダウンロードを数え ([crates.io: Download changes](https://blog.rust-lang.org/2024/03/11/crates-io-download-changes/))、2025-10-29 に cargo の User-Agent の要求だけを数えるようにした ([rust-lang/crates.io#12210](https://github.com/rust-lang/crates.io/pull/12210))。このサーバーは自分の User-Agent (`chatora-rustdoc-mcp/<版> (+<リポジトリ>)`。R-NW-1) を送るので、数えられない。README の通信の節に書いた
- テスト: cargo のキャッシュに置いた `.crate` を、オフラインの (要求すればエラーになる) クライアントが読めること、checksum の合わないものは使わないこと

**B7 (ビルドの範囲の判定。2026-10-20、cargo 1.99.1)**:

- `--target` を付けない形での M0 の再確認 (Q-31): M0 の合成の workspace とこのリポジトリで、M0 と同じ結論になった。`cargo tree -e normal` の出力は `--target <host>` の有無で同じ。`cargo rustdoc -p app --lib` の feature の組も M0 と同じで、resolver 2 では `--target` が無くても host 用と target 用を別の unit にする。`-p` で依存を名指しすると、resolver 2 では dev だけ・build だけ・ほかの target だけ・proc-macro の下だけの依存で panic し、resolver 1 ではすべて通る (B3・B7-9)。`RUSTC_BOOTSTRAP` の値を固定すれば、依存を 2 つ文書化した後の member の作り直しは 1 unit (B1)。dep-info は `<target dir>/doc/<crate>.d` に出て、`#[cfg(docsrs)]` のときだけ読むファイルも載る (B5。`--target` を付けると `<triple>/doc/`)
- `cargo tree` の形:
  - build と dev の辺は、`--prefix indent` の見出し (`[build-dependencies]`・`[dev-dependencies]`) にしか出ない。`--prefix depth` では出ないので、範囲の判定は indent の形を読む (`data::cargo_tree`)。見出しは、その子を持つ行と同じ深さの字下げで出る
  - `{p}` の取得元: crates.io は無し、path は `(<ディレクトリ>)`、git は `(<URL>?<参照>#<commit の先頭 8 桁>)`、別のレジストリは ``(registry `<設定での名前>`)``。名前と版が同じで取得元だけ違う package は実際にある (同じリポジトリの branch と tag) ので、`cargo metadata` の package id への対応付けは、名前と版で 1 つに決まらなければ取得元でも照合し、それでも決まらなければ範囲を判定できないものとする
- member の根が 2 つ出る件: proc-macro (member を含む) や build-dependency として使われる member は、`--workspace` の木で target 用と host 用の 2 つの根になり、出力からはどちらがどちらか分からない (並びは cargo が node を足した順で、dedupe の `(*)` もその順で決まる)。位置だけで判定すると host 用の部分木を target 用と誤る。その member だけ `-i <id> --no-dedupe` の逆向きの木を取り、根の下に proc-macro の行か `[build-dependencies]` の見出しがあれば host 用とする (target 用の node をたどった先はどちらにも行き着かない)。逆向きの木の根の並びと feature の組は順向きの木と同じだったので、順に対応付け、合わなければ範囲を判定できないものとする。`--no-dedupe` を順向きの木に使わないのは、このリポジトリでも 476 行が 6,545 行 (380 KB) になったため
- 名指しできる依存 (resolver 2 以上) の境目を測った: build の依存の下の proc-macro は名指しでき、build の依存の lib、dev の依存の先の依存、dev の feature でだけ有効になる optional の依存は panic した。そこで、dev を含まない解決 (`-e normal,build`) で target 用に出る package と proc-macro を名指しできるものとした。proc-macro の member の下の依存は、cargo は名指しできる (要求した proc-macro の member の依存の feature を target 用にも有効にする) が、`cargo check --all-targets` は host 用にしか作らないので、安全側に倒して名指しできないものとした。多くは crates.io の crate (syn など) で docs.rs に JSON がある
- `data::build_scope`: `Resolver::scope(workspace, target)` が `BuildScope` を返し、package id ごとに `ForTarget`・`ForHost`・`Unnameable` (`build_unsupported`)・`NotBuilt` を答える。`cargo tree` の起動は 2 回 (member の根が 2 つ出るときは 3 回)。`cargo metadata` の結果と一緒に target ごとに保持し、feature の組と同じく合流と失敗の保持をする。resolver の版は、ルートの Cargo.toml の `resolver` (`[workspace]` か `[package]`)、無ければルートの package の edition (2021 以降なら 2 以上)、仮想 workspace で無ければ 1 として読む。照会のツールにつなぐのは D7
- テスト (`tests/local/build_scope.rs`、fixture は `tests/fixtures/build-scope/`): resolver 1 と 2 の workspace (proc-macro の member が使う member、build の依存の member、OS ごとの依存、dev だけ・build だけ・proc-macro の下だけの依存) で、範囲 (package と host 用か target 用か) が `cargo check --workspace --all-targets --target <host>` の作る unit と一致すること、名指しできるかの判定が `cargo rustdoc -p` の panic の有無と一致すること (proc-macro の member の下の依存だけは上のとおり安全側に違う)、wasm32 では wasm32 だけの依存が範囲に入ることを確かめる。ライブラリが cargo をテストのプロセスの環境で起動するので、テストのバイナリをこのテストだけ隔離した環境の子として起動し直す (`isolated::in_isolated_child`)
- feature の組 (`data::features`) の、同じ package が host 用と target 用の両方の根に出るときの選び方 (M1 の B5d) は変えていない。上の逆向きの木で正確にできるが、D3 で feature の組を見直すときに合わせる

**B8 (版を明示しても、プロジェクトと同じ crates.io の版なら `enabled_in_project`。2026-10-20)**:

- 版を引数で決めた照会 (完全一致・semver 要件・`latest`) でも、プロジェクト (`project` 引数の、なければ既定のプロジェクト) がその名前の crates.io の package を持てば、解決した版と照合する。同じ版で、Cargo.lock の checksum が sparse index と合えば、プロジェクトの build で `enabled_in_project`・`enabled_in_dev_only` を出し、provenance に `project_from` を載せる。`resolved_from` は `explicit` などのまま。全ツールが `get_item` の解決を通るので、`crate_overview`・`list_*`・`search_items` (1 crate) にも効く
- 同じ版だが checksum が合わない (vendor や patch) ときは、出さずに warnings に書く。プロジェクトの取得元のものを返すのは D7 (R-VR-1 の残り)
- プロジェクトが無い、`cargo metadata` が失敗した、候補が複数で決まらないときは、今までどおりプロジェクトなしで続ける (版を明示した照会は止めない。R-MCP-3)
- 照合の候補があるときは、版を明示していてもキャッシュだけで済ませず sparse index を読む (checksum の照合のため。オフラインでは手元の index を使う)。候補が無ければ今までどおりキャッシュだけで返す
- テスト (`tests/stdio.rs`): crates.io の `demo` 0.1.0 を vendor のディレクトリから読むプロジェクト (Cargo.lock の checksum を、テストのキャッシュの `.crate` の sha256 にした) で、オフラインのサーバーに `demo@0.1.0` を引かせると `enabled_in_project` と `project_from` が付き、プロジェクトの無いサーバーでは付かない。照合を外すと落ちることを確かめた
- CI で見つけた B1 のテストの揺れ: SIGTERM の後に止めた cargo を、サーバーが回収するまでの間 zombie として残り、`kill -0` が成功して「まだ動いている」と数えていた (B7 の CI の Linux)。`ps` の状態で zombie を止まったものとして数えるようにした。あわせて、pid のファイルを中身が書かれる前に読んで空の pid を調べ、確かめずに通ることがありえたので、中身を待つようにした

### 6.4 M2c

**C1 (プロセスをまたぐロックの細部。2026-10-20)**:

- 置き場所: キャッシュのディレクトリの `locks/` (利用者が決めた。要件の案は state の `locks/`)。ロックが守るのはキャッシュの項目 (docs.rs の JSON・派生物・target dir) とビルドの枠なので、キャッシュを共有するプロセスどうしなら、state の場所 (`XDG_STATE_HOME` など) が違っても排他が効く。GC と `cache clear` は `locks/` を飛ばす (R-CA-4 のとおりロックファイルは消さない)。§7 で R-CA-4・Q-29・R-DIST-3 の記述を改める
- 名前: 鍵 (キャッシュの中の相対パスなど、どのプロセスでも同じに作れる文字列) の SHA-256 の先頭 32 桁 + `.lock` を、`locks/` の直下に置く。名前に形式の版は入れない (版の違うサーバーどうしも排他する)。空のファイルが鍵ごとに 1 つ増え、消さない
- 取る順: 鍵 → target dir → ビルドの枠 (要件の案のまま)。target dir は、ビルドが共有ロック、GC と切り替えた古い target dir の削除が排他ロック (案のまま)
- 版の違うサーバーの記録: 形式の版は、記録の中身ではなくパス (ディレクトリかファイル名の `v<版>`) に入れる。中身に入れると、互いに相手の記録を無いものとみなして書き直し合う。古い版の記録は、キャッシュの下なら GC、state の R-WS-6 の記録は書いた版のサーバーが消し、読めない記録には触らない (案のまま)。M1 からの記録 (JSON のダイジェストの `.sha256` は 16 進だけで形式が変わらない、否定キャッシュ) は、形式を変える段で版を付ける
- 部品 `data::process_lock`: std の `File::lock`・`lock_shared`・`try_lock` (Unix は `flock`、Windows は `LockFileEx`)。待つときは `try_lock` を 5 ms から 250 ms まで間隔を伸ばして繰り返す (future を落とせば待ちをやめられる。ブロックする呼び出しは取り消せないため)。Unix では `O_NOFOLLOW` と 0600 で開く。std はファイルを close-on-exec で開くので、cargo とその子はロックを持たない。キャッシュにつなぐのは C2
- OS のロックの挙動 (単体テスト。3 つの OS の CI で確かめる): 同じプロセスでも開いたファイルが別なら排他する (`flock` は open file description、`LockFileEx` はハンドルごと)。共有ロックは並び、排他ロックを阻む。もう 1 つのプロセス (テストのバイナリを起動し直す) が持つ間は取れず、そのプロセスを殺すと取れる (Windows は OS が資源を得しだい外すとされるので、10 秒まで待つ)
- 見つけたこと: ほかのスレッドが子を起動している間に手放したロックは、少し遅れて外れる (子は exec するまで親の fd の複製を持ち、`flock` は open file description に付くため)。並列のテストで、手放した直後の `try_lock` が失敗して分かった。待つ側は試し直すので害は無く、テストは短い間試し直すようにした
- NFS 上の `flock` は確かめられないので、キャッシュがロックを取り始める C2 で README に書く

**C2a (キャッシュのロックをプロセスをまたぐものに。2026-10-21)**: C2 を、OS のロックをキャッシュにつなぐ C2a と、取得の共有と取り消し (C2b) に分けた

- `Cache::lock`・`lock_blocking` は、プロセスの中の順番を取った後に、そのパスの OS のロック (鍵はキャッシュの中の相対パスを `/` でつないだもの) を取る。プロセスの中の 1 つのタスクだけが OS のロックを待つ。OS がロックできないとき (ロックファイルを作れないなど) は、ログに書いてプロセスの中の排他だけで続ける (失うのは二重の取得だけで、書くものは原子的な置き換えなので壊れない)
- ロックを取ってからキャッシュを確かめ直すこと (R-CA-4): M1 からの 7 か所 (docs.rs の JSON とアーカイブ、`.crate`、sparse index、std の展開と要約、横断の検索の読み込み) は、どれもロックの後に確かめ直していたので、変えずに済んだ
- テスト: テストのバイナリを子として 2 つ起動し (`test_process`)、同じ `.crate` を遅いローカルのサーバーから同時に取らせると要求が 1 回、同じ JSON を同時にアーカイブさせると解析が 1 回で、どちらも失敗しない (Windows で相手が mmap しているアーカイブを置き換えにいかない)。OS のロックを外すと、どちらも 2 回になって落ちることを確かめた。libtest は `--test-threads=1` のとき、テスト名の行の後に改行せずテストの出力を続けるので、子には付けない
- README のキャッシュの節に、`locks/` のロックと NFS では効かないことがある旨を書いた

**C2b (取得の共有と取り消し。2026-10-21)**:

- `data::flight::Flights`: 同じ鍵の呼び出しは、1 つの spawn したタスクの結果を待つ。待つ側はタスクへの `Arc` を持ち、最後の 1 つが落ちたら (取り消しで tool のタスクが abort されたら) タスクを abort する。結果を受け取らずにタスクが終わったら (panic) `None`。結果を共有するので型は `Clone` (docs.rs の JSON は `Result<Arc<Loaded>, Arc<LoadError>>` で、JSON を複製しない)。タスクには、始めた呼び出しの `Fetches` (R-OBS-1) とログの span を渡す
- docs.rs の JSON (`data::shared_docs_rs_json`): 取得のタスクがその crate のディレクトリのロック (プロセスの中と OS) を取ってから `docs_rs_json` を行い、キャッシュを確かめ直す。get_item と横断の検索は、(1) ロックなしでアーカイブを開く、(2) 共有の取得を待つ、(3) ディレクトリのロックを取ってアーカイブを確かめ直し、無ければ作る、の順に変えた。M1 は (1)〜(3) を通してロックを持っていたので、後の呼び出しは取得ではなくロックを待ち、先の呼び出しが取り消されると取り直していた
- テスト: `Flights` の単体テスト (同時の 3 つで 1 回、始めた呼び出しを取り消しても残りが結果を受け、作り直さない、すべて取り消すとタスクが止まる、panic は `None` で次は作り直す)。遅いローカルの docs.rs で、始めた呼び出しを取り消しても、もう 1 つが JSON を受け、要求は 1 回
- 共有にしたのは docs.rs の JSON だけ。`.crate` と sparse index は小さく速いので、取り消しで取り直しても費用が小さい。std の配布物 (数十 MB) と、`cargo metadata` の初回の取得、アーカイブの作成は、時間の上限で裏に回す C3 でまとめて扱う
- C3 に回す件 (C3 の下調べで分かった): 取り消しで tool のタスクが abort されると、アーカイブの作成 (blocking のスレッド) は最後まで走るのに、ディレクトリのロックは先に外れるので、別の呼び出しが同じ crate の解析を二重に始めうる (M1 から。書き込みは原子的なので壊れない)。C3 で索引の作成を照会から分けて裏で続けるときに、ロックを作成の側に持たせる

**C2c (C1・C2a のレビューの指摘の手当て。2026-10-21)**: 前提を渡さないレビュー (サブエージェント) の指摘のうち、利用者が先に直すと決めたもの

- blocking のスレッドでのロックの待ち: `File::lock` で OS の中で待っていたので、ロックを持つ別のプロセスが止まると (止めたセッション、デバッガ)、取り消せないスレッドが積もり、tokio の runtime の drop が blocking のタスクを無期限に待つのでサーバーも終わらなかった。`process_lock::lock_blocking` を上限付きの試し直しにし、`Cache::lock_blocking` は 1 分で諦めてログに書き、排他なしで続ける (失うのは二重の作業だけ)。`main` は `shutdown_timeout` (1 秒) で runtime を閉じる
- `a_lock_goes_with_its_process` は、`RUST_TEST_THREADS=1` か CPU が 1 つだと libtest がテスト名の行に子の出力を続けて書くので、`line == "locked"` に当たらず 600 秒待って落ちていた (C2a の `--test-threads=1` を外す手当てでは足りなかった)。`ends_with` で照合し、`RUST_TEST_THREADS=1` で通ることを確かめた。CI の各 job に `timeout-minutes` (check 30・testdata 45・ほか 15) を足した (既定は 6 時間)
- std の片付け (`remove_unused`) がロックを取らず、別のプロセスが使い始めた nightly を消しうり、Windows では mmap された archive だけが残る半端な状態になりえた。使ったことの印 (`used`) をその nightly のディレクトリのロックの下で書き、片付けは候補ごとにロックを待たずに試して (`Cache::try_lock`)、取れなければ使用中として残し、取れたら確かめ直してから、完了の印を先に消して消す。残る隙間: 照会がディレクトリのロックを手放してから印を書くまでの間に、別のプロセスの片付けが入りうる (7 日使われていない nightly を使い始めた瞬間だけ)
- ロックの名前: 版をまたいで共有するのに作り方を固定するテストが無かった。鍵に種類の前置き (`cache:` + キャッシュの中のパス) を付け、将来の target dir やビルドの枠 (共有ロックがありうる) のロックと同じファイルで混ざらないようにし、名前と鍵を固定の値で確かめるテストを足した。C2a の名前から変わったが、公開前なので版の違うサーバーは無い
- 細かいもの: `cache.rs` の冒頭の古い記述、`/nonexistent` を使っていたロックのテスト (一時ディレクトリに)、ロックできないときの warn は最初の 1 回だけ (後は debug)
- C3 に回したもの: 失敗が覚えられない取得 (std の要約、sparse index、`.crate`) は、通信先が応答しないと 1 回で約 30 秒かかり、ロックを待っていた各プロセスが順に同じ失敗を繰り返す (セッションの数だけ直列に重なる)。docs.rs の JSON は一時的な失敗を覚えるので当たらない。時間の上限を入れる C3 で、短い間失敗を覚えることと一緒に扱う
- 記録に残して直さないもの: 試し直しで待つ側が負け続けうる公平性 (OS の中で待つ側とプロセスの中の次の持ち主が先に取る)、NFS や SMB で `try_lock` がワーカーのスレッドを止めうること、`locks/` が増え続けること (消さない決まり。綴りの候補・nightly の日付ごとに増える)、UID の違うプロセスの共有 (0600 のロックファイルを開けず、排他なしになる。キャッシュの書き込みもそもそも壊れる)

**C3a (1 つの crate の照会の時間の上限と `status: building`。2026-10-21)**: C3 を、docs.rs の crate の時間の上限と応答の形 (C3a)、進捗の通知 (C3b)、std の配布物・`cargo metadata` の初回の取得・失敗を覚えない取得・ビルド中のカーソル (C3c) に分けた

- 設定 `limits.request_timeout_secs` (既定 30 秒) を足した。呼び出しが裏で続く仕事を待つのは、その 3 分の 2 (既定 20 秒。M1 の横断の検索の 20 秒と同じ) まで。残りは版の解決など仕事の前後に使う。期限は呼び出しの始め (ツールの `execute`) から数える
- docs.rs の crate は、取得からアーカイブの作成までを 1 つの共有の仕事にした (`query::load::docs_rs_archive`)。C2b の JSON だけの共有はやめた。get_item の `recover` にあった取得し直し (別の形式、壊れた JSON) も仕事の中に移した。get_item と横断の検索は、(1) ロックなしでアーカイブを開く、(2) 仕事を期限まで待つ、(3) アーカイブで引く、の順になった。横断の検索も取得し直しをするようになった
- 期限を過ぎたら仕事を切り離す (`Flights::join` が `Joined::Going` を返す)。仕事への参照を持つタスクを終わるまで残すので、待つ側がすべて取り消されても止まらない。応答は `provenance`・`status: building`・`building_reason`・`retry_after_secs` (10)・`warnings`・`hints` の順にした
  - `provenance` は source・crate・version・resolved_from・project_from・index_fetched_at・target
  - `building_reason` は `fetching` か `indexing`。仕事が今の段を `Stage` に書く
  - path が他の crate に続くとき (R-PR-4) も、続く先の crate が期限に間に合わなければ、その crate の `status: building` を返す。呼び直せば続く先で引ける
- 横断の検索は、待つ期限を設定から取る。取得中の crate の `sources` の `building_reason` には、`fetching` か `indexing` を入れる
- アーカイブの二重の解析 (C2b から回した件): ディレクトリのロックを blocking の作業に渡し、作業が終わってから外すようにした。仕事のタスクが abort されてもロックは外れない
- outputSchema: 1 つの crate を引く 7 つのツールに `status` (`building`)・`building_reason` (R-OUT-9 の 5 つの値)・`retry_after_secs` を足した。必須は `provenance` だけにした (ルートに `anyOf` を置かないため。R-OUT-1)。横断の検索の `sources` の `building_reason` も同じ 5 つの値にした
- テストの口: debug ビルドでテスト用のツールが有効なとき (`CHATORA_RUSTDOC_TEST_TOOLS=1`) だけ、`CHATORA_RUSTDOC_TEST_DOCS_RS_URL` で docs.rs の URL を差し替えられる (設定ではない)
- テスト:
  - `Flights`: 期限で `Going` と今の段を返す。待つ側をすべて取り消しても仕事は続き、後の呼び出しが結果を受ける
  - `docs_rs_archive`: 始めた呼び出しを取り消しても続く。期限を過ぎても続き、要求は 1 回。ロックは blocking の作業が終わるまで持つ
  - stdio: 遅い docs.rs で、1 回目の呼び出しは `status: building` で、outputSchema を通る。後の呼び出しは答えを受け、要求は 1 回
- `AbortOnDrop::detach` は使われなくなったので消した

### 6.5 M2d

(まだ無い)

### 6.6 M2e

(まだ無い)

### 6.7 M2f

(まだ無い)

## 7. M2 の終わり方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を次の版にし、§14 に経緯を書く。少なくとも次を入れる
   - R-MCP-3 と Q-32: §6.1 の実測 (通常の起動・`claude --worktree`・subagent の worktree・`--add-dir` で、サーバーが受け取るディレクトリと worktree の置き場所)、§5 の P3 (linked worktree を既定と同じに扱う)・P4・P5 の決定と A2 で確かめたこと (サブディレクトリでの起動、入れ子の worktree の影響)。R-MCP-3 の「M2 の前に確かめる (Q-32)」の文を改め、Q-32 を §11.1 (解決したもの) に移す
   - §7.3 の `project_from` の値 (P8)
   - R-MCP-3 のサブディレクトリでの起動 (git の作業ツリーの中で上へ探す。直下の子の workspace も作業ツリーのルートまで上を見る。§6.2 の A2)
   - Q-29・Q-31 の答え (§4 の各行)
   - R-WS-7 の照合の一覧: git の無いタグとブランチの `failed to find tag`・`failed to find branch` (B5)。cargo に `CARGO_TERM_COLOR=never` を渡すこと (B2)
   - R-CA-4・Q-29・R-DIST-3: ロックファイルをキャッシュの `locks/` に置くこと (C1。利用者が決めた)、名前の作り方、記録の形式の版をパスに入れること
   - R-MCP-3・R-WS-5・R-FT-2 の範囲の判定 (B7): `--target` を付けない形でも M0 と同じだったこと (「付けない形での再確認は M2」の文を改める)。`--prefix indent` の見出しで辺の種類を読むこと、2 つ出る member の根を逆向きの木で決めること、取得元での対応付け、名指しできる依存の定め方 (dev を含まない解決で target 用か proc-macro) と、proc-macro の member の下の依存を安全側に名指しできないとすること
   - §8: `local_build_required` の扱い (M2 で使われなくなるか)、ビルドの状態の値 (R-OUT-9)
   - R-SC-4 の `readme` の扱いを M3 に送る
   - §12 の M2 の到達点と、M3 以降に回したもの
3. README を合わせる (F6)。公開は M3 の後にまとめて行う (P9。利用者が行う)
4. M3 の計画を立てる
