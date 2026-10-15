'use strict';
// Prototype-only data and interactions. This file has no filesystem or CLI execution.
const cmLegacy = {render, homePage, candidateDetail, candidateRow, applyPlan, executionTick, finishOperation, makePlan, replacePlanMode, regeneratePlan, runPage};
const cmFeatures = {
  agentTab:'requests', requestFilter:'all', consoleTab:'json', console:null,
  planTab:'items', planQuery:'', coverageCase:'live', capabilityPlatform:'Windows', filesystem:'NTFS', permission:'normal',
  ruleFilter:'all', ruleQuery:'', pausedRules:new Set(), recoveryFilter:'all', reconciliation:null,
  policy:{rule:'demo.temp-cache v1', mode:'同卷隔离', singleMiB:1024, singleItems:1000, dailyMiB:2048, dailyItems:2000, days:30, usedMiB:512, usedItems:148, version:1, expires:Date.now()+30*86400000},
  requests:[
    {id:'REQ-001',agent:'Codex',title:'隔离过期工具缓存',scope:'用户临时缓存',amount:64,items:3,status:'pending',planId:null,time:'刚刚',reason:'计划已备好，等待人工审阅',candidate:'temp'},
    {id:'REQ-002',agent:'Claude',title:'整理项目构建产物',scope:'D:/DemoProject/BuildCache',amount:900,items:1,status:'blocked',planId:null,time:'5 分钟前',reason:'项目范围不在有限策略内',candidate:'build'},
    {id:'REQ-003',agent:'Codex',title:'核对中断的缓存操作',scope:'用户临时缓存',amount:64,items:1,status:'unknown',planId:null,time:'昨天',reason:'先核对对象，不能重复执行',historyId:'h2'}
  ]
};
const cmRules = [
  {key:'demo.temp-cache v1',title:'过期临时文件',owner:'Demo App',version:'v1',kind:'enabled',symbol:'◷',detail:'固定缓存根 · 14 天前 · 用户数据排除',impact:'工具会重新生成缓存',ids:['temp']},
  {key:'demo.code-cache v1',title:'编辑器可重建缓存',owner:'Visual Studio Code',version:'v1',kind:'enabled',symbol:'⌘',detail:'安装实例 · Code Cache / GPU Cache',impact:'首次启动与渲染可能稍慢',ids:['code','gpu']},
  {key:'demo.chrome-cache v1',title:'网页资源缓存',owner:'Google Chrome',version:'v1',kind:'enabled',symbol:'◉',detail:'独立安装实例 · 网站存储保留',impact:'页面资源需要重新加载',ids:['web']},
  {key:'demo.cursor-cache v1',title:'AI 工具下载缓存',owner:'Cursor',version:'v1',kind:'enabled',symbol:'↗',detail:'下载缓存与会话目录分离',impact:'可生成内容需要重新下载',ids:['ai-cache']},
  {key:'demo.codex-cache v1',title:'已替换工具缓存',owner:'Codex',version:'v1',kind:'enabled',symbol:'>_',detail:'固定旧对象 · 会话与配置保留',impact:'工具按需重建',ids:['ai-old']},
  {key:'demo.build-cache v1',title:'项目构建产物',owner:'Demo Project',version:'v1',kind:'enabled',symbol:'▱',detail:'已识别项目 · 源码与凭据排除',impact:'下次构建需要重新生成',ids:['build']},
  {key:'demo.downloads v0.1',title:'下载目录建议',owner:'待确认来源',version:'v0.1',kind:'experimental',symbol:'↓',detail:'缺少归属与真实平台验证',impact:'不能确定是否为用户独有内容',ids:[]},
  {key:'core.protection',title:'用户成果与凭据保护',owner:'核心保护',version:'v1',kind:'protected',symbol:'◇',detail:'会话、模型、源码、数据库与凭据',impact:'默认保留，优先于清理规则',ids:['credentials','sessions']}
];
state.recovery.push({id:'r3',name:'BuildCache.archive',amount:240,original:'E:/DemoProject/BuildCache.archive',date:'3 天前',conflict:false,offline:true});
Object.assign(pageNames,{agent:'Agent',coverage:'扫描记录',capabilities:'平台能力',rules:'规则与依据'});
mainPages.splice(mainPages.indexOf('maintenance'),0,'agent');
document.querySelector('.rail-bottom').insertAdjacentHTML('afterbegin','<button data-page="coverage" title="扫描记录" aria-label="扫描记录">▱</button><button data-page="rules" title="规则与依据" aria-label="规则与依据">◇</button>');

