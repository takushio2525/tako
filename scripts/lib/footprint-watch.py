#!/usr/bin/env python3
"""footprint の回帰の見張り（#2023 / エピック #2011 の S8）の記録と判定。

`scripts/nightly-footprint-watch.sh` が呼ぶ。シェルはロック・worktree・時間切れ・起票を受け持ち、
ここは「履歴 TSV へ 1 行足す」「最後の行を基準（直近 N 回の中央値）と比べる」「状態と突き合わせて
起票するかを決める」だけを行う（標準ライブラリだけ・python 3.9 で動く = macOS 同梱の python3）。

  record     計測 1 回ぶんを履歴へ足す（列は和集合で揃える = 計測スクリプトが列を足しても壊れない）
  judge      最後の行を判定し、result（key=value）と、悪化なら起票の title.txt / body.md を書く
  measured   そのコミットを「描画ありで」測り済みなら終了コード 0
  find-open  `tako todo list --json`（stdin）から、この見張りが載せた未完了の 1 件の id を出す
  status     直近の結果・基準・載せた悪化を人が読む形で出す（読むだけ）

閾値は設計書 `.agent/plans/2026-10-lightweight.md` §7.1 の 2 回のぶれから決めた（METRICS の注記）。
"""
import argparse
import csv
import os
import statistics
import sys
import tempfile

# 起票の title の頭。**同じ悪化を重ねて積まない**ための目印でもある（find-open がこれで探す）
PREFIX = "[footprint 回帰]"
# 履歴の先頭の列（計測スクリプトの列はこの後ろに和集合で並ぶ）
LEAD = ["date", "sha", "version", "status", "reason", "elapsed_s"]
# 履歴に残す行の上限（1 晩 1 行なので 3 年弱。古い行から落とす）
MAX_ROWS = 1000
# 同じ条件か（ペインの並び・プレビューの数が違う行どうしは比べない）
CONDITION = ["scenario", "panes", "front", "back_tab", "bg", "previews"]
# #1979 のフォーカスの無いペインの再描画の上限（既定 30 fps）。計測の②はフォーカスの無いペインを
# 出力させるので、描画回数はこの上限に張り付く。S0 の実測は 28.9〜30.4 回/秒（窓の端の数え漏れで
# 上限をわずかに超える）なので、ぶれ ±1.5 を足した 31.5 を超えたら「上限が効いていない」と読む
REDRAW_CAP = 30.0 + 1.5

# (列, 表示名, 単位, 判定の種類, 閾値, 描画が要るか)
#   rel     基準 ×（1 + 閾値）を超えたら悪化
#   abs     基準 + 閾値を超えたら悪化
#   abs_rel 基準 + 閾値[0] と 基準 ×（1 + 閾値[1]）の**両方**を超えたら悪化（小さい値の揺れで鳴らさない）
#   cap     閾値そのものを超えたら悪化（基準は要らない）
# 描画が要る列は、出力中に本体の描画が起きた行（drawn=あり）どうしでしか比べない
# （蓋閉じ・画面ロックの夜はフレームが組まれず、描画の資源・CPU・回数が未実測になる = §1.3）
METRICS = [
    # S0 の 2 回のぶれは footprint・tmux ±2%、プレビューのある std だけ +7〜10%（§7.1）→ std の目安 +20%
    ("footprint_mb", "放置の footprint", "MB", "rel", 0.20, False),
    # 描画の資源は描いている最中しか計上されないので、出力中の footprint は描画ありの行どうしで比べる
    ("busy_footprint_mb", "出力中の footprint", "MB", "rel", 0.20, True),
    ("tmux_mb", "tmux の footprint", "MB", "rel", 0.20, False),
    # 放置の CPU のぶれは ±0.15 ポイント → 目安 +0.3 ポイント。アイドルの CPU の大半は
    # 毎 vsync の display link（§3.4）なので、フレームが組まれない夜とは比べない
    ("cpu_pct", "放置の CPU", "%", "abs", 0.3, True),
    # 出力中の CPU のぶれは ±0.9 ポイント → 目安 +1.5 ポイント
    ("busy_cpu_pct", "出力中の CPU", "%", "abs", 1.5, True),
    # 回収済みの子（tmux の取り込み・ps 等）。2 回のぶれの実測が無いので、+1.0 ポイントかつ +50% の
    # 両方を要る形で鳴らしにくくしてある（器 1 枚の出力中が 1.05% = #2018 の前）
    ("busy_child_cpu_pct", "出力中の子プロセスの CPU", "%", "abs_rel", (1.0, 0.50), False),
    # スレッドのぶれは ±2 本。std は器 30 で 48〜49 本（1 枚 1 本 + 常駐 16〜18 本 = §3.5）
    ("threads", "スレッド", "本", "abs", 5, False),
    # 起動時間はその日の最初の起動かで 0.5〜2.6 秒と大きく動く（§3.5 / L16）ので、+1 秒かつ +50%
    ("startup_ms", "起動時間", "ms", "abs_rel", (1000, 0.50), False),
    ("busy_body_renders_s", "出力中の描画回数", "回/秒", "cap", REDRAW_CAP, True),
]
METRIC_KEYS = [m[0] for m in METRICS]


