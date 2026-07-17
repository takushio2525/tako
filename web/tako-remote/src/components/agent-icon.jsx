// agent 種別アイコン — カンプのデザインを SVG で再現（UI 絵文字禁止）。
// claude: 八芒星（✳ 相当）、codex: >_（テキスト）、agy: 半円（◒ 相当）

function ClaudeStar({ size }) {
  const s = size || 14;
  return (
    <svg width={s} height={s} viewBox="0 0 16 16" fill="none">
      <path d="M8 1L9.2 6.1L14 4.5L10.5 8L14 11.5L9.2 9.9L8 15L6.8 9.9L2 11.5L5.5 8L2 4.5L6.8 6.1Z" fill="currentColor" />
    </svg>
  );
}

function AgyHalf({ size }) {
  const s = size || 13;
  return (
    <svg width={s} height={s} viewBox="0 0 16 16" fill="none">
      <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.8" />
      <path d="M8 2A6 6 0 0 1 8 14Z" fill="currentColor" />
    </svg>
  );
}

export function AgentIcon({ type, small }) {
  const cls = `agent-icon ${type || 'plain'}${small ? ' agent-icon-sm' : ''}`;
  switch (type) {
    case 'claude':
      return <span class={cls}><ClaudeStar size={small ? 10 : 14} /></span>;
    case 'codex':
      return <span class={cls}>&gt;_</span>;
    case 'agy':
      return <span class={cls}><AgyHalf size={small ? 10 : 13} /></span>;
    default:
      return (
        <span class={cls}>
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
            <rect x="2" y="2" width="8" height="8" rx="2" stroke="currentColor" stroke-width="1.5" />
          </svg>
        </span>
      );
  }
}

export function agentColor(type) {
  switch (type) {
    case 'claude': return 'var(--claude)';
    case 'codex': return 'var(--codex)';
    case 'agy': return 'var(--agy)';
    default: return 'var(--fg3)';
  }
}

export function agentDarkColor(type) {
  switch (type) {
    case 'claude': return '#1B0E09';
    case 'codex': return '#0B0D10';
    case 'agy': return '#0A1A33';
    default: return '#0B0D10';
  }
}
