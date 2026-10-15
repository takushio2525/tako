// 鍵らしき文字の確認（#2050）: スマホの入力に API キー・パスワードらしきものがあれば送る前に確かめる
//
// ## 判定は PC 側の 1 実装
//
// 判定は PC の tako（`tako_core::paste_guard::scan_with`）が行う。ここに判定は 1 つも無い。
// daemon は `/api/panes/<id>/input` を「人の入力」として PC の貼り付けと同じ段に従わせ、
// - 確認の段で当たったら**送らずに** 409 と伏せ字（規則名・何行目・伏せ字だけ。本文は返らない）
// - 警告だけの段なら送った上で 200 に伏せ字の警告
// を返す。409 ならこのシートで「やめる / 送る」を出し、送るときだけ `confirm_secret: true` で
// 送り直す（daemon はそれを「人が確かめた」として送る）。やめたら呼び手が入力欄へ文字を戻す。
//
// 規則の名前（`github_token` 等）を人に見せる言い方へ直すだけの表をここに持つ（PC の GUI の
// `ui_text::paste_guard::rule_label` と同じ言い方。知らない規則は綴りのまま出す）。
import { useState } from 'preact/hooks';

const RULE_LABELS = {
  anthropic_api_key: 'Anthropic の API キー',
  openai_api_key: 'OpenAI の API キー',
  github_token: 'GitHub のトークン',
  github_pat: 'GitHub の個人用トークン',
  gitlab_token: 'GitLab のトークン',
  slack_token: 'Slack のトークン',
  slack_webhook: 'Slack の Webhook URL',
  discord_webhook: 'Discord の Webhook URL',
  aws_access_key: 'AWS のアクセスキー',
  google_api_key: 'Google の API キー',
  stripe_key: 'Stripe のキー',
  npm_token: 'npm のトークン',
  huggingface_token: 'Hugging Face のトークン',
  sendgrid_key: 'SendGrid の API キー',
  private_key: '秘密鍵',
  jwt: 'JWT（認証トークン）',
  url_credentials: 'URL に埋め込んだパスワード',
  assignment: 'パスワード・トークンの値',
  high_entropy: 'ランダムな文字列（鍵の可能性）',
};

/** 並べる上限（超えたぶんは「ほか N 件」） */
const MAX_ROWS = 5;

export function ruleLabel(rule) {
  return RULE_LABELS[rule] || rule;
}

function Findings({ guard }) {
  const findings = (guard && guard.findings) || [];
  return (
    <ul class="pg-findings" data-testid="paste-guard-findings">
      {findings.slice(0, MAX_ROWS).map((f, i) => (
        <li key={i}>
          <span class="pg-rule">{ruleLabel(f.rule)}・{f.line} 行目</span>
          <code class="pg-masked">{f.masked}</code>
        </li>
      ))}
      {findings.length > MAX_ROWS && <li class="pg-more">ほか {findings.length - MAX_ROWS} 件</li>}
    </ul>
  );
}

/** 送る前の確認シート（既定はやめる側。背景を押してもやめる） */
export function PasteGuardSheet({ guard, onAnswer }) {
  return (
    <div class="sheet-backdrop" onClick={() => onAnswer(false)}>
      <div class="sheet" data-testid="paste-guard-sheet" onClick={e => e.stopPropagation()}>
        <div class="sheet-head">
          <span class="sheet-title">鍵らしき文字が入っています</span>
          <button class="sheet-close" aria-label="閉じる" onClick={() => onAnswer(false)}>×</button>
        </div>
        <p class="pg-note">
          このまま送ると、PC のペインで動いているプログラム（claude・シェルなど）へそのまま届きます。
        </p>
        <Findings guard={guard} />
        {guard && guard.truncated && <p class="pg-note">先頭だけを確かめました（それより後ろは見ていません）</p>}
        <div class="cmd-confirm-actions">
          <button class="btn btn-primary" data-testid="paste-guard-cancel" onClick={() => onAnswer(false)}>
            やめる
          </button>
          <button class="btn" data-testid="paste-guard-send" onClick={() => onAnswer(true)}>
            送る
          </button>
        </div>
      </div>
    </div>
  );
}

/** 警告だけの段で送ったときの通知（押すと消える・数秒で消える） */
export function PasteGuardNotice({ guard, onClose }) {
  return (
    <div class="pg-notice" data-testid="paste-guard-notice" onClick={onClose}>
      <div class="pg-notice-title">鍵らしき文字を含む入力をそのまま送りました</div>
      <Findings guard={guard} />
    </div>
  );
}

/**
 * 送信の包み。`sendText(client, paneId, text, newline)` は送れたら true、確認で「やめる」なら
 * false を返す（呼び手は false なら入力欄へ文字を戻す）。それ以外の失敗はそのまま投げる。
 * `sheet` / `notice` を画面のどこかに置く
 */
export function usePasteGuard() {
  const [ask, setAsk] = useState(null);
  const [notice, setNotice] = useState(null);

  async function sendText(client, paneId, text, newline = true) {
    try {
      const r = await client.input(paneId, text, newline);
      if (r && r.paste_guard) {
        setNotice(r.paste_guard);
        setTimeout(() => setNotice(cur => (cur === r.paste_guard ? null : cur)), 8000);
      }
      return true;
    } catch (e) {
      const guard = e && e.status === 409 && e.body && e.body.paste_guard;
      if (!guard) throw e;
      const ok = await new Promise(resolve => setAsk({ guard, resolve }));
      setAsk(null);
      if (!ok) return false;
      await client.input(paneId, text, newline, { confirmSecret: true });
      return true;
    }
  }

  const sheet = ask ? <PasteGuardSheet guard={ask.guard} onAnswer={ok => ask.resolve(ok)} /> : null;
  const noticeEl = notice ? <PasteGuardNotice guard={notice} onClose={() => setNotice(null)} /> : null;
  return { sendText, sheet, notice: noticeEl };
}
