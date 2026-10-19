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

環境: rustc・cargo 1.99.1 (stable)、rmcp 3.5.1、tokio 1.53.3。Claude Code からはまだ呼んでいない (A3)。挙動は、生の JSON-RPC を流す結合テスト [tests/stdio.rs](../tests/stdio.rs) と、`src/mcp/tool/mod.rs` の単体テスト (当時は `src/mcp/mod.rs`)で固定した。

**決めたこと** (§3.3 の D1・D3 を含む)

- D1: ツールは rmcp のマクロを使わず、`ServerHandler` の `list_tools`・`call_tool` を手で実装した。D3: `serve` の引数は手で解析した。D2 (仮のキャッシュと `outputSchema`) は A4 以降
- バイナリ名を `chatora-rustdoc-mcp` (`src/bin/chatora-rustdoc-mcp.rs`) にし、serverInfo の `name` もそろえた。その後、A4 の前に、パッケージ・lib の crate・環境変数の接頭辞・ディレクトリ名なども含めて、名前をすべて `chatora-rustdoc-mcp` にそろえた (要件の §14 の v0.17 (作成中) に記録済み)
- lib は層 (`data`・`query`・`mcp`) と、層をまたぐ `logging` に分けた。tokio の runtime はバイナリで作り、lib の `mcp::serve` は async にした
- `mcp::serve` のエラーは thiserror の `ServeError` で返す。セッションのタスクの panic はエラーにせず、`resume_unwind` で投げ直す (不具合を終了コード 101 で伝えるため)。ツール 1 回分の panic は、逆に `internal_error` の結果に変える (R-ERR-1)
- ログは tracing で stderr に出す。絞り込みは環境変数 `CHATORA_RUSTDOC_MCP_LOG__FILTER` (設定キー `log.filter` に当たる。既定 `info`、誤った指定は無視する)
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

#### A3 (2026-10-19)

環境: Claude Code 2.1.303 (Linux)、モデルは Opus 5.5。実験用のディレクトリ [poc/m0a-a3/](../poc/m0a-a3/) (手順は同じ場所の README) で、サーバーを `.mcp.json` で登録し、やりとりを `run-server.sh` で `logs/` に写した。R-OUT-1 と R-ERR-2 を見分けるため、`structuredContent` と text に違う JSON を返す仮のツール `probe_result` を足した (A8 で消す)。headless の 3 回で約 0.48 ドル。

| 確かめたこと | 結果 |
| --- | --- |
| R-OUT-1 (headless) | 成功の結果では、モデルに渡ったのは `structuredContent` (`{"from":"structuredContent"}`) だけで、text は捨てられた。前提は変わっていない |
| R-ERR-2 (headless) | `isError: true` の結果では `structuredContent` が無視され、`content[0].text` (`{"from":"text"}`) だけが `is_error: true` で渡った。前提は変わっていない |
| 引数の誤り (headless) | `echo` を `{}` で呼ぶと、`invalid_argument` の JSON がそのまま `is_error: true` で渡り、モデルは内容を読めた |
| R-MCP-2 接続 | `server/discover` (id は文字列の `server-discover-probe-1`) → `tools/list` → `tools/call` の順。どれも `_meta` で 2026-07-28 を名乗り、`clientInfo` も付いていた。`initialize` は送られなかった |
| R-MCP-16 | `subscriptions/listen` は送られなかった (headless でも対話モードでも) |
| `tools/call` の `_meta` | 毎回 `progressToken` (対話モードでは数値の `1`) と `claudecode/toolUseId` が付いていた |
| ツールの読み込み | 遅延読み込み。モデルは ToolSearch で `select:mcp__chatora-rustdoc__…` を引いてから呼んだ |
| R-WS-6 進捗 (対話モード) | `sleep` 20 秒の間、`notifications/progress` を 1 秒ごとに送り、画面に `slept 5 of 20 seconds (25%)` と出た。割合は Claude Code が `progress / total` から計算している |
| R-WS-6 キャンセル (対話モード) | Esc で `notifications/cancelled` (`reason: "AbortError: user-cancel"`。この通知にも `_meta` が付く) が届き、サーバーはその場で `sleep` を止めた。進捗は 6 回目で止まり、結果は返さなかった。画面は `Interrupted · What should Claude do instead?` |

前提が崩れた項目は無いので、§7 の出力設計は今のままで A4 に進む。

#### A4 (2026-10-19)

環境: rustc・cargo 1.99.1、reqwest 0.13.5 (rustls + aws-lc-rs。cmake なしでビルドできた)、zstd 0.14.3。A4 の前に、名前をすべて `chatora-rustdoc-mcp` にそろえた (要件の §14 の v0.17 (作成中))。

**作ったもの**

- A4-1 `src/data/docs_rs.rs`: `/crate/{name}/{version}/json` の取得。User-Agent は `chatora-rustdoc-mcp/0.1.0 (+https://github.com/takaomag/chatora-rustdoc-mcp)` (R-NW-1)。接続 10 秒・無通信 120 秒のタイムアウト (R-NW-2)。200・404・400・429 と 5xx・その他を分ける。本体は 512 MiB まで (R-NW-3)。zstd はマジックナンバーで 3 段まで展開し、各段にも上限を付ける (R-DS-4)。crate 名は `src/data/crate_name.rs` の `CrateName` で検証する (R-SEC-4)
- A4-2 `src/data/cache.rs`: 置き場所は `CHATORA_RUSTDOC_MCP_CACHE__DIR` (絶対パスだけ。空なら未設定と同じ) か OS の既定 (R-CA-1)。仮のパス `docs.rs/{crate}/{version}/rustdoc.json` に、一時ファイル・fsync・rename で書く (R-CA-4)。symlink はたどらない。`data::docs_rs_json` がキャッシュと取得をつなぐ
- A4-3 `examples/fetch_docs_rs.rs`: 取得してキャッシュに置き、形式・大きさ・sha256・取得元を出す。`format_version` は A5 までの仮の読み方 (serde_json で全体を走査する)

**docs.rs の挙動** (2026-10-19 に実測): 完全な版の JSON は 200 で `content-type: application/zstd`、`content-length` と `etag` が付く。JSON の無い版 (serde 1.0.100) と存在しない crate はどちらも 404。版として解析できない値は 400。`/crate/serde/1.0/json` は 302 で完全な版へ。

**Q-30 の「通信の途中で切れた本体」**: `content-length` より短く切れた本体は、zstd の展開に届く前に、HTTP の層 (hyper) が本体のエラーにする (reqwest の `Network` のエラー)。手元の HTTP サーバーを使う単体テストで固定した。docs.rs は `content-length` を付けるので、一時的な通信の失敗と壊れた zstd は取り違えない。

**キャッシュ**: 同じ版を 2 回取ると、2 回目はキャッシュから読む (serde と tokio の 2 件で、1 回目 533 ms、2 回目 291 ms。2 回目は主に serde_json の全体の走査の時間)。

**形式ごとのテストデータ** (§3.2。一覧と sha256 は [testdata/docs-rs-json.txt](../testdata/docs-rs-json.txt))

| 形式 | crate@版 | 備考 |
| --- | --- | --- |
| 46 | tokio@1.45.1、clap@4.5.39 | tokio 1.45.1 は **zstd で 2 重に圧縮されていた** (R-DS-4 と R-QA-2 の実例) |
| 48 | syn@2.0.104 | clap には形式 48 の版が無い |
| 53 | clap@4.5.41 | tokio 1.46.0・1.46.1 も 53 |
| 54 | clap@4.5.42 | serde_json 1.0.141、tokio 1.47.0 も 54 |
| 55 | clap@4.5.46 | |
| 56 | clap@4.5.48 | 4.5.51・4.5.53 も 56 |
| 57 | clap@4.5.56、serde@1.0.228 | 4.5.60 も 57 |
| 60 | clap@4.6.1 | 4.6.5 も 60 |
| 61 | clap@4.6.7 | |

R-QA-2 の 46・53・54・56・57・60・61 はすべてそろった。49〜52・58・59 の形式は、今回取った版には無かった。版の形式は、docs.rs がその版をビルドした時期の nightly で決まる (clap 4.5.40 は 2025-06-09 の公開で 46、4.5.41 は 2025-07-09 の公開で 53)。

**決めたこと**

- データ層の同期のディスク I/O は `spawn_blocking` で回す。タスクの panic は `src/task.rs` の `resume_if_panic` で投げ直す (`mcp::serve` と共通。ツールの中なら `run_tool` が `internal_error` にする)
- 例とテストが使う dev-dependency (`serde`・`sha2`・`tempfile`) は、lib と bin のテストのビルドにも見えるので、`#[cfg(test)] use … as _;` で並べる

#### A5 (2026-10-19)

**R-FV-3 の先読みの方法を決めた**: 末尾からの走査を主にし、読めなければ全体の走査に落とす。全体を解析する A6 で、本文の値と照合する。

