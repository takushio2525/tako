#!/usr/bin/env bash
# changelog.sh — CHANGELOG.md へリリースの節を挿す正本（#1593）
#
# 夜間リリース（scripts/nightly-release.sh）がこれを source して版の節を挿す。
# scripts/test-release-retry.sh も同じ関数を source して「Unreleased は最上段に 1 つ」を検査する。
#
# 規則（Keep a Changelog）: `## [Unreleased]` は**最初の版の節より上に 1 つだけ**置く。
# 手書きの項目はその下へ書く。リリースの挿入は:
#   - `## [Unreleased]` の見出しは最上段に残す（無ければ作る）
#   - その直下へ新しい版の節（自動の前置き + コミット一覧）を挿す
#   - Unreleased に書かれていた手書きの項目は、新しい版の節の末尾（コミット一覧の後ろ）へ移す
#     = その版のリリースノート（release.sh の extract_changelog）に載る
#
# 旧版は「最初の `## [` の直前」へ挿していた。`## [Unreleased]` 自体がその `## [` に当たるので、
# Unreleased が新しい版の節の下へ押し出され、次の PR が最上段に新しい Unreleased を作る、を
# 繰り返して 5 つ並んでいた（#1593）。押し出された手書きの項目は版のリリースノートにも
# 載っていなかった（抽出は次の `## [` = 押し出された Unreleased の見出しで止まる）。
#
# A/B: TAKO_1593_LEGACY=1 で旧版の挿し方（scripts/test-release-retry.sh の負の対照が使う）。
#
# bash 3.2 で動くこと（launchd は /bin/bash = 3.2 で起動する）。awk は macOS 同梱の BSD awk で通す。

# changelog_insert_release <CHANGELOG のパス> <版の節のファイル>
#   版の節のファイルは `## [版] - 日付` で始まり、空行で終わる形（nightly-release.sh が作る）。
#   最上段より下にも `## [Unreleased]` がある（規則が既に崩れている）ときは、最上段の 1 つだけを
#   扱い、stderr に警告を出す（リリースは止めない。CI の番犬 changelog_unreleased_watchdog が落とす）
changelog_insert_release() {
  local file="$1" section="$2" tmp
  tmp="${file}.tmp"
  if [[ "${TAKO_1593_LEGACY:-}" = 1 ]]; then
    awk -v secfile="$section" '
      !ins && /^## \[/ { while ((getline line < secfile) > 0) print line; close(secfile); ins = 1 }
      { print }
    ' "$file" > "$tmp" && mv -f "$tmp" "$file"
    return
  fi
  awk -v secfile="$section" '
    function is_unreleased(s) { return tolower(s) ~ /^## \[unreleased\]/ }
    function emit_section(   line) {
      while ((getline line < secfile) > 0) print line
      close(secfile)
    }
    # 最上段の Unreleased を空にして残し、その下へ版の節と手書きの項目を出す
    function emit_release(   i, s, e) {
      print "## [Unreleased]"
      print ""
      emit_section()
      s = 1; while (s <= nb && body[s] ~ /^[ \t]*$/) s++
      e = nb; while (e >= s && body[e] ~ /^[ \t]*$/) e--
      if (s <= e) {
        for (i = s; i <= e; i++) print body[i]
        print ""
      }
    }
    BEGIN { state = "pre"; nb = 0; extra = 0 }
    # pre = 最初の `## [` より前 / unrel = 最上段の Unreleased の中 / rest = それ以降
    state == "pre" && /^## \[/ {
      if (is_unreleased($0)) { state = "unrel"; next }
      emit_release(); state = "rest"; print; next
    }
    state == "unrel" && /^## \[/ { emit_release(); state = "rest" }
    state == "unrel" { body[++nb] = $0; next }
    state == "rest" && is_unreleased($0) { extra++ }
    { print; last = $0 }
    END {
      if (state == "unrel") emit_release()
      if (state == "pre") {
        # 版の節が 1 つも無い: 末尾へ足す
        if (NR > 0 && last !~ /^[ \t]*$/) print ""
        emit_release()
      }
      if (extra > 0) {
        printf "WARN: CHANGELOG の最上段より下に ## [Unreleased] が %d 個ある（#1593 の規則違反。最上段の 1 つだけ扱った）\n", extra > "/dev/stderr"
      }
    }
  ' "$file" > "$tmp" && mv -f "$tmp" "$file"
}
