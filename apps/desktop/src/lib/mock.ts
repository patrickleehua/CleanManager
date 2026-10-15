// DEVELOPMENT-ONLY fixture backend for previewing the UI in a plain browser (`npm run dev`).
// It is imported behind `import.meta.env.DEV` and is not part of production builds. The UI
// shows a "preview data" banner whenever it is active. Semantics mirror the host: published
// revisions only, size-desc with unknown last and entry_id tie-break, bounded pages.

import type { Backend } from './api';
import { mockOps } from './mockOps';
import type {
  Entry,
  EntriesPage,
  EntriesQuery,
  FinishedEvent,
  IpcError,
  ProgressEvent,
  RootOffer,
  ScanStatus,
  Summary,
  TreemapLayer,
} from './types';

interface Node {
  id: number;
  parent: number | null;
  name: string;
  kind: Entry['kind'];
  logical: bigint | null;
  allocated: bigint | null;
  children: number[];
  unknownDesc: number;
  boundary?: Entry['boundary'];
  skipped?: number;
}

const KiB = 1024n;
const MiB = KiB * 1024n;
const GiB = MiB * 1024n;

function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

function buildTree(rootName: string): Map<number, Node> {
  const nodes = new Map<number, Node>();
  let next = 0;
  const add = (parent: number | null, name: string, kind: Entry['kind'], logical: bigint | null = null, extra: Partial<Node> = {}) => {
    next += 1;
    const n: Node = { id: next, parent, name, kind, logical, allocated: logical, children: [], unknownDesc: 0, ...extra };
    nodes.set(n.id, n);
    if (parent !== null) nodes.get(parent)!.children.push(n.id);
    return n.id;
  };
  const r = rng(7);
  const root = add(null, rootName, 'directory');
  const videos = add(root, 'Videos', 'directory');
  add(videos, '2026-家庭旅行.mov', 'file', 18n * GiB + 412n * MiB);
  add(videos, '发布会录屏.mp4', 'file', 6n * GiB + 90n * MiB);
  add(videos, 'drone_raw_0412.mov', 'file', 9n * GiB + 700n * MiB);
  add(videos, 'clip-short.mp4', 'file', 210n * MiB);
  const projects = add(root, 'Projects', 'directory');
  const app = add(projects, 'storefront', 'directory');
  const nm = add(app, 'node_modules', 'directory');
  for (let i = 0; i < 1400; i++) add(nm, `pkg-${String(i).padStart(4, '0')}.tgz`, 'file', BigInt(Math.floor(r() * 9_000_000) + 2_000));
  add(app, 'target', 'directory');
  add(app, 'README.md', 'file', 8_412n);
  add(app, 'design-assets.psd', 'file', 1n * GiB + 220n * MiB);
  const ml = add(projects, 'models', 'directory');
  add(ml, 'checkpoint-final.safetensors', 'file', 13n * GiB + 5n * MiB);
  add(ml, 'tokenizer.json', 'file', 2n * MiB + 337n * KiB);
  const archives = add(root, 'Archives', 'directory');
  add(archives, 'backup-2025.zip', 'file', 7n * GiB + 512n * MiB);
  add(archives, 'photos-2019.7z', 'file', 3n * GiB + 80n * MiB);
  add(archives, 'invoices.tar.gz', 'file', 96n * MiB);
  const docs = add(root, 'Documents', 'directory');
  for (let i = 0; i < 60; i++) add(docs, `报告-${2020 + (i % 6)}-${i + 1}.docx`, 'file', BigInt(Math.floor(r() * 40_000_000) + 20_000));
  add(docs, '合同扫描件.pdf', 'file', 412n * MiB);
  add(root, 'Locked', 'directory', null, { skipped: 1 });
  add(root, 'cloud-drive', 'directory', 0n, { boundary: 'cloud_provider' });
  add(root, 'sparse.vhdx', 'file', null);
  add(root, 'empty.log', 'file', 0n);
  add(root, 'notes.txt', 'file', 3_201n);
  // Aggregate (logical = known subtree; unknown counted).
  const agg = (id: number): { known: bigint; unknown: number } => {
    const n = nodes.get(id)!;
    if (n.kind !== 'directory') return { known: n.logical ?? 0n, unknown: n.logical === null ? 1 : 0 };
    let known = 0n;
    let unknown = n.skipped ?? 0;
    for (const c of n.children) {
      const a = agg(c);
      known += a.known;
      unknown += a.unknown;
    }
    n.unknownDesc = unknown;
    if (n.boundary) return { known: 0n, unknown: 0 };
    n.logical = known === 0n && unknown > 0 ? null : known;
    n.allocated = n.logical === null ? null : (n.logical * 97n) / 100n;
    return { known, unknown };
  };
  agg(root);
  return nodes;
}

