import {
  STORAGE_KEYS,
  executeWithTokenRefresh,
  fetchAdminPlatformStatus,
  maskToken,
  normalizeServerUrl,
  pushPlatformXAuthToken,
  selectBestXCookie,
  testPlatformXCollection,
} from './lib/sync-core.mjs';

const EXT_STORAGE = {
  SERVER_URL: 'ext.server_url',
  SESSION_MAP: 'ext.session_map',
  LANG: 'ext.lang',
};

const TEXT = {
  zh: {
    title: 'X Cookie 管理员同步',
    subtitle: '一键读取浏览器 x.com 登录态并热更新平台采集',
    langToggle: 'EN',
    serverSection: '1. 目标站点与管理员会话',
    serverUrlLabel: '服务地址 (Server URL)',
    useActiveTab: '当前页',
    syncSession: '同步登录态并检查权限',
    openPortal: '打开平台登录',
    manualSummary: '无法从标签页读取会话时，可手动填写管理员 Bearer Token',
    applyManual: '使用',
    cookieSection: '2. 浏览器 X (x.com) Cookie',
    refreshCookie: '刷新 Cookie 检测',
    openX: '打开 x.com',
    actionSection: '3. 一键导入与端到端验证',
    importBtn: '一键导入 X Cookie 并热更新',
    importingBtn: '正在加密导入并热更新...',
    testBtn: '测试抓取',
    testingBtn: '测试中...',
    badgeUnconnected: '未连接',
    badgeChecking: '检测中',
    badgeAdminReady: '管理员已就绪',
    badgeNeedLogin: '需登录平台',
    badgeNotAdmin: '非管理员账号',
    badgeCookieReady: '已检测到 X Cookie',
    badgeCookieMissing: '未检测到 auth_token',
    accountLabel: '当前账号',
    roleLabel: '角色',
    serverTokenLabel: '服务端 X Cookie',
    bridgeLabel: '采集桥接',
    configured: '已配置',
    unconfigured: '未配置',
    cookiePreviewLabel: 'auth_token 预览',
    cookieDomainLabel: '域名',
    cookieExpiresLabel: '过期时间',
    cookieCt0Label: '辅助 ct0',
    sessionCookie: '浏览器会话期',
    present: '存在',
    missing: '缺失',
    noCookieHint: '请先在同一 Chrome 配置中登录 x.com，然后点击“刷新 Cookie 检测”。',
    noTabSessionHint: '未在浏览器存储或当前打开的平台标签页中找到管理员登录态。请先点击“打开平台登录”并完成登录。',
    permissionDenied: '未授予目标站点的浏览器访问权限，无法读取登录态或调用接口。',
    importSuccessReloaded: '导入成功！已加密写入平台数据库并完成 RSSHub 零重启热更新（Hint: {hint}）。',
    importSuccessSavedOnly: '已加密保存到平台数据库（Hint: {hint}）。提示：{msg}',
    testPassed: '抓取验证成功：{count} 条动态（模式 {mode}，耗时 {ms}ms）',
    testFailed: '抓取验证失败：{msg}',
    invalidUrl: '请输入有效的 http/https 服务地址。',
    tabNotSite: '当前标签页不是目标平台页面，请先切换到 Gemini Daily Digest 标签页或手动输入地址。',
  },
  en: {
    title: 'Admin X Cookie Sync',
    subtitle: 'One-click x.com browser cookie import & live bridge hot-reload',
    langToggle: '中文',
    serverSection: '1. Target Server & Admin Session',
    serverUrlLabel: 'Server URL',
    useActiveTab: 'Current Tab',
    syncSession: 'Sync Session & Verify Admin',
    openPortal: 'Open Portal Login',
    manualSummary: 'Fallback: paste an admin Bearer token if no portal tab is open',
    applyManual: 'Apply',
    cookieSection: '2. Browser X (x.com) Cookie',
    refreshCookie: 'Refresh Cookie Check',
    openX: 'Open x.com',
    actionSection: '3. One-Click Import & Live Test',
    importBtn: 'Import X Cookie & Hot-Reload',
    importingBtn: 'Encrypting & hot-reloading...',
    testBtn: 'Run Test',
    testingBtn: 'Testing...',
    badgeUnconnected: 'Not Connected',
    badgeChecking: 'Checking',
    badgeAdminReady: 'Admin Ready',
    badgeNeedLogin: 'Sign-In Required',
    badgeNotAdmin: 'Not Admin',
    badgeCookieReady: 'X Cookie Ready',
    badgeCookieMissing: 'No auth_token',
    accountLabel: 'Signed In As',
    roleLabel: 'Role',
    serverTokenLabel: 'Server X Cookie',
    bridgeLabel: 'X Bridge',
    configured: 'Configured',
    unconfigured: 'Unconfigured',
    cookiePreviewLabel: 'auth_token Preview',
    cookieDomainLabel: 'Domain',
    cookieExpiresLabel: 'Expires',
    cookieCt0Label: 'Companion ct0',
    sessionCookie: 'Browser Session',
    present: 'Present',
    missing: 'Missing',
    noCookieHint: 'Sign in to x.com in this Chrome profile first, then click "Refresh Cookie Check".',
    noTabSessionHint: 'No admin session found in extension cache or open portal tabs. Click "Open Portal Login" and sign in first.',
    permissionDenied: 'Site access permission was denied. Allow access to read your session and call the admin API.',
    importSuccessReloaded: 'Imported! Encrypted in platform DB and hot-reloaded RSSHub with zero downtime (Hint: {hint}).',
    importSuccessSavedOnly: 'Encrypted and saved to platform DB (Hint: {hint}). Note: {msg}',
    testPassed: 'Live test passed: {count} items fetched (mode {mode}, {ms}ms).',
    testFailed: 'Live test failed: {msg}',
    invalidUrl: 'Enter a valid http:// or https:// server URL.',
    tabNotSite: 'The active tab is not the Gemini Daily Digest portal. Switch to it or enter the URL manually.',
  },
};

