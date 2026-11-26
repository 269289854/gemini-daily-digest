/**
 * Adapter pattern isolating Web Browser vs Tauri 2 Desktop runtime differences.
 * UI pages never inspect `window.__TAURI__` directly.
 */

export const PlatformRuntime = {
  WEB: 'web',
  DESKTOP: 'desktop',
} as const;
export type PlatformRuntime = (typeof PlatformRuntime)[keyof typeof PlatformRuntime];

export interface PlatformBridge {
  readonly runtime: PlatformRuntime;
  openExternalUrl(url: string): Promise<void>;
  notify(title: string, body: string): Promise<void>;
  copyToClipboard(text: string): Promise<void>;
}

export class WebPlatformBridge implements PlatformBridge {
  public readonly runtime = PlatformRuntime.WEB;

  public async openExternalUrl(url: string): Promise<void> {
    window.open(url, '_blank', 'noopener,noreferrer');
  }

  public async notify(title: string, body: string): Promise<void> {
    if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
      new Notification(title, { body });
    }
  }

  public async copyToClipboard(text: string): Promise<void> {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
    }
  }
}

export function createPlatformBridge(): PlatformBridge {
  return new WebPlatformBridge();
}
