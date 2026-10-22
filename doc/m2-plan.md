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

**C3b (進捗の通知。2026-10-21)**:

- 進捗を送るのは `tools/call` の入口 (`tool::call`) の 1 か所にした。ツールの仕事と取り消しを待つ `select!` のループに 3 秒ごとの tick を足し、そこで送る (間隔は R-WS-6 の 1〜30 秒の中で決めた)
  - 送るのは、要求に `progressToken` があるときだけ。値はその token
  - `progress` は送るたびに 3 ずつ増やす (経過秒数。`total` は付けない)
  - 応答も取り消しも、このループを抜けた後にしか起きないので、その後に進捗は送られない (MUST)。同時に起きたときは、`biased` で応答と取り消しを tick より先に見る
  - `sleep` は自分で進捗を送るので、この仕組みからは外した (2 つの送り手で値が単調でなくなるため)
- `message` は、呼び出しが待っている仕事とその段 (`fetching demo 0.1.0 (3 s)`)。仕事が複数なら件数と最初の 1 つ (`loading 5 crates: …`)、仕事を待っていなければ `looking it up`。待っている仕事は、`Fetches` と同じくタスクローカルの `data::waits::Waits` に、`Flights::join` が待つ間だけ書く。仕事を共有しても、待っている呼び出しごとに自分の token で送る (R-WS-6)。ツールの `spawn` は `Waits` も子のタスクに渡す
- rmcp の `RequestContext` を tool の中に渡さずに済んだ (C3 の下調べで、`dispatch` で `sleep` 以外には落としていると分かっていた)
- テスト (stdio):
  - 遅い docs.rs を待つ呼び出しは、3 秒目に `p1` の token で `progress: 3`・`fetching demo 0.1.0 (3 s)` を送る。応答の後には送らない。同時の token の無い呼び出しには送らない
  - 取り消した呼び出しには、その後なにも送らない
- clippy の `large_futures` に当たったので、ツールの future を `Box::pin` でヒープに置いた
- テスト用の docs.rs を使う stdio のテスト (C3a のものを含む) は、口が debug ビルドだけなので `#[cfg(debug_assertions)]` にした。前提を渡さないレビュー (サブエージェント) の指摘で、`biased` と、テストの待ちの余裕 (4 秒待つ間に tick 1 回) と、応答の届く順に依らない照合を足した。応答の順の件は、手元の全体のテストで一度落ちて確かめた

**C3c の分け方 (2026-10-21)**: 下調べ (サブエージェント) の結果から、C3c を 4 つに分けた

- C3c-1: 横断の検索の 0 件のときの `status: building` と、ビルド中のカーソル
- C3c-2: std の取得 (配布物の要約・アーカイブの取得と展開・読める nightly への最大 30 日のさかのぼり・索引の作成) を、期限付きの共有の仕事にする。今は期限が効かない。展開中のロックの隙間を塞ぐ
- C3c-3: `cargo metadata` の初回の取得を裏で続けられるようにする (今は呼び出しの future の中で走り、取り消すと cargo も止まる)。版を解決する前の `status: building` (provenance の `version`・`source` を省く)。`Activity::Fetching` をつなぐ
- C3c-4: sparse index・`.crate`・配布物 (要約とアーカイブ) の一時的な失敗を、短い間ディスクに覚える。今は応答の無い相手に 1 回で約 30 秒 (接続の 10 秒 × 3 回) かかり、プロセスの数だけ直列に繰り返す。配布物の失敗は、書き始めてから同じ仕組みで扱えると分かったので C3c-2 から移した

実測 (この PC、release ビルド): 取得 (展開と書き込み込み) と解析とアーカイブの書き込みは、tokio 1.54.0 (6.7 MB) が 0.2・0.06・0.08 秒、web-sys 0.3.106 (149 MB) が 1.0・1.2・0.8 秒、aws-sdk-ec2 1.269.0 (350 MB) が 2.5・4.3・3.6 秒。最大級でも既定の 20 秒に収まり、遅い回線や CI では超えうる。既定値 (待つ 20 秒・`retry_after_secs` 10 秒) は変えない

**C3c-1 (横断の検索の 0 件と、ビルド中のカーソル。2026-10-21)**:

- 結果が 0 件で、範囲に読み込み中 (取得か索引の作成) の crate があれば、最上位に `status: building` と `building_reason` (索引の作成中のものがあれば `indexing`、無ければ `fetching`) を付ける (R-OUT-9)。`partial`・`retry_after_secs`・`data`・`page` はそのまま返す (`did_you_mean` が役に立ちうるため)。hints で、待ってから呼び直すよう案内する
- ビルド中のカーソル (R-OUT-3): 利用者が「今のまま拒み、案内を足す」と決めた。カーソルは検索した crate のダイジェストに束縛したままで、読み込み中の crate が揃うと「データが変わった」で拒まれる。読み込み中の crate があって次のページがあるときは、hints でそのことと、最初から検索し直すことを案内する
- warnings の文言を「still being fetched」から「still being loaded」にした (索引の作成中も数えるため)
- テスト (stdio): 依存の demo を遅い docs.rs から取る間の横断の検索は `status: building` (`fetching`・`partial`・`retry_after_secs`) で、後の検索は `demo::hello` を見つけ、`partial` が無く、要求は 1 回
- 本物の Claude Code (利用者が試した。release ビルド、`limits.request_timeout_secs = 3` で待つのは 2 秒、空のキャッシュ) で aws-sdk-ec2 1.269.0 の `Client::describe_instances` を引いた: 1 回目と 2 回目は 2.0 秒で `status: building`、3 回目は 26 ms で答えた。docs.rs への要求は 1 回。モデルは記憶で答えず、待ってから呼び直した (1 回目の後は `retry_after_secs` の 10 秒より早い約 3 秒後、2 回目の後はシェルで待って約 18 秒後)。早い呼び直しの害は 2 秒の待ちが 1 回増えるだけなので、案内は変えない
- そこで分かったこと: 呼び出しのログで、`status: building` だった呼び出しが、通信しなかったので `cache: hit` と記録されていた。ログの行に `status: building` を書き、そのときは `cache` を書かないようにした (利用者が C3c-1 に含めると決めた。R-OBS-1)。README のログの節も直した

**C3c-2 (std の取得の時間の上限。2026-10-21)**:

- std の JSON を選び (`std_json::choose`)、配布物から取って展開し、std・alloc・core のアーカイブを作るまでを、1 つの共有の仕事にした (`query::load::std_json`)。鍵は target と、プロジェクトの toolchain の版 (それより古い nightly は選ばないため)。待つ期限と切り離しは C3a と同じ
  - 段は、選んで取る間が `fetching`、アーカイブを作る間が `indexing`
  - アーカイブを先に 3 つとも作るのは、std の path のほとんどが alloc か core の re-export で、照会の中で作ると期限が効かないため。作れなかったものはログに書き、照会がもう一度作ってその理由を返す
  - get_item (と同じ経路のツール) と、std の横断の検索の両方がこの仕事を待つ
- 応答: nightly が決まる前なので、provenance は `source: std`・`crate`・`target` だけで `version` が無い。R-OUT-9 が版の解決の前は `version`・`source` を省けるとしているので、outputSchema の provenance の必須を `crate`・`target` だけにした。hints は「`the documentation of std for <target> is being fetched from the Rust distribution server`」など。C3a の `building` を、provenance と案内の文を渡す形 (`building_response`) に分けた
- 展開中のロックの隙間 (C2b と同じ形): 配布物の展開は blocking のスレッドで最後まで走るのに、ディレクトリのロックは待つタスクが abort されると先に外れていた。ロックを展開の作業に渡し、展開が終わってから外すようにした
- テスト:
  - `std_json`: 遅いローカルの配布物 (要約とアーカイブを 300 ms ずつ遅らせる) で、100 ms の期限の呼び出しは `Going(Fetching)`。後の呼び出しは配布物から選んだものを受け、std のアーカイブは仕事が作っていて、要求は要約とアーカイブの 2 回
  - 版が決まる前の std の `status: building` の応答が、返しうる 7 つのツールの outputSchema に収まる (宣言の無い項目が無く、必須がある)。provenance の必須に `version` を残すと落ちる
  - 結果の検証器の自己テスト (`tests/real_data`) は、必須を欠く例に provenance の `version` を使っていたので、`target` に変えた
- テストの道具: `test_http::serve_sequence_after` (順に答える偽のサーバーを、各応答の前に待たせる) と、`dist::tests::setup_after`
- 実測 (この PC、release ビルド、空のキャッシュ、rustup の見えない PATH で配布物から): `std::vec::Vec` の初回は 3.0 秒 (要約の取得・アーカイブの取得と展開・3 つのアーカイブの作成)。続く `core::option::Option` は 0 秒で、先に作った core のアーカイブが使われた。既定の待ち 20 秒に収まる
- 利用者が、アーカイブを 3 つとも先に作る作りのままとすると決めた (代案は頼まれた crate だけ先に作ること。初回は速いが、re-export をたどる先の crate の作成に期限が効かない)

**C3c-3 (`cargo metadata` の時間の上限。2026-10-21)**:

- `Resolver` を `Flights` の共有の仕事にした。今まではワークスペースのルートごとの Mutex で直列に待ち、`cargo metadata` は呼び出しの future の中で走っていたので、取り消すと cargo も止まり、次の呼び出しが最初からやり直していた。中身 (オフラインか・`$CARGO_HOME`・覚えた結果) を `Inner` にまとめて `Arc` で仕事に渡す。`resolve` は期限なしで待ち (`Flights::wait`)、`resolve_until` は期限まで待つ。覚えた結果を使える呼び出しも仕事を通る (すぐ終わる)
- 段: cargo の stderr から分かる状態 (`cargo::Activity`。B2・B4 で作り、製品では使っていなかった) を仕事の段につないだ。ロック待ちは `waiting_for_lock` (`Doing::WaitingForLock` を足した。R-OUT-9 の値)、それ以外は `fetching` (`cargo metadata` で時間がかかるのは、ほとんどが依存の取得のため)
- 期限を過ぎたら、版が決まる前の `status: building` を返す。provenance は `crate`・`project_from`・`target` で、`version`・`source` が無い
  - 対象のツールは、get_item の経路 (版を省いたとき)、`resolve_version`、`list_crates` (provenance は `source: project`・`workspace`・`project_from`・`target`)、依存の横断の検索 (`target`・`project_from`)
  - `resolve_version` と `list_crates` の outputSchema も `with_building` を通し、`resolve_version` の provenance の必須を `crate`・`target` にした。`with_building` は `hints` が無ければ足す
  - 版を明示したときに `enabled_in_project` のためにプロジェクトを読む経路 (`crates_io_in_project`) は、期限を過ぎたらプロジェクトの情報なしで続ける (解決は裏で続く)
  - feature の組 (`cargo tree`) と、re-export の先の版を決める経路 (`in_project_build`) は、依存がそろった後で速いので、期限の外のままにした
- `ResolveError` に `Stopped` (仕事が結果なしに終わった) を足し、エラーを `Arc` で共有するようにした
- テスト:
  - `CARGO_HOME` を一時ディレクトリにした子プロセスで、パッケージキャッシュのロックを持ったまま解決させると、200 ms の期限の呼び出しが `Going(WaitingForLock)` を受け、ロックを外すと裏で続いた仕事が `cargo metadata` の結果を返す
  - 版が決まる前の応答 (get_item の系統・`resolve_version`・`list_crates`・横断の検索) が、それぞれの outputSchema に収まる (std の C3c-2 のテストを `fits_the_schemas` に一般化した)

**C3c-4 (一時的な失敗をディスクに覚える。2026-10-21)**:

- docs.rs の JSON が `miss.json` で使っていた記録 (`MissKind::Transient`、30 秒) を、項目のパスごとのファイル (`<パス>.miss.json`) にも書けるようにし、sparse index・`.crate`・配布物 (要約とアーカイブ) で使う。キャッシュの中にあるので、キャッシュを共有するほかのプロセスにも効く
  - 覚えるのは、通信の失敗と一時的な状態 (5xx・429) だけ。オフライン・無い (404)・予期しない状態・大きすぎる・チェックサムの不一致は覚えない。予期しない状態を覚えると、1 回目は `retryable: false` なのに 2 回目から `true` に変わるため (レビューの指摘。docs.rs の JSON は予期しない状態も覚えるが、今回は変えない)
  - 覚えている間は取りに行かず、`RecentFailure` (残りの秒数付き) を返す。ツールの応答は `network_error` (`retryable: true`) に `retry_after_secs` を付ける
- sparse index は、古いキャッシュがあれば、覚えている間は待たずにすぐそれを使う (warnings の文は「a moment ago」)。今までは応答の無い相手に約 30 秒待ってから古いものを使っていた。配布物の要約も、覚えている間は前に取ったものを warnings 付きで使う (前からの扱い)
- 配布物のアーカイブの失敗は、展開先のディレクトリの隣 (`std/<hash>/<triple>.miss.json`) に覚える。std の片付けは、期限の切れた記録のファイルを nightly のディレクトリと `std/` から消す (残ると nightly のディレクトリが空にならず、展開先ができなかったときは消す契機も無いため。レビューの指摘)
- テスト: 3 つの取得元それぞれで、503 で失敗した後の呼び出しは取りに行かずに `RecentFailure` になる (sparse index は、古いキャッシュがあればそれを使い、無い crate は失敗する)。要求の数で確かめた
- CI の揺れの修正: C3c-3 (85ca537) の CI で、testdata (ubuntu) の決定性のテストが `list_crates` の `docs_rs_json: cached` の有無で落ちた。同じテストのバイナリで並行に走る `questions.rs` が `reqwest@0.13.5` の `list_module` でホストの target の JSON をキャッシュに書くので、その前後で答えが変わる (b870897 からある揺れで、C3 によらない)。キャッシュにあるかは応答の入力ではない (R-ARCH-5) ので、比べる前にこの項目だけ除いた

**C4 の決定 (2026-10-21。利用者が 1 件ずつ決めた)**: 下調べ (サブエージェント) で、決めることを 7 つ挙げた

1. 記録の置き場所: 残った子孫の記録・作業用ディレクトリの持ち主・使えない target dir の印を、すべてキャッシュの下に置く (要件 R-DIST-3 は state)。キャッシュを共有するプロセスがすべて見られ、C1 のロック (`locks/`) とそろう。GC と `cache clear` は飛ばす。キャッシュが別のマシンと共有されても他のマシンの PID を扱わないよう、記録にマシンと起動の識別 (Linux の boot_id など) を入れ、同じものだけを扱う
2. 持ち主の生死: サーバーは起動から終了まで、`locks/` の自分のロック (乱数の実体 ID ごと) を持つ。記録にはその ID を書き、ロックが取れれば持ち主は死んでいる (ログのファイルと R-WS-10 の進行中の印と同じ考え方。PID の再利用を気にせず 3 つの OS で同じに動く。ロックは close-on-exec なので残った子孫は持たない)。案だった PID と起動時刻での判定はしない
3. 残った子孫の記録と照合: 両方を使う。(a) 子に close-on-exec でないロックの fd を継承させ、持ち主の死後もそのロックが取れなければ子孫が残っていると確実に分かる (target dir を使わない判定)。(b) Linux・macOS では、子のセッション ID (setsid で子の PID と同じ) とその起動時刻を記録し、子が終わったら消す。起動時に、持ち主の死んだ記録について、そのセッションに属し、起動時刻が記録した時刻以降のプロセスだけを止める。Windows は Job Object が止めるので (b) を記録しない。自分で setsid してセッションを抜けた子孫は見つけられないのを許す
4. 確かでないとき: 照合できないもの (起動時刻を読めない、別のマシンや起動の記録、版の違う記録など) は止めず、ログに書き、記録も消さない。継承したロックがまだ持たれていれば、その target dir は使わない。片付けは起動を待たせず裏で行い (R-MCP-11)、全体で 5 秒を超えたらやめて次の起動に回す
5. 使えない target dir の名前と、ほかのプロセスへの伝え方: target dir ができる D2 で決める
6. 版の違うサーバーの記録: 記録のファイル名に実体 ID を入れ、生死のロックの名前 (版を含まない) もその ID から作るので、中身を読まずに持ち主の生死が分かる。持ち主が死んでいて更新から 7 日経った他の版の記録は、子孫を止めずに記録だけ消す (Q-29 の案「読めない記録には触らない」を改める)

**C4a (サーバーの実体 ID と作業用ディレクトリ。2026-10-21)**: C4 を、実体 ID と作業用ディレクトリと起動時の掃除の骨組み (C4a)、子プロセスの記録 (C4b)、残った子孫を止める起動時の照合 (C4c) に分けた

- `data::owner::Owner`: サーバーは起動時に乱数の実体 ID (16 桁の 16 進) を作り、`locks/` の `owner:<id>` のロックを終了まで持つ (鍵は版をまたいで固定し、テストで固定した)。プロセスに 1 つ (`Owner::start_this`)。OS がロックできないときは、作業用ディレクトリを作らず、ほかのサーバーのものも消さない (生死が分からないため)
- 作業用ディレクトリはキャッシュの `work/<id>/` の下 (`Owner::work_dirs`。使うのは D のビルドから)。子の記録は `procs/v1/` に置く (C4b)
- 起動時の掃除 (`owner::sweep`): 起動を待たせず blocking のスレッドで、5 秒まで。ロックが取れる (持ち主が止まった) サーバーの `work/<id>/` を消し、そのロックファイルも消す (その ID をほかのサーバーが使うことは無いので消してよい。`locks/` が増え続けないように)。他の版の記録 (`procs/v<N>/<id>.*`) は、持ち主が止まっていて更新から 7 日経ったものを消す。この版の記録は C4c の照合に任せる
- 終了時 (どの終わり方でも) に、自分の `work/<id>/` とロックファイルを消す (`Owner::leave`)。セッションが始まる前に stdin が閉じた終わり方で消していなかったのを、stdio のテストで見つけた
- テスト: 止まったサーバーの作業用ディレクトリとロックファイルは消え、動いているものと自分のものは残る。他の版の記録は、1 週間より古く持ち主の止まったものだけ消える。終了時に自分のものを消す。期限で止まる。stdio: 前のサーバーが残した `work/` を起動時に消し、終了後に `locks/` に何も残さない
- README のキャッシュの節に、サーバーごとのロックと `work/` の片付けを書いた
- 前提を渡さないレビュー (サブエージェント) の指摘で直したもの: (1) 作業用ディレクトリを使う前に落ちたサーバーは、掃除で見つからずロックファイルが残り続けた。起動時に `work/<id>/` を作るようにした。(2) 他の版の記録の持ち主を確かめると、ロックファイルが作られて残った。記録と一緒に消す。(3) Windows で終了と掃除が重なると、ロックファイルが残りえた。作業用ディレクトリがもう無くてもロックファイルは消す。(4) `process_lock` の「ロックファイルは消さない」の説明に、`owner:<id>` だけの例外を書き足した。README の NFS の注意に、ほかのサーバーを止まったとみなしうることを足した

**C4b (子プロセスの記録。2026-10-21)**:

- `data::process_id`: プロセスの起動時刻と、マシンと起動の識別。Linux は `/proc/<pid>/stat` の 22 番目 (起動からの clock tick)、`/etc/machine-id`、`boot_id`。macOS は `proc_pidinfo` (`PROC_PIDTBSDINFO`) の起動時刻 (1970 からのマイクロ秒)、ホスト名、`kern.boottime`。手元で macOS 向けにビルドできない (aws-lc-sys の C のクロスコンパイルが要る) ので、macOS の部分は CI で確かめる
- Unix では、子を起動する前に `procs/v1/<実体 ID>.<番号>.lock` を作って排他ロックを取り、子の中でだけその fd の close-on-exec を外す (`pre_exec` の `fcntl(F_SETFD, 0)`)。子と子孫は同じ open file description を持つので、どれかが生きている間はロックが外れない。起動後に `<実体 ID>.<番号>.json` (マシン・起動・セッション ID (= 子の PID)・起動時刻) を書く
- グループを止めたら、サーバーは自分の fd を閉じてロックを試す。取れれば記録とロックファイルを消し、取れなければ (セッションを抜けた子孫がいる) 記録を残して C4c の掃除に任せる
- Windows は記録もロックの継承もしない (決定 3 から外した)。Job Object がサーバーの終了で中のプロセスを止めるうえ、Windows の継承は継承可能なハンドルを同時に起動した別の子にも渡し、子孫が残っているとの誤った判定を招くため
- テスト: 子は動いている間記録され (セッション ID と起動時刻が合う)、終わったら記録もロックファイルも残らない。Linux で、`setsid` でセッションを抜けた孫がいると、グループを止めても記録が残る (継承を外すと落ちることを確かめた)。起動時刻とマシン・起動の識別が読め、後で起動したプロセスのほうが後になる
- 前提を渡さないレビュー (サブエージェント) の指摘で直したもの: (1) 起動の要求が起動のスレッドの順番を待つ間に呼び出しが取り消されると、記録とロックの fd が先に閉じ、その番号が使い回されると別のファイルの close-on-exec が子の中で外れえた。記録を起動のスレッドへの要求に入れ、起動が終わってから手放す。(2) グループを SIGKILL した直後のロックの試しは、止めたプロセスが終わりきる前だと「残っている」になりがちだった。100 ms まで試し直す (`process_lock::lock_blocking_at`。`lock_blocking` はこれを使う)。(3) 記録を async のスレッドで fsync していた。起動の識別がクラッシュ後は変わるので fsync せず、書いてから rename するだけにした。(4) machine-id の無いコンテナでもテストが通るよう、マシンと起動の識別は読めたときだけ確かめる

**C4c (起動時に、残った子孫を照合して止める。2026-10-21)**:

- 起動時の掃除 (`owner::sweep`) は、作業用ディレクトリより先に、この版の子の記録を片付ける (`settle_children`)。対象は持ち主のロックが取れる (止まった) サーバーの記録
  - 継承ロックが取れれば、子孫は残っていないので、記録とロックファイルを消す
  - 取れなければ、記録のマシンと起動の識別が自分と同じで、起動時刻があるときだけ、記録したセッションのプロセスを列挙する (`process_id::session_members`。Linux は `/proc/*/stat` の 6 番目と 22 番目の欄、macOS は `proc_listallpids`・`getsid`・`proc_pidinfo`)。そのうち起動時刻が記録の時刻以降のもの (自分を除く) を SIGKILL で止め、1 秒までロックが外れるのを待って、外れたら記録を消す
  - 書いている途中で、照合の規則に穴を見つけた: PID が一巡すると、無関係なプロセスが同じセッション ID を持ちうる。セッションの長 (PID がセッション ID と同じプロセス) が生きていて、その起動時刻が記録と違えば、ID が使い回されたとみなして止めない (決定 3 の照合に足した)
  - どれにも当たらないとき (別のマシンや起動、起動時刻が無い、列挙できない、該当するプロセスが無い = セッションを抜けた子孫) は止めず、記録を残す (決定 4)
- テスト:
  - 子を起動したテストのプロセスを SIGKILL し、同じキャッシュの次のサーバーの掃除が孫を止め、記録を消す (Linux では子は `PR_SET_PDEATHSIG` で死に、孫が残る。macOS では子も残る)
  - 別の起動の記録と、セッションの長の起動時刻が違う記録では、何も止めず記録を残す
  - stdio: B1 の偽の rustc (孫を起動して止まる) でサーバーを SIGKILL し、同じキャッシュで起動した次のサーバーが、10 秒以内に rustc と孫を止める (照合を外すと落ちることを確かめた)
- 前提を渡さないレビュー (サブエージェント) の指摘で直したもの: (1) 持ち主が止まったかの確かめでロックファイルが作り直され、作業用ディレクトリがもう無いと誰も消さなかった。照合の後、作業用ディレクトリが無ければ消す。(2) セッションの長が居ないと ID の使い回しを確かめられず、使い回されたセッションに残った無関係なプロセスを止めうった。Linux では長の cargo がサーバーと一緒に死ぬので長が居ないのが普通であり、長が居なければ止めない、とすると片付けが働かない。そこで Linux では、止める各プロセスが継承したロックファイルを開いていること (`/proc/<pid>/fd` のデバイスと inode) も確かめる。macOS は長 (cargo。サーバーと一緒に死なない) が生きているときだけ止める
- テストの直し: 照合が合ってもロックを開いていないプロセスは止めない (Linux)、を足した。止めたプロセスはテストのプロセスの子のゾンビとして `kill -0` に答えるので、ゾンビを生きているとみなさない確かめ (`ps` の状態) に変えた (変えないと、確かめを外しても落ちなかった)
- C4b の CI (1fd1fb9) は、Windows と macOS の clippy で落ちていた (手元の Linux では出ない): macOS の `mem::size_of` の不要な修飾、Windows の使われないテストの import・Unix でしか読まないフィールド・何もしない関数の `const fn` の指摘。C4c と一緒に直した

### 6.5 M2d

**D1 の実測と決定 (2026-10-21)**:

- 実測 (rustup 1.29.1、この PC): 出る形式は stable 1.99.1 (`RUSTC_BOOTSTRAP=1`) が 61、nightly 1.101 (2026-10-19) が 61、nightly 1.98 (2026-06-11) が 57、nightly 1.92 (2025-10-10) が 56。入っていない toolchain を指す `rust-toolchain.toml` では、`RUSTUP_AUTO_INSTALL=0` の `rustc -vV`・`cargo metadata` が終了コード 1 で `error: toolchain '1.80.0-x86_64-unknown-linux-gnu' is not installed` と ``help: run `rustup toolchain install` to install it`` を出す。環境の `RUSTUP_TOOLCHAIN` は `rust-toolchain.toml` に勝ち、`+<toolchain>` はその両方に勝つ。`rustup run` は子に `RUSTUP_TOOLCHAIN` を渡す
- 実測の途中の事故: 比べるために `RUSTUP_AUTO_INSTALL=1` で 1 回走らせ、rustup が 1.80.0 を入れかけた (3 秒で止めた)。利用者の許可を得て、入れかけの toolchain と `~/.rustup` の `downloads`・`tmp` に残ったもの (どれも自分が作ったもの) を消した
- 利用者が 1 件ずつ決めたこと:
  1. 形式の知り方: 初めて使う前に、キャッシュの中の 1 行の crate に rustdoc を直接走らせて (cargo を使わない)、出た JSON の `format_version` を読む。結果は `rustc -vV` の commit-hash (と実行ファイルのパス) を鍵にキャッシュに覚える。`rustc -vV` は今どおり 60 秒だけ覚えるので、toolchain を入れ替えたら再起動なしで反映される
  2. 新しすぎる形式 (61 より新しい): ビルドして、最新の型で読めれば warnings (確かめていない旨) を付けて使い、読めなければ `unsupported_format`。別の toolchain には切り替えない (R-FV-5 と同じ)
  3. 実行ファイルのパスを一部だけ設定したとき: 指定された名前の版の接尾辞をまねて探し (`rustc-1.80` なら `rustdoc-1.80`)、無ければ接尾辞なし。symlink はたどらず、指定されたパスのディレクトリで探す。rustc と rustdoc の `-vV` の commit-hash が合わなければ、両方の版を書いたエラーにする (Q-31 のこの部分)
  4. プロジェクトの toolchain が入っていないとき: 事前の確かめは `RUSTUP_AUTO_INSTALL=0` で行い、オンラインならビルドの cargo に rustup の自動インストールを任せる (その間は `status: building`。D2・D3 でつなぐ)。オフラインなら `toolchain_missing`。rustup の「not installed」の文言を R-WS-7 の許す照合に足し、テストに固定する
  5. プロジェクトの cargo の設定の `build.rustc`・`build.rustdoc` と環境変数 `RUSTC`・`RUSTDOC`: cargo と同じものを事前の確かめにも使う (環境変数が優先)。`rustc-wrapper` は見ない。サーバーの設定 `build.*_path` があればそちらが勝ち、子に `RUSTC`・`RUSTDOC` として渡す
  6. `rust-version`: 事前に比べず、D5 で cargo の `requires rustc` などから `toolchain_too_old` にする
  7. `RUSTUP_TOOLCHAIN`: プロジェクトで走らせる子すべて (ビルドの cargo、`cargo metadata`・`cargo tree`、プロジェクトで走らせる `rustc -vV` と形式の試し) から外し、起動時に 1 回ログに書く。中立のディレクトリで走らせる host の `rustc -vV` はそのまま
  8. 古すぎる形式からの nightly への切り替え (要件どおり): `cargo +nightly` (`build.nightly_date` があれば `+nightly-<日付>`) に `RUSTUP_AUTO_INSTALL=0` を付け、無ければ `toolchain_missing`。パスの設定があるときと rustup が無いときは切り替えず `unsupported_format`。`build.nightly_date` は D1 で足す (std の配布物の選び方には効かせない)

