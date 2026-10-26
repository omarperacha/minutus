type IconName = "send" | "receive" | "copy" | "refresh" | "chevron" | "back" | "settings" | "lock" | "unlock" | "activity" | "wallet";

const paths: Record<IconName, string> = {
  send: "M6 18 18 6M6 6h12v12",
  receive: "M18 6 6 18M6 6v12h12",
  copy: "M9 9h11v11H9zM15 5V3H3v12h2",
  refresh: "M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1",
  chevron: "m8 10 4 4 4-4",
  back: "m14 6-6 6 6 6M8 12h12",
  settings: "M4 7h16M4 17h16M8 4v6M16 14v6",
  lock: "M5 10h14v11H5zM8 10V6a4 4 0 0 1 8 0v4",
  unlock: "M5 10h14v11H5zM16 10V6a3 3 0 0 1 6 0v2",
  activity: "M4 5h16M4 12h16M4 19h10",
  wallet: "M3 6h16v14H3zM3 6V3h13v3M15 11h6v5h-6z",
};

export function WalletIcon({ name }: { name: IconName }) {
  return <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}

export function TokenIcon({ symbol }: { symbol: string }) {
  return <span className={`wallet-token-icon ${symbol.toLowerCase()}`} aria-hidden="true">{symbol === "ETH" ? <svg viewBox="0 0 24 32" width="20" height="28" fill="currentColor"><path d="m12 1 10 15-10 6L2 16zm0 23 10-6-10 13L2 18z" /></svg> : "$"}</span>;
}
