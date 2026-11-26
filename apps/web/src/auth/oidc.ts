/**
 * OIDC Authorization Code + PKCE (S256) helpers using Web Crypto and centralized STORAGE_KEYS.
 */

import { DEFAULTS, HTTP_HEADERS, STORAGE_KEYS } from '../constants/routes';
import { isAllowedExternalUrl } from '../platform/bridge';

export interface OidcDiscoveryEndpoints {
  authorizationEndpoint: string;
  tokenEndpoint: string;
  endSessionEndpoint?: string;
}

export interface BuildAuthorizeUrlParams {
  authorizationEndpoint: string;
  clientId: string;
  redirectUri: string;
  scopes?: readonly string[];
  state: string;
  codeChallenge: string;
}

export interface ExchangeCodeParams {
  tokenEndpoint: string;
  clientId: string;
  code: string;
  redirectUri: string;
  codeVerifier: string;
}

export interface TokenExchangeResult {
  accessToken: string;
  idToken?: string;
  expiresIn?: number;
}

export interface StoredAuthSession {
  accessToken: string;
  idToken: string | null;
  expiresAt: number | null;
}

export interface PkceTransaction {
  verifier: string | null;
  state: string | null;
  postLoginRedirect: string | null;
  redirectUri?: string | null;
}

function bytesToBase64Url(bytes: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

/**
 * Pure TypeScript SHA-256 fallback for plain HTTP contexts where `crypto.subtle` is unavailable.
 */
function sha256Fallback(input: Uint8Array): Uint8Array {
  const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ]);
  const H = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);

  const bitLen = input.length * 8;
  const paddedLen = Math.ceil((input.length + 9) / 64) * 64;
  const padded = new Uint8Array(paddedLen);
  padded.set(input);
  padded[input.length] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(paddedLen - 8, Math.floor(bitLen / 0x100000000), false);
  view.setUint32(paddedLen - 4, bitLen >>> 0, false);

  const w = new Uint32Array(64);
  const rotr = (n: number, x: number): number => (x >>> n) | (x << (32 - n));

  for (let offset = 0; offset < paddedLen; offset += 64) {
    for (let i = 0; i < 16; i += 1) {
      w[i] = view.getUint32(offset + i * 4, false);
    }
    for (let i = 16; i < 64; i += 1) {
      const s0 = rotr(7, w[i - 15]) ^ rotr(18, w[i - 15]) ^ (w[i - 15] >>> 3);
      const s1 = rotr(17, w[i - 2]) ^ rotr(19, w[i - 2]) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }

    let [a, b, c, d, e, f, g, h] = H;
    for (let i = 0; i < 64; i += 1) {
      const S1 = rotr(6, e) ^ rotr(11, e) ^ rotr(25, e);
      const ch = (e & f) ^ (~e & g);
      const temp1 = (h + S1 + ch + K[i] + w[i]) >>> 0;
      const S0 = rotr(2, a) ^ rotr(13, a) ^ rotr(22, a);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (S0 + maj) >>> 0;
      h = g;
      g = f;
      f = e;
      e = (d + temp1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (temp1 + temp2) >>> 0;
    }

    H[0] = (H[0] + a) >>> 0;
    H[1] = (H[1] + b) >>> 0;
    H[2] = (H[2] + c) >>> 0;
    H[3] = (H[3] + d) >>> 0;
    H[4] = (H[4] + e) >>> 0;
    H[5] = (H[5] + f) >>> 0;
    H[6] = (H[6] + g) >>> 0;
    H[7] = (H[7] + h) >>> 0;
  }

  const out = new Uint8Array(32);
  const outView = new DataView(out.buffer);
  for (let i = 0; i < 8; i += 1) {
    outView.setUint32(i * 4, H[i], false);
  }
  return out;
}

function getRandomBytes(length: number): Uint8Array {
  const bytes = new Uint8Array(length);
  if (typeof globalThis.crypto?.getRandomValues === 'function') {
    globalThis.crypto.getRandomValues(bytes);
    return bytes;
  }
  throw new Error('Cryptographically secure random generator is unavailable');
}

export function generateCodeVerifier(): string {
  return bytesToBase64Url(getRandomBytes(32));
}

export async function computeCodeChallengeS256(verifier: string): Promise<string> {
  if (!verifier) {
    throw new Error('PKCE code verifier must not be empty');
  }
  const encoded = new TextEncoder().encode(verifier);
  if (typeof globalThis.crypto?.subtle?.digest === 'function') {
    const digest = await globalThis.crypto.subtle.digest('SHA-256', encoded);
    return bytesToBase64Url(new Uint8Array(digest));
  }
  return bytesToBase64Url(sha256Fallback(encoded));
}

export function generateOidcState(): string {
  return bytesToBase64Url(getRandomBytes(24));
}

