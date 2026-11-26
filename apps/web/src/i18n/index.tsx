import React, { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { STORAGE_KEYS } from '../constants/routes';
import {
  AlertPriority,
  DeliveryStatus,
  FetchOutcome,
  Locale,
  NotificationChannelType,
  RunStatus,
  RunType,
  SourceScope,
  SourceType,
} from '../types/enums';
import { enMessages, type TranslationKey } from './locales/en';
import { zhMessages } from './locales/zh';

export type { TranslationKey };

export type TranslationParams = Record<string, string | number>;
export type TranslateFn = (key: TranslationKey, params?: TranslationParams) => string;

const DICTIONARIES: Record<Locale, Record<TranslationKey, string>> = {
  [Locale.EN_US]: enMessages,
  [Locale.ZH_CN]: zhMessages,
};

function isSupportedLocale(value: string | null | undefined): value is Locale {
  return value === Locale.ZH_CN || value === Locale.EN_US;
}

function normalizeBrowserLanguage(rawLang: string): Locale {
  const normalized = rawLang.trim().toLowerCase();
  if (normalized.startsWith('zh')) {
    return Locale.ZH_CN;
  }
  return Locale.EN_US;
}

export function resolveInitialLocale(): Locale {
  if (typeof window !== 'undefined') {
    try {
      const stored = window.localStorage?.getItem(STORAGE_KEYS.LOCALE);
      if (isSupportedLocale(stored)) {
        return stored;
      }
    } catch {
      // Ignore storage read errors.
    }
  }

  if (typeof navigator !== 'undefined') {
    const candidates =
      Array.isArray(navigator.languages) && navigator.languages.length > 0
        ? navigator.languages
        : navigator.language
          ? [navigator.language]
          : [];
    for (const lang of candidates) {
      if (lang && lang.trim()) {
        return normalizeBrowserLanguage(lang);
      }
    }
  }

  return Locale.EN_US;
}

export function translateMessage(
  locale: Locale,
  key: TranslationKey,
  params?: TranslationParams,
): string {
  const dict = DICTIONARIES[locale] ?? enMessages;
  const template = dict[key] ?? enMessages[key] ?? key;
  if (!params) {
    return template;
  }
  return template.replace(/\{([A-Za-z0-9_]+)\}/g, (full, name: string) => {
    const value = params[name];
    return value === undefined ? full : String(value);
  });
}

export interface I18nContextValue {
  locale: Locale;
  setLocale: (next: Locale) => void;
  toggleLocale: () => void;
  t: (key: TranslationKey, params?: TranslationParams) => string;
  formatDateTime: (value: string | null | undefined, emptyFallbackKey?: TranslationKey) => string;
  formatShortTime: (value: string) => string;
  labelSourceType: (val: SourceType) => string;
  labelSourceScope: (val: SourceScope) => string;
  labelChannelType: (val: NotificationChannelType) => string;
  labelRunType: (val: RunType) => string;
  labelRunStatus: (val: RunStatus) => string;
  labelDeliveryStatus: (val: DeliveryStatus) => string;
  labelFetchOutcome: (val: FetchOutcome | null, emptyKey?: 'never_fetched' | 'pending') => string;
  labelAlertPriority: (val: AlertPriority) => string;
}

function createStaticI18nValue(
  locale: Locale,
  setLocale: (next: Locale) => void,
  toggleLocale: () => void,
): I18nContextValue {
  const t = (key: TranslationKey, params?: TranslationParams): string =>
    translateMessage(locale, key, params);

  const formatDateTime = (
    value: string | null | undefined,
    emptyFallbackKey: TranslationKey = 'common.unknownTime',
  ): string => {
    if (!value) {
      return t(emptyFallbackKey);
    }
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) {
      return value;
    }
    return parsed.toLocaleString(locale);
  };

  const formatShortTime = (value: string): string => {
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) {
      return value;
    }
    return parsed.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  };

  const labelSourceType = (val: SourceType): string =>
    t(`enum.sourceType.${val}` as TranslationKey);

  const labelSourceScope = (val: SourceScope): string =>
    t(`enum.sourceScope.${val}` as TranslationKey);

  const labelChannelType = (val: NotificationChannelType): string =>
    t(`enum.channelType.${val}` as TranslationKey);

  const labelRunType = (val: RunType): string => t(`enum.runType.${val}` as TranslationKey);

  const labelRunStatus = (val: RunStatus): string => t(`enum.runStatus.${val}` as TranslationKey);

  const labelDeliveryStatus = (val: DeliveryStatus): string =>
    t(`enum.deliveryStatus.${val}` as TranslationKey);

  const labelFetchOutcome = (
    val: FetchOutcome | null,
    emptyKey: 'never_fetched' | 'pending' = 'pending',
  ): string => {
    if (val === null) {
      return t(`enum.fetchOutcome.${emptyKey}` as TranslationKey);
    }
    return t(`enum.fetchOutcome.${val}` as TranslationKey);
  };

  const labelAlertPriority = (val: AlertPriority): string =>
    t(`enum.priority.${val}` as TranslationKey);

  return {
    locale,
    setLocale,
    toggleLocale,
    t,
    formatDateTime,
    formatShortTime,
    labelSourceType,
    labelSourceScope,
    labelChannelType,
    labelRunType,
    labelRunStatus,
    labelDeliveryStatus,
    labelFetchOutcome,
    labelAlertPriority,
  };
}

const I18nContext = createContext<I18nContextValue | null>(null);

export interface I18nProviderProps {
  initialLocale?: Locale;
  children: React.ReactNode;
}

export function I18nProvider({ initialLocale, children }: I18nProviderProps) {
  const [locale, setLocaleState] = useState<Locale>(() => initialLocale ?? resolveInitialLocale());

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    if (typeof window !== 'undefined') {
      try {
        window.localStorage?.setItem(STORAGE_KEYS.LOCALE, next);
      } catch {
        // Ignore storage write errors.
      }
    }
  }, []);

  const toggleLocale = useCallback(() => {
    setLocaleState((prev) => {
      const next = prev === Locale.ZH_CN ? Locale.EN_US : Locale.ZH_CN;
      if (typeof window !== 'undefined') {
        try {
          window.localStorage?.setItem(STORAGE_KEYS.LOCALE, next);
        } catch {
          // Ignore storage write errors.
        }
      }
      return next;
    });
  }, []);

  useEffect(() => {
    if (typeof document !== 'undefined') {
      document.documentElement.lang = locale;
    }
  }, [locale]);

  const value = useMemo(
    () => createStaticI18nValue(locale, setLocale, toggleLocale),
    [locale, setLocale, toggleLocale],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/**
 * Returns reactive i18n helpers when mounted under `<I18nProvider>`, or
 * seamlessly falls back to browser-detected locale for standalone component tests.
 */
export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  const fallback = useMemo(
    () =>
      createStaticI18nValue(
        resolveInitialLocale(),
        () => {},
        () => {},
      ),
    [],
  );
  return ctx ?? fallback;
}
