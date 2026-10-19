# A3・A9: Claude Code からツールを呼ぶ実験

[doc/m0-plan.md](../../doc/m0-plan.md) の A3 (仮のツール) と A9 (`get_item`) で使う。結果は同じ文書の §6.1 に書く。A3 の `probe_result` と `echo` は A8 で消したので、A3 の headless の手順は今のサーバーでは再現できない。

## 仕組み

- `.mcp.json` で、このディレクトリのサーバーを `chatora-rustdoc-mcp` という名前で登録する (利用者の設定には登録しない)
- `.claude/settings.json` で、そのサーバーを有効にし、ツールの呼び出しを許可する
- `run-server.sh` は `../../target/debug/chatora-rustdoc-mcp serve` を起動し、やりとりを `logs/` に写す
  - `<時刻>-<PID>.in.jsonl`: Claude Code から受けた行
  - `<時刻>-<PID>.out.jsonl`: サーバーが返した行
  - `<時刻>-<PID>.stderr.log`: サーバーのログ (debug)

先にリポジトリのルートで `cargo build` しておく。

## headless で確かめる (R-OUT-1・R-ERR-2)

このディレクトリで実行し、出力の `tool_result` を見る。

```sh
claude -p 'Call the mcp__chatora-rustdoc-mcp__probe_result tool once with {"is_error": false}. Then reply with exactly the tool result you received, verbatim, and nothing else.' --output-format stream-json --verbose
```

`{"is_error": true}` でも同じように実行する。

## 対話モードで確かめる (R-WS-6)

1. このディレクトリで `claude` を起動する
2. 次のように頼む: `Call mcp__chatora-rustdoc-mcp__sleep with {"seconds": 20}.`
3. 実行中の表示に進捗 (`slept 3 of 20 seconds` など) が出るかを見る
4. 10 秒ほどで Esc を押して止める
5. `/exit` で終える

その後、`logs/` の最新の `*.in.jsonl` に `notifications/cancelled` があるか、`*.out.jsonl` で進捗が止まり、`sleep` の結果が返っていないかを見る。

## A9: `get_item` を呼ぶ (headless)

このディレクトリで、次の問いをそれぞれ実行し、出力の `tool_use` (引数) と `tool_result` (モデルに渡った中身) を見る。サーバーは docs.rs から取った JSON を利用者のキャッシュに置く (`~/.cache/chatora-rustdoc-mcp/`)。

```sh
claude -p '<問い>' --output-format stream-json --verbose
```

1. 成功の結果 (R-OUT-1): `Call the mcp__chatora-rustdoc-mcp__get_item tool once with {"crate": "tokio@1.53.3", "path": "tokio::sync::Mutex::lock"}. Then reply with exactly the tool result you received, verbatim, and nothing else.`
2. エラーの結果 (R-ERR-2): 1 と同じで、引数を `{"crate": "tokio@1.0.0", "path": "tokio::spawn"}` にする
3. 版を指定した問い: `In axum 0.8.9, what arguments does Router::route take? Answer using the chatora-rustdoc-mcp tools.`
4. 別の crate の re-export: `In axum 0.8.9, where is debug_handler defined and what is its signature? Answer using the chatora-rustdoc-mcp tools.`
5. 版の無い問い: `What is the signature of tokio's spawn function? Answer using the chatora-rustdoc-mcp tools.`

3〜5 では、ツール名も引数も渡さない。説明文だけで正しい引数を作れるか、エラーや hint からどう続けるかを見る (eval は M5)。
