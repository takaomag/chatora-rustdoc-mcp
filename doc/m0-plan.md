# M0 (PoC) の計画

- 作成日: 2026-10-19
- 状態: 計画。進めながら §6 に結果を書き足す
- 前提: [requirements.md](requirements.md) v0.16。M0 の内容と完了条件は §12、M0 で決める未決事項は §11.2

---

## 1. 目的と分け方

M0 は、要件の文章では決めきれなかったことを、動くもので確かめる段階。§12 の M0 には性質の違う 2 つの作業が入っているので、分けて進める。

| 段 | 内容 | 性質 | コードの置き場所 |
| --- | --- | --- | --- |
| **M0a** | stdio の MCP サーバーと `get_item` 1 本 (docs.rs の JSON → 形式の正規化 → 公開パス索引 → 描画) | M1 の土台になる一本道。前提が崩れると §7 の出力設計と §10 の SDK の選択が連鎖して変わるものを、先に確かめる | `src/` (M1 に持ち越す) |
| **M0b** | ローカルビルドの実験 (cargo と rustdoc の挙動を測る) | 使い捨ての実験。結果を文書に残し、未決事項 (Q-11・Q-12・Q-17・Q-20・Q-30 など) を決める | `poc/m0b/` (スクリプトと小さな workspace) |

進める順: M0a を先に進める。M0a の中では、最も崩れたときの影響が大きい R-OUT-1 を、仮のツールで早めに確かめる (A3)。M0b は M0a と独立に進められるが、実際の workspace での測定 (B1・B4) には、M0a で依存を足したこのリポジトリ自身を使うので、A1 の後に行う。

## 2. 方針

- **M0a のコードは捨てる前提にしない**。層 (R-ARCH-1) と既存の lint を守って `src/` に書き、M1 で育てる。M1 で置き換える前提の仮の部分 (§3.3 の D2) には、コメントで印を付ける
- **M0b のスクリプトと実験用の workspace は `poc/m0b/` に置いてコミットする** (一時ディレクトリはシャットダウンで消え、実験をやり直せなくなるため)。`Cargo.toml` の `include` に入らないので、crate には同梱されない。ビルドの生成物は `.gitignore` で除く
- **測った結果は、日付と環境 (rustc・cargo・Claude Code の版) を付けて §6 に書く**。要件への反映は M0 の終わりにまとめて行う (§7)。それ以前の実測 (要件に「2026-10-1x に確認」とあるもの) と食い違ったら、§6 にその旨を書く
- **ホストの環境を変えない**
  - すでに入っている toolchain と target で進める。足りないもの (toolchain・コンポーネント・ツール) が出てきたら、入れる前に利用者に確かめる
  - 実験で起動する cargo と rustup には `RUSTUP_AUTO_INSTALL=0` を付ける (`cargo +nightly` や `rust-toolchain.toml` による toolchain の自動導入を防ぐ)
  - git を取得する実験では、利用者の git と cargo の設定 (`url.<base>.insteadOf` など) が効かないよう、`HOME`・`XDG_CONFIG_HOME`・`CARGO_HOME` を実験用のディレクトリに向ける
  - Claude Code での確認は、実験用のディレクトリに置いた `.mcp.json` と `.claude/settings.json` でサーバーを登録して行う (利用者の設定に登録しない)。モデルに渡った `tool_result` は、headless (`claude -p --output-format stream-json --verbose`) の出力で見る ([claude-code-issues.md](claude-code-issues.md) の「再確認のしかた」と同じ)
- **範囲の外** (M1 以降): バージョン解決 (§5.3。M0a は `crate@x.y.z` の完全な版だけを受ける)、target の選び方 (R-TG-1。M0a は docs.rs の既定の target の JSON を取り、実際の triple を provenance に出すだけ)、rkyv のアーカイブと索引のキャッシュ (R-PF-1)、std、検索、`list_*`、intra-doc link の書き換え (R-OUT-5)、crate をまたぐ re-export の解決 (R-PR-3 の手順 3 以降)、プロセスをまたぐロック

---