function cmFact(label,value){return `<div class="detail-fact"><span>${esc(label)}</span><b>${value}</b></div>`;}
function cmStat(label,value,detail=''){return `<div class="feature-stat"><span class="label">${label}</span><b>${value}</b><small>${detail}</small></div>`;}
function cmTab(key,label,current,action){return `<button class="${key===current?'active':''}" onclick="${action}" aria-pressed="${key===current}">${label}</button>`;}
function cmJson(value){return esc(JSON.stringify(value,null,2));}
function cmRequestStatus(r){
  if(r.historyId){const h=state.history.find(h=>h.id===r.historyId);return h?.kind==='unknown'?'unknown':'completed';}
  const p=state.plans.find(p=>p.id===r.planId),op=p&&state.operations[p.id];
  if(op)return op.status==='unknown'?'unknown':op.status==='running'?'running':op.status==='completed'?'completed':op.status==='cancelled'?'cancelled':'partial';
  return r.status;
}
function cmStatusLabel(s){return {pending:'需审阅',blocked:'范围外',unknown:'需核对',running:'处理中',completed:'已完成',cancelled:'已取消',partial:'部分完成'}[s]||s;}
function cmStatusPill(s){return `<span class="pill ${s==='completed'?'good':['blocked','unknown','partial'].includes(s)?'warn':''}">${cmStatusLabel(s)}</span>`;}
function cmAgentPage(){
  const pending=cmFeatures.requests.filter(r=>['pending','blocked'].includes(cmRequestStatus(r))).length;
  const done=cmFeatures.requests.filter(r=>cmRequestStatus(r)==='completed').length;
  const unknown=cmFeatures.requests.filter(r=>cmRequestStatus(r)==='unknown').length;
  const tabs=[['requests','请求'],['policy','有限策略'],['cli','CLI / Skill']];
  const top=heading('Agent','同一计划，从请求到结果','<button onclick="cmConnectionInfo()">连接入口</button><span class="quiet-label">CLI · 演示通道</span>')+
    `<div class="feature-head"><div class="tabs">${tabs.map(([id,label])=>cmTab(id,label,cmFeatures.agentTab,`cmFeatures.agentTab='${id}';render()`)).join('')}</div><span class="muted">Codex / Claude · 浏览器内模拟</span></div>`;
  if(cmFeatures.agentTab==='policy')return top+cmPolicyPage();
  if(cmFeatures.agentTab==='cli')return top+cmCliPage();
  const list=cmFeatures.requests.filter(r=>cmFeatures.requestFilter==='all'||cmRequestStatus(r)===cmFeatures.requestFilter);
  return top+`<div class="feature-stats">${cmStat('待人工处理',pending,'审阅范围与模式')}${cmStat('已完成',done,'逐项结果可查询')}${cmStat('待核对',unknown,'不会自动重试')}</div>`+
    `<div class="feature-layout"><section><div class="workflow-strip">${['请求','固定计划','审阅','执行','报告'].map((s,i)=>`${i?'<span class="arrow">→</span>':''}<span class="step ${i===2?'active':''}"><span>${i+1}</span>${s}</span>`).join('')}</div>`+
    `<div class="controls"><div class="chips">${[['all','全部'],['pending','需审阅'],['blocked','范围外'],['completed','已完成']].map(([id,label])=>cmTab(id,label,cmFeatures.requestFilter,`cmFeatures.requestFilter='${id}';render()`)).join('')}</div></div>`+
    `<div class="panel feature-list"><div class="feature-list-header"><span>请求与来源</span><span>固定范围 · 逐项结果</span></div>${list.length?list.map(r=>{
      const s=cmRequestStatus(r);return `<div class="request-row"><div class="agent-avatar ${r.agent==='Claude'?'claude':''}">${r.agent==='Codex'?'>_':'C'}</div><div class="row-main"><div class="row-name">${r.title}</div><div class="row-meta">${r.agent} · ${r.id} · ${r.time}<br>${r.items} 项 · ${size(r.amount)} · ${esc(r.scope)}</div></div><div class="row-actions">${cmStatusPill(s)}<button onclick="cmOpenRequest('${r.id}')">${s==='unknown'?'核对':s==='completed'?'结果':'查看'} ›</button></div></div>`;
    }).join(''):'<div class="empty-state">此筛选下没有请求</div>'}</div>${cmConsolePanel()}</section>`+
    `<aside class="panel"><h2>处理边界</h2>${cmFact('预授权',state.preauth?'<span class="pill good">已启用 · 演示</span>':'默认关闭')}${cmFact('当前方式','同卷隔离')}${cmFact('计划有效期','15 分钟')}<div class="detail-section"><h3>24 小时限额</h3>${cmQuota()}<button class="help-link" onclick="cmFeatures.agentTab='policy';render()">查看有限策略 →</button></div><div class="split-note"><span class="symbol">◇</span><span>已有授权可直接执行。范围变化或限额不足时，交回人工审阅。</span></div></aside></div>`;
}
function cmOpenRequest(id){
  const r=cmFeatures.requests.find(r=>r.id===id),p=state.plans.find(p=>p.id===r.planId),s=cmRequestStatus(r);
  drawer(r.title,`<span class="badge">${r.agent} · ${r.id}</span>${cmFact('状态',cmStatusPill(s))}${cmFact('固定对象',r.items+' 项')}${cmFact('逻辑量',size(r.amount))}<div class="detail-path">${esc(r.scope)}</div><div class="notice ${['blocked','unknown'].includes(s)?'warn':''}">${r.reason}</div>${p?cmFact('关联计划',p.id):''}<div class="detail-section"><h3>请求轨迹</h3><div class="journal-line"><span class="journal-time">10:24</span><div><b>读取范围与候选</b><div class="row-meta">本地示例数据 · 不执行 shell</div></div></div><div class="journal-line"><span class="journal-time">10:25</span><div><b>${s==='unknown'?'发现未确认的结果':p?'计划已固定':'等待交接计划'}</b><div class="row-meta">${p?p.id:'完整对象可在审阅页展开'}</div></div></div></div>`,
    s==='unknown'?`<button class="primary" onclick="closeDrawer();reconcile('${r.historyId||'H-'+r.planId}')">只读核对 →</button>`:
    r.historyId?`<button onclick="historyDetail('${r.historyId}')">查看核对记录 →</button>`:
    s==='completed'||s==='partial'||s==='cancelled'?`<button class="primary" onclick="closeDrawer();viewOperation('${r.planId}')">查看逐项报告 →</button><button onclick="cmSimulateAgentApply('${id}')">模拟重复提交</button>`:
    `<button class="primary" onclick="cmReviewRequest('${id}')">${p?'审阅同一计划':'打开固定计划'} →</button><button onclick="cmSimulateAgentApply('${id}')">模拟 CLI 调用</button>`);
}
function cmReviewRequest(id,stay=false){
  const r=cmFeatures.requests.find(r=>r.id===id);
  if(r.planId){state.currentPlan=r.planId;closeDrawer();if(!stay)route('plans');render();return;}
  let items=[getCandidate(r.candidate)];
  if(r.candidate==='temp')items=[16,24,24].map((amount,i)=>({...getCandidate('temp'),id:'demo-temp-'+(i+1),name:`preview-old-0${i+1}.tmp`,amount,root:state.policyRoot+`/preview-old-0${i+1}.tmp`}));
  makePlan('cache',items,{mode:'同卷隔离',origin:r.agent,requestId:r.id,stay});
  const p=activePlan();p.origin=r.agent;p.requestId=r.id;r.planId=p.id;cmRegisterPlan(p);cmFeatures.planTab='items';render();
}
function cmSimulateAgentApply(id){
  const r=cmFeatures.requests.find(r=>r.id===id);
  if(!r.planId)cmReviewRequest(id,true);
  const p=state.plans.find(p=>p.id===r.planId);
  if(p.status!=='等待审阅'&&state.operations[p.id]){const status=state.operations[p.id].status;cmLog(status==='unknown'?8:status==='partial'?5:status==='cancelled'?130:0,{kind:'operation_status',operation_id:'OP-'+p.id,status:status==='unknown'?'reconciliation_required':status,deduplicated:true});closeDrawer();route('agent');return;}
  const decision=cmPolicyDecision(p);
  if(!p.cmAuthorization&&!decision.allowed){cmLog(3,{kind:'error',code:'needs_approval',plan_id:p.id,request_id:id,reason:decision.reason,required_action:'Review the same plan in CleanManager'});closeDrawer();route('agent');return;}
  closeDrawer();applyPlan(p.id);
}
function cmLog(exit,data,command=null){cmFeatures.console={exit,data:{schema_version:'prototype/1',synthetic:true,...data},command:command||`cleanmanager plan apply ${data.plan_id||activePlan()?.id||'P-DEMO'} --non-interactive --json`};}
function cmConsolePanel(){
  const c=cmFeatures.console;
  return `<div class="panel console-panel"><div class="console-head"><span>最近 CLI 响应${c?' · 退出 '+c.exit:''}</span><div class="tabs">${[['json','JSON'],['terminal','终端']].map(([id,label])=>cmTab(id,label,cmFeatures.consoleTab,`cmFeatures.consoleTab='${id}';render()`)).join('')}</div></div>${c?`<pre class="console-code">${cmFeatures.consoleTab==='json'?cmJson(c.data):esc('$ '+c.command+'\n\n'+JSON.stringify(c.data,null,2)+'\nexit '+c.exit)}</pre>`:'<div class="console-empty">打开请求，体验缺授权时的结构化响应。</div>'}</div>`;
}
function cmQuota(){
  const p=cmFeatures.policy,percent=Math.min(100,p.usedMiB/p.dailyMiB*100);
  return `<div class="quota"><div class="quota-top"><b>${size(p.usedMiB)} / ${size(p.dailyMiB)}</b><span>${Math.round(percent)}%</span></div><div class="bar"><span style="width:${percent}%"></span></div><small>${p.usedItems} / ${p.dailyItems} 项 · 含示例历史用量</small></div>`;
}
function cmPolicyDecision(p){
  const policy=cmFeatures.policy;
  if(!state.preauth)return {allowed:false,reason:'有限预授权尚未启用'};
  if(Date.now()>=policy.expires)return {allowed:false,reason:'策略已到期'};
  if(!p||p.kind!=='cache'||p.mode!==policy.mode)return {allowed:false,reason:'操作模式不在策略内'};
  if(p.items.some(i=>!i.root||!(i.root===state.policyRoot||i.root.startsWith(state.policyRoot+'/'))))return {allowed:false,reason:'对象位于策略根之外'};
  if(p.items.some(i=>i.rule!==policy.rule||!i.eligible||candidateProtected(i)))return {allowed:false,reason:'规则版本或保护条件不匹配'};
  if(p.items.some(i=>i.amount===null||i.amount===undefined))return {allowed:false,reason:'大小未知，无法核算限额'};
  const total=p.items.reduce((n,i)=>n+i.amount,0);
  if(total>policy.singleMiB||p.items.length>policy.singleItems)return {allowed:false,reason:'超过单次限额'};
  if(policy.usedMiB+total>policy.dailyMiB||policy.usedItems+p.items.length>policy.dailyItems)return {allowed:false,reason:'24 小时累计余额不足'};
  return {allowed:true,reason:'精确根、规则版本、方式和限额全部匹配'};
}
function cmPolicyPanel(){
  const p=cmFeatures.policy;
  return `<div class="feature-head"><h2>Demo App 缓存策略</h2><span class="pill ${state.preauth?'good':''}">${state.preauth?'已启用':'默认关闭'}</span></div><div class="row"><span>在限额内自动授权</span><label class="toggle"><input type="checkbox" aria-label="启用有限策略" ${state.preauth?'checked':''} onchange="cmSetPolicyEnabled(this.checked)"><span class="switch"></span></label></div><div class="policy-form-grid"><div class="full"><label for="cmPolicyRoot">精确范围</label><input id="cmPolicyRoot" type="text" value="${esc(state.policyRoot)}"></div><div><label for="cmPolicyRule">规则版本</label><select id="cmPolicyRule"><option>demo.temp-cache v1</option></select></div><div><label for="cmPolicyMode">处理方式</label><select id="cmPolicyMode"><option>同卷隔离</option></select></div><div><label for="cmPolicySingle">单次 MiB</label><input id="cmPolicySingle" type="number" min="1" max="2048" value="${p.singleMiB}"></div><div><label for="cmPolicyDaily">24 小时 MiB</label><input id="cmPolicyDaily" type="number" min="1" max="8192" value="${p.dailyMiB}"></div></div><div class="policy-summary"><div class="row"><span>单次对象上限</span><b>${p.singleItems} 项</b></div><div class="row"><span>24 小时对象上限</span><b>${p.dailyItems} 项</b></div><div class="row"><span>有效期</span><b>30 天 · 演示模板</b></div><div class="row"><span>当前版本</span><b>policy-demo / v${p.version}</b></div></div><div class="split-note"><span class="symbol">◇</span><span>不覆盖会话、凭据或未知对象。规则、模式或范围变化会重新判断。</span></div><div class="feature-actions end section-gap"><button onclick="cmTestPolicy()">模拟匹配</button><button class="primary" onclick="cmSavePolicy()">保存演示策略</button></div>`;
}
function cmPolicyPage(){return `<div class="feature-layout"><section class="panel">${cmPolicyPanel()}</section><aside class="panel"><h2>累计用量</h2>${cmQuota()}<div class="detail-section">${cmFact('剩余额度',size(Math.max(0,cmFeatures.policy.dailyMiB-cmFeatures.policy.usedMiB)))}${cmFact('未确认动作','保留对应额度')}</div><div class="notice">这里展示浏览器内额度变化，正式额度需核心跨进程核算。</div><button class="help-link" onclick="cmTestQuota()">体验余额不足 →</button></aside></div>`;}
function cmSetPolicyEnabled(on){state.preauth=on;render();toast(on?'已启用演示策略，范围外仍交回人工':'策略已暂停，新的策略动作停止');}
function cmSavePolicy(){
  const root=$('cmPolicyRoot').value.trim().replace(/\/+$/,''),single=Number($('cmPolicySingle').value),daily=Number($('cmPolicyDaily').value);
  if(!root||!Number.isInteger(single)||!Number.isInteger(daily)||single<1||single>2048||daily<1||daily>8192||daily<single)return toast('请填写有效根与限额；累计上限不能小于单次');
  state.policyRoot=root;Object.assign(cmFeatures.policy,{singleMiB:single,dailyMiB:daily,version:cmFeatures.policy.version+1,expires:Date.now()+30*86400000});render();toast('演示策略已保存，旧版本不扩大授权');
}
function cmTestPolicy(){
  const r=cmFeatures.requests[0];if(!r.planId)cmReviewRequest(r.id,true);
  const p=state.plans.find(p=>p.id===r.planId),d=cmPolicyDecision(p);
  drawer('策略匹配结果',`<span class="badge ${d.allowed?'':'concept'}">${d.allowed?'可由策略覆盖':'需人工审阅'}</span>${cmFact('关联计划',p.id)}${cmFact('规则版本','demo.temp-cache v1')}${cmFact('逻辑量',size(64))}<div class="notice ${d.allowed?'':'warn'}">${d.reason}</div>`,'<button onclick="closeDrawer();cmReviewRequest(\'REQ-001\')">查看同一计划 →</button>');
}
function cmTestQuota(){cmFeatures.policy.usedMiB=Math.max(0,cmFeatures.policy.dailyMiB-16);cmFeatures.policy.usedItems=1960;render();toast('已载入演示用量，64 MiB 请求会超过余额');}
function cmCliPage(){
  return `<div class="feature-layout"><section><div class="panel"><h2>一个入口，同一套计划</h2><div class="row"><span>Codex</span><span class="pill good">CLI 工作流示例</span><button onclick="cmSkillInfo('Codex')">查看 Skill ›</button></div><div class="row"><span>Claude</span><span class="pill good">CLI 工作流示例</span><button onclick="cmSkillInfo('Claude')">查看 Skill ›</button></div><div class="row"><span>MCP</span><span class="pill">后续扩展</span><button onclick="cmMcpInfo()">了解范围 ›</button></div></div><div class="panel console-panel"><div class="console-head"><span>常用 CLI</span><button class="help-link" onclick="cmCopyCommands()">复制示例</button></div><pre class="console-code">${esc(cmCommands())}</pre></div>${cmConsolePanel()}</section><aside class="panel"><h2>响应约定</h2>${cmFact('0 / 5','完整 / 部分')}${cmFact('3','需要授权')}${cmFact('6','过期或冲突')}${cmFact('8','失败或待核对')}${cmFact('130','用户取消')}<div class="split-note"><span class="symbol">›</span><span>JSON / JSONL · 字节为精确字符串 · 按需分页，日志不混入结果。</span></div></aside></div>`;
}
function cmCommands(){return `cleanmanager capabilities --json\ncleanmanager scan --root "C:/Users/Lin" --format jsonl\ncleanmanager candidates list --scan S-DEMO --limit 50 --json\ncleanmanager plan show ${activePlan()?.id||'P-DEMO-001'} --json\ncleanmanager plan apply ${activePlan()?.id||'P-DEMO-001'} --non-interactive --json`;}
async function cmCopyText(value){try{await navigator.clipboard.writeText(value);toast('已复制演示文本')}catch{modal('复制示例',`<textarea class="form-input mono" rows="7" readonly>${esc(value)}</textarea>`);}}
function cmCopyCommands(){cmCopyText(cmCommands());}
function cmSkillInfo(agentName){drawer(agentName+' · Skill 工作流','<div class="workflow-strip">发现 → 摘要 → 计划 → 结果</div><div class="check-item"><span class="check-mark">1</span><div><b>先查询能力与范围</b><div class="row-meta">普通权限读取；缺口保持未知</div></div></div><div class="check-item"><span class="check-mark">2</span><div><b>按需分页、解释影响</b><div class="row-meta">不把整棵目录树交给模型</div></div></div><div class="check-item"><span class="check-mark">3</span><div><b>生成固定计划</b><div class="row-meta">缺授权交回人工，已有策略可直接使用</div></div></div><div class="check-item"><span class="check-mark">4</span><div><b>查明逐项结果</b><div class="row-meta">partial 不报全成功；unknown 不盲目重试</div></div></div>','<button onclick="cmCopyCommands()">复制 CLI 示例</button>');}
function cmConnectionInfo(){modal('连接 Agent','<div class="row"><span>入口</span><b>独立 CLI</b></div><div class="row"><span>客户端</span><b>Codex / Claude</b></div><div class="row"><span>此原型</span><span class="pill">浏览器内示例</span></div><div class="notice">真实 CLI、Skill 安装和权限连接尚未实现。</div>','<button class="primary" onclick="closeDialog();cmFeatures.agentTab=\'cli\';route(\'agent\');render()">查看调用方式</button>');}
function cmMcpInfo(){modal('MCP · 后续扩展','<p class="muted">计划是在稳定 CLI 之上增加薄工具入口，继续复用同一规则、授权和执行核心。</p><div class="notice">当前没有启动 MCP 服务，也没有新授予 Agent 文件权限。</div>');}