const state = {
  lang: navigator.language?.toLowerCase().startsWith('zh') ? 'zh' : 'en',
  baseUrl: '',
  origin: '',
  originPattern: '',
  session: null,
  adminUser: null,
  platformSettings: null,
  xCookie: null,
  xCt0Present: false,
  busy: false,
};

const el = {
  titleText: document.getElementById('titleText'),
  subtitleText: document.getElementById('subtitleText'),
  langToggleBtn: document.getElementById('langToggleBtn'),
  serverSectionLabel: document.getElementById('serverSectionLabel'),
  serverUrlLabel: document.getElementById('serverUrlLabel'),
  serverUrlInput: document.getElementById('serverUrlInput'),
  useActiveTabBtn: document.getElementById('useActiveTabBtn'),
  syncSessionBtn: document.getElementById('syncSessionBtn'),
  openPortalBtn: document.getElementById('openPortalBtn'),
  sessionBadge: document.getElementById('sessionBadge'),
  sessionMeta: document.getElementById('sessionMeta'),
  manualSummary: document.getElementById('manualSummary'),
  manualTokenInput: document.getElementById('manualTokenInput'),
  applyManualTokenBtn: document.getElementById('applyManualTokenBtn'),
  cookieSectionLabel: document.getElementById('cookieSectionLabel'),
  cookieBadge: document.getElementById('cookieBadge'),
  cookieDetails: document.getElementById('cookieDetails'),
  refreshCookieBtn: document.getElementById('refreshCookieBtn'),
  openXBtn: document.getElementById('openXBtn'),
  actionSectionLabel: document.getElementById('actionSectionLabel'),
  importCookieBtn: document.getElementById('importCookieBtn'),
  testHandleInput: document.getElementById('testHandleInput'),
  testXBtn: document.getElementById('testXBtn'),
  statusBox: document.getElementById('statusBox'),
  testResultBox: document.getElementById('testResultBox'),
};

function t(key, vars = {}) {
  let msg = TEXT[state.lang]?.[key] ?? TEXT.en[key] ?? key;
  for (const [k, v] of Object.entries(vars)) {
    msg = msg.replaceAll(`{${k}}`, String(v ?? ''));
  }
  return msg;
}

function setBadge(target, text, kind) {
  target.textContent = text;
  target.className = `badge badge-${kind}`;
}

function showStatus(message, kind = 'ok') {
  if (!message) {
    el.statusBox.classList.add('hidden');
    el.statusBox.textContent = '';
    return;
  }
  el.statusBox.className = `status-box status-${kind}`;
  el.statusBox.textContent = message;
}

