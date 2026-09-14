#!/usr/bin/env python3
"""`tako context-budget check --json` から master system prompt の実寸を 1 行で出す。

#1477 の実経路テストが A/B（`TAKO_1477_LEGACY`）の前後を比べるのに使う。
**内訳は tako が返した `pieces` をそのまま足す**（このスクリプトは数え直さない）。
"""

import json
import sys


def main() -> int:
    with open(sys.argv[1], encoding="utf-8") as f:
        report = json.load(f)
    for item in report["items"]:
        if item["kind"] != "system_prompt" or "master" not in item["path"]:
            continue
        pieces = item.get("pieces", [])
        base = sum(p["bytes"] for p in pieces if not p["name"].startswith("append"))
        append = sum(p["bytes"] for p in pieces if p["name"].startswith("append"))
        print(
            f"violations={len(item.get('violations', []))} "
            f"total={item['bytes']} base={base} append={append}"
        )
        names = [p["name"] for p in pieces if p["name"].startswith("append")]
        print("pieces=" + ",".join(names))
    return 0


if __name__ == "__main__":
    sys.exit(main())