function cmCoveragePage(){
  const interrupted=cmFeatures.coverageCase==='cancelled'||state.scan==='cancelled',changed=cmFeatures.coverageCase==='volume',running=['running','stopping'].includes(state.scan)&&cmFeatures.coverageCase==='live';
  const status=changed?'卷身份变化':interrupted?'已取消':running?state.scan==='stopping'?'正在停止':'正在发现':'部分覆盖';
  const scopes=[['Projects',24*1024,'已确认'],['Models',40*1024,'已确认'],['AppData',11*1024,'已确认'],['Downloads',12*1024,'已确认'],['Documents',5*1024,'已确认'],['Pictures',2*1024,'已确认'],['System Volume Information',null,'权限不足'],['OneDrive 占位目录',null,'云边界未验证']];
  return heading('扫描记录','S-DEMO-1042 · 演示快照 r7','<button onclick="route(\'capabilities\')">平台能力</button><button class="primary" onclick="cmFeatures.coverageCase=\'live\';startScan()">开始新扫描</button>')+
    `<div class="feature-stats">${cmStat('已知逻辑量','94.0 <span class="stat-unit">GiB</span>','已确认范围的小计')}${cmStat('未读取范围','2','里面的大小与条目数未知')}${cmStat('完整性',status,'不代表全盘原子快照')}</div>`+
    `<div class="controls"><div class="chips">${[['live','部分覆盖'],['cancelled','取消结果'],['volume','卷变化']].map(([id,label])=>cmTab(id,label,cmFeatures.coverageCase,`cmFeatures.coverageCase='${id}';render()`)).join('')}</div><span class="muted">查看演示情景</span></div>`+
    `<div class="feature-layout"><section><div class="panel feature-list"><div class="feature-list-header"><span>范围 · ${esc(state.scanRoot)}</span><span>已确认逻辑量</span></div>${scopes.map(([name,amount,s])=>`<div class="scope-row">${icon(amount===null?'◇':'▱')}<div class="row-main"><div class="row-name">${name}</div><div class="row-meta">${amount===null?name==='OneDrive 占位目录'?'避免触发占位内容下载':'目录读取被拒绝':'来自同一演示 revision · 元数据观察'}</div></div><div class="row-size">${size(amount)}<br><span class="pill ${amount===null?'warn':'good'}">${s}</span></div><button onclick="cmScopeDetail('${name}')">${amount===null?'查看原因':'查看来源'} ›</button></div>`).join('')}</div>`+
    `<div class="notice ${changed||interrupted?'warn':''}">${changed?'同路径对应了新卷，旧范围不自动继承。':interrupted?'停止后已确认结果保留，新增扫描会生成新的记录。':running?'任务仍由发起进程承载，导航不会取消它。':'未读取范围保持未知，不填成 0，也不计入图表面积。'}</div></section>`+
    `<aside class="panel"><div class="coverage-ring" style="background:conic-gradient(var(--accent) 0 270deg,var(--line) 270deg 360deg)"><div class="ring-copy"><b>6 / 8</b><small>演示范围有结果</small></div></div><h2>快照信息</h2>${cmFact('范围','用户目录')}${cmFact('时间','今天 10:24 · 演示')}${cmFact('后端','限定目录扫描')}${cmFact('卷身份','DEMO-VOLUME-A')}${cmFact('查询版本','r7')}<div class="split-note"><span class="symbol">▱</span><span>范围数量不是字节覆盖百分比。共享块和未读目录仍有独立限制。</span></div><button class="help-link section-gap" onclick="cmScopeDetail('查询版本')">理解版本与游标 →</button></aside></div>`;
}
function cmScopeDetail(name){
  const cloud=name==='OneDrive 占位目录',denied=name==='System Volume Information',revision=name==='查询版本';
  drawer(name,revision?`<span class="badge">同一快照</span>${cmFact('Scan','S-DEMO-1042')}${cmFact('Revision','r7')}<div class="notice">分页、排序和过滤绑定版本。刷新后旧游标不混入新结果。</div>`:
    `${cmFact('读取状态',cloud?'跳过云边界':denied?'权限不足':'演示元数据已确认')}${cmFact('大小',cloud||denied?'未知':'逻辑与分配分别记录')}${cmFact('内容读取','不读取内容')}<div class="detail-path">${esc(state.scanRoot+'/'+name)}</div><div class="notice ${cloud||denied?'warn':''}">${cloud?'当前 provider 未验证安全访问，默认跳过占位范围。':denied?'可以继续查看其他可读结果，读取权限与删除批准不同。':'这里只展示演示字段来源；实际平台访问仍需原型验证。'}</div>`,
    `<button onclick="closeDrawer();route('${revision?'disk':'capabilities'}')">${revision?'返回磁盘查询':'查看平台能力'} →</button>`);
}
function cmCapabilityRecords(){
  const mac=cmFeatures.capabilityPlatform==='macOS',remote=cmFeatures.filesystem==='网络目录',elevated=cmFeatures.permission==='elevated',denied=cmFeatures.permission==='denied';
  return [
    {id:'directory',title:'限定目录读取',status:denied?'需读取权限':'演示可用',detail:'范围内枚举，拒绝区域单独记录',next:denied?'查看系统读取权限；仍可保留其他结果。':'按选择的根扫描，默认不沿链接扩大范围。'},
    {id:'fast',title:mac?'批量属性扫描':'快速卷扫描',status:remote?'不可用':mac?'未验证':cmFeatures.filesystem!=='NTFS'?'不适用':elevated?'未验证':'需要额外权限',detail:remote?'网络目录使用有界遍历':mac?'原生目录属性能力需真 Mac 校准':'卷级范围与目录范围分别选择',next:mac?'在真实 Mac 上验证字段、权限与云边界。':'只有明确整卷范围与所需访问条件时才尝试快速后端。'},
    {id:'cloud',title:'云占位边界',status:'未验证范围跳过',detail:'不为取得大小自动下载内容',next:'不同 provider 需要分别验证，无法保证时保留未知。'},
    {id:'allocation',title:'空间与共享块',status:mac?'预计释放可未知':'分开记录',detail:remote?'服务器占用和共享量需单独验证':mac?cmFeatures.filesystem==='APFS'?'APFS clone / 快照 / 共享容量':'逻辑量 / 分配量 / 扩展属性':cmFeatures.filesystem==='NTFS'?'硬链接 / 稀疏 / 压缩量':'字段按当前文件系统取得',next:'逻辑量、分配量、可释放估计与卷空闲观测分别呈现。'},
    {id:'quarantine',title:'同卷隔离',status:remote?'当前不可用':'按目标验证',detail:'位置改变，不立即释放空间',next:'身份约束、恢复清单和源目录条件通过后才开放。'},
    {id:'helper',title:'特权写入',status:'默认关闭',detail:'不以管理员权限代替用户批准',next:'只有明确需要的受限能力才使用 helper；当前原型不安装服务。'}
  ];
}
function cmCapabilityPage(){
  const mac=cmFeatures.capabilityPlatform==='macOS';
  return heading('平台能力','同一用户任务，按平台与目标说明差异','<button onclick="route(\'coverage\')">返回扫描记录</button>')+
    `<div class="controls"><div class="platform-select">${['Windows','macOS'].map(p=>cmTab(p,p,cmFeatures.capabilityPlatform,`cmSetCapabilityPlatform('${p}')`)).join('')}</div><select aria-label="演示文件系统" onchange="cmFeatures.filesystem=this.value;render()">${(mac?['APFS','HFS+','网络目录']:['NTFS','exFAT','网络目录']).map(s=>`<option ${s===cmFeatures.filesystem?'selected':''}>${s}</option>`).join('')}</select><select aria-label="演示读取权限" onchange="cmFeatures.permission=this.value;render()"><option value="normal" ${cmFeatures.permission==='normal'?'selected':''}>普通目录权限</option><option value="elevated" ${cmFeatures.permission==='elevated'?'selected':''}>${mac?'已授予完全磁盘访问':'已具备额外读取权限'}</option><option value="denied" ${cmFeatures.permission==='denied'?'selected':''}>目标读取被拒绝</option></select></div>`+
    `<div class="notice"><span>${cmFeatures.capabilityPlatform} / ${cmFeatures.filesystem} · 能力状态为演示，不代表真机已验收</span></div><div class="capability-grid">${cmCapabilityRecords().map(c=>`<button class="capability-card" onclick="cmCapabilityDetail('${c.id}')"><div class="feature-head"><h3>${c.title}</h3><span class="pill ${c.status.includes('权限')||c.status.includes('未知')?'warn':c.status==='演示可用'?'good':''}">${c.status}</span></div><p>${c.detail}</p><div class="bottom"><span>原因与下一步</span><span>›</span></div></button>`).join('')}</div><div class="split-note"><span class="symbol">◇</span><span>读取范围、系统权限和产品写入授权是不同条件。切换这里只查看场景，不修改系统设置。</span></div>`;
}
function cmSetCapabilityPlatform(p){cmFeatures.capabilityPlatform=p;cmFeatures.filesystem=p==='macOS'?'APFS':'NTFS';render();}
function cmCapabilityDetail(id){const c=cmCapabilityRecords().find(c=>c.id===id);drawer(c.title,`<span class="badge">${cmFeatures.capabilityPlatform} / ${cmFeatures.filesystem}</span>${cmFact('当前场景',c.status)}<div class="detail-section"><h3>为什么</h3><p class="muted">${c.detail}</p></div><div class="detail-section"><h3>下一步</h3><p class="muted">${c.next}</p></div>`,'<button onclick="permissionInfo()">读取权限说明</button><button onclick="closeDrawer();route(\'plans\')">查看产品授权 →</button>');}
function cmRulesPage(){
  const list=cmRules.filter(r=>(cmFeatures.ruleFilter==='all'||r.kind===cmFeatures.ruleFilter)&&(r.title+' '+r.owner+' '+r.key).toLowerCase().includes(cmFeatures.ruleQuery.toLowerCase()));
  return heading('规则与依据','哪些内容可处理，为什么','<span class="quiet-label">规则资料 · 全部为演示</span>')+
    `<div class="controls"><label class="search">⌕ <input aria-label="搜索规则" placeholder="应用、规则或来源" value="${esc(cmFeatures.ruleQuery)}" oninput="cmSearchRules(this.value)"></label><div class="chips">${[['all','全部'],['enabled','演示规则'],['experimental','实验'],['protected','保护']].map(([id,label])=>cmTab(id,label,cmFeatures.ruleFilter,`cmFeatures.ruleFilter='${id}';render()`)).join('')}</div></div>`+
    `<div class="feature-layout"><section class="panel feature-list"><div class="feature-list-header"><span>${list.length} 条规则</span><span>版本与影响</span></div>${list.map(r=>`<div class="rule-row">${icon(r.symbol)}<div class="row-main"><div class="row-name">${r.title}<span class="rule-version">${r.version}</span></div><div class="row-meta">${r.owner} · ${r.detail}</div><div class="rule-brief">${r.impact}</div></div><div class="inline-actions"><span class="pill ${r.kind==='experimental'?'warn':r.kind==='protected'?'good':''}">${r.kind==='experimental'?'未启用':r.kind==='protected'?'默认保护':cmFeatures.pausedRules.has(r.key)?'已暂停':'演示启用'}</span><button onclick="cmRuleDetail('${r.key}')">依据 ›</button></div></div>`).join('')||'<div class="empty-state">没有匹配规则</div>'}</section>`+
    `<aside class="panel"><h2>规则的边界</h2>${cmFact('识别','来源 + 精确根')}${cmFact('用户成果','优先保留')}${cmFact('未知后代','不被父操作覆盖')}${cmFact('修改版本','重新判断计划')}<div class="split-note"><span class="symbol">◇</span><span>这些记录用于体验规则交互。正式 enabled 状态需要真实平台与应用版本验证。</span></div><button class="help-link section-gap" onclick="state.settingsTab='protection';route('settings')">查看保护路径 →</button></aside></div>`;
}
function cmSearchRules(value){cmFeatures.ruleQuery=value;const caret=document.activeElement.selectionStart;render();const input=document.querySelector('[aria-label="搜索规则"]');input.focus();input.setSelectionRange(caret,caret);}
function cmRuleDetail(key){
  const r=cmRules.find(r=>r.key===key);if(!r)return;
  const items=r.ids.map(getCandidate).filter(Boolean);
  drawer(r.title,`<span class="badge ${r.kind==='experimental'?'concept':''}">${r.kind==='experimental'?'实验资料':r.kind==='protected'?'核心保护':'演示规则'}</span>${cmFact('规则版本',esc(r.key))}${cmFact('归属',esc(r.owner))}${cmFact('影响',esc(r.impact))}<div class="detail-section"><h3>范围与排除</h3>${items.map(i=>`<div class="detail-path">${esc(i.root)}</div>`).join('')||'<p class="muted">来源和适用根尚未确定。</p>'}<div class="notice">会话、凭据、用户数据与共享容器不被缓存规则包含。</div></div><div class="detail-section"><h3>验证资料</h3>${cmFact('匹配正例','浏览器演示样例')}${cmFact('排除反例','用户内容保持保护')}${cmFact('真实平台记录','待实际原型验证')}</div>`,
    r.kind==='enabled'?`<button onclick="cmToggleRule('${key}')">${cmFeatures.pausedRules.has(key)?'恢复演示规则':'暂停演示规则'}</button>`:r.kind==='experimental'?'<button onclick="toast(\'缺少真实平台证据，实验规则保持未启用\')">为什么不能启用？</button>':'<button onclick="closeDrawer();state.settingsTab=\'protection\';route(\'settings\')">查看保护设置</button>');
}
function cmToggleRule(key){cmFeatures.pausedRules.has(key)?cmFeatures.pausedRules.delete(key):cmFeatures.pausedRules.add(key);candidates.filter(c=>c.rule===key).forEach(c=>state.selected.delete(c.id));closeDrawer();render();toast(cmFeatures.pausedRules.has(key)?'演示规则已暂停，候选与未执行计划会重新核验':'演示规则已恢复');}

