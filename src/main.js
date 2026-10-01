const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let state = null;

function hm(seconds) {
  const m = Math.floor(seconds / 60);
  return `${Math.floor(m / 60)}:${String(m % 60).padStart(2, '0')}`;
}

function showError(message) {
  const el = $('error');
  el.textContent = message;
  el.hidden = !message;
  if (message) setTimeout(() => (el.hidden = true), 6000);
}

async function call(cmd, args) {
  try {
    showError('');
    return await invoke(cmd, args);
  } catch (e) {
    showError(String(e));
    throw e;
  }
}

function render() {
  if (!state) return;
  $('signed-out').hidden = state.signed_in;
  $('signed-in').hidden = !state.signed_in;
  if (!state.signed_in) return;
  $('email').textContent = state.email || '';

  const t = state.timer;
  $('timer').classList.toggle('idle', !t);
  $('stop').hidden = !t;
  $('what').textContent = t ? (t.task_title || t.description || 'Untitled work') : 'No timer running';
  tick();

  const ws = $('ws');
  const current = ws.value;
  ws.innerHTML = '';
  for (const w of state.workspaces) {
    const o = document.createElement('option');
    o.value = w.id;
    o.textContent = w.name;
    ws.appendChild(o);
  }
  if (current) ws.value = current;
  ws.hidden = state.workspaces.length < 2;

  const list = $('tasks');
  list.innerHTML = '';
  for (const task of state.tasks) {
    const li = document.createElement('li');
    const running = t && t.task === task.id;
    li.className = running ? 'running' : '';
    const title = document.createElement('span');
    title.className = 'title';
    title.textContent = task.title;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = running ? 'danger round' : 'round';
    btn.textContent = running ? '■' : '▶';
    btn.title = running ? 'Stop' : 'Start timer';
    btn.onclick = async () => {
      if (running) await call('stop');
      else await call('start', { workspace: task.workspace, task: task.id });
    };
    li.append(btn, title);
    list.appendChild(li);
  }
  $('empty').hidden = state.tasks.length > 0;
}

function tick() {
  const t = state && state.timer;
  if (!t) {
    $('elapsed').textContent = '0:00';
    return;
  }
  $('elapsed').textContent = hm((Date.now() - new Date(t.started_at).getTime()) / 1000);
}

async function load() {
  state = await invoke('get_state');
  render();
}

$('signin').onclick = async () => {
  $('signin-wait').hidden = false;
  try {
    await call('sign_in');
  } finally {
    $('signin-wait').hidden = true;
  }
};
$('signout').onclick = () => call('sign_out');
$('stop').onclick = () => call('stop');
$('open').onclick = () => invoke('open_flowly', { path: '/day' });
$('quick').onsubmit = async (e) => {
  e.preventDefault();
  const ws = Number($('ws').value || (state.workspaces[0] && state.workspaces[0].id));
  if (!ws) return;
  await call('start', { workspace: ws, task: null, description: $('desc').value.trim() });
  $('desc').value = '';
};

listen('flowly://state', load);
listen('flowly://error', (e) => showError(String(e.payload)));
window.addEventListener('focus', () => invoke('reload'));
setInterval(tick, 1000);
load();