**D1a (ビルドに使う cargo・rustc・rustdoc。2026-10-21)**: D1 を、使うプログラムを決める D1a と、形式の試し・nightly への切り替えの D1b に分けた

- `data::toolchain::Programs`: 設定 `build.cargo_path`・`build.rustc_path`・`build.rustdoc_path` (絶対パス) を読み、設定の無いものを決定 3 のとおり探す (rustc は rustdoc、次に cargo の隣。rustdoc と cargo は rustc の隣)。指定が file でない・見つからないときは、起動時にログに書き、`Programs::problem` で後の段 (D2 のビルド) が返せるようにする。プロセスに 1 つ (`Programs::set_this`)。起動時に、環境に `RUSTUP_TOOLCHAIN` があれば、プロジェクトで走らせる子に渡さない旨をログに書く
- cargo の起動 (`cargo::project_command`: `cargo metadata`・`cargo tree`、D2 からビルド): 設定の cargo を使い、設定の rustc・rustdoc を `RUSTC`・`RUSTDOC` で渡し、`RUSTUP_TOOLCHAIN` を外す (決定 7)
- 事前の確かめに使う rustc・rustdoc (`Programs::used_in`、決定 5): 設定、`RUSTC`・`RUSTDOC` (相対パスはプロジェクトから)、プロジェクトの cargo の設定 (`cargo_config` に `build.rustc`・`build.rustdoc` の読み取りを足した。区切りのあるパスは `.cargo` の親から、名前は PATH から。cargo と同じ)、toolchain のもの、の順。std の選び方に使うプロジェクトの toolchain の版 (`project_toolchain`) もこれを使う
- `tool::output` に走らせる場所 (`Place::Neutral`・`Project`) を足し、プロジェクトでは `RUSTUP_TOOLCHAIN` を外す。プログラムをパスで受けるようにした
- `toolchain::check_target`: プロジェクトで `rustc -vV` と `--print sysroot` を走らせ (`RUSTUP_AUTO_INSTALL=0`)、rustup が toolchain を「not installed」と言えば `Missing::NotInstalled` (文言は R-WS-7 の許す照合に足し、テストに固定)。sysroot に `lib/rustlib/<target>` が無ければ `Missing::Target`、host なら `lib/rustlib/<host>/lib` に libcore があるかで見る (R-TG-1)
- `toolchain::same_toolchain`: rustc と rustdoc の `-vV` の commit-hash が合わなければ両方の版を書いたエラー (決定 3)。使うのは D2 のビルドの前
- テスト: 設定の無いものを接尾辞付きで隣から探す、file でない設定はエラー、cargo の環境 (`RUSTC`・`RUSTDOC`・`RUSTUP_TOOLCHAIN` を外す)、rustc の決まり方の順、rustup の文言の読み取り、host の toolchain は host を作れる、入っていない toolchain (`channel = "1.0.1"`) は `NotInstalled` (何も入らないことを確かめた)、偽の sysroot で他の target が `Target`、偽の rustc・rustdoc で commit の食い違い
- README の設定の表に `build.*_path` を足した
- 前提を渡さないレビュー (サブエージェント) で確信を持てる誤りは無かった。弱い指摘から、cargo が `build.rustc`・`build.rustdoc` の上書きに使う `CARGO_BUILD_RUSTC`・`CARGO_BUILD_RUSTDOC` を、`RUSTC`・`RUSTDOC` と設定ファイルの間の順に足し、テストの環境 (`Isolated::apply`) の説明に、サーバーが `RUSTUP_TOOLCHAIN` を子に渡さないことを書き足した
- D1b のための実測 (cargo を使わない形式の試し): `RUSTC_BOOTSTRAP=probe rustdoc +<toolchain> --edition 2021 --crate-type lib --crate-name probe -Z unstable-options --output-format json -o <dir> probe.rs` は `<dir>/probe.json` を出す。stable 1.99.1 が 0.10 秒、nightly が 0.41〜0.77 秒 (形式は 61・61・56・57)。stable は `RUSTC_BOOTSTRAP` が無いと `the option Z is only accepted on the nightly compiler` で失敗する

**D1b (形式の試しと toolchain の選択。2026-10-21)**:

- 設定 `build.nightly_date` (`YYYY-MM-DD`。`dist::NightlyDate` を使う) を足した。std の配布物の選び方には効かせない
- `toolchain::format_of`: プロジェクトで (`RUSTUP_TOOLCHAIN` を外して) rustdoc の `-vV` を走らせ、commit-hash と rustdoc のパスと `+toolchain` の SHA-256 を鍵に、キャッシュの `toolchains/v1/` の記録を読む。無ければ、作業用ディレクトリに 1 行の crate を書き、`RUSTC_BOOTSTRAP=probe` で `-Z unstable-options --output-format json` の rustdoc を走らせて、出た JSON の `format_version` を読み、記録する。commit-hash の無い (`unknown`) rustdoc は記録しない。rustup の「not installed」は `Missing::NotInstalled`
- `toolchain::choose`: プロジェクトの rustdoc (D1a の `used_in`) の形式が 46 以上ならプロジェクトの toolchain (61 より新しくても。決定 2)。46 未満なら、rustdoc が rustup の proxy (PATH の `rustdoc`。設定・環境変数・プロジェクトの設定が別のものを指していない) で rustup があるときだけ `+nightly` (`+nightly-<日付>`) を試し、それも入っていなければ `NightlyMissing` (`toolchain_missing`)、切り替えられなければ `TooOld` (`unsupported_format`)。プロジェクトの toolchain が入っていなければ `NotInstalled` を返し、扱い (オンラインならビルドに任せる、オフラインなら `toolchain_missing`) は D2 が決める。使うのは D2 のビルドの前
- `tool::output_with` (環境変数を足して走らせる) を足した
- テスト: 手元の rustdoc の形式が 46 以上で記録される。偽の rustdoc (`+nightly` で 61 を出し、試しの回数を数える) で、61・62 はプロジェクトの toolchain で試しは 1 回、40 は nightly に切り替わり、切り替えられなければ `TooOld`、nightly が入っていなければ `NightlyMissing`。テストの後に toolchain が増えていないことを確かめた
- README の設定の表に `build.nightly_date` を足した
- 前提を渡さないレビュー (サブエージェント) の指摘で直したもの: 試しに付けていた `--edition 2021` は 1.56 より前の rustdoc が受け付けず、まさに nightly に切り替えたい古い toolchain で試しが失敗していた。外した (doc コメント 1 行には要らない)。もっと古く JSON を出せない rustdoc は、nightly に切り替えられるなら切り替える (`project_format`・`NightlyMissing` の `found` は `None`)。切り替えられなければ試しの失敗をそのまま返す
- D1a (a13c883) の CI で、この文書の markdownlint が落ちていた (MD033・MD038): rustup の文言を書いたコードの中で、backtick の前に置いた逆斜線が、コードの区切りの対応を崩していた。二重の backtick で囲んだ
- 揺れるテストの修正 (C4c のテスト): `what_cannot_be_told_for_sure_is_left` が全体のテストで一度落ちた。持ち主のロックを手放した直後に掃除を走らせるので、ほかのテストが同じ瞬間に起動した子が、exec するまでロックの fd の写しを持ち、掃除が持ち主を生きているとみなして記録を飛ばしていた (C1 で見つけた現象と同じ。製品では、掃除は次のサーバーの起動時なので当たらない)。テストの助け (`Owner::killed_for_test`) が、ロックが本当に外れるまで待つようにし、同じ形の owner のテストもこれを使う。それでも 12 回の繰り返しで 1 回落ちた: 掃除自身が、同じ持ち主の記録ごとにロックを取っては手放していたので、2 件目以降で同じことが起きて「不確か」が 1 件足りなかった。これは製品でも、掃除と同時に子を起動すると起こりうる (記録が次の起動まで残るだけ)。掃除が持ち主ごとに記録をまとめ、ロックを 1 度だけ取って全件を片付けるまで持つようにした。子の記録の片付けと work dir の片付けの間で手放すのは残る (そこで当たると work dir が次の起動まで残るだけ)

**D2 の下調べと決定 (2026-10-21)**:

- 下調べ (サブエージェント、読むだけ): 起動の形は R-WS-1 で決まっている。使える部品は `cargo::project_command`・`run_reporting`、`Owner::work_dirs` (キャッシュと同じファイルシステムなので rename で原子的に公開できる)、`toolchain::choose`・`check_target`・`same_toolchain`、`load::archive_of`、`Flights`、`project::project_target` (`--target` の判定)。足りないものは `Doing::Building`、`cargo` の直後の `+toolchain`、stdout の JSON の行ごとの読み取り (今の `Child::output_watching` は stdout をまとめて読む)、member の `PackageId` の引き方 (`ProjectPackage` は持たない)、`build.jobs`。M0 の B1 のスクリプトの `RUSTC_BOOTSTRAP` の値には、v0.18 で足した cdylib・staticlib・dylib と member の bin が入っていない。M0 で測っていないのは、proc-macro の member、cdylib だけの lib、`--` の後の `-o`
- 利用者が 1 件ずつ決めたこと:
  1. 専用の target dir: `<cache>/t/<12 桁>/` の下に `target/` (`CARGO_TARGET_DIR`) と `build/` (`CARGO_BUILD_BUILD_DIR`)。12 桁はワークスペースのルートの正規化したパスの SHA-256 の先頭。toolchain と target は鍵に入れない (cargo が中を分ける)
  2. 使えない target dir (C4 の決定 5): 子の記録に使った target dir を足し、掃除が記録を「不確か」として残すとき、その dir に `unusable` の印を置く。ビルドは `<12 桁>`、`<12 桁>.1`、… のうち印の無い最初のものを使う。印の付いた dir は、記録が消えた後に GC が消す。ほかのプロセスは印を見て知る
  3. `RUSTC_BOOTSTRAP` の値の上限 (Q-31): 3 OS 共通で 30,000 バイト。超えたら `RUSTC_BOOTSTRAP=1` に落とし、ログと応答の warnings に書く。依存が nightly 機能を有効にして壊れたときに member の名前だけで再試行するかは、D5 の失敗の扱いで決める
  4. 同名の crate の上書き (Q-29): サーバーのロック `doc:<target dir>` を排他で取り、cargo の実行から JSON の移動・dep-info まで持つ。順は「鍵 → target dir (共有) → このロック → ビルドの枠」。移した JSON の crate 名・版・`target.triple` を頼んだものと照らす
  5. D4 までの作り直し: member を引く呼び出しのたびにビルドする (同時の呼び出しは `Flights` でまとめる)。中身が同じ JSON は sha で引くのでアーカイブは作り直さない。置き場所は `<cache>/local/<ワークスペースの 12 桁>/<package id の 12 桁>/<triple>/<lib か bin-名前>/` に `<format>.json`・`.sha256`・来歴の `record.json` (D4 のダイジェストの層を後で差し込める形)
  6. bin crate: パスの先頭を member の lib 名、次に bin 名と照らす。lib と bin が同名なら lib で、その bin は引けない (要るとわかったら引き方を足す)。ビルドするのは頼まれた target だけ
  7. features: `-p <member>` だけで cargo の既定に任せる (1 つの member では通常のビルドと同じ組になると M0 の B3 で確かめた)。使われた組は `compiler-artifact` の `features` から `features_effective` に。通常の組の明示・複数 member の統合・`[package.metadata.docs.rs]` は D3
  8. `--cfg docsrs`: 今から最後の rustdoc にだけ渡す (要件の既定)。外してやり直すのは D3
  9. 期限に間に合わないビルド: `Flights` の共有の仕事にし、期限が来たら `status: building` (ロック待ちなら `waiting_for_lock`) を返して裏で続ける。`Doing::Building` を足す
  10. ビルドの中で toolchain が入ったとき (D1 の決定 4): できた JSON の `format_version` を読む。古すぎれば `choose` をやり直して nightly で作り直し、新しすぎれば D1 の決定 2 に従う
  11. `-j`: 設定 `build.jobs` を足し、既定は max(1, `available_parallelism` の半分)。既定の値は F5 で見直す
- nightly でも `-Z fine-grain-locking`・`cargo doc --output-format json` は使わない (Q-17 で `cargo rustdoc` に決めてある)
- D2 は 3 つに分ける: D2a (ビルドの計画: `RUSTC_BOOTSTRAP` の値、target dir の名前と印、`--lib`・`--bin`・`--target`・doc dir の決め方、`-j`、起動の組み立て。外のプロセスを走らせない)、D2b (走らせて公開する: stdout の JSON の読み取り、ロック、work dir から照らして原子的に公開、fixture のテスト)、D2c (問い合わせにつなぐ: `get_item` の member を `local_build_required` からビルドに、`status: building`、来歴、R-SEC-1 の stdout のテスト、Claude Code での試し)

**D2a (ビルドの計画。2026-10-21)**:

- `data::build` を足した。外のプロセスは走らせない
  - `Bootstrap::of`: resolve の全パッケージの lib (`lib`・`rlib`・`dylib`・`cdylib`・`staticlib`)・proc-macro と、member の bin の crate 名 (`-` を `_` に) を辞書順にカンマでつなぐ。30,000 バイトを超えたら `1` (決定 3)。依存の bin・build script・test は入れない
  - `TargetDir::of`: `<cache>/t/<ワークスペースのルートの SHA-256 の先頭 12 桁>` (印が付いていれば `.1`、`.2`、…)。`target()`・`build()`・`doc_dir()` (`--target` があれば `<target>/doc`、proc-macro は常に host の `doc`) (決定 1・2)
  - `member_crate`: パスの先頭の名前を member の lib、次に member の bin と照らす (決定 6)
  - `passes_target`: プロジェクトが `build.target` を設定しているとき、または host でない (host がわからない) とき `--target` を渡す (R-WS-1)
  - `Plan::command`: R-WS-1 の形 (`--cfg docsrs`・`--document-private-items` を最後の rustdoc に。決定 8)。`RUSTC_BOOTSTRAP`・`CARGO_TARGET_DIR`・`CARGO_BUILD_BUILD_DIR` を設定する。`Plan::json` は出る JSON のパス
  - 設定 `build.jobs` (既定は max(1, `available_parallelism` の半分)。決定 11)。起動時に、環境に `RUSTC_BOOTSTRAP` があればビルドでは置き換える旨をログに書く (R-WS-8)
- `cargo::project_command` に、サーバーが選んだ toolchain を渡す引数を足した (`+<toolchain>` と `RUSTUP_AUTO_INSTALL=0`。D1 の決定 8)
- `toolchain::check_target` が、`rustc -vV` の host を返す (`--target` の判定に使う)
- 使えない target dir の印 (決定 2): 子の記録 (`procs/v1/`) に `target_dir` (`t/<名前>` の形。無ければ書かない。形式の版は変えない) を足した。ビルドの子は `child::spawn_building` で起動する (呼ぶのは D2b)。起動時の掃除が記録を「不確か」として残すとき、その dir に `unusable` を置く。記録の値は `t/` の下の英数字と `.` だけの名前のときだけ使い、キャッシュの外には書かない。印の付いた dir を消すのは F1 の GC
- `cargo metadata` の target 名は、lib では `_` だが bin では `-` が残る (このリポジトリの bin `chatora-rustdoc-mcp` で確かめた)。`metadata::Target::name` の doc コメントが誤っていたので直した。`--bin` には元の名前を渡し、`RUSTC_BOOTSTRAP` と照合には `_` の形を使う
- テスト: 値 (依存の bin・build script を除く、cdylib だけの依存を含む)、長すぎる値は `1`、lib が同名の bin より先、proc-macro、依存は引けない、印の付いた dir を飛ばす、`--target` の判定、proc-macro の doc dir、コマンドの引数と環境 (nightly・オフライン・`--bin`・`--target`、host 向けの lib)、子の記録の `target_dir`、掃除が印を置く・キャッシュの外を指す値には置かない
- 前提を渡さないレビュー (サブエージェント) で確信を持てる誤りは無かった。弱い指摘から、長すぎる値のテストを、ほかの名前に頼らず上限を超えるようにした。D2b への申し送り: `-p` に渡す package id は `cargo metadata` のもの。cargo 1.77 より前は別の形式 (`name ver (path+…)`) で、`-p` には渡せない。プロジェクトの toolchain が古いと、その cargo の `cargo metadata` はこの形式を出すので、D2b で `name@version` に直すか、確かめる
- D1b の CI (a323f43) は、Windows の `dropping_a_child_stops_its_grandchildren` (PowerShell で起動する孫が 30 秒以内に pid を書かなかった) だけ落ち、その job の再実行で通った。B1 からあるテストで、D1b は Windows の子の起動に触れていないので、揺れとみる

**D2b (ビルドを走らせて公開する。2026-10-21)**:

- `build::build`: target dir のロック (`target:<名前>` を共有) と書き込みのロック (`doc:<名前>` を排他。決定 4) を取り、待つ間は `Activity::WaitingForLock` を知らせる。OS がロックできなければ、`Cache::lock` と同じくログに書いて進む。`cargo::run_building` で `Plan::command` を走らせ (子の記録に target dir を残す `child::spawn_building`)、stdout の JSON のメッセージを 1 行ずつ読む (`Child::output_streaming`。stdout は溜めない)
  - `compiler-artifact`: 文書化する crate (package id と、lib なら lib の種類、bin なら名前と `bin`) の `features` を `features_effective` に。`fresh` でない unit の名前を `Built::rebuilt` に
  - `compiler-message`: `error` の `rendered` を最初の 5 件まで、失敗の `BuildError::Failed` に (`classify` の `Failure` と stderr の末尾と一緒に)
- 成功したら、出た JSON (`Plan::json`) を作業用ディレクトリ (呼ぶ側が渡す。D2c では `Owner::work_dirs`) に rename し、ここで 2 つのロックを手放す。読んで、根の item の名前・`crate_version`・(`--target` を渡し proc-macro でないときは) `target.triple` を `Plan` と照らし (違えば `NotTheCrate`)、`<cache>/local/<ワークスペースの 12 桁>/<package id の 12 桁>/<triple>/<lib か bin-名前>/<format>.json` に `.sha256` と一緒に書き、隣に `record.json` (`BuildRecord`: package id・crate 名・版・形式・target・toolchain (呼ぶ側が渡す `rustc -vV`)・サーバーが選んだ toolchain・`built_at`・`features_effective`・`RUSTC_BOOTSTRAP` が `1` だったか・SHA-256) を書く。書く間はその dir を `Cache::lock` で持つ
- ロックの鍵 `target:<名前>`・`doc:<名前>` はテストで固定した (版の違うサーバーが同じ名前で見つけるため)
- dep-info (`--emit=dep-info` の 2 回目の rustdoc) は D4 の鮮度で足す
- fixture (a)・(e): `tests/fixtures/local-build` (member: 1 つの lib と feature と `RUSTC_BOOTSTRAP` を見る build.rs を持つ path 依存の `one`、同名の lib と bin の `same`、名前の違う lib (`tool_core`) と bin (`tool`) の `tool`、proc-macro の `macros`、build.rs が cfg と環境変数を設定し `one` と同じ path 依存を持つ `cfgenv`、cdylib だけの `cdy`)。`tests/local/local_build.rs` が隔離した子で、全部をビルドして公開し、中身 (private の item、build.rs の cfg と変数、proc-macro を `--target` 付きで、cdylib、同名の lib と bin は別々に公開)、`features_effective`、別の member (`cfgenv`) をビルドしても共有の依存を作り直さないこと、bin の後に同じパッケージの lib を 2 回ビルドできること、2 回目の `one` で作り直したのが `one` だけであること、作業用ディレクトリに何も残らないことを確かめる。この PC で 10 回のビルドを含めて 3.0 秒 (P7: 3 OS の普通の `cargo test` で走らせる)
- 前提を渡さないレビュー (サブエージェント) の指摘と対応:
  - 「bin を文書化すると同じパッケージの lib の HTML もでき、その後の lib の 2 回目は cargo が fresh とみなして rustdoc を走らせず JSON が出ない」: cargo 1.99.1 で実測すると起きなかった (bin → lib → lib の 3 回とも文書化する unit は `fresh: false` で JSON が出た。lib の 2 回目の前に HTML の dir は消えていた)。この順をテストに足し、版によって起きたときに分かるよう、文書化する unit が fresh で JSON が無いときは `BuildError::Fresh` にした
  - 照合できる `compiler-artifact` が無かったとき `features_effective` が黙って空になっていた。`None` (不明) にした。cargo 1.77 より前の package id の形式 (`-p` に渡せない) は D2c で決める
  - future が落ちても、ロックを rename の後まで持つように、guard をブロッキングのスレッドに渡して手放す (移動と読み込みと削除も 1 つのスレッドで行い、work に残さない)。公開の `Cache::lock` も同じ
  - `Header::check` の単体テスト (根の ID が数値と文字列、版が無い・違う、別の crate、`--target` と proc-macro の例外) を足した。fixture の `one` の関数を crate 名と違う名前にした (根のモジュールの名前でも満たされていた)

**D2c の下調べと決定 (2026-10-21)**:

- 下調べ (サブエージェント、読むだけ): `crate_overview`・`list_module`・`list_impls`・`list_methods`・`list_implementors`・crate を 1 つ指定した `search_items` は、どれも `get_item::execute` を通る。`local_build_required` を返すのは `get_item::resolve` の 2 か所 (crates.io でない、docs.rs の照合が通らない) と `search_across::dependencies` (D7)。member を引くときに黙って別物を返しうる箇所: R-FT-1 のための `.crate` の取得 (`docs_rs_build`)、re-export の行き先を「crates.io の最新」に落とす `destination_request`、版を明示したときの crates.io。足りない部品: `Doing::Building`、ビルドの toolchain の `rustc -vV` を来歴に載せる手段、プロジェクトが `build.target` を設定したか (と独自の target か) を返す `project_target`、利用者の `net.offline` を含むオフラインの判定 (`Resolver::offline_in` は private)、来歴の `source: local`・`built_at`・`features_effective` とスキーマ。`tests/stdio.rs` の member に `local_build_required` を期待するテストは書き換える
- 利用者が 1 件ずつ決めたこと:
  1. 範囲: `get_item::execute` を通るツールで、ワークスペースの member だけ。ワークスペースの外の path 依存・git・ほかの registry・checksum の食い違い・横断の検索は今の `local_build_required` のまま (D7)
  2. member と同じ名前の `crate@version`: 版が member と同じなら member をビルドする (R-VR-1 の member の部分を先に)。違う版なら今どおり crates.io を引き、member ではない旨の warning を付ける
  3. 期限: 今の期限 (`limits.request_timeout_secs` の 3 分の 2)。`build.wait_secs` は D3
  4. ビルドの中で入った toolchain の JSON が古すぎたら、同じ共有の仕事の中で `choose` をやり直して nightly で作り直す (D2 の決定 10)。テストは古い形式を出す偽の rustdoc で
  5. `features_requested` は D3 まで出さない (スキーマでは省きうる項目)
  6. crates.io でないパッケージへの re-export は追わず、re-export (use の item) を返して warning を付ける。追うのは D6
  7. プロジェクトの `build.target` が独自の target (target の JSON・build-std) なら、host 向けに `--target <host>` でビルドして warning (B3b と同じ)。build-std で失敗したら `build_failed`
  8. cargo 1.77 より前の形式の package id (`name ver (source)`): `-p` には `name@version` を渡し、メッセージの照合も両方の形式から取り出した (名前, 版) で行う
- 聞かずに決めたこと (要件・前の決定から): member を引くときは `.crate` を取りに行かず docs.rs にも落ちない (R-WS-7)。`cargo metadata` が失敗して Cargo.lock だけで解決したときは、分類した失敗を返す (R-VR-2)。アーカイブは作った JSON の SHA-256 を鍵に `archive_of` で作る。エラーの対応づけの初期案: `ChooseError::NotInstalled` はオンラインならビルドに任せ、オフラインなら `toolchain_missing`、`NightlyMissing`・`Missing::Target` は `toolchain_missing` (入れるコマンド付き)、`TooOld` と読めない新しい形式は `unsupported_format`、`BuildError::Failed` は `classify` の名前 (`offline_miss`・`lockfile_outdated`・`fetch_failed`) か `build_failed`、`Programs::problem`・rustc と rustdoc の食い違い・`NoJson`・`Fresh`・`NotTheCrate`・`Unreadable` は `build_failed` (`retryable: false`)、`Io`・`Killed` は `build_failed` (`retryable: true`)。失敗の記憶・`refresh`・`toolchain_too_old`・診断の文字数の上限は D5
- D2c は 3 つに分ける: D2c-1 (データ側: `Doing::Building`、`Activity` からの写し、`rustc -vV` の要約、オフラインの判定、`project_target` の拡張、ローカルの共有の仕事と事前の確かめとアーカイブ、package id の形式、来歴の型)、D2c-2 (MCP 側: `resolve` での分岐、`.crate` の取得を飛ばす、building の応答、エラーの対応づけ、re-export の止め、スキーマ、stdio のテスト)、D2c-3 (Claude Code での試しと README)

**D2c-1 (データ側。2026-10-21)**:

- `Doing::Building` (`building`、「building the documentation of」) を足し、`Activity::of_build` で cargo の様子を写す (動いていれば `Building`、ロック待ちと取得はそのまま)。`resolving` と `being` の分岐にも足した
- `toolchain::check_target` が `+<toolchain>` (サーバーが選んだ nightly) を受け、host と一緒に `rustc -vV` の要約 (`rustc 1.99.1 (0123456789 2026-…)`。std の来歴と同じ `RustcVersion::summary`) を返す (`Checked`)
- `Resolver::offline_in` を公開した (利用者の `net.offline`・`CARGO_NET_OFFLINE` も見る)
- `PackageId::name_and_version`・`spec`・`is`: cargo 1.77 以降の id (`<source>#<name>@<version>`、名前がパスの最後と同じなら `<source>#<version>`) と、それより前の id (`<name> <version> (<source>)`) の両方から名前と版を取る。`-p` には古い形式なら `name@version` を渡し、メッセージの照合も (名前, 版) で行う (決定 8)
- `query::local::local_archive`: `Flights` の共有の仕事 (鍵はワークスペースのルート・package id・target・lib か bin)。仕事は、設定のプログラムの問題 → `Owner::work_dirs` → rustc と rustdoc の食い違い → `choose` (オンラインで toolchain が入っていなければ、ビルドの cargo に入れさせる) → `check_target` (host で `--target` を決める) → `build::build` → 入れさせた toolchain の形式が古すぎれば `choose` をやり直して nightly で作り直す (決定 4) → `archive_of` (鍵は JSON の SHA-256。読むときに SHA-256 を照らす)。warnings: `RUSTC_BOOTSTRAP` が `1` だったこと、形式が新しすぎること。失敗は `LocalError` (D2c-2 でエラーの code に写す)
- 来歴: `Origin::Local` (`resolved_from`・`project_from`・`toolchain`・`built_at`・`features_effective`)、`Source::Local` (`local`)、`Provenance` の `built_at`・`features_effective` とスキーマ。ソースの場所は、ローカルビルドでは URL を付けず、「docs.rs の URL が無い」warning も出さない。`provenance()` が長くなったので、`Origin` から項目を取り出す部分を `from_where` に分けた
- テスト: package id の両形式、`check_target` の要約、`tests/local/local_build.rs` に `local_archive` (同時の 2 つの呼び出しが 1 つの仕事を共有し、要約が載り、warning が無い)
- 試していないこと: ビルドの中で toolchain を入れさせる経路 (オンラインで入っていない toolchain) と、その後の nightly での作り直しは、toolchain を入れずには試せないので、テストが無い
- 前提を渡さないレビュー (サブエージェント) の指摘と対応:
  - `tool::output_kept` は失敗も 60 秒保持するので、ビルドの中で cargo が toolchain を入れた直後の選び直しが、保持された「not installed」で失敗していた。入れさせたビルドの後は、そのプロジェクトで保持したものを捨て (`toolchain::forget_project`)、`check_target` を 1 度走らせる。ビルドが失敗していて toolchain や target が欠けていれば `Missing` (`toolchain_missing`) を返し、成功していれば toolchain の要約を記録に入れて書き直す
  - nightly に切り替えてよいのは rustdoc が素の `rustdoc` のときだけを見ていたので、環境やプロジェクトが rustc だけを別のものにすると `+nightly` が toolchain を混ぜていた。rustc も素の `rustc` であることを足した
  - 古すぎる形式の 1 回目のビルドが公開されたまま残っていた。nightly で作り直す前に消す (`Built::unpublish`)。選び直しでもプロジェクトの toolchain になったら `TooOld`
  - 呼び出しのたびにビルドするので、文書化する crate の rustdoc だけで呼び出しの期限 (20 秒) より長くかかる member には、いつまでも `building` が返っていた (前のビルドが終わった後に来た呼び出しは、新しいビルドを始める)。利用者の決定で、期限を過ぎて裏で続いたビルドの結果は、次の 1 回の呼び出しに渡す: `Flights::handing_over` (結果を仕事の番号と一緒に残し、次の呼び出しが受け取って消す。走っている仕事に合流して結果を受け取った呼び出しも、同じ番号の残りを消す)。ローカルビルドの `Flights` はこれで作る。docs.rs などのほかの `Flights` は今のまま
- D2c-2 に残したこと: `project_target` がプロジェクトの `build.target` の有無と独自の target かを返すこと (決定 7)、`resolve` での member の分岐 (決定 1・2)、`.crate` の取得を飛ばすこと、building の応答、エラーの写し、re-export を追わないこと (決定 6)、stdio のテスト

**D2c-2 (MCP 側。2026-10-21)**:

- `mcp::tool::local`: `member` (crate 名を `build::member_crate` で member に照らし、`LocalWanted` を組み立てる: オフラインは `Resolver::offline_in`、プロジェクトが `build.target` を設定しているかは `project::sets_build_target`。`cargo metadata` が失敗して Cargo.lock だけのときは `Unknown` で分類した失敗、期限内に解決しなければ `Resolving`)、`built` (`local_archive` を待ち、期限が来たら `status: building`)、`LocalError` から `code` への写し (§6.5「D2c の下調べと決定」の初期案のとおり。`build_failed` には compiler の `error` を `diagnostics` に、無ければ stderr の末尾を `stderr` に。`fetch_failed` には `reason`。時間切れは `reason: timeout`。`unsupported_format` には `found`・`supported`)
- `build::member_crate` は、crate 名で見つからなければ、パッケージ名が同じ member の lib も探す (`[lib] name` が違う member をパッケージ名で引けるように)。`build::documented_name` を足した
- `get_item::resolve`: プロジェクトが決めた crates.io でないパッケージが member の crate ならビルドする (決定 1)。版を明示した照会は、その版が member の版と同じなら member をビルドし (決定 2、`resolved_from: explicit`)、違えば crates.io を引いて warning。member でない・解決中・`cargo metadata` の失敗では今どおり。`Request` に `local` (`LocalSource`: ビルド前の `Wanted`、ビルド後の `Built`) を持たせ、`look_up_release` は member なら `.crate` の取得と docs.rs を飛ばして `look_up_member` へ (ビルドを待ち、`Origin::Local` で引く。re-export を追えなかったときの引き直しは `Built` を使い、ビルドし直さない)。パスの先頭は lib の名前に置き換える
- `ToolState::locals` (`Flights::handing_over`)
- re-export の行き先が crates.io でないプロジェクトのパッケージなら追わない (決定 6): `destination_request` が `docs_unavailable` を返し、既存の後始末で re-export そのものを warning 付きで返す。これは docs.rs の crate からプロジェクトの path 依存への re-export にも効く (前は crates.io の最新に落ちていた)
- `local_build_required` の文言を「member だけをビルドする」に直した
- テスト (stdio): member を実際にビルドして答える (`source: local`・`resolved_from: cargo_lock`・`built_at`。`building` なら呼び直す。stdout の全行が JSON-RPC であることと出力のスキーマは、stdio のテストの仕組みが確かめる。R-SEC-1 の終わりの印)、member と同じ版の `demo@0.1.0` は member (`explicit`)、別の member の re-export は追わず warning。プロジェクトの `build.target` のテストは、パッケージ名を `demo` 以外にして docs.rs のキャッシュを引くようにした
- 前提を渡さないレビュー (サブエージェント) の指摘と対応:
  - 版を明示した普通の crate の照会が、プロジェクトのエラー (Cargo.lock が無い・壊れている、`project` が既定のプロジェクトの外、workspace が複数) で失敗するようになっていた (前は `crates_io_in_project` がエラーを飲み込んで crates.io を引いていた)。member の確かめでも、エラーは member でないとして扱う。stdio のテストに、Cargo.lock の無いプロジェクトと外のプロジェクトで `demo@0.1.0` を docs.rs から引くことを足した
  - member を `-` と `_` の違う名前 (コードでの書き方) で引くと、`Request` の名前が利用者の綴りのままで、パッケージ名で引く `FeatureSets` に当たらず、全 item が `enabled_in_project: false` になっていた。`Request` の名前を member のパッケージ名にした (来歴の `crate` もパッケージ名)。stdio のテストに、`hello-world` を `hello_world` で引いて `enabled_in_project: true` になることを足した
  - 版を明示して member に当たったとき、R-VR-1 の「crates.io の同じ版とは別物でありうる」warning が無かった。足した
  - member を名前の文字列で照らし直していたので、依存側で改名した member などを取りこぼしていた。プロジェクトが解決したパッケージ (名前と版) から member を直接選び (`build::member_package`)、その crate を、照会の名前の lib か bin、無ければ lib、無ければ 1 つだけの bin とする (`build::crate_of`)。版を明示した照会は、版が違えば cargo の設定を読む前に終える (`Member::OtherVersion`)
  - 残したもの: 版を明示した照会で resolve を 2 回する (2 回目は保持した結果の鮮度の確かめだけ)。解決中や `cargo metadata` の失敗では warning なしで crates.io を引く (決めたとおり。warning を付けると、失敗が続く間すべての照会に付く)。`diagnostics` と stderr の長さの上限は D5
- 実際のプロジェクトでの試し (このリポジトリ自身、デバッグビルドのサーバー、オフライン): 冷えた状態の `get_item data::build::Plan` は 62.8 秒で、20 秒と 43 秒に `building` が返り、3 回目で答えた (裏で続いたビルドの結果が渡った)。温まった状態の照会は 4〜5 秒。同じ target dir で `cargo rustdoc` を直接走らせると 4.5〜5.0 秒で、228 unit のうち作り直すのはこの crate だけ (値の固定が効いている)。JSON は約 10 MB。照会のたびの数秒は rustdoc そのもので、D4 の鮮度の判定でしか縮まない
- 独自の target (決定 7) の実測 (この PC): `.cargo/config.toml` に `build.target = "my-target.json"`・`[unstable] build-std = ["core"]`・その両方を置いて `cargo rustdoc --target <host>` を走らせると、stable 1.99.1 の cargo では 3 つとも成功した (`--target` が `build.target` に勝ち、stable の cargo は `[unstable]` を使わない)。stdio のテストに足した (host 向けにビルドし、「target of its own」の warning)。nightly の cargo は `build-std` を使い、rust-src から std をビルドしようとして、オフラインでは std の依存が無く失敗した (`no matching package named`。オフラインでは `offline_miss`)。形式が古くて nightly に切り替え、かつ build-std を設定したプロジェクトは、オンラインなら std ごとビルドして遅く、オフラインなら `offline_miss` になる。まれなので D7 で扱う

**D2 の後の順番 (2026-10-21)**: D2c-3 のうち Claude Code での試しは、照会のたびに rustdoc を走らせる遅さ (温まって 4〜5 秒) が目立つので、D4 (鮮度) の後に回すと利用者が決めた。次は計画どおり D3 (D4 の鮮度の鍵に、D3 で決める features の組が入るため)

**D3 の下調べ (2026-10-21、サブエージェント、読むだけ)**: D3 の範囲は、ビルドの枠 (`limits.concurrent_builds`、`waiting_for_slot`。§3.7 のローカルビルドの数の上限と同じもの)、`build.wait_secs`、`build.timeout_secs` (今は `cargo.rs` の `MAX_RUN_MINUTES` に固定)、`--cfg docsrs` を外しての再試行 (Q-11、`build.cfg_docsrs`)、member の features の組 (R-FT-3: 通常のビルドの組を `--no-default-features --features` で、`[package.metadata.docs.rs]` の組、R-MCP-3 の範囲の外の依存、壊れた組の再試行、`features_requested`)、Q-31 (複数の member・ワークスペースの path 依存・`default-members` での組の違い)。共有の仕事と取り消し (R-WS-6) は C2b・C3・D2c で済んでいる。分け方の案: D3a (枠と待ち)、D3b (docsrs の再試行)、D3c (Q-31 の実測)、D3d (通常の組の明示)、D3e (`[package.metadata.docs.rs]` の組)。要件・前の決定から聞かずに決めること: `build.cfg_docsrs` を足す (要件の設定の表)、member の `[package.metadata.docs.rs]` は member の Cargo.toml を `crate_file` の `build_of` で読む (R-SEC-4 の検証がある)、`features_requested` と `features_effective` を両方記録し、鍵にどちらを使うかは D4

**D3a (ビルドの枠と待ち。2026-10-21)**:

- 利用者が 1 件ずつ決めたこと:
  1. ビルドの枠 (`limits.concurrent_builds`、既定 2、プロセスごと) は cargo の 1 回の実行ごとに持つ。再試行のたびに手放して取り直す。`cargo metadata`・`cargo tree`・形式の試しは数えない (R-SEC-7 の「ローカルビルド」)。取るのは決めたロックの順のとおり、`doc:` のロックの後で cargo の直前
  2. 「呼び出しの上限を超えたとき、ビルドを待っているなら `status: building`」(R-SEC-7) は後 (D5 か F) に回す。今は `busy` のまま
  3. `build.wait_secs` (既定 10) は、ビルドを待ち始めたときから数え、呼び出しの期限との早いほうを使う。呼び出しが仕事を待つ時間 (`limits.request_timeout_secs` の 3 分の 2) より長ければ、そちらが効き、起動時にログで知らせる
  4. `build.timeout_secs` (既定 1,800) は cargo の 1 回の実行ごと (ビルド・`cargo metadata`・`cargo tree`)。サーバーのロックと枠を待つ時間は数えない。超えたら子を止めて `build_failed` (`reason: timeout`)
- 聞かずに決めたこと: 期限を過ぎて裏で続くビルドが枠を持ち続けるのは、R-WS-6 (裏で続くビルドは時間切れかサーバーの終了でしか止めない) のとおりにする (§4 の 197 行目の論点)
- 実装: `build::Slots` (セマフォ。取れなければ `Doing::WaitingForSlot` を知らせて待つ)、`build::build` の様子の知らせ方を `Activity` から `Doing` に (cargo の様子は `Activity::of_build` で写す)、`Doing::WaitingForSlot` (`waiting_for_slot`)、`cargo::set_run_time` (`MAX_RUN_MINUTES` の固定を置き換え、`RunError::TimedOut` に秒数)、`tool::wait_for_builds` と `ToolState::build_wait`、`look_up_member` の期限、設定 3 つと README
- テスト: 枠がすべて使われているときの待ちと知らせ、時間切れのエラーと秒数、`build.wait_secs` の既定・設定・上限
- 前提を渡さないレビュー (サブエージェント) で重大な問題は無かった。小さな指摘を直した: `build.wait_secs` が呼び出しの待ちより長いという warning は、利用者が明示したときだけ (`limits.request_timeout_secs` を小さくしただけなら debug)。ビルドの後の索引作り (`Indexing`) まで `build.wait_secs` で打ち切っていたので、その期限で索引作りに入っていれば呼び出しの期限まで待ち続ける (R-WS-6「ビルド以外の処理は R-ARCH-6 の上限」)。`build::set_this` で `&&` の短絡により設定が捨てられうるのを直した。README の `build.timeout_secs` に `cargo tree` を足した

**D3b の決定 (2026-10-21)**: 利用者が 1 件ずつ決めた。

1. `--cfg docsrs` を外して再試行するのは、compiler の `error` が文書化する member のもの (package id で照らす) のときだけ。`--cfg docsrs` は最後の rustdoc にしか渡さないので、壊れるのはそこだけ。依存やネットの失敗では再試行しない。再試行したら warning
2. `--cfg docsrs` で壊れた member は、package id と toolchain を鍵にプロセスの間だけ覚え、次からは外してビルドして warning。ディスクに残して `refresh` で消すのは D4・D5
3. member の `[package.metadata.docs.rs]` の `rustdoc-args` は、`--cfg <名前>` と `--cfg=<名前>` だけを、名前が cfg の文法に合うときに最後の rustdoc に渡す (要件の案)。入れるのは member の Cargo.toml を読む D3e。その cfg で壊れたら docsrs と一緒に外して再試行する

**D3b (`--cfg docsrs` を外しての再試行。2026-10-21)**:

- 実測 (stable 1.99.1): `#![cfg_attr(docsrs, feature(doc_auto_cfg))]` の crate は、`--cfg docsrs` を付けると文書化する crate 自身に `E0557` (feature has been removed) と `Compilation failed, aborting rustdoc` が出て失敗し、外すと成功する。compiler のメッセージには `package_id` と `target` がある
- 設定 `build.cfg_docsrs` (既定 true。README)。`Plan::cfg_docsrs` で `--cfg docsrs` を付けるかを決める。`BuildRecord::cfg_docsrs`
- `Messages` が、文書化する crate (package id と target で照らす) の `error` を見分け、`BuildError::Failed::in_documented` に
- `query::local::build_with`: `--cfg docsrs` 付きで文書化する crate のエラーで失敗したら、(package id, サーバーが選んだ toolchain) をプロセスの間覚えて、外してもう 1 回ビルドする。覚えてあれば最初から外す (決定 1・2)。記録が `--cfg docsrs` なしで設定がオンなら warning
- テスト: 文書化する crate の error だけを見分ける (依存の error・warning では見分けない)。fixture に `broken` (`doc_auto_cfg`) と `typo` (`#[cfg(docsrs)]` の item を持つ) を足した。`broken` は `--cfg docsrs` なしで作り直して warning が付き、`--cfg docsrs` 付きでも通るソースに直しても覚えているので外したまま (`docsrs_only` が出ないことで、1 回目の試行を飛ばしたとわかる)。`typo` に普通のコンパイルエラーを入れて失敗させ、直すと `--cfg docsrs` 付きに戻る (コードを「失敗しても覚える」に変えるとこのテストが落ちることを確かめた)
- 前提を渡さないレビュー (サブエージェント) の指摘と対応: 普通のコンパイルエラーでも再試行の前に「`--cfg docsrs` で壊れた」と覚えていた (直した後も外れたままで、誤った warning が付き続ける) ので、外した再試行が成功したときだけ覚える。両方失敗したら 2 回目 (外したほう) のエラーを返す。nightly で作り直すとき、前のビルドで外した状態を引き継いでいたので、元の値に戻す。記録の鍵の toolchain は、分かれば `rustc -vV` の要約 (途中で `rust-toolchain.toml` を変えても古い記録が効かない)。`LocalBuilt` に JSON のパスを持たせた (テストと D4 のため)。再試行して両方失敗したとき、外して試したことは応答に出ない (D5 で検討)

**D3c (Q-31 の実測。2026-10-21、cargo 1.99.1、resolver 2)**: scratchpad に、member `a`・`b`・proc-macro の `pm`、members に書いていないワークスペースの中の path 依存 `inner`、`default-members = ["a"]`、外の依存 `x` を作った。`a` は `x` (fa)・`inner` (既定)・`pm` に、`b` は `x` (fb)・`inner` (e)・`a` (feature `ae`) に、`pm` は `x` (fp) に依存する。`cargo metadata` の `workspace_members` には `inner` も入る (暗黙の member)。features は次のとおり:

| パッケージ | `cargo tree --workspace -e normal` | `cargo rustdoc -p a` | `cargo build` (default-members) | `cargo build --workspace` | `cargo rustdoc -p b` |
| --- | --- | --- | --- | --- | --- |
| `a` | ad, ae, default | ad, default | ad, default | ad, ae, default | ad, ae, default |
| `inner` | d, default, e | d, default | d, default | d, default, e | d, default, e |
| `x` (target) | fa, fb, fp | fa | fa | fa, fb, fp | fa, fb |
| `x` (host) | fp | fp | fp | fp | fp |

- `-p a` は引数なしの `cargo build` (default-members) と同じ組、`cargo tree --workspace` は `cargo build --workspace` と同じ組で、`enabled_in_project` (`cargo tree --workspace` から) と D2 のビルド (`-p`) が食い違う: `a` の `ae` は workspace では有効 (`b` が有効にする) だが、`-p a` のドキュメントには `ae` の item が無い
- rust-analyzer は既定で `--workspace` を check するので、エディタで見えるのは統合された組
- `-p a` に features を明示しても、依存 (`x`) の組は `a` の分しか統合されない。`a` のドキュメントに出るのは `a` 自身の item なので、依存の組の違いは re-export の行き先 (D6) の話

**D3d (member の features の組。2026-10-21)**:

- 利用者が 1 件ずつ決めたこと:
  1. member は、ワークスペース全体で統合した組 (`cargo tree --workspace -e normal` でその member が持つ組) を `--no-default-features --features` で明示してビルドする。`enabled_in_project` と一致し、rust-analyzer が既定で見る組 (`--workspace`) とも合う (R-FT-3 の「通常のビルドで有効な組を再現」)。依存の組はその member の分しか統合されないが、member のドキュメントには効かない
  2. その組が得られないとき (`cargo tree` の失敗、Cargo.lock だけの解決、通常のビルドで target 向けに作られない member) は、`-p` の既定でビルドして warning
  3. target 向けと host 向けの組の選び方 (`features.rs` の規則) は今のまま。逆向きの依存木に合わせるのは、依存のビルドを扱う D7
- 実装: `Plan::features`、`BuildRecord::features_requested`、`LocalWanted::warnings` (計画のときの warning をアーカイブのときに足す)、`mcp::tool::local::member` が `Resolver::features` から組を引く、来歴とスキーマの `features_requested` (D2c の決定 5 で D3 まで出さないとしたもの)
- テスト: コマンドの引数 (`--no-default-features --features default,extra`、組が無ければ付けない)、stdio で別の member が有効にする feature の item (`front::extra`) が引けて `features_requested: ["extra"]`・`enabled_in_project: true` (member の組を使わないように変えるとこのテストが落ちることを確かめた)
- 前提を渡さないレビュー (サブエージェント) の指摘と実測:
  - resolver 1 でルートにパッケージがあるワークスペースでは、`--features`・`--no-default-features` が `-p` の member ではなく cwd (ルート) のパッケージに効く: 実測で `cargo rustdoc -p b --no-default-features --features x` が `package app ... does not have the feature x` で失敗し、`--manifest-path b/Cargo.toml` を足すと (cwd はルートのまま) `b` に `x` が効いた。ビルドに member の `--manifest-path` を足した (`Plan::manifest`。cwd はルートなので、プロジェクトの設定と `rust-toolchain.toml` は今どおり効く)。stdio のテストを足し、`--manifest-path` を外すと落ちることを確かめた
  - 「proc-macro の member だけが使う lib の member に、host 向けの features が target 向けのビルドに混ざる」は、実測では当たらなかった: `cargo build --workspace --target wasm32-wasip1` でも、target 向けの `helper` は host 向けの依存から来た `std` 付きでビルドされた (cargo は選んだ member の組に host 向けの依存からの features も統合する)。サーバーの組は cargo の実際の組と同じ
  - 名前と版が同じで取得元の違うパッケージの組が混ざると、member に無い feature を渡して失敗しうる: `cargo metadata` の `features` (宣言された features と optional な依存の名前) を `metadata::Package::features` に読み、組をそれに絞る (`build::declared_features`)。あわせて feature 名の検証 (`crate_file::is_feature_name`、R-SEC-4) を通す
  - 残したもの: ワークスペース全体の組で member 自身が壊れる (相互排他の features) ときに `-p` の既定に戻らない (`cargo build --workspace` も壊れるプロジェクトの問題として `build_failed`)。組を引く `cargo tree` が期限の外で走る (前もビルドの後に同じものを引いていたので合計は増えない)

**D3e の決定 (2026-10-21)**: 利用者が 1 件ずつ決めた。

1. member の `[package.metadata.docs.rs]` の組 (`features`・`all-features`・`no-default-features`) は、その組で `cargo tree -p <member>` を走らせ、通常のビルドの範囲 (B7 の `build_scope`) に無いパッケージが増えなければ、その組でビルドする。増えるなら通常の組 (D3d) でビルドし、「docs.rs の組は `build_external_docs` で」と warning (R-MCP-3)。`enabled_in_project` は通常の組のまま (R-FT-3)
2. docs.rs の組で壊れ、通常の組で通った member は、`--cfg docsrs` と同じく、再試行が成功したときだけ package id と toolchain を鍵にプロセスの間覚え、次からは最初から通常の組で warning
3. 再試行の順: 文書化する crate のエラーなら先に `--cfg docsrs` (と `rustdoc-args` の `--cfg`) を外す。それでも失敗するか、依存のエラーなら通常の組に戻す。ビルドは最大 3 回。それぞれの壊れは別々に覚える

**D3e (member の `[package.metadata.docs.rs]`。2026-10-21)**:

- `crate_file::member_docs_rs`: member の Cargo.toml を `build_of` で読む (R-SEC-4 の名前の検証が入っている)。`DocsRsBuild` に、`[package.metadata.docs.rs]` があるか (`asked`) と、`rustdoc-args` の値の無い `--cfg` の名前 (`rustdoc_cfg`。`docsrs` と、値付きの `--cfg k="v"` と `rustc-args` の分は除く) を足した
- `Resolver::outside_scope` (`build_scope::outside`): docs.rs の組で `cargo tree --manifest-path <member> -p <member> --no-default-features --features … -e normal,build` を走らせ、通常のビルドの範囲 (`BuildScope`) に無いパッケージを返す。`cargo metadata` に無いパッケージ (有効になっていない optional な依存は `cargo metadata` に出ない) も範囲の外
- `mcp::tool::local::docs_rs_of`: docs.rs の組 (宣言された features に絞る) が通常の組に含まれればそのまま、そうでなければ範囲の外が増えないときだけ使う。増えるなら通常の組で warning (`build_external_docs` で)、確かめられなければ通常の組で warning。`rustdoc-args` の `--cfg` は `Plan::cfgs` に入れ、`--cfg docsrs` と一緒に付け外しする (D3b の決定 3)
- `query::local::Builder::build`: docs.rs の組 + `--cfg docsrs` (+ `cfgs`) で始め、文書化する crate のエラーなら `--cfg docsrs` を外し、それでも、または依存の、コードのエラー (`Failure::Other`) で失敗したら通常の組に戻す (最大 3 回)。それぞれ、外して通ったときだけ (package id, toolchain) をプロセスの間覚える (`DOCSRS_BROKE`・`DOCS_RS_FEATURES_BROKE`)。docs.rs の組でビルドしたら「`enabled_in_project` は通常の組」と warning、通常の組に戻したら warning。nightly で作り直すときは `cfg_docsrs` と組を最初の値に戻す
- テスト: `rustdoc-args` の `--cfg` (値付き・`docsrs`・`rustc-args`・名前でないものを除く)、`asked`。stdio で 1 つのワークスペースの 3 つの member: 範囲の中の docs.rs の組 (`extra` の item が出て `features_requested: ["extra"]`、`rustdoc-args` の `--cfg my_cfg` の item も出る)、`all-features` がコンパイルエラーになる組 (通常の組で作り直して warning)、外の optional な依存を引き込む組 (通常の組で「would build opt 0.1.0」の warning)

**D3e の後の直し (2026-10-21、D3e のコミットの後に届いたレビュー)**:

- `[package.metadata.docs.rs]` の表があるだけで docs.rs の組を使っていた (`rustdoc-args` しか書いていない member が `--no-default-features --features default` になり、D3d で入れた組の一部が消え、毎回 warning)。R-FT-3 のとおり `features`・`all-features`・`no-default-features` のどれかがあるとき (`DocsRsBuild::asks_features`) だけ使い、組が通常の組と同じなら使わない
- nightly で作り直すとき、捨てたビルドの warning が残って重なっていた。作り直す前に戻す
- features が原因で 3 回目 (通常の組) まで行くと、`--cfg docsrs` を外したままビルドし、「`--cfg docsrs` で壊れた」と誤った warning が付き、次の呼び出しでは付いて中身が変わっていた。3 回目は `--cfg docsrs` を最初の値に戻す (features と `--cfg docsrs` の両方で壊れる member は毎回失敗する。まれ)
- 通常の組に戻すのは、`Failure::Other` で compiler の `error` があるときだけ (ネットの一時的な失敗やインストールの失敗で戻して「壊れた」と覚えない)
- 範囲の確かめの `cargo tree` を呼び出しのたびに走らせていたので、`Resolver` が (ワークスペース、target、package id、features) ごとに、解決が変わるまで覚える
- `cargo metadata` に無いパッケージを、名前と版が同じ別の取得元のパッケージと取り違えうるので、範囲の確かめでは取得元も常に照らす (`Ids::exact`)
- `--cfg docsrs` を外したときの warning に、`rustdoc-args` の `--cfg` も外したことを書いた
- テスト: `rustdoc-args` しか書いていない member (`argsonly`) は通常の組で、warning が無く、`--cfg` は効く。`clash` は「`--cfg docsrs` なしで」の warning が無い。`wide` の `features_requested` は通常の組
- 残したもの: 記憶の鍵に docs.rs の組と `cfgs` を入れること (`[package.metadata.docs.rs]` を書き換えても、プロセスの間は前の記憶が効く)。鍵は決めたとおり package id と toolchain のままで、内容のダイジェストを鍵にディスクへ残すのは D4 で決める

**D4 の下調べと決定 (2026-10-21)**:

- 下調べ (サブエージェント、読むだけ): R-WS-4 は鍵に入れるものを列挙している (各 target の `src_path` の部分木・build.rs・Cargo.toml・Cargo.lock・cargo の設定 (`include` も)・path 依存のソース・features と rustdoc の引数・`RUSTC_BOOTSTRAP` の値・target・使った toolchain の `rustc -vV`・ビルドに効く環境変数)。鍵はビルドの前に取り、dep-info で分かったファイルを後で足し、ビルドの前より新しい mtime のファイルがあれば次は鮮度切れ。mtime は一次の絞り込みだけ。dep-info は同じ引数に `--emit=dep-info` を足した 2 回目の rustdoc (`<doc dir>/<crate>.d`。JSON は書かない。`cargo check` の `.d` は `#[cfg(docsrs)]` だけで読むファイルを落とす)。作り直しの間は前回の成功を `stale: true`・`stale_reason: rebuilding`・`built_at` 付きで、失敗は `build_failed` (R-WS-7)。M0 の B5: dep-info は `include_str!`・`#[path]`・OUT_DIR の生成物と `# env-dep:` を出す。build.rs 自身と、build.rs が `rerun-if-changed` で読むファイルは出ない。使い回せるもの: `cargo metadata` の鮮度の入力 (`metadata::input_files`・`with_included`)、`cargo_config::CargoConfig::files`、`Plan`、`archive_of`
- 利用者が 1 件ずつ決めたこと:
  1. 鍵は計画した入力 (docs.rs の組・通常の組・`--cfg docsrs` の設定・`rustdoc-args` の cfg など、ビルドの前に分かるもの) で作る (Q-31)。実際に使ったものは記録に残す
  2. 各ファイルは (パス・大きさ・mtime・SHA-256) を記録し、大きさか mtime が違うものだけ読み直して内容を比べる。記録の時刻に近い mtime のものは常に読み直す (粒度の粗いファイルシステムのため)
  3. dep-info の 2 回目の rustdoc は、JSON を公開した後、同じ共有の仕事の中で `doc:` のロックと枠を取り直して走らせ、記録に足す
  4. ソースが変わって作り直すときは、`build.wait_secs` まで待ち、間に合わなければ同じ crate の前回成功した JSON を `stale: true`・`stale_reason: rebuilding`・`retry_after_secs`・`built_at` 付きで返し、LSP で確かめるよう案内する。前回が無ければ `status: building`
  5. 置き場は今のまま (crate ごとに 1 つ)、鍵と入力の一覧は記録に
  6. `--cfg docsrs` や docs.rs の組で壊れた記録は、鍵 (内容のダイジェスト) と toolchain でディスクに残す (`refresh` (D5) で消せる形)。プロセスの中の記憶は外す
  7. ローカルビルドの `Flights::handing_over` はやめ、ディスクの鮮度の判定に任せる (仕組みは残す)
  8. 環境変数は、`RUSTFLAGS`・`RUSTDOCFLAGS`・`CARGO_ENCODED_RUSTFLAGS`・`CARGO_ENCODED_RUSTDOCFLAGS`・`RUSTC`・`RUSTDOC`・`RUSTC_WRAPPER`・`RUSTC_WORKSPACE_WRAPPER` と、`CARGO_BUILD_*`・`CARGO_TARGET_*`・`CARGO_PROFILE_*`・`CARGO_UNSTABLE_*`。build.rs が読む変数は dep-info の `env-dep` で入る
  9. ソースは部分木をたどり、dep-info のファイルも足す。target・`.git`・`.claude/worktrees`・入れ子のワークスペースはたどらない
  10. たどるのは member の Cargo.toml のあるディレクトリ全体 (入れ子のパッケージは除く): パッケージの中の build.rs の入力 (`proto/` など) も拾う
- 聞かずに決めたこと: LSP の案内は stale の応答に付ける (R-WS-9)。記録の形式の版はパスに (`local/v1/`。C1 の決定)。D4 は member だけで、依存の鍵に足すもの (取得元・依存の部分木・`[source]`・`[patch]`) は D7。確かめるのは呼び出しのたび (要件に TTL は無い)。`--emit=dep-info` を受け付けない toolchain では dep-info なしで進める (パッケージ全体をたどるので足りる)
- D4 は 4 つに分ける: D4a (鍵と鮮度が合えばビルドしない近道、記録の v1、パッケージのたどり方、`handing_over` をやめる)、D4b (dep-info)、D4c (作り直しの間の stale の応答と LSP の案内とスキーマ)、D4d (壊れた記録をディスクに、時間の測定、Claude Code での試し)

**D4a (鮮度の鍵と近道。2026-10-21)**:

- `data::freshness`: 鍵に入れるファイルを集め (`collect`)、(パス・大きさ・mtime・SHA-256) を記録する。前回の記録と大きさか mtime が違うファイル、または取った時刻に近い (3 秒以内) mtime のファイルだけ読み直す (決定 2)。鍵 (`key`) は計画の項目 (`Plan::facts`) とファイルの (パス, SHA-256) の SHA-256。5 万ファイルを超えたら鮮度を保たない (毎回ビルド)
- 集めるもの: `cargo metadata` の鮮度の入力 (Cargo.lock・path のパッケージのマニフェスト・cargo の設定と `include`・`rust-toolchain`。`Resolver::input_files`) と、member と、それが通常の依存かビルドの依存としてたどる path のパッケージのディレクトリ (`build::source_dirs`)
- 実測で決め直したこと: 決定 10 (パッケージのディレクトリ全体) のまま、このリポジトリで試すと、`poc/m0b/work` (`.gitignore` で無視、9.3 GB、cargo の build dir) まで読んで、入力が 30,109 ファイル・11 GB になり、初回 288 秒、2 回目から 0.6 秒だった。利用者の決定で、git の作業ツリーの中では `git ls-files --cached --others --exclude-standard` (追跡中と、無視されていない未追跡) で集める (`freshness::sources`。`core.fsmonitor=false`・`GIT_OPTIONAL_LOCKS=0`)。git の外ではたどり、`CACHEDIR.TAG` のあるディレクトリも除く。どちらも `target`・`.git`・`.claude`・入れ子のパッケージは除く。直した後は入力が 193 ファイル・3.8 MB、作り直しの 1 回目 8.7 秒、鮮度が合えば 0.0〜0.1 秒 (D4a の前は温まっても 4〜5 秒)
- `Plan::facts`: package id・マニフェスト・lib か bin・`rustc -vV`・サーバーが選んだ toolchain・`RUSTC_BOOTSTRAP` の値の SHA-256・`--target`・`--cfg docsrs` の設定・`rustdoc-args` の cfg・通常の組・docs.rs の組・環境変数 (決定 8 の一覧の SHA-256。プロセスで 1 回)。計画した入力で、再試行で変わる前 (決定 1)
- 記録 v1: 置き場を `local/v1/…` に (C1 の決定のとおり版をパスに)。`BuildRecord` に `key`・`inputs`・`warnings` (再試行で出た warning。鮮度が合って飛ばしたときも同じものを返す) を足した (`#[serde(default)]`)
- `query::local::Builder::fresh`: toolchain を確かめて `--target` を決めた後、ビルドの前に鍵を求め、前回の記録の鍵と同じで JSON があれば、ビルドせずに返す (枠もロックも取らない)。host が分からないとき (ビルドの中で toolchain を入れるとき) は近道しない。ビルドの再試行はどれも同じ鍵と入力を記録に書く
- ローカルビルドの `Flights` は `handing_over` をやめ、鍵の無いビルドだけを渡す (`Flights::handing_over_only`・`query::local::hands_over`)。決定 7 の帰結として、鍵を取れない member (5 万ファイル超など) は、ビルドが `build.wait_secs` を超えるたびに作り直し、`status: building` から抜けなくなるため (レビューの指摘)
- テスト: `freshness` の集め方 (除くもの)・鍵 (同じなら同じ、計画か編集で変わる)・時刻が近いときの読み直し。隔離した子で、編集しなければ作り直さず (`inputs.taken_at` が変わらない)、member のソース・パッケージの中の `proto/a.proto`・ワークスペースの外の path 依存のソースを編集すると作り直す。git の作業ツリーで、追跡中と未追跡が入り、無視したもの・`target`・入れ子のパッケージは入らず、git の外はたどる側に回る
- レビューで直したこと:
  - 古いビルドを新しいとして返していたもの:
    - `target` と `.claude` を除くのはパッケージの直下だけにした (`src/target/mod.rs` を拾う。`cc` に実例がある)。`.git` はどの深さでも除く。
    - git の一覧にパッケージの `Cargo.toml` が無ければ (パッケージごと git に無視されている)、たどる側に回す。
    - git がディレクトリとして挙げるもの (サブモジュール・入れ子のリポジトリ・ディレクトリへの symlink) はたどる。
    - たどるときは symlink を追う。ディレクトリへのものは実体のパスで 1 回だけたどる。
  - 鍵が取れなくなっていたもの:
    - たどった先の読めないディレクトリと、名前が UTF-8 でないファイルは、ログに残して飛ばす。読めないのが member のディレクトリ自体のときだけ、鍵を取らない。
  - `Plan::facts` を直した:
    - サーバーの版を入れた (更新でコマンドが変わっても、前の JSON を新しいとしない)。
    - proc-macro では target を入れない (target を変えて交互に頼んでも作り直さない)。
    - 環境変数から、サーバーが上書きする `CARGO_TARGET_DIR`・`CARGO_BUILD_TARGET_DIR`・`CARGO_BUILD_BUILD_DIR` と、出力に効かない `CARGO_BUILD_JOBS` を除いた。
  - 記録の書き直し (`Built::write_record`) を変えた:
    - 公開と同じロックの下で行う。
    - JSON の隣の `.sha256` が自分の `sha256` と同じときだけ書く。
    - `build::published` も `.sha256` と記録の `sha256` を照合し、違えば作り直す側に回す。別のプロセスが公開した JSON に、自分の記録が重なるのを防ぐため。
  - SHA-256 はファイル全体を読み込まずに、少しずつ読んで取る (`cache::sha256_hex_read`)。
- 残したもの:
  - `rustc -vV` は 60 秒覚える (`tool::output_kept`)。`rustup update` の後 60 秒の間は、前の toolchain のビルドを新しいとしうる。
  - パッケージの外のファイル (`include_str!("../../README.md")` など) と `env!` は D4b (dep-info) まで拾わない。
  - git が無視するファイルを build.rs が読む場合は、build.rs が `rerun-if-changed` で挙げていなければ、D4b の後も拾わない。
  - ビルドの間にファイルの mtime が戻されて、大きさも同じ場合は拾わない (決定 2 の帰結)。

**D4b の下調べ (2026-10-21)**:

- `--emit=dep-info` は書き先を指定しないと、古い nightly (2025-10-10) が cwd (= 利用者のワークスペースのルート) に `<crate>.d` を書く。`--emit=dep-info=<work dir のパス>` なら、stable と nightly (2025-10・2026-06・今) のどれも指定先にだけ書き、JSON は書かない
- `.d` は絶対パスで、`include_str!`・`#[path]`・OUT_DIR の生成物・パッケージの外のファイル・`#[cfg(docsrs)]` だけで読むファイル (`--cfg docsrs` を渡したとき) を挙げる。`# env-dep:NAME[=値]` の行もある (値が無ければ変数が無い)
- このリポジトリで dep-info の 2 回目の rustdoc は 5.57 秒 (`.d` は 92 行)
- ワークスペースの member では、`.d` の相対パスはワークスペースのルート (cargo が rustdoc を走らせる場所) からのもの (`one/src/lib.rs`)。ファイル名の空白は前に `\` を付けた形に、`#` と `:` はそのまま。`env-dep` の値は `\` が `\\`、改行が `\n` になる
- 通常のビルドで cargo は rustdoc の dep-info を出さない (cargo 1.99.1)。2 回目の rustdoc は要る
- 利用者の決定: dep-info の 2 回目の rustdoc は、呼び出しに JSON を返した後に裏で走らせる (作り直しの応答を遅らせない)。走っている間に来た呼び出しは、dep-info を足す前の鍵で判定する

**D4b (dep-info。2026-10-21)**:

- `build::dep_info`: 作り直しで公開した後、同じ計画 (再試行で変わった後のもの) に `--emit=dep-info=<work の中のパス>` を足して cargo を走らせ、`.d` と、各 build script の `output` (cargo が `out_dir` の隣に残す、build script が cargo に伝えたもの。`build-script-executed` のメッセージで場所が分かる。build script が fresh でもメッセージは出る) を読む。`build` と同じく target dir の共有のロック・`doc:` のロック・枠を取る (`take_turn` にまとめた)。toolchain が JSON を書いていたら消す
- `freshness::Deps` (記録では `dep_files`・`dep_dirs`・`env_deps`) に入れるもの:
  - `.d` のファイル (`..` を含むものは、あれば実体のパスで。symlink の後の `..` を OS と同じに読むため)
  - member と path 依存の build script の `rerun-if-changed` のファイルとディレクトリ (相対はパッケージのディレクトリから)、`rerun-if-env-changed` の変数。D4 の決定 8 の「build.rs が読む変数は dep-info の `env-dep` で入る」は誤りだった (`.d` に出るのは文書化する crate の `env!`・`option_env!` だけ)。利用者の決定で、cargo が build script を走らせ直す条件と同じものを鍵に入れる
  - `.d` の `env-dep` のうち、サーバーの環境の値と同じもの (無いものは無いまま)。cargo や build script が設定した変数 (`CARGO_PKG_NAME`・`OUT_DIR`・`rustc-env` のもの) は、その元が鍵に入っている
  - build script が target dir に書いたもの (OUT_DIR) は出力なので除く
- 鍵は、ビルドの前に取った入力に dep-info で分かったファイルを足し、前回の dep-info だけが挙げて今回は挙げないもの (パッケージの一覧とたどったディレクトリに無いもの) を除いたものと、`Plan::facts` に変数の値の SHA-256 を足したもの。次の判定は、パッケージのファイルに前回の `Deps` のファイルとディレクトリを足して集め、前回の変数の今の値で鍵を求める
- ビルドの前に取った時刻 (`taken_at`) から 3 秒前より後に変わった dep-info のファイルは、digest を持たない (`freshness::collect_after`): ビルドの間の編集を拾い、次の判定で作り直す。ビルドの直前に外のファイルを編集すると、1 回余計に作り直す
- 記録の `dep_info` (試したか): 新しいと判定したビルドに印が無ければ (サーバーが dep-info の前に終わったなど)、記録の入力 (ビルドの前のもの) のまま、その場で裏の dep-info を始める。同じビルドの dep-info は (JSON, 鍵) ごとに、プロセスの中で 1 つずつ (`Following`。最初は JSON ごとにしていて、前のビルドの dep-info の後片付けの間に公開した次のビルドが飛ばされていた)。始める前と書く前に、ディスクの記録が見つけたときと同じ (鍵が同じで印が無い) か確かめ、違えば何もしない (`Built::write_record_if`)。別のプロセスが先に済ませた記録を上書きしない
- dep-info の 2 回目の rustdoc は、ビルドが読んだものではなく、そのときのソースを読む (応答の直後に編集して戻すと、パッケージの外のファイルが鍵から落ちていた。2 回目のレビューで再現)。そこで、判定のときに見た入力 (`Fresh::seen`: 作り直しならビルドの前、新しいと判定したならそのとき) が同じか、2 度確かめる (`freshness::unchanged`)。比べるのは内容 (digest) で、大きさか mtime が違うファイルだけ読み直す (最初は大きさと mtime で比べていて、build script がパッケージの中のファイルを同じ内容で毎回書き直すと、呼び出しのたびに作り直していた。3 回目のレビューで再現)
  - dep-info の cargo を走らせる前、ロックと枠を取った後: 記録が見つけたときのままで、ソースが同じときだけ走らせる (`build::dep_info` の `ready`)。違えば何もしない (次の呼び出しが判定する)。新しいビルドの後に古い dep-info が数秒走るのも、これで避ける
  - 走らせた後: 違えば、dep-info がビルドと違うソースを読んだかもしれないので使わず、記録の鍵を外して次の呼び出しで作り直させる。走っている数秒の間に編集して戻されたものは拾えない
- 失敗の扱い: 記録には何も書かず、(JSON, 鍵) ごとにプロセスの間覚えて、新しいと判定するたびに走らせ直さない (失敗には途中の編集や一時的なものもあり、ディスクに残すと二度と取り直さないため)。覚えているビルドを新しいと判定したら、応答に warning を付ける (パッケージの外のファイルの編集で作り直さない旨)。記録の書き込みの失敗と、build script の `output` が読めないこと (cargo の置き場が変わると、見張るものが黙って落ちるため) も失敗に数える。サーバーの終了 (`child::stopping`) のときは覚えもしない。cache のパスに `,` があると rustdoc が `--emit` を区切ってしまうので、走らせずに印と warning (応答にも出る) を付ける
- ビルドの後に初めて見つかったファイルは、Unix では ctime も見る (`mv`・`cp -p`・`tar x` で古い mtime のまま置き換えたものを拾うため)。symlink 自体とその先の両方の ctime の新しいほう
- 書いている途中で、`Following::of` が条件にかかわらず `Following` を作り (`bool::then_some`)、走っている側があると、その `Drop` がロックを持ったまま同じ Mutex を取ってデッドロックした。`#[tokio::test]` は 1 スレッドなので、テストのプロセスごと止まり、cargo の子がゾンビで残った。走っている側の印も消していた。作るのを条件が真のときだけにし、単体テストを足した
- `build::source_dirs` は `source_packages` に置き換えた。package id は完全に同じものを先に探す (名前と版の同じ path パッケージが 2 つあるとき)。target dir の除外は実体のパスでも比べる (cache のパスに symlink があるとき)。`.d` の行で `#` と空白で始まるものだけをコメントとする
- 実測 (このリポジトリ): 作り直し 6.7〜7.1 秒 (dep-info は応答の後)。`.d` のファイル 85 件はどれもパッケージの中で、変数は残らない (`env!("CARGO_PKG_VERSION")` は cargo が設定する)。鮮度が合えば別のプロセスから 0.7 秒
- テスト: `.d` の読み方 (相対パス・`..`・空白・`env-dep` の値と無いもの)、symlink の後の `..`、build script の `output` の読み方、変数がサーバーの環境のものかの判定、ビルドの後に変わったファイルが digest を持たないこと。隔離した子で (結合テストの準備は `Members` にまとめた):
  - パッケージの外の `notes.md` を `include_str!` で読む member は、dep-info が記録された後も同じビルドのまま新しく、`notes.md` を編集すると作り直す。dep-info の前の記録に戻すと (サーバーが先に終わった場合)、次の呼び出しは新しいと判定し、dep-info を取り直して同じ鍵になる
  - build script がパッケージの外の `flags.txt` を `rerun-if-changed` で見張る member は、それが鍵に入り、編集すると作り直す。`rerun-if-env-changed` の変数と path 依存の build script のもの (`RUSTC_BOOTSTRAP`) は入り、`rustc-env` の変数と target dir の中のファイルは入らない
  - cache のパスに `,` があると、dep-info を取らずに warning を付ける
  - build script がパッケージの中のファイル (`src/gen.rs`) を毎回同じ内容で書き直しても、作り直さない
  - include を足して作り直した後と、include を外した後のどちらも、dep-info の後に同じ鍵に落ち着く。dep-info の間にソースを編集して戻すと (include を外す・壊す)、次の呼び出しで作り直し、外のファイルが鍵に入る
  - テストでは、外のファイルを書いた後に 3.5 秒待つ (`write_settled`)。ctime は戻せないので、ビルドの直前に書くと 1 回余計に作り直す
- 残したもの:
  - ビルドの間に外のファイルを書き換える build script (gitignore の下の `src/gen.rs` に毎回違う内容を書き、`include!` するもの) では、呼び出しのたびに作り直す
  - cargo が利用者の環境から作って渡す変数 (Windows の `PATH` など) は、サーバーの環境の値と違うので鍵に入らない。`option_env!` で UTF-8 でない値の変数を読むものも入らない
  - 裏の dep-info は枠と `doc:` のロックを数秒持つ (決定 3 のとおり)。その間、同じ target dir の作り直しは待つ
  - 依存 (registry・git) の build script の `rerun-if-env-changed` (`OPENSSL_DIR` など) は鍵に入らない (D4 は member と path 依存だけ)
  - Unix の外 (Windows) では ctime を見ないので、古い mtime のまま置き換えた外のファイルを拾わないことがある

**D4c・D4d の決定 (2026-10-21)**:

- 下調べ: 要件は、作り直しが `build.wait_secs` に間に合わなければ前回成功した JSON を返し、provenance に `stale: true`・`stale_reason: "rebuilding"`、最上位に `retry_after_secs` を付け、最上位の `status` は付けない (R-WS-6・R-OUT-9)。前回が無ければ `status: building`。作り直しに失敗したら前回があっても `build_failed` (stale は返さない)。`built_at` を付け、ソースがその後に変わっていれば LSP を hints で案内する (R-WS-9・§9.9)。stale の JSON の span でソースを読むときは行がずれうる旨を warnings に (R-OUT-9 の近く)
- 利用者が 1 件ずつ決めたこと (D4c):
  1. 前回のビルドは、flight の仕事が鮮度の判定で読んだものを共有の場所に置き、待ちをやめた呼び出しが使う (置き場の決め方を二重に持たない)。判定より前 (toolchain の確認や枠の待ち) で時間切れになったら `status: building`
  2. 計画 (features・toolchain・docsrs) が違っても、置き場の直近の成功を返す。provenance にはそのビルドの `features_requested`・`toolchain`・`built_at` をそのまま載せる
  3. 前回の JSON のアーカイブ (索引) が手元に無ければ `status: building` (索引を作る時間を待たせない)
  4. stale の応答すべてに、LSP を案内する hint と、span の行がずれうる warning を付ける
- 利用者が決めたこと (D4d): D4 の決定 6 を改め、`--cfg docsrs` や docs.rs の組で壊れた記録は新しく作らず、置き場の前回の成功の記録で判断する。前回が docsrs なし・通常の組に落ちていて、toolchain・`rustdoc-args` の cfg・docs.rs の組が同じなら、最初からそちらでビルドする。それらが変わるか `refresh` (D5) で試し直す。理由: 内容の鍵だと、編集のたびに docsrs を試し直して rustdoc 1 回分 (このリポジトリで約 5 秒) 余計にかかる

**D4c (作り直しの間の stale の応答。2026-10-21)**:

- `query::local`: flight の仕事が鮮度の判定で、置き場に前回のビルドを見つけて作り直すときは、それを flight の鍵ごとの表 (`REBUILDING`) に置き、仕事が終われば消す (`Rebuilding` の drop。成功でも失敗でも)。`rebuilding(cache, wanted)` は、その前回のビルドのアーカイブがあれば開いて返す (作らない。`load::archived`)。決定 1・2・3 のとおり、計画が違っても置き場の直近の成功を返し、判定より前やアーカイブが無ければ返さない
- `mcp::tool::local::built`: 待ちが期限を過ぎて `Joined::Going` なら、前回のビルドを返し、無ければ今までどおり `status: building`。作り直しに失敗すれば `build_failed` (stale は返さない)
- 応答 (`get_item` の member): provenance に `stale: true`・`stale_reason: "rebuilding"`、`provenance` の後に `retry_after_secs`、warnings に「前回のビルド (built_at) で、作り直しの間は `span` の行がずれうる」、hints に「built_at の後に編集したコードは LSP で、呼び直せば新しいビルド」(決定 4)。最上位の `status` は付けない。provenance の `features_requested`・`toolchain`・`built_at` は前回のビルドのもの。outputSchema の provenance に `stale`・`stale_reason` (今は `rebuilding` だけ) を足した
- テスト (stdio): build script が 6 秒眠る member を `build.wait_secs = 1` で作り、ソースを編集した後の呼び出しが、前回のビルドを stale の印・`retry_after_secs`・LSP の案内・span の warning 付きで返し、`status` を付けず、作り直しの後は新しいビルド (足した関数がある) を返すこと。3 回続けて通った (1 回 約 13 秒)
- レビューで直したこと:
  - 前回のビルドに無い item (編集で足した関数など) を引くと、stale と知らせずに `item_not_found` (`retryable: false`) を返していた: 前回のビルドでの item のエラー (re-export の行き先を除く) は `status: building` を返す (新しいビルドにあるかもしれない)
  - 作り直しが待ちの上限より長くかかって失敗すると、`build_failed` が届かず stale が返り続けていた (再現): 失敗した入力の鍵を (キャッシュ, flight の鍵) ごとに覚え、同じ鍵の間は前回のビルドを出さない (`REBUILD_FAILED`。D5 の失敗の記憶までのつなぎ)。失敗の結果も次の呼び出しに 1 度渡す (`hands_over` に失敗を足した)
  - stale の応答に、前回のビルドの記録にある warnings (docs.rs の組で作った旨など) も載せる
  - 前回のビルドの表は (キャッシュ, flight の鍵) で引き、項目は仕事の番号と組にして、後の仕事の項目を消さない
  - warning に crate 名を入れ、文面を「編集」前提から「作るもとが変わった」に改めた (toolchain や features が変わっただけの作り直しでも出るため)。warnings は `retry_after_secs` などの状態の項目の後に置く (`status: building` の応答と同じ順)
  - 決定 1 の「判定より前」は、実装では toolchain の確認まで: 枠や cargo のロックの待ちは判定の後なので、その間は前回のビルドを返す
  - テスト: 作り直しの間に足した item は `status: building`。作り直しが上限より長くかかって失敗すると `build_failed` が届く (2 回続けて通った)
- 残したもの: stale の member から re-export を追って別の crate に移った応答には、stale の印が付かない (warning に元の crate 名と built_at が出る)。stale の `LocalBuilt.json` は、作り直しで上書きされたファイルを指しうる (今はテストでしか読まない)

**D4d (壊れた記録を前回の記録で。2026-10-21)**:

- 決定のとおり、`--cfg docsrs` と docs.rs の組で壊れたことは、プロセスの中の記憶 (`DOCSRS_BROKE`・`DOCS_RS_FEATURES_BROKE`、package id と toolchain が鍵) をやめ、置き場の前回のビルドの記録で判断する
  - 記録に `planned` (再試行で変わる前の計画: `--cfg docsrs` の設定・`rustdoc-args` の cfg・docs.rs の組) を足した
  - 作り直すとき、前回のビルドの `planned` が今の計画と同じで、`rustc -vV` の toolchain とサーバーが選んだ toolchain も同じなら、前回が docsrs なしで終わっていれば最初から docsrs なしで、docs.rs の組で終わっていなければ (`features_requested` が違えば) 最初から通常の組でビルドする (`query::local::broke`)
  - 前回が無い、`planned` の無い前の記録、計画か toolchain が違うときは、試し直す。crate 自身のエラーで全部失敗したときは何も公開しないので、取り違えない
  - テスト: `broke` の単体テスト (同じ計画なら前回の終わり方から、toolchain か計画が違えば試し直す、記録や `planned` が無ければ試す)。docsrs で壊れる member の記録に `planned` が残ること
- レビューで直したこと:
  - 1 回目が途中まで書いたソースのエラーで落ち、2 回目までに直されると、docsrs (や docs.rs の組) のせいと取り違え、その終わり方を記録に残して以後ずっと試さなかった (前の記憶はプロセスの中だけだった): 再試行したビルドは、ビルドの前に見たソースが今も同じとき (`freshness::unchanged`) だけ `planned` を記録に残す。確かめられなければ残さず、次は試し直す
  - 鮮度の判定ができない道 (入力が読めない) でも、前回のビルドの記録で始め方を決める
  - `Planned` にサーバーの版を入れた (更新したら試し直す)
  - 結合テストは作り直しの道を通っていなかった (入力を渡していなかったので、編集しても新しいと判定されていた): パッケージを入力に渡し、作り直されたこと (JSON の SHA-256 が変わる) と docsrs なしで作られたことを確かめる。`broke` を一時的に無効にすると落ちることを確かめた。単体テストに、4 通りの終わり方、選んだ toolchain・cfg・版だけの違い、docs.rs の組が無いとき、前の形式の記録の読み込みを足した
  - テストの `Server::log`: 呼び出しの記録がそろい、中身が落ち着くまで待つ (WARN の行も同じスレッドが書くため)。改行で終わらない最後の行は読み飛ばす