## 3. M0a: サーバーと `get_item`

### 3.1 手順

| # | やること | 確かめること (要件) | 終わりの印 |
| --- | --- | --- | --- |
| A1 | 依存を足し、層ごとのモジュールの枠 (データ層・クエリ層・MCP) を作る。`serve` サブコマンドで stdio サーバーを起動する | 依存の版は ferritin と `cargo info` で確かめて決める。§10 の推しは rmcp ≥ 3.5.1、tokio、reqwest、zstd、serde_json (`unbounded_depth`) + serde_stacker、rustdoc-types 0.61。docs.rs の zstd は `Content-Encoding` ではなく本文として届くので、reqwest の `zstd` feature (自動の展開) は使わず、zstd crate で展開する (R-DS-4) | `cargo clippy --all-targets` と `cargo test` が通る |
| A2 | 固定の値を返す仮のツール 1 つで、MCP の骨組みを作る (`server/discover`・`tools/list`・`tools/call`)。ツールは `ServerHandler` の `list_tools` と `call_tool` を手で実装する (D1) | R-MCP-14 (discover の中身)、R-MCP-11 (即答)、R-MCP-2 (`initialize` に `-32022` と独自の `message`、必須の `_meta` を欠く要求に `-32602`、`ping` に `-32601`)、R-MCP-5 (`ttlMs`・`cacheScope`)、R-MCP-16 (`subscriptions/listen` の ack と、サーバーから閉じるときの SDK の挙動)、R-ARG-1 (引数を JSON のオブジェクトで受けて自前で検証する)、R-ERR-1 (ハンドラの panic を `isError` の結果にできるか)、R-SEC-1 (stdout の各行が 1 つの JSON-RPC のメッセージか)。あわせて、進捗通知とキャンセル (R-WS-6) を rmcp で配線できるかを確かめる (実際に使うのは M2 だが、§10 で mcplease を退けた理由なので、ここで裏を取る) | 生の JSON-RPC を流す結合テスト (`tests/`) が通る。rmcp で満たせない点があれば、回避策か tower-mcp への切り替えを決める |
| A3 | Claude Code から仮のツールを呼ぶ (1 回目) | R-OUT-1 (`outputSchema` を宣言したとき、`structuredContent` だけがモデルに渡るか)、R-ERR-2 (エラーで `content[0].text` だけが渡るか)、2026-07-28 で接続されるか (従来の handshake に戻っていないか)、進捗通知が表示されるか、Esc で `notifications/cancelled` が届くか | 結果を §6 に書く。前提が崩れていたら、A4 に進む前に §7 の出力設計を見直す |
| A4 | docs.rs から JSON を取る。`/crate/{name}/{version}/json` を取り、先頭が zstd のマジックナンバーである限り展開を繰り返す (上限 3 段)。取った元の JSON をキャッシュの下に置き、2 回目からはそれを読む | R-DS-2 の手順 1、R-DS-4、R-DS-6 (404・400 の扱い)、R-NW-1 (連絡先付きの User-Agent)、R-NW-3 (上限付きのストリーム)。Q-30 の「通信の途中で切れた本体が、zstd の展開の失敗として先に出るか」 | 形式ごとのテストデータ (§3.2) を取れる |
| A5 | `format_version` の先読みを作る (R-FV-3) | 候補 (本文の末尾からのバイト走査 / `format_version` 以外を読み飛ばすストリーミングの解析) を、各形式のテストデータで、正しさと所要時間で比べる | 方式を決めて §6 に書く |
| A6 | 正規化: canonical (rustdoc-types 0.61) と typed shim を作る。M0a では形式 54〜61 を読む (shim は、§5.2 の表にある 57 の `ExternalCrate::path` と 61 の `Stability::level` の 2 つ。ほかに要るものが見つかれば足す)。46〜53 (`attrs` の文字列の解析。R-FV-2) は M1。ネストの深さの上限を外す (R-FV-8) | R-FV-1、R-FV-5 (canonical より新しい形式の扱い)、R-FV-7、R-FV-9 (網羅的な分解)。1 回のストリーミングの解析で済むか | 形式 54〜61 の実データを読める。300 段のネストのテストが通る |
| A7 | 公開パス索引を作る (crate の中だけ)。ルートから `Module.items` と `Use` (glob と非 glob) をたどり、§4 の規則で正準パスを選ぶ。`Type::method` は、親を引いてから impl の中をたどる (R-PR-1 のフォールバック) | R-PR-2、R-PR-3 の手順 1・2、循環の検出と深さの上限、`doc(hidden)`・`doc(inline)`。private module の中の公開型を正準パスで引けるか | §3.2 の代表の item を正準パスで引ける |
| A8 | `get_item` を作る。引数は `crate` (`crate@x.y.z`) と `path`。シグネチャを Rust のコード文字列に描画し (関数・メソッド・struct・enum・trait・型の別名・定数)、ドキュメントの第 1 段落と provenance を付ける (`detail=normal` に当たるもの)。エラーは `item_not_found`・`docs_unavailable`・`unsupported_format`・`invalid_argument` を返す | R-OUT-2 (描画と膨張率の実測。同じ item の rustdoc の部分木を compact な JSON にした長さと、応答の `structuredContent` の長さを比べる)、R-SH-1、R-SH-2、§7.3 の骨格、R-OUT-9 (provenance の値をデータ本体から読む)、R-ERR-2 | 代表の item の応答をスナップショット (insta) で固定する。膨張率を §6 に書く |
| A9 | Claude Code から `get_item` を呼ぶ (2 回目) | A3 と同じ点を実データで。エージェントが説明文だけで正しい引数を作れるかも軽く見る (eval は M5) | 結果を §6 に書く |
| A10 | rustdoc_processor を評価する | A7 と同じ crate で、公開パスの索引の結果を比べる (食い違えば、どちらの誤りかを調べる)。docs.rs の JSON を入力に渡せるか、依存の重さ (SQLite、cargo の起動)、ライセンス (Apache-2.0 のみ。依存にはできるが、コードは写せない。R-DIST-4)。依存にしない場合に、テストケースの比較相手として使えるか | 依存にするかを決めて §6 に書く |

