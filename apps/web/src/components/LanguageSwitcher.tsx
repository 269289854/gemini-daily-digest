import { Languages } from 'lucide-react';
import { useI18n } from '../i18n';
import { Locale } from '../types/enums';

export interface LanguageSwitcherProps {
  className?: string;
}

export function LanguageSwitcher({ className = '' }: LanguageSwitcherProps) {
  const { locale, setLocale, t } = useI18n();
  const isZh = locale === Locale.ZH_CN;

  return (
    <div
      role="group"
      aria-label={t('common.languageToggleAria')}
      className={`inline-flex items-center gap-0.5 rounded-lg border border-slate-700/80 bg-slate-900 p-0.5 text-xs ${className}`}
    >
      <Languages className="mx-1.5 h-3.5 w-3.5 text-slate-400" />
      <button
        type="button"
        onClick={() => setLocale(Locale.ZH_CN)}
        aria-pressed={isZh}
        className={`rounded-md px-2 py-1 font-medium transition ${
          isZh ? 'bg-indigo-600 text-white' : 'text-slate-400 hover:text-slate-200'
        }`}
      >
        {t('common.langZh')}
      </button>
      <button
        type="button"
        onClick={() => setLocale(Locale.EN_US)}
        aria-pressed={!isZh}
        className={`rounded-md px-2 py-1 font-medium transition ${
          !isZh ? 'bg-indigo-600 text-white' : 'text-slate-400 hover:text-slate-200'
        }`}
      >
        {t('common.langEn')}
      </button>
    </div>
  );
}
