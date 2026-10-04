import { describe, it, expect } from 'vitest';
import { detectLanguage, dictionaries } from './i18n';
describe('browser language preferences', () => {
  it('recognizes Traditional Chinese variants and falls back to English', () => {
    expect(detectLanguage('zh-Hant-HK')).toBe('zh-TW');
    expect(detectLanguage('zh-HK')).toBe('zh-TW');
    expect(detectLanguage('zh-CN')).toBe('zh-CN');
    expect(detectLanguage('fr-FR')).toBe('en');
  });
  it('provides all UI keys in every supported language', () => {
    for (const values of Object.values(dictionaries)) expect(Object.keys(values).sort()).toEqual(Object.keys(dictionaries.en).sort());
  });
});
