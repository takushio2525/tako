#!/usr/bin/env python3
"""`tako context-budget check --json` から system prompt の超過と助言を書き出す。

#1477 の受け入れ条件 4（分割後も常時部が予算を超えるときに「区切りを何行上げるか」が
**具体値つき**で返る）を実経路で確かめるために使う。
"""

import json
import sys


def main() -> int:
    with open(sys.argv[1], encoding="utf-8") as f:
        report = json.load(f)
    for item in report["items"]:
        if item["kind"] != "system_prompt" or "master" not in item["path"]:
            continue
        for v in item.get("violations", []):
            print(
                f"metric={v['metric']} actual={v['actual']} limit={v['limit']} "
                f"move_lines={v.get('move_lines')}"
            )
            print("advice=" + str(v.get("advice_ja")))
    for p in report.get("proposals", []):
        if "master" in p.get("path", ""):
            print("proposal_move_lines=" + str(p.get("move_lines")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