- テストデータ 11 件 (形式 46〜61) は、どれも 1 行の compact な JSON で末尾に改行が無く、トップレベルのキーは `root, crate_version, includes_private, index, paths, external_crates, target, format_version` の順だった。`format_version` は常に最後のメンバー
- `src/data/format_version.rs`
  - `peek`: 末尾から `}`・数字・`:`・`"format_version"` を読み、その手前がトップレベルの区切り (`,` か `{`) であることを確かめる。空白と改行を許す。入れ子のオブジェクトや文字列の中の `format_version` は読まない (最後の `}` の手前が数字にならない)
  - `scan`: serde で `format_version` だけを持つ構造体に逆直列化する。serde_json の既定の再帰の上限のまま (深い入れ子への備え (R-FV-8) は A6)
  - `read`: `peek` で読めなければ warn を出して `scan` に落とす
- テストデータを使うテスト `tests/testdata.rs` (`#[ignore]`。`cargo test --release --test testdata -- --ignored --nocapture` で回す) で、11 件すべてについて、sha256 の照合と、`peek` と `scan` が一覧と同じ形式を返すことを確かめた

時間 (release ビルド。`peek` は大きさによらない):

| JSON | 大きさ | `peek` | `scan` |
| --- | --- | --- | --- |
| syn@2.0.104 (形式 48) | 9.6 MB | 0.3 µs | 8.7 ms |
| tokio@1.45.1 (形式 46) | 6.1 MB | 0.6 µs | 5.9 ms |
| serde@1.0.228 (形式 57) | 3.2 MB | 0.2 µs | 3.1 ms |
| clap@4.6.7 (形式 61) | 0.3 MB | 0.1 µs | 0.24 ms |

`scan` は約 1 GB/s で、core (約 61 MB) なら約 60 ms、windows-rs (約 1 GB) なら約 1 秒かかる見込み。`peek` なら、ファイルから読むときも末尾の数十バイトを seek して読めば足りる (R-PF-1 の mmap とも相性がよい)。

**v0.17 で要件に反映すること**: R-FV-3 の案を「決定」にし、上の方式 (末尾からの走査 → 全体の走査 → 全体の解析での照合) を書く。

#### A6 (2026-10-19)

環境: rustdoc-types 0.61.0 (形式 61)、serde_json 1.0.154 (`unbounded_depth`)、serde_stacker 0.1.14。

**形式 54〜61 の違い** (rustdoc-types 0.54・0.56・0.57.4・0.60・0.61 の定義を見比べた。0.61 の更新履歴は「TODO」のまま):

| 形式 | 変更 | 0.61 の型で古い形式を読むと |
| --- | --- | --- |
| 55 | `Attribute::MacroExport` を追加 (それまでは `Other("#[macro_export]")`)。**更新履歴は `MacroUse` と書いているが、型は `MacroExport`** | 読めるが、54 では意味が変わる |
| 56 | `ItemKind::Attribute` を追加 | 影響なし |
| 57 | `ExternalCrate::path: PathBuf` を追加 (必須) | 54〜56 は読めない |
| 58・59 | `Item::stability`・`const_stability` (`Option`) を追加 | 影響なし |
| 60 | `default_unstable` (`Option`) を追加 | 影響なし |
| 61 | `Stability::level` を、`level` の値で区別して平らに並べる形から、入れ子の形に変更 | 58〜60 で `stability` に値があると読めない |

**作ったもの** (`src/data/rustdoc_json/`)

- `parse`: 先読みした形式で型を選び、serde_json で 1 回のストリーミングの解析をする (`Value` を経由しない)。57 と 61 以降は 0.61 の型で直接、54〜56 と 58〜60 は shim。53 以下は `UnsupportedFormat` (M1)。62 以上は 0.61 の型で読んでみて、読めれば `newer_than_supported` を立てて warn を出し、読めなければ `UnsupportedFormat` (R-FV-5)。解析した `format_version` が先読みと違えば `FormatMismatch`
- `v54_to_56.rs`: `ExternalCrate` を `path` なしで受け、空の `PathBuf` (「不明」) を入れる。54 だけ `Other("#[macro_export]")` を `MacroExport` に写す
- `v58_to_60.rs`: `Item` の `stability`・`const_stability` を古い形の `Stability` で受けて写す
- shim から 0.61 の型への変換は、`..` を使わずにフィールドを全部分解する (R-FV-9)
- 再帰の上限を外し、serde_stacker でスタックを伸ばす (R-FV-8)。300 段の入れ子の型を、単体テストで読めることを確かめた

**テストデータでの結果** (release ビルド。`tests/testdata.rs` の `#[ignore]` のテスト):

| JSON | 形式 | 大きさ | item | paths | 解析 |
| --- | --- | --- | --- | --- | --- |
| clap@4.5.42 | 54 | 0.43 MB | 22 | 2,768 | 4.7 ms |
| clap@4.5.46 | 55 | 0.43 MB | 22 | 2,772 | 3.2 ms |
| clap@4.5.48 | 56 | 0.44 MB | 23 | 2,780 | 2.8 ms |
| clap@4.5.56 | 57 | 0.44 MB | 23 | 2,693 | 3.1 ms |
| serde@1.0.228 | 57 | 3.2 MB | 2,916 | 2,644 | 23.3 ms |
| clap@4.6.1 | 60 | 0.44 MB | 23 | 2,727 | 2.7 ms |
| clap@4.6.7 | 61 | 0.30 MB | 23 | 1,065 | 1.4 ms |

- 形式 54〜61 はすべて読め、`crate_version` が一覧の版と一致し、`root` が `index` にあった。外部 crate の `path` は、56 以前ではすべて空、57 以降ではすべて空でなかった。46・48・53 は `UnsupportedFormat` になった
- 全体の解析は約 140 MB/s で、A5 の `scan` (約 1 GB/s) の約 7 倍遅い。core (約 61 MB) なら約 0.45 秒かかる見込みで、R-PF-1 の rkyv のアーカイブ (M1) で読み込みを速くする根拠になる
- 58〜60 の shim は、`stability` を持つ JSON (std など) がテストデータに無いので、単体テストの手作りの JSON でだけ確かめた。M1 で std の JSON を扱うときに実物で確かめる
- clap の item は 22〜23 個しか無い (中身は clap_builder の re-export)。crate をまたぐ re-export (R-PR-3 の手順 3・R-PR-4) の題材になる。M0a の公開パス索引は crate の中だけなので、A7 の代表の item は tokio・serde などから選ぶ

**v0.17 で要件に反映すること**: R-FV-2 の「形式 54: `Item::attrs` の型変更、57: `ExternalCrate::path`、61: `Stability::level`」に、55 の `MacroExport` (54 の `Other("#[macro_export]")` を写す) を足す。`ExternalCrate::path` が無い形式では空のパスを「不明」として扱うことを書く。

#### A7 (2026-10-19)

**JSON で確かめたこと** (tokio 1.53.3・axum 0.8.9。どちらも形式 61):

- docs.rs の JSON の `index` に private module は無い。tokio の `sync::mutex` は無く、中の `Mutex` は `index` にあって、`tokio::sync` の `Use` (`source: "mutex::Mutex"`) からだけたどれる
- `#[doc(hidden)]` の item は、rustdoc が既定で JSON から除くので、どの crate にも無かった (気にするのは `--document-hidden-items` で作る M4 の `api_diff` のとき)
- glob の `Use` は少なく (tokio 0、axum 2、serde 1、clap 1)、crate の外を指すものが多い (clap の中身は `pub use clap_builder::*` の 1 つ)
- crate をまたぐ re-export の定義パスも private module を含みうる (`axum::body::Bytes` の行き先は `bytes::bytes::Bytes`)。M1 で行き先の crate の公開パス索引で引き直す必要がある
- serde 1.0.228 の `Deserialize` は、crate をまたぐ re-export ではなく serde 自身の item (定義パス `serde::de::Deserialize`) として `index` にあった (予想と違ったので、テストの期待値を直した)

**作ったもの** (`src/query/path_index.rs`)

- `PathIndex::build`: ルートから `Module.items` と `Use` を深さ優先にたどり、item ごとの公開パスを集める (R-PR-2)。非 glob の `Use` は改名を反映し、glob は crate の中のモジュールの item と enum の variant を展開する。公開でない item と `is_stripped` のモジュールは公開パスに入れない。crate をまたぐ re-export と glob は記録だけして、中身は引かない (M1)。プリミティブ型の re-export (`id` が無い `Use`) も M1。たどっている途中のモジュールに戻ったら止め、深さは 256 まで (R-PR-3)
- 正準パス: 改名の無いパス → 短いパス → 辞書順 (§4。hidden・unstable のモジュールの除外は、データのある M1・M4)
- `PathIndex::resolve`: 公開パス → crate をまたぐ re-export → 定義パスの順に引き、無ければ親を引いてからメンバー (固有の impl → trait の impl → blanket impl の item、trait の item、enum の variant) を名前で探す (R-PR-1)。同じパスに複数の item があれば `Ambiguous` (R-PR-7)。crate 名の `-` と `_` は同一視する
- `ItemEnum` の変種は `_` でまとめずに書き出した (`wildcard_enum_match_arm`)。rustdoc-types に変種が増えたら、ここで見直すことになる
- 長く保持する文字列は `Box<str>`、パスは `Box<[Box<str>]>`

**代表の item** (release ビルド。`tests/testdata.rs` の `#[ignore]` のテスト):

