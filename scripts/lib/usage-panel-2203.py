#!/usr/bin/env python3
# usage-panel-2203.py — scripts/test-usage-panel-2203.sh の素材作りと照合（#2203）
#
#   gen-main  <cfg> <sessions.yaml> <projects.yaml> <now>   偽の会話記録・カタログ・プロジェクト
#   gen-huge  <cfg> <projects.yaml> <now> <会話の数>         数万の会話（1 本 1 応答・50 プロジェクト）
#   limits    <login> <now> <mode>                           偽の観測（limits.jsonl の行。mode = main / huge）
#   break     <usage dir>                                    索引と観測へ読めない行を混ぜる
#   check     <visual.log> <cli dir>                         画面の行と CLI の答えの照合（表を出す）
#
# 会話の本文・パスは偽（/home/testuser）。tako の書式（`tako_core::usage::display`）はここで
# 別に書き直してあり、画面の文字とも CLI の文字とも突き合わせる（同じ関数の写しではない照合）。
import datetime
import json
import os
import sys


def iso_ago(now, minutes):
    t = datetime.datetime.fromtimestamp(now - minutes * 60, datetime.timezone.utc)
    return t.strftime("%Y-%m-%dT%H:%M:%S.000Z")


def line(msg_id, sid, now, minutes, output, cache_read, cwd, model="claude-opus-5-5"):
    return json.dumps(
        {
            "type": "assistant",
            "message": {
                "id": msg_id,
                "model": model,
                "role": "assistant",
                "content": [{"type": "text", "text": "本文"}],
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": output,
                    "cache_read_input_tokens": cache_read,
                    "speed": "standard",
                },
            },
            "requestId": "req_" + msg_id,
            "timestamp": iso_ago(now, minutes),
            "sessionId": sid,
            "cwd": cwd,
            "uuid": "u-" + msg_id,
        },
        ensure_ascii=False,
        # Claude Code の記録と同じ詰めた区切り（索引の読み手は `"type":"assistant"` の部分一致で先に絞る）
        separators=(",", ":"),
    )


def flat(cwd):
    return cwd.replace("/", "-")


def write_session(cfg, sid, cwd, lines):
    d = os.path.join(cfg, "projects", flat(cwd))
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, sid + ".jsonl"), "w") as f:
        for l in lines:
            f.write(l + "\n")


HOME = "/home/testuser"
ALPHA = HOME + "/dev/alpha"
BETA = HOME + "/dev/beta"
MISC = HOME + "/misc"


def gen_main(cfg, sessions_yaml, projects_yaml, now):
    os.makedirs(cfg, exist_ok=True)
    with open(os.path.join(cfg, ".claude.json"), "w") as f:
        f.write('{"oauthAccount":{"accountUuid":"aaaaaaaa-2203-4222-8333-444444444444"}}\n')
    # (会話, cwd, [(何分前, 出力, 読み出し)])
    plan = [
        ("s-a1-2201", ALPHA, [(300, 200000, 2000000), (200, 100000, 2000000), (100, 150000, 2000000)]),
        ("s-a2-2202", ALPHA, [(250, 120000, 1000000), (90, 80000, 1000000)]),
        ("s-a3-2202", ALPHA, [(60, 50000, 500000)]),
        ("s-m1-alpha", HOME, [(280, 30000, 3000000), (150, 30000, 3000000), (20, 30000, 3000000)]),
        ("s-b1-beta", BETA, [(240, 60000, 0), (70, 40000, 0), (8, 20000, 0)]),
        ("s-u1-misc", MISC, [(130, 25000, 0)]),
    ]
    n = 0
    for sid, cwd, rows in plan:
        lines = []
        for minutes, output, read in rows:
            n += 1
            lines.append(line("msg_2203_%03d" % n, sid, now, minutes, output, read, cwd))
        write_session(cfg, sid, cwd, lines)
    stamp = "2026-10-20T00:00:00Z"
    with open(sessions_yaml, "w") as f:
        f.write("entries:\n")
        for sid, kind, extra in [
            ("s-a1-2201", "worker", 'label: "#2201 索引"\n    project: alpha'),
            ("s-a2-2202", "worker", 'label: "#2202 区間按分"\n    project: alpha'),
            ("s-a3-2202", "worker", 'label: "#2202 続き"\n    project: alpha'),
            ("s-m1-alpha", "master", "profile: alpha"),
        ]:
            f.write(
                "  %s:\n    kind: %s\n    %s\n    started_at: \"%s\"\n    last_seen_at: \"%s\"\n"
                % (sid, kind, extra, stamp, stamp)
            )
    with open(projects_yaml, "w") as f:
        f.write("projects:\n  alpha:\n    cwd: %s\n  beta:\n    cwd: %s\n" % (ALPHA, BETA))


def gen_huge(cfg, projects_yaml, now, count):
    os.makedirs(cfg, exist_ok=True)
    with open(os.path.join(cfg, ".claude.json"), "w") as f:
        f.write('{"oauthAccount":{"accountUuid":"bbbbbbbb-2203-4222-8333-444444444444"}}\n')
    projects = ["p%02d" % i for i in range(50)]
    with open(projects_yaml, "w") as f:
        f.write("projects:\n")
        for p in projects:
            f.write("  %s:\n    cwd: %s/dev/%s\n" % (p, HOME, p))
    for i in range(count):
        p = projects[i % len(projects)]
        cwd = "%s/dev/%s" % (HOME, p)
        sid = "h-%06d" % i
        # 直近 6 日に散らす（7d の窓の中）
        minutes = 5 + (i * 7919) % (6 * 24 * 60)
        write_session(cfg, sid, cwd, [line("msg_h_%06d" % i, sid, now, minutes, 1000 + i % 977, 10000, cwd)])