### 3.2 テストデータと代表の item

- 形式ごとに docs.rs の JSON を 1 つずつ集め、`crate@版 → 形式` の対応を §6 に残す (R-QA-2 の 46・53・54・56・57・60・61。46 は tokio 1.45.1)。46〜53 は A5 の先読みにだけ使う。二重に zstd 圧縮されたものが見つかれば加える
- 大きな JSON はリポジトリに入れない。取得するスクリプトと sha256 の一覧をコミットし、データは git で無視するディレクトリに置く。小さな手作りのもの (300 段のネスト、glob の循環) は fixtures としてコミットする
- 代表の item (初期案): `tokio::sync::Mutex` と `tokio::sync::Mutex::lock` (private module の中の型とそのメソッド)、`axum::Json` (定義パスは `axum::json::Json`)、`axum::Router`、`serde::Deserialize` (trait)。std は M1

### 3.3 M0a の中で決めること

| # | 論点 | 推す案 | 理由 |
| --- | --- | --- | --- |
| D1 | ツールの登録を rmcp のマクロ (`#[tool_router]`・`#[tool]`) でするか、`ServerHandler` を手で実装するか | 手で実装する | マクロの経路では、どれもマクロの外で手当てが要る: `Parameters<T>` は逆直列化の失敗を `-32602` にする (R-ARG-1)、schemars は `format: uint64` を出す (R-MCP-10)、説明文は Markdown のファイルから読む (§6.2) |
| D2 | M0a の仮の部分 | キャッシュは元の JSON をファイルに置くだけにする (原子的な書き込みと派生物は M1。R-CA-4)。`outputSchema` は `get_item` の分だけ手で書く (骨格の確定は M1。R-OUT-9) | M1 で作り直す前提 |
| D3 | 引数の解析ライブラリ (clap など) | M0a は `serve` だけを手で解析し、`cache` を足す M2 で決める | 依存を増やさない |

---