| 引いたパス | 結果 |
| --- | --- |
| `tokio::sync::Mutex` | item。正準パス `tokio::sync::Mutex`、定義パス `tokio::sync::mutex::Mutex` |
| `tokio::sync::mutex::Mutex` (定義パス) | 同じ item |
| `tokio::sync::Mutex::lock` | `Mutex` の固有の impl の `lock` |
| `axum::Json` | item。正準パス `axum::Json`、定義パス `axum::json::Json` |
| `axum::Router` | item。正準パス `axum::Router`、定義パス `axum::routing::Router` |
| `axum::body::Body` | crate をまたぐ re-export (`axum_core::body::Body`) |
| `serde::Deserialize` | item。正準パス `serde::Deserialize`、定義パス `serde::de::Deserialize` |

索引を作る時間は、tokio で約 0.5 ms、axum で約 0.3 ms、serde で約 0.1 ms。

#### A8 (2026-10-19)

環境: rustc 1.99.1、rmcp 3.5.1、serde_json 1.0.154 (`preserve_order`・`unbounded_depth`)、insta 1.49.0。A8-1 (描画)・A8-2 (照会)・A8-3 (MCP のツール) に分けて進めた。

**JSON とまわりで確かめたこと**

- qualified path (`<F as Future>::Output`) の trait の `path` は、たいてい空の文字列で、id だけが入る (tokio は 67 件中 57 件、axum は 685 件中 572 件)。`F::Output` と trait を書かずに書いたものがこうなる。名前は `Crate::paths` から引ける (rustdoc の HTML と同じ)
- docs.rs の JSON の span の `filename` は、package root からの相対パス (`src/sync/mutex.rs`)
- 外部 crate の `html_root_url` は `https://docs.rs/axum-macros/0.5.1/x86_64-unknown-linux-gnu/` の形で、crates.io の名前 (`-` のまま) と版が読める。JSON の `name` は `axum_macros`
- `ItemEnum::Macro` の文字列は、`macro_rules! name { (…) => { ... }; }` の形で、規則の本体は rustdoc がもう `{ ... }` に省いている
- docs.rs の行の指し方 (2026-10-19 に HTML と JS を取って確かめた): ソースブラウザ (`/crate/{name}/{version}/source/{file}`) も rustdoc のソースページ (`/{name}/{version}/src/{crate}/{file}.html`) も、行ごとに `id="434"` があり、どちらの JS も `/^#?(\d+)(?:-(\d+))?$/` で `#434` と `#434-466` を強調表示する
- rmcp の `CallToolResult::structured_content` は `serde_json::Value` で、serde_json の既定の Map (BTreeMap) ではキーが辞書順に並び替わる。構造体のフィールドの順が消え、`docs` が `signature` より前に出ていた

**作ったもの**

- `src/query/render.rs` (A8-1): 型・ジェネリクス・境界・where 句と、item の種類ごとの宣言を Rust の文字列にする (R-OUT-2)。
  - `ItemEnum`・`Type` などの変種は `_` でまとめずに全部書き出した
  - この crate の型は正準パスで描き、ほかの crate の型は rustdoc が書いたまま (`Path.path`) にする
  - `self` は `self`・`&self`・`&'a mut self` に略し、それ以外は `self: Box<Self>` の形にする
  - `impl Trait` の引数に rustdoc が足したジェネリクスの引数 (`is_synthetic`) は出さない
  - `&(dyn A + B)` のように、要るところに括弧を付ける
  - 描けない形 (パターン型・名前の分からないパス) は warnings に入れる
- `src/query/get_item.rs` (A8-2): 通信もファイルの読み書きもしない関数。parse 済みの crate と索引から、§7.3 の形の応答を作る。
  - provenance の版・target・形式は JSON 本体から読み、要求と違えば warnings に出す (R-OUT-9)
  - メンバーには `parent` と、impl から来たものなら impl の見出し (where 句込み) を付ける (R-SH-2)
  - fields・variants は 1 つずつの文字列にし、非公開のものがあれば `/* private fields */`・`/* some variants omitted */` を足す。tuple struct はシグネチャに含める (R-SH-7)
  - ドキュメントは第 1 段落だけにする (コードブロックの中の空行では切らない)。続きがあれば `docs_truncated`
  - 属性は `deprecated`・`must_use`・`non_exhaustive` だけを出す (R-SH-6)
  - ほかの公開パスは `also_reexported_at` に出す
  - `source_location` に、ファイル・行・docs.rs のソースブラウザの URL を入れる (R-OUT-8)。1 行だけなら `"lines": "301"` と `#301`
  - ほかの crate の re-export は成功として返し、`reexport_of` (`axum-macros@0.5.1` と定義パス) と、そちらを引く hint を付ける
  - `kind` は rustdoc の `ItemKind` の語に合わせ、impl と trait の中の関数だけ `method` にする
- `src/mcp/tool/get_item.rs` (A8-3):
  - 引数は自前で検証する (R-ARG-1)。`crate` は `name@x.y.z` の完全な版だけで、版の省略や範囲は `invalid_argument`。`path` の先頭が crate 名でなければ crate 名を補う
  - JSON をキャッシュか docs.rs から読み、parse・索引・照会を `spawn_blocking` で行う。parse した結果はメモリに残さない
  - outputSchema は手で書き、全部の欄を埋めた応答のキーがすべて宣言されていることを単体テストで確かめる
  - annotations は R-MCP-4 のとおり (`readOnlyHint: false`・`destructiveHint: false`・`idempotentHint: true`・`openWorldHint: true`)
  - 説明文は `get_item.md` (1,236 文字) に置いて埋め込む (§6.2)
- エラーの対応 (§8):

  | 起きたこと | code |
  | --- | --- |
  | docs.rs の 404・400 | `docs_unavailable` (`no_json`) |
  | 大きさの上限・zstd の層が多すぎる | `docs_unavailable` (`too_large`) |
  | 展開できない・JSON が壊れている・形式の食い違い | `docs_unavailable` (`unparsable`) |
  | 一時的な HTTP エラー・通信の失敗 | `network_error` (retryable) |
  | 想定外の HTTP ステータス | `network_error` |
  | 対応していない形式 | `unsupported_format` (`found`・`supported`) |
  | パスが見つからない | `item_not_found` (探した経路を message に書く) |
  | 同名の item が複数 | `ambiguous_path` (`candidates`) |
  | キャッシュの読み出しの失敗・panic・ランタイムの終了 | `internal_error` |

- サーバーは起動時に `Cache` と `DocsRs` を作り、作れなければ起動しない。echo と probe_result を消し、sleep は debug ビルドだけに残した。引数の解析は `CallToolResult` ではなく小さな `InvalidArgument` を返す (`preserve_order` で `CallToolResult` が大きくなり、`clippy::result_large_err` に当たったため)
- `tests/stdio.rs` は、サーバーごとの一時ディレクトリをキャッシュにして、手で組み立てた crate を置く (ネットワークに出ない)。`tests/testdata.rs` の `#[ignore]` のテストで、代表の 11 件の応答を insta で固定した (`tests/snapshots/`)

**決めたこと**

- where 句は `detail=normal` にも入れる。rustdoc は境界の多くを where 句に移すので、入れないと `tokio::spawn` の `F: Future + Send + 'static` が消える
- ほかの crate の型の正準パスは M1 (その crate の公開パスの索引が要る。R-PR-4)。定義パスは private module を含みうるので使わない。M1 で、prelude の型 (`Option`・`Result`・`Vec`・`String`・`Box`) を短いまま残すかを決める
- ほかの crate の re-export は、エラーにせず行き先を返す
- R-SH-2 の「trait」は、別の欄にせず impl の見出し (`impl<T> Trait for Type`) で示す。見出しに trait が入るので、欄を分けると同じ情報が 2 回出る
- serde_json の `preserve_order` を有効にし、構造体のフィールドの順 (provenance → data、signature → where → docs) を JSON に残す (R-SH-1)
- `source_location.url` は docs.rs のソースブラウザにする。span のパスがそのまま使え、src の外のファイルも指せる
- R-ERR-2 の `request_id` は、突き合わせる相手のログ (R-OBS-1) と一緒に M1 で入れる

**R-OUT-2 の膨張率** (release ビルド。`tests/testdata.rs` の `representative_items_are_returned_by_get_item`)

左の比は、rustdoc の部分木 (item と、そのフィールドか variant) の compact な JSON と、応答全体 (`structuredContent`) の長さの比。右の比は、部分木から `docs`・`links`・`span` を除いたものと、描画した signature・where・fields・variants の長さの比。

| item | 部分木 / 応答 | シグネチャの木 / 文字列 |
| --- | --- | --- |
| `tokio::sync::Mutex` | 5,302 / 489 (10.8 倍) | 547 / 53 (10.3 倍) |
| `tokio::sync::Mutex::lock` | 1,416 / 691 (2.0 倍) | 602 / 60 (10.0 倍) |
| `tokio::spawn` | 6,264 / 656 (9.5 倍) | 1,661 / 148 (11.2 倍) |
| `tokio::select` | 19,952 / 768 (26.0 倍) | 597 / 255 (2.3 倍) |
| `tokio::io::AsyncReadExt::read_buf` | 3,650 / 686 (5.3 倍) | 1,306 / 110 (11.9 倍) |
| `tokio::runtime::RuntimeFlavor` | 1,352 / 526 (2.6 倍) | 776 / 55 (14.1 倍) |
| `axum::Router::route` | 4,820 / 606 (8.0 倍) | 687 / 87 (7.9 倍) |
| `axum::extract::State` | 8,552 / 445 (19.2 倍) | 501 / 29 (17.3 倍) |
| `serde::Deserialize` | 3,471 / 565 (6.1 倍) | 1,948 / 35 (55.7 倍) |

