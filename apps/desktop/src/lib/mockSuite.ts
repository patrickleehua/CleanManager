// DEVELOPMENT-ONLY fixture for the five-tool suite pages (build artifacts, installers, manual
// trash, startup, insights, status, readiness). Shapes mirror the host DTOs; plans go through the
// shared review fixture (`__plan`), so the review and result pages behave as for cleanup plans.

import type { Backend } from './api';
import type {
  Catalog,
  CatalogItem,
  CatalogPreview,
  Insight,
  IpcError,
  Mode,
  PlanDetail,
  PlanItem,
  PurgeRoot,
  PurgeRoots,
  ReadinessOp,
  StartupItem,
  StatusMetric,
  TrashInspection,
} from './types';

type SuiteBackend = Pick<
  Backend,
  | 'purgeRoots'
  | 'purgeRootsAdd'
  | 'purgeRootsRemove'
  | 'purgeScan'
  | 'installerLocations'
  | 'installerScan'
  | 'createCatalogPlan'
  | 'trashInspect'
  | 'trashPlan'
  | 'startupList'
  | 'startupPlan'
  | 'insightsList'
  | 'statusSnapshot'
  | 'readiness'
  | 'revealEntry'
>;

type MakePlan = (kind: PlanDetail['kind'], mode: Mode | null, items: PlanItem[], extra?: Partial<PlanDetail>) => string;
type EntryInfo = (entryId: string) => { name: string; path: string; kind: string; logical: string | null; unknownDesc: number } | null;

const MiB = 1024n * 1024n;
const GiB = 1024n * MiB;
const HOME = 'C:\\Users\\Lin';
const err = (code: string, reason: string, message: string): IpcError => ({ code, reason, message });
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
const iso = (minutes = 0) => new Date(Date.now() + minutes * 60_000).toISOString().replace(/\.\d+Z$/, 'Z');
const b = (n: bigint) => n.toString();

function item(catalog: Catalog, n: number, path: string, tier: CatalogItem['tier'], size: bigint | null, extra: Partial<CatalogItem> = {}): CatalogItem {
  const name = path.split('\\').pop() ?? path;
  const blocked = tier === 'blocked';
  // One candidate set per scanned root (purge) or declared location (installer), as the host stores them.
  const label = extra.group_label ?? '';
  const root = catalog === 'purge' ? label.split('\\').slice(0, -1).join('\\') : label;
  return {
    candidate_set_id: `cs_${catalog}_${root.replace(/[^A-Za-z0-9]+/g, '_').toLowerCase()}`,
    candidate_id: `${catalog === 'purge' ? 'pc' : 'ic'}_${n}`,
    entry_id: String(100 + n),
    name,
    path_display: path,
    tier,
    tier_reason: '',
    skip_reason: blocked ? 'protected' : null,
    plannable: !blocked,
    selected_by_default: tier === 'default_selected',
    allowed_modes: blocked ? [] : ['trash', 'quarantine', 'delete'],
    logical_bytes: size === null ? null : b(size),
    unknown_size_count: size === null ? 1 : 0,
    rebuild_class: catalog === 'purge' ? 'local_rebuild' : 'network_redownload',
    impact: '',
    notes: [],
    group_label: '',
    ...extra,
  };
}

