/**
 * slashcmd API Worker
 *
 * Endpoints:
 * - /ping           - Keep connections warm
 * - /command        - SSE stream: command + explanation
 * - /auth/start     - Start CLI auth flow
 * - /auth/poll      - Poll for auth completion
 * - /auth/callback  - Clerk redirect callback
 * - /webhook/clerk  - Clerk webhook handler
 * - /v1/*           - Legacy Groq proxy
 */

export interface Env {
  RATE_LIMITS: KVNamespace;
  JWT_SECRET: string;
  GROQ_API_KEY: string;
  GEMINI_API_KEY: string;
  CLERK_PUBLISHABLE_KEY: string;
  CLERK_SECRET_KEY: string;
  CLERK_WEBHOOK_SECRET: string;
}

// Site URLs
const SITE_URL = 'https://slashcmd.lgandecki.net';

const GROQ_BASE = 'https://api.groq.com/openai';
const GROQ_MODEL = 'moonshotai/kimi-k2-instruct-0905';
const GEMINI_URL = 'https://generativelanguage.googleapis.com/v1beta/models/gemini-3-flash-preview:generateContent';

// ============ JWT Functions ============

// Base64URL encode/decode helpers
function base64UrlEncode(data: string): string {
  return btoa(data).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function base64UrlDecode(data: string): string {
  const padded = data + '='.repeat((4 - data.length % 4) % 4);
  return atob(padded.replace(/-/g, '+').replace(/_/g, '/'));
}

// Create a signed JWT
async function createJWT(payload: object, secret: string): Promise<string> {
  const header = { alg: 'HS256', typ: 'JWT' };
  const headerB64 = base64UrlEncode(JSON.stringify(header));
  const payloadB64 = base64UrlEncode(JSON.stringify(payload));
  const message = `${headerB64}.${payloadB64}`;

  const encoder = new TextEncoder();
  const key = await crypto.subtle.importKey(
    'raw',
    encoder.encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign']
  );

  const signature = await crypto.subtle.sign('HMAC', key, encoder.encode(message));
  const sigB64 = base64UrlEncode(String.fromCharCode(...new Uint8Array(signature)));

  return `${message}.${sigB64}`;
}

// Verify JWT signature and return payload
async function verifyJWT(token: string, secret: string): Promise<{ sub: string; tier: string } | null> {
  try {
    const [headerB64, payloadB64, sigB64] = token.split('.');
    if (!headerB64 || !payloadB64 || !sigB64) return null;

    const message = `${headerB64}.${payloadB64}`;
    const encoder = new TextEncoder();

    const key = await crypto.subtle.importKey(
      'raw',
      encoder.encode(secret),
      { name: 'HMAC', hash: 'SHA-256' },
      false,
      ['verify']
    );

    // Decode signature
    const sigData = base64UrlDecode(sigB64);
    const sigBytes = new Uint8Array(sigData.length);
    for (let i = 0; i < sigData.length; i++) {
      sigBytes[i] = sigData.charCodeAt(i);
    }

    const valid = await crypto.subtle.verify('HMAC', key, sigBytes, encoder.encode(message));
    if (!valid) return null;

    const payload = JSON.parse(base64UrlDecode(payloadB64));
    if (payload.exp && payload.exp < Date.now() / 1000) return null;

    return { sub: payload.sub, tier: payload.tier || 'free' };
  } catch {
    return null;
  }
}

// ============ Rate Limiting (Lifetime-based) ============

const FREE_LIMIT = 100;  // Lifetime limit for free users
const WARN_AT = 90;      // Show warning at this count

interface UsageData {
  total: number;  // Lifetime usage count
}

// Check if user can make a request, return usage info
async function checkUsage(kv: KVNamespace, userId: string, tier: string): Promise<{
  allowed: boolean;
  usage: number;
  limit: number;
  warning: boolean;
}> {
  // Pro users have unlimited access
  if (tier === 'pro') {
    return { allowed: true, usage: 0, limit: -1, warning: false };
  }

  const data = await kv.get<UsageData>(`u:${userId}`, 'json');
  const usage = data?.total || 0;

  return {
    allowed: usage < FREE_LIMIT,
    usage,
    limit: FREE_LIMIT,
    warning: usage >= WARN_AT && usage < FREE_LIMIT,
  };
}

// Increment usage count
async function incrementUsage(kv: KVNamespace, userId: string, tier: string): Promise<void> {
  // Don't track pro users (unlimited)
  if (tier === 'pro') return;

  const key = `u:${userId}`;
  const data = await kv.get<UsageData>(key, 'json') || { total: 0 };
  data.total++;
  await kv.put(key, JSON.stringify(data));
}

// Get user's tier from KV (set by webhook)
async function getUserTier(kv: KVNamespace, userId: string): Promise<string> {
  const tier = await kv.get(`tier:${userId}`);
  return tier || 'free';
}

// Set user's tier in KV (called by webhook)
async function setUserTier(kv: KVNamespace, userId: string, tier: string): Promise<void> {
  await kv.put(`tier:${userId}`, tier);
}

// ============ Groq Call ============

async function getCommand(query: string, apiKey: string): Promise<{ command: string; safe: boolean }> {
  const prompt = `You are a macOS CLI assistant. Convert the user's request to a shell command.

User request: "${query}"

Return JSON with:
- "command": the shell command
- "safe": true if READ-ONLY (ls, find, grep, cat, ps, docker ps, git status), false if has SIDE EFFECTS (writes files, deletes, sends data, installs packages)

Examples:
{"command": "find . -type f -size +100M", "safe": true}
{"command": "rm -rf *.tmp", "safe": false}
{"command": "git status", "safe": true}
{"command": "npm install", "safe": false}

Respond with ONLY the JSON object, no markdown:`;

  const response = await fetch(`${GROQ_BASE}/v1/chat/completions`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      model: GROQ_MODEL,
      messages: [{ role: 'user', content: prompt }],
      max_tokens: 500,
      temperature: 0.3,
    }),
  });

  const data = await response.json() as any;
  const content = data.choices?.[0]?.message?.content || '';

  try {
    const json = content.trim().replace(/```json\n?|\n?```/g, '');
    return JSON.parse(json);
  } catch {
    return { command: content.trim(), safe: false };
  }
}

