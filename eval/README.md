# eval

[doc/m1-plan.md](../doc/m1-plan.md) の E1 (§5 の P5、要件 R-QA-6) と [doc/m2-plan.md](../doc/m2-plan.md) の M2f の F4a の小さな eval。[cases.json](cases.json) の問いを headless の Claude Code (`claude -p --output-format stream-json`) に 1 問ずつ尋ね、選んだツール・最初のツール・組み込みのツールの呼び出し・呼び出しの回数・`tool_result` の大きさ・トークン・費用・答えが正しいかを記録する。

**回すたびに API の利用料がかかる** (1 問あたり `--budget-usd` まで。既定は 1 ドル)。手で回す。

```sh
python3 -I eval/run.py                       # 全問
python3 -I eval/run.py --case q8-send        # 1 問だけ
python3 -I eval/run.py --model sonnet        # モデルを指定する
python3 -I eval/run.py --dry-run             # 各問いのプロジェクトを作って下準備だけをし、claude の引数を表示する (利用料はかからない)
```

- サーバーは `cargo build --release --locked` で作り直してから使う。キャッシュは普段と同じ場所 (`CHATORA_RUSTDOC_MCP_CACHE__DIR` があればそこ)、ログは結果のディレクトリの `logs/`
- 問いの「このプロジェクト」は [fixture](fixture) (版を testdata に合わせて固定した workspace。tokio の `net` はわざと有効にしない。自前の型 `Config`・trait `Greeter`・private な関数を持つ)。問いごとに結果のディレクトリの `cases/<問い>/fixture` に写し、それぞれ git のリポジトリにして (1 回コミットする)、そこでセッションを動かす
- 既定では組み込みのツールを外し (`--tools ""`)、ほかの MCP サーバーも読まない (`--strict-mcp-config`)。利用者の設定 (hook・許可・`env`) も読まない (`--setting-sources project`)。`"builtin": true` の問いは、モデルが grep や LSP でなくこのサーバーを選ぶかを見るため、Read・Grep・Glob・LSP・ToolSearch を使えるようにし、LSP の server を出す利用者のプラグインのために利用者の設定も読む (`--setting-sources user,project`。Bash は外す)。利用者の CLAUDE.md と `~/.claude/rules` はどちらでも読まれる (`--bare` はそれらを外すが、LSP と利用者のログインも外れる)。結果を読むときはそのことに注意する
- 結果は `eval/results/<時刻>/` に残る (Git には入れない): 問いごとの stream-json、`summary.json`、`summary.md` (表)
- 問いの項目: `prompt` の `{fixture}`・`{other}` (tests/fixtures/other-workspace の写し)・`{worktree}` (fixture の写しの git worktree) は、その問いのディレクトリに置き換わる。`env` はサーバーに渡す環境変数 (`CHATORA_RUSTDOC_MCP_BUILD__WAIT_SECS` を 1 にしてビルド中を返させるなど)。`setup` はセッションの前の下準備で、`"build"` (別のサーバーで member を先にビルドする)、`{"write": [ファイル, 中身]}`・`{"append": [ファイル, 中身]}`・`{"replace": [ファイル, 前, 後]}` (fixture の写しを書き換える。5 秒待つ build.rs を書けば作り直しが待ちを越え、stale やビルド中になる) を順に行う。`retry` は `status: building` の後に呼び直したかを、`arguments` は照会のツールに渡った引数 (`{"project": "{worktree}"}` など) を確かめる。`repeat` は同じ問いを何度か尋ねる
- 各問いの `tools` は、どれか 1 つを呼べば良いツール、`answer` は答えに含まれるべき正規表現 (大文字小文字は問わない。利用者の CLAUDE.md で答えが日本語になりうるので、日本語の言い方も入れる)。`shape` の問いは、モデルに渡った最初の `tool_result` の形を確かめる: `structured` は `structuredContent` (provenance と data) が、`error` はエラーの JSON (code と request_id) が渡っていること (要件 R-OUT-1・R-ERR-2。Claude Code の版を上げたら回す)