## 4. M0b: ローカルビルドの実験

### 4.1 実験の対象

- **小さな合成の workspace**: 要件の実測 (2026-10-14〜16) と同じく、確かめたい挙動ごとに作る (resolver 1 と 2、target 別の依存、build-dependency、dev-dependency、proc-macro、lib と bin の crate 名が同じもの、build.rs が `RUSTC_BOOTSTRAP` を見る依存など)
- **実際の workspace**: M0a で依存を足したこのリポジトリ (proc-macro2 や syn を含む見込み)。依存の多い公開の workspace を 1 つ加えるかは、B1 の結果を見て決める

### 4.2 実験

| # | 問い | 方法 | 決まること |
| --- | --- | --- | --- |
| B1 | Q-12: stable + crate 名を指定した `RUSTC_BOOTSTRAP` を、ローカルビルドの既定にしてよいか | 実際の workspace で、値を固定したときと変えたときの作り直しの範囲と時間を測る。`RUSTC_BOOTSTRAP` を見る依存 (`rerun-if-env-changed=RUSTC_BOOTSTRAP` を出すもの) の一覧を作る。依存を文書化したとき、nightly 用の機能を自分の crate 名で調べる依存 (anyhow など) の文書が stable と違うかを比べる | 既定にするか、値を安定させる方式 (Q-12 の案)、値を R-WS-4 の鍵に入れるか |
| B2 | Q-17: `cargo doc --output-format json` と `cargo rustdoc` の違い | nightly で両方を同じ workspace に実行し、引数、fingerprint (2 回目に作り直されるか)、所要時間、同名で版の違う依存の扱い (unit のディレクトリの JSON)、crate を絞った `RUSTC_BOOTSTRAP` と両立しないことを比べる | どちらを使うか (R-WS-1) |
| B3 | Q-20・R-FT-3: どの feature の組でビルドするか | target 別の依存・build-dependency・dev-dependency で feature が分かれる合成の workspace で、`cargo tree --workspace --target <target> -e normal -f '{p} [{f}]'` の出力と、`cargo check --all-targets` が実際に使う組を比べる | ビルドに使う組の決め方 |
| B4 | Q-11: `--cfg docsrs` で壊れて、渡さずにやり直す頻度 | 実際の workspace の依存をすべて `--cfg docsrs` 付きで文書化し、失敗した crate と理由を数える (stable + `RUSTC_BOOTSTRAP` と nightly の両方で) | 頻度の記録 (壊れた crate を記録するかは M2 で決める) |
| B5 | R-WS-4: 文書化する crate の dep-info の取り方 | `--emit=dep-info` を付けて rustdoc をもう一度実行する方式と、その crate の `cargo check` の dep-info を使う方式で、所要時間と、`include_str!`・`#[path]`・build.rs が生成するコードが dep-info に載るかを比べる | どちらにするか |
| B6 | R-MCP-3: `cargo tree` の出力の形の安定性と、host 用か target 用かの判定 | `-e normal,dev` と `-e build` を分けた出力と、nightly の `--unit-graph` を比べる。package id を取れるか。stable と nightly の cargo で形が変わるか | 範囲の判定の方法 |
| B7 | Q-30 の各項目 | 下の表 | Q-30 の各項目 |

Q-30 の内訳:

| # | 項目 | 方法 |
| --- | --- | --- |
| B7-1 | `--target` を付けると `RUSTFLAGS` が build script と proc-macro に効かなくなることの影響 | host で `build.target` も無いときに、`--target` の有無で、`RUSTFLAGS` の効き方と依存の作り直しを比べる |
| B7-2 | `cargo tree` で依存の種類と package id を取れるか。`--workspace --target` を付けた `-e normal` の feature の組 | B3・B6 で兼ねる |
| B7-3 | `cargo metadata` の失敗から、Cargo.toml の誤りと通信の失敗を見分けられるか | Cargo.toml の誤り、届かないレジストリと git、オフラインのそれぞれで、終了コードと stderr を比べる |
| B7-4 | git の取得の決定的な失敗を判定する文言と、`LC_ALL=C` を渡すか | libgit2 と git の CLI (`net.git-fetch-with-cli`) で、rev・ブランチ・リポジトリが無いとき、認証に失敗したときの文言を集める (2026-10-16 の例を、`cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update -p` に広げる)。git の CLI のメッセージが地域化で変わるかも見る |
| B7-5 | 通信の途中で切れた本体が、zstd の展開の失敗として先に出るか | A4 で兼ねる |
| B7-6 | 他の target の `rust-std` が無いことを、sysroot の `lib/rustlib/<target>` で先に判定できるか | 入っていない target を指して、sysroot の中身と cargo の失敗を比べる (target を新たに入れずに確かめる) |
| B7-7 | 実行ファイルのパスを一部だけ指定したときの扱いと、rustup が無い環境での既定 | `RUSTC`・`RUSTDOC` を一部だけ渡したときの挙動を見る。PATH に toolchain の `bin` だけを置いて、rustup の無い環境を模す |
| B7-8 | target の JSON 仕様のパスを `build.target` に書いたプロジェクトの扱い | 合成の workspace で、`cargo tree` と `cargo rustdoc` の挙動を見る |
| B7-9 | resolver 1 と 2 で、`-p` にどの依存を名指しすると cargo が失敗または panic するか | dev-dependency だけ・build-dependency だけ・他の target でだけ使う・workspace の外の依存を、resolver 1 と 2 で `cargo rustdoc -p` に渡す。R-WS-5 と R-MCP-3 の記述の食い違いを解く |

### 4.3 進め方

- 実験は `poc/m0b/` の下の作業用のディレクトリで行い、利用者の `target/` を使わない (専用の `CARGO_TARGET_DIR` と `CARGO_BUILD_BUILD_DIR`。R-WS-2)
- 実験ごとに、再現の手順 (スクリプト) と結果 (表) を残す。所要時間は、コールドとウォームを分けて測る

---

## 5. M0 の完了条件との対応

§12 の M0 の完了条件と、それを確かめる手順の対応。

| §12 の完了条件 | 手順 |
| --- | --- |
| Claude Code から呼べる | A3・A9 |
| R-OUT-1 の挙動を再確認 | A3・A9 |
| Q-12 | B1 |
| Q-17 | B2 |
| Q-20 | B3 |
| R-FV-3 の先読みの方法 | A5 |
| R-WS-4 の dep-info の取り方 | B5 |
| Q-11 の、渡さずにやり直す頻度 | B4 |
| R-MCP-16 のサーバーから閉じるときの扱い | A2 |
| R-OUT-2 の膨張率 | A8 |
| R-MCP-3 の `cargo tree` の出力の形の安定性 | B6 |
| Q-30 の cargo の挙動 | B7 |
| rustdoc_processor を依存にするか (§10) | A10 |

---

## 6. 結果

進めながら書き足す。手順ごとに、日付、環境 (rustc・cargo・Claude Code の版)、結果、決めたことを書く。

### 6.1 M0a

#### A1・A2 (2026-10-19)

環境: rustc・cargo 1.99.1 (stable)、rmcp 3.5.1、tokio 1.53.3。Claude Code からはまだ呼んでいない (A3)。挙動は、生の JSON-RPC を流す結合テスト [tests/stdio.rs](../tests/stdio.rs) と、`src/mcp/mod.rs` の単体テストで固定した。

**決めたこと** (§3.3 の D1・D3 を含む)