- シグネチャだけで比べると、おおむね 8〜17 倍。要件の「syphar で約 5〜6 倍、ferritin の JSON で 8〜12 倍」と同じか、それより大きい
- 外れ値: `tokio::select` は、マクロの規則の文字列を JSON でも応答でもほぼそのまま持つので 2.3 倍。`serde::Deserialize` は、trait の JSON が実装者の id の一覧 (`implementations`) を抱えているので 55.7 倍
- 応答全体の比は、ドキュメントを第 1 段落に絞った分も含む (`tokio::select` の 26 倍は主に長いドキュメント)
- 描画の時間は、tokio の全 6,490 item で 3.4 ms、axum の全 4,851 item で 2.9 ms (使い捨ての example で測った。warnings は 0 件)

**実際のバイナリで確かめたこと** (release ビルドに JSON-RPC を流した)

- `tokio@1.53.3` の `sync::Mutex::lock`: crate 名が補われ、正準パス、`impl<T: ?Sized> tokio::sync::Mutex<T>`、`pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, T>` が返った
- `tokio@1.0.0`: docs.rs が 404 で、`docs_unavailable` (`no_json`)
- `clap@4.5.39`: 形式 46 で、`unsupported_format` (`found: 46`、`supported: {from: 54, to: 61}`)

**M1 への申し送り**

- ほかの crate の型の正準パス (上の「決めたこと」)
- ドキュメントの intra-doc link の書き換え (R-OUT-5)、doctest の隠し行の除去 (R-OUT-6)、残りの節の見出しと行数 (R-OUT-7)。今は第 1 段落をそのまま出していて、`` [`MutexGuard`] `` がリンク先なしで残る
- trait の impl のメソッドにドキュメントが無いとき、trait 側のドキュメントで補う
- `item_not_found` の `did_you_mean` (類似度の下限が要る)
- `request_id` と R-OBS-1 のログ
- 壊れたキャッシュの取り直し (R-CA-4)。今はキャッシュの読み出しの失敗を `internal_error` にしている
- docs.rs の 404 は「crate や版が無い」と「JSON が無い」を区別できない。`crate_not_found`・`version_not_found` に分けるには crates.io の index を見る (R-VR-4)
- 性能 (A10 の後に測った。release ビルド、キャッシュ済み): 1 回の `get_item` の時間はほぼ parse で、rmcp (14 MB) では読み出し 6 ms・parse 62 ms・索引 0.6 ms・照会と直列化 0.02 ms・解放 9 ms。解放は応答の後に別のスレッドへ回した (33a4341)。parse した crate と索引をメモリに残す案 (2 回目以降が 1 ms 未満になる。R-MEM-1 の上限と R-PF-1 の rkyv と合わせて設計する) と、メモリの確保器 (mimalloc など) を替える案は、M1 で測ってから決める。公開パス索引のパスを crate から借りる (`&str`) と索引を作るときの確保が減るが、crate と索引を一緒にメモリに残すときに自己参照になるので、メモリに残す案と一緒に決める

**v0.17 で要件に反映すること**

- R-OUT-7: where 句を `full` だけでなく `normal` にも入れる
- R-SH-2: trait は impl の見出しで示すことを書く
- R-OUT-2: 膨張率の実測 (上の表) を書き、「M0 で実測して確かめる」を消す。ほかの crate の型の正準パスは M1 からであることを書く
- R-OUT-1: `structuredContent` のキーの順を保つため、serde_json の `preserve_order` を使うことを書く (R-SH-1 の前提)
- R-OUT-8: `source_location` の形 (`file`・`lines`・`url`) と、docs.rs のソースブラウザの URL に行のアンカー (`#434-466`) を付けることを書く
- R-ERR-2: `request_id` は M1 から
- §8: docs.rs の 400 も `no_json` にすること。想定外の HTTP ステータスの扱い (`network_error`、retryable false)

#### A9 (2026-10-19)

環境: Claude Code 2.1.303 (Linux)、モデルは Opus 5.5、サーバーは debug ビルド。A3 と同じ [poc/m0a-a3/](../poc/m0a-a3/) で、README の A9 の節の問いを headless (`claude -p … --output-format stream-json --verbose`) で 5 回実行した。合計で約 1.08 ドル。

| 回 | 確かめたこと | 結果 |
| --- | --- | --- |
| 1 | R-OUT-1 (成功、引数を指定) | モデルに渡った tool_result は `structuredContent` の JSON で、キーの順 (provenance → data、signature → docs) も保たれていた |
| 2 | R-ERR-2 (エラー、`tokio@1.0.0`) | `is_error: true` で、text の JSON (`docs_unavailable`、`reason: no_json`、suggestions) がそのまま渡った |
| 3 | 版を指定した問い (axum 0.8.9 の `Router::route`) | 説明文だけで、1 回目に `{"crate": "axum@0.8.9", "path": "axum::Router::route"}` を作れた。答えには impl の where 句を使い、`docs_truncated` を見て「本文は確かめていない」と断っていた |
| 4 | ほかの crate の re-export (axum 0.8.9 の `debug_handler`) | `reexport_of` と hint を受けて、`axum-macros@0.5.1` の `axum_macros::debug_handler` を引き直した (docs.rs からその場で取得)。ツールの呼び出しは 2 回で、回り道は無かった |
| 5 | 版の無い問い (tokio の `spawn`) | `invalid_argument` にはならなかった。説明文の「Cargo.lock is not read yet」を読み、モデルが自分で Bash で Cargo.lock を探して、版を決めてから 1 回で呼んだ |

- 接続の順は A3 と同じ (`server/discover` → `tools/list` → `tools/call`、どれも `_meta` で 2026-07-28)。ツールは遅延読み込みで、毎回 ToolSearch で `select:` してから呼んだ
- 回 2 で、モデルの返答は `<error>…</error>` で囲まれていた。stream-json の tool_result の中身にはこの囲みが無いので、Claude Code が `is_error` の結果をモデルに渡すときに囲んでいると考えられる (確かめてはいない)
- 回 5 は、作業ディレクトリ `poc/m0a-a3` に Cargo.lock が無かったので、モデルはリポジトリ直下の Cargo.lock (tokio 1.53.3) を使った。これは実験の置き方によるもので、実際の利用では作業ディレクトリのプロジェクトの Cargo.lock になる。ただ、モデルに任せると別のプロジェクトの Cargo.lock を拾いうるので、M1 でサーバーがプロジェクトを決めて (R-MCP-3) Cargo.lock から版を決める (R-VR-2) 意味がある
- モデルが答えの中で「出力に無い」と書いたものは、ドキュメントの本文 (回 3) と必要な feature (回 4、axum の `macros`)。どちらも M1 以降の範囲 (R-OUT-7 の `full`、R-FT)

前提が崩れた項目は無いので、§7 の出力設計は今のままで A10 に進む。

#### A10 (2026-10-19)

環境: rustdoc_processor 0.1.2 (2026-10-12 公開)。突き合わせのプロジェクトは [poc/m0a-a10/](../poc/m0a-a10/) (独立した cargo プロジェクト。使い方は同じ場所の README)。

**rustdoc_processor について分かったこと** (crates.io の依存の一覧と、ソースを読んで):

- rustdoc-types を `^0.57.3` に固定している (このサーバーは 0.61)。0.57 の型で直接読めるのは形式 57 だけ
- 依存が重い: guppy (cargo metadata の依存グラフ)、rusqlite (`bundled` で SQLite の C のソースをビルド)、r2d2、rayon、rkyv 0.8、bincode、toml、globwalk など
- API の中心は、`PackageGraph` を受けて必要なら `cargo rustdoc` を起動し、SQLite にキャッシュする一式 (`CrateCollection`)。docs.rs の JSON をそのまま渡す入口は無いが、下の層の `queries::Crate::index_without_visitor(CrateData, PackageId)` で手元の JSON から索引を作れる
- 正準パスは「最短 → 辞書順」で、改名を避けない。索引に入れるのはモジュールと、trait・struct・enum・union・関数・型の別名・定数・static・プリミティブ。マクロは入れない
- ライセンスは Apache-2.0 のみ (依存にはできるが、コードは写せない。R-DIST-4)

**突き合わせの方法**: 同じ docs.rs の JSON を、このサーバーの parse (0.61 の型) で読み、JSON に戻して `format_version` を 57 に書き換え、0.57 の型で読み直して rustdoc_processor に渡す。形式 57〜61 の違いは stability などの欄だけで、0.57 の型は知らない欄を無視する (`deny_unknown_fields` は無い)。item ごとに、公開パスの集合と正準パスを比べた。

**結果** (11 crate、どれも形式 61。release ビルド):

