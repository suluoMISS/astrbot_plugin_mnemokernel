const $ = (id) => document.getElementById(id);
const bridge = window.AstrBotPluginPage;
let scopeOffset = 0, offset = 0, scopeSerial = 0, serial = 0;
const limit = 20;
const labels = {active:'有效', archived:'已归档', superseded:'已替代', purged:'正文已清除', redacted:'已脱敏', default:'默认', opted_out:'已退出采集', ok:'命中', not_found:'未找到', declined:'已拒绝', error:'失败', candidate:'候选'};
const fields = {id:'记录 ID',scope_id:'会话标识',time_ms:'时间戳',state:'状态',kind:'类型',confidence:'置信度',activation:'激活度',version:'版本',sender:'发送者',sender_is_bot:'机器人消息',reply_to_source_key:'回复来源',open_loops_json:'未完成事项',decision_reason:'判断说明',depth:'回忆深度'};
const showError = (error) => { $('notice').textContent = error.message || '读取失败，请重试'; };
function pageCount(result) { return `共 ${result.total} 条 · 第 ${Math.floor(result.offset / limit) + 1} 页`; }
function node(tag, text) { const el = document.createElement(tag); el.textContent = text; return el; }
function clearRecords(message) {
  serial++; $('records').replaceChildren(); $('count').textContent = '';
  $('prev').disabled = true; $('next').disabled = true; $('notice').textContent = message;
}
async function loadRecords() {
  clearRecords('正在读取…');
  if (!$('scope').value) { $('notice').textContent = '请选择会话；没有会话时，请先启用采集并产生新消息。'; return; }
  const ticket = serial;
  try {
    const result = await bridge.apiGet('inspector/browse', {collection:$('collection').value,scope_id:$('scope').value,query:$('query').value,limit,offset});
    if (ticket !== serial) return;
    $('notice').textContent = result.items.length ? '仅显示当前会话的数据。' : '没有匹配记录。日记需要启用整理，记忆卡由日记生成；也可切换分类或清空搜索。';
    for (const item of result.items) {
      const card = document.createElement('article');
      card.append(node('h3', item.title || item.kind || (item.sender_is_bot ? '机器人' : item.sender) || '记录'));
      card.append(node('p', `${new Date(item.time_ms).toLocaleString()} · ${labels[item.state] || item.state}`));
      card.append(node('pre', item.content ?? '正文已清除，不再可读。'));
      const details = document.createElement('details'); details.append(node('summary','查看记录详情'));
      for (const [key,value] of Object.entries(item)) {
        if (key === 'content' || key === 'title' || value === null) continue;
        details.append(node('p', `${fields[key] || key}：${value}`));
      }
      card.append(details); $('records').append(card);
    }
    $('count').textContent = pageCount(result);
    $('prev').disabled = offset === 0;
    $('next').disabled = offset + limit >= result.total;
  } catch(error) { if (ticket === serial) showError(error); }
}
async function loadScopes() {
  const ticket = ++scopeSerial;
  const previous = $('scope').value;
  $('scope').replaceChildren(); $('scope').disabled = true;
  $('scope-prev').disabled = true; $('scope-next').disabled = true;
  clearRecords('正在读取会话…');
  try {
    const result = await bridge.apiGet('inspector/browse', {collection:'scopes',query:$('scope-query').value,limit,offset:scopeOffset});
    if (ticket !== scopeSerial) return;
    for (const item of result.items) {
      const option = node('option', `${item.platform_id} · ${item.conversation_kind === 'group' ? '群聊' : '私聊'} ${item.session_id} · ${item.persona_id} · ${labels[item.state] || item.state}`);
      option.value = item.scope_id; $('scope').append(option);
    }
    if (result.items.some(item => item.scope_id === previous)) $('scope').value = previous;
    $('scope').disabled = !result.items.length;
    $('scope-count').textContent = pageCount(result);
    $('scope-prev').disabled = scopeOffset === 0;
    $('scope-next').disabled = scopeOffset + limit >= result.total;
    offset = 0; await loadRecords();
  } catch(error) { if (ticket === scopeSerial) showError(error); }
}
async function refresh() {
  $('refresh').disabled = true;
  scopeSerial++; clearRecords('正在检查状态…'); $('scope').replaceChildren();
  try {
    const status = await bridge.apiGet('inspector/status');
    $('status').textContent = `内核${status.available ? '就绪' : '未就绪'} · 浏览接口${status.inspector_available ? '可用' : '不可用'} · 采集${status.capture_enabled ? '已开启' : '未开启'} · 日记${status.diary_enabled ? '已开启' : '未开启'} · 回忆${status.recall_enabled ? '已开启' : '未开启'} · 采集异常 ${status.capture_errors} 次`;
    $('database').textContent = status.database_path ? `数据库位置（AstrBot 运行主机）：${status.database_path}` : '插件尚未打开数据库。';
    if (!status.enabled) throw new Error('插件已在配置中停用。');
    if (!status.available) throw new Error('原生内核未就绪，请检查插件日志，并安装匹配操作系统的安装包。');
    if (!status.inspector_available) throw new Error('当前进程加载的原生内核没有数据库浏览接口。请重载插件；若仍不可用，请完全重启 AstrBot 后再打开页面。');
    await loadScopes();
  } catch(error) { showError(error); }
  finally { $('refresh').disabled = false; }
}
$('search-form').onsubmit = (event) => { event.preventDefault(); offset=0; loadRecords(); };
$('scope-form').onsubmit = (event) => { event.preventDefault(); scopeOffset=0; loadScopes(); };
for (const id of ['scope','collection']) $(id).onchange = () => { offset=0; loadRecords(); };
$('prev').onclick = () => { offset=Math.max(0,offset-limit); loadRecords(); };
$('next').onclick = () => { offset+=limit; loadRecords(); };
$('scope-prev').onclick = () => { scopeOffset=Math.max(0,scopeOffset-limit); loadScopes(); };
$('scope-next').onclick = () => { scopeOffset+=limit; loadScopes(); };
$('refresh').onclick = refresh;
if (!bridge) {
  $('status').textContent = '请从 AstrBot → 插件 → 忆核 → 插件页面打开，直接打开 HTML 文件无法读取数据库。';
  document.querySelectorAll('button,input,select').forEach(el => { el.disabled = true; });
} else {
  try {
    const syncTheme = (context) => { document.documentElement.dataset.dark = String(context?.isDark ?? false); };
    syncTheme(await bridge.ready());
    bridge.onContext(syncTheme);
    await refresh();
  } catch(error) { showError(error); }
}