- D1: ツールは rmcp のマクロを使わず、`ServerHandler` の `list_tools`・`call_tool` を手で実装した。D3: `serve` の引数は手で解析した。D2 (仮のキャッシュと `outputSchema`) は A4 以降
- バイナリ名を `chatora-rustdoc-mcp` (`src/bin/chatora-rustdoc-mcp.rs`) にした。パッケージ名と lib の crate 名は `chatora-rustdoc` のまま。serverInfo の `name` もバイナリ名にそろえた (要件の §14 の v0.17 (作成中) に記録済み)
- lib は層 (`data`・`query`・`mcp`) と、層をまたぐ `logging` に分けた。tokio の runtime はバイナリで作り、lib の `mcp::serve` は async にした
- `mcp::serve` のエラーは thiserror の `ServeError` で返す。セッションのタスクの panic はエラーにせず、`resume_unwind` で投げ直す (不具合を終了コード 101 で伝えるため)。ツール 1 回分の panic は、逆に `internal_error` の結果に変える (R-ERR-1)
- ログは tracing で stderr に出す。絞り込みは環境変数 `CHATORA_RUSTDOC_LOG__FILTER` (設定キー `log.filter` に当たる。既定 `info`、誤った指定は無視する)
- `initialize` の拒否の `message` は const_format で組み立てる。const_format とその依存 3 つのライセンスは Zlib で、M5 の cargo-deny (R-DIST-4) で許可の一覧に入れる必要がある
- 仮のツールは `echo` (配線の確認) と `sleep` (進捗通知とキャンセルの確認。A3 でも使う) の 2 つ。どちらも A8 で消す
- エラー本文の `request_id` (R-ERR-2) は、ログと対応付ける R-OBS-1 と一緒に M1 で入れる

**確かめたこと**

| 要件 | 結果 |
| --- | --- |
| R-MCP-14・R-MCP-5 `server/discover` | `supportedVersions: ["2026-07-28"]`、`capabilities: {"tools":{}}`、`ttlMs: 3600000`、`cacheScope: "public"`、serverInfo を返す。rmcp の既定 (`ttlMs: 0`・`private`) は `discover` の上書きで直した |
| R-MCP-5 `tools/list` | 同じキャッシュの指定を付けた。`cursor` 付きの要求は `-32602`。ツールは決まった順で返す |
| R-MCP-2 `initialize` | `-32022`、独自の `message` (対応する版と Claude Code v2.1.292 以降が要ること)、`data` の `requested`・`supported`。rmcp はその後セッションを終え、プロセスは終了コード 1 で終わる (受け入れる。下記) |
| R-MCP-2 その他 | `_meta` を欠く要求は `-32602`。セッション開始後の `ping` と `logging/setLevel` は `-32601`。どれも rmcp の既定のままで要件どおり |
| R-ARG-1・R-ERR-2・R-OUT-1 | 知らないツール名だけが `-32602`。引数の誤りは `isError: true` のツールの結果で、`content[0].text` が自己完結した JSON (`code`・`message`・`retryable`・`suggestions`)。`structuredContent` は付けない。成功は `CallToolResult::structured` で `structuredContent` と同じ JSON の text |
| R-ERR-1 | rmcp は要求ごとのタスクの panic で応答を送らない (`service.rs:1603`)。ツールを別のタスクで動かして `JoinError` を `internal_error` の結果に変えた |
| R-WS-6 | token があるときだけ `notifications/progress` を送れる。`notifications/cancelled` (`_meta` の有無によらない) で `context.ct` が cancel され、rmcp はその要求の応答を捨てる。キャンセル後も他の要求には応答する |
| R-MCP-16 `subscriptions/listen` | rmcp は `accepted_subscription_filter` が `None` (既定) だと `-32601` を返す。空のフィルターを返すようにし、ack (`subscriptionId` は要求の id、`notifications` は空) を返して保持するようにした。stdio でサーバーが終わるのはクライアントが stdin を閉じたときかシグナルだけなので、「つながったままサーバーから閉じる」場面は v1 では起きない (仕様のページ間の食い違いは M6 の HTTP で扱う) |
| R-SEC-1 | stdout の各行が単独の JSON-RPC のメッセージであることを、結合テストのすべての出力で確かめている |

**rmcp の挙動で、要件と違うが受け入れるもの** (ハンドラからは変えられない。`tests/stdio.rs` の `rmcp_deviations_from_the_requirements` で固定し、rmcp が変われば気付けるようにした)