- 残したもの: `build::build` が `planned: None` で公開してから `tries` が書き直すまでの間に、別のサーバーが記録を読んで dep-info を始めると、その書き直しで `planned` と warnings が消えうる (warnings は D4b からの同じ競合。次のビルドで 1 回試し直すだけ)
- 時間の測定 (このリポジトリの HEAD を写したもの、debug ビルドのサーバー、オフライン、`build.wait_secs` は既定の 10 秒): 初回 (target dir が空) 52.3 秒 (`building` を 4 回返し、5 回目に結果)、同じプロセスで新しいと判定 0.0 秒、別のプロセスで新しいと判定 0.8 秒、1 ファイルを編集した後の作り直し 4.9 秒 (上限の内なので、そのまま新しいビルド)、作り直した後 0.0 秒。D4a の前は、温まっても照会のたびに 4〜5 秒だった
- Claude Code での試し (利用者が別のセッションで、このリポジトリで `claude --mcp-config` に release ビルドを足して。2026-10-21):
  1. 自分のワークスペースの item (`chatora_rustdoc_mcp::data::freshness::key`・`query::load::archive_from`) をパスだけで頼むと、モデルはこのサーバーを使わず、grep でソースを読んで答えた。「chatora-rustdoc の MCP で」と明示すると使う。サーバーの instructions とツールの説明文、M2f の eval (F4) の課題
  2. ソースを編集した直後に引く: このセッションで member を初めてビルドしたので前回の成功が無く、決定 3 のとおり `status: building` を 2 回返し (モデルは `retry_after_secs` に従って呼び直した)、約 30 秒で編集の入った新しいビルドを返した
  3. 依存 (`tokio::sync::Mutex::lock`) は Cargo.lock の版 (1.53.3) を docs.rs から、`use` に使えるパスで返した。std (`std::vec::Vec::push`) は配布物の nightly から、const の stability 付きで返した。どちらも 1 回で返った
  4. private な item (`query::local::Rebuilding`): 引けたが、可視性が `pub(in ::query::local)` と表示された。Rust として書けない形で、元のコード (何も付かない private) とも見た目が違う。D6 で直す (含むモジュールと同じなら何も付けず、違えば `pub(in crate::…)`)
- テストの揺れの直し: ログを書き込み専用のスレッドに渡した後 (直し 2)、stdio のテストがログを読む時点で最後の呼び出しの記録がまだ書かれていないことがあった (全体を走らせたときに 1 度落ちた)。テストの `Server::log` は、送った `tools/call` の数だけ呼び出しの記録がそろうまで (最大 10 秒) 待つ

**D5 の下調べと決定 (2026-10-21)**:

- 下調べ: R-WS-7 は、`build_failed` に error の診断を先頭 N 件 (`build.max_diagnostics` の初期値 2、1 件 3,500 文字・合計 8,000 文字。JSON にエスケープした後の長さ。M2 で実際の診断を見て決める)、JSON の診断が無ければ stderr の末尾を付ける。失敗を一時的 (シグナル・サーバーが止めた・ロック待ちの打ち切り・`cargo fetch --locked` の失敗。`retryable: true`、約 30 秒だけ作り直しを抑える。取り消しで止めたビルドは記録しない) と決定的 (それ以外、`timeout` を含む。R-WS-4 の鍵 (toolchain を含む) で記録し、鍵が変わるか `refresh` まで作り直さずに同じ `build_failed` を `retryable: false`・失敗の時刻・`refresh` の hint 付きで返す) に分ける。依存の取得はビルドの前に `cargo fetch --locked` で分ける。E0658 と cargo の `requires rustc` は `toolchain_too_old`。判定の文言はテストに固定する。§6.4 の `refresh` は、cargo を起動しうる照会のツールの引数で、決定的な失敗の固定を解く (成功した結果は作り直さない)。ほかに解く記録は M2 で決める
- 今のコード: 診断は 5 件まで、stderr の末尾は 8,000 文字。失敗は記録せず呼ぶたびに作り直す (D4c のつなぎの `REBUILD_FAILED` だけ)。`retryable` は SIGKILL と I/O だけ。`refresh`・`toolchain_too_old`・`cargo fetch --locked` は無い
- `cargo fetch --locked` は、このリポジトリで温まった状態で 0.11 秒 (オフラインでもオンラインでも)
- 利用者が 1 件ずつ決めたこと:
  1. 失敗の記録 (決定的な失敗と、一時的な失敗の 30 秒の抑え) は、ディスクの member の置き場 (`local/v1/…`) に `failed.json` で置く (入力の鍵・toolchain・失敗の種類・時刻・返したエラーの本文)。再起動や別のセッションでも作り直さない。docs.rs の `.miss.json` と同じ流儀。D4c のつなぎ (プロセスの間の記憶) は置き換える
  2. `refresh` は、頼まれた crate について、失敗と後退を覚えているものをすべて解く: member なら失敗の記録・docsrs と docs.rs の組の後退 (D4d の `planned`)・dep-info の失敗の記憶、docs.rs の crate ならその版の否定キャッシュ (`.miss.json`) と index に名前が無かった記憶。成功した結果は作り直さない
- 聞かずに決めたこと: 診断の件数と文字数は要件の初期値で入れ、F5 の測定で決め直す。`cargo fetch --locked` は要件どおりビルドの前に (作り直すときだけ)
- D5 は 2 つに分ける: D5a (失敗の分類と記録、診断の上限、`cargo fetch --locked`、`toolchain_too_old`、文言のテスト)、D5b (引数 `refresh` と、解く記録)
- D5b の下調べ (2026-10-21): `get_item`・`list_module`・`crate_overview`・`list_methods`・`list_impls`・`list_implementors`・`search_items`・`resolve_version` は `get_item::parse_release` の `ReleaseArguments` を共有し、`list_crates` は別。ローカル機能の無い構成では宣言しない (どのツールも cargo を起動しないので。R-MCP-4・R-ARCH-3)。既定のプロジェクトの外では無視する。決定的な失敗の応答の hints に「原因を直したら `refresh`」を足す (R-WS-7)。決定 2 のほかに、短い失敗の記憶がある: `cargo metadata`・`cargo tree` の失敗 (`Resolver`、ワークスペースごとに 30 秒)、`rustc -vV`・rustup の出力 (`tool::output_kept`、「入っていない」を含めて 60 秒)、std の配布物の取得の失敗 (30 秒)
- D5b で利用者が 1 件ずつ決めたこと:
  1. member のビルドが新しい (鍵が合う) まま、前回 docsrs なし・通常の組に後退していたときは、記録の `planned` を消すだけで作り直さない (次に作り直すとき docsrs と docs.rs の組から試す。成功した結果は作り直さない)
  2. 短い失敗の記憶もすべて解く (`cargo metadata`・`cargo tree`、`rustc -vV`・rustup、std の配布物)。そのワークスペース (std ならその配布物) の分だけ
  3. 何を解いたかは応答で伝えない (ログに書く)
  4. 1 つの crate を指さない呼び出し (`crate` なしの `search_items`、`list_crates`) では、ワークスペースの分 (`cargo metadata`・`cargo tree`、`rustc -vV`・rustup) だけを解く。横断する crate の否定キャッシュや失敗の記録は解かない
  5. 同じ member のビルドが進行中なら合流する。`refresh` が効くのは仕事を始めた呼び出しの分だけで、合流した呼び出しは進行中の仕事を変えない (進行中の仕事が記録を読んだ後に合流したときは、古い失敗や後退が残りうるが、もう一度 `refresh` すれば解ける)。最初は「記録を解いてから合流する」と決めたが、実装で印が後の仕事に残る問題が出て、利用者がこの形に改めた (2026-10-21)

**D5a (失敗の分類と記録・診断の上限・`cargo fetch`・`toolchain_too_old`。2026-10-21)**:

- 分類 (`data::cargo`・`data::build`): `Failure::TooOld` (`toolchain_too_old`) を足した。cargo の stderr の `requires rustc` (cargo 1.99.1 の文言 `a@0.1.0 requires rustc 1.999` と、古い cargo の `cannot be built because it requires rustc …` をテストに固定) と、`--message-format=json` の error の診断のコード `E0658` で判定する。error の code は `Failure::code` に移した (`mcp::tool::project::failure_name` をやめた)
- `BuildError::is_transient`: 一時的なのは、cargo の子が殺された (`Killed`)、cargo を起動できなかった、JSON を公開できなかった (I/O)、cargo が残したものがビルドのものでなかった (別のビルドが書きうる: `NotTheCrate`・`Fresh`・`Unreadable`)、`cargo fetch` の失敗 (`fetch_failed`・`lockfile_outdated`・`offline_miss` を除く。§8 の `retryable` に合わせる。fetch の時間切れと起動の失敗も一時的)。ほかはすべて決定的 (ビルドの `timeout` を含む)
- `cargo fetch --locked` (`build::fetch`): 作り直すとき、ビルドの前に 1 回走らせ、失敗は `BuildError::Fetch`。ビルドが `--target` を渡すときは fetch にも渡す (プロジェクトの `build.target` が target JSON だと、stable の cargo の fetch は `-Zjson-target-spec` を求めて落ちる。stdio のテストで見つかった)
- 診断の上限 (`query::local::failure_body`): `build.max_diagnostics` (既定 2) 件、1 件 3,500 文字、合計 8,000 文字 (どれも JSON にエスケープした後の長さ)。切ったら `…` を付け、見せなかった件数を `diagnostics_omitted` に書く。JSON の診断が無ければ stderr の末尾を 8,000 文字で付ける。ビルドが覚える診断は 20 件まで (前は 5 件)。`toolchain_too_old` の suggestions は、サーバーが選んだ nightly (`nightly-<日付>` なら `build.nightly_date` を後の日に、`nightly` なら `rustup update nightly`)、`rust-toolchain.toml` (か `rust-toolchain`) がプロジェクトの根か上にあればその更新、無ければ `rustup update`。rustup の有無は見ていない
- 失敗の記録 (`build::Failed`、member の置き場の `failed.json`): 入力の鍵・`rustc -vV` の toolchain・一時的か・時刻 (RFC 3339 と Unix 秒)・返したエラーの本文。鮮度の判定で、前回の成功と鍵が合わず、記録が今の鍵で、決定的か 30 秒以内の一時的な失敗なら、ビルドも stale の応答もせずに記録の本文を返す (`LocalError::Failed { kept: true }`)。応答には `failed_at` と、一時的なら `retry_after_secs` を足す。書くのは鍵が取れたときだけで、サーバーの終了で止めたビルド (`child::stopping`) は書かない。公開と同じロック (`Cache::lock`) の下で書き、その間に同じ鍵のビルドが公開されていれば書かない。公開に成功したら消す
- D4c のつなぎ (`REBUILD_FAILED`・`Rebuilding::ended`) は消した。`hands_over` は、鍵の無い成功と、記録できなかった失敗 (鍵が取れない、書けない) だけを次の呼び出しに渡す。記録した失敗は次の呼び出しが記録から見つける (渡すと、直した後に 1 度古い失敗が返るため)
- docs.rs の組での再試行は、`E0658` の失敗 (`toolchain_too_old`) でも行う (docs.rs の組が nightly の機能を有効にしうる)
- テスト: 分類の文言、`E0658` の読み取り、記録の有効期限 (別の鍵・30 秒過ぎ・時計が戻ったとき・鍵が無いとき)、記録の読み書きとキャッシュの外に書かないこと、エスケープ後の長さでの切り方、診断の件数と合計の上限、本文の code・`retryable`・`reason`・`toolchain_too_old` の suggestions。結合テスト (local): member のソースを壊すと、前回の成功があっても `build_failed` (`retryable: false`、診断付き) を返して `failed.json` を書き、もう一度呼んでもビルドせず (記録は書き直されない) 同じ本文を返し、直すと作り直して記録を消す
- レビューで直したこと:
  - 鍵を取った後、rustdoc が読む前にソースが編集されて失敗すると、編集前の鍵で失敗を固定していた (編集を元に戻すと、もう無いコードの `build_failed` を返し続ける。レビューで再現): 記録する前に、鍵を取ったときのソースが今も同じか (`settled`) を確かめ、変わっていれば記録せず (`kept: false`)、次の呼び出しに 1 度渡す。build script が 3 秒眠る間に壊す編集を入れる結合テストを足し、直しを外すと落ちることを確かめた
  - `cargo fetch` の時間切れが「ビルドの時間切れ」(決定的) になっていた: fetch の実行の失敗は `BuildError::Fetch` (一時的) に包む
  - rustc の `aborting due to N previous errors` (code も span も無い error) が診断の 2 枠の 1 つを使っていた: 数えない。覚える 20 件を超えた error も数え、`diagnostics_omitted` に足す
  - `Killed` の本文に、殺されたことを書く (診断があると stderr の SIGKILL の行が出ないため)
  - suggestions を失敗の種類で分けた: `fetch_failed` の `auth` は §8 の案内 (URL を疑う、端末で `cargo fetch`、`gh auth setup-git`)、`no_repository`・`no_rev` は URL と rev の確認、`lockfile_outdated`・`offline_miss` にも案内。`refresh` の hint は D5b
  - テストを足した: 本物の cargo で `rust-version = "1.999"` の member が `toolchain_too_old`、`hands_over` が記録した失敗だけを渡さないこと、同じ鍵の公開があれば記録を書かないこと、一時的かどうかの表、MCP の応答の `failed_at` と `retry_after_secs` (切り上げ)
- 残したもの (レビューの指摘のうち): ビルドの時間切れが、ほとんど cargo のロック待ちだった場合も決定的になる (要件の「ロック待ちの打ち切りは一時的」とずれる)。`toolchain_too_old` は rustup の無い環境でも `rustup update` を案内する。記録は置き場に 1 つなので、計画やサーバーの版の違うビルドが交互に動くと公開のたびに消える。一時的な失敗が 30 秒で切れて作り直す結合テスト、サーバーの終了で止めたビルドを記録しないことのテストは無い。D5a だけでは鍵が変わらないまま原因を直したとき (システムライブラリ・git の認証・環境変数) に抜ける手段が無いので、D5b と一緒に push する
- 残したもの: 出力のサニタイズ (R-SEC-7) はまだどこにも無いので、診断は切ってからサニタイズすることになる (除く文字は長さを減らすだけなので、上限は超えない)。`refresh` は D5b。README には失敗を覚えることを D5b で書く
- README: `build.max_diagnostics` を設定の表に足した

**D5b (引数 `refresh`。2026-10-21)**:

- 引数: ローカル機能のある構成で、すべての照会のツール (9 本) の入力スキーマに `refresh` (真偽値) を足す (`mcp::tool::with_refresh`。`sleep` を除く)。ローカル機能の無い構成では宣言しない。真偽値でなければ `invalid_argument`。カーソルの照合 (`request_digest`) には入れない (`refresh` 付きの呼び出しの次のページを、付けずに頼める)
- 解くもの (決定 2 と D5b の決定 2・4):
  - ワークスペース (`mcp::tool::local::refresh_project`): `Resolver::forget_failures` (`cargo metadata` の失敗と、target ごとの `cargo tree` の features・範囲の失敗。成功は残す)、`toolchain::forget_failures` (`rustc -vV`・rustup の出力のうち失敗だけ)。既定のプロジェクトの外を指したら何もしない。`list_crates` と `crate` なしの `search_items` はこれだけ
  - crate を指す照会 (`get_item::execute`): ワークスペースの分、index に名前が無かった記憶と index の取得の失敗 (`SparseIndex::forget`)。版を決めた後、docs.rs の crate なら、その版の置き場 (頼んだ target と既定の target) の `miss.json`・`*.miss.json` と `.crate` の取得の失敗 (`data::forget_failures`・`forget_failure_of`)。std なら配布物の取得の失敗 (キャッシュの `std/` の下)。`resolve_version` はワークスペースの分と index の記憶
  - member: `LocalWanted.refresh` を立て、ビルドの仕事が、`failed.json` を読まない、前回の記録の終わり方 (`planned`) で始め方を決めない、新しいビルドなら `planned` を消して記録を書き直し dep-info の失敗の記憶 (`DEP_INFO_FAILED`) を消す (作り直さない。決定 1)
  - 何を解いたかは応答に書かず、info のログに書く (決定 3)
- 決定 5 の実装と、試して見つけて変えたこと:
  - member の `refresh` は、仕事を始めた呼び出しの分だけ効く (`Flights::join` の `start` が受け取る `wanted` の印)。進行中の仕事に合流した呼び出しは、その仕事を変えない (決定 5 を改めた。利用者の決定)。最初はプロセスの中の集合に鍵を入れて仕事が読む形にしたが、合流した呼び出しの印が残り、後の仕事が忘れてしまうので、やめた
  - stdio のテストで、待ちの上限 (`build.wait_secs`) より長くかかって失敗する member を、`refresh: true` を付けたまま呼び直すと、呼ぶたびに記録した失敗を忘れて作り直し、`status: building` を返し続けた (モデルは `retry_after_secs` に従って同じ引数で呼び直すので、終わらない)。最初は `refresh` で作った失敗を次の呼び出しに渡して直したが、レビューで、(1) 同じことが `cargo metadata`・`cargo tree`・docs.rs・std・`.crate` の遅い失敗でも起きる、(2) 渡した失敗が、ソースを直した後の `refresh` なしの呼び出しにも 1 度返る (stale の応答で呼び直しをやめた後など) と指摘された
  - そこで、`refresh` は同じもの (プロジェクトと crate、プロジェクトだけ、std) について 1 度だけ効かせ、そのものの呼び出しが「呼び直せ」でない応答 (エラーか、`retry_after_secs` の無い結果。`status: building` と作り直しの間の stale は呼び直せ) で終わるまで、続く `refresh` は効かせない (`mcp::tool::local::refresh_acts`・`refresh_told`。印は最長 10 分で外す)。待つ間に付け続けても、作り直した結果か失敗が返る。受け渡しの変更はやめた
- 決定的な失敗の応答の suggestions に、原因を直したら `"refresh": true` で呼び直すことを足した (R-WS-7。一時的な失敗には付けない)
- R-QA-5 の「引数ごとに結果が変わる」検査: `refresh` は成功した結果を変えないので、ほかのテストで確かめる印 (`Change::CheckedElsewhere`) を足した (宣言された引数に場合があることの検査は通る)
- テスト: 引数の読み取りとカーソル、ツールの宣言、`miss.json` を忘れること、index の記憶を忘れること、`hands_over`、suggestions。結合テスト (local): 記録した失敗が `refresh` で作り直されて記録が書き直されること、docsrs なしに後退した新しいビルドが `refresh` で作り直されずに `planned` を消し、次の編集で docsrs ありで作られること (どちらも `refresh` を無効にすると落ちることを確かめた)。stdio: 記録した失敗をすぐ返し suggestions に `refresh` があること、`refresh: true` で呼び直し続けると作り直した失敗 (時刻が新しい) が返ること (3 回続けて通った、約 10 秒)
- レビューで直したこと (ほかに):
  - 既定のディレクトリの直下に workspace が複数あると (`project` なし)、`refresh` をまるごと無視していた: プロジェクトの外を `project` で指したときだけ無視し、ほかは workspace に関係しない分 (toolchain・index・docs.rs・std) を解く
  - 後退を忘れること (`planned` を消す) が、裏の dep-info の書き込みと競合していた: `Built::forget_planned` が公開のロックの下で記録を読み直して `planned` だけを消し、dep-info の `follow` は `planned` が変わっていれば書かない (次の呼び出しが dep-info を取り直す)
  - 記録しなかった失敗 (`kept: false`) にも `refresh` の案内を付けていた: 記録したものだけに付ける
  - 解いたもののログ: member の失敗の記録を読まなかったこと、`cargo tree` の失敗、`rustc -vV`・rustup の失敗、index の名前の記憶、dep-info の失敗の記憶も info に書く
  - テストを足した: `refresh` を 1 回だけ効かせること、`Resolver::forget_failures` (失敗は `cargo metadata` を走らせ直し、成功は残す)、toolchain の失敗だけを忘れること
- 残したもの: README は F6 でまとめて書く (照会のツールのビルドとあわせて)。`toolchain::forget_failures` はすべてのディレクトリの `rustc -vV`・rustup の失敗を、std は全日付の配布物の失敗を解く (決定 2 の「その分だけ」より広いが、失敗だけで害は小さい)。起動時の host の `rustc -vV` の失敗 (`state.host`) は解けない。`cargo metadata` の git の決定的な失敗 (`fetch_failed`) は `Resolver` で 30 秒しか覚えず、`metadata_failed` に `refresh` の案内も無い (前からある。R-WS-7 の「`refresh` までキャッシュ」とずれる)。docs.rs・std・`list_crates`・`search_items`・`resolve_version` の `refresh` を通しで確かめる結合テストは無い (`CheckedElsewhere` が指すのは member のテスト)

**D6 の下調べと決定 (2026-10-21)**:

- 下調べ (サブエージェント、読むだけ): 可視性は `render::push_visibility` が描き、rustdoc の `restricted` のパスをそのまま `pub(in ::query::local)` と出す (rustdoc は何も付けない private を `restricted { parent: 含むモジュール }` で書き、`pub(super)` も同じ形)。trait の item・enum の variant・impl の item は `default` で、親の可視性を継がない。可視性は応答の項目に無い。公開パス索引は `Public` だけをたどり、private な item は定義パスと「公開パスが無い」の warning でなら引ける (`crate::` は受けない)。検索と一覧は公開索引だけを使うが、公開の型の private な固有メソッドは検索と `list_methods` に出る。docs.rs は `rustdoc-args` を尊重するので、`--document-private-items`・`--document-hidden-items` でビルドされた依存の JSON がありうるが、除くのは公開索引だけ (定義パスの引き当て・固有メソッド・フィールド・hidden が漏れる)。member の JSON では private なフィールドが消されないので `has_private_fields` と「`/* private fields */`」が誤る。member どうしの re-export は追わない (D2c-2)
- 利用者が決めたこと:
  1. D6 を 3 つに分ける: D6a (可視性の表示・応答の項目・親から継ぐ、private な item の crate 内のパスと `crate::` の入力、private なフィールドの誤り)、D6b (検索と一覧で member は private を含め、依存と std は非公開と `doc(hidden)` をどの経路でも除く)、D6c (member と path 依存への re-export を追う。行き先をビルドし、版は依存グラフ)
  2. 応答の可視性は Rust の書き方の文字列 (`pub`・`pub(crate)`・`pub(in crate::query)`・`private`)
  3. (D6b) member の `list_module`・`crate_overview` は private な item も並べ、private なモジュールも開ける (検索とそろえる。長くなる一覧はページングで受ける)
  4. (D6b) member の `#[doc(hidden)]` な item も検索と一覧に出し、hidden の印を付ける。依存と std の hidden は除く
  5. (D6b) 検索と一覧の項目には、`pub` でないものにだけ `visibility` を付ける (`get_item` は member で常に付ける)
  6. (D6c) member の re-export をたどって別の member に入るとき、行き先がまだビルドされていなければ、member の照会と同じ共有の仕事でビルドして `build.wait_secs` まで待つ。間に合わなければ、行き先を直接引いたときと同じく前回のビルド (stale) か `status: building`
  7. (D6c) crate をまたぐ glob (`pub use other_member::*;`) は、member の間だけたどる。docs.rs の crate と std は今までどおりたどらない (R-PR-3)
- 聞かずに決めたこと (D6c): member でない path 依存・git 依存への re-export は D7 まで今までどおりたどらない (m2-plan の D7 の範囲)。行き先の member の版は、起点の member の依存 (改名を含む) から決め、`html_root_url` は使わない (R-PR-4)
- 聞かずに決めたこと: 含むモジュールと同じ範囲は `private` (シグネチャには何も付けない)、違えば `pub(in crate::…)` (`pub(super)` は JSON から分からないので書かない)。crate のルートの private は rustdoc が `crate` で出すので `pub(crate)` (同じ意味)。trait の item と trait impl の item は trait の、enum の variant は enum の可視性を継ぐ。公開パスの無い member の item は `crate::a::b::Item` で示し、`crate::` で始まるパスを受ける (R-PR-2)

**D6a (可視性・crate 内のパス・private なフィールド。2026-10-21)**:

- 公開パス索引に、含むモジュールに限られた item の集合 (`PathIndex::module_private`) を足した: モジュールの item と型 (struct・union) のフィールドのうち `Restricted { parent }` の `parent` が含むモジュールのもの (Rust で `pub` を付けずに書いたもの)、固有の `impl` の item のうち型のモジュールに限られたもの (rustdoc の JSON はモジュールの item に `impl` を入れないので、`impl` は型と同じモジュールにあるとみなす。別のモジュールや関数の本体にある `impl` の private は `pub(in crate::…)` と出る)。private な item を含むビルドでだけ中身がある。アーカイブの形式 (`ARCHIVE_SCHEMA`) を 6 に上げた
- シグネチャ (`render::push_visibility`): 含むモジュールに限られたものは何も付けず、ほかの `Restricted` は `pub(in crate::…)` (rustdoc の `::a::b` を `crate::a::b` に。`render::in_crate`)
- 応答の `data.visibility` (member の JSON のときだけ。docs.rs と std は公開の item しか出さないので付けない): `pub`・`pub(crate)`・`pub(in crate::…)`・`private`。`Default` (trait の item・variant・trait の impl の item) は親のもの: trait の item は trait の、variant は enum の、trait の impl の item は、この crate の trait なら trait の、外の trait なら型の。親が分からなければ付けない。出力スキーマに足した
- パス: member の公開パスの無い item は `crate::a::b::Item` で示し、「公開パスが無い」の warning を付けない (`Query::path_of`)。引数の `path` は `crate::` で始まってよい (`with_crate_name` が crate 名に置き換える)
- private なフィールド: `list_impls` の `has_private_fields` を、rustdoc が除いたものに加えて `pub` でないフィールドでも真にする (member の JSON では private なフィールドが除かれないので、偽になっていた)。`get_item` の `fields` は member では可視性付きで並ぶ (`count: u8`、`pub(crate) shared: u8`)
- `get_item` の説明文: member をビルドすること、`visibility`、`crate::a::Item` を書いた (上限 2,048 文字に収めるため、`std::vec::Vec` の例と「Cargo's」を削った)
- テスト: stdio で、private なモジュール・`pub(super)`・`pub(crate)`・private なフィールド・`pub(crate)` の trait と enum を持つ member を作り、可視性 (`private`・`pub(in crate::open)`・`pub(crate)`・親から継ぐもの)、シグネチャ、`crate::` のパスの入力と表示、フィールド、`has_private_fields` を確かめた (集合を無効にすると落ちることを確かめた)。`in_crate` の単体テスト
- 確かめたこと (rustdoc 1.99.1。レビューの実測を含む): 何も付けない private・`pub(in crate::a)`・`pub(self)`・`pub(super)` は `Restricted { parent }`。crate のルートの private と、ルートのすぐ下のモジュールの `pub(super)` は `Crate` (`pub(crate)` と出る。ルートのフィールドも)。trait の item・variant とそのフィールド・trait の impl の item・impl そのものは `Default`。`--document-hidden-items` を付けないので、member の `#[doc(hidden)]` は JSON に入らない
- レビューで直したこと:
  - 固有の `impl` の private なメソッドが `pub(in crate::…)` と出ていた (いちばん多い private なのに、`impl` がモジュールの item に入らないことを見落とし、テストも無かった): 型のモジュールで判定する
  - trait の impl の item が、決定と違い型の可視性を継いでいた: この crate の trait なら trait の可視性
  - 親が分からない `Default` (曖昧なパスで variant を定義パスで引いたときなど) を `private` としていた: 付けない
  - private なモジュールの `list_module` が、理由の warning なしに空を返していた (`path_of` が warning を出さなくなったため): member で公開パスの無いモジュールは、公開パスのある item だけを並べる旨を warning に書く (D6b で中身を並べるまで)
  - テストに、固有の `impl` の private なメソッド・自分の crate の trait の impl の item・ルートの private・private なモジュールの一覧を足した (型のモジュールでの判定を外すと落ちることを確かめた)
- 残したもの: 型と別のモジュールや関数の本体にある `impl` の private (`pub(in crate::…)` と出る)。`list_implementors` の sealed の文言は、member でも定義パス (`crate::` でない)。`get_item` の説明文の書き出しは docs.rs と std のまま (F4 で見直す)。アーカイブの形式を上げた後の最初の呼び出しは、作り直しの間の前回のビルド (D4c) を開けず `status: building` になる

**D6b-1 (hidden・検索・list_module・crate_overview。2026-10-21)**:

- D6b を 2 つに分けた: D6b-1 (この段) と D6b-2 (`list_methods`・`list_impls`・`list_implementors`、依存の非公開を `get_item` の定義パスとフィールドから除く)
- member のビルドに `--document-hidden-items` を足した (決定 4。member の `#[doc(hidden)]` は、これが無いと JSON に入らない。R-WS-1 のコマンドに前から書いてある)。鍵の facts に `"documents": "private hidden"` を入れ、前のビルドを作り直す
- 実測 (rustdoc 1.99.1): `#[doc(hidden)]` は `Attribute::Other("#[doc(hidden)]")`。hidden なモジュールの中の item には付かないので、パスで判断する
- 公開パス索引に hidden の集合 (`PathIndex::hidden`) を足した: `#[doc(hidden)]` の item、公開パスのすべてがそれを通る item (正準パスは hidden を通らないものを先に選ぶ)、`#[doc(hidden)]` の `impl` の item、公開パスの無い item は定義パスの上のモジュールがそれのもの。ほかの crate の item の再公開は、通った `use` とモジュールで決める (`hidden_reexports`)。`path_index::is_doc_hidden`。アーカイブの形式を 7 に上げた
- 検索 (`SearchIndex`): 項目に `restricted` (公開でないか hidden) を足した。公開パスのある item のうち hidden なもの、メンバーのうち可視性が `Public`・`Default` でないものか hidden なもの、private を含むビルドでは公開パスの無い item (crate 内のパス `crate::…` で、パスの順に) とそのメンバーを `restricted` にする。検索 (`hits`) と綴りの候補 (`did_you_mean`) は、member (`Origin::Local`) でなければ `restricted` を除く (決定 4、R-SH-5)
- `list_module`: member では、各モジュールの公開パスの無い子 (`impl`・`use` を除く) を、公開の子の後に crate 内のパスで並べる。公開パスの無いモジュールでは、子をすべて並べる (公開パスのある子は正準パスで)。`recursive` ではそれらのモジュールの中もたどる。member でなければ hidden な item を除く。D6a のつなぎの warning は消した
- `crate_overview`: member の数は定義パスのある item すべて (variant・フィールド・`impl` を除く)、ルートの公開パスの無い子も並べる。member でなければ hidden な item を数えず、並べない
- 応答: `data.hidden` (member の hidden な item)。検索と一覧の項目は、`visibility` が `pub` なら付けない (`ItemData::listed`。決定 5)。出力スキーマに `hidden` を足した
- テスト: 検索の単体テスト (private を含むビルドで、crate の `lock` と hidden な `Mutex` が docs.rs の crate では見つからず、member では見つかる)。stdio で、member の private な固有メソッドと hidden な関数が検索で見つかること (`visibility`・`hidden`、`pub` に `visibility` が付かないこと)、公開パスの無いモジュールとルートの一覧、`crate_overview` のモジュールと数
- レビューで直したこと (レビューは std・core・alloc の JSON も `includes_private: true` で hidden を含むこと (core 902・std 120・alloc 69 件) を実測した。除くのは docs.rs の珍しいビルドだけでなく、std で常に効く):
  - ほかの crate の item の hidden な再公開が漏れていた (`list_module std::panic` の `panic_2021`、hidden なモジュール `std::rt` の中身、alloc の `io::IoHandle`): 通った `use` とモジュールで判定し、検索・一覧・`crate_overview` で除き、member では `hidden` を付ける
  - `#[doc(hidden)]` の `impl` のメソッドが hidden にならなかった: その `impl` の item を hidden に
  - 見えるパスでも使える item が、正準パスが hidden を通るだけで隠れていた: 正準パスは hidden を通らないパスを先に選び、すべてのパスが通るときだけ hidden に
  - 公開パスの無いモジュールの子を並べると、glob で公開モジュールに並んだ item が `recursive` の一覧で 2 度出た: 並べたものは足さない (外すと落ちることを確かめた)
  - ほかの crate の hidden なモジュールが空の一覧として開けた: `NotFound`
  - `crate_overview` の数で variant を除いたのが docs.rs と std にも効いていた: member でだけ除く
  - 公開パスの無い item の並びを、同じパスなら id でも決める。`is_doc_hidden` は `#[doc` で始まる属性だけを調べる (レビューの測定で、core のアーカイブを作る時間が 338 ms から 603 ms に増えていた)
  - テストを足した: `list_module` の単体テスト (hidden なモジュールと再公開を除き、hidden でない根の再公開の `Mutex` は残り、hidden なモジュールは `NotFound`)。stdio で、hidden な `impl` のメソッドと hidden な再公開に `hidden`、見えるパスのある item は hidden でなく正準パスがそのパス、glob の item が `recursive` の一覧に 1 度だけ
- 残したもの: BM25 の統計は `restricted` の項目も数える (std・core・alloc では private と hidden が文書の数に入る。上位の順位はほぼ同じとレビューが確かめた)。std の公開パスの無い item も検索の索引に入る (member か分かるのは照会のときなので、アーカイブを作る時間と大きさが増える)。member の名前の `internal`・`_private` も点が半分になる (`penalized`)。crate 名を付けて引いた `crate::…` の item は Path の段に乗らない。形式 56 の `cfg_attr` の hidden は読めない。依存の hidden なフィールドと、依存の hidden な item を `get_item` で引く経路は D6b-2。説明文 (`search_items`・`list_module`・`crate_overview`) に private と hidden を書いていない (F4)

**D6b-2 (メソッド・impl・実装者、依存の非公開を引かない。2026-10-21)**:

- 解決 (`Query::resolve`): member でなければ `ArchivedPathIndex::resolve_public` で公開の item だけに解決する: 公開パスのある item、公開パスのある親の、`pub` のメンバー、または `Default` のメンバー (trait の item・variant・trait の impl の item) で、その trait がこの crate のものなら公開パスがあって hidden でないもの。メンバーは `impl` の組ごとに公開のものから探す (`find_member_where`)。定義パスで引けるのも公開パスのある item だけ (std の `alloc::vec::Vec` など)。docs.rs の crate と std の private な item、公開でないモジュールの中の `pub` な item を引くと `NotFound` (R-SH-5)。`get_item`・`list_module`・`list_impls`・`list_implementors` の型や trait の解決が通る
- `trait_origin`: member では、公開パスの無い自分の trait も `crate::…` のパスで返す (`list_impls` と `list_methods` に private な trait の impl が出る)。member でなければ、hidden な自分の trait は返さない
- `list_methods`: member でなければ、固有メソッドのうち `pub` でないものと hidden なもの、hidden な `impl` を除く。項目は `ItemData::listed` (`pub` でなければ `visibility`)
- `list_impls`: member でなければ hidden な `impl` を除く
- `list_implementors`: member では、公開パスの無い型の実装も出す。member でなければ hidden な型の実装も除く
- フィールドと variant: member でなければ、`pub` でないか hidden なフィールドを `fields` とシグネチャ (tuple struct の `_`) から除いて「`/* private fields */`」とし、hidden な variant を除いて「`/* some variants omitted */`」とする (`Renderer::public_only`)。variant のフィールドは `Default` (enum と同じだけ公開) なので見せる
- テスト: `get_item` の単体テスト (private を含むビルドで、crate の `lock` を定義パスで引くと `NotFound`、crate のフィールドは「`/* private fields */`」)。stdio で、member の `list_methods` に private なメソッド (`visibility: private`)、`list_impls` に `pub(crate)` の trait の impl、`list_implementors` に公開パスの無い型の実装
- 依存の hidden な item は、`get_item` で正しいパスを指せば引ける (D6b の決定 4 の問いで示したとおり)
- レビューで直したこと (レビューは be2aa1c と並べて std の JSON と試しの crate で比べた。std の `list_methods`・`list_impls`・`list_implementors` で減ったものは、下の 1 を除いてすべて private か hidden だった):
  1. variant のフィールドがすべて隠れていた (`Option` の variants が `Some(_)`、`Cow` が `Borrowed(_)`。variant のフィールドは rustdoc では `Default` なのに `Public` だけを通していた): `Default` も通す。テストを足し、外すと落ちることを確かめた
  2. 公開でないモジュールの中の `pub` な item が公開パスなしで引けた (`std::sys::cmath::erff`、`std::backtrace_rs::print::PrintFmt`、sealed の文言が「not public」と言う trait): 公開パスを求める
  3. 公開パスの無い trait の impl の item が引けた (`std::fs::File::copy` が `SpecCopy::copy`、`Vec::spec_extend` が `ambiguous_path`): `Default` のメンバーは、その trait がこの crate のもので公開パスが無いか hidden なら除く
  4. private な固有メソッドが同じ名前の公開の trait のメソッドを隠して `NotFound` になった: `impl` の組ごとに公開のものから探す
  5. `list_implementors` が hidden な `impl` を除かなかった: 除く
  6. hidden な trait の item (`PartialOrd::__chaining_*`、`Eq::assert_receiver_is_total_eq` など) が `list_methods` と `list_implementors` の `items` に出ていた (前からある): この crate の trait の hidden な item は、`impl` に書かれたものも提供されたものも除く
  - テストを足した: 公開でないモジュールは引けず、そこから公開されたものは引けること、variant のフィールド
- 残したもの: ほかの crate の hidden か非公開の trait の blanket impl (`core::mem::SizedTypeProperties` などを、alloc や std の型の `list_impls` が出す。ほかの crate の trait が hidden かは分からない。通常の docs.rs の JSON には無い)。core の中の型で `needs_use` が誤る (前からある)。`list_impls` の `has_private_fields` は hidden な `pub` のフィールドを数えない (`get_item` は「`/* private fields */`」にする)。member の `list_implementors` の型は `crate::` でなくソースの書き方。名前で書けない戻り値の型は、依存では引けなくなった (R-PR-2 の `unnameable` は未実装)。private な固有メソッドと同名の trait のメソッドのテストは無い

**D6c (member の間の re-export をたどる。2026-10-22)**:

- 依存グラフ (`PackageGraph`): member の依存に、どの member のものかを足し、`member_named` (名前と版の member) と `dependency_of` (member がコードで使う名前の依存) を足した
- 行き先 (`Query::destination`): 起点が member なら、JSON の `html_root_url` (crate が自分で書いたもので古いことがある) から版を取らない。ほかの crate の glob (`pub use other::*;`) を、member でだけ行き先にする (`glob_destination`。いちばん長い接頭辞のもの 1 つ。決定 7)
- MCP (`in_project_build_of`): 起点が member なら、行き先の package を起点の依存 (`dependency_of`) から、無ければ名前で引く。行き先が member なら `InProjectBuild::Member` で、member の照会と同じ共有の仕事でビルドして引く (`member_destination`。決定 6。間に合わなければ前回のビルドか `status: building`、cargo が解決・計画中ならその応答)。行き先のパスの先頭はパッケージの名前に替える。member でない path 依存・git 依存は今までどおりたどらない (D7)
- 一覧の再公開 (`reexport_target`): 起点が member なら、版を付けず (分からない旨の warning も付けない)、crate の名前で示す
- 実測: rustdoc は re-export の連鎖を最後の定義に解決するので、`front` が `middle` 経由で再公開した `back` の item は、`back` へ 1 度で入る (`front` が直接依存していない crate。名前で引く)。JSON の外の crate の名前は lib の名前で、依存の改名ではない
- テスト: stdio の `a_reexport_of_another_member_is_not_followed` を `a_reexport_of_another_member_is_followed_into_it` に置き換えた (`front`→`middle`→`back`、`middle` は `back` を改名して依存): `front::Thing` は `back` をビルドして `data.path: front::Thing`・`defined_at: back::Thing`、glob の `front::in_middle` は `middle` で引ける (glob を外すと落ちることを確かめた)。一覧の再公開は `back` で、版が分からない旨の warning が無い
- レビューで直したこと (試しの workspace で stdio を叩いて実測):
  1. glob の行き先の crate を `use` の `source` の文字列から取っていた (`::middle`・`super::aliased`・別名で壊れ、`super` を crates.io に引きに行った。R-PR-3 は source を最後の手段とする): 公開パス索引の `ExternalGlob` に、`use` の行き先の id から取った定義パス (`target`) を持たせ、それで行き先を決める。アーカイブの形式を 8 に上げた
  2. glob の行き先が std や crates.io の crate にも向き、glob を持つ member では typo のパスでも行き先を引きに行った (`pub use std::collections::*;` で何もかも `crate_not_found`): glob は行き先が member のときだけたどり、そうでなければその候補を捨てる。どの glob も持たなければ、起点の `item_not_found`
  3. 同じ長さの接頭辞の glob が複数あると最後の 1 つしか試さなかった: 候補をすべて持ち (`Destination::others`。長い接頭辞の順、同じなら書かれた順)、`item_not_found` なら次へ。ビルドに失敗したら、その失敗に行き先を直接引く案内を付けて返す
  4. lib 名とパッケージ名が違い bin も持つ member に入ると、bin を文書化していた: 行き先の crate 名 (lib 名) で計画する
  5. docs.rs の crate から、同じ名前の member に入りえた: 起点が member のときだけ member に入る
  6. 改名した依存の照合が効いていなかった (JSON の外の crate の名前は lib 名): `dependency_of` は名前 (改名) の次に lib 名で照合する。起点が member で、プロジェクトが版を教えなければ、crates.io の latest にせず re-export を返す
  7. glob でたどったときの warning の文言 (「front is a re-export of b」と誤っていた)
  - テストを広げた: 複数の glob を順に、std の glob があっても typo は `front` の `item_not_found`、lib 名の違う member の lib (`other-pkg` の `other_lib`)
- 残したもの: member の glob で再公開された item は、`list_module` と検索には出ない (たどるのは `get_item` 系のパスの解決だけ)。glob の行き先の member のビルドに失敗したとき、`refresh` は行き先に届かない (行き先を直接引く案内を付ける)。`dependency_of` は dev・build・target 別の依存を区別しない。glob の連鎖では 1 回の呼び出しで `build.wait_secs` の数倍待ちうる

**D7 の下調べと決定 (2026-10-22)**:

- 下調べ (サブエージェント、読むだけ): path・git・別レジストリ・checksum の不一致の依存は `local_build_required` (`docs_unavailable`)、`crate` なしの検索では `unavailable`。`no_target_json` は既定のプロジェクトの依存かを区別しない。B7 の範囲の判定 (`build_scope::Buildable`) は照会のツールから呼ばれていない (`build_unsupported`・`scope_unknown` はどこからも返さない)。member の計画を依存に使うには、`--manifest-path` を依存の Cargo.toml でなく workspace の根にする (依存の Cargo.toml だと cargo がそれを根にして Cargo.lock を作り直す。R-WS-5)、private と hidden を付けない印、照会の側の「member なので private も出す」(`Origin::Local`) と別の印が要る。独自の target の `target_unsupported` と docs.rs の既定の target へのフォールバックは無い。`crate` なしの検索に workspace の member は入らない。path・git の依存と同じ版を明示すると crates.io の同名を warning 無しで引く
- 実測 (build-scope の fixture、resolver 2、rustc 1.99.1): `cargo rustdoc -p <依存>` は、`--target` を付けても付けなくても、プロジェクトの通常のビルドの feature の組で文書化する (`shared` は `normal` だけ。build 用の `build` は入らない)。build-dependency だけ・dev-dependency だけ・ほかの target だけの依存は、`--target` の有無によらず feature resolver が panic する。proc-macro の下の依存は通る (B7 の名指しできるかの判定と一致)
- 利用者が決めたこと:
  1. D7 を 4 つに分ける: D7a (依存のローカルビルドの計画と path・git・別レジストリ)、D7b (checksum の不一致と `no_target_json` をローカルビルドに)、D7c (実際のビルド target と違う `target` ではビルドしない、独自の target は `target_unsupported` と docs.rs の既定の target)、D7d (`crate` なしの検索に workspace、`sources` の `building`、明示した版の扱い、`real_data` を 3 OS で)
  2. member でない依存は、ローカルでビルドしても公開の item だけを出す (R-SH-5 のまま。依存の private を出すと、モデルが使えない item を使い、検索と一覧が内部で埋まり、取得元で同じ crate の見え方が変わるため)
  3. ローカルでビルドした依存の provenance に `package_source` (`path`・`git`・`registry`・`crates.io`) を足す (`source` は `local` のまま。member には付けない)
- 聞かずに決めたこと: 依存はプロジェクトの workspace の根で `--locked -p <依存>` でビルドし、feature は渡さず cargo の既定 (プロジェクトの通常の組) に任せる。名指しできない依存はビルドの前に B7 の判定で `build_unsupported`、判定できなければ `scope_unknown` (R-WS-5)

**D7a (path・git・別レジストリの依存のローカルビルド。2026-10-22)**:

- 計画 (`Plan`): `document_private` を足し、member は `--document-private-items --document-hidden-items` を付け、依存は付けない (決定 2)。鮮度の鍵の facts の `documents` も `private hidden` か `public`。ビルドの記録に `private` を足した (前の記録は member なので既定は真)
- 依存の計画 (`mcp::tool::local::dependency`): プロジェクトの package (`project::Found`) のうち member でないものを `cargo metadata` の package (名前・版・取得元) に対応づけ、`Resolver::scope_until` (新設。範囲の判定を期限付きで待つ) で `-p` で名指しできるかを確かめる (名指しできなければ `build_unsupported`、その target で作られなければ `build_unsupported` と target の案内、範囲が分からなければ `scope_unknown`、cargo の解決・計画の途中なら `status: building`)。`--manifest-path` は workspace の根の Cargo.toml (依存の Cargo.toml だと cargo が別の根にして Cargo.lock を作り直す。R-WS-5)、文書化するのは lib (`build::lib_of`)、features は渡さない (実測でプロジェクトの通常の組)、`[package.metadata.docs.rs]` の組と cfg は使わない、入力は member と同じく workspace のファイルと path の package
- 照会: `get_item` 系の `in_member` で member でない package は `dependency_request` で依存の計画に回す (前は `local_build_required`)。`Origin::Local` に `private` (member なら真) と `package_source` を足し、private の扱い (`is_member`・`public_only`・検索の `restricted`) は `private` で決める。provenance に `package_source` (`path`・`git`・`registry`・`crates.io`。パッケージ id から。決定 3)。出力スキーマに足した
- テスト: stdio で、workspace の外の path 依存がローカルでビルドされ (`source: local`・`package_source: path`、`visibility` が無い)、private な関数は `item_not_found`、build-dependency だけの path 依存は `build_unsupported` (依存にも private を付けると落ちることを確かめた)
- レビュー (サブエージェント) の指摘と直し:
  1. branch・tag・rev を付けた git 依存は、package id の `?branch=main` を cargo が glob と読み、`-p` が `glob patterns on package selection are not supported` で落ちる (cargo 1.99.1)。`PackageId::spec` は、取得元に `?` があれば `git+` と query を外して `<url>#<name>@<version>` を渡す。tests/local に、member と同じ名前・版の git 依存 (branch 指定) を依存の計画でビルドし、公開の item だけが出て member のものが出ないことを確かめるテストを足した (直しを外すと上の文言で落ちることを確かめた)
  2. member と同じ名前・版の git・別レジストリの依存が、member としてビルドされる。`in_member` は取得元が path でない package を member の判定に回さず依存に回す。同じ名前の member と版が違う path 依存 (`Member::OtherVersion`) も依存に回す (前は `local_build_required`)。re-export の行き先を探すとき、member の依存の表 (`member_named`) は起点が member のビルドのときだけ使う (`LocalSource::is_member`)
  3. member・依存から path・git の依存への re-export を追わなかった (`NotFromCratesIo`)。行き先がプロジェクトの crates.io でない package で、起点がローカルのビルドなら、依存の計画でビルドして追う (`InProjectBuild::Dependency`、`member_destination` を `built_here_destination` に広げた)。docs.rs の crate からは、docs.rs でビルドに使ったのが crates.io の版なので、今までどおり追わない。依存のビルドの JSON の `html_root_url` も、member と同じく版の手がかりに使わない (`is_built_here`)。stdio のテストに、member の re-export が path 依存に入ること (`package_source: path`) を足した
  4. feature resolver 1 のとき、build-dependency (proc-macro でない) は名指しできる (`Buildable::ForHost`) が、頼まれた target でビルドしていた。host の target でビルドし、頼まれた target と違えば warning を付ける。stdio のテストに、resolver 1 の build-dependency を `wasm32-unknown-unknown` で引いて host でビルドされることを足した
  5. `package_source` が古い形式の package id (`name ver (source)`) を見分けなかった。括弧の中を見る。単体テストを足した
  6. 文言: `build_unsupported` は、host でしか使わない (build script・proc-macro のための) 依存と書き、提案は build_external_docs を先に (path・git は docs.rs に無い)。`scope_unknown` は、失敗を覚えるので `retryable: false` と refresh の提案。`not_built_for` の提案は `target` の引数と書く
  7. 古い文言: `local_build_required` は「member だけ」をやめ、lib の無い依存か、crates.io の版の写し (D7b まで) と書く。docsrs を外した warning は、依存では rustdoc-args の cfg に触れない。`planning_crate` は feature の組に限らず「プロジェクトのビルドを読んでいる」。ビルド中の応答 (`local_provenance`) にも `package_source`
  8. (記録だけ) 鮮度の鍵の facts の `RUSTC_BOOTSTRAP` は、依存でも値の全体 (R-WS-4 は依存の分だけを想定していた)。依存の鍵が要らず変わることがあるだけで、正しさには響かない
- 直しの再レビュー (サブエージェント) の指摘と直し:
  1. 依存のビルドからの re-export の行き先を名前だけで引き (`look_up`)、member が直接使う版や、同じ名前・版の member に飛んでいた。`PackageGraph` に全 package の依存の辺 (`edges`) を持たせ、`dependency_of` を member に限らず引けるようにした。起点が依存なら、その package id の名前・版の member でない package (`non_member_named`、ひとつに決まるときだけ) の依存の表から引く
  2. ビルド中・待ち・失敗の応答の provenance に `package_source` が付かなかった (`local_provenance` が見る `arguments.local` は先に取り出されていた)。取り出す前に出どころを求めて渡す (`LocalSource::package_source`)
  3. `build_unsupported` の文言が dev-dependency だけの依存に当てはまらなかった。「target への通常の依存で届かない (テスト・build script・proc-macro だけが使う)」と書く
  4. host でビルドする旨の warning を共有の計画 (`LocalWanted.warnings`) に入れていたので、鍵 (host) が同じ別の target の呼び出しと取り違え、記録にも残りえた。この warning はやめ、proc-macro と同じく、頼まれた target とデータの target が違う旨の前からある warning に任せる (呼び出しごとに付く)
  5. `package_source` の判定を、部分一致から `Source::parse` (取得元の完全一致) に合わせた
  6. `not_built_for` の案内を「ほかの target で使うなら」と条件付きに (どの target でも作られない依存にも出るため)
  7. `PackageId::spec` の単体テストの、実在しない形の id の例を直した
  8. テスト: stdio に、member と同じ名前・版の git 依存 (branch 指定、`file://`、テストの cargo home) を、member の re-export と path 依存の re-export から引き、どちらも git の依存のビルド (`package_source: git`、公開の item だけ) になり、ビルド中の応答にも `package_source` が付くことを足した (1 の直しを外すと path 依存からの方が落ちることを確かめた)
- 再レビューで残したもの (記録だけ):
  - `ForHost` の host は、キャッシュのディレクトリでの `rustc -vV` の host で、プロジェクトの toolchain の host ではない (`--target` を省く判定の決定と違う)。プロジェクトの toolchain が別の host 向けのとき (Rosetta の Mac など) だけ食い違う。直すにはビルドの側で `check_target` の host を使う印が要る
  - 同じ git の URL を別の branch・rev で引いた同じ名前・版の package が 2 つあると、query を外した spec が両方に合い、cargo が曖昧と言って失敗する。cargo 1.77 より前の形式の id も `name@version` に落ちるので、同じ名前・版の member と git 依存では曖昧になる
  - `FeatureSets` の鍵が名前・版なので、member と同じ名前・版の git 依存の `enabled_in_project` を member の組で判定しうる
  - member をやめて path 依存にした直後、期限を過ぎた呼び出しは前回の member のビルド (`private: true`) を返す (D4c の決定どおり、ビルドし直すまでの間だけ)
- 残したもの: 別レジストリの依存の結合テストは無い (path・git と同じ道を通る)。checksum の不一致と `no_target_json` は D7b、`crate` なしの検索の `local_build_required` は D7d

**D7b の下調べと決定 (2026-10-22)**:

- 下調べ (サブエージェント、読むだけ): checksum の不一致・checksum 無し・index に無い版 (`DocsRsCheck`) は、`get_item` 系の `resolve` で `local_build_required` (crate を引くほかのツールも `get_item::execute` を通るのでここで決まる)。プロジェクトの crates.io の package の crate が index に無い (私的なミラー) と `crate_not_found` で、`NotInIndex` に届かない。re-export の行き先 (`InProjectBuild::Release`) と `crate` なしの検索 (`search_across::dependencies`) は照合せず、合わない写しを黙って docs.rs の版で見せる (R-VR-2 に反する)。`no_target_json` は docs.rs の読み込み (`load_error_of`) で `docs_unavailable` になり、プロジェクトの依存かを区別しない (`Request` はプロジェクトの package を持たない)。要件 §5.1・R-DS-3 は `no_json` (2025-05-23 より前・docs.rs のビルドの失敗・対応しない形式) もローカルビルドで補うと求める。Cargo.lock の版 1 は checksum を `[metadata]` に持ち、今は読まないので crates.io の package がすべて `NoChecksum`。D7a の依存のビルドも渡された target でビルドする (R-MCP-3 の判定が無い)。`build.prefer_local` (R-DS-1) は実装が無い
- 利用者が決めたこと:
  1. 戻す範囲: `no_target_json` に加え `no_json` (2025-05-23 より前・docs.rs のビルドの失敗・対応しない形式) もローカルビルドで補う。`unparsable`・`too_large`・`version_mismatch` は今のまま
  2. 戻す時点: docs.rs の読み込みが失敗した後 (`Request` にプロジェクトの package を持たせる)。docs.rs に聞く回数は今と同じ。覚えている失敗は `.crate` の取得の前に見て、無駄な取得を省く
  3. 対象: プロジェクトが版を決めたもの (`cargo_lock`)、そこからの re-export の行き先、`crate@version` で明示した版がプロジェクトの依存と同じもの (計画では D7d だった分を含める)。`cargo_lock_raw` はビルドしない (R-VR-2)
  4. target の制限: D7c の「実際のビルド target か」の判定だけを D7b に前倒しし、依存の計画の共通の入口に置く (D7a の path・git も正しくなる)。独自の target の `target_unsupported` と docs.rs の既定の target は D7c
  5. `NoChecksum`: Cargo.lock の版 1 の `[metadata]` の checksum も読み、本当に無いものだけローカルビルドに (cargo が版 1 を `--locked` で受けるかは実装で実測)
  6. index に無い: TTL を無視して 1 回読み直し、それでも版が無ければローカルビルド。crate 自体が無い (私的なミラー) ときもローカルビルド。オフライン・通信の失敗で読めないときは今のまま (`offline_miss`・`network_error`、残りとして記録)
  7. 戻した後にビルドが失敗したら、ビルドの失敗 (`build_failed` など) を返し、失敗の記録と refresh (D5) に乗せる。docs.rs にあるもの (既定の target の JSON など) を message か suggestions に足す
  8. 知らせ方: provenance は D7a のまま (`source: local`・`package_source: crates.io`)、理由は warnings に 1 行。スキーマは増やさない
  9. re-export の行き先: 起点がローカルのビルドなら index で照合し、合わなければ依存としてローカルでビルド。起点が docs.rs の crate なら今のまま docs.rs を使い warning を付ける
  10. `crate` なしの検索の照合とローカルビルドは D7d でまとめて (並列度・`sources` の `building` と)。それまで合わない依存を docs.rs で検索することを残りとして記録
  11. `build.prefer_local` (R-DS-1) は M2 の後に回し、記録だけ
- 段の分け方 (聞かずに決めたこと): D7b-1 (実際のビルド target の判定。決定 4)、D7b-2 (checksum の照合。決定 5・6・9)、D7b-3 (docs.rs の失敗と明示した版。決定 1・2・3・7・8)
- 利用者が決めたこと (続き):
  12. D7b-1 の判定は member にも当てる (R-MCP-3 のとおり。member を他の target で見る手段は外のツール (M2e) までなくなる)
- 実測 (rustc・cargo 1.99.1、オフライン、cargo home はテスト用): vendor (`replace-with` で directory source) の crates.io の package は、`cargo metadata` の id が `registry+https://github.com/rust-lang/crates.io-index#demo@0.1.0` のままで、`cargo rustdoc --locked -p <その id>` が vendor の中身を文書化する。Cargo.lock の checksum は vendor の `.cargo-checksum.json` の `package` の値。Cargo.lock の版 1 (`version` の行が無く、checksum は `[metadata]` の `"checksum <name> <version> (<source>)"`) は、`cargo metadata --locked`・`cargo rustdoc --locked` とも受け、Cargo.lock を書き換えない

**D7b-1 (実際のビルド target の判定。2026-10-22)**:

- `project::build_targets`: プロジェクトの実際のビルド target。cargo の設定の `build.target` (配列ならそのすべて)、無ければ host (サーバーのキャッシュのディレクトリでの `rustc -vV`)。独自の target (target JSON・`build-std`) と読めない設定は、`target` を省いた照会と同じく host とみなす (D7c まで)
- `local::built_for`: member の計画 (`local::member`、member と決まった後) と依存の計画 (`local::dependency`) の入口で、頼まれた target が実際のビルド target のどれでもなければビルドせず、`docs_unavailable`・`reason: no_json`・`retryable: false` (実際のビルド target の一覧と、外のツール (後の版) の案内)。member でない crate (`crate@version` の docs.rs の照会など) は止めない
- テスト: stdio の build-dependency のテストを、プロジェクトの `build.target` を `wasm32-unknown-unknown` にして `target` を省く形に改め (host 用の依存は host でビルドし、データの target が違う旨の warning)、`x86_64-unknown-none` では path 依存も member もビルドしないことを足した
- レビュー (サブエージェント) の指摘と直し:
  1. `crate@version` で明示した版の member が他の target で止められると、`explicit_member` がその `Err` を握りつぶし、crates.io の同じ名前の別物の照会に化けた (テストでは crates.io の `app` の `version_not_found`)。`Member::NotForTarget` を足し、明示した版の経路ではそのエラーを返す
  2. cargo 1.91 から `build.target` に書ける `"host-tuple"` (host の triple に置き換わる) を文字どおりの target として扱っていた (前からの `project_target` も)。`build.target` の解析を `targets_of` にまとめ、`host-tuple` を host にする。`build_targets`・`project_target`・`sets_build_target` は設定を同じ関数 (`read_cargo_config`) で読む
  3. 解析できない要素があるとき、判定 (残りを使う) と既定の target (host) が食い違った。`targets_of` は 1 つでも解析できなければ全体を host とみなす
  4. build-dependency・proc-macro の依存を host を明示して引くと止めていた。host が実際のビルド target でないときは、実際のビルド target (先頭) の範囲で host 用 (`ForHost`) のものだけ通す
  5. 文言: target が 1 つのときの案内を `give <target> as "target"` に。同じ名前の変数が関数を隠していたのを改名
  6. テスト: `targets_of` の単体テスト (配列・`host-tuple`・target JSON・build-std・target でない値)。stdio に、host を明示した host 用の依存、明示した版の member、host を明示した target 用の依存が止まること、`reason: no_json`・`retryable: false` を足した (1 の直しを外すと落ちることを確かめた)
- 残したもの: 判定の host はキャッシュのディレクトリでの `rustc -vV` の host (R-TG-1・`default_target` と同じ)。プロジェクトの toolchain が別の host 向けのときと、`rustc -vV` が使えず代わりの値のときは食い違う (D7a の再レビューの残りと同じ)。stdio のテストのサーバーは、走らせる人の `CARGO_BUILD_TARGET` と `~/.cargo/config.toml` を引き継ぐ (前から)

**D7b-2 (checksum の照合。2026-10-22)**:

- 版を Cargo.lock が決めた crates.io の package (`resolve` の `entry_or_copy`): index と照合し (`project_entry`)、release でなければ (checksum の不一致・checksum 無し・index に無い版・crate が index に無い) 依存の計画 (`dependency_request`) でローカルにビルドし、理由を warnings に 1 行 (`copy_request`。決定 8)。lib の無いものは `local_build_required`。`cargo_lock_raw` は `cargo metadata` の失敗を返し、ビルドしない
- index の読み直し (決定 6): index に版が無い、または crate が無いときは、`SparseIndex::entry_fresh` で TTL によらず条件付きの要求で 1 回読み直す (「無い」の記憶も消す)。同じ crate の読み直しは TTL (10 分) に 1 回で、その間は普通に読む。オフライン・通信の失敗は今のまま (キャッシュか `offline_miss`・`network_error`)
- Cargo.lock の版 1 (決定 5): `[metadata]` の `"checksum <name> <version> (<source>)"` から checksum を読む (`<none>` は無し)
- re-export の行き先 (決定 9): `InProjectBuild::Release` にプロジェクトの package を持たせ、`release_destination` で照合する。起点がここでビルドした crate なら写しを依存としてローカルでビルドし warning、起点が docs.rs の crate なら release のまま (docs.rs はそれでビルドした) で warning。index が読めなければ release のまま
- 文言: `local_build_required` は lib の無い依存だけになった。`metadata_failed` は「not from crates.io」をやめ「docs.rs does not have」
- テスト: stdio に、vendor で中身を変えた `demo` 0.1.0 (checksum が index と合わない、オフライン) を直接引いても member の re-export から引いても、依存としてローカルでビルドされ (`package_source: crates.io`・理由の warning)、private な関数は出ないことを足した (行き先の照合を外すと re-export の方が落ちることを確かめた)。単体テストに、Cargo.lock の版 1 の checksum、`entry_fresh` の 1 回だけの読み直しと、無かった crate の読み直し
- レビュー (サブエージェント) の指摘と直し:
  - (1) 照合する index を問い合わせの名前で引いていた。lib 名 (`md5` の `md-5`) や改名で引くと別の crate の index を読み、誤った理由の warning 付きでビルドしていた (前は誤った `local_build_required`)。プロジェクトの package 名で引く。stdio のテストに、vendor の `demo` の lib 名を `patched_demo` にして lib 名で引くことを足した (直しを外すと `offline_miss` で落ちる)
  - (2) 起点が docs.rs の crate で行き先が合わない写しのとき、要求にプロジェクトの checksum と workspace が残り、`.crate` の取得が checksum の不一致で毎回失敗し、写しのビルドの組で `enabled_in_project` を付けていた。この分岐では checksum を外し (index の値で取り直す)、プロジェクトのビルドの組も付けない
  - (3) 古い index (オフライン・通信の失敗) で版が無いことを「crates.io が載せていない」と断定してビルドしていた。前例 (R-CA-3、`version_not_found`) と決定 6 の「今のまま」に合わせ、`offline_miss`・`network_error` を返す (`InIndex::Unlisted`、`stale_index_error` を共有)。checksum の不一致は版が載っているので古い index でも照合できる
  - (4) 写しの経路のエラー (`build_unsupported`・`no_json`・`metadata_failed` など) とビルド中の応答に docs.rs を使わない理由が出なかった。message の頭か warnings に付ける (`told_why`)
  - (6) refresh (`SparseIndex::forget`) が読み直しの記録を消さず、10 分の間は読み直さなかった。消す
  - (8) 文言: docs.rs が起点のときの warning を「crates.io の版を代わりに見せる」に。行き先の `local_build_required` に実際の理由 (check) を渡す。`metadata_failed` の doc コメント
  - (9) テスト: `entry_fresh` のテストが 3 回目の要求が無いことを守っていなかった (サーバーが閉じて古いキャッシュに落ちても通った)。古くないことと要求が無いことを確かめ、refresh の後は読み直すことを足した
- 残したもの:
  - (5) 読み直しの記録は取り直しの前に付けるので、同じ crate の同時の呼び出しのうち後のものが、取り直しがキャッシュのロックを取る前に古いキャッシュを読むと、答えが割れうる (窓は短い)
  - (7) `entry()` が今まさに取り直した直後でも、版が無ければもう 1 回条件付きの要求を出す。re-export の行き先ごとに index を読む (決定 9 のとおり、TTL 内はキャッシュ)
  - (10) `resolve_version` は新しい照合を使わず、版が index に無いと `version_not_found`・crate が無いと `crate_not_found` (D7d で)
  - 結合テストの無い経路: 版や crate が index に無い (オンラインの index のモックが要る)、読み直しで公開済みと分かり docs.rs に戻る、docs.rs を起点にした行き先の写し、`cargo_lock_raw` の写し
  - `crate` なしの検索の照合は D7d (決定 10)。`list_crates` の `docs_rs_json` は checksum を見ずに `cached` を付ける (軽微)

**D7b-3 (docs.rs に JSON が無い release。2026-10-22)**:

- `Request` に、ここでビルドするもの (`Here`: 前からの `local` と、新しい `if_missing`) を持たせた。`if_missing` は docs.rs に JSON が無いときにビルドするプロジェクトの package (`project::Found`) と refresh の印で、入れるのは Cargo.lock が版を決めたとき (`cargo_lock`。`cargo_lock_raw` は入れない、R-VR-2)、明示した版がプロジェクトの crates.io の release と同じとき (`the_release_in_build`。決定 3)、re-export の行き先がプロジェクトの release のとき (`release_destination`)。行き先が合わない写しで docs.rs の版を見せるときは入れない
- 戻すきっかけ (決定 1): docs.rs の読み込みの `NoTargetJson` と `Fetch(NotFound | BadRequest)` (2025-05-23 より前・docs.rs のビルドの失敗・対応しない形式は、どれもこれになる)。`unparsable`・`too_large`・`version_mismatch` は今のまま
- 戻す時点 (決定 2): 読み込みが失敗した後 (`load_and_look_up` が `Unloaded::NoJson` を返す)。覚えている失敗 (`data::remembered_no_json`) は `.crate` の取得の前に見て、取得を省く
- 戻し (`built_instead`): 依存の計画 (`dependency_request`) で要求を作り、refresh を引き継ぎ (`refresh_release` が `if_missing` にも印を付ける)、理由を warnings に 1 行 (決定 8)。計画やビルドが失敗したら、その失敗を返し、message の頭に docs.rs に無い理由を付ける (決定 7。`told_why`)。`no_target_json` の理由は「docs.rs は他の target の JSON だけを持つ」
- `resolve` が長くなったので、`chosen_by` は構造体 `VersionChoice` を返す
- テスト: stdio に、vendor の `demo` 0.1.0 (crates.io の release と同じ checksum) を、キャッシュに docs.rs の失敗の記録 (`missing`・`no_target`) を置いてオフラインで、Cargo.lock の版・明示した版・member の re-export から引き、どれもローカルでビルドされて理由の warning が付くことを足した (戻しを外すと落ち、`.crate` の前の判定だけ外しても読み込みの失敗の後の経路で通ることを確かめた)
- レビュー (サブエージェント) の指摘と直し:
  - (1) 対応しない形式 (R-FV-4) は、1 回目は形式の読み取りの失敗 (`ArchiveError::Make` の `UnsupportedFormat`) で返り、戻らなかった (2 回目は覚えた失敗で戻った)。これも戻す
  - (2) lib 名が package 名と違う crate (`md-5` の `md5`) を戻すと、path の頭が二重 (`md5::md5::Md5`) になり `item_not_found` だった。`member_request` は頭を lib 名に置き換えた後、続く lib 名を 1 段落とす (docs.rs の照会の `with_root_name` と同じ規則)
  - (3) 起点が `cargo_lock_raw` の re-export の行き先にも `if_missing` が入っていた。入れない
  - (4) 戻したビルドにその item が無いだけの `item_not_found` にも理由の前置きが付いた。付けない
  - (5) 読み込みの失敗の後に戻すとき、docs.rs のビルドについての warning (`.crate` の取得の失敗など) が残った。docs.rs を読む前の分まで戻す
  - テスト: D7b-3 のテストの vendor の `demo` の lib 名を `patched_demo` にして lib 名で引くこと (2 の直しを外すと落ちる)、docs.rs のビルドの warning が無いこと、一時的な失敗 (`transient`) を覚えているときは戻さないことを足した
- 再レビューで残したもの (記録だけ):
  - (6) プロジェクトのビルド target でない `target` を渡すと、`no_target_json` だった応答が、依存の計画の `no_json` (他の target) か `build_unsupported` に理由の前置きを付けたものになる (R-MCP-3 のとおり)
  - (7) 戻す対象は「明示した版」より広く、版の要件 (`crate@^1`) や latest でもプロジェクトの release と同じ版に解決すれば戻す (`enabled_in_project` を出す前からの扱いと同じ)
  - (8) target 付きの 404 の後の HEAD が失敗すると何も覚えず `Fetch(NotFound)` になり、理由の文は no_json のもの (前から)
  - (10) オフラインで失敗を覚えていなければ戻さず `offline_miss` (決定 1 の範囲の外)
  - `.crate` の前の判定は通信を 1 回省くだけで、オフラインのテストでは見分けられない (外しても読み込みの失敗の後の経路で同じ応答になる)。対応しない形式で戻る経路の結合テストは無い (docs.rs のモックで形式の違う JSON を返すものが要る)
- テストの待ちの上限 (`tests/stdio.rs` の `TIMEOUT`) を 5 秒から 30 秒に (利用者の了承)。D7a の CI の Windows と、手元でテストを並べたときに、最初の応答を 5 秒待って落ちた。上限なので、通るテストは長く待たない
- 残したもの: `crate` なしの検索 (`search_across`) と `resolve_version` は D7d。戻した依存が lib を持たないとき (`local_build_required`) の理由の文言は crates.io の release のものではない (稀)

**D7c の下調べと決定 (2026-10-22)**:

- 下調べ: 要件 R-TG-1 は、独自の target (`build.target` が target の JSON 仕様、`[unstable] build-std`) のプロジェクトで、cargo を起動せずに `target_unsupported` を返し member は手元で作る方法を案内する、docs.rs の照会はその crate の docs.rs の既定の target に代えて実際の triple を provenance と warnings に示す (§ の決定 27)、と求める。今は `target` を省いた照会の target を host にして warning を付け (`project_target`)、D7b-1 の `build_targets` も host とみなすので、member と依存を host でビルドし、docs.rs も host の target で引く。D7c の「実際のビルド target と違う `target` ではビルドしない」は D7b-1 で済んだ
- 実測 (cargo 1.99.1、オフライン): target の JSON 仕様でも `build-std` でも `cargo metadata` は動く。`cargo tree` は target の JSON 仕様では `-Zjson-target-spec` を求めて落ち、`build-std` では動く
- 利用者が決めたこと:
  1. 版の解決の `cargo metadata` は走らせる (ビルドも build script も走らない)。ビルド (`cargo rustdoc`) と `cargo tree` (feature の組・範囲) は走らせず、`enabled_in_project` は出さない (warning)。要件の「cargo を起動せずに」はビルドのことと読む
  2. `target` を明示した照会は、docs.rs ではその target の JSON を引き (既定の target に代えるのは省いたときだけ)、member と依存のローカルビルドはどの target でも `target_unsupported` (プロジェクトはその target のためにビルドされていない。R-MCP-3 と同じ)
- 段の分け方 (聞かずに決めたこと): D7c-1 (ビルドの `target_unsupported`)、D7c-2 (`target` を省いた docs.rs の照会を既定の target に。照会の target を省いたとも表せる形にするので、多くのツールに及ぶ)

**D7c-1 (独自の target のプロジェクトはビルドしない。2026-10-22)**:

- `CargoConfig::own_target`: 独自の target (`build.target` の target の JSON 仕様、`[unstable] build-std` なら `build.target` の target、無ければ `build-std`)。`build-std` だけで `build.target` が無いものも独自の target とする (前は host とみなしていた)
- `project::build_targets` は `BuildTargets::These` か `OwnTarget` を返し、`targets_of` はエラーを独自の target と target でない値に分ける。`target` を省いた照会の target (`project_target`) は今までどおり host と warning (D7c-2 まで)
- member と依存の計画 (`local::targets_built`): 独自の target なら、どの `target` でも cargo を起動せずに `docs_unavailable`・`reason: target_unsupported` (プロジェクトの terminal で文書化する案内。決定 2)
- `cargo tree` (`Resolver` の feature の組) は独自の target では走らせず `FeaturesError::OwnTarget` (`enabled_in_project` を出さない旨の warning。決定 1)。`cargo metadata` は走らせる
- テスト: `targets_of` の単体テスト (`build-std` だけ・target の JSON 仕様・target でない値)。stdio に、`build.target` が `my-target.json` のプロジェクトで member と path 依存が `target` の有無によらず `target_unsupported`、crates.io の依存は明示した target で docs.rs から引けて、プロジェクトのビルドを読まない warning が付くことを足した (`own_target` を外すと落ちる)。D2 からの「独自の target の member は host でビルドする」テストは、`build-std` だけ (`build.target` 無し) で `target_unsupported` になることを確かめる形に改めた
- レビュー (サブエージェント) の指摘は低だけ。直したもの: `build.target` の値の前後の空白を落としてから判定する。文言を「its own std, for a target of its own or with build-std」に (`build-std` と通常の triple の組み合わせにも当てはまるように)。古い doc コメント (`sets_build_target`・`Member::NotForTarget`)。`targets_of` の単体テストに、target でない値と target の JSON 仕様が混ざる配列 (JSON 仕様が勝つ) と空の配列
- 残したもの (記録だけ): 環境変数 `CARGO_UNSTABLE_BUILD_STD` は読まない (cargo が受けるかは未確認)。stable のプロジェクトでも `$CARGO_HOME` の設定に `build-std` があれば `target_unsupported` (R-TG-1 の文言どおり)。`.json` を付けずに `RUST_TARGET_PATH` で名指しする独自の target と、member のディレクトリだけに `.cargo/config.toml` がある構成は判定の外 (前者はビルドの前の検査で `toolchain_missing`)。`cargo tree` の範囲 (`read_scope`) には判定を置いていない (呼び出し元はどれも判定の後)。`CARGO_BUILD_TARGET` で JSON 仕様を足す経路・`list_crates` と `crate` なしの検索の結合テストは無い

**D7c-2 (`target` を省いた docs.rs の照会を既定の target に。2026-10-22)**:

- 利用者が決めたこと: docs.rs を読むツールすべてで一度に (`get_item` の系統・`resolve_version` の `docs_rs_json`・`list_crates` の印・`crate` なしの検索)。std は配布物から読むので対象外 (host と warning のまま)
- `project::lookup_target`: 照会の target (`LookupTarget`: target と、docs.rs をその crate の既定の target で読むか) と warning を返し、各ツールの「`target` が無ければ既定」の分岐をまとめた。独自の target のプロジェクトで `target` を省いたときだけ既定の target (決定 2)。std の照会は `default_target` (host と、独自の target なら host を使う warning)
- `get_item`: 要求の `Here` に `docs_rs_default` を持たせ (`resolve` の後に付ける。re-export の行き先は起点から引き継ぐ)、docs.rs のキャッシュの置き場・読み込み・覚えた失敗・照会の target を `Request::docs_rs_target` (既定なら `None`) にした。照会に target を渡さないので、provenance の target は JSON の実際の triple、「頼まれた target と違う」warning は出ない。読む前の応答 (ビルド中) の provenance には target を載せない
- `crate` なしの検索: 読み込みと照会の target を既定に、全体と読めなかった crate の provenance の target を省く (読めた crate の `sources` は JSON の triple。スキーマはレビューの後に必須を外した)
- `resolve_version`: `docs_rs_json` は target の無い URL で確かめる (`available`・`no_json`)。出力スキーマが provenance の `target` を必須にしているので、そこは host のまま (warning で既定の target で調べたと示す)
- `list_crates`: `docs_rs_json: cached` の印を既定の target の置き場で見る
- テスト: 独自の target のテストに、`target` を省いた `get_item` が既定の target の置き場の JSON を読み (target の置き場を消しても)、provenance の target が JSON の triple で、既定の target の warning が付き、`resolve_version` の `docs_rs_json` が `available` になることを足した (印を外すと落ちる)
- レビュー (サブエージェント) の指摘と直し:
  - (1・2) `crate` なしの検索の応答 (全体と読めなかった crate の `sources`) と `get_item` 系統のビルド中の応答が、provenance の `target` を欠いて出力スキーマの必須に反した。利用者の決定: スキーマで必須を外す (`get_item` 系統の provenance は `crate` だけ、`crate` なしの検索の provenance は必須なし、`sources` は `crate`・`version`)。実際の triple が分からない (docs.rs の既定の target の JSON を読む前) ときだけ省き、分かれば載せる。説明文にその旨
  - (3) 版を明示したときの近道 (`cached_exact`) が host の置き場を見ていた。`resolve` に `LookupTarget` を渡し、既定の target の置き場を見る
  - (4) warning の「provenance が示す」は `resolve_version`・`list_crates` では事実と違った。文言から外し、`get_item` では読んだ後に実際の triple を warning にも書く (`tell_default_target`。R-TG-2)
  - (5) std への re-export を辿ると host の std になる旨の warning が無かった。足した
  - (6) テストの JSON の triple が Linux の host と同じで区別できなかった。`aarch64-unknown-linux-gnu` にし、provenance と warning にその値が出ること、`crate` なしの検索の provenance に `target` が無く `sources` に JSON の triple が載ることを足した (スキーマの必須を戻すと落ちる)。`real_data` の検証器の自己テストは、必須を欠く例を `target` から `crate` に変えた
  - (7) 印 (`docs_rs_default`) をここでビルドする要求にも付けていた。docs.rs の照会だけに付ける
- 残したもの: `resolve_version` と `list_crates` の provenance の `target` は host のまま (読む JSON が無い)。(8) `get_item` の cargo の解決中の応答には host の `target` が載り、`crate` なしの検索の解決中は省く (どちらもスキーマには合う)。`list_crates`・ビルド中の応答・refresh で既定の target の失敗を忘れることの結合テストは無い

**D7d の下調べと決定 (2026-10-22)**:

- `real_data` (tests/real_data) は、このリポジトリをプロジェクトにして testdata の版 (`clap@4.5.39` など、プロジェクトの依存でない) を target を省いて引くので、macOS・Windows では host の target の JSON が docs.rs に無く `no_target_json` になる。D7b-3 の戻しはプロジェクトの依存だけなので効かない。R-TG-1 は docs.rs で別の target の文書で代用しない
- 利用者が決めたこと:
  1. D7 の終わりの印「3 つの OS で `real_data` が通る」は、照会に Linux の target を明示して満たす。テストのクライアント (`tests/real_data/client.rs` の `with_data_target`) が、`target` の無い照会に `x86_64-unknown-linux-gnu` を足す (std の crate には足さない。std は配布物から host の target で読み、3 つの OS で通っている)。target の選び方は stdio のテストが守る。CI の `testdata` の job の `real_data` の Linux だけの条件を外した
- 下調べ (サブエージェント、読むだけ): `crate` なしの検索は範囲が直接依存だけで member を含まず、crates.io でない依存は `unavailable`、crates.io の依存は照合しない。同じ workspace のビルドは target dir のロックで 1 つずつ。明示した版がプロジェクトの path・git・別レジストリの依存と同じ版なら crates.io の同名を warning 無しで返す (R-VR-1 に反する)、写しなら docs.rs の release を warning 付きで返す、`cargo metadata` の解決中は黙って crates.io。`resolve_version` は明示した版でプロジェクトを見ず、照合 (`project_entry`) も使わず、問い合わせの名前で index を引く
- 段の分け方 (聞かずに決めたこと): D7d-1 (`real_data`・明示した版・`resolve_version`)、D7d-2 (`crate` なしの検索)
- 利用者が決めたこと (続き):
  2. R-VR-1 (明示した版がプロジェクトの版と同じならプロジェクトの取得元のものを返して warning) は、版を完全一致で書いたときだけ。semver の要件と latest は crates.io の版を決める問いなので今のまま
  3. 同じ名前・同じ版の package が複数あるときは、member → crates.io の release (checksum が一致) → ひとつに決まる crates.io でない package の順に選び、ほかにもあることを warnings に書く
  4. 版を明示した照会で `cargo metadata` が期限を過ぎて解決中なら `status: building` (答えを揺らさない。member も)
  5. 「crate@version で crates.io の release を読める」の提案は消し、crates.io の同名を引くにはプロジェクトの外で引くよう案内する (引数は足さない)
  6. `resolve_version`: プロジェクトの crates.io の package が写しなら `data.source` は crates.io のまま `docs_rs_json: not_on_docs_rs` と warning (index に無ければ `latest` などは省く)。明示した版が path・git・member と同じなら、今の `not_from_crates_io` の形 (`source: project` と warning)。スキーマは増やさない
  7. `crate` なしの検索が起こすローカルビルドは、範囲の分を一度に共有の仕事として起こし、期限まで待って残りは `sources` に `building` (鮮度の合うものはロックを取らずにすぐ返る)
  8. 検索でも、docs.rs に JSON の無い crates.io の依存と checksum の合わない写しをローカルビルドに戻す (D7b の決定とそろえる)
  9. 範囲の member の crate は、すべての member の lib と、lib の無い member で bin が 1 つだけならその bin (`get_item` の `crate_of` と同じ規則)
  10. ローカルビルドできない crate の `status`: 作れないと決まっているもの (`build_unsupported`・`target_unsupported`・他の target の `no_json`・`scope_unknown`・lib が無い) は `unavailable`、作ろうとして失敗したもの (`build_failed`・`toolchain_missing`・`cargo metadata` の失敗など) は `failed`
  11. 検索の `refresh` は D5 のまま (workspace の失敗だけを忘れる)。失敗した source の suggestions で、その crate を `get_item` の `refresh` で引くよう案内する
  12. 作り直し中の member は前回のビルドで検索し、その source に `stale: true`・`stale_reason: rebuilding` (D4c とそろえる)
  13. 照合で index が読めない crates.io の依存は、release とみなして docs.rs で検索し warning (re-export の行き先と同じ)

**D7d-1 (`real_data` を 3 つの OS で・明示した版・`resolve_version`。2026-10-22)**:

- `real_data`: 決定 1 のとおり (テストのクライアントの `with_data_target`、CI の条件を外した)。手元は Linux なので効き目は CI の macOS・Windows で確かめる
- 明示した版 (`get_item`、R-VR-1。決定 2〜5): 版を完全一致で書いたとき、`explicit_found` がプロジェクトを解決し (期限を過ぎて解決中なら `status: building`)、`explicit_package` が同じ名前 (package 名か lib 名) と版の package を member → crates.io の package → ひとつに決まる crates.io でない package の順に選び、ほかを warnings に書く。member は今までどおり (`explicit_member`)、crates.io の package は照合して写しならローカルでビルド (`copy_request`)、release なら docs.rs、crates.io でない package は依存としてローカルでビルドし、どれも「プロジェクトのもので crates.io の同名と違いうる」warning。プロジェクトに無ければ今までどおり (別の版の member は warning)
- `local_build_required` の「crate@version で crates.io の release を読む」提案は、版を書いてもプロジェクトの外の `project` を渡すよう案内に変えた (決定 5)
- `resolve_version` (決定 6): 完全一致の版も `explicit_found` でプロジェクトの package を探し、crates.io でない package と member は `source: project` の形と R-VR-1 の warning。Cargo.lock が決めた (または明示した) crates.io の package は package 名で照合し (`project_entry` を get_item から共有)、index に無い版なら `source: project`・`docs_rs_json: not_on_docs_rs`・理由の warning、index が古くて読めなければ `stale_index_error`、それ以外は index を package 名で読む (lib 名で引いても別の crate の index を読まない)
- テスト: stdio の path 依存のテストに `libdep@0.1.0` を明示した `get_item` (`package_source: path` と R-VR-1 の warning) と `resolve_version` (`source: project`)。写しのテストに `demo@0.1.0` の `get_item` (写しのビルド) と lib 名 (`patched_demo`) の `resolve_version` (`not_on_docs_rs`)。明示した版の扱いを外すと両方、`resolve_version` を package 名で引くのをやめると写しのテストが落ちることを確かめた
- レビュー (サブエージェント) の指摘と直し:
  - (2) 版を明示した照会が、プロジェクトに無い crate でも `cargo metadata` の解決を待ち `status: building` になっていた (`resolve_version` は前はすぐ答えた)。待つ前に Cargo.lock を直に読み (`metadata::lock_may_have`)、その名前 (package 名、`-` と `_` と大小を区別しない) と版が無ければ待たずに crates.io へ (lib 名で引くときは R-VR-1 を当てない割り切り)。`real_data` のクライアントは `status: building` なら 5 秒おきに呼び直す (遅い runner で `cargo metadata` が期限を越えても揺れない)
  - (3・5) 選んだ package が member なら、その package 名で引く (`explicit_found_member`)。Cargo.lock の直読みのとき path 依存が member とみなされて黙って crates.io に流れていたのは、`cargo metadata` の失敗を返すようになった。lib の無い member や同じ lib 名の member も選んだものを引く
  - (4) 同じ名前・同じ版の crates.io でない package が複数で選べないとき、「too」ではなく「several packages …, so the release of that name on crates.io is looked up」と書く
  - (7) 古いコメントと説明文 (ci.yml、`real_data` の `OTHER_TARGET` と determinism、`get_item`・`resolve_version` の `project` の説明に「完全一致の版のプロジェクトの package を crates.io のものの代わりに」)
  - (8) `real_data` のクライアントの std の判定を、大小と `-` を区別しない形に
  - (9) 写しのテストに、`demo@0.1.0` の R-VR-1 の warning と、`resolve_version` の `demo@0.1.0` (`not_on_docs_rs`)。`lock_may_have` の単体テスト
- CI (aeb4ecc): 11 job すべて緑。macOS と Windows の `testdata` の job でも `real_data` の 4 件が通った (D7 の終わりの印の「3 つの OS で `real_data` が通る」を満たした)
- 残したもの: (1) lib 名と版で引いて release のとき、crates.io を lib 名で引く (前から。`resolve_version` は package 名で答えるので食い違う)。(6) 写しの経路で `copy_request` の warning と R-VR-1 の warning がほぼ重なる。明示した版で解決中の `status: building`・`cargo_lock_raw`・同名同版が複数の結合テストは無い。「プロジェクトの外の `project`」の案内で crates.io の release が引けることは、レビューでコードを追って確かめた (結合テストは無い。`refresh` はそのとき効かない)

**D7d-2 (`crate` なしの検索に workspace とローカルビルド。2026-10-22)**:

- 範囲: workspace のすべての member と、今までどおりの直接依存 (feature の組で通常か dev のビルドに入るもの)。member は lib か、lib が無く bin が 1 つならその bin (決定 9。`local::member` の `crate_of`)。それ以外の member は範囲に入れない
- package ごとの仕事 (`project_outcome`) を一度に起こして期限まで待つ (`project_all`、決定 7): member は `local::member`、crates.io でない依存は `local::dependency` (lib が無ければ `local_build_required`)、crates.io の依存は照合し (`project_entry`)、写しなら依存としてビルド、index に無く古い index しか読めなければ `stale_index_error`、release か照合できなければ docs.rs (照合できないときは warning。決定 13)、docs.rs に JSON が無ければ (`no_json`・`no_target_json`) 依存としてビルド (決定 8)。ビルドは `local::built` の結果を検索の `Outcome` に写す (成功は新しい `Built`、ビルド中の応答は `Going`、エラーは `Failed`)
- 照会: `Built` は `Origin::Local` (member は private、依存は公開だけと `package_source`) で検索する。作り直し中の前回のビルドは、その source に `stale: true`・`stale_reason: rebuilding` (決定 12)
- `status` (決定 10): `local_build_required`・`build_unsupported`・`target_unsupported`・`scope_unknown`・`no_json`・`no_target_json` は `unavailable`、ほかの失敗は `failed`。`failed` があれば、原因を直した後に `get_item` の `refresh` で引くよう hints で案内する (決定 11)。応答の `building_reason` は docs.rs の索引・取得、ビルド、枠の待ち、ロックの待ちの順に選ぶ
- テスト: stdio に、path 依存のプロジェクトの `crate` なしの検索が member (`source: local`) と path 依存 (`package_source: path`) を範囲に入れ、依存の private な関数は出ないこと (member を外すと落ちる)。写しのテストに、検索で写しがビルドされて見つかること。独自の target のテストに、member が `unavailable`・`target_unsupported`。検索を `partial` が消えるまで呼び直す補助 `searched_in_full`
- レビュー (サブエージェント) の指摘と直し:
  - (1、高) member と crates.io でない依存の名前に lib 名を使っていたので、package 名で引く feature の組が当たらず、`-` を含む package 名や `[lib] name` の違う crate の item がすべて `enabled_in_project: false` になり、`sources` の `crate` も lib 名だった。名前は `get_item` と同じく package 名にし、照会の root は、ここでビルドしたものなら JSON の crate 名 (`Member::root`)。stdio のテストに、package 名 `dash-name` の member の検索 (lib 名に戻すと落ちる)
  - (2) 作り直し中の crate を「まだ来る」側に数えず、`partial`・`retry_after_secs` が付かず、それだけを探して 0 件なら「無い」と答えていた。作り直し中も数え、source にも `retry_after_secs`、hints に呼び直す案内 (R-SR-5・R-OUT-9)
  - (3) crates.io の依存の照合で index を期限なしに読んでいた。呼び出しの期限で包み、過ぎたら照合できないときと同じく docs.rs で warning
  - (4) 既存のテスト (`a_search_that_finds_nothing_while_crates_are_fetched_is_building`) は、member のビルドが終わる前に `status` が消えると `partial` で落ちうる。`partial` が消えるまで呼び直す
- CI (014c701) の `check` が 3 つの OS で落ちた: (4) の直しで `partial` が消えるまで待つようにしたが、このテストは `target` に Linux の triple を明示するので、macOS・Windows では member がプロジェクトのビルド target の外として `unavailable` のまま `partial` が消えず (Windows は時間切れ)、`sources[0]` も `demo` でなく member になりうる (Linux では先に `ok`、macOS では `unavailable`)。テストは `demo` の source を名前で探して確かめ、`building` の source が無くなるまで呼び直す形にした
  - (5) refresh の案内が docs.rs の失敗にも出ていた。ローカルビルドの失敗 (`build_failed`・`fetch_failed`・`toolchain_missing`・`toolchain_too_old`) だけにする
  - (6) 照合できない warning が crate ごとに並んでいた。1 行にまとめる
  - (8) 名前を読めない crate も仕事を起こしていた。起こす前に読む
  - (9) stale の印を名前と版で探していたので、同じ名前・版の source が 2 つあると取り違えた。検索した順の位置で付ける
  - (10) 古い説明文 (search_items.md・README・search_across と search_items のコメント)。説明文には「workspace の crate (private も) と直接依存」(2048 文字に収めた)
  - (12) `failure_of` が `retry_after_secs` を落としていた。残す
- 残したもの: (7) ローカルビルドの warning (docsrs なし・新しい形式など) を検索では出さない (docs.rs の分も前から出していない)。(11) 検索のたびに範囲のビルドを一度に起こし `build.wait_secs` まで待つので、member の編集直後は検索が最大 10 秒待ってから stale を返し、その間の `get_item` は同じ workspace のビルドのロックの後ろに並びうる (決定 7 のとおり)。作り直し中の `stale` の検索・index が読めないとき・`cargo_lock_raw`・member の間の path 依存の結合テストは無い。`search.scope` の設定 (R-SR-2) は計画に無く未実装

**M2d の区切りの Claude Code での試し (headless、`claude -p --mcp-config`、release ビルド、空のキャッシュ。2026-10-22)**:

1. 名指しせずに自分の workspace の private な item (`query::local::Rebuilding`) を聞くと、モデルはこのサーバーを使わず grep・sed・LSP で答えた (答えは正確)。D4d の試しと同じ課題が残る (M2f の F4: instructions・説明文)
2. 名指しして `crate` なしの検索: member (`chatora-rustdoc-mcp`、private を含む) と直接依存 28 個の 29 個を範囲に検索した。空のキャッシュで初めの 3 回は member が `building` で `partial: true`、モデルは cargo のプロセスの終わりを待って呼び直し、29 個すべて `ok` になった
3. 名指しして private な item: `visibility: private`、パスは `crate::query::local::Rebuilding` (D4d の試しの `pub(in ::x)` は D6 で直った)。モデルから、フィールドに可視性の印が無い (`pub struct` に private なフィールドが混ざるときに外から作れるかが分からない) と指摘があった
4. 依存 (`tokio::sync::Mutex::lock`): Cargo.lock の版 (1.53.3) を docs.rs から 1 回で返した
- モデルの使用量は 4 問で約 1.7 USD (利用者のアカウント)

**止まる危険の監査と直し 1 (2026-10-21)**:

- 利用者の指示で、デッドロックと止まって戻らない危険を、コード全体で監査した (サブエージェント、読むだけ。D4b のハングの後)。ロックの循環、ガードを持ったままの `.await` (clippy の `await_holding_lock` も `suspicious` の deny で効いている)、自分の結果を待つ形の循環は無かった。止まって長く戻らない箇所は次のとおり
  1. 子の出力: cargo の子孫 (build script が起こした常駐のプロセスなど) が stdout・stderr を持ち続けると、子が終わっても EOF を待ち、cargo なら `build.timeout_secs` (30 分) の間、ビルドの枠と `doc:` のロックを持ったまま止まる (偽の git で再現)
  2. プロセスをまたぐファイルロックの非同期の待ちに上限が無い: ロックを持つ別のプロセスが止まると (Ctrl-Z・デバッガ)、共有の仕事がいつまでも `waiting_for_lock`。同期の待ちには 1 分の上限があり、扱いが食い違っていた
  3. `Resolver::features`・`scope` は、`cargo tree` の間 target ごとの Mutex を持つ。cargo のロックを rust-analyzer が持つと、同じワークスペースのほかの呼び出しも待つ。`Resolver::resolve` は期限なしで待つ
  4. `ChildRecord::drop` が async のスレッドで `thread::sleep` する (子 1 つで最大 約 155 ms。遅れるだけ)
  5. サーバーの終了: stdout が詰まると閉じるのを上限なしで待つ。2 回目の SIGTERM が飲み込まれる (推測)
  6. テストの補助が std の `Command::output()` を時間の上限なしで使う
  7. そのほか (低): `Flights` が `flying` の Mutex を持ったまま `start` を呼ぶ (今の `start` はどれも遅延評価で安全)、解析の枠を待つ blocking スレッドが尽きうる、async の中の小さな同期の I/O
- この段で直したもの (1・2・6):
  - 1: `Child::read_lines` は、子が終わったら出力を 2 秒 (`DRAIN`) だけ読み、それでも終わらなければグループを止めて、もう 2 秒読む。それでも終わらなければ (session を抜けた子孫が持つ)、読めた分で返して warning のログ。読んだものは読み取りの外に置き、諦めても残す。`Child::output` も同じ道を通す。単体テストで、孫が出力を持ち続けても約 2 秒で戻り、孫は止められること、`setsid` で抜けた孫 (Linux) でも約 4 秒で戻り、読めた分が残ることを確かめた
  - 2: `process_lock::lock` に上限を足した。`Cache::lock` は 10 分 (持つ側は大きな JSON の取得と解析の間持つので、同期の `lock_blocking` の 1 分では二重に作ってメモリを倍に使いうる。レビューの指摘) で、過ぎたらロックなしで進む (二重に作っても書いたものは壊れない)。ビルドの `target:`・`doc:` のロックは、上限なしのまま: 決めた順 (`doc:` → 枠) では、持つ側は枠を待つ間も持つので正しく持つ時間に上限が無く、過ぎて進むと Q-29 の上書きが戻る (最初は `build.timeout_secs` + 1 分にしていて、レビューで指摘された)。止まったプロセスが持つと `waiting_for_lock` のまま
  - 6: テストの補助に `output_within` を足し、cargo を走らせる local のテスト (隔離した子・`generate-lockfile`・build_scope・tools・git_failures) で使う。上限を過ぎたら Unix ではプロセスのグループごと止め、出力を付けて失敗させる。出力は読んだそばから送るので、パイプが終わらなくても読めた分が残る。子が終わった後の出力は 5 秒だけ待つ。隔離した子 (600 秒) の中の cargo は 300 秒にして、内側の失敗の本文が出るようにした
- 残したもの (次の段で): 3・4・5・7。`Cache::lock` は、同じパスを待つタスクがプロセスの中に k 個あると、プロセスの中の Mutex を持ったまま順に OS のロックを待つので、k 番目は 10 分 × k 待ちうる (前からある。同期の `lock_blocking` も同じ)
- 直し 2 の後に、デッドロックと止まる危険をもう一度コード全体で監査する (利用者の指示)

**止まる危険の直し 2 (2026-10-21)**:

- 3 (利用者の決定: 期限で building を返す): `Resolver` の feature の組 (`features`) と範囲 (`scope`)、docs.rs の組で範囲の外を作るか (`outside_scope`) を、`cargo metadata` と同じく (ワークスペース, target) ごとの共有の仕事 (`Flights`) にし、期限付きの待ち (`features_until`・`outside_scope_until`) を足した。結果を覚える Mutex は `cargo tree` の間持たない (共有の仕事が 1 回にまとめる)
  - member の計画 (`mcp::tool::local::member`) は期限までに分からなければ `Member::Resolving` (`status: building`、`building_reason` は cargo の待ち)
  - 結果の注記にだけ使う `get_item`・`list_crates`・`search_items` は、期限までに分からなければ、分からない旨の warning を付けて結果を返す (注記が無いだけで、待たせて building にするほどではない。`in_project_build` の `resolve` も期限付きにした)
  - `cargo tree` の待ちの理由: `cargo::run` が、共有の仕事が tokio の task-local に置いた `Stage` に、cargo が stderr に書くロック待ちと取得を伝える (`cargo::staged`)。`cargo tree` もパッケージキャッシュのロックを rust-analyzer などが持つと `waiting_for_lock` になり、空けば結果が出ることをテストで確かめた (`cargo metadata` のテストに足した)
- 4: `ChildRecord::drop` の片付け (ロックの待ちに最大 約 155 ms の `thread::sleep` を含む) を、tokio のランタイムの中なら blocking のスレッドに回す
- 5: シグナルで止め始めて 10 秒 (`STOP_LIMIT`。rmcp は呼び出しを 5 秒、自分のタスクを 2 秒待つ) たっても終わらないか、2 回目のシグナルが来たら、セッションを待たずに `serve` から戻る (`clippy::exit` を禁じているので `process::exit` は使わず、`main` の `shutdown_timeout(1s)` で終わる)。後のサーバーが残りを片付ける (C4)
- 7:
  - `Flights::join` の `start` は flights を lock したまま呼ばれる: 仕事を作るだけで、同じ flights を呼んだり待ったりしてはならない制約を doc に書いた (今の `start` はどれも守っている)
  - 解析の枠 (同時に 2 つ) の待ちを、async の呼び出し元では task として待つ (`Parsing::enter_as_task`。blocking のスレッドを Condvar で塞がない)。`load::archive_from` (`archive_of` の非同期版) は、ロック (`Cache::lock`) と枠を非同期に待ち、解析と書き込みだけを blocking のスレッドで行う。ローカルビルド・std の先回り・検索の std・docs.rs (`Release::make`) はこれを使う。`get_item` が std のパスをたどる途中で開く分 (同期のたどり方の中) だけは同期の `archive_of` のまま (std は共有の仕事が先に作るので、普通は開くだけ)
- 利用者の指示で、async の中の長い同期処理は `spawn_blocking` に回し、同期のロックより非同期のロックが望ましいところは直す。ロックの基準 (利用者が了承): 持ったまま `.await`・I/O・長い待ちをするものは非同期 (`tokio::sync`) にするかロックの外に出し、表や数を一瞬だけ触るもの、`Drop` や blocking のスレッドの中のものは `parking_lot` のまま (`Drop` と同期の関数では `.await` できず、`parking_lot` のガードは `Send` でないので `.await` をまたぐ持ち方を型で防げる)
  - サブエージェントで洗い出した (読むだけ)。async のワーカーを分単位で止める同期の待ちは無かった。blocking のスレッドに回したもの:
    - docs.rs の JSON の zstd の展開 (数百 MB まで)
    - ローカルビルドの JSON のヘッダ読みと SHA-256、記録の JSON 化 (入力の数に比例)
    - std の dist のアーカイブの SHA-256 (数十 MB。展開と同じ blocking の中で)
    - sparse index の解析 (版の多い crate で MB。本体は `Arc<[u8]>` で渡す)
    - `freshness::sources` の git の一覧の後処理 (ファイルごとの stat)
    - Cargo.lock の解析 (2 回していたのを、読むのと同じ blocking の中で 1 回に)
    - `.crate` を丸ごと読んで SHA-256 を取り、キャッシュに書く `CratesFiles::crate_file` と、`.crate` から Cargo.toml を取り出す `docs_rs_build`
    - toolchain の形式の記録の読み書きと試しのファイル操作 (fsync を含む)
    - cargo の設定の読み込み (`Programs::used_in` を async に)
    - `cargo tree` の出力の解析
  - `std_json` が tokio の `spawn_blocking` を直接使っていた 2 か所を `crate::task::spawn_blocking` に (ログの `request_id` を保つ)
  - `get_item` の std の照会は、頼まれた crate と std・alloc・core のアーカイブを先に非同期版で用意し、blocking のスレッドの照会は開くだけにした (同期の `lock_blocking` と解析の枠の Condvar を通らない)
  - ログのファイル: ロックを持ったまま書いていたのを、書き込み専用のスレッド (`logging::writer::Lines`、上限 4096 行の `sync_channel`) に渡す形にした (利用者の決定)。あふれたときだけ書く側が待つ。終了のとき (`main` の最後) に 1 秒まで書き切る。tracing-appender の `non_blocking` も考えたが、その `WorkerGuard` は drop のときにチャネルが詰まっていると stdout に `println!` する (0.2.5 の `non_blocking.rs:296`)。stdout は JSON-RPC 専用 (R-SEC-1) なので使わず、自前で書いた (利用者の決定)
  - 残したもの: 一瞬で終わる stat・小さなファイルの読み・プロジェクトの Cargo.toml の読み、sysroot の `read_dir` (約 100 件)。パッケージ数の 2 乗の CPU 処理 (`PackageGraph::from_metadata`・`list_crates` の並べ替え・`follow` の `retain`。索引を作る話で、別の段)。`get_item` が std のパスをたどる途中で開く同期の `archive_of` は残る (先に用意するので、普通は開くだけ)
- レビューで直したこと:
  - 版を明示した照会 (`name@=ver`) で、member だと分かった後に `cargo tree` が期限に間に合わないと、`Member::Resolving` (member かどうか分からない) と取り違えて crates.io の同名・同版を返していた。新しい `Member::Planning` で、どちらの照会の道でも `status: building` を返す (文言は「cargo is reading the features of the project's build」)
  - 入れ子の共有の仕事 (features → resolve、outside → scope → resolve) の待ちの理由が外側に伝わっていなかった。`Flights::wait_telling` で、内側の仕事の段を 0.1 秒ごとに外側の段に写す (task-local の `Stage` がある仕事の中だけ)
  - `list_crates` は features を新しい期限で待ち、呼び出しの期限を越えていた: 呼び出しの `until` を渡す
  - 注記の warning の文言をそろえた (`still_reading`、仕事が結果なしで終わったときの `NOT_READ`)。member の計画では、features の仕事が結果なしで終わっても `stopped` にせず、読めなかったときと同じに扱う
  - `get_item` の std の用意は呼び出しの期限で打ち切り、間に合わなければ `status: building` (作りかけは裏で続く)
  - `ChildRecord::drop` は、まず待たずにロックを試して消す (終了の途中では blocking の仕事が捨てられるため)。取れなければ blocking のスレッドで待つ
  - `.crate` のロックを、チェックサムの確かめと書き込みの blocking に持たせる (呼び出しが落とされても書き終えるまで持つ)
  - ログ: `logging::init` は、drop で書き切る `Flushing` を返す (`main` から panic が巻き戻るときも書き切る)。書き込みのスレッドが止まったら stderr に 1 度だけ知らせる
  - シグナルの受け口は最初に 1 度だけ作る (`StopSignals`): 1 回目と 2 回目の間に来たシグナルを取りこぼさない
  - 前からあった warning の文字列の、行の継ぎ目の空白を直した
- 直し 2 の後のデッドロックと止まる危険の再監査 (利用者の指示。サブエージェント、読むだけ): 永久に解けないデッドロック (ロックの循環・自分の結果を待つ循環・通知の取りこぼし) は無かった。clippy に `await_holding_lock`・`significant_drop_in_scrutinee`・`significant_drop_tightening`・`let_underscore_lock` を足しても警告は 0 件。直したもの:
  - stdin が閉じても、クライアントが stdout を読まないと、rmcp が stdout を閉じるのを上限なしで待ってサーバーが終わらなかった (再現: 60 秒以上)。stdin の終わりも `StopChildrenAtEnd` が `Notify` で知らせ、シグナルと同じく `STOP_LIMIT` の計時を始める (直した後は 11.0 秒で終わる)
  - `search_items` の std の用意も呼び出しの期限で打ち切る
  - `test_process::finish` は 5 分で諦める (サーバーを相手にするテストの `child.wait()` は、サーバーが stdin の終わりから 10 秒ほどで終わるので上限が付いた)
  - `Resolver` の入力ファイルは普通のファイルだけ読む (FIFO で止まらない)
- 残したもの:
  - `get_item` の std で、用意に失敗した crate を照会がもう一度解析しうる (失敗のときだけ)。
  - `Cache::lock` は、プロセスの中の同じパスの待ちが 1 列になり、10 分 × 待つ数になりうる (前からある)。
  - ビルドの `doc:`・`target:` のロックは上限なし (決めたとおり)。
  - ログのキューやあふれ、stderr を読まないクライアントでは、ログを書くスレッドが待つ (決めたとおり、前と同じ)。
  - 呼び出しの進捗の通知 (`notify_progress`) は stdout が詰まると待つ (Claude Code は終了のときに SIGTERM を送るので、`STOP_LIMIT` で終わる)。
  - テストの `generate-lockfile` などの短い std の `status()`・`output()` には上限が無い。`enter_as_task` の起こし・`archive_from` の取り消し・ログのあふれ・2 回目のシグナルの結合テストは無い

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
   - R-ARCH-6・R-WS-6・§9 の設定の表 (C3a): `limits.request_timeout_secs` (既定 30 秒) を設定ファイルと環境変数で読むこと。呼び出しが裏の仕事を待つのはその 3 分の 2 までで、残りは版の解決など仕事の前後に使うこと。期限は呼び出しの始めから数えること。M1 の横断の検索の 20 秒も、この 3 分の 2 から出すようにしたこと
   - R-OUT-9 (C3a): 1 つの crate の `status: building` の応答の項目と並び (`provenance`・`status`・`building_reason`・`retry_after_secs`・`warnings`・`hints`)、`retry_after_secs` の値 (10 秒)、path が他の crate に続くときは続く先の crate の `status: building` を返すこと。outputSchema では、1 つの crate を引くツールの必須を `provenance` だけにしたこと (R-OUT-1 のルートに `anyOf` を置かない決まりから)。`building_reason` の 5 つの値は、いま使わないものも宣言したこと
   - R-WS-6 (C3a・C3b): 取得と索引の作成は呼び出しの間で共有する 1 つの仕事で、待つ側が `retry_after_secs` を返したら切り離すこと (以後は取り消しで止めない)。ロックは blocking の作業が終わるまで持つこと。進捗の通知の間隔 (3 秒)、`progress` の値 (経過秒数を 3 ずつ。`total` なし)、`message` の形 (`fetching serde 1.0.228 (3 s)`、仕事が複数なら件数)、送るのは `tools/call` の入口の 1 か所であること。`sleep` は自分で送ること
   - R-QA-5 (C3a): テスト用の口 `CHATORA_RUSTDOC_TEST_DOCS_RS_URL` (debug ビルドで `CHATORA_RUSTDOC_TEST_TOOLS=1` のときだけ効く。設定ではない)
   - R-OUT-9 (C3c): 横断の検索で結果が 0 件で読み込み中の crate があれば、最上位の `status: building` (`partial` と `data`・`page` も返す)。版が決まる前 (std の nightly、`cargo metadata` の解決) の `status: building` の provenance と、outputSchema の provenance の必須を `crate`・`target` だけにしたこと。`building_reason: waiting_for_lock` を `cargo metadata` のロック待ちに使うこと。`resolve_version`・`list_crates` も `status: building` を返すこと
   - R-OUT-3 (C3c-1): ビルド中のカーソルは、検索した crate のダイジェストに束縛したままで、読み込み中の crate が揃うと拒まれる。次のページがあるときは hints で案内する (利用者が決めた。「ビルド中の扱いは M2」の文を改める)
   - R-WS-6 (C3c-2・C3c-3): std の取得と `cargo metadata` も期限付きの共有の仕事にしたこと。std は std・alloc・core のアーカイブを先に作ること (利用者が決めた)。feature の組の `cargo tree` と re-export の先の版を決める経路は期限の外のままであること
   - R-ERR-3 (C3c-4): 一時的な失敗を項目ごとのファイル (`<パス>.miss.json`) に 30 秒覚えること (sparse index・`.crate`・配布物)。覚えている間は取りに行かず `retry_after_secs` 付きの `network_error`、sparse index は古いキャッシュを待たずに使う
   - R-OBS-1 (C3c-1): 応答が `status: building` の呼び出しはログに `status` を書き、`cache` を書かないこと
   - R-WS-6・R-CA-4・R-DIST-3・Q-29 (C4。利用者が決めた): 残った子孫の記録・作業用ディレクトリ・使えない target dir の印をキャッシュの下 (`procs/v<版>/`・`work/<実体 ID>/`) に置くこと (「state に置く」を改める)。持ち主の生死は、サーバーが生きている間持つ `owner:<実体 ID>` のロックで判定すること (案の PID と起動時刻をやめる。このロックファイルだけは、持ち主の止まった後に掃除が消す)。子孫は、継承させるロックで残っていることを確かめ、セッション ID と起動時刻 (マシンと起動の識別が同じもの) で誰かを確かめて止めること。Windows は記録しないこと (Job Object が止める)。確かでなければ止めず記録を残すこと。起動時の片付けは裏で 5 秒までで、残りは次の起動に回すこと (「判定の上限」)。他の版の記録は持ち主が止まって 7 日で消すこと (「読めない記録には触らない」を改める)。使えない target dir の名前と伝え方は D2
   - §8: `local_build_required` の扱い (M2 で使われなくなるか)、ビルドの状態の値 (R-OUT-9)
   - R-SC-4 の `readme` の扱いを M3 に送る
   - §12 の M2 の到達点と、M3 以降に回したもの
3. README を合わせる (F6)。公開は M3 の後にまとめて行う (P9。利用者が行う)
4. M3 の計画を立てる