def limits(login, now, mode):
    def obs(kind, percent, resets, minutes):
        at = (now - minutes * 60) * 1000
        return json.dumps(
            {
                "login": login,
                "key_from": "account",
                "kind": kind,
                "percent": percent,
                "resets_at": resets,
                "observed_at": at,
                "recorded_at": at,
                "source": "mod",
            },
            separators=(",", ":"),
        )

    r7 = now + 3 * 86400
    r5 = now + 2 * 3600
    if mode == "main":
        rows = [
            obs("seven_day", 10, r7, 320),
            obs("seven_day", 18, r7, 210),
            obs("seven_day", 26, r7, 120),
            obs("seven_day", 31, r7, 45),
            obs("seven_day", 34, r7, 15),
            obs("five_hour", 3, r5, 170),
            obs("five_hour", 9, r5, 80),
            obs("five_hour", 12, r5, 15),
        ]
    else:
        rows = [obs("seven_day", p, r7, m) for p, m in [(5, 8000), (20, 6000), (40, 3000), (60, 600), (70, 10)]]
        rows += [obs("five_hour", 4, r5, 100), obs("five_hour", 9, r5, 10)]
    print("\n".join(rows))


def break_index(usage):
    junk = ['{"broken": ', "not json at all", '{"session_id": 3']
    targets = [os.path.join(usage, "keys.jsonl"), os.path.join(usage, "limits.jsonl")]
    buckets = os.path.join(usage, "buckets")
    for name in sorted(os.listdir(buckets)):
        targets.append(os.path.join(buckets, name))
    for t in targets:
        with open(t, "a") as f:
            for j in junk:
                f.write(j + "\n")
    print(len(targets) * len(junk))


# --- 照合 ---------------------------------------------------------------------------------------


def tokens(n):
    if n >= 100_000_000:
        return "%.2f 億" % (n / 100_000_000.0)
    if n >= 10_000:
        return "%.1f 万" % (n / 10_000.0)
    return str(n)


def pct(v):
    return "%.1f%%" % v


def usd(v):
    return "$%.2f" % v


def check(log, cli_dir):
    queries = {}
    rows = []
    for raw in open(log, encoding="utf-8", errors="replace"):
        parts = raw.rstrip("\n").split("\t")
        if parts[0] == "TAKO_USAGE_QUERY":
            queries[parts[1]] = parts[2:]
        elif parts[0] == "TAKO_USAGE_ROW":
            rows.append(parts[1:])
    fails = 0
    checked = 0
    table = []
    for step, li, key, name, main, est, secondary, gusd, gtokens, gsessions in rows:
        path = os.path.join(cli_dir, step + ".json")
        text_path = os.path.join(cli_dir, step + ".txt")
        if not os.path.exists(path):
            continue
        cli = json.load(open(path))
        text = open(text_path, encoding="utf-8").read() if os.path.exists(text_path) else ""
        window = cli.get("window", "7d")
        main_kind = "five_hour" if window == "5h" else "seven_day"
        login = cli["logins"][int(li)]
        want_key = json.loads(key)
        group = next((g for g in login["usage"]["groups"] if g["key"] == want_key), None)
        if group is None:
            print("  [NG] %s: 画面の行 %s が CLI の答えに無い" % (step, key))
            fails += 1
            continue
        share = (group.get("limit_share") or {}).get(main_kind) or {}
        cli_main = pct(share["percent"]) if "percent" in share else "-"
        cli_usd = usd(group["api_usd"])
        cli_tokens = tokens(group["total_tokens"])
        ok = (cli_main, cli_usd, cli_tokens, str(group["sessions"])) == (main, gusd, gtokens, gsessions)
        # CLI の既定出力の行（名前の行）にも同じ文字が並ぶ
        line = next((l for l in text.splitlines() if ("  " + group["name"] + "  API 換算") in l), "")
        in_text = all(s in line for s in [main if main != "-" else "", "API 換算 " + gusd, "トークン " + gtokens, "会話 " + gsessions])
        checked += 1
        if not (ok and in_text):
            fails += 1
        table.append(
            (step, group["name"], main, cli_main, gusd, cli_usd, gtokens, cli_tokens, gsessions, str(group["sessions"]), "一致" if ok and in_text else "不一致")
        )
    print("  | 段 | 行（CLI の名前） | 画面 % | CLI % | 画面 API 換算 | CLI | 画面 トークン | CLI | 画面 会話 | CLI | 判定 |")
    print("  |---|---|---|---|---|---|---|---|---|---|---|")
    for r in table:
        print("  | " + " | ".join(r) + " |")
    print("CHECKED=%d FAILS=%d" % (checked, fails))
    return 0 if fails == 0 and checked > 0 else 1


def main():
    cmd = sys.argv[1]
    a = sys.argv[2:]
    if cmd == "gen-main":
        gen_main(a[0], a[1], a[2], int(a[3]))
    elif cmd == "gen-huge":
        gen_huge(a[0], a[1], int(a[2]), int(a[3]))
    elif cmd == "limits":
        limits(a[0], int(a[1]), a[2])
    elif cmd == "break":
        break_index(a[0])
    elif cmd == "check":
        sys.exit(check(a[0], a[1]))
    else:
        sys.exit("unknown command: " + cmd)


if __name__ == "__main__":
    main()
