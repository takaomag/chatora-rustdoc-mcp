# eval

[doc/m1-plan.md](../doc/m1-plan.md) の E1 (§5 の P5、要件 R-QA-6) の小さな eval。[cases.json](cases.json) の問いを、このサーバーのツールだけを持つ headless の Claude Code (`claude -p --output-format stream-json`) に 1 問ずつ尋ね、選んだツール・呼び出しの回数・`tool_result` の大きさ・トークン・費用・答えが正しいかを記録する。

**回すたびに API の利用料がかかる** (1 問あたり `--budget-usd` まで。既定は 1 ドル)。手で回す。

```sh
python3 -I eval/run.py                       # 全問
python3 -I eval/run.py --case q8-send        # 1 問だけ
python3 -I eval/run.py --model sonnet        # モデルを指定する
```

- サーバーは `cargo build --release --locked` で作り直してから使う。キャッシュは普段と同じ場所 (`CHATORA_RUSTDOC_MCP_CACHE__DIR` があればそこ)、ログは結果のディレクトリの `logs/`
- 問いの「このプロジェクト」は [fixture](fixture) (版を testdata に合わせて固定した workspace。tokio の `net` はわざと有効にしない)。セッションはそこで動く
- 組み込みのツール (Read・Bash など) は外し、ほかの MCP サーバーも読まない (`--strict-mcp-config`)。利用者の設定 (hook・許可・`env`) も読まない (`--setting-sources project`)。利用者の CLAUDE.md は読まれうる
- 結果は `eval/results/<時刻>/` に残る (Git には入れない): 問いごとの stream-json、`summary.json`、`summary.md` (表)
- 各問いの `tools` は、どれか 1 つを呼べば良いツール、`answer` は答えに含まれるべき正規表現 (大文字小文字は問わない。利用者の CLAUDE.md で答えが日本語になりうるので、日本語の言い方も入れる)。`shape` の問いは、モデルに渡った最初の `tool_result` の形を確かめる: `structured` は `structuredContent` (provenance と data) が、`error` はエラーの JSON (code と request_id) が渡っていること (要件 R-OUT-1・R-ERR-2。Claude Code の版を上げたら回す)
