// Origins the browser legitimately talks to besides the dashboard itself: the API
// and WebSocket endpoints (same defaults as lib/api.ts and hooks/useWebSocket.ts),
// plus HTTPS/WSS for WalletConnect relays and chain RPC providers.
function configuredOrigins() {
  const urls = [
    process.env.NEXT_PUBLIC_API_URL || 'http://localhost:3000',
    process.env.NEXT_PUBLIC_WS_URL || 'ws://localhost:3000',
  ];
  const origins = [];
  for (const url of urls) {
    try {
      origins.push(new URL(url).origin);
    } catch {
      // Ignore a malformed value; the browser would reject it as well.
    }
  }
  return origins;
}

// Report-only on purpose: wallet connectors (WalletConnect, RainbowKit, injected
// providers) load scripts, frames and sockets from hosts that are hard to enumerate,
// and an enforcing policy could silently break wallet login. Violations are logged
// to the browser console so the policy can be tightened before it is enforced.
// Clickjacking protection is enforced separately via X-Frame-Options.
const contentSecurityPolicyReportOnly = [
  "default-src 'self'",
  "script-src 'self' 'unsafe-inline' 'unsafe-eval'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob: https:",
  "font-src 'self' data:",
  `connect-src ${["'self'", 'https:', 'wss:', ...configuredOrigins()].join(' ')}`,
  "frame-src 'self' https://verify.walletconnect.com https://verify.walletconnect.org",
  "frame-ancestors 'none'",
  "base-uri 'self'",
  "form-action 'self'",
  "object-src 'none'",
].join('; ');

const securityHeaders = [
  { key: 'X-Frame-Options', value: 'DENY' },
  { key: 'X-Content-Type-Options', value: 'nosniff' },
  { key: 'Referrer-Policy', value: 'strict-origin-when-cross-origin' },
  { key: 'Content-Security-Policy-Report-Only', value: contentSecurityPolicyReportOnly },
];

/** @type {import('next').NextConfig} */
const nextConfig = {
  output: 'standalone',
  poweredByHeader: false,
  experimental: {
    serverActions: {
      bodySizeLimit: '2mb',
    },
  },
  async headers() {
    return [{ source: '/:path*', headers: securityHeaders }];
  },
};

module.exports = nextConfig;