const PURGE_ITEMS: CatalogItem[] = [
  item('purge', 1, `${HOME}\\source\\repos\\cm-tools\\target`, 'default_selected', 6n * GiB + 140n * MiB, {
    group_label: `${HOME}\\source\\repos\\cm-tools`,
    tier_reason: 'build output next to its project marker; rebuilt locally',
    impact: '下次构建会重新编译全部依赖，耗时更长。',
    notes: ['项目标记：Cargo.toml', '恢复方式：cargo build'],
  }),
  item('purge', 2, `${HOME}\\Projects\\site\\.next`, 'default_selected', 412n * MiB, {
    group_label: `${HOME}\\Projects\\site`,
    tier_reason: 'build output next to its project marker; rebuilt locally',
    impact: '下次 next build 会重新生成。',
    notes: ['项目标记：package.json, next.config.js', '恢复方式：npm run build'],
  }),
  item('purge', 3, `${HOME}\\Projects\\site\\dist`, 'review_only', 120n * MiB, {
    group_label: `${HOME}\\Projects\\site`,
    tier_reason: 'recent project: listed, not preselected',
    unknown_size_count: 3,
    impact: '可能包含手动放入的发布文件，请先查看。',
    notes: ['项目在 7 天内修改过', '3 个文件大小未知（读取被拒绝）'],
  }),
  item('purge', 4, 'D:\\Work\\Projects\\ml-lab\\.venv', 'review_only', 3n * GiB + 210n * MiB, {
    group_label: 'D:\\Work\\Projects\\ml-lab',
    tier_reason: 'dependencies: review only, reinstalling needs the network',
    rebuild_class: 'network_redownload',
    impact: '需要联网重新安装依赖（pip install -r requirements.txt）。',
    notes: ['项目标记：pyproject.toml, requirements.txt'],
  }),
  item('purge', 5, 'D:\\Work\\Projects\\ml-lab\\build', 'default_selected', 88n * MiB, {
    group_label: 'D:\\Work\\Projects\\ml-lab',
    tier_reason: 'build output next to its project marker; rebuilt locally',
    impact: '重新构建即可生成。',
  }),
  item('purge', 6, `${HOME}\\source\\repos\\legacy\\build`, 'blocked', 640n * MiB, {
    group_label: `${HOME}\\source\\repos\\legacy`,
    tier_reason: 'vcs_tracked: build/ contains files tracked by git',
    skip_reason: 'vcs_tracked',
    impact: '这个目录里有被版本控制跟踪的文件，不能作为构建产物处理。',
  }),
  item('purge', 7, 'D:\\Work\\Projects\\storefront\\node_modules', 'blocked', 1n * GiB + 820n * MiB, {
    group_label: 'D:\\Work\\Projects\\storefront',
    tier_reason: 'user_protected: D:\\Work\\Projects\\storefront',
    skip_reason: 'protected',
    rebuild_class: 'network_redownload',
    impact: '位于你保护的文件夹中。',
  }),
];

const INSTALLER_ITEMS: CatalogItem[] = [
  item('installer', 1, `${HOME}\\Downloads\\VSCodeUserSetup-x64-1.104.0.exe`, 'review_only', 104n * MiB + 300n * 1024n, {
    group_label: 'downloads',
    tier_reason: 'installers are review-only: choose them one by one or by source',
    impact: '需要时可从官网重新下载。',
    notes: ['签名：Microsoft Corporation', '已安装 Visual Studio Code 1.105.1（更新的版本）'],
  }),
  item('installer', 2, `${HOME}\\Downloads\\python-3.12.6-amd64.exe`, 'review_only', 26n * MiB + 410n * 1024n, {
    group_label: 'downloads',
    tier_reason: 'installers are review-only: choose them one by one or by source',
    impact: '修复或卸载 Python 时可能需要原安装包。',
    notes: ['签名：Python Software Foundation', '已安装 Python 3.12.7'],
  }),
  item('installer', 3, `${HOME}\\Downloads\\node-v22.11.0-x64.msi`, 'review_only', 31n * MiB, {
    group_label: 'downloads',
    tier_reason: 'installers are review-only: choose them one by one or by source',
    impact: 'MSI 修复可能需要原安装包。',
    notes: ['签名：OpenJS Foundation', '已安装 Node.js 22.11.0（同版本）'],
  }),
  item('installer', 4, `${HOME}\\Downloads\\Win11_24H2_Chinese_Simplified_x64.iso`, 'blocked', 5n * GiB + 412n * MiB, {
    group_label: 'downloads',
    tier_reason: 'mounted: the image is attached as drive E:',
    skip_reason: 'mounted',
    impact: '映像正在挂载使用。',
    notes: ['已挂载：在资源管理器中右键 > 弹出，然后重新扫描'],
  }),
  item('installer', 5, `${HOME}\\Desktop\\WeChatSetup.exe`, 'review_only', 238n * MiB, {
    group_label: 'desktop',
    tier_reason: 'installers are review-only: choose them one by one or by source',
    impact: '需要时可从官网重新下载。',
    notes: ['签名：Tencent Technology (Shenzhen) Company Limited'],
  }),
  item('installer', 6, `${HOME}\\Downloads\\未确认 381244.crdownload`, 'review_only', null, {
    group_label: 'browser_downloads',
    tier_reason: 'evidence incomplete',
    skip_reason: 'partial_evidence',
    plannable: false,
    allowed_modes: [],
    impact: '浏览器可能仍在下载。',
    notes: ['大小未知：文件正在被写入'],
  }),
];

