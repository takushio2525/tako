// agent 種別アイコン — カンプの ✳ (claude) / >_ (codex) / ◒ (agy) を
// SVG/CSS で再現（絵文字禁止）。✳ は特殊文字として許容する（Unicode 記号）。
export function AgentIcon({ type, small }) {
  const cls = `agent-icon ${type || 'plain'}${small ? ' agent-icon-sm' : ''}`;
  switch (type) {
    case 'claude':
      return <span class={cls}>{'✳'}</span>;
    case 'codex':
      return <span class={cls}>&gt;_</span>;
    case 'agy':
      return <span class={cls}>{'◒'}</span>;
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
