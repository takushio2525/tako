#!/usr/bin/env bash
# gh-retry.sh — gh の一時的な接続エラーをやり直し、届かないままなら「到達性」で終わる 1 実装（#1704）
#
# なぜ要るか:
#   `wait-pr-checks.sh` は `gh pr checks` が `error connecting to api.github.com` で 1 回落ちただけで
#   3（引数・gh のエラー）で終わっていた。9/24 に PR #1669 / #1630 の worker が踏み、CI 自体は全緑だった。
#   呼び手（`merge-pr.sh`・master・worker）は 3 も 1（赤）も「merge してはいけない」と読むので、
#   **ネットの瞬断が「CI が赤い」に化ける**。
#
#   そこで gh の呼び出しをここへ集め、**接続エラーと読めるときだけ**指数バックオフで数回やり直す。
#   それでも届かなければ「到達性」を名指しした別の終了コード（GH_UNREACHABLE_EXIT）で終わる。
#   認証切れ・存在しない PR・壊れた応答など**やり直しても変わらない失敗**はやり直さない
#   （従来どおり呼び手が 3 で終わる）。
#
# 使い方（source して使う。単体では何もしない）:
#   . "${SCRIPT_DIR}/lib/gh-retry.sh"
#   if ! gh_retry pr checks "${PR}" --json name,bucket; then
#     echo "到達不能: $(gh_unreachable_reason)" >&2
#     exit "${GH_UNREACHABLE_EXIT}"
#   fi
#   # 届いた（GH_RC / GH_OUT / GH_ERR を見て、gh 自身の成否は呼び手が決める）
#
# 注入（テスト・計測専用）:
#   TAKO_GH_RETRY_ATTEMPTS = 試す回数（既定 5。1 以上の整数）
#   TAKO_GH_RETRY_BASE     = 最初の待ちの秒数（既定 1。0 以上の整数。n 回目の後の待ちは base × 2^(n-1)）
#   TAKO_1704_LEGACY=1     = 修正前の腕（やり直さず、接続エラーも 3 のまま = 到達性で終わらない）

# 届かないまま使い切ったときの終了コード（wait-pr-checks.sh / merge-pr.sh 共通）。
# 既存の 0 = 全部緑 / 1 = 失敗・拒否 / 2 = タイムアウト / 3 = 引数・gh のエラー /
# 4 = base と衝突している（lib/pr-conflict.sh）と重ならない値
GH_UNREACHABLE_EXIT=5

# 既定: 5 回試す = 待ちは 4 回で 1 + 2 + 4 + 8 = 計 15 秒（数十秒以内。gh 自身の接続待ちは別）
GH_RETRY_DEFAULT_ATTEMPTS=5
GH_RETRY_DEFAULT_BASE=1

GH_RETRY_ATTEMPTS="${TAKO_GH_RETRY_ATTEMPTS:-${GH_RETRY_DEFAULT_ATTEMPTS}}"
[[ "${GH_RETRY_ATTEMPTS}" =~ ^[0-9]+$ && "${GH_RETRY_ATTEMPTS}" -ge 1 ]] || GH_RETRY_ATTEMPTS="${GH_RETRY_DEFAULT_ATTEMPTS}"
GH_RETRY_BASE="${TAKO_GH_RETRY_BASE:-${GH_RETRY_DEFAULT_BASE}}"
[[ "${GH_RETRY_BASE}" =~ ^[0-9]+$ ]] || GH_RETRY_BASE="${GH_RETRY_DEFAULT_BASE}"
GH_RETRY_LEGACY="${TAKO_1704_LEGACY:-0}"

