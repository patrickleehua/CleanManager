// Types for scripts/contrast.mjs (imported by src/lib/contrast.test.ts).
export const PAIRS: [string, string, number, string][];
export function ratio(a: string, b: string): number;
export function checkStylesheet(): ReturnType<typeof check>;
export function check(css: string): { theme: string; fg: string; bg: string; fgHex: string; bgHex: string; ratio: number; min: number; ok: boolean; use: string }[];