function renderMetaRows(container, rows) {
  container.replaceChildren();
  for (const [label, value] of rows) {
    const line = document.createElement('div');
    line.className = 'meta-line';
    const keySpan = document.createElement('span');
    keySpan.className = 'meta-key';
    keySpan.textContent = label;
    const valSpan = document.createElement('span');
    valSpan.className = 'meta-val';
    valSpan.textContent = value;
    line.append(keySpan, valSpan);
    container.append(line);
  }
}

function renderLanguage() {
  document.documentElement.lang = state.lang === 'zh' ? 'zh-CN' : 'en';
  el.titleText.textContent = t('title');
  el.subtitleText.textContent = t('subtitle');
  el.langToggleBtn.textContent = t('langToggle');
  el.serverSectionLabel.textContent = t('serverSection');
  el.serverUrlLabel.textContent = t('serverUrlLabel');
  el.useActiveTabBtn.textContent = t('useActiveTab');
  el.syncSessionBtn.textContent = t('syncSession');
  el.openPortalBtn.textContent = t('openPortal');
  el.manualSummary.textContent = t('manualSummary');
  el.applyManualTokenBtn.textContent = t('applyManual');
  el.cookieSectionLabel.textContent = t('cookieSection');
  el.refreshCookieBtn.textContent = t('refreshCookie');
  el.openXBtn.textContent = t('openX');
  el.actionSectionLabel.textContent = t('actionSection');
  el.importCookieBtn.textContent = state.busy ? t('importingBtn') : t('importBtn');
  el.testXBtn.textContent = state.busy ? t('testingBtn') : t('testBtn');
  renderSessionSection();
  renderCookieSection();
  updateActionButtons();
}

function renderSessionSection() {
  if (!state.adminUser || !state.platformSettings) {
    el.sessionMeta.classList.add('hidden');
    return;
  }
  setBadge(el.sessionBadge, t('badgeAdminReady'), 'ok');
  el.sessionMeta.classList.remove('hidden');
  const who = state.adminUser.displayName || state.adminUser.email || 'Admin';
  const tokenHint = state.platformSettings.hasXAuthToken
    ? state.platformSettings.xAuthTokenHint || t('configured')
    : t('unconfigured');
  const bridgeStatus = state.platformSettings.xBridgeConfigured
    ? state.platformSettings.xBridgeReloadUrl
      ? t('configured')
      : 'URL ok'
    : t('unconfigured');
  renderMetaRows(el.sessionMeta, [
    [t('accountLabel'), who],
    [t('roleLabel'), state.adminUser.role || 'admin'],
    [t('serverTokenLabel'), tokenHint],
    [t('bridgeLabel'), bridgeStatus],
  ]);
}

function renderCookieSection() {
  if (!state.xCookie) {
    setBadge(el.cookieBadge, t('badgeCookieMissing'), 'warn');
    el.cookieDetails.replaceChildren();
    const note = document.createElement('div');
    note.textContent = t('noCookieHint');
    el.cookieDetails.append(note);
    return;
  }
  setBadge(el.cookieBadge, t('badgeCookieReady'), 'ok');
  const expiresStr =
    typeof state.xCookie.expirationDate === 'number' && Number.isFinite(state.xCookie.expirationDate)
      ? new Date(state.xCookie.expirationDate * 1000).toLocaleString()
      : t('sessionCookie');
  renderMetaRows(el.cookieDetails, [
    [t('cookiePreviewLabel'), maskToken(state.xCookie.value)],
    [t('cookieDomainLabel'), state.xCookie.domain || 'x.com'],
    [t('cookieExpiresLabel'), expiresStr],
    [t('cookieCt0Label'), state.xCt0Present ? t('present') : t('missing')],
  ]);
}

function updateActionButtons() {
  const adminReady = Boolean(state.adminUser && state.baseUrl);
  const cookieReady = Boolean(state.xCookie?.value);
  el.importCookieBtn.disabled = state.busy || !adminReady || !cookieReady;
  el.testXBtn.disabled = state.busy || !adminReady;
}

async function saveExtensionValue(key, value) {
  await chrome.storage.local.set({ [key]: value });
}

async function readExtensionValue(key, fallback = null) {
  const res = await chrome.storage.local.get([key]);
  return res[key] ?? fallback;
}

