# A3: Claude Code から仮のツールを呼ぶ実験

[doc/m0-plan.md](../../doc/m0-plan.md) の A3 で使う。結果は同じ文書の §6.1 に書く。

## 仕組み

- `.mcp.json` で、このディレクトリのサーバーを `chatora-rustdoc` という名前で登録する (利用者の設定には登録しない)
- `.claude/settings.json` で、そのサーバーを有効にし、ツールの呼び出しを許可する
- `run-server.sh` は `../../target/debug/chatora-rustdoc-mcp serve` を起動し、やりとりを `logs/` に写す
  - `<時刻>-<PID>.in.jsonl`: Claude Code から受けた行
  - `<時刻>-<PID>.out.jsonl`: サーバーが返した行
  - `<時刻>-<PID>.stderr.log`: サーバーのログ (debug)

先にリポジトリのルートで `cargo build` しておく。

## headless で確かめる (R-OUT-1・R-ERR-2)

このディレクトリで実行し、出力の `tool_result` を見る。

```sh
claude -p 'Call the mcp__chatora-rustdoc__probe_result tool once with {"is_error": false}. Then reply with exactly the tool result you received, verbatim, and nothing else.' --output-format stream-json --verbose
```

`{"is_error": true}` でも同じように実行する。

## 対話モードで確かめる (R-WS-6)

1. このディレクトリで `claude` を起動する
2. 次のように頼む: `Call mcp__chatora-rustdoc__sleep with {"seconds": 20}.`
3. 実行中の表示に進捗 (`slept 3 of 20 seconds` など) が出るかを見る
4. 10 秒ほどで Esc を押して止める
5. `/exit` で終える

その後、`logs/` の最新の `*.in.jsonl` に `notifications/cancelled` があるか、`*.out.jsonl` で進捗が止まり、`sleep` の結果が返っていないかを見る。