export async function discoverOidcEndpoints(
  issuerUrl: string,
  fetchImpl?: typeof fetch,
): Promise<OidcDiscoveryEndpoints> {
  if (!isAllowedExternalUrl(issuerUrl)) {
    throw new Error('Invalid OIDC issuer URL: must use http or https');
  }
  const fetcher = fetchImpl ?? globalThis.fetch;
  if (!fetcher) {
    throw new Error('Fetch implementation is unavailable');
  }
  const normalizedIssuer = issuerUrl.trim().replace(/\/+$/, '');
  const discoveryUrl = `${normalizedIssuer}/.well-known/openid-configuration`;
  const response = await fetcher(discoveryUrl, {
    method: 'GET',
    headers: {
      [HTTP_HEADERS.ACCEPT]: HTTP_HEADERS.CONTENT_TYPE_JSON,
    },
  });
  if (!response.ok) {
    throw new Error(`OIDC discovery failed (${response.status})`);
  }
  const payload = (await response.json()) as Record<string, unknown>;
  const authorizationEndpoint =
    typeof payload.authorization_endpoint === 'string' ? payload.authorization_endpoint : '';
  const tokenEndpoint = typeof payload.token_endpoint === 'string' ? payload.token_endpoint : '';
  const endSessionEndpoint =
    typeof payload.end_session_endpoint === 'string' &&
    isAllowedExternalUrl(payload.end_session_endpoint)
      ? payload.end_session_endpoint
      : undefined;

  if (!isAllowedExternalUrl(authorizationEndpoint) || !isAllowedExternalUrl(tokenEndpoint)) {
    throw new Error('OIDC discovery document missing valid authorization or token endpoint');
  }

  return {
    authorizationEndpoint,
    tokenEndpoint,
    endSessionEndpoint,
  };
}

export function buildAuthorizeUrl(params: BuildAuthorizeUrlParams): string {
  if (!isAllowedExternalUrl(params.authorizationEndpoint)) {
    throw new Error('Invalid OIDC authorization endpoint');
  }
  const scopes =
    params.scopes && params.scopes.length > 0 ? params.scopes : DEFAULTS.DEFAULT_OIDC_SCOPES;
  const url = new URL(params.authorizationEndpoint);
  url.searchParams.set('response_type', 'code');
  url.searchParams.set('client_id', params.clientId);
  url.searchParams.set('redirect_uri', params.redirectUri);
  url.searchParams.set('scope', scopes.join(' '));
  url.searchParams.set('state', params.state);
  url.searchParams.set('code_challenge', params.codeChallenge);
  url.searchParams.set('code_challenge_method', 'S256');
  return url.toString();
}

export async function exchangeCodeForToken(
  params: ExchangeCodeParams,
  fetchImpl?: typeof fetch,
): Promise<TokenExchangeResult> {
  if (!isAllowedExternalUrl(params.tokenEndpoint)) {
    throw new Error('Invalid OIDC token endpoint');
  }
  const fetcher = fetchImpl ?? globalThis.fetch;
  if (!fetcher) {
    throw new Error('Fetch implementation is unavailable');
  }

  const body = new URLSearchParams();
  body.set('grant_type', 'authorization_code');
  body.set('client_id', params.clientId);
  body.set('code', params.code);
  body.set('redirect_uri', params.redirectUri);
  body.set('code_verifier', params.codeVerifier);

  const response = await fetcher(params.tokenEndpoint, {
    method: 'POST',
    headers: {
      [HTTP_HEADERS.CONTENT_TYPE]: 'application/x-www-form-urlencoded',
      [HTTP_HEADERS.ACCEPT]: HTTP_HEADERS.CONTENT_TYPE_JSON,
    },
    body: body.toString(),
  });

  if (!response.ok) {
    let detail = `Token exchange failed (${response.status})`;
    try {
      const errJson = (await response.json()) as Record<string, unknown>;
      const errDesc =
        typeof errJson.error_description === 'string'
          ? errJson.error_description
          : typeof errJson.error === 'string'
            ? errJson.error
            : '';
      if (errDesc) {
        detail = errDesc;
      }
    } catch {
      // Keep status message when error body is not JSON.
    }
    throw new Error(detail);
  }

  const payload = (await response.json()) as Record<string, unknown>;
  const accessToken = typeof payload.access_token === 'string' ? payload.access_token.trim() : '';
  if (!accessToken) {
    throw new Error('Token response did not include an access_token');
  }
  const idToken = typeof payload.id_token === 'string' && payload.id_token ? payload.id_token : undefined;
  const expiresIn =
    typeof payload.expires_in === 'number' && Number.isFinite(payload.expires_in)
      ? payload.expires_in
      : undefined;

  return {
    accessToken,
    idToken,
    expiresIn,
  };
}

function getStoragePair(): { primary: Storage | null; fallback: Storage | null } {
  if (typeof window === 'undefined') {
    return { primary: null, fallback: null };
  }
  let local: Storage | null = null;
  let session: Storage | null = null;
  try {
    local = window.localStorage ?? null;
  } catch {
    local = null;
  }
  try {
    session = window.sessionStorage ?? null;
  } catch {
    session = null;
  }
  return { primary: local, fallback: session };
}

