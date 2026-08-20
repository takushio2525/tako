#!/usr/bin/env bash
# launch-services.sh — Launch Services（macOS）の登録を扱う共有ヘルパ（Issue #837）
#
# 使い方: source "$REPO_ROOT/scripts/lib/launch-services.sh"
#
# ディスク上に同じ identity の tako.app が 2 つあると Launch Services は両方を登録し、
# Finder の「このアプリケーションで開く」に tako が 2 つ並ぶ（/Applications 側とビルド
# 出力側でバージョン表記まで食い違って紛らわしい）。macOS 26 / Darwin 25.4 での実測:
#
#   - `lsregister -u` だけでは足りない。ファイルを一切触らなくても**1 分前後で自動的に
#     再登録される**（実測 48〜70 秒。LS は Spotlight とは別に、自力でディスク上の
#     .app を拾う）
#   - 置き場所を変える回避は効かない。親ディレクトリを `*.noindex` にしても、
#     `.metadata_never_index` を置いても、`chflags hidden` を立てても 1 分以内に登録された。
#     ドット始まりの隠しディレクトリは 97 秒後まで 0 件だったが 133 秒後に登録された
#     （短い観測では騙される）。`.noindex` 配下は Spotlight の importer 属性
#     （kMDItemCFBundleIdentifier）が付かないまま LS には登録されていた
#   - 逆に実体を消しても LS の登録だけは残り、残骸として候補に出続ける
#
# → **実体を消す + `lsregister -u` の両方**をやったときだけ恒久的に消える
#   （存在しないパスは再登録されないため）。ビルド出力を置いたままにできる回避策は
#   無いので、install / リリースを終えた時点で出力を片付けるのが唯一の恒久対策。
#   この不変条件は tako-app の open_files.rs の番犬テストが機械検証している。

# tako の正本。Finder の「このアプリケーションで開く」に出ていてよいのはここだけ
LS_CANONICAL_APP=${LS_CANONICAL_APP:-/Applications/tako.app}

# 検証用に差し替え可能にしておく（存在しないパスを指すと LS 操作はすべて no-op になる。
# モックテストが本番の LS データベースに触らないようにするため）
LSREGISTER=${TAKO_LSREGISTER:-/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Support/lsregister}

ls_available() { [[ -x "$LSREGISTER" ]]; }

# 明示登録（#708）。/Applications への配置でも通常は自動で登録されるが、
# rm -rf → cp -R の差し替えでは古い登録が残り「このアプリケーションで開く」の
# 候補・アイコンが更新されないことがあるので、決定論的に登録し直す
ls_register() {
  ls_available || return 0
  "$LSREGISTER" -f "$1" || true
}

ls_unregister() {
  ls_available || return 0
  "$LSREGISTER" -u "$1" >/dev/null 2>&1 || true
}

# LS に登録されている tako.app のパスを列挙する（dump の `path: /... (0x1234)` 形式から抜く）
ls_registered_tako_paths() {
  ls_available || return 0
  "$LSREGISTER" -dump 2>/dev/null | awk '
    /^[[:space:]]*path:/ {
      line = $0
      sub(/^[[:space:]]*path:[[:space:]]*/, "", line)
      sub(/[[:space:]]*\([^)]*\)[[:space:]]*$/, "", line)
      if (line ~ /\/tako\.app$/) print line
    }' | sort -u
}

# 実体が無い登録を外す（存在しないパスは再登録されないので恒久的に効く）。
# ディスク上に残っている別の tako.app は勝手に消さず、掃除手順を提示するだけにする
ls_sweep_stale_registrations() {
  ls_available || return 0
  local registered note
  local leftovers=()
  while IFS= read -r registered; do
    [[ -z "$registered" ]] && continue
    [[ "$registered" == "$LS_CANONICAL_APP" ]] && continue
    if [[ -d "$registered" ]]; then
      leftovers+=("$registered")
    else
      ls_unregister "$registered"
      echo "    登録解除（実体なし）: $registered"
    fi
  done < <(ls_registered_tako_paths)

  ((${#leftovers[@]} > 0)) || return 0
  echo "警告: $LS_CANONICAL_APP 以外にも tako.app がディスク上にあります（#837）。" >&2
  echo "      「このアプリケーションで開く」の候補を宣言しているものは Finder に重複して並びます:" >&2
  for registered in "${leftovers[@]}"; do
    # CFBundleDocumentTypes を宣言していない古いバンドルは、登録されても候補には出ない
    if /usr/libexec/PlistBuddy -c 'Print :CFBundleDocumentTypes' \
      "$registered/Contents/Info.plist" >/dev/null 2>&1; then
      note="候補に出る"
    else
      note="候補には出ない"
    fi
    echo "        ${registered}（${note}）" >&2
    echo "          rm -rf \"$registered\" && \"$LSREGISTER\" -u \"$registered\"" >&2
  done
}

# ビルド出力の tako.app を後始末する（#837）。install / リリースが終わって用済みに
# なった時点で呼ぶ。**実体を消してから**登録を外す（順序が逆だと、消す前の -u が
# 約 40 秒後に取り消される）
#   $1 = 消す .app のパス
#   $2 = その親ディレクトリ（省略可。空になったときだけ rmdir する）
ls_drop_build_output() {
  local app=$1 dist=${2:-}
  if [[ -d "$app" && "$app" == */tako.app ]]; then
    rm -rf "$app"
    echo "==> ビルド出力を削除（Finder の候補を二重化させない。#837）: $app"
    if [[ -n "$dist" ]]; then
      rmdir "$dist" 2>/dev/null || true  # 他の配布物（zip 等）が残っていれば消えない
    fi
  fi
  ls_sweep_stale_registrations
}
