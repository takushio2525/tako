// ANSI SGR パーサ — tmux capture-pane -e の出力行を装飾セグメント列に変換する。
//
// 完全な端末エミュレーションではない。capture-pane -e の出力は「行テキスト + SGR
// （色・太字等）」だけで構成される（カーソル移動等は含まれない）ため、SGR のみ解釈し、
// その他のエスケープシーケンス（OSC・その他 CSI）は安全に読み飛ばす。
// 描画は呼び出し側（リーダービュー）が segment 列から DOM を組み立てる。

/** SGR 状態の初期値。fg/bg: null=デフォルト / 0-255=パレット / [r,g,b]=truecolor */
export function defaultSgrState() {
  return {
    fg: null,
    bg: null,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    reverse: false,
    strike: false,
  };
}

function isDefaultState(s) {
  return (
    s.fg === null && s.bg === null && !s.bold && !s.dim &&
    !s.italic && !s.underline && !s.reverse && !s.strike
  );
}

/**
 * 1 行をパースして装飾セグメント列に分解する。
 * `state` から開始し、行末時点の状態を `state` として返す（行を跨ぐ SGR 継続に対応）。
 * @returns {{ segments: Array<{text: string, style: object|null}>, state: object }}
 *   style は装飾なしの場合 null（描画側の最適化用）
 */
export function parseAnsiLine(line, state = defaultSgrState()) {
  const segments = [];
  let cur = { ...state };
  let text = '';
  const flush = () => {
    if (text) {
      segments.push({ text, style: isDefaultState(cur) ? null : { ...cur } });
      text = '';
    }
  };
  let i = 0;
  while (i < line.length) {
    const ch = line[i];
    if (ch === '\x1b') {
      const next = line[i + 1];
      if (next === '[') {
        // CSI: ESC [ パラメータ... 終端バイト(0x40-0x7E)
        let j = i + 2;
        while (j < line.length && !(line[j] >= '@' && line[j] <= '~')) j++;
        if (j < line.length) {
          if (line[j] === 'm') {
            flush();
            applySgr(cur, line.slice(i + 2, j));
          }
          i = j + 1;
        } else {
          i = line.length;
        }
        continue;
      }
      if (next === ']') {
        // OSC: ESC ] ... (BEL | ESC \)
        let j = i + 2;
        while (j < line.length && line[j] !== '\x07' && !(line[j] === '\x1b' && line[j + 1] === '\\')) j++;
        i = j >= line.length ? line.length : line[j] === '\x07' ? j + 1 : j + 2;
        continue;
      }
      // その他の ESC シーケンスは 2 文字読み飛ばす
      i += 2;
      continue;
    }
    text += ch;
    i++;
  }
  flush();
  return { segments, state: cur };
}

/** SGR パラメータ文字列（`1;38;5;208` 等）を状態へ適用する（破壊的） */
function applySgr(st, params) {
  const parts = params === '' ? [0] : params.split(';').map(p => (p === '' ? 0 : parseInt(p, 10)));
  let i = 0;
  while (i < parts.length) {
    const n = parts[i];
    if (Number.isNaN(n)) { i++; continue; }
    if (n === 0) {
      Object.assign(st, defaultSgrState());
    } else if (n === 1) st.bold = true;
    else if (n === 2) st.dim = true;
    else if (n === 3) st.italic = true;
    else if (n === 4) st.underline = true;
    else if (n === 7) st.reverse = true;
    else if (n === 9) st.strike = true;
    else if (n === 22) { st.bold = false; st.dim = false; }
    else if (n === 23) st.italic = false;
    else if (n === 24) st.underline = false;
    else if (n === 27) st.reverse = false;
    else if (n === 29) st.strike = false;
    else if (n >= 30 && n <= 37) st.fg = n - 30;
    else if (n === 39) st.fg = null;
    else if (n >= 40 && n <= 47) st.bg = n - 40;
    else if (n === 49) st.bg = null;
    else if (n >= 90 && n <= 97) st.fg = n - 90 + 8;
    else if (n >= 100 && n <= 107) st.bg = n - 100 + 8;
    else if (n === 38 || n === 48) {
      // 拡張色: 38;5;N（256 色）/ 38;2;R;G;B（truecolor）
      const mode = parts[i + 1];
      if (mode === 5 && i + 2 < parts.length) {
        const c = parts[i + 2];
        if (c >= 0 && c <= 255) {
          if (n === 38) st.fg = c; else st.bg = c;
        }
        i += 3;
        continue;
      }
      if (mode === 2 && i + 4 < parts.length) {
        const rgb = [parts[i + 2], parts[i + 3], parts[i + 4]].map(v => Math.max(0, Math.min(255, v || 0)));
        if (n === 38) st.fg = rgb; else st.bg = rgb;
        i += 5;
        continue;
      }
      // 不正な拡張色指定は以降を読み捨てて終了（安全側）
      break;
    }
    i++;
  }
}

/**
 * fg/bg の色値を CSS 色文字列に解決する。
 * 0-15 はテーマの CSS 変数（--tc0〜--tc15）、16-255 は xterm 256 色パレット計算、
 * [r,g,b] は truecolor。null は null を返す（デフォルト色 = CSS 継承に任せる）
 */
export function colorToCss(c) {
  if (c === null || c === undefined) return null;
  if (Array.isArray(c)) return `rgb(${c[0]},${c[1]},${c[2]})`;
  if (c < 16) return `var(--tc${c})`;
  if (c < 232) {
    // 6x6x6 カラーキューブ
    const v = c - 16;
    const steps = [0, 95, 135, 175, 215, 255];
    const r = steps[Math.floor(v / 36)];
    const g = steps[Math.floor(v / 6) % 6];
    const b = steps[v % 6];
    return `rgb(${r},${g},${b})`;
  }
  // グレースケール 232-255
  const gray = 8 + (c - 232) * 10;
  return `rgb(${gray},${gray},${gray})`;
}