function setStorageValue(preferSession: boolean, key: string, value: string): void {
  const { primary: local, fallback: session } = getStoragePair();
  const target = preferSession ? (session ?? local) : (local ?? session);
  try {
    target?.setItem(key, value);
  } catch {
    // Ignore storage quota errors.
  }
}

function getStorageValue(preferSession: boolean, key: string): string | null {
  const { primary: local, fallback: session } = getStoragePair();
  const first = preferSession ? session : local;
  const second = preferSession ? local : session;
  try {
    const val = first?.getItem(key);
    if (val !== null && val !== undefined) {
      return val;
    }
  } catch {
    // Try secondary storage.
  }
  try {
    return second?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function removeStorageValue(key: string): void {
  const { primary: local, fallback: session } = getStoragePair();
  try {
    session?.removeItem(key);
  } catch {
    // Ignore errors.
  }
  try {
    local?.removeItem(key);
  } catch {
    // Ignore errors.
  }
}

export function savePkceTransaction(
  verifier: string,
  state: string,
  postLoginRedirect?: string,
  redirectUri?: string,
): void {
  setStorageValue(true, STORAGE_KEYS.PKCE_VERIFIER, verifier);
  setStorageValue(true, STORAGE_KEYS.OIDC_STATE, state);
  if (postLoginRedirect) {
    setStorageValue(true, STORAGE_KEYS.POST_LOGIN_REDIRECT, postLoginRedirect);
  } else {
    removeStorageValue(STORAGE_KEYS.POST_LOGIN_REDIRECT);
  }
  if (redirectUri) {
    setStorageValue(true, STORAGE_KEYS.OIDC_REDIRECT_URI, redirectUri);
  } else {
    removeStorageValue(STORAGE_KEYS.OIDC_REDIRECT_URI);
  }
}

export function loadPkceTransaction(): PkceTransaction {
  return {
    verifier: getStorageValue(true, STORAGE_KEYS.PKCE_VERIFIER),
    state: getStorageValue(true, STORAGE_KEYS.OIDC_STATE),
    postLoginRedirect: getStorageValue(true, STORAGE_KEYS.POST_LOGIN_REDIRECT),
    redirectUri: getStorageValue(true, STORAGE_KEYS.OIDC_REDIRECT_URI),
  };
}

export function clearPkceTransaction(): void {
  removeStorageValue(STORAGE_KEYS.PKCE_VERIFIER);
  removeStorageValue(STORAGE_KEYS.OIDC_STATE);
  removeStorageValue(STORAGE_KEYS.POST_LOGIN_REDIRECT);
  removeStorageValue(STORAGE_KEYS.OIDC_REDIRECT_URI);
}

export function saveAuthSession(session: {
  accessToken: string;
  idToken?: string | null;
  expiresIn?: number | null;
}): StoredAuthSession {
  const expiresAt =
    typeof session.expiresIn === 'number' && Number.isFinite(session.expiresIn) && session.expiresIn > 0
      ? Date.now() + session.expiresIn * 1000
      : null;

  setStorageValue(false, STORAGE_KEYS.ACCESS_TOKEN, session.accessToken);
  if (session.idToken) {
    setStorageValue(false, STORAGE_KEYS.ID_TOKEN, session.idToken);
  } else {
    removeStorageValue(STORAGE_KEYS.ID_TOKEN);
  }
  if (expiresAt !== null) {
    setStorageValue(false, STORAGE_KEYS.TOKEN_EXPIRES_AT, String(expiresAt));
  } else {
    removeStorageValue(STORAGE_KEYS.TOKEN_EXPIRES_AT);
  }

  return {
    accessToken: session.accessToken,
    idToken: session.idToken ?? null,
    expiresAt,
  };
}

export function loadAuthSession(): StoredAuthSession | null {
  const accessToken = getStorageValue(false, STORAGE_KEYS.ACCESS_TOKEN);
  if (!accessToken || !accessToken.trim()) {
    return null;
  }
  const idToken = getStorageValue(false, STORAGE_KEYS.ID_TOKEN);
  const rawExpiresAt = getStorageValue(false, STORAGE_KEYS.TOKEN_EXPIRES_AT);
  let expiresAt: number | null = null;
  if (rawExpiresAt !== null && rawExpiresAt.trim() !== '') {
    const parsed = Number(rawExpiresAt);
    expiresAt = Number.isFinite(parsed) ? parsed : 0;
  }
  return {
    accessToken: accessToken.trim(),
    idToken,
    expiresAt,
  };
}

export function clearAuthSession(): void {
  removeStorageValue(STORAGE_KEYS.ACCESS_TOKEN);
  removeStorageValue(STORAGE_KEYS.ID_TOKEN);
  removeStorageValue(STORAGE_KEYS.TOKEN_EXPIRES_AT);
}

export function isTokenExpired(
  expiresAtMs: number | null | undefined,
  skewSeconds = 30,
): boolean {
  if (expiresAtMs === null || expiresAtMs === undefined) {
    return false;
  }
  if (!Number.isFinite(expiresAtMs) || expiresAtMs <= 0) {
    return true;
  }
  return Date.now() + skewSeconds * 1000 >= expiresAtMs;
}