async function saveOriginSession(origin, session) {
  const map = (await readExtensionValue(EXT_STORAGE.SESSION_MAP, {})) || {};
  if (session) {
    map[origin] = session;
  } else {
    delete map[origin];
  }
  await saveExtensionValue(EXT_STORAGE.SESSION_MAP, map);
}

async function loadOriginSession(origin) {
  const map = (await readExtensionValue(EXT_STORAGE.SESSION_MAP, {})) || {};
  return map[origin] ?? null;
}

async function detectBrowserXCookies() {
  const [xAuth, twAuth, xCt0, twCt0] = await Promise.all([
    chrome.cookies.getAll({ domain: 'x.com', name: 'auth_token' }),
    chrome.cookies.getAll({ domain: 'twitter.com', name: 'auth_token' }),
    chrome.cookies.getAll({ domain: 'x.com', name: 'ct0' }),
    chrome.cookies.getAll({ domain: 'twitter.com', name: 'ct0' }),
  ]);
  state.xCookie = selectBestXCookie([...xAuth, ...twAuth]);
  state.xCt0Present = [...xCt0, ...twCt0].some((c) => typeof c.value === 'string' && c.value.trim().length > 0);
  renderCookieSection();
  updateActionButtons();
}

async function ensureHostPermission(originPattern, interactive) {
  const already = await chrome.permissions.contains({ origins: [originPattern] });
  if (already) {
    return true;
  }
  if (!interactive) {
    return false;
  }
  return chrome.permissions.request({ origins: [originPattern] });
}

async function readSessionFromPortalTabs(originPattern) {
  const tabs = await chrome.tabs.query({ url: originPattern });
  for (const tab of tabs) {
    if (typeof tab.id !== 'number') {
      continue;
    }
    try {
      const results = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (keys) => {
          const readKey = (k) => {
            try {
              const v = window.localStorage?.getItem(k);
              if (v) return v;
            } catch {
              // ignore
            }
            try {
              return window.sessionStorage?.getItem(k) ?? null;
            } catch {
              return null;
            }
          };
          const accessToken = readKey(keys.ACCESS_TOKEN);
          const refreshToken = readKey(keys.REFRESH_TOKEN);
          const rawExp = readKey(keys.TOKEN_EXPIRES_AT);
          const expiresAt = rawExp && Number.isFinite(Number(rawExp)) ? Number(rawExp) : null;
          if (!accessToken && !refreshToken) {
            return null;
          }
          return { accessToken, refreshToken, expiresAt };
        },
        args: [STORAGE_KEYS],
      });
      const extracted = results?.[0]?.result ?? null;
      if (extracted && (extracted.accessToken || extracted.refreshToken)) {
        return extracted;
      }
    } catch {
      // Ignore restricted/discarded tabs and keep scanning.
    }
  }
  return null;
}

async function writeSessionBackToPortalTabs(originPattern, session) {
  if (!session?.accessToken) {
    return;
  }
  const tabs = await chrome.tabs.query({ url: originPattern });
  for (const tab of tabs) {
    if (typeof tab.id !== 'number') {
      continue;
    }
    try {
      await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: (keys, next) => {
          try {
            if (next.accessToken) window.localStorage?.setItem(keys.ACCESS_TOKEN, next.accessToken);
            if (next.refreshToken) window.localStorage?.setItem(keys.REFRESH_TOKEN, next.refreshToken);
            if (typeof next.expiresAt === 'number') {
              window.localStorage?.setItem(keys.TOKEN_EXPIRES_AT, String(next.expiresAt));
            }
          } catch {
            // ignore storage errors
          }
        },
        args: [STORAGE_KEYS, session],
      });
    } catch {
      // ignore tab write failures
    }
  }
}

