// DEVELOPMENT-ONLY fixture backend for previewing the UI in a plain browser (`npm run dev`).
// It is imported behind `import.meta.env.DEV` and is not part of production builds. The UI
// shows a "preview data" banner whenever it is active. Semantics mirror the host: published
// revisions only, size-desc with unknown last and entry_id tie-break, bounded pages.

import { mockResident } from './mockResident';
import { mockMac } from './mockMac';
import { previewPlatform } from './platform';
import type { Backend } from './api';
import { mockDiag } from './mockDiag';
import { mockOps, PREVIEW_REMOVALS } from './mockOps';
import { mockSoftware } from './mockSoftware';
import { mockSuite } from './mockSuite';
import type {
  CleanableHint,
  Entry,
  EntriesPage,
  EntriesQuery,
  FinishedEvent,
  IpcError,
  Issue,
  ProgressEvent,
  RemovedFact,
  RemovedTotal,
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
  /** DIN-05 preview: the hint the host would attach after verifying the signature. */
  hint?: CleanableHint;
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

function buildTree(rootName: string, gaps = true): Map<number, Node> {
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
  add(app, 'package.json', 'file', 2_310n);
  const nm = add(app, 'node_modules', 'directory', null, { hint: { basis: 'build_artifact', rule_id: 'purge.node-modules', detail: 'package.json', flow: 'purge' } });
  for (let i = 0; i < 1400; i++) add(nm, `pkg-${String(i).padStart(4, '0')}.tgz`, 'file', BigInt(Math.floor(r() * 9_000_000) + 2_000));
  add(app, 'target', 'directory', null, { hint: { basis: 'cachedir_tag', rule_id: null, detail: 'CACHEDIR.TAG', flow: 'manual_trash' } });
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
  // The completed preview (`?scan=completed`) has no unread range and no unknown size.
  if (gaps) {
    add(root, 'Locked', 'directory', null, { skipped: 1 });
    add(root, 'cloud-drive', 'directory', 0n, { boundary: 'cloud_provider' });
    add(root, 'sparse.vhdx', 'file', null);
  }
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

/** Preview paths follow `?platform=` (Windows paths in the Windows preview, macOS paths in the macOS one). */
const pick = (win: string, mac: string): string => (previewPlatform() === 'macos' ? mac : win);
const defaultRoot = (): string => pick('D:\\Work', '/Users/lin/Work');

function pathOf(nodes: Map<number, Node>, id: number): string {
  const n = nodes.get(id)!;
  if (n.parent === null) return n.name;
  const parent = pathOf(nodes, n.parent);
  return parent.startsWith('/') ? `${parent}/${n.name}` : `${parent}\\${n.name}`;
}

/**
 * Overview preview states (`?scan=…`, browser preview only; docs/visual-identity.md lists them). The scan
 * opened at start is the first one; `s_preview_0` is an older completed scan for the "other scans" list.
 */
export type ScanPreview =
  | 'partial'
  | 'completed'
  | 'partial-pending'
  | 'failed-file'
  | 'failed-link'
  | 'failed-missing'
  | 'failed-unknown'
  | 'cancelled'
  | 'interrupted'
  | 'running'
  | 'none';

export const SCAN_PREVIEWS: ScanPreview[] = ['partial', 'completed', 'partial-pending', 'failed-file', 'failed-link', 'failed-missing', 'failed-unknown', 'cancelled', 'interrupted', 'running', 'none'];

function scanPreview(): ScanPreview {
  const v = new URLSearchParams(globalThis.location?.search ?? '').get('scan') as ScanPreview | null;
  return v && SCAN_PREVIEWS.includes(v) ? v : 'partial';
}

const previewRoot = (p: ScanPreview): string | undefined =>
  (
    {
      'partial-pending': '/Users/lin',
      'failed-file': pick('C:\\Users\\Lin\\Downloads\\季度报告-2026-Q3-最终版.pdf', '/Users/lin/Downloads/季度报告-2026-Q3-最终版.pdf'),
      'failed-link': pick('C:\\Users\\Lin\\Projects-link', '/Users/lin/Projects-link'),
      'failed-missing': pick('E:\\Backup\\Photos', '/Volumes/Backup/Photos'),
    } as Partial<Record<ScanPreview, string>>
  )[p];

interface MockScan {
  state: ScanStatus['state'];
  seen: number;
  cancel: boolean;
  started: string;
  finished: string | null;
  root: string;
  /** Overrides of what the host would report for this preview state. */
  issues?: Issue[];
  published?: boolean;
  summary?: Summary | null;
  skipped?: number;
  hosted?: boolean;
}

function childNamed(nodes: Map<number, Node>, name: string): number | null {
  for (const n of nodes.values()) if (n.parent === 1 && n.name === name) return n.id;
  return null;
}

/** The scans of one preview state: the state itself first, then an older completed scan. */
function seedScans(preview: ScanPreview, root: string, nodes: Map<number, Node>, scans: Map<string, MockScan>, summaryOf: (ids: number[]) => Summary): void {
  if (preview === 'none') return;
  const done = { cancel: false, started: '2026-10-15T01:12:00Z', finished: '2026-10-15T01:12:41Z', root };
  const failed = (code: string, message: string): MockScan => ({ ...done, state: 'failed', seen: 0, published: true, summary: null, skipped: 0, finished: '2026-10-15T01:12:00Z', issues: [{ code, scope_display: root, message }] });
  const kept = [childNamed(nodes, 'Videos'), childNamed(nodes, 'Archives')].filter((x): x is number => x !== null);
  const keptSummary = () => ({ ...summaryOf(kept), unread_ranges: 0 });
  const first: Record<Exclude<ScanPreview, 'none'>, () => MockScan> = {
    partial: () => ({ ...done, state: 'partial', seen: nodes.size }),
    completed: () => ({ ...done, state: 'completed', seen: nodes.size, issues: [], skipped: 0 }),
    'partial-pending': () => {
      const base = summaryOf([1]);
      const pending = (dir: string): Issue => ({ code: 'permission_required', scope_display: `${root}/${dir}`, message: 'permission_required (permission_cause=privacy_prompt_pending)', permission_cause: 'privacy_prompt_pending' });
      return {
        ...done,
        state: 'partial',
        seen: nodes.size,
        skipped: 3,
        summary: { ...base, unread_ranges: 3, logical_unknown_count: base.logical_unknown_count + 2 },
        issues: [pending('Documents'), pending('Desktop'), { code: 'permission_required', scope_display: `${root}/Library/Mail`, message: 'Operation not permitted (os error 1)', permission_cause: 'undetermined' }],
      };
    },
    'failed-file': () => failed('not_a_directory', 'root is not a directory'),
    'failed-link': () => failed('root_is_link', 'This location is a link (symbolic link or junction); CleanManager never follows links'),
    'failed-missing': () => failed('root_unavailable', pick('The system cannot find the path specified. (os error 3)', 'No such file or directory (os error 2)')),
    'failed-unknown': () => ({ ...failed('index_quota_exhausted', 'index quota exhausted while writing rows'), root: defaultRoot(), issues: [{ code: 'index_quota_exhausted', scope_display: defaultRoot(), message: 'index quota exhausted while writing rows' }] }),
    cancelled: () => ({ ...done, state: 'cancelled', seen: 812, summary: keptSummary(), issues: [], skipped: 0, finished: '2026-10-15T01:12:19Z' }),
    interrupted: () => ({ ...done, state: 'interrupted', seen: 812, summary: keptSummary(), issues: [], skipped: 0, finished: '2026-10-15T01:12:19Z' }),
    running: () => ({ ...done, state: 'running', seen: 4318, published: false, hosted: true, started: new Date(Date.now() - 37_000).toISOString(), finished: null }),
  };
  scans.set('s_preview_1', first[preview]());
  const docs = childNamed(nodes, 'Documents');
  const docsRoot = root.startsWith('/') ? '/Users/lin/Documents' : 'C:\\Users\\Lin\\Documents';
  scans.set('s_preview_0', { state: 'completed', seen: 62, cancel: false, started: '2026-10-14T08:02:00Z', finished: '2026-10-14T08:02:09Z', root: docsRoot, issues: [], skipped: 0, summary: docs ? { ...summaryOf([docs]), unread_ranges: 0 } : null });
}

/** `?choose=file|link`: the folder picker returns a file or a link once, so the scope page shows its refusal. */
function chooseRefusal(): RootOffer | null {
  const v = new URLSearchParams(globalThis.location?.search ?? '').get('choose');
  if (v === 'file')
    return { token: 'rc_file', label: '季度报告.pdf', kind: 'chosen', path_display: pick('C:\\Users\\Lin\\Downloads\\季度报告.pdf', '/Users/lin/Downloads/季度报告.pdf'), problem: err('invalid_argument', 'not_a_directory', 'This location is not a folder') };
  if (v === 'link')
    return {
      token: 'rc_link',
      label: 'Projects-link',
      kind: 'chosen',
      path_display: pick('C:\\Users\\Lin\\Projects-link', '/Users/lin/Projects-link'),
      problem: err('invalid_argument', 'root_is_link', 'This location is a link (symbolic link or junction); CleanManager never follows links. Choose the folder it points to instead'),
    };
  return null;
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
    cleanable_hint: n.kind === 'directory' && !n.boundary ? (n.hint ?? null) : null,
  };
}

const err = (code: string, reason: string, message: string): IpcError => ({ code, reason, message });

/** The host's gate table (cm_core::capability::GATED); no gate has passed. */
const MOCK_GATES: [string, string][] = [
  ['clean.catalog', 'G1'],
  ['purge.artifacts', 'G1'],
  ['installer.cleanup', 'G1'],
  ['trash.manual', 'G1'],
  ['insights.overview', 'G1'],
  ['apps.uninstall', 'G3'],
  ['apps.updates', 'G3'],
  ['startup.toggle', 'G3'],
  ['maintenance.run', 'G3'],
  ['status.metrics', 'G3'],
  ['process.end', 'G3'],
  ['resident.utilities', 'G4'],
  ['lifecycle.self_update', 'G3'],
  ['diag.ai', 'AID'],
];
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** `?index=corrupt|incompatible`: the scan index cannot be used (NEW-04); history and recovery still work. */
function indexPreview(): 'index_corrupt' | 'index_incompatible' | null {
  const v = new URLSearchParams(globalThis.location?.search ?? '').get('index');
  return v === 'corrupt' ? 'index_corrupt' : v === 'incompatible' ? 'index_incompatible' : null;
}

export function mockBackend(): Backend {
  const preview = scanPreview();
  const DEFAULT_ROOT = defaultRoot();
  const ROOT = previewRoot(preview) ?? DEFAULT_ROOT;
  // A failed root has no tree; its "D:\Work" tree only backs the older scan in the list.
  const nodes = buildTree(preview.startsWith('failed') ? DEFAULT_ROOT : ROOT, preview !== 'completed');
  const progress = new Set<(e: ProgressEvent) => void>();
  const finished = new Set<(e: FinishedEvent) => void>();
  const scans = new Map<string, MockScan>();
  let counter = 1;
  let offerCounter = 0;
  // D15: gated tools are off until 设置 › 实验功能 is turned on (the preview starts where a new install starts).
  let experimental = false;
  const gateRows = () =>
    MOCK_GATES.map(([capability, gate]) => ({ capability, gate, gate_passed: false, enabled: experimental }));

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

  // A damaged index has no readable scans until it is rebuilt (then it is empty: scans must be run again).
  let indexBroken = indexPreview();
  const indexError = () =>
    err(
      'index_unavailable',
      indexBroken ?? 'index_corrupt',
      indexBroken === 'index_incompatible'
        ? 'scan index schema 9 is newer than this program supports (4); run `cleanmanager doctor --repair-index` (the file is kept) and scan again'
        : 'scan index is damaged (database disk image is malformed); run `cleanmanager doctor --repair-index` (the damaged file is kept) and scan again',
    );
  const needIndex = () => {
    if (indexBroken) throw indexError();
  };
  if (!indexBroken) seedScans(preview, ROOT, nodes, scans, summaryOf);
  if (preview === 'running') {
    // The running preview keeps going until it is cancelled.
    const s = scans.get('s_preview_1')!;
    const tick = setInterval(() => {
      if (s.cancel && s.state === 'running') s.state = 'stopping';
      else if (s.state === 'stopping') {
        s.state = 'cancelled';
        s.published = true;
        s.hosted = false;
        s.finished = new Date().toISOString();
        s.summary = summaryOf([1]);
        s.issues = [];
        s.skipped = 0;
        clearInterval(tick);
        finished.forEach((cb) => cb({ scan_id: 's_preview_1', state: 'cancelled', error: null }));
        return;
      } else s.seen = Math.min(nodes.size * 40, s.seen + 37);
      progress.forEach((cb) => cb({ scan_id: 's_preview_1', state: s.state, entries_seen: s.seen, skipped_ranges: 0, cancel_requested: s.cancel, heartbeat_at: new Date().toISOString() }));
    }, 450);
  }

  const status = (id: string): ScanStatus => {
    const s = scans.get(id);
    if (!s) throw err('invalid_argument', 'not_found', `not found: ${id}`);
    const terminal = !['created', 'running', 'stopping'].includes(s.state);
    const published = s.published ?? terminal;
    const issues = s.issues ?? (published ? [{ code: 'permission_required', scope_display: `${s.root}${s.root.startsWith('/') ? '/' : '\\'}Locked`, message: 'Access is denied. (os error 5)' }] : []);
    const mac = s.root.startsWith('/');
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
      hosted_here: s.hosted ?? !published,
      heartbeat_at: new Date().toISOString(),
      entries_seen: s.seen,
      skipped_ranges: s.skipped ?? (published ? 1 : 0),
      started_at: s.started,
      finished_at: s.finished,
      root_display: s.root,
      root_entry_id: published && s.seen > 0 ? '1' : null,
      // A failed scan reports no sizes (unknown, never 0), like the host.
      summary: s.summary !== undefined ? s.summary : published && s.state !== 'failed' ? summaryOf([1]) : null,
      backend: mac ? 'macos.posix' : 'win.directory',
      backend_reason: mac ? 'openat/fstatat walk without following links' : 'Directory enumeration with handle-relative opens',
      elevated_read: [],
      issues,
      issues_total: issues.length,
      removed_since_scan: published && terminal && s.state !== 'failed' && s.seen > 0 ? removedTotal(id, null) : null,
    };
  };

  // "Removed since scan" (PREVIEW_REMOVALS in mockOps): the preview disk scan's entries that the fixture operations
  // removed after it finished, by node id; computed once. Only finished scans with results carry an overlay.
  let removedNodes: Map<number, RemovedFact> | null = null;
  const nodeAt = (below: string[]): number | null => {
    let cur = 1;
    for (const name of below) {
      const next = nodes.get(cur)?.children.find((c) => nodes.get(c)!.name === name);
      if (next === undefined) return null;
      cur = next;
    }
    return cur;
  };
  const removals = (scanId: string): Map<number, RemovedFact> => {
    const st = scans.get(scanId);
    if (scanId !== 's_preview_1' || !st || ['created', 'running', 'stopping', 'failed'].includes(st.state)) return new Map();
    if (removedNodes) return removedNodes;
    const found = PREVIEW_REMOVALS.map((r) => ({ r, id: nodeAt(r.below) })).filter((x): x is { r: (typeof PREVIEW_REMOVALS)[number]; id: number } => x.id !== null);
    const facts = ops.__removals(
      found.map(({ r, id }) => {
        const n = nodes.get(id)!;
        return { mode: r.mode, minutesAgo: r.minutesAgo, name: n.name, path_display: pathOf(nodes, id), logical_bytes: n.logical === null ? null : n.logical.toString(), directory: n.kind === 'directory' };
      }),
    );
    removedNodes = new Map(found.map(({ id }, i) => [id, facts[i]]));
    return removedNodes;
  };
  const above = (id: number): number[] => {
    const out: number[] = [];
    for (let p = nodes.get(id)!.parent; p !== null; p = nodes.get(p)!.parent) out.push(p);
    return out;
  };
  /** Top-most removed entries (beneath `under`, or the whole scan), counted like the host: stored sizes, unknown kept. */
  const removedTotal = (scanId: string, under: number | null): RemovedTotal => {
    const marks = removals(scanId);
    let count = 0;
    let known = 0n;
    let unknown = 0;
    for (const id of marks.keys()) {
      const ups = above(id);
      if (ups.some((a) => marks.has(a))) continue;
      if (under !== null && !ups.includes(under)) continue;
      const n = nodes.get(id)!;
      count += 1;
      if (n.logical === null) unknown += 1;
      else {
        known += n.logical;
        if (n.unknownDesc > 0) unknown += 1;
      }
    }
    const latest = [...marks.values()].map((m) => m.at).sort().pop() ?? null;
    return { count, known_bytes: count && (known > 0n || unknown < count) ? known.toString() : null, unknown_count: unknown, latest_at: latest, signature: `preview.${marks.size}` };
  };
  const withRemoved = (scanId: string, n: Node, e: Entry): Entry => {
    const marks = removals(scanId);
    if (!marks.size) return { ...e, removed: null, removed_count: 0, removed_bytes: null, removed_unknown_count: 0 };
    const own = marks.get(n.id);
    const up = own ? undefined : above(n.id).find((a) => marks.has(a));
    const fact = own ? { ...own, inherited: false } : up !== undefined ? { ...marks.get(up)!, inherited: true } : null;
    const beneath = n.kind === 'directory' && !fact ? removedTotal(scanId, n.id) : null;
    return { ...e, removed: fact, removed_count: beneath?.count ?? 0, removed_bytes: beneath?.known_bytes ?? null, removed_unknown_count: beneath?.unknown_count ?? 0 };
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
      items: page.map((n) => withRemoved(q.scan_id, n, toEntry(nodes, n))),
      next_cursor: nextOffset < items.length ? `o${nextOffset}` : null,
      unknown_size_items: page.filter((n) => n.logical === null).length,
    };
  };

  const offers: RootOffer[] = [
    { token: 'r1', label: '下载', kind: 'user_folder', path_display: pick('C:\\Users\\Lin\\Downloads', '/Users/lin/Downloads'), problem: null },
    { token: 'r2', label: '文档', kind: 'user_folder', path_display: pick('C:\\Users\\Lin\\Documents', '/Users/lin/Documents'), problem: null },
    { token: 'r3', label: '用户目录', kind: 'user_home', path_display: pick('C:\\Users\\Lin', '/Users/lin'), problem: null },
    { token: 'r4', label: pick('D: 卷', 'Macintosh HD'), kind: 'volume', path_display: pick('D:\\', '/'), problem: null },
    {
      token: 'r5',
      label: pick('E: 卷', 'Backup'),
      kind: 'volume',
      path_display: pick('E:\\', '/Volumes/Backup'),
      problem: err('invalid_argument', 'root_missing', 'This folder does not exist or its volume is offline'),
    },
  ];

  const diag = mockDiag((entryId) => {
    const n = nodes.get(Number(entryId));
    if (!n) return null;
    return { name: n.name, path: pathOf(nodes, n.id), kind: n.kind, logical: n.logical === null ? null : n.logical.toString(), unknownDesc: n.unknownDesc };
  });
  const ops = mockOps((entryId) => (nodes.has(Number(entryId)) ? pathOf(nodes, Number(entryId)) : null));
  const suite = mockSuite(ops.__plan, (entryId) => {
    const n = nodes.get(Number(entryId));
    if (!n) return null;
    return { name: n.name, path: pathOf(nodes, n.id), kind: n.kind, logical: n.logical === null ? null : n.logical.toString(), unknownDesc: n.unknownDesc };
  });
  const soft = mockSoftware(ops.__plan, ops.__seed);
  // `?platform=macos` (or a Mac browser): the macOS host's answers where they differ, so the preview shows macOS words.
  const mac = previewPlatform() === 'macos';
  const resident = mockResident(mac);
  const macOverrides = mac ? mockMac(ops.__plan) : {};
  const suiteStatus = macOverrides.statusSnapshot ?? suite.statusSnapshot;
  let alerted = false;
  return {
    ...ops,
    ...suite,
    ...soft,
    ...diag,
    ...resident,
    ...macOverrides,
    preview: true,
    async statusSnapshot() {
      const snap = await suiteStatus();
      // STS-05 preview: with alerts on, one sustained-CPU alert is raised once (then the cooldown holds).
      if (soft.__alertsOn() && !alerted) {
        alerted = true;
        const at = new Date().toISOString().replace(/\.\d+Z$/, 'Z');
        snap.alerts = [
          mac
            ? { pid: 18872, name: 'node', image_path: '/opt/homebrew/bin/node', min_cpu_percent: 86.5, threshold_percent: 80, window_seconds: 120, since: at, raised_at: at, link: 'status.processes?pid=18872' }
            : { pid: 18872, name: 'node.exe', image_path: 'C:\\Program Files\\nodejs\\node.exe', min_cpu_percent: 86.5, threshold_percent: 80, window_seconds: 120, since: at, raised_at: at, link: 'status.processes?pid=18872' },
        ];
      }
      return snap;
    },
    async capabilities() {
      return {
        platform: mac ? 'macos' : 'windows',
        engine_version: '0.1.0',
        schema_version: '1.0',
        capabilities: [
          { name: 'scan.win.directory', state: 'unverified', reason: 'Not verified on a real device in this build' },
          { name: 'execute.trash', state: 'unverified', reason: 'Not verified on a real device in this build' },
        ],
        store_dir_display: mac ? '/Users/lin/Library/Application Support/CleanManager/store' : 'C:\\Users\\Lin\\AppData\\Local\\CleanManager\\store',
        store_problem: indexBroken ? indexError() : null,
        bypass_privileges_disabled: true,
        bypass_privileges_note: null,
        interrupted_at_startup: [],
        operations_to_reconcile: [],
        write_commands: true,
        file_actions_verified: true,
        rules_dir_display: mac ? '/Applications/CleanManager.app/Contents/Resources/rules' : 'C:\\Program Files\\CleanManager\\rules',
        rules_problem: null,
        rule_count: 3,
        principal: mac ? 'user:lin' : 'user:DESKTOP\\Lin',
        experimental_features: experimental,
        gates: gateRows(),
      };
    },
    async setExperimentalFeatures(enabled) {
      await delay(150);
      experimental = enabled;
      return this.capabilities();
    },
    async listScanRoots() {
      await delay(250);
      return [...offers];
    },
    async chooseFolder() {
      offerCounter += 1;
      // Like the host, a refused pick is answered with its reason and is not added to the offered roots.
      const refused = offerCounter === 1 ? chooseRefusal() : null;
      if (refused) return refused;
      const o: RootOffer = { token: `rc${offerCounter}`, label: 'Work', kind: 'chosen', path_display: DEFAULT_ROOT, problem: null };
      offers.push(o);
      return o;
    },
    async repairIndex(_confirmed: true) {
      await delay(700);
      if (!indexBroken) return { status: 'not_needed' as const, previous_state: 'healthy', kept_files: [], message: 'The scan index passed the full integrity check; nothing was changed.' };
      const previous = indexBroken === 'index_incompatible' ? 'incompatible' : 'corrupt';
      indexBroken = null;
      scans.clear();
      const store = mac ? '/Users/lin/Library/Application Support/CleanManager/store' : 'C:\\Users\\Lin\\AppData\\Local\\CleanManager\\store';
      const sep = mac ? '/' : '\\';
      const kept = ['', '-wal'].map((x) => `${store}${sep}index.db.damaged-20261023T061500Z${x}`);
      return { status: 'repaired' as const, previous_state: previous, kept_files: kept, message: `The scan index was unusable. It was moved aside as ${kept.join(', ')} and an empty index was created.` };
    },
    async recentScans() {
      needIndex();
      return [...scans.entries()].map(([id, s]) => ({
        scan_id: id,
        root_display: s.root,
        started_at: s.started,
        state: s.state,
        published: s.published ?? !['created', 'running', 'stopping'].includes(s.state),
        host_kind: 'desktop',
        completeness: s.state === 'completed' ? 'verified_scope' : s.state === 'partial' ? 'partial_scope' : null,
        finished_at: s.finished,
        problem: null,
      }));
    },
    async startScan(token) {
      needIndex();
      const o = offers.find((x) => x.token === token);
      if (!o) throw err('invalid_argument', 'unknown_root', 'unknown root');
      if (o.problem) throw o.problem;
      if ([...scans.values()].some((s) => ['created', 'running', 'stopping'].includes(s.state))) throw err('conflict', 'scan_running', 'A scan is still running');
      counter += 1;
      const id = `s_preview_${counter}`;
      const s: MockScan = { state: 'running', seen: 0, cancel: false, started: new Date().toISOString(), finished: null, root: o.path_display };
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
      return { scan_id: id, root_display: o.path_display };
    },
    async scanStatus(id) {
      await delay(60);
      needIndex();
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
        parent_removed: ((r) => (r.count ? r : null))(removedTotal(scanId, Number(parent ?? '1'))),
        rest: {
          count: rest.length,
          known_logical_bytes: rest.reduce((a, e) => a + (e.logical_bytes ? BigInt(e.logical_bytes) : 0n), 0n).toString(),
          unknown_count: rest.filter((e) => e.logical_bytes === null).length,
          complete: true,
        },
      };
    },
    async removedEntries(scanId, ids) {
      await delay(60);
      const marks = removals(scanId);
      const entries = ids.flatMap((id) => {
        const n = nodes.get(Number(id));
        if (!n || !marks.size) return [];
        const e = withRemoved(scanId, n, toEntry(nodes, n));
        return e.removed || e.removed_count ? [{ entry_id: id, removed: e.removed ?? null, removed_count: e.removed_count ?? 0 }] : [];
      });
      return { scan_id: scanId, total: removedTotal(scanId, null), entries };
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
    async onFeaturesChanged() {
      return () => {};
    },
  };
}
