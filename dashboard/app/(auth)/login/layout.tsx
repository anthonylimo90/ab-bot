import { WalletProvider } from '@/providers/WalletProvider';

/**
 * Wallet sign-in is the dashboard's only wallet feature, so only /login loads
 * the wallet stack (wagmi, RainbowKit, WalletConnect).
 */
export default function LoginLayout({ children }: { children: React.ReactNode }) {
  return <WalletProvider>{children}</WalletProvider>;
}