async function syncAndVerifySession({ interactive = true } = {}) {
  let normalized;
  try {
    normalized = normalizeServerUrl(el.serverUrlInput.value);
  } catch {
    setBadge(el.sessionBadge, t('badgeUnconnected'), 'muted');
    if (interactive) {
      showStatus(t('invalidUrl'), 'err');
    }
    return false;
  }

  state.baseUrl = normalized.baseUrl;
  state.origin = normalized.origin;
  state.originPattern = normalized.originPattern;
  el.serverUrlInput.value = normalized.baseUrl;
  await saveExtensionValue(EXT_STORAGE.SERVER_URL, normalized.baseUrl);

  const permitted = await ensureHostPermission(normalized.originPattern, interactive);
  if (!permitted) {
    setBadge(el.sessionBadge, t('badgeUnconnected'), 'muted');
    if (interactive) {
      showStatus(t('permissionDenied'), 'warn');
    }
    return false;
  }

  setBadge(el.sessionBadge, t('badgeChecking'), 'muted');
  const tabSession = await readSessionFromPortalTabs(normalized.originPattern);
  const cachedSession = await loadOriginSession(normalized.origin);
  let candidateSession = tabSession ?? cachedSession ?? { accessToken: null, refreshToken: null, expiresAt: null };
  if (tabSession && cachedSession?.refreshToken && !tabSession.refreshToken) {
    candidateSession = { ...tabSession, refreshToken: cachedSession.refreshToken };
  }

  try {
    const outcome = await executeWithTokenRefresh({
      baseUrl: normalized.baseUrl,
      session: candidateSession,
      operation: (token) =>
        fetchAdminPlatformStatus({
          baseUrl: normalized.baseUrl,
          accessToken: token,
        }),
    });
    state.session = outcome.session;
    state.adminUser = outcome.result.user;
    state.platformSettings = outcome.result.settings;
    await saveOriginSession(normalized.origin, outcome.session);
    if (outcome.refreshed) {
      await writeSessionBackToPortalTabs(normalized.originPattern, outcome.session);
    }
    renderSessionSection();
    updateActionButtons();
    showStatus('');
    return true;
  } catch (err) {
    state.adminUser = null;
    state.platformSettings = null;
    el.sessionMeta.classList.add('hidden');
    if (err?.code === 'FORBIDDEN_NOT_ADMIN' || err?.status === 403) {
      setBadge(el.sessionBadge, t('badgeNotAdmin'), 'err');
      showStatus(err.message, 'err');
    } else if (err?.status === 401 || err?.code === 'UNAUTHORIZED') {
      await saveOriginSession(normalized.origin, null);
      state.session = null;
      setBadge(el.sessionBadge, t('badgeNeedLogin'), 'warn');
      if (interactive) {
        showStatus(t('noTabSessionHint'), 'warn');
      }
    } else {
      setBadge(el.sessionBadge, t('badgeUnconnected'), 'err');
      if (interactive) {
        showStatus(err?.message || String(err), 'err');
      }
    }
    updateActionButtons();
    return false;
  }
}

async function handleImportCookie() {
  if (!state.xCookie?.value || !state.baseUrl || !state.session) {
    return;
  }
  state.busy = true;
  renderLanguage();
  showStatus('');
  el.testResultBox.classList.add('hidden');

  try {
    const outcome = await executeWithTokenRefresh({
      baseUrl: state.baseUrl,
      session: state.session,
      operation: (token) =>
        pushPlatformXAuthToken({
          baseUrl: state.baseUrl,
          accessToken: token,
          authToken: state.xCookie.value,
        }),
    });
    state.session = outcome.session;
    await saveOriginSession(state.origin, outcome.session);
    if (outcome.refreshed) {
      await writeSessionBackToPortalTabs(state.originPattern, outcome.session);
    }
    state.platformSettings = outcome.result.settings;
    renderSessionSection();

    const hint = outcome.result.settings?.xAuthTokenHint || maskToken(state.xCookie.value);
    if (outcome.result.bridgeReloaded) {
      showStatus(t('importSuccessReloaded', { hint }), 'ok');
    } else {
      showStatus(
        t('importSuccessSavedOnly', {
          hint,
          msg: outcome.result.bridgeMessage || '',
        }),
        'warn',
      );
    }
  } catch (err) {
    showStatus(err?.message || String(err), 'err');
  } finally {
    state.busy = false;
    renderLanguage();
  }
}

