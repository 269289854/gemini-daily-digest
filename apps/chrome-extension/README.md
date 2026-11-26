# Gemini Daily Digest — Admin X Cookie Sync (Chrome Extension)

A lightweight, zero-dependency Manifest V3 Chrome extension for Gemini Daily Digest administrators to import the browser's `x.com` `auth_token` cookie (including `HttpOnly`) directly into the server's encrypted platform settings with zero-restart RSSHub hot-reload.

## Features

- Reads `auth_token` (and checks companion `ct0`) from Chrome's `x.com` / `twitter.com` cookie jar without manual DevTools copying.
- Reuses and silently refreshes your existing Gemini Daily Digest OIDC admin session from an open portal tab (or cached `refresh_token`).
- Sends `{ "xAuthToken": "..." }` over HTTPS to `PUT /api/v1/admin/platform` (encrypted at rest via AES-256-GCM and pushed to the internal bridge).
- Runs live verification (`POST /api/v1/admin/platform/x/test`) right inside the popup.
- Bilingual `中文 / EN` interface.

## Installation (Developer Mode / Unpacked)

1. Open Chrome and navigate to `chrome://extensions/`.
2. Enable **Developer mode** (top-right toggle).
3. Click **Load unpacked** and select this directory:
   `apps/chrome-extension` (or extract `gemini-daily-digest-chrome-extension.zip` from a GitHub Release and select the extracted folder).
4. Pin **Gemini Daily Digest — Admin X Cookie Sync** to the Chrome toolbar, ensure you are signed in to both `x.com` and your Gemini Daily Digest admin portal, then click **Import X Cookie & Hot-Reload**.

## Verification

```bash
node --test apps/chrome-extension/tests/sync-core.test.mjs
```