| crate | 公開パスを持つ item (こちら / rustdoc_processor) | 完全一致 | 違い |
| --- | --- | --- | --- |
| tokio 1.53.3 | 319 / 314 | 314 | こちらだけ: マクロ 5 |
| axum 0.8.9 | 185 / 185 | 185 | なし |
| serde 1.0.228 | 60 / 59 | 59 | こちらだけ: マクロ 1 |
| futures-util 0.3.34 | 304 / 296 | 296 | こちらだけ: マクロ 8 |
| tracing-subscriber 0.3.23 | 120 / 120 | 117 | 正準パスの違い 3 |
| rmcp 3.5.1 | 544 / 541 | 541 | こちらだけ: マクロ 3 |
| reqwest 0.13.5 | 55 / 54 | 54 | こちらだけ: モジュール 1 |
| serde_json 1.0.154 | 49 / 48 | 48 | こちらだけ: マクロ 1 |
| hyper 1.12.0 | 140 / 140 | 140 | なし |
| tower 0.5.3 | 172 / 170 | 170 | こちらだけ: モジュール 2 |
| clap 4.6.7 | 21 / 21 | 20 | 公開パスの集合の違い 1 |

違いの原因 (JSON で確かめた):

- マクロ (18 件): rustdoc_processor は索引に入れない。こちらが正しい
- `use` で再エクスポートしたモジュール (4 件。`reqwest::multipart`、`tower::util::future::optional` と `tower::util::error::optional` (どちらも改名)、`clap::_derive::_cookbook`): rustdoc_processor は中の item はたどるが、モジュール自身のパスを記録しない。こちらが正しい
- 改名 (1 件。tracing-subscriber の `fmt::Subscriber` と `FmtSubscriber`): R-PR-2 の規則の違い (こちらは改名の無いパスを選ぶ)
- `pub use Trait as _;` (2 件。tracing-subscriber の `prelude`): **両方のバグ**。どちらも `tracing_subscriber::prelude::_` を公開パスに入れていた (`_` は名前ではなく、`use` できない)。rustdoc_processor はこれを正準パスに選ぶ。こちらは正準パスには選ばないが、`also_reexported_at` に出し、`resolve` でも引けていた。`visit_use` で `as _` を飛ばすように直し、単体テストを足した。直した後は、こちらの集合からこのパスが消えた

**決めたこと**: rustdoc_processor は依存にしない。理由は、rustdoc-types の版が合わないこと、依存の重さ、cargo の起動を前提にした API、突き合わせでこちらの索引のほうが正しかったこと。比較相手としては役に立ったので、M1 で公開パス索引を直したら poc/m0a-a10 でもう一度突き合わせる。突き合わせに使った 7 crate は、テストデータの一覧に足した。

**M1 への申し送り**: `pub use Trait as _;` は名前を作らないが、`use tracing_subscriber::prelude::*;` で trait がスコープに入ることは利用者に役立つ。この情報 (どの glob で trait が使えるようになるか) を持つかを M1 で決める。

**v0.17 で要件に反映すること**: §10 のデータ層の行の「依存にするかは M0 で評価」を、評価の結果 (依存にしない、比較相手として使う) に改める。R-PR-2 に、`as _` の再エクスポートはパスを作らないことを書く。

### 6.2 M0b

実験のスクリプトと合成の workspace は [poc/m0b/](../poc/m0b/) にある (共通の設定は `env.sh`、手順は README)。「実際の workspace」はこのリポジトリ (Cargo.lock に 242 パッケージ、member のビルドで 186 unit)。

#### B1 (2026-10-19)

環境は B2 と同じ (stable 1.99.1、nightly 1.101.0-nightly 2026-10-17)。スクリプトは [poc/m0b/b1-rustc-bootstrap/run.sh](../poc/m0b/b1-rustc-bootstrap/run.sh)。どれも stable の `cargo rustdoc … -- -Z unstable-options --output-format json` で、1 つの target dir の中で順に実行した。

**`RUSTC_BOOTSTRAP` を見る依存**: このリポジトリの 242 パッケージのうち、build.rs が `cargo:rerun-if-env-changed=RUSTC_BOOTSTRAP` を出すのも、build.rs のソースで `RUSTC_BOOTSTRAP` に触れるのも、**proc-macro2** (1.0.107) と **thiserror** (2.0.21) の 2 つだけだった。どちらも自分の crate 名が値に入っていると nightly の機能を有効にする (proc-macro2 は `proc_macro_span`・`proc_macro_span_file`・`proc_macro_span_location`・`wrap_proc_macro`、thiserror は `error_generic_member_access`)。

**作り直しの範囲**:

| 値の決め方 | member (コールド) | 依存の文書化 | その後の member |
| --- | --- | --- | --- |
| crate ごとに変える (`chatora_rustdoc_mcp` → `serde` → … ) | 24.0 s (186 unit) | serde 4.0 s・thiserror 3.2 s (各 5 unit) | 12.3〜12.4 s (58 unit) |
| 固定 (`chatora_rustdoc_mcp,serde,thiserror,tokio`) | 22.9 s | serde 0.25 s・tokio 0.85 s・thiserror 0.17 s (各 1 unit) | 0.56 s (1 unit) |
| 文書化した依存の名前を足していく | 23.7 s | serde 4.0 s・thiserror 3.3 s (各 5 unit) | 10.4 s (54 unit) |
| 解決結果の全 crate 名 (238 名、2,806 文字) に固定 | 23.5 s | thiserror 0.17 s・serde 0.24 s (各 1 unit) | 0.54 s (1 unit) |
| member の値と、値なしの `cargo check --workspace --all-targets` で target dir を共有 | check 24.0 s (208 unit) | — | 切り替えるたびに 12.2〜12.9 s (58〜65 unit) |

- 値が変わると proc-macro2 の build.rs がやり直され、proc-macro2 → quote・syn → すべての proc-macro → それを使う crate (serde・thiserror・tracing・tokio まで) の 54〜65 unit が作り直される。値を固定すれば、やり直すのは文書化する crate の rustdoc の 1 unit だけ
- 値なしのビルドと target dir を共有すると、行き来するたびに同じ範囲が作り直される (R-WS-2 の専用 target dir の裏付け)

**文書の違い** (JSON は、キーを並べ替え、target dir の絶対パスを含む `external_crates[].path` を除いてから比べた。そのままでは map の並び順と絶対パスで毎回違う):

- member の JSON は、crate ごと・固定・足していく・`cargo check` と共有、のどの値でも同一だった。値に `thiserror` が入って thiserror が nightly の経路でコンパイルされても、member の文書は変わらない
- 値を解決結果の全 crate 名にすると、member の JSON のうち 4 item (thiserror の `#[from]` が生成する `impl From` など、derive が生成した impl) の span の列だけが変わった (例: 13〜17 列 → 11〜18 列)。proc-macro2 が `proc_macro_span` を有効にし、derive が返す span がより正確になるため。中身はほかに変わらない
- thiserror の JSON は、stable + `RUSTC_BOOTSTRAP` (どの値でも同一) と docs.rs (形式 61) で item 単位で一致した。nightly (1.101、これも形式 61) とは、属性 1 つの表記 (`CfgAttrTrace`) だけが違った

**決めたこと (Q-12)**:

- ローカルビルドの既定は、プロジェクトの toolchain (stable) + crate 名を指定した `RUSTC_BOOTSTRAP` とする (R-WS-8 の候補を確定)
- 値は、プロジェクトの専用 target dir ごとに、**依存の解決結果のすべての lib・proc-macro の crate 名 (member を含む) を辞書順に並べたもの**に固定する。値は Cargo.lock と member の構成が変わらない限り変わらないので、作り直しは文書化する crate だけになり、target dir も 1 つで済む。自分の名前で nightly の機能を調べる依存 (proc-macro2・thiserror) は nightly の経路でコンパイルされ、derive が生成した item の span の列が少し変わる (docs.rs と同じ側になる)。その経路でビルドが壊れたときに、member の名前だけの値でやり直すかは M2 で決める
- 値 (のダイジェスト) を R-WS-4 の鮮度の鍵に入れる (値で文書の span が変わることを確かめたため)
- 依存の多い公開の workspace は足さない (作り直しは proc-macro2 から広がる仕組みで、workspace によらない)

**v0.17 で要件に反映すること**: Q-12 を決定にし、R-WS-8 の「値を安定させる」の案を上の方式に改める。R-WS-8 の「影響範囲を絞るため crate 名を指定する」は、値が全 crate 名になるので意味が変わる。`RUSTC_BOOTSTRAP=1` との違いは、cargo 自身の `-Z` (cargo は値が `1` のときだけ受ける。B2) と、build script (crate 名は `build_script_build`) に効かないことだけになる、と書き直す。R-WS-4 の鍵に値のダイジェストを足す。

#### B2 (2026-10-19)

環境: stable 1.99.1 (2026-10-13)・cargo 1.99.1、nightly 1.101.0-nightly (2026-10-17)・cargo 1.101.0-nightly (2026-10-12)、host x86_64-unknown-linux-gnu、12 コア。スクリプトは [poc/m0b/b2-doc-vs-rustdoc/](../poc/m0b/b2-doc-vs-rustdoc/) の `run.sh` (このリポジトリ) と `dup.sh` (同名で版の違う依存。`dup-ws/` は itoa 0.4.8 と 1.0.18 に通常の依存として依存する)。どのケースも専用の target dir で、コールドとウォームを測った。