function cmCanonical(value){if(Array.isArray(value))return '['+value.map(cmCanonical).join(',')+']';if(value&&typeof value==='object')return '{'+Object.keys(value).sort().map(k=>JSON.stringify(k)+':'+cmCanonical(value[k])).join(',')+'}';return JSON.stringify(value);}
function cmRegisterPlan(p){
  if(!p||p.cmBody)return;
  p.cmBody=Object.freeze({engine_contract:'prototype-plan/v1',synthetic:true,plan_id:p.id,created_at:new Date(p.created).toISOString(),expires_at:new Date(p.expires).toISOString(),kind:p.kind,mode:p.mode,scope:['restore','purge'].includes(p.kind)?'quarantine-manifest-demo':state.scanRoot,revision:['restore','purge'].includes(p.kind)?'manifest-demo-v1':'r7-demo',destination:p.destination,items:p.items.map(i=>({entry_id:i.id,name:i.name,path:i.root||i.original||null,logical_bytes:i.amount===null||i.amount===undefined?null:(BigInt(i.amount)*1048576n).toString(),rule:i.rule||'recovery-manifest'})).sort((a,b)=>a.entry_id.localeCompare(b.entry_id))});
  p.cmDigest='正在计算';
  if(!crypto.subtle){p.cmDigest='本浏览器不支持摘要计算';return;}
  crypto.subtle.digest('SHA-256',new TextEncoder().encode(cmCanonical(p.cmBody))).then(bytes=>{
    p.cmDigest=Array.from(new Uint8Array(bytes),n=>n.toString(16).padStart(2,'0')).join('');
    document.querySelectorAll('[data-plan-digest]').forEach(el=>{if(el.dataset.planDigest===p.id)el.textContent=p.cmDigest.slice(0,12)+'…'+p.cmDigest.slice(-8);});
  });
}
function cmPlanTotal(p){return p.items.some(i=>i.amount===null||i.amount===undefined)?null:p.items.reduce((n,i)=>n+i.amount,0);}
function cmShowPlan(id){state.currentPlan=id;cmFeatures.planQuery='';cmFeatures.planTab='items';closeDialog();closeDrawer();route('plans');render();}
function cmPlanChecks(p){
  const expired=Date.now()>=p.expires||state.scenario==='expired',protectedItem=p.items.some(i=>candidateProtected(i)),policy=cmPolicyDecision(p);
  return [
    {title:['restore','purge'].includes(p.kind)?'固定对象与隔离清单':'固定对象与扫描版本',detail:`${p.items.length} 项 · ${['restore','purge'].includes(p.kind)?'manifest-demo-v1':'r7-demo'} · 新出现的文件不会加入`,good:true},
    {title:'对象与父范围',detail:state.scenario==='changed'?'首项会在演示执行中因身份变化跳过':'执行前仍会核验，不把旧快照当当前状态',good:state.scenario!=='changed'},
    {title:'保护与规则',detail:protectedItem?'存在保护或暂停规则，相关条目会跳过':'用户成果与被保护后代不进入处理',good:!protectedItem},
    {title:'计划有效期',detail:expired?'旧计划已过期，需要重新生成':'创建后 15 分钟，到期不启动新动作',good:!expired},
    {title:'授权来源',detail:p.cmAuthorization?'已登记演示人工/策略授权':policy.allowed?'完整命中有限策略':'等待人工审阅；系统读取权限不代替批准',good:!!p.cmAuthorization||policy.allowed}
  ];
}
function cmPlansPage(){
  const p=activePlan();
  if(!p)return heading('操作计划','固定范围，先审阅再处理','<button onclick="route(\'agent\')">Agent 请求</button>')+empty('还没有计划','选择缓存，或接手一个 Agent 请求。','<button class="primary" onclick="route(\'agent\')">体验请求交接 →</button>');
  cmRegisterPlan(p);
  const expired=Date.now()>=p.expires||state.scenario==='expired',policy=cmPolicyDecision(p),authorized=!!p.cmAuthorization||policy.allowed,op=state.operations[p.id],total=cmPlanTotal(p),waiting=p.status==='等待审阅'&&!p.cmReplacedBy;
  const minutes=Math.max(0,Math.ceil((p.expires-Date.now())/60000));
  const q=cmFeatures.planQuery.toLowerCase(),filtered=p.items.filter(i=>(i.name+' '+(i.root||i.original||'')).toLowerCase().includes(q));
  const tabContent=cmFeatures.planTab==='items'?`<div class="controls"><label class="search">⌕ <input aria-label="搜索计划对象" placeholder="名称或原始位置" value="${esc(cmFeatures.planQuery)}" oninput="cmSearchPlan(this.value)"></label><span class="muted">${filtered.length} / ${p.items.length} 项 · 固定对象</span></div><div class="plan-table-wrap"><table class="plan-table"><thead><tr><th>对象与位置</th><th class="num">逻辑量</th><th>依据</th><th>状态</th></tr></thead><tbody>${filtered.map(i=>`<tr><td><button onclick="cmPlanItem('${p.id}','${i.id}')">${esc(i.name)}</button><div class="path">${esc(i.root||i.original||'固定演示对象')}</div></td><td class="num">${size(i.amount)}</td><td><span class="pill">${i.rule?'版本化规则':'隔离清单'}</span></td><td><span class="pill ${candidateProtected(i)?'warn':'good'}">${candidateProtected(i)?'已受保护':'待现场核验'}</span></td></tr>`).join('')||'<tr><td colspan="4">没有匹配对象；原计划清单未改变。</td></tr>'}</tbody></table></div>`:
    cmFeatures.planTab==='checks'?cmPlanChecks(p).map(c=>`<div class="check-item"><span class="check-mark ${c.good?'':'warn'}">${c.good?'✓':'!'}</span><div><b>${c.title}</b><div class="row-meta">${c.detail}</div></div></div>`).join(''):
    `<div class="row"><span>请求来源</span><b>${esc(p.origin||'人工选择')}${p.requestId?' · '+p.requestId:''}</b></div><div class="row"><span>Plan ID</span><b class="mono">${p.id}</b></div><div class="row"><span>规则快照</span><span>${[...new Set(p.items.map(i=>i.rule||'恢复清单'))].map(esc).join('<br>')}</span></div>${state.plans.filter(x=>x.requestId&&x.requestId===p.requestId||x.id===p.id||x.cmReplacedBy===p.id||p.cmReplacedBy===x.id).map((x,i)=>`<div class="version-row"><span class="version-number">${i+1}</span><div class="row-main"><b class="mono">${x.id}</b><div class="row-meta">${x.mode} · ${x.status}</div></div>${x.id===p.id?'<span class="pill good">当前查看</span>':`<button onclick="cmShowPlan('${x.id}')">查看 ›</button>`}</div>`).join('')}<div class="split-note"><span class="symbol">◇</span><span>改变对象、模式或版本会生成新的 body 与摘要，旧记录仍可查询。</span></div>`;
  let footer='';
  if(p.cmReplacedBy)footer=`<div class="notice">这个计划已被新版本替代，旧清单保留供审阅。</div><button class="primary" onclick="cmShowPlan('${p.cmReplacedBy}')">查看新计划 →</button>`;
  else if(op)footer=`<div class="feature-actions end"><span>${cmStatusPill(op.status==='unknown'?'unknown':op.status)}</span><button class="primary" onclick="viewOperation('${p.id}')">查看执行与结果 →</button></div>`;
  else if(expired)footer='<div class="notice warn">计划已过期，不能延长旧计划或直接重试。</div><button class="primary" onclick="regeneratePlan()">重新生成计划 →</button>';
  else if(waiting)footer=`<label class="review-consent"><input id="consent" type="checkbox" ${authorized?'checked disabled':cmFeatures.consent?.has(p.id)?'checked':''} onchange="cmSetConsent('${p.id}',this.checked)">${authorized?'已有演示授权；执行仍进行现场复核':'我已审阅固定对象、方式与恢复限制'}</label><div class="feature-actions"><button onclick="cmHandoffDialog(activePlan())">Agent 交接</button><button id="applyPlan" class="primary" ${!authorized&&!cmFeatures.consent?.has(p.id)?'disabled':''} onclick="applyPlan('${p.id}')">${authorized?'按已有授权执行':'授权并执行'} · 演示</button></div>`;
  return heading('审阅计划',`${p.id} · ${esc(p.origin||'人工选择')} · ${p.concept?'扩展概念':'固定对象'}`,'<button onclick="showPlans()">全部计划</button><button onclick="cmHandoffDialog(activePlan())">交接入口</button>')+
    `<div class="feature-stats four">${cmStat('固定对象',p.items.length,'完整清单可查看')}${cmStat('逻辑量',size(total),'处理量，不等于收益')}${cmStat('预计释放',p.mode==='同卷隔离'||p.mode==='回收站'?'不计移动量':'未知',p.mode==='同卷隔离'?'隔离后内容仍在该卷':'依据与共享块分别判断')}${cmStat('有效期',expired?'已到期':minutes+' 分钟','旧计划不会自动续期')}</div>`+
    `<div class="feature-layout wide-aside"><section class="panel"><div class="feature-head"><div class="tabs">${[['items','对象明细'],['checks','核验与授权'],['versions','版本与来源']].map(([id,label])=>cmTab(id,label,cmFeatures.planTab,`cmFeatures.planTab='${id}';render()`)).join('')}</div><span class="pill ${expired?'warn':''}">${expired&&!op?'已过期':p.status}</span></div>${tabContent}<div class="plan-footer">${footer}</div></section>`+
    `<aside class="panel"><h2>处理方式</h2>${p.kind==='cache'?`<div class="mode-controls">${['同卷隔离','回收站','永久删除'].map(m=>`<button class="${m===p.mode?'active':''}" ${!waiting||expired?'disabled':''} onclick="replacePlanMode('${m}')">${m}</button>`).join('')}</div>`:cmFact('方式',p.mode)}<div class="notice ${p.mode==='永久删除'||p.kind==='purge'?'warn':''}">${modeExplanation(p)}</div>${p.kind==='restore'?cmFact('恢复目标',`<span class="digest-value">${esc(p.destination||'逐项原位置')}</span>`):''}${cmFact('授权',p.cmAuthorization?p.cmAuthorization.kind==='policy'?'有限策略':'人工审阅':policy.allowed?'有限策略可覆盖':'等待人工审阅')}${cmFact('范围',['restore','purge'].includes(p.kind)?'固定对象 · 隔离清单':'固定对象 · 演示快照')}${cmFact('摘要',`<span class="digest-value" data-plan-digest="${p.id}">${p.cmDigest.length===64?p.cmDigest.slice(0,12)+'…'+p.cmDigest.slice(-8):p.cmDigest}</span>`)}<button class="help-link" onclick="planInfo('${p.id}')">查看演示 body / 摘要 →</button>${!policy.allowed&&p.kind==='cache'?`<div class="split-note"><span class="symbol">◇</span><span>${policy.reason}</span></div>`:''}</aside></div>`;
}
function cmSearchPlan(value){cmFeatures.planQuery=value;const caret=document.activeElement.selectionStart;render();const input=document.querySelector('[aria-label="搜索计划对象"]');input.focus();input.setSelectionRange(caret,caret);}
function cmSetConsent(id,on){cmFeatures.consent??=new Set();on?cmFeatures.consent.add(id):cmFeatures.consent.delete(id);$('applyPlan').disabled=!on;}
function cmPlanItem(planId,id){const p=state.plans.find(p=>p.id===planId),item=p.items.find(i=>i.id===id);drawer(item.name,`<span class="badge">固定清单对象</span><div class="detail-path">${esc(item.root||item.original||'演示维护对象')}</div>${cmFact('Plan ID',p.id)}${cmFact('Entry ID',esc(item.id))}${cmFact('逻辑量',size(item.amount))}${cmFact('规则',esc(item.rule||'隔离清单'))}${cmFact('当前要求','执行前核验源与父范围')}<div class="notice">这个条目不会在执行时被同名新对象替换。</div>`,item.rule?`<button onclick="cmRuleDetail('${item.rule}')">查看规则依据 →</button>`:'<button onclick="closeDrawer()">返回计划</button>');}
function cmHandoffDialog(p){if(!p)return route('agent');modal('交接同一计划',`<span class="badge">${p.id}</span>${cmFact('来源',esc(p.origin||'人工'))}${cmFact('方式',p.mode)}${cmFact('固定对象',p.items.length+' 项')}<pre class="console-code">${esc(`cleanmanager plan show ${p.id} --json\ncleanmanager plan apply ${p.id} --non-interactive --json`)}</pre><div class="notice">机器调用只使用既有授权；缺授权返回 needs_approval，不重建近似清单。</div>`,`<button onclick="cmCopyText('cleanmanager plan show ${p.id} --json')">复制查询命令</button><button class="primary" onclick="closeDialog();route('agent')">查看 Agent 请求 →</button>`);}

