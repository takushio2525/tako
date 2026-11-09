#!/usr/bin/env bash
# tako:run: bash scripts/warning-gate.sh cargo build --workspace
# warning-gate.sh — 自分のクレートのコンパイラの警告を 0 件に保つゲート（#1537 / #1827）
#
# 渡した cargo のコマンドを走らせ、出力をそのまま流したうえで、cargo が数えた警告
# （`warning: `<crate>` (<target>) generated N warning(s)`）が 1 件でもあれば、
# 位置（file:line:col）と本文を名指しして終了コード 3 で止まる。
# 依存クレートの警告は cargo が --cap-lints allow で黙らせるので、数えるのは
# ワークスペースのクレートの分だけ。build script の `cargo:warning=` は数えない。
#
# 済んだビルドに当てても数えられる: cargo はコンパイル済みの単位の警告を再生する
# （再コンパイルは起きない。色の設定を変えても同じ）。CI の Windows ジョブはこれで
# ビルドとテストのコンパイルの後に数秒で数える。手元の `scripts/check-windows.sh` も
# 同じ 1 実装を通すので、手元と CI で数え方がずれない。
#
# RUSTFLAGS=-D warnings を使わない理由: env の RUSTFLAGS は .cargo/config.toml の
# [target.x86_64-pc-windows-msvc] rustflags（+crt-static）を丸ごと上書きし、
# 配布物が VC++ の DLL を要求する形に変わる（#587）。キャッシュの鍵も変わる。
# cargo の build.warnings = "deny" は 1.95 の安定版では効かない（黙って無視される）。
#
# 使い方:
#   scripts/warning-gate.sh cargo build --workspace
#   scripts/warning-gate.sh cargo test --workspace --no-run
# 終了コード: 0 = 警告 0 件 / 3 = 警告がある（コマンドは成功）/ 2 = 使い方の誤り /
#             それ以外 = コマンド自身の終了コード（コンパイルエラーなど）
set -u

if [ "$#" -eq 0 ]; then
  echo "使い方: scripts/warning-gate.sh <cargo のコマンド…>" >&2
  exit 2
fi
case " $* " in
  *" --message-format"*)
    # 数えるのは人向けの出力。JSON などでは summary の行が出ない
    echo "warning-gate.sh: --message-format を付けると警告を数えられません（人向けの出力を読みます）" >&2
    exit 2
    ;;
esac

out="$(mktemp "${TMPDIR:-/tmp}/tako-warning-gate.XXXXXX")" || exit 1
trap 'rm -f "${out:?}"' EXIT

# 色の符号は読むときの邪魔になるので切る（済んだ単位も色なしで再生される）
CARGO_TERM_COLOR=never "$@" 2>&1 | tee "$out"
rc=${PIPESTATUS[0]}
if [ "$rc" -ne 0 ]; then
  exit "$rc"
fi

# 1 行目 = cargo が数えた件数の合計、2 行目以降 = 「位置<TAB>本文」（位置の分かる警告だけ）。
# cargo の `generated N warnings (M duplicates)` の N は、別の単位（lib と lib test など）で
# 既に出した警告 M 件を含む。重複は表示されないので N - M を足す（名指した行の数と揃う）。
# 位置は Windows の `\` を `/` へ寄せる（CI の注釈と手元の表示を揃える）
report="$(awk '
  { sub(/\r$/, "") }
  /^warning: `[^`]+` \(.*\) generated [0-9]+ warnings?/ {
    if (match($0, /generated [0-9]+ warning/)) total += substr($0, RSTART + 10, RLENGTH - 18) + 0
    if (match($0, /\([0-9]+ duplicates?\)/)) total -= substr($0, RSTART + 1) + 0
    pending = ""
    next
  }
  /^warning: / { pending = substr($0, 10); next }
  /^error/ { pending = ""; next }
  /^ *--> / {
    if (pending != "") {
      loc = $0
      sub(/^ *--> /, "", loc)
      gsub(/\\/, "/", loc)
      n++
      line[n] = loc "\t" pending
      pending = ""
    }
    next
  }
  END {
    print total + 0
    for (i = 1; i <= n; i++) print line[i]
  }
' "$out")"
total="$(printf '%s\n' "$report" | sed -n 1p)"
located="$(printf '%s\n' "$report" | sed 1d)"
listed=0
if [ -n "$located" ]; then
  listed="$(printf '%s\n' "$located" | wc -l | tr -d ' ')"
fi

if [ "$total" -eq 0 ] && [ "$listed" -eq 0 ]; then
  echo "警告ゲート（#1537）: 自分のクレートの警告は 0 件"
  exit 0
fi

count="$total"
if [ "$listed" -gt "$count" ]; then
  count="$listed"
fi
{
  echo
  echo "警告ゲート（#1537）: 自分のクレートにコンパイラの警告が ${count} 件あります。0 件に保ってください"
  if [ -n "$located" ]; then
    printf '%s\n' "$located" | awk -F '\t' '{ print "  " $1 ": " $2 }'
  fi
  if [ "$total" -gt "$listed" ]; then
    echo "  （位置の分からない警告が $((total - listed)) 件。上の cargo の出力を見てください）"
  fi
  echo "  直し方: 本当に使っていないものは消し、片方の OS でだけ使うものは使う側の cfg へ寄せる"
  echo "  （逃がすなら #[cfg_attr(windows, allow(dead_code))] に理由のコメントを添える）"
} >&2
# GitHub Actions ではチェックの注釈に位置を名指しで出す
if [ "${GITHUB_ACTIONS:-}" = "true" ] && [ -n "$located" ]; then
  printf '%s\n' "$located" | awk -F '\t' '{
    loc = $1; msg = $2
    file = loc; ln = ""; col = ""
    if (match(loc, /:[0-9]+:[0-9]+$/)) {
      file = substr(loc, 1, RSTART - 1)
      split(substr(loc, RSTART + 1), p, ":")
      ln = p[1]; col = p[2]
    }
    gsub(/%/, "%25", msg)
    printf "::error file=%s,line=%s,col=%s::[#1537 警告ゲート] %s\n", file, ln, col, msg
  }'
fi
exit 3