function preview(catalog: Catalog, items: CatalogItem[]): CatalogPreview {
  const def = items.filter((i) => i.selected_by_default && i.plannable);
  const sets = [...new Set(items.map((i) => i.candidate_set_id))];
  return {
    supported: true,
    catalog,
    observed_at: iso(0),
    sets: sets.map((id, n) => {
      const label = items.find((i) => i.candidate_set_id === id)!.group_label;
      return { candidate_set_id: id, scan_id: `s_${catalog}_${n}`, root_display: catalog === 'purge' ? label.split('\\').slice(0, -1).join('\\') : label, source_label: catalog === 'installer' ? label : null };
    }),
    items,
    summary: {
      default_selected_count: def.length,
      default_selected_known_bytes: b(def.reduce((a, i) => a + BigInt(i.logical_bytes ?? '0'), 0n)),
      default_selected_unknown_count: def.reduce((a, i) => a + i.unknown_size_count, 0),
      review_only_count: items.filter((i) => i.tier === 'review_only').length,
      blocked_count: items.filter((i) => i.tier === 'blocked').length,
      skipped_count: items.filter((i) => i.skip_reason !== null).length,
    },
    issues:
      catalog === 'purge'
        ? [{ code: 'root_missing', scope_display: 'E:\\Archive\\code', message: 'The configured root does not exist or its volume is offline; it was skipped' }]
        : [],
  };
}

