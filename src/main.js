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

let composing = null; // 'add' | 'bulk' | null

function fillWorkspaces(select) {
  const current = select.value;
  select.innerHTML = '';
  for (const w of state.all_workspaces || []) {
    const o = document.createElement('option');
    o.value = w.id;
    o.textContent = w.name;
    select.appendChild(o);
  }
  const personal = (state.all_workspaces || []).find((w) => w.name.toLowerCase() === 'personal');
  select.value = current || (personal ? personal.id : select.value);
  select.hidden = (state.all_workspaces || []).length < 2;
}

/** Tasks a list will create: non-empty lines that aren't indented under another one. */
function countTasks(text) {
  const lines = text.split(/\r?\n/).filter((l) => l.trim());
  if (!lines.length) return 0;
  const base = Math.min(...lines.map((l) => l.length - l.trimStart().length));
  return lines.filter((l) => l.length - l.trimStart().length === base).length;
}

function setComposing(mode) {
  composing = state && state.signed_in ? mode : null;
  $('compose-add').hidden = composing !== 'add';
  $('compose-bulk').hidden = composing !== 'bulk';
  $('btn-add').classList.toggle('on', composing === 'add');
  $('btn-bulk').classList.toggle('on', composing === 'bulk');
  if (composing === 'add') setTimeout(() => $('add-text').focus(), 30);
  if (composing === 'bulk') setTimeout(() => $('bulk-text').focus(), 30);
}

function flash(message) {
  const el = $('added');
  el.textContent = message;
  el.hidden = false;
  clearTimeout(flash.t);
  flash.t = setTimeout(() => (el.hidden = true), 3500);
}

async function addTasks(text, ws, day) {
  const res = await call('add_tasks', { text, workspace: Number(ws) || null, day });
  const n = res.created.length;
  flash(n === 1 ? `Added “${res.created[0].title}” to ${res.workspace.name}` : `Added ${n} tasks to ${res.workspace.name}`);
  return res;
}

function render() {
  if (!state) return;
  $('signed-out').hidden = state.signed_in;
  $('signed-in').hidden = !state.signed_in;
  $('btn-add').hidden = $('btn-bulk').hidden = !state.signed_in;
  if (!state.signed_in) {
    setComposing(null);
    return;
  }
  fillWorkspaces($('add-ws'));
  fillWorkspaces($('bulk-ws'));
  $('add-shortcut').textContent = state.shortcut || '';
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

$('btn-add').onclick = () => setComposing(composing === 'add' ? null : 'add');
$('btn-bulk').onclick = () => setComposing(composing === 'bulk' ? null : 'bulk');
document.querySelectorAll('[data-close]').forEach((b) => (b.onclick = () => setComposing(null)));
$('add-form').onsubmit = async (e) => {
  e.preventDefault();
  const text = $('add-text').value.trim();
  if (!text) return;
  await addTasks(text, $('add-ws').value, $('add-day').value);
  $('add-text').value = '';
  $('add-text').focus();
};
$('bulk-text').oninput = () => {
  const n = countTasks($('bulk-text').value);
  $('bulk-submit').disabled = n === 0;
  $('bulk-submit').textContent = n === 0 ? 'Add' : n === 1 ? 'Add 1 task' : `Add ${n} tasks`;
};
$('bulk-text').onkeydown = (e) => {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) $('bulk-form').requestSubmit();
};
$('bulk-form').onsubmit = async (e) => {
  e.preventDefault();
  const text = $('bulk-text').value;
  if (!countTasks(text)) return;
  await addTasks(text, $('bulk-ws').value, $('bulk-day').value);
  $('bulk-text').value = '';
  $('bulk-text').oninput();
  setComposing(null);
};
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && composing) setComposing(null);
});

listen('flowly://compose', (e) => setComposing(String(e.payload)));
listen('flowly://state', load);
listen('flowly://error', (e) => showError(String(e.payload)));
window.addEventListener('focus', () => invoke('reload'));
setInterval(tick, 1000);
load();