# 接続エラー（GitHub に届かない・GitHub が応答を返せない）と読める gh の言い分。大文字小文字は問わない。
#   - gh は DNS の失敗だけを `error connecting to <host>` に整形し、ほかは Go の原文のまま出す
#     （`dial tcp …: connection refused` / `read: connection reset by peer` / `TLS handshake timeout` 等）
#   - 502 / 503 / 504 は GitHub 側の一時的な不調（REST は `HTTP 502: …`、GraphQL は `status code: 502`）
#   - GraphQL の `Something went wrong while executing your query` は GitHub 側のタイムアウト
# 入れないもの: 401（認証切れ）/ 404 / rate limit / 500 / `Could not resolve to a PullRequest`
# （存在しない PR。`could not resolve` だけで拾うと取り違えるので `could not resolve host` に限る）
GH_UNREACHABLE_PATTERN='error connecting to|check your internet connection|no such host|could not resolve host|temporary failure in name resolution|dial tcp|connection refused|connection reset|connection timed out|broken pipe|network is unreachable|network is down|host is unreachable|no route to host|i/o timeout|tls handshake timeout|context deadline exceeded|client\.timeout exceeded|unexpected eof|http 50[234]|status code: 50[234]|bad gateway|service unavailable|gateway timeout|something went wrong while executing your query'

# $1 = gh の stderr → 0 = 接続エラーと読める（やり直す価値がある）
gh_unreachable_error() {
  [[ -n "${1:-}" ]] || return 1
  grep -qiE -- "${GH_UNREACHABLE_PATTERN}" <<<"$1"
}

# $1 = 最初の待ち（秒）, $2 = 何回目の失敗の後か（1 始まり）→ 待つ秒数（base × 2^(n-1)）
gh_retry_delay() {
  printf '%d' "$(($1 * (1 << ($2 - 1))))"
}

# gh の言い分の最初の空でない行（1 行の理由に載せる）
gh_first_line() {
  awk 'NF { print; exit }' <<<"${1:-}"
}

# gh を呼び、接続エラーのときだけ指数バックオフでやり直す。引数は gh へそのまま渡す。
# 結果は大域変数に置く（bash 3.2 には nameref が無い）:
#   GH_OUT = stdout / GH_ERR = stderr / GH_RC = gh の終了コード / GH_TRIES = 試した回数 /
#   GH_LABEL = 「gh pr checks」のような呼び出しの名前（理由の 1 行に使う）
# 戻り値: 0 = GitHub に届いた（gh 自身の成否は GH_RC で呼び手が決める）/
#         1 = 試す回数を使い切っても接続エラーのまま（= 到達性で終わるべき）
# gh が JSON を返しつつ非 0 で終わる口（`gh pr checks` の 1 = 失敗あり / 8 = 実行中）があるので、
# **stdout が空で、stderr が接続エラーと読めるとき**だけをやり直しの対象にする
gh_retry() {
  local err delay
  GH_LABEL="gh $1 ${2:-}"
  GH_LABEL="${GH_LABEL% }"
  GH_TRIES=0
  while :; do
    GH_TRIES=$((GH_TRIES + 1))
    err="$(mktemp)"
    if GH_OUT="$(gh "$@" 2>"${err}")"; then GH_RC=0; else GH_RC=$?; fi
    GH_ERR="$(cat "${err}")"
    rm -f "${err:?}"
    if [[ ${GH_RC} -eq 0 || -n "${GH_OUT}" || "${GH_RETRY_LEGACY}" == "1" ]]; then return 0; fi
    gh_unreachable_error "${GH_ERR}" || return 0
    [[ ${GH_TRIES} -lt ${GH_RETRY_ATTEMPTS} ]] || return 1
    delay="$(gh_retry_delay "${GH_RETRY_BASE}" "${GH_TRIES}")"
    echo "[$(date +%H:%M:%S)] ${GH_LABEL} が GitHub に届かない（${GH_TRIES}/${GH_RETRY_ATTEMPTS} 回目: $(gh_first_line "${GH_ERR}")）。${delay} 秒後にやり直す" >&2
    sleep "${delay}"
  done
}

# gh_retry が 1 を返した直後に呼ぶ。到達性を名指しした理由（呼び手が前置きと後ろの案内を足す）
gh_unreachable_reason() {
  printf 'GitHub API に届かない（%s を %d 回試して全部が接続エラー / 最後: %s）' \
    "${GH_LABEL:-gh}" "${GH_TRIES:-0}" "$(gh_first_line "${GH_ERR:-}")"
}
