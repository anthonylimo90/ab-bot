import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { loadTsModule } from './test-helpers.mjs';

const require = createRequire(import.meta.url);
const React = require('react');
const { renderToString } = require('react-dom/server');
// Real wagmi: the point is that its hooks throw outside a provider.
const wagmi = require('wagmi');
const { polygon } = require('wagmi/chains');

const { createElement: h } = React;

function loadWalletButton() {
  const passthrough = ({ children }) => children ?? null;
  const mocks = {
    '@rainbow-me/rainbowkit': {
      ConnectButton: {
        // Enough of RainbowKit's render-prop contract for the logged-out state.
        Custom: ({ children }) => children({ account: undefined, chain: undefined, authenticationStatus: 'unauthenticated', mounted: true, openConnectModal() {} }),
      },
    },
    '@/components/ui/button': { Button: ({ children, disabled }) => h('button', { disabled }, children) },
    'lucide-react': { Wallet: () => h('svg', { 'data-icon': 'wallet' }), Loader2: passthrough },
    '@/hooks/useWalletAuth': { useWalletAuth: () => ({ signIn: async () => false, isLoading: false }) },
  };
  return loadTsModule('components/wallet/WalletButton.tsx', { mocks }).WalletLoginButton;
}

test('wagmi hooks throw WagmiProviderNotFoundError outside a provider (the premise of the defect)', () => {
  function Probe() {
    wagmi.useAccount();
    return null;
  }
  assert.throws(() => renderToString(h(Probe)), (error) => error.name === 'WagmiProviderNotFoundError');
});

test('WalletLoginButton renders an inert placeholder instead of throwing before WagmiProvider mounts', () => {
  const WalletLoginButton = loadWalletButton();
  let html;
  assert.doesNotThrow(() => { html = renderToString(h(WalletLoginButton, { className: 'w-full' })); });
  assert.match(html, /Connect Wallet/);
  assert.match(html, /<button[^>]*disabled/);
});

test('WalletLoginButton renders the live button once WagmiProvider is in the tree', () => {
  const WalletLoginButton = loadWalletButton();
  const config = wagmi.createConfig({
    chains: [polygon],
    transports: { [polygon.id]: wagmi.http('http://127.0.0.1:1') },
    ssr: true,
  });
  const html = renderToString(h(wagmi.WagmiProvider, { config }, h(WalletLoginButton, { className: 'w-full' })));
  assert.match(html, /Connect Wallet/);
  // The live button is enabled (the placeholder is disabled).
  assert.doesNotMatch(html, /<button[^>]*disabled/);
});