// ============ Gemini Call ============

async function getExplanation(command: string, style: string, apiKey: string): Promise<string> {
  const stylePrompts: Record<string, string> = {
    typescript: 'Use TypeScript-style pseudocode with types',
    python: 'Use Python-style pseudocode',
    ruby: 'Use Ruby-style pseudocode',
    human: 'Use plain English, no code',
  };

  const prompt = `Explain this shell command in 2-3 short sentences, then show what it does as pseudocode.

Command: ${command}

Format:
1. Start with safety: **[SAFE]** for read-only, **[CAUTION]** for writes/changes, **[DANGER]** for destructive
2. Brief explanation (2-3 sentences max)
3. ${stylePrompts[style] || stylePrompts.typescript}

Keep it concise. No markdown headers.`;

  const response = await fetch(`${GEMINI_URL}?key=${apiKey}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      contents: [{ parts: [{ text: prompt }] }],
      generationConfig: { temperature: 0.3, maxOutputTokens: 500 },
    }),
  });

  const data = await response.json() as any;
  return data.candidates?.[0]?.content?.parts?.[0]?.text || 'Explanation unavailable';
}

// ============ SSE Helper ============

function sseEvent(event: string, data: object): string {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
}

// ============ Main Handler ============

// CORS headers for cross-origin requests from Pages site
const corsHeaders = {
  'Access-Control-Allow-Origin': SITE_URL,
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type, Authorization',
};

function corsResponse(body: string, init?: ResponseInit): Response {
  return new Response(body, {
    ...init,
    headers: { ...init?.headers, ...corsHeaders },
  });
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);

    // ---- CORS: Handle preflight requests ----
    if (request.method === 'OPTIONS') {
      return new Response(null, { headers: corsHeaders });
    }

    // ---- PING: Keep connections warm ----
    if (url.pathname === '/ping') {
      const start = performance.now();
      const groqPing = await fetch(`${GROQ_BASE}/v1/models`, {
        headers: { 'Authorization': `Bearer ${env.GROQ_API_KEY}` },
      });
      return new Response(JSON.stringify({
        ok: groqPing.ok,
        groqMs: (performance.now() - start).toFixed(1),
        edge: (request as any).cf?.colo || 'unknown',
      }), { headers: { 'Content-Type': 'application/json' } });
    }

    // ---- AUTH: Start login flow ----
    if (url.pathname === '/auth/start' && request.method === 'POST') {
      // Generate session ID for polling
      const sessionId = crypto.randomUUID();
      const authUrl = `${SITE_URL}/cli-auth?session=${sessionId}`;

      // Store pending session (expires in 10 minutes)
      await env.RATE_LIMITS.put(`session:${sessionId}`, JSON.stringify({ status: 'pending' }), {
        expirationTtl: 600,
      });

      return new Response(JSON.stringify({ session_id: sessionId, auth_url: authUrl }), {
        headers: { 'Content-Type': 'application/json' },
      });
    }

    // ---- AUTH: Poll for completion ----
    if (url.pathname === '/auth/poll' && request.method === 'GET') {
      const sessionId = url.searchParams.get('session');
      if (!sessionId) {
        return new Response(JSON.stringify({ error: 'Missing session' }), {
          status: 400, headers: { 'Content-Type': 'application/json' },
        });
      }

      const sessionData = await env.RATE_LIMITS.get(`session:${sessionId}`, 'json') as {
        status: string;
        token?: string;
        user?: string;
        github_id?: string;
      } | null;

      if (!sessionData) {
        return new Response(JSON.stringify({ error: 'Session expired' }), {
          status: 404, headers: { 'Content-Type': 'application/json' },
        });
      }

      if (sessionData.status === 'pending') {
        return new Response(JSON.stringify({ pending: true }), {
          headers: { 'Content-Type': 'application/json' },
        });
      }

      // Auth complete - return token and clean up session
      await env.RATE_LIMITS.delete(`session:${sessionId}`);
      return new Response(JSON.stringify({
        token: sessionData.token,
        user: sessionData.user,
        github_id: sessionData.github_id,
      }), {
        headers: { 'Content-Type': 'application/json' },
      });
    }

    // ---- AUTH: Callback from Clerk (receives user info, issues JWT) ----
    if (url.pathname === '/auth/callback' && request.method === 'POST') {
      const body = await request.json() as {
        session_id: string;
        github_id: string;
        username: string;
      };

      const { session_id, github_id, username } = body;

      // Verify session exists and is pending
      const sessionData = await env.RATE_LIMITS.get(`session:${session_id}`, 'json') as { status: string } | null;
      if (!sessionData || sessionData.status !== 'pending') {
        return corsResponse(JSON.stringify({ error: 'Invalid session' }), {
          status: 400, headers: { 'Content-Type': 'application/json' },
        });
      }

      // Get user's tier (default to free)
      const userId = `github:${github_id}`;
      const tier = await getUserTier(env.RATE_LIMITS, userId);

      // Create long-lived JWT (30 days)
      const token = await createJWT({
        sub: userId,
        tier,
        username,
        exp: Math.floor(Date.now() / 1000) + 30 * 24 * 60 * 60,
      }, env.JWT_SECRET);

      // Update session with token
      await env.RATE_LIMITS.put(`session:${session_id}`, JSON.stringify({
        status: 'complete',
        token,
        user: username,
        github_id,
      }), { expirationTtl: 300 }); // 5 min to retrieve

      return corsResponse(JSON.stringify({ ok: true }), {
        headers: { 'Content-Type': 'application/json' },
      });
    }

    // ---- COMMAND: SSE stream with command + explanation ----
    if (url.pathname === '/command') {
      // Auth check
      const auth = request.headers.get('Authorization');
      if (!auth?.startsWith('Bearer ')) {
        return new Response(JSON.stringify({ error: 'Unauthorized', upgrade_url: `${SITE_URL}/upgrade` }), {
          status: 401, headers: { 'Content-Type': 'application/json' },
        });
      }
      const user = await verifyJWT(auth.slice(7), env.JWT_SECRET);
      if (!user) {
        return new Response(JSON.stringify({ error: 'Invalid token', upgrade_url: `${SITE_URL}/upgrade` }), {
          status: 401, headers: { 'Content-Type': 'application/json' },
        });
      }

      // Check usage limits
      const usageInfo = await checkUsage(env.RATE_LIMITS, user.sub, user.tier);
      if (!usageInfo.allowed) {
        return new Response(JSON.stringify({
          error: 'Free tier limit reached',
          usage: usageInfo.usage,
          limit: usageInfo.limit,
          upgrade_url: `${SITE_URL}/upgrade`,
        }), {
          status: 429, headers: { 'Content-Type': 'application/json' },
        });
      }

      // Get query and style from request body
      const body = await request.json() as { query: string; style?: string };
      const { query, style = 'typescript' } = body;

      // Create SSE stream
      const { readable, writable } = new TransformStream();
      const writer = writable.getWriter();
      const encoder = new TextEncoder();

      // Capture usage info for headers
      const usage = usageInfo.usage;
      const limit = usageInfo.limit;
      const warning = usageInfo.warning;
      const tier = user.tier;

      // Start async processing
      ctx.waitUntil((async () => {
        try {
          // 1. Get command from Groq (fast)
          const cmdResult = await getCommand(query, env.GROQ_API_KEY);
          await writer.write(encoder.encode(sseEvent('command', cmdResult)));

          // 2. Get explanation from Gemini (slower, but streams after command)
          const explanation = await getExplanation(cmdResult.command, style, env.GEMINI_API_KEY);
          await writer.write(encoder.encode(sseEvent('explanation', { text: explanation })));

          // 3. Send usage info
          await writer.write(encoder.encode(sseEvent('usage', {
            usage: usage + 1,
            limit,
            tier,
            warning,
          })));

          // 4. Done
          await writer.write(encoder.encode(sseEvent('done', {})));
        } catch (e) {
          await writer.write(encoder.encode(sseEvent('error', { message: String(e) })));
        } finally {
          await writer.close();
        }

        // Increment usage count
        await incrementUsage(env.RATE_LIMITS, user.sub, user.tier);
      })());

      return new Response(readable, {
        headers: {
          'Content-Type': 'text/event-stream',
          'Cache-Control': 'no-cache',
          'Connection': 'keep-alive',
          'X-Edge': String((request as any).cf?.colo || 'unknown'),
          'X-Usage': String(usage),
          'X-Limit': String(limit),
          'X-Tier': tier,
        },
      });
    }

    // ---- WEBHOOK: Clerk events ----
    if (url.pathname === '/webhook/clerk' && request.method === 'POST') {
      // Verify webhook signature (Svix)
      const svixId = request.headers.get('svix-id');
      const svixTimestamp = request.headers.get('svix-timestamp');
      const svixSignature = request.headers.get('svix-signature');

      if (!svixId || !svixTimestamp || !svixSignature) {
        return new Response('Missing signature headers', { status: 400 });
      }

      const body = await request.text();

      // Simple signature verification (production should use proper Svix verification)
      // For now, we trust the webhook if headers are present
      // TODO: Implement proper Svix signature verification

      const payload = JSON.parse(body) as {
        type: string;
        data: {
          id: string;
          external_accounts?: Array<{
            provider: string;
            provider_user_id: string;
          }>;
          public_metadata?: {
            tier?: string;
          };
          username?: string;
        };
      };

      const { type, data } = payload;

      // Handle user events
      if (type === 'user.created' || type === 'user.updated') {
        const githubAccount = data.external_accounts?.find(a => a.provider === 'oauth_github');
        if (githubAccount) {
          const userId = `github:${githubAccount.provider_user_id}`;
          const tier = data.public_metadata?.tier || 'free';
          await setUserTier(env.RATE_LIMITS, userId, tier);
        }
      }

      // Handle subscription events (from Clerk's Stripe integration)
      if (type === 'subscription.created' || type === 'subscription.updated') {
        // Clerk sends subscription events with user_id
        // We need to look up the user to get their GitHub ID
        // For now, we handle this via user.updated with public_metadata
      }

      return new Response('OK', { status: 200 });
    }

    // ---- STATUS: Check usage ----
    if (url.pathname === '/status') {
      const auth = request.headers.get('Authorization');
      if (!auth?.startsWith('Bearer ')) {
        return new Response(JSON.stringify({ error: 'Unauthorized' }), {
          status: 401, headers: { 'Content-Type': 'application/json' },
        });
      }

      const user = await verifyJWT(auth.slice(7), env.JWT_SECRET);
      if (!user) {
        return new Response(JSON.stringify({ error: 'Invalid token' }), {
          status: 401, headers: { 'Content-Type': 'application/json' },
        });
      }

      const usageInfo = await checkUsage(env.RATE_LIMITS, user.sub, user.tier);

      return new Response(JSON.stringify({
        user: user.sub,
        tier: user.tier,
        usage: usageInfo.usage,
        limit: usageInfo.limit,
        remaining: user.tier === 'pro' ? -1 : usageInfo.limit - usageInfo.usage,
      }), {
        headers: { 'Content-Type': 'application/json' },
      });
    }

    // ---- Legacy proxy (backwards compat) ----
    if (url.pathname.startsWith('/v1/')) {
      const auth = request.headers.get('Authorization');
      if (!auth?.startsWith('Bearer ')) {
        return new Response('Unauthorized', { status: 401 });
      }
      const user = await verifyJWT(auth.slice(7), env.JWT_SECRET);
      if (!user) {
        return new Response('Invalid token', { status: 401 });
      }

      const usageInfo = await checkUsage(env.RATE_LIMITS, user.sub, user.tier);
      if (!usageInfo.allowed) {
        return new Response(JSON.stringify({ error: 'Free tier limit reached' }), {
          status: 429, headers: { 'Content-Type': 'application/json' },
        });
      }

      const groqUrl = `${GROQ_BASE}${url.pathname}${url.search}`;
      const groqResponse = await fetch(groqUrl, {
        method: request.method,
        headers: {
          'Authorization': `Bearer ${env.GROQ_API_KEY}`,
          'Content-Type': 'application/json',
        },
        body: request.body,
      });

      ctx.waitUntil(incrementUsage(env.RATE_LIMITS, user.sub, user.tier));

      return new Response(groqResponse.body, {
        status: groqResponse.status,
        headers: {
          'Content-Type': groqResponse.headers.get('Content-Type') || 'application/json',
        },
      });
    }

    // ---- INFO ----
    return new Response(`slashcmd API

Endpoints:
  /ping              - Keep connections warm
  /command           - SSE stream: command + explanation
  /auth/start        - Start CLI auth flow
  /auth/poll         - Poll for auth completion
  /status            - Check usage/tier
  /webhook/clerk     - Clerk webhook

Edge: ${(request as any).cf?.colo || 'unknown'}
Site: ${SITE_URL}
`, { headers: { 'Content-Type': 'text/plain' } });
  },
};