| 比べたこと | `cargo rustdoc … -- -Z unstable-options --output-format json` | `cargo doc -Z unstable-options --output-format json` |
| --- | --- | --- |
| member (`--lib`) をコールドで | 25.2 s | 26.3 s |
| 依存 (serde 1.0.229) をコールドで | 6.5 s | 6.1 s |
| 何も変えずに 2 回目 | member 0.51 s・serde 0.25 s。文書化する crate の rustdoc は毎回やり直す (186 unit のうち 185 が fresh) | member 0.23 s・serde 0.09 s。全部 fresh |
| JSON の出る場所 | `target/<triple>/doc/<name>.json` だけ | 同じ場所に加えて、unit ごとの `<build dir>/…/build/<pkg>/<hash>/out/<name>.json` |
| artifact のメッセージ | 文書化した unit の `filenames` が空 | uplift した JSON のパスを指す |
| 依存をまとめて文書化 | できない (1 crate ずつ) | `-p` なしで 146 crate の JSON を 38.4 s (member だけより 13 s 多いだけ。ウォームは 0.18 s) |
| stable の cargo + `RUSTC_BOOTSTRAP=chatora_rustdoc_mcp` | 動く (24.7 s) | `the -Z flag is only accepted on the nightly channel of Cargo` で拒否。`RUSTC_BOOTSTRAP=1` (プロセス全体) なら動く (26.5 s) |
| 同名で版の違う依存 (itoa 0.4.8 と 1.0.18) | 1 版ずつ作れるが、どちらも `target/<triple>/doc/itoa.json` に出て、後のビルドが上書きする | 両方を `-p` で指すと `document output filename collision` で失敗する。`-p` なしだと 0.4.8 は check だけで文書化されず、警告も無い (1.0.18 だけ) |

- 同じ target dir で `cargo rustdoc` → `cargo doc` → `cargo rustdoc` → `cargo check` と続けても、依存の unit は使い回され、やり直すのは member の 1 unit だけだった (`--output-format json` は文書化する crate の最後の rustdoc にだけ効き、依存の check の fingerprint を変えない)
- **要件との食い違い**: R-WS-1 の「同名で版の違う依存は別々のファイルに出るが、メッセージの `filenames` は両方とも同じ `target/doc/dep.json` を指す (2026-10-14 に確認)」は、今回の cargo では再現しなかった。`cargo doc` は片方しか文書化しない (`-p` で両方を指すとエラー)
- proc-macro からしか使われない依存 (このリポジトリの syn 2.0.119 と 3.0.9) を `-p` で名指しすると、`cargo rustdoc` も `cargo doc` も、cargo の feature resolver の panic (`did not find features for (… syn …, NormalOrDev) within activated_features`、`resolver/features.rs:326`) で止まった。R-WS-5 の「`-p` で cargo が失敗する依存」が resolver 2 で実際に起きる (B7-9 で種類ごとに確かめる)
- target dir 1 つの大きさは 0.6〜0.75 GB (member の文書化で 627 MB、依存をまとめた `cargo doc` で 757 MB)

**決めたこと (Q-17)**: ローカルビルドは R-WS-1 のとおり `cargo rustdoc` で行う。stable + crate を絞った `RUSTC_BOOTSTRAP` (R-WS-8) と両立し、同名の複数版も 1 版ずつ作れる。毎回やり直すのは文書化する crate の rustdoc だけで、鮮度は R-WS-4 でサーバーが判定するので問題にならない。JSON の場所はメッセージに頼らず、ターゲット名から求める (R-WS-1 のとおり)。同名の複数版は、ビルドごとに JSON を移す (R-WS-1 のとおり)。`cargo doc --output-format json` は、nightly を使うときに依存をまとめて文書化する用途の候補として M2 に残す (同名の複数版の古い方が黙って落ちることに注意)。

**v0.17 で要件に反映すること**: Q-17 を決定にする。R-WS-1 の `cargo doc` の節の「同名で版の違う依存は別々のファイルに出る」を、今回の結果 (`-p` で両方を指すとエラー、`-p` なしだと古い方が落ちる) に直す。

#### B3・B6・B7-2・B7-9 (2026-10-19)

環境は B1 と同じ。スクリプトは [poc/m0b/b3-features/run.sh](../poc/m0b/b3-features/run.sh)。合成の workspace [ws/](../poc/m0b/b3-features/ws/) の member は `app` だけで、`deps/` の crate を依存の種類ごとに使う: `shared` を通常の依存 (feature `normal`)・Windows でだけ使う依存 (`windows`)・build-dependency (`build`)・dev-dependency (`dev`) として、ほかに dev-dependency だけの `devonly`・build-dependency だけの `buildonly`・Windows でだけ使う `winonly`・proc-macro の `pm` とその依存 `pmdep`・どこからも使わない `outside`。どれも workspace の外の path の crate (外へ通信しない)。スクリプトは workspace を `resolver = "2"` と `"1"` に書き換えて複製する。

**B3・B7-2 (feature の組)**: `shared` の feature

| | resolver 2 | resolver 1 |
| --- | --- | --- |
| `cargo tree --workspace --target <host> -e normal -f '{p} [{f}]'` | `normal` | `build,dev,normal,windows` |
| 同じ、`--target x86_64-pc-windows-gnu` | `normal,windows` | 同じ 4 つ |
| `cargo tree … -e normal,dev` | `dev,normal` | 同じ 4 つ |
| `cargo tree … -e build` | `build` | 同じ 4 つ |
| `cargo check --workspace` の target 向けの unit | `normal` | 同じ 4 つ |
| `cargo check --workspace --all-targets` の target 向けの unit | `dev,normal` | 同じ 4 つ |
| `cargo rustdoc -p app --lib` の target 向けの unit (文書化するとき) | `normal` | 同じ 4 つ |
| build-dependency 向けの host の unit | `build` (target 向けとは別の unit) | 同じ 4 つ (host 向けも target 向けも) |

- `cargo tree -e normal --target <t>` の組は、`cargo rustdoc --lib` が実際に使う組と、resolver 1 でも 2 でも一致した。dev-dependency の feature は `--all-targets` のときにだけ混ざる

**B6 (`cargo tree` の出力の形)**:

- `cargo tree --workspace --target <host> -e all --prefix depth -f '{p} [{f}]'` の出力は、stable の cargo 1.99.1 と nightly の cargo 1.101.0 で同一だった
- `{p}` は名前と版、path や git の取得元 (crates.io は出ない)、`(proc-macro)` の印を出す。package id は出ない。`{lib}` (lib の名前) は stable で使えた。`{r}` (repository) は path の crate では空
- `-e normal` の木には、proc-macro (`pm`) の下の依存 (`pmdep`) も出る。実際には host 向けにビルドされるが、木の上では区別されない。host 向けかどうかは、build の辺の先と、`(proc-macro)` の印が付いた node の下で判断することになる
- nightly の `cargo check --unit-graph -Z unstable-options` は、unit ごとに package id (`path+file:///…#0.1.0`)・`platform` (host なら null)・`mode`・feature を出す (例: resolver 2 の `shared` は `mode=build platform=host features=["build"]` と `mode=check platform=x86_64-unknown-linux-gnu features=["dev","normal"]`)
- 実行中に一度だけ `Blocking waiting for file lock on package cache` が出た (エディタの rust-analyzer と見られる。R-WS-2 のロックの共有の実例)

**B7-9 (`cargo rustdoc -p` で依存を名指し)**:

| 依存の種類 | resolver 2 | resolver 1 |
| --- | --- | --- |
| 通常の依存 (`shared`)・proc-macro 自身 (`pm`) | 成功 | 成功 |
| dev-dependency だけ (`devonly`) | cargo が panic | 成功 |
| build-dependency だけ (`buildonly`) | cargo が panic | 成功 |
| Windows でだけ使う (`winonly`)、`--target <host>` で | cargo が panic | 成功 |
| 同じ、`--target x86_64-pc-windows-gnu` で | 成功 | 成功 |
| proc-macro からしか使われない (`pmdep`) | cargo が panic | 成功 |
| 依存でない (`outside`) | `package ID specification … did not match any packages` | 同じ |

- panic はどれも `resolver/features.rs:326` の `did not find features for (…, NormalOrDev)` で、B2・B4 と同じ。2026-10-16 のレビューの「resolver 2 で dev だけ・build だけ・ほかの target だけの依存を名指しすると panic する」は正しく、proc-macro からしか使われない依存も同じだった。resolver 1 では起きない。R-WS-5 と R-MCP-3 の食い違いはこれで解ける

**決めたこと**:

- Q-20・R-FT-3: ローカルビルドの feature の組は `cargo tree -e normal --target <t>` の組を基準にする (`cargo rustdoc --lib` が使う組と一致し、v0.15 で「プロジェクトで有効」の基準にしたものとそろう)
- R-MCP-3・Q-30: 依存が host 向けか target 向けかは、stable の `cargo tree` で判断する (build の辺の先と `(proc-macro)` の node の下が host 向け、それ以外が target 向け)。package id は `cargo tree` に出ないので、名前・版・取得元から `cargo metadata` の id に対応付ける。nightly の `--unit-graph` は使わない (B1 で stable を既定にしたため)
- R-WS-5・Q-30: resolver 2 以上の workspace では、member から target 向けの通常の辺でたどれない依存 (dev だけ・build だけ・ほかの target だけ・proc-macro の下だけ) を `cargo tree` で判定し、cargo を起動せずに `docs_unavailable` (`reason: build_unsupported`) を返す。crates.io の依存なら docs.rs を、ほかの target だけの依存ならその `target` の指定を案内する。resolver 1 では panic しないので、そのまま cargo を起動する。stderr の panic の文言は照合しない