- セッション開始前の `ping` (`_meta` なし) に空の結果を返す (R-MCP-2 は `-32601`)。rmcp の受信ループが直接答えている
- `arguments` がオブジェクトでない `tools/call` に `-32601` を返す (R-ARG-1 は `-32602`)。要求の逆直列化の失敗が method not found になる
- `initialize` を拒んだ後にプロセスが終わる。古いクライアントは拒まれた後に続けられないので、実害は無い
- `_meta` で対応外の版を名乗った要求への `-32022` の `message` は rmcp の固定の文言 (R-MCP-2 が独自の `message` を求めるのは `initialize` だけ)

**ほかに分かったこと**

- 応答は要求の順に返るとは限らない (rmcp が要求ごとのタスクで処理するため)。テストは id で対応付ける
- rmcp は受けた通知を INFO で中身ごとログに出す。量が問題になれば、ログの既定を `info,rmcp=warn` などにする
- stdin の EOF を受けると、rmcp は処理中の要求の応答を最大 5 秒待ってから終わる (`service.rs:1762`。listen を保持したまま、または `sleep 30` の最中に閉じると 5.2 秒、listen をキャンセルしてから閉じると 0.4 秒)

**SDK の比較** (A2 の途中で、rmcp の手当てが多いので代替を調べた。scratchpad で最小の stdio サーバーを作り、同じ JSON-RPC を流した)

| 観点 | rmcp 3.5.1 | tower-mcp 0.24.0 | rust-mcp-sdk 2.0.0 | pmcp 2.22.7 |
| --- | --- | --- | --- | --- |
| stdio で 2026-07-28 | ○ | ○ | ○ | × (`server/discover` でプロセスが終わる) |
| ツールの panic | 応答なし (自前で包む) | `.catch_panics()` で `isError` | 応答なし | 応答なし。以後の要求にも応答しない |
| キャンセル後に応答しない (R-WS-6 の MUST) | ○ | ○ | × | × |
| `initialize` の `message` | 上書きできる | 固定 | 上書きできる | 固定 (`-32602`) |
| `server/discover` のキャッシュの指定 | 設定できる | 0・private に固定 | 設定できる | — |
| `_meta` を欠く要求 | `-32602` | `-32602` | `-32601` | `-32602` |
| 文字列の `arguments` | `-32601` | ハンドラに届く | `-32601` | ハンドラに届く |
| 規模 (2026-10-19) | 約 3,700 万 DL。公式の SDK | 約 3.5 万 DL。7 か月で破壊的な minor が 16 回 | 約 32 万 DL。最終リリースは 53 日前 | 約 13 万 DL |

turbomcp (alpha 版) と turul-mcp-server (約 2,500 DL) は比べる前に外した。結論として rmcp を続ける。tower-mcp は panic の仕組みと引数の扱いが良いが、R-MCP-5・R-MCP-2 を満たせず、更新のたびに互換性が壊れる頻度が高い。rust-mcp-sdk はキャンセル後の MUST を満たせない。

**M1・M2 への申し送り**

- M1: `request_id` (R-ERR-2・R-OBS-1)。`outputSchema` と応答の骨格の確定 (R-OUT-9。D2)
- M2: stdin の EOF でローカルビルドを止める処理を自前で持つ (rmcp の 5 秒の待ちの間も子の cargo が動き続けるため。R-WS-6)。シグナル (SIGINT・SIGTERM) の扱いも同時に決める

**v0.17 で要件に反映すること**

- R-DIST-3 の設定の表に `log.filter` を足す
- §10 の SDK の行に、上の比較を根拠として足す (推しは rmcp のまま)
- R-MCP-2・R-ARG-1 に、rmcp の挙動で受け入れる違いを書く
- R-MCP-16 に、stdio では「サーバーから閉じる」場面が起きないことを書く

### 6.2 M0b

(未着手)

---

## 7. M0 の終え方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を v0.17 にする。決まった未決事項を §11.1 に移し、本文 (R-FV-3・R-WS-1・R-WS-4・R-WS-8・R-FT-3・R-MCP-3・R-MCP-16・R-OUT-2・§10 など) を直し、§14 に経緯を書く。先送りしていた v0.15 以降の全体の再レビューは、このときに合わせて行う
3. M1 の計画を立てる
