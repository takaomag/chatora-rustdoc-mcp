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
| D7 | docs.rs で引けない依存を照会のツールにつなぐ: `local_build_required` (path・git・別レジストリ・checksum の不一致) と、既定のプロジェクトの依存で求める target の JSON が無いとき (`no_target_json`) をローカルビルドに置き換える (ビルドしてよい組は B7。名指しできない依存は `build_unsupported`、判定できなければ `scope_unknown`)。照会のツールに実際のビルド target と違う `target` を渡されたらビルドしない。明示した版がプロジェクトの版と同じなら、プロジェクトの取得元のものを返して warnings を付ける (R-VR-1 の残り)。`crate` を省いた `search_items` の範囲に workspace を足す (R-SR-2)。ビルド中の crate は `sources` に `building` | R-TG-1・R-DS-1・R-VR-2・R-VR-1・R-MCP-3・R-WS-5・R-SR-2・R-SR-5 | (d) と (a) を組み合わせた workspace (path 依存、別のレジストリの依存 (偽の crates.io を別のレジストリとして)、checksum の合わない vendor)。(b) git の依存。(g) 依存の多い workspace (テストの中で生成) | 共通の印。path・git・別レジストリの依存を引けるテスト。docs.rs のモックで target の JSON が無い依存がローカルビルドされるテスト。範囲の外の target を渡すとビルドしないテスト |

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

### 6.3 M2b

(まだ無い)

### 6.4 M2c

(まだ無い)

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
   - §8: `local_build_required` の扱い (M2 で使われなくなるか)、ビルドの状態の値 (R-OUT-9)
   - R-SC-4 の `readme` の扱いを M3 に送る
   - §12 の M2 の到達点と、M3 以降に回したもの
3. README を合わせる (F6)。公開は M3 の後にまとめて行う (P9。利用者が行う)
4. M3 の計画を立てる
