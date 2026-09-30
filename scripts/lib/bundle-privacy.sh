#!/usr/bin/env bash
# bundle-privacy.sh — 配布する .app に個人情報が入っていないかを調べる（Issue #1848）
#
# source して使う。呼ぶのは 2 か所だけ（検査の実装はここの 1 本）:
#   - scripts/build-app.sh … 署名の後（ここで落ちれば .app は配布物にならない）
#   - scripts/release.sh   … zip を作る直前（--skip-build で古い dist/tako.app を包む経路も塞ぐ）
#
#   check_bundle_privacy <.app>   # 0 = 混入なし / 1 = 混入あり / 2 = 使い方の誤り
#
# 見るもの:
#   1. バンドル内の全ファイルの中身（実行ファイル・埋め込みのシェーダー・Info.plist・署名・
#      同梱の文書）に、このマシンの識別子が無いこと。識別子は**環境から作る**
#      （値をリポジトリに置かない = #927。no_personal_data.rs の検査 2 と同じ考え方）:
#        - ホームディレクトリ（$HOME）… v0.8.24 は panic の位置情報とシェーダーで約 1,400 箇所
#        - ログイン名（$USER。runner 等の汎用の名前と 3 バイト未満は除く）
#        - アカウントのフルネーム（id -F）
#        - TAKO_PII_TERMS（, 区切り。外から足す語。no_personal_data.rs と同じ入口）
#   2. 署名の名義（codesign -dvvv の Authority=）が個人の開発用証明書でないこと。
#      Apple Development / Mac Developer / iPhone Developer は名義が必ず個人名で、配布用でもない
#      （Gatekeeper の扱いは ad-hoc と同じ = 公証なしで rejected）。名義に 1 の語が入るものも落とす。
#      個人アカウントの Developer ID も名義は個人名になるので、入れるときは 1 の語で止まる
#
# 見つけた値そのものは出さない（CI・夜間リリースのログへ個人情報を書き戻さない）。
# 出すのは種類・件数・場所（バンドル内の相対パス）と、伏せ字にした Authority だけ。

# 1 行 1 語で「種類<TAB>語」を出す
bundle_privacy_terms() {
  if [[ -n "${HOME:-}" && "$HOME" != "/" ]]; then
    printf 'ビルド機のホームパス\t%s\n' "$HOME"
  fi
  local user=${USER:-}
  case "$user" in
    "" | root | admin | user | users | runner | build | builder | test | ci) ;;
    *) [[ ${#user} -ge 3 ]] && printf 'ログイン名\t%s\n' "$user" ;;
  esac
  local full
  full=$(id -F 2>/dev/null || true)
  if [[ ${#full} -ge 3 ]]; then
    printf 'アカウントのフルネーム\t%s\n' "$full"
  fi
  local terms=${TAKO_PII_TERMS:-} t
  local IFS=,
  for t in $terms; do
    # 前後の空白を落とす
    t="${t#"${t%%[![:space:]]*}"}"
    t="${t%"${t##*[![:space:]]}"}"
    [[ ${#t} -ge 3 ]] && printf 'TAKO_PII_TERMS の語\t%s\n' "$t"
  done
  return 0
}

# Authority= の行を、種類（「Apple Development:」まで）だけ残して伏せる
bundle_privacy_mask_authority() {
  # BSD sed は `t` の後ろを行末までラベル名として読むので、式を -e で分ける
  sed -E -e 's/^(Authority=[^:]*:).*/\1 <伏せ字>/' -e 't' -e 's/^(Authority=).*/\1<伏せ字>/'
}

check_bundle_privacy() {
  local app=${1:-}
  if [[ -z "$app" || ! -d "$app" ]]; then
    echo "エラー: check_bundle_privacy: .app が見つからない（${app:-指定なし}）" >&2
    return 2
  fi
  local problems=() kind term f rel n

  # --- 1. 中身にこのマシンの識別子が無いこと ---
  local terms_file
  terms_file=$(mktemp "${TMPDIR:-/tmp}/tako-bundle-privacy.XXXXXX")
  bundle_privacy_terms >"$terms_file"
  while IFS=$'\t' read -r kind term; do
    [[ -n "$term" ]] || continue
    while IFS= read -r -d '' f; do
      n=$({ LC_ALL=C grep -a -o -F -- "$term" "$f" 2>/dev/null || true; } | wc -l | tr -d ' ')
      if [[ "$n" -gt 0 ]]; then
        rel=${f#"$app"/}
        problems+=("${rel}: ${kind}が ${n} 箇所")
      fi
    done < <(find "$app" -type f -print0)
  done <"$terms_file"

  # --- 2. 署名の名義 ---
  local target sig line label
  for target in "$app" "$app/Contents/MacOS/tako"; do
    [[ -e "$target" ]] || continue
    label=${target#"$app"}
    label=${label#/}
    label=${label:-tako.app}
    if ! sig=$(codesign -dvvv "$target" 2>&1); then
      problems+=("${label}: 署名を読めない（codesign -dvvv が失敗）")
      continue
    fi
    while IFS= read -r line; do
      case "$line" in
        "Authority=Apple Development:"* | "Authority=Mac Developer:"* | "Authority=iPhone Developer:"*)
          problems+=("${label}: 署名が個人の開発用証明書（$(printf '%s\n' "$line" | bundle_privacy_mask_authority)）")
          ;;
        Authority=*)
          while IFS=$'\t' read -r kind term; do
            if [[ -n "$term" && "$line" == *"$term"* ]]; then
              problems+=("${label}: 署名の名義に${kind}が入っている（$(printf '%s\n' "$line" | bundle_privacy_mask_authority)）")
            fi
          done <"$terms_file"
          ;;
      esac
    done <<<"$sig"
  done
  rm -f "$terms_file"

  if [[ ${#problems[@]} -gt 0 ]]; then
    echo "エラー: 配布物の個人情報チェックに通らない（Issue #1848。値は伏せて出す）: $(basename "$app")" >&2
    local p
    for p in ${problems[@]+"${problems[@]}"}; do
      echo "  - $p" >&2
    done
    echo "  直し方: パスは scripts/build-app.sh の付け替え（--remap-path-prefix / xcrun の包み）、" >&2
    echo "          署名は ad-hoc（TAKO_CODESIGN_IDENTITY を外す）。詳細は .agent/release.md" >&2
    return 1
  fi
  echo "    OK: ビルド機の識別子 0 件・署名の名義に個人名なし"
  return 0
}
