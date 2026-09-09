#!/usr/bin/env bash
# clean-trust-residue.sh — テスト由来の事前信頼エントリを claude の設定から掃除する（#1030 / #1022 / #944）
#
# tako のテスト・セルフテストは spawn の前に claude の `.claude.json` へ
# 「このフォルダを信頼する」エントリを書く（#32 の事前信頼）。書き先が本番だった時代に
# **一時ディレクトリを指すエントリ**が数千件積もった。実害は無い（存在しないパスなので
# claude は無視する）が、ユーザーの生きた設定ファイルが読めなくなるので掃除する。
#
# 書き先そのものは #944 で塞いだ（`cargo test` は隔離先へ書く）。これは**過去の残骸**専用。
#
# 使い方:
#   bash scripts/clean-trust-residue.sh            # 既定は dry-run（一覧を出すだけ）
#   bash scripts/clean-trust-residue.sh --apply    # 実際に消す（.bak を残す）
#   bash scripts/clean-trust-residue.sh --json     # 機械可読の要約
#
# 対象は `$HOME/.claude.json` と `$HOME/.claude/.claude.json`（`HOME` を差し替えれば検証できる）。
#
# 安全側の判定（**両方**を満たすものだけ消す）:
#   1. キーが一時ディレクトリの下にある（`/tmp` `/private/tmp` `/var/folders/**/T` `$TMPDIR`）
#   2. tako のテストが作る名前に一致する（`tako-selftest-*` / `tako-e2e-*` / `tako-<数字>-*` 等）
# 一時ディレクトリの下でも名前が一致しないものは「要確認」として**消さずに**並べる。
# 実プロジェクトのパス（ホーム配下の作業ディレクトリ等）は判定に入らない。
set -euo pipefail

APPLY=0
JSON=0
for arg in "$@"; do
  case "$arg" in
    --apply) APPLY=1 ;;
    --json) JSON=1 ;;
    -h | --help)
      sed -n '2,25p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "不明な引数: $arg" >&2
      exit 2
      ;;
  esac
done

/usr/bin/env python3 - "$APPLY" "$JSON" <<'PYEOF'
import json, os, re, sys, shutil, time
from collections import Counter

apply_changes = sys.argv[1] == "1"
as_json = sys.argv[2] == "1"

home = os.path.expanduser("~")
targets = [os.path.join(home, ".claude.json"), os.path.join(home, ".claude", ".claude.json")]

# 一時ディレクトリの根（ここより下だけが掃除の候補）
tmp_roots = ["/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/"]
tmpdir = os.environ.get("TMPDIR")
if tmpdir:
    tmp_roots.append(tmpdir if tmpdir.endswith("/") else tmpdir + "/")

# tako のテスト・セルフテストが作るディレクトリ名
name_patterns = [
    re.compile(r"^tako-selftest-\d+-\d+$"),   # GUI セルフテスト（項目番号-pid）
    re.compile(r"^tako-e2e-[\w.-]+$"),        # 実 claude の e2e（#571 / #577 / #32）
    re.compile(r"^tako-\d+-[\w.-]+$"),        # 単体テストの一時 dir（tako-1055-prev-<pid> 等）
    re.compile(r"^tako-t\d+-\d+$"),           # tako-t558-<pid>
    re.compile(r"^tako-(stale|trust|test)[\w.-]*$"),
    re.compile(r"^tako-\d+-fakehome-[\w.-]+$"),
]

def under_tmp(path: str) -> bool:
    return any(path.startswith(root) for root in tmp_roots)

def matched_component(path: str):
    """パスの成分のうち tako のテスト名に一致した最初のもの（数字は N に丸める）。
    `.../tako-1055-prof-<pid>/want` のように末尾が作業用の名前でも、
    上位の成分でテスト由来と分かる"""
    for part in path.strip("/").split("/"):
        for pat in name_patterns:
            if pat.match(part):
                # `e2e` の 1 桁は残し、pid や項目番号（2 桁以上）だけ丸める
                return re.sub(r"\d{2,}", "N", part)
    return None


def looks_like_test_dir(path: str) -> bool:
    return matched_component(path) is not None

report = {"files": [], "removable_total": 0, "review_total": 0, "applied": apply_changes}

for path in targets:
    entry = {"path": path.replace(home, "~"), "exists": os.path.exists(path)}
    if not entry["exists"]:
        report["files"].append(entry)
        continue
    try:
        with open(path, encoding="utf-8") as fh:
            root = json.load(fh)
    except Exception as exc:  # 壊れた JSON には触らない
        entry["error"] = f"読めない: {exc}"
        report["files"].append(entry)
        continue
    projects = root.get("projects")
    if not isinstance(projects, dict):
        entry["error"] = "projects がオブジェクトでない"
        report["files"].append(entry)
        continue

    removable, review = [], []
    for key in projects:
        if not under_tmp(key):
            continue
        (removable if looks_like_test_dir(key) else review).append(key)

    families = Counter()
    for key in removable:
        families[matched_component(key)] += 1

    entry.update({
        "total": len(projects),
        "removable": len(removable),
        "review": len(review),
        "families": dict(families.most_common()),
        "kept": len(projects) - len(removable),
    })
    report["removable_total"] += len(removable)
    report["review_total"] += len(review)

    if apply_changes and removable:
        backup = f"{path}.residue-backup.{int(time.time())}"
        shutil.copy2(path, backup)
        for key in removable:
            projects.pop(key, None)
        tmp = f"{path}.tako-tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump(root, fh, indent=2, ensure_ascii=False)
        os.replace(tmp, path)
        entry["backup"] = backup.replace(home, "~")
    review_groups = Counter()
    for key in review:
        parts = key.strip("/").split("/")
        review_groups["/" + "/".join(parts[:3])] += 1
    entry["review_groups"] = {
        k.replace(home, "~"): v for k, v in review_groups.most_common(10)
    }
    report["files"].append(entry)

if as_json:
    print(json.dumps(report, ensure_ascii=False, indent=2))
    sys.exit(0)

mode = "実削除" if apply_changes else "dry-run（何も消していない）"
print(f"テスト由来の事前信頼エントリの掃除 — {mode}")
for entry in report["files"]:
    print(f"\n[{entry['path']}]")
    if not entry.get("exists"):
        print("  ファイルが無い")
        continue
    if "error" in entry:
        print(f"  {entry['error']}")
        continue
    print(f"  projects 合計 {entry['total']} 件 / 掃除対象 {entry['removable']} 件 / 残す {entry['kept']} 件")
    for name, count in entry["families"].items():
        print(f"    {count:6d}  {name}")
    if entry["review"]:
        print(f"  要確認（一時ディレクトリ配下だがテスト名に一致しない・消さない）: {entry['review']} 件")
        for key, count in entry["review_groups"].items():
            print(f"    {count:6d}  {key}/…")
    if "backup" in entry:
        print(f"  退避: {entry['backup']}")

print(f"\n掃除対象の合計: {report['removable_total']} 件 / 要確認 {report['review_total']} 件")
if not apply_changes and report["removable_total"]:
    print("実際に消すには --apply を付けて実行する（元ファイルは .residue-backup.<epoch> へ退避される）")
PYEOF