function cmRecoveryPage(){
  const list=state.recovery.filter(r=>cmFeatures.recoveryFilter==='all'||cmFeatures.recoveryFilter==='ready'&&!r.offline&&!r.conflict||cmFeatures.recoveryFilter==='attention'&&(r.offline||r.conflict));
  const available=state.recovery.filter(r=>!r.offline&&!r.conflict).length,attention=state.recovery.length-available;
  return heading('隔离恢复','原位置、对象身份与恢复条件','<span class="quiet-label">30 天后提醒 · 不自动清空</span>')+
    `<div class="feature-stats">${cmStat('隔离内容',state.recovery.length,'仍占用原卷数据空间')}${cmStat('可审阅恢复',available,'执行前重新核验')}${cmStat('需要处理',attention,'冲突或原卷离线')}</div>`+
    `<div class="controls"><div class="chips">${[['all','全部'],['ready','可恢复'],['attention','需处理']].map(([id,label])=>cmTab(id,label,cmFeatures.recoveryFilter,`cmFeatures.recoveryFilter='${id}';render()`)).join('')}</div></div>`+
    `<div class="feature-layout"><section class="panel feature-list"><div class="feature-list-header"><span>隔离对象与原位置</span><span>恢复条件</span></div>${list.map(r=>`<div class="request-row"><input type="checkbox" aria-label="选择恢复 ${esc(r.name)}" ${state.recoverySelection.has(r.id)?'checked':''} ${r.offline?'disabled':''} onchange="this.checked?state.recoverySelection.add('${r.id}'):state.recoverySelection.delete('${r.id}');render()"><div class="row-main"><div class="row-name">${esc(r.name)}</div><div class="row-meta">${esc(r.original)}<br>${r.date} · ${size(r.amount)}</div></div><div class="row-actions"><span class="pill ${r.offline||r.conflict?'warn':'good'}">${r.offline?'原卷离线':r.conflict?'原位置冲突':'可审阅恢复'}</span><button onclick="cmRecoveryDetail('${r.id}')">详情 ›</button></div></div>`).join('')||'<div class="empty-state">当前没有隔离内容</div>'}<div class="request-row"><span class="row-main muted">${state.recoverySelection.size} 项已选${state.recoverySelection.size>list.filter(r=>state.recoverySelection.has(r.id)).length?' · 包含筛选外选择':''}</span><div class="feature-actions"><button class="danger" ${!state.recoverySelection.size?'disabled':''} onclick="createRecovery('purge')">清空计划</button><button class="primary" ${!state.recoverySelection.size?'disabled':''} onclick="createRecovery('restore')">恢复计划 →</button></div></div></section>`+
    `<aside class="panel"><h2>每次恢复，重新确认</h2>${cmFact('原位置已有文件','默认不覆盖')}${cmFact('原卷离线','保留隔离清单')}${cmFact('永久清空','独立计划与批准')}<div class="split-note"><span class="symbol">↶</span><span>更换目标会生成新计划。系统回收站的恢复入口按真实平台能力提供。</span></div><button class="help-link section-gap" onclick="route('activity')">查看来源与活动 →</button></aside></div>`;
}
function cmRecoveryDetail(id){
  const r=state.recovery.find(r=>r.id===id);if(!r)return;
  drawer(r.name,`<span class="badge">隔离清单 · ${r.id}</span><div class="detail-path">${esc(r.original)}</div>${cmFact('隔离时间',r.date)}${cmFact('逻辑量',size(r.amount))}${cmFact('原卷',r.offline?'离线 · DEMO-VOLUME-B':'DEMO-VOLUME-A')}${cmFact('目标条件',r.conflict?'同名新文件存在':'尚需执行前复核')}<div class="detail-section"><h3>恢复前检查</h3><div class="check-item"><span class="check-mark">1</span><div><b>隔离对象身份</b><div class="row-meta">与演示清单的同一对象核对</div></div></div><div class="check-item"><span class="check-mark ${r.offline||r.conflict?'warn':''}">2</span><div><b>卷与原位置</b><div class="row-meta">${r.offline?'卷离线时不会换到同路径的新卷':r.conflict?'现有文件与隔离文件都保留':'没有默认覆盖操作'}</div></div></div></div>`,
    r.offline?`<button onclick="cmReconnectVolume('${id}')">模拟原卷重新连接</button>`:`<button class="primary" onclick="state.recoverySelection.add('${id}');closeDrawer();createRecovery('restore')">审阅恢复计划 →</button>`);
}
function cmReconnectVolume(id){const r=state.recovery.find(r=>r.id===id);r.offline=false;closeDrawer();render();toast('演示原卷身份匹配，恢复仍需独立计划');}
const cmBaseRecovery=createRecovery;
createRecovery=function(kind,destination=null){if(state.recovery.some(r=>state.recoverySelection.has(r.id)&&r.offline))return toast('原卷离线，保留清单并等待身份核对');cmBaseRecovery(kind,destination);};
const cmBaseReconcile=finishReconcile;
reconcile=function(id){cmFeatures.reconciliation={id,checked:false};cmRenderReconcile();};
function cmRenderReconcile(){
  const info=cmFeatures.reconciliation,h=state.history.find(h=>h.id===info.id),op=state.operations[h?.planId],items=op?op.results.filter(r=>r.status==='unknown').map(r=>r.item):[{name:'过期临时文件',root:'C:/Users/Lin/AppData/Local/Demo/Cache',id:'demo-unknown'}];
  modal('中断操作核对',`<span class="badge">只读对照 · 演示</span><div class="workflow-strip"><span class="step active"><span>1</span>源 / 目标</span><span class="arrow">→</span><span class="step ${info.checked?'active':''}"><span>2</span>查明结果</span><span class="arrow">→</span><span class="step"><span>3</span>记录核对</span></div>${items.map(i=>`<div class="row"><b>${esc(i.name)}</b><span class="pill ${info.checked?'good':'warn'}">${info.checked?'可确认未执行':'结果未知'}</span></div><div class="reconcile-grid"><div class="panel"><h3>源位置</h3><div class="detail-path">${esc(i.root||i.original||'示例源位置')}</div><span class="pill ${info.checked?'good':''}">${info.checked?'示例原对象仍在':'等待身份对照'}</span></div><div class="panel"><h3>隔离位置</h3><div class="detail-path">演示私有隔离位置 / ${esc(i.id)}</div><span class="pill">${info.checked?'未出现对应示例对象':'等待目标对照'}</span></div></div>`).join('')}<div class="notice ${info.checked?'':'warn'}">${info.checked?'这组演示对照支持“未执行”结论，后续处理需要另建计划。':'不会因缺少成功日志直接认定未执行，也不会重放旧动作。'}</div>`,
    info.checked?`<button class="primary" onclick="cmCommitReconcile()">记录核对结果</button>`:`<button class="primary" onclick="cmFeatures.reconciliation.checked=true;cmRenderReconcile()">检查演示对象</button>`);
}
function cmCommitReconcile(){
  const info=cmFeatures.reconciliation;if(!info?.checked)return;
  const history=state.history.find(h=>h.id===info.id),op=state.operations[history?.planId],p=state.plans.find(p=>p.id===history?.planId),unknown=op?.results.filter(r=>r.status==='unknown')||[];
  if(p?.cmReserved&&unknown.length){cmFeatures.policy.usedMiB=Math.max(0,cmFeatures.policy.usedMiB-unknown.reduce((n,r)=>n+(r.item.amount||0),0));cmFeatures.policy.usedItems=Math.max(0,cmFeatures.policy.usedItems-unknown.length);}
  cmBaseReconcile(info.id);cmLog(5,{kind:'operation_report',history_id:info.id,status:'partial',reconciliation:'synthetic_source_present_target_absent',retryable:false},`cleanmanager operations show OP-${info.id} --json`);cmFeatures.reconciliation=null;if(state.page==='agent')render();
}
function cmJournal(id){
  const op=state.operations[id],p=state.plans.find(p=>p.id===id);if(!op||!p)return;
  drawer('操作轨迹',`<span class="badge">OP-${id} · 演示</span>${cmFact('来源',esc(p.origin||'人工'))}${cmFact('授权',p.cmAuthorization?.kind==='policy'?'有限策略':'人工审阅')}<div class="detail-section"><h3>逐项意图与结果</h3>${op.items.map((i,n)=>{const result=op.results[n];return `<div class="journal-line"><span class="journal-time">${String(n+1).padStart(2,'0')}</span><div class="row-main"><b>${esc(i.name)}</b><div class="row-meta">${result?result.status==='unknown'?'已登记意图 · 结果未确认':result.status==='success'?'意图 → 现场核验 → 动作 → 结果':'意图 → 核验未满足 → 跳过':'尚未开始'}</div></div><span class="pill ${result?.status==='unknown'?'warn':''}">${result?.status==='success'?'已确认':result?.status==='unknown'?'需核对':result?.status==='skipped'?'跳过':'未开始'}</span></div>`;}).join('')}</div><div class="notice">轨迹为浏览器演示，不是本机文件操作日志。</div>`,'<button onclick="cmReportModal()">查看结构化报告</button>');
}
function cmReportModal(){
  const op=state.operations[state.viewOperationId]||state.operation;
  const report=op?{schema_version:'prototype/1',synthetic:true,operation_id:'OP-'+op.planId,plan_id:op.planId,status:op.status==='unknown'?'reconciliation_required':op.status,items:op.items.map((i,n)=>({entry_id:i.id,name:i.name,status:op.results[n]?.status||'not_started'})),observed_free_delta_bytes:'0',attribution:'Synthetic observation, not measured OS free space'}:{schema_version:'prototype/1',synthetic:true,history:state.history.map(h=>({id:h.id,title:h.title,status:h.status}))};
  modal('演示报告','<pre class="console-code">'+cmJson(report)+'</pre>',`<button onclick="cmCopyText(${esc(JSON.stringify(JSON.stringify(report,null,2)))})">复制 JSON</button>`);
}

