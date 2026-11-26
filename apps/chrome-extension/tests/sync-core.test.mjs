import assert from 'node:assert/strict';
import test from 'node:test';
import {
  ApiHttpError,
  decodeJwtExpMs,
  executeWithTokenRefresh,
  fetchAdminPlatformStatus,
  isTokenExpired,
  maskToken,
  normalizeServerUrl,
  pushPlatformXAuthToken,
  selectBestXCookie,
  testPlatformXCollection,
} from '../lib/sync-core.mjs';

function jsonResponse(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function makeFakeJwt(payloadObj) {
  const b64 = (obj) => Buffer.from(JSON.stringify(obj), 'utf8').toString('base64url');
  return `${b64({ alg: 'RS256', typ: 'JWT' })}.${b64(payloadObj)}.sig`;
}

test('normalizeServerUrl validates http/https and trims trailing slashes', () => {
  assert.deepEqual(normalizeServerUrl('https://digest.example.com///'), {
    baseUrl: 'https://digest.example.com',
    origin: 'https://digest.example.com',
    originPattern: 'https://digest.example.com/*',
  });
  assert.deepEqual(normalizeServerUrl('digest.example.com'), {
    baseUrl: 'https://digest.example.com',
    origin: 'https://digest.example.com',
    originPattern: 'https://digest.example.com/*',
  });
  assert.throws(() => normalizeServerUrl(''));
  assert.throws(() => normalizeServerUrl('ftp://digest.example.com'));
});

test('maskToken redacts middle characters without exposing secrets', () => {
  assert.equal(maskToken('a0a359548dd9288bdfe6ec56640b0c29b3f11cac'), 'a0a3****1cac');
  assert.equal(maskToken('short'), 'sh****');
  assert.equal(maskToken(''), '****');
});

test('selectBestXCookie prefers x.com over twitter.com and newest expiration', () => {
  const picked = selectBestXCookie([
    { name: 'auth_token', value: 'tw-old', domain: '.twitter.com', expirationDate: 9000 },
    { name: 'auth_token', value: 'x-older', domain: '.x.com', expirationDate: 1000 },
    { name: 'auth_token', value: 'x-newest', domain: '.x.com', expirationDate: 5000 },
    { name: 'auth_token', value: '   ', domain: '.x.com', expirationDate: 99999 },
  ]);
  assert.equal(picked?.value, 'x-newest');
  assert.equal(selectBestXCookie([]), null);
});

test('decodeJwtExpMs and isTokenExpired inspect JWT exp accurately', () => {
  const expSec = 2_000_000_000;
  const tok = makeFakeJwt({ sub: 'u1', exp: expSec });
  assert.equal(decodeJwtExpMs(tok), expSec * 1000);
  assert.equal(decodeJwtExpMs('opaque-token'), null);
  assert.equal(isTokenExpired(expSec * 1000, 30, (expSec - 10) * 1000), true);
  assert.equal(isTokenExpired(expSec * 1000, 30, (expSec - 120) * 1000), false);
});

test('fetchAdminPlatformStatus enforces admin role and returns platform settings', async () => {
  const adminFetch = async (url) => {
    if (String(url) === 'https://digest.example.com/api/v1/me') {
      return jsonResponse({ user: { id: 'u1', role: 'admin', email: 'admin@example.com' } });
    }
    if (String(url) === 'https://digest.example.com/api/v1/admin/platform') {
      return jsonResponse({ hasXAuthToken: true, xAuthTokenHint: 'a0a3****1cac', xBridgeConfigured: true });
    }
    return jsonResponse({}, 404);
  };
  const status = await fetchAdminPlatformStatus({
    baseUrl: 'https://digest.example.com',
    accessToken: 'admin-jwt',
    fetchImpl: adminFetch,
  });
  assert.equal(status.user.email, 'admin@example.com');
  assert.equal(status.settings.xAuthTokenHint, 'a0a3****1cac');

  const memberFetch = async () => jsonResponse({ user: { id: 'u2', role: 'member' } });
  await assert.rejects(
    () =>
      fetchAdminPlatformStatus({
        baseUrl: 'https://digest.example.com',
        accessToken: 'member-jwt',
        fetchImpl: memberFetch,
      }),
    (err) => err instanceof ApiHttpError && err.status === 403 && err.code === 'FORBIDDEN_NOT_ADMIN',
  );
});

test('executeWithTokenRefresh silently refreshes expired OIDC session and retries on 401', async () => {
  let tokenGrantCalls = 0;
  const mockFetch = async (url, init) => {
    const u = String(url);
    if (u === 'https://digest.example.com/api/v1/auth/config') {
      return jsonResponse({
        authMode: 'oidc',
        issuerUrl: 'https://auth.example.com/application/o/digest/',
        clientId: 'client-1',
      });
    }
    if (u === 'https://auth.example.com/application/o/digest/.well-known/openid-configuration') {
      return jsonResponse({ token_endpoint: 'https://auth.example.com/token' });
    }
    if (u === 'https://auth.example.com/token') {
      tokenGrantCalls += 1;
      const body = new URLSearchParams(String(init?.body || ''));
      assert.equal(body.get('grant_type'), 'refresh_token');
      assert.equal(body.get('refresh_token'), 'ref-1');
      return jsonResponse({
        access_token: 'fresh-jwt-2',
        refresh_token: 'ref-2',
        expires_in: 3600,
      });
    }
    if (u === 'https://digest.example.com/api/v1/admin/platform' && init?.method === 'PUT') {
      assert.equal(init.headers.authorization, 'Bearer fresh-jwt-2');
      assert.equal(JSON.parse(String(init.body)).xAuthToken, 'cookie-tok-abc');
      return jsonResponse({
        settings: { hasXAuthToken: true, xAuthTokenHint: 'cook****-abc' },
        bridgeReloaded: true,
        bridgeMessage: 'Bridge hot-reloaded with updated X auth_token',
      });
    }
    if (u === 'https://digest.example.com/api/v1/admin/platform/x/test' && init?.method === 'POST') {
      assert.equal(JSON.parse(String(init.body)).handle, 'GoogleDeepMind');
      return jsonResponse({
        ok: true,
        mode: 'rsshub_bridge',
        handle: 'GoogleDeepMind',
        itemsFetched: 5,
        sampleTitles: ['Post 1'],
        latencyMs: 320,
        message: 'Fetched 5 items via rsshub_bridge',
      });
    }
    return jsonResponse({}, 404);
  };

  const pushed = await executeWithTokenRefresh({
    baseUrl: 'https://digest.example.com',
    session: {
      accessToken: 'expired-jwt-1',
      refreshToken: 'ref-1',
      expiresAt: Date.now() - 10_000,
    },
    operation: (token) =>
      pushPlatformXAuthToken({
        baseUrl: 'https://digest.example.com',
        accessToken: token,
        authToken: 'cookie-tok-abc',
        fetchImpl: mockFetch,
      }),
    fetchImpl: mockFetch,
  });

  assert.equal(tokenGrantCalls, 1);
  assert.equal(pushed.refreshed, true);
  assert.equal(pushed.session.accessToken, 'fresh-jwt-2');
  assert.equal(pushed.session.refreshToken, 'ref-2');
  assert.equal(pushed.result.bridgeReloaded, true);

  const tested = await testPlatformXCollection({
    baseUrl: 'https://digest.example.com',
    accessToken: pushed.session.accessToken,
    handle: '@GoogleDeepMind',
    fetchImpl: mockFetch,
  });
  assert.equal(tested.ok, true);
  assert.equal(tested.itemsFetched, 5);
});
