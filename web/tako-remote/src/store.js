// #283: 旧 machines / token の localStorage 保存は全廃した。
// PWA は daemon 自身から配信され（同一 origin）、認証はサーバー側の
// 機器ペアリングが行うため、クライアントに保存する接続情報は存在しない。
// 残るのはペアリング要求時に使うデバイス表示名の記憶だけ。
const NAME_KEY = 'tako-remote-device-name';

export function getDeviceName() {
  try {
    return localStorage.getItem(NAME_KEY) || '';
  } catch {
    return '';
  }
}

export function setDeviceName(name) {
  try {
    localStorage.setItem(NAME_KEY, name);
  } catch {
    // プライベートブラウズ等で保存できなくても動作は継続する
  }
}

// 旧バージョンが残した machines / token を掃除する（一度だけ走れば十分）
export function cleanupLegacyStore() {
  try {
    localStorage.removeItem('tako-remote');
  } catch {
    // 失敗しても害はない
  }
}