const cmOriginalProtected=candidateProtected;
candidateProtected=function(c){return typeof c?.root==='string'&&cmOriginalProtected(c)||cmFeatures.pausedRules.has(c?.rule);};
candidateRow=function(c){const html=cmLegacy.candidateRow(c);return cmFeatures.pausedRules.has(c.rule)?html.replace('自定义保护','规则已暂停'):html;};
candidateDetail=function(id){cmLegacy.candidateDetail(id);const c=getCandidate(id);if(c?.rule&&cmRules.some(r=>r.key===c.rule))document.querySelector('#drawerContent .drawer-actions').insertAdjacentHTML('afterbegin',`<button onclick="cmRuleDetail('${c.rule}')">查看规则依据 →</button>`);};
policyMatches=function(p){return cmPolicyDecision(p).allowed;};
plansPage=cmPlansPage;
recoveryPage=cmRecoveryPage;
planInfo=function(id){const p=state.plans.find(p=>p.id===id);cmRegisterPlan(p);drawer('计划 body 与摘要',`<span class="badge">${p.id}</span>${cmFact('演示 SHA-256',`<span class="digest-value" data-plan-digest="${p.id}">${esc(p.cmDigest)}</span>`)}<pre class="console-code">${cmJson(p.cmBody)}</pre><div class="notice">摘要绑定演示 body，不证明授权；这个原型格式不是已发布 CLI 契约。</div>`,`<button onclick="cmCopyText(JSON.stringify(activePlan().cmBody,null,2))">复制演示 body</button>`);};
agentHandoff=function(){if(state.page==='plans'&&activePlan())cmHandoffDialog(activePlan());else route('agent');};
scanGaps=function(){route('coverage');};
makePlan=function(kind,items,opts={}){
  if(!items.length)return toast('请先选择内容');
  if(opts.stay){const now=Date.now();state.planCounter++;const p={id:'P-DEMO-'+String(state.planCounter).padStart(3,'0'),kind,items:Object.freeze(items.map(i=>Object.freeze({...i}))),mode:opts.mode||'同卷隔离',concept:!!opts.concept,created:now,expires:now+900000,status:'等待审阅',destination:opts.destination||null};state.plans.unshift(p);state.currentPlan=p.id;}
  else cmLegacy.makePlan(kind,items,opts);
  const p=activePlan();p.origin=opts.origin||'人工选择';p.requestId=opts.requestId||null;cmFeatures.planTab='items';cmFeatures.planQuery='';cmRegisterPlan(p);render();return p;
};
replacePlanMode=function(mode){
  const previous=activePlan();if(!previous||previous.mode===mode||previous.status!=='等待审阅')return;
  cmLegacy.replacePlanMode(mode);const p=activePlan();p.origin=previous.origin;p.requestId=previous.requestId;previous.cmReplacedBy=p.id;previous.status='已替换';const r=cmFeatures.requests.find(r=>r.id===p.requestId);if(r)r.planId=p.id;render();
};
regeneratePlan=function(){const previous=activePlan();cmLegacy.regeneratePlan();const p=activePlan();p.origin=previous.origin;p.requestId=previous.requestId;previous.cmReplacedBy=p.id;previous.status='已替换';const r=cmFeatures.requests.find(r=>r.id===p.requestId);if(r)r.planId=p.id;render();};
applyPlan=function(id){
  const p=state.plans.find(p=>p.id===id);if(!p)return;
  if(p.cmReplacedBy)return toast('旧计划已被替代，请审阅新的计划');
  if(p.status!=='等待审阅')return cmLegacy.applyPlan(id);
  if(state.operation?.status==='running')return toast('已有操作正在进行');
  if(Date.now()>=p.expires||state.scenario==='expired')return toast('计划已过期，请重新生成');
  if(p.cmDigest?.length!==64)return toast('正在准备演示摘要，请稍后');
  const decision=cmPolicyDecision(p),human=cmFeatures.consent?.has(p.id)&&state.page==='plans';
  if(!decision.allowed&&!human&&!p.cmAuthorization){cmLog(3,{kind:'error',code:'needs_approval',plan_id:id});return toast('这个计划尚未获得授权');}
  if(!p.cmAuthorization)p.cmAuthorization={kind:decision.allowed?'policy':'human',digest:p.cmDigest,policyVersion:cmFeatures.policy.version,at:Date.now()};
  if(p.cmAuthorization.digest!==p.cmDigest)return toast('计划摘要已改变，需要重新审阅');
  cmLegacy.applyPlan(id);
  if(p.status==='执行中'&&p.cmAuthorization.kind==='policy'&&!p.cmReserved){p.cmReserved=true;cmFeatures.policy.usedMiB+=cmPlanTotal(p)||0;cmFeatures.policy.usedItems+=p.items.length;}
  if(p.status==='执行中')cmLog(0,{kind:'operation_status',operation_id:'OP-'+id,plan_id:id,status:'running',authorization:p.cmAuthorization.kind});
};
executionTick=function(){
  const op=state.operation,p=op&&state.plans.find(p=>p.id===op.planId);
  if(op?.status==='running'&&p&&(Date.now()>=p.expires||p.cmAuthorization?.kind==='policy'&&(!state.preauth||p.cmAuthorization.policyVersion!==cmFeatures.policy.version||Date.now()>=cmFeatures.policy.expires))){op.stop=true;op.phase='授权或计划已失效 · 停止中';op.cmStopReason='计划/策略到期、撤销或版本变化';}
  cmLegacy.executionTick();
};
finishOperation=function(){
  const op=state.operation,p=state.plans.find(p=>p.id===op.planId);
  if(p.cmReserved&&!p.cmQuotaSettled){const retained=op.results.filter(r=>r.status==='success'||r.status==='unknown');cmFeatures.policy.usedMiB=Math.max(0,cmFeatures.policy.usedMiB-(cmPlanTotal(p)||0)+retained.reduce((n,r)=>n+(r.item.amount||0),0));cmFeatures.policy.usedItems=Math.max(0,cmFeatures.policy.usedItems-p.items.length+retained.length);p.cmQuotaSettled=true;}
  cmLegacy.finishOperation();cmLog(op.status==='unknown'?8:op.status==='partial'?5:op.status==='cancelled'?130:0,{kind:'operation_report',operation_id:'OP-'+op.planId,plan_id:op.planId,status:op.status==='unknown'?'reconciliation_required':op.status,succeeded:op.results.filter(r=>r.status==='success').length,skipped:op.results.filter(r=>r.status==='skipped').length,unknown:op.results.filter(r=>r.status==='unknown').length,not_started:op.items.length-op.index});
  if(state.page==='agent'&&cmFeatures.agentTab!=='policy')render();
};
runPage=function(){const op=state.operations[state.viewOperationId]||state.operation;return cmLegacy.runPage()+(op?`<div class="panel section-gap"><div class="feature-head"><h2>来源与操作轨迹</h2><span class="quiet-label">OP-${op.planId}</span></div>${op.cmStopReason?`<div class="notice warn">${op.cmStopReason}，已发生的操作保留。</div>`:''}<div class="feature-actions"><button onclick="cmJournal('${op.planId}')">查看逐项轨迹</button><button onclick="cmReportModal()">结构化报告</button><button onclick="route('agent')">返回 Agent 请求 →</button></div></div>`:'');};
homePage=function(){return cmLegacy.homePage()+`<div class="journey-shortcuts">${feature('>_','Agent 请求','接手同一计划，查明逐项结果','请求与交接','agent')}${feature('▱','扫描覆盖','已知小计、读取缺口与平台能力','查看扫描记录','coverage')}${feature('◇','规则与依据','查看归属、影响与保护条件','审阅规则','rules')}</div>`;};
function cmUpgradeBase(){
  const actions=document.querySelector('.page-heading .heading-actions');
  if(['clean','disk'].includes(state.page)&&actions)actions.insertAdjacentHTML('afterbegin','<button onclick="route(\'coverage\')">扫描记录</button>');
  if(state.page==='activity'&&actions){actions.innerHTML='<button onclick="cmReportModal()">查看演示报告</button>';document.querySelector('.page-heading').insertAdjacentHTML('afterend',`<div class="feature-stats">${cmStat('活动记录',state.history.length,'计划、授权与结果关联')}${cmStat('待核对',state.history.filter(h=>h.kind==='unknown').length,'未知副作用不自动重试')}${cmStat('隔离内容',state.recovery.length,'恢复 / 清空独立计划')}</div>`);}
  if(state.page==='settings'){
    const panel=document.querySelector('.settings-layout>section:last-child');
    if(state.settingsTab==='policy'&&panel)panel.innerHTML=cmPolicyPanel();
    if(state.settingsTab==='capabilities'&&panel)panel.insertAdjacentHTML('beforeend','<div class="feature-actions section-gap"><button class="primary" onclick="route(\'capabilities\')">查看平台能力矩阵 →</button><button onclick="route(\'coverage\')">扫描覆盖</button></div>');
  }
}
filterCommands=function(query){
  const q=query.trim().toLowerCase(),aliases={agent:'Codex Claude CLI Skill 策略 授权 额度',coverage:'扫描 取消 完整性 覆盖 云文件',capabilities:'Windows macOS NTFS APFS 权限 平台',rules:'规则 来源 依据 保护 缓存',plans:'计划 摘要 审阅 版本',recovery:'恢复 隔离 冲突 清空',activity:'记录 核对 中断 日志',ai:'会话 模型 工作树 开发者'};
  const list=Object.entries(pageNames).filter(([id,name])=>id!=='run'&&(name+' '+(aliases[id]||'')).toLowerCase().includes(q));
  $('commandList').innerHTML=list.map(([id,name])=>`<button onclick="cmJumpTarget('${id}',${esc(JSON.stringify(query))})"><span>${name}${q&&aliases[id]?`<small class="row-meta" style="display:block">${esc(aliases[id])}</small>`:''}</span><span>↵</span></button>`).join('')||'<div class="empty-state">没有匹配功能</div>';
};
function cmJumpTarget(page,query){if(page==='agent')cmFeatures.agentTab=/cli|skill/i.test(query)?'cli':/策略|授权|额度/.test(query)?'policy':'requests';closeDialog();route(page);}
const cmViews={agent:cmAgentPage,coverage:cmCoveragePage,capabilities:cmCapabilityPage,rules:cmRulesPage};
render=function(){
  const page=location.hash.slice(1);
  if(cmViews[page]){
    state.page=page;$('mainNav').innerHTML=mainPages.map(p=>`<button data-page="${p}" class="${p===page?'active':''}">${({ai:'AI',disk:'分析',maintenance:'维护'}[p]||pageNames[p])}</button>`).join('');
    document.querySelectorAll('.rail button').forEach(b=>b.classList.toggle('active',b.dataset.page===(page==='capabilities'?'settings':page)));
    $('main').innerHTML=cmViews[page]();updateSelectionBar();updateTaskChip();document.title='CleanManager · '+pageNames[page];
  }else{cmLegacy.render();cmUpgradeBase();}
};
window.addEventListener('hashchange',()=>window.scrollTo({top:0,behavior:'instant'}));
render();
