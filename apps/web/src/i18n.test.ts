import { describe, it, expect } from 'vitest';
import { detectLanguage, dictionaries } from './i18n';
describe('browser language preferences', () => {
  it('maps Chinese variants to Simplified Chinese and falls back to English', () => {
    expect(detectLanguage('zh-Hant-HK')).toBe('zh-CN');
    expect(detectLanguage('zh-HK')).toBe('zh-CN');
    expect(detectLanguage('zh-CN')).toBe('zh-CN');
    expect(detectLanguage('fr-FR')).toBe('en');
  });
  it('provides all UI keys in every supported language', () => {
    expect(Object.keys(dictionaries).sort()).toEqual(['en', 'zh-CN']);
    for (const values of Object.values(dictionaries)) expect(Object.keys(values).sort()).toEqual(Object.keys(dictionaries.en).sort());
  });
});
