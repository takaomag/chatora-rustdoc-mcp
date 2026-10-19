# 追っている Claude Code の Issue

- 最終更新: 2026-10-16
- 目的: このプロジェクトの要件・README が前提にしている Claude Code の不具合を 1 か所にまとめる。直ったときに、どこを直せばよいかが分かるようにする。ask のルールの効き方の実測もここに置く (requirements.md の R-SEC-2 から参照している)
- 状態は最終更新の時点のもの。確かめるときは `gh issue view <番号> --repo anthropics/claude-code` を使う

| Issue | 内容 | 状態 | 私たちの関わり | 直ったときに直すところ |
| --- | --- | --- | --- | --- |
| [#95741](https://github.com/anthropics/claude-code/issues/95741) | `bypassPermissions` で ask のルールが効かない | open | 2026-10-16 に MCP ツールでの再現をコメントし、👍 を付けた ([コメント](https://github.com/anthropics/claude-code/issues/95741#issuecomment-6211249052)) | README の Security の表と案内、requirements.md の R-SEC-2 (「README に書くこと」の許可ルールの書き方)、この文書の「ask のルールの効き方」、セキュリティの一覧ページ (A2 の cons とまとめ) |
| [#83766](https://github.com/anthropics/claude-code/issues/83766) | auto モードで ask のルールが効かない | open | 2026-10-16 に、VS Code の auto でだけ起きる (端末の auto では確認が出る) ことをコメントし、👍 を付けた ([コメント](https://github.com/anthropics/claude-code/issues/83766#issuecomment-6211249684)) | 同上 |
| [#98256](https://github.com/anthropics/claude-code/issues/98256) | VS Code 拡張が elicitation を宣言するのに、確認画面を出さず `decline` を返す | open | 2026-10-16 に VS Code の `/bug` で報告し、2.1.301 での再現をコメントし、👍 を付けた ([コメント](https://github.com/anthropics/claude-code/issues/98256#issuecomment-6207252759))。端末では確認画面が出て、headless は即座に `cancel` を返す | なし (v0.14 で確認をやめたので、今の要件には影響しない。requirements.md の §14 の v0.14 とセキュリティの一覧ページの F にある記述は経緯として残す。MRTR で確認を求め直すときに再確認する) |
| [#79944](https://github.com/anthropics/claude-code/issues/79944) | `structuredContent` があると、text の content がモデルに渡らない | open | なし (2.1.301 で再現を確かめた) | requirements.md の R-OUT-1 (§7.1。出力の形式の前提) と、それを理由にする R-OUT-8 (`resource_link` に頼らない)、§11.1 の Q-1、§13 の Issue の行、付録 A の Claude Code の行 |
| [#86032](https://github.com/anthropics/claude-code/issues/86032) | エラーの結果で `structuredContent` が捨てられる | open | なし (2.1.301 で確かめた。text のブロックは改行でつないで渡る) | requirements.md の R-ERR-2、R-OUT-1 (エラーの結果に `structuredContent` を付けない理由)、§11.1 の Q-1、§13 の Issue の行、付録 A の Claude Code の行 |

## ask のルールの効き方 (#95741・#83766)

Claude Code の文書 ([permission-modes](https://code.claude.com/docs/en/permission-modes)) は、「明示的な ask のルールに当たるツールは、`bypassPermissions` を含むどのモードでも自動では許可しない」と書いている。実際の挙動は次のとおりで、2 か所が文書と食い違う (2.1.301、Linux、2026-10-16)。

| 環境 | 権限モード | ask に書いた MCP ツール |
| --- | --- | --- |
| 端末 | Manual・`acceptEdits`・auto・plan (計画の承認の後も) | 確認が出る |
| 端末 | `bypassPermissions` | **確認なしで実行される** |
| VS Code | Manual | 確認が出る |
| VS Code | auto | **確認なしで実行される** |
| headless (`claude -p`) | `--allowedTools`・`bypassPermissions`・`dontAsk`・hook の `allow` のどれでも | 実行されない (拒否される) |

このプロジェクトは、外のコードをビルドする `build_external_docs` の前の確認を、利用者の ask のルールに任せている (R-SEC-2)。そのため README で、確認が要るなら `bypassPermissions` を使わず、VS Code では Manual を使うよう案内している。直ったら、冒頭の表の「直ったときに直すところ」に挙げた各所で、該当する行を「確認が出る」に直し、`bypassPermissions` と VS Code の auto を避ける案内を消す。

## 再確認のしかた

Python の標準ライブラリだけで書いた小さな stdio の MCP サーバーで確かめた。サーバーは MCP 2026-07-28 の `server/discover` と `tools/list` に応え、`tools/call` を受けたらツール名と引数をログに書いて短い文字列を返すだけでよい (70 行ほど)。ツールは 2 つ用意する。

- `lookup`: 引数なし。許可ルールの `allow` にだけ当たる
- `build_external_docs`: 引数 `git` (文字列)。`ask` にも当たる

そのサーバーを置いたディレクトリに、次の 2 つのファイルを置く。

- `.mcp.json`: サーバーを `poc5` などの名前で登録する
- `.claude/settings.json`: 次の内容にする

```json
{
  "enableAllProjectMcpServers": true,
  "permissions": {
    "allow": ["mcp__poc5__*"],
    "ask": ["mcp__poc5__build_external_docs"]
  }
}
```

各環境とモードで `build_external_docs` を呼ばせ、確認の画面が出たかと、サーバーのログに呼び出しが残ったかを見る。VS Code では、Manual で `lookup` が確認なしに動くことも確かめる (プロジェクトの設定が読まれている証拠になる)。headless では `--output-format json` の結果で拒否されたかが分かる。

`structuredContent` とエラーの結果 (#79944・#86032) は、headless で `--output-format stream-json --verbose` を付けて実行すると、モデルに渡った `tool_result` の中身をそのまま見られる。
