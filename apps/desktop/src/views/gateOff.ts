// D15 — a section whose capability has not passed its release gate is off unless 设置 › 实验功能 is on. It keeps
// its place in the navigation (GUI-08) and shows this explanatory page instead of the tool; the host refuses the
// tool's commands as well, so nothing here depends on the page being hidden.

import type { App } from '../main';
import { h, replace } from '../lib/dom';
import { EXTENSIONS, toolForPage, type GatedSection } from '../lib/nav';
import { words } from '../lib/platform';
import type { Page } from '../lib/store';
import { gateOffPage } from './kit';

/** What each gated section does, in the person's words (shown while it is off). */
const DOES: Partial<Record<Page, () => string[]>> = {
  purge: () => ['在你添加的项目文件夹里发现可重新生成的构建产物，例如 node_modules、target 和 build。', '最近改动、被版本控制跟踪的项目默认不选，生成的计划需要你审阅。'],
  installer: () => ['在下载等声明位置发现已经用过的安装包和压缩包，并标出来源。', '安装包默认需要逐项确认，生成的计划需要你审阅。'],
  recyclebin: () => [`列出${words().bin}中的项目，并把选中的项目冻结成永久删除计划。`, `在“分析 › 磁盘”中手选对象，生成${words().moveToBin}计划。`],
  insights: () => ['汇总虚拟机、快照、系统文件和旧下载等已知大型对象，标出观测时间。', '只给出处理指引，不会生成任何清理。'],
  uninstall: () => ['用应用登记的卸载程序或包管理器卸载应用，再单独审阅残留。', '注册表残留先备份再删除，可以从历史中恢复。'],
  updates: () => ['只在你要求时通过 winget、商店和 Scoop 检查更新。', '经验证的包管理器更新会生成需要审阅的计划，其他应用指向它自己的更新入口。'],
  startup: () => ['列出登录启动项与后台任务，显示来源和签名。', '停用、启用或删除失效项都会生成需要审阅的计划，并且可以恢复。'],
  maintenance: () => ['运行系统维护任务，例如重建图标缓存、清理更新缓存和 TRIM。', '需要管理员的任务在一次 UAC 确认后由单独的工作进程执行。'],
  diagnosis: () => ['只读查看处理器、内存和磁盘的占用来源，帮助判断哪里变慢。', '诊断从不结束进程或更改设置。'],
  status: () => ['显示处理器、内存、磁盘、网络和电池的实时指标。', '可选的持续高 CPU 提醒默认关闭。'],
  processes: () => ['列出进程的位置、签名、用途和保护等级。', '结束一个进程需要单独审阅；关键系统进程会被拒绝。'],
  resident: () => ['在通知区域显示简要状态，并提供亮屏、清洁屏幕、隐私指示和电量提醒。', '关闭实验功能会停止这些常驻工具并删除登录启动项。'],
  diag: () => ['把你选中的一个对象的事实发送给你配置的 AI 提供方，请它解释用途和风险。', '发送前先预览实际资料和目的地；诊断从不批准或生成计划。'],
};

export function titleOf(page: Page): string {
  if (page === 'recyclebin') return words().bin;
  return toolForPage(page)?.sections.find((s) => s.page === page)?.label ?? EXTENSIONS.find((e) => e.page === page)?.label ?? page;
}

/** What an off section says it does (empty for a page that is not gated). */
export function offDoes(page: Page): string[] {
  return words().offDoes[page] ?? DOES[page]?.() ?? [];
}

/** Bring 设置 › 实验功能 into view with focus on its switch. */
export function openExperimentalSettings(app: App): void {
  app.navigate('settings');
  requestAnimationFrame(() => {
    document.getElementById('set-experimental-title')?.scrollIntoView({ block: 'start' });
    (document.getElementById('experimental-switch') as HTMLElement | null)?.focus({ preventScroll: true });
  });
}

export class GateOffView {
  el: HTMLElement;
  private gate: GatedSection;

  constructor(
    private app: App,
    private page: Page,
    gate: GatedSection,
  ) {
    this.gate = gate;
    this.el = h('section', { class: `page page-${page} page-gate-off`, 'aria-labelledby': `off-${page}-title` });
  }

  /** The host's row may name a different gate than the local fallback; keep the page in step. */
  show(gate: GatedSection): void {
    this.gate = gate;
  }

  update(): void {
    replace(
      this.el,
      gateOffPage({
        id: `off-${this.page}`,
        title: titleOf(this.page),
        capability: this.gate.capability,
        gate: this.gate.gate,
        does: offDoes(this.page),
        openSettings: () => openExperimentalSettings(this.app),
      }),
    );
  }
}