const STARTUP: StartupItem[] = [
  st('st_onedrive', 'Microsoft OneDrive', 'run_key', 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run : OneDrive', `${HOME}\\AppData\\Local\\Microsoft\\OneDrive\\OneDrive.exe`, 'Microsoft Corporation', 'user', 'enabled'),
  st('st_teams', 'Microsoft Teams', 'store_startup_task', 'MSTeams_8wekyb3d8bbwe!TeamsStartupTask', 'C:\\Program Files\\WindowsApps\\MSTeams_25153.1010.3727.5483_x64__8wekyb3d8bbwe\\ms-teams.exe', 'Microsoft Corporation', 'user', 'enabled', { publisher_evidence: 'package_identity' }),
  st('st_steam', 'Steam', 'run_key', 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run : Steam', 'C:\\Program Files (x86)\\Steam\\steam.exe', 'Valve Corp.', 'user', 'disabled'),
  st('st_razer', 'Razer Synapse', 'scheduled_task', '\\Razer\\Razer Synapse Service Process', 'C:\\Program Files (x86)\\Razer\\Synapse3\\Service\\Razer Synapse Service Process.exe', 'Razer USA Ltd.', 'system', 'enabled', { toggle_capability: 'permission_required', reason: 'System-wide task: changing it needs an administrator' }),
  st('st_old', 'Removed Tool', 'startup_folder', `${HOME}\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\Removed Tool.lnk`, 'D:\\Tools\\Removed\\tool.exe', null, 'user', 'broken', { publisher_evidence: 'none', reason: 'Target is missing on a mounted volume' }),
  st('st_defender', 'Windows Security notification icon', 'run_key', 'HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Run : SecurityHealth', 'C:\\Windows\\System32\\SecurityHealthSystray.exe', 'Microsoft Windows', 'system', 'enabled', { protected: true, toggle_capability: 'unsupported', reason: 'Security software: shown read-only' }),
];

function st(id: string, name: string, source: string, definition: string, target: string, publisher: string | null, scope: string, state: string, extra: Partial<StartupItem> = {}): StartupItem {
  return {
    startup_item_id: id,
    name,
    source,
    definition,
    target: { display: target },
    publisher,
    publisher_evidence: publisher ? 'authenticode' : 'none',
    scope,
    state,
    toggle_capability: 'available',
    protected: false,
    reason: null,
    ...extra,
  };
}

const INSIGHTS: Insight[] = [
  ins('i_hiberfil', 'windows.hibernation_file', 'Hibernation file', 'C:\\hiberfil.sys', 6n * GiB + 380n * MiB, 'file_metadata', '保存休眠和快速启动时的内存内容。', [
    { tool: 'powercfg', steps: 'powercfg /hibernate off', requires_admin: true, impact: '关闭休眠与快速启动；Windows 会自行删除该文件。' },
  ]),
  ins('i_pagefile', 'windows.page_file', 'Page file', 'C:\\pagefile.sys', 12n * GiB, 'file_metadata', '虚拟内存的分页文件，由系统按需调整。', [
    { tool: '系统属性 › 高级 › 性能设置 › 虚拟内存', steps: '调整分页文件大小或改为系统管理', requires_admin: true, impact: '过小可能导致程序在内存紧张时崩溃。' },
  ]),
  ins('i_winsxs', 'windows.component_store', 'Component store (WinSxS)', 'C:\\Windows\\WinSxS', null, 'not_measured', 'Windows 组件与更新的存储；资源管理器显示的大小包含大量硬链接，并非实际占用。', [
    { tool: 'DISM', steps: 'Dism.exe /Online /Cleanup-Image /AnalyzeComponentStore', requires_admin: true, impact: '只分析，不改动。清理用 /StartComponentCleanup。' },
  ], '不测量：硬链接会让目录遍历结果严重偏大'),
  ins('i_wsl', 'windows.wsl_disk', 'WSL disk (Ubuntu)', `${HOME}\\AppData\\Local\\Packages\\CanonicalGroupLimited.Ubuntu_79rhkp1fndgsc\\LocalState\\ext4.vhdx`, 38n * GiB + 600n * MiB, 'file_metadata', 'WSL 发行版的虚拟磁盘，删除其中的文件不会自动缩小它。', [
    { tool: 'wsl', steps: 'wsl --manage Ubuntu --set-sparse true', requires_admin: false, impact: '启用稀疏模式后，空间会逐步归还给 Windows。' },
  ]),
  ins('i_shadow', 'windows.shadow_storage', 'Restore points (shadow storage)', null, 4n * GiB + 100n * MiB, 'system_query', '系统还原点与卷影副本。', [
    { tool: '系统保护', steps: '系统属性 › 系统保护 › 配置 › 删除', requires_admin: true, impact: '删除后无法再还原到这些时间点。' },
  ], null, 3),
];

function ins(id: string, kind: string, title: string, path: string | null, size: bigint | null, basis: string, explanation: string, guidance: Insight['guidance'], note: string | null = null, count: number | null = null): Insight {
  return {
    insight_id: id,
    kind,
    title,
    path: path ? { display: path } : null,
    logical_bytes: size === null ? null : b(size),
    count,
    size_basis: basis,
    size_note: note,
    observed_at: iso(-3),
    explanation,
    handling: 'read_only_guidance',
    guidance,
  };
}

export function mockSuite(makePlan: MakePlan, entry: EntryInfo): SuiteBackend {
  let roots: PurgeRoot[] = [
    { root_id: 'r_8c1f0e22a91b', display: `${HOME}\\source\\repos`, source: 'default', exists: true },
    { root_id: 'r_51d9e0f3c2aa', display: `${HOME}\\Projects`, source: 'default', exists: true },
    { root_id: 'r_0b7d33e1f046', display: 'D:\\Work\\Projects', source: 'user', exists: true },
    { root_id: 'r_e4a2c9017bd3', display: 'E:\\Archive\\code', source: 'user', exists: false },
  ];
  let added = 0;
  const list = (): PurgeRoots => ({ supported: true, roots: roots.map((r) => ({ ...r })), recent_days: 7, max_depth: 6 });
  const startup = STARTUP.map((s) => ({ ...s }));
  let cpu = 23;

  const catalogPlan = (catalog: Catalog, ids: string[], mode: Mode): string => {
    const all = catalog === 'purge' ? PURGE_ITEMS : INSTALLER_ITEMS;
    const chosen = ids.map((id) => all.find((i) => i.candidate_id === id));
    if (chosen.some((c) => !c || !c.plannable)) throw err('policy_denied', 'not_selectable', 'candidate cannot be planned');
    let n = 0;
    const items: PlanItem[] = chosen.flatMap((c) => {
      const parts = catalog === 'purge' ? ['deps', 'build', 'incremental', '.fingerprint'] : [''];
      const total = BigInt(c!.logical_bytes ?? '0');
      return parts.map((p, i) => {
        n += 1;
        const size = parts.length === 1 ? total : i === parts.length - 1 ? total - (total / 4n) * 3n : total / 4n;
        return {
          action_id: `a${String(n).padStart(7, '0')}`,
          name: p || c!.name,
          path_display: p ? `${c!.path_display}\\${p}` : c!.path_display,
          logical_bytes: b(size),
          allocated_bytes: b(size),
          candidate_id: c!.candidate_id,
          original_path_display: null,
          quarantine_id: null,
          item_kind: p ? 'directory' : 'file',
        };
      });
    });
    return makePlan('cleanup', mode, items, {
      catalog,
      rule_refs: catalog === 'purge' ? ['purge.cargo-target@1', 'purge.next-build@1'] : ['installer.windows-exe@1', 'installer.msi@1'],
      scan_root_display: chosen[0]!.group_label,
      default_selected_items: chosen.filter((c) => c!.selected_by_default).length,
      not_included: null,
    });
  };

  return {
    async purgeRoots() {
      await delay(80);
      return list();
    },
    async purgeRootsAdd() {
      await delay(200);
      added += 1;
      roots = [...roots, { root_id: `r_new_${added}`, display: `D:\\Code\\workspace-${added}`, source: 'user', exists: true }];
      return list();
    },
    async purgeRootsRemove(rootId) {
      await delay(120);
      const r = roots.find((x) => x.root_id === rootId);
      if (!r) throw err('invalid_argument', 'unknown_root', 'not a configured root');
      roots = roots.filter((x) => x.root_id !== rootId);
      return list();
    },
    async purgeScan() {
      await delay(1600);
      return preview('purge', PURGE_ITEMS);
    },
    async installerLocations() {
      return {
        supported: true,
        max_depth: 2,
        locations: [
          { display: `${HOME}\\Downloads`, source_label: 'downloads', exists: true },
          { display: `${HOME}\\Desktop`, source_label: 'desktop', exists: true },
          { display: `${HOME}\\Downloads（浏览器下载位置）`, source_label: 'browser_downloads', exists: true },
        ],
      };
    },
    async installerScan() {
      await delay(1100);
      return preview('installer', INSTALLER_ITEMS);
    },
    async createCatalogPlan(catalog, _set, ids, mode) {
      await delay(300);
      return { plan_id: catalogPlan(catalog, ids, mode) };
    },
    async trashInspect(scanId, entryId): Promise<TrashInspection> {
      await delay(260);
      const e = entry(entryId);
      if (!e) throw err('invalid_argument', 'not_found', 'entry is not in this scan');
      const isProj = e.name === 'Projects' || e.name === 'storefront';
      const isDir = e.kind === 'directory';
      return {
        scan_id: scanId,
        entry_id: entryId,
        name: e.name,
        path_display: e.path,
        parent_display: e.path.split('\\').slice(0, -1).join('\\'),
        object_kind: e.kind,
        cloud: 'not_cloud',
        logical_bytes: e.logical,
        allocated_bytes: e.logical,
        files: isDir ? 42 : 1,
        directories: isDir ? 3 : 0,
        protected: isProj ? [{ path_display: `${e.path}${e.name === 'Projects' ? '\\storefront' : ''}\\.git`, reason: '默认保护类别：源码工作区' }] : [],
        protected_count: isProj ? 1 : 0,
        coverage_gaps: e.unknownDesc ? [{ path_display: `${e.path}\\Locked`, reason: '读取被拒绝' }] : [],
        gap_count: e.unknownDesc ? 1 : 0,
        trashable: !isProj && e.name !== 'Locked',
        refusal: isProj
          ? { code: 'policy_denied', reason: 'protected_descendants', message: '1 protected object(s) inside; select unprotected descendants instead' }
          : e.name === 'Locked'
            ? { code: 'policy_denied', reason: 'coverage_gap', message: 'part of this folder could not be read' }
            : null,
        holders: e.name.endsWith('.mov') ? [] : null,
        recycle_bin_ready: true,
        recycle_bin_problem: null,
        guidance: isProj ? '只能查看或复制路径；请改选其中未受保护的子项。' : '生成计划后需要你审阅批准才会移动。',
      };
    },
    async trashPlan(scanId, entryIds) {
      await delay(300);
      const items: PlanItem[] = entryIds.map((id, i) => {
        const e = entry(id);
        if (!e) throw err('invalid_argument', 'not_found', `entry ${id} is not in this scan`);
        return {
          action_id: `a${String(i + 1).padStart(2, '0')}`,
          name: e.name,
          path_display: e.path,
          logical_bytes: e.logical ?? '0',
          allocated_bytes: e.logical,
          candidate_id: null,
          original_path_display: null,
          quarantine_id: null,
          item_kind: e.kind === 'directory' ? 'directory' : 'file',
          action_type: 'fs_trash',
        };
      });
      const id = makePlan('manual_trash', 'trash', items, {
        contract: 'plan/v2',
        plan_kind_v2: 'manual_selection',
        scan_id: scanId,
        scan_revision: 1,
        scan_root_display: 'D:\\Work',
        cli: { show: '', review: '', apply: '' },
      });
      return { plan_id: id };
    },
    async startupList() {
      await delay(220);
      return {
        supported: true,
        is_admin: false,
        items: startup.map((s) => ({ ...s })),
        issues: [{ code: 'permission_denied', scope_display: 'Task Scheduler \\Microsoft\\Windows', message: '2 task folders could not be read without an administrator' }],
      };
    },
    async startupPlan(itemId, operation) {
      await delay(260);
      const s = startup.find((x) => x.startup_item_id === itemId);
      if (!s) throw err('invalid_argument', 'not_found', 'no startup item');
      if (s.protected) throw err('policy_denied', 'policy_denied', 'Security software is shown read-only');
      if (s.toggle_capability === 'permission_required') throw err('permission_required', 'permission_required', 'System-wide item: run CleanManager as an administrator to change it');
      if ((operation === 'disable' && s.state === 'disabled') || (operation === 'enable' && s.state === 'enabled')) throw err('conflict', 'conflict', `already ${s.state}`);
      const id = makePlan(
        'startup',
        null,
        [
          {
            action_id: 'a01',
            name: s.name,
            path_display: s.definition,
            logical_bytes: '0',
            allocated_bytes: null,
            candidate_id: null,
            original_path_display: null,
            quarantine_id: null,
            item_kind: 'startup',
            action_type: operation === 'disable' ? 'startup_disable' : 'startup_enable',
            scope: s.scope,
          },
        ],
        { contract: 'plan/v2', plan_kind_v2: 'startup', scan_id: null, scan_revision: null, scope_id: null, scan_root_display: null, rule_set_digest: null, total_logical_bytes: '0', allocated_known_bytes: null, requires_admin: s.scope === 'system' },
      );
      return { plan_id: id };
    },
    async insightsList() {
      await delay(300);
      return { supported: true, observed_at: iso(-3), items: INSIGHTS };
    },
    async statusSnapshot() {
      await delay(140);
      cpu = Math.max(4, Math.min(96, cpu + Math.round((Math.random() - 0.45) * 14)));
      const m = (name: string, value: number | null, unit: string, reason: string | null = null): StatusMetric => ({ name, availability: value === null ? 'unavailable' : 'available', value, unit, reason });
      return {
        supported: true,
        sampled_at: iso(0),
        stale: false,
        metrics: [
          m('cpu.total_percent', cpu, 'percent'),
          m('cpu.logical_count', 16, 'count'),
          m('memory.used_percent', 71.4, 'percent'),
          m('memory.total_bytes', 32 * 1024 ** 3, 'bytes'),
          m('memory.available_bytes', 9.15 * 1024 ** 3, 'bytes'),
          m('disk.system.free_percent', 11.8, 'percent'),
          m('disk.system.free_bytes', 56.2 * 1024 ** 3, 'bytes'),
          m('disk.system.total_bytes', 476 * 1024 ** 3, 'bytes'),
          m('disk.busy_percent', 6.5, 'percent'),
          m('disk.read_bytes_per_sec', 2.4 * 1024 ** 2, 'bytes_per_second'),
          m('disk.write_bytes_per_sec', 0.8 * 1024 ** 2, 'bytes_per_second'),
          m('thermal.max_zone_celsius', 58, 'celsius'),
          m('gpu.temperature_celsius', null, 'celsius', 'No documented Windows interface reports GPU temperature without vendor software'),
          m('battery.charge_percent', 84, 'percent'),
          m('battery.on_ac_power', 1, 'bool'),
          m('battery.health_percent', 91, 'percent'),
          m('network.received_bytes_per_sec', 310 * 1024, 'bytes_per_second'),
          m('network.sent_bytes_per_sec', 42 * 1024, 'bytes_per_second'),
        ],
        network: {
          classification: 'interface_type',
          received_bytes_per_sec: 310 * 1024,
          sent_bytes_per_sec: 42 * 1024,
          may_double_count: false,
          counted: [{ name: 'WLAN', class: 'physical', received_bytes_per_sec: 310 * 1024, sent_bytes_per_sec: 42 * 1024 }],
          separate: [
            { name: 'Example VPN', class: 'tunnel', received_bytes_per_sec: 290 * 1024, sent_bytes_per_sec: 38 * 1024 },
            { name: 'vEthernet (WSL)', class: 'virtual', received_bytes_per_sec: 0, sent_bytes_per_sec: 0 },
            { name: '本地连接* 8', class: 'virtual', received_bytes_per_sec: 0, sent_bytes_per_sec: 0 },
          ],
          loopback_excluded: 1,
          note: 'Physical interfaces only; loopback is excluded and tunnel or virtual adapters are listed separately, not added',
        },
        health: {
          score: 78,
          weights: { cpu: 30, memory: 25, disk: 20, thermal: 15, io: 10 },
          lowered_by: ['disk'],
          dimensions: [
            { dimension: 'cpu', metric: 'cpu.total_percent', value: cpu, full_score_at: 60, zero_score_at: 95, score: cpu <= 60 ? 100 : Math.max(0, Math.round(100 - ((cpu - 60) / 35) * 100)) },
            { dimension: 'memory', metric: 'memory.used_percent', value: 71.4, full_score_at: 75, zero_score_at: 95, score: 100 },
            { dimension: 'disk', metric: 'disk.system.free_percent', value: 11.8, full_score_at: 20, zero_score_at: 5, score: 45 },
            { dimension: 'thermal', metric: 'thermal.max_zone_celsius', value: 58, full_score_at: 70, zero_score_at: 95, score: 100 },
            { dimension: 'io', metric: 'disk.busy_percent', value: 6.5, full_score_at: 50, zero_score_at: 95, score: 100 },
          ],
        },
        diagnosis: { code: 'low_free_space', message: 'System volume free space is low', link: 'analyze' },
      };
    },
    async readiness(operation: ReadinessOp) {
      await delay(120);
      const writes = operation !== 'scan';
      return {
        platform: 'windows',
        operation,
        is_admin: false,
        checks: [
          { check_id: 'store', state: 'ready', blocking: true, detail: null },
          { check_id: 'protection_list', state: 'ready', blocking: writes, detail: null },
          { check_id: 'administrator', state: 'missing', blocking: false, detail: 'process token is not elevated' },
          { check_id: 'bypass_privileges', state: 'ready', blocking: false, detail: null },
          ...(writes && operation !== 'startup' ? [{ check_id: 'recycle_bin', state: 'ready' as const, blocking: false, detail: null }] : []),
        ],
      };
    },
    async revealEntry() {
      await delay(60);
      return true;
    },
  };
}