def num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def fmt(v, unit):
    if v is None:
        return "-"
    if unit in ("本", "ms"):
        return "%d" % round(v)
    if unit == "%":
        return "%.2f" % v
    return "%.1f" % v


def vu(v, unit):
    """値と単位（% は数字に詰める: 0.85%・345.0 MB）"""
    return fmt(v, unit) + ("%" if unit == "%" else " " + unit)


def read_tsv(path):
    if not path or not os.path.exists(path) or os.path.getsize(path) == 0:
        return [], []
    with open(path, newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f, delimiter="\t")
        rows = [dict(r) for r in reader]
        return list(reader.fieldnames or []), rows


def write_tsv(path, fields, rows):
    d = os.path.dirname(os.path.abspath(path))
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".history-", dir=d)
    with os.fdopen(fd, "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fields, delimiter="\t", lineterminator="\n", extrasaction="ignore")
        w.writeheader()
        for r in rows:
            w.writerow({k: (r.get(k) if r.get(k) not in (None, "") else "-") for k in fields})
    os.replace(tmp, path)


def read_kv(path):
    out = {}
    if path and os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.rstrip("\n")
                if "=" in line:
                    k, v = line.split("=", 1)
                    out[k] = v
    return out


def write_kv(path, kv):
    with open(path, "w", encoding="utf-8") as f:
        for k, v in kv.items():
            # 改行は 1 行に畳む（シェルが sed -n 's/^k=//p' で読む）
            f.write("%s=%s\n" % (k, str(v).replace("\n", " ")))


def env_parts(row):
    """env 欄 → (足した env, 偽の claude か)。夜間の標準は足した env が「-」"""
    tokens = (row.get("env") or "-").split()
    stub = "stub-claude" in tokens
    extra = " ".join(t for t in tokens if t != "stub-claude") or "-"
    return extra, stub


def filled_complete(row):
    a, _, b = (row.get("filled") or "").partition("/")
    return a != "" and a == b


def cond_key(row):
    return tuple(row.get(k, "-") for k in CONDITION)


# --- record ------------------------------------------------------------------
def cmd_record(a):
    fields, rows = read_tsv(a.history)
    mfields, measured = read_tsv(a.row)
    new = {"date": a.date, "sha": a.sha, "version": a.version or "-", "status": a.status,
           "reason": a.reason or "-", "elapsed_s": a.elapsed or "-"}
    if a.status == "ok":
        if not measured:
            print("record: 計測の行が無いのに status=ok", file=sys.stderr)
            return 2
        new.update({k: v for k, v in measured[-1].items() if k not in LEAD})
    union = list(LEAD)
    for k in fields + mfields:
        if k not in union:
            union.append(k)
    rows.append(new)
    write_tsv(a.history, union, rows[-MAX_ROWS:])
    return 0


# --- judge -------------------------------------------------------------------
def judge(rows, state, window, min_n, scale):
    """最後の行を判定する。戻り値は dict（verdict / flagged / lines / baseline …）"""
    cur = rows[-1]
    res = {"cur": cur, "flagged": [], "lines": [], "baseline": [], "verdict": "", "summary": ""}
    if cur.get("status") != "ok":
        res["verdict"] = "unmeasured"
        res["summary"] = "未実測（%s）" % (cur.get("reason") or "-")
        return res
    if not filled_complete(cur):
        res["verdict"] = "condition"
        res["summary"] = "条件が崩れたので判定しない（中身の入ったペイン %s）" % cur.get("filled", "-")
        return res
    drawn = cur.get("drawn") == "あり"
    _, stub = env_parts(cur)
    base = [r for r in rows[:-1]
            if r.get("status") == "ok" and cond_key(r) == cond_key(cur) and filled_complete(r)
            and env_parts(r) == ("-", stub) and (r.get("drawn") == "あり") == drawn][-window:]
    res["baseline"] = base
    for key, label, unit, kind, th, needs_draw in METRICS:
        v = num(cur.get(key))
        if v is None:
            continue
        if needs_draw and not drawn:
            res["lines"].append("%s: %s（描画なしの夜なので比べない）" % (label, vu(v, unit)))
            continue
        if kind == "cap":
            bad = v > th
            res["lines"].append("%s: %s（上限 %s）%s" % (label, vu(v, unit), fmt(th, unit), " 悪化" if bad else ""))
            if bad:
                res["flagged"].append({"key": key, "label": label, "unit": unit, "cur": v, "base": None,
                                       "limit": th, "short": "%s %s（上限 %s）" % (label, vu(v, unit), fmt(th, unit))})
            continue
        vals = [x for x in (num(r.get(key)) for r in base) if x is not None]
        if len(vals) < min_n:
            res["lines"].append("%s: %s（基準づくり中 %d/%d 回）" % (label, vu(v, unit), len(vals), min_n))
            continue
        b = statistics.median(vals)
        if kind == "rel":
            limit = b * (1 + th * scale)
            bad = v > limit
        elif kind == "abs":
            limit = b + th * scale
            bad = v > limit
        else:  # abs_rel
            limit = max(b + th[0] * scale, b * (1 + th[1] * scale))
            bad = v > limit
        res["lines"].append("%s: %s（基準 %s・上限 %s）%s" % (label, vu(v, unit), fmt(b, unit), fmt(limit, unit), " 悪化" if bad else ""))
        if bad:
            # 量（MB・ms）は比率、CPU（%）はポイント、本数はそのままの差で言う
            if unit == "%":
                delta = "+%.2f ポイント" % (v - b)
            elif unit in ("MB", "ms") and b:
                delta = "+%.1f%%" % ((v - b) / b * 100)
            else:
                delta = "+%s %s" % (fmt(v - b, unit), unit)
            res["flagged"].append({"key": key, "label": label, "unit": unit, "cur": v, "base": b, "limit": limit,
                                   "short": "%s %s" % (label, delta)})
    if res["flagged"]:
        res["verdict"] = "regression"
        res["summary"] = "悪化: " + "・".join(f["short"] for f in res["flagged"])
    elif len(base) < min_n:
        res["verdict"] = "baseline"
        res["summary"] = "基準づくり中（同じ条件で%sの行 %d/%d 回）" % ("描画あり" if drawn else "描画なし", len(base), min_n)
    else:
        res["verdict"] = "clean"
        res["summary"] = "回帰なし（基準 = 直近 %d 回の中央値・%s）" % (len(base), "描画あり" if drawn else "描画なし")
    return res


def decide(res, state):
    """状態（前の夜から続いている悪化）と突き合わせて、起票するかを決める"""
    prev = [k for k in (state.get("open_metrics") or "").split(",") if k]
    flagged = [f["key"] for f in res["flagged"]]
    if res["verdict"] == "regression":
        new = [k for k in flagged if k not in prev]
        union = [k for k in METRIC_KEYS if k in flagged or k in prev]
        return ("report" if new else "continuing"), new, union
    if res["verdict"] == "clean":
        return ("clear" if prev else "none"), [], []
    # 未実測・条件崩れ・基準づくり中は状態を動かさない（判定していないので「解消した」とも言わない）
    return "keep", [], prev


def body_md(res, new, prev_open, window):
    cur = res["cur"]
    sha = cur.get("sha", "-")
    base = res["baseline"]
    lines = [
        "夜間の計測（`scripts/measure-footprint-2011.sh` の std = 器 30・プレビュー %s・放置 60 秒 + 出力中 60 秒）で、"
        "直近 %d 回の中央値（基準）より閾値を超えて悪化した。" % (cur.get("previews", "-"), window),
        "",
        "- 計測: %s・v%s（`%s`）・描画%s・起動 %s ms" % (cur.get("date", "-"), cur.get("version", "-"), sha[:12],
                                                cur.get("drawn", "-"), cur.get("startup_ms", "-")),
    ]
    if base:
        lines.append("- 基準: 直近 %d 回（%s〜%s・`%s`〜`%s`）の中央値" % (
            len(base), base[0].get("date", "-"), base[-1].get("date", "-"),
            base[0].get("sha", "-")[:7], base[-1].get("sha", "-")[:7]))
    lines += ["", "| 項目 | 今回 | 基準 | 上限 |", "|---|---|---|---|"]
    for f in res["flagged"]:
        mark = "（新しく）" if f["key"] in new and prev_open else ""
        lines.append("| %s%s | %s | %s | %s |" % (
            f["label"], mark, vu(f["cur"], f["unit"]),
            vu(f["base"], f["unit"]) if f["base"] is not None else "-", vu(f["limit"], f["unit"])))
    lines += [
        "",
        "- 手元で測り直す: `bash scripts/measure-footprint-2011.sh`（約 4 分。窓は tako-vd に出る）。"
        "修正前後は同じバイナリの A/B（`TAKO_FOOTPRINT_ENV=\"TAKO_xxxx_LEGACY=1\"`）で比べる",
        "- 同じ悪化は重ねて載せない（新しい項目が悪化したときだけこの 1 件を書き換える。"
        "回帰なしの夜が来たら見張りは解消扱いに戻る。この 1 件は人が片付ける）",
        "- 直近の結果と基準: `bash scripts/nightly-footprint-watch.sh --status`。閾値と仕組みは `.agent/release.md`「footprint の回帰の見張り」",
    ]
    return "\n".join(lines) + "\n"


def title_txt(res):
    shorts = [f["short"] for f in res["flagged"]]
    head = "・".join(shorts[:3]) + ("ほか %d 項目" % (len(shorts) - 3) if len(shorts) > 3 else "")
    return "%s %s（%s）" % (PREFIX, head, res["cur"].get("sha", "-")[:7])


def prev_measured_sha(rows):
    """今回より前に測れた（status=ok・標準の env）最後の行のコミット（変更の範囲を出すため）"""
    cur_sha = rows[-1].get("sha")
    for r in reversed(rows[:-1]):
        if r.get("status") == "ok" and env_parts(r)[0] == "-" and r.get("sha") != cur_sha:
            return r.get("sha")
    return ""


def cmd_judge(a):
    _, rows = read_tsv(a.history)
    if not rows:
        print("judge: 履歴が空", file=sys.stderr)
        return 2
    state = read_kv(a.state)
    res = judge(rows, state, a.window, a.min, a.scale)
    action, new, union = decide(res, state)
    os.makedirs(a.out_dir, exist_ok=True)
    kv = {
        "verdict": res["verdict"],
        "action": action,
        "metrics": ",".join(f["key"] for f in res["flagged"]),
        "new_metrics": ",".join(new),
        "open_metrics": ",".join(union),
        "todo": state.get("todo", ""),
        "since": state.get("since", ""),
        "baseline_n": len(res["baseline"]),
        "prev_sha": prev_measured_sha(rows),
        "summary": res["summary"],
    }
    write_kv(os.path.join(a.out_dir, "result"), kv)
    with open(os.path.join(a.out_dir, "lines"), "w", encoding="utf-8") as f:
        f.write("\n".join(res["lines"]) + ("\n" if res["lines"] else ""))
    if res["verdict"] == "regression":
        with open(os.path.join(a.out_dir, "title.txt"), "w", encoding="utf-8") as f:
            f.write(title_txt(res) + "\n")
        prev = [k for k in (state.get("open_metrics") or "").split(",") if k]
        with open(os.path.join(a.out_dir, "body.md"), "w", encoding="utf-8") as f:
            f.write(body_md(res, new, prev, a.window))
    return 0


# --- measured / find-open / status ------------------------------------------
def cmd_measured(a):
    _, rows = read_tsv(a.history)
    for r in rows:
        if r.get("sha") == a.sha and r.get("status") == "ok" and r.get("drawn") == "あり" \
                and env_parts(r)[0] == "-" and filled_complete(r):
            return 0
    return 1


def cmd_find_open(a):
    import json
    try:
        data = json.load(sys.stdin)
    except ValueError:
        return 1
    tasks = [t for t in (data.get("tasks") or []) if t.get("status") == "open"
             and str(t.get("title", "")).startswith(PREFIX)]
    pick = next((t for t in tasks if a.prefer and t.get("id") == a.prefer), None) or (tasks[0] if tasks else None)
    if pick:
        print(pick.get("id", ""))
        return 0
    return 1


def cmd_status(a):
    _, rows = read_tsv(a.history)
    state = read_kv(a.state)
    print("履歴: %s（%d 行）" % (a.history_label or a.history, len(rows)))
    if not rows:
        print("  まだ 1 回も記録していない（夜間リリースの後に scripts/nightly-footprint-watch.sh が測る）")
        return 0
    print("直近の %d 回:" % min(len(rows), a.last))
    print("  %-16s %-8s %-10s %-4s %9s %9s %7s %7s %7s %5s" % (
        "日時", "コミット", "状態", "描画", "放置MB", "出力中MB", "放置CPU", "出力CPU", "描画/秒", "スレ"))
    for r in rows[-a.last:]:
        st = r.get("status", "-")
        if st != "ok":
            st = st + "(" + (r.get("reason") or "-")[:18] + ")"
        print("  %-16s %-8s %-10s %-4s %9s %9s %7s %7s %7s %5s" % (
            r.get("date", "-")[:16], r.get("sha", "-")[:7], st, r.get("drawn", "-"),
            r.get("footprint_mb", "-"), r.get("busy_footprint_mb", "-"), r.get("cpu_pct", "-"),
            r.get("busy_cpu_pct", "-"), r.get("busy_body_renders_s", "-"), r.get("threads", "-")))
    res = judge(rows, state, a.window, a.min, a.scale)
    print("最後の行の判定: %s" % res["summary"])
    for line in res["lines"]:
        print("  " + line)
    if state.get("open_metrics"):
        print("載せた悪化: %s（%s・%s から。同じ項目が悪化し続ける間は重ねて載せない）" % (
            state.get("open_metrics"), state.get("todo") or "未起票", state.get("since") or "-"))
    else:
        print("載せた悪化: なし")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd")
    r = sub.add_parser("record")
    r.add_argument("--history", required=True)
    r.add_argument("--row", default="")
    r.add_argument("--date", required=True)
    r.add_argument("--sha", required=True)
    r.add_argument("--version", default="")
    r.add_argument("--status", required=True, choices=["ok", "unmeasured", "failed"])
    r.add_argument("--reason", default="")
    r.add_argument("--elapsed", default="")
    for name in ("judge", "status"):
        j = sub.add_parser(name)
        j.add_argument("--history", required=True)
        j.add_argument("--state", required=True)
        j.add_argument("--window", type=int, default=5)
        j.add_argument("--min", type=int, default=3)
        j.add_argument("--scale", type=float, default=1.0)
        if name == "judge":
            j.add_argument("--out-dir", required=True)
        else:
            j.add_argument("--last", type=int, default=10)
            j.add_argument("--history-label", default="")
    m = sub.add_parser("measured")
    m.add_argument("--history", required=True)
    m.add_argument("--sha", required=True)
    f = sub.add_parser("find-open")
    f.add_argument("--prefer", default="")
    a = p.parse_args()
    fn = {"record": cmd_record, "judge": cmd_judge, "measured": cmd_measured,
          "find-open": cmd_find_open, "status": cmd_status}.get(a.cmd)
    if fn is None:
        p.print_help(sys.stderr)
        return 2
    return fn(a)


if __name__ == "__main__":
    sys.exit(main())
