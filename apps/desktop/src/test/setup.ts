// Vitest setup: the page and word tests describe the 简体中文 wording (byte for byte, LIF-05 keeps it in
// src/i18n/zh-CN.ts), so every test starts in zh-CN whatever the machine's own language is. Tests of the
// English resources switch explicitly.
import { beforeEach } from 'vitest';
import { setLocale } from '../i18n';

setLocale('zh-CN');
beforeEach(() => setLocale('zh-CN'));