**v0.17 で要件に反映すること**: Q-20 を決定にする。R-WS-5 の panic の記述を確かめた結果 (proc-macro の下だけの依存を足す) と、起動前の判定に改め、R-MCP-3 の記述をそろえる。Q-30 の該当項目 (`cargo tree` の形と package id、`-p` の失敗) を決定にする。

#### B4 (2026-10-19)

環境は B1 と同じ。スクリプトは [poc/m0b/b4-cfg-docsrs/run.sh](../poc/m0b/b4-cfg-docsrs/run.sh)。このリポジトリが host 向けにビルドする通常の依存 146 個 (`cargo tree --workspace --target <host> -e normal` の、member を除いたもの) を、1 つずつ `cargo rustdoc -p <name>@<version> --lib` で文書化した。`--cfg docsrs` は最後の rustdoc にだけ渡し (R-WS-1)、失敗したら渡さずにやり直した。stable は B1 で決めた `RUSTC_BOOTSTRAP` の値、nightly は値なしで、それぞれ専用の target dir で行った。

| 結果 | stable | nightly |
| --- | --- | --- |
| 成功 | 135 | 135 |
| `--cfg docsrs` のときだけ壊れる | 1 (rustls-platform-verifier 0.7.1) | 1 (同じ) |
| どちらでも壊れる | 0 | 0 |
| cargo が panic (`-p` で名指しできない) | 10 | 10 |
| 合計の時間 (コールドを含む) | 71.1 s | 71.4 s |

- **`--cfg docsrs` で壊れたもの**: rustls-platform-verifier は `#[cfg(any(all(doc, docsrs), target_os = "android"))] mod android;` で Android 用のモジュールを文書に入れ、その中で `jni` を使う。`jni` は Android の target の依存と、crate 自身の `docsrs` feature (optional) でだけ入る。ローカルビルドはプロジェクトの feature の組で作るので、`--cfg docsrs` だけを渡すと `jni` が無く `E0433` で壊れる (docs.rs ではこの feature を有効にしているとみられる)。渡さずにやり直すと成功した。文書化できた 136 個のうち 1 個 (0.7%) で、stable と nightly で同じだった
- **cargo の panic**: convert_case・proc-macro2・quote・serde_derive_internals・syn (2.0.119 と 3.0.9)・synstructure・unicode-ident・unicode-segmentation・unicode-xid の 10 個。どれも proc-macro からしか使われない依存で、`-p` で名指しすると `resolver/features.rs:326` で `did not find features for (…, NormalOrDev)` の panic になる (B2 と同じ)。proc-macro の crate 自身 (serde_derive・thiserror-impl など) は文書化できた。stable の cargo 1.99.1 と nightly の cargo 1.101.0 の両方で起きた

**決めたこと (Q-11)**: R-WS-1 の「既定で `--cfg docsrs` を渡し、壊れたら渡さずにやり直す」はそのままにする。やり直しの頻度はこのリポジトリで 136 個中 1 個で、原因は「`--cfg docsrs` を前提にした crate 自身の feature が、プロジェクトの組では有効でない」ことだった。壊れた crate を記録して次から渡さないかは、要件のとおり M2 で決める (頻度が低いので、記録しなくても費用はやり直し 1 回分にとどまる)。proc-macro からしか使われない依存の扱いは B7-9 で決める。

**v0.17 で要件に反映すること**: Q-11 に頻度 (136 個中 1 個) と原因の型を書く。

#### B5 (2026-10-19)

環境は B1 と同じ。スクリプトは [poc/m0b/b5-dep-info/run.sh](../poc/m0b/b5-dep-info/run.sh)。合成の crate [probe/](../poc/m0b/b5-dep-info/probe/) は、`#![doc = include_str!(…)]`・`include_bytes!`・`#[path]`・build.rs が `OUT_DIR` に生成して `include!` するコード・`#[cfg(docsrs)]` のときだけ読むモジュールを持つ。どれも stable + B1 で決めた `RUSTC_BOOTSTRAP` で、JSON と同じ rustdoc の引数 (`--cfg docsrs` を含む) を使った。

| | (a) rustdoc をもう一度 (`--emit=dep-info` を足す) | (b) その crate の `cargo check` の dep-info |
| --- | --- | --- |
| 出る場所 | `<target dir>/<triple>/doc/<crate>.d` | `<build dir>/<triple>/debug/deps/<crate>-<hash>.d` |
| member (chatora-rustdoc-mcp) の時間 | 0.53 s (JSON の生成と同じくらい) | 0.61 s (member の check の 1 unit が増える。文書化だけでは member の check は作られない) |
| 依存 (tokio 1.53.3) の時間 | 0.92 s | 0.09 s (依存として check 済み。新しく作るものは無い) |
| `include_str!`・`include_bytes!`・`#[path]`・build.rs が生成したコード | すべて載る。`# env-dep:OUT_DIR=…` も載る | すべて載る |
| `#[cfg(docsrs)]` のときだけ読むファイル | 載る | **載らない**。tokio では 250 件のうち 5 件 (`src/doc/mod.rs`・`src/doc/os.rs`・`src/io/bsd/poll_aio.rs`・`src/net/windows/mod.rs`・`src/net/windows/named_pipe.rs`) が欠けた |
| JSON | 書き換えない (直前の JSON が残る) | 影響なし |

- build.rs 自体は、どちらにも載らない (build script 自身の dep-info `build_script_build-<hash>.d` にある。R-WS-4 は build.rs を別に鍵に入れているので問題ない)
- (a) の後に JSON をもう一度作っても、依存は fresh のままだった (やり直すのは文書化する crate の 1 unit だけ)

**決めたこと (R-WS-4)**: dep-info は (a) で取る。JSON と同じ引数に `--emit=dep-info` を足して rustdoc をもう一度実行する。(b) は依存なら無料だが、`--cfg docsrs` のときだけ読むファイルを取りこぼし、その変更で鮮度切れにならない。(a) の費用はその crate の rustdoc 1 回分。JSON を書き換えないので、JSON を移した後に実行してよい。

**v0.17 で要件に反映すること**: R-WS-4 の「どちらにするかは M0 で所要時間を測って決める」を、(a) に決めたことと理由 (`#[cfg(docsrs)]` のファイル) に改める。

#### B7-1・B7-6・B7-7・B7-8 (2026-10-19)

環境は B1 と同じ。スクリプトは [poc/m0b/b7-toolchain/run.sh](../poc/m0b/b7-toolchain/run.sh)。合成の workspace [ws/](../poc/m0b/b7-toolchain/ws/) の member `lib` は、`--cfg foo` が届いた場所を item で示す: crate 自身 (`lib_saw_foo`)、build script (`build_saw_foo`。build.rs が `cfg!(foo)` を見て cfg を出す)、proc-macro `foomacro` (`macro_saw_foo`。proc-macro が `cfg!(foo)` を見て関数を生成する)。

**B7-1 (`RUSTFLAGS="--cfg foo"` と `--target`)**:

| | build script | proc-macro | 文書化する crate 自身 |
| --- | --- | --- | --- |
| `--target` なし | 届いた | 届いた | 届かない |
| `--target <host>` | 届かない | 届かない | 届かない |

- `RUSTFLAGS` は rustc にだけ効き、rustdoc には効かないので、文書化する crate 自身にはどちらでも届かない (`RUSTDOCFLAGS` の領分。R-WS-1 で使わないと決めている)
- `--target` を付けると、host 向けの build script と proc-macro にも届かなくなる。`RUSTFLAGS` や `build.rustflags` を使うプロジェクトでは、build.rs や proc-macro が生成する item が、利用者の普段のビルド (`--target` なし) と違ってくる

**B7-6 (`rust-std` が無い target)**: nightly の sysroot には `lib/rustlib/aarch64-unknown-linux-gnu` が無く (rustup の一覧とも一致)、`cargo +nightly rustdoc --target aarch64-unknown-linux-gnu` は `error[E0463]: can't find crate for std` で失敗した。stable (入っている) では成功した。

**B7-7 (実行ファイルの組み合わせ)**:

| 組み合わせ | 結果 |
| --- | --- |
| stable の cargo・rustc + nightly の rustdoc (`RUSTDOC`) | `error[E0786]: found invalid metadata files for crate` |
| stable の cargo・rustdoc + nightly の rustc (`RUSTC`) | 同じく `E0786` |
| nightly の cargo + stable の rustc・rustdoc | 成功 |
| rustup を通さない (PATH に toolchain の `bin` とシステムだけを置き、その cargo をパスで起動) | 成功 (PATH の rustc を使い、形式 61) |

rustc と rustdoc は同じ toolchain でないと、依存のメタデータが読めずに壊れる。cargo は別の toolchain でもよい。