const ROOT = 'D:\\Work';

function pathOf(nodes: Map<number, Node>, id: number): string {
  const n = nodes.get(id)!;
  if (n.parent === null) return n.name;
  return `${pathOf(nodes, n.parent)}\\${n.name}`;
}

function toEntry(nodes: Map<number, Node>, n: Node): Entry {
  return {
    entry_id: String(n.id),
    parent_entry_id: n.parent === null ? null : String(n.parent),
    name: n.name,
    path_display: pathOf(nodes, n.id),
    kind: n.kind,
    logical_bytes: n.logical === null ? null : n.logical.toString(),
    allocated_bytes: n.allocated === null ? null : n.allocated.toString(),
    size_basis: n.logical === null ? 'unknown' : n.kind === 'directory' ? 'subtree_known' : 'entity',
    unknown_descendants: n.kind === 'directory' ? n.unknownDesc : null,
    boundary: n.boundary ?? null,
    identity_quality: 'strong',
    link_count: null,
  };
}

const err = (code: string, reason: string, message: string): IpcError => ({ code, reason, message });
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function mockBackend(): Backend {
  const nodes = buildTree(ROOT);
  const progress = new Set<(e: ProgressEvent) => void>();
  const finished = new Set<(e: FinishedEvent) => void>();
  const scans = new Map<string, { state: ScanStatus['state']; seen: number; cancel: boolean; started: string; finished: string | null; root: string }>();
  scans.set('s_preview_1', { state: 'partial', seen: nodes.size, cancel: false, started: '2026-10-15T01:12:00Z', finished: '2026-10-15T01:12:41Z', root: ROOT });
  let counter = 1;
  let offerCounter = 0;

  const summaryOf = (ids: number[]): Summary => {
    let known = 0n;
    let unknown = 0;
    let unread = 0;
    let covered = 0;
    const walk = (id: number) => {
      const n = nodes.get(id)!;
      covered += 1;
      unread += n.skipped ?? 0;
      if (n.kind !== 'directory') {
        if (n.logical === null) unknown += 1;
        else known += n.logical;
      }
      n.children.forEach(walk);
    };
    ids.forEach(walk);
    unknown += unread;
    return {
      logical_known_bytes: known.toString(),
      logical_unknown_count: unknown,
      allocated_exact_bytes: unknown === 0 ? ((known * 97n) / 100n).toString() : null,
      allocated_known_bytes: ((known * 97n) / 100n).toString(),
      allocated_unknown_count: unknown,
      covered_entries: covered,
      overlapping_selected: 0,
      unread_ranges: unread,
    };
  };

  const status = (id: string): ScanStatus => {
    const s = scans.get(id);
    if (!s) throw err('invalid_argument', 'not_found', `not found: ${id}`);
    const published = !['created', 'running', 'stopping'].includes(s.state);
    return {
      scan_id: id,
      scope_id: 'scope_preview',
      state: s.state,
      revision: 1,
      published,
      completeness: published ? (s.state === 'completed' ? 'verified_scope' : 'partial_scope') : null,
      cancel_requested: s.cancel,
      host_kind: 'desktop',
      host_pid: 4242,
      hosted_here: !published,
      heartbeat_at: new Date().toISOString(),
      entries_seen: s.seen,
      skipped_ranges: published ? 1 : 0,
      started_at: s.started,
      finished_at: s.finished,
      root_display: s.root,
      root_entry_id: published ? '1' : null,
      summary: published ? summaryOf([1]) : null,
      backend: 'win.directory',
      backend_reason: 'Directory enumeration with handle-relative opens',
      elevated_read: [],
      issues: published ? [{ code: 'permission_required', scope_display: `${ROOT}\\Locked`, message: 'Access is denied. (os error 5)' }] : [],
      issues_total: published ? 1 : 0,
    };
  };

  const list = (q: EntriesQuery): EntriesPage => {
    const st = scans.get(q.scan_id);
    if (!st) throw err('invalid_argument', 'not_found', 'not found');
    if (['created', 'running', 'stopping'].includes(st.state)) throw err('internal_error', 'not_published', 'scan revision is still staging');
    if (q.page_size < 1 || q.page_size > 500) throw err('invalid_argument', 'invalid_argument', 'page_size must be 1..=500');
    let items = [...nodes.values()].filter((n) => (q.parent_entry_id === null ? true : n.parent === Number(q.parent_entry_id)));
    if (q.kind !== 'any') items = items.filter((n) => n.kind === q.kind);
    if (q.name_contains) items = items.filter((n) => n.name.includes(q.name_contains!));
    const key = (n: Node) => (q.sort === 'allocated_desc' ? n.allocated : n.logical);
    items.sort((a, b) => {
      if (q.sort === 'name_asc') return a.name < b.name ? -1 : a.name > b.name ? 1 : a.id - b.id;
      const ka = key(a);
      const kb = key(b);
      if (ka === null && kb === null) return a.id - b.id;
      if (ka === null) return 1;
      if (kb === null) return -1;
      return ka === kb ? a.id - b.id : ka > kb ? -1 : 1;
    });
    const offset = q.cursor ? Number(q.cursor.slice(1)) : 0;
    const page = items.slice(offset, offset + q.page_size);
    const nextOffset = offset + q.page_size;
    return {
      scan_id: q.scan_id,
      scope_id: 'scope_preview',
      revision: 1,
      sort: q.sort,
      kind: q.kind,
      name_contains: q.name_contains,
      parent_entry_id: q.parent_entry_id,
      page_size: q.page_size,
      items: page.map((n) => toEntry(nodes, n)),
      next_cursor: nextOffset < items.length ? `o${nextOffset}` : null,
      unknown_size_items: page.filter((n) => n.logical === null).length,
    };
  };

  const offers: RootOffer[] = [
    { token: 'r1', label: '下载', kind: 'user_folder', path_display: 'C:\\Users\\Lin\\Downloads', problem: null },
    { token: 'r2', label: '文档', kind: 'user_folder', path_display: 'C:\\Users\\Lin\\Documents', problem: null },
    { token: 'r3', label: '用户目录', kind: 'user_home', path_display: 'C:\\Users\\Lin', problem: null },
    { token: 'r4', label: 'D: 卷', kind: 'volume', path_display: 'D:\\', problem: null },
    {
      token: 'r5',
      label: 'E: 卷',
      kind: 'volume',
      path_display: 'E:\\',
      problem: err('invalid_argument', 'root_missing', 'This folder does not exist or its volume is offline'),
    },
  ];

  return {
    ...mockOps(),
    preview: true,
    async capabilities() {
      return {
        platform: 'windows',
        engine_version: '0.1.0',
        schema_version: '1.0',
        capabilities: [
          { name: 'scan.win.directory', state: 'unverified', reason: 'Not verified on a real device in this build' },
          { name: 'execute.trash', state: 'unverified', reason: 'Not verified on a real device in this build' },
        ],
        store_dir_display: 'C:\\Users\\Lin\\AppData\\Local\\CleanManager\\store',
        store_problem: null,
        bypass_privileges_disabled: true,
        bypass_privileges_note: null,
        interrupted_at_startup: [],
        operations_to_reconcile: [],
        write_commands: true,
        file_actions_verified: true,
        rules_dir_display: 'C:\\Program Files\\CleanManager\\rules',
        rules_problem: null,
        rule_count: 3,
        principal: 'user:DESKTOP\\Lin',
      };
    },
    async listScanRoots() {
      await delay(250);
      return offers;
    },
    async chooseFolder() {
      offerCounter += 1;
      const o: RootOffer = { token: `rc${offerCounter}`, label: 'Work', kind: 'chosen', path_display: ROOT, problem: null };
      offers.push(o);
      return o;
    },
    async recentScans() {
      return [...scans.entries()].map(([id, s]) => ({
        scan_id: id,
        root_display: s.root,
        started_at: s.started,
        state: s.state,
        published: !['created', 'running', 'stopping'].includes(s.state),
        host_kind: 'desktop',
        completeness: s.state === 'completed' ? 'verified_scope' : s.state === 'partial' ? 'partial_scope' : null,
        finished_at: s.finished,
        problem: null,
      }));
    },
    async startScan(token) {
      const o = offers.find((x) => x.token === token);
      if (!o) throw err('invalid_argument', 'unknown_root', 'unknown root');
      if (o.problem) throw o.problem;
      if ([...scans.values()].some((s) => ['created', 'running', 'stopping'].includes(s.state))) throw err('conflict', 'scan_running', 'A scan is still running');
      counter += 1;
      const id = `s_preview_${counter}`;
      const s = { state: 'running' as ScanStatus['state'], seen: 0, cancel: false, started: new Date().toISOString(), finished: null as string | null, root: ROOT };
      scans.set(id, s);
      const tick = setInterval(() => {
        if (s.cancel && s.state === 'running') s.state = 'stopping';
        else if (s.state === 'stopping') {
          s.state = 'cancelled';
        } else if (s.seen >= nodes.size) s.state = 'partial';
        else s.seen = Math.min(nodes.size, s.seen + 137);
        if (['cancelled', 'partial'].includes(s.state)) {
          s.finished = new Date().toISOString();
          clearInterval(tick);
          finished.forEach((cb) => cb({ scan_id: id, state: s.state, error: null }));
          return;
        }
        progress.forEach((cb) => cb({ scan_id: id, state: s.state, entries_seen: s.seen, skipped_ranges: 0, cancel_requested: s.cancel, heartbeat_at: new Date().toISOString() }));
      }, 450);
      return { scan_id: id, root_display: ROOT };
    },
    async scanStatus(id) {
      await delay(60);
      return status(id);
    },
    async cancelScan(id) {
      const s = scans.get(id);
      if (!s) throw err('invalid_argument', 'not_found', 'not found');
      const terminal = !['created', 'running', 'stopping'].includes(s.state);
      if (!terminal) s.cancel = true;
      return { scan_id: id, accepted: !terminal, state_at_receipt: terminal ? 'already_terminal' : s.state };
    },
    async listEntries(q) {
      await delay(80 + Math.random() * 160);
      return list(q);
    },
    async treemapLayer(scanId, parent, limit): Promise<TreemapLayer> {
      await delay(120 + Math.random() * 200);
      const all: Entry[] = [];
      let cursor: string | null = null;
      do {
        const p = list({ scan_id: scanId, sort: 'logical_desc', kind: 'any', name_contains: null, parent_entry_id: parent ?? '1', page_size: 500, cursor });
        all.push(...p.items);
        cursor = p.next_cursor;
      } while (cursor);
      const rest = all.slice(limit);
      return {
        scan_id: scanId,
        revision: 1,
        parent_entry_id: parent ?? '1',
        parent_summary: summaryOf([Number(parent ?? '1')]),
        items: all.slice(0, limit),
        rest: {
          count: rest.length,
          known_logical_bytes: rest.reduce((a, e) => a + (e.logical_bytes ? BigInt(e.logical_bytes) : 0n), 0n).toString(),
          unknown_count: rest.filter((e) => e.logical_bytes === null).length,
          complete: true,
        },
      };
    },
    async summarizeSelection(scanId, ids) {
      if (!scans.has(scanId)) throw err('invalid_argument', 'not_found', 'not found');
      return summaryOf(ids.map(Number));
    },
    async onProgress(cb) {
      progress.add(cb);
      return () => progress.delete(cb);
    },
    async onFinished(cb) {
      finished.add(cb);
      return () => finished.delete(cb);
    },
    async onClosing() {
      return () => {};
    },
  };
}
