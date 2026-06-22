const KEY = 'tako-remote';

function load() {
  try {
    return JSON.parse(localStorage.getItem(KEY)) || { machines: [], activeId: null };
  } catch {
    return { machines: [], activeId: null };
  }
}

function save(data) {
  localStorage.setItem(KEY, JSON.stringify(data));
}

export function getMachines() {
  return load().machines;
}

export function addMachine({ id, name, host, token }) {
  const data = load();
  const idx = data.machines.findIndex(m => m.id === id);
  const entry = { id, name: name || id, host, token, lastSeen: Date.now() };
  if (idx >= 0) {
    Object.assign(data.machines[idx], entry);
  } else {
    data.machines.push(entry);
  }
  save(data);
  return entry;
}

export function removeMachine(id) {
  const data = load();
  data.machines = data.machines.filter(m => m.id !== id);
  if (data.activeId === id) data.activeId = null;
  save(data);
}

export function getActiveMachine() {
  const data = load();
  return data.machines.find(m => m.id === data.activeId) || null;
}

export function setActiveMachine(id) {
  const data = load();
  data.activeId = id;
  const m = data.machines.find(x => x.id === id);
  if (m) m.lastSeen = Date.now();
  save(data);
}