**B7-8 (`build.target` に target の JSON 仕様)**: 合成の workspace の複製に、nightly の `--print target-spec-json` で作った host の仕様を `build.target` として書いた。

| | stable (crate 名を指定した `RUSTC_BOOTSTRAP`) | nightly |
| --- | --- | --- |
| `cargo tree` | `.json target specs require -Zjson-target-spec` | 同じ |
| `[unstable] json-target-spec = true` を足して `cargo tree` | 同じエラー (`[unstable]` が無視される。`RUSTC_BOOTSTRAP=1` なら通る) | 通る |
| 同じく `cargo rustdoc` | 同じエラー | `error[E0463]: can't find crate for std` (独自の target に std が無い。`-Z build-std` が要る) |

**決めたこと**:

- B7-1: target が host で、プロジェクトに `build.target` も無いときは、`--target` を付けない (利用者の普段のビルドと同じく、`RUSTFLAGS` が build script と proc-macro に届く)。JSON は `<target dir>/doc/` に出る (proc-macro の JSON と同じ場所)。target が host と違うときと `build.target` があるときだけ `--target` を付ける
- B7-6: cargo を起動する前に `<sysroot>/lib/rustlib/<target>` の有無を見て、無ければ `toolchain_missing` を返す (rustup の環境なら `rustup target add` を案内する)。cargo の `E0463` の文言は照合しない
- B7-7: 実行ファイルのパスを一部だけ指定されたら、残りは同じディレクトリから探す (rustc と rustdoc は同じ toolchain でなければならない。見つからなければ、起動時にログと hook で知らせ、照会ではエラーにする)。cargo は別の toolchain でもよい。rustup が無く指定も無ければ、PATH の cargo・rustc・rustdoc を使い、toolchain を切り替えない (Q-30 の案のとおり)
- B7-8: `build.target` が `.json` のプロジェクトは v1 では対応しない。cargo を起動せずに `docs_unavailable` (理由を書く) を返し、依存は docs.rs を、member は利用者が手元で作る方法を案内する。対応は要望があれば M2 以降

**v0.17 で要件に反映すること**: R-WS-1 の「`--target` は常に明示する」を、host で `build.target` も無いときは付けないことに改め、出力の場所の記述 (`<target dir>/<triple>/doc/` と `<target dir>/doc/`) を直す。R-WS-1・R-WS-8 の実行ファイルのパスの節と Q-30 の該当項目 (`RUSTFLAGS`・`rust-std` の判定・一部だけの指定・rustup の無い環境・target の JSON 仕様) を決定にする。

#### B7-3・B7-4 (2026-10-19)

環境: stable の cargo 1.99.1、git 2.56.0。スクリプトは [poc/m0b/b7-cargo-failures/run.sh](../poc/m0b/b7-cargo-failures/run.sh)。場面ごとに小さなパッケージを作業用のディレクトリに作り、`isolate_home` (`HOME`・git の設定・`CARGO_HOME` を空のディレクトリに向ける) と `LC_ALL=C` で実行した。crates.io・github.com・名前の引けないホスト (`unreachable.invalid`) に通信する。

**B7-3 (`cargo metadata --format-version 1` の失敗)**: 終了コードはどれも 101 (警告だけなら 0) で、見分けられるのは stderr の文言だけ。

| 場面 | 終了コード | 文言 |
| --- | --- | --- |
| Cargo.toml の文法の誤り | 101 | `error: key with no value, expected =` と ` --> Cargo.toml:8:6` |
| 知らないキー (`no-such-key = true`) | 0 | 警告だけ |
| 存在しない版 (`=999.0.0`) | 101 | `failed to select a version for the requirement` |
| レジストリに届かない (crates.io を置き換えて名前の引けないホストへ) | 101 | `unable to update registry` → `download of config.json failed` → `Could not resolve hostname` |
| git のホストに届かない | 101 | `failed to resolve address for unreachable.invalid … class=Net` |
| `--offline` でキャッシュに無い | 101 | `no matching package named itoa found` (crate 名の誤りと同じ文言) |

**B7-4 (git の依存の決定的な失敗)**: `cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update` の 4 つで文言は同じだった。

| 場面 | libgit2 (既定) | git の CLI (`net.git-fetch-with-cli`) |
| --- | --- | --- |
| rev が無い | `revision <rev> not found` (加えて `network failure seems to have happened`。libgit2 は通信の失敗とみなして再試行するので遅い) | `fatal: remote error: upload-pack: not our ref <rev>` と cargo の `revision <rev> not found` |
| ブランチが無い | `failed to find branch` と `cannot locate remote-tracking branch 'origin/<branch>'` | `fatal: couldn't find remote ref refs/heads/<branch>` |
| GitHub の https でリポジトリが無い | `failed to authenticate when downloading repository` | `fatal: could not read Username for 'https://github.com': terminal prompts disabled` |
| `file://` でリポジトリが無い | `failed to resolve path '…': No such file or directory` | `fatal: '…' does not appear to be a git repository` |

- **要件との食い違い**: Q-30 の「rev が無い → libgit2 も CLI も `revspec '…' not found`」(2026-10-16) は、今の cargo では `revision … not found` になっていた
- **地域化**: この環境の git には翻訳 (`git.mo`) が入っておらず、`LC_ALL` を変えても文言が変わらないので、地域化の影響は確かめられなかった
- **資格情報の問い合わせ**: 最初の実行では、git の CLI が GitHub の存在しないリポジトリで資格情報を求められ、利用者のデスクトップの `SSH_ASKPASS` (lxqt-openssh-askpass) を起動してダイアログを出した (`GIT_TERMINAL_PROMPT=0` は端末の問い合わせしか止めない。GitHub は存在しないリポジトリにも認証を求め、https のパスワードには個人アクセストークンしか受けないので、入れるべき値は無い)。実験の環境では `SSH_ASKPASS`・`GIT_ASKPASS` を外し、`SSH_ASKPASS_REQUIRE=never`・`GCM_INTERACTIVE=never` を付けて実行し直した (`env.sh`)
- 1 回の実行に約 5 分かかった (多くは libgit2 の再試行)

**決めたこと**:

- Q-30: `cargo metadata` の失敗は、どれも `build_failed` (`stage: metadata`、`retryable: true`) として cargo の診断を添えて返し、30 秒だけ否定キャッシュする (Q-30 の案のとおり。文言で分類しない)。キャッシュに無いことによる失敗は、サーバー自身が `--offline` を渡したときだけ `offline_miss` にする
- R-WS-7・Q-30: git の取得の決定的な失敗 (`fetch_failed`) は、上の表の文言 (libgit2 と CLI の両方) で rev・ブランチ・リポジトリが無い・認証の失敗を判定し (R-WS-7 で認めた照合の例外)、文言をテストで固定する。git を取得しうる子プロセス (`cargo fetch`・`cargo metadata`・`cargo tree`・`cargo update`) には `LC_ALL=C` を渡す
- R-WS-7: 子プロセスでは資格情報の問い合わせをすべて止める。`SSH_ASKPASS`・`GIT_ASKPASS` を外し、`GIT_TERMINAL_PROMPT=0`・`SSH_ASKPASS_REQUIRE=never`・`GCM_INTERACTIVE=never` を付ける。保存済みの資格情報 (`credential.helper`・ssh-agent) はそのまま使われ、無ければ問い合わせずに `fetch_failed` (`reason: auth`) になり、既存の案内 (利用者の端末で `cargo fetch` を実行して資格情報を通し、`refresh` で再試行) を返す。cargo の既定の経路 (libgit2) も人に問い合わせない (`credential.helper` と ssh-agent だけを試す) ので、それにそろえる。いったん「GUI の askpass は許す」と決めたが、次の理由で改めた: `net.git-fetch-with-cli` の経路では cargo が git の CLI の失敗を一時的な失敗として `net.retry` の回数だけやり直すので、1 回の照会でダイアログが何度も出うる。エージェントの照会のついでに予期せず出て、入力を待つあいだ照会が止まる。存在しない GitHub のリポジトリ (URL の誤り) でも、入れる値の無いダイアログが出る (cargo 1.99.1 のソース `src/sources/git/utils.rs` の `with_authentication` と `fetch_with_cli` で確かめた)

**v0.17 で要件に反映すること**: Q-30 の該当項目 (`cargo metadata` の失敗の返し方、git の文言と `LC_ALL=C`) を決定にし、rev の文言を直す。R-WS-7 に、子プロセスの資格情報の問い合わせをすべて止めること (外す変数と付ける変数) と、その理由 (cargo の既定の経路も問い合わせない、CLI の経路では再試行でダイアログが繰り返される、存在しない GitHub のリポジトリでも出る) を書く。

---

## 7. M0 の終え方

1. §6 の結果から、要件に反映する決定の一覧を作る。判断が要るものは 1 件ずつ決める
2. requirements.md を v0.17 にする。決まった未決事項を §11.1 に移し、本文 (R-FV-3・R-WS-1・R-WS-4・R-WS-8・R-FT-3・R-MCP-3・R-MCP-16・R-OUT-2・§10 など) を直し、§14 に経緯を書く。先送りしていた v0.15 以降の全体の再レビューは、このときに合わせて行う
3. M1 の計画を立てる