async function handleTestX() {
  if (!state.baseUrl || !state.session) {
    return;
  }
  state.busy = true;
  renderLanguage();
  el.testResultBox.classList.add('hidden');

  try {
    const handle = el.testHandleInput.value.trim();
    const outcome = await executeWithTokenRefresh({
      baseUrl: state.baseUrl,
      session: state.session,
      operation: (token) =>
        testPlatformXCollection({
          baseUrl: state.baseUrl,
          accessToken: token,
          handle,
        }),
    });
    state.session = outcome.session;
    await saveOriginSession(state.origin, outcome.session);
    const res = outcome.result;
    el.testResultBox.replaceChildren();
    el.testResultBox.classList.remove('hidden');

    const summary = document.createElement('div');
    summary.className = res.ok ? 'badge badge-ok' : 'badge badge-err';
    summary.textContent = res.ok
      ? t('testPassed', { count: res.itemsFetched, mode: res.mode, ms: res.latencyMs })
      : t('testFailed', { msg: res.message });
    el.testResultBox.append(summary);

    if (Array.isArray(res.sampleTitles) && res.sampleTitles.length > 0) {
      const list = document.createElement('ul');
      for (const itemTitle of res.sampleTitles) {
        const li = document.createElement('li');
        li.textContent = itemTitle;
        list.append(li);
      }
      el.testResultBox.append(list);
    }
  } catch (err) {
    showStatus(err?.message || String(err), 'err');
  } finally {
    state.busy = false;
    renderLanguage();
  }
}

async function detectOriginFromActiveTab(forceFill = false) {
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  const activeUrl = tabs?.[0]?.url;
  if (!activeUrl) {
    if (forceFill) showStatus(t('tabNotSite'), 'warn');
    return;
  }
  try {
    const parsed = new URL(activeUrl);
    const host = parsed.hostname.toLowerCase();
    const isNonPortal =
      (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') ||
      host === 'x.com' ||
      host.endsWith('.x.com') ||
      host === 'twitter.com' ||
      host.endsWith('.twitter.com');
    if (isNonPortal) {
      if (forceFill) showStatus(t('tabNotSite'), 'warn');
      return;
    }
    el.serverUrlInput.value = parsed.origin;
    if (forceFill) {
      await syncAndVerifySession({ interactive: true });
    }
  } catch {
    if (forceFill) showStatus(t('tabNotSite'), 'warn');
  }
}

el.langToggleBtn.addEventListener('click', async () => {
  state.lang = state.lang === 'zh' ? 'en' : 'zh';
  await saveExtensionValue(EXT_STORAGE.LANG, state.lang);
  renderLanguage();
});

el.useActiveTabBtn.addEventListener('click', () => {
  void detectOriginFromActiveTab(true);
});

el.syncSessionBtn.addEventListener('click', () => {
  void syncAndVerifySession({ interactive: true });
});

el.openPortalBtn.addEventListener('click', () => {
  try {
    const { baseUrl } = normalizeServerUrl(el.serverUrlInput.value);
    void chrome.tabs.create({ url: baseUrl });
  } catch {
    showStatus(t('invalidUrl'), 'err');
  }
});

el.applyManualTokenBtn.addEventListener('click', async () => {
  let normalized;
  try {
    normalized = normalizeServerUrl(el.serverUrlInput.value);
  } catch {
    showStatus(t('invalidUrl'), 'err');
    return;
  }
  const manualToken = el.manualTokenInput.value.trim();
  if (!manualToken) {
    return;
  }
  await saveOriginSession(normalized.origin, {
    accessToken: manualToken,
    refreshToken: null,
    expiresAt: null,
  });
  el.manualTokenInput.value = '';
  await syncAndVerifySession({ interactive: true });
});

el.refreshCookieBtn.addEventListener('click', () => {
  void detectBrowserXCookies();
});

el.openXBtn.addEventListener('click', () => {
  void chrome.tabs.create({ url: 'https://x.com/home' });
});

el.importCookieBtn.addEventListener('click', () => {
  void handleImportCookie();
});

el.testXBtn.addEventListener('click', () => {
  void handleTestX();
});

async function bootstrapPopup() {
  const savedLang = await readExtensionValue(EXT_STORAGE.LANG);
  if (savedLang === 'zh' || savedLang === 'en') {
    state.lang = savedLang;
  }
  const savedUrl = await readExtensionValue(EXT_STORAGE.SERVER_URL, '');
  if (typeof savedUrl === 'string' && savedUrl.trim()) {
    el.serverUrlInput.value = savedUrl.trim();
  } else {
    await detectOriginFromActiveTab(false);
  }
  renderLanguage();
  await detectBrowserXCookies();
  if (el.serverUrlInput.value.trim()) {
    await syncAndVerifySession({ interactive: false });
  }
}

void bootstrapPopup();
