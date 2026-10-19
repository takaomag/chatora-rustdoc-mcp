#!/usr/bin/env python3
"""schemastore の JSON Schema を取得し、外部ファイルへの `$ref` を埋め込んで保存する。

taplo 0.10.0 は別ファイルを指す `$ref` を取りに行かない。schemastore の `cargo.json` は
`[lints]` の中身を `cargo-lints-rust.json` などへの `$ref` で定義しているので、参照先が
解決できずに `[lints]` 全体が `is not valid under any of the schemas listed in the 'anyOf'
keyword` になる。参照先を root の `definitions` (`$defs` があればそちら) に取り込んだ 1 ファイルにすれば taplo で検証できる。

使い方: `python3 .taplo/update-schemas.py` (ネットワークが必要。実行場所は問わない)。
"""

import json
import pathlib
import urllib.parse
import urllib.request

# (保存するファイル名, 取得元)
SCHEMAS = [
    ("cargo.json", "https://www.schemastore.org/cargo.json"),
]

OUT_DIR = pathlib.Path(__file__).resolve().parent / "schemas"
USER_AGENT = "chatora-rustdoc-mcp/update-schemas (https://github.com/takaomag/chatora-rustdoc-mcp)"


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def defs_key(schema):
    return "$defs" if "$defs" in schema else "definitions"


def prefix_for(url):
    """参照先ファイルの定義名に付ける接頭辞。`.../cargo-lints-rust.json` → `cargo-lints-rust__`。"""
    stem = pathlib.PurePosixPath(urllib.parse.urlsplit(url).path).stem
    return f"{stem}__"


def inline(root_url):
    root = fetch(root_url)
    root_defs = root.setdefault(defs_key(root), {})
    root_defs_key = defs_key(root)
    loaded = set()  # 取り込み済みの参照先 URL

    def local_ref(name):
        return f"#/{root_defs_key}/{name}"

    def rewrite_internal(node, prefix, sub_defs_key):
        """参照先ファイル内部の `#/definitions/X` を、取り込み後の名前に書き換える。"""
        if isinstance(node, dict):
            result = {}
            for key, value in node.items():
                head = f"#/{sub_defs_key}/"
                if key == "$ref" and isinstance(value, str) and value.startswith(head):
                    result[key] = local_ref(prefix + value[len(head) :])
                elif key == "$ref" and value == "#":
                    result[key] = local_ref(prefix + "ROOT")
                else:
                    result[key] = rewrite_internal(value, prefix, sub_defs_key)
            return result
        if isinstance(node, list):
            return [rewrite_internal(item, prefix, sub_defs_key) for item in node]
        return node

    def load(url):
        if url in loaded:
            return
        loaded.add(url)
        sub = fetch(url)
        prefix = prefix_for(url)
        sub_defs_key = defs_key(sub)
        for name, body in sub.get(sub_defs_key, {}).items():
            root_defs[prefix + name] = rewrite_internal(body, prefix, sub_defs_key)
        body = {k: v for k, v in sub.items() if k not in (sub_defs_key, "$schema", "$id")}
        root_defs[prefix + "ROOT"] = rewrite_internal(body, prefix, sub_defs_key)

    def resolve_remote(node):
        """外部 URL を指す `$ref` を、取り込んだ定義への参照に置き換える。"""
        changed = False
        if isinstance(node, dict):
            ref = node.get("$ref")
            if isinstance(ref, str) and ref.startswith(("http://", "https://")):
                url, _, fragment = ref.partition("#")
                load(url)
                prefix = prefix_for(url)
                if fragment:
                    name = fragment.rsplit("/", 1)[-1]
                    node["$ref"] = local_ref(prefix + name)
                else:
                    node["$ref"] = local_ref(prefix + "ROOT")
                changed = True
            # load() が root の definitions に追記するので、反復前に複製する
            for value in list(node.values()):
                changed |= resolve_remote(value)
        elif isinstance(node, list):
            for item in node:
                changed |= resolve_remote(item)
        return changed

    # 取り込んだ定義がさらに外部を参照していることがあるので、無くなるまで繰り返す
    while resolve_remote(root):
        pass
    return root


def main():
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name, url in SCHEMAS:
        schema = inline(url)
        path = OUT_DIR / name
        path.write_text(json.dumps(schema, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"wrote {path.relative_to(OUT_DIR.parent.parent)} from {url}")


if __name__ == "__main__":
    main()
