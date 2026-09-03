// Claude 公式 Remote Control への送り出し（#1077 / エピック #1059 柱 1-C）
//
// daemon が各ペインに載せる `remote_link` を読んで
//   - 繋がっていれば「Claude で開く」（claude.ai / Claude アプリでその会話が開く）
//   - 繋がっていなければ**理由 + PC 側で有効化する方法**を出す
// を表示する。
//
// ## ここで守っている線
//
// - **押せるボタンは「開く」だけ**。opt-in は PC 側の設定なので、スマホから
//   勝手に書き換えるボタンは作らない（設計の正本 §9-C）。案内は文字とコマンドだけ。
// - **理由の文言は Rust 側が正**（`claude_remote_link::LinkGuidance`）。PWA に i18n
//   機構が無いので、表示言語の解決は daemon で終わらせて文字列で受け取る。
//   ここで文言を組み立てると tako 本体の設定（#435）と食い違う。
// - **URL を組み立てない**。`state === 'connected'` のときだけ daemon が URL を持つので、
//   PWA は受け取ったものをそのまま開く（id から組むと上流の書式変更で壊れたリンクを出す）。
import { useState } from 'preact/hooks';

/// 公式リンクで開けるか。**URL が実在することまで見る**（state だけ信じない）
export function isRemoteConnected(link) {
  return !!(link && link.state === 'connected' && link.url);
}

/// このペインに Remote Control の話が関係あるか。
/// 素のシェルには daemon が `remote_link` を付けないので、その場合は何も出さない
export function hasRemoteLink(link) {
  return !!(link && link.state);
}

/// 会話がどの tako アカウント配下かを示すチップ。
///
/// スマホが別アカウントで claude.ai にログインしていると**一覧に出ない**ので、
/// これが無いと「押しても出てこない」を切り分けられない（設計の正本 §5）
export function AccountChip({ link }) {
  if (!link || !link.account_label) return null;
  return (
    <span class="card-chip card-chip-account" title="このアカウントで claude.ai にログインしていると会話が出ます">
      {link.account_label}
    </span>
  );
}

function ExternalIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M6.5 3.5H3.5v9h9v-3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
      <path d="M9.5 3.5h3v3M12.5 3.5L7.5 8.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    </svg>
  );
}

/// 「Claude で開く」。**新しいタブ / アプリで開く**ので `target="_blank"`。
/// カードの中に置くのでタップがカード自身の onClick へ伝播しないよう止める
export function ClaudeOpenLink({ link, block = false }) {
  if (!isRemoteConnected(link)) return null;
  return (
    <a
      class={`claude-open${block ? ' claude-open-block' : ''}`}
      href={link.url}
      target="_blank"
      rel="noopener noreferrer"
      onClick={(e) => e.stopPropagation()}
    >
      <span class="claude-open-label">Claude で開く</span>
      <ExternalIcon />
    </a>
  );
}

function CopyButton({ text }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      class="remote-link-copy"
      onClick={(e) => {
        e.stopPropagation();
        // 失敗（権限なし / 非セキュアコンテキスト）は黙って諦める。
        // コマンドは画面に出ているので写して使える
        navigator.clipboard?.writeText(text).then(
          () => { setCopied(true); setTimeout(() => setCopied(false), 1800); },
          () => {},
        );
      }}
    >
      {copied ? 'コピーしました' : 'コピー'}
    </button>
  );
}

/// 理由 + 次の一手 + PC 側で有効化するコマンド。
/// `reason` / `next_step` / `enable_command` は daemon が表示言語で解決済み
export function RemoteLinkReason({ link }) {
  if (!link) return null;
  return (
    <div class="remote-link-reason">
      {link.reason && <p class="remote-link-why">{link.reason}</p>}
      {link.next_step && <p class="remote-link-next">{link.next_step}</p>}
      {link.enable_command && (
        <div class="remote-link-cmd">
          <code>{link.enable_command}</code>
          <CopyButton text={link.enable_command} />
        </div>
      )}
    </div>
  );
}

/// ペインカードの 1 行（#1077）。
///
/// 繋がっていれば幅いっぱいの「Claude で開く」、繋がっていなければ
/// **1 行のたたんだ表示**にして、タップで理由を開く。一覧に理由を常時展開すると
/// カードが縦に伸びて「どれがどれだか分かる」（#621 の設計ゴール）を壊す
export function RemoteLinkRow({ link }) {
  const [open, setOpen] = useState(false);
  if (!hasRemoteLink(link)) return null;
  if (isRemoteConnected(link)) {
    return (
      <div class="remote-link-row connected">
        <ClaudeOpenLink link={link} block />
      </div>
    );
  }
  return (
    <div class="remote-link-row">
      <button
        class="remote-link-toggle"
        aria-expanded={open ? 'true' : 'false'}
        onClick={(e) => { e.stopPropagation(); setOpen(!open); }}
      >
        <span class="remote-link-state">Claude アプリ: 未接続</span>
        <span class="remote-link-more">{open ? '閉じる' : '理由'} {open ? '▴' : '▾'}</span>
      </button>
      {open && <RemoteLinkReason link={link} />}
    </div>
  );
}
